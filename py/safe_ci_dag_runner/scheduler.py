"""Dependency-aware, resource-aware concurrent DAG execution.

The scheduler gates on dependencies, named resource capacities, memory, and fan-out;
orders ready work by estimated duration; and stops launching work after a failure.
Per-step supervision reaps complete process trees and records structured outcomes.
"""

from __future__ import annotations

import os
import subprocess
import sys
import threading
import time
from collections.abc import Mapping, Sequence

from safe_ci_dag_runner.ambient import (
    AmbientSnapshot,
    PsiReading,
    capture_ambient_snapshot,
)
from safe_ci_dag_runner.attribution import (
    LOG_DIR_ENV,
    NO_LOGS_ENV,
    Culprit,
    RunEvidence,
    StepStream,
    default_log_dir,
    sanitize as sanitize_evidence_tag,
)
from safe_ci_dag_runner.model import (
    DagConfig,
    Step,
    command_with_inner_jobs,
    effective_cpu_count,
    effective_cpu_timeout,
    preferred_inner_jobs,
    step_classification,
    write_domain_violations,
)
from safe_ci_dag_runner.profile_enrich import (
    resolve_effective_inner_jobs,
    step_enrichment_columns,
)
from safe_ci_dag_runner.protocols import (
    CgroupManager,
    MetricsSink,
    RunResult,
    RunWindow,
    StepOutcome,
)
from safe_ci_dag_runner.sizing import step_mem_cap_bytes
from safe_ci_dag_runner.teardown import (
    STEP_NONCE_ENV,
    mint_step_nonce,
    reap,
    reap_many,
)

__all__ = ["Runner", "run_dag"]

#: Scheduler idle interval between ready-set sweeps (seconds). Matches the reference's
#: ``time.sleep(0.05)`` — short enough to keep dispatch latency low, long enough to avoid
#: busy-spinning the lock while steps run.
_LOOP_SLEEP_S = 0.05

#: Per-step monitor poll interval (seconds) for descendant-thread-peak sampling.
_MONITOR_INTERVAL_S = 1.0

#: Grace period to wait for a process to die after a timeout-triggered reap (seconds).
_POST_TIMEOUT_WAIT_S = 10.0

#: Brief join timeout for the daemon reader/monitor threads at step end (seconds).
_THREAD_JOIN_S = 2.0


def _warn(message: str) -> None:
    """Emit a visible degraded-behavior warning (No Silent Failure)."""
    print(f"[scheduler] ⚠ {message}", file=sys.stderr)


def _psi_reading(pressure: Mapping[str, float] | None) -> PsiReading | None:
    """Adapt a cgroup ``cpu.pressure`` ``{avg10, avg60}`` mapping to a typed :class:`PsiReading`
    for the enrichment builder; ``None`` (unreadable / unboxed) passes straight through."""
    if pressure is None:
        return None
    return PsiReading(avg10=pressure["avg10"], avg60=pressure["avg60"])


class _NoopRunWindow:
    """A no-op :class:`RunWindow`: records nothing, returns ``None`` from ``finish``."""

    def finish(
        self, *, result: str, n_steps: int, jobs: int
    ) -> Mapping[str, object] | None:
        return None


class _NoopMetricsSink:
    """The default :class:`MetricsSink` used when the caller passes ``metrics=None``.

    Every method is a benign no-op — this is a deliberately-chosen "record nothing" sink,
    NOT a silent skip of a requested destination, so it warns about nothing.
    """

    def start_run_window(self) -> RunWindow:
        return _NoopRunWindow()

    def record_step_profiles(
        self, rows: Sequence[Mapping[str, object]], *, jobs: int
    ) -> str | None:
        return None


class _NoopCgroupManager:
    """The default :class:`CgroupManager` used when the caller passes ``cgroups=None``.

    :attr:`enabled` is ``False`` and every method is a no-op, so a step runs unwrapped and
    teardown falls back to the process-group kill in :func:`safe_ci_dag_runner.teardown.reap`
    — observationally identical to DeepScry's "no cgroups" path.
    """

    enabled: bool = False

    def prepare_command(
        self,
        tag: str,
        cmd: str,
        mem_max: int | None = None,
        cpu_count: int | None = None,
    ) -> str:
        return cmd

    def kill(self, tag: str) -> bool:
        return False

    def cleanup(self, tag: str) -> None:
        return None

    def oom_kills(self, tag: str) -> int:
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


class Runner:
    """Executes one :class:`DagConfig` to completion, then hands back a :class:`RunResult`.

    Construct once per run and call :meth:`run`. All scheduling state is guarded by a single
    :class:`threading.Lock`; per-step supervisors run on daemon threads. The public surface
    is :meth:`run` (drive the DAG, returns overall success) and :meth:`result` (assemble the
    typed :class:`RunResult` after :meth:`run`).
    """

    def __init__(
        self,
        cfg: DagConfig,
        *,
        jobs: int,
        cgroups: CgroupManager,
        keep_going: bool = False,
        verbosity: int = 1,
        order: Sequence[str] | None = None,
        core_budget: int | None = None,
        run_timeout_s: int | None = None,
    ) -> None:
        self.cfg = cfg
        self.jobs = max(1, jobs)
        self.cgroups = cgroups
        self.keep_going = keep_going
        # verbosity: 0 quiet(+failures), 1 default(+summaries), >=2 stream child stdout.
        self.verbosity = verbosity
        # CPA core-budget gate (MCPA's insight, PLANNER_DESIGN.md §5.7): when set, the ready-set
        # loop never lets the summed inner-jobs width of concurrently-running steps exceed this
        # total core budget P, so a boxed run stays true to the measured curves the allocator used.
        # None (the default, non-CPA planners) disables the gate — behavior is unchanged.
        self.core_budget = core_budget
        self.cores_used = 0
        self.steps: dict[str, Step] = cfg.by_tag()
        # Dispatch order. When the caller supplies an explicit `order` (e.g. a critical-path
        # planner's), use it verbatim; otherwise default to LONGEST-processing-time first (LPT
        # makespan heuristic): sort by the static duration hint DESCENDING. The sort is STABLE, so
        # steps with equal/no hint keep registration order. Order only decides which READY step is
        # picked first when a slot/resource frees; dependency and resource gating are enforced
        # separately in run().
        if order is not None:
            self.order = list(order)
        else:
            self.order = sorted(
                (s.tag for s in cfg.steps),
                key=lambda tag: self.steps[tag].hint.est_duration_s,
                reverse=True,
            )
        # Remaining capacity per named scarce resource (mutable copy of the caps).
        self.resource_avail: dict[str, int] = dict(cfg.resource_caps)
        self.lock = threading.Lock()
        self.done: dict[str, StepOutcome] = {}
        self.running: set[str] = set()
        # tag -> the in-flight step's Popen, so a sibling that FAILS can eager-reap it.
        self.running_procs: dict[str, subprocess.Popen[bytes]] = {}
        # The exact inherited ownership token reaches environment-preserving setsid/double-fork
        # escapees after process-group and parentage tracking lose their originating step.
        self.running_nonces: dict[str, str] = {}
        self.aborted: set[str] = set()  # tags killed by eager-exit (labelled ABORTED, not FAIL)
        self.step_profile_rows: list[Mapping[str, object]] = []
        self.failed = False  # a genuine (non-aborted) step failed
        self.stop = False  # stop scheduling new steps after a failure
        self.wall = 0.0
        # OUTER wall budget for the WHOLE run (None = unbounded). Independent of every per-step
        # budget, and that independence is the point: no combination of individually-legal steps
        # can run past it.
        self.run_timeout_s = run_timeout_s if (run_timeout_s or 0) > 0 else None
        self.run_timed_out = False
        self.evidence = RunEvidence.open(default_log_dir())

    # -- gating helpers (mirror validate.py Runner._deps_ok / _deps_known / _res_free) --

    def _deps_ok(self, step: Step) -> bool:
        """True when EVERY dep has a done, successful outcome."""
        return all(d in self.done and self.done[d].ok for d in step.deps)

    def _deps_known(self, step: Step) -> bool:
        """True when every dep has reached a terminal outcome (regardless of success)."""
        return all(d in self.done for d in step.deps)

    def _res_free(self, step: Step) -> bool:
        """True when the step's scarce-resource demand currently fits remaining capacity."""
        return all(
            self.resource_avail.get(r, 0) >= n for r, n in step.hint.resources.items()
        )

    def _step_width(self, step: Step) -> int:
        """The step's inner-jobs width for the core-budget gate (its ``preferred_inner_jobs``, else
        1). Under ``--planner cpa`` this is the allocated width baked into the hint."""
        width = preferred_inner_jobs(step)
        return width if (width is not None and width > 0) else 1

    def _cores_free(self, step: Step) -> bool:
        """True when the step fits the remaining core budget, OR nothing is running (so a step
        wider than the whole budget still runs — alone — instead of deadlocking). The gate is
        inactive (always True) when ``core_budget`` is ``None``."""
        if self.core_budget is None:
            return True
        if self.cores_used == 0:
            return True
        return self.cores_used + self._step_width(step) <= self.core_budget

    def _acquire(self, step: Step) -> None:
        for r, n in step.hint.resources.items():
            self.resource_avail[r] -= n

    def _release(self, step: Step) -> None:
        for r, n in step.hint.resources.items():
            self.resource_avail[r] += n

    def _skipped(self) -> set[str]:
        """Tags whose deps FAILED (transitively) so they must never run.

        A fixpoint closure: a step is skipped if any dep is done-and-failed OR already in the
        skip set. Ported verbatim from ``validate.py`` ``Runner._skipped``.
        """
        sk: set[str] = set()
        changed = True
        while changed:
            changed = False
            for tag, step in self.steps.items():
                if tag in sk or tag in self.done or tag in self.running:
                    continue
                for d in step.deps:
                    if (d in self.done and not self.done[d].ok) or d in sk:
                        sk.add(tag)
                        changed = True
                        break
        return sk

    def _emit(self, line: str) -> None:
        """Serialize a status line to stdout under the runner lock."""
        with self.lock:
            sys.stdout.write(line + "\n")
            sys.stdout.flush()

    def run(self) -> bool:
        """Drive the DAG to completion; returns ``True`` when no genuine failure occurred.

        Greedy ready-set loop: each pass launches every currently-ready step (deps satisfied,
        resources free, under the ``-j`` fan-out, in LPT order) on its own daemon supervisor
        thread, then sleeps. Terminates when nothing is running AND either every step has a
        terminal outcome (done or dep-skipped) OR fail-fast has tripped — the fail-fast clause
        is REQUIRED: once ``stop`` is set, steps whose deps SUCCEEDED are neither launched nor
        counted as skipped, so without it the loop would busy-wait forever.
        """
        threads: list[threading.Thread] = []
        wall_start = time.time()
        if self.evidence is not None:
            print(
                f"[scheduler] per-step logs + test-boundary journal: "
                f"{self.evidence.directory} (set {LOG_DIR_ENV} to relocate, {NO_LOGS_ENV}=1 "
                "to disable)",
                file=sys.stderr,
            )
            self.evidence.record(
                "containment",
                [("state", "boxed" if self.cgroups.enabled else "unboxed")],
            )
        deadline = (
            wall_start + self.run_timeout_s if self.run_timeout_s is not None else None
        )
        while True:
            with self.lock:
                # OUTER BUDGET, CHECKED IN OUR OWN LOOP AND NOT BY AN EXTERNAL KILLER. Stopping the
                # run from inside is the entire reason this exists: an outside kill (a CI job
                # cancellation, a systemd RuntimeMaxSec) also destroys the evidence, so the bound
                # that fires FIRST must be one that can still write rows and hand back a verdict.
                if (
                    deadline is not None
                    and time.time() >= deadline
                    and not self.run_timed_out
                ):
                    self.run_timed_out = True
                    self.failed = True
                    self.stop = True
                    cut = list(self.running_procs.items())
                    print(
                        f"[scheduler] RUN TIMEOUT: the whole run exceeded its outer budget of "
                        f"{self.run_timeout_s}s ({time.time() - wall_start:.1f}s elapsed). "
                        f"Cutting {len(cut)} in-flight step(s) short so the run can still report: "
                        + (", ".join(tag for tag, _ in cut) or "<none running>"),
                        file=sys.stderr,
                    )
                    if self.evidence is not None:
                        self.evidence.record(
                            "run_timeout",
                            [
                                ("budget_s", str(self.run_timeout_s or 0)),
                                ("elapsed_s", f"{time.time() - wall_start:.3f}"),
                                ("cut_steps", ",".join(tag for tag, _ in cut)),
                                ("done", str(len(self.done))),
                            ],
                        )
                    for other, _other_proc in cut:
                        self.aborted.add(other)
                    reap_many(
                        tuple(
                            (other_proc, other, self.running_nonces.get(other))
                            for other, other_proc in cut
                        ),
                        self.cgroups,
                    )
                skipped = self._skipped()
                if not self.running and (
                    self.stop or len(self.done) + len(skipped) >= len(self.steps)
                ):
                    break
                launchable: list[Step] = []
                if not self.stop:
                    for tag in self.order:
                        if tag in self.done or tag in self.running or tag in skipped:
                            continue
                        step = self.steps[tag]
                        if not self._deps_known(step):
                            continue  # deps not resolved yet
                        if not self._deps_ok(step):
                            continue  # a dep failed -> handled as skip next pass
                        if len(self.running) >= self.jobs:
                            break
                        if not self._res_free(step):
                            continue
                        if not self._cores_free(step):
                            continue
                        launchable.append(step)
                        self.running.add(tag)
                        self._acquire(step)
                        self.cores_used += self._step_width(step)
                for step in launchable:
                    t = threading.Thread(
                        target=self._run_step, args=(step,), daemon=True
                    )
                    t.start()
                    threads.append(t)
            time.sleep(_LOOP_SLEEP_S)
        for t in threads:
            t.join()
        self.wall = time.time() - wall_start
        return not self.failed

    def _run_step(self, step: Step) -> None:
        """Supervise ONE step: launch, pump stdout, enforce the timeout, reap, classify."""
        self._emit(f"[{step.tag}] ▶ START  {step.desc}")
        env = dict(os.environ)
        env.update(step.env)
        nonce = mint_step_nonce()
        # Runner authority wins over a DAG-supplied environment value.
        env[STEP_NONCE_ENV] = nonce
        inner_jobs = preferred_inner_jobs(step)
        # SMALL default caps for an undeclared step (the forcing function): fall back to the
        # DAG's tight 1-GiB memory.max / 1-core cpu.max / 10-s CPU-time floor when the step
        # declares nothing for that dimension. An explicit hint always wins.
        mem_max = step_mem_cap_bytes(
            step,
            mem_cap_factor=self.cfg.mem_cap_factor,
            default_cap_bytes=self.cfg.default_step_mem_cap_bytes,
        )
        cpu_count = effective_cpu_count(step, self.cfg.default_step_cpu_count)
        cpu_budget = effective_cpu_timeout(step, self.cfg.default_step_cpu_timeout)
        start = time.time()
        stream = self.verbosity >= 2
        timed_out = False
        cpu_timed_out = False

        # Append the step's inner-parallelism (concurrency) flag when it declares one, using the
        # step's jobs_flag template (or the DagConfig default, e.g. "-j"). No-op when the step
        # declares no preferred_inner_jobs.
        base_cmd = command_with_inner_jobs(step, self.cfg.default_jobs_flag, inner_jobs)

        # When per-step cgroups are enabled, prepare_command wraps the command so the bash
        # leader self-moves into the step's child cgroup BEFORE forking any grandchild (the
        # cgroup-v2 fork-inheritance rule), applying the inner memory/CPU caps. A disabled /
        # noop manager returns the command unchanged.
        run_cmd = self.cgroups.prepare_command(
            step.tag, base_cmd, mem_max=mem_max, cpu_count=cpu_count
        )
        # Parallel-speedup ENRICHMENT capture (only under real cgroup boxing, matching DeepScry).
        # prepare_command has already created the step's child cgroup, so cpu.pressure is readable;
        # bracket the step with two host-load snapshots so contention can be attributed later.
        boxed = self.cgroups.enabled
        ambient_start: AmbientSnapshot | None = (
            capture_ambient_snapshot(()) if boxed else None
        )
        step_pressure_start = _psi_reading(self.cgroups.cpu_pressure(step.tag)) if boxed else None
        # start_new_session=True gives the step its OWN process group/session (pgid == child
        # pid) so teardown can reap the whole tree (bash leader + any server/browser
        # grandchildren) without ever touching the runner's own group.
        proc: subprocess.Popen[bytes] = subprocess.Popen(
            ["bash", "-c", run_cmd],
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            start_new_session=True,
        )
        with self.lock:
            self.running_procs[step.tag] = proc
            self.running_nonces[step.tag] = nonce

        sink = StepStream(step.tag, self.evidence)
        if self.evidence is not None:
            self.evidence.record(
                "step_start",
                [
                    ("step", step.tag),
                    ("pid", str(proc.pid)),
                    ("timeout_s", str(step.timeout)),
                    ("cmd", run_cmd),
                ],
            )
        captured: list[bytes] = []

        def _pump() -> None:
            # A step may spawn grandchildren (servers, browsers) that outlive it and hold the
            # stdout pipe's write-end open, so a naive read on the main thread would never see
            # EOF. We read on a DAEMON thread; the main thread blocks only on proc.wait().
            try:
                stdout = proc.stdout
                if stdout is None:
                    return
                pending = bytearray()
                # `BufferedReader.read(n)` may wait for all n bytes. Read the pipe descriptor
                # directly so an unterminated libtest start marker is journaled while the test is
                # still running, rather than only after teardown closes the pipe.
                while raw := os.read(stdout.fileno(), 8192):
                    captured.append(raw)
                    sink.ingest(raw)
                    if stream:
                        pending.extend(raw)
                        while b"\n" in pending:
                            index = pending.index(b"\n")
                            line = bytes(pending[: index + 1])
                            del pending[: index + 1]
                            self._emit(
                                f"[{step.tag}] "
                                + line.decode(errors="replace").rstrip("\n")
                            )
                if stream and pending:
                    self._emit(
                        f"[{step.tag}] "
                        + bytes(pending).decode(errors="replace").rstrip("\n")
                    )
            except Exception:
                pass  # a broken/held pipe must never crash the supervisor

        monitor_stop = threading.Event()
        thread_peak: int | None = None

        def _monitor() -> None:
            # Poll the step's cgroup descendant-thread count for a per-step peak (metrics
            # only; a noop/disabled manager returns None and this stays a cheap no-op).
            # When the step declares a CPU-time budget, this same 1 Hz loop also enforces
            # it from the cgroup's cpu.stat usage_usec (user+system). CPU time is immune to
            # machine load, so this guard can be far tighter than the wall `timeout` without
            # flaking; the wall timeout stays as a backstop for a step that blocks/hangs
            # while burning no CPU. Enforcement is best-effort at the poll granularity
            # (_MONITOR_INTERVAL_S), and inert when cgroup boxing is off (cpu_stats is None).
            nonlocal thread_peak, cpu_timed_out
            while not monitor_stop.wait(_MONITOR_INTERVAL_S):
                count = self.cgroups.thread_count(step.tag)
                if count is not None:
                    thread_peak = count if thread_peak is None else max(thread_peak, count)
                if cpu_budget > 0 and not cpu_timed_out:
                    cs = self.cgroups.cpu_stats(step.tag)
                    if cs is not None:
                        cpu_used_s = cs.get("usage_usec", 0) / 1_000_000
                        if cpu_used_s >= cpu_budget:
                            # Over CPU budget: reap the whole group now. The main thread's
                            # proc.wait() then returns normally (no wall TimeoutExpired).
                            cpu_timed_out = True
                            if self.evidence is not None:
                                culprit_now = sink.culprit()
                                self.evidence.record(
                                    "cpu_timeout",
                                    [
                                        ("step", step.tag),
                                        ("cpu_timeout_s", str(cpu_budget)),
                                        ("culprit_test", culprit_now.test or ""),
                                        ("culprit_basis", culprit_now.how),
                                        ("tests_completed", str(culprit_now.completed)),
                                    ],
                                )
                            reap(proc, self.cgroups, step.tag, nonce=nonce)
                            return

        reader = threading.Thread(target=_pump, daemon=True)
        monitor = threading.Thread(target=_monitor, daemon=True)
        reader.start()
        monitor.start()
        try:
            proc.wait(timeout=step.timeout)
        except subprocess.TimeoutExpired:
            # Genuine hang: reap the step's whole process group now (safe because
            # start_new_session gave it its own group; reap guards the runner's group).
            timed_out = True
            if self.evidence is not None:
                culprit_now = sink.culprit()
                self.evidence.record(
                    "step_timeout",
                    [
                        ("step", step.tag),
                        ("elapsed_s", f"{time.time() - start:.3f}"),
                        ("timeout_s", str(step.timeout)),
                        ("culprit_test", culprit_now.test or ""),
                        ("culprit_basis", culprit_now.how),
                        ("tests_completed", str(culprit_now.completed)),
                    ],
                )
            reap(proc, self.cgroups, step.tag, nonce=nonce)
            try:
                proc.wait(timeout=_POST_TIMEOUT_WAIT_S)
            except Exception:
                pass
        monitor_stop.set()
        monitor.join(timeout=_THREAD_JOIN_S)
        # The daemon reader may still be blocked on an orphan-held pipe; because it is a
        # daemon and we never block on it, the step completes regardless. Brief join, move on.
        reader.join(timeout=_THREAD_JOIN_S)
        # Reap the whole process group so any orphan grandchildren are SIGKILLed now instead
        # of leaking into later steps (and this lets the abandoned reader finally see EOF).
        reap(proc, self.cgroups, step.tag, nonce=nonce)

        # Read the step's cgroup measurements BEFORE cleanup() rmdirs the child cgroup.
        oom = self.cgroups.oom_kills(step.tag)
        peak = self.cgroups.peak_bytes(step.tag)
        cpu_stats = self.cgroups.cpu_stats(step.tag)
        step_pressure_end = _psi_reading(self.cgroups.cpu_pressure(step.tag)) if boxed else None
        ambient_end: AmbientSnapshot | None = capture_ambient_snapshot(()) if boxed else None
        self.cgroups.cleanup(step.tag)

        elapsed = time.time() - start
        dur = round(elapsed)
        returncode = proc.returncode
        ok = returncode == 0 and not timed_out and not cpu_timed_out
        summary = _last_line(captured)
        culprit: Culprit | None = sink.culprit() if timed_out or cpu_timed_out else None

        row: dict[str, object] = {
            "step": step.tag,
            "classification": step_classification(step).value,
            # Resolve to a NUMBER (the effective parallelism the step ran in) — never the old
            # "ambient" string — so the speedup model can group samples by parallelism level.
            "inner_jobs": resolve_effective_inner_jobs(inner_jobs),
            "elapsed_s": round(elapsed, 3),
            "returncode": returncode,
            "ok": ok,
            "timed_out": timed_out,
            "cpu_timed_out": cpu_timed_out,
            "oom_kills": oom,
            "peak_bytes": peak,
            "thread_peak": thread_peak,
        }
        if cpu_stats is not None:
            for key, value in cpu_stats.items():
                row[f"cpu.{key}"] = value
        # Rich parallel-speedup enrichment columns (effective_cores, throttled_s, contention, PSI).
        # Only under real boxing; an un-boxed run leaves them blank (the writer fills them from the
        # STEP_PROFILE_COLUMNS schema), matching DeepScry's "blank when unavailable" posture.
        if boxed:
            row.update(
                step_enrichment_columns(
                    elapsed_s=elapsed,
                    inner_jobs=inner_jobs,
                    cpu_stats=cpu_stats,
                    ambient_start=ambient_start,
                    ambient_end=ambient_end,
                    step_pressure_start=step_pressure_start,
                    step_pressure_end=step_pressure_end,
                )
            )

        with self.lock:
            self.running.discard(step.tag)
            self.running_procs.pop(step.tag, None)
            self.running_nonces.pop(step.tag, None)
            self._release(step)
            self.cores_used -= self._step_width(step)
            self.step_profile_rows.append(row)
            was_aborted = step.tag in self.aborted
            if was_aborted:
                outcome = StepOutcome(
                    tag=step.tag,
                    ok=False,
                    duration_s=elapsed,
                    summary=summary,
                    returncode=returncode,
                    reason="ABORTED (eager-exit after another step failed; keep_going lets in-flight steps finish)",
                    aborted=True,
                )
            elif ok:
                outcome = StepOutcome(
                    tag=step.tag,
                    ok=True,
                    duration_s=elapsed,
                    summary=summary,
                    returncode=returncode,
                )
            else:
                outcome = StepOutcome.failed(
                    step.tag,
                    duration_s=elapsed,
                    summary=summary,
                    returncode=returncode,
                    oomed=oom > 0,
                    oom_kills=oom,
                    timed_out=timed_out,
                    timeout=step.timeout,
                    cpu_timed_out=cpu_timed_out,
                    cpu_timeout=cpu_budget,
                    pids_guard_tripped=False,
                    pids_guard_reason=None,
                    detail_write_failure=(),
                )
            self.done[step.tag] = outcome
            if not was_aborted and not ok:
                # A REAL failure. Mark failed + stop launching NEW steps. EAGER-EXIT (default):
                # reap every step still running in parallel NOW so a fast failure doesn't wait for
                # a slow in-flight build. keep_going instead lets those in-flight steps finish (so
                # they report their own pass/fail); it does NOT launch any further steps.
                self.failed = True
                self.stop = True
                if not self.keep_going:
                    others = list(self.running_procs.items())
                    for other, _other_proc in others:
                        self.aborted.add(other)  # its thread will label itself ABORTED
                    reap_many(
                        tuple(
                            (other_proc, other, self.running_nonces.get(other))
                            for other, other_proc in others
                        ),
                        self.cgroups,
                    )

        # Emit terminal status OUTSIDE the lock (_emit re-acquires it).
        if outcome.aborted and self.run_timed_out:
            # Distinguish the two ways a step gets cancelled. "Another step failed" and "the whole
            # run ran out of budget" call for different follow-up, and the eager-exit wording sends
            # a reader hunting for a failing peer that does not exist.
            self._emit(
                f"[{step.tag}] ⊘ ABORT  {step.desc} "
                f"({dur}s — cut short by the OUTER run budget, not by a failure of its own "
                f"or of a peer)"
            )
        elif outcome.aborted:
            self._emit(
                f"[{step.tag}] ⊘ ABORT  {step.desc} "
                f"({dur}s — eager-exit after another step failed; keep_going lets in-flight steps finish)"
            )
        elif outcome.ok:
            extra = f"  [{summary}]" if (summary and self.verbosity >= 1) else ""
            self._emit(f"[{step.tag}] ✓ PASS   {step.desc} ({dur}s){extra}")
        else:
            if culprit is not None:
                self._emit(f"[{step.tag}] ↳ {culprit.describe()}")
                if self.evidence is not None:
                    self._emit(
                        f"[{step.tag}] ↳ full step output preserved at "
                        f"{self.evidence.directory}/{sanitize_evidence_tag(step.tag)}.log"
                    )
            self._emit(f"[{step.tag}] ✗ FAIL   {step.desc} ({dur}s, {outcome.reason})")
            if oom > 0:
                self._emit(
                    f"[{step.tag}] ▲ MEMORY CAP HIT: OOM-killed at its inner cgroup "
                    f"MemoryMax (cap≈{_fmt_bytes(mem_max)}, peak≈{_fmt_bytes(peak)}). "
                    "Confirm this is genuine growth, not an unbounded leak, before raising the "
                    "step's rss_baseline_bytes / hard_mem_max_bytes hint."
                )
            # Self-contained failure: dump the captured child output, tagged.
            self._emit(f"[{step.tag}] ----- detail -----")
            for line in b"".join(captured).decode(errors="replace").splitlines():
                self._emit(f"[{step.tag}] {line}")
            self._emit(f"[{step.tag}] ----- end detail -----")

        if self.evidence is not None:
            counts = sink.counts()
            self.evidence.record(
                "step_end",
                [
                    ("step", step.tag),
                    ("ok", str(ok).lower()),
                    ("aborted", str(outcome.aborted).lower()),
                    ("timed_out", str(timed_out).lower()),
                    ("cpu_timed_out", str(cpu_timed_out).lower()),
                    ("elapsed_s", f"{elapsed:.3f}"),
                    ("tests_started", str(counts.started)),
                    ("tests_completed", str(counts.completed)),
                    ("culprit_test", culprit.test if culprit and culprit.test else ""),
                ],
            )

    def result(self) -> RunResult:
        """Assemble the typed :class:`RunResult` after :meth:`run` has returned.

        Outcomes are ordered by the LPT dispatch order for stable, readable reporting;
        ``skipped`` lists tags whose deps failed so they never ran.
        """
        outcomes = tuple(self.done[tag] for tag in self.order if tag in self.done)
        skipped = tuple(sorted(self._skipped()))
        return RunResult(
            ok=not self.failed,
            wall_s=self.wall,
            outcomes=outcomes,
            skipped=skipped,
            step_profile_rows=tuple(self.step_profile_rows),
            run_timed_out=self.run_timed_out,
        )


def steps_violating_run_timeout(
    cfg: DagConfig, run_timeout_s: int
) -> list[tuple[str, int]]:
    """Steps whose own wall budget is not STRICTLY SMALLER than the run's outer budget.

    INNER < OUTER IS THE WHOLE ORDERING, and it is checkable before anything runs. A step allowed
    to run as long as (or longer than) the run itself can only ever be terminated by the outer
    bound, which attributes the failure to "the run overran" instead of to the node that caused
    it — precisely the report that made a real regression unexplainable.
    """
    if run_timeout_s <= 0:
        return []
    return sorted(
        (s.tag, s.timeout) for s in cfg.steps if s.timeout >= run_timeout_s
    )


def run_dag(
    cfg: DagConfig,
    *,
    jobs: int,
    cgroups: CgroupManager | None = None,
    metrics: MetricsSink | None = None,
    keep_going: bool = False,
    verbosity: int = 1,
    order: Sequence[str] | None = None,
    core_budget: int | None = None,
    run_timeout_s: int | None = None,
) -> RunResult:
    """Run a whole DAG and return its :class:`RunResult`.

    This is the one-call entry point most callers want. It brackets the run in a
    :class:`RunWindow` (whole-run metrics), drives the :class:`Runner`, flushes the outer-scope
    cgroup backstop, records the accumulated per-step profile rows, and returns the typed
    result.

    :param cfg: the DAG plus caller policy (steps, resource caps, memory tunables).
    :param jobs: outer scheduler fan-out (``-j``); clamped to at least 1.
    :param cgroups: per-step containment manager, or ``None`` for the no-containment path
        (a :class:`_NoopCgroupManager` is substituted; teardown falls back to process-group
        kill). The function does not establish an outer cgroup scope itself. A
        present-but-disabled manager triggers a visible degraded-enforcement warning.
    :param metrics: durable measurement sink, or ``None`` for no recording.
    :param keep_going: on a failure, let already-running steps finish instead of eager-cancelling
        them; the scheduler still stops launching new steps (it does NOT run every still-runnable
        step), so in-flight steps report their own pass/fail rather than ABORTED.
    :param verbosity: 0 quiet (+failures), 1 default (+summaries), >=2 stream child stdout.
    :param order: explicit dispatch order (e.g. a critical-path planner's); ``None`` uses the
        built-in longest-processing-time default.
    :param core_budget: total inner-jobs core budget ``P`` for the CPA dispatch gate; when set the
        scheduler never lets the summed width of concurrently-running steps exceed it. ``None``
        disables the gate (the non-CPA default).
    :param run_timeout_s: OUTER wall budget for the WHOLE run. On breach the scheduler stops
        launching, terminates every in-flight step's tree, and RETURNS with
        ``RunResult.run_timed_out`` set — it does not abandon the process to an outside killer,
        because an outside kill takes the evidence with it. ``None`` leaves the run unbounded.
        FAIL CLOSED: if any step may run as long as the whole run, this refuses to start, because
        a bound whose breach cannot be attributed to a node reads like enforcement without being
        usable as one.
    """
    domain_errors = write_domain_violations(cfg)
    if domain_errors:
        print(
            "[scheduler] ERROR: REFUSING to run before any node starts: "
            "write-domain policy violation(s): " + "; ".join(domain_errors),
            file=sys.stderr,
        )
        return RunResult(ok=False, wall_s=0.0)
    if (run_timeout_s or 0) > 0:
        assert run_timeout_s is not None
        bad = steps_violating_run_timeout(cfg, run_timeout_s)
        if bad:
            detail = ", ".join(f"{tag} ({secs}s)" for tag, secs in bad)
            print(
                f"[scheduler] ERROR: REFUSING to run: the outer run budget is {run_timeout_s}s "
                f"but {len(bad)} step(s) declare a wall budget at least that large, so the outer "
                f"bound would fire before they do and the failure could not be attributed to a "
                f"node: {detail}. Lower those step timeouts or raise --run-timeout.",
                file=sys.stderr,
            )
            return RunResult(ok=False, wall_s=0.0)
    sink: MetricsSink = metrics if metrics is not None else _NoopMetricsSink()
    manager: CgroupManager = cgroups if cgroups is not None else _NoopCgroupManager()
    if cgroups is not None and not cgroups.enabled:
        # A supplied manager represents requested containment, so expose degraded enforcement.
        _warn(
            "per-step cgroup manager is present but disabled; containment is DEGRADED "
            "(falling back to process-group kill for teardown, no inner memory/CPU caps)."
        )

    runner = Runner(
        cfg,
        jobs=jobs,
        cgroups=manager,
        keep_going=keep_going,
        verbosity=verbosity,
        order=order,
        core_budget=core_budget,
        run_timeout_s=run_timeout_s,
    )
    window: RunWindow = sink.start_run_window()
    ok = runner.run()

    # NORMAL-exit backstop: reap any step cgroup that still has live procs (a setsid orphan a
    # step left behind lives there). Does NOT stop the outer scope, so a green run stays green.
    if manager.enabled:
        leftover = manager.kill_all_remaining()
        if leftover:
            print(
                f"[scheduler] reaped {leftover} leftover step cgroup(s) on exit "
                "(setsid orphans a step left behind)."
            )

    result = runner.result()
    window.finish(result="pass" if ok else "fail", n_steps=len(runner.done), jobs=jobs)
    location = sink.record_step_profiles(result.step_profile_rows, jobs=jobs)
    if metrics is not None and location is None and result.step_profile_rows:
        # A real sink was supplied yet recording was skipped — surface it (No Silent Failure).
        _warn(
            f"metrics sink recorded no location for {len(result.step_profile_rows)} step "
            "profile row(s); the rows may have been dropped."
        )
    return result


def _last_line(chunks: Sequence[bytes]) -> str:
    """Best-effort one-line summary: the last non-empty decoded line of captured output."""
    for raw in reversed(chunks):
        text = raw.decode(errors="replace").strip()
        if text:
            return text
    return ""


def _fmt_bytes(n: int | None) -> str:
    """Human-readable byte count (e.g. ``3.5 GiB``); ``"?"`` when unknown."""
    if n is None:
        return "?"
    value = float(n)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if value < 1024.0 or unit == "TiB":
            return f"{value:.1f} {unit}" if unit != "B" else f"{int(value)} B"
        value /= 1024.0
    return f"{int(n)} B"
