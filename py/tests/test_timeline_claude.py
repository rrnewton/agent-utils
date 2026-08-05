"""Provider tests for Claude Code transcript ingestion."""

from __future__ import annotations

from pathlib import Path
import shutil

import pytest

from agent_team_timeline.claude import (
    ClaudeParseError,
    ClaudeSourceCopy,
    discover_claude_sources,
    load_claude_team,
    snapshot_claude_lineage,
)


SESSION_ID = "11111111-1111-4111-8111-111111111111"
FIXTURE_ROOT = Path(__file__).parent / "fixtures" / "claude"
DAY_TWO_MS = 1_767_312_000_000
DAY_THREE_MS = 1_767_398_400_000


def _session(root: Path = FIXTURE_ROOT) -> Path:
    return root / f"{SESSION_ID}.jsonl"


def test_discovers_root_nested_agent_logs_and_metadata() -> None:
    relative = [
        path.relative_to(FIXTURE_ROOT).as_posix()
        for path in discover_claude_sources(_session())
    ]

    assert relative == [
        f"{SESSION_ID}.jsonl",
        f"{SESSION_ID}/subagents/agent-a-child.jsonl",
        f"{SESSION_ID}/subagents/agent-a-child.meta.json",
        f"{SESSION_ID}/subagents/agent-a-nested.jsonl",
        f"{SESSION_ID}/subagents/agent-a-nested.meta.json",
    ]


def test_loads_provider_neutral_nested_team_and_joins_tools() -> None:
    team = load_claude_team(
        _session(), "claude-fixture", "UTC"
    )

    assert team.provider == "claude"
    assert team.root_thread_id == SESSION_ID
    assert len(team.sources) == 5
    assert len(team.agents) == 3
    agents = {agent.thread_id: agent for agent in team.agents}
    assert agents[SESSION_ID].agent_path == "/root"
    assert agents["a-child"].parent_thread_id == SESSION_ID
    assert agents["a-child"].depth == 1
    assert agents["a-child"].role == "researcher"
    assert agents["a-child"].agent_path == "/root/audit_release_evidence"
    assert agents["a-nested"].parent_thread_id == "a-child"
    assert agents["a-nested"].depth == 2
    assert agents["a-nested"].agent_path.endswith("/check_test_receipts")

    tools = {tool.call_id: tool for tool in team.tool_calls}
    assert set(tools) == {
        "tool-spawn-child",
        "tool-bash-root",
        "tool-message-child",
        "tool-spawn-nested",
        "tool-read-nested",
    }
    assert tools["tool-bash-root"].status == "completed"
    assert tools["tool-bash-root"].output_text == "checks passed"
    assert tools["tool-read-nested"].ended_at_ms is not None

    assert [(edge.kind, edge.from_thread_id, edge.to_thread_id) for edge in team.edges] == [
        ("spawn", SESSION_ID, "a-child"),
        ("spawn", "a-child", "a-nested"),
        ("followup", SESSION_ID, "a-child"),
    ]
    assert len(team.turns) == 4
    assert all(event.turn_id is not None for event in team.events)
    assert "private reasoning" not in "\n".join(
        event.text or "" for event in team.events
    )


def test_half_open_window_clips_activity_and_retains_ancestors() -> None:
    team = load_claude_team(
        _session(),
        "claude-window",
        "UTC",
        start_ms=DAY_TWO_MS,
        end_ms=DAY_THREE_MS,
    )

    assert {agent.thread_id for agent in team.agents} == {
        SESSION_ID,
        "a-child",
        "a-nested",
    }
    assert all(DAY_TWO_MS <= event.timestamp_ms < DAY_THREE_MS for event in team.events)
    assert all(tool.call_id != "tool-bash-root" for tool in team.tool_calls)
    root_spawn = next(edge for edge in team.edges if edge.to_thread_id == "a-child")
    assert root_spawn.timestamp_ms == DAY_TWO_MS
    assert all(agent.started_at_ms >= DAY_TWO_MS for agent in team.agents)
    assert any("overnight audit" in (event.text or "") for event in team.events)


def test_trailing_incomplete_json_is_ignored(tmp_path: Path) -> None:
    copied = tmp_path / "claude"
    shutil.copytree(FIXTURE_ROOT, copied)
    path = _session(copied)
    with path.open("ab") as handle:
        handle.write(b'{"type":"assistant"')

    team = load_claude_team(path, "claude-tail", "UTC")

    assert len(team.agents) == 3
    assert team.sources[0].complete_bytes < team.sources[0].size_bytes


def test_complete_malformed_record_is_rejected(tmp_path: Path) -> None:
    copied = tmp_path / "claude"
    shutil.copytree(FIXTURE_ROOT, copied)
    path = _session(copied)
    with path.open("ab") as handle:
        handle.write(b'{"type": bad}\n')

    with pytest.raises(ClaudeParseError, match="invalid JSON"):
        load_claude_team(path, "claude-bad", "UTC")


def test_window_without_activity_is_rejected() -> None:
    with pytest.raises(ClaudeParseError, match="contains no transcript activity"):
        load_claude_team(
            _session(),
            "claude-empty",
            "UTC",
            start_ms=DAY_THREE_MS,
            end_ms=DAY_THREE_MS + 86_400_000,
        )


def test_snapshot_is_idempotent_and_accepts_only_monotonic_append(
    tmp_path: Path,
) -> None:
    live = tmp_path / "live"
    shutil.copytree(FIXTURE_ROOT, live)
    snapshot = tmp_path / "snapshot"

    first = snapshot_claude_lineage(
        _session(live), snapshot, (), "2026-01-03T00:00:00Z"
    )
    second = snapshot_claude_lineage(
        _session(live), snapshot, first.sources, "2026-01-03T01:00:00Z"
    )

    assert first.files_changed == 5
    assert second.files_changed == 0
    assert second.sources == first.sources

    root = _session(live)
    with root.open("ab") as handle:
        handle.write(
            b'{"type":"system","timestamp":"2026-01-02T10:02:00.000Z",'
            b'"sessionId":"11111111-1111-4111-8111-111111111111"}\n'
        )
    advanced = snapshot_claude_lineage(
        root, snapshot, second.sources, "2026-01-03T02:00:00Z"
    )
    assert advanced.files_changed == 1
    root_copy = next(
        source for source in advanced.sources if source.source_path == root.name
    )
    assert root_copy.copied_bytes > first.sources[0].copied_bytes

    root.write_bytes(root.read_bytes()[:100])
    with pytest.raises(ClaudeParseError, match="truncated or rewritten"):
        snapshot_claude_lineage(
            root, snapshot, advanced.sources, "2026-01-03T03:00:00Z"
        )


def test_source_copy_json_validation_rejects_identity_change(tmp_path: Path) -> None:
    live = tmp_path / "live"
    shutil.copytree(FIXTURE_ROOT, live)
    first = snapshot_claude_lineage(
        _session(live), tmp_path / "snapshot", (), "2026-01-03T00:00:00Z"
    )
    value = first.sources[0].to_json_obj()

    assert ClaudeSourceCopy.from_json_obj(value, "source") == first.sources[0]
    value["thread_id"] = "../different"
    with pytest.raises(ClaudeParseError, match="unsafe thread id"):
        ClaudeSourceCopy.from_json_obj(value, "source")
