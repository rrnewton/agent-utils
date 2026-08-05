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


def test_cpuset_verification_requires_the_exact_requested_ids(tmp_path: Path) -> None:
    (tmp_path / "cgroup.controllers").write_text("cpu memory cpuset")
    (tmp_path / "cpuset.cpus").write_text("")
    (tmp_path / "cpuset.cpus.effective").write_text("2-3")
    (tmp_path / "cgroup.subtree_control").write_text("")

    assert not cgroup._try_cgroup_cpuset(tmp_path, [0, 1])
    assert cgroup._try_cgroup_cpuset(tmp_path, [2, 3])


def test_specific_cores_never_mutates_an_unowned_ambient_scope(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    naming = cgroup.DEFAULT_NAMING
    monkeypatch.delenv(naming.env_in_scope, raising=False)
    monkeypatch.delenv(naming.env_direct_cgroup, raising=False)
    monkeypatch.setattr(cgroup, "scope_cgroup_from_self", lambda _naming: tmp_path)

    assert cgroup.apply_specific_cores([0], naming) is None
    assert not (tmp_path / "cpuset.cpus").exists()
