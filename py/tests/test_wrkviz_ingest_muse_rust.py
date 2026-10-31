"""Differential: Rust `wrkviz ingest-muse` against the Python oracle.

Both editions ingest the same synthetic Muse session
directory (root journal plus bound subagent journals) into
twin archives; stdout, stderr, exit code, and the file
trees must agree. Run records and manifests compare with
timestamps, run ids, and snapshot mtimes normalized. The
CPython-vs-serde JSON syntax wording is normalized (the
cross-slice named gap). Fixtures only; no live session dir.
"""

from __future__ import annotations

import json
import re
import subprocess
import sys
from pathlib import Path

import pytest

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkviz"
ROOT = "root-session"
CHILD = "child-session"
GRAND = "grand-session"


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


def _rec(
    seconds: float, payload_type: str, payload: dict[str, object]
) -> dict[str, object]:
    return {
        "recorded_at": int(seconds * 1_000_000),
        "payload_type": payload_type,
        "payload": payload,
    }


def _run_event(run_id: str, event: dict[str, object]) -> dict[str, object]:
    return {"kind": "run", "run_id": run_id, "event": event}


def _write(path: Path, records: list[dict[str, object]], tail: bytes = b"") -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    data = b"".join(
        json.dumps(item, sort_keys=True).encode() + b"\n" for item in records)
    path.write_bytes(data + tail)


def _root_records() -> list[dict[str, object]]:
    spawn_args = json.dumps({"role": "reviewer", "objective": "Review the widget"})
    return [
        _rec(1000, "runtime.user_intent.accepted", {
            "intent_id": "run-1",
            "model_messages": [{"content": [
                {"kind": "text", "text": "Build the thing"},
                {"kind": "text", "text": "Second part"}]}]}),
        _rec(1001, "runtime.session", _run_event("run-1", {"kind": "started"})),
        _rec(1002, "runtime.session", _run_event("run-1", {
            "kind": "assistant_message_committed", "text": "Working on it"})),
        _rec(1003, "runtime.session", _run_event("run-1", {
            "kind": "assistant_tool_calls_committed", "message_id": "batch-1",
            "tool_calls": [
                {"call_id": "call-spawn", "id": "item-1",
                 "name": "subagent_spawn", "args": spawn_args},
                {"call_id": "call-bash", "id": "item-2",
                 "name": "bash", "args": "ls -la"}]})),
        _rec(1004, "tool_batch.effect.started", {
            "record": {"call_id": "call-bash", "tool_name": "bash"}}),
        _rec(1005, "runtime.session", _run_event("run-1", {
            "kind": "tool_result_batch_committed", "batch_id": "batch-1",
            "results": [{"text": "spawned"}, {"text": "file.txt"}]})),
        _rec(1006, "tool_batch.effect.terminal", {
            "record": {"call_id": "call-bash",
                       "outcome": {"kind": "failed"}}}),
        _rec(1007, "subagent.control.child_session_bound", {
            "record": {"child_session_id": CHILD}}),
        _rec(1008, "subagent.control.start_attested", {
            "record": {"subagent_session_id": CHILD,
                       "dispatched_via_tool_call_id": "call-spawn"}}),
        _rec(1009, "runtime.session", _run_event("run-1", {
            "kind": "inbox_item_drained",
            "delivery_snapshot": {"body": "Background task finished"}})),
        _rec(1010, "runtime.session", _run_event("run-1", {
            "kind": "terminal", "terminal": "completed",
            "time_to_first_token_ms": 250})),
        _rec(1011, "runtime.session", {"kind": "diagnostics", "note": "ignored"}),
    ]


def _child_records() -> list[dict[str, object]]:
    return [
        _rec(1020, "runtime.session", _run_event("run-c1", {
            "kind": "started", "prompt": "Review the widget"})),
        _rec(1021, "runtime.session", _run_event("run-c1", {
            "kind": "assistant_message_committed", "text": "Child findings"})),
        _rec(1022, "subagent.control.child_session_bound", {
            "record": {"child_session_id": GRAND}}),
        _rec(1023, "runtime.session", _run_event("run-c1", {
            "kind": "terminal", "terminal": "completed"})),
    ]


def _grand_records() -> list[dict[str, object]]:
    return [
        _rec(1030, "runtime.session", _run_event("run-g1", {
            "kind": "started", "prompt": "Deep check"})),
        _rec(1031, "runtime.session", _run_event("run-g1", {
            "kind": "assistant_message_committed", "text": "Grand findings"})),
        _rec(1032, "runtime.session", _run_event("run-g1", {
            "kind": "terminal", "terminal": "completed"})),
    ]


def _session(root: Path) -> Path:
    session_dir = root / "upstream" / ROOT
    _write(session_dir / "session.jsonl", _root_records())
    _write(session_dir / "subagent" / CHILD / "session.jsonl", _child_records())
    _write(session_dir / "subagent" / GRAND / "session.jsonl", _grand_records())
    # An observer session the runtime never bound: left out.
    _write(session_dir / "subagent" / "observer-session" / "session.jsonl", [
        _rec(1040, "runtime.session", _run_event("run-o1", {
            "kind": "started", "prompt": "observer noise"}))])
    return session_dir


def _run(command: list[str], args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [*command, "ingest-muse", *args],
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
          second: bool = False, session_dir: Path | None = None,
         ) -> tuple[subprocess.CompletedProcess[str],
                    subprocess.CompletedProcess[str]]:
    if session_dir is None:
        session_dir = _session(tmp_path / "shared")
    py_root = tmp_path / "py"
    rs_root = tmp_path / "rs"
    shared_args = ["--team", "muse-team", "--session-dir", str(session_dir),
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


def test_muse_fresh_and_second_run(tmp_path: Path, rust_wrkviz: Path) -> None:
    oracle, result = _pair(tmp_path / "fresh", rust_wrkviz, [])
    assert oracle.returncode == 0
    assert "3 agents" in result.stdout
    _pair(tmp_path / "second", rust_wrkviz, [], second=True)


def test_muse_missing_session(tmp_path: Path, rust_wrkviz: Path) -> None:
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "muse-team",
            "--session-dir", str(tmp_path / "absent-session")])
        assert done.returncode == 2, (label, done.stderr)
        assert "not a regular file" in done.stderr


def test_muse_rewrite_refused(tmp_path: Path, rust_wrkviz: Path) -> None:
    session_dir = _session(tmp_path / "shared")
    target = session_dir / "session.jsonl"
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        args = ["--output", str(root / "archive"), "--team", "muse-team",
                "--session-dir", str(session_dir)]
        assert _run(cmd, args).returncode == 0
    data = bytearray(target.read_bytes())
    data[100] = ord("X") if data[100] != ord("X") else ord("Y")
    target.write_bytes(bytes(data))
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "muse-team",
            "--session-dir", str(session_dir)])
        assert done.returncode == 2, (label, done.stderr)
        assert "append-only" in done.stderr


def test_muse_malformed_record(tmp_path: Path, rust_wrkviz: Path) -> None:
    session_dir = tmp_path / "upstream" / ROOT
    path = session_dir / "session.jsonl"
    path.parent.mkdir(parents=True)
    path.write_bytes(
        b'{"recorded_at":1000000000,"payload_type":"runtime.session",'
        b'"payload":{"kind":"run","run_id":"r1","event":{"kind":"started",'
        b'"prompt":"hi"}}}\n{bad\n')
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "muse-team",
            "--session-dir", str(session_dir)])
        assert done.returncode == 2, (label, done.stderr)


def test_muse_upstream_retired_child(tmp_path: Path, rust_wrkviz: Path) -> None:
    session_dir = _session(tmp_path / "shared")
    grand = session_dir / "subagent" / GRAND / "session.jsonl"
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        args = ["--output", str(root / "archive"), "--team", "muse-team",
                "--session-dir", str(session_dir)]
        assert _run(cmd, args).returncode == 0
    grand.unlink()
    oracle = result = None
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "muse-team",
            "--session-dir", str(session_dir)])
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
        f"subagent/{GRAND}/session.jsonl"]


def test_muse_raw_utf8_text(tmp_path: Path, rust_wrkviz: Path) -> None:
    # Raw UTF-8 inside a record: continuation bytes are not
    # line boundaries for CPython `bytes.splitlines`.
    session_dir = tmp_path / "upstream" / ROOT
    path = session_dir / "session.jsonl"
    path.parent.mkdir(parents=True)
    records = [
        _rec(1000, "runtime.user_intent.accepted", {
            "intent_id": "run-1",
            "model_messages": [{"content": [
                {"kind": "text", "text": "Å café — naïve"}]}]}),
        _rec(1001, "runtime.session", _run_event("run-1", {
            "kind": "assistant_message_committed", "text": "Å done"})),
        _rec(1002, "runtime.session", _run_event("run-1", {
            "kind": "terminal", "terminal": "completed"})),
    ]
    path.write_bytes(b"".join(
        json.dumps(item, sort_keys=True, ensure_ascii=False).encode() + b"\n"
        for item in records))
    _pair(tmp_path / "utf8", rust_wrkviz, [], session_dir=session_dir)


def test_muse_window_and_identity(tmp_path: Path, rust_wrkviz: Path) -> None:
    _pair(tmp_path, rust_wrkviz, [
        "--project", "Widget=https://github.com/example/widget.git",
        "--source-host", "Host01.Example",
        "--timezone", "UTC",
        "--start-time", "1970-01-01T00:16:00Z",
        "--end-time", "1970-01-01T00:20:00Z",
    ])
