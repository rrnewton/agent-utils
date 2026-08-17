"""Memory-footprint modeling and safe concurrency selection.

The model enumerates dependency-compatible and resource-compatible steps that may run
together, then chooses the largest concurrency that fits the supplied memory budget.
"""

from __future__ import annotations

import itertools
import os
import re
from collections.abc import Mapping, Sequence
from pathlib import Path

from safe_ci_dag_runner.model import (
    DEFAULT_SMALL_MEM_CAP_BYTES,
    DagConfig,
    Step,
    StepClass,
    step_classification,
)


def step_mem_cap_bytes(
    step: Step, *, mem_cap_factor: float, default_cap_bytes: int | None = None
) -> int | None:
    """Inner-cgroup MemoryMax for a step. An explicit hard cap wins; otherwise ``factor x`` the
    RSS baseline. When the step declares NEITHER (uncharacterized), fall back to
    ``default_cap_bytes`` — the SMALL forcing-function default the scheduler passes from
    ``DagConfig.default_step_mem_cap_bytes`` — or None if no default is supplied (the active-step
    sizing model calls with no default, so an uncharacterized step stays excluded from the
    footprint sum)."""
    if step.hint.hard_mem_max_bytes is not None:
        return step.hint.hard_mem_max_bytes
    base = step.hint.rss_baseline_bytes
    if base:
        return int(base * mem_cap_factor)
    return default_cap_bytes


def step_mem_cap_for_inner_jobs(
    step: Step, inner_jobs: int | None, *, mem_cap_factor: float
) -> int:
    """Per-step cap scaled for internal parallelism.

    Conservative ``P x J`` model pending measured matrices: an explicit hard cap and
    non-CPU-bound steps keep the base cap; CPU-bound steps scale linearly above J=4.
    """
    cap = step_mem_cap_bytes(step, mem_cap_factor=mem_cap_factor) or 0
    if (
        step.hint.hard_mem_max_bytes is not None
        or inner_jobs is None
        or step_classification(step) is not StepClass.CPU_BOUND
    ):
        return cap
    return max(cap, int(cap * inner_jobs / 4))


def transitive_deps(steps: Sequence[Step]) -> dict[str, set[str]]:
    """Map each step tag to the set of all tags it transitively depends on."""
    direct = {step.tag: set(step.deps) for step in steps}
    result: dict[str, set[str]] = {}

    def visit(tag: str) -> set[str]:
        if tag in result:
            return result[tag]
        deps = set(direct.get(tag, set()))
        for dep in tuple(deps):
            deps.update(visit(dep))
        result[tag] = deps
        return deps

    for tag in direct:
        visit(tag)
    return result


def schedulable_peak_mem_bytes(
    cfg: DagConfig,
    jobs: int,
    inner_jobs: int | None = None,
    *,
    widths: Mapping[str, int] | None = None,
) -> tuple[int, tuple[str, ...]]:
    """Maximum per-step-cap sum over any scheduler-reachable concurrent set of size <= jobs.

    A set is reachable only when no member transitively depends on another and the summed
    scarce-resource demand fits ``cfg.resource_caps``. Only non-engine, non-skipped steps with a
    memory baseline participate.

    ``inner_jobs`` applies ONE internal-parallelism width to every step (the ``--max-mem``
    sizing path). ``widths`` instead supplies a PER-STEP width map (a step absent from the map
    falls back to ``inner_jobs``); the CPA allocator uses it so a step widened on the critical
    path is charged its own scaled memory cap while its siblings keep theirs. Passing both
    is allowed; ``widths`` wins per tag.
    """
    by_tag = {
        step.tag: step
        for step in cfg.steps
        if step.hint.rss_baseline_bytes is not None
        and not step.engine_only
        and step.skip_reason is None
    }
    dependencies = transitive_deps(list(cfg.steps))
    tags = tuple(by_tag)
    width = min(max(1, jobs), len(tags))
    best_total = 0
    best: tuple[str, ...] = ()

    def cap_of(tag: str) -> int:
        w = widths.get(tag, inner_jobs) if widths is not None else inner_jobs
        return step_mem_cap_for_inner_jobs(by_tag[tag], w, mem_cap_factor=cfg.mem_cap_factor)

    for count in range(1, width + 1):
        for candidate in itertools.combinations(tags, count):
            if any(
                left in dependencies[right] or right in dependencies[left]
                for left, right in itertools.combinations(candidate, 2)
            ):
                continue
            if any(
                sum(by_tag[tag].hint.resources.get(resource, 0) for tag in candidate) > cap
                for resource, cap in cfg.resource_caps.items()
            ):
                continue
            total = sum(cap_of(tag) for tag in candidate)
            if total > best_total:
                best_total, best = total, candidate
    return best_total, best


def jobs_footprint_bytes(cfg: DagConfig, jobs: int, inner_jobs: int | None = None) -> int:
    """Worst-case footprint at the given active-step count, clamped to the configured floor."""
    peak, _ = schedulable_peak_mem_bytes(cfg, jobs, inner_jobs)
    return max(cfg.mem_cap_floor_bytes, int(peak * cfg.outer_mem_safety_factor))


def jobs_for_budget(cfg: DagConfig, budget: int) -> tuple[int, int]:
    """Largest active-step count whose worst-case footprint fits ``budget``.

    The count is at least one and capped at the CPU count. Returns ``(max_steps,
    footprint_at_that_count)``. A box too small for even one step is a WAIT/abort decision for the
    caller, not a zero-concurrency result.
    """
    ncpu = os.cpu_count() or 4
    best = 1
    for n in range(1, ncpu + 1):
        if jobs_footprint_bytes(cfg, n) <= budget:
            best = n
        else:
            break  # footprint is monotonic non-decreasing in n
    return best, jobs_footprint_bytes(cfg, best)


# Worst-case peak RSS of ONE compile/link job, used to bound build parallelism by the
# memory a boxed scope actually grants. DERIVED from a measured footprint, never guessed:
# hermit#1584's build.dbi_release OOM-killed at the 8.0 GiB cap under j=32 (memory.events
# oom_kill=2) yet is stable at j8 (dag-mem-caps-pinned-jobs-fix #1583), i.e. ~8.0 GiB / 8
# ~= 1.0 GiB is the largest per-job footprint that still fits. This constant is the single
# source for "how many build jobs does a given memory cap afford" so the -j a cap implies
# is never re-derived (or drift) at a second site.
PER_BUILD_JOB_MEM_BYTES = 1 * 1024**3


def derive_build_jobs(cpu_count: int | None, mem_max_bytes: int | None) -> int:
    """Derive a bounded build-job count from colocated CPU and memory caps.

    Build tools commonly auto-detect all granted cores without accounting for the memory
    needed by each concurrent compilation. Keep the parallelism decision with its caps:

      jobs = min(granted_cores, mem_cap // PER_BUILD_JOB_MEM_BYTES)

    ``cpu_count`` is the granted cores (a step's inner ``cpu.max``, or the scope's effective
    quota for an unpinned step); ``mem_max_bytes`` is the co-located memory cap (the step's
    ``memory.max`` or the scope's). Either bound alone is insufficient. The result is always
    at least one; rejecting an undersized cap remains the caller's scheduling decision.
    """
    cores = cpu_count if (cpu_count is not None and cpu_count > 0) else (os.cpu_count() or 1)
    jobs = int(cores)
    if mem_max_bytes is not None and mem_max_bytes > 0:
        jobs = min(jobs, mem_max_bytes // PER_BUILD_JOB_MEM_BYTES)
    return max(1, int(jobs))


_SIZE_RE = re.compile(r"\s*(\d+(?:\.\d+)?)\s*([KkMmGgTt]?)([Bb]?)\s*")
_SIZE_MULT = {"": 1, "k": 1024, "m": 1024**2, "g": 1024**3, "t": 1024**4}


def parse_size(spec: str | int | None) -> int | None:
    """Parse '8G' / '4096M' / '2048K' / '12345' (bytes) into an int, or None if bad."""
    if spec is None or spec == "":
        return None
    match = _SIZE_RE.fullmatch(str(spec))
    if not match:
        return None
    value = float(match.group(1))
    return int(value * _SIZE_MULT[match.group(2).lower()])


def mem_available_bytes() -> int | None:
    """Bytes currently allocatable without swapping (MemAvailable), or None if unreadable."""
    try:
        for line in Path("/proc/meminfo").read_text().splitlines():
            if line.startswith("MemAvailable:"):
                return int(line.split()[1]) * 1024
    except (OSError, ValueError, IndexError):
        return None
    return None


#: cgroup-v2 unified memory limit for the whole run's container/machine.
_MEMORY_MAX_PATH = Path("/sys/fs/cgroup/memory.max")


def cgroup_mem_max_bytes() -> int | None:
    """The cgroup-v2 ``memory.max`` cap (bytes) of the container the run is boxed in, or ``None``
    when it is unbounded (``max``) or unreadable. This is the HARD ceiling the kernel enforces on
    the whole boxed subtree, so it bounds how many concurrent copies can coexist before the OOM
    killer fires."""
    try:
        raw = _MEMORY_MAX_PATH.read_text().strip()
    except OSError:
        return None
    if raw == "max":
        return None
    try:
        return int(raw)
    except ValueError:
        return None


def box_mem_budget_bytes() -> int | None:
    """The memory budget (bytes) a stress fan-out must fit inside, or ``None`` when it cannot be
    determined.

    Takes the MINIMUM of the readable signals — the cgroup ``memory.max`` hard cap (what the
    kernel will OOM-kill above) and ``/proc/meminfo`` ``MemAvailable`` (headroom actually free
    right now, which respects sibling agents on a shared box). The minimum is deliberately
    conservative: exceeding EITHER risks an OOM, so the smaller wins."""
    signals = [b for b in (cgroup_mem_max_bytes(), mem_available_bytes()) if b is not None]
    return min(signals) if signals else None


def stress_copy_footprint_bytes(
    cfg: DagConfig, *, default_step_bytes: int = DEFAULT_SMALL_MEM_CAP_BYTES
) -> int:
    """Conservative worst-case memory footprint (bytes) of ONE copy (shard) of ``cfg``, used to
    derive the safe ``--stress`` fan-out.

    Sums each step's per-step inner memory cap (``step_mem_cap_bytes`` — an explicit
    ``hard_mem_max_bytes`` wins, else ``mem_cap_factor x rss_baseline_bytes``); an active step that
    DECLARES NO memory is charged ``default_step_bytes`` (the SMALL 1-GiB forcing-function
    default the rest of the package uses). Summing every step (rather than the reachable
    concurrent set) is a deliberate UPPER BOUND: it never under-charges, so the derived ceiling
    errs toward refusing rather than OOMing sibling agents. Intentional pre-execution skips are
    excluded because no copy can spawn them. For the common single-node stress
    (``--only dbi.file_metadata --stress N``) the sum is exactly that one node's cap."""
    total = 0
    for step in cfg.steps:
        if step.engine_only or step.skip_reason is not None:
            continue
        cap = step_mem_cap_bytes(
            step, mem_cap_factor=cfg.mem_cap_factor, default_cap_bytes=default_step_bytes
        )
        total += cap if cap is not None else default_step_bytes
    return max(total, default_step_bytes)
