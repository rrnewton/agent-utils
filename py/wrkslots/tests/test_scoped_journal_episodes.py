"""A scoped journal path carries one operation after another."""

from __future__ import annotations

import json
import os
from collections.abc import Mapping, Sequence
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots
from wrkslots.tests.test_lifecycle import (
    active_slots,
    allow_test_host_for_absent_validate_recovery,
    command,
    create,
    make_project,
    mark_owner_dead,
    raw_command_with_census_authority_stub,
    remove_completed_validation,
    set_liveness,
    run_absent_validate_recovery,
    stub_validate_batch_censuses,
    write_absent_validate_input,
)

CHURN = (
    "same-UID process evidence changed during three liveness attempts; "
    "preserve the slot and rerun remove"
)


def _prepare(tmp_path: Path) -> tuple[Path, wrkslots.Config]:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project, slot="target", slot_type="validate", branch=None)
    assert made.returncode == 0, made.stderr
    mark_owner_dead(project)
    set_liveness(project, "dead")
    return project, wrkslots._load_config(str(project), "testhost")


def _finish_events(config: wrkslots.Config) -> list[tuple[str, str | None, str | None]]:
    path = wrkslots._finish_journal_path(config, "target")
    observed: list[tuple[str, str | None, str | None]] = []
    for event in wrkslots._load_events(config, config.machine):
        payload = event["payload"]
        assert isinstance(payload, dict)
        if payload.get("journal_path") != path.name:
            continue
        journal = payload.get("journal")
        phase = journal.get("phase") if isinstance(journal, dict) else None
        archive = journal.get("archive_id") if isinstance(journal, dict) else None
        assert isinstance(event["kind"], str)
        observed.append((event["kind"], phase, archive))
    return observed


def _event_bytes(config: wrkslots.Config) -> dict[str, bytes]:
    directory = config.control / f"EVENTS.{config.machine}"
    return {path.name: path.read_bytes() for path in sorted(directory.glob("*.json"))}


def _refuse_first_fresh_census(monkeypatch: pytest.MonkeyPatch) -> list[int]:
    stub_validate_batch_censuses(monkeypatch)
    calls: list[int] = []

    def fresh(
        _paths: Sequence[Path], *, budget: wrkslots._ReadOnlyCommandBudget
    ) -> wrkslots._ProcessPathCensus:
        del budget
        calls.append(len(calls))
        if len(calls) == 1:
            raise wrkslots.Refusal(CHURN)
        return wrkslots._ProcessPathCensus((), ())

    monkeypatch.setattr(wrkslots, "_capture_same_uid_process_path_census", fresh)
    monkeypatch.setattr(wrkslots, "_assert_slot_unused", lambda *_a, **_k: None)
    return calls


def _refused_then_rolled_back(
    project: Path,
    config: wrkslots.Config,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> str:
    target = wrkslots._slot_directory(config, "target", "validate")
    calls = _refuse_first_fresh_census(monkeypatch)
    assert remove_completed_validation(project) == 3
    assert CHURN in capsys.readouterr().err
    assert calls == [0]
    first = _finish_events(config)
    assert [(kind, phase) for kind, phase, _archive in first] == [
        ("operation-progress-recorded", "prepared"),
        ("operation-progress-recorded", "fenced"),
        ("operation-progress-recorded", "prepared"),
        ("operation-completed", None),
    ]
    assert len({archive for _kind, _phase, archive in first[:3]}) == 1
    assert target.is_dir()
    assert not tuple(target.parent.glob(".target.fenced.1.*"))
    assert not wrkslots._finish_journal_path(config, "target").exists()
    assert not wrkslots._validate_batch_seal_journal_path(config).exists()
    assert [row["slot"] for row in active_slots(project) if isinstance(row, dict)] == [
        "target"
    ]
    archive = first[0][2]
    assert archive is not None
    return archive


def test_retry_after_refused_private_finish_removes_the_slot(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, config = _prepare(tmp_path)
    first_archive = _refused_then_rolled_back(project, config, monkeypatch, capsys)
    history = _event_bytes(config)

    assert remove_completed_validation(project) == 0, capsys.readouterr().err

    target = wrkslots._slot_directory(config, "target", "validate")
    assert not target.exists()
    assert not tuple(target.parent.glob(".target.fenced.1.*"))
    assert not wrkslots._finish_journal_path(config, "target").exists()
    assert not wrkslots._validate_batch_seal_journal_path(config).exists()
    assert active_slots(project) == []
    after = _event_bytes(config)
    assert {name: after[name] for name in history} == history
    second = _finish_events(config)[4:]
    assert second[0][:2] == ("operation-progress-recorded", "prepared")
    assert second[-1][:2] == ("operation-completed", None)
    assert {archive for _kind, _phase, archive in second[:-1]} - {None} != {first_archive}


def test_retry_interrupted_after_fence_is_recoverable(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, config = _prepare(tmp_path)
    _refused_then_rolled_back(project, config, monkeypatch, capsys)
    target = wrkslots._slot_directory(config, "target", "validate")
    seal = wrkslots._validate_batch_seal_journal_path(config)
    finish = wrkslots._finish_journal_path(config, "target")

    interrupted = raw_command_with_census_authority_stub(
        project,
        "remove",
        "target",
        "--validate-complete",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        env={"WRKSLOTS_TEST_INTERRUPT": "after-validate-batch-fresh-census"},
    )
    assert interrupted.returncode == 86, interrupted.stderr
    # The production shape: canonical path absent, one private fence, the
    # scoped journal fenced, the paired seal present, the ACTIVE row intact.
    assert not target.exists()
    assert len(tuple(target.parent.glob(".target.fenced.1.*"))) == 1
    assert seal.is_file()
    assert finish.is_file()
    assert [row["slot"] for row in active_slots(project) if isinstance(row, dict)] == [
        "target"
    ]
    history = _event_bytes(config)
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    rows = write_absent_validate_input(
        project, list(wrkslots._load_active(config).slots)
    )
    capsys.readouterr()
    planned = run_absent_validate_recovery(project, rows, apply=False)
    # The canonical path is absent only because an outstanding finish fenced
    # it; that finish, not an absent row, is what the plan must report.
    streams = capsys.readouterr()
    assert "reused after completion" not in streams.out + streams.err
    assert planned != 0
    assert "interrupted validation-batch seal" in streams.err
    assert _event_bytes(config) == history

    recovered = raw_command_with_census_authority_stub(
        project,
        "recover",
        "--slot",
        "target",
        "--coordinator-authorized",
        "--coordinator-pid",
        str(os.getpid()),
    )

    assert recovered.returncode == 0, recovered.stderr
    assert not target.exists()
    assert not tuple(target.parent.glob(".target.fenced.1.*"))
    assert not seal.exists()
    assert not finish.exists()
    assert active_slots(project) == []
    after = _event_bytes(config)
    assert {name: after[name] for name in history} == history


def test_create_retried_after_abort_recovers_its_interruption(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    config = wrkslots._load_config(str(project), "testhost")
    journal = wrkslots._create_journal_path(config, "slot01")
    interrupt = {"WRKSLOTS_TEST_INTERRUPT": "after-create-worktree"}
    first = create(project, env=interrupt)
    assert first.returncode == 86, first.stderr
    aborted = command(
        project, "recover", "--coordinator-pid", str(os.getpid()), "--abort-create"
    )
    assert aborted.returncode == 0, aborted.stderr
    assert active_slots(project) == []
    assert not journal.exists()

    retried = create(project, env=interrupt)
    assert retried.returncode == 86, retried.stderr
    assert journal.is_file()

    recovered = command(
        project, "recover", "--coordinator-pid", str(os.getpid()), "--slot", "slot01"
    )

    assert recovered.returncode == 0, recovered.stderr
    assert [row["slot"] for row in active_slots(project) if isinstance(row, dict)] == [
        "slot01"
    ]
    assert not journal.exists()


def _episode_events(
    config: wrkslots.Config,
) -> tuple[list[Mapping[str, object]], dict[str, object]]:
    path = wrkslots._finish_journal_path(config, "target")
    events: list[Mapping[str, object]] = [
        event
        for event in wrkslots._load_events(config, config.machine)
        if isinstance(event["payload"], dict)
        and event["payload"].get("journal_path") == path.name
    ]
    payload = events[0]["payload"]
    assert isinstance(payload, dict)
    journal = payload["journal"]
    assert isinstance(journal, dict)
    return events, {str(key): value for key, value in journal.items()}


def _progress(
    template: Mapping[str, object], journal: Mapping[str, object]
) -> dict[str, object]:
    payload = template["payload"]
    assert isinstance(payload, dict)
    return {**template, "payload": {**payload, "journal": dict(journal)}}


@pytest.mark.parametrize(
    "shape", ("pending", "completed", "stale", "later-phase", "repeated-completion")
)
def test_scoped_finish_episodes_bind_the_latest_operation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    shape: str,
) -> None:
    project, config = _prepare(tmp_path)
    _refused_then_rolled_back(project, config, monkeypatch, capsys)
    history, first = _episode_events(config)
    assert [event["kind"] for event in history] == [
        "operation-progress-recorded",
        "operation-progress-recorded",
        "operation-progress-recorded",
        "operation-completed",
    ]
    fenced = history[1]["payload"]
    assert isinstance(fenced, dict)
    retry = {**first, "archive_id": f"{first['archive_id']}-retry"}
    opened = _progress(history[0], retry)
    events: list[Mapping[str, object]] = [*history, opened]
    raw: Mapping[str, object] = retry
    if shape == "completed":
        events.append(history[3])
    elif shape == "stale":
        # The rolled-back operation's final bytes after a new one has begun.
        stale = history[2]["payload"]
        assert isinstance(stale, dict)
        raw = json.loads(json.dumps(stale["journal"]))
    elif shape == "later-phase":
        events = [*history, history[1]]
        raw = json.loads(json.dumps(fenced["journal"]))
    elif shape == "repeated-completion":
        events = [*history, history[3], opened]
    monkeypatch.setattr(wrkslots, "_load_events", lambda *_a, **_k: list(events))
    path = wrkslots._finish_journal_path(config, "target")

    if shape == "stale":
        with pytest.raises(wrkslots.StateError, match="differs from append-only"):
            wrkslots._scoped_operation_journal_completed(config, path, raw, "finish")
    elif shape == "later-phase":
        with pytest.raises(wrkslots.StateError, match="without a new operation"):
            wrkslots._scoped_operation_journal_completed(config, path, raw, "finish")
    else:
        assert wrkslots._scoped_operation_journal_completed(
            config, path, raw, "finish"
        ) == (shape == "completed")
