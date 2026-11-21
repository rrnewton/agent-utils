"""transfer-owner validates lock-free and holds the lock only to write (#1975).

`transfer-owner` used to run the whole-fleet validation twice plus the
registry storage scan and the event load while it held the exclusive
configuration lock, so re-attaching lanes after a reboot queued behind
each transfer for minutes. These tests pin where that work runs now,
by counting instrumented operations while the exclusive lock is held,
never by wall time: the fleet-wide work runs before the lock, and
under it run only the registry identity check, the touched record's
own checks, the write, and a post-write check that reads only the
appended events. A writer that commits during the lock-free validation
sends the command back to validate; a tip that changed into a state
the transfer must refuse is refused by that revalidation.
"""

from __future__ import annotations

import contextlib
import dataclasses
import fcntl
import os
import sys
from collections.abc import Callable, Iterator
from pathlib import Path
from typing import ParamSpec, TypeVar

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import (
    active_slots,
    command,
    control_directory,
    create,
    make_project,
    mark_owner_dead,
)

_Parameters = ParamSpec("_Parameters")
_Result = TypeVar("_Result")

_INSTRUMENTED = {
    "_validate_global_state": "validate",
    "_registry_storage_inconsistencies": "storage-scan",
    "_load_events": "log-load",
    "_event_file_names": "log-list",
}


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")
    monkeypatch.setenv("WRKSLOTS_LOCK_ORDER_CHECK", "1")


def _flock_held(path: Path, flags: int) -> bool:
    fd = os.open(path, flags | os.O_CLOEXEC)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return True
        fcntl.flock(fd, fcntl.LOCK_UN)
        return False
    finally:
        os.close(fd)


def _exclusive_lock_held(project: Path) -> bool:
    """The project-root flock, or the ACTIVE write lock a transfer now
    commits under without the configuration locks (#2000)."""
    registry = control_directory(project) / "ACTIVE.global.lock"
    return _flock_held(project, os.O_RDONLY | os.O_DIRECTORY) or (
        registry.exists() and _flock_held(registry, os.O_RDONLY)
    )


class _LockLedger:
    def __init__(self, monkeypatch: pytest.MonkeyPatch, project: Path) -> None:
        self.project = project
        self.calls: list[tuple[str, bool]] = []
        self.exclusive_rounds = 0
        self.during_validation: Callable[[], None] | None = None
        for attribute, label in _INSTRUMENTED.items():
            monkeypatch.setattr(cli, attribute, self._recorded(label, getattr(cli, attribute)))
        locks = cli._mutation_locks

        @contextlib.contextmanager
        def counted_locks(
            config: cli.Config,
            wait_seconds: float,
            *,
            deadline: float | None = None,
            shared: bool = False,
            release_config_after_check: bool = False,
        ) -> Iterator[None]:
            with locks(
                config,
                wait_seconds,
                deadline=deadline,
                shared=shared,
                release_config_after_check=release_config_after_check,
            ):
                if not shared:
                    self.exclusive_rounds += 1
                yield

        monkeypatch.setattr(cli, "_mutation_locks", counted_locks)

    def _recorded(
        self, label: str, function: Callable[_Parameters, _Result]
    ) -> Callable[_Parameters, _Result]:
        def wrapper(*args: _Parameters.args, **kwargs: _Parameters.kwargs) -> _Result:
            held = _exclusive_lock_held(self.project)
            self.calls.append((label, held))
            if label == "validate" and not held and self.during_validation is not None:
                self.during_validation()
            return function(*args, **kwargs)

        return wrapper

    def count(self, label: str, *, held: bool) -> int:
        return sum(1 for name, was_held in self.calls if name == label and was_held == held)


def _project_with_slots(tmp_path: Path, count: int = 3) -> Path:
    project, _repository, _remote = make_project(tmp_path)
    for index in range(1, count + 1):
        made = create(
            project,
            slot=f"slot{index:02d}",
            agent=f"codex-{index}",
            branch=f"codex/task{index}",
        )
        assert made.returncode == 0, made.stderr
    mark_owner_dead(project, slot="slot01")
    return project


def _transfer_argv(project: Path) -> list[str]:
    return [
        "--project-root", str(project), "transfer-owner", "slot01",
        "--agent", "codex-1", "--owner-pid", str(os.getpid()),
        "--expected-generation", "1",
    ]


def _event_count(project: Path) -> int:
    return len(list((control_directory(project) / "EVENTS.testhost").glob("*.json")))


def _row(project: Path, slot: str) -> dict[str, object]:
    for row in active_slots(project):
        assert isinstance(row, dict)
        if row["slot"] == slot:
            return row
    raise AssertionError(f"no row for {slot}")


def test_transfer_owner_runs_no_fleet_validation_while_holding_the_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project = _project_with_slots(tmp_path)
    events_before = _event_count(project)
    ledger = _LockLedger(monkeypatch, project)

    assert cli.main(_transfer_argv(project)) == 0, capsys.readouterr().err

    assert _event_count(project) - events_before == 1
    row = _row(project, "slot01")
    assert isinstance(row["owner"], dict) and row["owner"]["pid"] == os.getpid()
    # The fleet-wide work ran before the lock, never under it.
    assert ledger.count("validate", held=False) >= 1
    assert ledger.count("validate", held=True) == 0
    assert ledger.count("storage-scan", held=True) == 0
    # Nothing under the lock loads or lists the event log.
    assert ledger.count("log-load", held=True) == 0
    assert ledger.count("log-list", held=True) == 0
    assert ledger.exclusive_rounds == 1


def test_transfer_owner_at_many_rows_still_counts_zero_fleet_work_under_the_lock(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project = _project_with_slots(tmp_path, count=12)
    ledger = _LockLedger(monkeypatch, project)

    assert cli.main(_transfer_argv(project)) == 0, capsys.readouterr().err

    assert ledger.count("validate", held=True) == 0
    assert ledger.count("storage-scan", held=True) == 0
    assert ledger.count("log-load", held=True) == 0
    assert ledger.count("log-list", held=True) == 0


def test_transfer_owner_revalidates_when_a_writer_commits_during_validation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project = _project_with_slots(tmp_path)
    ledger = _LockLedger(monkeypatch, project)
    writes: list[int] = []

    def concurrent_hold() -> None:
        if writes:
            return
        completed = command(project, "hold", "slot02", "--reason", "concurrent writer")
        assert completed.returncode == 0, completed.stderr
        writes.append(completed.returncode)

    ledger.during_validation = concurrent_hold

    assert cli.main(_transfer_argv(project)) == 0, capsys.readouterr().err

    assert writes == [0]
    assert ledger.count("validate", held=False) == 2
    assert ledger.count("validate", held=True) == 0


def test_transfer_owner_refuses_when_the_tip_changes_to_a_stale_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """A generation bump landing in the prepare window still refuses."""
    project = _project_with_slots(tmp_path)
    ledger = _LockLedger(monkeypatch, project)
    writes: list[int] = []

    def bump_generation() -> None:
        if writes:
            return
        config = cli._load_config(str(project), "testhost")
        state = cli._load_active(config)
        record = cli._find_record(state, "slot01")
        cli._write_active_state(
            config,
            cli._replace_record(
                state, dataclasses.replace(record, generation=2)
            ),
            action="test-tip-changed",
            slot="slot01",
        )
        writes.append(0)

    ledger.during_validation = bump_generation

    rc = cli.main(_transfer_argv(project))

    captured = capsys.readouterr()
    assert rc == 3
    assert "stale generation for slot slot01" in captured.err
    assert writes == [0]
    row = _row(project, "slot01")
    assert row["generation"] == 2
    assert isinstance(row["owner"], dict) and row["owner"]["boot_id"] == "finished-boot"


def _hold_toggle(project: Path, writes: list[int]) -> Callable[[], None]:
    def write() -> None:
        if len(writes) % 2 == 0:
            completed = command(project, "hold", "slot02", "--reason", "concurrent writer")
        else:
            completed = command(project, "unhold", "slot02")
        assert completed.returncode == 0, completed.stderr
        writes.append(completed.returncode)

    return write


def test_transfer_owner_falls_back_to_locked_body_when_every_round_is_overtaken(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """Busy rounds must not refuse in a reboot storm: wait and transfer under the lock."""
    project = _project_with_slots(tmp_path)
    monkeypatch.setattr(cli, "REGISTRY_VALIDATION_ATTEMPTS", 3)
    ledger = _LockLedger(monkeypatch, project)
    writes: list[int] = []
    ledger.during_validation = _hold_toggle(project, writes)
    events_before = _event_count(project)

    rc = cli.main(_transfer_argv(project))

    captured = capsys.readouterr()
    assert rc == 0, captured.err
    assert "registry is busy" not in captured.err
    assert "transferred slot=slot01" in captured.out
    assert len(writes) == 3
    # Three overtaken lock-free rounds plus the locked fallback's own event.
    assert _event_count(project) - events_before == 4
    row = _row(project, "slot01")
    assert isinstance(row["owner"], dict) and row["owner"]["pid"] == os.getpid()
    # The fallback validated under the lock, as the pre-#1975 body did.
    assert ledger.count("validate", held=True) >= 1


def test_transfer_owner_with_an_outstanding_journal_takes_the_locked_body(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """An outstanding journal defers to the locked body; its bytes are untouched."""
    project = _project_with_slots(tmp_path)
    interrupted = create(
        project,
        slot="slot09",
        agent="codex-9",
        branch="codex/task9",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    journals = list(control_directory(project).glob("CREATE.*.journal"))
    assert len(journals) == 1
    journal_bytes = journals[0].read_bytes()
    events_before = _event_count(project)
    ledger = _LockLedger(monkeypatch, project)

    rc = cli.main(_transfer_argv(project))

    captured = capsys.readouterr()
    assert rc == 3, captured.err
    assert "registry is busy" not in captured.err
    assert journals[0].read_bytes() == journal_bytes
    assert _event_count(project) == events_before
    row = _row(project, "slot01")
    assert isinstance(row["owner"], dict) and row["owner"]["boot_id"] == "finished-boot"
    # The locked body was entered under the mutation locks (it refuses on
    # the journal before any fleet validation runs there).
    assert ledger.exclusive_rounds >= 1


def test_transfer_owner_keeps_a_dead_mount_note_from_an_overtaken_round(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """A dead-mount note emitted in round 1 still prints once when round 2 succeeds."""
    project = _project_with_slots(tmp_path)
    ledger = _LockLedger(monkeypatch, project)
    writes: list[int] = []
    toggle = _hold_toggle(project, writes)

    def churn_once() -> None:
        if not writes:
            toggle()

    ledger.during_validation = churn_once
    point = project / "dead-entry"

    def report_dead(config: object, states: object, record: object) -> None:
        if point in cli._DEAD_MOUNT_REPORTED.get():
            return
        cli._DEAD_MOUNT_REPORTED.set((*cli._DEAD_MOUNT_REPORTED.get(), point))
        print(
            f"WARNING: dead FUSE entry at {point}, owning slot slot02",
            file=sys.stderr,
        )

    monkeypatch.setattr(cli, "_report_dead_for_record", report_dead)

    rc = cli.main(_transfer_argv(project))

    captured = capsys.readouterr()
    assert rc == 0, captured.err
    assert writes == [0]
    assert captured.err.count("dead FUSE entry at") == 1
    assert "transferred slot=slot01" in captured.out
