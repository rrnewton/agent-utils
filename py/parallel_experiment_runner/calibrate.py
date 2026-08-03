"""Concurrency calibration: the mandatory 1 -> 2 -> 4 ramp and resource-slot width selection.

The 470-process incident this tool exists to prevent was UNBOUNDED fan-out. So concurrency
here is a DECLARED, ENFORCED number derived from three independent budgets — the coordinator's
lane, the live host capacity, and the measured per-instance footprint — never "however many the
caller spawns". The functions are pure over sampled inputs so the ramp and downshift logic are
directly unit-testable; the one impure helper (:func:`live_capacity`) is isolated.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path

from safe_ci_dag_runner import mem_available_bytes

from parallel_experiment_runner.model import ResourceSlice, WorkerLimits

#: Multiplier applied to a MEASURED per-instance peak before it bounds concurrency — leaves
#: headroom so a worker slightly hotter than its sample does not blow the budget.
MEM_HEADROOM = 1.25


@dataclass(frozen=True)
class LiveCapacity:
    """A snapshot of what the host can actually give RIGHT NOW (never trusts the lane alone)."""

    cpu_cores: int
    mem_available_bytes: int
    disk_free_bytes: int
    load_avg_1m: float


@dataclass(frozen=True)
class PerInstance:
    """Per-worker footprint bounds used to size concurrency.

    Before stage 1 these are the DECLARED hard caps; after a calibration rung they are the
    MEASURED highs (with :data:`MEM_HEADROOM`), bounded by the hard caps. ``None`` means the
    dimension is unbounded/unknown and therefore does not constrain the width.
    """

    cpu_cores: int
    memory_bytes: int | None
    disk_bytes: int | None


@dataclass(frozen=True)
class WidthDecision:
    """The chosen width plus the independent slot counts, so the LIMITING resource is visible."""

    width: int
    limiting_dimension: str
    cpu_slots: int
    mem_slots: int
    disk_slots: int
    ceiling: int


def live_capacity(work_dir: Path) -> LiveCapacity:
    """Sample physical cores, allocatable memory, free disk (at ``work_dir``), and 1m load."""
    cores = os.cpu_count() or 1
    mem = mem_available_bytes() or 0
    try:
        st = os.statvfs(str(work_dir))
        disk_free = st.f_bavail * st.f_frsize
    except OSError:
        disk_free = 0
    try:
        load = os.getloadavg()[0]
    except OSError:
        load = 0.0
    return LiveCapacity(
        cpu_cores=cores, mem_available_bytes=mem, disk_free_bytes=disk_free, load_avg_1m=load
    )


def _slots(usable: int, per_instance: int | None) -> int:
    """How many workers of ``per_instance`` size fit in ``usable`` (unbounded dim -> huge)."""
    if per_instance is None or per_instance <= 0:
        return 1 << 30  # dimension does not constrain
    return max(0, usable // per_instance)


def resolve_width(
    slice_: ResourceSlice,
    live: LiveCapacity,
    per_instance: PerInstance,
    ceiling: int,
) -> WidthDecision:
    """Max width that fits EVERY budget: ``usable = min(lane, live)`` per dimension.

    ``width = min(cpu_slots, mem_slots, disk_slots, ceiling)``. Live capacity can only ever
    REDUCE the lane (never raise it), so a busy host shrinks the round even when the coordinator
    granted more. The returned slot counts make the binding constraint auditable.
    """
    usable_cpu = min(slice_.cpu_cores, live.cpu_cores)
    usable_mem = min(slice_.memory_bytes, live.mem_available_bytes)
    usable_disk = min(slice_.disk_bytes, live.disk_free_bytes)

    per_mem = None if per_instance.memory_bytes is None else int(per_instance.memory_bytes)
    cpu_slots = _slots(usable_cpu, per_instance.cpu_cores)
    mem_slots = _slots(usable_mem, per_mem)
    disk_slots = _slots(usable_disk, per_instance.disk_bytes)

    candidates = {
        "cpu": cpu_slots,
        "memory": mem_slots,
        "disk": disk_slots,
        "ceiling": max(0, ceiling),
    }
    limiting = min(candidates, key=lambda k: candidates[k])
    width = candidates[limiting]
    return WidthDecision(
        width=width,
        limiting_dimension=limiting,
        cpu_slots=cpu_slots,
        mem_slots=mem_slots,
        disk_slots=disk_slots,
        ceiling=max(0, ceiling),
    )


def measured_per_instance(
    limits: WorkerLimits,
    peak_mem_bytes: int | None,
    *,
    mem_headroom: float = MEM_HEADROOM,
) -> PerInstance:
    """Fold a measured peak into per-instance bounds, clamped to the declared hard caps.

    Uses the MEASURED peak (plus headroom) when available, else the declared cap. Never exceeds
    the hard cap: a worker cannot use more than its enforced ``memory.max`` regardless of a hot
    sample.
    """
    hard = limits.memory_bytes
    mem: int | None
    if peak_mem_bytes is not None:
        inflated = int(peak_mem_bytes * mem_headroom)
        mem = inflated if hard is None else min(hard, inflated)
    else:
        mem = hard
    return PerInstance(cpu_cores=limits.cpu_cores, memory_bytes=mem, disk_bytes=limits.disk_bytes)


def ramp_next_width(current_width: int, fit: WidthDecision) -> int:
    """The next ramp width: DOUBLE, but never past what currently fits; downshift immediately.

    * A shrink (``fit.width < current``) drops straight to the largest safe lower width — never
      relies on a past wide run to ignore current pressure.
    * Otherwise grow by at most 2x (the mandatory ``1 -> 2 -> 4`` ramp), capped by ``fit.width``.
    """
    if fit.width < current_width:
        return fit.width
    return min(current_width * 2, fit.width)


def initial_width(fit: WidthDecision) -> int:
    """Stage 1 is always a single sequential worker to profile the real footprint — unless not
    even one fits (then 0, and the caller must WAIT rather than launch)."""
    return 0 if fit.width < 1 else 1
