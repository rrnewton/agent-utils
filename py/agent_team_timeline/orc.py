"""Read-only Orc SQLite snapshots and provider-neutral timeline normalization."""

from __future__ import annotations

import hashlib
import json
import os
import re
import secrets
import sqlite3
import stat
from collections import Counter, defaultdict
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath

from agent_team_timeline.model import (
    Agent,
    Edge,
    Event,
    SourceSnapshot,
    TeamData,
    ToolCall,
    Turn,
)


_SAFE_COMPONENT = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]*")
_ORC_TOOL = re.compile(r"\borc\.([A-Za-z_][A-Za-z0-9_]*)\s*\(")


class OrcParseError(ValueError):
    """Raised when an Orc source or its append-only history is invalid."""


@dataclass(frozen=True)
class OrcSourceCopy:
    """One consistent SQLite backup and its logical append-prefix evidence."""

    source_path: str
    snapshot_path: str
    kind: str
    owner_session_id: str
    source_size: int
    snapshot_size: int
    sha256: str
    append_count: int
    append_max_id: int
    append_prefix_sha256: str
    auxiliary_count: int
    auxiliary_prefix_sha256: str
    captured_at: str

    def to_json_obj(self) -> dict[str, object]:
        return {
            "source_path": self.source_path,
            "snapshot_path": self.snapshot_path,
            "kind": self.kind,
            "owner_session_id": self.owner_session_id,
            "source_size": self.source_size,
            "snapshot_size": self.snapshot_size,
            "sha256": self.sha256,
            "append_count": self.append_count,
            "append_max_id": self.append_max_id,
            "append_prefix_sha256": self.append_prefix_sha256,
            "auxiliary_count": self.auxiliary_count,
            "auxiliary_prefix_sha256": self.auxiliary_prefix_sha256,
            "captured_at": self.captured_at,
        }

    @classmethod
    def from_json_obj(
        cls, raw: Mapping[str, object], where: str
    ) -> OrcSourceCopy:
        source_path = _required_string(raw.get("source_path"), f"{where}.source_path")
        snapshot_path = _required_string(
            raw.get("snapshot_path"), f"{where}.snapshot_path"
        )
        if source_path != snapshot_path:
            raise OrcParseError(f"{where}: source and snapshot paths must match")
        _safe_relative(source_path)
        kind = _required_string(raw.get("kind"), f"{where}.kind")
        if kind not in ("session", "task"):
            raise OrcParseError(f"{where}.kind: unsupported Orc database kind {kind!r}")
        sha256 = _required_string(raw.get("sha256"), f"{where}.sha256")
        append_digest = _required_string(
            raw.get("append_prefix_sha256"), f"{where}.append_prefix_sha256"
        )
        auxiliary_digest = _required_string(
            raw.get("auxiliary_prefix_sha256"),
            f"{where}.auxiliary_prefix_sha256",
        )
        if any(
            re.fullmatch(r"[0-9a-f]{64}", digest) is None
            for digest in (sha256, append_digest, auxiliary_digest)
        ):
            raise OrcParseError(f"{where}: invalid SHA-256 evidence")
        return cls(
            source_path=source_path,
            snapshot_path=snapshot_path,
            kind=kind,
            owner_session_id=_required_string(
                raw.get("owner_session_id"), f"{where}.owner_session_id"
            ),
            source_size=_nonnegative_integer(
                raw.get("source_size"), f"{where}.source_size"
            ),
            snapshot_size=_nonnegative_integer(
                raw.get("snapshot_size"), f"{where}.snapshot_size"
            ),
            sha256=sha256,
            append_count=_nonnegative_integer(
                raw.get("append_count"), f"{where}.append_count"
            ),
            append_max_id=_nonnegative_integer(
                raw.get("append_max_id"), f"{where}.append_max_id"
            ),
            append_prefix_sha256=append_digest,
            auxiliary_count=_nonnegative_integer(
                raw.get("auxiliary_count"), f"{where}.auxiliary_count"
            ),
            auxiliary_prefix_sha256=auxiliary_digest,
            captured_at=_required_string(raw.get("captured_at"), f"{where}.captured_at"),
        )


@dataclass(frozen=True)
class OrcSnapshotResult:
    sources: tuple[OrcSourceCopy, ...]
    files_changed: int


@dataclass(frozen=True)
class _SessionMeta:
    session_id: str
    parent_id: str | None
    name: str
    db_name: str | None
    created_at_ms: int
    updated_at_ms: int
    source_path: str


@dataclass(frozen=True)
class _LogicalState:
    append_count: int
    append_max_id: int
    append_prefix_sha256: str
    auxiliary_count: int
    auxiliary_prefix_sha256: str


@dataclass(frozen=True)
class _Spawn:
    thread_id: str
    parent_thread_id: str
    official_name: str
    timestamp_ms: int
    source_line: int
    source_path: str


def _required_string(value: object, where: str) -> str:
    if not isinstance(value, str) or not value:
        raise OrcParseError(f"{where}: expected a non-empty string")
    return value


def _optional_string(value: object, where: str) -> str | None:
    if value is None or value == "":
        return None
    if not isinstance(value, str):
        raise OrcParseError(f"{where}: expected a string or null")
    return value


def _nonnegative_integer(value: object, where: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or value < 0:
        raise OrcParseError(f"{where}: expected a non-negative integer")
    return value


def _integer(value: object, where: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int):
        raise OrcParseError(f"{where}: expected an integer")
    return value


def _mapping(value: object, where: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise OrcParseError(f"{where}: expected an object")
    return {str(key): item for key, item in value.items()}


def _array(value: object, where: str) -> list[object]:
    if not isinstance(value, list):
        raise OrcParseError(f"{where}: expected an array")
    return list(value)


def _row(value: object, where: str) -> tuple[object, ...]:
    if not isinstance(value, tuple):
        raise OrcParseError(f"{where}: SQLite returned an invalid row")
    return tuple(value)


def _one(connection: sqlite3.Connection, sql: str, where: str) -> tuple[object, ...]:
    raw: object = connection.execute(sql).fetchone()
    if raw is None:
        raise OrcParseError(f"{where}: expected one row")
    return _row(raw, where)


def _iso_ms(value: object, where: str) -> int:
    text = _required_string(value, where)
    normalized = text[:-1] + "+00:00" if text.endswith("Z") else text
    try:
        parsed = datetime.fromisoformat(normalized)
    except ValueError as error:
        raise OrcParseError(f"{where}: invalid ISO timestamp {text!r}") from error
    if parsed.tzinfo is None:
        parsed = parsed.replace(tzinfo=timezone.utc)
    return int(parsed.timestamp() * 1000)


def _safe_component(value: str, where: str) -> str:
    if len(value) > 255 or _SAFE_COMPONENT.fullmatch(value) is None:
        raise OrcParseError(f"{where}: unsafe path component {value!r}")
    return value


def _safe_relative(value: str) -> PurePosixPath:
    relative = PurePosixPath(value)
    if (
        relative.is_absolute()
        or not relative.parts
        or any(part in ("", ".", "..") for part in relative.parts)
    ):
        raise OrcParseError(f"unsafe Orc snapshot path {value!r}")
    return relative


def _snapshot_path(snapshot_root: Path, relative: str) -> Path:
    safe = _safe_relative(relative)
    return snapshot_root.joinpath(*safe.parts)


def _read_only(path: Path) -> sqlite3.Connection:
    if path.is_symlink() or not path.is_file():
        raise OrcParseError(f"Orc SQLite source is missing or not a regular file: {path}")
    try:
        connection = sqlite3.connect(path.resolve(strict=True).as_uri() + "?mode=ro", uri=True)
        connection.execute("PRAGMA query_only = ON")
        return connection
    except sqlite3.Error as error:
        raise OrcParseError(f"cannot open Orc SQLite source read-only at {path}: {error}") from error


def _require_tables(
    connection: sqlite3.Connection, names: Sequence[str], where: str
) -> None:
    rows = connection.execute(
        "SELECT name FROM sqlite_master WHERE type = 'table'"
    ).fetchall()
    found = {_required_string(_row(raw, where)[0], where) for raw in rows}
    missing = sorted(set(names) - found)
    if missing:
        raise OrcParseError(f"{where}: missing required tables: {', '.join(missing)}")


def _session_meta(path: Path, source_path: str) -> _SessionMeta:
    connection = _read_only(path)
    try:
        _require_tables(
            connection,
            ("session_meta", "content_blocks", "conversation_state"),
            str(path),
        )
        row = _one(
            connection,
            "SELECT id, parent_id, name, db_name, created_at, updated_at "
            "FROM session_meta LIMIT 1",
            str(path),
        )
    finally:
        connection.close()
    session_id = _safe_component(_required_string(row[0], f"{path}: id"), f"{path}: id")
    parent_id = _optional_string(row[1], f"{path}: parent_id")
    if parent_id is not None:
        _safe_component(parent_id, f"{path}: parent_id")
    db_name = _optional_string(row[3], f"{path}: db_name")
    if db_name is not None:
        _safe_component(db_name, f"{path}: db_name")
    return _SessionMeta(
        session_id=session_id,
        parent_id=parent_id,
        name=_required_string(row[2], f"{path}: name"),
        db_name=db_name,
        created_at_ms=_iso_ms(row[4], f"{path}: created_at"),
        updated_at_ms=_iso_ms(row[5], f"{path}: updated_at"),
        source_path=source_path,
    )


def _discover_sources(
    source_root: Path, root_session_id: str
) -> tuple[tuple[str, str, str], ...]:
    root_id = _safe_component(root_session_id, "root session id")
    sessions_root = source_root / ".orc" / "sessions"
    if not sessions_root.is_dir() or sessions_root.is_symlink():
        raise OrcParseError(f"missing Orc sessions directory: {sessions_root}")
    metas: dict[str, _SessionMeta] = {}
    for session_dir in sorted(sessions_root.iterdir(), key=lambda item: item.name):
        if session_dir.is_symlink() or not session_dir.is_dir():
            continue
        database = session_dir / "session.db"
        if not database.is_file() or database.is_symlink():
            continue
        relative = database.relative_to(source_root).as_posix()
        meta = _session_meta(database, relative)
        if meta.session_id != session_dir.name:
            raise OrcParseError(
                f"Orc session directory {session_dir.name!r} contains session {meta.session_id!r}"
            )
        metas[meta.session_id] = meta
    if root_id not in metas:
        raise OrcParseError(f"root Orc session {root_id!r} was not found under {sessions_root}")

    selected = {root_id}
    changed = True
    while changed:
        changed = False
        for meta in metas.values():
            if meta.parent_id in selected and meta.session_id not in selected:
                selected.add(meta.session_id)
                changed = True

    result: list[tuple[str, str, str]] = []
    task_paths: set[str] = set()
    for session_id in sorted(selected):
        meta = metas[session_id]
        result.append((meta.source_path, "session", session_id))
        if meta.db_name is None:
            continue
        task_relative = f".tg/{meta.db_name}.db"
        task_path = source_root / task_relative
        if not task_path.is_file() or task_path.is_symlink():
            raise OrcParseError(
                f"session {session_id!r} names task database {meta.db_name!r}, "
                f"but {task_path} is missing"
            )
        if task_relative not in task_paths:
            result.append((task_relative, "task", session_id))
            task_paths.add(task_relative)
    return tuple(sorted(result))


def _update_digest(digest: hashlib._Hash, value: object) -> None:
    if value is None:
        digest.update(b"N")
        return
    if isinstance(value, bool):
        digest.update(b"I1" if value else b"I0")
        return
    if isinstance(value, int):
        payload = str(value).encode("ascii")
        prefix = b"I"
    elif isinstance(value, float):
        payload = value.hex().encode("ascii")
        prefix = b"F"
    elif isinstance(value, str):
        payload = value.encode("utf-8")
        prefix = b"S"
    elif isinstance(value, bytes):
        payload = value
        prefix = b"B"
    else:
        raise OrcParseError(f"unsupported SQLite value type {type(value).__name__}")
    digest.update(prefix)
    digest.update(len(payload).to_bytes(8, "big"))
    digest.update(payload)


def _query_digest(
    connection: sqlite3.Connection, sql: str, parameters: tuple[object, ...] = ()
) -> tuple[int, str]:
    digest = hashlib.sha256()
    count = 0
    for raw in connection.execute(sql, parameters):
        row = _row(raw, "SQLite digest query")
        digest.update(b"R")
        for value in row:
            _update_digest(digest, value)
        count += 1
    return count, digest.hexdigest()


def _conversation_messages(connection: sqlite3.Connection) -> list[object]:
    row = _one(
        connection,
        "SELECT conversation_json FROM conversation_state WHERE id = 1",
        "conversation_state",
    )
    raw = _required_string(row[0], "conversation_state.conversation_json")
    try:
        parsed: object = json.loads(raw)
    except json.JSONDecodeError as error:
        raise OrcParseError(f"invalid conversation_state JSON: {error}") from error
    root = _mapping(parsed, "conversation_state")
    return _array(root.get("messages"), "conversation_state.messages")


def _messages_digest(messages: Sequence[object]) -> str:
    try:
        encoded = json.dumps(
            list(messages),
            ensure_ascii=False,
            sort_keys=True,
            separators=(",", ":"),
        ).encode("utf-8")
    except (TypeError, ValueError) as error:
        raise OrcParseError(f"conversation state is not canonical JSON: {error}") from error
    return hashlib.sha256(encoded).hexdigest()


def _logical_state(
    path: Path,
    kind: str,
    *,
    prefix_max_id: int | None = None,
    auxiliary_count: int | None = None,
) -> _LogicalState:
    connection = _read_only(path)
    try:
        check = _one(connection, "PRAGMA quick_check", str(path))
        if check != ("ok",):
            raise OrcParseError(f"SQLite quick_check failed for {path}: {check!r}")
        if kind == "session":
            _require_tables(
                connection,
                ("session_meta", "content_blocks", "conversation_state"),
                str(path),
            )
            count_row = _one(
                connection,
                "SELECT COUNT(*), COALESCE(MAX(rowid), 0) FROM content_blocks",
                str(path),
            )
            append_count = _nonnegative_integer(count_row[0], f"{path}: content count")
            append_max_id = _nonnegative_integer(count_row[1], f"{path}: max rowid")
            limit = append_max_id if prefix_max_id is None else prefix_max_id
            prefix_count, prefix_digest = _query_digest(
                connection,
                "SELECT rowid, id, message_id, session_id, block_index, created_at_ms, "
                "turn_index, role, block_type, content, searchable_text, code_input, "
                "code_output, code_exit_code, model, user_source, token_count, extra "
                "FROM content_blocks WHERE rowid <= ? ORDER BY rowid",
                (limit,),
            )
            messages = _conversation_messages(connection)
            message_limit = len(messages) if auxiliary_count is None else auxiliary_count
            if message_limit > len(messages):
                raise OrcParseError(
                    f"conversation history shrank from {message_limit} to {len(messages)} messages"
                )
            auxiliary_digest = _messages_digest(messages[:message_limit])
            return _LogicalState(
                append_count=append_count if prefix_max_id is None else prefix_count,
                append_max_id=append_max_id,
                append_prefix_sha256=prefix_digest,
                auxiliary_count=len(messages),
                auxiliary_prefix_sha256=auxiliary_digest,
            )
        if kind == "task":
            _require_tables(connection, ("tasks", "task_notes"), str(path))
            count_row = _one(
                connection,
                "SELECT COUNT(*), COALESCE(MAX(id), 0) FROM task_notes",
                str(path),
            )
            append_count = _nonnegative_integer(count_row[0], f"{path}: note count")
            append_max_id = _nonnegative_integer(count_row[1], f"{path}: max note id")
            limit = append_max_id if prefix_max_id is None else prefix_max_id
            prefix_count, prefix_digest = _query_digest(
                connection,
                "SELECT id, task_id, content, created_at, server_comment_id, author_unixname "
                "FROM task_notes WHERE id <= ? ORDER BY id",
                (limit,),
            )
            return _LogicalState(
                append_count=append_count if prefix_max_id is None else prefix_count,
                append_max_id=append_max_id,
                append_prefix_sha256=prefix_digest,
                auxiliary_count=0,
                auxiliary_prefix_sha256=hashlib.sha256(b"").hexdigest(),
            )
        raise OrcParseError(f"unsupported Orc source kind {kind!r}")
    except sqlite3.Error as error:
        raise OrcParseError(f"failed to inspect Orc SQLite source {path}: {error}") from error
    finally:
        connection.close()


def _sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(1024 * 1024)
            if not chunk:
                return digest.hexdigest()
            digest.update(chunk)


def _ensure_snapshot_parent(path: Path, snapshot_root: Path) -> None:
    snapshot_root.mkdir(parents=True, exist_ok=True)
    if snapshot_root.is_symlink() or not snapshot_root.is_dir():
        raise OrcParseError(f"snapshot root is a symlink or non-directory: {snapshot_root}")
    relative = path.parent.relative_to(snapshot_root)
    current = snapshot_root
    for part in relative.parts:
        current = current / part
        if current.exists():
            if current.is_symlink() or not current.is_dir():
                raise OrcParseError(
                    f"snapshot path component is a symlink or non-directory: {current}"
                )
        else:
            current.mkdir(mode=0o700)


def _backup_database(source: Path, destination: Path, snapshot_root: Path) -> None:
    _ensure_snapshot_parent(destination, snapshot_root)
    if destination.exists() and (destination.is_symlink() or not destination.is_file()):
        raise OrcParseError(f"snapshot target is a symlink or non-file: {destination}")
    source_connection = _read_only(source)
    try:
        destination_connection = sqlite3.connect(destination)
        try:
            source_connection.backup(destination_connection)
            destination_connection.commit()
        finally:
            destination_connection.close()
    except sqlite3.Error as error:
        raise OrcParseError(f"failed to back up Orc SQLite source {source}: {error}") from error
    finally:
        source_connection.close()


def snapshot_orc_lineage(
    source_root: Path,
    root_session_id: str,
    snapshot_root: Path,
    previous_sources: Sequence[OrcSourceCopy],
    captured_at: str,
) -> OrcSnapshotResult:
    """Create consistent SQLite backups after validating logical append monotonicity."""

    discovered = _discover_sources(source_root, root_session_id)
    discovered_paths = {path for path, _, _ in discovered}
    previous_by_path = {source.source_path: source for source in previous_sources}
    disappeared = sorted(set(previous_by_path) - discovered_paths)
    if disappeared:
        raise OrcParseError(
            "previously observed Orc source disappeared: " + ", ".join(disappeared)
        )

    staged: list[tuple[Path, Path, OrcSourceCopy]] = []
    temporary_paths: list[Path] = []
    try:
        for relative, kind, owner_session_id in discovered:
            source_path = source_root / relative
            target = _snapshot_path(snapshot_root, relative)
            previous = previous_by_path.get(relative)
            if previous is not None:
                if not target.is_file() or target.is_symlink():
                    raise OrcParseError(f"previous Orc snapshot is missing: {target}")
                if _sha256_file(target) != previous.sha256:
                    raise OrcParseError(
                        f"previous Orc snapshot does not match its manifest: {target}"
                    )
            _ensure_snapshot_parent(target, snapshot_root)
            temporary = target.parent / (
                f".{target.name}.{os.getpid()}.{secrets.token_hex(8)}.tmp"
            )
            temporary_paths.append(temporary)
            _backup_database(source_path, temporary, snapshot_root)
            full = _logical_state(temporary, kind)
            if previous is not None:
                prefix = _logical_state(
                    temporary,
                    kind,
                    prefix_max_id=previous.append_max_id,
                    auxiliary_count=previous.auxiliary_count,
                )
                if full.append_count < previous.append_count:
                    raise OrcParseError(
                        f"Orc {kind} append history shrank for {relative}: "
                        f"{previous.append_count} to {full.append_count} rows"
                    )
                if prefix.append_count != previous.append_count:
                    raise OrcParseError(
                        f"Orc {kind} append prefix lost rows for {relative}"
                    )
                if prefix.append_prefix_sha256 != previous.append_prefix_sha256:
                    raise OrcParseError(
                        f"Orc {kind} existing append prefix was rewritten for {relative}"
                    )
                if full.auxiliary_count < previous.auxiliary_count:
                    raise OrcParseError(
                        f"Orc conversation history shrank for {relative}"
                    )
                if (
                    prefix.auxiliary_prefix_sha256
                    != previous.auxiliary_prefix_sha256
                ):
                    raise OrcParseError(
                        f"Orc conversation prefix was rewritten for {relative}"
                    )
            snapshot_size = temporary.stat().st_size
            snapshot_sha256 = _sha256_file(temporary)
            effective_captured_at = (
                previous.captured_at
                if previous is not None and previous.sha256 == snapshot_sha256
                else captured_at
            )
            staged.append(
                (
                    temporary,
                    target,
                    OrcSourceCopy(
                        source_path=relative,
                        snapshot_path=relative,
                        kind=kind,
                        owner_session_id=owner_session_id,
                        source_size=source_path.stat().st_size,
                        snapshot_size=snapshot_size,
                        sha256=snapshot_sha256,
                        append_count=full.append_count,
                        append_max_id=full.append_max_id,
                        append_prefix_sha256=full.append_prefix_sha256,
                        auxiliary_count=full.auxiliary_count,
                        auxiliary_prefix_sha256=full.auxiliary_prefix_sha256,
                        captured_at=effective_captured_at,
                    ),
                )
            )

        changed = 0
        for temporary, target, source in staged:
            if target.is_file() and not target.is_symlink() and _sha256_file(target) == source.sha256:
                temporary.unlink()
                continue
            os.replace(temporary, target)
            changed += 1
        return OrcSnapshotResult(
            sources=tuple(item[2] for item in staged), files_changed=changed
        )
    finally:
        for temporary in temporary_paths:
            if temporary.exists():
                temporary.unlink()


def _conversation_spawns(
    path: Path, meta: _SessionMeta
) -> tuple[_Spawn, ...]:
    connection = _read_only(path)
    try:
        messages = _conversation_messages(connection)
    finally:
        connection.close()
    result: list[_Spawn] = []
    for message_index, raw_message in enumerate(messages):
        message = _mapping(raw_message, f"{path}: messages[{message_index}]")
        timestamp_ms = _integer(
            message.get("created_at_ms"),
            f"{path}: messages[{message_index}].created_at_ms",
        )
        blocks = _array(
            message.get("blocks"), f"{path}: messages[{message_index}].blocks"
        )
        for block_index, raw_block in enumerate(blocks):
            block = _mapping(
                raw_block,
                f"{path}: messages[{message_index}].blocks[{block_index}]",
            )
            if block.get("type") != "AgentBlock":
                continue
            block_id = _integer(block.get("id"), f"{path}: AgentBlock.id")
            official_name = _required_string(
                block.get("agent_id"), f"{path}: AgentBlock.agent_id"
            )
            safe_name = re.sub(r"[^A-Za-z0-9._-]+", "-", official_name).strip("-")
            if not safe_name:
                safe_name = "agent"
            result.append(
                _Spawn(
                    thread_id=f"orc-agent-{meta.session_id[:8]}-{block_id}",
                    parent_thread_id=meta.session_id,
                    official_name=official_name,
                    timestamp_ms=timestamp_ms,
                    source_line=block_id,
                    source_path=meta.source_path,
                )
            )
    return tuple(
        sorted(result, key=lambda item: (item.timestamp_ms, item.source_line))
    )


def _content_records(
    path: Path, meta: _SessionMeta
) -> tuple[tuple[Event, ...], tuple[ToolCall, ...], tuple[Turn, ...]]:
    connection = _read_only(path)
    try:
        rows = connection.execute(
            "SELECT rowid, id, message_id, created_at_ms, turn_index, role, block_type, "
            "content, code_input, code_output, code_exit_code FROM content_blocks "
            "ORDER BY created_at_ms, rowid"
        ).fetchall()
    finally:
        connection.close()
    events: list[Event] = []
    tools: list[ToolCall] = []
    turn_bounds: dict[int, tuple[int, int, str | None]] = {}
    for raw in rows:
        row = _row(raw, str(path))
        rowid = _integer(row[0], f"{path}: rowid")
        block_id = _required_string(row[1], f"{path}: block id")
        timestamp_ms = _integer(row[3], f"{path}: created_at_ms")
        turn_index = _integer(row[4], f"{path}: turn_index")
        role = _required_string(row[5], f"{path}: role")
        block_type = _required_string(row[6], f"{path}: block_type")
        content = _optional_string(row[7], f"{path}: content")
        turn_id = f"orc-turn-{meta.session_id[:8]}-{turn_index}"
        prior = turn_bounds.get(turn_index)
        if prior is None:
            turn_bounds[turn_index] = (timestamp_ms, timestamp_ms + 1, content)
        else:
            last_message = content if role == "assistant" and content else prior[2]
            turn_bounds[turn_index] = (
                min(prior[0], timestamp_ms),
                max(prior[1], timestamp_ms + 1),
                last_message,
            )
        event_kind: str | None = None
        event_role: str | None = None
        if block_type == "text" and role == "user":
            event_kind = "user_prompt"
            event_role = "user"
        elif block_type == "text" and role in ("assistant", "notification"):
            event_kind = "assistant_message"
            event_role = "assistant"
        if event_kind is not None and content:
            events.append(
                Event(
                    event_id=f"orc-block-{block_id}",
                    thread_id=meta.session_id,
                    turn_id=turn_id,
                    timestamp_ms=timestamp_ms,
                    kind=event_kind,
                    role=event_role,
                    phase=None,
                    text=content,
                    content_availability="plaintext",
                    encrypted_content=None,
                    author=None,
                    recipient=None,
                    source_line=rowid,
                )
            )
        if block_type == "code_execution":
            code_input = _optional_string(row[8], f"{path}: code_input")
            code_output = _optional_string(row[9], f"{path}: code_output")
            exit_code = row[10]
            counts = Counter(_ORC_TOOL.findall(code_input or ""))
            status = (
                "failed"
                if isinstance(exit_code, int) and not isinstance(exit_code, bool) and exit_code != 0
                else "completed"
            )
            tools.append(
                ToolCall(
                    call_id=f"orc-code-{block_id}",
                    item_id=block_id,
                    thread_id=meta.session_id,
                    turn_id=turn_id,
                    name="code_execution",
                    namespace="orc",
                    started_at_ms=timestamp_ms,
                    ended_at_ms=timestamp_ms + 1,
                    status=status,
                    input_text=code_input,
                    output_text=code_output,
                    nested_tools=tuple(sorted(counts.items())),
                    source_line=rowid,
                )
            )
    turns = tuple(
        Turn(
            turn_id=f"orc-turn-{meta.session_id[:8]}-{turn_index}",
            thread_id=meta.session_id,
            started_at_ms=bounds[0],
            ended_at_ms=bounds[1],
            status="completed",
            first_token_ms=None,
            error=None,
            last_agent_message=bounds[2],
        )
        for turn_index, bounds in sorted(turn_bounds.items())
    )
    return tuple(events), tuple(tools), turns


def _select_spawn(
    spawns: Mapping[tuple[str, str], Sequence[_Spawn]],
    coordinator_id: str,
    owner: str,
    timestamp_ms: int,
) -> _Spawn | None:
    eligible = [
        spawn
        for spawn in spawns.get((coordinator_id, owner), ())
        if spawn.timestamp_ms <= timestamp_ms
    ]
    return eligible[-1] if eligible else None


def _task_records(
    path: Path,
    source_path: str,
    coordinator_id: str,
    spawns: Mapping[tuple[str, str], Sequence[_Spawn]],
) -> tuple[tuple[Event, ...], tuple[Turn, ...], tuple[_Spawn, ...]]:
    connection = _read_only(path)
    try:
        rows = connection.execute(
            "SELECT n.id, n.task_id, n.content, n.created_at, n.author_unixname, "
            "t.owner, t.title FROM task_notes n JOIN tasks t ON t.local_id = n.task_id "
            "ORDER BY n.created_at, n.id"
        ).fetchall()
    finally:
        connection.close()
    events: list[Event] = []
    turns: list[Turn] = []
    inferred: dict[str, _Spawn] = {}
    for raw in rows:
        row = _row(raw, str(path))
        note_id = _integer(row[0], f"{path}: note id")
        task_id = _required_string(row[1], f"{path}: task id")
        content = _optional_string(row[2], f"{path}: note content")
        if content is None:
            continue
        timestamp_ms = _iso_ms(row[3], f"{path}: note created_at")
        author = _optional_string(row[4], f"{path}: author")
        owner = author or _optional_string(row[5], f"{path}: owner")
        if owner is None:
            continue
        title = _required_string(row[6], f"{path}: task title")
        spawn = _select_spawn(spawns, coordinator_id, owner, timestamp_ms)
        if spawn is None:
            spawn = inferred.get(owner)
        if spawn is None:
            owner_slug = re.sub(r"[^A-Za-z0-9._-]+", "-", owner).strip("-") or "agent"
            digest = hashlib.sha256(
                f"{coordinator_id}\0{owner}".encode("utf-8")
            ).hexdigest()[:12]
            spawn = _Spawn(
                thread_id=f"orc-owner-{owner_slug[:80]}-{digest}",
                parent_thread_id=coordinator_id,
                official_name=owner,
                timestamp_ms=timestamp_ms,
                source_line=note_id,
                source_path=source_path,
            )
            inferred[owner] = spawn
        turn_id = f"orc-note-turn-{coordinator_id[:8]}-{note_id}"
        text = f"[{task_id} · {title}]\n\n{content}"
        events.append(
            Event(
                event_id=f"orc-note-{coordinator_id[:8]}-{note_id}",
                thread_id=spawn.thread_id,
                turn_id=turn_id,
                timestamp_ms=timestamp_ms,
                kind="assistant_message",
                role="assistant",
                phase=None,
                text=text,
                content_availability="plaintext",
                encrypted_content=None,
                author=owner,
                recipient=coordinator_id,
                source_line=note_id,
            )
        )
        turns.append(
            Turn(
                turn_id=turn_id,
                thread_id=spawn.thread_id,
                started_at_ms=timestamp_ms,
                ended_at_ms=timestamp_ms + 1000,
                status="completed",
                first_token_ms=None,
                error=None,
                last_agent_message=text,
            )
        )
    return tuple(events), tuple(turns), tuple(inferred.values())


def _agent_depth(
    thread_id: str, parents: Mapping[str, str | None]
) -> int:
    depth = 0
    seen = {thread_id}
    parent = parents.get(thread_id)
    while parent is not None and parent not in seen:
        seen.add(parent)
        depth += 1
        parent = parents.get(parent)
    return depth


def load_orc_team(
    snapshot_root: Path,
    root_session_id: str,
    team_slug: str,
    display_timezone: str,
    source_copies: Sequence[OrcSourceCopy],
) -> TeamData:
    """Normalize validated archive-local Orc SQLite backups into ``TeamData``."""

    session_sources = [source for source in source_copies if source.kind == "session"]
    task_sources = [source for source in source_copies if source.kind == "task"]
    metas: dict[str, _SessionMeta] = {}
    for source in session_sources:
        path = _snapshot_path(snapshot_root, source.snapshot_path)
        meta = _session_meta(path, source.snapshot_path)
        metas[meta.session_id] = meta
    if root_session_id not in metas:
        raise OrcParseError(f"snapshot set does not contain root session {root_session_id!r}")

    session_events: list[Event] = []
    tools: list[ToolCall] = []
    turns: list[Turn] = []
    all_spawns: list[_Spawn] = []
    for session_id in sorted(metas):
        meta = metas[session_id]
        path = _snapshot_path(snapshot_root, meta.source_path)
        content_events, session_tools, session_turns = _content_records(path, meta)
        session_events.extend(content_events)
        tools.extend(session_tools)
        turns.extend(session_turns)
        all_spawns.extend(_conversation_spawns(path, meta))

    spawns_by_name: dict[tuple[str, str], list[_Spawn]] = defaultdict(list)
    for spawn in all_spawns:
        spawns_by_name[(spawn.parent_thread_id, spawn.official_name)].append(spawn)
    for values in spawns_by_name.values():
        values.sort(key=lambda item: (item.timestamp_ms, item.source_line))

    task_events: list[Event] = []
    inferred_spawns: list[_Spawn] = []
    for source in task_sources:
        path = _snapshot_path(snapshot_root, source.snapshot_path)
        note_events, task_turns, inferred = _task_records(
            path,
            source.snapshot_path,
            source.owner_session_id,
            spawns_by_name,
        )
        task_events.extend(note_events)
        turns.extend(task_turns)
        inferred_spawns.extend(inferred)
    all_spawns.extend(inferred_spawns)

    normalized_events = sorted(
        [*session_events, *task_events],
        key=lambda item: (item.timestamp_ms, item.event_id),
    )
    events_by_thread: dict[str, list[Event]] = defaultdict(list)
    for event in normalized_events:
        events_by_thread[event.thread_id].append(event)

    parents: dict[str, str | None] = {
        meta.session_id: meta.parent_id for meta in metas.values()
    }
    for spawn in all_spawns:
        parents[spawn.thread_id] = spawn.parent_thread_id

    agents: list[Agent] = []
    for session_id in sorted(metas):
        meta = metas[session_id]
        parent = meta.parent_id if meta.parent_id in metas else None
        agent_path = "/root" if session_id == root_session_id else f"/root/{meta.name}"
        own = events_by_thread.get(session_id, [])
        ended = max((event.timestamp_ms + 1 for event in own), default=meta.updated_at_ms)
        agents.append(
            Agent(
                thread_id=session_id,
                parent_thread_id=parent,
                agent_path=agent_path,
                nickname=meta.name if session_id != root_session_id else None,
                role="coordinator",
                depth=_agent_depth(session_id, parents),
                started_at_ms=meta.created_at_ms,
                ended_at_ms=max(meta.created_at_ms + 1, ended),
                status="completed",
                source_path=meta.source_path,
            )
        )

    spawns_by_official: dict[tuple[str, str], list[_Spawn]] = defaultdict(list)
    for spawn in all_spawns:
        spawns_by_official[(spawn.parent_thread_id, spawn.official_name)].append(spawn)
    for values in spawns_by_official.values():
        values.sort(key=lambda item: (item.timestamp_ms, item.source_line))
    for spawn_key in sorted(spawns_by_official):
        values = spawns_by_official[spawn_key]
        official_name = spawn_key[1]
        for index, spawn in enumerate(values):
            own = events_by_thread.get(spawn.thread_id, [])
            activity_end = max(
                (event.timestamp_ms + 1000 for event in own),
                default=spawn.timestamp_ms + 1000,
            )
            next_start = values[index + 1].timestamp_ms if index + 1 < len(values) else None
            ended = min(activity_end, next_start) if next_start is not None else activity_end
            parent_path = next(
                (
                    agent.agent_path
                    for agent in agents
                    if agent.thread_id == spawn.parent_thread_id
                ),
                "/root",
            )
            agents.append(
                Agent(
                    thread_id=spawn.thread_id,
                    parent_thread_id=spawn.parent_thread_id,
                    agent_path=f"{parent_path.rstrip('/')}/{official_name}",
                    nickname=None,
                    role="worker",
                    depth=_agent_depth(spawn.thread_id, parents),
                    started_at_ms=spawn.timestamp_ms,
                    ended_at_ms=max(spawn.timestamp_ms + 1, ended),
                    status="completed",
                    source_path=spawn.source_path,
                )
            )

    agent_ids = {agent.thread_id for agent in agents}
    filtered_events = tuple(
        event for event in normalized_events if event.thread_id in agent_ids
    )
    edges = tuple(
        Edge(
            edge_id=f"orc-spawn-{spawn.thread_id}",
            call_id=f"orc-spawn-{spawn.source_line}",
            from_thread_id=spawn.parent_thread_id,
            to_thread_id=spawn.thread_id,
            kind="spawn",
            timestamp_ms=spawn.timestamp_ms,
            message_text=None,
            content_availability="none",
            encrypted_content=None,
            source_line=spawn.source_line,
        )
        for spawn in sorted(
            all_spawns, key=lambda item: (item.timestamp_ms, item.thread_id)
        )
        if spawn.parent_thread_id in agent_ids
    )
    source_snapshots = tuple(
        SourceSnapshot(
            path=source.snapshot_path,
            thread_id=source.owner_session_id,
            size_bytes=source.snapshot_size,
            mtime_ns=_snapshot_path(snapshot_root, source.snapshot_path).stat().st_mtime_ns,
            sha256=source.sha256,
            complete_bytes=source.snapshot_size,
            line_count=source.append_count,
        )
        for source in sorted(source_copies, key=lambda item: item.snapshot_path)
    )
    return TeamData(
        team_slug=team_slug,
        provider="orc",
        root_thread_id=root_session_id,
        display_timezone=display_timezone,
        sources=source_snapshots,
        agents=tuple(sorted(agents, key=lambda item: (item.started_at_ms, item.thread_id))),
        turns=tuple(sorted(turns, key=lambda item: (item.started_at_ms, item.turn_id))),
        events=filtered_events,
        tool_calls=tuple(
            sorted(tools, key=lambda item: (item.started_at_ms, item.call_id))
        ),
        edges=edges,
    )


__all__ = [
    "OrcParseError",
    "OrcSnapshotResult",
    "OrcSourceCopy",
    "load_orc_team",
    "snapshot_orc_lineage",
]
