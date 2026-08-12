from __future__ import annotations

import hashlib
import json
import shutil
import sqlite3
import stat
from collections.abc import Sequence
from dataclasses import replace
from datetime import datetime, timezone
from pathlib import Path

import agent_team_timeline.orc as orc_module
import agent_team_timeline.pipeline as pipeline_module
import pytest

from agent_team_timeline.archive import (
    JsonValue,
    as_array,
    as_int,
    as_object,
    canonical_json,
    narrow_json,
    read_json,
    write_json_if_changed,
)
from agent_team_timeline.orc import (
    OrcParseError,
    OrcSourceCopy,
    load_orc_team,
    snapshot_orc_lineage,
)
from agent_team_timeline.model import TeamData, source_digest
from agent_team_timeline.phases import build_phases
from agent_team_timeline.pipeline import (
    build_archive,
    ingest_orc,
    load_archived_team,
    summarize_archive,
)
from agent_team_timeline.window import apply_date_window, parse_date_window


ROOT = "11111111-1111-1111-1111-111111111111"
NESTED = "22222222-2222-2222-2222-222222222222"


def _ms(value: str) -> int:
    return int(datetime.fromisoformat(value).astimezone(timezone.utc).timestamp() * 1000)


def _session_database(
    path: Path,
    session_id: str,
    *,
    parent_id: str | None,
    db_name: str | None,
    messages: list[dict[str, object]],
    blocks: Sequence[tuple[object, ...]],
) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    connection = sqlite3.connect(path)
    try:
        connection.executescript(
            """
            CREATE TABLE session_meta (
                id TEXT PRIMARY KEY, parent_id TEXT, name TEXT NOT NULL, db_name TEXT,
                created_at TEXT NOT NULL, updated_at TEXT NOT NULL
            );
            CREATE TABLE conversation_state (
                id INTEGER PRIMARY KEY CHECK (id = 1), conversation_json TEXT NOT NULL,
                updated_at TEXT NOT NULL
            );
            CREATE TABLE associated_dbs (db_name TEXT PRIMARY KEY);
            CREATE TABLE content_blocks (
                id TEXT PRIMARY KEY, message_id TEXT NOT NULL, session_id TEXT NOT NULL,
                block_index INTEGER NOT NULL, created_at_ms INTEGER NOT NULL,
                turn_index INTEGER NOT NULL DEFAULT 0, role TEXT NOT NULL,
                block_type TEXT NOT NULL, content TEXT, searchable_text TEXT,
                code_input TEXT, code_output TEXT, code_exit_code INTEGER, model TEXT,
                user_source TEXT, token_count INTEGER, extra TEXT
            );
            """
        )
        connection.execute(
            "INSERT INTO session_meta VALUES (?, ?, ?, ?, ?, ?)",
            (
                session_id,
                parent_id,
                "root" if parent_id is None else "nested",
                db_name,
                "2026-07-20T19:00:00+00:00",
                "2026-07-22T04:00:00+00:00",
            ),
        )
        conversation = json.dumps({"messages": messages}, separators=(",", ":"))
        connection.execute(
            "INSERT INTO conversation_state VALUES (1, ?, ?)",
            (conversation, "2026-07-22T04:00:00+00:00"),
        )
        connection.executemany(
            "INSERT INTO content_blocks VALUES "
            "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            blocks,
        )
        connection.commit()
    finally:
        connection.close()


def _task_database(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    connection = sqlite3.connect(path)
    try:
        connection.executescript(
            """
            CREATE TABLE tasks (
                local_id TEXT PRIMARY KEY, title TEXT NOT NULL, owner TEXT
            );
            CREATE TABLE task_notes (
                id INTEGER PRIMARY KEY AUTOINCREMENT, task_id TEXT NOT NULL,
                content TEXT NOT NULL, created_at TEXT NOT NULL,
                server_comment_id TEXT, author_unixname TEXT
            );
            INSERT INTO tasks VALUES ('task-a', 'Audit the scheduler', 'worker');
            """
        )
        connection.executemany(
            "INSERT INTO task_notes(task_id, content, created_at) VALUES (?, ?, ?)",
            (
                ("task-a", "First incarnation finding", "2026-07-21T10:00:00+00:00"),
                ("task-a", "Second incarnation result", "2026-07-21T17:00:00+00:00"),
                ("task-a", "", "2026-07-21T18:00:00+00:00"),
                ("task-a", "Exactly at exclusive end", "2026-07-22T04:00:00+00:00"),
            ),
        )
        connection.commit()
    finally:
        connection.close()


def _index_database(path: Path, sessions: Sequence[tuple[str, str | None]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    connection = sqlite3.connect(path)
    try:
        connection.execute(
            "CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_id TEXT)"
        )
        connection.executemany("INSERT INTO sessions VALUES (?, ?)", sessions)
        connection.commit()
    finally:
        connection.close()


def _append_index_session(path: Path, session_id: str, parent_id: str | None) -> None:
    connection = sqlite3.connect(path)
    try:
        connection.execute(
            "INSERT INTO sessions VALUES (?, ?)", (session_id, parent_id)
        )
        connection.commit()
    finally:
        connection.close()


def _rewrite_conversation(
    path: Path, messages: list[dict[str, object]]
) -> None:
    connection = sqlite3.connect(path)
    try:
        connection.execute(
            "UPDATE conversation_state SET conversation_json = ? WHERE id = 1",
            (json.dumps({"messages": messages}, separators=(",", ":")),),
        )
        connection.commit()
    finally:
        connection.close()


def _snapshot_database(snapshot_root: Path, source: OrcSourceCopy) -> Path:
    return snapshot_root.joinpath(*Path(source.snapshot_path).parts)


def _manifest_snapshot_database(archive: Path, kind: str) -> Path:
    manifest_path = (
        archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
    )
    root = as_object(read_json(manifest_path), str(manifest_path))
    source = next(
        as_object(item, f"{manifest_path}: source")
        for item in as_array(root.get("sources"), f"{manifest_path}: sources")
        if as_object(item, f"{manifest_path}: source").get("kind") == kind
    )
    relative = source.get("snapshot_path")
    if not isinstance(relative, str):
        raise AssertionError("test fixture snapshot path is not a string")
    return archive / "teams" / "orc-test" / "source_snapshots" / relative


def _managed_snapshot_objects(archive: Path) -> tuple[Path, ...]:
    root = archive / "teams" / "orc-test" / "source_snapshots" / ".objects"
    if not root.is_dir():
        return ()
    return tuple(
        sorted(
            path
            for path in root.glob("[0-9a-f][0-9a-f]/*.db")
            if path.is_file() and not path.is_symlink()
        )
    )


def _managed_task_projections(archive: Path) -> tuple[Path, ...]:
    root = archive / "teams" / "orc-test" / "source_snapshots" / ".projections"
    if not root.is_dir():
        return ()
    return tuple(
        sorted(
            path
            for path in root.glob("[0-9a-f][0-9a-f]/*.json")
            if path.is_file() and not path.is_symlink()
        )
    )


def _manifest_task_projection(archive: Path) -> tuple[Path, dict[str, JsonValue]]:
    manifest_path = (
        archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
    )
    root = as_object(read_json(manifest_path), str(manifest_path))
    task = next(
        as_object(item, f"{manifest_path}: source")
        for item in as_array(root.get("sources"), f"{manifest_path}: sources")
        if as_object(item, f"{manifest_path}: source").get("kind") == "task"
    )
    projection = as_object(
        task.get("task_projection"), f"{manifest_path}: task_projection"
    )
    relative = projection.get("path")
    if not isinstance(relative, str):
        raise AssertionError("test fixture projection path is not a string")
    path = archive / "teams" / "orc-test" / "source_snapshots" / relative
    return path, projection


def _append_root_message(path: Path, block_id: str, content: str) -> None:
    connection = sqlite3.connect(path)
    try:
        connection.execute(
            "INSERT INTO content_blocks VALUES "
            "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            (
                block_id,
                f"message-{block_id}",
                ROOT,
                0,
                _ms("2026-07-21T19:00:00+00:00"),
                3,
                "assistant",
                "text",
                content,
                None,
                None,
                None,
                None,
                "model",
                None,
                None,
                None,
            ),
        )
        connection.commit()
    finally:
        connection.close()


def _append_task_note(path: Path, content: str) -> None:
    connection = sqlite3.connect(path)
    try:
        connection.execute(
            "INSERT INTO task_notes(task_id, content, created_at) VALUES (?, ?, ?)",
            ("task-a", content, "2026-07-21T20:00:00+00:00"),
        )
        connection.commit()
    finally:
        connection.close()


def _non_agent_rewrite_messages(text: str) -> list[dict[str, object]]:
    return [
        {
            "id": 1,
            "role": "System",
            "created_at_ms": _ms("2026-07-21T03:00:00+00:00"),
            "blocks": [{"type": "AgentBlock", "id": 10, "agent_id": "worker"}],
        },
        {
            "id": 2,
            "role": "System",
            "created_at_ms": _ms("2026-07-21T16:00:00+00:00"),
            "blocks": [{"type": "AgentBlock", "id": 11, "agent_id": "worker"}],
        },
        {
            "id": 30,
            "role": "Assistant",
            "created_at_ms": _ms("2026-07-21T17:30:00+00:00"),
            "blocks": [{"type": "TextBlock", "id": 30, "text": text}],
        },
    ]


def _semantic_team(team: TeamData) -> tuple[object, ...]:
    """Exclude physical source-object provenance from normalized semantics."""

    return (
        team.team_slug,
        team.provider,
        team.root_thread_id,
        team.display_timezone,
        team.agents,
        team.turns,
        team.events,
        team.tool_calls,
        team.edges,
        team.window_start_ms,
        team.window_end_ms,
    )


def _legacy_source_digest(team: TeamData) -> str:
    """Reproduce the exact pre-semantic-provenance cache-key algorithm."""

    snapshots: list[JsonValue] = [
        {
            "thread_id": source.thread_id,
            "complete_bytes": source.complete_bytes,
            "sha256": source.sha256,
        }
        for source in team.sources
    ]
    return hashlib.sha256(canonical_json(snapshots).encode("utf-8")).hexdigest()


def _rewritten_messages(*, first_agent: str = "worker") -> list[dict[str, object]]:
    """Return a shorter conversation projection with all old spawn facts retained."""

    return [
        {
            "id": 1,
            "role": "System",
            "created_at_ms": _ms("2026-07-21T03:00:00+00:00"),
            "blocks": [{"type": "AgentBlock", "id": 10, "agent_id": first_agent}],
        },
        {
            "id": 2,
            "role": "System",
            "created_at_ms": _ms("2026-07-21T16:00:00+00:00"),
            "blocks": [{"type": "AgentBlock", "id": 11, "agent_id": "worker"}],
        },
        {
            "id": 5,
            "role": "System",
            "created_at_ms": _ms("2026-07-21T18:30:00+00:00"),
            "blocks": [{"type": "AgentBlock", "id": 12, "agent_id": "reviewer"}],
        },
    ]


def _fixture(tmp_path: Path) -> tuple[Path, Path, Path]:
    source = tmp_path / "source"
    root_db = source / ".orc" / "sessions" / ROOT / "session.db"
    nested_db = source / ".orc" / "sessions" / NESTED / "session.db"
    task_db = source / ".tg" / "project.db"
    messages = [
        {
            "id": 1,
            "role": "System",
            "created_at_ms": _ms("2026-07-21T03:00:00+00:00"),
            "blocks": [{"type": "AgentBlock", "id": 10, "agent_id": "worker"}],
        },
        {
            "id": 2,
            "role": "System",
            "created_at_ms": _ms("2026-07-21T16:00:00+00:00"),
            "blocks": [{"type": "AgentBlock", "id": 11, "agent_id": "worker"}],
        },
        {
            "id": 3,
            "role": "Assistant",
            "created_at_ms": _ms("2026-07-21T17:00:00+00:00"),
            "blocks": [{"type": "TextBlock", "id": 20, "text": "temporary"}],
        },
        {
            "id": 4,
            "role": "Assistant",
            "created_at_ms": _ms("2026-07-21T17:01:00+00:00"),
            "blocks": [{"type": "TextBlock", "id": 21, "text": "temporary"}],
        },
    ]
    root_blocks = [
        (
            "root-user",
            "message-1",
            ROOT,
            0,
            _ms("2026-07-21T04:00:00+00:00"),
            1,
            "user",
            "text",
            "Start the complete local day",
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
        ),
        (
            "root-tool",
            "message-2",
            ROOT,
            0,
            _ms("2026-07-21T04:00:01+00:00"),
            1,
            "assistant",
            "code_execution",
            None,
            None,
            "await orc.readFile('x'); await orc.readFile('y'); await orc.sendAgent('w','x')",
            "ok",
            0,
            "model",
            None,
            None,
            None,
        ),
        (
            "root-end",
            "message-3",
            ROOT,
            0,
            _ms("2026-07-22T04:00:00+00:00"),
            2,
            "assistant",
            "text",
            "Outside the half-open window",
            None,
            None,
            None,
            None,
            "model",
            None,
            None,
            None,
        ),
    ]
    nested_blocks = [
        (
            "nested-text",
            "nested-message",
            NESTED,
            0,
            _ms("2026-07-21T12:00:00+00:00"),
            1,
            "assistant",
            "text",
            "Nested coordinator update",
            None,
            None,
            None,
            None,
            "model",
            None,
            None,
            None,
        )
    ]
    _session_database(
        root_db,
        ROOT,
        parent_id=None,
        db_name="project",
        messages=messages,
        blocks=root_blocks,
    )
    _session_database(
        nested_db,
        NESTED,
        parent_id=ROOT,
        db_name=None,
        messages=[],
        blocks=nested_blocks,
    )
    _index_database(
        source / ".orc" / "index.db",
        ((ROOT, None), (NESTED, ROOT)),
    )
    _task_database(task_db)
    return source, root_db, task_db


def test_snapshot_and_parse_orc_lineage_read_only_and_idempotently(
    tmp_path: Path,
) -> None:
    source, root_db, task_db = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    root_db.chmod(0o444)
    task_db.chmod(0o444)

    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    team = load_orc_team(snapshot, ROOT, "orc-test", "America/New_York", first.sources)
    second = snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")

    assert len(first.sources) == 3
    assert second.files_changed == 0
    assert second.sources == first.sources
    assert stat.S_IMODE(root_db.stat().st_mode) & 0o222 == 0
    assert stat.S_IMODE(task_db.stat().st_mode) & 0o222 == 0
    nested = next(agent for agent in team.agents if agent.thread_id == NESTED)
    assert nested.parent_thread_id == ROOT
    assert nested.depth == 1
    workers = [agent for agent in team.agents if agent.agent_path.endswith("/worker")]
    assert len(workers) == 2
    first_event = next(event for event in team.events if "First incarnation" in (event.text or ""))
    second_event = next(
        event for event in team.events if "Second incarnation" in (event.text or "")
    )
    assert first_event.thread_id != second_event.thread_id
    assert first_event.kind == "inter_agent_message"
    assert first_event.phase is None
    assert first_event.author == first_event.thread_id
    assert first_event.recipient == ROOT
    tools = team.tool_calls
    assert len(tools) == 1
    assert tools[0].nested_tools == (("readFile", 2), ("sendAgent", 1))


def test_classifies_orc_inputs_from_user_source_and_extra(tmp_path: Path) -> None:
    source, root_db, _ = _fixture(tmp_path)
    inputs: list[tuple[object, ...]] = [
        (
            "gchat-legacy-owner",
            "message-gchat-legacy-owner",
            ROOT,
            0,
            _ms("2026-07-21T04:59:00+00:00"),
            9,
            "user",
            "text",
            "Legacy owner message",
            None,
            None,
            None,
            None,
            None,
            json.dumps(
                {
                    "GChat": {
                        "message_name": "spaces/x/messages/legacy-owner",
                        "sender_name": "users/owner-id",
                    }
                }
            ),
            None,
            None,
        ),
        (
            "gchat-owner",
            "message-gchat-owner",
            ROOT,
            0,
            _ms("2026-07-21T05:00:00+00:00"),
            10,
            "user",
            "text",
            "Owner message",
            None,
            None,
            None,
            None,
            None,
            json.dumps(
                {
                    "GChat": {
                        "message_name": "spaces/x/messages/owner",
                        "sender_unixname": "newton",
                        "sender_name": "users/owner-id",
                        "is_owner": True,
                    }
                }
            ),
            None,
            json.dumps({"sender_display_name": "Ryan Newton"}),
        ),
        (
            "gchat-other",
            "message-gchat-other",
            ROOT,
            0,
            _ms("2026-07-21T05:01:00+00:00"),
            11,
            "user",
            "text",
            "Another person's message",
            None,
            None,
            None,
            None,
            None,
            json.dumps(
                {
                    "GChat": {
                        "message_name": "spaces/x/messages/other",
                        "sender_display_name": "A Collaborator",
                        "is_owner": False,
                    }
                }
            ),
            None,
            None,
        ),
        (
            "orc-child",
            "message-orc-child",
            ROOT,
            0,
            _ms("2026-07-21T05:02:00+00:00"),
            12,
            "user",
            "text",
            "Child Orc report",
            None,
            None,
            None,
            None,
            None,
            json.dumps({"Orc": {"sender_session": "child-session"}}),
            None,
            None,
        ),
        (
            "submitted-web",
            "message-submitted-web",
            ROOT,
            0,
            _ms("2026-07-21T05:03:00+00:00"),
            13,
            "user",
            "text",
            "Web submission",
            None,
            None,
            None,
            None,
            None,
            json.dumps({"Submitted": {"source": {"Web": {"view": "Inbox"}}}}),
            None,
            None,
        ),
        (
            "tui-unknown",
            "message-tui-unknown",
            ROOT,
            0,
            _ms("2026-07-21T05:04:00+00:00"),
            14,
            "user",
            "text",
            "Unattributed terminal input",
            None,
            None,
            None,
            None,
            None,
            json.dumps({"Submitted": {"source": "Tui"}}),
            None,
            None,
        ),
        (
            "scheduled-reminder",
            "message-scheduled-reminder",
            ROOT,
            0,
            _ms("2026-07-21T05:05:00+00:00"),
            15,
            "user",
            "text",
            "This is your periodic reminder to make sure your running state is aligned "
            "with your overarching goals, listed below.",
            None,
            None,
            None,
            None,
            None,
            json.dumps({"Submitted": {"source": "Tui"}}),
            None,
            None,
        ),
        (
            "scheduled-web-reminder",
            "message-scheduled-web-reminder",
            ROOT,
            0,
            _ms("2026-07-21T05:06:00+00:00"),
            16,
            "user",
            "text",
            "This is your periodic reminder to make sure your running state is aligned "
            "with your overarching goals, listed below.",
            None,
            None,
            None,
            None,
            None,
            json.dumps({"Submitted": {"source": {"Web": {"view": "Inbox"}}}}),
            None,
            None,
        ),
    ]
    connection = sqlite3.connect(root_db)
    try:
        connection.executemany(
            "INSERT INTO content_blocks VALUES "
            "(?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            inputs,
        )
        connection.commit()
    finally:
        connection.close()

    snapshot = tmp_path / "snapshot"
    captured = snapshot_orc_lineage(source, ROOT, snapshot, (), "captured")
    team = load_orc_team(snapshot, ROOT, "orc-authorship", "UTC", captured.sources)
    events = {event.event_id: event for event in team.events}

    owner = events["orc-block-gchat-owner"]
    assert owner.kind == "user_prompt"
    assert owner.author == "newton"
    assert owner.author_kind == "owner_human"
    assert owner.ingress_kind == "gchat"
    assert owner.source_native_id == "spaces/x/messages/owner"
    assert events["orc-block-gchat-legacy-owner"].author_kind == "owner_human"
    other = events["orc-block-gchat-other"]
    assert other.kind == "external_message"
    assert other.author_kind == "other_human"
    child = events["orc-block-orc-child"]
    assert child.kind == "inter_agent_message"
    assert child.author == "child-session"
    assert child.recipient == ROOT
    assert events["orc-block-submitted-web"].author_kind == "external_or_unknown"
    assert events["orc-block-tui-unknown"].author_kind == "unknown"
    scheduled = events["orc-block-scheduled-reminder"]
    assert scheduled.kind == "system_input"
    assert scheduled.ingress_kind == "scheduled"
    assert scheduled.author_kind == "system"
    assert events["orc-block-scheduled-web-reminder"].kind == "system_input"
    assert all(
        event.classification_version == "authorship-v2"
        for event in events.values()
        if event.event_id.startswith("orc-block-")
    )


def test_rewritten_auxiliary_history_accepts_stable_spawns_and_is_idempotent(
    tmp_path: Path,
) -> None:
    source, root_db, _ = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    before = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", first.sources
    )

    _append_root_message(root_db, "root-appended", "Authoritative appended result")
    _rewrite_conversation(root_db, _rewritten_messages())
    second = snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")
    after = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", second.sources
    )
    root_source = next(
        source_copy
        for source_copy in second.sources
        if source_copy.kind == "session" and source_copy.owner_session_id == ROOT
    )

    assert {event.event_id for event in before.events} < {
        event.event_id for event in after.events
    }
    assert {agent.thread_id for agent in before.agents} < {
        agent.thread_id for agent in after.agents
    }
    assert root_source.auxiliary.message_count == 3
    assert root_source.auxiliary.stable_spawn_count == 3
    assert root_source.auxiliary.rewrite_count == 1
    assert root_source.auxiliary.last_rewrite_at == "second"
    assert root_source.auxiliary.degraded is True
    assert root_source.auxiliary.degradation_reason == (
        "conversation-history-rewritten-stable-spawns-preserved"
    )

    third = snapshot_orc_lineage(source, ROOT, snapshot, second.sources, "third")
    assert third.files_changed == 0
    assert third.sources == second.sources


def test_repeated_auxiliary_rewrites_increment_once_per_distinct_observation(
    tmp_path: Path,
) -> None:
    source, root_db, _ = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    _rewrite_conversation(root_db, _non_agent_rewrite_messages("first rewrite"))
    second = snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")
    _rewrite_conversation(root_db, _non_agent_rewrite_messages("second rewrite"))
    third = snapshot_orc_lineage(source, ROOT, snapshot, second.sources, "third")
    fourth = snapshot_orc_lineage(source, ROOT, snapshot, third.sources, "fourth")
    root_second = next(item for item in second.sources if item.kind == "session")
    root_third = next(item for item in third.sources if item.kind == "session")
    root_fourth = next(item for item in fourth.sources if item.kind == "session")

    assert root_second.auxiliary.rewrite_count == 1
    assert root_second.auxiliary.last_rewrite_at == "second"
    assert root_third.auxiliary.rewrite_count == 2
    assert root_third.auxiliary.last_rewrite_at == "third"
    assert root_fourth.auxiliary.rewrite_count == 2
    assert root_fourth.auxiliary.last_rewrite_at == "third"
    assert fourth.files_changed == 0


@pytest.mark.parametrize(
    ("rewrite_kind", "message"),
    (
        ("agent", "stable spawn evidence was rewritten"),
        ("message", "stable spawn evidence was rewritten"),
        ("timestamp", "stable spawn evidence was rewritten"),
        ("missing", "stable spawn evidence disappeared"),
    ),
)
def test_rewritten_stable_spawn_is_rejected_and_snapshot_is_preserved(
    tmp_path: Path, rewrite_kind: str, message: str
) -> None:
    source, root_db, _ = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    root_source = next(
        item
        for item in first.sources
        if item.kind == "session" and item.owner_session_id == ROOT
    )
    snapshot_db = _snapshot_database(snapshot, root_source)
    prior_bytes = snapshot_db.read_bytes()
    _append_root_message(root_db, "root-appended", "Must not replace the snapshot")
    rewritten = _rewritten_messages(
        first_agent="renamed-worker" if rewrite_kind == "agent" else "worker"
    )
    if rewrite_kind == "message":
        rewritten[0]["id"] = 99
    if rewrite_kind == "timestamp":
        rewritten[0]["created_at_ms"] = _ms("2026-07-21T03:00:01+00:00")
    if rewrite_kind == "missing":
        rewritten = rewritten[1:]
    _rewrite_conversation(root_db, rewritten)

    with pytest.raises(OrcParseError, match=message):
        snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")
    assert snapshot_db.read_bytes() == prior_bytes


def test_window_excludes_exact_end_but_retains_pre_window_spawn_context(
    tmp_path: Path,
) -> None:
    source, _, _ = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    copied = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    team = load_orc_team(snapshot, ROOT, "orc-test", "America/New_York", copied.sources)
    window = parse_date_window("2026-07-21", "2026-07-22", "America/New_York")
    assert window is not None
    assert window.start_ms is not None
    phases = build_phases(apply_date_window(team, window))
    transcript = "\n".join(phase.transcript_text for phase in phases)

    assert "Start the complete local day" in transcript
    assert "First incarnation finding" in transcript
    assert "Second incarnation result" in transcript
    assert "Exactly at exclusive end" not in transcript
    assert "Outside the half-open window" not in transcript
    assert any(agent.started_at_ms < window.start_ms for agent in team.agents)
    assert sum(phase.stats.inter_agent_messages for phase in phases) == 2


def test_rewritten_task_note_prefix_is_rejected(tmp_path: Path) -> None:
    source, _, task_db = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    connection = sqlite3.connect(task_db)
    try:
        connection.execute(
            "UPDATE task_notes SET content = 'rewritten' WHERE id = 1"
        )
        connection.commit()
    finally:
        connection.close()

    with pytest.raises(OrcParseError, match="existing append prefix was rewritten"):
        snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")


@pytest.mark.parametrize("field", ("owner", "title"))
def test_rewritten_task_metadata_preserves_frozen_note_enrichment(
    tmp_path: Path, field: str
) -> None:
    source, _, task_db = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    before = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", first.sources
    )
    connection = sqlite3.connect(task_db)
    try:
        connection.execute(
            f"UPDATE tasks SET {field} = ? WHERE local_id = 'task-a'",
            (f"changed-{field}",),
        )
        connection.commit()
    finally:
        connection.close()

    second = snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")
    after = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", second.sources
    )
    task_source = next(item for item in second.sources if item.kind == "task")

    assert _semantic_team(after) == _semantic_team(before)
    assert source_digest(after) == source_digest(before)
    assert task_source.task_projection is not None
    assert task_source.task_projection.rewrite_count == 1
    assert task_source.task_projection.degraded is True


def test_growing_mutable_task_metadata_preserves_semantic_source_digest(
    tmp_path: Path,
) -> None:
    source, _, task_db = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    before = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", first.sources
    )
    first_task = next(item for item in first.sources if item.kind == "task")
    connection = sqlite3.connect(task_db)
    try:
        connection.execute(
            "UPDATE tasks SET title = ? WHERE local_id = 'task-a'",
            ("expanded-" + "x" * 100_000,),
        )
        connection.commit()
    finally:
        connection.close()

    second = snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")
    after = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", second.sources
    )
    second_task = next(item for item in second.sources if item.kind == "task")

    assert second_task.snapshot_size > first_task.snapshot_size
    assert _semantic_team(after) == _semantic_team(before)
    assert source_digest(after) == source_digest(before)


def test_task_note_server_sync_id_preserves_team_digest_and_summary_cache(
    tmp_path: Path,
) -> None:
    source, _, task_db = _fixture(tmp_path)
    archive = tmp_path / "archive"
    before, _ = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )
    summarize_archive(archive, "orc-test", "heuristic", "fixture")
    connection = sqlite3.connect(task_db)
    try:
        connection.execute(
            "UPDATE task_notes SET server_comment_id = 'server-1' WHERE id = 1"
        )
        connection.commit()
    finally:
        connection.close()

    after, _ = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )
    cached = summarize_archive(archive, "orc-test", "heuristic", "fixture")
    task = next(item for item in after.sources if item.path == ".tg/project.db")
    task_manifest = next(
        item
        for item in as_array(
            as_object(
                read_json(
                    archive
                    / "teams"
                    / "orc-test"
                    / "raw"
                    / "source-manifest.json"
                ),
                "manifest",
            ).get("sources"),
            "manifest.sources",
        )
        if as_object(item, "manifest.source").get("kind") == "task"
    )
    projection = as_object(
        as_object(task_manifest, "manifest.task").get("task_projection"),
        "manifest.task.task_projection",
    )

    assert _semantic_team(after) == _semantic_team(before)
    assert source_digest(after) == source_digest(before)
    assert task.semantic_sha256 is not None
    assert projection.get("rewrite_count") == 0
    assert cached.cache_misses == 0
    assert cached.cache_hits > 0


def test_task_note_author_fill_and_change_preserve_frozen_attribution_and_cache(
    tmp_path: Path,
) -> None:
    source, _, task_db = _fixture(tmp_path)
    archive = tmp_path / "archive"
    before, _ = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )
    summarize_archive(archive, "orc-test", "heuristic", "fixture")

    for author, expected_rewrites in (("reviewer", 1), ("third-owner", 2)):
        connection = sqlite3.connect(task_db)
        try:
            connection.execute(
                "UPDATE task_notes SET author_unixname = ? WHERE id = 1",
                (author,),
            )
            connection.commit()
        finally:
            connection.close()
        after, _ = ingest_orc(
            archive, source, ROOT, "orc-test", "America/New_York"
        )
        cached = summarize_archive(
            archive, "orc-test", "heuristic", "fixture"
        )
        manifest_path = (
            archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
        )
        manifest = as_object(read_json(manifest_path), str(manifest_path))
        task = next(
            as_object(item, "manifest.source")
            for item in as_array(manifest.get("sources"), "manifest.sources")
            if as_object(item, "manifest.source").get("kind") == "task"
        )
        projection = as_object(
            task.get("task_projection"), "manifest.task.task_projection"
        )

        assert _semantic_team(after) == _semantic_team(before)
        assert source_digest(after) == source_digest(before)
        assert projection.get("rewrite_count") == expected_rewrites
        assert cached.cache_misses == 0
        assert cached.cache_hits > 0


def test_external_task_note_is_user_provenance_without_agent_or_edge(
    tmp_path: Path,
) -> None:
    source, _, task_db = _fixture(tmp_path)
    connection = sqlite3.connect(task_db)
    try:
        connection.execute(
            "UPDATE task_notes SET author_unixname = 'external-reviewer' WHERE id = 1"
        )
        connection.commit()
    finally:
        connection.close()

    copied = snapshot_orc_lineage(source, ROOT, tmp_path / "snapshot", (), "first")
    team = load_orc_team(
        tmp_path / "snapshot", ROOT, "orc-test", "America/New_York", copied.sources
    )
    event = next(item for item in team.events if item.event_id.endswith("-1"))
    phases = build_phases(team)

    assert event.kind == "external_message"
    assert event.role == "user"
    assert event.thread_id == ROOT
    assert event.author == "external-reviewer"
    assert event.recipient is None
    assert "external author: external-reviewer" in (event.text or "")
    assert all("external-reviewer" not in agent.agent_path for agent in team.agents)
    assert all(edge.call_id != event.event_id for edge in team.edges)
    assert sum(phase.stats.external_messages for phase in phases) == 1
    assert sum(phase.stats.user_prompts for phase in phases) == 1


def test_lone_external_task_note_creates_a_visible_phase(tmp_path: Path) -> None:
    source, root_db, task_db = _fixture(tmp_path)
    nested_db = source / ".orc" / "sessions" / NESTED / "session.db"
    for session_db in (root_db, nested_db):
        connection = sqlite3.connect(session_db)
        try:
            connection.execute("DELETE FROM content_blocks")
            connection.commit()
        finally:
            connection.close()
    connection = sqlite3.connect(task_db)
    try:
        connection.execute("DELETE FROM task_notes WHERE id != 1")
        connection.execute(
            "UPDATE task_notes SET author_unixname = 'external-reviewer' WHERE id = 1"
        )
        connection.commit()
    finally:
        connection.close()

    snapshot = tmp_path / "snapshot"
    copied = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    team = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", copied.sources
    )
    phases = build_phases(team)

    assert len(team.events) == 1
    assert len(phases) == 1
    assert phases[0].stats.external_messages == 1
    assert "EXTERNAL:" in phases[0].transcript_text
    assert "First incarnation finding" in phases[0].transcript_text


def test_unattributed_local_task_notes_are_preserved_on_synthetic_worker(
    tmp_path: Path,
) -> None:
    source, _, task_db = _fixture(tmp_path)
    connection = sqlite3.connect(task_db)
    try:
        connection.execute("UPDATE tasks SET owner = NULL WHERE local_id = 'task-a'")
        connection.commit()
    finally:
        connection.close()

    snapshot = tmp_path / "snapshot"
    copied = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    team = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", copied.sources
    )
    notes = [event for event in team.events if event.event_id.startswith("orc-note-")]
    unattributed = next(
        agent for agent in team.agents if agent.agent_path.endswith("/Unattributed Task Work")
    )

    assert {event.source_line for event in notes} == {1, 2, 4}
    assert {event.thread_id for event in notes} == {unattributed.thread_id}
    assert all(event.kind == "inter_agent_message" for event in notes)
    assert all("unattributed local task work" in (event.text or "") for event in notes)
    assert sum(
        edge.kind == "message" and edge.from_thread_id == unattributed.thread_id
        for edge in team.edges
    ) == 3


def test_new_task_identity_is_a_monotonic_addition(tmp_path: Path) -> None:
    source, _, task_db = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    connection = sqlite3.connect(task_db)
    try:
        connection.execute(
            "INSERT INTO tasks VALUES ('task-b', 'New task', 'reviewer')"
        )
        connection.commit()
    finally:
        connection.close()
    second = snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")
    first_task = next(item for item in first.sources if item.kind == "task")
    task_source = next(item for item in second.sources if item.kind == "task")

    assert task_source.task_projection is not None
    assert first_task.task_projection is not None
    assert task_source.task_projection.note_count == 4
    assert task_source.task_projection.path == first_task.task_projection.path
    assert task_source.semantic_sha256 == first_task.semantic_sha256


def test_new_note_captures_current_enrichment_then_freezes_it(tmp_path: Path) -> None:
    source, _, task_db = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    before = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", first.sources
    )
    connection = sqlite3.connect(task_db)
    try:
        connection.execute(
            "UPDATE tasks SET owner = 'reviewer', title = 'Reassigned task title' "
            "WHERE local_id = 'task-a'"
        )
        connection.commit()
    finally:
        connection.close()
    _append_task_note(task_db, "Finding after reassignment")
    second = snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")
    after_append = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", second.sources
    )
    new_event = next(
        event for event in after_append.events if event.event_id.endswith("-5")
    )
    prior_events = {event.event_id: event for event in before.events}

    assert new_event.text is not None
    assert new_event.text.startswith("[task-a · Reassigned task title]")
    assert all(
        event == prior_events[event.event_id]
        for event in after_append.events
        if event.event_id in prior_events
    )
    second_task = next(item for item in second.sources if item.kind == "task")
    assert second_task.task_projection is not None
    assert second_task.task_projection.note_count == 5
    assert second_task.task_projection.rewrite_count == 1

    connection = sqlite3.connect(task_db)
    try:
        connection.execute(
            "UPDATE tasks SET owner = 'third-owner', title = 'Third mutable title' "
            "WHERE local_id = 'task-a'"
        )
        connection.commit()
    finally:
        connection.close()
    third = snapshot_orc_lineage(source, ROOT, snapshot, second.sources, "third")
    after_rewrite = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", third.sources
    )
    third_task = next(item for item in third.sources if item.kind == "task")

    assert _semantic_team(after_rewrite) == _semantic_team(after_append)
    assert source_digest(after_rewrite) == source_digest(after_append)
    assert third_task.task_projection is not None
    assert third_task.task_projection.rewrite_count == 2
    assert third_task.task_projection.last_rewrite_at == "third"


def test_semantic_identity_is_equal_for_one_shot_and_two_step_ingest(
    tmp_path: Path,
) -> None:
    one_source, one_root, one_task = _fixture(tmp_path / "one")
    two_source, two_root, two_task = _fixture(tmp_path / "two")
    _append_root_message(one_root, "root-appended", "deterministic final state")
    _append_task_note(one_task, "deterministic task state")
    one_snapshot = tmp_path / "one-snapshot"
    one = snapshot_orc_lineage(one_source, ROOT, one_snapshot, (), "one")
    one_team = load_orc_team(
        one_snapshot, ROOT, "orc-test", "America/New_York", one.sources
    )

    two_snapshot = tmp_path / "two-snapshot"
    two_initial = snapshot_orc_lineage(two_source, ROOT, two_snapshot, (), "first")
    _append_root_message(two_root, "root-appended", "deterministic final state")
    _append_task_note(two_task, "deterministic task state")
    two = snapshot_orc_lineage(
        two_source, ROOT, two_snapshot, two_initial.sources, "second"
    )
    two_team = load_orc_team(
        two_snapshot, ROOT, "orc-test", "America/New_York", two.sources
    )

    one_identities = {
        item.source_path: (
            item.semantic_identity_mode,
            item.semantic_sha256,
            item.semantic_complete_bytes,
        )
        for item in one.sources
    }
    two_identities = {
        item.source_path: (
            item.semantic_identity_mode,
            item.semantic_sha256,
            item.semantic_complete_bytes,
        )
        for item in two.sources
    }
    assert one_identities == two_identities
    assert source_digest(one_team) == source_digest(two_team)
    assert _semantic_team(one_team) == _semantic_team(two_team)


def test_tampered_semantic_identity_is_rejected(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    session = next(item for item in first.sources if item.kind == "session")
    tampered = replace(session, semantic_sha256="0" * 64)
    sources = tuple(tampered if item == session else item for item in first.sources)

    with pytest.raises(OrcParseError, match="deterministic semantic identity"):
        snapshot_orc_lineage(source, ROOT, snapshot, sources, "second")


@pytest.mark.parametrize(
    ("field", "value", "message"),
    (
        (
            "created_at",
            "2026-07-20T20:00:00+00:00",
            "immutable session metadata was rewritten",
        ),
        (
            "updated_at",
            "2026-07-19T20:00:00+00:00",
            "session updated_at moved backwards",
        ),
    ),
)
def test_session_metadata_rewrite_is_rejected(
    tmp_path: Path, field: str, value: str, message: str
) -> None:
    source, root_db, _ = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    connection = sqlite3.connect(root_db)
    try:
        connection.execute(f"UPDATE session_meta SET {field} = ?", (value,))
        connection.commit()
    finally:
        connection.close()

    with pytest.raises(OrcParseError, match=message):
        snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")


def test_session_name_change_is_accepted_as_semantic_change(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    nested_db = source / ".orc" / "sessions" / NESTED / "session.db"
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    before = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", first.sources
    )
    connection = sqlite3.connect(nested_db)
    try:
        connection.execute("UPDATE session_meta SET name = 'renamed-worker'")
        connection.commit()
    finally:
        connection.close()

    second = snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")
    after = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", second.sources
    )

    assert source_digest(after) != source_digest(before)
    assert any(agent.nickname == "renamed-worker" for agent in after.agents)


def test_truncated_task_notes_are_rejected_and_snapshot_is_preserved(
    tmp_path: Path,
) -> None:
    source, _, task_db = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    snapshot_task = _snapshot_database(
        snapshot, next(item for item in first.sources if item.kind == "task")
    )
    prior_bytes = snapshot_task.read_bytes()
    connection = sqlite3.connect(task_db)
    try:
        connection.execute("DELETE FROM task_notes WHERE id = 4")
        connection.commit()
    finally:
        connection.close()

    with pytest.raises(OrcParseError, match="append history shrank"):
        snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")
    assert snapshot_task.read_bytes() == prior_bytes


def test_disappeared_task_database_is_rejected(tmp_path: Path) -> None:
    source, _, task_db = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    task_db.unlink()

    with pytest.raises(OrcParseError, match="source disappeared"):
        snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")


def test_provider_initial_session_without_name_uses_session_task_database(
    tmp_path: Path,
) -> None:
    source, root_db, task_db = _fixture(tmp_path)
    fallback = source / ".tg" / f"{ROOT}.db"
    task_db.replace(fallback)
    connection = sqlite3.connect(root_db)
    try:
        connection.execute("UPDATE session_meta SET db_name = NULL")
        connection.commit()
    finally:
        connection.close()

    copied = snapshot_orc_lineage(source, ROOT, tmp_path / "snapshot", (), "first")

    assert {item.source_path for item in copied.sources if item.kind == "task"} == {
        f".tg/{ROOT}.db"
    }


def test_delegated_session_uses_its_id_even_when_db_name_is_inherited(
    tmp_path: Path,
) -> None:
    source, _, task_db = _fixture(tmp_path)
    nested_db = source / ".orc" / "sessions" / NESTED / "session.db"
    delegated_task = source / ".tg" / f"{NESTED}.db"
    shutil.copy2(task_db, delegated_task)
    connection = sqlite3.connect(nested_db)
    try:
        connection.execute("UPDATE session_meta SET db_name = 'project'")
        connection.commit()
    finally:
        connection.close()

    copied = snapshot_orc_lineage(
        source, NESTED, tmp_path / "snapshot", (), "first"
    )

    assert {item.source_path for item in copied.sources if item.kind == "task"} == {
        f".tg/{NESTED}.db"
    }


def test_missing_never_observed_task_references_are_lazy(tmp_path: Path) -> None:
    source, root_db, _, = _fixture(tmp_path)
    connection = sqlite3.connect(root_db)
    try:
        connection.execute("UPDATE session_meta SET db_name = 'not-created'")
        connection.execute(
            "INSERT INTO associated_dbs(db_name) VALUES ('also-not-created')"
        )
        connection.commit()
    finally:
        connection.close()

    copied = snapshot_orc_lineage(source, ROOT, tmp_path / "snapshot", (), "first")

    assert all(item.kind == "session" for item in copied.sources)


def test_no_index_ignores_unrelated_session_debris(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    (source / ".orc" / "index.db").unlink()
    unrelated = source / ".orc" / "sessions" / "unrelated" / "session.db"
    unrelated.parent.mkdir(parents=True)
    unrelated.write_bytes(b"not a sqlite database")

    copied = snapshot_orc_lineage(source, ROOT, tmp_path / "snapshot", (), "first")

    assert {item.owner_session_id for item in copied.sources if item.kind == "session"} == {
        ROOT
    }
    assert {item.source_path for item in copied.sources if item.kind == "task"} == {
        ".tg/project.db"
    }


def test_indexed_selected_corrupt_child_fails_closed(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    child = "33333333-3333-3333-3333-333333333333"
    _append_index_session(source / ".orc" / "index.db", child, ROOT)
    database = source / ".orc" / "sessions" / child / "session.db"
    database.parent.mkdir(parents=True)
    database.write_bytes(b"not a sqlite database")

    with pytest.raises(OrcParseError, match="failed to inspect Orc session metadata"):
        snapshot_orc_lineage(source, ROOT, tmp_path / "snapshot", (), "first")


def test_associated_database_reference_change_during_snapshot_retries(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source, root_db, _ = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    original_backup = orc_module._backup_database
    changed = False

    def mutate_then_backup(
        source_path: Path, destination: Path, snapshot_root: Path
    ) -> None:
        nonlocal changed
        if not changed and source_path == root_db:
            connection = sqlite3.connect(root_db)
            try:
                connection.execute(
                    "INSERT INTO associated_dbs(db_name) VALUES ('during-copy')"
                )
                connection.commit()
            finally:
                connection.close()
            changed = True
        original_backup(source_path, destination, snapshot_root)

    monkeypatch.setattr(orc_module, "_backup_database", mutate_then_backup)
    with pytest.raises(OrcParseError, match="references changed during snapshot"):
        snapshot_orc_lineage(source, ROOT, snapshot, (), "first")

    assert not tuple(snapshot.glob(".objects/*/*.db"))


def test_parent_lifetime_contains_nested_session_activity(tmp_path: Path) -> None:
    source, root_db, task_db = _fixture(tmp_path)
    connection = sqlite3.connect(root_db)
    try:
        connection.execute("DELETE FROM content_blocks")
        connection.execute(
            "UPDATE conversation_state SET conversation_json = '{\"messages\":[]}'"
        )
        connection.commit()
    finally:
        connection.close()
    connection = sqlite3.connect(task_db)
    try:
        connection.execute("DELETE FROM task_notes")
        connection.commit()
    finally:
        connection.close()

    snapshot = tmp_path / "snapshot"
    copied = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    team = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", copied.sources
    )
    root = next(agent for agent in team.agents if agent.thread_id == ROOT)
    nested = next(agent for agent in team.agents if agent.thread_id == NESTED)

    assert nested.ended_at_ms is not None
    assert root.ended_at_ms is not None
    assert root.ended_at_ms >= nested.ended_at_ms
    assert root.ended_at_ms > root.started_at_ms + 1


def test_lifetime_end_propagates_across_two_nested_hops(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    child = "33333333-3333-3333-3333-333333333333"
    child_block = (
        "child-late",
        "child-message",
        child,
        0,
        _ms("2026-07-23T12:00:00+00:00"),
        1,
        "assistant",
        "text",
        "Grandchild coordinator activity",
        None,
        None,
        None,
        None,
        "model",
        None,
        None,
        None,
    )
    _session_database(
        source / ".orc" / "sessions" / child / "session.db",
        child,
        parent_id=NESTED,
        db_name="inherited-name",
        messages=[],
        blocks=[child_block],
    )
    _append_index_session(source / ".orc" / "index.db", child, NESTED)

    snapshot = tmp_path / "snapshot"
    copied = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    team = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", copied.sources
    )
    agents = {agent.thread_id: agent for agent in team.agents}

    child_end = agents[child].ended_at_ms
    nested_end = agents[NESTED].ended_at_ms
    root_end = agents[ROOT].ended_at_ms
    assert child_end is not None
    assert nested_end is not None
    assert root_end is not None
    assert nested_end >= child_end
    assert root_end >= nested_end


def test_shared_task_database_keeps_its_recorded_owner(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    earlier = "00000000-0000-0000-0000-000000000001"
    _session_database(
        source / ".orc" / "sessions" / earlier / "session.db",
        earlier,
        parent_id=ROOT,
        db_name="project",
        messages=[],
        blocks=[],
    )
    connection = sqlite3.connect(
        source / ".orc" / "sessions" / earlier / "session.db"
    )
    try:
        connection.execute(
            "INSERT INTO associated_dbs(db_name) VALUES ('project')"
        )
        connection.commit()
    finally:
        connection.close()
    _append_index_session(source / ".orc" / "index.db", earlier, ROOT)
    second = snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")
    task_source = next(item for item in second.sources if item.kind == "task")

    assert task_source.owner_session_id == ROOT


def test_shared_task_database_keeps_owner_after_its_attachment_is_removed(
    tmp_path: Path,
) -> None:
    source, root_db, _ = _fixture(tmp_path)
    nested_db = source / ".orc" / "sessions" / NESTED / "session.db"
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    for path, db_name in ((root_db, None), (nested_db, "ignored-inherited-name")):
        connection = sqlite3.connect(path)
        try:
            connection.execute("UPDATE session_meta SET db_name = ?", (db_name,))
            if path == nested_db:
                connection.execute(
                    "INSERT INTO associated_dbs(db_name) VALUES ('project')"
                )
            connection.commit()
        finally:
            connection.close()

    second = snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")
    task = next(item for item in second.sources if item.kind == "task")

    assert task.owner_session_id == ROOT
    assert task.source_state == "live"


def test_late_task_database_attachment_is_discovered(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    nested_db = source / ".orc" / "sessions" / NESTED / "session.db"
    late_task_db = source / ".tg" / "late.db"
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    _task_database(late_task_db)
    connection = sqlite3.connect(nested_db)
    try:
        connection.execute(
            "INSERT INTO associated_dbs(db_name) VALUES ('late')"
        )
        connection.commit()
    finally:
        connection.close()

    second = snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")

    assert {item.source_path for item in second.sources} == {
        *{item.source_path for item in first.sources},
        ".tg/late.db",
    }


def test_detached_task_database_retains_archived_history(tmp_path: Path) -> None:
    source, root_db, task_db = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    before = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", first.sources
    )
    connection = sqlite3.connect(root_db)
    try:
        connection.execute("UPDATE session_meta SET db_name = NULL")
        connection.commit()
    finally:
        connection.close()
    task_db.unlink()

    second = snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")
    after = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", second.sources
    )
    task = next(item for item in second.sources if item.kind == "task")

    assert task.source_state == "detached"
    assert {event.event_id for event in before.events} <= {
        event.event_id for event in after.events
    }

    third = snapshot_orc_lineage(source, ROOT, snapshot, second.sources, "third")
    assert third.sources == second.sources


def test_task_database_replacement_retains_old_and_adds_namespaced_new_history(
    tmp_path: Path,
) -> None:
    source, root_db, _ = _fixture(tmp_path)
    replacement = source / ".tg" / "replacement.db"
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    _task_database(replacement)
    connection = sqlite3.connect(root_db)
    try:
        connection.execute("UPDATE session_meta SET db_name = 'replacement'")
        connection.commit()
    finally:
        connection.close()

    before = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", first.sources
    )
    second = snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")
    after = load_orc_team(
        snapshot, ROOT, "orc-test", "America/New_York", second.sources
    )
    tasks = {item.source_path: item for item in second.sources if item.kind == "task"}

    assert tasks[".tg/project.db"].source_state == "detached"
    assert tasks[".tg/project.db"].task_source_ordinal == 0
    assert tasks[".tg/replacement.db"].source_state == "live"
    assert tasks[".tg/replacement.db"].task_source_ordinal == 1
    assert {event.event_id for event in before.events} <= {
        event.event_id for event in after.events
    }
    assert any("-s1-" in event.event_id for event in after.events)


def test_duplicate_and_changed_previous_source_records_are_rejected(
    tmp_path: Path,
) -> None:
    source, _, _ = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    with pytest.raises(OrcParseError, match="duplicate previous Orc source"):
        snapshot_orc_lineage(
            source, ROOT, snapshot, (*first.sources, first.sources[0]), "second"
        )
    session_source = next(item for item in first.sources if item.kind == "session")
    changed_kind = replace(session_source, kind="task")
    with pytest.raises(OrcParseError, match="source kind changed"):
        snapshot_orc_lineage(
            source,
            ROOT,
            snapshot,
            tuple(
                changed_kind if item == session_source else item
                for item in first.sources
            ),
            "second",
        )
    task_source = next(item for item in first.sources if item.kind == "task")
    changed_owner = replace(task_source, owner_session_id="unrelated-session")
    with pytest.raises(OrcParseError, match="source ownership changed"):
        snapshot_orc_lineage(
            source,
            ROOT,
            snapshot,
            tuple(
                changed_owner if item == task_source else item for item in first.sources
            ),
            "second",
        )


def test_session_prefix_collision_is_rejected_before_normalization(
    tmp_path: Path,
) -> None:
    source, _, _ = _fixture(tmp_path)
    collision = "11111111-aaaa-aaaa-aaaa-aaaaaaaaaaaa"
    _session_database(
        source / ".orc" / "sessions" / collision / "session.db",
        collision,
        parent_id=ROOT,
        db_name=None,
        messages=[],
        blocks=[],
    )
    _append_index_session(source / ".orc" / "index.db", collision, ROOT)
    snapshot = tmp_path / "snapshot"
    copied = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")

    with pytest.raises(OrcParseError, match="session-id prefix collision"):
        load_orc_team(snapshot, ROOT, "orc-test", "America/New_York", copied.sources)


def test_duplicate_agent_block_identity_is_rejected(tmp_path: Path) -> None:
    source, root_db, _ = _fixture(tmp_path)
    messages = _non_agent_rewrite_messages("duplicate follows")
    messages.append(
        {
            "id": 31,
            "role": "System",
            "created_at_ms": _ms("2026-07-21T18:00:00+00:00"),
            "blocks": [{"type": "AgentBlock", "id": 10, "agent_id": "other"}],
        }
    )
    _rewrite_conversation(root_db, messages)

    with pytest.raises(OrcParseError, match="duplicate AgentBlock identity"):
        snapshot_orc_lineage(source, ROOT, tmp_path / "snapshot", (), "first")


def _downgrade_orc_manifest_to_v1(path: Path) -> tuple[OrcSourceCopy, ...]:
    root = as_object(read_json(path), str(path))
    snapshot_root = path.parent.parent / "source_snapshots"
    legacy_sources: list[JsonValue] = []
    object_paths: set[Path] = set()
    for index, raw_source in enumerate(
        as_array(root.get("sources"), f"{path}: sources")
    ):
        source = as_object(raw_source, f"{path}: sources[{index}]")
        auxiliary = as_object(
            source.get("auxiliary"), f"{path}: sources[{index}].auxiliary"
        )
        legacy_source = dict(source)
        del legacy_source["auxiliary"]
        del legacy_source["task_projection"]
        del legacy_source["semantic_identity_mode"]
        del legacy_source["semantic_sha256"]
        del legacy_source["semantic_complete_bytes"]
        del legacy_source["semantic_baseline_path"]
        del legacy_source["source_state"]
        del legacy_source["task_source_ordinal"]
        legacy_source["auxiliary_count"] = as_int(
            auxiliary.get("message_count"),
            f"{path}: sources[{index}].auxiliary.message_count",
        )
        message_sha256 = auxiliary.get("message_sha256")
        if not isinstance(message_sha256, str):
            raise AssertionError("test fixture auxiliary digest is not a string")
        legacy_source["auxiliary_prefix_sha256"] = message_sha256
        source_path = legacy_source.get("source_path")
        snapshot_path = legacy_source.get("snapshot_path")
        if not isinstance(source_path, str) or not isinstance(snapshot_path, str):
            raise AssertionError("test fixture source paths are not strings")
        object_path = snapshot_root.joinpath(*Path(snapshot_path).parts)
        legacy_path = snapshot_root.joinpath(*Path(source_path).parts)
        legacy_path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(object_path, legacy_path)
        if legacy_source.get("kind") == "task":
            legacy_state = orc_module._logical_state(
                legacy_path, "task", legacy_task_fields=True
            )
            legacy_source["append_count"] = legacy_state.append_count
            legacy_source["append_max_id"] = legacy_state.append_max_id
            legacy_source["append_prefix_sha256"] = (
                legacy_state.append_prefix_sha256
            )
        object_paths.add(object_path)
        legacy_source["snapshot_path"] = source_path
        legacy_sources.append(legacy_source)
    legacy = dict(root)
    legacy["schema_version"] = 1
    legacy["sources"] = legacy_sources
    assert write_json_if_changed(path, legacy)
    for object_path in object_paths:
        object_path.unlink()
    return tuple(
        OrcSourceCopy.from_json_obj(
            as_object(item, f"{path}: sources[{index}]"),
            f"{path}: sources[{index}]",
            1,
        )
        for index, item in enumerate(legacy_sources)
    )


def test_pipeline_migrates_v1_manifest_across_auxiliary_rewrite_idempotently(
    tmp_path: Path,
) -> None:
    source, root_db, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    before, _ = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )
    manifest_path = (
        archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
    )
    _downgrade_orc_manifest_to_v1(manifest_path)
    _append_root_message(root_db, "root-appended", "Authoritative appended result")
    _rewrite_conversation(root_db, _rewritten_messages())

    after, migrated = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )
    manifest = as_object(read_json(manifest_path), str(manifest_path))
    manifest_sources = as_array(manifest.get("sources"), f"{manifest_path}: sources")
    root_source = next(
        as_object(item, f"{manifest_path}: source")
        for item in manifest_sources
        if as_object(item, f"{manifest_path}: source").get("kind") == "session"
    )
    auxiliary = as_object(
        root_source.get("auxiliary"), f"{manifest_path}: source.auxiliary"
    )

    assert manifest.get("schema_version") == 2
    assert auxiliary.get("policy") == "stable-spawn-subset-v1"
    assert auxiliary.get("rewrite_count") == 1
    assert auxiliary.get("degraded") is True
    assert {event.event_id for event in before.events} < {
        event.event_id for event in after.events
    }
    assert {agent.thread_id for agent in before.agents} < {
        agent.thread_id for agent in after.agents
    }
    assert migrated.files_changed > 0

    migrated_bytes = manifest_path.read_bytes()
    same, repeated = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )
    assert same == after
    assert repeated.files_changed == 0
    assert manifest_path.read_bytes() == migrated_bytes


def test_v1_unchanged_migration_preserves_semantics_digest_and_summary_cache(
    tmp_path: Path,
) -> None:
    source, _, task_db = _fixture(tmp_path)
    archive = tmp_path / "archive"
    initial, _ = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )
    manifest_path = (
        archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
    )
    legacy_sources = _downgrade_orc_manifest_to_v1(manifest_path)
    snapshot_root = archive / "teams" / "orc-test" / "source_snapshots"
    legacy_team = load_orc_team(
        snapshot_root, ROOT, "orc-test", "America/New_York", legacy_sources
    )
    before = replace(initial, sources=legacy_team.sources)
    raw_team_path = archive / "teams" / "orc-test" / "raw" / "team.json"
    assert write_json_if_changed(raw_team_path, narrow_json(before.to_json_obj()))
    (archive / "teams" / "orc-test" / "raw" / "artifacts.json").unlink()
    before_digest = _legacy_source_digest(before)
    assert source_digest(before) == before_digest
    before_paths = tuple(item.path for item in before.sources)
    summarize_archive(archive, "orc-test", "heuristic", "fixture")
    connection = sqlite3.connect(task_db)
    try:
        connection.execute(
            "UPDATE tasks SET owner = 'reassigned', title = 'Rewritten mutable title' "
            "WHERE local_id = 'task-a'"
        )
        connection.commit()
    finally:
        connection.close()

    after, _ = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )
    cached = summarize_archive(archive, "orc-test", "heuristic", "fixture")
    manifest = as_object(read_json(manifest_path), str(manifest_path))
    manifest_sources = [
        as_object(item, f"{manifest_path}: source")
        for item in as_array(manifest.get("sources"), f"{manifest_path}: sources")
    ]

    assert _semantic_team(after) == _semantic_team(before)
    assert tuple(item.path for item in after.sources) == before_paths
    assert all(path.startswith((".orc/", ".tg/")) for path in before_paths)
    assert source_digest(after) == before_digest
    assert manifest.get("schema_version") == 2
    assert all(
        item.get("source_path") != item.get("snapshot_path")
        for item in manifest_sources
    )
    task_source = next(item for item in manifest_sources if item.get("kind") == "task")
    task_projection = as_object(
        task_source.get("task_projection"), f"{manifest_path}: task_projection"
    )
    assert task_projection.get("rewrite_count") == 1
    assert cached.cache_misses == 0
    assert cached.cache_hits > 0


def test_non_agent_conversation_rewrite_preserves_semantics_and_summary_cache(
    tmp_path: Path,
) -> None:
    source, root_db, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    before, _ = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )
    summarize_archive(archive, "orc-test", "heuristic", "fixture")
    _rewrite_conversation(root_db, _non_agent_rewrite_messages("replacement"))
    connection = sqlite3.connect(root_db)
    try:
        connection.execute(
            "UPDATE session_meta SET updated_at = '2026-07-23T04:00:00+00:00'"
        )
        connection.commit()
    finally:
        connection.close()

    after, _ = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )
    cached = summarize_archive(archive, "orc-test", "heuristic", "fixture")

    assert _semantic_team(after) == _semantic_team(before)
    assert source_digest(after) == source_digest(before)
    assert tuple(item.path for item in after.sources) == tuple(
        item.path for item in before.sources
    )
    assert cached.cache_misses == 0
    assert cached.cache_hits > 0


def test_partial_object_publication_failure_keeps_manifest_and_retries(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source, root_db, task_db = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    manifest_path = (
        archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
    )
    prior_manifest = manifest_path.read_bytes()
    prior_root = _manifest_snapshot_database(archive, "session")
    prior_root_bytes = prior_root.read_bytes()
    _append_root_message(root_db, "root-appended", "publish candidate")
    _append_task_note(task_db, "publish second candidate")
    original_publish = orc_module._publish_snapshot_candidate
    new_targets = 0

    def fail_second_new_object(
        temporary: Path, target: Path, sha256: str, snapshot_root: Path
    ) -> bool:
        nonlocal new_targets
        if not target.exists():
            new_targets += 1
            if new_targets == 2:
                raise OSError("injected publication failure")
        return original_publish(temporary, target, sha256, snapshot_root)

    monkeypatch.setattr(
        orc_module, "_publish_snapshot_candidate", fail_second_new_object
    )
    with pytest.raises(OSError, match="injected publication failure"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    assert manifest_path.read_bytes() == prior_manifest
    assert prior_root.read_bytes() == prior_root_bytes
    assert len(_managed_snapshot_objects(archive)) > 3

    monkeypatch.setattr(orc_module, "_publish_snapshot_candidate", original_publish)
    after, retried = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )
    assert any(event.event_id == "orc-block-root-appended" for event in after.events)
    assert retried.files_changed > 0
    assert not prior_root.exists()
    assert len(_managed_snapshot_objects(archive)) == len(after.sources)


def test_task_projection_publication_failure_keeps_manifest_and_retries(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source, _, task_db = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    manifest_path = (
        archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
    )
    prior_manifest = manifest_path.read_bytes()
    prior_projection, _ = _manifest_task_projection(archive)
    prior_projection_bytes = prior_projection.read_bytes()
    _append_task_note(task_db, "projection publication candidate")
    original_publish = orc_module._publish_snapshot_candidate

    def fail_projection(
        temporary: Path, target: Path, sha256: str, snapshot_root: Path
    ) -> bool:
        if target.suffix == ".json" and not target.exists():
            raise OSError("injected projection publication failure")
        return original_publish(temporary, target, sha256, snapshot_root)

    monkeypatch.setattr(orc_module, "_publish_snapshot_candidate", fail_projection)
    with pytest.raises(OSError, match="injected projection publication failure"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    assert manifest_path.read_bytes() == prior_manifest
    assert prior_projection.read_bytes() == prior_projection_bytes

    monkeypatch.setattr(orc_module, "_publish_snapshot_candidate", original_publish)
    after, retried = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )
    current_projection, _ = _manifest_task_projection(archive)

    assert any(
        event.event_id == "orc-note-11111111-5" for event in after.events
    )
    assert retried.files_changed > 0
    assert current_projection != prior_projection
    assert not prior_projection.exists()
    assert _managed_task_projections(archive) == (current_projection,)


def test_task_projection_hash_corruption_fails_closed(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    projection, _ = _manifest_task_projection(archive)
    projection.write_text("{}\n", encoding="utf-8")

    with pytest.raises(OrcParseError, match="task projection hash mismatch"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")


def test_task_projection_symlink_fails_closed(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    projection, _ = _manifest_task_projection(archive)
    outside = tmp_path / "outside.json"
    outside.write_bytes(projection.read_bytes())
    projection.unlink()
    projection.symlink_to(outside)

    with pytest.raises(OrcParseError, match="task projection is missing or unsafe"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")


@pytest.mark.parametrize("component", (".orc", ".tg"))
def test_live_source_intermediate_symlink_fails_closed(
    tmp_path: Path, component: str
) -> None:
    source, _, _ = _fixture(tmp_path)
    original = source / component
    relocated = tmp_path / f"relocated-{component[1:]}"
    original.replace(relocated)
    original.symlink_to(relocated, target_is_directory=True)

    with pytest.raises(OrcParseError, match="symlink in Orc live source path"):
        snapshot_orc_lineage(source, ROOT, tmp_path / "snapshot", (), "first")


@pytest.mark.parametrize("artifact_kind", ("object", "projection"))
def test_snapshot_intermediate_symlink_fails_closed(
    tmp_path: Path, artifact_kind: str
) -> None:
    source, _, _ = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    copied = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    if artifact_kind == "object":
        source_copy = next(item for item in copied.sources if item.kind == "session")
        artifact = _snapshot_database(snapshot, source_copy)
    else:
        source_copy = next(item for item in copied.sources if item.kind == "task")
        assert source_copy.task_projection is not None
        artifact = snapshot.joinpath(
            *Path(source_copy.task_projection.path).parts
        )
    prefix = artifact.parent
    relocated = tmp_path / f"relocated-{artifact_kind}-prefix"
    prefix.replace(relocated)
    prefix.symlink_to(relocated, target_is_directory=True)

    with pytest.raises(OrcParseError, match="snapshot path component"):
        load_orc_team(
            snapshot, ROOT, "orc-test", "America/New_York", copied.sources
        )


def test_noncanonical_task_projection_fails_closed(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    manifest_path = (
        archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
    )
    root = as_object(read_json(manifest_path), str(manifest_path))
    task = next(
        as_object(item, f"{manifest_path}: source")
        for item in as_array(root.get("sources"), f"{manifest_path}: sources")
        if as_object(item, f"{manifest_path}: source").get("kind") == "task"
    )
    projection = as_object(
        task.get("task_projection"), f"{manifest_path}: task_projection"
    )
    prior_path, _ = _manifest_task_projection(archive)
    value = as_object(json.loads(prior_path.read_text(encoding="utf-8")), "projection")
    records = as_array(value.get("records"), "projection.records")
    records.append(records[-1])
    text = json.dumps(value, sort_keys=True, separators=(",", ":")) + "\n"
    sha256 = hashlib.sha256(text.encode("utf-8")).hexdigest()
    relative = f".projections/{sha256[:2]}/{sha256}.json"
    replacement = (
        archive / "teams" / "orc-test" / "source_snapshots" / relative
    )
    replacement.parent.mkdir(parents=True, exist_ok=True)
    replacement.write_text(text, encoding="utf-8")
    projection["path"] = relative
    projection["sha256"] = sha256
    projection["note_count"] = len(records)
    assert write_json_if_changed(manifest_path, root)

    with pytest.raises(OrcParseError, match="note IDs are not strictly ordered"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")


def test_directory_fsync_failure_leaves_reusable_orphan(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    source, root_db, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    manifest_path = (
        archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
    )
    prior_manifest = manifest_path.read_bytes()
    _append_root_message(root_db, "root-appended", "fsync candidate")
    original_fsync_directory = orc_module._fsync_directory
    failed = False

    def fail_object_directory(path: Path) -> None:
        nonlocal failed
        if not failed and path.parent.name == ".objects":
            failed = True
            raise OSError("injected object-directory fsync failure")
        original_fsync_directory(path)

    monkeypatch.setattr(orc_module, "_fsync_directory", fail_object_directory)
    with pytest.raises(OSError, match="injected object-directory fsync failure"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    assert manifest_path.read_bytes() == prior_manifest
    assert len(_managed_snapshot_objects(archive)) > 3

    monkeypatch.setattr(orc_module, "_fsync_directory", original_fsync_directory)
    after, _ = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )
    assert any(event.event_id == "orc-block-root-appended" for event in after.events)
    assert len(_managed_snapshot_objects(archive)) == len(after.sources)


def test_preexisting_content_address_must_match_its_hash(tmp_path: Path) -> None:
    source, root_db, _ = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    first_root = next(item for item in first.sources if item.kind == "session")
    first_root_path = _snapshot_database(snapshot, first_root)
    first_root_bytes = first_root_path.read_bytes()
    _append_root_message(root_db, "root-appended", "new immutable object")
    second = snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")
    second_root = next(item for item in second.sources if item.kind == "session")
    second_root_path = _snapshot_database(snapshot, second_root)
    second_root_path.write_bytes(b"corrupt preexisting object")

    with pytest.raises(OrcParseError, match="preexisting snapshot object has the wrong hash"):
        snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "retry")
    assert first_root_path.read_bytes() == first_root_bytes


def test_managed_object_gc_rejects_symlinks(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    prefix = (
        archive
        / "teams"
        / "orc-test"
        / "source_snapshots"
        / ".objects"
        / "ff"
    )
    prefix.mkdir()
    unsafe = prefix / ("f" * 64 + ".db")
    unsafe.symlink_to(tmp_path / "outside.db")

    with pytest.raises(OrcParseError, match="symlink in Orc snapshot object store"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")


def test_post_snapshot_parse_failure_keeps_old_manifest_and_raw_team(
    tmp_path: Path,
) -> None:
    source, root_db, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    manifest_path = (
        archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
    )
    raw_team_path = archive / "teams" / "orc-test" / "raw" / "team.json"
    prior_manifest = manifest_path.read_bytes()
    prior_raw_team = raw_team_path.read_bytes()
    prior_root = _manifest_snapshot_database(archive, "session")
    prior_root_bytes = prior_root.read_bytes()
    _append_root_message(root_db, "malformed", "candidate parse must fail")
    connection = sqlite3.connect(root_db)
    try:
        connection.execute(
            "UPDATE content_blocks SET role = ? WHERE id = 'malformed'",
            (sqlite3.Binary(b"invalid-role"),),
        )
        connection.commit()
    finally:
        connection.close()

    with pytest.raises(OrcParseError, match="role: expected a non-empty string"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    assert manifest_path.read_bytes() == prior_manifest
    assert raw_team_path.read_bytes() == prior_raw_team
    assert prior_root.read_bytes() == prior_root_bytes

    connection = sqlite3.connect(root_db)
    try:
        connection.execute(
            "UPDATE content_blocks SET role = 'assistant' WHERE id = 'malformed'"
        )
        connection.commit()
    finally:
        connection.close()
    after, _ = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )
    assert any(event.event_id == "orc-block-malformed" for event in after.events)
    assert not prior_root.exists()
    assert len(_managed_snapshot_objects(archive)) == len(after.sources)


def test_malformed_v2_projection_is_rejected_before_snapshot(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    manifest_path = (
        archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
    )
    root = as_object(read_json(manifest_path), str(manifest_path))
    sources = as_array(root.get("sources"), f"{manifest_path}: sources")
    task = next(
        as_object(item, f"{manifest_path}: source")
        for item in sources
        if as_object(item, f"{manifest_path}: source").get("kind") == "task"
    )
    task["task_projection"] = None
    assert write_json_if_changed(manifest_path, root)

    with pytest.raises(OrcParseError, match="invalid schema-v2 projections"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")


def test_v2_manifest_rejects_unknown_top_level_field(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    manifest_path = (
        archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
    )
    manifest = as_object(read_json(manifest_path), str(manifest_path))
    manifest["unexpected"] = True
    assert write_json_if_changed(manifest_path, manifest)

    with pytest.raises(OrcParseError, match="unknown=.*unexpected"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")


def test_v2_manifest_rejects_missing_nested_field(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    manifest_path = (
        archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
    )
    manifest = as_object(read_json(manifest_path), str(manifest_path))
    session = next(
        as_object(item, "manifest.source")
        for item in as_array(manifest.get("sources"), "manifest.sources")
        if as_object(item, "manifest.source").get("kind") == "session"
    )
    auxiliary = as_object(session.get("auxiliary"), "manifest.source.auxiliary")
    del auxiliary["message_sha256"]
    assert write_json_if_changed(manifest_path, manifest)

    with pytest.raises(OrcParseError, match="missing message_sha256"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")


def test_v2_manifest_rejects_invalid_observed_enrichment_digest(
    tmp_path: Path,
) -> None:
    source, _, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    manifest_path = (
        archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
    )
    manifest = as_object(read_json(manifest_path), str(manifest_path))
    task = next(
        as_object(item, "manifest.source")
        for item in as_array(manifest.get("sources"), "manifest.sources")
        if as_object(item, "manifest.source").get("kind") == "task"
    )
    projection = as_object(
        task.get("task_projection"), "manifest.task.task_projection"
    )
    projection["observed_enrichment_sha256"] = "f" * 63 + "z"
    assert write_json_if_changed(manifest_path, manifest)

    with pytest.raises(OrcParseError, match="expected a SHA-256 digest"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")


def test_v1_migration_failure_preserves_snapshot_and_manifest(tmp_path: Path) -> None:
    source, root_db, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    manifest_path = (
        archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
    )
    _downgrade_orc_manifest_to_v1(manifest_path)
    snapshot_db = _manifest_snapshot_database(archive, "session")
    prior_manifest = manifest_path.read_bytes()
    prior_snapshot = snapshot_db.read_bytes()
    _append_root_message(root_db, "root-appended", "Must not be ingested")
    _rewrite_conversation(root_db, _rewritten_messages(first_agent="renamed-worker"))

    with pytest.raises(OrcParseError, match="stable spawn evidence was rewritten"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    assert manifest_path.read_bytes() == prior_manifest
    assert snapshot_db.read_bytes() == prior_snapshot


def test_unknown_orc_manifest_schema_is_rejected_before_snapshot(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    manifest_path = (
        archive / "teams" / "orc-test" / "raw" / "source-manifest.json"
    )
    root = as_object(read_json(manifest_path), str(manifest_path))
    snapshot_db = _manifest_snapshot_database(archive, "session")
    root["schema_version"] = 99
    assert write_json_if_changed(manifest_path, root)
    prior_snapshot = snapshot_db.read_bytes()

    with pytest.raises(OrcParseError, match="invalid Orc source manifest"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    assert snapshot_db.read_bytes() == prior_snapshot


@pytest.mark.parametrize(
    "failure_name", ("source-manifest.json", "artifacts.json", "team.json")
)
def test_normalized_generation_marker_fails_closed_and_recovers(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    failure_name: str,
) -> None:
    source, root_db, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    _append_root_message(root_db, "generation-two", "new committed generation")
    original_write = pipeline_module._write_json_durable

    def fail_after_write(path: Path, value: JsonValue) -> bool:
        changed = original_write(path, value)
        if path.name == failure_name:
            raise OSError(f"injected failure after {failure_name}")
        return changed

    monkeypatch.setattr(pipeline_module, "_write_json_durable", fail_after_write)
    with pytest.raises(OSError, match="injected failure"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    with pytest.raises(ValueError, match="normalized generation"):
        load_archived_team(archive, "orc-test")

    monkeypatch.setattr(
        pipeline_module, "_write_json_durable", original_write
    )
    repaired, _ = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )
    assert load_archived_team(archive, "orc-test") == repaired


def test_stale_managed_staging_candidate_is_pruned(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    staging = archive / "teams" / "orc-test" / "source_snapshots" / ".staging"
    stale = staging / "orc-123-0123456789abcdef.db"
    stale.write_bytes(b"interrupted candidate")

    _, report = ingest_orc(
        archive, source, ROOT, "orc-test", "America/New_York"
    )

    assert not stale.exists()
    assert report.files_changed == 1


@pytest.mark.parametrize("unsafe_kind", ("symlink", "directory"))
def test_unsafe_staging_entry_is_rejected(
    tmp_path: Path, unsafe_kind: str
) -> None:
    source, _, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")
    staging = archive / "teams" / "orc-test" / "source_snapshots" / ".staging"
    unsafe = staging / "unsafe"
    if unsafe_kind == "symlink":
        unsafe.symlink_to(tmp_path / "outside")
    else:
        unsafe.mkdir()

    with pytest.raises(OrcParseError, match="unsafe entry in Orc staging"):
        ingest_orc(archive, source, ROOT, "orc-test", "America/New_York")


def test_orc_pipeline_builds_one_day_archive_idempotently(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    window = parse_date_window("2026-07-21", "2026-07-22", "America/New_York")
    assert window is not None
    assert window.start_ms is not None
    assert window.end_ms is not None

    team, first = ingest_orc(
        archive,
        source,
        ROOT,
        "orc-test",
        "America/New_York",
        window,
    )
    _, second = ingest_orc(
        archive,
        source,
        ROOT,
        "orc-test",
        "America/New_York",
        window,
    )
    summaries = summarize_archive(archive, "orc-test", "heuristic", "fixture")
    built = build_archive(archive, "orc-test")
    timeline = json.loads(
        (archive / "data" / "timeline.json").read_text(encoding="utf-8")
    )

    assert team.provider == "orc"
    assert first.sources == 3
    assert second.files_changed == 0
    assert summaries.cache_misses > 0
    assert built["agents"] == 4
    assert timeline["range"] == {
        "start_ms": window.start_ms,
        "end_ms": window.end_ms,
    }
    assert len(timeline["rollups"]) == 4
    message_edges = [edge for edge in timeline["edges"] if edge["kind"] == "message"]
    assert len(message_edges) == 2
    assert {
        (edge["source_id"], edge["target_id"])
        for edge in message_edges
    } == {
        (event.thread_id, ROOT)
        for event in team.events
        if event.kind == "inter_agent_message"
        and window.start_ms <= event.timestamp_ms < window.end_ms
    }
    assert [event["kind"] for event in timeline["events"]].count(
        "inter_agent_message"
    ) == 2
    manifest = json.loads(
        (archive / "teams" / "orc-test" / "raw" / "source-manifest.json").read_text(
            encoding="utf-8"
        )
    )
    assert manifest["provider"] == "orc"
    assert manifest["schema_version"] == 2
    assert manifest["date_window"]["end_date"] == "2026-07-22"
