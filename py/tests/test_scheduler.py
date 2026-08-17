"""End-to-end scheduler tests: real subprocess steps verifying observable behavior."""

from __future__ import annotations

import os
import tempfile
import time

from safe_ci_dag_runner.model import (
    DEFAULT_SMALL_CPU_COUNT,
    DEFAULT_SMALL_CPU_TIMEOUT,
    DEFAULT_SMALL_MEM_CAP_BYTES,
    DagConfig,
    IntentionalSkipReason,
    ResourceHint,
    Step,
)
from safe_ci_dag_runner.scheduler import run_dag, steps_violating_run_timeout


def _step(
    group: str,
    job: str,
    cmd: str,
    *,
    deps: list[str] | None = None,
    resources: dict[str, int] | None = None,
    est: float = 0.0,
    skip_reason: IntentionalSkipReason | None = None,
) -> Step:
    return Step(
        group,
        job,
        "",
        cmd,
        deps=list(deps or []),
        hint=ResourceHint(resources=dict(resources or {}), est_duration_s=est),
        skip_reason=skip_reason,
    )


def test_default_config_enables_small_forcing_caps() -> None:
    cfg = DagConfig(steps=())
    assert cfg.default_step_mem_cap_bytes == DEFAULT_SMALL_MEM_CAP_BYTES
    assert cfg.default_step_cpu_count == DEFAULT_SMALL_CPU_COUNT
    assert cfg.default_step_cpu_timeout == DEFAULT_SMALL_CPU_TIMEOUT


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


def test_intentional_skip_never_spawns_and_nonempty_peer_runs() -> None:
    with tempfile.TemporaryDirectory() as d:
        forbidden = os.path.join(d, "forbidden")
        ran = os.path.join(d, "ran")
        cfg = DagConfig(
            steps=(
                _step(
                    "g",
                    "empty",
                    f"touch {forbidden}",
                    skip_reason=IntentionalSkipReason.EMPTY_MANIFEST_BUCKET,
                ),
                _step("g", "nonempty", f"touch {ran}"),
            )
        )
        res = run_dag(cfg, jobs=2, verbosity=0)
        assert res.ok
        assert not os.path.exists(forbidden)
        assert os.path.exists(ran)
        assert res.intentional_skips == (
            ("g.empty", IntentionalSkipReason.EMPTY_MANIFEST_BUCKET),
        )
        assert res.skipped == ()
        assert [outcome.tag for outcome in res.outcomes] == ["g.nonempty"]


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


def test_fail_fast_reports_independent_step_as_not_launched() -> None:
    cfg = DagConfig(
        steps=(
            _step("g", "fail", "exit 1", est=100.0),
            _step("g", "dependent", "true", deps=["g.fail"], est=90.0),
            _step("g", "independent", "true", est=80.0),
        )
    )
    res = run_dag(cfg, jobs=1, keep_going=False, verbosity=0)
    assert not res.ok
    assert [outcome.tag for outcome in res.outcomes] == ["g.fail"]
    assert res.skipped == ("g.dependent",)
    assert res.not_launched == ("g.independent",)


def test_keep_going_launches_independent_step_after_failure() -> None:
    cfg = DagConfig(
        steps=(
            _step("g", "fail", "exit 1", est=100.0),
            _step("g", "dependent", "true", deps=["g.fail"], est=90.0),
            _step("g", "independent", "true", est=80.0),
        )
    )
    res = run_dag(cfg, jobs=1, keep_going=True, verbosity=0)
    assert not res.ok
    outcomes = {outcome.tag: outcome for outcome in res.outcomes}
    assert outcomes["g.fail"].ok is False
    assert outcomes["g.independent"].ok is True
    assert res.skipped == ("g.dependent",)
    assert res.not_launched == ()
    assert len(res.outcomes) + len(res.skipped) + len(res.intentional_skips) == len(cfg.steps)


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


def _timed_step(job: str, cmd: str, timeout: int) -> Step:
    """A step with an explicit wall budget (the INNER bound of the two-level ordering)."""
    return Step("a", job, "", cmd, deps=[], timeout=timeout, cpu_timeout=600)


def test_outer_run_budget_cuts_a_long_run_early_and_still_reports() -> None:
    """A run longer than its outer budget is stopped BY THE RUNNER and still returns rows.

    Per-step budgets cannot bound a run: any number of individually-legal steps can sum past any
    ceiling. Before an outer bound existed the only thing that stopped such a run was an external
    job kill, and an external kill discards the logs that would explain why it was needed. So the
    bound that fires first must be one the scheduler enforces on itself.
    """
    cfg = DagConfig(
        steps=(
            _timed_step("one", "sleep 4", 5),
            _timed_step("two", "sleep 4", 5),
            _timed_step("three", "sleep 4", 5),
        )
    )
    started = time.time()
    res = run_dag(cfg, jobs=1, verbosity=0, run_timeout_s=6)
    elapsed = time.time() - started

    assert res.ok is False
    assert res.run_timed_out is True
    # EARLY is the load-bearing word: ~12s would mean the budget did nothing.
    assert elapsed < 10.0, f"expected a cut near 6s, took {elapsed:.1f}s"
    # The evidence survives, which is the whole difference from an outside kill.
    assert len(res.step_profile_rows) == 2
    outcomes = {o.tag: o for o in res.outcomes}
    assert outcomes["a.one"].ok is True
    assert outcomes["a.two"].aborted is True


def test_misordered_budgets_are_refused_before_anything_runs() -> None:
    """A step allowed to outlive the run is refused: its breach could not be attributed."""
    cfg = DagConfig(steps=(_timed_step("wide", "sleep 1", 600),))
    started = time.time()
    res = run_dag(cfg, jobs=1, verbosity=0, run_timeout_s=6)
    elapsed = time.time() - started

    assert res.ok is False
    assert res.run_timed_out is False  # nothing ran, so nothing timed out
    assert res.outcomes == ()
    assert elapsed < 3.0, f"the refusal must precede execution, took {elapsed:.1f}s"
    assert steps_violating_run_timeout(cfg, 6) == [("a.wide", 600)]


def test_clean_run_inside_its_outer_budget_is_untouched() -> None:
    """The control. A fix that only shows the positive would cut every run short."""
    cfg = DagConfig(
        steps=(
            _timed_step("one", "sleep 1", 10),
            _timed_step("two", "sleep 1", 10),
        )
    )
    res = run_dag(cfg, jobs=1, verbosity=0, run_timeout_s=60)
    assert res.ok is True
    assert res.run_timed_out is False
    assert all(not o.aborted for o in res.outcomes)
    assert len(res.step_profile_rows) == 2
