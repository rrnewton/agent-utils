"""Deterministic, zero-model transcript search records for static exports."""

from __future__ import annotations

from dataclasses import dataclass

from agent_team_timeline.archive import JsonValue
from agent_team_timeline.model import Event, TeamData, ToolCall


SEARCH_RECORD_SCHEMA_VERSION = 1

_TEXT_ROLES = {
    "user_prompt": "user",
    "assistant_message": "assistant",
    "inter_agent_message": "agent",
    "external_message": "external",
    "goal_updated": "goal",
}


@dataclass(frozen=True)
class _PromptLink:
    at_ms: int
    event_id: str
    author_kind: str | None


def _message_ref(team_slug: str, event_id: str) -> str:
    return f"message:{team_slug}::{event_id}"


def _tool_ref(team_slug: str, call_id: str) -> str:
    return f"tool:{team_slug}::{call_id}"


def _agent_ref(team_slug: str, thread_id: str) -> str:
    return f"agent:{team_slug}::{thread_id}"


def _agent_id(team_slug: str, thread_id: str, namespace_agents: bool) -> str:
    return f"{team_slug}::{thread_id}" if namespace_agents else thread_id


def _event_text(event: Event) -> str:
    if event.content_availability == "encrypted":
        message_type = "message"
        for line in (event.text or "").splitlines():
            if line.startswith("Message Type:"):
                candidate = line.partition(":")[2].strip().lower().replace("_", " ")
                if candidate:
                    message_type = candidate
                break
        route = ""
        if event.author or event.recipient:
            route = f" from {event.author or 'unknown'} to {event.recipient or 'unknown'}"
        return (
            f"[Encrypted Codex collaboration {message_type}{route}; "
            "message body unavailable offline.]"
        )
    return (event.text or "").strip()


def _tool_text(tool: ToolCall) -> str:
    if tool.nested_tools:
        parts = [f"{count} {name}" for name, count in sorted(tool.nested_tools)]
        count = sum(value for _, value in tool.nested_tools)
    else:
        qualified = f"{tool.namespace}.{tool.name}" if tool.namespace else tool.name
        parts = [f"1 {qualified}"]
        count = 1
    noun = "tool" if count == 1 else "tools"
    return f"{count} {noun} used: " + ", ".join(parts)


def _record_type(kind: str) -> str:
    return {
        "user_prompt": "prompt",
        "assistant_message": "response",
        "inter_agent_message": "inter_agent",
        "external_message": "external",
        "goal_updated": "goal",
    }[kind]


def _prompt_lists(team: TeamData) -> tuple[
    dict[tuple[str, str], tuple[_PromptLink, ...]],
    dict[str, tuple[_PromptLink, ...]],
]:
    by_turn_mutable: dict[tuple[str, str], list[_PromptLink]] = {}
    by_thread_mutable: dict[str, list[_PromptLink]] = {}
    for event in sorted(
        team.events, key=lambda item: (item.timestamp_ms, item.source_line, item.event_id)
    ):
        if event.kind != "user_prompt":
            continue
        link = _PromptLink(event.timestamp_ms, event.event_id, event.author_kind)
        by_thread_mutable.setdefault(event.thread_id, []).append(link)
        if event.turn_id is not None:
            by_turn_mutable.setdefault((event.thread_id, event.turn_id), []).append(link)
    return (
        {key: tuple(value) for key, value in by_turn_mutable.items()},
        {key: tuple(value) for key, value in by_thread_mutable.items()},
    )


def _linked_prompt(
    thread_id: str,
    turn_id: str | None,
    at_ms: int,
    by_turn: dict[tuple[str, str], tuple[_PromptLink, ...]],
    by_thread: dict[str, tuple[_PromptLink, ...]],
) -> _PromptLink | None:
    candidates = (
        by_turn.get((thread_id, turn_id), ())
        if turn_id is not None
        else by_thread.get(thread_id, ())
    )
    for prompt in reversed(candidates):
        if prompt.at_ms <= at_ms:
            return prompt
    return None


def _in_scope(
    at_ms: int, start_ms: int | None, end_ms: int | None
) -> bool:
    return (start_ms is None or at_ms >= start_ms) and (
        end_ms is None or at_ms < end_ms
    )


def build_search_records(
    team: TeamData,
    visible_agent_ids: frozenset[str],
    *,
    namespace_agents: bool = False,
    start_ms: int | None = None,
    end_ms: int | None = None,
) -> tuple[dict[str, JsonValue], ...]:
    """Build unique searchable transcript records from canonical events and tool calls.

    Prompt linkage is mechanical: a response or tool record links to the latest preceding prompt
    in the same provider turn, or in the same thread only when no turn identity was recorded.
    Tool payloads remain excluded; their compact names/counts match the full-transcript UI.
    """

    if start_ms is not None and end_ms is not None and start_ms >= end_ms:
        raise ValueError("search record start must be earlier than end")
    agents = {agent.thread_id: agent for agent in team.agents}
    by_turn, by_thread = _prompt_lists(team)
    sortable: list[tuple[int, int, str, dict[str, JsonValue]]] = []
    seen: set[str] = set()

    for event in team.events:
        role = _TEXT_ROLES.get(event.kind)
        if (
            role is None
            or event.thread_id not in visible_agent_ids
            or not _in_scope(event.timestamp_ms, start_ms, end_ms)
        ):
            continue
        text = _event_text(event)
        if not text:
            continue
        reference = _message_ref(team.team_slug, event.event_id)
        if reference in seen:
            raise ValueError(f"duplicate transcript search record {reference}")
        seen.add(reference)
        prompt = (
            _PromptLink(event.timestamp_ms, event.event_id, event.author_kind)
            if event.kind == "user_prompt"
            else _linked_prompt(
                event.thread_id,
                event.turn_id,
                event.timestamp_ms,
                by_turn,
                by_thread,
            )
        )
        agent = agents.get(event.thread_id)
        record: dict[str, JsonValue] = {
            "schema_version": SEARCH_RECORD_SCHEMA_VERSION,
            "ref": reference,
            "record_type": _record_type(event.kind),
            "role": role,
            "team": team.team_slug,
            "agent_id": _agent_id(
                team.team_slug, event.thread_id, namespace_agents
            ),
            "agent_ref": _agent_ref(team.team_slug, event.thread_id),
            "agent_path": agent.agent_path if agent is not None else event.thread_id,
            "event_id": event.event_id,
            "turn_id": event.turn_id,
            "at_ms": event.timestamp_ms,
            "text": text,
            "author_kind": event.author_kind,
            "ingress_kind": event.ingress_kind,
            "prompt_ref": (
                _message_ref(team.team_slug, prompt.event_id)
                if prompt is not None
                else None
            ),
            "prompt_author_kind": prompt.author_kind if prompt is not None else None,
            "content_fidelity": (
                "encrypted-placeholder"
                if event.content_availability == "encrypted"
                else "verbatim"
            ),
        }
        sortable.append((event.timestamp_ms, 1, reference, record))

    for tool in team.tool_calls:
        if (
            tool.thread_id not in visible_agent_ids
            or not _in_scope(tool.started_at_ms, start_ms, end_ms)
        ):
            continue
        reference = _tool_ref(team.team_slug, tool.call_id)
        if reference in seen:
            raise ValueError(f"duplicate transcript search record {reference}")
        seen.add(reference)
        prompt = _linked_prompt(
            tool.thread_id,
            tool.turn_id,
            tool.started_at_ms,
            by_turn,
            by_thread,
        )
        agent = agents.get(tool.thread_id)
        record = {
            "schema_version": SEARCH_RECORD_SCHEMA_VERSION,
            "ref": reference,
            "record_type": "tool",
            "role": "tool",
            "team": team.team_slug,
            "agent_id": _agent_id(
                team.team_slug, tool.thread_id, namespace_agents
            ),
            "agent_ref": _agent_ref(team.team_slug, tool.thread_id),
            "agent_path": agent.agent_path if agent is not None else tool.thread_id,
            "event_id": tool.call_id,
            "turn_id": tool.turn_id,
            "at_ms": tool.started_at_ms,
            "text": _tool_text(tool),
            "author_kind": "agent",
            "ingress_kind": "tool",
            "prompt_ref": (
                _message_ref(team.team_slug, prompt.event_id)
                if prompt is not None
                else None
            ),
            "prompt_author_kind": prompt.author_kind if prompt is not None else None,
            "content_fidelity": "condensed",
        }
        sortable.append((tool.started_at_ms, 0, reference, record))

    sortable.sort(key=lambda item: item[:3])
    return tuple(record for _, _, _, record in sortable)


__all__ = ["SEARCH_RECORD_SCHEMA_VERSION", "build_search_records"]
