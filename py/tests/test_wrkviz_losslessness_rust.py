"""Differential: Rust `wrkviz audit-losslessness` vs the oracle.

One archive ingested by the Python oracle from hand-built Codex
rollouts exercises every rule in the table at least once: retained,
payload, state, duplicate, redacted, partial and dropped rows, an
imported prefix, a superseded output, and an unbounded session.
Audit is read-only, so both editions run against the same fixture
and stdout/stderr/exit must agree byte-for-byte. Mutations of a
copy of the fixture then exercise the failure paths: unaccounted
rows, unverified claims, payload and source problems, and the
skipped-team reports.
"""

from __future__ import annotations

import hashlib
import json
import shutil
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

import pytest

from wrkviz.build_store import team_build_root
from wrkviz.pipeline import ingest_codex
from tests.timeline_snapshots import snapshot_root

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkviz"

ROOT = "root-thread"
CHILD = "child-thread"
LOST = "lost-thread"
COMMAND = json.dumps({"cmd": ["bash", "-lc", "make validate"], "workdir": "/work/project"})
STDOUT = "ok\n" * 400 + "warning: /home/private/customer/project is not a repository\n"


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


def _iso(seconds: float) -> str:
    return datetime.fromtimestamp(seconds, timezone.utc).isoformat().replace("+00:00", "Z")


def _line(timestamp: float, kind: str, payload: dict[str, object]) -> bytes:
    record: dict[str, object] = {
        "timestamp": _iso(timestamp),
        "type": kind,
        "payload": payload,
    }
    return json.dumps(record, sort_keys=True, separators=(",", ":")).encode("utf-8") + b"\n"


def _root_bytes() -> bytes:
    return b"".join(
        (
            _line(1_000, "session_meta", {
                "id": ROOT, "session_id": ROOT, "timestamp": _iso(1_000),
                "cwd": "/home/private/customer/project",
                "git": {"repository_url": "git@github.com:example-user/dev-widget.git"},
                "source": "cli",
            }),
            _line(1_000.5, "turn_context", {"turn_id": "root-turn"}),
            _line(1_001, "event_msg",
                  {"type": "task_started", "turn_id": "root-turn", "started_at": 1_001}),
            _line(1_001.5, "event_msg",
                  {"type": "user_message", "message": "Please run the tests"}),
            _line(1_002, "response_item", {
                "type": "message", "id": "root-answer", "role": "assistant",
                "content": [{"type": "output_text", "text": "Root done"}],
            }),
            _line(1_002.1, "response_item", {
                "type": "message", "id": "dev-note", "role": "developer",
                "content": [{"type": "output_text", "text": "system prompt"}],
            }),
            _line(1_002.2, "response_item", {
                "type": "custom_tool_call", "id": "tool-item", "call_id": "call-exec",
                "name": "exec", "input": COMMAND,
            }),
            _line(1_002.3, "response_item", {
                "type": "custom_tool_call_output", "call_id": "call-exec",
                "output": "5/20 verified",
            }),
            _line(1_002.4, "response_item", {
                "type": "custom_tool_call_output", "call_id": "call-exec",
                "output": STDOUT,
            }),
            _line(1_002.5, "response_item", {
                "type": "function_call", "id": "fn-item", "call_id": "call-fn",
                "name": "lookup", "arguments": "{\"x\":1}",
            }),
            _line(1_002.6, "response_item", {
                "type": "function_call_output", "call_id": "call-fn",
                "output": "fn result",
            }),
            _line(1_002.7, "event_msg",
                  {"type": "agent_message", "message": "structured verdict"}),
            _line(1_002.8, "response_item", {
                "type": "agent_message", "author": "/root", "recipient": "/root/worker",
                "content": [{"type": "output_text", "text": "please check"}],
            }),
            _line(1_002.9, "event_msg", {
                "type": "sub_agent_activity", "agent_path": "/root/worker",
                "agent_thread_id": CHILD, "event_id": "act-1", "kind": "started",
                "occurred_at_ms": 1_003_000,
            }),
            _line(1_003, "event_msg", {"type": "context_compacted"}),
            _line(1_003.1, "event_msg", {
                "type": "thread_goal_updated",
                "goal": {"status": "active", "objective": "ship the audit"},
            }),
            _line(1_003.2, "event_msg", {
                "type": "item_completed",
                "item": {"id": "root-answer", "type": "AgentMessage"},
            }),
            _line(1_003.3, "event_msg", {
                "type": "item_completed", "turn_id": "root-turn",
                "item": {"id": "exec-9", "type": "CommandExecution",
                         "command": "make validate", "aggregated_output": STDOUT},
            }),
            _line(1_003.4, "event_msg", {
                "type": "item_completed", "turn_id": "root-turn",
                "item": {"id": "fc-1", "type": "FileChange",
                         "changes": [{"path": "a.py", "kind": "add"}]},
            }),
            _line(1_003.5, "event_msg", {
                "type": "item_completed", "turn_id": "root-turn",
                "item": {"id": "um-1", "type": "UserMessage"},
            }),
            _line(1_003.6, "event_msg",
                  {"type": "token_count", "total_tokens": 42}),
            _line(1_003.7, "event_msg", {
                "type": "patch_apply_end", "call_id": "exec-9",
                "stdout": "applied", "changes": {"a.py": {"type": "add"}},
            }),
            _line(1_003.8, "response_item", {
                "type": "reasoning", "id": "rs-1", "summary": [],
                "encrypted_content": "ciphertext",
            }),
            _line(1_003.9, "event_msg",
                  {"type": "thread_settings_applied", "model": "example-model"}),
            _line(1_004, "event_msg",
                  {"type": "mcp_tool_call_end", "call_id": "mcp-1"}),
            _line(1_004.1, "world_state", {"instructions": "be helpful"}),
            _line(1_004.2, "compacted", {"replacement": "earlier history"}),
            _line(1_004.3, "inter_agent_communication_metadata", {"trigger": True}),
            _line(1_005, "event_msg", {
                "type": "task_complete", "turn_id": "root-turn",
                "started_at": 1_001, "completed_at": 1_005,
            }),
            _line(1_006, "event_msg",
                  {"type": "task_started", "turn_id": "root-turn-2", "started_at": 1_006}),
            _line(1_007, "event_msg",
                  {"type": "turn_aborted", "turn_id": "root-turn-2"}),
        )
    )


def _child_bytes() -> bytes:
    return b"".join(
        (
            _line(1_010, "session_meta", {
                "id": CHILD, "session_id": ROOT, "parent_thread_id": ROOT,
                "timestamp": _iso(1_010), "agent_path": "/root/worker",
                "source": {"subagent": {"thread_spawn": {
                    "parent_thread_id": ROOT, "agent_path": "/root/worker", "depth": 1,
                }}},
            }),
            # Imported: the parent's turn, replayed before the child's own work.
            _line(1_001, "event_msg",
                  {"type": "task_started", "turn_id": "root-turn", "started_at": 1_001}),
            _line(1_011, "event_msg",
                  {"type": "task_started", "turn_id": "child-turn", "started_at": 1_011}),
            _line(1_012, "response_item", {
                "type": "message", "id": "child-answer", "role": "assistant",
                "content": [{"type": "output_text", "text": "Child done"}],
            }),
            _line(1_013, "event_msg",
                  {"type": "token_count", "total_tokens": 7}),
        )
    )


def _lost_bytes() -> bytes:
    # A subagent session with no canonical boundary: no incoming
    # agent_message, and its only turn predates its own timestamp.
    return b"".join(
        (
            _line(2_000, "session_meta", {
                "id": LOST, "session_id": ROOT, "parent_thread_id": ROOT,
                "timestamp": _iso(2_000), "agent_path": "/root/lost",
                "source": {"subagent": {"thread_spawn": {
                    "parent_thread_id": ROOT, "agent_path": "/root/lost", "depth": 1,
                }}},
            }),
            _line(1_500, "event_msg",
                  {"type": "task_started", "turn_id": "lost-turn", "started_at": 1_500}),
            _line(1_501, "response_item", {
                "type": "message", "id": "lost-answer", "role": "assistant",
                "content": [{"type": "output_text", "text": "Lost done"}],
            }),
            _line(1_502, "event_msg", {
                "type": "task_complete", "turn_id": "lost-turn",
                "started_at": 1_500, "completed_at": 1_502,
            }),
        )
    )


def _archive(tmp_path: Path) -> Path:
    sessions = tmp_path / "sessions"
    archive = tmp_path / "archive"
    day = sessions / "2026" / "08" / "05"
    day.mkdir(parents=True)
    (day / "rollout-root.jsonl").write_bytes(_root_bytes())
    (day / "rollout-child.jsonl").write_bytes(_child_bytes())
    (day / "rollout-lost.jsonl").write_bytes(_lost_bytes())
    ingest_codex(archive, sessions, ROOT, "codex-test", "UTC")
    return archive


def _run(command: list[str], args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command + args, cwd=AU_ROOT, capture_output=True, text=True,
        check=False, timeout=300,
    )


def _both(args: list[str], expect: int = 0) -> dict[str, subprocess.CompletedProcess[str]]:
    outcomes = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(RUST_WRKVIZ)])):
        outcomes[label] = _run(command, ["audit-losslessness", *args])
    oracle, result = outcomes["py"], outcomes["rs"]
    assert oracle.returncode == expect, oracle.stderr
    assert result.returncode == oracle.returncode
    assert result.stdout == oracle.stdout
    assert result.stderr == oracle.stderr
    return outcomes


def _copy(tmp_path: Path, archive: Path, name: str) -> Path:
    # Copy the whole fixture parent: the build and snapshot stores
    # live beside the archive and resolution follows the copy.
    target_parent = tmp_path / name
    shutil.copytree(archive.parent, target_parent)
    return target_parent / archive.name


def test_audit_losslessness_text_and_json(tmp_path: Path, rust_wrkviz: Path) -> None:
    archive = _archive(tmp_path)
    outcomes = _both(["--output", str(archive)])
    assert "AUDIT FAILED" not in outcomes["py"].stdout  # sound, but lossy
    assert "sound: yes; lossless: no" in outcomes["py"].stdout
    for name in ("session-metadata", "tool-call", "tool-output",
                 "superseded-tool-output", "unbounded-session",
                 "imported-turn-prefix", "item-completed-command",
                 "token-count", "non-assistant-message"):
        assert name in outcomes["py"].stdout
    outcomes = _both(["--output", str(archive), "--format", "json"])
    report = json.loads(outcomes["py"].stdout)
    (team,) = report["teams"]
    assert report["sound"] is True and report["lossless"] is False
    assert team["covered"] is True and team["vendor_files"] == 3
    assert team["absent_rows"] > 0 and team["partial_rows"] == 1
    assert team["unaccounted_rows"] == 0 and team["unverified_rows"] == 0
    # Sound but lossy: the deletion gate refuses.
    _both(["--output", str(archive), "--require-lossless"], expect=1)


def test_audit_losslessness_selection_and_skips(tmp_path: Path, rust_wrkviz: Path) -> None:
    archive = _archive(tmp_path)
    _both(["--output", str(archive), "--team", "codex-test"])
    outcomes = _both(["--output", str(archive), "--team", "ghost-team"])
    assert "not covered: the team has no raw/source-manifest.json" in outcomes["py"].stdout
    other = team_build_root(archive, "claude-test") / "raw"
    other.mkdir(parents=True)
    (other / "source-manifest.json").write_text(
        json.dumps({"provider": "claude"}) + "\n")
    outcomes = _both(["--output", str(archive), "--team", "claude-test"])
    assert "no rule table exists for provider 'claude'" in outcomes["py"].stdout
    _both(["--output", str(archive), "--team", "codex-test", "--team", "claude-test"])
    shutil.rmtree(snapshot_root(archive, "codex-test"))
    outcomes = _both(["--output", str(archive), "--team", "codex-test"])
    assert "vendor snapshots are already absent" in outcomes["py"].stdout


def test_audit_losslessness_findings(tmp_path: Path, rust_wrkviz: Path) -> None:
    archive = _archive(tmp_path / "base")
    manifest_path = team_build_root(archive, "codex-test") / "raw" / "source-manifest.json"
    manifest = json.loads(manifest_path.read_text())

    # An unknown row shape is unaccounted, not silently dropped.
    unknown = _copy(tmp_path, archive, "unknown")
    root_file = snapshot_root(unknown, "codex-test") / "2026/08/05/rollout-root.jsonl"
    extra = _line(1_008, "vendor_new_thing", {"type": "surprise"})
    with root_file.open("ab") as handle:
        handle.write(extra)
    raw = root_file.read_bytes()
    for entry in manifest["sources"]:
        if entry["snapshot_path"].endswith("rollout-root.jsonl"):
            entry["copied_bytes"] = len(raw)
            entry["sha256"] = hashlib.sha256(raw).hexdigest()
    (team_build_root(unknown, "codex-test") / "raw" / "source-manifest.json").write_text(
        json.dumps(manifest) + "\n")
    outcomes = _both(["--output", str(unknown)], expect=1)
    assert "UNACCOUNTED" in outcomes["py"].stdout

    # A recorded rollout that is gone is a source problem.
    deleted = _copy(tmp_path, archive, "deleted")
    (snapshot_root(deleted, "codex-test") / "2026/08/05/rollout-child.jsonl").unlink()
    outcomes = _both(["--output", str(deleted)], expect=1)
    assert "recorded in raw/source-manifest.json but absent" in outcomes["py"].stdout

    # A truncated rollout is caught by its recorded digest.
    truncated = _copy(tmp_path, archive, "truncated")
    victim = snapshot_root(truncated, "codex-test") / "2026/08/05/rollout-root.jsonl"
    kept = victim.read_bytes().split(b"\n")[:3]
    victim.write_bytes(b"\n".join(kept) + b"\n")
    outcomes = _both(["--output", str(truncated)], expect=1)
    assert "vendor snapshots:" in outcomes["py"].stdout

    # Tampered payload text is a payload problem and an unverified claim.
    tampered = _copy(tmp_path, archive, "tampered")
    payload_root = team_build_root(tampered, "codex-test") / "payloads"
    shard = next(path for path in payload_root.iterdir() if path.suffix == ".jsonl")
    lines = shard.read_text().split("\n")
    record = json.loads(lines[0])
    record["text"] = "forged output"
    lines[0] = json.dumps(record, sort_keys=True, separators=(",", ":"))
    shard.write_text("\n".join(lines))
    outcomes = _both(["--output", str(tampered)], expect=1)
    assert "payload store:" in outcomes["py"].stdout

    # A negative copied_bytes slices from the end in the oracle;
    # the digest over that prefix is not a source problem.
    negative = _copy(tmp_path, archive, "negative")
    child_file = snapshot_root(negative, "codex-test") / "2026/08/05/rollout-child.jsonl"
    child_raw = child_file.read_bytes()
    manifest_copy = json.loads(
        (team_build_root(negative, "codex-test") / "raw" / "source-manifest.json"
         ).read_text())
    for entry in manifest_copy["sources"]:
        if entry["snapshot_path"].endswith("rollout-child.jsonl"):
            entry["copied_bytes"] = -1
            entry["sha256"] = hashlib.sha256(child_raw[:-1]).hexdigest()
    (team_build_root(negative, "codex-test") / "raw" / "source-manifest.json"
     ).write_text(json.dumps(manifest_copy) + "\n")
    _both(["--output", str(negative)], expect=0)

    # A CRLF shard reads back translated in the oracle, so a
    # manifest digest over the LF form still verifies.
    crlf = _copy(tmp_path, archive, "crlf")
    shard = next(
        path for path in (team_build_root(crlf, "codex-test") / "payloads").iterdir()
        if path.suffix == ".jsonl")
    shard.write_bytes(shard.read_bytes().replace(b"\n", b"\r\n"))
    outcomes = _both(["--output", str(crlf)], expect=0)
    assert "payload store:" not in outcomes["py"].stdout


def test_audit_losslessness_refusals(tmp_path: Path, rust_wrkviz: Path) -> None:
    archive = _archive(tmp_path)
    _both(["--output", str(archive), "--team", "codex-test",
           "--team", "codex-test"], expect=2)
    _both(["--output", str(tmp_path / "missing")], expect=2)
    _both(["--output", str(archive), "--team", "Bad Slug"], expect=2)


def test_audit_losslessness_help(rust_wrkviz: Path) -> None:
    oracle = _run([sys.executable, str(PYTHON_WRKVIZ)],
                  ["audit-losslessness", "--help"])
    result = _run([str(RUST_WRKVIZ)], ["audit-losslessness", "--help"])
    assert result.returncode == oracle.returncode == 0
    assert result.stdout == oracle.stdout
    assert result.stderr == oracle.stderr
