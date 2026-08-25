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
import traceback
from collections.abc import Iterator, Mapping, Sequence
from dataclasses import replace
from enum import Enum
from typing import overload

from dagrun.ambient import (
    AmbientSnapshot,
    PsiReading,
    capture_ambient_snapshot,
)
from dagrun.attribution import (
    CAPTURE_TRUNCATION_NOTICE,
    LOG_DIR_ENV,
    NO_LOGS_ENV,
    Culprit,
    RunEvidence,
    StepStream,
    bind_process_tests,
    capture_max_bytes,
    default_log_dir,
    process_snapshot,
    sanitize as sanitize_evidence_tag,
)
from dagrun.capabilities import Lane, is_enforced
from dagrun.model import (
    DagConfig,
    Step,
    command_with_inner_jobs,
    effective_cpu_count,
    effective_cpu_timeout,
    effective_jobs_flag,
    canonical_cpu_timeout,
    resolved_wall_timeout,
    scale_cpu_timeout,
    preferred_inner_jobs,
    step_classification,
    undeclared_resource_demands,
    write_domain_violations,
    env_with_inner_jobs,
    step_width_is_resizable,
    JOBS_ENV_ENV,
)
from dagrun.profile_enrich import (
    resolve_effective_inner_jobs,
    step_enrichment_columns,
)
from dagrun.protocols import (
    CgroupManager,
    MetricsSink,
    RunResult,
    RunWindow,
    StepOutcome,
)
from dagrun.sizing import _step_mem_cap_for_inner_jobs
from dagrun.teardown import (
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

#: Largest console line the ``-vv`` live stream will hold back waiting for a newline.
#:
#: The live-stream buffer is drained only when it CONTAINS a newline, so output with no newline
#: in it grows without limit. This is deliberately a fixed display bound rather than another
#: environment knob: it decides how a line is broken on a console, never what is retained, and
#: the durable log and the in-memory capture are both unaffected by it. MUST match
#: ``STREAM_LINE_MAX_BYTES`` in ``rs/dagrun/src/scheduler.rs``.
_STREAM_LINE_MAX_BYTES = 1024 * 1024


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
#: Rendered for a capacity lookup that found NOTHING. A distinct token, never a value, because
#: ``.get(r, 0)`` FUSES "not declared" with "declared as 0" and that fusion is the whole defect
#: class here: an undeclared ``resource_caps`` entry reads identically to a deliberate
#: zero-capacity bucket, so a config typo and a deliberate serialization are indistinguishable in
#: the diagnostics as well as in the behavior. MUST match the `Observed::Absent` rendering in
#: `rs/dagrun/src/scheduler.rs`.
_ABSENT = "<absent>"


def _ungrantable_resources(
    resource_avail: Mapping[str, int], steps: Mapping[str, Step], tags: Sequence[str]
) -> list[str]:
    """The starved steps whose demand LIVE capacity can never grant, rendered as refusals.

    Each line carries enough to be fixed WITHOUT opening the source: where it was found, what was
    required, what was actually observed (``<absent>`` distinct from a declared number), and the
    surrounding declarations that turn a refusal into a spotted typo.

    Safe to read ``resource_avail`` as DECLARED capacity only because the caller invokes it with
    nothing running: every ``_acquire`` is matched by a ``_release`` when its step completes, so
    with no live step the map has returned to the configured caps. Returns an empty list when the
    starve has some other cause (dangling dep, dependency cycle) -- the detector still refuses;
    this only supplies the named cause when the cause is capacity.

    Reports EVERY violation rather than the first: a first-failure abort makes the reader iterate
    N times for N typos, and the count is itself evidence of how wide the misdeclaration is.

    MUST stay behaviorally identical to ``ungrantable_resources`` in
    ``rs/dagrun/src/scheduler.rs``, down to the rendered text: a refusal a reader
    compares across the two editions must read the same in both.
    """
    declared = ", ".join(sorted(f"{k}={v}" for k, v in resource_avail.items()))
    out: list[str] = []
    for tag in tags:
        step = steps.get(tag)
        if step is None:
            continue
        for r, n in sorted(step.hint.resources.items()):
            cap = resource_avail.get(r)
            if (cap if cap is not None else 0) < n:
                observed = _ABSENT if cap is None else f"{r}={cap}"
                line = f'step "{tag}": requires {r}={n}, but got {observed}'
                out.append(f"{line} (declared: {declared})" if declared else line)
    return out


def _psi_reading(pressure: Mapping[str, float] | None) -> PsiReading | None:
    """Adapt a cgroup ``cpu.pressure`` ``{avg10, avg60}`` mapping to a typed :class:`PsiReading`
    for the enrichment builder; ``None`` (unreadable / unboxed) passes straight through."""
    if pressure is None:
        return None
    return PsiReading(avg10=pressure["avg10"], avg60=pressure["avg60"])


def uncontained_cpu_budget_warning(cfg: DagConfig) -> str | None:
    """The one line an UNCONTAINED run owes its operator about per-step CPU-time budgets.

    The CPU guard is implemented by reading the step cgroup's ``cpu.stat``. With no cgroup there
    is no counter, so the budget is not merely approximate — it does not run at all, and a step
    that burns unbounded CPU against a declared 3-second budget exits 0 and is reported green.
    That silence is the defect: ``--allow-cgroup-failure`` is an explicit choice, but nothing said
    which guards the choice gave up, and the capability manifest asserted the opposite.

    Returns ``None`` when no step carries a live budget, so a graph that has genuinely disabled
    the guard everywhere is not nagged about a bound it never asked for.
    """
    live = [
        budget
        for step in cfg.steps
        if (
            budget := effective_cpu_timeout(
                step, cfg.default_step_cpu_timeout, cfg.cpu_timeout_multiplier
            )
        )
        > 0
    ]
    if not live:
        return None
    return (
        "UNCONTAINED run: cpu.stat is unreadable without a cgroup, so the per-step CPU-time "
        f"budget is NOT enforced; {len(live)} step(s) carry one (largest {max(live)}s) and are "
        "bounded only by their wall timeout. `capabilities` says which guards hold on this lane."
    )


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
            and not step_width_is_resizable(step, cfg.default_jobs_flag, cfg.default_jobs_env)
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
        f"--max-cpus {max(1, max_cpus)} cannot lower guest parallelism for step(s) that offer "
        f"no width channel: {detail}; this machine must declare one -- set "
        f"${JOBS_ENV_ENV} to the guest's worker-count ENV VAR (e.g. CARGO_BUILD_JOBS), or set "
        "the step's jobs_flag to its worker-count OPTION -- or reduce preferred_inner_jobs, or "
        "raise --max-cpus"
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
    teardown falls back to the process-group kill in :func:`dagrun.teardown.reap`
    — observationally identical to the originating pipeline's "no cgroups" path.
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

    def memory_events(self, tag: str) -> Mapping[str, int] | None:
        return None

    def applied_memory_max(self, tag: str) -> str | None:
        # None (cap UNKNOWN), never "max": nothing was contained here, so nothing is
        # known about what bounded the step.
        return None

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
        # Tags whose Popen lifetime has been counted into ``active_processes`` and not yet
        # uncounted. A supervisor that dies between the two would otherwise leave the count
        # permanently inflated, and ``max_concurrent_steps`` is a max over it.
        self.counted_processes: set[str] = set()
        # Tags whose admission-time accounting (named resources, cores_used, running/procs/nonces)
        # has already been handed back. See :meth:`_retire`.
        self.retired: set[str] = set()
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
        # Monotonic origin every profiled step measures its start/finish offset from, so two
        # rows of one run can be tested for OVERLAP. Set here so a step supervised without a
        # preceding :meth:`run` still has an origin, and re-set at the top of :meth:`run` so
        # the offsets are measured from the execution, not from construction. Monotonic, not
        # wall clock: a clock step mid-run must not make one step appear to precede another.
        self.run_origin_monotonic = time.monotonic()

    # -- gating helpers (mirror validate.py Runner._deps_ok / _deps_known / _res_free) --

    def _deps_ok(self, step: Step) -> bool:
        """True when EVERY dep has a done, successful outcome."""
        return all(d in self.done and self.done[d].ok for d in step.deps)

    def _deps_known(self, step: Step) -> bool:
        """True when every dep has reached a terminal outcome (regardless of success)."""
        return all(d in self.done for d in step.deps)

    def _res_free(self, step: Step) -> bool:
        """True when the step's scarce-resource demand currently fits remaining capacity.

        An ABSENT cap counts as 0, i.e. never schedulable, deliberately: silently granting
        unlimited capacity to an undeclared resource would turn a config typo into an unbounded
        fan-out. The two cases are identical HERE (both block) but must never be identical in the
        diagnostics -- see :func:`_ungrantable_resources`, which renders ``<absent>`` distinctly so
        the reader can tell "you forgot to declare it" from "you set it to zero on purpose".
        """
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

    def _uncount_process(self, tag: str) -> None:
        """Stop counting ``tag``'s child toward :attr:`active_processes`, AT MOST ONCE.

        Caller holds :attr:`lock`. Idempotent for the same reason :meth:`_retire` is: the normal
        path uncounts as soon as ``proc.wait()`` returns, well before the step retires, so a
        supervisor that dies in between must be able to uncount without double-counting a step
        that already did.
        """
        if tag in self.counted_processes:
            self.counted_processes.discard(tag)
            self.active_processes -= 1

    def _retire(self, step: Step) -> bool:
        """Hand back everything ``step``'s admission took, EXACTLY ONCE. Caller holds the lock.

        Returns ``True`` when this call did the work, ``False`` when the step was already retired.

        The once-only guard is not defensive tidiness. Every release site (spawn failure, normal
        completion, and the supervisor-crash paths added for #80 runner-supervisor-crash-loud)
        gives back the same named-resource counts and the same ``cores_used`` width, and a crash
        landing AFTER a normal release would otherwise release a second time. That drifts
        ``resource_avail`` ABOVE its declared cap, which is worse than the leak it looks like: the
        cap silently stops being a cap, and the next run's over-admission has no visible cause.
        """
        if step.tag in self.retired:
            return False
        self.retired.add(step.tag)
        self.running.discard(step.tag)
        self.running_procs.pop(step.tag, None)
        self.running_nonces.pop(step.tag, None)
        self._uncount_process(step.tag)
        self._release(step)
        self.cores_used -= self._step_width(step)
        return True

    def _trip_fail_fast(self) -> None:
        """Record the run as failed and, unless ``keep_going``, cut every in-flight peer short.

        Caller holds :attr:`lock`. Shared by the ordinary step-failure path, the spawn-failure
        path and the supervisor-crash paths so all three cancel peers identically.
        """
        self.failed = True
        if self.keep_going:
            return
        self.stop = True
        # A node that exists to EXPLAIN this failure is spared. Reaping it destroys the only
        # account of why the run failed, which is the opposite of what eager-exit is for: the
        # point is to stop paying for work that cannot matter, and the diagnosis is the one piece
        # of remaining work that matters most. Everything else is still cut short.
        failed = self._genuinely_failed()
        spared = {tag for tag in self.running if self._exempt_from_eager_exit(tag, failed)}
        others = [
            (tag, proc) for tag, proc in self.running_procs.items() if tag not in spared
        ]
        for other in self.running:
            if other in spared:
                continue
            self.aborted.add(other)  # its thread will label itself ABORTED
        reap_many(
            tuple(
                (other_proc, other, self.running_nonces.get(other))
                for other, other_proc in others
            ),
            self.cgroups,
        )

    def _genuinely_failed(self) -> set[str]:
        """Tags whose own run FAILED, excluding steps that were merely cancelled.

        An aborted step is not evidence of anything, so it must not trigger a diagnostic's
        exemption; only a real failure does. Caller holds :attr:`lock`.
        """
        return {
            tag
            for tag, outcome in self.done.items()
            if not outcome.ok and not outcome.aborted
        }

    def _exempt_from_eager_exit(self, tag: str, failed: set[str]) -> bool:
        """Whether ``tag`` is a diagnostic for one of the failures in ``failed``.

        This is the ONLY thing that survives eager-exit, and it is deliberately conditional: a
        step declaring ``explains`` is reaped like any other peer unless one of the specific
        nodes it names has genuinely failed. So the exemption cannot be used as a blanket
        opt-out -- a step that explains nothing about THIS failure gets no protection from it.
        """
        step = self.steps.get(tag)
        return step is not None and step.explains_a_failure_in(failed)

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

    def _sweep_dead_supervisors(
        self, threads: Sequence[tuple[threading.Thread, Step]]
    ) -> None:
        """Give an outcome to every supervisor thread that ENDED without publishing one.

        LAYER TWO of the two guards behind #80 runner-supervisor-crash-loud. Layer one — the
        ``except BaseException`` in :meth:`_run_step` — cannot cover a failure of layer one
        itself, and a wedge is not an acceptable second-order failure mode.

        THE KEY IS (launched) AND (finished) AND (no terminal outcome), and it is deliberately NOT
        "the tag is still in :attr:`running`". :meth:`_retire` removes the tag from ``running``
        BEFORE ``done`` is written, so a supervisor that dies between those two lines is in
        NEITHER set. A running-keyed sweep is invisible to exactly that window — which is the
        window a crash in the outcome-construction code lands in. That distinction was found by
        mutation, not by reasoning, and it is the detail most easily lost in a rewrite.

        A finished thread that DID publish is not touched, so this never contradicts a real
        result. Nothing here uses :meth:`_emit` (which takes :attr:`lock`); it prints directly,
        for the same reason :meth:`_publish_supervisor_failure` does.
        """
        vanished: list[Step] = []
        with self.lock:
            for thread, step in threads:
                if thread.is_alive() or step.tag in self.done:
                    continue
                vanished.append(step)
        for step in vanished:
            reason = (
                "SUPERVISOR VANISHED (its thread ended without publishing an outcome and without "
                "a recorded traceback; the step's real result is UNKNOWN)"
            )
            if self._publish_supervisor_failure(
                step,
                reason=reason,
                summary="supervisor thread ended without publishing an outcome",
                duration_s=0.0,
            ):
                message = (
                    f"[scheduler] ✗ SUPERVISOR VANISHED for step {step.tag!r}: its thread is no "
                    "longer alive and it never recorded a terminal outcome. Reporting the step as "
                    "FAILED so the run terminates instead of waiting for a thread that is already "
                    "gone. This is a runner bug; the step's own result is UNKNOWN."
                )
                print(message)
                sys.stdout.flush()
                print(message, file=sys.stderr)
                sys.stderr.flush()

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
        threads: list[tuple[threading.Thread, Step]] = []
        wall_start = time.time()
        self.run_origin_monotonic = time.monotonic()
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
            self._sweep_dead_supervisors(threads)
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
                # After eager-exit has tripped, ONE class of step may still start: a diagnostic
                # for a failure that has actually happened. Sparing an already-running diagnostic
                # is not enough -- the measured case had the diagnostic still QUEUED when its
                # subject failed, so it was never launched at all and the run reported the
                # symptom with no account of the cause.
                failed_now = self._genuinely_failed() if self.stop else set()

                def _startable_after_stop(tag: str) -> bool:
                    if not self.stop:
                        return True
                    return self._exempt_from_eager_exit(tag, failed_now)

                def _pending(tag: str) -> bool:
                    return (
                        tag not in self.done
                        and tag not in self.running
                        and tag not in skipped
                        and tag not in self.intentional_skip_tags
                    )

                # Do not end the run while a permitted diagnostic is still waiting to start.
                # Terminates: the set is finite, each member runs at most once, and a member
                # whose deps can never be satisfied is excluded here rather than waited on.
                pending_diagnostics = [
                    tag
                    for tag in self.order
                    if self.stop
                    and _pending(tag)
                    and _startable_after_stop(tag)
                    and self._deps_known(self.steps[tag])
                    and self._deps_ok(self.steps[tag])
                ]
                if (
                    not self.running
                    and not pending_diagnostics
                    and (
                        self.stop
                        or len(self.done) + len(skipped) + len(self.intentional_skips)
                        >= len(self.steps)
                    )
                ):
                    break
                launchable: list[Step] = []
                if True:
                    for tag in self.order:
                        if (
                            tag in self.done
                            or tag in self.running
                            or tag in skipped
                            or tag in self.intentional_skip_tags
                        ):
                            continue
                        if not _startable_after_stop(tag):
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
                    # TERMINAL STARVE. Nothing is launchable, nothing is running, and work
                    # remains: no future event can change that, because every state transition in
                    # this loop is caused by a running step completing. Sleeping here is what
                    # turned three distinct defects -- an unsatisfiable resource cap, a dangling
                    # dep, and a dependency cycle -- into one indistinguishable symptom: a live
                    # process at 0% CPU with a frozen log and no exit.
                    #
                    # SOUNDNESS: the --max-steps cap cannot be the cause of an empty `launchable`
                    # here. `len(self.running) >= self.max_steps` can only break the scan while
                    # `running` is NON-empty (max_steps is max(1, ...) in __init__, so it is >= 1).
                    # And with nothing running, no supervisor thread can be mutating `done` or
                    # `resource_avail`, so the counts read below are stable rather than merely
                    # sampled, and `resource_avail` has returned to the configured caps.
                    accounted = (
                        len(self.done) + len(skipped) + len(self.intentional_skips)
                    )
                    remaining = max(0, len(self.steps) - accounted)
                    if not launchable and not self.running and remaining > 0:
                        stuck = sorted(
                            tag
                            for tag in self.order
                            if tag not in self.done
                            and tag not in skipped
                            and tag not in self.intentional_skip_tags
                        )
                        print(
                            f"[scheduler] REFUSED: terminal starve -- {remaining} step(s) can "
                            "never be admitted; nothing is running and nothing is launchable, so "
                            "no future event can unblock them.",
                            file=sys.stderr,
                        )
                        for refusal in _ungrantable_resources(
                            self.resource_avail, self.steps, stuck
                        ):
                            print(f"[scheduler]   {refusal}", file=sys.stderr)
                        print(
                            f"[scheduler]   starved step(s) ({len(stuck)}): "
                            + ", ".join(stuck),
                            file=sys.stderr,
                        )
                        if self.evidence is not None:
                            self.evidence.record(
                                "terminal_starve",
                                [
                                    ("starved", str(len(stuck))),
                                    ("steps", ",".join(stuck)),
                                ],
                            )
                        self.failed = True
                        self.stop = True
                        break
                for step in launchable:
                    t = threading.Thread(
                        target=self._run_step, args=(step,), daemon=True
                    )
                    t.start()
                    threads.append((t, step))
            time.sleep(_LOOP_SLEEP_S)
        for t, _step in threads:
            t.join()
        self.wall = time.time() - wall_start
        return not self.failed

    def _publish_supervisor_failure(
        self, step: Step, *, reason: str, summary: str, duration_s: float
    ) -> bool:
        """Give a step whose supervisor died a TERMINAL outcome, so the run can finish.

        Returns ``True`` when this call published the outcome, ``False`` when the step already had
        one (a crash in the reporting tail, after ``done`` was written, is a real bug but not a
        wedge — the run can still terminate and must not be told the step failed twice).

        Deliberately does NOT use :meth:`_emit`: ``_emit`` takes :attr:`lock`, and the whole point
        of this path is to survive an exception raised from anywhere, including code that was
        about to write a status line.
        """
        with self.lock:
            if step.tag in self.done:
                return False
            self._retire(step)
            self.done[step.tag] = StepOutcome(
                tag=step.tag,
                ok=False,
                duration_s=duration_s,
                summary=summary,
                returncode=None,
                reason=reason,
                aborted=False,
                pids_events=0,
            )
            self._trip_fail_fast()
        if self.evidence is not None:
            self.evidence.record(
                "supervisor_crash",
                [
                    ("step", step.tag),
                    ("reason", reason),
                    ("elapsed_s", f"{duration_s:.3f}"),
                ],
            )
        return True

    def _run_step(self, step: Step) -> None:
        """Supervise ONE step, converting ANY escaping exception into a NAMED failure.

        LAYER ONE of the two guards behind #80 runner-supervisor-crash-loud. Before it existed,
        exactly one failure mode inside the supervisor was handled (``Popen`` raising), and every
        other exception escaped this thread: the tag stayed in :attr:`running` with nothing in
        :attr:`done`, so the ready-set loop in :meth:`run` could never reach its break condition.
        The run then produced no outcome, no exit and no visible traceback — a wedge that looks
        exactly like work in progress, which is the worst thing this tool can do.

        ``BaseException`` is deliberate and is NOT over-broad here. A bare ``SystemExit`` or
        ``KeyboardInterrupt`` raised on a worker thread ends that thread just as silently as a
        ``TypeError`` does, and silence is the defect. The exception is re-reported, never
        swallowed: the traceback goes to BOTH stdout (where the step's own output is) and stderr
        (where a CI system looks), and the step's outcome NAMES the exception.
        """
        started = time.time()
        try:
            self._supervise_step(step)
        except BaseException as exc:  # noqa: BLE001 - see the docstring; silence is the defect
            detail = f"{type(exc).__name__}: {exc}"
            trace = traceback.format_exc()
            header = (
                f"[{step.tag}] ✗ SUPERVISOR CRASHED: the supervisor thread for this step raised "
                f"{detail}. The step's own result is UNKNOWN; it is being reported as FAILED so "
                "the run cannot wedge. This is a runner bug, not a step failure."
            )
            print(header)
            print(trace)
            sys.stdout.flush()
            print(header, file=sys.stderr)
            print(trace, file=sys.stderr)
            sys.stderr.flush()
            self._publish_supervisor_failure(
                step,
                reason=f"SUPERVISOR CRASHED ({detail})",
                summary=detail,
                duration_s=time.time() - started,
            )

    def _supervise_step(self, step: Step) -> None:
        """Supervise ONE step: launch, pump stdout, enforce the timeout, reap, classify."""
        self._emit(f"[{step.tag}] ▶ START  {step.desc}")
        env = dict(os.environ)
        env.update(step.env)
        nonce = mint_step_nonce()
        # Runner authority wins over a DAG-supplied environment value.
        env[STEP_NONCE_ENV] = nonce
        inner_jobs = preferred_inner_jobs(step)
        # Deliver the width through this machine's env channel when it has one. Empty overlay
        # when the host configured none, so behaviour is unchanged where nothing is set.
        env.update(env_with_inner_jobs(step, self.cfg.default_jobs_env, inner_jobs))
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
        # The wall ceiling this step actually runs under. A step that declared none carries the
        # 0 sentinel and gets a backstop derived from its own CPU budget instead of the graph's
        # baked-in 1800 (see resolved_wall_timeout).
        wall_budget = resolved_wall_timeout(
            step, self.cfg.default_step_timeout, self.cfg.cpu_timeout_multiplier
        )
        start = time.time()
        started_offset_s = time.monotonic() - self.run_origin_monotonic
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
        # Parallel-speedup ENRICHMENT capture (only under real cgroup boxing, as in the original).
        # prepare_command has already created the step's child cgroup, so cpu.pressure is readable;
        # bracket the step with two host-load snapshots so contention can be attributed later.
        boxed = self.cgroups.enabled
        # WHICH LANE THIS STEP IS ON. Every guard below asks the capability registry about
        # THIS lane rather than about the engine in the abstract, so the `uncontained` column
        # of the published manifest is load-bearing in exactly the way the `contained` one
        # is: an unboxed run that advertises no CPU-time enforcement does not perform any.
        lane = Lane.CONTAINED if boxed else Lane.UNCONTAINED
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
                timeout=wall_budget,
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
                self._retire(step)
                self.done[step.tag] = outcome
                self._trip_fail_fast()
            self._emit(f"[{step.tag}] ✗ FAIL   {step.desc} ({summary})")
            return
        with self.lock:
            self.running_procs[step.tag] = proc
            self.running_nonces[step.tag] = nonce
            self.active_processes += 1
            self.counted_processes.add(step.tag)
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
                    ("timeout_s", str(wall_budget)),
                    ("cmd", run_cmd),
                ],
            )
        captured = _BoundedCapture(capture_max_bytes())

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
                    captured.feed(raw)
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
                        # THE SECOND UNBOUNDED BUFFER. `pending` is drained only while it CONTAINS
                        # a newline, so newline-free output grows it without limit -- on exactly
                        # the path that streams a runaway to a human. Bounding the capture alone
                        # would have left this hole open. Flush the oversized prefix as its own
                        # console line: a console line is a display artifact, and splitting one is
                        # a far smaller cost than holding an unbounded one in memory.
                        while len(pending) >= _STREAM_LINE_MAX_BYTES:
                            forced = bytes(pending[:_STREAM_LINE_MAX_BYTES])
                            del pending[:_STREAM_LINE_MAX_BYTES]
                            self._emit(
                                f"[{step.tag}] " + forced.decode(errors="replace")
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
            # THE MONITOR IS THE ONLY ENFORCER of the per-step CPU-time budget. If it dies, the
            # budget is not merely unmeasured — it stops being enforced at all, and nothing else
            # notices, because the supervisor never joins it for a result. Say so out loud rather
            # than leaving a switched-off guard that still reads as configured. (#80
            # runner-supervisor-crash-loud)
            try:
                _monitor_body()
            except BaseException as exc:  # noqa: BLE001 - a dead enforcer must not be silent
                _warn(
                    f"step {step.tag!r}: the CPU-budget monitor thread DIED with "
                    f"{type(exc).__name__}: {exc}. The {cpu_budget}s CPU-time budget is NO LONGER "
                    "ENFORCED for this step; only the wall timeout still applies."
                )
                print(traceback.format_exc(), file=sys.stderr)

        def _monitor_body() -> None:
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
                # The `cpu_timeout` guard the `capabilities` manifest advertises IS this
                # branch, so it asks the registry that publishes the claim (capabilities.py).
                # An engine that stops reaping here stops advertising it, in the same edit.
                # On the UNCONTAINED lane the registry says false and the branch is skipped
                # outright: there is no cpu.stat to read, which is the whole of #75.
                if cpu_budget > 0 and not cpu_timed_out and is_enforced("cpu_timeout", lane):
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
            # `wall_timeout` likewise: no deadline is passed at all when the registry says this
            # engine does not enforce one on this lane, so the advertisement and the wait agree
            # by construction rather than by two people remembering. The deadline itself is the
            # DERIVED backstop, not the graph's raw `timeout`.
            proc.wait(timeout=wall_budget if is_enforced("wall_timeout", lane) else None)
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
                limit_s=wall_budget,
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
            self._uncount_process(step.tag)
        monitor_stop.set()
        monitor.join(timeout=_THREAD_JOIN_S)
        # The daemon reader may still be blocked on an orphan-held pipe; because it is a
        # daemon and we never block on it, the step completes regardless. Brief join, move on.
        reader.join(timeout=_THREAD_JOIN_S)
        # Reap the whole process group so any orphan grandchildren are SIGKILLed now instead
        # of leaking into later steps (and this lets the abandoned reader finally see EOF).
        reap(proc, self.cgroups, step.tag, nonce=nonce)

        # Read the step's cgroup measurements BEFORE cleanup() rmdirs the child cgroup.
        # memory_events is read once and oom is taken from it, so the OOM count and the
        # recorded event counters can never disagree about the same step.
        memory_events = self.cgroups.memory_events(step.tag)
        # `oom_detection` is the ATTRIBUTION, and it is what the `capabilities` manifest
        # advertises: unenforced means the oom_kill counter is not consulted, so nothing
        # downstream can call a failure an OOM. The rest of memory.events is a recorded
        # measurement, not a guard, and is kept either way -- a profile that stops recording is
        # a different loss from a guard that stops guarding.
        oom = (
            0
            if memory_events is None or not is_enforced("oom_detection", lane)
            else memory_events.get("oom_kill", 0)
        )
        # The cap the KERNEL held, not the cap that was requested: a peak is only
        # interpretable against the ceiling that was actually in force.
        applied_memory_max = self.cgroups.applied_memory_max(step.tag)
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
        summary = captured.last_line()
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
            # Run-overlap + applied-cap provenance. Offsets share one monotonic run origin, so
            # two rows of the same run_id overlap iff their [started, finished] intervals do.
            "started_offset_s": round(started_offset_s, 3),
            "finished_offset_s": round(
                time.monotonic() - self.run_origin_monotonic, 3
            ),
            # Blank means UNKNOWN and the literal "max" means unbounded; they are not the
            # same answer and the writer must not flatten one into the other.
            "memory_max_bytes": "" if applied_memory_max is None else applied_memory_max,
        }
        # memory.events counters, which need no subtraction to be per-step deltas (the child
        # cgroup lives exactly as long as the step). Left blank wholesale when the file could
        # not be read, so "the step had no such event" and "we never looked" stay distinct.
        for counter in ("low", "high", "max", "oom", "oom_kill"):
            row[f"memory_events_{counter}"] = (
                "" if memory_events is None else memory_events.get(counter, 0)
            )
        # NOTE: pids_events is deliberately NOT a CSV column — it rides on the in-memory
        # StepOutcome instead (see StepOutcome.pids_events), so the Python/Rust CSV-header
        # differential stays byte-identical while a PID-cap breach is still classifiable.
        if cpu_stats is not None:
            for key, value in cpu_stats.items():
                row[f"cpu.{key}"] = value
        # Rich parallel-speedup enrichment columns (effective_cores, throttled_s, contention, PSI).
        # Only under real boxing; an un-boxed run leaves them blank (the writer fills them from the
        # STEP_PROFILE_COLUMNS schema), matching the originating "blank when unavailable" posture.
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
            self._retire(step)
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
                    timeout=wall_budget,
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
                self._trip_fail_fast()

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
            # Self-contained failure: dump the captured child output, tagged. The dump is a TAIL
            # (see :class:`_BoundedCapture`), so when anything was dropped it says so IN BAND and
            # in numbers, rather than presenting a partial dump as if it were the whole output.
            self._emit(f"[{step.tag}] ----- detail -----")
            if captured.dropped:
                self._emit(
                    f"[{step.tag}] "
                    + CAPTURE_TRUNCATION_NOTICE.format(
                        total=captured.total, kept=captured.kept
                    )
                )
            for line in captured.iter_lines():
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
            if wall_budget > 0:
                fields.append(("wall_limit_s", str(wall_budget)))
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
    # The RESOLVED bound, not the declared field. A step that declares no wall budget carries the
    # 0 sentinel, which would pass a `>= run_timeout_s` test trivially while the value it will
    # actually run under — derived from its CPU budget, or 1800 — might not.
    resolved = (
        (s.tag, resolved_wall_timeout(s, cfg.default_step_timeout, cfg.cpu_timeout_multiplier))
        for s in cfg.steps
    )
    return sorted((tag, bound) for tag, bound in resolved if bound >= run_timeout_s)


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
    if not manager.enabled:
        # NAME THE GUARD THAT IS NOT RUNNING, not just the containment state. Covers every
        # uncontained lane at once — no manager at all, or a present-but-disabled one — because
        # both read `cpu.stat` exactly zero times.
        notice = uncontained_cpu_budget_warning(cfg)
        if notice is not None:
            _warn(notice)

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


class _BoundedCapture:
    """The last ``limit`` bytes of a step's output, in a buffer that never grows past ``limit``.

    WHAT THIS REPLACES, and why the shape matters. The capture used to be a ``list[bytes]`` with
    one unconditional append per 8 KiB read, so a step held its ENTIRE output in the runner's RSS
    for the step's whole lifetime, and the failure path's ``b"".join(...)`` doubled that peak at
    exactly the wrong moment. A runaway step OOM-killed the RUNNER — taking the run's verdict, its
    profile rows and its evidence with it — before it could fill the disk that
    :data:`~dagrun.attribution.DEFAULT_LOG_MAX_BYTES` was protecting.

    A PREALLOCATED RING, not a deque of chunks and not a growable ``bytearray``, and that is a
    measured choice rather than a stylistic one:

    * a deque of chunks bounded by total bytes still holds whole chunks, so its peak depends on
      the step's write sizes rather than on the ceiling, and the join to serve the tail costs the
      whole content again;
    * ``bytearray.__iadd__`` keeps amortised growth headroom, so a bytearray trimmed to the
      ceiling still peaks meaningfully above it;
    * a ring allocated once at exactly ``limit`` bytes has a steady-state cost of exactly
      ``limit``, independent of both the step's output size and its chunk sizes. The only
      transient above that is the single tail copy a failure dump needs, giving ``2L + O(1)``.

    ``limit=None`` means unlimited and is the explicit opt-out, not a fallback.
    """

    __slots__ = ("_buf", "_limit", "_pos", "_wrapped", "total")

    def __init__(self, limit: int | None) -> None:
        self._limit = limit
        self._buf = bytearray() if limit is None else bytearray(limit)
        self._pos = 0
        self._wrapped = False
        #: EVERY byte the step produced, including the ones dropped. Kept because "you are seeing
        #: a tail" is only actionable next to how much tail there was.
        self.total = 0

    def feed(self, chunk: bytes) -> None:
        """Absorb one read, in O(len(chunk)) and with no allocation proportional to ``total``."""
        self.total += len(chunk)
        limit = self._limit
        if limit is None:
            self._buf.extend(chunk)
            return
        if limit == 0:
            return
        if len(chunk) >= limit:
            # This read alone overruns the ring: only its own tail can survive, so write that
            # and reset the cursor rather than walking the ring len(chunk) times.
            self._buf[:] = chunk[len(chunk) - limit :]
            self._pos = 0
            self._wrapped = True
            return
        head = limit - self._pos
        if len(chunk) <= head:
            self._buf[self._pos : self._pos + len(chunk)] = chunk
            self._pos += len(chunk)
            if self._pos == limit:
                self._pos = 0
                self._wrapped = True
        else:
            self._buf[self._pos :] = chunk[:head]
            rest = len(chunk) - head
            self._buf[:rest] = chunk[head:]
            self._pos = rest
            self._wrapped = True

    @property
    def dropped(self) -> bool:
        """True when output was discarded, i.e. what remains is a TAIL and not the whole thing."""
        return self.kept < self.total

    @property
    def kept(self) -> int:
        """How many bytes the ring is currently holding."""
        if self._limit is None:
            return len(self._buf)
        return self._limit if self._wrapped else self._pos

    def tail(self) -> bytes:
        """The retained bytes, oldest first. The ONE allocation proportional to the ceiling."""
        if self._limit is None:
            return bytes(self._buf)
        if not self._wrapped:
            return bytes(self._buf[: self._pos])
        return bytes(self._buf[self._pos :]) + bytes(self._buf[: self._pos])

    def last_line(self) -> str:
        """Best-effort one-line summary: the last non-empty line of the retained output.

        A step whose output exceeded the ceiling still has its last line, because the ring keeps
        the TAIL — which is exactly why a tail is the right thing to keep.
        """
        buf = self.tail()
        end = len(buf)
        while end > 0:
            start = buf.rfind(b"\n", 0, end)
            text = buf[start + 1 : end].decode(errors="replace").strip()
            if text:
                return text
            if start < 0:
                break
            end = start
        return ""

    def iter_lines(self) -> Iterator[str]:
        """Decode the retained bytes into lines INCREMENTALLY, one line at a time.

        ``b"".join(chunks).decode().splitlines()`` materialised the whole decoded text AND a list
        object per line on top of the capture that was already in memory — for the same input,
        several times the size of the thing being bounded. Bounding the capture and then blowing
        the budget rendering it would have moved the peak rather than removed it. Yielding keeps
        the transient at one line.
        """
        buf = self.tail()
        start = 0
        length = len(buf)
        while start < length:
            end = buf.find(b"\n", start)
            if end == -1:
                yield buf[start:].decode(errors="replace")
                return
            yield buf[start:end].decode(errors="replace").rstrip("\r")
            start = end + 1


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
