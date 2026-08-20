"""Dependency-aware, resource-aware concurrent DAG execution.

The scheduler gates on dependencies, named resource capacities, and active-step count; orders ready
work by estimated duration; and stops launching after a failure. ``max_cpus`` caps the width of
each individual step, while concurrent steps may deliberately overcommit that outer bandwidth.
Per-step supervision reaps complete process trees and records structured outcomes.
"""

from __future__ import annotations

import os
import subprocess
import sys
import threading
import time
from collections.abc import Mapping, Sequence
from dataclasses import replace
from enum import Enum
from typing import overload

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
    bind_process_tests,
    default_log_dir,
    process_snapshot,
    sanitize as sanitize_evidence_tag,
)
from safe_ci_dag_runner.model import (
    DagConfig,
    Step,
    command_with_inner_jobs,
    effective_cpu_count,
    effective_jobs_flag,
    canonical_cpu_timeout,
    scale_cpu_timeout,
    preferred_inner_jobs,
    step_classification,
    undeclared_resource_demands,
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
from safe_ci_dag_runner.sizing import _step_mem_cap_for_inner_jobs
from safe_ci_dag_runner.teardown import (
    STEP_NONCE_ENV,
    mint_step_nonce,
    reap,
    reap_many,
)

__all__ = [
    "Runner",
    "cap_config_max_cpus",
    "cap_config_cpu_jobs",
    "run_dag",
    "run_dag_limited",
]

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


class BudgetUnit(Enum):
    """The quantity a per-step budget bounds, and the ONLY unit a breach may be reported in.

    A wall ceiling and a CPU ceiling are different physical quantities, and for the same step
    they are different numbers: wall keeps rising while the step is descheduled, CPU does not.
    Naming the unit is not decoration.  A termination record that printed an unlabelled
    ``elapsed_s`` (wall) beside a ``limit_s`` that was CPU-seconds invited the natural reading
    that the two were comparable, which is how a CPU breach came to be quoted as consuming more
    seconds than its own run's CPU rollup contained.

    The values are part of the durable journal's public shape, so every paired implementation
    of this runner spells them identically.
    """

    CPU_SECONDS = "cpu_seconds"
    WALL_SECONDS = "wall_seconds"


def _warn(message: str) -> None:
    """Emit a visible degraded-behavior warning (No Silent Failure)."""
    print(f"[scheduler] ⚠ {message}", file=sys.stderr)


#: cgroup ``cpu.stat`` counters worth keeping, mapped to journal keys that name their unit.
_CPU_JOURNAL_COUNTERS = (
    ("usage_usec", "cpu_usage_usec"),
    ("nr_throttled", "cpu_nr_throttled"),
    ("throttled_usec", "cpu_throttled_usec"),
)


def _cpu_journal_fields(stats: Mapping[str, int] | None) -> list[tuple[str, str]]:
    """Final cgroup CPU counters for the durable step journal, with units in the key names.

    These are the readings already taken BEFORE ``cleanup()`` removes the step's cgroup, so they
    cost nothing extra and are the last CPU figures that will ever exist for the step.  They
    belong in the journal for the same reason the terminal record exists at all: a hard kill
    destroys the end-of-run profile flush, and then the journal is the only thing left that can
    say what the step consumed against the budget it was given.

    A missing input map, or a kernel that does not publish one of these counters, stays ABSENT.
    It must not become a measured zero — that is the same substitution the CPU guard used to make.
    """
    if stats is None:
        return []
    return [
        (journal_key, str(stats[source]))
        for source, journal_key in _CPU_JOURNAL_COUNTERS
        if source in stats
    ]


def _cpu_seconds_from_stats(stats: Mapping[str, int]) -> float | None:
    """Consumed user+system CPU-seconds from a step's cgroup ``cpu.stat``, else ``None``.

    ABSENT IS NOT ZERO.  A missing ``usage_usec`` means the step's CPU cannot be MEASURED, not
    that it has consumed none.  Reading it as 0 made the budget comparison permanently
    unsatisfiable, so a declared CPU-time budget silently enforced nothing — an enforcement guard
    switched off by a missing field, with no warning anywhere.  ``None`` forces the caller to say
    so instead.
    """
    usage_usec = stats.get("usage_usec")
    return None if usage_usec is None else usage_usec / 1_000_000


def _psi_reading(pressure: Mapping[str, float] | None) -> PsiReading | None:
    """Adapt a cgroup ``cpu.pressure`` ``{avg10, avg60}`` mapping to a typed :class:`PsiReading`
    for the enrichment builder; ``None`` (unreadable / unboxed) passes straight through."""
    if pressure is None:
        return None
    return PsiReading(avg10=pressure["avg10"], avg60=pressure["avg60"])


def cap_config_max_cpus(cfg: DagConfig, max_cpus: int) -> DagConfig:
    """Copy ``cfg`` with every runner-controlled per-step CPU width capped to ``max_cpus``.

    The clamp is intentionally visible: a caller-authored width changed by the run budget must
    not look as though it executed unchanged. A step whose effective ``jobs_flag`` is empty manages
    its own command width and cannot be rewritten; its declared width is left unchanged so run
    entry points can refuse an over-budget configuration instead of corrupting scheduling/profile
    metadata. The undeclared-step ``cpu.max`` default is capped too.
    """
    budget = max(1, max_cpus)
    default_cpu_count = cfg.default_step_cpu_count
    if default_cpu_count is not None and default_cpu_count > budget:
        _warn(
            f"default_step_cpu_count={default_cpu_count} exceeds --max-cpus {budget}; "
            f"capping undeclared steps' per-step cpu.max to {budget}"
        )
        default_cpu_count = budget

    steps: list[Step] = []
    for step in cfg.steps:
        width = step.hint.preferred_inner_jobs
        if width is not None and width > budget:
            if not effective_jobs_flag(step, cfg.default_jobs_flag).strip():
                steps.append(step)
                continue
            _warn(
                f"step {step.tag} preferred_inner_jobs={width} exceeds --max-cpus {budget}; "
                f"capping its command width and per-step cpu.max to {budget}"
            )
            step = replace(
                step,
                hint=replace(step.hint, preferred_inner_jobs=budget),
                deps=list(step.deps),
                env=dict(step.env),
            )
        steps.append(step)
    return replace(
        cfg,
        steps=tuple(steps),
        default_step_cpu_count=default_cpu_count,
        resource_caps=dict(cfg.resource_caps),
    )


def _self_managed_width_violations(
    cfg: DagConfig, max_cpus: int
) -> tuple[tuple[str, int], ...]:
    """Declared over-budget widths the runner cannot rewrite because ``jobs_flag`` is empty."""

    budget = max(1, max_cpus)
    return tuple(
        sorted(
            (step.tag, width)
            for step in cfg.steps
            if step.skip_reason is None
            and (width := step.hint.preferred_inner_jobs) is not None
            and width > budget
            and not effective_jobs_flag(step, cfg.default_jobs_flag).strip()
        )
    )


def _self_managed_width_error(cfg: DagConfig, max_cpus: int) -> str | None:
    """Actionable refusal text for an unclampable declared width, or ``None`` when safe."""

    violations = _self_managed_width_violations(cfg, max_cpus)
    if not violations:
        return None
    detail = ", ".join(
        f"{tag} (preferred_inner_jobs={width})" for tag, width in violations
    )
    return (
        f"--max-cpus {max(1, max_cpus)} cannot lower guest parallelism for step(s) with an empty "
        f"effective jobs_flag: {detail}; set each step's jobs_flag to the guest's worker-count "
        "option (or remove the empty override and set default_jobs_flag), reduce "
        "preferred_inner_jobs, or raise --max-cpus"
    )


def cap_config_cpu_jobs(cfg: DagConfig, cpu_jobs: int) -> DagConfig:
    """Compatibility alias for :func:`cap_config_max_cpus`."""

    return cap_config_max_cpus(cfg, cpu_jobs)


def _resolve_max_cpus_argument(
    max_cpus: int | None, cpu_jobs: int | None
) -> int:
    """Resolve the canonical keyword and its 0.13 compatibility alias."""

    if max_cpus is not None and cpu_jobs is not None and max_cpus != cpu_jobs:
        raise TypeError("max_cpus and legacy cpu_jobs disagree")
    resolved = max_cpus if max_cpus is not None else cpu_jobs
    if resolved is None:
        raise TypeError("missing required keyword-only argument: 'max_cpus'")
    return max(1, resolved)


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

    def set_worker_pids_max(self, limit: int | None) -> None:
        return None

    def pids_events(self, tag: str) -> int:
        return 0

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

    @overload
    def __init__(
        self,
        cfg: DagConfig,
        *,
        max_steps: int,
        max_cpus: int,
        cgroups: CgroupManager,
        cpu_jobs: None = None,
        keep_going: bool = False,
        verbosity: int = 1,
        order: Sequence[str] | None = None,
        run_timeout_s: int | None = None,
    ) -> None: ...

    @overload
    def __init__(
        self,
        cfg: DagConfig,
        *,
        max_steps: int,
        max_cpus: int,
        cpu_jobs: int,
        cgroups: CgroupManager,
        keep_going: bool = False,
        verbosity: int = 1,
        order: Sequence[str] | None = None,
        run_timeout_s: int | None = None,
    ) -> None: ...

    @overload
    def __init__(
        self,
        cfg: DagConfig,
        *,
        max_steps: int,
        cpu_jobs: int,
        cgroups: CgroupManager,
        max_cpus: None = None,
        keep_going: bool = False,
        verbosity: int = 1,
        order: Sequence[str] | None = None,
        run_timeout_s: int | None = None,
    ) -> None: ...

    def __init__(
        self,
        cfg: DagConfig,
        *,
        max_steps: int,
        max_cpus: int | None = None,
        cpu_jobs: int | None = None,
        cgroups: CgroupManager,
        keep_going: bool = False,
        verbosity: int = 1,
        order: Sequence[str] | None = None,
        run_timeout_s: int | None = None,
    ) -> None:
        self.max_cpus = _resolve_max_cpus_argument(max_cpus, cpu_jobs)
        if error := _self_managed_width_error(cfg, self.max_cpus):
            raise ValueError(error)
        # Public 0.13 attribute retained for source compatibility; new code uses max_cpus.
        self.cpu_jobs = self.max_cpus
        self.cfg = cap_config_max_cpus(cfg, self.max_cpus)
        self.max_steps = max(1, max_steps)
        self.cgroups = cgroups
        self.keep_going = keep_going
        # verbosity: 0 quiet(+failures), 1 default(+summaries), >=2 stream child stdout.
        self.verbosity = verbosity
        # Requested-width telemetry retained for source compatibility. This is deliberately NOT an
        # admission budget: concurrent steps may overcommit the outer max_cpus bandwidth, so the
        # sum can exceed self.max_cpus while --max-steps still bounds active DAG nodes.
        self.cores_used = 0
        self.steps: dict[str, Step] = self.cfg.by_tag()
        self.intentional_skips = tuple(
            (step.tag, step.skip_reason)
            for step in self.cfg.steps
            if step.skip_reason is not None
        )
        self.intentional_skip_tags = {tag for tag, _reason in self.intentional_skips}
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
                (s.tag for s in self.cfg.steps),
                key=lambda tag: self.steps[tag].hint.est_duration_s,
                reverse=True,
            )
        # Remaining capacity per named scarce resource (mutable copy of the caps).
        self.resource_avail: dict[str, int] = dict(self.cfg.resource_caps)
        self.lock = threading.Lock()
        self.done: dict[str, StepOutcome] = {}
        self.running: set[str] = set()
        # tag -> the in-flight step's Popen, so a sibling that FAILS can eager-reap it.
        self.running_procs: dict[str, subprocess.Popen[bytes]] = {}
        # The exact inherited ownership token reaches environment-preserving setsid/double-fork
        # escapees after process-group and parentage tracking lose their originating step.
        self.running_nonces: dict[str, str] = {}
        # Measured child-process concurrency. Scheduler admission is not enough: a short command
        # can exit before a later admitted thread creates its child, and named resources can keep
        # all but one child from starting. Count only successful Popen -> observed-exit lifetimes.
        self.active_processes = 0
        self.max_concurrent_steps = 0
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
        """The declared CPU width included in :attr:`cores_used` telemetry.

        An explicit ``preferred_inner_jobs`` wins; otherwise the DAG's per-step ``cpu.max``
        default is the effective width. Only a fully unbounded/invalid default falls back to one.
        The jobs-flag behavior remains separate: an undeclared step does not receive a synthetic
        command-line ``-j`` merely because its cgroup default is wider than one.
        """
        width = effective_cpu_count(step, self.cfg.default_step_cpu_count)
        return width if (width is not None and width > 0) else 1

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
                if (
                    tag in sk
                    or tag in self.done
                    or tag in self.running
                    or tag in self.intentional_skip_tags
                ):
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

    def _capture_termination_evidence(
        self,
        *,
        sink: StepStream,
        step: Step,
        proc: subprocess.Popen[bytes],
        nonce: str,
        event: str,
        unit: BudgetUnit,
        limit_s: int,
        measured_s: float,
        wall_elapsed_s: float,
    ) -> Culprit:
        """Persist test/process state before SIGTERM starts the graceful-kill window.

        ``measured_s`` is the quantity the guard actually compared against ``limit_s``, and one
        ``unit`` covers both: recording a wall figure against a CPU bound is unrepresentable
        rather than merely discouraged.  ``wall_elapsed_s`` rides along as context and is only
        the compared quantity when the limit is itself a wall limit.
        """
        observations = process_snapshot(proc.pid, nonce)
        for row in observations:
            self._emit(
                f"[{step.tag}] ↳ process pid={row.pid} ppid={row.ppid} "
                f"signature={row.signature} wall={row.wall_elapsed_s:.3f}s "
                f"cpu={row.cpu_elapsed_s:.3f}s"
                + (f" test={row.test}" if row.test else "")
                + f" cmd={row.command}"
            )
            if self.evidence is not None:
                self.evidence.record(
                    "process_snapshot",
                    [
                        ("step", step.tag),
                        ("pid", str(row.pid)),
                        ("ppid", str(row.ppid)),
                        ("signature", row.signature),
                        ("wall_elapsed_s", f"{row.wall_elapsed_s:.3f}"),
                        ("cpu_elapsed_s", f"{row.cpu_elapsed_s:.3f}"),
                        ("test", row.test or ""),
                        ("test_basis", row.test_basis or ""),
                        ("command", row.command),
                    ],
                )
        culprit = bind_process_tests(sink.culprit(), observations)
        if self.evidence is not None:
            self.evidence.record(
                event,
                [
                    ("step", step.tag),
                    ("measured_s", f"{measured_s:.3f}"),
                    ("limit_s", str(limit_s)),
                    # Applies to BOTH numbers above, which is the point: they are one comparison.
                    ("unit", unit.value),
                    ("wall_elapsed_s", f"{wall_elapsed_s:.3f}"),
                    ("culprit_test", culprit.test or ""),
                    ("culprit_basis", culprit.how),
                    ("tests_completed", str(culprit.completed)),
                    ("in_flight_count", str(len(culprit.in_flight))),
                    (
                        "in_flight_tests",
                        ",".join(
                            f"{test.name}@{test.elapsed_s:.3f}s"
                            for test in culprit.in_flight
                        ),
                    ),
                ],
            )
        return culprit

    def run(self) -> bool:
        """Drive the DAG to completion; returns ``True`` when no genuine failure occurred.

        Greedy ready-set loop: each pass launches every currently-ready step (deps satisfied,
        resources free, and under the active-step limit, in LPT order) on its own daemon supervisor
        thread, then sleeps. Terminates when nothing is running AND either every step has a
        terminal outcome (done or dep-skipped) OR fail-fast has tripped — the fail-fast clause
        is REQUIRED: once ``stop`` is set, steps whose deps SUCCEEDED are neither launched nor
        counted as skipped, so without it the loop would busy-wait forever. Under ``keep_going``
        a failure does not trip fail-fast, so termination rests on the first clause: independent
        work keeps launching and only true dependents enter the dependency-skip closure.
        """
        threads: list[threading.Thread] = []
        wall_start = time.time()
        for tag, reason in self.intentional_skips:
            self._emit(f"[{tag}] SKIPPED reason={reason.value}")
            if self.evidence is not None:
                self.evidence.record(
                    "step_skip", [("step", tag), ("reason", reason.value)]
                )
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
                    for other in self.running:
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
                    self.stop
                    or len(self.done) + len(skipped) + len(self.intentional_skips)
                    >= len(self.steps)
                ):
                    break
                launchable: list[Step] = []
                if not self.stop:
                    for tag in self.order:
                        if (
                            tag in self.done
                            or tag in self.running
                            or tag in skipped
                            or tag in self.intentional_skip_tags
                        ):
                            continue
                        step = self.steps[tag]
                        if not self._deps_known(step):
                            continue  # deps not resolved yet
                        if not self._deps_ok(step):
                            continue  # a dep failed -> handled as skip next pass
                        if len(self.running) >= self.max_steps:
                            break
                        if not self._res_free(step):
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
        cpu_count = effective_cpu_count(step, self.cfg.default_step_cpu_count)
        # SMALL default caps for an undeclared step (the forcing function): fall back to the
        # DAG's tight 1-GiB memory.max / 1-core cpu.max / 10-s CPU-time floor when the step
        # declares nothing for that dimension. CPU-bound caps use the same effective preferred /
        # default width and scaling rule as --max-mem planning; an explicit hard cap still wins.
        mem_max = _step_mem_cap_for_inner_jobs(
            step,
            cpu_count,
            mem_cap_factor=self.cfg.mem_cap_factor,
            default_cap_bytes=self.cfg.default_step_mem_cap_bytes,
        )
        cpu_canonical = canonical_cpu_timeout(step, self.cfg.default_step_cpu_timeout)
        # The ENFORCED budget is the canonical one scaled for this platform; both are kept
        # so a breach can name the graph's number and the policy that changed it.
        cpu_budget = scale_cpu_timeout(cpu_canonical, self.cfg.cpu_timeout_multiplier)
        start = time.time()
        stream = self.verbosity >= 2
        timed_out = False
        cpu_timed_out = False
        termination_culprit: Culprit | None = None

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
        # Profile the width the step ACTUALLY ran under. An undeclared boxed command intentionally
        # gets no jobs flag but is constrained by the default per-step cpu.max, so it belongs in
        # that width bucket. Unboxed execution has no applied default cap and must retain the
        # ambient fallback instead of claiming one-core enforcement that did not exist.
        profile_inner_jobs = (
            inner_jobs if inner_jobs is not None else (cpu_count if boxed else None)
        )
        ambient_start: AmbientSnapshot | None = (
            capture_ambient_snapshot(()) if boxed else None
        )
        step_pressure_start = _psi_reading(self.cgroups.cpu_pressure(step.tag)) if boxed else None
        # start_new_session=True gives the step its OWN process group/session (pgid == child
        # pid) so teardown can reap the whole tree (bash leader + any server/browser
        # grandchildren) without ever touching the runner's own group.
        try:
            proc: subprocess.Popen[bytes] = subprocess.Popen(
                ["bash", "-c", run_cmd],
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                start_new_session=True,
            )
        except (OSError, ValueError) as exc:
            # A thread that dies here without publishing a terminal outcome leaves the tag in
            # ``running`` forever, so the scheduler busy-waits and the thread traceback is the
            # only explanation. Convert process-creation failure into an ordinary failed step and
            # restore every scheduler-accounting field acquired before this supervisor started.
            elapsed = time.time() - start
            summary = f"spawn failed: {exc}"
            outcome = StepOutcome.failed(
                step.tag,
                duration_s=elapsed,
                summary=summary,
                returncode=None,
                oomed=False,
                oom_kills=0,
                timed_out=False,
                timeout=step.timeout,
                cpu_timed_out=False,
                cpu_timeout=cpu_budget,
                pids_guard_tripped=False,
                pids_guard_reason=None,
                detail_write_failure=(),
                cpu_timeout_canonical=cpu_canonical,
                cpu_timeout_multiplier=self.cfg.cpu_timeout_multiplier,
                cpu_timeout_platform=self.cfg.cpu_timeout_platform,
            )
            self.cgroups.cleanup(step.tag)
            with self.lock:
                self.running.discard(step.tag)
                self.running_procs.pop(step.tag, None)
                self.running_nonces.pop(step.tag, None)
                self._release(step)
                self.cores_used -= self._step_width(step)
                self.done[step.tag] = outcome
                self.failed = True
                if not self.keep_going:
                    self.stop = True
                    others = list(self.running_procs.items())
                    for other in self.running:
                        self.aborted.add(other)
                    reap_many(
                        tuple(
                            (other_proc, other, self.running_nonces.get(other))
                            for other, other_proc in others
                        ),
                        self.cgroups,
                    )
            self._emit(f"[{step.tag}] ✗ FAIL   {step.desc} ({summary})")
            return
        with self.lock:
            self.running_procs[step.tag] = proc
            self.running_nonces[step.tag] = nonce
            self.active_processes += 1
            self.max_concurrent_steps = max(
                self.max_concurrent_steps, self.active_processes
            )
            abort_after_spawn = step.tag in self.aborted
        if abort_after_spawn:
            # A peer can fail after this tag was admitted but before its Popen registered. The
            # failing thread marks every pre-admitted tag aborted; honor that mark immediately so
            # the narrow registration race cannot turn eager-exit into a full sibling wait.
            reap(proc, self.cgroups, step.tag, nonce=nonce)

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
            nonlocal thread_peak, cpu_timed_out, termination_culprit
            # One-shot, so an unmeasurable budget is stated once per step, not once per tick.
            unmeasurable_warned = False
            while not monitor_stop.wait(_MONITOR_INTERVAL_S):
                count = self.cgroups.thread_count(step.tag)
                if count is not None:
                    thread_peak = count if thread_peak is None else max(thread_peak, count)
                if cpu_budget > 0 and not cpu_timed_out:
                    cs = self.cgroups.cpu_stats(step.tag)
                    if cs is not None:
                        # ABSENT IS NOT ZERO: see :func:`_cpu_seconds_from_stats`. Say so once
                        # and leave the budget explicitly unenforced rather than reading a
                        # missing counter as "has consumed none".
                        measured = _cpu_seconds_from_stats(cs)
                        if measured is None:
                            if not unmeasurable_warned:
                                unmeasurable_warned = True
                                _warn(
                                    f"step {step.tag!r}: cgroup cpu.stat has no 'usage_usec', so "
                                    f"the {cpu_budget}s CPU-time budget CANNOT be enforced for "
                                    "this step; only the wall timeout still applies."
                                )
                            continue
                        cpu_used_s = measured
                        if cpu_used_s >= cpu_budget:
                            # Over CPU budget: reap the whole group now. The main thread's
                            # proc.wait() then returns normally (no wall TimeoutExpired).
                            cpu_timed_out = True
                            termination_culprit = self._capture_termination_evidence(
                                sink=sink,
                                step=step,
                                proc=proc,
                                nonce=nonce,
                                event="cpu_timeout",
                                unit=BudgetUnit.CPU_SECONDS,
                                limit_s=cpu_budget,
                                # The very reading the comparison above was made on.
                                measured_s=cpu_used_s,
                                wall_elapsed_s=time.time() - start,
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
            termination_culprit = self._capture_termination_evidence(
                sink=sink,
                step=step,
                proc=proc,
                nonce=nonce,
                event="step_timeout",
                unit=BudgetUnit.WALL_SECONDS,
                limit_s=step.timeout,
                measured_s=time.time() - start,
                wall_elapsed_s=time.time() - start,
            )
            reap(proc, self.cgroups, step.tag, nonce=nonce)
            try:
                proc.wait(timeout=_POST_TIMEOUT_WAIT_S)
            except Exception:
                pass
        # proc.wait() has now observed the child exit (normally or after timeout teardown).
        # Stop counting it before monitor/reader joins, which can outlive the child when a
        # grandchild holds an output pipe open.
        with self.lock:
            self.active_processes -= 1
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
        pids_events = self.cgroups.pids_events(step.tag)
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
        culprit: Culprit | None = (
            termination_culprit or sink.culprit()
            if timed_out or cpu_timed_out
            else None
        )

        row: dict[str, object] = {
            "step": step.tag,
            "classification": step_classification(step).value,
            # Resolve to a NUMBER (the effective parallelism the step ran in) — never the old
            # "ambient" string — so the speedup model can group samples by parallelism level.
            "inner_jobs": resolve_effective_inner_jobs(profile_inner_jobs),
            "elapsed_s": round(elapsed, 3),
            "returncode": returncode,
            "ok": ok,
            "timed_out": timed_out,
            "cpu_timed_out": cpu_timed_out,
            "oom_kills": oom,
            "peak_bytes": peak,
            "thread_peak": thread_peak,
        }
        # NOTE: pids_events is deliberately NOT a CSV column — it rides on the in-memory
        # StepOutcome instead (see StepOutcome.pids_events), so the Python/Rust CSV-header
        # differential stays byte-identical while a PID-cap breach is still classifiable.
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
                    inner_jobs=profile_inner_jobs,
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
                    reason="ABORTED (eager-exit after another step failed; --keep-going would continue independent work)",
                    aborted=True,
                    pids_events=pids_events,
                )
            elif ok:
                # A step can exit 0 yet still have hit its inner pids.max (e.g. a shell whose
                # backgrounded forks were denied but that returns 0 anyway). Carry the count so a
                # consumer can still surface the fork-bomb containment; ``ok`` itself is unchanged.
                outcome = StepOutcome(
                    tag=step.tag,
                    ok=True,
                    duration_s=elapsed,
                    summary=summary,
                    returncode=returncode,
                    pids_events=pids_events,
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
                    pids_guard_tripped=pids_events > 0,
                    pids_guard_reason=(
                        f"hit inner pids.max ({pids_events} denied fork/clone event(s))"
                        if pids_events > 0 else None
                    ),
                    pids_events=pids_events,
                    cpu_timeout_canonical=cpu_canonical,
                    cpu_timeout_multiplier=self.cfg.cpu_timeout_multiplier,
                    cpu_timeout_platform=self.cfg.cpu_timeout_platform,
                    detail_write_failure=(),
                )
            self.done[step.tag] = outcome
            if not was_aborted and not ok:
                # A REAL failure. The run is failed either way; what differs is COVERAGE.
                #
                # EAGER-EXIT (default): stop launching NEW steps AND reap every step still running
                # in parallel NOW, so a fast failure doesn't wait for a slow in-flight build.
                #
                # keep_going: do NEITHER. The failure is recorded but scheduling stays open, so
                # independent ready steps keep launching and in-flight steps report their own
                # pass/fail. Steps whose deps genuinely failed are still excluded — ``_skipped()``
                # closes over failed deps transitively — so wider coverage never means running a
                # step on broken prerequisites.
                #
                # Termination still holds because ``self.stop`` stays False here: the loop's exit
                # condition then rests on its other clause, done + skipped + intentional >=
                # len(steps), which every step now reaches. The ``stop`` clause exists precisely
                # for the eager-exit case, where deps-succeeded steps are neither launched nor
                # skipped and the loop would otherwise spin forever.
                self.failed = True
                if not self.keep_going:
                    self.stop = True
                    others = list(self.running_procs.items())
                    for other in self.running:
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
                f"({dur}s — eager-exit after another step failed; --keep-going would continue independent work)"
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
            if pids_events > 0:
                self._emit(
                    f"[{step.tag}] ▲ PIDS CAP HIT: {pids_events} fork/clone(s) denied at its "
                    "inner cgroup pids.max (PID-exhaustion / fork-bomb containment). The process "
                    "was contained, not kernel-killed; the cpu/wall guard reaps it."
                )
            # Self-contained failure: dump the captured child output, tagged.
            self._emit(f"[{step.tag}] ----- detail -----")
            for line in b"".join(captured).decode(errors="replace").splitlines():
                self._emit(f"[{step.tag}] {line}")
            self._emit(f"[{step.tag}] ----- end detail -----")

        if self.evidence is not None:
            counts = sink.counts()
            fields = [
                ("step", step.tag),
                ("ok", str(ok).lower()),
                ("aborted", str(outcome.aborted).lower()),
                ("timed_out", str(timed_out).lower()),
                ("cpu_timed_out", str(cpu_timed_out).lower()),
                ("wall_elapsed_s", f"{elapsed:.3f}"),
                ("tests_started", str(counts.started)),
                ("tests_completed", str(counts.completed)),
                ("culprit_test", culprit.test if culprit and culprit.test else ""),
            ]
            # The two ceilings this step ran under, each named for the quantity it bounds.
            # Recorded only when they are live, so a disabled budget stays absent rather than
            # reading as 0.
            if cpu_budget > 0:
                fields.append(("cpu_limit_s", str(cpu_budget)))
            if step.timeout > 0:
                fields.append(("wall_limit_s", str(step.timeout)))
            fields.extend(_cpu_journal_fields(cpu_stats))
            self.evidence.record("step_end", fields)

    def result(self) -> RunResult:
        """Assemble the typed :class:`RunResult` after :meth:`run` has returned.

        Outcomes are ordered by the LPT dispatch order for stable, readable reporting;
        ``skipped`` lists tags whose deps failed so they never ran. ``not_launched`` names every
        other configured step with no terminal outcome, so absent work cannot read as passed.
        """
        outcomes = tuple(self.done[tag] for tag in self.order if tag in self.done)
        skipped = tuple(sorted(self._skipped()))
        not_launched = tuple(
            sorted(
                tag
                for tag in self.order
                if tag not in self.done
                and tag not in skipped
                and tag not in self.intentional_skip_tags
            )
        )
        return RunResult(
            ok=not self.failed and not not_launched,
            wall_s=self.wall,
            outcomes=outcomes,
            skipped=skipped,
            not_launched=not_launched,
            intentional_skips=self.intentional_skips,
            step_profile_rows=tuple(self.step_profile_rows),
            run_timed_out=self.run_timed_out,
            max_concurrent_steps=self.max_concurrent_steps,
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

    ``jobs`` remains the compatibility combined limit: absent the compatibility ``core_budget``
    override, it bounds active DAG steps and each individual step's runner-controlled width. New
    callers that need independent values should use :func:`run_dag_limited`. Concurrent steps may
    have widths whose sum exceeds that per-step limit; an independently established outer cgroup is
    what enforces whole-run CPU bandwidth.
    """
    return run_dag_limited(
        cfg,
        max_steps=jobs,
        max_cpus=core_budget if core_budget is not None else jobs,
        cgroups=cgroups,
        metrics=metrics,
        keep_going=keep_going,
        verbosity=verbosity,
        order=order,
        run_timeout_s=run_timeout_s,
    )


@overload
def run_dag_limited(
    cfg: DagConfig,
    *,
    max_steps: int,
    max_cpus: int,
    cpu_jobs: None = None,
    cgroups: CgroupManager | None = None,
    metrics: MetricsSink | None = None,
    keep_going: bool = False,
    verbosity: int = 1,
    order: Sequence[str] | None = None,
    run_timeout_s: int | None = None,
) -> RunResult:
    """Type signature accepting the canonical per-step-width keyword."""

    ...


@overload
def run_dag_limited(
    cfg: DagConfig,
    *,
    max_steps: int,
    max_cpus: int,
    cpu_jobs: int,
    cgroups: CgroupManager | None = None,
    metrics: MetricsSink | None = None,
    keep_going: bool = False,
    verbosity: int = 1,
    order: Sequence[str] | None = None,
    run_timeout_s: int | None = None,
) -> RunResult:
    """Type signature accepting matching canonical and compatibility keywords."""

    ...


@overload
def run_dag_limited(
    cfg: DagConfig,
    *,
    max_steps: int,
    cpu_jobs: int,
    max_cpus: None = None,
    cgroups: CgroupManager | None = None,
    metrics: MetricsSink | None = None,
    keep_going: bool = False,
    verbosity: int = 1,
    order: Sequence[str] | None = None,
    run_timeout_s: int | None = None,
) -> RunResult:
    """Type signature accepting the compatibility per-step-width keyword."""

    ...


def run_dag_limited(
    cfg: DagConfig,
    *,
    max_steps: int,
    max_cpus: int | None = None,
    cpu_jobs: int | None = None,
    cgroups: CgroupManager | None = None,
    metrics: MetricsSink | None = None,
    keep_going: bool = False,
    verbosity: int = 1,
    order: Sequence[str] | None = None,
    run_timeout_s: int | None = None,
) -> RunResult:
    """Run a DAG with independent active-step and per-step CPU-width limits.

    It brackets the run in a :class:`RunWindow` (whole-run metrics), drives the
    :class:`Runner`, flushes the outer-scope cgroup backstop, records the accumulated per-step
    profile rows, and returns the typed result.

    :param cfg: the DAG plus caller policy (steps, resource caps, memory tunables).
    :param max_steps: maximum number of concurrently active DAG steps; clamped to at least 1.
    :param max_cpus: maximum runner-controlled width of any individual step; clamped to at least
        1. Concurrent widths may sum above this value. This library function does not establish an
        outer CPU-bandwidth cgroup; the CLI does, and callers may supply equivalent containment.
    :param cpu_jobs: compatibility alias for ``max_cpus``; if both are passed they must agree.
    :param cgroups: per-step containment manager, or ``None`` for the no-containment path
        (a :class:`_NoopCgroupManager` is substituted; teardown falls back to process-group
        kill). The function does not establish an outer cgroup scope itself. A
        present-but-disabled manager triggers a visible degraded-enforcement warning.
    :param metrics: durable measurement sink, or ``None`` for no recording.
    :param keep_going: after a failure, keep launching independent ready steps and skip only
        true dependents; in-flight steps are left to report their own pass/fail rather than
        ABORTED, so ONE run collects EVERY independent failure.
    :param verbosity: 0 quiet (+failures), 1 default (+summaries), >=2 stream child stdout.
    :param order: explicit dispatch order (e.g. a critical-path planner's); ``None`` uses the
        built-in longest-processing-time default.
    :param run_timeout_s: OUTER wall budget for the WHOLE run. On breach the scheduler stops
        launching, terminates every in-flight step's tree, and RETURNS with
        ``RunResult.run_timed_out`` set — it does not abandon the process to an outside killer,
        because an outside kill takes the evidence with it. ``None`` leaves the run unbounded.
        FAIL CLOSED: if any step may run as long as the whole run, this refuses to start, because
        a bound whose breach cannot be attributed to a node reads like enforcement without being
        usable as one.
    """
    resolved_max_cpus = _resolve_max_cpus_argument(max_cpus, cpu_jobs)
    if error := _self_managed_width_error(cfg, resolved_max_cpus):
        print(
            f"[scheduler] ERROR: REFUSING to run before any node starts: {error}",
            file=sys.stderr,
        )
        return RunResult(ok=False, wall_s=0.0)
    domain_errors = write_domain_violations(cfg)
    if domain_errors:
        print(
            "[scheduler] ERROR: REFUSING to run before any node starts: "
            "write-domain policy violation(s): " + "; ".join(domain_errors),
            file=sys.stderr,
        )
        return RunResult(ok=False, wall_s=0.0)
    undeclared = undeclared_resource_demands(cfg)
    if undeclared:
        # ABSENT IS NOT ZERO. Left to the ready-set loop this is an infinite 50 ms sleep at 0%
        # CPU with nothing printed — indistinguishable from a deliberate cap of 0, and from a
        # hang. Name it here, before anything can wait on it.
        print(
            f"[scheduler] ERROR: REFUSING to run before any node starts: {len(undeclared)} "
            "step/resource pair(s) demand a named resource with NO declared cap in "
            "resource_caps, so they can never become ready: " + "; ".join(undeclared) + ". "
            "Declare the capacity, or set the cap to 0 explicitly to block them on purpose.",
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
        max_steps=max_steps,
        max_cpus=resolved_max_cpus,
        cgroups=manager,
        keep_going=keep_going,
        verbosity=verbosity,
        order=order,
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
    # The persisted ``outer_jobs`` schema retains its historical meaning (active-step ceiling)
    # for compatibility; the independent CPU budget is intentionally not a schema migration.
    window.finish(
        result="pass" if ok else "fail", n_steps=len(runner.done), jobs=max(1, max_steps)
    )
    location = sink.record_step_profiles(result.step_profile_rows, jobs=max(1, max_steps))
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
