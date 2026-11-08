"""`summarize --agent-scope coordinator`: the coordinator's own sessions, apart from the team's."""

from __future__ import annotations

import json
import re
from dataclasses import replace
from datetime import datetime, timezone
from pathlib import Path

import pytest

from tests.test_timeline_pipeline import CHILD, ROOT, START, _team, _write_team
from tests.test_timeline_claude_history import _write_history
from wrkviz.agent_scope import (
    COORDINATOR_SCOPE_NOTE,
    coordinator_thread_ids,
    restrict_to_agent_scope,
    select_agent_scope,
)
from wrkviz.claude_history import load_claude_history_team
from wrkviz.cli import main as timeline_main
from wrkviz.model import Agent, Edge, Event, TeamData
from wrkviz.pipeline import _summary_root, plan_summaries, summarize_archive
from wrkviz.periods import Period
from wrkviz.render import _local_time, _rollup_item_stamp
from wrkviz.window import parse_date_window
from zoneinfo import ZoneInfo

CONTINUATION = "00000000-0000-0000-0000-000000000003"
CONTINUATION_CHILD = "00000000-0000-0000-0000-000000000004"


def _edge(edge_id: str, source: str, target: str, kind: str, offset_ms: int) -> Edge:
    return Edge(
        edge_id, edge_id, source, target, kind, START + offset_ms, None, "plaintext", None, 1
    )


def _event(event_id: str, thread_id: str, offset_ms: int, text: str) -> Event:
    return Event(
        event_id=event_id,
        thread_id=thread_id,
        turn_id=None,
        timestamp_ms=START + offset_ms,
        kind="assistant_message",
        role="assistant",
        phase="commentary",
        text=text,
        content_availability="plaintext",
        encrypted_content=None,
        author=thread_id,
        recipient=None,
        source_line=1,
    )


def _continued_team() -> TeamData:
    """The fixture team, continued in a second coordinator session that spawns its own child."""

    team = _team()
    return replace(
        team,
        agents=team.agents
        + (
            Agent(
                CONTINUATION, ROOT, "/root/continuation-x", None, "coordinator", 1,
                START + 24_000, START + 40_000, "completed", "continuation",
            ),
            Agent(
                CONTINUATION_CHILD, CONTINUATION, "/root/continuation-x/helper", "Bo", None, 2,
                START + 26_000, START + 30_000, "completed", "continuation-child",
            ),
        ),
        events=team.events
        + (
            _event("continuation-work", CONTINUATION, 25_000, "The continued coordinator resumed."),
            _event("continuation-child-work", CONTINUATION_CHILD, 27_000, "Helper-only detail."),
        ),
        edges=team.edges
        + (
            _edge("continue-1", ROOT, CONTINUATION, "continuation", 23_500),
            _edge("spawn-2", CONTINUATION, CONTINUATION_CHILD, "spawn", 26_000),
        ),
    )


def test_coordinator_threads_are_the_root_and_its_continuation_chain() -> None:
    team = _continued_team()

    assert coordinator_thread_ids(team) == {ROOT, CONTINUATION}
    narrowed = restrict_to_agent_scope(team, "coordinator")
    assert {agent.thread_id for agent in narrowed.agents} == {ROOT, CONTINUATION}
    assert {event.thread_id for event in narrowed.events} == {ROOT, CONTINUATION}
    assert {tool.thread_id for tool in narrowed.tool_calls} == set()
    assert {turn.thread_id for turn in narrowed.turns} == {ROOT}
    assert [edge.kind for edge in narrowed.edges] == ["continuation"]
    # `all` is the identity, not an equal copy: the default path is untouched.
    assert restrict_to_agent_scope(team, "all") is team
    with pytest.raises(ValueError, match="unknown agent scope"):
        restrict_to_agent_scope(team, "children")


def _phase_agents(directory: Path) -> set[str]:
    return {
        json.loads(path.read_text(encoding="utf-8"))["agent_id"]
        for path in directory.glob("*.json")
    }


def test_coordinator_scope_drops_child_phases_and_keeps_its_own_tree(tmp_path: Path) -> None:
    team = _continued_team()
    _write_team(tmp_path, team)
    team_dir = tmp_path / "teams" / team.team_slug

    narrowed = summarize_archive(
        tmp_path, team.team_slug, "heuristic", "test-model", agent_scope="coordinator"
    )

    scope_root = team_dir / "summary_scopes" / "coordinator"
    assert _phase_agents(scope_root / "phases") == {ROOT, CONTINUATION}
    assert narrowed.phases == 2
    assert narrowed.agent_scope == "coordinator"
    assert narrowed.to_json_obj()["agent_scope"] == "coordinator"
    # Nothing of the coordinator-only run lands where the full team's summaries live.
    assert not (team_dir / "summary_data").exists()
    state = json.loads((scope_root / "summarize_state.json").read_text(encoding="utf-8"))
    assert state["request"]["agent_scope"] == "coordinator"

    full = summarize_archive(tmp_path, team.team_slug, "heuristic", "test-model")

    assert _phase_agents(team_dir / "summary_data" / "phases") == {
        ROOT, CHILD, CONTINUATION, CONTINUATION_CHILD,
    }
    assert full.phases == 4
    assert full.agent_scope == "all"
    assert "agent_scope" not in full.to_json_obj()
    full_state = json.loads(
        (team_dir / "summary_data" / "summarize_state.json").read_text(encoding="utf-8")
    )
    assert "agent_scope" not in full_state["request"]
    # Separate trees mean separate caches: the full run generated every summary it needed itself,
    # including the coordinator phases the narrowed run had already written in its own tree.
    assert full.cache_hits == 0
    assert full.cache_misses == (
        full.phases + 2 * full.rollups + full.agent_names + full.project_overviews
    )
    assert _phase_agents(scope_root / "phases") == {ROOT, CONTINUATION}


def test_coordinator_rollup_markdown_says_it_is_not_full_team(tmp_path: Path) -> None:
    team = _continued_team()
    _write_team(tmp_path, team)

    summarize_archive(
        tmp_path,
        team.team_slug,
        "heuristic",
        "test-model",
        agent_scope="coordinator",
        rollup_kinds=("daily", "weekly"),
    )

    summaries = tmp_path / "teams" / team.team_slug / "summary_scopes" / "coordinator" / "summaries"
    weekly_pages = sorted((summaries / "weekly").rglob("*.md"))
    daily = sorted((summaries / "daily").rglob("*.md"))
    assert weekly_pages and daily
    assert any(page.name.endswith("-weekly-plain-language.md") for page in weekly_pages)
    for page in (*weekly_pages, *daily):
        text = page.read_text(encoding="utf-8")
        heading, _, note = text.splitlines()[:3]
        assert "coordinator-only" in heading
        assert note == f"> {COORDINATOR_SCOPE_NOTE}"
    # A weekly item is anchored at the start of the day it came from, so both of its pages give
    # that local date, item by item; a daily item keeps the phase time it came from.
    zone = ZoneInfo(team.display_timezone)
    rollups = _summary_root(tmp_path, team.team_slug, "coordinator") / "rollups" / "weekly"
    checked = 0
    for rollup_path in sorted(rollups.glob("*.json")):
        rollup = json.loads(rollup_path.read_text(encoding="utf-8"))
        for audience, key in (("weekly", "technical_summary"), ("weekly-plain-language", "plain_language_summary")):
            (page,) = [path for path in weekly_pages if path.name.endswith(f"-{audience}.md")
                       and path.name.startswith(rollup["key"])]
            stamps = re.findall(r"^- \*\*(.+?)\*\* — ", page.read_text(encoding="utf-8"), re.MULTILINE)
            expected = [
                datetime.fromtimestamp(item["at_ms"] / 1000, tz=timezone.utc)
                .astimezone(zone)
                .strftime("%Y-%m-%d %a")
                for item in rollup[key]["work_summary"]
            ]
            assert stamps and stamps == expected
            checked += 1
    assert checked == 2 * len(list(rollups.glob("*.json"))) > 0
    daily_items = [
        line
        for page in daily
        for line in page.read_text(encoding="utf-8").splitlines()
        if line.startswith("- **")
    ]
    assert daily_items and all(re.match(r"- \*\*\S+ \d{2}:\d{2}:\d{2} ", line) for line in daily_items)
    # The website build never sees the scoped tree.
    assert not (tmp_path / "teams" / team.team_slug / "summaries").exists()


def test_plan_and_cli_accept_the_scope(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    team = _continued_team()
    _write_team(tmp_path, team)

    narrowed = plan_summaries(
        tmp_path, team.team_slug, "heuristic", "test-model", agent_scope="coordinator"
    )
    full = plan_summaries(tmp_path, team.team_slug, "heuristic", "test-model")
    assert narrowed.stages[0].total == 2
    assert full.stages[0].total == 4

    common = [
        "summarize", "--output", str(tmp_path), "--team", team.team_slug,
        "--backend", "heuristic", "--rollup-kind", "daily",
    ]
    assert timeline_main([*common, "--agent-scope", "coordinator"]) == 0
    capsys.readouterr()
    runs = sorted((tmp_path / "runs").glob("*.json"))
    receipt = json.loads(runs[-1].read_text(encoding="utf-8"))
    assert receipt["summaries"]["agent_scope"] == "coordinator"
    with pytest.raises(SystemExit):
        timeline_main([*common, "--agent-scope", "children"])


def _tree_digest(root: Path) -> dict[str, bytes]:
    return {
        str(path.relative_to(root)): path.read_bytes()
        for path in sorted(root.rglob("*"))
        if path.is_file()
    }


def test_coordinator_run_after_a_full_run_leaves_summary_data_untouched(tmp_path: Path) -> None:
    team = _continued_team()
    _write_team(tmp_path, team)
    summarize_archive(tmp_path, team.team_slug, "heuristic", "test-model")
    full_root = _summary_root(tmp_path, team.team_slug)
    before = _tree_digest(full_root)

    narrowed = summarize_archive(
        tmp_path, team.team_slug, "heuristic", "test-model", agent_scope="coordinator",
        rollup_kinds=("daily",),
    )

    assert narrowed.phases == 2
    assert _tree_digest(full_root) == before
    rollups = sorted((_summary_root(tmp_path, team.team_slug, "coordinator") / "rollups").rglob("*.json"))
    assert rollups
    for path in rollups:
        assert json.loads(path.read_text(encoding="utf-8"))["agent_scope"] == "coordinator"
    for path in sorted((full_root / "rollups").rglob("*.json")):
        assert "agent_scope" not in json.loads(path.read_text(encoding="utf-8"))


def test_the_kept_selection_is_reported_and_recorded(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    team = _continued_team()
    _write_team(tmp_path, team)

    _, selection = select_agent_scope(team, "coordinator")
    assert selection is not None
    assert selection.kept_thread_ids == (ROOT, CONTINUATION)
    assert selection.total_agents == 4
    assert select_agent_scope(team, "all") == (team, None)

    common = [
        "summarize", "--output", str(tmp_path), "--team", team.team_slug,
        "--backend", "heuristic", "--rollup-kind", "daily", "--agent-scope", "coordinator",
    ]
    line = f"agent scope coordinator: kept 2 of 4 agents ({ROOT}, {CONTINUATION})"
    assert timeline_main([*common, "--plan"]) == 0
    assert line in capsys.readouterr().out
    assert timeline_main(common) == 0
    # Once under the plan the run prints before its first model call, once after the run.
    assert capsys.readouterr().out.count(line) == 2
    receipt = json.loads(sorted((tmp_path / "runs").glob("*.json"))[-1].read_text(encoding="utf-8"))
    recorded = receipt["summaries"]["agent_scope_selection"]
    assert recorded == {
        "agent_scope": "coordinator",
        "kept_agents": 2,
        "total_agents": 4,
        "kept_thread_ids": [ROOT, CONTINUATION],
    }
    state_path = _summary_root(tmp_path, team.team_slug, "coordinator") / "summarize_state.json"
    state = json.loads(state_path.read_text(encoding="utf-8"))
    assert state["request"]["agent_scope_selection"] == recorded
    # The default scope records none of it.
    assert timeline_main(common[:-2]) == 0
    capsys.readouterr()
    receipt = json.loads(sorted((tmp_path / "runs").glob("*.json"))[-1].read_text(encoding="utf-8"))
    assert "agent_scope_selection" not in receipt["summaries"]


def test_a_team_without_its_root_among_its_agents_is_refused(tmp_path: Path) -> None:
    team = replace(_continued_team(), root_thread_id="not-an-agent")

    with pytest.raises(ValueError, match="root thread not-an-agent is not among its 4 agents"):
        select_agent_scope(team, "coordinator")
    # Only the narrowed scope needs the root; `all` is still the identity.
    assert restrict_to_agent_scope(team, "all") is team


def test_claude_history_teams_are_refused_by_name(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    history = _write_history(tmp_path / "history.jsonl")
    team, _ = load_claude_history_team(history, "history-team", "UTC", project_pattern="widget")
    # Every session is a parentless root with no edges: following continuation edges from the
    # first one would silently keep only the oldest session.
    assert len([agent for agent in team.agents if agent.parent_thread_id is None]) > 1
    with pytest.raises(ValueError, match="claude-history team"):
        select_agent_scope(team, "coordinator")

    _write_team(tmp_path, team)
    status = timeline_main([
        "summarize", "--output", str(tmp_path), "--team", "history-team",
        "--backend", "heuristic", "--agent-scope", "coordinator",
    ])
    assert status != 0
    assert "claude-history team" in capsys.readouterr().err
    assert not (tmp_path / "teams" / "history-team" / "summary_scopes").exists()


def test_a_coordinator_idle_in_the_window_is_not_refused(tmp_path: Path) -> None:
    team = _continued_team()
    _write_team(tmp_path, team)
    idle_window = parse_date_window("2031-01-01", "2031-01-08", "UTC")

    report = summarize_archive(
        tmp_path, team.team_slug, "heuristic", "test-model", agent_scope="coordinator",
        summary_window=idle_window, rollup_kinds=("daily",),
    )

    assert report.phases == 0
    assert report.agent_scope_selection is not None
    assert report.agent_scope_selection.kept_thread_ids == (ROOT, CONTINUATION)


def _period(kind: str, start: datetime, end: datetime) -> Period:
    return Period(kind, kind, kind, int(start.timestamp() * 1000), int(end.timestamp() * 1000), "", False)


def _ms(zone: ZoneInfo, year: int, month: int, day: int, hour: int = 0, minute: int = 0) -> int:
    return int(datetime(year, month, day, hour, minute, tzinfo=zone).timestamp() * 1000)


def test_week_level_items_are_labelled_by_the_day_week_or_month_holding_them() -> None:
    tz = "America/New_York"
    zone = ZoneInfo(tz)

    def ms(year: int, month: int, day: int, hour: int = 0, minute: int = 0) -> int:
        return _ms(zone, year, month, day, hour, minute)

    week = _period("weekly", datetime(2026, 11, 2, tzinfo=zone), datetime(2026, 11, 9, tzinfo=zone))
    october = _period("monthly", datetime(2026, 10, 1, tzinfo=zone), datetime(2026, 11, 1, tzinfo=zone))
    month = _period("monthly", datetime(2026, 11, 1, tzinfo=zone), datetime(2026, 12, 1, tzinfo=zone))
    quarter = _period("quarterly", datetime(2026, 10, 1, tzinfo=zone), datetime(2027, 1, 1, tzinfo=zone))
    day = _period("daily", datetime(2026, 11, 4, tzinfo=zone), datetime(2026, 11, 5, tzinfo=zone))

    # A weekly page: the local day holding the item.
    assert _rollup_item_stamp(ms(2026, 11, 4), week, tz, True) == "2026-11-04 Wed"
    # A monthly page: the ISO week holding the item, by that week's first day in the month,
    # whatever day of the week the item itself falls on...
    assert _rollup_item_stamp(ms(2026, 11, 2), month, tz, True) == "Week 45 · Nov 2"
    assert _rollup_item_stamp(ms(2026, 11, 4, 15), month, tz, True) == "Week 45 · Nov 2"
    assert _rollup_item_stamp(ms(2026, 10, 29, 14), october, tz, True) == "Week 44 · Oct 26"
    assert _rollup_item_stamp(ms(2026, 11, 30), month, tz, True) == "Week 49 · Nov 30"
    # ...clipped to the month when the week began before it...
    assert _rollup_item_stamp(ms(2026, 11, 1, 10, 30), month, tz, True) == "Week 44 · Nov 1"
    # ...and in local time: Sunday evening in New York is already Monday in UTC.
    assert _rollup_item_stamp(ms(2026, 11, 8, 20), month, tz, True) == "Week 45 · Nov 2"
    # A quarterly page: the local month holding the item.
    assert _rollup_item_stamp(ms(2026, 11, 1), quarter, tz, True) == "November 2026"
    assert _rollup_item_stamp(ms(2026, 12, 1), quarter, tz, True) == "December 2026"
    assert _rollup_item_stamp(ms(2026, 12, 31, 19, 30), quarter, tz, True) == "December 2026"
    # Daily items, and every page outside a narrowed scope, keep the time the item carries.
    at = ms(2026, 11, 4, 14, 5)
    assert _rollup_item_stamp(at, day, tz, True) == _local_time(at, tz)
    for period in (week, month, quarter):
        assert _rollup_item_stamp(at, period, tz, False) == _local_time(at, tz)


def test_week_level_labels_east_of_utc_use_the_local_calendar() -> None:
    tz = "Asia/Tokyo"
    zone = ZoneInfo(tz)
    month = _period("monthly", datetime(2026, 11, 1, tzinfo=zone), datetime(2026, 12, 1, tzinfo=zone))
    quarter = _period("quarterly", datetime(2026, 10, 1, tzinfo=zone), datetime(2027, 1, 1, tzinfo=zone))

    # Each of these Tokyo times is still the previous day in UTC: Nov 1 00:00 and 01:00 JST are
    # October 31, and Nov 2 00:00 JST is Sunday Nov 1, in the previous ISO week.
    assert _rollup_item_stamp(_ms(zone, 2026, 11, 1), quarter, tz, True) == "November 2026"
    assert _rollup_item_stamp(_ms(zone, 2026, 11, 2), month, tz, True) == "Week 45 · Nov 2"
    assert _rollup_item_stamp(_ms(zone, 2026, 11, 1, 1), month, tz, True) == "Week 44 · Nov 1"
