"""Import a Muse Code session and the subagents it spawned into the provider-neutral model.

A Muse session directory holds an event-sourced log, ``session.jsonl``: one JSON record per line,
each with a ``recorded_at`` instant in microseconds, a ``payload_type`` and a ``payload``. Spawned
subagents keep their own logs, in the same format, under ``subagent/<child-session-id>/``.
Most records are runtime bookkeeping (hook runs, context diagnostics, resource samples); the
importer reads only these:

* ``runtime.user_intent.accepted`` -- a prompt delivered to the session (``model_messages``);
* run events (``payload.kind == "run"``) keyed by ``run_id`` -- one run is one turn:
  ``assistant_message_committed`` (the agent's text), ``inbox_item_drained`` (a runtime notice
  delivered mid-turn, kept as a system input), ``assistant_tool_calls_committed`` (tool
  name and arguments), ``tool_result_batch_committed`` (results, aligned with the calls of the
  same batch), ``terminal`` (turn end and its status), and a subagent's ``started`` prompt;
* ``tool_batch.effect.started`` / ``.terminal`` -- when each tool call ran and how it ended;
* ``subagent.control.child_session_bound`` and ``.start_attested`` -- which child session a
  ``subagent_spawn`` call created, and through which tool call.

Only children bound by ``child_session_bound`` are imported. The runtime's own observer agents
(reminders and goal checks) also log under ``subagent/`` but are never bound that way, so they
are left out rather than drawn as work.

Ingestion snapshots every selected log before parsing, exactly as for Claude transcripts: the
newline-complete prefix is copied into the snapshot store, a rerun accepts only an append, a log
upstream no longer has is retired (kept from the archive), and a ``.zst`` spelling is read as the
plain file. Parsing reads only the archive-local copies.
"""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from dataclasses import dataclass, replace
import hashlib
import json
from pathlib import Path, PurePosixPath
import re

from wrkviz.archive import write_text_if_changed
from wrkviz.compressed_sources import read_source_bytes, resolve_source, source_exists
from wrkviz.model import Agent, Edge, Event, SourceSnapshot, TeamData, ToolCall, Turn

_SESSION_LOG = "session.jsonl"
_SAFE_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}\Z")
_CLASSIFICATION_VERSION = "authorship-v1"
_SPAWN_TOOL = "subagent_spawn"


class MuseParseError(ValueError):
    """A selected Muse session is malformed or cannot be imported safely."""


@dataclass(frozen=True)
class MuseSourceCopy:
    """One archive-local copy of a Muse session log."""

    source_path: str
    original_path: str
    snapshot_path: str
    thread_id: str
    copied_bytes: int
    line_count: int
    sha256: str
    updated_at: str
    upstream_retired_at: str | None = None

    def to_json_obj(self) -> dict[str, object]:
        """Return this source-copy record as a JSON object."""

        result: dict[str, object] = {
            "source_path": self.source_path,
            "original_path": self.original_path,
            "snapshot_path": self.snapshot_path,
            "thread_id": self.thread_id,
            "copied_bytes": self.copied_bytes,
            "line_count": self.line_count,
            "sha256": self.sha256,
            "updated_at": self.updated_at,
        }
        if self.upstream_retired_at is not None:
            result["upstream_retired_at"] = self.upstream_retired_at
        return result

    @classmethod
    def from_json_obj(cls, raw: Mapping[str, object], where: str) -> MuseSourceCopy:
        """Validate and decode one source-copy record from archive JSON."""

        def text(key: str, *, optional: bool = False) -> str | None:
            value = raw.get(key)
            if value is None and optional:
                return None
            if not isinstance(value, str) or not value:
                raise MuseParseError(f"{where}.{key} must be a non-empty string")
            return value

        def count(key: str) -> int:
            value = raw.get(key)
            if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                raise MuseParseError(f"{where}.{key} must be a non-negative integer")
            return value

        source_path = str(text("source_path"))
        snapshot_path = str(text("snapshot_path"))
        sha256 = str(text("sha256"))
        thread_id = str(text("thread_id"))
        _safe_relative(source_path)
        if snapshot_path != source_path:
            raise MuseParseError(f"{where}: snapshot_path must equal source_path")
        if re.fullmatch(r"[0-9a-f]{64}", sha256) is None:
            raise MuseParseError(f"{where}: invalid source digest")
        _validate_id(thread_id, where)
        return cls(
            source_path=source_path,
            original_path=str(text("original_path")),
            snapshot_path=snapshot_path,
            thread_id=thread_id,
            copied_bytes=count("copied_bytes"),
            line_count=count("line_count"),
            sha256=sha256,
            updated_at=str(text("updated_at")),
            upstream_retired_at=text("upstream_retired_at", optional=True),
        )


@dataclass(frozen=True)
class MuseSnapshotResult:
    """Result of copying a Muse session and its subagents into the snapshot store."""

    sources: tuple[MuseSourceCopy, ...]
    files_changed: int
    upstream_retired: tuple[str, ...] = ()


def _safe_relative(value: str) -> PurePosixPath:
    path = PurePosixPath(value)
    if path.is_absolute() or not path.parts or any(part in ("", ".", "..") for part in path.parts):
        raise MuseParseError(f"unsafe Muse source path {value!r}")
    return path


def _validate_id(value: str, where: str) -> None:
    if _SAFE_ID.fullmatch(value) is None:
        raise MuseParseError(f"{where}: unsafe Muse session id {value!r}")


def _complete_jsonl(data: bytes) -> bytes:
    if data.endswith(b"\n"):
        return data
    newline = data.rfind(b"\n")
    return data[: newline + 1] if newline >= 0 else b""


def _records(data: bytes, where: str) -> list[tuple[int, Mapping[str, object]]]:
    records: list[tuple[int, Mapping[str, object]]] = []
    for line_number, raw in enumerate(data.splitlines(), start=1):
        if not raw.strip():
            continue
        try:
            value = json.loads(raw)
        except (json.JSONDecodeError, UnicodeDecodeError) as exc:
            raise MuseParseError(f"{where}:{line_number}: invalid JSON: {exc}") from exc
        if isinstance(value, dict):
            records.append((line_number, value))
    return records


def _mapping(value: object) -> Mapping[str, object]:
    return value if isinstance(value, dict) else {}


def _string(value: object) -> str | None:
    return value if isinstance(value, str) and value else None


def _at_ms(record: Mapping[str, object]) -> int | None:
    recorded = record.get("recorded_at")
    if isinstance(recorded, int) and not isinstance(recorded, bool) and recorded > 0:
        return recorded // 1000
    return None


def _bound_children(data: bytes, where: str) -> tuple[tuple[str, int], ...]:
    """``(child session id, line)`` for each subagent a log bound to a real spawn."""

    children: list[tuple[str, int]] = []
    for line_number, record in _records(data, where):
        if record.get("payload_type") != "subagent.control.child_session_bound":
            continue
        child = _string(_mapping(_mapping(record.get("payload")).get("record")).get("child_session_id"))
        if child is not None:
            _validate_id(child, f"{where}:{line_number}")
            children.append((child, line_number))
    return tuple(children)


def discover_muse_sources(session_dir: Path) -> tuple[Path, ...]:
    """The session's own log and the logs of every subagent it (transitively) spawned."""

    root_log = session_dir / _SESSION_LOG
    if not source_exists(root_log):
        raise MuseParseError(f"Muse session log is not a regular file: {root_log}")
    _validate_id(session_dir.name, str(session_dir))
    found = [root_log]
    pending = [root_log]
    seen = {session_dir.name}
    while pending:
        log = pending.pop(0)
        for child, _line in _bound_children(read_source_bytes(log), str(log)):
            if child in seen:
                continue
            seen.add(child)
            for candidate in (
                session_dir / "subagent" / child / _SESSION_LOG,
                log.parent / "subagent" / child / _SESSION_LOG,
            ):
                if source_exists(candidate):
                    found.append(candidate)
                    pending.append(candidate)
                    break
    return tuple(found)


def snapshot_muse_lineage(
    session_dir: Path,
    snapshot_root: Path,
    previous_sources: Sequence[MuseSourceCopy],
    updated_at: str,
) -> MuseSnapshotResult:
    """Copy the latest append-only Muse logs before parsing them."""

    previous_by_path = {source.source_path: source for source in previous_sources}
    root_log = session_dir / _SESSION_LOG
    # A session whose log upstream has retired keeps its archived copy; only a session that was
    # never imported and has no log is an error, and a log present but irregular still refuses.
    if resolve_source(root_log) is None and previous_by_path:
        paths: tuple[Path, ...] = ()
    else:
        paths = discover_muse_sources(session_dir)
    relative_to_path = {path.relative_to(session_dir).as_posix(): path for path in paths}
    copies: list[MuseSourceCopy] = []
    retired: list[str] = []
    changed = 0
    for relative in sorted(set(previous_by_path) - set(relative_to_path)):
        recorded = previous_by_path[relative]
        upstream = session_dir.joinpath(*_safe_relative(relative).parts)
        if resolve_source(upstream) is not None:
            raise MuseParseError(
                "append-only source violation: previously observed Muse log is still present "
                f"but no longer reachable from the session: {relative}"
            )
        target = snapshot_root.joinpath(*_safe_relative(relative).parts)
        if target.is_symlink() or not target.is_file():
            raise MuseParseError(
                "previously observed Muse log is gone upstream and the archive's own snapshot is "
                f"missing too: {relative} (expected {target})"
            )
        held = target.read_bytes()
        if (
            len(held) < recorded.copied_bytes
            or hashlib.sha256(held[: recorded.copied_bytes]).hexdigest() != recorded.sha256
        ):
            raise MuseParseError(
                "previously observed Muse log is gone upstream and the archive's own snapshot "
                f"does not match its manifest: {target}"
            )
        retired.append(relative)
        copies.append(
            replace(recorded, upstream_retired_at=recorded.upstream_retired_at or updated_at)
        )
    for relative, source in sorted(relative_to_path.items()):
        _safe_relative(relative)
        complete = _complete_jsonl(read_source_bytes(source))
        if not complete:
            raise MuseParseError(f"Muse log has no newline-complete record: {source}")
        target = snapshot_root.joinpath(*PurePosixPath(relative).parts)
        if target.is_symlink():
            raise MuseParseError(f"Muse snapshot target is a symlink: {target}")
        existing = target.read_bytes() if target.is_file() else b""
        previous = previous_by_path.get(relative)
        if previous is not None and (
            len(existing) < previous.copied_bytes
            or hashlib.sha256(existing[: previous.copied_bytes]).hexdigest() != previous.sha256
        ):
            raise MuseParseError(f"Muse snapshot differs from its manifest: {target}")
        if len(complete) < len(existing) or complete[: len(existing)] != existing:
            raise MuseParseError(
                f"append-only source violation: Muse log was truncated or rewritten: {relative}"
            )
        changed += int(write_text_if_changed(target, complete.decode("utf-8")))
        digest = hashlib.sha256(complete).hexdigest()
        copies.append(
            MuseSourceCopy(
                source_path=relative,
                original_path=str(source.resolve()),
                snapshot_path=relative,
                thread_id=session_dir.name if relative == _SESSION_LOG else source.parent.name,
                copied_bytes=len(complete),
                line_count=complete.count(b"\n"),
                sha256=digest,
                updated_at=(
                    previous.updated_at
                    if previous is not None
                    and previous.copied_bytes == len(complete)
                    and previous.sha256 == digest
                    else updated_at
                ),
            )
        )
    return MuseSnapshotResult(
        tuple(sorted(copies, key=lambda copy: copy.source_path)), changed, tuple(retired)
    )


@dataclass
class _Thread:
    thread_id: str
    source_path: str
    events: list[Event]
    turns: dict[str, list[int]]
    turn_status: dict[str, str]
    first_token: dict[str, int]
    last_message: dict[str, str]
    activity: list[int]


def _user_text(payload: Mapping[str, object]) -> str:
    parts: list[str] = []
    messages = payload.get("model_messages")
    for message in messages if isinstance(messages, list) else ():
        content = _mapping(message).get("content")
        for block in content if isinstance(content, list) else ():
            text = _string(_mapping(block).get("text"))
            if text is not None:
                parts.append(text)
    return "\n\n".join(parts)


def load_muse_team(
    snapshot_dir: Path,
    root_session_id: str,
    team_slug: str,
    display_timezone: str,
    *,
    source_paths: Sequence[str],
) -> TeamData:
    """Normalize the archive-local copy of one Muse session and its spawned subagents.

    *snapshot_dir* holds the copied ``session.jsonl`` and its ``subagent/`` tree; the root
    session's identity is given, because the copy no longer lives in a directory named for it.
    """

    root_id = root_session_id
    _validate_id(root_id, "root session id")
    if _SESSION_LOG not in source_paths:
        raise MuseParseError("Muse source set omits the root session log")
    threads: dict[str, _Thread] = {}
    snapshots: list[SourceSnapshot] = []
    tool_calls: dict[str, dict[str, object]] = {}
    calls_by_batch: dict[tuple[str, str], list[str]] = {}
    edges: list[Edge] = []
    parent_of: dict[str, str] = {}
    spawn_call_of: dict[str, str] = {}
    description_of: dict[str, str] = {}
    pending_bound: dict[str, tuple[str, int, int]] = {}
    for relative in sorted(source_paths, key=lambda item: (item != _SESSION_LOG, item)):
        path = snapshot_dir.joinpath(*_safe_relative(relative).parts)
        if path.is_symlink() or not path.is_file():
            raise MuseParseError(f"missing Muse source {path}")
        data = _complete_jsonl(path.read_bytes())
        thread_id = root_id if relative == _SESSION_LOG else path.parent.name
        _validate_id(thread_id, relative)
        thread = _Thread(thread_id, relative, [], {}, {}, {}, {}, [])
        threads[thread_id] = thread
        metadata = path.stat()
        snapshots.append(
            SourceSnapshot(
                path=relative,
                thread_id=thread_id,
                size_bytes=metadata.st_size,
                mtime_ns=metadata.st_mtime_ns,
                sha256=hashlib.sha256(data).hexdigest(),
                complete_bytes=len(data),
                line_count=data.count(b"\n"),
            )
        )
        prompted_runs: set[str] = set()
        for line_number, record in _records(data, relative):
            at_ms = _at_ms(record)
            if at_ms is None:
                continue
            payload_type = _string(record.get("payload_type")) or ""
            payload = _mapping(record.get("payload"))
            record_id = _string(record.get("id")) or f"{relative}:{line_number}"
            if payload_type == "runtime.user_intent.accepted":
                text = _user_text(payload)
                if text:
                    run_id = _string(payload.get("intent_id"))
                    if run_id is not None:
                        prompted_runs.add(run_id)
                    thread.events.append(
                        _event(thread_id, None, at_ms, "user_prompt", "user", text, line_number,
                               record_id, "unknown", None)
                    )
                    thread.activity.append(at_ms)
                continue
            if payload_type in ("tool_batch.effect.started", "tool_batch.effect.terminal"):
                effect = _mapping(payload.get("record"))
                call_id = _string(effect.get("call_id"))
                if call_id is None:
                    continue
                entry = tool_calls.setdefault(call_id, {"thread": thread_id, "line": line_number})
                entry.setdefault("name", _string(effect.get("tool_name")))
                if payload_type.endswith("started"):
                    entry.setdefault("started", at_ms)
                else:
                    entry["ended"] = at_ms
                    outcome = _string(_mapping(effect.get("outcome")).get("kind"))
                    entry["status"] = outcome or "completed"
                thread.activity.append(at_ms)
                continue
            if payload_type == "subagent.control.child_session_bound":
                bound = _mapping(payload.get("record"))
                child = _string(bound.get("child_session_id"))
                if child is not None:
                    pending_bound[child] = (thread_id, at_ms, line_number)
                continue
            if payload_type == "subagent.control.start_attested":
                attested = _mapping(payload.get("record"))
                child = _string(attested.get("subagent_session_id"))
                call = _string(attested.get("dispatched_via_tool_call_id"))
                if child is not None and call is not None:
                    spawn_call_of[child] = call
                continue
            if payload.get("kind") != "run":
                continue
            run_id = _string(payload.get("run_id"))
            event = _mapping(payload.get("event"))
            kind = _string(event.get("kind"))
            if run_id is not None:
                thread.turns.setdefault(run_id, []).append(at_ms)
            if kind == "started" and _string(event.get("prompt")) and run_id not in prompted_runs:
                if run_id is not None:
                    prompted_runs.add(run_id)
                thread.events.append(
                    _event(thread_id, run_id, at_ms, "user_prompt", "user", str(event["prompt"]),
                           line_number, record_id, "unknown", None)
                )
                thread.activity.append(at_ms)
            elif kind == "inbox_item_drained":
                # Runtime notices delivered into a running turn (a background task ended, a
                # subagent's result is ready): inputs, but not anyone's prompt.
                body = _string(_mapping(event.get("delivery_snapshot")).get("body"))
                if body is not None:
                    thread.events.append(
                        _event(thread_id, run_id, at_ms, "system_input", "system", body,
                               line_number, record_id, "system", None, ingress="muse_inbox")
                    )
                    thread.activity.append(at_ms)
            elif kind == "assistant_message_committed":
                reply = _string(event.get("text"))
                if reply is not None:
                    thread.events.append(
                        _event(thread_id, run_id, at_ms, "assistant_message", "assistant", reply,
                               line_number, record_id, "agent", thread_id)
                    )
                    thread.activity.append(at_ms)
                    if run_id is not None:
                        thread.last_message[run_id] = reply
            elif kind == "assistant_tool_calls_committed":
                batch = _string(event.get("message_id")) or record_id
                committed = event.get("tool_calls")
                for call in committed if isinstance(committed, list) else ():
                    call_map = _mapping(call)
                    call_id = _string(call_map.get("call_id"))
                    if call_id is None:
                        continue
                    entry = tool_calls.setdefault(call_id, {"thread": thread_id, "line": line_number})
                    entry["name"] = _string(call_map.get("name")) or entry.get("name")
                    entry["item_id"] = _string(call_map.get("id"))
                    entry["input"] = _string(call_map.get("args"))
                    entry["committed"] = at_ms
                    entry["turn"] = run_id
                    calls_by_batch.setdefault((thread_id, batch), []).append(call_id)
                    if entry["name"] == _SPAWN_TOOL:
                        try:
                            arguments = _mapping(json.loads(str(entry["input"] or "{}")))
                        except json.JSONDecodeError:
                            arguments = {}
                        entry["spawn_label"] = (
                            _string(arguments.get("role")) or _string(arguments.get("command_id"))
                        )
                        entry["spawn_text"] = _string(arguments.get("objective"))
                thread.activity.append(at_ms)
            elif kind == "tool_result_batch_committed":
                batch = _string(event.get("batch_id")) or ""
                results = event.get("results")
                ordered = calls_by_batch.get((thread_id, batch), [])
                for call_id, result in zip(ordered, results if isinstance(results, list) else ()):
                    output = _string(_mapping(result).get("text"))
                    if output is not None:
                        tool_calls[call_id]["output"] = output
            elif kind == "terminal" and run_id is not None:
                thread.turn_status[run_id] = _string(event.get("terminal")) or "completed"
                first = event.get("time_to_first_token_ms")
                if isinstance(first, int) and not isinstance(first, bool):
                    start = min(thread.turns.get(run_id, [at_ms]))
                    thread.first_token[run_id] = start + first
    for child, (parent, at_ms, line_number) in pending_bound.items():
        if child not in threads:
            continue
        parent_of[child] = parent
        call = spawn_call_of.get(child, f"muse-spawn-{child}")
        spawn = tool_calls.get(call, {})
        description_of[child] = str(spawn.get("spawn_label") or child)
        edges.append(
            Edge(
                edge_id=f"muse-spawn-{child}",
                call_id=call,
                from_thread_id=parent,
                to_thread_id=child,
                kind="spawn",
                timestamp_ms=at_ms,
                message_text=_string(spawn.get("spawn_text")),
                content_availability="plain",
                encrypted_content=None,
                source_line=line_number,
            )
        )

    turns: list[Turn] = []
    for thread in threads.values():
        for run_id, times in sorted(thread.turns.items(), key=lambda item: (min(item[1]), item[0])):
            start, end = min(times), max(times)
            turns.append(
                Turn(
                    turn_id=run_id,
                    thread_id=thread.thread_id,
                    started_at_ms=start,
                    ended_at_ms=max(start + 1, end),
                    status=thread.turn_status.get(run_id, "unknown"),
                    first_token_ms=thread.first_token.get(run_id),
                    error=None,
                    last_agent_message=thread.last_message.get(run_id),
                )
            )
    calls: list[ToolCall] = []
    for call_id, entry in sorted(tool_calls.items()):
        started = entry.get("started", entry.get("committed"))
        if not isinstance(started, int):
            continue
        ended = entry.get("ended")
        name = str(entry.get("name") or "unknown")
        namespace = name.split("__")[1] if name.startswith("mcp__") and name.count("__") >= 2 else None
        calls.append(
            ToolCall(
                call_id=call_id,
                item_id=_string(entry.get("item_id")),
                thread_id=str(entry["thread"]),
                turn_id=_string(entry.get("turn")),
                name=name,
                namespace=namespace,
                started_at_ms=started,
                ended_at_ms=ended if isinstance(ended, int) and ended >= started else None,
                status=str(entry.get("status") or ("running" if ended is None else "completed")),
                input_text=_string(entry.get("input")),
                output_text=_string(entry.get("output")),
                nested_tools=(),
                source_line=int(str(entry.get("line") or 1)),
            )
        )
    paths: dict[str, str] = {}

    def agent_path(thread_id: str) -> str:
        if thread_id in paths:
            return paths[thread_id]
        parent = parent_of.get(thread_id)
        if parent is None:
            paths[thread_id] = "/root"
            return "/root"
        leaf = re.sub(r"[^a-z0-9]+", "-", description_of[thread_id].lower()).strip("-")[:48]
        candidate = f"{agent_path(parent)}/{leaf or thread_id[-8:]}"
        if candidate in paths.values():
            candidate = f"{candidate}_{thread_id[-6:]}"
        paths[thread_id] = candidate
        return candidate

    agents: list[Agent] = []
    for thread in threads.values():
        if thread.thread_id != root_id and thread.thread_id not in parent_of:
            continue
        activity = thread.activity or [min((t for ts in thread.turns.values() for t in ts), default=0)]
        path_text = agent_path(thread.thread_id)
        agents.append(
            Agent(
                thread_id=thread.thread_id,
                parent_thread_id=parent_of.get(thread.thread_id),
                agent_path=path_text,
                nickname=None,
                role=description_of.get(thread.thread_id),
                depth=path_text.count("/") - 1,
                started_at_ms=min(activity),
                ended_at_ms=max(activity) + 1,
                status=(
                    "completed"
                    if any(status == "completed" for status in thread.turn_status.values())
                    else "unknown"
                ),
                source_path=thread.source_path,
            )
        )
    included = {agent.thread_id for agent in agents}
    events = sorted(
        (event for thread in threads.values() if thread.thread_id in included for event in thread.events),
        key=lambda event: (event.timestamp_ms, event.thread_id, event.source_line),
    )
    if not events and not calls:
        raise MuseParseError("Muse session contains no renderable activity")
    return TeamData(
        team_slug=team_slug,
        provider="muse",
        root_thread_id=root_id,
        display_timezone=display_timezone,
        sources=tuple(
            sorted(
                (snapshot for snapshot in snapshots if snapshot.thread_id in included),
                key=lambda snapshot: snapshot.path,
            )
        ),
        agents=tuple(sorted(agents, key=lambda agent: (agent.depth, agent.started_at_ms, agent.thread_id))),
        turns=tuple(sorted(
            (turn for turn in turns if turn.thread_id in included),
            key=lambda turn: (turn.started_at_ms, turn.turn_id),
        )),
        events=tuple(events),
        tool_calls=tuple(sorted(
            (call for call in calls if call.thread_id in included),
            key=lambda call: (call.started_at_ms, call.call_id),
        )),
        edges=tuple(sorted(edges, key=lambda edge: (edge.timestamp_ms, edge.edge_id))),
    )


def _event(
    thread_id: str,
    turn_id: str | None,
    at_ms: int,
    kind: str,
    role: str,
    text: str,
    line: int,
    native_id: str,
    author_kind: str,
    author: str | None,
    *,
    ingress: str = "muse",
) -> Event:
    return Event(
        event_id=f"muse-{thread_id}-{line}",
        thread_id=thread_id,
        turn_id=turn_id,
        timestamp_ms=at_ms,
        kind=kind,
        role=role,
        phase=None,
        text=text,
        content_availability="plain",
        encrypted_content=None,
        author=author,
        recipient=thread_id if role == "user" else None,
        source_line=line,
        ingress_kind=ingress,
        author_kind=author_kind,
        source_native_id=native_id,
        classification_version=_CLASSIFICATION_VERSION,
    )


__all__ = [
    "MuseParseError",
    "MuseSnapshotResult",
    "MuseSourceCopy",
    "discover_muse_sources",
    "load_muse_team",
    "snapshot_muse_lineage",
]
