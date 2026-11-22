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
    _kill_grouped_batch_after_prepare,
    active_slots,
    make_project,
    prepare_dead_validate_slots,
    prepare_paired_validation_finish,
    start_ready_holder,
    stub_validate_batch_censuses,
    terminate_process,
)

DID_NOT_CLOSE = (
    "same-UID process census did not close: each of 8 process inventories "
    "found generations that forked or exited before their scan completed; "
    "retry when fewer processes are starting"
)


def _owner_dead(
    monkeypatch: pytest.MonkeyPatch, record: wrkslots.ActiveRecord
) -> None:
    """Prove the fixture's recorded owner dead (it is the test pid)."""

    real = wrkslots._process_state
    owner = record.owner
    assert owner is not None

    def state(identity: wrkslots.ProcessIdentity | None) -> tuple[str, str]:
        if (
            identity is not None
            and identity.pid == owner.pid
            and identity.start_ticks == owner.start_ticks
            and identity.boot_id == owner.boot_id
            and identity.cgroup_path == owner.cgroup_path
        ):
            return "dead", "recorded owner exited"
        return real(identity)

    monkeypatch.setattr(wrkslots, "_process_state", state)


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
    slot: str = "target",
) -> None:
    assert slot_path.is_dir()
    assert stat.S_IMODE(slot_path.stat().st_mode) == original_mode
    assert _fenced(slot_path, slot) == []
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

    # Seal recovery now decides from the batch's own cgroup (#2128):
    # an unreadable cgroup refuses fail-closed, with the same rollback.
    _owner_dead(monkeypatch, record)
    monkeypatch.setattr(
        wrkslots,
        "_scoped_validation_run_cgroup_state",
        lambda _record: (
            "unknown",
            None,
            "recorded owner cgroup cannot be read, "
            "so no scoped no-holder decision is possible",
        ),
    )

    assert _recover(project, "--slot", "target") == 3
    assert "fail closed" in capsys.readouterr().err
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


def _fenced_pair(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> tuple[Path, wrkslots.Config, Path, Path, Path, int]:
    """Two dead validate slots fenced by a grouped batch that then died."""

    project, _repository, _remote = make_project(tmp_path)
    slot_paths = prepare_dead_validate_slots(project, ("slot01", "slot02"))
    stub_validate_batch_censuses(monkeypatch)
    _kill_grouped_batch_after_prepare(project, monkeypatch, ("slot01", "slot02"))
    config = wrkslots._load_config(str(project), "testhost")
    finish_path = wrkslots._finish_journal_path(config, "slot01")
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    journal = json.loads(finish_path.read_text(encoding="utf-8"))
    original_mode = journal["original_mode"]
    assert isinstance(original_mode, int)
    assert next(iter(_fenced(slot_paths["slot01"], "slot01"))).is_dir()
    return project, config, slot_paths["slot01"], finish_path, seal_path, original_mode


def _holder_in(
    fenced: Path, holders: list[subprocess.Popen[str]]
) -> subprocess.Popen[str]:
    return start_ready_holder(
        "import time; print('ready', flush=True); time.sleep(3600)",
        cwd=fenced,
        holders=holders,
    )


def test_early_census_refusal_with_fenced_holder_retains_journal_and_retry_removes(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    # The sibling wedge (#1923, cold review): phase=fenced, a live holder
    # in the fenced path, and the EARLIER census refusing must roll back
    # with the journal retained at phase=prepared, so no stale fenced
    # journal is written back and a retry after the holder dies finishes.
    # Captured before the fixture stubs the censuses: the earlier census
    # runs for real below (the holder guarantees its refusal).
    real_all_process = wrkslots._capture_process_path_census
    project, config, slot_path, finish_path, seal_path, original_mode = _fenced_pair(
        tmp_path, monkeypatch
    )
    seal_before = seal_path.read_bytes()
    fenced = next(iter(_fenced(slot_path, "slot01")))
    holders: list[subprocess.Popen[str]] = []
    holder = _holder_in(fenced, holders)
    stub_validate_batch_censuses(monkeypatch)
    monkeypatch.setattr(wrkslots, "_capture_process_path_census", real_all_process)
    monkeypatch.setattr(
        wrkslots,
        "_capture_lsof_process_path_census",
        lambda _paths, **_kwargs: wrkslots._ProcessPathCensus((), ()),
    )
    try:
        rc = _recover(project, "--slot", "slot01")
    finally:
        terminate_process(holder)
    assert rc == 3
    assert f"live process {holder.pid} uses slot" in capsys.readouterr().err
    _assert_rolled_back(
        config, slot_path, finish_path, seal_path, seal_before, original_mode, slot="slot01"
    )

    stub_validate_batch_censuses(monkeypatch)
    assert _recover(project, "--slot", "slot01") == 0
    assert not slot_path.exists()
    assert not finish_path.exists()
    # slot02 is still sealed and active: the seal outlives slot01's finish.
    assert seal_path.is_file()
    assert "slot02" in {
        row["slot"] for row in active_slots(project) if isinstance(row, dict)
    }


@pytest.mark.load_quarantine  # #2128: scoped refusal replaces the real same-UID census on this path
def test_real_holder_late_census_refusal_in_recover_unfences(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    # A real holder inside the batch cgroup refuses (#2128): the
    # scoped decision reports it as a cgroup member using the slot.
    project, config, slot_path, finish_path, seal_path, original_mode = _fenced_pair(
        tmp_path, monkeypatch
    )
    seal_before = seal_path.read_bytes()
    fenced = next(iter(_fenced(slot_path, "slot01")))
    holders: list[subprocess.Popen[str]] = []
    holder = _holder_in(fenced, holders)
    stub_validate_batch_censuses(monkeypatch)
    _owner_dead(
        monkeypatch,
        wrkslots._find_record(wrkslots._load_active(config), "slot01"),
    )
    monkeypatch.setattr(
        wrkslots,
        "_scoped_validation_run_cgroup_state",
        lambda _record: (
            "nonempty",
            holder.pid,
            "recorded owner cgroup still has living members",
        ),
    )
    try:
        rc = _recover(project, "--slot", "slot01")
    finally:
        terminate_process(holder)
    assert rc == 3
    assert (
        f"live process {holder.pid} remains in the recorded owner cgroup"
        in capsys.readouterr().err
    )
    _assert_rolled_back(
        config, slot_path, finish_path, seal_path, seal_before, original_mode, slot="slot01"
    )

    stub_validate_batch_censuses(monkeypatch)
    assert _recover(project, "--slot", "slot01") == 0
    assert not slot_path.exists()
    assert not finish_path.exists()
