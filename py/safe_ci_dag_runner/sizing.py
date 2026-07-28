"""Memory footprint model + memory-aware ``-j`` selection (pure functions over a DagConfig).

Ports DeepScry's reachable-concurrent-set memory model: rather than a flat per-job RAM
estimate, it enumerates which steps can actually co-run (no transitive dependency between
them, and their summed scarce-resource demand fits the caps) and takes the worst-case sum
of their per-step memory caps. That yields an exact "largest ``-jN`` that fits budget M".
"""

from __future__ import annotations

import itertools
import os
import re
from collections.abc import Mapping, Sequence
from pathlib import Path

from safe_ci_dag_runner.model import DagConfig, Step, StepClass, step_classification


def step_mem_cap_bytes(step: Step, *, mem_cap_factor: float) -> int | None:
    """Inner-cgroup MemoryMax for a step, or None if uncharacterized (only the outer cap
    then applies). An explicit hard cap wins; otherwise ``factor x`` the RSS baseline."""
    if step.hint.hard_mem_max_bytes is not None:
        return step.hint.hard_mem_max_bytes
    base = step.hint.rss_baseline_bytes
    return int(base * mem_cap_factor) if base else None


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
    scarce-resource demand fits ``cfg.resource_caps``. Only steps with a memory baseline
    and that are not engine-only participate (mirrors DeepScry).

    ``inner_jobs`` applies ONE internal-parallelism width to every step (the ``--max-mem``
    sizing path). ``widths`` instead supplies a PER-STEP width map (a step absent from the map
    falls back to ``inner_jobs``); the CPA allocator uses it so a step widened on the critical
    path is charged its own scaled memory cap while its siblings keep theirs (PLANNER_DESIGN.md
    §5.6). Passing both is allowed; ``widths`` wins per tag.
    """
    by_tag = {
        step.tag: step
        for step in cfg.steps
        if step.hint.rss_baseline_bytes is not None and not step.engine_only
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
    """Worst-case footprint (bytes) at the given ``-j``, clamped to the configured floor."""
    peak, _ = schedulable_peak_mem_bytes(cfg, jobs, inner_jobs)
    return max(cfg.mem_cap_floor_bytes, int(peak * cfg.outer_mem_safety_factor))


def jobs_for_budget(cfg: DagConfig, budget: int) -> tuple[int, int]:
    """Largest ``-jN`` (>=1, capped at CPU count) whose worst-case footprint fits ``budget``
    bytes. Returns ``(jobs, footprint_at_that_jobs)``. Always >= 1: a box too small for even
    one step is a WAIT/abort decision for the caller, not a ``-j0``."""
    ncpu = os.cpu_count() or 4
    best = 1
    for n in range(1, ncpu + 1):
        if jobs_footprint_bytes(cfg, n) <= budget:
            best = n
        else:
            break  # footprint is monotonic non-decreasing in n
    return best, jobs_footprint_bytes(cfg, best)


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
