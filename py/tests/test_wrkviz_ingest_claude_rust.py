"""Differential: Rust `wrkviz ingest-claude` against the Python oracle.

Both editions ingest the same synthetic Claude session
lineage (root transcript plus subagent sidecars) into twin
archives; stdout, stderr, exit code, and the file trees
must agree. Run records and manifests compare with
timestamps, run ids, and snapshot mtimes normalized. The
CPython-vs-serde JSON syntax wording is normalized (the
cross-slice named gap). Fixtures only; no live session tree.
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
ROOT = "root-session"
CHILD = "child1"


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


def _write(path: Path, records: list[dict[str, object]], tail: bytes = b"") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    data = b"".join(
        json.dumps(item, sort_keys=True).encode() + b"\n" for item in records)
    path.write_bytes(data + tail)


def _user(uuid: str, ts: float, content: object, **extra: object) -> dict[str, object]:
    record: dict[str, object] = {
        "type": "user", "uuid": uuid, "sessionId": ROOT,
        "timestamp": _iso(ts),
        "message": {"role": "user", "content": content},
    }
    record.update(extra)
    return record


def _assistant(
    uuid: str, ts: float, blocks: list[dict[str, object]], **extra: object
) -> dict[str, object]:
    message: dict[str, object] = {"role": "assistant", "content": blocks}
    message_extra = extra.pop("message_extra", {})
    assert isinstance(message_extra, dict)
    message.update(message_extra)
    record: dict[str, object] = {
        "type": "assistant", "uuid": uuid, "sessionId": ROOT,
        "timestamp": _iso(ts), "message": message,
    }
    record.update(extra)
    return record


def _root_records() -> list[dict[str, object]]:
    return [
        _user("u1", 1000, "Build the thing", origin={"kind": "human"}),
        _assistant("u2", 1010, [{"type": "text", "text": "Working on it"}]),
        _assistant("u3", 1020, [{
            "type": "tool_use", "id": "call-spawn", "name": "Agent",
            "input": {"description": "Review widget",
                      "prompt": "Review the widget please"}}]),
        _user("u4", 1030, [{
            "type": "tool_result", "tool_use_id": "call-spawn",
            "content": "spawned"}]),
        _assistant("u5", 1040, [{
            "type": "tool_use", "id": "call-msg", "name": "SendMessage",
            "input": {"recipient": CHILD, "message": "Please review"}}]),
        _user("u6", 1050, [{
            "type": "tool_result", "tool_use_id": "call-msg",
            "content": "sent"}]),
        _assistant("u7", 1060, [{"type": "text", "text": "Root done"}],
                   message_extra={"stop_reason": "end_turn"}),
        {"type": "attachment", "uuid": "u8", "sessionId": ROOT,
         "timestamp": _iso(900),
         "attachment": {"type": "queued_command", "prompt": "Queued thought",
                        "origin": {"kind": "human"}}},
    ]


def _child_records() -> list[dict[str, object]]:
    records: list[dict[str, object]] = []
    prompt = _user("c1", 1025, "Review the widget please")
    prompt["agentId"] = CHILD
    records.append(prompt)
    done = _assistant("c2", 1035, [{"type": "text", "text": "Child findings"}],
                      message_extra={"stop_reason": "end_turn"})
    done["agentId"] = CHILD
    records.append(done)
    call = _assistant("c3", 1045, [{
        "type": "tool_use", "id": "call-bash", "name": "Bash",
        "input": {"command": "ls"}}])
    call["agentId"] = CHILD
    records.append(call)
    result = _user("c4", 1055, [{
        "type": "tool_result", "tool_use_id": "call-bash",
        "content": "file.txt"}])
    result["agentId"] = CHILD
    records.append(result)
    return records


def _session(root: Path, *, fork: bool = False) -> Path:
    base = root / "upstream"
    session = base / f"{ROOT}.jsonl"
    _write(session, _root_records(), tail=b'{"partial"')
    child = base / ROOT / "subagents" / f"agent-{CHILD}.jsonl"
    records = _child_records()
    if fork:
        copies: list[dict[str, object]] = []
        for held in _root_records()[:2]:
            copy = dict(held)
            copy["agentId"] = CHILD
            copy["isSidechain"] = True
            copies.append(copy)
        records = [
            {"type": "fork-context-ref", "uuid": "c0", "sessionId": ROOT,
             "agentId": CHILD, "timestamp": _iso(1021)},
            *copies,
            *records,
        ]
    _write(child, records)
    meta = base / ROOT / "subagents" / f"agent-{CHILD}.meta.json"
    meta.write_text(json.dumps({
        "toolUseId": "call-spawn", "description": "Review widget",
        "agentType": "reviewer"}))
    return session


def _run(command: list[str], args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [*command, "ingest-claude", *args],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=120,
    )


def _normalize(text: str, root: Path) -> str:
    text = text.replace(str(root), "ROOT")
    text = re.sub(r"runs/[0-9A-Za-z-]+\.json", "runs/RUN.json", text)
    text = re.sub(r"invalid JSON: .*", "invalid JSON: <syntax>", text)
    text = re.sub(r"invalid JSON \(.*\)", "invalid JSON (<syntax>)", text)
    return text


def _scrub(value: object, root: Path) -> object:
    if isinstance(value, dict):
        out: dict[str, object] = {}
        for key, item in value.items():
            if key in ("run_id", "started_at", "completed_at", "created_at",
                       "last_run_at", "last_run_id", "updated_at", "mtime_ns",
                       "upstream_retired_at"):
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
          second: bool = False, session: Path | None = None,
          fork: bool = False) -> tuple[subprocess.CompletedProcess[str],
                                       subprocess.CompletedProcess[str]]:
    if session is None:
        session = _session(tmp_path / "shared", fork=fork)
    py_root = tmp_path / "py"
    rs_root = tmp_path / "rs"
    shared_args = ["--team", "claude-team", "--session-file", str(session),
                   *extra]
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


def test_claude_fresh_and_second_run(tmp_path: Path, rust_wrkviz: Path) -> None:
    oracle, result = _pair(tmp_path / "fresh", rust_wrkviz, [])
    assert oracle.returncode == 0
    assert "2 agents" in result.stdout
    _pair(tmp_path / "second", rust_wrkviz, [], second=True)


def test_claude_fork_prefix(tmp_path: Path, rust_wrkviz: Path) -> None:
    oracle, result = _pair(tmp_path, rust_wrkviz, [], fork=True, second=True)
    assert oracle.returncode == result.returncode == 0


def test_claude_missing_session(tmp_path: Path, rust_wrkviz: Path) -> None:
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "claude-team",
            "--session-file", str(tmp_path / "absent.jsonl")])
        assert done.returncode == 2, (label, done.stderr)
        assert "not a regular file" in done.stderr


def test_claude_rewrite_refused(tmp_path: Path, rust_wrkviz: Path) -> None:
    session = _session(tmp_path / "shared")
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        args = ["--output", str(root / "archive"), "--team", "claude-team",
                "--session-file", str(session)]
        assert _run(cmd, args).returncode == 0
    data = bytearray(session.read_bytes())
    data[100] = ord("X") if data[100] != ord("X") else ord("Y")
    session.write_bytes(bytes(data))
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "claude-team",
            "--session-file", str(session)])
        assert done.returncode == 2, (label, done.stderr)
        assert "append-only" in done.stderr


def test_claude_malformed_record(tmp_path: Path, rust_wrkviz: Path) -> None:
    session = tmp_path / "upstream" / f"{ROOT}.jsonl"
    session.parent.mkdir(parents=True)
    session.write_bytes(
        b'{"type":"user","uuid":"u1","sessionId":"root-session",'
        b'"timestamp":"2026-08-05T00:16:40Z","message":{"role":"user",'
        b'"content":"hi"}}\n{bad\n')
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "claude-team",
            "--session-file", str(session)])
        assert done.returncode == 2, (label, done.stderr)


def test_claude_upstream_retired_subagent(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    session = _session(tmp_path / "shared")
    child = session.parent / ROOT / "subagents" / f"agent-{CHILD}.jsonl"
    meta = session.parent / ROOT / "subagents" / f"agent-{CHILD}.meta.json"
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        args = ["--output", str(root / "archive"), "--team", "claude-team",
                "--session-file", str(session)]
        assert _run(cmd, args).returncode == 0
    child.unlink()
    meta.unlink()
    oracle = result = None
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "claude-team",
            "--session-file", str(session)])
        assert done.returncode == 0, (label, done.stderr)
        if label == "py":
            oracle = done
        else:
            result = done
    assert oracle is not None and result is not None
    assert _normalize(result.stdout, tmp_path / "rs") == _normalize(
        oracle.stdout, tmp_path / "py")
    assert _archives(tmp_path / "rs") == _archives(tmp_path / "py")
    runs = list((tmp_path / "rs" / "archive" / "runs").glob("*.json"))
    receipt = json.loads(sorted(runs)[-1].read_text())
    assert receipt["ingest"]["upstream_retired_sources"] == [
        f"{ROOT}/subagents/agent-{CHILD}.jsonl",
        f"{ROOT}/subagents/agent-{CHILD}.meta.json",
    ]


def test_claude_conflicting_duplicate_uuid(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    session = tmp_path / "upstream" / f"{ROOT}.jsonl"
    _write(session, [
        _user("u1", 1000, "first"),
        _user("u1", 1010, "second"),
    ])
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "claude-team",
            "--session-file", str(session)])
        assert done.returncode == 2, (label, done.stderr)
        assert "conflicting duplicate UUID" in done.stderr


def test_claude_window_and_identity(tmp_path: Path, rust_wrkviz: Path) -> None:
    _pair(tmp_path, rust_wrkviz, [
        "--project", "Widget=https://github.com/example/widget.git",
        "--source-host", "Host01.Example",
        "--timezone", "UTC",
        "--start-time", "1970-01-01T00:16:00Z",
        "--end-time", "1970-01-01T00:20:00Z",
    ])


def test_claude_provenance_and_timestamp_edges(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    session = tmp_path / "upstream" / f"{ROOT}.jsonl"
    _write(session, [
        # Same-millisecond prompts: turn attribution follows
        # the first prompt at that instant.
        _user("u1", 1000, "one"),
        _user("u2", 1000, "two"),
        _assistant("u3", 1000, [{
            "type": "tool_use", "id": "call-tie", "name": "Bash",
            "input": {"command": "true"}}]),
        # A control-only prompt strips to nothing; a
        # seconds-free timestamp still parses.
        _user("u4", 1010, "\u001c"),
        {"type": "user", "uuid": "u5", "sessionId": ROOT,
         "timestamp": "1970-01-01T00:17:40Z",
         "message": {"role": "user", "content": "seconds-free"}},
        _assistant("u6", 1070, [{"type": "text", "text": "done"}],
                   message_extra={"stop_reason": "end_turn"}),
    ])
    _pair(tmp_path / "edges", rust_wrkviz, [], session=session)


def test_claude_zstd_compressed_subagent(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    import shutil
    if shutil.which("zstd") is None:
        pytest.skip("zstd command unavailable")
    session = _session(tmp_path / "shared")
    child = session.parent / ROOT / "subagents" / f"agent-{CHILD}.jsonl"
    subprocess.run(["zstd", "-q", "-f", str(child)], check=True)
    child.unlink()
    _pair(tmp_path / "zst", rust_wrkviz, [], session=session)
