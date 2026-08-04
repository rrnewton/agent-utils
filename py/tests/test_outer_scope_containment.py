from pathlib import Path

import pytest

from safe_ci_dag_runner import cgroup


def test_outer_memory_cap_is_derived_and_override_only_tightens(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(cgroup, "mem_available_bytes", lambda: 1_000_000)
    monkeypatch.delenv(cgroup.OUTER_MEMORY_MAX_ENV, raising=False)
    assert cgroup.outer_memory_max_bytes() == 900_000

    monkeypatch.setenv(cgroup.OUTER_MEMORY_MAX_ENV, "2000000")
    assert cgroup.outer_memory_max_bytes() == 900_000

    monkeypatch.setenv(cgroup.OUTER_MEMORY_MAX_ENV, "500000")
    assert cgroup.outer_memory_max_bytes() == 500_000


def test_outer_oom_group_write_is_read_back(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    control = tmp_path / "memory.oom.group"
    control.write_text("0")
    monkeypatch.setattr(cgroup, "scope_cgroup_from_self", lambda naming: tmp_path)

    assert cgroup.enable_outer_oom_group()
    assert control.read_text() == "1"


def test_outer_oom_group_readback_mismatch_is_refused(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    control = tmp_path / "memory.oom.group"
    control.write_text("0")
    monkeypatch.setattr(cgroup, "scope_cgroup_from_self", lambda naming: tmp_path)
    real_read = cgroup._read_cgroup_value

    def stale_read(group: Path, name: str) -> str | None:
        if name == "memory.oom.group":
            return "0"
        return real_read(group, name)

    monkeypatch.setattr(cgroup, "_read_cgroup_value", stale_read)
    assert not cgroup.enable_outer_oom_group()


def test_scope_limit_audit_requires_all_three_memory_controls(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    (tmp_path / "memory.max").write_text("104857600")
    (tmp_path / "memory.swap.max").write_text("0")
    (tmp_path / "memory.oom.group").write_text("1")
    (tmp_path / "cpu.max").write_text("max 100000")
    monkeypatch.setattr(cgroup, "scope_cgroup_from_self", lambda naming: tmp_path)

    assert cgroup.verify_scope_limits(104857600, None)
    (tmp_path / "memory.oom.group").write_text("0")
    assert not cgroup.verify_scope_limits(104857600, None)
