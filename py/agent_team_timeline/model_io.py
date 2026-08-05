"""Strict loading of the normalized provider-neutral archive snapshot."""

from __future__ import annotations

from collections.abc import Mapping, Sequence

from agent_team_timeline.model import (
    Agent,
    Edge,
    Event,
    SourceSnapshot,
    TeamData,
    ToolCall,
    Turn,
)


def _object(value: object, where: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise ValueError(f"{where}: expected an object")
    result: dict[str, object] = {}
    for key, item in value.items():
        if not isinstance(key, str):
            raise ValueError(f"{where}: object key is not a string")
        result[key] = item
    return result


def _array(value: object, where: str) -> list[object]:
    if not isinstance(value, list):
        raise ValueError(f"{where}: expected an array")
    return list(value)


def _string(value: object, where: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{where}: expected a string")
    return value


def _optional_string(value: object, where: str) -> str | None:
    if value is None:
        return None
    return _string(value, where)


def _integer(value: object, where: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise ValueError(f"{where}: expected an integer")
    return value


def _optional_integer(value: object, where: str) -> int | None:
    if value is None:
        return None
    return _integer(value, where)


def _items(root: Mapping[str, object], key: str) -> list[dict[str, object]]:
    return [
        _object(value, f"team.{key}[{index}]")
        for index, value in enumerate(_array(root.get(key), f"team.{key}"))
    ]


def team_from_json_obj(value: object) -> TeamData:
    """Reconstruct :class:`TeamData` written by ``TeamData.to_json_obj``."""

    root = _object(value, "team")
    sources = tuple(
        SourceSnapshot(
            path=_string(item.get("path"), "source.path"),
            thread_id=_string(item.get("thread_id"), "source.thread_id"),
            size_bytes=_integer(item.get("size_bytes"), "source.size_bytes"),
            mtime_ns=_integer(item.get("mtime_ns"), "source.mtime_ns"),
            sha256=_string(item.get("sha256"), "source.sha256"),
            complete_bytes=_integer(item.get("complete_bytes"), "source.complete_bytes"),
            line_count=_integer(item.get("line_count"), "source.line_count"),
        )
        for item in _items(root, "sources")
    )
    agents = tuple(
        Agent(
            thread_id=_string(item.get("thread_id"), "agent.thread_id"),
            parent_thread_id=_optional_string(
                item.get("parent_thread_id"), "agent.parent_thread_id"
            ),
            agent_path=_string(item.get("agent_path"), "agent.agent_path"),
            nickname=_optional_string(item.get("nickname"), "agent.nickname"),
            role=_optional_string(item.get("role"), "agent.role"),
            depth=_integer(item.get("depth"), "agent.depth"),
            started_at_ms=_integer(item.get("started_at_ms"), "agent.started_at_ms"),
            ended_at_ms=_optional_integer(item.get("ended_at_ms"), "agent.ended_at_ms"),
            status=_string(item.get("status"), "agent.status"),
            source_path=_string(item.get("source_path"), "agent.source_path"),
        )
        for item in _items(root, "agents")
    )
    turns = tuple(
        Turn(
            turn_id=_string(item.get("turn_id"), "turn.turn_id"),
            thread_id=_string(item.get("thread_id"), "turn.thread_id"),
            started_at_ms=_integer(item.get("started_at_ms"), "turn.started_at_ms"),
            ended_at_ms=_optional_integer(item.get("ended_at_ms"), "turn.ended_at_ms"),
            status=_string(item.get("status"), "turn.status"),
            first_token_ms=_optional_integer(item.get("first_token_ms"), "turn.first_token_ms"),
            error=_optional_string(item.get("error"), "turn.error"),
            last_agent_message=_optional_string(
                item.get("last_agent_message"), "turn.last_agent_message"
            ),
        )
        for item in _items(root, "turns")
    )
    events = tuple(
        Event(
            event_id=_string(item.get("event_id"), "event.event_id"),
            thread_id=_string(item.get("thread_id"), "event.thread_id"),
            turn_id=_optional_string(item.get("turn_id"), "event.turn_id"),
            timestamp_ms=_integer(item.get("timestamp_ms"), "event.timestamp_ms"),
            kind=_string(item.get("kind"), "event.kind"),
            role=_optional_string(item.get("role"), "event.role"),
            phase=_optional_string(item.get("phase"), "event.phase"),
            text=_optional_string(item.get("text"), "event.text"),
            content_availability=_string(
                item.get("content_availability"), "event.content_availability"
            ),
            encrypted_content=_optional_string(
                item.get("encrypted_content"), "event.encrypted_content"
            ),
            author=_optional_string(item.get("author"), "event.author"),
            recipient=_optional_string(item.get("recipient"), "event.recipient"),
            source_line=_integer(item.get("source_line"), "event.source_line"),
        )
        for item in _items(root, "events")
    )
    tools: list[ToolCall] = []
    for item in _items(root, "tool_calls"):
        nested = tuple(
            (
                _string(entry.get("name"), "tool.nested_tools.name"),
                _integer(entry.get("count"), "tool.nested_tools.count"),
            )
            for entry in (
                _object(value, "tool.nested_tools[]")
                for value in _array(item.get("nested_tools"), "tool.nested_tools")
            )
        )
        tools.append(
            ToolCall(
                call_id=_string(item.get("call_id"), "tool.call_id"),
                item_id=_optional_string(item.get("item_id"), "tool.item_id"),
                thread_id=_string(item.get("thread_id"), "tool.thread_id"),
                turn_id=_optional_string(item.get("turn_id"), "tool.turn_id"),
                name=_string(item.get("name"), "tool.name"),
                namespace=_optional_string(item.get("namespace"), "tool.namespace"),
                started_at_ms=_integer(item.get("started_at_ms"), "tool.started_at_ms"),
                ended_at_ms=_optional_integer(item.get("ended_at_ms"), "tool.ended_at_ms"),
                status=_string(item.get("status"), "tool.status"),
                input_text=_optional_string(item.get("input_text"), "tool.input_text"),
                output_text=_optional_string(item.get("output_text"), "tool.output_text"),
                nested_tools=nested,
                source_line=_integer(item.get("source_line"), "tool.source_line"),
            )
        )
    edges = tuple(
        Edge(
            edge_id=_string(item.get("edge_id"), "edge.edge_id"),
            call_id=_string(item.get("call_id"), "edge.call_id"),
            from_thread_id=_string(item.get("from_thread_id"), "edge.from_thread_id"),
            to_thread_id=_string(item.get("to_thread_id"), "edge.to_thread_id"),
            kind=_string(item.get("kind"), "edge.kind"),
            timestamp_ms=_integer(item.get("timestamp_ms"), "edge.timestamp_ms"),
            message_text=_optional_string(item.get("message_text"), "edge.message_text"),
            content_availability=_string(
                item.get("content_availability"), "edge.content_availability"
            ),
            encrypted_content=_optional_string(
                item.get("encrypted_content"), "edge.encrypted_content"
            ),
            source_line=_integer(item.get("source_line"), "edge.source_line"),
        )
        for item in _items(root, "edges")
    )
    return TeamData(
        team_slug=_string(root.get("team_slug"), "team.team_slug"),
        provider=_string(root.get("provider"), "team.provider"),
        root_thread_id=_string(root.get("root_thread_id"), "team.root_thread_id"),
        display_timezone=_string(root.get("display_timezone"), "team.display_timezone"),
        sources=sources,
        agents=agents,
        turns=turns,
        events=events,
        tool_calls=tuple(tools),
        edges=edges,
    )


__all__ = ["team_from_json_obj"]
