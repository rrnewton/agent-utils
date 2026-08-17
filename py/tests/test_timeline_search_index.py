from __future__ import annotations

from agent_team_timeline.model import Agent, Event, TeamData, ToolCall
from agent_team_timeline.search_index import build_search_records


START = 1_800_000_000_000
ROOT = "root"


def _event(
    event_id: str,
    at_ms: int,
    kind: str,
    text: str,
    *,
    turn_id: str | None,
    author_kind: str | None,
) -> Event:
    return Event(
        event_id=event_id,
        thread_id=ROOT,
        turn_id=turn_id,
        timestamp_ms=at_ms,
        kind=kind,
        role="user" if kind == "user_prompt" else "assistant",
        phase=None,
        text=text,
        content_availability="plaintext",
        encrypted_content=None,
        author=None,
        recipient=None,
        source_line=at_ms - START,
        author_kind=author_kind,
    )


def _team() -> TeamData:
    return TeamData(
        team_slug="alpha",
        provider="codex",
        root_thread_id=ROOT,
        display_timezone="UTC",
        sources=(),
        agents=(
            Agent(
                ROOT,
                None,
                "/root",
                "Coordinator",
                None,
                0,
                START,
                START + 10_000,
                "completed",
                "session.jsonl",
            ),
        ),
        turns=(),
        events=(
            _event(
                "owner-prompt",
                START + 1_000,
                "user_prompt",
                "Measure backend maturity.",
                turn_id="turn-1",
                author_kind="owner_human",
            ),
            _event(
                "answer",
                START + 2_000,
                "assistant_message",
                "DBI is a solid B3 at 130/152.",
                turn_id="turn-1",
                author_kind="agent",
            ),
            _event(
                "unlinked",
                START + 3_000,
                "assistant_message",
                "A scheduled update has no prompt.",
                turn_id="turn-2",
                author_kind="agent",
            ),
        ),
        tool_calls=(
            ToolCall(
                call_id="call-1",
                item_id=None,
                thread_id=ROOT,
                turn_id="turn-1",
                name="exec_command",
                namespace="functions",
                started_at_ms=START + 1_500,
                ended_at_ms=START + 1_600,
                status="completed",
                input_text="secret raw command",
                output_text="secret raw output",
                nested_tools=(("bash", 2), ("git", 1)),
                source_line=3,
            ),
        ),
        edges=(),
    )


def test_search_records_link_responses_and_condense_tools() -> None:
    records = build_search_records(_team(), frozenset({ROOT}))

    assert [record["role"] for record in records] == [
        "user",
        "tool",
        "assistant",
        "assistant",
    ]
    prompt, tool, response, unlinked = records
    prompt_ref = "message:alpha::owner-prompt"
    assert prompt["ref"] == prompt_ref
    assert prompt["prompt_ref"] == prompt_ref
    assert prompt["prompt_author_kind"] == "owner_human"
    assert tool["text"] == "3 tools used: 2 bash, 1 git"
    assert "secret" not in str(tool)
    assert tool["prompt_ref"] == prompt_ref
    assert response["prompt_ref"] == prompt_ref
    assert response["prompt_author_kind"] == "owner_human"
    assert unlinked["prompt_ref"] is None


def test_search_records_are_windowed_and_support_combined_agent_ids() -> None:
    records = build_search_records(
        _team(),
        frozenset({ROOT}),
        namespace_agents=True,
        start_ms=START + 1_500,
        end_ms=START + 2_500,
    )

    assert [record["ref"] for record in records] == [
        "tool:alpha::call-1",
        "message:alpha::answer",
    ]
    assert all(record["agent_id"] == "alpha::root" for record in records)
    assert records[1]["prompt_ref"] == "message:alpha::owner-prompt"


def test_search_records_reject_an_empty_window() -> None:
    try:
        build_search_records(
            _team(),
            frozenset({ROOT}),
            start_ms=START,
            end_ms=START,
        )
    except ValueError as error:
        assert str(error) == "search record start must be earlier than end"
    else:
        raise AssertionError("empty search window should fail")
