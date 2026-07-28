"""Per-step measurement ENRICHMENT: turn a step's cgroup + ambient deltas into the rich
profile-row columns the parallel-speedup model reads back.

The scheduler measures each step's cgroup (``cpu.stat``, ``memory.peak``, ``cpu.pressure``) and
brackets it with two host-load snapshots (:mod:`safe_ci_dag_runner.ambient`). This module is the
PURE function that folds those raw measurements into the named, self-describing columns the
profile store records — so the speedup reader (:mod:`safe_ci_dag_runner.estimates`) can later
learn each step's speedup curve vs. inner ``-j``.

The column NAMES deliberately mirror DeepScry's ``scripts/validate_perflog.py``
``STEP_PROFILE_COLUMNS`` (``effective_cores``, ``quota_utilization_pct``, ``throttled_s``,
``external_cpu_s`` / ``external_cores``, ``co_tenants_*``, ``ambient_bucket``, ``load*``, and the
host/step PSI columns), so a later unification of the two schemas is a RENAME, not a redesign.

Everything here is best-effort and captured UNDER cgroup boxing on Linux: when a measurement is
unavailable (an un-boxed run, a missing ``/proc`` file) the corresponding column is simply left out
of the returned row and the writer fills it blank — matching DeepScry's "blank when unavailable"
posture. No value is ever fabricated (No Silent Failure).

Cross-language note: these are WRITER columns whose exact numeric values legitimately differ per
host and per run, so they are NOT byte-compared across the Python and Rust builds (only the CSV
*schema* is). The READER that consumes them (the speedup model) IS cross-checked byte-identical.
"""

from __future__ import annotations

from collections.abc import Mapping
from pathlib import Path

from safe_ci_dag_runner import perflog
from safe_ci_dag_runner.ambient import (
    AmbientSnapshot,
    PsiReading,
    ambient_bucket,
    attribute_external_cores,
)

__all__ = [
    "step_enrichment_columns",
    "resolve_effective_inner_jobs",
    "container_core_budget",
]

_USEC = 1_000_000.0

#: cgroup-v2 unified CPU quota file for the whole run's container/machine.
_CPU_MAX_PATH = Path("/sys/fs/cgroup/cpu.max")


def container_core_budget() -> int:
    """The effective core count of the cgroup/container/machine the run is boxed in.

    Prefers the cgroup-v2 CPU quota (``/sys/fs/cgroup/cpu.max`` ``quota period`` -> rounded
    ``quota/period`` cores) when it is bounded, falling back to :func:`perflog.nproc` (the
    affinity width) when the quota is ``max`` or unreadable. This is the NUMBER an "ambient"
    (un-``-j``-capped) step's effective parallelism resolves to — never the string ``"ambient"``.
    """
    try:
        raw = _CPU_MAX_PATH.read_text().split()
        quota, period = int(raw[0]), int(raw[1])
        if quota > 0 and period > 0:
            return max(1, round(quota / period))
    except (OSError, ValueError, IndexError):
        pass
    return perflog.nproc()


def resolve_effective_inner_jobs(inner_jobs: int | None) -> int:
    """Resolve a step's recorded ``inner_jobs`` to a NUMBER.

    An explicit ``preferred_inner_jobs`` width is used verbatim; an "ambient" step (no explicit
    ``-j``) resolves to :func:`container_core_budget` — the effective parallelism of the
    cgroup/container/machine it ran in. This replaces the old string ``"ambient"`` sentinel so the
    speedup model can group samples by a real parallelism level.
    """
    if inner_jobs is not None:
        return inner_jobs
    return container_core_budget()


def _psi_columns(
    row: dict[str, object],
    prefix: str,
    start: PsiReading | None,
    end: PsiReading | None,
) -> None:
    """Fill the four ``<prefix>_psi_avg{10,60}_{start,end}`` columns from two PSI readings.

    An absent reading leaves its columns out of ``row`` (the writer records them blank), so a
    kernel without PSI is distinguishable from genuine zero pressure."""
    if start is not None:
        row[f"{prefix}_psi_avg10_start"] = round(start.avg10, 2)
        row[f"{prefix}_psi_avg60_start"] = round(start.avg60, 2)
    if end is not None:
        row[f"{prefix}_psi_avg10_end"] = round(end.avg10, 2)
        row[f"{prefix}_psi_avg60_end"] = round(end.avg60, 2)


def step_enrichment_columns(
    *,
    elapsed_s: float,
    inner_jobs: int | None,
    cpu_stats: Mapping[str, int] | None,
    ambient_start: AmbientSnapshot | None,
    ambient_end: AmbientSnapshot | None,
    step_pressure_start: PsiReading | None,
    step_pressure_end: PsiReading | None,
) -> dict[str, object]:
    """Build the rich per-step profile columns from one step's cgroup + ambient measurements.

    Only columns whose inputs are present are returned; the caller's CSV writer fills the rest
    blank (they are part of :data:`safe_ci_dag_runner.perflog.STEP_PROFILE_COLUMNS`). Derivations:

    * ``effective_cores`` = child CPU-seconds / wall (``cpu.stat`` ``usage_usec`` / ``elapsed_s``),
      the ACHIEVED parallelism; ``user_s`` / ``sys_s`` / ``throttled_s`` from the matching
      ``cpu.stat`` counters.
    * ``quota_utilization_pct`` = ``effective_cores`` / the applied inner ``-j`` cap (only when an
      explicit ``inner_jobs`` cap was set — an ambient step has no cap to measure against).
    * ``external_cpu_s`` / ``external_cores`` = host CPU burned by OTHER tenants during the step
      window (busy-jiffies delta minus our own cgroup usage), and ``ambient_bucket`` the
      quiet/moderate/busy verdict — so the reader can contention-discount this sample.
    * ``co_tenants_*`` / ``load*`` / host+step PSI = the ambient-load context around the window.
    """
    row: dict[str, object] = {}
    usage_usec = 0
    have_cpu = cpu_stats is not None and elapsed_s > 0.0
    if have_cpu and cpu_stats is not None:
        usage_usec = cpu_stats.get("usage_usec", 0)
        effective_cores = usage_usec / (elapsed_s * _USEC)
        row["effective_cores"] = round(effective_cores, 4)
        row["user_s"] = round(cpu_stats.get("user_usec", 0) / _USEC, 3)
        row["sys_s"] = round(cpu_stats.get("system_usec", 0) / _USEC, 3)
        row["throttled_s"] = round(cpu_stats.get("throttled_usec", 0) / _USEC, 3)
        if inner_jobs:  # quota cores == the applied inner cpu cap (blank for an ambient step)
            row["quota_utilization_pct"] = round(effective_cores / inner_jobs * 100.0, 2)
        if ambient_start is not None and ambient_end is not None:
            external_cores = attribute_external_cores(
                busy_jiffies_start=ambient_start.busy_jiffies,
                busy_jiffies_end=ambient_end.busy_jiffies,
                own_cpu_usec=usage_usec,
                elapsed_s=elapsed_s,
            )
            # external_cpu_s == external_cores * elapsed_s (both derive from the same clamped delta).
            row["external_cpu_s"] = round(external_cores * elapsed_s, 3)
            row["external_cores"] = round(external_cores, 3)
            row["ambient_bucket"] = ambient_bucket(external_cores, ambient_end)
    if ambient_start is not None and ambient_end is not None:
        row["co_tenants_start"] = ambient_start.co_tenants
        row["co_tenants_end"] = ambient_end.co_tenants
        row["load1_start"] = round(ambient_start.load1, 3)
        row["load1_end"] = round(ambient_end.load1, 3)
        row["load5_start"] = round(ambient_start.load5, 3)
        row["load5_end"] = round(ambient_end.load5, 3)
        _psi_columns(row, "host_cpu", ambient_start.cpu_psi, ambient_end.cpu_psi)
        _psi_columns(row, "host_memory", ambient_start.memory_psi, ambient_end.memory_psi)
        _psi_columns(row, "host_io", ambient_start.io_psi, ambient_end.io_psi)
    _psi_columns(row, "step_cpu", step_pressure_start, step_pressure_end)
    return row
