"""Profile-store FEEDBACK: turn recorded per-step samples into planning estimates.

This is the *reading* half of the learned-duration profile store (ds-7pzdgm / ds-afzsqf).
The *writing* half already ships (:mod:`safe_ci_dag_runner.perflog` auto-logs a per-step CSV
per run). This module reads that store back and derives, for the current
``(machine_id, container_class)``:

* ``est_duration_s`` — a ROBUST central estimate (contention-discounted MAD-trimmed MEDIAN)
  of the recorded ``elapsed_s`` for each step, recovering the step's INTRINSIC (uncontended)
  duration. It is a MEDIAN (not a mean) so a single slow sample cannot drag it, and it is
  discounted by whatever contention signal the store carries (``pct_other`` / ``external_cores``
  / a CPU-pressure column / ``co_tenants`` — whichever is present), so a duration measured on a
  loaded box is corrected back toward the quiet-box value.
* ``rss_estimate_bytes`` — a ROBUST HIGH-WATER estimate (a high percentile) of the recorded
  ``peak_bytes`` for the memory model, so one spuriously large sample does not inflate the cap
  while a genuinely high footprint is still respected.

Sparse / missing data degrades gracefully: a step with no samples (or no parseable column)
yields ``None`` for that field and the caller falls back to the DAG-authored hint. Nothing here
raises on a malformed row — an unparseable cell is skipped (never silently coerced to a wrong
value), matching the No-Silent-Failure posture of the rest of the package.

Determinism note (cross-language parity): every derived number is computed with the SAME
arithmetic in the Python and Rust builds and the plan is rendered with fixed-precision
formatting, so ``plan --format json`` is byte-identical across the two builds for a given store
+ DAG. The percentile uses integer rank arithmetic (no floating ``ceil``) for exactly this
reason. See ``cross/differential.py``.
"""

from __future__ import annotations

import csv
import os
from collections.abc import Mapping, Sequence
from dataclasses import dataclass
from enum import Enum
from pathlib import Path

from safe_ci_dag_runner import perflog
from safe_ci_dag_runner.model import DagConfig, ResourceHint, Step

__all__ = [
    "MACHINE_ID_ENV",
    "CONTAINER_CLASS_ENV",
    "DEFAULT_MIN_SAMPLES",
    "Planner",
    "StepSamples",
    "SpeedupLevel",
    "StepSpeedup",
    "PlanEntry",
    "Plan",
    "feedback_identity",
    "load_step_samples",
    "load_step_speedups",
    "build_plan",
    "apply_plan_to_config",
    "plan_to_json",
    "plan_to_text",
]

#: Environment overrides for the feedback identity. Normally the reader derives the machine id
#: and container class from the current host (so a run learns from THIS machine's own history);
#: these let a test (or a caller pinning heterogeneous-but-equivalent runners to one identity)
#: force the ``step_profiles_<machine>_<container>.csv`` the reader loads. Used by the cross
#: differential to read a fixed synthetic store host-independently.
MACHINE_ID_ENV = "SAFE_CI_DAG_RUNNER_MACHINE_ID"
CONTAINER_CLASS_ENV = "SAFE_CI_DAG_RUNNER_CONTAINER_CLASS"

#: Minimum recorded samples before the store overrides the DAG hint for a step. Below this the
#: DAG-authored hint wins (the store has too little signal to trust yet).
DEFAULT_MIN_SAMPLES = 1

#: Robust-stat tunables (named so both builds share one source of truth).
#: MAD-trim: drop duration samples more than this many MADs from the median before re-medianing.
_MAD_TRIM_K = 3.5
#: RSS high-water percentile expressed as an exact integer fraction (num/den) so the nearest-rank
#: index is computed with integer arithmetic — no floating ``ceil`` whose rounding could diverge
#: across languages. 9/10 == the 90th percentile.
_RSS_PCTL_NUM = 9
_RSS_PCTL_DEN = 10
#: A contention fraction is clamped to this maximum so a pathological signal (e.g. a bogus
#: ``pct_other`` of 100) cannot discount a duration all the way to zero.
_MAX_CONTENTION = 0.95

#: The inclusive range of a signed 64-bit integer. Python's ``int`` is arbitrary-precision, so an
#: out-of-range value it would happily keep must be REJECTED here to match Rust's
#: ``str::parse::<i64>()`` (which fails on overflow). Otherwise the two builds would derive different
#: estimates from the SAME store cell (e.g. a huge ``peak_bytes``), breaking cross-language parity.
_I64_MIN = -(2**63)
_I64_MAX = 2**63 - 1

#: ASCII whitespace trimmed from a numeric cell before parsing — EXACTLY the five characters Rust's
#: ``char::is_ascii_whitespace`` trims (tab, newline, form-feed, carriage-return, space; note it does
#: NOT include the vertical tab 0x0b). Trimming the same set in both builds lets a whitespace-padded
#: cell (a common CSV-producer artifact from spreadsheets/awk/hand-edited fixtures) parse to the same
#: number in both, instead of Python accepting it while Rust's strict parse silently drops it.
_ASCII_WS = "\t\n\x0c\r "


class Planner(Enum):
    """Which scheduling planner to use for dispatch ordering."""

    GREEDY_LPT = "greedy-lpt"
    CRITICAL_PATH = "critical-path"

    @classmethod
    def from_value(cls, text: str) -> "Planner | None":
        for planner in cls:
            if planner.value == text:
                return planner
        return None


# --------------------------------------------------------------------------- reader


@dataclass(frozen=True)
class StepSamples:
    """Aggregated store estimates for ONE step, from its recorded samples.

    ``est_duration_s`` / ``rss_estimate_bytes`` are ``None`` when no sample carried a parseable
    value for that column, so the caller can fall back to the DAG hint.
    """

    step: str
    samples: int
    est_duration_s: float | None
    rss_estimate_bytes: int | None


def feedback_identity() -> tuple[str, str]:
    """The ``(machine_id, container_class)`` the feedback reader selects the store file by.

    Derived from the current host (so a run learns from this machine's own history), unless the
    :data:`MACHINE_ID_ENV` / :data:`CONTAINER_CLASS_ENV` overrides are set. Both builds resolve
    this identically (the underlying :func:`perflog.machine_id` / :func:`perflog.container_class`
    are already cross-checked to agree)."""
    machine_id = os.environ.get(MACHINE_ID_ENV) or perflog.machine_id()
    container_class = os.environ.get(CONTAINER_CLASS_ENV) or perflog.container_class()
    return machine_id, container_class


def _median(sorted_values: Sequence[float]) -> float:
    """Median of an already-sorted, non-empty sequence (average of the two middle values for an
    even count). The caller guarantees non-emptiness."""
    n = len(sorted_values)
    mid = n // 2
    if n % 2 == 1:
        return sorted_values[mid]
    return (sorted_values[mid - 1] + sorted_values[mid]) / 2.0


def _robust_median(values: Sequence[float]) -> float:
    """MAD-trimmed median: median after dropping samples more than ``_MAD_TRIM_K`` MADs from the
    median (median-absolute-deviation outlier rejection). Falls back to the plain median when the
    MAD is zero (all-equal, or a single sample)."""
    xs = sorted(values)
    m = _median(xs)
    deviations = sorted(abs(x - m) for x in xs)
    mad = _median(deviations)
    if mad > 0.0:
        cutoff = _MAD_TRIM_K * mad
        kept = [x for x in xs if abs(x - m) <= cutoff]
        if kept:
            return _median(sorted(kept))
    return m


def _high_percentile(values: Sequence[int]) -> int:
    """The :data:`_RSS_PCTL_NUM`/:data:`_RSS_PCTL_DEN` percentile of a non-empty int sequence by
    NEAREST-RANK, using integer arithmetic so the chosen index is identical across languages.

    ``rank = ceil(num*n/den)`` via the integer identity ``ceil(a/b) == (a + b - 1) // b``; the
    1-based rank clamps to ``n`` and the returned value is the rank-th smallest."""
    xs = sorted(values)
    n = len(xs)
    rank = (_RSS_PCTL_NUM * n + _RSS_PCTL_DEN - 1) // _RSS_PCTL_DEN
    if rank < 1:
        rank = 1
    if rank > n:
        rank = n
    return xs[rank - 1]


#: Contention columns the reader understands, in priority order. Each maps a recorded value to a
#: fraction ``c`` in ``[0, 1)`` of machine capacity consumed by OTHER work during the sample; the
#: intrinsic (uncontended) duration is estimated as ``elapsed * (1 - c)``. Only the FIRST present,
#: parseable column is used. ``pct_*`` are percentages (0..100); ``external_cores`` is absolute
#: cores (divided by the affinity width parsed from ``container_class``); ``co_tenants`` is a count
#: of co-running tenants (n co-tenants sharing equally => the step keeps ``1/(n+1)`` => ``c =
#: n/(n+1)``). These are consumed "where present": today's store does not populate per-step
#: contention columns, so a real run currently uses the plain (undiscounted) median; a synthetic
#: fixture with these columns exercises the discount path, and a future writer enhancement can
#: populate them (see the speedup-curve follow-on).
_CONTENTION_PCT_COLUMNS = ("pct_other", "psi_cpu_some_avg10", "cpu_pressure_some_avg10")


def _affinity_width(container_class: str) -> int | None:
    """Parse the affinity (core) width out of a ``container_class`` like
    ``affinity316_cpu-max-...`` -> ``316``. ``None`` if the shape is unexpected."""
    if not container_class.startswith("affinity"):
        return None
    rest = container_class[len("affinity"):]
    digits = ""
    for ch in rest:
        # ASCII digits ONLY, matching Rust's ``char::is_ascii_digit``. Python's ``str.isdigit()``
        # also returns True for Unicode digit characters (e.g. the superscript ``²``) that ``int()``
        # then cannot parse, which would raise here — restricting to ASCII keeps this function total
        # (the module never raises, per the docstring) and identical to the Rust build.
        if ch.isascii() and ch.isdigit():
            digits += ch
        else:
            break
    if not digits:
        return None
    value = int(digits)
    # Bound to i64 as Rust's ``digits.parse::<i64>().ok()`` does, so an absurdly wide affinity token
    # is rejected identically in both builds instead of Python keeping an out-of-range int.
    if value < _I64_MIN or value > _I64_MAX:
        return None
    return value


def _clean_numeric_cell(cell: str | None) -> str | None:
    """Trim surrounding ASCII whitespace and reject anything Rust's strict ``str::parse`` would
    reject anyway (non-ASCII characters and PEP-515 ``_`` separators), returning the cleaned token or
    ``None``.

    This is the single source of truth that keeps :func:`_parse_float` / :func:`_parse_int` accepting
    EXACTLY the same set of numeric tokens as the Rust build: Python's ``float()`` / ``int()`` are
    permissive (they accept leading/trailing whitespace, underscore separators, and — for ``int`` —
    out-of-``i64`` magnitudes) while Rust's ``parse`` is strict, so without this normalization the
    two builds would silently derive different estimates from the same store."""
    if cell is None:
        return None
    token = cell.strip(_ASCII_WS)
    if not token or not token.isascii() or "_" in token:
        return None
    return token


def _parse_float(cell: str | None) -> float | None:
    token = _clean_numeric_cell(cell)
    if token is None:
        return None
    try:
        return float(token)
    except ValueError:
        return None


def _parse_int(cell: str | None) -> int | None:
    token = _clean_numeric_cell(cell)
    if token is None:
        return None
    try:
        value = int(token)
    except ValueError:
        return None
    # Reject out-of-i64 magnitudes so a value Python's arbitrary-precision ``int`` would keep is
    # dropped exactly as Rust's ``str::parse::<i64>()`` drops it (see :data:`_I64_MIN`).
    if value < _I64_MIN or value > _I64_MAX:
        return None
    return value


def _contention_fraction(row: Mapping[str, str], affinity_width: int | None) -> float:
    """The fraction of machine capacity taken by OTHER work during a sample, from whichever
    contention column is present (see :data:`_CONTENTION_PCT_COLUMNS`). ``0.0`` when the store
    carries no usable contention signal (=> no discount). Clamped to :data:`_MAX_CONTENTION`."""
    fraction: float | None = None
    for col in _CONTENTION_PCT_COLUMNS:
        pct = _parse_float(row.get(col))
        if pct is not None:
            fraction = pct / 100.0
            break
    if fraction is None:
        external = _parse_float(row.get("external_cores"))
        if external is not None and affinity_width and affinity_width > 0:
            fraction = external / float(affinity_width)
    if fraction is None:
        co_tenants = _parse_float(row.get("co_tenants"))
        if co_tenants is not None and co_tenants > 0.0:
            fraction = co_tenants / (co_tenants + 1.0)
    if fraction is None:
        return 0.0
    if fraction < 0.0:
        return 0.0
    if fraction > _MAX_CONTENTION:
        return _MAX_CONTENTION
    return fraction


def _load_store(
    profile_dir: str | Path, machine_id: str, container_class: str
) -> tuple[list[dict[str, str]], int | None] | None:
    """Read ``<profile_dir>/step_profiles_<machine_id>_<container_class>.csv`` into a list of
    column->value row dicts plus the parsed affinity (core) width, or ``None`` when the file is
    absent. The single CSV-reading path shared by :func:`load_step_samples` and
    :func:`load_step_speedups`, so both consume the store identically (DRY)."""
    path = Path(profile_dir) / f"step_profiles_{machine_id}_{container_class}.csv"
    if not path.is_file():
        return None
    rows: list[dict[str, str]] = []
    with path.open(newline="") as handle:
        for raw in csv.DictReader(handle):
            rows.append(
                {k: (v if isinstance(v, str) else "") for k, v in raw.items() if isinstance(k, str)}
            )
    return rows, _affinity_width(container_class)


def load_step_samples(
    profile_dir: str | Path, machine_id: str, container_class: str
) -> dict[str, StepSamples]:
    """Read the per-step profile CSV for ``(machine_id, container_class)`` under ``profile_dir``
    and aggregate the recorded samples per step into robust estimates.

    Reads exactly ``<profile_dir>/step_profiles_<machine_id>_<container_class>.csv`` (the file the
    writer names per machine + container). Returns ``{}`` when that file is absent — the caller
    then falls back to DAG hints for every step (graceful sparse-data handling). Each returned
    :class:`StepSamples` carries the contention-discounted MAD-trimmed median ``elapsed_s`` and the
    high-percentile ``peak_bytes`` for that step, or ``None`` for a column that no sample supplied.
    """
    loaded = _load_store(profile_dir, machine_id, container_class)
    if loaded is None:
        return {}
    rows, affinity_width = loaded
    durations: dict[str, list[float]] = {}
    peaks: dict[str, list[int]] = {}
    counts: dict[str, int] = {}
    for row in rows:
        step = row.get("step")
        if not step:
            continue
        counts[step] = counts.get(step, 0) + 1
        elapsed = _parse_float(row.get("elapsed_s"))
        if elapsed is not None and elapsed >= 0.0:
            intrinsic = elapsed * (1.0 - _contention_fraction(row, affinity_width))
            durations.setdefault(step, []).append(intrinsic)
        peak = _parse_int(row.get("peak_bytes"))
        if peak is not None and peak >= 0:
            peaks.setdefault(step, []).append(peak)
    result: dict[str, StepSamples] = {}
    for step, n in counts.items():
        step_durations = durations.get(step, [])
        step_peaks = peaks.get(step, [])
        result[step] = StepSamples(
            step=step,
            samples=n,
            est_duration_s=_robust_median(step_durations) if step_durations else None,
            rss_estimate_bytes=_high_percentile(step_peaks) if step_peaks else None,
        )
    return result


# --------------------------------------------------------------------------- speedup model

#: A level must be at least this many times faster than the PREVIOUS (fewer-thread) level to make
#: the extra threads worthwhile; below this the marginal speedup has plateaued (a knee).
_SPEEDUP_MIN_MARGINAL_GAIN = 1.15
#: If total CPU-seconds (user+sys) grow by more than this factor between two consecutive levels the
#: step is doing materially more total work per added thread (sub-linear scaling / contention), a
#: work-conservation signal to stop adding threads even if the wall still nudged down.
_SPEEDUP_MAX_WORK_GROWTH = 1.5
#: A step needs at least this many DISTINCT inner_jobs levels (with wall data) to model a curve;
#: fewer and there is nothing to say about speedup vs. parallelism.
_SPEEDUP_MIN_LEVELS = 2


@dataclass(frozen=True)
class SpeedupLevel:
    """One measured point on a step's speedup curve: its robust wall (and work) at one inner_jobs
    width, plus the speedup relative to the smallest measured width."""

    inner_jobs: int
    samples: int
    #: Contention-discounted MAD-trimmed median ``elapsed_s`` at this width.
    wall_s: float
    #: Robust-median total CPU-seconds (``user_s`` + ``sys_s``) — the work-conservation signal.
    cpu_s: float | None
    #: Robust-median achieved parallelism (``effective_cores``) at this width.
    effective_cores: float | None
    #: Robust-median ``throttled_s`` at this width (rising throttling is a saturation signal).
    throttled_s: float | None
    #: ``wall(baseline) / wall(this)`` — the measured speedup vs. the smallest width.
    speedup: float


@dataclass(frozen=True)
class StepSpeedup:
    """A step's fitted speedup curve across inner_jobs widths, plus the recommended width.

    ``recommended_inner_jobs`` is the best wall time still within diminishing-returns AND the core
    budget: the loop advances the recommendation to a wider level only while that level is
    materially faster than the previous one (:data:`_SPEEDUP_MIN_MARGINAL_GAIN`) and does not blow
    up total CPU-seconds (:data:`_SPEEDUP_MAX_WORK_GROWTH`), stopping at the knee."""

    step: str
    baseline_inner_jobs: int
    recommended_inner_jobs: int
    #: Achieved parallelism at the recommended width (``None`` if not measured there).
    measured_effective_cores: float | None
    levels: tuple[SpeedupLevel, ...]


def _build_step_speedup(
    step: str,
    raw_levels: Sequence[tuple[int, int, float, float | None, float | None, float | None]],
    core_budget: int | None,
) -> StepSpeedup:
    """Assemble a :class:`StepSpeedup` from per-level ``(inner_jobs, samples, wall, cpu, eff,
    throttled)`` tuples SORTED ascending by inner_jobs. Deterministic across builds (only compares
    robust medians of identical inputs)."""
    baseline_j = raw_levels[0][0]
    baseline_wall = raw_levels[0][2]
    levels: list[SpeedupLevel] = []
    recommended = baseline_j
    still_scaling = True
    prev_wall = baseline_wall
    prev_cpu = raw_levels[0][3]
    eff_by_j: dict[int, float | None] = {}
    for idx, (j, n, wall, cpu, eff, throttled) in enumerate(raw_levels):
        speedup = baseline_wall / wall if wall > 0.0 else 1.0
        levels.append(
            SpeedupLevel(
                inner_jobs=j,
                samples=n,
                wall_s=wall,
                cpu_s=cpu,
                effective_cores=eff,
                throttled_s=throttled,
                speedup=speedup,
            )
        )
        eff_by_j[j] = eff
        if idx > 0 and still_scaling:
            gain = prev_wall / wall if wall > 0.0 else 1.0
            work_growth = (
                cpu / prev_cpu if (cpu is not None and prev_cpu is not None and prev_cpu > 0.0) else None
            )
            within_budget = core_budget is None or j <= core_budget
            if (
                gain >= _SPEEDUP_MIN_MARGINAL_GAIN
                and (work_growth is None or work_growth <= _SPEEDUP_MAX_WORK_GROWTH)
                and within_budget
            ):
                recommended = j
            else:
                still_scaling = False
        prev_wall = wall
        prev_cpu = cpu
    return StepSpeedup(
        step=step,
        baseline_inner_jobs=baseline_j,
        recommended_inner_jobs=recommended,
        measured_effective_cores=eff_by_j.get(recommended),
        levels=tuple(levels),
    )


def load_step_speedups(
    profile_dir: str | Path, machine_id: str, container_class: str
) -> dict[str, StepSpeedup]:
    """Model each step's PARALLEL-SPEEDUP curve from its samples ACROSS inner_jobs widths.

    Reads the same per-step store as :func:`load_step_samples`, groups samples by
    ``(step, inner_jobs)``, and for each width derives a robust (MAD-trimmed median),
    contention-discounted wall time plus the work-conservation signal (median total CPU-seconds
    ``user_s`` + ``sys_s``), achieved ``effective_cores``, and ``throttled_s``. From those it fits
    the per-step speedup(inner_jobs) curve and a RECOMMENDED width (best wall within the knee and
    the machine's core budget, the affinity width parsed from ``container_class``).

    Only steps with at least :data:`_SPEEDUP_MIN_LEVELS` distinct widths (with wall data) get a
    model; the rest are absent (the caller shows no speedup curve for them). Never raises on a
    malformed row (an unparseable cell is skipped, matching :func:`load_step_samples`).
    """
    loaded = _load_store(profile_dir, machine_id, container_class)
    if loaded is None:
        return {}
    rows, affinity_width = loaded
    core_budget = affinity_width
    walls: dict[tuple[str, int], list[float]] = {}
    cpus: dict[tuple[str, int], list[float]] = {}
    effs: dict[tuple[str, int], list[float]] = {}
    thrs: dict[tuple[str, int], list[float]] = {}
    for row in rows:
        step = row.get("step")
        if not step:
            continue
        inner = _parse_int(row.get("inner_jobs"))
        if inner is None or inner <= 0:
            continue
        key = (step, inner)
        elapsed = _parse_float(row.get("elapsed_s"))
        if elapsed is not None and elapsed >= 0.0:
            walls.setdefault(key, []).append(
                elapsed * (1.0 - _contention_fraction(row, affinity_width))
            )
        user = _parse_float(row.get("user_s"))
        system = _parse_float(row.get("sys_s"))
        if user is not None and system is not None and user >= 0.0 and system >= 0.0:
            cpus.setdefault(key, []).append(user + system)
        eff = _parse_float(row.get("effective_cores"))
        if eff is not None and eff >= 0.0:
            effs.setdefault(key, []).append(eff)
        throttled = _parse_float(row.get("throttled_s"))
        if throttled is not None and throttled >= 0.0:
            thrs.setdefault(key, []).append(throttled)
    by_step: dict[str, list[int]] = {}
    for step, inner in walls:
        by_step.setdefault(step, []).append(inner)
    result: dict[str, StepSpeedup] = {}
    for step, widths in by_step.items():
        levels_j = sorted(set(widths))
        if len(levels_j) < _SPEEDUP_MIN_LEVELS:
            continue
        raw_levels: list[tuple[int, int, float, float | None, float | None, float | None]] = []
        for inner in levels_j:
            key = (step, inner)
            wall_samples = walls[key]
            cpu_samples = cpus.get(key)
            eff_samples = effs.get(key)
            thr_samples = thrs.get(key)
            raw_levels.append(
                (
                    inner,
                    len(wall_samples),
                    _robust_median(wall_samples),
                    _robust_median(cpu_samples) if cpu_samples else None,
                    _robust_median(eff_samples) if eff_samples else None,
                    _robust_median(thr_samples) if thr_samples else None,
                )
            )
        result[step] = _build_step_speedup(step, raw_levels, core_budget)
    return result


# --------------------------------------------------------------------------- planner


@dataclass(frozen=True)
class PlanEntry:
    """The resolved estimate + planner metadata for one step (what the plan display shows)."""

    tag: str
    est_duration_s: float
    est_source: str  # "store" | "hint" | "default"
    rss_estimate_bytes: int | None
    rss_source: str  # "store" | "hint" | "none"
    bottom_level_s: float
    samples: int
    #: The learned parallel-speedup curve for this step, or ``None`` when the store has fewer than
    #: two inner_jobs widths for it (nothing to model).
    speedup: "StepSpeedup | None" = None


@dataclass(frozen=True)
class Plan:
    """A complete plan: per-step resolved estimates, the dispatch order, and the critical path."""

    planner: Planner
    order: tuple[str, ...]
    critical_path: tuple[str, ...]
    critical_path_length_s: float
    entries: tuple[PlanEntry, ...]

    def by_tag(self) -> dict[str, PlanEntry]:
        return {entry.tag: entry for entry in self.entries}


def _resolved_estimate(
    step: Step, samples: StepSamples | None, min_samples: int
) -> tuple[float, str, int | None, str, int]:
    """Resolve one step's effective ``(est_duration_s, est_source, rss, rss_source, samples)``:
    the store wins when it has enough samples and a value; otherwise the DAG hint, then the
    built-in default."""
    n = samples.samples if samples is not None else 0
    store_ok = samples is not None and n >= min_samples

    if store_ok and samples is not None and samples.est_duration_s is not None:
        est, est_source = samples.est_duration_s, "store"
    elif step.hint.est_duration_s != 0.0:
        est, est_source = step.hint.est_duration_s, "hint"
    else:
        est, est_source = step.hint.est_duration_s, "default"

    if store_ok and samples is not None and samples.rss_estimate_bytes is not None:
        rss, rss_source = samples.rss_estimate_bytes, "store"
    elif step.hint.rss_baseline_bytes is not None:
        rss, rss_source = step.hint.rss_baseline_bytes, "hint"
    else:
        rss, rss_source = None, "none"

    return est, est_source, rss, rss_source, n


def _successors(cfg: DagConfig) -> dict[str, list[str]]:
    """Map each step tag to the tags that DEPEND on it (its downstream successors), in cfg order."""
    succ: dict[str, list[str]] = {step.tag: [] for step in cfg.steps}
    for step in cfg.steps:
        for dep in step.deps:
            if dep in succ:
                succ[dep].append(step.tag)
    return succ


def _bottom_levels(
    cfg: DagConfig, est: Mapping[str, float], successors: Mapping[str, list[str]]
) -> dict[str, float]:
    """Bottom-level (longest remaining est-weighted path to a sink) for every step.

    ``bottom_level(v) = est(v) + max(bottom_level(w) for w in successors(v))`` (just ``est(v)`` at
    a sink). Memoized DFS; the DAG is acyclic so recursion terminates."""
    bottom: dict[str, float] = {}

    def visit(tag: str) -> float:
        cached = bottom.get(tag)
        if cached is not None:
            return cached
        succ = successors.get(tag, [])
        value = est.get(tag, 0.0)
        if succ:
            value += max(visit(w) for w in succ)
        bottom[tag] = value
        return value

    for step in cfg.steps:
        visit(step.tag)
    return bottom


def _critical_path(
    cfg: DagConfig, bottom: Mapping[str, float], successors: Mapping[str, list[str]]
) -> tuple[list[str], float]:
    """The longest est-weighted path through the DAG: start at the step with the greatest
    bottom-level (tie-break by tag) and follow the successor with the greatest bottom-level (same
    tie-break) to a sink. Returns ``(path, length)``; empty for an empty DAG."""
    if not cfg.steps:
        return [], 0.0
    tags = [step.tag for step in cfg.steps]
    start = max(tags, key=lambda t: (bottom.get(t, 0.0), _neg_key(t)))
    path = [start]
    current = start
    while True:
        succ = successors.get(current, [])
        if not succ:
            break
        current = max(succ, key=lambda t: (bottom.get(t, 0.0), _neg_key(t)))
        path.append(current)
    return path, bottom.get(start, 0.0)


class _NegKey:
    """Reverse-ordering wrapper so ``max(..., key=...)`` breaks bottom-level ties by SMALLEST tag
    (ascending), matching the ``(-bottom_level, tag)`` sort key used for dispatch order."""

    __slots__ = ("value",)

    def __init__(self, value: str) -> None:
        self.value = value

    def __lt__(self, other: "_NegKey") -> bool:
        return self.value > other.value

    def __eq__(self, other: object) -> bool:
        return isinstance(other, _NegKey) and self.value == other.value


def _neg_key(tag: str) -> _NegKey:
    return _NegKey(tag)


def _plan_order(
    cfg: DagConfig, planner: Planner, est: Mapping[str, float], bottom: Mapping[str, float]
) -> list[str]:
    """The deterministic dispatch order the scheduler considers ready steps in.

    * ``greedy-lpt``: by ``est_duration`` DESCENDING, ties keeping registration (cfg) order — a
      STABLE reverse sort, matching the scheduler's built-in default.
    * ``critical-path``: by ``bottom_level`` DESCENDING, ties broken by tag ASCENDING — so among
      ready steps the one on the longest remaining path is launched first (critical-path-first
      list scheduling).
    Both are identical across the Python and Rust builds for the same estimates + DAG."""
    tags = [step.tag for step in cfg.steps]
    if planner is Planner.CRITICAL_PATH:
        return sorted(tags, key=lambda t: (-bottom.get(t, 0.0), t))
    # greedy-lpt: stable reverse sort by est (ties keep registration order).
    return sorted(tags, key=lambda t: est.get(t, 0.0), reverse=True)


def build_plan(
    cfg: DagConfig,
    store_samples: Mapping[str, StepSamples],
    *,
    planner: Planner = Planner.GREEDY_LPT,
    min_samples: int = DEFAULT_MIN_SAMPLES,
    speedups: Mapping[str, StepSpeedup] | None = None,
) -> Plan:
    """Resolve every step's estimate (store-over-hint-over-default) and build the plan for
    ``planner``: the per-step resolved estimates, the critical path, and the dispatch order.

    ``speedups`` (from :func:`load_step_speedups`) attaches each step's learned parallel-speedup
    curve for the plan display; it does not change the dispatch order (the co-scheduling
    inner_jobs allocation planner is a scoped follow-on)."""
    speedups = speedups or {}
    resolved: dict[str, tuple[float, str, int | None, str, int]] = {}
    est: dict[str, float] = {}
    for step in cfg.steps:
        r = _resolved_estimate(step, store_samples.get(step.tag), min_samples)
        resolved[step.tag] = r
        est[step.tag] = r[0]
    successors = _successors(cfg)
    bottom = _bottom_levels(cfg, est, successors)
    critical, length = _critical_path(cfg, bottom, successors)
    order = _plan_order(cfg, planner, est, bottom)
    entries = tuple(
        PlanEntry(
            tag=step.tag,
            est_duration_s=resolved[step.tag][0],
            est_source=resolved[step.tag][1],
            rss_estimate_bytes=resolved[step.tag][2],
            rss_source=resolved[step.tag][3],
            bottom_level_s=bottom[step.tag],
            samples=resolved[step.tag][4],
            speedup=speedups.get(step.tag),
        )
        for step in cfg.steps
    )
    return Plan(
        planner=planner,
        order=tuple(order),
        critical_path=tuple(critical),
        critical_path_length_s=length,
        entries=entries,
    )


def apply_plan_to_config(cfg: DagConfig, plan: Plan) -> DagConfig:
    """Return a copy of ``cfg`` whose per-step hints carry the plan's resolved estimates.

    ``est_duration_s`` is set for every step (store value when the store won, else the unchanged
    hint value), and ``rss_baseline_bytes`` is overridden ONLY when the store won (so a hint-only
    or unmeasured baseline is preserved). This is what feeds both the scheduler's ordering and the
    memory-aware ``--max-mem`` sizing, so planning improves automatically as runs accumulate."""
    by_tag = plan.by_tag()
    new_steps: list[Step] = []
    for step in cfg.steps:
        entry = by_tag.get(step.tag)
        if entry is None:
            new_steps.append(step)
            continue
        rss = (
            entry.rss_estimate_bytes
            if entry.rss_source == "store"
            else step.hint.rss_baseline_bytes
        )
        new_hint = ResourceHint(
            resources=step.hint.resources,
            est_duration_s=entry.est_duration_s,
            rss_baseline_bytes=rss,
            hard_mem_max_bytes=step.hint.hard_mem_max_bytes,
            classification=step.hint.classification,
            preferred_inner_jobs=step.hint.preferred_inner_jobs,
            measured_effective_cores=step.hint.measured_effective_cores,
            measured_cpu_utilization=step.hint.measured_cpu_utilization,
        )
        new_steps.append(
            Step(
                group=step.group,
                job=step.job,
                desc=step.desc,
                cmd=step.cmd,
                description=step.description,
                deps=list(step.deps),
                env=dict(step.env),
                hint=new_hint,
                networkonly=step.networkonly,
                engine_only=step.engine_only,
                timeout=step.timeout,
                jobs_flag=step.jobs_flag,
            )
        )
    return DagConfig(
        steps=tuple(new_steps),
        description=cfg.description,
        resource_caps=cfg.resource_caps,
        mem_cap_factor=cfg.mem_cap_factor,
        mem_cap_floor_bytes=cfg.mem_cap_floor_bytes,
        outer_mem_safety_factor=cfg.outer_mem_safety_factor,
        default_step_timeout=cfg.default_step_timeout,
        default_jobs_flag=cfg.default_jobs_flag,
    )


# --------------------------------------------------------------------------- rendering


def _fmt_secs(value: float) -> str:
    """Fixed 3-decimal seconds, byte-identical to the Rust ``format!(\"{:.3}\")``."""
    return f"{value:.3f}"


def _opt_secs_json(value: float | None) -> str:
    """JSON value for an optional fixed-3-decimal number: ``null`` or a quoted string (so parity
    does not depend on float ``repr``)."""
    return "null" if value is None else f'"{_fmt_secs(value)}"'


def _speedup_level_json(level: SpeedupLevel) -> str:
    """One speedup-curve level as a single-line JSON object (byte-identical across builds)."""
    return (
        f'{{"inner_jobs": {level.inner_jobs}, "wall_s": "{_fmt_secs(level.wall_s)}", '
        f'"speedup": "{_fmt_secs(level.speedup)}", "cpu_s": {_opt_secs_json(level.cpu_s)}, '
        f'"effective_cores": {_opt_secs_json(level.effective_cores)}, '
        f'"throttled_s": {_opt_secs_json(level.throttled_s)}, "samples": {level.samples}}}'
    )


def _speedup_to_json(speedup: "StepSpeedup | None") -> str:
    """The ``"speedup"`` field value for a step in the plan JSON: ``null`` or a nested object with
    the recommended width, achieved cores, and the full measured curve. Indented to embed after
    ``\"speedup\": `` at the step object's 6-space field indent."""
    if speedup is None:
        return "null"
    levels = ",\n".join(f"          {_speedup_level_json(level)}" for level in speedup.levels)
    return (
        "{\n"
        f'        "baseline_inner_jobs": {speedup.baseline_inner_jobs},\n'
        f'        "recommended_inner_jobs": {speedup.recommended_inner_jobs},\n'
        f'        "measured_effective_cores": {_opt_secs_json(speedup.measured_effective_cores)},\n'
        '        "levels": [\n'
        f"{levels}\n"
        "        ]\n"
        "      }"
    )


def plan_to_json(plan: Plan) -> str:
    """Canonical, machine-readable plan JSON (2-space indent), byte-identical across builds.

    Computed floats are emitted as fixed-3-decimal STRINGS (not JSON numbers), so parity does not
    depend on float ``repr`` — only on the shared fixed-precision formatting. Steps are listed in
    dispatch order."""
    by_tag = plan.by_tag()
    steps_json: list[str] = []
    for tag in plan.order:
        entry = by_tag[tag]
        rss = "null" if entry.rss_estimate_bytes is None else str(entry.rss_estimate_bytes)
        steps_json.append(
            "    {\n"
            f'      "tag": {_json_str(entry.tag)},\n'
            f'      "est_duration_s": "{_fmt_secs(entry.est_duration_s)}",\n'
            f'      "est_source": {_json_str(entry.est_source)},\n'
            f'      "rss_estimate_bytes": {rss},\n'
            f'      "rss_source": {_json_str(entry.rss_source)},\n'
            f'      "bottom_level_s": "{_fmt_secs(entry.bottom_level_s)}",\n'
            f'      "samples": {entry.samples},\n'
            f'      "speedup": {_speedup_to_json(entry.speedup)}\n'
            "    }"
        )
    parts = [
        "{",
        f'  "planner": {_json_str(plan.planner.value)},',
        f'  "critical_path": {_json_str_list(plan.critical_path)},',
        f'  "critical_path_length_s": "{_fmt_secs(plan.critical_path_length_s)}",',
        f'  "order": {_json_str_list(plan.order)},',
    ]
    if steps_json:
        parts.append('  "steps": [')
        parts.append(",\n".join(steps_json))
        parts.append("  ]")
    else:
        parts.append('  "steps": []')
    parts.append("}")
    return "\n".join(parts)


def _json_str(value: str) -> str:
    """JSON-quote/escape a string exactly like the Rust ``json_str`` (ASCII control set)."""
    out = ['"']
    for ch in value:
        code = ord(ch)
        if ch == '"':
            out.append('\\"')
        elif ch == "\\":
            out.append("\\\\")
        elif ch == "\n":
            out.append("\\n")
        elif ch == "\t":
            out.append("\\t")
        elif ch == "\r":
            out.append("\\r")
        elif ch == "\b":
            out.append("\\b")
        elif ch == "\f":
            out.append("\\f")
        elif code < 0x20:
            out.append(f"\\u{code:04x}")
        else:
            out.append(ch)
    out.append('"')
    return "".join(out)


def _json_str_list(items: Sequence[str]) -> str:
    """A JSON string array formatted like ``json.dumps(indent=2)`` at 2-space base indent."""
    if not items:
        return "[]"
    inner = ",\n".join(f"    {_json_str(item)}" for item in items)
    return "[\n" + inner + "\n  ]"


def _human_bytes(n: int | None) -> str:
    if n is None:
        return "-"
    value = float(n)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if value < 1024.0 or unit == "TiB":
            return f"{int(value)} B" if unit == "B" else f"{value:.1f} {unit}"
        value /= 1024.0
    return f"{n} B"


def plan_to_text(plan: Plan) -> str:
    """A compact, human-readable plan for the terminal: the planner, a per-step estimate table
    (with the SOURCE of each estimate), the critical path, and the scheduled order.

    Deterministic (no color) so it is also cross-checkable; the CLI wraps it in optional color."""
    by_tag = plan.by_tag()
    headers = [
        "step",
        "est_duration_s",
        "source",
        "rss_estimate",
        "rss_source",
        "bottom_level_s",
        "samples",
    ]
    rows: list[list[str]] = []
    for tag in plan.order:
        entry = by_tag[tag]
        rows.append(
            [
                tag,
                _fmt_secs(entry.est_duration_s),
                entry.est_source,
                _human_bytes(entry.rss_estimate_bytes),
                entry.rss_source,
                _fmt_secs(entry.bottom_level_s),
                str(entry.samples),
            ]
        )
    widths = [len(h) for h in headers]
    for row in rows:
        for i, cell in enumerate(row):
            widths[i] = max(widths[i], len(cell))

    def fmt(cells: Sequence[str]) -> str:
        return "  ".join(
            f"{cells[i]:<{widths[i]}}" if i == 0 else f"{cells[i]:>{widths[i]}}"
            for i in range(len(headers))
        )

    lines = [
        f"plan: {plan.planner.value}",
        "per-step estimates (source: store = learned from the profile store; "
        "hint = DAG-authored; default = none):",
        fmt(headers),
        "  ".join("-" * w for w in widths),
    ]
    lines.extend(fmt(row) for row in rows)
    crit = " -> ".join(plan.critical_path) if plan.critical_path else "(none)"
    lines.append(f"critical path ({_fmt_secs(plan.critical_path_length_s)}s): {crit}")
    lines.append("scheduled order: " + (", ".join(plan.order) if plan.order else "(none)"))
    lines.extend(_speedup_text_lines(plan))
    return "\n".join(lines) + "\n"


def _speedup_text_lines(plan: Plan) -> list[str]:
    """The optional parallel-speedup section for :func:`plan_to_text`: one row per step that HAS a
    learned curve (>=2 inner_jobs widths), showing the recommended width, achieved cores, the
    speedup at that (knee) width, and the full ``inner_jobs->speedup`` curve. Empty (no lines) when
    no step has a model, so a store without multi-width samples renders exactly as before."""
    by_tag = plan.by_tag()
    modeled = [(tag, by_tag[tag].speedup) for tag in plan.order if by_tag[tag].speedup is not None]
    if not modeled:
        return []
    headers = ["step", "rec_inner_jobs", "eff_cores", "speedup@rec", "curve(inner_jobs->speedup)"]
    rows: list[list[str]] = []
    for tag, speedup in modeled:
        assert speedup is not None  # narrowed by the filter above (for the type checker)
        knee = next(
            (lvl.speedup for lvl in speedup.levels if lvl.inner_jobs == speedup.recommended_inner_jobs),
            1.0,
        )
        eff = (
            f"{speedup.measured_effective_cores:.3f}"
            if speedup.measured_effective_cores is not None
            else "-"
        )
        curve = " ".join(f"{lvl.inner_jobs}:{lvl.speedup:.2f}x" for lvl in speedup.levels)
        rows.append(
            [tag, str(speedup.recommended_inner_jobs), eff, f"{knee:.2f}x", curve]
        )
    widths = [len(h) for h in headers]
    for row in rows:
        for i, cell in enumerate(row):
            widths[i] = max(widths[i], len(cell))

    def fmt(cells: Sequence[str]) -> str:
        return "  ".join(
            f"{cells[i]:<{widths[i]}}" if i == 0 else f"{cells[i]:>{widths[i]}}"
            for i in range(len(headers))
        )

    out = [
        "",
        "parallel-speedup model (recommended inner_jobs = best wall within the knee + core budget; "
        "speedup@rec = speedup at that width):",
        fmt(headers),
        "  ".join("-" * w for w in widths),
    ]
    out.extend(fmt(row) for row in rows)
    return out
