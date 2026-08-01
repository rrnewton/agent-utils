"""End-to-end scheduler tests: real subprocess steps verifying observable behavior."""

from __future__ import annotations

import os
import tempfile

from safe_ci_dag_runner.model import DagConfig, ResourceHint, Step
from safe_ci_dag_runner.scheduler import run_dag


def _step(
    group: str,
    job: str,
    cmd: str,
    *,
    deps: list[str] | None = None,
    resources: dict[str, int] | None = None,
    est: float = 0.0,
) -> Step:
    return Step(
        group,
        job,
        "",
        cmd,
        deps=list(deps or []),
        hint=ResourceHint(resources=dict(resources or {}), est_duration_s=est),
    )


def test_simple_dag_all_pass_respects_deps() -> None:
    with tempfile.TemporaryDirectory() as d:
        order = os.path.join(d, "order")
        cfg = DagConfig(
            steps=(
                _step("g", "A", f"echo A >> {order}"),
                _step("g", "B", f"echo B >> {order}", deps=["g.A"]),
            )
        )
        res = run_dag(cfg, jobs=4, verbosity=0)
        assert res.ok
        assert {o.tag for o in res.outcomes} == {"g.A", "g.B"}
        assert all(o.ok for o in res.outcomes)
        with open(order, encoding="utf-8") as fh:
            assert fh.read().split() == ["A", "B"]  # dependent runs strictly after its dep


def test_dep_failure_skips_dependent() -> None:
    cfg = DagConfig(
        steps=(
            _step("g", "A", "exit 1"),
            _step("g", "B", "true", deps=["g.A"]),
        )
    )
    res = run_dag(cfg, jobs=2, verbosity=0)
    assert not res.ok
    outcomes = {o.tag: o for o in res.outcomes}
    assert outcomes["g.A"].ok is False
    assert "g.B" in res.skipped
    assert "g.B" not in outcomes  # a skipped step never produces an outcome


def test_eager_exit_aborts_inflight_step() -> None:
    cfg = DagConfig(
        steps=(
            _step("g", "fast", "sleep 0.2; exit 1", est=0.0),
            _step("g", "slow", "sleep 5", est=100.0),
        )
    )
    res = run_dag(cfg, jobs=2, verbosity=0)
    assert not res.ok
    outcomes = {o.tag: o for o in res.outcomes}
    assert outcomes["g.fast"].ok is False
    assert outcomes["g.slow"].aborted is True  # eager-exit cancelled the in-flight step
    assert outcomes["g.slow"].ok is False


def test_resource_cap_serializes_concurrent_steps() -> None:
    with tempfile.TemporaryDirectory() as d:
        log = os.path.join(d, "intervals")

        def cmd(tag: str) -> str:
            return (
                f"echo S {tag} $(date +%s.%N) >> {log}; "
                f"sleep 0.3; "
                f"echo E {tag} $(date +%s.%N) >> {log}"
            )

        cfg = DagConfig(
            steps=(
                _step("g", "one", cmd("one"), resources={"slot": 1}, est=1.0),
                _step("g", "two", cmd("two"), resources={"slot": 1}, est=1.0),
            ),
            resource_caps={"slot": 1},
        )
        res = run_dag(cfg, jobs=4, verbosity=0)
        assert res.ok
        starts: dict[str, float] = {}
        ends: dict[str, float] = {}
        with open(log, encoding="utf-8") as fh:
            for line in fh:
                kind, tag, ts = line.split()
                (starts if kind == "S" else ends)[tag] = float(ts)
        # A cap of 1 must serialize the two: their run intervals cannot overlap.
        assert ends["one"] <= starts["two"] or ends["two"] <= starts["one"]


def test_fail_fast_stops_launching_queued_independent_steps() -> None:
    # Baseline (default fail-fast): with jobs=1 the highest-est step is dispatched FIRST;
    # when it fails, self.stop halts launching, so the independent later steps never run.
    with tempfile.TemporaryDirectory() as d:
        ran = os.path.join(d, "ran")
        cfg = DagConfig(
            steps=(
                _step("g", "boom", "exit 1", est=100.0),  # LPT: dispatched first
                _step("g", "later1", f"echo 1 >> {ran}", est=1.0),
                _step("g", "later2", f"echo 2 >> {ran}", est=1.0),
            )
        )
        res = run_dag(cfg, jobs=1, verbosity=0)
        assert not res.ok
        # The independent later steps were never launched (no outcome, no marker file).
        assert not os.path.exists(ran)
        assert {o.tag for o in res.outcomes} == {"g.boom"}


def test_run_to_completion_runs_queued_steps_after_failure() -> None:
    # run_to_completion: same DAG, jobs=1, but every INDEPENDENT step still runs to its own
    # outcome after the first failure. Overall result.ok is still False (a step failed).
    with tempfile.TemporaryDirectory() as d:
        ran = os.path.join(d, "ran")
        cfg = DagConfig(
            steps=(
                _step("g", "boom", "exit 1", est=100.0),  # LPT: dispatched first, fails
                _step("g", "later1", f"echo 1 >> {ran}", est=1.0),
                _step("g", "later2", f"echo 2 >> {ran}", est=1.0),
            )
        )
        res = run_dag(cfg, jobs=1, run_to_completion=True, verbosity=0)
        assert not res.ok  # a genuine failure still makes the run fail overall
        outcomes = {o.tag: o for o in res.outcomes}
        assert outcomes["g.boom"].ok is False
        assert outcomes["g.later1"].ok is True  # independent steps ran despite the failure
        assert outcomes["g.later2"].ok is True
        with open(ran, encoding="utf-8") as fh:
            assert sorted(fh.read().split()) == ["1", "2"]


def test_run_to_completion_still_skips_dependents_of_failure() -> None:
    # No-fail-fast must NOT run a failed step's dependents: dependency correctness is preserved.
    with tempfile.TemporaryDirectory() as d:
        ran = os.path.join(d, "ran")
        cfg = DagConfig(
            steps=(
                _step("g", "boom", "exit 1", est=100.0),
                _step("g", "dependent", f"echo dep >> {ran}", deps=["g.boom"], est=1.0),
                _step("g", "independent", f"echo ind >> {ran}", est=1.0),
            )
        )
        res = run_dag(cfg, jobs=1, run_to_completion=True, verbosity=0)
        assert not res.ok
        outcomes = {o.tag: o for o in res.outcomes}
        assert outcomes["g.independent"].ok is True  # independent step ran
        assert "g.dependent" in res.skipped  # dependent of the failure is still skipped
        assert "g.dependent" not in outcomes
        with open(ran, encoding="utf-8") as fh:
            assert fh.read().split() == ["ind"]  # only the independent step wrote


def test_per_step_timeout_is_enforced() -> None:
    # Documents the (pre-existing) per-step timeout: a step that overruns its own Step.timeout
    # is killed and reported as a timeout failure, without waiting for the whole run.
    cfg = DagConfig(
        steps=(
            Step("g", "hang", "", "sleep 30", timeout=1),
            _step("g", "quick", "true"),
        )
    )
    res = run_dag(cfg, jobs=2, run_to_completion=True, verbosity=0)
    assert not res.ok
    outcomes = {o.tag: o for o in res.outcomes}
    assert outcomes["g.hang"].ok is False
    assert "TIMEOUT" in outcomes["g.hang"].reason  # killed at its own Step.timeout
    assert outcomes["g.quick"].ok is True  # the healthy step still passes
