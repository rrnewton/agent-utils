"""`summarize --agent-scope coordinator`: the coordinator's own sessions, apart from the team's."""

from __future__ import annotations

import json
from dataclasses import replace
from pathlib import Path

import pytest

from tests.test_timeline_pipeline import CHILD, ROOT, START, _team, _write_team
from wrkviz.agent_scope import (
    COORDINATOR_SCOPE_NOTE,
    coordinator_thread_ids,
    restrict_to_agent_scope,
)
from wrkviz.cli import main as timeline_main
from wrkviz.model import Agent, Edge, Event, TeamData
from wrkviz.pipeline import plan_summaries, summarize_archive

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
    # Separate trees mean separate caches: the full run paid for every phase it needed itself
    # (the heuristic backend still records a cache entry per generated summary).
    assert full.cache_misses > 0
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
    weekly = sorted((summaries / "weekly").rglob("*-weekly.md"))
    daily = sorted((summaries / "daily").rglob("*.md"))
    assert weekly and daily
    for page in (*weekly, *daily):
        text = page.read_text(encoding="utf-8")
        heading, _, note = text.splitlines()[:3]
        assert "coordinator-only" in heading
        assert note == f"> {COORDINATOR_SCOPE_NOTE}"
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
