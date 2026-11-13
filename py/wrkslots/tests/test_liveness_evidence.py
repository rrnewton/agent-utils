"""Phase 2a (#1300): pidfd-first liveness evidence and scoped census.

Cost assertions are counts (processes enumerated, /proc listings), never
wall time. Pre-namespace records (the five-key identity) must answer
exactly as before.
"""

from __future__ import annotations

import os
from pathlib import Path

import pytest

from wrkslots import cli


def _identity(pid: int, **overrides: object) -> cli.ProcessIdentity:
    values: dict[str, object] = {
        "pid": pid,
        "start_ticks": 1,
        "boot_id": "boot",
        "host_id": "host",
        "cgroup_path": "/fixture",
    }
    values.update(overrides)
    return cli.ProcessIdentity(**values)  # type: ignore[arg-type]


def test_identity_round_trip_with_dual_namespace_fields() -> None:
    identity = _identity(
        42, namespace_pid=7, pidns_inode="pid:[4026532000]"
    )
    obj = cli._identity_to_obj(identity)
    assert isinstance(obj, dict)
    assert obj["namespace_pid"] == 7
    assert obj["pidns_inode"] == "pid:[4026532000]"
    assert cli._identity_from_obj(obj, "owner") == identity


def test_old_identity_without_dual_fields_parses_and_omits_on_write() -> None:
    old = {
        "pid": 42,
        "start_ticks": 1,
        "boot_id": "boot",
        "host_id": "host",
        "cgroup_path": "/fixture",
    }
    identity = cli._identity_from_obj(old, "owner")
    assert identity is not None
    assert identity.namespace_pid is None
    assert identity.pidns_inode is None
    # Omitting absent fields is what lets old readers keep reading new records.
    assert cli._identity_to_obj(identity) == old


def test_live_owner_answers_via_pidfd_without_listing_proc(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    identity = cli._read_process_identity(os.getpid())
    listings: list[object] = []
    real_listdir = os.listdir

    def counting_listdir(
        path: str | bytes | os.PathLike[str] | os.PathLike[bytes] | int = ".",
        *args: object,
        **kwargs: object,
    ) -> list[str]:
        listings.append(path)
        result = real_listdir(path)
        return [str(item) for item in result]

    monkeypatch.setattr(os, "listdir", counting_listdir)
    state, _detail, source = cli._process_state_with_evidence(identity)
    assert state == "live"
    assert source == "pidfd"
    assert "/proc" not in listings and Path("/proc") not in listings


def test_pidfd_unavailable_falls_back_to_named_legacy_scan(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    identity = cli._read_process_identity(os.getpid())

    def refused(pid: int) -> int:
        raise PermissionError(1, "pidfd unavailable for test")

    monkeypatch.setattr(os, "pidfd_open", refused)
    state, _detail, source = cli._process_state_with_evidence(identity)
    assert state == "live"
    assert source == "legacy-scan"


def test_dead_owner_answers_via_pidfd() -> None:
    # A PID above the kernel maximum cannot be live; pidfd_open proves it.
    from dataclasses import replace

    live = cli._read_process_identity(os.getpid())
    identity = replace(live, pid=4_194_304 + 12345)
    state, _detail, source = cli._process_state_with_evidence(identity)
    assert state == "dead"
    assert source == "pidfd"


def test_no_owner_has_no_evidence_source() -> None:
    state, _detail, source = cli._process_state_with_evidence(None)
    assert state == "indeterminate"
    assert source == "none"


def _fake_cgroup(root: Path, name: str, pids: list[int]) -> Path:
    group = root / name
    group.mkdir(parents=True)
    (group / "cgroup.procs").write_text(
        "".join(f"{pid}\n" for pid in pids), encoding="ascii"
    )
    return group


def test_cgroup_member_pids_counts_only_the_named_cgroup(tmp_path: Path) -> None:
    _fake_cgroup(tmp_path, "team", [os.getpid(), 4_194_304 + 1])
    _fake_cgroup(tmp_path, "other", [4_194_304 + 2, 4_194_304 + 3])
    members = cli._cgroup_member_pids("/team", cgroup_root=tmp_path)
    assert members == (os.getpid(), 4_194_304 + 1)


def test_cgroup_member_pids_unavailable_is_none(tmp_path: Path) -> None:
    assert cli._cgroup_member_pids("/missing", cgroup_root=tmp_path) is None


def test_scoped_census_sees_only_cgroup_members(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    monkeypatch.chdir(slot)
    _fake_cgroup(tmp_path, "team", [os.getpid()])
    record_owner = _identity(os.getpid(), cgroup_path="/team")
    record = object.__new__(cli.ActiveRecord)
    object.__setattr__(record, "owner", record_owner)
    users = cli._scoped_cgroup_slot_users(slot, record, cgroup_root=tmp_path)
    assert users is not None
    assert any(pid == os.getpid() for pid, _kind, _detail in users)


def test_scoped_census_clean_when_member_does_not_use_slot(
    tmp_path: Path,
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    _fake_cgroup(tmp_path, "team", [os.getpid()])
    record_owner = _identity(os.getpid(), cgroup_path="/team")
    record = object.__new__(cli.ActiveRecord)
    object.__setattr__(record, "owner", record_owner)
    users = cli._scoped_cgroup_slot_users(slot, record, cgroup_root=tmp_path)
    assert users == ()
