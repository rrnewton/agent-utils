"""Prepared removal must not write ACTIVE from a stale state (#1807).

A single remove, retire-pending and remove-agent-batch all validate and
salvage lock-free, then commit under the mutation lock. If another
slot's heartbeat lands while salvage runs, the prepared whole-registry
state is stale: appending from it forks the event log's revisions and
rolls the other slot's heartbeat back in ACTIVE, after which every
later load refuses. These tests interleave such a heartbeat
deterministically, by hook, with no sleeps.
"""

from __future__ import annotations

import dataclasses
import os
import shutil
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import dead_agent_batch_project


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def _renew_other_slot(config: cli.Config, slot: str) -> str:
    """Append one ttl-renewed event for ``slot`` from the current state."""

    state = cli._load_active(config, require_repository=False)
    record = cli._find_record(state, slot)
    updated = dataclasses.replace(record, heartbeat_at=cli._utc_now())
    cli._write_active_state(
        config,
        cli._replace_record(state, updated),
        action="ttl-renewed",
        slot=slot,
        evidence={"heartbeat_at": updated.heartbeat_at},
        require_repository=False,
    )
    return updated.heartbeat_at


def test_remove_rebases_when_another_slot_heartbeats_during_salvage(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    project, _repository, slots = dead_agent_batch_project(tmp_path, count=2)
    target, other = slots
    renewed: list[str] = []
    original_salvage = cli._removal_salvage

    def interleaved_salvage(
        config: cli.Config, *args: object, **kwargs: object
    ) -> tuple[tuple[dict[str, object], ...], tuple[str, ...]]:
        if not renewed:
            renewed.append(_renew_other_slot(config, other))
        return original_salvage(config, *args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(cli, "_removal_salvage", interleaved_salvage)

    code = cli.main(
        [
            "--project-root",
            str(project),
            "remove",
            target,
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
        ]
    )

    assert code == 0, capsys.readouterr().err
    assert renewed, "the heartbeat never interleaved with salvage"
    config = cli._load_config(str(project), "testhost")
    # A fresh reader replays the whole log: before the fix this raises
    # "append-only active-record event revision does not continue the
    # derived state for slot ..." at the removal's first event.
    derived, _archive = cli._states_from_events(  # type: ignore[misc]
        config, "testhost", require_repository=False
    )
    snapshot = cli._load_active_snapshot(config, "testhost", require_repository=False)
    assert snapshot == derived
    remaining = {record.slot: record for record in derived.slots}
    assert target not in remaining
    assert remaining[other].heartbeat_at == renewed[0]


def test_write_active_state_refuses_a_stale_previous(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, slots = dead_agent_batch_project(tmp_path, count=2)
    target, other = slots
    config = cli._load_config(str(project), "testhost")
    stale = cli._load_active(config, require_repository=False)
    _renew_other_slot(config, other)
    events_before = len(cli._load_events(config, "testhost"))
    active_before = cli._load_active_snapshot(
        config, "testhost", require_repository=False
    )

    record = cli._find_record(stale, target)
    candidate = cli._replace_record(
        stale, dataclasses.replace(record, heartbeat_at=cli._utc_now())
    )
    with pytest.raises(cli.StateError, match="stale previous revision"):
        cli._write_active_state(
            config,
            candidate,
            action="ttl-renewed",
            slot=target,
            evidence={"heartbeat_at": cli._utc_now()},
            require_repository=False,
            previous=stale,
        )

    assert len(cli._load_events(config, "testhost")) == events_before
    assert (
        cli._load_active_snapshot(config, "testhost", require_repository=False)
        == active_before
    )


def test_write_active_state_refuses_a_stale_previous_without_an_event_log(
    tmp_path: Path,
) -> None:
    """The defence's empty-log branch judges ``previous`` by the snapshot."""

    project, _repository, slots = dead_agent_batch_project(tmp_path, count=1)
    config = cli._load_config(str(project), "testhost")
    snapshot = cli._load_active_snapshot(config, "testhost", require_repository=False)
    shutil.rmtree(cli._event_directory(config, "testhost"))
    stale = dataclasses.replace(snapshot, revision=snapshot.revision - 1)
    record = cli._find_record(snapshot, slots[0])
    writer = cli._EventWriter(
        directory=cli._event_directory(config, "testhost"),
        machine="testhost",
        sequence=1,
        previous_sha256="0" * 64,
    )
    with pytest.raises(cli.StateError, match="stale previous revision"):
        cli._write_active_state(
            config,
            cli._replace_record(
                snapshot, dataclasses.replace(record, heartbeat_at=cli._utc_now())
            ),
            action="ttl-renewed",
            slot=slots[0],
            require_repository=False,
            event_writer=writer,
            previous=stale,
        )
    assert not cli._event_directory(config, "testhost").exists()
