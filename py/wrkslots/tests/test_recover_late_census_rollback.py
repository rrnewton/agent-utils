"""Late-census rollback for a paired validation finish (#1923).

A validate run that died mid-finish leaves a validation-batch seal and a
finish journal. Recover restores the seal evidence, then runs a late
same-UID census just before deletion. On a busy host that census can
refuse after its eight inventories even though every owner is dead; the
refusal must roll the fence back (checkout back at its canonical path,
finish journal kept at phase=prepared, seal kept byte-for-byte) so a
later retry can finish, instead of leaving the slot fenced behind the
host-wide seal. A `--recovery-note` on such a journal-driven recover is
not a new ownerless cleanup and must be accepted.
"""

from __future__ import annotations

import json
import os
import stat
import subprocess
from collections.abc import Sequence
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots
from wrkslots.tests.test_lifecycle import (
    active_slots,
    prepare_paired_validation_finish,
    stub_validate_batch_censuses,
)

DID_NOT_CLOSE = (
    "same-UID process census did not close: each of 8 process inventories "
    "found generations that forked or exited before their scan completed; "
    "retry when fewer processes are starting"
)


def _recover(project: Path, *extra: str) -> int:
    return wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover",
            "--coordinator-pid",
            str(os.getpid()),
            *extra,
        ]
    )


def _fenced(slot_path: Path, slot: str) -> list[Path]:
    return list(slot_path.parent.glob(f".{slot}.fenced.*"))


def _assert_rolled_back(
    config: wrkslots.Config,
    slot_path: Path,
    finish_path: Path,
    seal_path: Path,
    seal_before: bytes,
    original_mode: int,
) -> None:
    assert slot_path.is_dir()
    assert stat.S_IMODE(slot_path.stat().st_mode) == original_mode
    assert _fenced(slot_path, "target") == []
    assert finish_path.is_file()
    journal = json.loads(finish_path.read_text(encoding="utf-8"))
    assert journal["phase"] == "prepared"
    assert seal_path.is_file()
    assert seal_path.read_bytes() == seal_before


def test_late_census_refusal_in_recover_unfences_and_retry_removes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    project, config, finish_path, raw, record = prepare_paired_validation_finish(
        tmp_path, monkeypatch, "scoped"
    )
    slot_path = wrkslots._slot_directory(config, record.slot, record.slot_type)
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    seal_before = seal_path.read_bytes()
    original_mode = raw["original_mode"]
    assert isinstance(original_mode, int)

    def refuse(
        _paths: Sequence[Path], *, budget: wrkslots._ReadOnlyCommandBudget
    ) -> wrkslots._ProcessPathCensus:
        del budget
        raise wrkslots.Refusal(DID_NOT_CLOSE)

    monkeypatch.setattr(wrkslots, "_capture_same_uid_process_path_census", refuse)

    assert _recover(project, "--slot", "target") == 3
    assert DID_NOT_CLOSE in capsys.readouterr().err
    _assert_rolled_back(
        config, slot_path, finish_path, seal_path, seal_before, original_mode
    )
    assert record.slot in {
        row["slot"] for row in active_slots(project) if isinstance(row, dict)
    }

    # Once the census closes, the retained journal and seal finish.
    stub_validate_batch_censuses(monkeypatch)
    assert _recover(project, "--slot", "target") == 0
    assert not slot_path.exists()
    assert not finish_path.exists()
    assert not seal_path.exists()
    assert record.slot not in {
        row["slot"] for row in active_slots(project) if isinstance(row, dict)
    }


def test_recovery_note_on_journal_recover_is_accepted(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    project, config, finish_path, _raw, record = prepare_paired_validation_finish(
        tmp_path, monkeypatch, "scoped"
    )
    slot_path = wrkslots._slot_directory(config, record.slot, record.slot_type)

    assert (
        _recover(
            project,
            "--slot",
            "target",
            "--recovery-note",
            "seal and finish owners are both dead; finishing the journal",
        )
        == 0
    )
    assert not slot_path.exists()
    assert not finish_path.exists()


def test_real_churn_late_census_refusal_in_recover_unfences(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    real_same_uid = wrkslots._capture_same_uid_process_path_census
    project, config, finish_path, raw, record = prepare_paired_validation_finish(
        tmp_path, monkeypatch, "scoped"
    )
    slot_path = wrkslots._slot_directory(config, record.slot, record.slot_type)
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    seal_before = seal_path.read_bytes()
    original_mode = raw["original_mode"]
    assert isinstance(original_mode, int)
    # Only the earlier all-process census is stubbed; the late same-UID
    # census runs for real against real short-lived churn, so its eight
    # inventories fail exactly as on a busy host.
    monkeypatch.setattr(
        wrkslots, "_capture_same_uid_process_path_census", real_same_uid
    )
    monkeypatch.setattr(
        wrkslots,
        "_capture_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus((), ()),
    )
    monkeypatch.setattr(
        wrkslots,
        "_capture_lsof_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus((), ()),
    )
    churners = [
        subprocess.Popen(["/bin/sh", "-c", "while :; do /usr/bin/true; done"])
        for _ in range(6)
    ]
    try:
        rc = _recover(project, "--slot", "target")
    finally:
        for churner in churners:
            churner.terminate()
        for churner in churners:
            churner.wait()
    assert rc == 3
    assert "same-UID process census did not close" in capsys.readouterr().err
    _assert_rolled_back(
        config, slot_path, finish_path, seal_path, seal_before, original_mode
    )

    stub_validate_batch_censuses(monkeypatch)
    assert _recover(project, "--slot", "target") == 0
    assert not slot_path.exists()
    assert not seal_path.exists()
