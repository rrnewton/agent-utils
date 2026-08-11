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


def test_eager_exit_leaves_a_later_step_accounted_for_NOWHERE() -> None:
    """The coverage hole keep_going exists to close, pinned as a property.

    With jobs=1 the longest-processing-time order runs ``first`` (est=100) before ``later``
    (est=0). ``first`` fails, eager-exit sets the stop flag, and ``later`` is then NEVER
    launched -- yet its own deps are fine, so it is not dep-skipped either. It therefore
    appears in NO bucket at all: not in outcomes, not in skipped. That silent gap is exactly
    why a caller reconciling ``selected - accounted`` is the only way to notice it.
    """
    cfg = DagConfig(
        steps=(
            _step("g", "first", "exit 1", est=100.0),
            _step("g", "later", "true", est=0.0),
        )
    )
    res = run_dag(cfg, jobs=1, verbosity=0)
    assert not res.ok
    outcomes = {o.tag: o for o in res.outcomes}
    assert outcomes["g.first"].ok is False
    assert "g.later" not in outcomes  # never ran
    assert "g.later" not in res.skipped  # and NOT dep-skipped -- accounted for nowhere


def test_keep_going_launches_steps_after_a_failure() -> None:
    """REGRESSION: keep_going must keep LAUNCHING, not merely stop reaping in-flight steps.

    Same DAG as the test above. Before this behavior existed, ``keep_going`` only suppressed
    the eager-cancel of already-running steps and left the stop flag set, so ``later`` still
    never launched and this assertion failed.
    """
    cfg = DagConfig(
        steps=(
            _step("g", "first", "exit 1", est=100.0),
            _step("g", "later", "true", est=0.0),
        )
    )
    res = run_dag(cfg, jobs=1, keep_going=True, verbosity=0)
    assert not res.ok  # the run still FAILS; only coverage changes
    outcomes = {o.tag: o for o in res.outcomes}
    assert outcomes["g.first"].ok is False
    assert "g.later" in outcomes, "keep_going must launch steps queued after a failure"
    assert outcomes["g.later"].ok is True
    assert outcomes["g.later"].aborted is False


def test_keep_going_collects_every_independent_failure_in_one_run() -> None:
    """The point of the mode: one run surfaces all independent failures, not just the first."""
    cfg = DagConfig(
        steps=(
            _step("g", "bad1", "exit 1", est=100.0),
            _step("g", "bad2", "exit 1", est=50.0),
            _step("g", "good", "true", est=0.0),
        )
    )
    res = run_dag(cfg, jobs=1, keep_going=True, verbosity=0)
    assert not res.ok
    outcomes = {o.tag: o for o in res.outcomes}
    failed = sorted(t for t, o in outcomes.items() if not o.ok and not o.aborted)
    assert failed == ["g.bad1", "g.bad2"], "both independent failures must be reported"
    assert outcomes["g.good"].ok is True


def test_keep_going_still_skips_steps_whose_deps_failed() -> None:
    """Wider coverage must NOT mean running a step on broken prerequisites.

    keep_going widens what launches, so this guards the boundary: a dependent of a failed step
    is still dep-skipped and still produces no outcome.
    """
    cfg = DagConfig(
        steps=(
            _step("g", "A", "exit 1", est=100.0),
            _step("g", "B", "true", deps=["g.A"]),
            _step("g", "independent", "true", est=0.0),
        )
    )
    res = run_dag(cfg, jobs=1, keep_going=True, verbosity=0)
    assert not res.ok
    outcomes = {o.tag: o for o in res.outcomes}
    assert "g.B" in res.skipped, "a dependent of a failed step must still be skipped"
    assert "g.B" not in outcomes
    assert outcomes["g.independent"].ok is True, "unrelated steps still run"
