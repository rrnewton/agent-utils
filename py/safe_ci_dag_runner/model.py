"""Core DAG vocabulary for safe-ci-dag-runner.

Pure data + pure helpers, no I/O. A caller describes their build/test graph as a set of
:class:`Step` values (each carrying a :class:`ResourceHint`) bundled in a
:class:`DagConfig`, then hands it to the runner. This is the generic replacement for a
project-specific ``build_registry()`` plus the per-step cost / memory / scheduling
constant tables that a project like DeepScry keeps inline.
"""

from __future__ import annotations

import signal
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from enum import Enum

DEFAULT_STEP_TIMEOUT = 1800


class StepClass(Enum):
    """How a step uses the machine, used for scheduling decisions."""

    CPU_BOUND = "cpu-bound"
    LATENCY_BOUND = "latency-bound"
    LIGHT = "light"


@dataclass(frozen=True)
class ResourceHint:
    """Per-step scheduling knowledge: scarce-resource demand, cost estimate, memory.

    Every field is optional. With none supplied the runner falls back to a fixed
    concurrency with no memory model; supplying them enables memory-aware ``-j`` sizing
    and longest-processing-time dispatch ordering. This folds what a project like DeepScry
    keeps in separate global tag-keyed tables (duration hints, RSS baselines, memory caps,
    scheduling profiles) onto the step itself.
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
    deps: list[str] = field(default_factory=list)  # tags ("group.job") this step depends on
    env: dict[str, str] = field(default_factory=dict)
    hint: ResourceHint = field(default_factory=ResourceHint)
    networkonly: bool = False  # skipped when networking is disabled
    engine_only: bool = False  # selected only by an engine-only subset preset
    timeout: int = DEFAULT_STEP_TIMEOUT

    @property
    def tag(self) -> str:
        return f"{self.group}.{self.job}"


def step_classification(step: Step) -> StepClass:
    """A step's class: an explicit non-default hint wins; a browser-resource step is
    latency-bound; otherwise light. (Mirrors DeepScry's ``step_class``.)"""
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
) -> str:
    """Describe a failed step without conflating an external signal with an OOM.

    Precedence (load-bearing for cross-language parity):
    OOM > timeout > pids-guard > detail-capture-failure > signal > exit code.

    A negative ``returncode`` means the child received a Unix signal; that must never be
    reported as an OOM, since raising a memory baseline when an external supervisor killed
    the step would hide the real problem.
    """
    if oomed:
        return f"OOM-KILLED (hit inner MemoryMax; {oom_kills} oom_kill event(s))"
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
    """A whole DAG plus caller policy.

    ``steps`` is the graph; ``resource_caps`` bounds concurrent scarce-resource demand
    (e.g. {"browser": 2, "net": 1}). The memory-model tunables mirror DeepScry's outer-cap
    behavior and can be left at their defaults.
    """

    steps: tuple[Step, ...]
    resource_caps: Mapping[str, int] = field(default_factory=dict)
    # Multiplier from a step's measured RSS baseline to its inner memory cap (headroom).
    mem_cap_factor: float = 1.25
    # Lower bound (bytes) on the modeled worst-case footprint, so -j selection never
    # concludes "0 fits". Default 8 GiB.
    mem_cap_floor_bytes: int = 8 * 1024**3
    # Multiplier applied to the modeled peak to leave headroom. 1.0 = no inflation.
    outer_mem_safety_factor: float = 1.0
    default_step_timeout: int = DEFAULT_STEP_TIMEOUT

    def by_tag(self) -> dict[str, Step]:
        return {step.tag: step for step in self.steps}
