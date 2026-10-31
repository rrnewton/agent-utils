"""Differential: Rust `wrkviz ingest` (Codex) against the Python oracle.

Both editions ingest the same synthetic Codex sessions tree
into twin archives; stdout, stderr, exit code, and the file
trees must agree. Run records and manifests compare with
timestamps, run ids, and snapshot mtimes normalized. The
CPython-vs-serde JSON syntax wording is normalized (the
cross-slice named gap). Fixtures only; no live sessions tree.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

import pytest

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkviz"
ROOT = "root-thread"
CHILD = "child-thread"
NEXT = "next-thread"


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


def _iso(seconds: float) -> str:
    return datetime.fromtimestamp(seconds, timezone.utc).isoformat().replace(
        "+00:00", "Z")


def _record(ts: float, kind: str, payload: dict[str, object]) -> dict[str, object]:
    return {"timestamp": _iso(ts), "type": kind, "payload": payload}


def _write(path: Path, records: list[dict[str, object]], tail: bytes = b"") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    data = b"".join(
        json.dumps(item, sort_keys=True).encode() + b"\n" for item in records)
    path.write_bytes(data + tail)


def _root_records() -> list[dict[str, object]]:
    meta: dict[str, object] = {
        "id": ROOT, "session_id": ROOT, "timestamp": _iso(1000),
        "cwd": "/work/project", "source": "cli", "thread_source": "user",
        "git": {"repository_url": "https://github.com/example/widget.git"},
    }
    return [
        _record(1000, "session_meta", meta),
        _record(1010, "event_msg", {
            "type": "task_started", "turn_id": "root-turn", "started_at": 1010}),
        _record(1011, "response_item", {
            "type": "message", "id": "msg-user", "role": "user",
            "content": [{"type": "input_text", "text": "Build the thing"}]}),
        _record(1011, "event_msg", {
            "type": "user_message", "message": "Build the thing"}),
        _record(1012, "response_item", {
            "type": "message", "id": "msg-asst", "role": "assistant",
            "phase": "commentary",
            "content": [{"type": "output_text", "text": "Working on it"}]}),
        _record(1013, "response_item", {
            "type": "custom_tool_call", "id": "ct-exec", "call_id": "call-exec",
            "name": "exec",
            "input": "await tools.exec_command({}); await tools.apply_patch('x')"}),
        _record(1014, "response_item", {
            "type": "custom_tool_call_output", "id": "ctco-exec",
            "call_id": "call-exec", "output": "ok"}),
        _record(1015, "response_item", {
            "type": "function_call", "id": "fc-spawn", "call_id": "call-spawn",
            "namespace": "collaboration", "name": "spawn_agent",
            "arguments": json.dumps({
                "task_name": "worker", "message": "gAAAA-encrypted-spawn"})}),
        _record(1015.2, "event_msg", {
            "type": "sub_agent_activity", "event_id": "call-spawn",
            "occurred_at_ms": 1015200, "agent_thread_id": CHILD,
            "agent_path": "/root/worker", "kind": "started"}),
        _record(1015.3, "response_item", {
            "type": "function_call_output", "id": "fco-spawn",
            "call_id": "call-spawn", "output": '{"task_name":"/root/worker"}'}),
        _record(1025, "response_item", {
            "type": "agent_message", "id": "amsg-1", "author": "/root/worker",
            "recipient": "/root",
            "content": [
                {"type": "input_text", "text": "Message Type: MESSAGE\nPayload:\n"},
                {"type": "encrypted_content", "encrypted_content": "gAAAA-mid"}]}),
        _record(1050, "event_msg", {
            "type": "task_complete", "turn_id": "root-turn",
            "started_at": 1010, "completed_at": 1050,
            "time_to_first_token_ms": 12, "last_agent_message": "Root done"}),
    ]


def _child_records(root_meta: dict[str, object]) -> list[dict[str, object]]:
    meta: dict[str, object] = {
        "id": CHILD, "session_id": ROOT, "parent_thread_id": ROOT,
        "timestamp": _iso(1020), "cwd": "/work/project",
        "thread_source": "subagent", "agent_path": "/root/worker",
        "agent_nickname": "Ada",
        "source": {"subagent": {"thread_spawn": {
            "parent_thread_id": ROOT, "depth": 1,
            "agent_path": "/root/worker", "agent_nickname": "Ada",
            "agent_role": "reviewer"}}},
    }
    return [
        _record(1020, "session_meta", meta),
        _record(1020, "session_meta", root_meta),
        _record(1020, "response_item", {
            "type": "message", "id": "fork-user", "role": "user",
            "content": [{"type": "input_text", "text": "Build the thing"}]}),
        _record(1030, "event_msg", {
            "type": "task_started", "turn_id": "child-turn", "started_at": 1030}),
        _record(1031, "event_msg", {
            "type": "user_message", "message": "Review the widget"}),
        _record(1031, "response_item", {
            "type": "message", "id": "msg-child-user", "role": "user",
            "content": [{"type": "input_text", "text": "Review the widget"}]}),
        _record(1040, "event_msg", {
            "type": "task_complete", "turn_id": "child-turn",
            "started_at": 1030, "completed_at": 1040}),
    ]


def _sessions(root: Path, *, continuation: bool = False) -> Path:
    sessions = root / "sessions"
    payload = _root_records()[0]["payload"]
    assert isinstance(payload, dict)
    root_meta: dict[str, object] = payload
    _write(sessions / "2026/08/05/rollout-root.jsonl", _root_records(),
           tail=b'{"partial"')
    _write(sessions / "2026/08/05/rollout-child.jsonl",
           _child_records(root_meta))
    if continuation:
        meta: dict[str, object] = {
            "id": NEXT, "session_id": NEXT, "timestamp": _iso(2000),
            "cwd": "/work/project", "source": "cli", "thread_source": "user",
        }
        _write(sessions / "2026/08/06/rollout-next.jsonl", [
            _record(2000, "session_meta", meta),
            _record(2010, "event_msg", {
                "type": "task_started", "turn_id": "next-turn",
                "started_at": 2010}),
            _record(2011, "event_msg", {
                "type": "user_message", "message": "Continue please"}),
            _record(2050, "event_msg", {
                "type": "task_complete", "turn_id": "next-turn",
                "started_at": 2010, "completed_at": 2050}),
        ])
    return sessions


def _run(command: list[str], args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [*command, "ingest", *args],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=120,
    )


def _normalize(text: str, root: Path) -> str:
    text = text.replace(str(root), "ROOT")
    text = re.sub(r"runs/[0-9A-Za-z-]+\.json", "runs/RUN.json", text)
    text = re.sub(r"invalid JSON: .*", "invalid JSON: <syntax>", text)
    return text


def _scrub(value: object, root: Path) -> object:
    if isinstance(value, dict):
        out: dict[str, object] = {}
        for key, item in value.items():
            if key in ("run_id", "started_at", "completed_at", "created_at",
                       "last_run_at", "last_run_id", "updated_at", "mtime_ns"):
                out[key] = "<ts>"
            elif key == "command" and isinstance(item, list):
                out[key] = [
                    ("ROOT" if str(arg) == str(root) or str(arg).startswith(str(root))
                     else arg)
                    for arg in item
                ]
            else:
                out[key] = _scrub(item, root)
        return out
    if isinstance(value, list):
        return [_scrub(item, root) for item in value]
    if isinstance(value, str):
        return value.replace(str(root), "ROOT")
    return value


def _tree(root: Path) -> dict[str, object]:
    result: dict[str, object] = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(root).as_posix()
        if rel.startswith("runs/"):
            rel = "runs/RUN.json"
        if path.suffix == ".json":
            result[rel] = _scrub(json.loads(path.read_text()), root)
        else:
            result[rel] = path.read_bytes()
    return result


def _archives(root: Path) -> dict[str, object]:
    combined: dict[str, object] = {}
    for name in ("archive", "archive.build", "archive.sources"):
        for rel, value in _tree(root / name).items():
            combined[f"{name}/{rel}"] = value
    return combined


def _pair(tmp_path: Path, rust_wrkviz: Path, extra: list[str], *,
          continuation: bool = False, second: bool = False,
          sessions: Path | None = None) -> tuple[subprocess.CompletedProcess[str],
                                                 subprocess.CompletedProcess[str]]:
    if sessions is None:
        sessions = _sessions(tmp_path / "shared", continuation=continuation)
    py_root = tmp_path / "py"
    rs_root = tmp_path / "rs"
    shared_args = ["--team", "codex-team", "--sessions-root", str(sessions),
                   "--root-session", ROOT, *extra]
    oracle = _run([sys.executable, str(PYTHON_WRKVIZ)],
                  ["--output", str(py_root / "archive"), *shared_args])
    result = _run([str(rust_wrkviz)],
                  ["--output", str(rs_root / "archive"), *shared_args])
    if second:
        oracle = _run([sys.executable, str(PYTHON_WRKVIZ)],
                      ["--output", str(py_root / "archive"), *shared_args])
        result = _run([str(rust_wrkviz)],
                      ["--output", str(rs_root / "archive"), *shared_args])
    assert result.returncode == oracle.returncode, (result.stderr, oracle.stderr)
    assert _normalize(result.stdout, rs_root) == _normalize(oracle.stdout, py_root)
    assert _normalize(result.stderr, rs_root) == _normalize(oracle.stderr, py_root)
    assert _archives(rs_root) == _archives(py_root)
    return oracle, result


def test_codex_fresh_and_second_run(tmp_path: Path, rust_wrkviz: Path) -> None:
    oracle, result = _pair(tmp_path / "fresh", rust_wrkviz, [])
    assert oracle.returncode == 0
    assert "2 agents, 5 messages/events, 2 outer tools, 1 edges" in result.stdout
    _pair(tmp_path / "second", rust_wrkviz, [], second=True)


def test_codex_continuation(tmp_path: Path, rust_wrkviz: Path) -> None:
    oracle, result = _pair(
        tmp_path, rust_wrkviz, ["--continuation-session", NEXT],
        continuation=True, second=True)
    assert oracle.returncode == result.returncode == 0


def test_codex_missing_root_and_sessions(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    sessions = _sessions(tmp_path / "shared")
    oracle, result = _pair(tmp_path / "missing", rust_wrkviz, [],
                           sessions=sessions)
    assert oracle.returncode == 0  # sanity: shared sessions work
    for label, root in (("py", tmp_path / "py2"), ("rs", tmp_path / "rs2")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "codex-team",
            "--sessions-root", str(sessions), "--root-session", "absent"])
        assert done.returncode == 2, (label, done.stderr)
        assert "were not found" in done.stderr


def test_codex_rewrite_refused(tmp_path: Path, rust_wrkviz: Path) -> None:
    sessions = _sessions(tmp_path / "shared")
    target = sessions / "2026/08/05/rollout-root.jsonl"
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        args = ["--output", str(root / "archive"), "--team", "codex-team",
                "--sessions-root", str(sessions), "--root-session", ROOT]
        assert _run(cmd, args).returncode == 0
    data = bytearray(target.read_bytes())
    data[100] = ord("X") if data[100] != ord("X") else ord("Y")
    target.write_bytes(bytes(data))
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "codex-team",
            "--sessions-root", str(sessions), "--root-session", ROOT])
        assert done.returncode == 2, (label, done.stderr)
        assert "append-only" in done.stderr


def test_codex_malformed_rollout(tmp_path: Path, rust_wrkviz: Path) -> None:
    sessions = tmp_path / "sessions"
    path = sessions / "2026/08/05/rollout-root.jsonl"
    path.parent.mkdir(parents=True)
    path.write_bytes(b'{"type":"session_meta","payload":{"id":"root-thread"}}\n{bad\n')
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "codex-team",
            "--sessions-root", str(sessions), "--root-session", ROOT])
        assert done.returncode == 2, (label, done.stderr)


def test_codex_upstream_retired_rollout(tmp_path: Path, rust_wrkviz: Path) -> None:
    sessions = _sessions(tmp_path / "shared")
    child = sessions / "2026/08/05/rollout-child.jsonl"
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        args = ["--output", str(root / "archive"), "--team", "codex-team",
                "--sessions-root", str(sessions), "--root-session", ROOT]
        assert _run(cmd, args).returncode == 0
    child.unlink()
    oracle = result = None
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "codex-team",
            "--sessions-root", str(sessions), "--root-session", ROOT])
        assert done.returncode == 0, (label, done.stderr)
        if label == "py":
            oracle = done
        else:
            result = done
    assert oracle is not None and result is not None
    assert _normalize(result.stdout, tmp_path / "rs") == _normalize(
        oracle.stdout, tmp_path / "py")
    runs = list((tmp_path / "rs" / "archive" / "runs").glob("*.json"))
    receipt = json.loads(sorted(runs)[-1].read_text())
    assert receipt["ingest"]["upstream_retired_sources"] == [
        "2026/08/05/rollout-child.jsonl"]


def test_codex_zstd_compressed_rollout(tmp_path: Path, rust_wrkviz: Path) -> None:
    import shutil
    if shutil.which("zstd") is None:
        pytest.skip("zstd command unavailable")
    sessions = _sessions(tmp_path / "shared")
    child = sessions / "2026/08/05/rollout-child.jsonl"
    subprocess.run(["zstd", "-q", "-f", str(child)], check=True)
    child.unlink()
    _pair(tmp_path / "zst", rust_wrkviz, [], sessions=sessions)


def test_codex_falsy_and_empty_content(tmp_path: Path, rust_wrkviz: Path) -> None:
    sessions = tmp_path / "sessions"
    meta: dict[str, object] = {
        "id": ROOT, "session_id": ROOT, "timestamp": _iso(1000),
        "cwd": " /work/widget ", "source": "cli", "thread_source": "user"}
    records = [
        _record(1000, "session_meta", meta),
        _record(1010, "event_msg", {
            "type": "task_started", "turn_id": "t1", "started_at": 1010}),
        _record(1011, "response_item", {
            "type": "message", "id": "", "role": "assistant",
            "content": [{"type": "input_text", "text": ""}]}),
        _record(1012, "response_item", {
            "type": "message", "id": "", "role": "assistant",
            "content": [{"type": "output_text", "text": "café note"}]}),
        _record(1013, "response_item", {
            "type": "custom_tool_call", "id": "ct", "call_id": "call-x",
            "name": "exec", "status": "", "input": "run"}),
        _record(1014, "response_item", {
            "type": "custom_tool_call_output", "id": "co", "call_id": "call-x",
            "output": {"note": "café"}}),
        _record(1050, "event_msg", {
            "type": "task_complete", "turn_id": "t1", "started_at": 1010,
            "completed_at": 1050, "error": "", "reason": "boom"}),
    ]
    _write(sessions / "2026/08/05/rollout-root.jsonl", records)
    _pair(tmp_path / "case", rust_wrkviz, [], sessions=sessions)


def test_codex_window_and_identity(tmp_path: Path, rust_wrkviz: Path) -> None:
    _pair(tmp_path, rust_wrkviz, [
        "--project", "Widget=https://github.com/example/widget.git",
        "--source-host", "Host01.Example",
        "--timezone", "UTC",
        "--start-time", "1970-01-01T00:16:00Z",
        "--end-time", "1970-01-01T00:20:00Z",
    ])
