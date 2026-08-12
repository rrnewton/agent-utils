"""Token-free activity bins for semantic timeline zoom levels."""

from __future__ import annotations

from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from datetime import date, datetime, time, timedelta, timezone
from typing import Literal, TypeAlias
from zoneinfo import ZoneInfo

from agent_team_timeline.phases import PhaseWindow


ActivityResolution: TypeAlias = Literal["hourly", "daily", "weekly"]
ActivityRole: TypeAlias = Literal["coordinator", "workers"]
Interval: TypeAlias = tuple[int, int]

_HOUR_MS = 60 * 60 * 1000
_ACTIVE_STATE_KINDS = frozenset({"active", "tool"})
_ROLES: tuple[ActivityRole, ...] = ("coordinator", "workers")
_RESOLUTIONS: tuple[ActivityResolution, ...] = ("hourly", "daily", "weekly")


@dataclass(frozen=True)
class ActivityBin:
    """One non-empty aggregate activity interval for a team role."""

    team: str
    role: ActivityRole
    resolution: ActivityResolution
    start_ms: int
    end_ms: int
    avg_active_concurrency: float
    peak_concurrency: int
    activity_coverage_fraction: float
    distinct_active_agents: int

    def to_json_obj(self) -> dict[str, object]:
        """Return a stable JSON representation consumed by the static site."""

        return {
            "team": self.team,
            "role": self.role,
            "resolution": self.resolution,
            "start_ms": self.start_ms,
            "end_ms": self.end_ms,
            "avg_active_concurrency": self.avg_active_concurrency,
            "peak_concurrency": self.peak_concurrency,
            "activity_coverage_fraction": self.activity_coverage_fraction,
            "distinct_active_agents": self.distinct_active_agents,
        }


def _merge_intervals(intervals: Iterable[Interval]) -> tuple[Interval, ...]:
    ordered = sorted((start, end) for start, end in intervals if end > start)
    merged: list[Interval] = []
    for start, end in ordered:
        if not merged or start > merged[-1][1]:
            merged.append((start, end))
            continue
        previous_start, previous_end = merged[-1]
        merged[-1] = (previous_start, max(previous_end, end))
    return tuple(merged)


def _aligned_start(timestamp_ms: int, size_ms: int, offset_ms: int) -> int:
    return ((timestamp_ms - offset_ms) // size_ms) * size_ms + offset_ms


def _epoch_ms(value: datetime) -> int:
    return int(value.astimezone(timezone.utc).timestamp() * 1000)


def _midnight(day: date, zone: ZoneInfo) -> datetime:
    return datetime.combine(day, time.min, tzinfo=zone)


def _bucket_bounds(
    timestamp_ms: int,
    resolution: ActivityResolution,
    zone: ZoneInfo,
) -> Interval:
    if resolution == "hourly":
        start_ms = _aligned_start(timestamp_ms, _HOUR_MS, 0)
        return start_ms, start_ms + _HOUR_MS

    local_day = datetime.fromtimestamp(
        timestamp_ms / 1000, tz=timezone.utc
    ).astimezone(zone).date()
    if resolution == "weekly":
        local_day -= timedelta(days=local_day.weekday())
        next_day = local_day + timedelta(days=7)
    else:
        next_day = local_day + timedelta(days=1)
    return _epoch_ms(_midnight(local_day, zone)), _epoch_ms(
        _midnight(next_day, zone)
    )


def _peak_concurrency(intervals_by_agent: dict[str, tuple[Interval, ...]]) -> int:
    deltas: dict[int, int] = {}
    for intervals in intervals_by_agent.values():
        for start, end in intervals:
            deltas[start] = deltas.get(start, 0) + 1
            deltas[end] = deltas.get(end, 0) - 1
    current = 0
    peak = 0
    for timestamp_ms in sorted(deltas):
        current += deltas[timestamp_ms]
        peak = max(peak, current)
    return peak


def _build_resolution(
    team: str,
    active_by_role: dict[ActivityRole, dict[str, tuple[Interval, ...]]],
    resolution: ActivityResolution,
    zone: ZoneInfo,
    observed_start_ms: int,
    observed_end_ms: int,
) -> list[ActivityBin]:
    bins: dict[tuple[ActivityRole, int, int], dict[str, list[Interval]]] = {}
    for role in _ROLES:
        for agent_id, intervals in active_by_role[role].items():
            for start, end in intervals:
                cursor_ms = max(start, observed_start_ms)
                effective_end_ms = min(end, observed_end_ms)
                while cursor_ms < effective_end_ms:
                    calendar_start_ms, calendar_end_ms = _bucket_bounds(
                        cursor_ms, resolution, zone
                    )
                    bucket_start_ms = max(calendar_start_ms, observed_start_ms)
                    bucket_end_ms = min(calendar_end_ms, observed_end_ms)
                    clipped_start = max(start, bucket_start_ms)
                    clipped_end = min(end, bucket_end_ms)
                    if clipped_end > clipped_start:
                        agents = bins.setdefault(
                            (role, bucket_start_ms, bucket_end_ms), {}
                        )
                        agents.setdefault(agent_id, []).append(
                            (clipped_start, clipped_end)
                        )
                    cursor_ms = calendar_end_ms

    result: list[ActivityBin] = []
    for role in _ROLES:
        bounds = sorted(
            (start_ms, end_ms)
            for bin_role, start_ms, end_ms in bins
            if bin_role == role
        )
        for start_ms, end_ms in bounds:
            raw_agents = bins[(role, start_ms, end_ms)]
            intervals_by_agent = {
                agent_id: _merge_intervals(intervals)
                for agent_id, intervals in raw_agents.items()
            }
            active_agent_ms = sum(
                end - start
                for intervals in intervals_by_agent.values()
                for start, end in intervals
            )
            if active_agent_ms <= 0:
                continue
            all_intervals = (
                interval
                for intervals in intervals_by_agent.values()
                for interval in intervals
            )
            covered_ms = sum(
                end - start for start, end in _merge_intervals(all_intervals)
            )
            duration_ms = end_ms - start_ms
            if duration_ms <= 0:
                continue
            result.append(
                ActivityBin(
                    team=team,
                    role=role,
                    resolution=resolution,
                    start_ms=start_ms,
                    end_ms=end_ms,
                    avg_active_concurrency=round(active_agent_ms / duration_ms, 6),
                    peak_concurrency=_peak_concurrency(intervals_by_agent),
                    activity_coverage_fraction=round(covered_ms / duration_ms, 6),
                    distinct_active_agents=len(intervals_by_agent),
                )
            )
    return result


def build_activity_bins(
    team: str,
    coordinator_agent_id: str,
    phases: Sequence[PhaseWindow],
    *,
    display_timezone: str,
    observed_start_ms: int,
    observed_end_ms: int,
) -> tuple[ActivityBin, ...]:
    """Aggregate active/tool intervals inside the observed archive range."""

    if observed_end_ms <= observed_start_ms:
        raise ValueError("observed activity range must have positive duration")
    zone = ZoneInfo(display_timezone)

    raw_by_role: dict[ActivityRole, dict[str, list[Interval]]] = {
        "coordinator": {},
        "workers": {},
    }
    for phase in phases:
        role: ActivityRole = (
            "coordinator" if phase.agent_id == coordinator_agent_id else "workers"
        )
        intervals = raw_by_role[role].setdefault(phase.agent_id, [])
        intervals.extend(
            (state.start_ms, state.end_ms)
            for state in phase.states
            if state.kind.lower() in _ACTIVE_STATE_KINDS
            and state.end_ms > state.start_ms
        )

    active_by_role: dict[ActivityRole, dict[str, tuple[Interval, ...]]] = {
        role: {
            agent_id: merged
            for agent_id, intervals in raw_by_role[role].items()
            if (merged := _merge_intervals(intervals))
        }
        for role in _ROLES
    }
    result: list[ActivityBin] = []
    for resolution in _RESOLUTIONS:
        result.extend(
            _build_resolution(
                team,
                active_by_role,
                resolution,
                zone,
                observed_start_ms,
                observed_end_ms,
            )
        )
    return tuple(result)


__all__ = ["ActivityBin", "ActivityResolution", "ActivityRole", "build_activity_bins"]
