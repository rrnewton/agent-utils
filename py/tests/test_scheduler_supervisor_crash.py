"""No Silent Hang: an exception on the step path must FAIL LOUDLY, never wedge the scheduler.

Regression suite for the defect where an unexpected exception raised inside a step supervisor
thread was swallowed with the tag still marked running, so the ready-set loop spun forever: no
outcome, no traceback, no exit — until an outer timeout killed the whole lane (observed: exit
124 at 60s with ZERO output).

Every test here is bracketed in both directions, because a refusal-only test cannot distinguish
a working guard from an inert one:

* POSITIVE — a planted exception at a real call site produces a ``SUPERVISOR CRASH`` outcome
  and a failed run (the guard FIRES).
* NEGATIVE — the identical DAG with nothing planted produces no such outcome (the assertion can
  actually tell the two apart, so a green here is not vacuous).

Exceptions are planted through the ``CgroupManager`` protocol wherever possible, so the
exception is raised at a genuine call site on the real step path rather than by monkeypatching
the module under test. The three plant points cover the three structurally distinct windows:

* BEFORE the child is spawned — the tag never leaves ``running`` and its resources are never
  released, so both the loop AND the resource accounting wedge.
* AFTER the child exits, before bookkeeping — a real process ran and was measured.
* INSIDE the locked bookkeeping block — the discovering case (a keyword argument
  ``StepOutcome.failed`` does not accept), where ``running`` is already cleared but ``done`` is
  never written.
"""

from __future__ import annotations

import threading
from collections.abc import Mapping

import pytest

from safe_ci_dag_runner import scheduler as sched
from safe_ci_dag_runner.model import DagConfig, ResourceHint, Step
from safe_ci_dag_runner.protocols import RunResult, StepOutcome
from safe_ci_dag_runner.scheduler import SUPERVISOR_CRASH_REASON, run_dag

#: Every test asserts the run finishes inside this budget. The defect's signature is an
#: UNBOUNDED wait, so any finite bound discriminates; this one is ~100x the clean runtime of
#: these DAGs (measured: the whole unmodified scheduler suite runs in 1.2s).
_DEADLINE_S = 30.0


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


def _run_bounded(cfg: DagConfig, **kwargs: object) -> RunResult:
    """Run a DAG on a daemon thread and FAIL (not hang) if it does not finish in time.

    This is the load-bearing assertion of the whole file. Calling ``run_dag`` directly would
    reproduce the defect as an infinite hang, which wedges the suite instead of reporting it;
    joining with a deadline turns "the scheduler never terminates" into an ordinary failing
    assertion, and daemon=True keeps a wedged runner from blocking interpreter exit.
    """
    box: dict[str, RunResult] = {}
    err: list[BaseException] = []

    def _go() -> None:
        try:
            box["res"] = run_dag(cfg, **kwargs)  # type: ignore[arg-type]
        except BaseException as exc:  # pragma: no cover - surfaced via err below
            err.append(exc)

    t = threading.Thread(target=_go, daemon=True)
    t.start()
    t.join(timeout=_DEADLINE_S)
    assert not t.is_alive(), (
        f"scheduler HUNG: run_dag did not return within {_DEADLINE_S}s. This is the "
        "silent-hang regression — a step supervisor died without recording an outcome, so "
        "the ready-set loop can never reach its break condition."
    )
    if err:
        raise err[0]
    return box["res"]


def _crash_outcomes(res: RunResult) -> list[StepOutcome]:
    return [o for o in res.outcomes if o.reason.startswith(SUPERVISOR_CRASH_REASON)]


class _RaisingCgroups:
    """A :class:`CgroupManager` that raises from ONE chosen method, and is otherwise a no-op.

    Planting here rather than by patching the scheduler means the exception originates at a
    call site the supervisor really makes, in the real order, on the real thread.
    """

    def __init__(self, raise_in: str, *, enabled: bool = False) -> None:
        self._raise_in = raise_in
        self.enabled = enabled

    def _maybe(self, name: str) -> None:
        if name == self._raise_in:
            raise RuntimeError(f"planted supervisor fault in {name}")

    def prepare_command(
        self,
        tag: str,
        cmd: str,
        mem_max: int | None = None,
        cpu_count: int | None = None,
    ) -> str:
        self._maybe("prepare_command")
        return cmd

    def kill(self, tag: str) -> bool:
        return False

    def cleanup(self, tag: str) -> None:
        self._maybe("cleanup")

    def oom_kills(self, tag: str) -> int:
        self._maybe("oom_kills")
        return 0

    def peak_bytes(self, tag: str) -> int | None:
        return None

    def cpu_stats(self, tag: str) -> Mapping[str, int] | None:
        return None

    def cpu_pressure(self, tag: str) -> Mapping[str, float] | None:
        return None

    def thread_count(self, tag: str) -> int | None:
        return None

    def kill_all_remaining(self) -> int:
        return 0


def _one_step_cfg() -> DagConfig:
    return DagConfig(steps=(_step("g", "A", "true"),))


# --------------------------------------------------------------------------------------
# NEGATIVE CONTROL — the same DAGs, nothing planted. Establishes that the positive tests
# below are detecting the plant and not something that is true of every run.
# --------------------------------------------------------------------------------------


def test_clean_run_records_no_supervisor_crash() -> None:
    res = _run_bounded(_one_step_cfg(), jobs=2, verbosity=0)
    assert res.ok
    assert _crash_outcomes(res) == []


def test_ordinary_step_failure_is_not_labelled_a_supervisor_crash() -> None:
    """A step that exits non-zero is a PRODUCT failure; it must not be tarred with the runner
    -bug marker, or the marker would be useless for triage."""
    res = _run_bounded(
        DagConfig(steps=(_step("g", "A", "exit 3"),)), jobs=2, verbosity=0
    )
    assert not res.ok
    assert _crash_outcomes(res) == []
    assert res.outcomes[0].returncode == 3


# --------------------------------------------------------------------------------------
# LAYER 1 — the per-supervisor exception guard, at each of the three windows.
# --------------------------------------------------------------------------------------


@pytest.mark.parametrize("plant", ["prepare_command", "oom_kills", "cleanup"])
def test_exception_on_step_path_fails_loudly_instead_of_hanging(plant: str) -> None:
    """Planted at a real call site before the spawn (``prepare_command``) and after the child
    exits (``oom_kills`` / ``cleanup``)."""
    res = _run_bounded(
        _one_step_cfg(),
        jobs=2,
        verbosity=0,
        cgroups=_RaisingCgroups(plant),
    )
    assert not res.ok, "a supervisor crash must fail the run"
    crashes = _crash_outcomes(res)
    assert len(crashes) == 1, f"expected exactly one crash outcome, got {res.outcomes}"
    outcome = crashes[0]
    assert outcome.tag == "g.A"
    assert outcome.ok is False
    # The reason must NAME the exception — a bare "something went wrong" would leave the
    # operator in the same position as the silent hang.
    assert "RuntimeError" in outcome.reason
    assert plant in outcome.reason


def test_crash_reaches_stdout_and_stderr(capfd: pytest.CaptureFixture[str]) -> None:
    """Loudness is the point: the traceback must be visible on BOTH streams, because a caller
    running at verbosity 0 with stdout captured would otherwise still see nothing."""
    res = _run_bounded(
        _one_step_cfg(), jobs=2, verbosity=0, cgroups=_RaisingCgroups("oom_kills")
    )
    assert not res.ok
    out, err = capfd.readouterr()
    for stream_name, stream in (("stdout", out), ("stderr", err)):
        assert SUPERVISOR_CRASH_REASON in stream, f"crash not reported on {stream_name}"
        assert "RuntimeError: planted supervisor fault" in stream, (
            f"exception message missing from {stream_name}"
        )
    assert "Traceback (most recent call last)" in err


def test_discovering_case_typeerror_from_stepoutcome_failed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """The exact shape that found this defect: a bad keyword argument to
    ``StepOutcome.failed``, raised INSIDE the locked bookkeeping block after ``running`` has
    already been cleared but before ``done`` is written."""
    def _boom(*args: object, **kwargs: object) -> StepOutcome:
        raise TypeError(
            "StepOutcome.failed() got an unexpected keyword argument 'planted_bogus_kwarg'"
        )

    # Same class object the scheduler holds, so patching here patches its call site.
    monkeypatch.setattr(StepOutcome, "failed", _boom)
    res = _run_bounded(
        DagConfig(
            steps=(
                _step("g", "A", "exit 1"),
                _step("g", "B", "true", deps=["g.A"]),
            )
        ),
        jobs=2,
        verbosity=0,
    )
    assert not res.ok
    crashes = _crash_outcomes(res)
    assert len(crashes) == 1
    assert "TypeError" in crashes[0].reason
    assert "planted_bogus_kwarg" in crashes[0].reason
    # The dependent must still be skipped: the crash closes over dependents like any failure.
    assert "g.B" in res.skipped


def test_crash_releases_resources_exactly_once() -> None:
    """A crash must give the step's scarce-resource and core budget back — and give it back
    ONCE. Double-release would silently raise the cap above its declared value, converting a
    loud bug into quiet over-subscription."""
    cfg = DagConfig(
        steps=(_step("g", "A", "true", resources={"slot": 1}),),
        resource_caps={"slot": 1},
    )
    runner = sched.Runner(
        cfg, jobs=2, cgroups=_RaisingCgroups("oom_kills"), verbosity=0
    )
    ok = runner.run()
    assert ok is False
    assert runner.resource_avail == {"slot": 1}, "resource not returned exactly once"
    assert runner.cores_used == 0
    assert runner.running == set()
    # Idempotence, directly: a second retire of an already-retired step is a no-op.
    step = cfg.steps[0]
    with runner.lock:
        runner._retire(step)
        runner._retire(step)
    assert runner.resource_avail == {"slot": 1}
    assert runner.cores_used == 0


def test_crash_in_one_step_does_not_lose_a_healthy_sibling() -> None:
    """keep_going: a crashed supervisor must not take an unrelated in-flight step's own
    verdict away from it."""
    cfg = DagConfig(
        steps=(
            _step("g", "crasher", "true", est=0.0),
            _step("g", "healthy", "sleep 0.3", est=100.0),
        )
    )

    class _OnlyCrasher(_RaisingCgroups):
        def oom_kills(self, tag: str) -> int:
            if tag == "g.crasher":
                raise RuntimeError("planted supervisor fault in oom_kills")
            return 0

    res = _run_bounded(
        cfg, jobs=2, verbosity=0, keep_going=True, cgroups=_OnlyCrasher("never")
    )
    assert not res.ok
    outcomes = {o.tag: o for o in res.outcomes}
    assert outcomes["g.crasher"].reason.startswith(SUPERVISOR_CRASH_REASON)
    assert outcomes["g.healthy"].ok is True
    assert outcomes["g.healthy"].reason == ""


# --------------------------------------------------------------------------------------
# LAYER 2 — the dead-supervisor sweep. Independent backstop: it must catch a thread that
# vanishes even when the layer-1 guard does not run at all.
# --------------------------------------------------------------------------------------


@pytest.mark.filterwarnings("ignore::pytest.PytestUnhandledThreadExceptionWarning")
def test_dead_supervisor_is_swept_even_when_layer_one_is_absent(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Disable layer 1 entirely (replace the guarded entry point with one that just dies) and
    confirm the scheduler still terminates with a loud outcome.

    Without layer 2 this test does not merely fail — it HANGS, which ``_run_bounded`` reports
    as a failed assertion. That is the anti-vacuity evidence for the sweep.
    """

    def _die_immediately(self: sched.Runner, step: Step) -> None:
        raise RuntimeError("layer-1 guard bypassed; supervisor thread dies unguarded")

    monkeypatch.setattr(sched.Runner, "_run_step", _die_immediately)
    res = _run_bounded(_one_step_cfg(), jobs=2, verbosity=0)
    assert not res.ok
    crashes = _crash_outcomes(res)
    assert len(crashes) == 1
    assert "exited without recording an outcome" in crashes[0].reason
    assert crashes[0].tag == "g.A"


@pytest.mark.filterwarnings("ignore::pytest.PytestUnhandledThreadExceptionWarning")
def test_dead_supervisor_sweep_catches_a_crash_after_the_tag_left_running(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """The window a ``running``-keyed sweep MISSES, and why the predicate is outcome-keyed.

    The discovering case crashes inside the locked bookkeeping block — after ``_retire`` has
    cleared ``running`` but before ``done`` is written — so the tag sits in NEITHER set. With
    layer 1 disabled and the sweep keyed on ``running``, this hung for the full deadline; it
    is the case that proves layer 2 is a real backstop rather than a partial one.
    """
    real_body = sched.Runner._run_step_body

    def _unguarded(self: sched.Runner, step: Step) -> None:
        self._run_step_body(step)  # layer 1 disabled: no try/except around the body

    def _body_then_boom(self: sched.Runner, step: Step) -> None:
        real_body(self, step)  # completes normally: retires the tag AND records the outcome
        if step.tag != "g.A":
            return
        with self.lock:
            self.done.pop(step.tag, None)  # reproduce the window: outcome never written
        raise TypeError("planted: crash after _retire, before done was recorded")

    monkeypatch.setattr(sched.Runner, "_run_step", _unguarded)
    monkeypatch.setattr(sched.Runner, "_run_step_body", _body_then_boom)
    # A second, slower step keeps the ready-set loop alive past g.A's crash window, so the
    # sweep is genuinely exercised instead of the loop racing to its break condition first.
    cfg = DagConfig(
        steps=(_step("g", "A", "true"), _step("g", "keepalive", "sleep 1.0")),
    )
    res = _run_bounded(cfg, jobs=2, verbosity=0, keep_going=True)
    assert not res.ok
    crashes = _crash_outcomes(res)
    assert [c.tag for c in crashes] == ["g.A"]
    assert "exited without recording an outcome" in crashes[0].reason


def test_dead_supervisor_sweep_is_inert_on_a_healthy_run() -> None:
    """Positive control for the sweep's OTHER direction: it must not mistake a live or
    just-launched supervisor for a dead one. A DAG of several concurrent short steps exercises
    the launch/exit boundary repeatedly."""
    cfg = DagConfig(
        steps=tuple(_step("g", f"s{i}", "sleep 0.05") for i in range(8)),
    )
    res = _run_bounded(cfg, jobs=8, verbosity=0)
    assert res.ok, [(o.tag, o.reason) for o in res.outcomes]
    assert _crash_outcomes(res) == []
    assert len(res.outcomes) == 8


def test_dead_supervisor_detection_ignores_a_step_with_a_recorded_outcome() -> None:
    """A supervisor that recorded its outcome and then exited is NOT lost, even though its
    thread is dead — the sweep keys on (still in running) AND (no outcome), not on liveness
    alone."""
    cfg = _one_step_cfg()
    runner = sched.Runner(cfg, jobs=1, cgroups=sched._NoopCgroupManager(), verbosity=0)
    step = cfg.steps[0]
    dead = threading.Thread(target=lambda: None)
    dead.start()
    dead.join()
    with runner.lock:
        runner.running.add(step.tag)
        runner.step_threads[step.tag] = dead
        # (a) dead thread, no outcome -> lost
        assert [s.tag for s in runner._dead_supervisors()] == [step.tag]
        # (b) same dead thread, outcome present -> not lost
        runner.done[step.tag] = StepOutcome(
            tag=step.tag, ok=True, duration_s=0.0, summary="", returncode=0
        )
        assert runner._dead_supervisors() == []


def test_supervisor_crash_in_the_reporting_tail_is_not_swallowed(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """A crash AFTER the outcome was recorded still fails the run loudly. The outcome already
    present must not be overwritten, but neither may the traceback be dropped just because the
    step happened to finish first."""
    real_emit = sched.Runner._emit
    state = {"n": 0}

    def _emit_then_boom(self: sched.Runner, line: str) -> None:
        real_emit(self, line)
        # Match the terminal PASS STATUS line only, by prefix. A substring match on "PASS"
        # (even "✓ PASS") also matches the crash handler's own traceback dump, which quotes
        # the emit call site verbatim — so the plant fired twice and re-entered the handler.
        # That accident is how the reporting guard in _record_lost_supervisor was found; the
        # prefix keeps this test measuring one thing.
        if line.startswith("[g.A] ✓ PASS"):
            state["n"] += 1
            raise RuntimeError("planted fault in the reporting tail")

    monkeypatch.setattr(sched.Runner, "_emit", _emit_then_boom)
    res = _run_bounded(_one_step_cfg(), jobs=1, verbosity=0)
    assert state["n"] == 1, "the plant did not fire; the test would be vacuous"
    assert not res.ok, "a runner bug must fail the run even if the step itself passed"
    outcomes = {o.tag: o for o in res.outcomes}
    assert outcomes["g.A"].ok is True, "the recorded outcome must not be overwritten"


def test_supervisor_crash_reason_marker_is_stable() -> None:
    """The marker is a triage contract (it distinguishes a RUNNER bug from a product failure),
    so pin its text."""
    assert SUPERVISOR_CRASH_REASON == "SUPERVISOR CRASH"


# --------------------------------------------------------------------------------------
# The OTHER daemon thread on the step path. A monitor death does not hang the run, but it
# silently disables the only per-step CPU-time enforcement there is — the same class of
# invisible degradation, so it must be visible.
# --------------------------------------------------------------------------------------


def test_monitor_thread_death_warns_and_does_not_fail_the_step(
    capfd: pytest.CaptureFixture[str],
) -> None:
    class _ThreadCountBoom(_RaisingCgroups):
        enabled = True

        def thread_count(self, tag: str) -> int | None:
            raise RuntimeError("planted fault in the monitor poll")

    cfg = DagConfig(steps=(_step("g", "A", "sleep 1.3"),))
    res = _run_bounded(cfg, jobs=1, verbosity=0, cgroups=_ThreadCountBoom("never"))
    _, err = capfd.readouterr()
    assert "monitor thread died" in err, f"degradation was silent; stderr was: {err!r}"
    assert "RuntimeError" in err
    # The step keeps its own verdict: the wall timeout is still a live backstop, so a dead
    # monitor must not be escalated into a step failure.
    assert res.ok, [(o.tag, o.reason) for o in res.outcomes]
    assert _crash_outcomes(res) == []
