"""Turn persisted per-step profiles into memory-admission estimates, conservatively.

The profile store records what each step's cgroup peaked at. It does NOT follow that the peak
is what the step wanted: a step whose ``peak_bytes`` equals the ``memory.max`` that was applied
to it used everything it was allowed, and a step the kernel killed at that ceiling wanted
strictly more. Both are CENSORED observations. Fitting a cap to them ratchets the cap down to
whatever it already was and freezes the mistake, which is why the default planner feedback in
:mod:`safe_ci_dag_runner.estimates` is left alone and this is a separate, opt-in path a caller
must ask for by name.

The rules here are the ones a caller can rely on:

* A censored sample never lowers anything. It is used only as a FLOOR — proof that demand was
  at least that large — never as an estimate of the maximum.
* A sample whose censoring cannot be determined (no applied-cap column, no event counters, no
  peak) is not evidence at all. It is counted and reported, and it never moves the estimate.
* With no uncensored evidence, the static hint is retained and the reason says which of the
  three ways the evidence failed.
* ``hard_mem_max_bytes`` is never touched. An explicit hard cap is an instruction, not a guess.

Every estimate carries its own provenance — the sample counts by verdict, the percentile, the
margin, and the floor — so a plan can show why a number moved or why it did not.

The columns this reads are the ones the writer records per step; see
:data:`safe_ci_dag_runner.perflog.CENSORING_COLUMNS`.
"""

from __future__ import annotations

from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, replace
from enum import Enum
from pathlib import Path

from safe_ci_dag_runner.estimates import (
    _high_percentile,
    _load_store,
    _parse_int,
    _RSS_PCTL_DEN,
    _RSS_PCTL_NUM,
    feedback_identity,
)
from safe_ci_dag_runner.model import DagConfig, Step

__all__ = [
    "DEFAULT_MARGIN_PCT",
    "DEFAULT_MIN_UNCENSORED_SAMPLES",
    "Censoring",
    "PeakObservation",
    "MemoryAdmission",
    "peak_observation_from_row",
    "memory_admission_from_rows",
    "load_memory_admissions",
    "apply_memory_admissions",
]


#: Uncensored samples a step needs before its recorded peaks may replace the authored hint. One
#: sample is a measurement, not a distribution; a cap derived from it would move on every run.
DEFAULT_MIN_UNCENSORED_SAMPLES = 5

#: Headroom added above the percentile, as a percentage. The percentile describes the samples
#: that were taken; the margin covers the run that has not happened yet.
DEFAULT_MARGIN_PCT = 20


class Censoring(Enum):
    """What a single recorded peak proves about the step's memory demand."""

    #: The step ran under a known ceiling it never reached, with no reclaim and no OOM. The peak
    #: is a genuine observation of demand.
    UNCENSORED = "uncensored"
    #: The step was held at, or killed at, a ceiling. The peak is a LOWER BOUND on demand.
    CENSORED = "censored"
    #: The row does not say. Missing peak, missing applied cap, or missing event counters — an
    #: older row, or an unboxed run. It is not evidence in either direction.
    UNKNOWN = "unknown"


@dataclass(frozen=True)
class PeakObservation:
    """One recorded peak, with the verdict on whether it measured demand or a ceiling."""

    step: str
    peak_bytes: int | None
    #: The applied ``memory.max`` in bytes, or ``None`` when the step was unbounded or the cap
    #: was not recorded. :attr:`cap_known` and :attr:`cap_unbounded` disambiguate the two.
    applied_cap_bytes: int | None
    cap_known: bool
    cap_unbounded: bool
    #: ``memory.events`` ``max`` — times the kernel held the step at its ceiling by reclaiming.
    reclaim_events: int | None
    oom_kills: int | None
    verdict: Censoring
    #: ``memory.events`` ``high`` — times the step was throttled into direct reclaim by a SOFT
    #: ceiling. The runner clears ``memory.high`` to ``max``, best-effort; when that write failed
    #: an inherited soft cap was still throttling, and the peak under it is not free demand.
    throttle_events: int | None = None
    #: ``memory.events`` ``oom`` — times the OOM killer was invoked in the cgroup. A step can
    #: record this with ``oom_kill == 0`` (no task was reapable), and it is still a ceiling hit.
    oom_events: int | None = None


def _blank(value: str | None) -> bool:
    return not (value or "").strip()


def peak_observation_from_row(row: Mapping[str, str]) -> PeakObservation:
    """Classify one profile row's recorded peak.

    UNKNOWN whenever the row cannot answer, which is the safe direction: an unknown row is
    excluded from the estimate rather than assumed comfortable. Specifically, a row is UNKNOWN
    when it has no parseable ``peak_bytes``, when ``memory_max_bytes`` is blank (the cap was
    never recorded, so nothing bounds the interpretation), or when the ``memory_events``
    counters are blank (reclaim at the ceiling cannot be ruled out).

    CENSORED when ANY of the four pressure counters #34 records fired — ``high`` (throttled
    into direct reclaim by a soft ceiling), ``max`` (held at the hard ceiling by reclaim),
    ``oom`` (the OOM killer was invoked) or ``oom_kill`` (a task was reaped) — or when the peak
    reached the applied cap. The last is a ``>=`` and not a ``==`` deliberately: a cap the
    kernel rounded down to a page boundary still censors a peak that sits above it.

    ``memory_events_low`` is deliberately NOT read: it counts reclaim that breached a
    ``memory.low`` PROTECTION, which sets a floor the kernel tries to preserve rather than a
    ceiling the cgroup is held under, so it does not bound the peak.

    UNCENSORED only when the cap is known, the step stayed under it, and no counter fired.
    """
    step = (row.get("step") or "").strip()
    peak = _parse_int(row.get("peak_bytes"))
    peak_bytes = peak if (peak is not None and peak >= 0) else None
    cap_cell = (row.get("memory_max_bytes") or "").strip()
    cap_known = bool(cap_cell)
    cap_unbounded = cap_cell == "max"
    applied = None if cap_unbounded else _parse_int(cap_cell)
    applied_cap_bytes = applied if (applied is not None and applied >= 0) else None
    reclaim = _parse_int(row.get("memory_events_max"))
    reclaim_events = reclaim if (reclaim is not None and reclaim >= 0) else None
    oom_cell = row.get("memory_events_oom_kill")
    if _blank(oom_cell):
        # Fall back to the long-standing per-step OOM column, which predates the event counters.
        oom_cell = row.get("oom_kills")
    oom = _parse_int(oom_cell)
    oom_kills = oom if (oom is not None and oom >= 0) else None
    throttle = _parse_int(row.get("memory_events_high"))
    throttle_events = throttle if (throttle is not None and throttle >= 0) else None
    invoked = _parse_int(row.get("memory_events_oom"))
    oom_events = invoked if (invoked is not None and invoked >= 0) else None

    verdict = _verdict(
        peak_bytes=peak_bytes,
        cap_known=cap_known,
        cap_unbounded=cap_unbounded,
        applied_cap_bytes=applied_cap_bytes,
        reclaim_events=reclaim_events,
        oom_kills=oom_kills,
        throttle_events=throttle_events,
        oom_events=oom_events,
        timed_out=_truthy(row.get("timed_out")) or _truthy(row.get("cpu_timed_out")),
    )
    return PeakObservation(
        step=step,
        peak_bytes=peak_bytes,
        applied_cap_bytes=applied_cap_bytes,
        cap_known=cap_known,
        cap_unbounded=cap_unbounded,
        reclaim_events=reclaim_events,
        oom_kills=oom_kills,
        verdict=verdict,
        throttle_events=throttle_events,
        oom_events=oom_events,
    )


def _truthy(value: str | None) -> bool:
    return (value or "").strip().lower() in {"true", "1", "yes"}


def _verdict(
    *,
    peak_bytes: int | None,
    cap_known: bool,
    cap_unbounded: bool,
    applied_cap_bytes: int | None,
    reclaim_events: int | None,
    oom_kills: int | None,
    throttle_events: int | None,
    oom_events: int | None,
    timed_out: bool,
) -> Censoring:
    """The classification rules, separated so they can be read and tested without a CSV row."""
    if peak_bytes is None:
        return Censoring.UNKNOWN
    if oom_kills is not None and oom_kills > 0:
        return Censoring.CENSORED
    if oom_events is not None and oom_events > 0:
        # The OOM killer was invoked even if it reaped nothing reapable: the step was at a
        # ceiling.
        return Censoring.CENSORED
    if timed_out:
        # The step was cut short, so its peak is where it had got to, not where it was going.
        # That is a lower bound on demand, exactly like a ceiling hit.
        return Censoring.CENSORED
    if not cap_known:
        return Censoring.UNKNOWN
    if reclaim_events is None:
        return Censoring.UNKNOWN
    if reclaim_events > 0:
        return Censoring.CENSORED
    if throttle_events is not None and throttle_events > 0:
        # A soft ceiling was throttling. The runner clears memory.high to "max" best-effort, so
        # a non-zero count means that write did not take and an inherited soft cap held the step.
        return Censoring.CENSORED
    if cap_unbounded:
        return Censoring.UNCENSORED
    if applied_cap_bytes is None:
        return Censoring.UNKNOWN
    if peak_bytes >= applied_cap_bytes:
        return Censoring.CENSORED
    return Censoring.UNCENSORED


@dataclass(frozen=True)
class MemoryAdmission:
    """A step's candidate ``rss_baseline_bytes``, and everything needed to justify it.

    ``rss_baseline_bytes`` is ``None`` when the evidence does not support replacing the authored
    hint; ``reason`` then says which way it fell short. When it is not ``None`` the caller may
    use it directly: it already includes the margin and is already raised above every observed
    peak, censored or not.
    """

    step: str
    rss_baseline_bytes: int | None
    #: ``"profile"`` when the samples decided the number, ``"hint"`` when they did not.
    source: str
    reason: str
    samples: int
    uncensored_samples: int
    censored_samples: int
    unknown_samples: int
    #: The percentile of the uncensored peaks used as the central estimate, as ``num/den``.
    percentile: str
    margin_pct: int
    #: The largest CENSORED peak: a proven lower bound on demand that the estimate may not go
    #: below. ``None`` when no sample was censored.
    censored_floor_bytes: int | None
    #: The largest UNCENSORED peak, which the estimate may also not go below.
    observed_peak_bytes: int | None

    def censoring_excluded_samples(self) -> bool:
        """Whether any sample was withheld from the central estimate because of censoring."""
        return self.censored_samples > 0


_PERCENTILE = f"{_RSS_PCTL_NUM}/{_RSS_PCTL_DEN}"


def memory_admission_from_rows(
    rows: Iterable[Mapping[str, str]],
    *,
    min_uncensored_samples: int = DEFAULT_MIN_UNCENSORED_SAMPLES,
    margin_pct: int = DEFAULT_MARGIN_PCT,
    profile_base_sha: str | None = None,
) -> dict[str, MemoryAdmission]:
    """Aggregate profile ``rows`` into one :class:`MemoryAdmission` per step.

    ``profile_base_sha``, when given, restricts the evidence to rows recorded against that
    source revision. A cap learned across a change that altered a step's memory behaviour is a
    cap learned from two different workloads, and the count of rows dropped for this reason is
    reported in the reason string rather than being silently absorbed.

    The estimate for a step with enough uncensored evidence is::

        max(percentile(uncensored peaks) * (1 + margin), max uncensored peak, max censored peak)

    The percentile is the central estimate; the two maxima are floors. A censored peak reaches
    the result ONLY through that floor, so it can raise a cap and can never lower one.
    """
    observations: dict[str, list[PeakObservation]] = {}
    dropped_by_sha: dict[str, int] = {}
    seen_steps: set[str] = set()
    for row in rows:
        observation = peak_observation_from_row(row)
        if not observation.step:
            continue
        seen_steps.add(observation.step)
        if profile_base_sha is not None:
            recorded = (row.get("profile_base_sha") or "").strip()
            if recorded != profile_base_sha:
                dropped_by_sha[observation.step] = dropped_by_sha.get(observation.step, 0) + 1
                continue
        observations.setdefault(observation.step, []).append(observation)

    result: dict[str, MemoryAdmission] = {}
    for step in sorted(seen_steps):
        result[step] = _admission_for_step(
            step,
            observations.get(step, ()),
            dropped_by_sha.get(step, 0),
            min_uncensored_samples=min_uncensored_samples,
            margin_pct=margin_pct,
        )
    return result


def _admission_for_step(
    step: str,
    observations: Sequence[PeakObservation],
    dropped_by_sha: int,
    *,
    min_uncensored_samples: int,
    margin_pct: int,
) -> MemoryAdmission:
    uncensored = [
        o.peak_bytes
        for o in observations
        if o.verdict is Censoring.UNCENSORED and o.peak_bytes is not None
    ]
    censored = [
        o.peak_bytes
        for o in observations
        if o.verdict is Censoring.CENSORED and o.peak_bytes is not None
    ]
    unknown = sum(1 for o in observations if o.verdict is Censoring.UNKNOWN)
    floor = max(censored) if censored else None
    observed = max(uncensored) if uncensored else None
    sha_note = (
        f"; {dropped_by_sha} sample(s) excluded as recorded against another source revision"
        if dropped_by_sha
        else ""
    )
    base = MemoryAdmission(
        step=step,
        rss_baseline_bytes=None,
        source="hint",
        reason="",
        samples=len(observations) + dropped_by_sha,
        uncensored_samples=len(uncensored),
        censored_samples=len(censored),
        unknown_samples=unknown,
        percentile=_PERCENTILE,
        margin_pct=margin_pct,
        censored_floor_bytes=floor,
        observed_peak_bytes=observed,
    )
    if not uncensored:
        if censored and not unknown:
            why = (
                f"every one of {len(censored)} recorded peak(s) was censored by its applied cap"
            )
        elif censored:
            why = (
                f"no uncensored peak: {len(censored)} censored and {unknown} of unknown provenance"
            )
        elif unknown:
            why = (
                f"{unknown} sample(s) carry no applied-cap or event provenance, so censoring "
                "cannot be ruled out"
            )
        else:
            why = "no recorded peaks for this step"
        return replace(base, reason=why + sha_note)
    if len(uncensored) < min_uncensored_samples:
        return replace(
            base,
            reason=(
                f"only {len(uncensored)} uncensored sample(s); {min_uncensored_samples} required"
                + sha_note
            ),
        )
    central = _high_percentile(uncensored)
    with_margin = central + (central * margin_pct) // 100
    estimate = max(with_margin, observed or 0, floor or 0)
    detail = f"{len(uncensored)} uncensored sample(s), {_PERCENTILE} percentile +{margin_pct}%"
    if censored:
        detail += f", floored at the largest of {len(censored)} censored peak(s)"
    if unknown:
        detail += f", {unknown} sample(s) of unknown provenance ignored"
    return replace(
        base,
        rss_baseline_bytes=estimate,
        source="profile",
        reason=detail + sha_note,
    )


def load_memory_admissions(
    profile_dir: str | Path,
    machine_id: str | None = None,
    container_class: str | None = None,
    *,
    min_uncensored_samples: int = DEFAULT_MIN_UNCENSORED_SAMPLES,
    margin_pct: int = DEFAULT_MARGIN_PCT,
    profile_base_sha: str | None = None,
) -> dict[str, MemoryAdmission]:
    """Read the store for one machine/container identity and aggregate its admissions.

    The identity defaults to this host's, via :func:`safe_ci_dag_runner.estimates.
    feedback_identity`, because a cap learned on one container class does not transfer to
    another. Returns ``{}`` when no store exists for that identity, which the caller must read
    as "keep every authored hint", not as "no memory is needed".
    """
    if machine_id is None or container_class is None:
        host_machine, host_container = feedback_identity()
        machine_id = machine_id or host_machine
        container_class = container_class or host_container
    loaded = _load_store(profile_dir, machine_id, container_class)
    if loaded is None:
        return {}
    rows, _affinity = loaded
    return memory_admission_from_rows(
        rows,
        min_uncensored_samples=min_uncensored_samples,
        margin_pct=margin_pct,
        profile_base_sha=profile_base_sha,
    )


def apply_memory_admissions(
    cfg: DagConfig, admissions: Mapping[str, MemoryAdmission]
) -> DagConfig:
    """Return a copy of ``cfg`` whose steps carry the profile-derived ``rss_baseline_bytes``.

    Only a step with an admission whose ``source`` is ``"profile"`` is changed; every other step
    keeps its authored hint verbatim. ``hard_mem_max_bytes`` is carried through untouched in all
    cases: an explicit hard cap is the caller's instruction and this path does not get to
    reinterpret it. Intentionally skipped steps are left alone for the same reason
    :func:`safe_ci_dag_runner.estimates.apply_plan_to_config` leaves them alone — a skip is not
    a licence to erase authored hints.
    """
    new_steps: list[Step] = []
    for step in cfg.steps:
        admission = admissions.get(step.tag)
        if (
            admission is None
            or admission.source != "profile"
            or admission.rss_baseline_bytes is None
            or step.skip_reason is not None
        ):
            new_steps.append(step)
            continue
        new_hint = replace(step.hint, rss_baseline_bytes=admission.rss_baseline_bytes)
        new_steps.append(
            replace(step, hint=new_hint, deps=list(step.deps), env=dict(step.env))
        )
    return cfg.with_steps(new_steps)
