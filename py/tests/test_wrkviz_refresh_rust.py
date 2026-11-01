"""Differential: Rust `wrkviz refresh{,-claude,-orc}` vs the oracle.

Twin sources are refreshed end-to-end (ingest + heuristic summarize +
build, one run receipt) by each edition. stdout, stderr, exit code,
and both archive trees must agree with timestamps, run ids, and
zlib-generation bytes normalized. Fixtures only.
"""

from __future__ import annotations

import json
import os
import re
import sqlite3
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

import pytest

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkvizd"
CODEX_ROOT = "root-thread"
CLAUDE_ROOT = "11111111-2222-3333-4444-555555555555"
CLAUDE_CHILD = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
ORC_ROOT = "11111111-2222-3333-4444-555555555555"
ORC_CHILD = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
T0 = 1767225600000

TS_KEYS = {
    "edition", "run_id", "started_at", "completed_at", "created_at", "last_run_at",
    "last_run_id", "updated_at", "generated_at", "pid", "fetched_at",
    "mtime_ns", "upstream_retired_at",
}
GEN_KEYS = {"c_bytes", "c_size", "c_sha256", "c_off", "c_len",
            "shard_object_bytes", "shard_transfer_bytes",
            "bootstrap_bytes", "bootstrap_transfer_bytes"}


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


def _run(command: list[str], args: list[str],
         env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    full_env = dict(os.environ)
    if env:
        full_env.update(env)
    return subprocess.run(
        command + args, cwd=AU_ROOT, capture_output=True, text=True,
        check=False, timeout=600, env=full_env,
    )


def _normalize(text: str, archive: Path) -> str:
    text = text.replace(str(archive), "<ARCHIVE>")
    text = text.replace(str(archive.parent), "<BASE>")
    text = re.sub(r"runs/[^\s]+\.json", "runs/<RUN>.json", text)
    return re.sub(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z", "<TS>", text)


ID_KEYS = {"usage_receipt_id", "receipt_id", "batch_receipt_ids",
           "artifact_receipt_ids"}


def _scrub(value: object) -> object:
    if isinstance(value, dict):
        return {
            key: ("<GEN>" if key in GEN_KEYS else
                  "<TS>" if key in TS_KEYS else
                  "<ID>" if key in ID_KEYS else
                  "<RUN>" if key == "command" and isinstance(item, list) else
                  _scrub(item))
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [_scrub(item) for item in value]
    if isinstance(value, str):
        value = re.sub(r"(py|rs)-out", "OUT", value)
        return re.sub(
            r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z", "<TS>", value)
    return value


def _decoded(path: Path, raw: bytes) -> object:
    import gzip
    if path.suffix == ".gz":
        raw = gzip.decompress(raw)
    try:
        return _scrub(json.loads(raw))
    except (ValueError, UnicodeDecodeError):
        text = raw.decode("utf-8", "replace")
        text = re.sub(r"[0-9a-f]{64}", "<HEX>", text)
        for key in sorted(GEN_KEYS):
            text = re.sub(rf'"{key}": ?\d+', f'"{key}": <GEN>', text)
        return text


def _tree(root: Path) -> dict[str, object]:
    out: dict[str, object] = {}
    for path in sorted(root.rglob("*")):
        if path.is_dir() or path.name == ".wrkviz.lock":
            continue
        rel = f"{root.name}/{path.relative_to(root)}"
        if "runs" in path.parts:
            rel = f"{root.name}/runs/<RUN>.json"
        out[rel] = _decoded(path, path.read_bytes())
    return out


def _both(archives: dict[str, Path], verb: str, args: list[str],
          env: dict[str, str] | None = None, expect: int = 0,
          ) -> dict[str, subprocess.CompletedProcess[str]]:
    outcomes = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(RUST_WRKVIZ)])):
        outcomes[label] = _run(command, [
            verb, "--output", str(archives[label]), "--team", "widget", *args,
        ], env=env)
    oracle, result = outcomes["py"], outcomes["rs"]
    assert oracle.returncode == expect, oracle.stderr
    assert result.returncode == oracle.returncode
    assert _normalize(result.stdout, archives["rs"]) == _normalize(
        oracle.stdout, archives["py"])
    assert _normalize(result.stderr, archives["rs"]) == _normalize(
        oracle.stderr, archives["py"])
    assert _tree(archives["rs"].parent) == _tree(archives["py"].parent)
    return outcomes




def _claude_write(path: Path, records: list[dict[str, object]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"".join(
        json.dumps(item, sort_keys=True).encode() + b"\n" for item in records))


def _claude_user(uuid: str, ts: float, content: object, **extra: object) -> dict[str, object]:
    record: dict[str, object] = {
        "type": "user", "uuid": uuid, "sessionId": CLAUDE_ROOT,
        "timestamp": _iso(ts),
        "message": {"role": "user", "content": content},
    }
    record.update(extra)
    return record


def _claude_assistant(uuid: str, ts: float, blocks: list[dict[str, object]],
                      **extra: object) -> dict[str, object]:
    message: dict[str, object] = {"role": "assistant", "content": blocks}
    message_extra = extra.pop("message_extra", {})
    assert isinstance(message_extra, dict)
    message.update(message_extra)
    record: dict[str, object] = {
        "type": "assistant", "uuid": uuid, "sessionId": CLAUDE_ROOT,
        "timestamp": _iso(ts), "message": message,
    }
    record.update(extra)
    return record


def _claude_root_records() -> list[dict[str, object]]:
    return [
        _claude_user("u1", 1000, "Build the thing", origin={"kind": "human"}),
        _claude_assistant("u2", 1010, [{"type": "text", "text": "Working on it"}]),
        _claude_assistant("u3", 1020, [{
            "type": "tool_use", "id": "call-spawn", "name": "Agent",
            "input": {"description": "Review widget",
                      "prompt": "Review the widget please"}}]),
        _claude_user("u4", 1030, [{
            "type": "tool_result", "tool_use_id": "call-spawn",
            "content": "spawned"}]),
        _claude_assistant("u5", 1040, [{"type": "text", "text": "Root done"}],
                          message_extra={"stop_reason": "end_turn"}),
    ]


def _claude_session(root: Path) -> Path:
    session = root / "upstream" / f"{CLAUDE_ROOT}.jsonl"
    _claude_write(session, _claude_root_records())
    return session




def _orc_message(mid: int, role: str, ts: int, text: str) -> str:
    return json.dumps({
        "id": f"m{mid}",
        "role": role,
        "created_at_ms": ts,
        "source": "",
        "blocks": [{"type": "text", "id": mid * 10, "text": text}],
    }, sort_keys=True)


def _orc_session_db(path: Path, session_id: str, parent_id: str | None,
                    name: str, db_name: str | None, created_iso: str,
                    messages: list[tuple[int, str, int, str]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        path.unlink()
    con = sqlite3.connect(path)
    con.execute("CREATE TABLE session_meta (id TEXT, parent_id TEXT, name TEXT, "
                "db_name TEXT, created_at TEXT, updated_at TEXT)")
    con.execute("CREATE TABLE content_blocks (rowid INTEGER PRIMARY KEY, id TEXT, "
                "message_id TEXT, session_id TEXT, block_index INTEGER, "
                "created_at_ms INTEGER, turn_index INTEGER, role TEXT, "
                "block_type TEXT, content TEXT, searchable_text TEXT, "
                "code_input TEXT, code_output TEXT, code_exit_code INTEGER, "
                "model TEXT, user_source TEXT, token_count INTEGER, extra TEXT)")
    con.execute("CREATE TABLE messages (id INTEGER PRIMARY KEY, session_id TEXT, "
                "role TEXT, created_at_ms INTEGER, message_json TEXT, "
                "search_text TEXT)")
    con.execute("CREATE TABLE schema_version (id INTEGER PRIMARY KEY, "
                "version INTEGER, updated_at TEXT)")
    con.execute("INSERT INTO schema_version VALUES (1, 3, '2026-01-01T00:00:00Z')")
    con.execute("INSERT INTO session_meta VALUES (?,?,?,?,?,?)",
                (session_id, parent_id, name, db_name, created_iso, created_iso))
    for mid, role, ts, text in messages:
        con.execute("INSERT INTO messages VALUES (?,?,?,?,?,?)",
                    (mid, session_id, role, ts,
                     _orc_message(mid, role, ts, text), text))
    con.commit()
    con.close()


def _orc_task_db(path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    if path.exists():
        path.unlink()
    con = sqlite3.connect(path)
    con.execute("CREATE TABLE tasks (local_id TEXT PRIMARY KEY, owner TEXT, "
                "title TEXT)")
    con.execute("CREATE TABLE task_notes (id INTEGER PRIMARY KEY AUTOINCREMENT, "
                "task_id TEXT, content TEXT, created_at TEXT, "
                "author_unixname TEXT)")
    con.execute("INSERT INTO tasks VALUES ('task-1', 'worker-a', 'Build widget')")
    con.execute("INSERT INTO task_notes (id, task_id, content, created_at, "
                "author_unixname) VALUES (1, 'task-1', 'Started on the widget', "
                "'2026-01-01T00:10:00Z', NULL)")
    con.commit()
    con.close()


def _orc_upstream(base: Path) -> Path:
    src = base / "upstream"
    _orc_session_db(src / ".orc" / "sessions" / ORC_ROOT / "session.db",
                    ORC_ROOT, None, "root", "maindb", "2026-01-01T00:00:00Z",
                    [(1, "user", T0 + 60_000, "Build the widget"),
                     (2, "assistant", T0 + 120_000, "Widget built")])
    _orc_session_db(src / ".orc" / "sessions" / ORC_CHILD / "session.db",
                    ORC_CHILD, ORC_ROOT, "child", None, "2026-01-01T00:05:00Z",
                    [(3, "user", T0 + 300_000, "Check the widget"),
                     (4, "assistant", T0 + 360_000, "Widget checked")])
    _orc_task_db(src / ".tg" / "maindb.db")
    index = src / ".orc" / "index.db"
    con = sqlite3.connect(index)
    con.execute("CREATE TABLE sessions (id TEXT, parent_id TEXT)")
    con.execute("INSERT INTO sessions VALUES (?, NULL)", (ORC_ROOT,))
    con.execute("INSERT INTO sessions VALUES (?, ?)", (ORC_CHILD, ORC_ROOT))
    con.commit()
    con.close()
    return src




def _codex_sessions(base: Path) -> Path:
    root = base / "codex-sessions"
    root.mkdir(parents=True, exist_ok=True)

    def rec(ts: int, kind: str, payload: dict[str, object]) -> dict[str, object]:
        return {"timestamp": _iso(ts), "type": kind, "payload": payload}

    records = [
        rec(1000, "session_meta", {
            "id": CODEX_ROOT, "session_id": CODEX_ROOT,
            "timestamp": _iso(1000), "cwd": "/work/project",
            "source": "cli", "thread_source": "user",
            "git": {"repository_url": "https://github.com/example/widget.git"}}),
        rec(1010, "event_msg", {
            "type": "task_started", "turn_id": "root-turn",
            "started_at": 1010}),
        rec(1011, "response_item", {
            "type": "message", "id": "msg-user", "role": "user",
            "content": [{"type": "input_text",
                         "text": "Build the widget parser and test it"}]}),
        rec(1011, "event_msg", {
            "type": "user_message",
            "message": "Build the widget parser and test it"}),
        rec(1015, "response_item", {
            "type": "message", "id": "msg-asst", "role": "assistant",
            "phase": "commentary",
            "content": [{"type": "output_text",
                         "text": "Implemented the widget parser and verified "
                                 "the fix with tests that passed"}]}),
        rec(1050, "event_msg", {
            "type": "task_complete", "turn_id": "root-turn",
            "started_at": 1010, "completed_at": 1050,
            "time_to_first_token_ms": 12, "last_agent_message": "Root done"}),
    ]
    path = root / "rollout-2026-01-01T00-00-00-root-thread.jsonl"
    path.write_bytes(b"".join(
        json.dumps(item, sort_keys=True).encode() + b"\n" for item in records))
    return root


def _tree_inner(root: Path) -> dict[str, object]:
    import hashlib

    out: dict[str, object] = {}
    for rel, value in _tree(root).items():
        inner = rel.split("/", 1)[1]
        name = Path(inner).name
        if re.fullmatch(r"[0-9a-f]{64}\.json", name):
            digest = hashlib.sha256(json.dumps(
                value, sort_keys=True, default=str).encode()).hexdigest()[:16]
            inner = f"{Path(inner).parent.as_posix()}/HASH-{digest}.json"
        out[inner] = value
    return out


def _archives(tmp_path: Path) -> dict[str, Path]:
    return {"py": tmp_path / "py-out", "rs": tmp_path / "rs-out"}


def test_refresh_codex(tmp_path: Path, rust_wrkviz: Path) -> None:
    sessions = _codex_sessions(tmp_path)
    _both(_archives(tmp_path), "refresh", [
        "--sessions-root", str(sessions), "--root-session", CODEX_ROOT,
        "--backend", "heuristic",
    ])


def test_refresh_codex_second_run(tmp_path: Path, rust_wrkviz: Path) -> None:
    sessions = _codex_sessions(tmp_path)
    archives = _archives(tmp_path)
    args = [
        "--sessions-root", str(sessions), "--root-session", CODEX_ROOT,
        "--backend", "heuristic",
    ]
    _both(archives, "refresh", args)
    _both(archives, "refresh", args)


def test_refresh_claude(tmp_path: Path, rust_wrkviz: Path) -> None:
    archives = {}
    for label in ("py", "rs"):
        session = _claude_session(tmp_path / label)
        archives[label] = tmp_path / f"{label}-out"
        archives[label + "-session"] = session
    outcomes = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(RUST_WRKVIZ)])):
        outcomes[label] = _run(command, [
            "refresh-claude", "--output", str(archives[label]),
            "--team", "widget",
            "--session-file", str(archives[label + "-session"]),
            "--backend", "heuristic",
        ])
    oracle, result = outcomes["py"], outcomes["rs"]
    assert oracle.returncode == 0, oracle.stderr
    assert result.returncode == oracle.returncode
    assert _normalize(result.stdout, archives["rs"]) == _normalize(
        oracle.stdout, archives["py"])
    assert _normalize(result.stderr, archives["rs"]) == _normalize(
        oracle.stderr, archives["py"])
    assert _tree_inner(archives["rs"]) == _tree_inner(archives["py"])


def test_refresh_orc(tmp_path: Path, rust_wrkviz: Path) -> None:
    archives = {}
    sources = {}
    for label in ("py", "rs"):
        sources[label] = _orc_upstream(tmp_path / label)
        archives[label] = tmp_path / f"{label}-out"
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(RUST_WRKVIZ)])):
        outcome = _run(command, [
            "refresh-orc", "--output", str(archives[label]),
            "--team", "widget",
            "--source-root", str(sources[label]),
            "--root-session", ORC_ROOT,
            "--backend", "heuristic",
        ])
        assert outcome.returncode == 0, outcome.stderr
        if label == "py":
            oracle = outcome
        else:
            result = outcome
    assert result.returncode == oracle.returncode
    assert _normalize(result.stdout, archives["rs"]) == _normalize(
        oracle.stdout, archives["py"])
    assert _normalize(result.stderr, archives["rs"]) == _normalize(
        oracle.stderr, archives["py"])
    assert _tree_inner(archives["rs"]) == _tree_inner(archives["py"])


def test_refresh_help(rust_wrkviz: Path) -> None:
    for verb in ("refresh", "refresh-claude", "refresh-orc"):
        oracle = _run([sys.executable, str(PYTHON_WRKVIZ)], [verb, "--help"])
        result = _run([str(rust_wrkviz)], [verb, "--help"])
        assert result.returncode == oracle.returncode == 0
        assert result.stdout == oracle.stdout
        assert result.stderr == oracle.stderr


FAKE_GH = "#!/bin/sh\nprintf 'HTTP/2 404 \r\n\r\n{}'\n"


def test_refresh_with_github_flag(tmp_path: Path, rust_wrkviz: Path) -> None:
    sessions = _codex_sessions(tmp_path)
    gh = tmp_path / "fake-gh"
    gh.write_text(FAKE_GH)
    gh.chmod(0o755)
    outcomes = _both(_archives(tmp_path), "refresh", [
        "--sessions-root", str(sessions), "--root-session", CODEX_ROOT,
        "--backend", "heuristic", "--github-metadata",
    ], env={"AGENT_UTILS_GH": str(gh)})
    assert "github metadata: 0 evidenced references" in outcomes["py"].stdout
    assert "rebuild: 0 presentation files changed" in outcomes["py"].stdout
