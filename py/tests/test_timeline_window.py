from __future__ import annotations

import json
from datetime import datetime, timezone
from pathlib import Path

import pytest

from agent_team_timeline.archive import narrow_json, write_json_if_changed
from agent_team_timeline.model import Agent, Event, SourceSnapshot, TeamData
from agent_team_timeline.model_io import team_from_json_obj
from agent_team_timeline.phases import build_phases
from agent_team_timeline.pipeline import build_archive, summarize_archive
from agent_team_timeline.window import apply_date_window, parse_date_window


ROOT = "root-thread"
OUTSIDE = "outside-thread"


def _ms(value: str) -> int:
    return int(datetime.fromisoformat(value).astimezone(timezone.utc).timestamp() * 1000)


def _event(event_id: str, thread_id: str, timestamp_ms: int, text: str) -> Event:
    return Event(
        event_id=event_id,
        thread_id=thread_id,
        turn_id=None,
        timestamp_ms=timestamp_ms,
        kind="assistant_message",
        role="assistant",
        phase=None,
        text=text,
        content_availability="plaintext",
        encrypted_content=None,
        author=None,
        recipient=None,
        source_line=1,
    )


def _team() -> TeamData:
    start_ms = _ms("2026-07-21T04:00:00+00:00")
    end_ms = _ms("2026-07-22T04:00:00+00:00")
    return TeamData(
        team_slug="window-test",
        provider="fixture",
        root_thread_id=ROOT,
        display_timezone="America/New_York",
        sources=(SourceSnapshot("fixture", ROOT, 1, 1, "a" * 64, 1, 1),),
        agents=(
            Agent(ROOT, None, "/root", None, None, 0, start_ms - 10_000, None, "active", "x"),
            Agent(
                OUTSIDE,
                None,
                "/outside",
                None,
                None,
                0,
                end_ms + 1_000,
                None,
                "active",
                "y",
            ),
        ),
        turns=(),
        events=(
            _event("prior", ROOT, start_ms - 1_000, "Prior terminology context"),
            _event("start", ROOT, start_ms, "Included at the start"),
            _event("last", ROOT, end_ms - 1, "Included before the end"),
            _event("end", ROOT, end_ms, "Excluded at the end"),
            _event("outside", OUTSIDE, end_ms + 1_000, "Unrelated later work"),
        ),
        tool_calls=(),
        edges=(),
    )


def test_local_dates_become_half_open_utc_bounds_with_dst() -> None:
    spring = parse_date_window("2026-03-08", "2026-03-09", "America/New_York")
    assert spring is not None
    assert spring.end_ms is not None and spring.start_ms is not None
    assert spring.end_ms - spring.start_ms == 23 * 60 * 60 * 1000

    fall = parse_date_window("2026-11-01", "2026-11-02", "America/New_York")
    assert fall is not None
    assert fall.end_ms is not None and fall.start_ms is not None
    assert fall.end_ms - fall.start_ms == 25 * 60 * 60 * 1000


@pytest.mark.parametrize(
    ("start", "end", "zone", "message"),
    [
        ("07/21/2026", "2026-07-22", "UTC", "YYYY-MM-DD"),
        ("2026-07-22", "2026-07-22", "UTC", "earlier"),
        ("2026-07-23", "2026-07-22", "UTC", "earlier"),
        ("2026-07-21", "2026-07-22", "Mars/Olympus", "unknown display timezone"),
    ],
)
def test_invalid_date_windows_are_rejected(
    start: str, end: str, zone: str, message: str
) -> None:
    with pytest.raises(ValueError, match=message):
        parse_date_window(start, end, zone)


def test_phases_use_prior_context_but_exclude_both_outside_sides() -> None:
    window = parse_date_window("2026-07-21", "2026-07-22", "America/New_York")
    assert window is not None
    phases = build_phases(apply_date_window(_team(), window), phase_minutes=30)

    transcript = "\n".join(phase.transcript_text for phase in phases)
    context = "\n".join(phase.prior_context for phase in phases)
    assert "Included at the start" in transcript
    assert "Included before the end" in transcript
    assert "Prior terminology context" not in transcript
    assert "Excluded at the end" not in transcript
    assert "Unrelated later work" not in transcript
    assert "Prior terminology context" in context
    assert {phase.agent_id for phase in phases} == {ROOT}


def test_built_site_uses_exact_window_and_only_selected_agents(tmp_path: Path) -> None:
    window = parse_date_window("2026-07-21", "2026-07-22", "America/New_York")
    assert window is not None
    team = apply_date_window(_team(), window)
    raw_path = tmp_path / "teams" / team.team_slug / "raw" / "team.json"
    write_json_if_changed(raw_path, narrow_json(team.to_json_obj()))

    report = summarize_archive(tmp_path, team.team_slug, "heuristic", "test-model")
    built = build_archive(tmp_path, team.team_slug)
    timeline = json.loads(
        (tmp_path / "data" / "timeline.json").read_text(encoding="utf-8")
    )

    assert report.agent_names == 1
    assert built["agents"] == 1
    assert timeline["range"] == {
        "start_ms": window.start_ms,
        "end_ms": window.end_ms,
    }
    assert [agent["id"] for agent in timeline["agents"]] == [ROOT]
    assert all(event["at_ms"] < window.end_ms for event in timeline["events"])
    assert [rollup["kind"] for rollup in timeline["rollups"]].count("daily") == 1
    daily = next(rollup for rollup in timeline["rollups"] if rollup["kind"] == "daily")
    assert "partial" not in daily["label"]


def test_team_json_round_trip_keeps_window_fields() -> None:
    window = parse_date_window("2026-07-21", "2026-07-22", "America/New_York")
    assert window is not None
    bounded = apply_date_window(_team(), window)
    restored = team_from_json_obj(bounded.to_json_obj())
    assert restored.window_start_ms == window.start_ms
    assert restored.window_end_ms == window.end_ms
