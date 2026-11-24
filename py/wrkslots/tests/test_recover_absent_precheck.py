"""Absent-validate-row pre-checks run before the registry locks (#2088).

`recover-absent-validate-rows --apply` used to take every registry
lock, parse the whole event history, and only then refuse at the
Git-registration check. The cheap refusal checks now run from an
event-index-proven ACTIVE snapshot *before* the locks. The test runs
the apply command while the caller holds every registry lock: a
refusable row must return promptly with the same refusal, proving no
lock is taken and no locked path is entered, instead of waiting.
"""

from __future__ import annotations

import hashlib
import json
import pytest

from pathlib import Path

from wrkslots import cli as wrkslots
from wrkslots.tests.test_eden import (
    _absent_args,
    _absent_project,
    _eden_validate_record,
    _write_row,
)
from wrkslots.tests.test_lifecycle import (
    allow_test_host_for_absent_validate_recovery,
    command,
    control_directory,
    create,
    make_project,
    make_validate_row_absent,
    mark_recorded_owner_dead,
    run_absent_validate_recovery,
    write_absent_validate_input,
)


def test_refusable_row_never_takes_the_registry_locks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _upstream = _absent_project(tmp_path)
    config = wrkslots._load_config(str(root), "node-a")
    backing = str(tmp_path / "eden-backing-repos" / "backing")
    record = _eden_validate_record(config, backing)
    _write_row(config, record)
    # The Git-registration/storage check is the seam the field report
    # names; refusable is what matters here, and it must fire pre-lock.
    def refuse(_config: object, _records: object) -> object:
        raise wrkslots.Refusal("Git still registers absent validation checkout")

    monkeypatch.setattr(
        wrkslots, "_assert_absent_validate_rows_storage", refuse
    )
    input_path = root / "absent-validate-rows.json"
    input_path.write_text(
        '{"schema": 1, "rows": [{"machine": "node-a", "slot": '
        f'"{record.slot}", "generation": {record.generation}, '
        f'"record_sha256": '
        f'"{wrkslots._record_sha256(record)}"}}]}}'
    )
    monkeypatch.setattr(wrkslots, "_short_hostname", lambda: "node-a")
    monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", lambda: ())
    monkeypatch.setattr(
        wrkslots, "_absent_validate_privileged_path_match", lambda *_a, **_k: None
    )
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: ())

    created = []

    def fail_if_locked(*args: object, **kwargs: object) -> object:
        created.append((args, kwargs))
        raise AssertionError("the pre-check took the registry locks")

    monkeypatch.setattr(wrkslots, "_mutation_locks", fail_if_locked)
    with pytest.raises(wrkslots.Refusal):
        wrkslots._cmd_recover_absent_validate_rows(
            _absent_args(root, input_path, apply=True)
        )
    assert created == []


def test_precheck_refusal_names_an_already_recovered_row_without_locking(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    """A pre-check refusal must emit the locked path's outcome rows.

    One row is exactly archived already and the other is refused by the
    pre-check (a bad record checksum in the ACTIVE snapshot). The
    refusal happens before the locks, yet the archived row must still
    be named 'already-recovered', nothing may be written, and no
    registry lock may be taken.
    """

    project, repository, _remote = make_project(tmp_path)
    recovered_made = create(
        project, slot="already", agent="validate-a", branch=None, slot_type="validate"
    )
    assert recovered_made.returncode == 0, recovered_made.stderr
    invalid_made = create(
        project, slot="invalid", agent="validate-b", branch=None, slot_type="validate"
    )
    assert invalid_made.returncode == 0, invalid_made.stderr
    recovered = mark_recorded_owner_dead(project, "already")
    invalid = mark_recorded_owner_dead(project, "invalid")
    make_validate_row_absent(project, repository, recovered)
    make_validate_row_absent(project, repository, invalid)
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    recovered_input = write_absent_validate_input(project, [recovered])
    assert run_absent_validate_recovery(project, recovered_input, apply=True) == 0
    capsys.readouterr()

    mixed_input = write_absent_validate_input(project, [recovered, invalid])
    value = json.loads(mixed_input.read_text(encoding="utf-8"))
    value["rows"][1]["record_sha256"] = "0" * 64
    mixed_input.write_text(json.dumps(value), encoding="utf-8")
    control = control_directory(project)
    before = {
        path.relative_to(control): path.read_bytes()
        for path in control.rglob("*")
        if path.is_file()
    }

    created: list[object] = []

    def fail_if_locked(*args: object, **kwargs: object) -> object:
        created.append((args, kwargs))
        raise AssertionError("the pre-check took the registry locks")

    monkeypatch.setattr(wrkslots, "_mutation_locks", fail_if_locked)
    assert (
        run_absent_validate_recovery(
            project, mixed_input, apply=True, output_format="json"
        )
        == 3
    )
    outcomes = json.loads(capsys.readouterr().out)["rows"]
    assert [row["outcome"] for row in outcomes] == [
        "already-recovered",
        "refused",
    ]
    after = {
        path.relative_to(control): path.read_bytes()
        for path in control.rglob("*")
        if path.is_file()
    }
    assert after == before
    assert created == []


def _fail_if_locked(monkeypatch: pytest.MonkeyPatch) -> list[object]:
    created: list[object] = []

    def fail_if_locked(*args: object, **kwargs: object) -> object:
        created.append((args, kwargs))
        raise AssertionError("the pre-check took the registry locks")

    monkeypatch.setattr(wrkslots, "_mutation_locks", fail_if_locked)
    return created


def _eden_input(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> tuple[Path, wrkslots.Config, wrkslots.ActiveRecord, Path]:
    root, _upstream = _absent_project(tmp_path)
    config = wrkslots._load_config(str(root), "node-a")
    backing = str(tmp_path / "eden-backing-repos" / "backing")
    record = _eden_validate_record(config, backing)
    _write_row(config, record)
    input_path = root / "absent-validate-rows.json"
    input_path.write_text(
        '{"schema": 1, "rows": [{"machine": "node-a", "slot": '
        f'"{record.slot}", "generation": {record.generation}, '
        f'"record_sha256": '
        f'"{wrkslots._record_sha256(record)}"}}]}}'
    )
    monkeypatch.setattr(wrkslots, "_short_hostname", lambda: "node-a")
    monkeypatch.setattr(wrkslots, "_absent_validate_process_snapshot", lambda: ())
    monkeypatch.setattr(
        wrkslots, "_absent_validate_privileged_path_match", lambda *_a, **_k: None
    )
    monkeypatch.setattr(wrkslots, "_user_systemd_snapshot", lambda: ())
    return root, config, record, input_path


def test_foreign_journal_refuses_before_the_locks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _config, _record, input_path = _eden_input(tmp_path, monkeypatch)
    journal = root / "worktrees" / "ACTIVE.node-a.journal"
    monkeypatch.setattr(
        wrkslots, "_outstanding_journals", lambda *_a, **_k: [journal]
    )
    monkeypatch.setattr(
        wrkslots, "_load_journal", lambda *_a, **_k: (journal, {"kind": "create"})
    )
    created = _fail_if_locked(monkeypatch)
    with pytest.raises(wrkslots.Refusal, match="another interrupted mutation"):
        wrkslots._cmd_recover_absent_validate_rows(
            _absent_args(root, input_path, apply=True)
        )
    assert created == []


def test_own_resumable_journal_falls_through_to_the_locks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _config, _record, input_path = _eden_input(tmp_path, monkeypatch)
    journal = root / "worktrees" / "ACTIVE.node-a.journal"
    digest = hashlib.sha256(input_path.read_bytes()).hexdigest()
    monkeypatch.setattr(
        wrkslots, "_outstanding_journals", lambda *_a, **_k: [journal]
    )
    monkeypatch.setattr(
        wrkslots,
        "_load_journal",
        lambda *_a, **_k: (
            journal,
            {"kind": "recover-absent-validate-rows", "input_sha256": digest},
        ),
    )
    _fail_if_locked(monkeypatch)
    with pytest.raises(AssertionError, match="took the registry locks"):
        wrkslots._cmd_recover_absent_validate_rows(
            _absent_args(root, input_path, apply=True)
        )


def test_dead_seal_refuses_before_the_locks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root, _config, _record, input_path = _eden_input(tmp_path, monkeypatch)
    seal = root / "worktrees" / "VALIDATE-BATCH-SEAL.node-a.journal"
    monkeypatch.setattr(
        wrkslots, "_validate_batch_seal_journals", lambda *_a, **_k: [seal]
    )
    monkeypatch.setattr(
        wrkslots,
        "_validate_batch_seal_owner_liveness",
        lambda *_a, **_k: ("dead", 4242, None, "owner process is gone"),
    )
    created = _fail_if_locked(monkeypatch)
    with pytest.raises(wrkslots.Refusal, match="interrupted validation-batch seal"):
        wrkslots._cmd_recover_absent_validate_rows(
            _absent_args(root, input_path, apply=True)
        )
    assert created == []


def test_held_row_refuses_before_the_locks(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, repository, _remote = make_project(tmp_path)
    made = create(
        project, slot="held", agent="validate-a", branch=None, slot_type="validate"
    )
    assert made.returncode == 0, made.stderr
    record = mark_recorded_owner_dead(project, "held")
    held = command(project, "hold", "held", "--reason", "retain for review")
    assert held.returncode == 0, held.stderr
    make_validate_row_absent(project, repository, record)
    allow_test_host_for_absent_validate_recovery(project, monkeypatch)
    input_path = write_absent_validate_input(project, [record])
    created = _fail_if_locked(monkeypatch)
    assert (
        run_absent_validate_recovery(
            project, input_path, apply=True, output_format="json"
        )
        == 3
    )
    outcomes = json.loads(capsys.readouterr().out)["rows"]
    assert [row["outcome"] for row in outcomes] == ["refused"]
    assert "is held" in outcomes[0]["detail"]
    assert created == []
