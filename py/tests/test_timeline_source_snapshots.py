"""Append-only source-backup tests for the Codex timeline ingest."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import threading

import pytest

import agent_team_timeline.codex as codex_module
import agent_team_timeline.pipeline as pipeline_module
from agent_team_timeline.codex import (
    CodexParseError,
    CodexSnapshotResult,
    CodexSourceCopy,
    load_codex_team,
    snapshot_codex_lineage,
)
from agent_team_timeline.pipeline import ingest_codex


ROOT = "root-thread"
CHILD = "child-thread"


def _iso(seconds: int) -> str:
    return datetime.fromtimestamp(seconds, timezone.utc).isoformat().replace("+00:00", "Z")


def _line(timestamp: int, kind: str, payload: dict[str, object]) -> bytes:
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


def _origin(sessions: Path) -> Path:
    return sessions / "2026" / "08" / "05" / "rollout-root.jsonl"


def _snapshot(archive: Path) -> Path:
    return (
        archive
        / "teams"
        / "codex-test"
        / "source_snapshots"
        / "2026"
        / "08"
        / "05"
        / "rollout-root.jsonl"
    )


def _manifest(archive: Path) -> Path:
    return archive / "teams" / "codex-test" / "raw" / "source-manifest.json"


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
        archive / "teams" / "codex-test" / "source_snapshots",
        ROOT,
        "codex-test",
        "UTC",
    )
    assert [event.text for event in parsed.events] == ["Root done"]
    assert manifest["source_root"] == str(sessions.resolve())

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


def test_disappeared_source_fails_and_preserves_snapshot(tmp_path: Path) -> None:
    sessions, archive, origin = _first_ingest(tmp_path)
    copied_before = _snapshot(archive).read_bytes()
    manifest_before = _manifest(archive).read_bytes()
    origin.unlink()

    with pytest.raises(CodexParseError, match="previously observed rollout disappeared"):
        ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")

    assert _snapshot(archive).read_bytes() == copied_before
    assert _manifest(archive).read_bytes() == manifest_before


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


def test_orphan_snapshot_is_excluded_from_manifest_bound_parse(tmp_path: Path) -> None:
    sessions, archive, _ = _first_ingest(tmp_path)
    orphan = (
        archive
        / "teams"
        / "codex-test"
        / "source_snapshots"
        / "orphan"
        / "rollout-child.jsonl"
    )
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
    (archive / ".agent-team-timeline.json").write_text(
        '{"schema_version":1,"tool":"agent-team-timeline"}\n', encoding="utf-8"
    )
    snapshot_root = archive / "teams" / "codex-test" / "source_snapshots"
    outside = tmp_path / "outside"
    outside.mkdir()
    if symlink_level == "root":
        snapshot_root.parent.mkdir(parents=True)
        snapshot_root.symlink_to(outside, target_is_directory=True)
    else:
        snapshot_root.mkdir(parents=True)
        (snapshot_root / "2026").symlink_to(outside, target_is_directory=True)

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
        archive / "teams" / "codex-test" / "source_snapshots",
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
