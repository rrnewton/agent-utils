"""Event history is read outside `config.lock`, then rechecked (#2125).

`recover-absent-validate-rows` (and `relocate-repository`) used to
replay the whole event history while holding the registry locks, so
a passing call held `config.lock` for minutes and slot heartbeats
waiting on it were refused. The replay and the plan now run with no
lock held; the locks are taken only once the event-index proof
(snapshot revisions and event-log tips) shows the history has not
moved since the read, and the mutation re-derives nothing. A history
that moved in between is re-read (bounded rounds), never mutated
from the stale read.
"""

from __future__ import annotations

import contextlib
import dataclasses
import json
from collections.abc import Iterator
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots
from wrkslots.tests.test_lifecycle import (
    allow_test_host_for_absent_validate_recovery,
    create,
    make_project,
    make_validate_row_absent,
    mark_recorded_owner_dead,
    moved_sibling_repository,
    relocate,
    run_absent_validate_recovery,
    write_absent_validate_input,
)


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def _bump_history(config: wrkslots.Config, slot: str) -> None:
    """One concurrent write: the same row recorded at the next revision."""

    state = wrkslots._load_active_snapshot(config, require_repository=False)
    writer = wrkslots._event_writer(config, config.machine)
    wrkslots._write_active_state(
        config,
        dataclasses.replace(state, revision=state.revision + 1),
        action="test-concurrent-write",
        slot=slot,
        previous=state,
        event_writer=writer,
        require_repository=False,
    )


def _pad_history(config: wrkslots.Config, slot: str, count: int) -> None:
    """Grow the event log with state-preserving revisions of one row."""

    state = wrkslots._load_active_snapshot(config, require_repository=False)
    writer = wrkslots._event_writer(config, config.machine)
    for _ in range(count):
        advanced = dataclasses.replace(state, revision=state.revision + 1)
        wrkslots._write_active_state(
            config,
            advanced,
            action="test-history-pad",
            slot=slot,
            previous=state,
            event_writer=writer,
            require_repository=False,
        )
        state = advanced


def _absent_project(
    tmp_path: Path,
) -> tuple[Path, wrkslots.ActiveRecord]:
    project, repository, _remote = make_project(tmp_path)
    made = create(
        project, slot="gone", agent="validate-a", branch=None, slot_type="validate"
    )
    assert made.returncode == 0, made.stderr
    record = mark_recorded_owner_dead(project, "gone")
    make_validate_row_absent(project, repository, record)
    return project, record


def test_history_moving_between_read_and_lock_is_reread(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A write landing after the plan forces a re-read, not a stale mutation."""

    project, record = _absent_project(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")
    input_path = write_absent_validate_input(project, [record])
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    original_plan = wrkslots._plan_absent_validate_rows
    plans: list[int] = []

    def plan_then_concurrent_write(
        config_arg: wrkslots.Config,
        items: object,
        *,
        prepared: object = None,
    ) -> object:
        planned = original_plan(config_arg, items, prepared=prepared)  # type: ignore[arg-type]
        plans.append(1)
        if len(plans) == 1:
            _bump_history(config, record.slot)
        return planned

    monkeypatch.setattr(
        wrkslots, "_plan_absent_validate_rows", plan_then_concurrent_write
    )
    assert run_absent_validate_recovery(project, input_path, apply=True) == 0
    # The first plan judged a history the concurrent write then moved:
    # the proof under the locks must have refused it and re-read.
    assert len(plans) >= 2
    state = wrkslots._load_active(config)
    assert [row.slot for row in state.slots] == []


def test_passing_row_holds_no_replay_under_the_locks(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """On a padded history the locked section contains no history replay."""

    project, record = _absent_project(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")
    _pad_history(config, record.slot, 400)
    input_path = write_absent_validate_input(project, [record])
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)

    held: list[bool] = []
    replayed_under_lock: list[str] = []
    original_locks = wrkslots._mutation_locks
    original_streamed = wrkslots._validate_global_state_streamed
    original_rows_state = wrkslots._absent_validate_rows_state

    @contextlib.contextmanager
    def tracking_locks(
        config_arg: wrkslots.Config, wait_seconds: float, **kwargs: object
    ) -> Iterator[None]:
        with original_locks(config_arg, wait_seconds, **kwargs):  # type: ignore[arg-type]
            held.append(True)
            try:
                yield
            finally:
                held.pop()

    def tracking_streamed(config_arg: wrkslots.Config, **kwargs: object) -> object:
        if held:
            replayed_under_lock.append("streamed")
        return original_streamed(config_arg, **kwargs)  # type: ignore[arg-type]

    def tracking_rows_state(
        config_arg: wrkslots.Config, items: object, **kwargs: object
    ) -> object:
        if held:
            replayed_under_lock.append("rows-state")
        return original_rows_state(config_arg, items, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(wrkslots, "_mutation_locks", tracking_locks)
    monkeypatch.setattr(wrkslots, "_validate_global_state_streamed", tracking_streamed)
    monkeypatch.setattr(wrkslots, "_absent_validate_rows_state", tracking_rows_state)

    assert (
        run_absent_validate_recovery(
            project, input_path, apply=True, output_format="json"
        )
        == 0
    )
    payload = json.loads(capsys.readouterr().out)
    assert [row["outcome"] for row in payload["rows"]] == ["recovered"]
    assert replayed_under_lock == []
    # The history replay happened before the locks, so the reported
    # hold is the proof comparison and the writes: well under a second
    # where the locked replay of this fixture takes far longer.
    assert payload["lock_seconds"] < 2.0
    state = wrkslots._load_active(config)
    assert [row.slot for row in state.slots] == []


def test_relocate_rereads_a_history_that_moved_before_the_locks(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """The slot move (relocate-repository) re-reads on a moved proof."""

    project, _moved, _remote = moved_sibling_repository(tmp_path, monkeypatch)
    config = wrkslots._load_config(str(project), "testhost")
    capsys.readouterr()

    original_proof = wrkslots._history_tip_proof
    proofs: list[int] = []

    def proof_then_concurrent_write(config_arg: wrkslots.Config) -> object:
        proofs.append(1)
        if len(proofs) == 2:
            # The under-lock recheck of round one: a concurrent write
            # lands first, so the proof it compares has moved.
            _bump_history(config, "slot01")
        return original_proof(config_arg)

    monkeypatch.setattr(wrkslots, "_history_tip_proof", proof_then_concurrent_write)
    assert relocate(project, "../sibling/inner", apply=True) == 0
    assert len(proofs) >= 3
    relocated = [
        item
        for item in wrkslots._load_events(config)
        if item["kind"] == "active-state-recorded"
        and wrkslots._as_mapping(item["payload"], "test payload")["action"]
        == "repository-relocated"
    ]
    assert sorted(
        str(wrkslots._as_mapping(item["payload"], "test payload")["slot"])
        for item in relocated
    ) == ["slot01", "slot02"]
