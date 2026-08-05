"""Provider-neutral local-calendar windows for timeline archives."""

from __future__ import annotations

from dataclasses import dataclass, replace
from datetime import date, datetime, time
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from agent_team_timeline.model import TeamData


@dataclass(frozen=True)
class DateWindow:
    """A half-open UTC interval derived from dates in one display timezone."""

    start_date: str | None
    end_date: str | None
    start_ms: int | None
    end_ms: int | None

    def to_json_obj(self) -> dict[str, object]:
        """Return stable manifest metadata for this local-calendar selection."""

        return {
            "start_date": self.start_date,
            "end_date": self.end_date,
            "start_ms": self.start_ms,
            "end_ms": self.end_ms,
        }

    def contains(self, timestamp_ms: int) -> bool:
        """Return whether a point timestamp is inside this half-open interval."""

        if self.start_ms is not None and timestamp_ms < self.start_ms:
            return False
        return self.end_ms is None or timestamp_ms < self.end_ms

    def overlaps(self, start_ms: int, end_ms: int | None) -> bool:
        """Return whether a non-empty lifetime intersects the interval."""

        effective_end = end_ms if end_ms is not None else start_ms + 1
        if effective_end <= start_ms:
            effective_end = start_ms + 1
        if self.end_ms is not None and start_ms >= self.end_ms:
            return False
        return self.start_ms is None or effective_end > self.start_ms


def _parse_date(raw: str, label: str) -> date:
    try:
        parsed = date.fromisoformat(raw)
    except ValueError as error:
        raise ValueError(f"{label} must be a calendar date in YYYY-MM-DD form") from error
    if parsed.isoformat() != raw:
        raise ValueError(f"{label} must be a calendar date in YYYY-MM-DD form")
    return parsed


def _midnight_ms(value: date, timezone_name: str) -> int:
    try:
        display_timezone = ZoneInfo(timezone_name)
    except ZoneInfoNotFoundError as error:
        raise ValueError(f"unknown display timezone {timezone_name!r}") from error
    local_midnight = datetime.combine(value, time.min, tzinfo=display_timezone)
    return int(local_midnight.timestamp() * 1000)


def parse_date_window(
    start_date: str | None,
    end_date: str | None,
    timezone_name: str,
) -> DateWindow | None:
    """Parse inclusive start and exclusive end dates in ``timezone_name``.

    Either bound may be omitted. When both are absent no window is requested.
    """

    if start_date is None and end_date is None:
        # Validate the timezone even when the archive is unbounded; providers share this gate.
        try:
            ZoneInfo(timezone_name)
        except ZoneInfoNotFoundError as error:
            raise ValueError(f"unknown display timezone {timezone_name!r}") from error
        return None
    parsed_start = _parse_date(start_date, "start date") if start_date is not None else None
    parsed_end = _parse_date(end_date, "end date") if end_date is not None else None
    start_ms = (
        _midnight_ms(parsed_start, timezone_name) if parsed_start is not None else None
    )
    end_ms = _midnight_ms(parsed_end, timezone_name) if parsed_end is not None else None
    if start_ms is not None and end_ms is not None and start_ms >= end_ms:
        raise ValueError("start date must be earlier than end date")
    return DateWindow(
        start_date=start_date,
        end_date=end_date,
        start_ms=start_ms,
        end_ms=end_ms,
    )


def apply_date_window(team: TeamData, window: DateWindow | None) -> TeamData:
    """Attach a display/summarization window while retaining earlier context.

    Providers deliberately keep their complete normalized event set. Phase construction and
    rendering honor these bounds, while summary prompts can still read earlier parent history for
    consistent terminology.
    """

    if window is None:
        return replace(team, window_start_ms=None, window_end_ms=None)
    return replace(
        team,
        window_start_ms=window.start_ms,
        window_end_ms=window.end_ms,
    )


__all__ = ["DateWindow", "apply_date_window", "parse_date_window"]
