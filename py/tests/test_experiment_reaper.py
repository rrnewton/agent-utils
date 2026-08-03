"""Tests for reap-on-teardown: the pure abandonment planner and unit-name pid parsing.

The impure enumerate/kill path (:func:`reap_orphaned_runs`) is validated live; here we pin the
decision logic that must never false-kill a live run and must always reap a dead-launcher scope."""

from __future__ import annotations

from safe_ci_dag_runner.cgroup import ScopeNaming

from parallel_experiment_runner.reaper import (
    ScopeInfo,
    parse_launcher_pid,
    plan_reaps,
)

_NAMING = ScopeNaming(
    slice_name="parallel-experiment.slice",
    unit_prefix="parallel-experiment",
    env_in_scope="PARALLEL_EXPERIMENT_IN_SCOPE",
    env_scope_unit="PARALLEL_EXPERIMENT_SCOPE_UNIT",
    env_direct_cgroup="PARALLEL_EXPERIMENT_DIRECT_CGROUP",
    log_prefix="[parallel-experiment]",
    supervisor_name="supervisor",
)


def test_parse_launcher_pid() -> None:
    assert parse_launcher_pid("parallel-experiment-12345.scope", _NAMING) == 12345
    # A CI scope (different prefix) is not ours to parse or touch.
    assert parse_launcher_pid("safe-ci-999.scope", _NAMING) is None
    assert parse_launcher_pid("parallel-experiment-.scope", _NAMING) is None
    assert parse_launcher_pid("parallel-experiment-12.service", _NAMING) is None


def test_plan_reaps_reaps_dead_launcher() -> None:
    scopes = [ScopeInfo(unit="parallel-experiment-100.scope", launcher_pid=100, task_count=415)]
    plans = plan_reaps(scopes, self_pid=999, pid_alive=lambda _p: False)
    assert len(plans) == 1
    assert plans[0].unit == "parallel-experiment-100.scope"
    assert plans[0].task_count == 415
    assert "415" in plans[0].reason


def test_plan_reaps_spares_live_launcher() -> None:
    scopes = [ScopeInfo(unit="parallel-experiment-100.scope", launcher_pid=100, task_count=8)]
    # Launcher pid 100 is still alive => a live run owns this scope; never touch it.
    assert plan_reaps(scopes, self_pid=999, pid_alive=lambda p: p == 100) == []


def test_plan_reaps_never_reaps_self() -> None:
    scopes = [ScopeInfo(unit="parallel-experiment-555.scope", launcher_pid=555, task_count=1)]
    # Even if the liveness probe says dead, our own pid is never a reap target (pid-reuse guard).
    assert plan_reaps(scopes, self_pid=555, pid_alive=lambda _p: False) == []


def test_plan_reaps_skips_unparseable_pid() -> None:
    scopes = [ScopeInfo(unit="parallel-experiment-weird.scope", launcher_pid=None, task_count=3)]
    assert plan_reaps(scopes, self_pid=999, pid_alive=lambda _p: False) == []


def test_plan_reaps_reports_unknown_task_count() -> None:
    scopes = [ScopeInfo(unit="parallel-experiment-100.scope", launcher_pid=100, task_count=None)]
    plans = plan_reaps(scopes, self_pid=999, pid_alive=lambda _p: False)
    assert len(plans) == 1
    assert plans[0].task_count is None
    assert "unknown" in plans[0].reason
