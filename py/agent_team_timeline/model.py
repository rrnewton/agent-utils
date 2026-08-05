"""Typed, JSON-serializable data model for agent-team timelines."""

from __future__ import annotations

import hashlib
from dataclasses import dataclass

from agent_team_timeline.archive import JsonValue, canonical_json


JsonObject = dict[str, object]


@dataclass(frozen=True)
class SourceSnapshot:
    """The exact complete-line snapshot consumed from one live rollout file."""

    path: str
    thread_id: str
    size_bytes: int
    mtime_ns: int
    sha256: str
    complete_bytes: int
    line_count: int

    def to_json_obj(self) -> JsonObject:
        return {
            "path": self.path,
            "thread_id": self.thread_id,
            "size_bytes": self.size_bytes,
            "mtime_ns": self.mtime_ns,
            "sha256": self.sha256,
            "complete_bytes": self.complete_bytes,
            "line_count": self.line_count,
        }


@dataclass(frozen=True)
class Agent:
    """One coordinator or subagent thread in a Codex team lineage."""

    thread_id: str
    parent_thread_id: str | None
    agent_path: str
    nickname: str | None
    role: str | None
    depth: int
    started_at_ms: int
    ended_at_ms: int | None
    status: str
    source_path: str

    def to_json_obj(self) -> JsonObject:
        return {
            "thread_id": self.thread_id,
            "parent_thread_id": self.parent_thread_id,
            "agent_path": self.agent_path,
            "nickname": self.nickname,
            "role": self.role,
            "depth": self.depth,
            "started_at_ms": self.started_at_ms,
            "ended_at_ms": self.ended_at_ms,
            "status": self.status,
            "source_path": self.source_path,
        }


@dataclass(frozen=True)
class Turn:
    """A model turn, including its authoritative lifecycle boundary."""

    turn_id: str
    thread_id: str
    started_at_ms: int
    ended_at_ms: int | None
    status: str
    first_token_ms: int | None
    error: str | None
    last_agent_message: str | None

    def to_json_obj(self) -> JsonObject:
        return {
            "turn_id": self.turn_id,
            "thread_id": self.thread_id,
            "started_at_ms": self.started_at_ms,
            "ended_at_ms": self.ended_at_ms,
            "status": self.status,
            "first_token_ms": self.first_token_ms,
            "error": self.error,
            "last_agent_message": self.last_agent_message,
        }


@dataclass(frozen=True)
class Event:
    """Canonical transcript or lifecycle event (UI duplicate records excluded)."""

    event_id: str
    thread_id: str
    turn_id: str | None
    timestamp_ms: int
    kind: str
    role: str | None
    phase: str | None
    text: str | None
    content_availability: str
    encrypted_content: str | None
    author: str | None
    recipient: str | None
    source_line: int

    def to_json_obj(self) -> JsonObject:
        return {
            "event_id": self.event_id,
            "thread_id": self.thread_id,
            "turn_id": self.turn_id,
            "timestamp_ms": self.timestamp_ms,
            "kind": self.kind,
            "role": self.role,
            "phase": self.phase,
            "text": self.text,
            "content_availability": self.content_availability,
            "encrypted_content": self.encrypted_content,
            "author": self.author,
            "recipient": self.recipient,
            "source_line": self.source_line,
        }


@dataclass(frozen=True)
class ToolCall:
    """One model tool invocation joined to its output by Codex ``call_id``."""

    call_id: str
    item_id: str | None
    thread_id: str
    turn_id: str | None
    name: str
    namespace: str | None
    started_at_ms: int
    ended_at_ms: int | None
    status: str
    input_text: str | None
    output_text: str | None
    nested_tools: tuple[tuple[str, int], ...]
    source_line: int

    def to_json_obj(self) -> JsonObject:
        nested: list[JsonObject] = [
            {"name": name, "count": count} for name, count in self.nested_tools
        ]
        return {
            "call_id": self.call_id,
            "item_id": self.item_id,
            "thread_id": self.thread_id,
            "turn_id": self.turn_id,
            "name": self.name,
            "namespace": self.namespace,
            "started_at_ms": self.started_at_ms,
            "ended_at_ms": self.ended_at_ms,
            "status": self.status,
            "input_text": self.input_text,
            "output_text": self.output_text,
            "nested_tools": nested,
            "source_line": self.source_line,
        }


@dataclass(frozen=True)
class Edge:
    """A spawn or later coordinator/subagent interaction."""

    edge_id: str
    call_id: str
    from_thread_id: str
    to_thread_id: str
    kind: str
    timestamp_ms: int
    message_text: str | None
    content_availability: str
    encrypted_content: str | None
    source_line: int

    def to_json_obj(self) -> JsonObject:
        return {
            "edge_id": self.edge_id,
            "call_id": self.call_id,
            "from_thread_id": self.from_thread_id,
            "to_thread_id": self.to_thread_id,
            "kind": self.kind,
            "timestamp_ms": self.timestamp_ms,
            "message_text": self.message_text,
            "content_availability": self.content_availability,
            "encrypted_content": self.encrypted_content,
            "source_line": self.source_line,
        }


@dataclass(frozen=True)
class TeamData:
    """A deterministic, provider-neutral snapshot ready for JSON emission."""

    team_slug: str
    provider: str
    root_thread_id: str
    display_timezone: str
    sources: tuple[SourceSnapshot, ...]
    agents: tuple[Agent, ...]
    turns: tuple[Turn, ...]
    events: tuple[Event, ...]
    tool_calls: tuple[ToolCall, ...]
    edges: tuple[Edge, ...]

    def to_json_obj(self) -> JsonObject:
        return {
            "team_slug": self.team_slug,
            "provider": self.provider,
            "root_thread_id": self.root_thread_id,
            "display_timezone": self.display_timezone,
            "sources": [item.to_json_obj() for item in self.sources],
            "agents": [item.to_json_obj() for item in self.agents],
            "turns": [item.to_json_obj() for item in self.turns],
            "events": [item.to_json_obj() for item in self.events],
            "tool_calls": [item.to_json_obj() for item in self.tool_calls],
            "edges": [item.to_json_obj() for item in self.edges],
        }


def source_digest(team: TeamData) -> str:
    """Canonical digest of exactly the complete source prefixes consumed for a team."""

    snapshots: list[JsonValue] = [
        {
            "thread_id": source.thread_id,
            "complete_bytes": source.complete_bytes,
            "sha256": source.sha256,
        }
        for source in team.sources
    ]
    material = canonical_json(snapshots)
    return hashlib.sha256(material.encode("utf-8")).hexdigest()


__all__ = [
    "Agent",
    "Edge",
    "Event",
    "JsonObject",
    "SourceSnapshot",
    "TeamData",
    "ToolCall",
    "Turn",
    "source_digest",
]
