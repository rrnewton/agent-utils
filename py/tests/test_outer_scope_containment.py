import errno
from pathlib import Path

import pytest

from safe_ci_dag_runner import cgroup


def test_scope_drain_waits_past_first_empty_sample_for_late_member() -> None:
    observations = iter([["runner"], [], ["late-systemd-helper"], [], []])
    moved: list[str] = []
    sleeps: list[float] = []

    cgroup._drain_scope_root_with(
        lambda: next(observations, []),
        moved.append,
        sleeps.append,
    )

    assert moved == ["runner", "late-systemd-helper"]
    assert sleeps == [cgroup._SCOPE_DRAIN_RETRY_SECONDS] * 4


def test_scope_drain_refuses_a_root_that_never_quiesces() -> None:
    moves = 0

    def refuse_move(_pid: str) -> None:
        nonlocal moves
        moves += 1
        raise PermissionError("planted refusal")

    with pytest.raises(BlockingIOError, match="persistent-member"):
        cgroup._drain_scope_root_with(
            lambda: ["persistent-member"],
            refuse_move,
            lambda _seconds: None,
        )

    assert moves == cgroup._SCOPE_DRAIN_ATTEMPTS


def test_scope_drain_propagates_an_unreadable_roster() -> None:
    moved = False

    def unreadable() -> list[str]:
        raise PermissionError("planted unreadable cgroup.procs")

    def move(_pid: str) -> None:
        nonlocal moved
        moved = True

    with pytest.raises(PermissionError, match="planted unreadable"):
        cgroup._drain_scope_root_with(unreadable, move, lambda _seconds: None)

    assert not moved


def test_controller_enable_redrains_a_member_in_the_check_act_gap() -> None:
    observations = iter([[], [], ["late-between-check-and-write"], [], []])
    moved: list[str] = []
    writes = 0

    def write_controller() -> None:
        nonlocal writes
        writes += 1
        if writes == 1:
            raise OSError(errno.EBUSY, "planted late member")

    cgroup._enable_controller_with(
        lambda: next(observations, []),
        moved.append,
        write_controller,
        lambda _seconds: None,
    )

    assert writes == 2
    assert moved == ["late-between-check-and-write"]


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


def test_max_mem_becomes_the_outer_scope_ceiling_and_can_only_tighten(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """``--max-mem`` is a CONTAINMENT limit, not just a sizing input.

    Before this, ``--max-mem 20G`` sized the schedule and the outer scope still admitted 90% of
    the host, so "two validates with 20 GiB each" held only for as long as the arithmetic did.
    It obeys the same one-way rule as the environment override: tighten, never widen.
    """
    monkeypatch.setattr(cgroup, "mem_available_bytes", lambda: 1_000_000)
    monkeypatch.delenv(cgroup.OUTER_MEMORY_MAX_ENV, raising=False)

    # Tightens: the request is below the derived 90% boundary and becomes the cap.
    assert cgroup.outer_memory_max_bytes(400_000) == 400_000
    # Cannot widen: a request above the derived boundary leaves the boundary in place.
    assert cgroup.outer_memory_max_bytes(5_000_000) == 900_000
    # Absent request is byte-for-byte the previous behaviour.
    assert cgroup.outer_memory_max_bytes(None) == 900_000

    # The SMALLEST of the three wins, whichever it is -- the env override and --max-mem do not
    # override each other, they compose.
    monkeypatch.setenv(cgroup.OUTER_MEMORY_MAX_ENV, "500000")
    assert cgroup.outer_memory_max_bytes(400_000) == 400_000
    assert cgroup.outer_memory_max_bytes(700_000) == 500_000


def test_a_nonpositive_max_mem_request_is_refused_not_ignored(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # Same treatment as a non-positive environment value: the caller asked for a ceiling this
    # cannot express, and silently dropping it would hand back an unbounded run under the name
    # of a bounded one.
    monkeypatch.setattr(cgroup, "mem_available_bytes", lambda: 1_000_000)
    monkeypatch.delenv(cgroup.OUTER_MEMORY_MAX_ENV, raising=False)
    assert cgroup.outer_memory_max_bytes(0) is None
    assert cgroup.outer_memory_max_bytes(-1) is None


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
