"""Scoped seal-recovery census (#2128): batch cgroups decide, never /proc.

The decision table runs over a fake cgroup tree. Integration cases
run a paired validation finish recovery under unrelated process
churn, with a live holder inside the batch cgroup, and with the
cgroup present but unreadable.
"""

from __future__ import annotations

import subprocess
import time
from collections.abc import Sequence
from pathlib import Path
from types import SimpleNamespace
from typing import cast

import pytest

from wrkslots import cli as wrkslots
from wrkslots.tests.test_lifecycle import (
    prepare_paired_validation_finish,
    stub_validate_batch_censuses,
    terminate_process,
)

_REAL_SCOPED = wrkslots._scoped_validation_run_cgroup_state
_REAL_SAME_UID = wrkslots._capture_same_uid_process_path_census


def _record(slot: str, cgroup_path: str | None) -> wrkslots.ActiveRecord:
    owner = None
    if cgroup_path is not None:
        owner = wrkslots.ProcessIdentity(
            pid=4242,
            start_ticks=1,
            boot_id="boot",
            host_id="host",
            cgroup_path=cgroup_path,
        )
    record = SimpleNamespace(slot=slot, owner=owner)
    return cast("wrkslots.ActiveRecord", record)


def _tree(tmp_path: Path) -> Path:
    root = tmp_path / "cgroup"
    owner = root / "user.slice" / "owner.scope"
    owner.mkdir(parents=True)
    (owner / "cgroup.events").write_text("populated 0\nfrozen 0\n", encoding="ascii")
    (owner / "cgroup.procs").write_text("", encoding="ascii")
    return root


def _state(root: Path, slot: str = "target") -> tuple[str, int | None, str]:
    return wrkslots._scoped_validation_run_cgroup_state(
        _record(slot, "/user.slice/owner.scope"), cgroup_root=root
    )


def test_scoped_state_empty_owner(tmp_path: Path) -> None:
    root = _tree(tmp_path)
    state, holder, _detail = _state(root)
    assert state == "empty"
    assert holder is None


def test_scoped_state_nonempty_owner_names_holder(tmp_path: Path) -> None:
    root = _tree(tmp_path)
    owner = root / "user.slice" / "owner.scope"
    (owner / "cgroup.events").write_text("populated 1\nfrozen 0\n", encoding="ascii")
    (owner / "cgroup.procs").write_text("6160\n", encoding="ascii")
    state, holder, detail = _state(root)
    assert state == "nonempty"
    assert holder == 6160
    assert "owner.scope" in detail


def test_scoped_state_absent_owner_is_empty(tmp_path: Path) -> None:
    # The service manager removes a dead owner's scope: an absent
    # directory cannot have members.
    root = _tree(tmp_path)
    state, holder, _detail = wrkslots._scoped_validation_run_cgroup_state(
        _record("target", "/user.slice/gone.scope"), cgroup_root=root
    )
    assert state == "empty"
    assert holder is None


def test_scoped_state_unreadable_events_is_unknown(tmp_path: Path) -> None:
    root = _tree(tmp_path)
    (root / "user.slice" / "owner.scope" / "cgroup.events").unlink()
    state, _holder, _detail = _state(root)
    assert state == "unknown"
    (root / "user.slice" / "owner.scope" / "cgroup.events").write_text(
        "populated \n", encoding="ascii"
    )
    state, _holder, _detail = _state(root)
    assert state == "unknown"


def test_scoped_state_no_owner_is_unknown(tmp_path: Path) -> None:
    root = _tree(tmp_path)
    state, _holder, _detail = wrkslots._scoped_validation_run_cgroup_state(
        _record("target", None), cgroup_root=root
    )
    assert state == "unknown"


def test_scoped_state_ignores_unrelated_churn(tmp_path: Path) -> None:
    root = _tree(tmp_path)
    churners = [
        subprocess.Popen(["/bin/sh", "-c", "while :; do /usr/bin/true; done"])
        for _ in range(4)
    ]
    try:
        started = time.monotonic()
        state, holder, _detail = _state(root)
        elapsed = time.monotonic() - started
    finally:
        for churner in churners:
            churner.terminate()
        for churner in churners:
            churner.wait()
    assert state == "empty"
    assert holder is None
    assert elapsed < 10


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


def _recover(project: Path) -> int:
    import os

    return wrkslots.main(
        [
            "--project-root",
            str(project),
            "recover",
            "--coordinator-pid",
            str(os.getpid()),
            "--slot",
            "target",
        ]
    )


def _real_scoped(
    monkeypatch: pytest.MonkeyPatch,
    record: wrkslots.ActiveRecord,
    root: Path,
) -> None:
    """Drive recover with the REAL scoped function on a fake cgroup root."""

    import dataclasses

    assert record.owner is not None
    fake_owner = dataclasses.replace(
        record.owner, cgroup_path="/user.slice/owner.scope"
    )

    def scoped(rec: wrkslots.ActiveRecord) -> tuple[str, int | None, str]:
        return _REAL_SCOPED(
            dataclasses.replace(rec, owner=fake_owner), cgroup_root=root
        )

    monkeypatch.setattr(wrkslots, "_scoped_validation_run_cgroup_state", scoped)


def _unclosed_census(
    _paths: Sequence[Path], *, budget: wrkslots._ReadOnlyCommandBudget
) -> wrkslots._ProcessPathCensus:
    del budget
    raise wrkslots._CensusDidNotClose(
        "same-UID process census did not close", ()
    )


def test_real_scoped_decision_recovers_and_refuses_holder(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """The REAL scoped function (fake cgroup root) drives recover."""

    project, config, finish_path, _raw, record = prepare_paired_validation_finish(
        tmp_path, monkeypatch, "scoped"
    )
    slot_path = wrkslots._slot_directory(config, record.slot, record.slot_type)
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    _owner_dead(monkeypatch, record)
    root = _tree(tmp_path)
    _real_scoped(monkeypatch, record, root)
    monkeypatch.setattr(
        wrkslots, "_capture_same_uid_process_path_census", _unclosed_census
    )
    holder: subprocess.Popen[str] = subprocess.Popen(
        ["sleep", "3600"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        text=True,
    )
    owner_dir = root / "user.slice" / "owner.scope"
    try:
        # A populated shared cgroup does not refuse by itself: its
        # members do not use the slot, so the census decides — and
        # here it cannot close, so recovery refuses without naming
        # the unrelated member as a holder.
        (owner_dir / "cgroup.events").write_text(
            "populated 1\nfrozen 0\n", encoding="ascii"
        )
        (owner_dir / "cgroup.procs").write_text(
            f"{holder.pid}\n", encoding="ascii"
        )
        assert _recover(project) == 3
        assert f"holder pid {holder.pid}" not in capsys.readouterr().err
        assert slot_path.is_dir()
        assert seal_path.is_file()

        # A present but unreadable cgroup refuses fail-closed.
        (owner_dir / "cgroup.events").unlink()
        assert _recover(project) == 3
        assert "fail closed" in capsys.readouterr().err
        assert slot_path.is_dir()
        assert finish_path.is_file()
        assert seal_path.is_file()
    finally:
        terminate_process(holder)

    # Empty again: the non-closing census no longer blocks recovery.
    (owner_dir / "cgroup.events").write_text(
        "populated 0\nfrozen 0\n", encoding="ascii"
    )
    (owner_dir / "cgroup.procs").write_text("", encoding="ascii")
    assert _recover(project) == 0
    assert not finish_path.exists()
    assert not seal_path.exists()


def test_populated_shared_cgroup_members_not_using_slot_recover(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A populated shared owner cgroup: the census decides, and recovers."""

    project, config, finish_path, _raw, record = prepare_paired_validation_finish(
        tmp_path, monkeypatch, "scoped"
    )
    slot_path = wrkslots._slot_directory(config, record.slot, record.slot_type)
    _owner_dead(monkeypatch, record)
    root = _tree(tmp_path)
    _real_scoped(monkeypatch, record, root)
    owner_dir = root / "user.slice" / "owner.scope"
    (owner_dir / "cgroup.events").write_text(
        "populated 1\nfrozen 0\n", encoding="ascii"
    )
    monkeypatch.setattr(
        wrkslots,
        "_capture_same_uid_process_path_census",
        lambda _paths, *, budget: wrkslots._ProcessPathCensus((), ()),
    )
    # Even an indeterminate member check (over its cap, unreadable)
    # lets the census decide rather than refusing.
    monkeypatch.setattr(
        wrkslots, "_scoped_cgroup_slot_users", lambda *_a, **_k: None
    )
    assert _recover(project) == 0
    assert not slot_path.exists()
    assert not finish_path.exists()


def test_census_refusals_other_than_unclosed_still_refuse(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Only an unclosed census that found nothing is substituted."""

    project, config, finish_path, _raw, record = prepare_paired_validation_finish(
        tmp_path, monkeypatch, "scoped"
    )
    slot_path = wrkslots._slot_directory(config, record.slot, record.slot_type)
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    _owner_dead(monkeypatch, record)

    def enumerate_refusal(
        _paths: Sequence[Path], *, budget: wrkslots._ReadOnlyCommandBudget
    ) -> wrkslots._ProcessPathCensus:
        del budget
        raise wrkslots.Refusal("cannot enumerate live processes: [Errno 24]")

    monkeypatch.setattr(
        wrkslots, "_capture_same_uid_process_path_census", enumerate_refusal
    )
    assert _recover(project) == 3
    assert slot_path.is_dir()
    assert seal_path.is_file()

    def unclosed_with_match(
        _paths: Sequence[Path], *, budget: wrkslots._ReadOnlyCommandBudget
    ) -> wrkslots._ProcessPathCensus:
        del budget
        raise wrkslots._CensusDidNotClose(
            "same-UID process census did not close: found live generations",
            ((424242, str(slot_path), "cwd", str(slot_path)),),
        )

    monkeypatch.setattr(
        wrkslots, "_capture_same_uid_process_path_census", unclosed_with_match
    )
    assert _recover(project) == 3
    assert slot_path.is_dir()
    assert finish_path.is_file()
    assert seal_path.is_file()


def test_real_holder_with_real_unclosed_census_refuses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A real holder named by a real non-closing census still refuses."""

    project, config, finish_path, _raw, record = prepare_paired_validation_finish(
        tmp_path, monkeypatch, "scoped"
    )
    slot_path = wrkslots._slot_directory(config, record.slot, record.slot_type)
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    _owner_dead(monkeypatch, record)
    root = _tree(tmp_path)
    _real_scoped(monkeypatch, record, root)
    # Only the same-UID census runs for real; one inventory bound makes
    # its non-closure deterministic while the holder guarantees a match.
    monkeypatch.setattr(wrkslots, "_SAME_UID_CENSUS_INVENTORIES", 1)
    monkeypatch.setattr(
        wrkslots, "_capture_same_uid_process_path_census", _REAL_SAME_UID
    )
    holder: subprocess.Popen[str] = subprocess.Popen(
        ["sleep", "3600"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        text=True,
        cwd=slot_path,
    )
    try:
        rc = _recover(project)
    finally:
        terminate_process(holder)
    assert rc == 3
    assert slot_path.is_dir()
    assert finish_path.is_file()
    assert seal_path.is_file()


def test_real_churn_with_real_census_recovers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Real churn + real non-closing census + real scoped decision."""

    project, config, finish_path, _raw, record = prepare_paired_validation_finish(
        tmp_path, monkeypatch, "scoped"
    )
    slot_path = wrkslots._slot_directory(config, record.slot, record.slot_type)
    seal_path = wrkslots._validate_batch_seal_journal_path(config)
    _owner_dead(monkeypatch, record)
    root = _tree(tmp_path)
    _real_scoped(monkeypatch, record, root)
    monkeypatch.setattr(wrkslots, "_SAME_UID_CENSUS_INVENTORIES", 1)
    swallowed: list[bool] = []

    def census_spy(
        paths: Sequence[Path], *, budget: wrkslots._ReadOnlyCommandBudget
    ) -> wrkslots._ProcessPathCensus:
        try:
            return _REAL_SAME_UID(paths, budget=budget)
        except wrkslots._CensusDidNotClose:
            swallowed.append(True)
            raise

    monkeypatch.setattr(
        wrkslots, "_capture_same_uid_process_path_census", census_spy
    )
    churners = [
        subprocess.Popen(["/bin/sh", "-c", "while :; do /usr/bin/true; done"])
        for _ in range(6)
    ]
    try:
        rc = _recover(project)
    finally:
        for churner in churners:
            churner.terminate()
        for churner in churners:
            churner.wait()
    assert rc == 0
    assert not slot_path.exists()
    assert not finish_path.exists()
    assert not seal_path.exists()
