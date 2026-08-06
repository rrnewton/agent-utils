"""Core DAG configuration, step, resource, and result types.

The module contains pure data and helpers; callers provide the graph and its resource
hints, then pass a :class:`DagConfig` to the scheduler.
"""

from __future__ import annotations

import math
import os
import signal
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from enum import Enum

DEFAULT_STEP_TIMEOUT = 1800

#: Deliberately SMALL default caps for a step that DECLARES NOTHING — the "forcing function".
#: An undeclared step is boxed into a tight 1-core / 1-GiB / 10-s-CPU floor, so a real step
#: immediately hits the cap and must DECLARE its true needs. That generates per-node resource
#: metadata EMPIRICALLY (from measured breaches) instead of by guessing. Each default applies
#: ONLY when the step leaves the matching hint unset; an explicit hint always wins. They are
#: DagConfig fields (below), so any caller can override or disable a dimension.
DEFAULT_SMALL_MEM_CAP_BYTES = 1024**3  # 1 GiB inner memory.max when no memory hint is declared
DEFAULT_SMALL_CPU_COUNT = 1  # 1-core cpu.max when no inner-parallelism width is declared
DEFAULT_SMALL_CPU_TIMEOUT = 10  # 10 s CPU-time budget when cpu_timeout is unset

#: Per-platform CPU-budget multiplier, applied at EXECUTION time to whatever CPU budget is in
#: effect for a step. A CPU second is load-immune (wall = cpu_busy + wait; contention inflates
#: only wait) but it is NOT clock-immune: a slower core retires the same instruction stream over
#: more seconds of CPU occupancy, so identical work legitimately burns more CPU-seconds on an
#: underpowered runner. A graph therefore carries ONE canonical `cpu_timeout` per step and the
#: platform scales it here.
#:
#: Applying it at execution — rather than baking a second column of pre-multiplied numbers into
#: the graph — is the whole point: two independently-maintained timeout tables drift, and a step
#: has only one `cpu_timeout` field, so a per-platform column would force declaration authors to
#: pick a single number that is either too tight for the slow platform or too loose for the fast
#: one (hiding the very hangs the budget exists to catch).
#:
#: 1.0 is a strict no-op: unset, every platform enforces the canonical budget exactly as before.
#: A platform opts in explicitly (`--cpu-timeout-multiplier`, or
#: $SAFE_CI_DAG_RUNNER_CPU_TIMEOUT_MULTIPLIER in a lane's environment), and every breach message
#: then states the canonical budget, the multiplier and the platform label, so a kill stays
#: attributable to a specific policy rather than to an anonymous number.
DEFAULT_CPU_TIMEOUT_MULTIPLIER = 1.0

#: Environment override for :data:`DEFAULT_CPU_TIMEOUT_MULTIPLIER`, so a CI lane can set the
#: policy once for its whole platform without threading a flag through every invocation.
CPU_TIMEOUT_MULTIPLIER_ENV = "SAFE_CI_DAG_RUNNER_CPU_TIMEOUT_MULTIPLIER"

#: Companion label naming the platform the multiplier describes. Free-form (e.g.
#: "github-hosted"); it appears verbatim in the breach message so the reader can find the lane
#: that set it. Empty when the multiplier is 1.0 or the caller supplied no label.
CPU_TIMEOUT_PLATFORM_ENV = "SAFE_CI_DAG_RUNNER_CPU_TIMEOUT_PLATFORM"

#: Default template for the inner-parallelism (concurrency) flag appended to a step's command
#: when the step declares ``preferred_inner_jobs``. See :func:`render_jobs_flag`.
DEFAULT_JOBS_FLAG = "-j"


class StepClass(Enum):
    """How a step uses the machine, used for scheduling decisions."""

    CPU_BOUND = "cpu-bound"
    LATENCY_BOUND = "latency-bound"
    LIGHT = "light"


class IntentionalSkipReason(Enum):
    """Closed vocabulary for nodes deliberately omitted before process spawn."""

    EMPTY_MANIFEST_BUCKET = "empty-manifest-bucket"


@dataclass(frozen=True)
class ResourceHint:
    """Optional per-step resource demand, duration, parallelism, and memory hints.

    Estimates enable memory-aware concurrency and longest-processing-time dispatch;
    scarce-resource demands constrain which steps may run together.
    """

    # Scarce-resource DEMAND for this step, e.g. {"browser": 1}. The runner never lets the
    # summed demand of concurrently-running steps exceed DagConfig.resource_caps.
    resources: Mapping[str, int] = field(default_factory=dict)
    # Estimated wall-clock seconds, used only to order ready steps (longest first). 0 sorts
    # last; stale values only mildly degrade packing and are never a correctness contract.
    est_duration_s: float = 0.0
    # Estimated peak resident memory (bytes). None excludes the step from the memory model.
    rss_baseline_bytes: int | None = None
    # Explicit hard per-step memory cap (bytes); overrides the derived cap when set.
    hard_mem_max_bytes: int | None = None
    classification: StepClass = StepClass.LIGHT
    # Internal parallelism width for the step's own command (e.g. a build's -j). None means
    # "not measured/declared".
    preferred_inner_jobs: int | None = None
    measured_effective_cores: float | None = None
    measured_cpu_utilization: float | None = None


@dataclass
class Step:
    """One node in the DAG: a shell command plus its dependencies and resource hint."""

    group: str
    job: str
    desc: str
    cmd: str  # shell command (bash -c), run from the run's working directory
    # Optional long-form documentation for this node (default empty). Unlike `desc` — a short
    # label shown by `list`/`run` — `description` is free-form prose (often multi-line, e.g. a
    # YAML block scalar) that documents WHY the step exists. It never affects scheduling.
    description: str = ""
    deps: list[str] = field(default_factory=list)  # tags ("group.job") this step depends on
    env: dict[str, str] = field(default_factory=dict)
    hint: ResourceHint = field(default_factory=ResourceHint)
    networkonly: bool = False  # skipped when networking is disabled
    engine_only: bool = False  # selected only by an engine-only subset preset
    timeout: int = DEFAULT_STEP_TIMEOUT
    # CPU-time budget in seconds (user+system, measured from the step's cgroup
    # cpu.stat). 0 disables the CPU-time guard, leaving only the wall `timeout`.
    # Unlike wall time, CPU time is immune to machine load, so a CPU budget can be
    # set much tighter than a load-tolerant wall timeout without flaking. Enforced
    # only when cgroup boxing is active (cpu.stat available); otherwise inert.
    cpu_timeout: int = 0
    # Template for the inner-parallelism flag appended to `cmd` when this step declares
    # `preferred_inner_jobs`. None inherits DagConfig.default_jobs_flag; "" disables appending
    # (the step manages its own concurrency). See render_jobs_flag for the template forms.
    jobs_flag: str | None = None
    # A typed, pre-execution omission. This is not PASS and is kept separate from
    # dependency-skipped nodes in RunResult. Unknown strings are rejected by the loader.
    skip_reason: IntentionalSkipReason | None = None

    @property
    def tag(self) -> str:
        """Return the stable ``group.job`` identifier for this step."""
        return f"{self.group}.{self.job}"


def render_jobs_flag(template: str, inner_jobs: int) -> str:
    """Render an inner-parallelism (concurrency) flag from a template and a job count.

    Three forms let a caller match any tool's flag spelling:

    * template contains ``%d`` -> substitute (full control, no auto-space):
      ``"-j%d"`` -> ``"-j4"``, ``"--num-threads=%d"`` -> ``"--num-threads=4"``.
    * template ends with ``=`` -> concatenate (no space): ``"--jobs="`` -> ``"--jobs=4"``.
    * otherwise -> space-separated: ``"--num-threads"`` -> ``"--num-threads 4"``, and the
      default ``"-j"`` -> ``"-j 4"``.
    """
    if "%d" in template:
        return template.replace("%d", str(inner_jobs))
    if template.endswith("="):
        return f"{template}{inner_jobs}"
    return f"{template} {inner_jobs}"


def effective_jobs_flag(step: Step, default_jobs_flag: str) -> str:
    """The jobs-flag template in effect for a step: its own ``jobs_flag`` overrides the
    DagConfig-level default; ``None`` inherits the default."""
    return step.jobs_flag if step.jobs_flag is not None else default_jobs_flag


def command_with_inner_jobs(
    step: Step, default_jobs_flag: str, inner_jobs: int | None
) -> str:
    """The step's shell command with its inner-parallelism flag appended, when applicable.

    Appends ``<rendered-flag>`` (see :func:`render_jobs_flag`) to the command when
    ``inner_jobs`` is set and the effective jobs-flag template is non-empty. A ``None``
    ``inner_jobs`` or an empty template leaves the command unchanged (the step then declares no
    inner parallelism, or manages its own).
    """
    if inner_jobs is None:
        return step.cmd
    template = effective_jobs_flag(step, default_jobs_flag)
    if not template:
        return step.cmd
    return f"{step.cmd} {render_jobs_flag(template, inner_jobs)}"


def step_classification(step: Step) -> StepClass:
    """Return the explicit class, infer latency-bound browser work, or default to light."""
    if step.hint.classification is not StepClass.LIGHT:
        return step.hint.classification
    if "browser" in step.hint.resources:
        return StepClass.LATENCY_BOUND
    return StepClass.LIGHT


def preferred_inner_jobs(step: Step, experiment_override: int | None = None) -> int | None:
    """Internal parallelism width for a step: an explicit override wins, else the hint."""
    if experiment_override is not None:
        return experiment_override
    return step.hint.preferred_inner_jobs


def canonical_cpu_timeout(step: Step, default_cpu_timeout: int) -> int:
    """CANONICAL CPU-time budget (seconds) for a step, before any per-platform scaling: its
    declared ``cpu_timeout`` (>0) wins; otherwise the DAG's SMALL default. Both 0 means the guard
    is disabled. This is the forcing-function default for the CPU-time dimension (see
    DEFAULT_SMALL_CPU_TIMEOUT). This is the number a graph declares and a derivation pipeline
    produces — one table, platform-independent."""
    return step.cpu_timeout if step.cpu_timeout > 0 else default_cpu_timeout


def scale_cpu_timeout(canonical: int, multiplier: float) -> int:
    """Apply a per-platform multiplier to a canonical CPU budget.

    Rounds to whole seconds (the enforcement poll is 1 Hz, so sub-second precision is not
    meaningful) and never rounds a live budget down to 0 — that would silently DISABLE the guard
    on a platform whose multiplier is small, turning a scaling policy into an opt-out. A disabled
    budget (canonical 0) stays disabled regardless of the multiplier.
    """
    if canonical <= 0:
        return 0
    if multiplier == DEFAULT_CPU_TIMEOUT_MULTIPLIER:
        return canonical
    # Round HALF AWAY FROM ZERO, not Python's banker's rounding. Two reasons, both
    # load-bearing: (1) Rust's f64::round() is half-away-from-zero, and a budget that differs
    # between the engines by a second is a real cross-language divergence (round(4.5) is 4 in
    # Python, 5 in Rust); (2) at a tie the more generous budget is the right default for a
    # guard whose whole purpose is to avoid false-killing a healthy-but-slow platform.
    return max(1, math.floor(canonical * multiplier + 0.5))


def effective_cpu_timeout(
    step: Step,
    default_cpu_timeout: int,
    multiplier: float = DEFAULT_CPU_TIMEOUT_MULTIPLIER,
) -> int:
    """CPU-time budget actually ENFORCED for a step on this platform: the canonical budget
    (:func:`canonical_cpu_timeout`) scaled by the platform multiplier
    (:func:`scale_cpu_timeout`). With the default 1.0 multiplier this is exactly the canonical
    budget, so an unconfigured platform behaves as it always did."""
    return scale_cpu_timeout(canonical_cpu_timeout(step, default_cpu_timeout), multiplier)


def resolve_cpu_timeout_multiplier(
    explicit: float | None = None,
    env: Mapping[str, str] | None = None,
) -> tuple[float, str]:
    """Resolve the platform CPU-budget multiplier and its platform label.

    Precedence: an explicit CLI value wins over the environment, which wins over the 1.0 no-op.
    A malformed or non-positive environment value is REFUSED rather than silently ignored — a
    typo that quietly reverted the platform to 1.0 would loosen enforcement invisibly, which is
    the failure mode this whole mechanism exists to prevent.
    """
    environ = os.environ if env is None else env
    label = (environ.get(CPU_TIMEOUT_PLATFORM_ENV) or "").strip()
    if explicit is not None:
        if explicit <= 0:
            raise ValueError(f"cpu-timeout multiplier must be > 0, got {explicit}")
        return explicit, label
    raw = (environ.get(CPU_TIMEOUT_MULTIPLIER_ENV) or "").strip()
    if not raw:
        return DEFAULT_CPU_TIMEOUT_MULTIPLIER, label
    try:
        value = float(raw)
    except ValueError as exc:
        raise ValueError(
            f"{CPU_TIMEOUT_MULTIPLIER_ENV}={raw!r} is not a number"
        ) from exc
    if value <= 0:
        raise ValueError(f"{CPU_TIMEOUT_MULTIPLIER_ENV}={raw!r} must be > 0")
    return value, label


def effective_cpu_count(step: Step, default_cpu_count: int | None) -> int | None:
    """Core cap (cgroup ``cpu.max``) in effect for a step: its declared ``preferred_inner_jobs``
    wins; otherwise the DAG's SMALL default. Bounds ONLY the cgroup cpu.max, never the command's
    inner ``-j`` flag (which stays keyed to the declared width, so an undeclared step gets a
    1-core box without a bogus ``-j 1`` appended to a command that may not accept it)."""
    inner = step.hint.preferred_inner_jobs
    return inner if inner is not None else default_cpu_count


def _cpu_timeout_policy_suffix(
    canonical: int, multiplier: float, platform: str
) -> str:
    """`` (canonical 3s x2 github-hosted)`` when a platform multiplier scaled the budget, else
    empty. Silent at 1.0 so the overwhelmingly common unscaled message is unchanged."""
    if multiplier == DEFAULT_CPU_TIMEOUT_MULTIPLIER or canonical <= 0:
        return ""
    rendered = f"{multiplier:g}"
    label = f" {platform}" if platform else ""
    return f" (canonical {canonical}s x{rendered}{label})"


def step_failure_reason(
    *,
    returncode: int | None,
    oomed: bool,
    oom_kills: int,
    timed_out: bool,
    timeout: int,
    pids_guard_tripped: bool,
    pids_guard_reason: str | None,
    detail_write_failure: Sequence[str],
    cpu_timed_out: bool = False,
    cpu_timeout: int = 0,
    cpu_timeout_canonical: int = 0,
    cpu_timeout_multiplier: float = DEFAULT_CPU_TIMEOUT_MULTIPLIER,
    cpu_timeout_platform: str = "",
) -> str:
    """Describe a failed step without conflating an external signal with an OOM.

    Failure causes use this precedence:
    OOM > CPU-timeout > timeout > pids-guard > detail-capture-failure > signal > exit code.

    CPU-timeout is reported ahead of the wall timeout because it is the more specific
    cause: when a CPU budget is exceeded the runner reaps the step, and the wall guard
    may also observe the resulting exit. Distinguishing them keeps the failure reason
    honest about which budget actually tripped.

    A negative ``returncode`` means the child received a Unix signal; that must never be
    reported as an OOM, since raising a memory baseline when an external supervisor killed
    the step would hide the real problem.
    """
    if oomed:
        return f"OOM-KILLED (hit inner MemoryMax; {oom_kills} oom_kill event(s))"
    if cpu_timed_out:
        # When a platform multiplier is in effect the enforced number is NOT the number written
        # in the graph, so the message must carry both plus the policy that connects them —
        # otherwise the reader cannot tell a genuine overrun from a mis-set platform policy, and
        # cannot find which knob to turn.
        return f"CPU-TIMEOUT >{cpu_timeout}s cpu" + _cpu_timeout_policy_suffix(
            cpu_timeout_canonical, cpu_timeout_multiplier, cpu_timeout_platform
        )
    if timed_out:
        return f"TIMEOUT >{timeout}s"
    if pids_guard_tripped:
        return f"PIDS GUARD ({pids_guard_reason})"
    if detail_write_failure:
        return f"DETAIL CAPTURE FAILED ({detail_write_failure[0]})"
    if returncode is not None and returncode < 0:
        try:
            signal_name = signal.Signals(-returncode).name
        except ValueError:
            signal_name = f"signal {-returncode}"
        return (
            f"received {signal_name} with no validate timeout, pids guard, "
            "or child-cgroup OOM recorded"
        )
    return f"exit {returncode}"


@dataclass(frozen=True)
class DagConfig:
    """A complete step graph plus scheduling and containment policy.

    ``resource_caps`` bounds concurrent scarce-resource demand. Memory and CPU policy
    fields have conservative defaults and may be overridden per workload.
    """

    steps: tuple[Step, ...]
    # Optional long-form documentation for the WHOLE DAG (default empty). Free-form prose
    # (often multi-line) describing the pipeline as a whole; never affects scheduling.
    description: str = ""
    resource_caps: Mapping[str, int] = field(default_factory=dict)
    # Multiplier from a step's measured RSS baseline to its inner memory cap (headroom).
    mem_cap_factor: float = 1.25
    # Lower bound (bytes) on the modeled worst-case footprint, so -j selection never
    # concludes "0 fits". Default 8 GiB.
    mem_cap_floor_bytes: int = 8 * 1024**3
    # Multiplier applied to the modeled peak to leave headroom. 1.0 = no inflation.
    outer_mem_safety_factor: float = 1.0
    default_step_timeout: int = DEFAULT_STEP_TIMEOUT
    # Default inner-parallelism flag template for steps that don't set their own `jobs_flag`.
    default_jobs_flag: str = DEFAULT_JOBS_FLAG
    # --- Deliberately SMALL default caps applied to a step that DECLARES NOTHING ---
    # The forcing function (see the module-level DEFAULT_SMALL_* constants): an undeclared step is
    # boxed into a tight floor so it must declare its real needs. These are active by default; the
    # declarations-first migration has supplied measured budgets for nodes that exceed the floor.
    # Each applies ONLY when the step leaves the matching hint unset (an explicit hint wins).
    # `--unsafe-no-cgroups` is the deliberately loud escape hatch for an unboxed run.
    default_step_mem_cap_bytes: int | None = DEFAULT_SMALL_MEM_CAP_BYTES
    default_step_cpu_count: int | None = DEFAULT_SMALL_CPU_COUNT
    default_step_cpu_timeout: int = DEFAULT_SMALL_CPU_TIMEOUT
    # Tags (``group.job``) whose FAILURE is a DECLARED known-failure: it is reported and named
    # loudly but does NOT flip the run's aggregate verdict, so one persistently-flaky node (e.g.
    # a host-dependent test) can't invalidate every other step's validate record. Derived from a
    # declared file (see :func:`safe_ci_dag_runner.io.load_known_failures`); NEVER silent — the
    # scheduler names each excluded failure at runtime and the loader names what it loaded. A
    # non-allowlisted failure still fails the run, and an allowlisted step that PASSES is
    # unaffected (membership is consulted only on failure). Empty by default (fail-closed).
    known_failures: frozenset[str] = frozenset()

    # --- Per-platform CPU-budget scaling (see DEFAULT_CPU_TIMEOUT_MULTIPLIER) ---
    # Execution-time multiplier over whatever canonical CPU budget is in effect, so one graph
    # runs unchanged on a fast dev box and an underpowered hosted runner. NOT persisted with the
    # graph: this is caller/platform policy, not a property of the pipeline, and writing it into
    # the DAG file would recreate the per-platform table this mechanism replaces.
    cpu_timeout_multiplier: float = DEFAULT_CPU_TIMEOUT_MULTIPLIER
    # Free-form platform label reported alongside the multiplier in a breach message.
    cpu_timeout_platform: str = ""

    def by_tag(self) -> dict[str, Step]:
        """Index configured steps by their stable tags."""
        return {step.tag: step for step in self.steps}
