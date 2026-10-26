"""Append-only source-backup tests for the Codex timeline ingest."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import shutil
import subprocess
import threading

import pytest

import wrkviz.codex as codex_module
import wrkviz.pipeline as pipeline_module
from wrkviz.build_store import team_build_root
from wrkviz.cli import main as timeline_main
from wrkviz.codex import (
    CodexParseError,
    CodexSnapshotResult,
    CodexSourceCopy,
    load_codex_team,
    snapshot_codex_lineage,
)
from wrkviz.claude import ClaudeParseError, snapshot_claude_lineage
from wrkviz.compressed_sources import (
    compressed_sibling,
    iter_logical_glob,
    logical_path,
    materialize_source,
    read_first_line,
    read_source_bytes,
    resolve_source,
)
from wrkviz.model import TeamData
from wrkviz.muse import MuseParseError
from wrkviz.orc import OrcParseError, snapshot_orc_lineage
from wrkviz.pipeline import IngestReport, ingest_codex, ingest_muse, ingest_orc
from wrkviz.phases import build_phases
from wrkviz.pipeline import _phase_jobs
from wrkviz.summarize import _input_hash
from tests.test_timeline_claude import FIXTURE_ROOT, _session
from tests.test_timeline_orc import ROOT as ORC_ROOT
from tests.test_timeline_orc import _fixture as _orc_fixture
from tests.timeline_snapshots import snapshot_root


ROOT = "root-thread"
CHILD = "child-thread"
CONTINUATION = "continuation-thread"
CONTINUATION_CHILD = "continuation-child"
CONTINUATION_TWO = "continuation-thread-two"


def _iso(seconds: float) -> str:
    return datetime.fromtimestamp(seconds, timezone.utc).isoformat().replace("+00:00", "Z")


def _line(timestamp: float, kind: str, payload: dict[str, object]) -> bytes:
    record: dict[str, object] = {
        "timestamp": _iso(timestamp),
        "type": kind,
        "payload": payload,
    }
    return json.dumps(record, sort_keys=True, separators=(",", ":")).encode("utf-8") + b"\n"


def _root_bytes(*, incomplete_tail: bytes = b"") -> bytes:
    records = (
        _line(
            1_000,
            "session_meta",
            {
                "id": ROOT,
                "session_id": ROOT,
                "timestamp": _iso(1_000),
                "cwd": "/work/project",
                "git": {
                    "repository_url": "git@github.com:example-org/dev-widget.git"
                },
                "source": "cli",
            },
        ),
        _line(
            1_001,
            "event_msg",
            {"type": "task_started", "turn_id": "root-turn", "started_at": 1_001},
        ),
        _line(
            1_002,
            "response_item",
            {
                "type": "message",
                "id": "root-answer",
                "role": "assistant",
                "content": [{"type": "output_text", "text": "Root done"}],
            },
        ),
        _line(
            1_002.1,
            "response_item",
            {
                "type": "function_call",
                "id": "shared-tool-item",
                "call_id": "shared-call",
                "name": "exec",
                "arguments": "{}",
            },
        ),
        _line(
            1_002.2,
            "response_item",
            {
                "type": "function_call_output",
                "call_id": "shared-call",
                "output": "canonical output",
            },
        ),
        _line(
            1_003,
            "event_msg",
            {
                "type": "task_complete",
                "turn_id": "root-turn",
                "started_at": 1_001,
                "completed_at": 1_003,
            },
        ),
    )
    return b"".join(records) + incomplete_tail


def _child_bytes() -> bytes:
    return b"".join(
        (
            _line(
                1_010,
                "session_meta",
                {
                    "id": CHILD,
                    "session_id": ROOT,
                    "parent_thread_id": ROOT,
                    "timestamp": _iso(1_010),
                    "agent_path": "/root/worker",
                    "source": {
                        "subagent": {
                            "thread_spawn": {
                                "parent_thread_id": ROOT,
                                "agent_path": "/root/worker",
                                "depth": 1,
                            }
                        }
                    },
                },
            ),
            _line(
                1_011,
                "event_msg",
                {"type": "task_started", "turn_id": "child-turn", "started_at": 1_011},
            ),
            _line(
                1_012,
                "response_item",
                {
                    "type": "message",
                    "id": "child-answer",
                    "role": "assistant",
                    "phase": "final_answer",
                    "content": [{"type": "output_text", "text": "Child done"}],
                },
            ),
            _line(
                1_013,
                "event_msg",
                {
                    "type": "task_complete",
                    "turn_id": "child-turn",
                    "started_at": 1_011,
                    "completed_at": 1_013,
                },
            ),
        )
    )


def _continuation_root_bytes() -> bytes:
    return b"".join(
        (
            _line(
                1_005.263,
                "session_meta",
                {
                    "id": CONTINUATION,
                    "session_id": CONTINUATION,
                    "timestamp": _iso(1_005.263),
                    "cwd": "/work/project",
                    "source": "cli",
                    "thread_source": "user",
                },
            ),
            _line(
                1_006,
                "event_msg",
                {"type": "task_started", "turn_id": "root-turn", "started_at": 1_006},
            ),
            _line(
                1_006.1,
                "response_item",
                {
                    "type": "message",
                    "id": "root-answer",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "Continuation work"}],
                },
            ),
            _line(
                1_006.11,
                "response_item",
                {
                    "type": "function_call",
                    "id": "shared-tool-item",
                    "call_id": "shared-call",
                    "name": "exec",
                    "arguments": "{}",
                },
            ),
            _line(
                1_006.12,
                "response_item",
                {
                    "type": "function_call_output",
                    "call_id": "shared-call",
                    "output": "continuation output",
                },
            ),
            _line(
                1_006.2,
                "response_item",
                {
                    "type": "function_call",
                    "id": "spawn-item",
                    "call_id": "call-continuation-spawn",
                    "namespace": "collaboration",
                    "name": "spawn_agent",
                    "arguments": json.dumps(
                        {"task_name": "worker", "message": "continue child"}
                    ),
                },
            ),
            _line(
                1_006.3,
                "event_msg",
                {
                    "type": "sub_agent_activity",
                    "event_id": "call-continuation-spawn",
                    "occurred_at_ms": 1_006_300,
                    "agent_thread_id": CONTINUATION_CHILD,
                    "agent_path": "/root/worker",
                    "kind": "started",
                },
            ),
            _line(
                1_007,
                "event_msg",
                {
                    "type": "task_complete",
                    "turn_id": "root-turn",
                    "started_at": 1_006,
                    "completed_at": 1_007,
                },
            ),
        )
    )


def _continuation_child_bytes() -> bytes:
    return b"".join(
        (
            _line(
                1_006.5,
                "session_meta",
                {
                    "id": CONTINUATION_CHILD,
                    "session_id": CONTINUATION,
                    "parent_thread_id": CONTINUATION,
                    "timestamp": _iso(1_006.5),
                    "agent_path": "/root/worker",
                    "source": {
                        "subagent": {
                            "thread_spawn": {
                                "parent_thread_id": CONTINUATION,
                                "agent_path": "/root/worker",
                                "depth": 1,
                            }
                        }
                    },
                },
            ),
            _line(
                1_006.6,
                "event_msg",
                {"type": "task_started", "turn_id": "child-turn", "started_at": 1_006.6},
            ),
            _line(
                1_006.7,
                "response_item",
                {
                    "type": "message",
                    "id": "child-answer",
                    "role": "assistant",
                    "content": [{"type": "output_text", "text": "Continuation child"}],
                },
            ),
        )
    )


def _second_continuation_bytes() -> bytes:
    return b"".join(
        (
            _line(
                1_009.5,
                "session_meta",
                {
                    "id": CONTINUATION_TWO,
                    "session_id": CONTINUATION_TWO,
                    "timestamp": _iso(1_009.5),
                    "cwd": "/work/project",
                    "source": "cli",
                    "thread_source": "user",
                },
            ),
            _line(
                1_010,
                "event_msg",
                {"type": "task_started", "turn_id": "root-turn", "started_at": 1_010},
            ),
            _line(
                1_011,
                "response_item",
                {
                    "type": "message",
                    "id": "root-answer",
                    "role": "assistant",
                    "phase": "final_answer",
                    "content": [{"type": "output_text", "text": "Second continuation"}],
                },
            ),
            _line(
                1_012,
                "event_msg",
                {
                    "type": "task_complete",
                    "turn_id": "root-turn",
                    "started_at": 1_010,
                    "completed_at": 1_012,
                },
            ),
        )
    )


def _write_continuation_sources(sessions: Path) -> tuple[Path, Path]:
    root = sessions / "2026" / "08" / "05" / "rollout-continuation.jsonl"
    child = sessions / "2026" / "08" / "05" / "rollout-continuation-child.jsonl"
    root.write_bytes(_continuation_root_bytes())
    child.write_bytes(_continuation_child_bytes())
    return root, child


def _write_second_continuation_source(sessions: Path) -> Path:
    path = sessions / "2026" / "08" / "05" / "rollout-continuation-two.jsonl"
    path.write_bytes(_second_continuation_bytes())
    return path


def _origin(sessions: Path) -> Path:
    return sessions / "2026" / "08" / "05" / "rollout-root.jsonl"


def _snapshot(archive: Path) -> Path:
    return (
        snapshot_root(archive, "codex-test")
        / "2026"
        / "08"
        / "05"
        / "rollout-root.jsonl"
    )


def _manifest(archive: Path) -> Path:
    return team_build_root(archive, "codex-test") / "raw" / "source-manifest.json"


def _first_ingest(tmp_path: Path, data: bytes | None = None) -> tuple[Path, Path, Path]:
    sessions = tmp_path / "sessions"
    archive = tmp_path / "archive"
    origin = _origin(sessions)
    origin.parent.mkdir(parents=True)
    origin.write_bytes(_root_bytes() if data is None else data)
    ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")
    return sessions, archive, origin


def test_ingest_copies_complete_lines_then_parses_the_backup(tmp_path: Path) -> None:
    incomplete = b'{"timestamp":"unfinished"'
    complete = _root_bytes()
    sessions, archive, _ = _first_ingest(tmp_path, complete + incomplete)

    copied = _snapshot(archive)
    assert copied.read_bytes() == complete
    manifest = json.loads(_manifest(archive).read_text(encoding="utf-8"))
    source = manifest["sources"][0]
    assert source["copied_bytes"] == len(complete)
    assert source["sha256"] == hashlib.sha256(complete).hexdigest()
    assert source["line_count"] == complete.count(b"\n")
    assert "/teams/*/source_snapshots/" in (
        archive / ".gitignore"
    ).read_text(encoding="utf-8").splitlines()

    # The normalized parser has everything it needs in the copy, independent of the live root.
    parsed = load_codex_team(
        snapshot_root(archive, "codex-test"),
        ROOT,
        "codex-test",
        "UTC",
    )
    assert [event.text for event in parsed.events] == ["Root done"]
    assert manifest["source_root"] == str(sessions.resolve())
    identity = json.loads(
        (
            team_build_root(archive, "codex-test")
            / "raw"
            / "site-identity.json"
        ).read_text(encoding="utf-8")
    )
    assert identity["projects"] == [
        {
            "label": "dev-widget",
            "repository_url": "https://github.com/example-org/dev-widget",
            "primary": True,
            "source": "session_metadata",
        }
    ]
    assert identity["display_timezone"] == "UTC"
    assert identity["display_timezone_source"] == "api"

    _, repeat = ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")
    assert repeat.files_changed == 0


def test_append_replaces_snapshot_with_longer_complete_prefix(tmp_path: Path) -> None:
    sessions, archive, origin = _first_ingest(tmp_path)
    before = _snapshot(archive).read_bytes()
    appended = _line(
        1_004,
        "event_msg",
        {"type": "thread_goal_updated", "goal": {"status": "complete"}},
    )
    origin.write_bytes(before + appended + b'{"partial":')

    _, report = ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")

    expected = before + appended
    assert _snapshot(archive).read_bytes() == expected
    manifest = json.loads(_manifest(archive).read_text(encoding="utf-8"))
    assert manifest["sources"][0]["copied_bytes"] == len(expected)
    assert manifest["sources"][0]["sha256"] == hashlib.sha256(expected).hexdigest()
    assert report.source_bytes == len(expected)


def test_real_ingest_writes_no_per_thread_message_projection(tmp_path: Path) -> None:
    _, archive, _ = _first_ingest(tmp_path)

    # Nothing anywhere in the archive, not just under the one team: this is the check that catches
    # a second writer being added later under a different team root.
    assert [path for path in archive.rglob("messages") if path.is_dir()] == []
    raw = team_build_root(archive, "codex-test") / "raw"
    assert sorted(path.name for path in raw.iterdir()) == [
        "artifacts.json",
        "site-identity.json",
        "source-manifest.json",
        "source-snapshot.json",
        "team.json",
    ]


def test_real_ingest_retires_a_legacy_projection_and_says_so_on_stderr(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    sessions, archive, _ = _first_ingest(tmp_path)
    legacy = team_build_root(archive, "codex-test") / "raw" / "messages"
    legacy.mkdir()
    payload = json.dumps({"agent": {}, "turns": [], "messages": []}) + "\n"
    for thread_id in (ROOT, CHILD):
        (legacy / f"{thread_id}.json").write_text(payload, encoding="utf-8")
    freed = 2 * len(payload.encode("utf-8"))
    capsys.readouterr()

    status = timeline_main(
        (
            "ingest",
            "--sessions-root",
            str(sessions),
            "--root-session",
            ROOT,
            "--team",
            "codex-test",
            "--output",
            str(archive),
            "--timezone",
            "UTC",
        )
    )

    assert status == 0
    assert not legacy.exists()
    captured = capsys.readouterr()
    # On stderr, not stdout: a scheduled run redirects stdout, and this is the operator's only
    # live notice that a few thousand tracked files just left their archive.
    assert "retired" in captured.err and "raw/messages" in captured.err
    assert "removed 2 retired" in captured.err
    assert "raw/messages" not in captured.out

    run_path = sorted((archive / "runs").glob("*.json"))[-1]
    run = json.loads(run_path.read_text(encoding="utf-8"))
    assert run["ingest"]["retired_message_projections"] == 2
    assert run["ingest"]["retired_message_projection_bytes"] == freed

    # Second run: nothing left to sweep, and nothing said about it.
    capsys.readouterr()
    assert (
        timeline_main(
            (
                "ingest",
                "--sessions-root",
                str(sessions),
                "--root-session",
                ROOT,
                "--team",
                "codex-test",
                "--output",
                str(archive),
                "--timezone",
                "UTC",
            )
        )
        == 0
    )
    assert "raw/messages" not in capsys.readouterr().err


def test_disappeared_source_is_retired_and_preserves_snapshot(tmp_path: Path) -> None:
    # Upstream retention is expected; tests/test_timeline_upstream_retention.py covers the rest.
    sessions, archive, origin = _first_ingest(tmp_path)
    copied_before = _snapshot(archive).read_bytes()
    origin.unlink()

    _, report = ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")

    assert report.upstream_retired_sources == (origin.relative_to(sessions).as_posix(),)
    assert _snapshot(archive).read_bytes() == copied_before


def test_truncated_source_fails_and_preserves_snapshot(tmp_path: Path) -> None:
    sessions, archive, origin = _first_ingest(tmp_path)
    copied_before = _snapshot(archive).read_bytes()
    manifest_before = _manifest(archive).read_bytes()
    final_line = copied_before.rsplit(b"\n", 2)[1] + b"\n"
    origin.write_bytes(copied_before[: -len(final_line)])

    with pytest.raises(CodexParseError, match="newline-complete prefix shrank"):
        ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")

    assert _snapshot(archive).read_bytes() == copied_before
    assert _manifest(archive).read_bytes() == manifest_before


def test_rewritten_same_length_prefix_fails_and_preserves_snapshot(tmp_path: Path) -> None:
    sessions, archive, origin = _first_ingest(tmp_path)
    copied_before = _snapshot(archive).read_bytes()
    manifest_before = _manifest(archive).read_bytes()
    rewritten = copied_before.replace(b"Root done", b"Root gone")
    assert len(rewritten) == len(copied_before)
    origin.write_bytes(rewritten)

    with pytest.raises(CodexParseError, match="existing prefix was rewritten"):
        ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")

    assert _snapshot(archive).read_bytes() == copied_before
    assert _manifest(archive).read_bytes() == manifest_before


def test_new_lineage_child_is_added_without_recopying_root(tmp_path: Path) -> None:
    sessions, archive, _ = _first_ingest(tmp_path)
    root_snapshot = _snapshot(archive)
    root_mtime = root_snapshot.stat().st_mtime_ns
    child = sessions / "2026" / "08" / "05" / "rollout-child.jsonl"
    child.write_bytes(_child_bytes())

    team, _ = ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")

    assert {agent.thread_id for agent in team.agents} == {ROOT, CHILD}
    assert root_snapshot.stat().st_mtime_ns == root_mtime
    manifest = json.loads(_manifest(archive).read_text(encoding="utf-8"))
    assert {source["thread_id"] for source in manifest["sources"]} == {ROOT, CHILD}


def test_explicit_continuation_preserves_old_ids_hashes_and_is_idempotent(
    tmp_path: Path,
) -> None:
    sessions, archive, _ = _first_ingest(tmp_path)
    before_team, before_report = ingest_codex(
        archive, sessions, ROOT, "codex-test", "UTC"
    )
    assert before_report.files_changed == 0
    before_snapshot = _snapshot(archive)
    before_snapshot_bytes = before_snapshot.read_bytes()
    before_snapshot_mtime = before_snapshot.stat().st_mtime_ns
    before_agents = {agent.thread_id: agent for agent in before_team.agents}
    before_phases = build_phases(before_team)
    before_jobs = {
        job.key: job for job in _phase_jobs(before_team, before_phases, (), 16_000)
    }
    before_hashes = {
        key: _input_hash(job, "heuristic", "fixture")
        for key, job in before_jobs.items()
    }
    _write_continuation_sources(sessions)

    after, _ = ingest_codex(
        archive,
        sessions,
        ROOT,
        "codex-test",
        "UTC",
        continuation_thread_ids=(CONTINUATION,),
    )
    manifest_bytes = _manifest(archive).read_bytes()
    manifest = json.loads(manifest_bytes)
    agents = {agent.thread_id: agent for agent in after.agents}
    edges = {edge.edge_id: edge for edge in after.edges}

    assert after.root_thread_id == ROOT
    assert before_snapshot.read_bytes() == before_snapshot_bytes
    assert before_snapshot.stat().st_mtime_ns == before_snapshot_mtime
    assert {source["thread_id"] for source in manifest["sources"]} == {
        ROOT,
        CONTINUATION,
        CONTINUATION_CHILD,
    }
    assert manifest["schema_version"] == 1
    assert manifest["continuation_sessions"] == [
        {
            "predecessor_thread_id": ROOT,
            "thread_id": CONTINUATION,
            "predecessor_source_path": "2026/08/05/rollout-root.jsonl",
            "predecessor_source_line": 6,
            "predecessor_at_ms": 1_003_000,
            "source_path": "2026/08/05/rollout-continuation.jsonl",
            "started_at_ms": 1_005_263,
            "gap_ms": 2_263,
        }
    ]
    assert agents[ROOT] == before_agents[ROOT]
    assert agents[CONTINUATION].parent_thread_id == ROOT
    assert agents[CONTINUATION].depth == 1
    assert agents[CONTINUATION].role == "coordinator"
    assert agents[CONTINUATION].agent_path == (
        f"/root/continuation-{CONTINUATION}"
    )
    assert agents[CONTINUATION_CHILD].parent_thread_id == CONTINUATION
    assert agents[CONTINUATION_CHILD].depth == 2
    assert agents[CONTINUATION_CHILD].agent_path == (
        f"/root/continuation-{CONTINUATION}/worker"
    )
    continuation_edge = edges[f"codex-continuation-{CONTINUATION}"]
    assert (
        continuation_edge.from_thread_id,
        continuation_edge.to_thread_id,
        continuation_edge.kind,
        continuation_edge.timestamp_ms,
    ) == (ROOT, CONTINUATION, "continuation", 1_003_000)
    assert continuation_edge.source_line == 6
    assert "2263 ms later" in (continuation_edge.message_text or "")
    assert "root-answer" in {event.event_id for event in after.events}
    scoped_answer = codex_module._scoped_id(
        CONTINUATION, ROOT, "root-answer"
    )
    assert (
        scoped_answer
        in {event.event_id for event in after.events}
    )
    assert "root-turn" in {turn.turn_id for turn in after.turns}
    scoped_turn = codex_module._scoped_id(CONTINUATION, ROOT, "root-turn")
    assert (
        scoped_turn
        in {turn.turn_id for turn in after.turns}
    )
    assert {"shared-call", codex_module._scoped_id(CONTINUATION, ROOT, "shared-call")} <= {
        tool.call_id for tool in after.tool_calls
    }
    assert codex_module._scoped_id("a-b", ROOT, "c") != codex_module._scoped_id(
        "a", ROOT, "b-c"
    )

    after_phases = build_phases(after)
    after_jobs = {
        job.key: job
        for job in _phase_jobs(after, after_phases, (), 16_000)
        if job.end_ms < 1_005_263
    }
    assert set(after_jobs) == set(before_jobs)
    assert after_jobs == before_jobs
    assert {
        key: _input_hash(job, "heuristic", "fixture")
        for key, job in after_jobs.items()
    } == before_hashes

    repeated, repeat_report = ingest_codex(
        archive, sessions, ROOT, "codex-test", "UTC"
    )
    assert repeat_report.files_changed == 0
    assert repeated == after
    assert _manifest(archive).read_bytes() == manifest_bytes


def test_unconfigured_continuation_is_not_auto_linked(tmp_path: Path) -> None:
    sessions = tmp_path / "sessions"
    archive = tmp_path / "archive"
    origin = _origin(sessions)
    origin.parent.mkdir(parents=True)
    origin.write_bytes(_root_bytes())
    _write_continuation_sources(sessions)

    team, _ = ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")
    manifest = json.loads(_manifest(archive).read_text(encoding="utf-8"))

    assert {agent.thread_id for agent in team.agents} == {ROOT}
    assert "continuation_sessions" not in manifest
    assert {source["thread_id"] for source in manifest["sources"]} == {ROOT}


def test_cli_accepts_ordered_repeatable_continuation_sessions(tmp_path: Path) -> None:
    sessions = tmp_path / "sessions"
    archive = tmp_path / "archive"
    origin = _origin(sessions)
    origin.parent.mkdir(parents=True)
    origin.write_bytes(_root_bytes())
    _write_continuation_sources(sessions)
    _write_second_continuation_source(sessions)

    status = timeline_main(
        (
            "ingest",
            "--sessions-root",
            str(sessions),
            "--root-session",
            ROOT,
            "--continuation-session",
            CONTINUATION,
            "--continuation-session",
            CONTINUATION_TWO,
            "--team",
            "codex-test",
            "--output",
            str(archive),
            "--timezone",
            "UTC",
        )
    )

    assert status == 0
    manifest = json.loads(_manifest(archive).read_text(encoding="utf-8"))
    assert [
        link["thread_id"] for link in manifest["continuation_sessions"]
    ] == [CONTINUATION, CONTINUATION_TWO]
    assert manifest["continuation_sessions"][1]["predecessor_thread_id"] == CONTINUATION
    raw = json.loads(
        (team_build_root(archive, "codex-test") / "raw" / "team.json").read_text(
            encoding="utf-8"
        )
    )
    agents = {agent["thread_id"]: agent for agent in raw["agents"]}
    assert agents[CONTINUATION_TWO]["parent_thread_id"] == CONTINUATION
    assert agents[CONTINUATION_TWO]["depth"] == 2


def test_refresh_records_and_reuses_explicit_continuation(tmp_path: Path) -> None:
    sessions = tmp_path / "sessions"
    archive = tmp_path / "archive"
    origin = _origin(sessions)
    origin.parent.mkdir(parents=True)
    origin.write_bytes(_root_bytes())
    _write_continuation_sources(sessions)
    common = (
        "--sessions-root",
        str(sessions),
        "--root-session",
        ROOT,
        "--team",
        "codex-test",
        "--output",
        str(archive),
        "--timezone",
        "UTC",
        "--backend",
        "heuristic",
        "--model",
        "deterministic-local",
    )

    first_status = timeline_main(
        ("refresh", *common, "--continuation-session", CONTINUATION)
    )
    first_manifest = json.loads(_manifest(archive).read_text(encoding="utf-8"))
    second_status = timeline_main(("refresh", *common))
    second_manifest = json.loads(_manifest(archive).read_text(encoding="utf-8"))

    assert first_status == second_status == 0
    assert first_manifest["continuation_sessions"] == second_manifest[
        "continuation_sessions"
    ]
    assert second_manifest["continuation_sessions"][0]["thread_id"] == CONTINUATION


def test_recorded_continuation_prefix_can_only_be_extended_in_order(
    tmp_path: Path,
) -> None:
    sessions, archive, _ = _first_ingest(tmp_path)
    _write_continuation_sources(sessions)
    ingest_codex(
        archive,
        sessions,
        ROOT,
        "codex-test",
        "UTC",
        continuation_thread_ids=(CONTINUATION,),
    )
    one_link_manifest = _manifest(archive).read_bytes()
    _write_second_continuation_source(sessions)

    with pytest.raises(CodexParseError, match="recorded ordered prefix"):
        ingest_codex(
            archive,
            sessions,
            ROOT,
            "codex-test",
            "UTC",
            continuation_thread_ids=(CONTINUATION_TWO,),
        )
    assert _manifest(archive).read_bytes() == one_link_manifest

    extended, _ = ingest_codex(
        archive,
        sessions,
        ROOT,
        "codex-test",
        "UTC",
        continuation_thread_ids=(CONTINUATION, CONTINUATION_TWO),
    )
    assert [
        edge.to_thread_id for edge in extended.edges if edge.kind == "continuation"
    ] == [CONTINUATION, CONTINUATION_TWO]

    with pytest.raises(CodexParseError, match="recorded ordered prefix"):
        ingest_codex(
            archive,
            sessions,
            ROOT,
            "codex-test",
            "UTC",
            continuation_thread_ids=(CONTINUATION,),
        )
    repeated, report = ingest_codex(
        archive, sessions, ROOT, "codex-test", "UTC"
    )
    assert repeated == extended
    assert report.files_changed == 0


def test_cross_lineage_raw_thread_collision_fails_closed(tmp_path: Path) -> None:
    sessions = tmp_path / "sessions"
    archive = tmp_path / "archive"
    root = _origin(sessions)
    root.parent.mkdir(parents=True)
    root.write_bytes(_root_bytes())
    (root.parent / "rollout-child.jsonl").write_bytes(_child_bytes())
    (root.parent / "rollout-continuation.jsonl").write_bytes(
        _continuation_root_bytes()
    )
    collision = _continuation_child_bytes().replace(
        CONTINUATION_CHILD.encode("utf-8"), CHILD.encode("utf-8")
    )
    (root.parent / "rollout-continuation-collision.jsonl").write_bytes(collision)

    with pytest.raises(CodexParseError, match="occurs in multiple configured lineages"):
        ingest_codex(
            archive,
            sessions,
            ROOT,
            "codex-test",
            "UTC",
            continuation_thread_ids=(CONTINUATION,),
        )

    assert not _manifest(archive).exists()


def test_encoded_continuation_tool_id_cannot_overwrite_canonical_tool(
    tmp_path: Path,
) -> None:
    sessions = tmp_path / "sessions"
    archive = tmp_path / "archive"
    root = _origin(sessions)
    root.parent.mkdir(parents=True)
    encoded = codex_module._scoped_id(CONTINUATION, ROOT, "shared-call")
    root.write_bytes(
        _root_bytes().replace(b'"shared-call"', json.dumps(encoded).encode("utf-8"))
    )
    _write_continuation_sources(sessions)

    with pytest.raises(CodexParseError, match="collides across Codex lineages"):
        ingest_codex(
            archive,
            sessions,
            ROOT,
            "codex-test",
            "UTC",
            continuation_thread_ids=(CONTINUATION,),
        )

    assert not _manifest(archive).exists()


def test_continuation_path_component_is_injective_and_path_safe() -> None:
    slash_root = codex_module._continuation_path("a/b")
    percent_root = codex_module._continuation_path("a%2Fb")

    assert slash_root == "/root/continuation-a%2Fb"
    assert percent_root == "/root/continuation-a%252Fb"
    assert slash_root != percent_root
    assert slash_root != codex_module._continuation_path("a") + "/b"


def test_collaboration_edge_cannot_overwrite_continuation_edge(tmp_path: Path) -> None:
    sessions, archive, origin = _first_ingest(tmp_path)
    child = origin.with_name("rollout-child.jsonl")
    child.write_bytes(_child_bytes())
    baseline, _ = ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")
    raw_team = team_build_root(archive, "codex-test") / "raw" / "team.json"
    raw_before = raw_team.read_bytes()
    edge_id = f"codex-continuation-{CONTINUATION}"
    origin.write_bytes(
        origin.read_bytes()
        + _line(
            1_004.1,
            "response_item",
            {
                "type": "function_call",
                "id": "collision-item",
                "call_id": edge_id,
                "namespace": "collaboration",
                "name": "send_message",
                "arguments": json.dumps({"target": CHILD, "message": "collision"}),
            },
        )
        + _line(
            1_004.2,
            "event_msg",
            {
                "type": "sub_agent_activity",
                "event_id": edge_id,
                "occurred_at_ms": 1_004_200,
                "agent_thread_id": CHILD,
                "kind": "message",
            },
        )
    )
    _write_continuation_sources(sessions)

    with pytest.raises(CodexParseError, match="collides with a continuation edge"):
        ingest_codex(
            archive,
            sessions,
            ROOT,
            "codex-test",
            "UTC",
            continuation_thread_ids=(CONTINUATION,),
        )

    assert raw_team.read_bytes() == raw_before
    assert baseline.root_thread_id == ROOT


def test_persisted_continuation_boundary_is_frozen_when_predecessor_advances(
    tmp_path: Path,
) -> None:
    sessions, archive, origin = _first_ingest(tmp_path)
    _write_continuation_sources(sessions)
    ingest_codex(
        archive,
        sessions,
        ROOT,
        "codex-test",
        "UTC",
        continuation_thread_ids=(CONTINUATION,),
    )
    initial = json.loads(_manifest(archive).read_text(encoding="utf-8"))
    initial_link = initial["continuation_sessions"][0]
    # This appended predecessor record would be a closer boundary if the evidence
    # were recomputed instead of reused from the durable manifest.
    origin.write_bytes(
        origin.read_bytes()
        + _line(1_005.1, "ignored_record", {"type": "ignored_after_link"})
    )

    _, report = ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")

    assert report.files_changed > 0
    updated = json.loads(_manifest(archive).read_text(encoding="utf-8"))
    assert updated["continuation_sessions"][0] == initial_link


def test_continuation_source_truncation_fails_without_replacing_archive(
    tmp_path: Path,
) -> None:
    sessions, archive, _ = _first_ingest(tmp_path)
    _, continuation_child = _write_continuation_sources(sessions)
    before, _ = ingest_codex(
        archive,
        sessions,
        ROOT,
        "codex-test",
        "UTC",
        continuation_thread_ids=(CONTINUATION,),
    )
    manifest_before = _manifest(archive).read_bytes()
    raw_team = team_build_root(archive, "codex-test") / "raw" / "team.json"
    team_before = raw_team.read_bytes()
    child_bytes = continuation_child.read_bytes()
    continuation_child.write_bytes(child_bytes.rsplit(b"\n", 2)[0] + b"\n")

    with pytest.raises(CodexParseError, match="newline-complete prefix shrank"):
        ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")

    assert _manifest(archive).read_bytes() == manifest_before
    assert raw_team.read_bytes() == team_before
    assert before.root_thread_id == ROOT


def test_orphan_snapshot_is_excluded_from_manifest_bound_parse(tmp_path: Path) -> None:
    sessions, archive, _ = _first_ingest(tmp_path)
    orphan = snapshot_root(archive, "codex-test") / "orphan" / "rollout-child.jsonl"
    orphan.parent.mkdir(parents=True)
    orphan.write_bytes(_child_bytes())

    team, _ = ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")

    assert [agent.thread_id for agent in team.agents] == [ROOT]
    manifest = json.loads(_manifest(archive).read_text(encoding="utf-8"))
    assert [source["thread_id"] for source in manifest["sources"]] == [ROOT]
    assert orphan.is_file()  # ignored, not silently adopted or destructively removed


@pytest.mark.parametrize("symlink_level", ["root", "intermediate"])
def test_snapshot_directory_symlink_escape_is_rejected(
    tmp_path: Path, symlink_level: str
) -> None:
    sessions = tmp_path / "sessions"
    archive = tmp_path / "archive"
    origin = _origin(sessions)
    origin.parent.mkdir(parents=True)
    origin.write_bytes(_root_bytes())
    archive.mkdir()
    (archive / ".wrkviz.json").write_text(
        '{"schema_version":1,"tool":"wrkviz"}\n', encoding="utf-8"
    )
    snapshots = snapshot_root(archive, "codex-test")
    outside = tmp_path / "outside"
    outside.mkdir()
    if symlink_level == "root":
        snapshots.parent.mkdir(parents=True)
        snapshots.symlink_to(outside, target_is_directory=True)
    else:
        snapshots.mkdir(parents=True)
        (snapshots / "2026").symlink_to(outside, target_is_directory=True)

    with pytest.raises(CodexParseError, match="symlink or non-directory"):
        ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")

    assert not (outside / "08" / "05" / "rollout-root.jsonl").exists()


def test_same_content_snapshot_target_symlink_is_rejected(tmp_path: Path) -> None:
    sessions, archive, _ = _first_ingest(tmp_path)
    snapshot = _snapshot(archive)
    outside = tmp_path / "outside-rollout.jsonl"
    outside.write_bytes(snapshot.read_bytes())
    snapshot.unlink()
    snapshot.symlink_to(outside)

    with pytest.raises(CodexParseError, match="target must not be a symlink"):
        ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")

    assert outside.read_bytes() == _root_bytes()


def _manifest_sources(archive: Path) -> tuple[CodexSourceCopy, ...]:
    manifest: object = json.loads(_manifest(archive).read_text(encoding="utf-8"))
    if not isinstance(manifest, dict):
        raise AssertionError("source manifest is not an object")
    raw_sources = manifest.get("sources")
    if not isinstance(raw_sources, list):
        raise AssertionError("source manifest lacks a sources array")
    result: list[CodexSourceCopy] = []
    for index, raw_source in enumerate(raw_sources):
        if not isinstance(raw_source, dict):
            raise AssertionError("source record is not an object")
        result.append(
            CodexSourceCopy.from_json_obj(
                {str(key): value for key, value in raw_source.items()},
                f"sources[{index}]",
            )
        )
    return tuple(result)


def test_retry_recovers_copy_that_advanced_before_manifest(tmp_path: Path) -> None:
    sessions, archive, origin = _first_ingest(tmp_path)
    old_manifest = _manifest(archive).read_bytes()
    appended = _line(
        1_004,
        "event_msg",
        {"type": "thread_goal_updated", "goal": {"status": "complete"}},
    )
    expected = _root_bytes() + appended
    origin.write_bytes(expected)

    copied_only = snapshot_codex_lineage(
        sessions,
        ROOT,
        snapshot_root(archive, "codex-test"),
        _manifest_sources(archive),
        "2026-08-05T12:00:00Z",
    )
    assert copied_only.files_changed == 1
    assert _snapshot(archive).read_bytes() == expected
    assert _manifest(archive).read_bytes() == old_manifest

    _, report = ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")

    assert _snapshot(archive).read_bytes() == expected
    assert report.source_bytes == len(expected)
    assert json.loads(_manifest(archive).read_text(encoding="utf-8"))["sources"][0][
        "copied_bytes"
    ] == len(expected)


def test_pipeline_does_not_reopen_live_original_after_copy(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    sessions = tmp_path / "sessions"
    archive = tmp_path / "archive"
    origin = _origin(sessions)
    origin.parent.mkdir(parents=True)
    origin.write_bytes(_root_bytes())
    real_snapshot = snapshot_codex_lineage

    def snapshot_then_remove(
        sessions_root: Path,
        root_thread_id: str,
        snapshot_root: Path,
        previous_sources: Sequence[CodexSourceCopy],
        updated_at: str,
    ) -> CodexSnapshotResult:
        result = real_snapshot(
            sessions_root,
            root_thread_id,
            snapshot_root,
            previous_sources,
            updated_at,
        )
        origin.unlink()
        return result

    monkeypatch.setattr(pipeline_module, "snapshot_codex_lineage", snapshot_then_remove)

    team, report = ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")

    assert report.sources == 1
    assert [event.text for event in team.events] == ["Root done"]
    assert not origin.exists()


def test_archive_lock_prevents_interleaved_writer_regression(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    sessions, archive, origin = _first_ingest(tmp_path)
    medium_line = _line(
        1_004,
        "event_msg",
        {"type": "thread_goal_updated", "goal": {"status": "working"}},
    )
    newest_line = _line(
        1_005,
        "event_msg",
        {"type": "thread_goal_updated", "goal": {"status": "complete"}},
    )
    medium = _root_bytes() + medium_line
    newest = medium + newest_line
    origin.write_bytes(medium)
    older_at_write = threading.Event()
    release_older = threading.Event()
    newer_done = threading.Event()
    errors: list[BaseException] = []
    real_write = codex_module._write_snapshot_file

    def delayed_write(
        snapshot_root: Path,
        source_path: str,
        data: bytes,
        expected: bytes | None,
    ) -> bool:
        if threading.current_thread().name == "older-ingest":
            older_at_write.set()
            if not release_older.wait(timeout=5):
                raise AssertionError("timed out releasing older ingest")
        return real_write(snapshot_root, source_path, data, expected)

    monkeypatch.setattr(codex_module, "_write_snapshot_file", delayed_write)

    def run_ingest(done: threading.Event | None = None) -> None:
        try:
            ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")
        except BaseException as error:
            errors.append(error)
        finally:
            if done is not None:
                done.set()

    older = threading.Thread(target=run_ingest, name="older-ingest")
    older.start()
    assert older_at_write.wait(timeout=5)
    origin.write_bytes(newest)
    newer = threading.Thread(target=run_ingest, args=(newer_done,), name="newer-ingest")
    newer.start()
    assert not newer_done.wait(timeout=0.1)
    release_older.set()
    older.join(timeout=5)
    newer.join(timeout=5)

    assert not older.is_alive()
    assert not newer.is_alive()
    assert errors == []
    assert _snapshot(archive).read_bytes() == newest
    manifest = json.loads(_manifest(archive).read_text(encoding="utf-8"))
    assert manifest["sources"][0]["copied_bytes"] == len(newest)


def test_metadata_replacement_between_discovery_and_read_is_rejected(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    sessions = tmp_path / "sessions"
    archive = tmp_path / "archive"
    origin = _origin(sessions)
    origin.parent.mkdir(parents=True)
    origin.write_bytes(_root_bytes())
    real_discover = codex_module._discover_codex_lineage

    def discover_then_replace(
        sessions_root: Path,
        root_thread_id: str,
        *,
        exclude_root: Path | None = None,
    ) -> tuple[tuple[Path, Mapping[str, object]], ...]:
        result = real_discover(
            sessions_root,
            root_thread_id,
            exclude_root=exclude_root,
        )
        origin.write_bytes(_root_bytes().replace(b'"cwd":"/work/project"', b'"cwd":"/work/changed"'))
        return result

    monkeypatch.setattr(codex_module, "_discover_codex_lineage", discover_then_replace)

    with pytest.raises(CodexParseError, match="metadata changed between discovery"):
        ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")

    assert not _snapshot(archive).exists()

# --- Upstream retention: the archive is the record and upstream is a feed --------------------
#
# The archive is the record and upstream is a feed: retention and compression upstream are expected.
# 
# Four situations a months-long archive meets, one test group each:
# 
# * a rollout or transcript upstream has rotated away -- the archive keeps it and says so;
# * an upstream file compressed in place (``X`` -> ``X.zst``) -- read as if it were not;
# * upstream content changed in place for records already imported -- still a refusal;
# * new records appended after the last import -- appended, and kept after upstream retires them.

ZSTD = shutil.which("zstd")
needs_zstd = pytest.mark.skipif(ZSTD is None, reason="the zstd command is not installed")
RETENTION_TEAM = "codex-test"


def _compress_in_place(path: Path) -> Path:
    """Do what the operator did: replace ``X`` with ``X.zst``."""

    assert ZSTD is not None
    subprocess.run([ZSTD, "--quiet", "--rm", str(path)], check=True)
    assert not path.exists()
    return compressed_sibling(path)


def _manifest_records(archive: Path) -> dict[str, dict[str, object]]:
    raw = json.loads(_manifest(archive).read_text(encoding="utf-8"))
    return {str(source["source_path"]): source for source in raw["sources"]}


def _ingest(sessions: Path, archive: Path) -> tuple[TeamData, IngestReport]:
    return ingest_codex(archive, sessions, ROOT, RETENTION_TEAM, "UTC")


def _lineage(tmp_path: Path) -> tuple[Path, Path, Path, Path]:
    sessions = tmp_path / "sessions"
    archive = tmp_path / "archive"
    root = _origin(sessions)
    root.parent.mkdir(parents=True)
    root.write_bytes(_root_bytes())
    child = root.parent / "rollout-child.jsonl"
    child.write_bytes(_child_bytes())
    _ingest(sessions, archive)
    return sessions, archive, root, child


# --- rotated away ----------------------------------------------------------------------------


def test_rotated_away_rollout_is_retired_not_refused(tmp_path: Path) -> None:
    sessions, archive, root, child = _lineage(tmp_path)
    snapshot_before = _snapshot(archive).read_bytes()
    record_before = _manifest_records(archive)[root.relative_to(sessions).as_posix()]
    root.unlink()

    team, report = _ingest(sessions, archive)

    relative = root.relative_to(sessions).as_posix()
    assert report.upstream_retired_sources == (relative,)
    assert {agent.thread_id for agent in team.agents} == {ROOT, CHILD}
    assert _snapshot(archive).read_bytes() == snapshot_before
    record = _manifest_records(archive)[relative]
    retired_at = record.pop("upstream_retired_at")
    assert isinstance(retired_at, str) and retired_at
    assert record == record_before
    assert "upstream_retired_at" not in _manifest_records(archive)[
        child.relative_to(sessions).as_posix()
    ]

    # Sticky: the run that noticed is the date recorded, not every run since.
    _, again = _ingest(sessions, archive)
    assert again.upstream_retired_sources == (relative,)
    assert _manifest_records(archive)[relative]["upstream_retired_at"] == retired_at


def test_whole_lineage_retired_upstream_still_ingests(tmp_path: Path) -> None:
    sessions, archive, root, child = _lineage(tmp_path)
    root.unlink()
    child.unlink()

    team, report = _ingest(sessions, archive)

    assert len(report.upstream_retired_sources) == 2
    assert {agent.thread_id for agent in team.agents} == {ROOT, CHILD}


def test_retired_rollout_whose_archive_copy_is_gone_refuses(tmp_path: Path) -> None:
    sessions, archive, root, _ = _lineage(tmp_path)
    manifest_before = _manifest(archive).read_bytes()
    root.unlink()
    _snapshot(archive).unlink()

    with pytest.raises(CodexParseError, match="archive's own snapshot is missing"):
        _ingest(sessions, archive)
    assert _manifest(archive).read_bytes() == manifest_before


def test_retired_rollout_whose_archive_copy_was_altered_refuses(tmp_path: Path) -> None:
    sessions, archive, root, _ = _lineage(tmp_path)
    root.unlink()
    snapshot = _snapshot(archive)
    snapshot.write_bytes(snapshot.read_bytes().replace(b"Root done", b"Root gone"))

    with pytest.raises(CodexParseError, match="does not match its manifest record"):
        _ingest(sessions, archive)


def test_retired_claude_transcript_keeps_the_archived_lineage(tmp_path: Path) -> None:
    live = tmp_path / "live"
    shutil.copytree(FIXTURE_ROOT, live)
    snapshot = tmp_path / "snapshot"
    first = snapshot_claude_lineage(_session(live), snapshot, (), "2026-01-03T00:00:00Z")

    shutil.rmtree(live)
    live.mkdir()
    retired = snapshot_claude_lineage(
        _session(live), snapshot, first.sources, "2026-01-04T00:00:00Z"
    )

    assert retired.files_changed == 0
    assert set(retired.upstream_retired) == {source.source_path for source in first.sources}
    assert [
        (source.source_path, source.sha256, source.copied_bytes) for source in retired.sources
    ] == [(source.source_path, source.sha256, source.copied_bytes) for source in first.sources]
    assert {source.upstream_retired_at for source in retired.sources} == {
        "2026-01-04T00:00:00Z"
    }

    (snapshot / _session(live).name).unlink()
    with pytest.raises(ClaudeParseError, match="archive's own snapshot is missing"):
        snapshot_claude_lineage(_session(live), snapshot, retired.sources, "2026-01-05T00:00:00Z")


def test_never_imported_claude_session_without_upstream_is_still_an_error(
    tmp_path: Path,
) -> None:
    with pytest.raises(ClaudeParseError, match="not a regular file"):
        snapshot_claude_lineage(
            _session(tmp_path / "empty"), tmp_path / "snapshot", (), "2026-01-03T00:00:00Z"
        )


# --- compressed in place ---------------------------------------------------------------------


def test_resolution_prefers_the_plain_spelling(tmp_path: Path) -> None:
    plain = tmp_path / "a.jsonl"
    assert resolve_source(plain) is None
    compressed_sibling(plain).write_bytes(b"x")
    assert resolve_source(plain) == compressed_sibling(plain)
    plain.write_bytes(b"y")
    assert resolve_source(plain) == plain
    assert logical_path(compressed_sibling(plain)) == plain
    assert iter_logical_glob(tmp_path, "*.jsonl") == [plain]
    with pytest.raises(FileNotFoundError):
        read_source_bytes(tmp_path / "missing.jsonl")


@needs_zstd
def test_compressed_file_reads_as_its_logical_bytes(tmp_path: Path) -> None:
    plain = tmp_path / "rollout.jsonl"
    data = b'{"a":1}\n{"b":2}\n'
    plain.write_bytes(data)
    _compress_in_place(plain)

    assert read_source_bytes(plain) == data
    assert read_first_line(plain) == b'{"a":1}\n'
    copy = materialize_source(plain, tmp_path / "scratch")
    assert copy.read_bytes() == data and copy.parent == tmp_path / "scratch"


@needs_zstd
def test_compressed_upstream_rollout_is_not_a_change(tmp_path: Path) -> None:
    sessions, archive, root, child = _lineage(tmp_path)
    manifest_before = _manifest(archive).read_bytes()
    _compress_in_place(root)
    _compress_in_place(child)

    _, report = _ingest(sessions, archive)

    assert report.upstream_retired_sources == ()
    assert report.files_changed == 0
    assert _manifest(archive).read_bytes() == manifest_before


@needs_zstd
def test_first_ingest_can_start_from_compressed_upstream(tmp_path: Path) -> None:
    sessions = tmp_path / "sessions"
    root = _origin(sessions)
    root.parent.mkdir(parents=True)
    root.write_bytes(_root_bytes())
    _compress_in_place(root)

    team, report = _ingest(sessions, tmp_path / "archive")

    assert {agent.thread_id for agent in team.agents} == {ROOT}
    assert _snapshot(tmp_path / "archive").read_bytes() == _root_bytes()
    record = _manifest_records(tmp_path / "archive")[root.relative_to(sessions).as_posix()]
    assert record["original_path"] == str(root.resolve())
    assert report.upstream_retired_sources == ()


@needs_zstd
def test_compressed_upstream_rewritten_in_place_still_refuses(tmp_path: Path) -> None:
    sessions, archive, root, _ = _lineage(tmp_path)
    snapshot_before = _snapshot(archive).read_bytes()
    root.write_bytes(root.read_bytes().replace(b"Root done", b"Root gone"))
    _compress_in_place(root)

    with pytest.raises(CodexParseError, match="existing prefix was rewritten"):
        _ingest(sessions, archive)
    assert _snapshot(archive).read_bytes() == snapshot_before


# --- changed in place ------------------------------------------------------------------------


def test_upstream_changed_in_place_refuses_and_keeps_the_archive(tmp_path: Path) -> None:
    sessions, archive, root, _ = _lineage(tmp_path)
    snapshot_before = _snapshot(archive).read_bytes()
    manifest_before = _manifest(archive).read_bytes()
    root.write_bytes(root.read_bytes().replace(b"Root done", b"Root gone"))

    with pytest.raises(CodexParseError, match="existing prefix was rewritten"):
        _ingest(sessions, archive)
    assert _snapshot(archive).read_bytes() == snapshot_before
    assert _manifest(archive).read_bytes() == manifest_before


# --- incremental append after a checkpoint ---------------------------------------------------


def test_append_after_checkpoint_is_kept_once_upstream_retires_it(tmp_path: Path) -> None:
    sessions, archive, root, _ = _lineage(tmp_path)
    relative = root.relative_to(sessions).as_posix()
    checkpoint = _manifest_records(archive)[relative]
    appended = _line(
        1_020,
        "response_item",
        {
            "type": "message",
            "id": "root-late",
            "role": "assistant",
            "content": [{"type": "output_text", "text": "Appended after the checkpoint"}],
        },
    )
    root.write_bytes(root.read_bytes() + appended)

    team, report = _ingest(sessions, archive)
    advanced = _manifest_records(archive)[relative]
    assert report.files_changed > 0
    assert advanced["copied_bytes"] == int(str(checkpoint["copied_bytes"])) + len(appended)
    assert _snapshot(archive).read_bytes().endswith(appended)
    assert any("Appended after the checkpoint" in (event.text or "") for event in team.events)

    root.unlink()
    retired_team, retired = _ingest(sessions, archive)
    assert retired.upstream_retired_sources == (relative,)
    assert _manifest_records(archive)[relative]["copied_bytes"] == advanced["copied_bytes"]
    assert any(
        "Appended after the checkpoint" in (event.text or "") for event in retired_team.events
    )
    assert (team_build_root(archive, RETENTION_TEAM) / "raw" / "team.json").is_file()


# --- Orc -------------------------------------------------------------------------------------


@needs_zstd
def test_compressed_orc_databases_snapshot_identically(tmp_path: Path) -> None:
    source, root_db, task_db = _orc_fixture(tmp_path)
    snapshot = tmp_path / "snapshot"
    first = snapshot_orc_lineage(source, ORC_ROOT, snapshot, (), "first")
    for database in (root_db, task_db, source / ".orc" / "index.db"):
        _compress_in_place(database)

    second = snapshot_orc_lineage(source, ORC_ROOT, snapshot, first.sources, "second")

    assert second.files_changed == 0
    assert [(s.source_path, s.sha256) for s in second.sources] == [
        (s.source_path, s.sha256) for s in first.sources
    ]


def test_orc_lineage_retired_upstream_keeps_its_archive(tmp_path: Path) -> None:
    source, _, _ = _orc_fixture(tmp_path)
    archive = tmp_path / "archive"
    team, _ = ingest_orc(archive, source, ORC_ROOT, "orc-test", "UTC")
    manifest = team_build_root(archive, "orc-test") / "raw" / "source-manifest.json"
    manifest_before = manifest.read_bytes()
    shutil.rmtree(source)

    retired_team, report = ingest_orc(archive, source, ORC_ROOT, "orc-test", "UTC")

    assert set(report.upstream_retired_sources) == {
        str(raw["source_path"]) for raw in json.loads(manifest_before)["sources"]
    }
    assert manifest.read_bytes() == manifest_before
    assert retired_team.events == team.events
    assert retired_team.agents == team.agents


def test_orc_lineage_retired_with_damaged_archive_copy_refuses(tmp_path: Path) -> None:
    source, _, _ = _orc_fixture(tmp_path)
    archive = tmp_path / "archive"
    ingest_orc(archive, source, ORC_ROOT, "orc-test", "UTC")
    shutil.rmtree(source)
    objects = sorted((snapshot_root(archive, "orc-test") / ".objects").rglob("*.db"))
    objects[0].write_bytes(b"not the database the manifest recorded")

    with pytest.raises(OrcParseError, match="does not match its manifest"):
        ingest_orc(archive, source, ORC_ROOT, "orc-test", "UTC")


def test_rollout_still_present_but_rewritten_is_refused_not_retired(tmp_path: Path) -> None:
    sessions, archive, root, _ = _lineage(tmp_path)
    data = root.read_bytes()
    first, rest = data.split(b"\n", 1)
    root.write_bytes(first.replace(ROOT.encode(), b"someone-else") + b"\n" + rest)

    with pytest.raises(CodexParseError, match="still present but no longer readable"):
        _ingest(sessions, archive)


def test_retired_rollout_keeps_a_snapshot_longer_than_its_record(tmp_path: Path) -> None:
    sessions, archive, root, _ = _lineage(tmp_path)
    snapshot = _snapshot(archive)
    tail = _line(1_030, "event_msg", {"type": "agent_message", "message": "written, not recorded"})
    snapshot.write_bytes(snapshot.read_bytes() + tail)
    root.unlink()

    _, report = _ingest(sessions, archive)

    assert report.upstream_retired_sources == (root.relative_to(sessions).as_posix(),)
    assert snapshot.read_bytes().endswith(tail)


def test_claude_root_replaced_by_a_symlink_is_refused(tmp_path: Path) -> None:
    live = tmp_path / "live"
    shutil.copytree(FIXTURE_ROOT, live)
    snapshot = tmp_path / "snapshot"
    first = snapshot_claude_lineage(_session(live), snapshot, (), "2026-01-03T00:00:00Z")
    root = _session(live)
    root.unlink()
    root.symlink_to(tmp_path / "elsewhere.jsonl")

    with pytest.raises(ClaudeParseError, match="not a regular file"):
        snapshot_claude_lineage(root, snapshot, first.sources, "2026-01-04T00:00:00Z")


def test_claude_subagents_keep_being_read_after_the_root_retires(tmp_path: Path) -> None:
    live = tmp_path / "live"
    shutil.copytree(FIXTURE_ROOT, live)
    snapshot = tmp_path / "snapshot"
    first = snapshot_claude_lineage(_session(live), snapshot, (), "2026-01-03T00:00:00Z")
    root = _session(live)
    child = root.parent / root.stem / "subagents" / "agent-a-child.jsonl"
    root.unlink()
    with child.open("ab") as handle:
        handle.write(
            b'{"type":"system","uuid":"child-late","timestamp":"2026-01-02T10:03:00.000Z",'
            b'"sessionId":"11111111-1111-4111-8111-111111111111","agentId":"a-child"}\n'
        )

    after = snapshot_claude_lineage(root, snapshot, first.sources, "2026-01-04T00:00:00Z")

    assert after.upstream_retired == (root.name,)
    copies = {copy.source_path: copy for copy in after.sources}
    child_relative = child.relative_to(root.parent).as_posix()
    before = {copy.source_path: copy for copy in first.sources}
    assert copies[child_relative].copied_bytes > before[child_relative].copied_bytes


# --- Muse provider ---------------------------------------------------------------------------
#
# A redacted Muse session in the shapes the runtime writes: an event-sourced session.jsonl whose
# records carry recorded_at (microseconds), payload_type and payload; one subagent spawned through
# subagent_spawn and bound by child_session_bound; and one runtime observer under subagent/ that
# was never bound, which must stay out of the team.

MUSE_ROOT = "01a10000-0000-7000-8000-000000000001"
MUSE_CHILD = "01a10000-0000-7000-8000-000000000002"
MUSE_OBSERVER = "01a10000-0000-7000-8000-000000000003"
MUSE_RUN = "01a10000-0000-7000-8000-0000000000a1"
MUSE_CHILD_RUN = "01a10000-0000-7000-8000-0000000000b1"


def _muse_record(seconds: float, payload_type: str, payload: dict[str, object]) -> bytes:
    record = {
        "schema_version": 1,
        "id": f"rec-{payload_type}-{seconds}",
        "recorded_at": int((1_790_000_000 + seconds) * 1_000_000),
        "record_type": "event",
        "payload_type": payload_type,
        "payload": payload,
    }
    return json.dumps(record, sort_keys=True).encode("utf-8") + b"\n"


def _muse_run(seconds: float, run_id: str, event: dict[str, object]) -> bytes:
    return _muse_record(seconds, "runtime.session", {"kind": "run", "run_id": run_id, "event": event})


def _muse_root_log() -> bytes:
    return b"".join(
        (
            _muse_record(0, "runtime.session.route_facts",
                         {"kind": "route_facts", "record": {"cwd": "/work/project"}}),
            _muse_record(1, "runtime.user_intent.accepted", {
                "intent_id": MUSE_RUN,
                "model_messages": [{"content": [{"kind": "text", "text": "Review the parser."}]}],
            }),
            _muse_run(1.1, MUSE_RUN, {"kind": "hook_run_started", "event": "SessionStart"}),
            _muse_run(2, MUSE_RUN, {
                "kind": "assistant_tool_calls_committed", "message_id": "batch-1",
                "tool_calls": [
                    {"call_id": "call-read", "id": "fc-read", "name": "read_file",
                     "args": "{\"path\": \"src/parser.rs\"}"},
                    {"call_id": "call-spawn", "id": "fc-spawn", "name": "subagent_spawn",
                     "args": "{\"role\": \"adversarial reviewer\", \"objective\": \"Try to break the parser.\"}"},
                ],
            }),
            _muse_record(2.1, "tool_batch.effect.started", {"kind": "tool_batch_effect", "record": {
                "call_id": "call-read", "kind": "started", "tool_name": "read_file"}}),
            _muse_record(2.4, "tool_batch.effect.terminal", {"kind": "tool_batch_effect", "record": {
                "call_id": "call-read", "kind": "terminal", "tool_name": "read_file",
                "outcome": {"kind": "completed"}}}),
            _muse_record(2.5, "tool_batch.effect.started", {"kind": "tool_batch_effect", "record": {
                "call_id": "call-spawn", "kind": "started", "tool_name": "subagent_spawn"}}),
            _muse_record(2.6, "subagent.control.child_session_bound", {"kind": "subagent_control", "record": {
                "child_session_id": MUSE_CHILD, "kind": "child_session_bound", "subagent_id": "sub-1"}}),
            _muse_record(2.7, "subagent.control.start_attested", {"kind": "subagent_control", "record": {
                "dispatched_via_tool_call_id": "call-spawn", "kind": "start_attested",
                "subagent_session_id": MUSE_CHILD}}),
            _muse_record(3, "tool_batch.effect.terminal", {"kind": "tool_batch_effect", "record": {
                "call_id": "call-spawn", "kind": "terminal", "tool_name": "subagent_spawn",
                "outcome": {"kind": "completed"}}}),
            _muse_run(3.1, MUSE_RUN, {
                "kind": "tool_result_batch_committed", "batch_id": "batch-1",
                "results": [{"text": "fn parse() {}"}, {"text": "spawned"}],
            }),
            _muse_run(40, MUSE_RUN, {
                "kind": "inbox_item_drained",
                "delivery_snapshot": {"body": "Subagent result ready\nsource: subagent_result"},
            }),
            _muse_run(41, MUSE_RUN, {
                "kind": "assistant_message_committed", "message_id": "m-1",
                "text": "The parser accepts trailing commas; the reviewer agrees.",
            }),
            _muse_run(42, MUSE_RUN, {"kind": "terminal", "terminal": "completed",
                                     "time_to_first_token_ms": 500, "turn_duration_ms": 41000}),
        )
    )


def _muse_child_log() -> bytes:
    return b"".join(
        (
            _muse_run(2.8, MUSE_CHILD_RUN, {"kind": "started", "prompt": "Try to break the parser."}),
            _muse_run(30, MUSE_CHILD_RUN, {
                "kind": "assistant_message_committed", "message_id": "c-1",
                "text": "Trailing commas parse; nothing else breaks.",
            }),
            _muse_run(31, MUSE_CHILD_RUN, {"kind": "terminal", "terminal": "completed"}),
        )
    )


def _muse_observer_log() -> bytes:
    return _muse_run(5, "observer-run", {"kind": "started", "prompt": "You are a reminder observer."})


def _muse_session(tmp_path: Path) -> Path:
    session = tmp_path / "muse" / "2026" / "01" / "05" / MUSE_ROOT
    (session / "subagent" / MUSE_CHILD).mkdir(parents=True)
    (session / "subagent" / MUSE_OBSERVER).mkdir(parents=True)
    (session / "session.jsonl").write_bytes(_muse_root_log())
    (session / "subagent" / MUSE_CHILD / "session.jsonl").write_bytes(_muse_child_log())
    (session / "subagent" / MUSE_OBSERVER / "session.jsonl").write_bytes(_muse_observer_log())
    return session


def test_muse_session_ingests_turns_tools_and_its_spawned_subagent(tmp_path: Path) -> None:
    session = _muse_session(tmp_path)

    team, report = ingest_muse(tmp_path / "archive", session, "muse-test", "UTC")

    assert team.provider == "muse"
    assert report.sources == 2  # the observer that was never bound is not part of the team
    assert {agent.thread_id: agent.parent_thread_id for agent in team.agents} == {
        MUSE_ROOT: None,
        MUSE_CHILD: MUSE_ROOT,
    }
    child = next(agent for agent in team.agents if agent.thread_id == MUSE_CHILD)
    assert child.agent_path == "/root/adversarial-reviewer"
    (edge,) = team.edges
    assert (edge.kind, edge.call_id, edge.message_text) == (
        "spawn", "call-spawn", "Try to break the parser.",
    )
    tools = {tool.call_id: tool for tool in team.tool_calls}
    read = tools["call-read"]
    assert read.ended_at_ms is not None
    assert (read.name, read.status, read.ended_at_ms - read.started_at_ms) == ("read_file", "completed", 300)
    kinds = sorted((event.thread_id == MUSE_CHILD, event.kind) for event in team.events)
    assert kinds == [
        (False, "assistant_message"), (False, "system_input"), (False, "user_prompt"),
        (True, "assistant_message"), (True, "user_prompt"),
    ]
    root_turn = next(turn for turn in team.turns if turn.turn_id == MUSE_RUN)
    assert root_turn.status == "completed"
    assert root_turn.last_agent_message == "The parser accepts trailing commas; the reviewer agrees."


def test_muse_logs_are_append_only_retired_and_read_compressed(tmp_path: Path) -> None:
    session = _muse_session(tmp_path)
    archive = tmp_path / "archive"
    ingest_muse(archive, session, "muse-test", "UTC")

    log = session / "session.jsonl"
    extra = _muse_run(50, MUSE_RUN, {"kind": "assistant_message_committed", "text": "One more note."})
    log.write_bytes(log.read_bytes() + extra)
    team, _ = ingest_muse(archive, session, "muse-test", "UTC")
    assert any(event.text == "One more note." for event in team.events)

    if ZSTD is not None:
        _compress_in_place(log)
        _, unchanged = ingest_muse(archive, session, "muse-test", "UTC")
        assert unchanged.upstream_retired_sources == ()

    shutil.rmtree(session)
    retired_team, retired = ingest_muse(archive, session, "muse-test", "UTC")
    assert set(retired.upstream_retired_sources) == {
        "session.jsonl", f"subagent/{MUSE_CHILD}/session.jsonl",
    }
    assert any(event.text == "One more note." for event in retired_team.events)


def test_muse_log_rewritten_in_place_refuses(tmp_path: Path) -> None:
    session = _muse_session(tmp_path)
    archive = tmp_path / "archive"
    ingest_muse(archive, session, "muse-test", "UTC")
    log = session / "session.jsonl"
    log.write_bytes(log.read_bytes().replace(b"Review the parser.", b"Review the lexer.."))

    with pytest.raises(MuseParseError, match="truncated or rewritten"):
        ingest_muse(archive, session, "muse-test", "UTC")


def test_muse_team_through_project_config_and_cli(tmp_path: Path) -> None:
    session = _muse_session(tmp_path)
    config = tmp_path / "project.json"
    config.write_text(json.dumps({
        "schema_version": 1,
        "output": "archive",
        "timezone": "UTC",
        "teams": [{"slug": "muse-team", "provider": "muse",
                   "source": {"session_dir": str(session)}}],
    }), encoding="utf-8")

    assert timeline_main(["ingest-project", "--config", str(config)]) == 0
    assert timeline_main([
        "ingest-muse", "--output", str(tmp_path / "cli-archive"), "--team", "muse-cli",
        "--timezone", "UTC", "--session-dir", str(session),
    ]) == 0
