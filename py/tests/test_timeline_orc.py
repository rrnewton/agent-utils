from __future__ import annotations

import json
import sqlite3
import stat
from collections.abc import Sequence
from datetime import datetime, timezone
from pathlib import Path

import pytest

from agent_team_timeline.orc import (
    OrcParseError,
    load_orc_team,
    snapshot_orc_lineage,
)
from agent_team_timeline.phases import build_phases
from agent_team_timeline.pipeline import (
    build_archive,
    ingest_orc,
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
    tools = team.tool_calls
    assert len(tools) == 1
    assert tools[0].nested_tools == (("readFile", 2), ("sendAgent", 1))


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


def test_truncated_task_notes_are_rejected_and_snapshot_is_preserved(
    tmp_path: Path,
) -> None:
    source, _, task_db = _fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ROOT, snapshot, (), "first")
    snapshot_task = snapshot / ".tg" / "project.db"
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

    with pytest.raises(OrcParseError, match="task database.*missing"):
        snapshot_orc_lineage(source, ROOT, snapshot, first.sources, "second")


def test_orc_pipeline_builds_one_day_archive_idempotently(tmp_path: Path) -> None:
    source, _, _ = _fixture(tmp_path)
    archive = tmp_path / "archive"
    window = parse_date_window("2026-07-21", "2026-07-22", "America/New_York")
    assert window is not None

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
    manifest = json.loads(
        (archive / "teams" / "orc-test" / "raw" / "source-manifest.json").read_text(
            encoding="utf-8"
        )
    )
    assert manifest["provider"] == "orc"
    assert manifest["date_window"]["end_date"] == "2026-07-22"
