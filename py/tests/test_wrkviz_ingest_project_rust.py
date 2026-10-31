"""Differential: Rust `wrkviz ingest-project` against the Python oracle.

Both editions load the same strict schema-v1 project manifest
(twin copies differing only in the output directory), ingest a
Codex team and an Orc team into twin archives, and refresh the
transcript JSONL; stdout, stderr, exit code, and the file trees
must agree with run-record timestamps, config digests, run ids,
and the SQLite backup stamps normalized (see the ingest-orc
differential for the stamp class). Team failures are recorded,
never fatal to sibling teams.

Fixtures only; no live sources.
"""

from __future__ import annotations

import gzip
import json
import re
import sqlite3
import subprocess
import sys
from pathlib import Path

import pytest

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkviz"
CODEX_ROOT = "root-thread"
ORC_ROOT_ID = "11111111-2222-3333-4444-555555555555"
T0 = 1767225600000  # 2026-01-01T00:00:00Z
SHA_KEYS = {"sha256", "snapshot_path", "source_manifest_sha256",
            "source_digest", "team_sha256", "superseded_sha256",
            "superseded_snapshot_path", "config_sha256"}
TS_KEYS = {"run_id", "started_at", "completed_at", "created_at",
           "last_run_at", "last_run_id", "updated_at", "mtime_ns",
           "captured_at", "upstream_retired_at", "accepted_at",
           "last_rewrite_at"}


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


def _iso(seconds: float) -> str:
    hours, rem = divmod(int(seconds), 3600)
    minutes, secs = divmod(rem, 60)
    return f"2026-01-01T{hours:02d}:{minutes:02d}:{secs:02d}.000Z"


def _record(ts: float, kind: str, payload: dict[str, object]) -> dict[str, object]:
    return {"timestamp": _iso(ts), "type": kind, "payload": payload}


def _codex_sessions(base: Path) -> Path:
    root = base / "codex-sessions"
    path = root / "rollout-2026-01-01T00-00-00-root-thread.jsonl"
    records = [
        _record(1000, "session_meta", {
            "id": CODEX_ROOT, "session_id": CODEX_ROOT,
            "timestamp": _iso(1000), "cwd": "/work/project",
            "source": "cli", "thread_source": "user",
            "git": {"repository_url": "https://github.com/example/widget.git"}}),
        _record(1010, "event_msg", {
            "type": "task_started", "turn_id": "root-turn",
            "started_at": 1010}),
        _record(1011, "response_item", {
            "type": "message", "id": "msg-user", "role": "user",
            "content": [{"type": "input_text", "text": "Build the thing"}]}),
        _record(1011, "event_msg", {
            "type": "user_message", "message": "Build the thing"}),
        _record(1015, "response_item", {
            "type": "message", "id": "msg-asst", "role": "assistant",
            "phase": "commentary",
            "content": [{"type": "output_text", "text": "Working on it"}]}),
        _record(1050, "event_msg", {
            "type": "task_complete", "turn_id": "root-turn",
            "started_at": 1010, "completed_at": 1050,
            "time_to_first_token_ms": 12, "last_agent_message": "Root done"}),
    ]
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(b"".join(
        json.dumps(item, sort_keys=True).encode() + b"\n" for item in records))
    return root


def _message(mid: int, role: str, ts: int, text: str) -> str:
    return json.dumps({
        "id": f"m{mid}", "role": role, "created_at_ms": ts, "source": "",
        "blocks": [{"type": "text", "id": mid * 10, "text": text}],
    }, sort_keys=True)


def _orc_source(base: Path) -> Path:
    src = base / "orc-src"
    db = src / ".orc" / "sessions" / ORC_ROOT_ID / "session.db"
    db.parent.mkdir(parents=True, exist_ok=True)
    if db.exists():
        db.unlink()
    con = sqlite3.connect(db)
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
    con.execute("INSERT INTO session_meta VALUES (?,?,?,NULL,?,?)",
                (ORC_ROOT_ID, None, "root", "2026-01-01T00:00:00Z",
                 "2026-01-01T00:00:00Z"))
    for mid, role, ts, text in [
            (1, "user", T0 + 60_000, "Build the widget"),
            (2, "assistant", T0 + 120_000, "Widget built")]:
        con.execute("INSERT INTO messages VALUES (?,?,?,?,?,?)",
                    (mid, ORC_ROOT_ID, role, ts,
                     _message(mid, role, ts, text), text))
    con.commit()
    con.close()
    return src


def _config(base: Path, label: str, *, orc_root: str = ORC_ROOT_ID) -> Path:
    held: dict[str, object] = {
        "schema_version": 1,
        "output": f"{label}-out",
        "teams": [
            {"slug": "code-team", "provider": "codex",
             "source": {"sessions_root": "codex-sessions",
                        "root_session": CODEX_ROOT}},
            {"slug": "orc-team", "provider": "orc",
             "source": {"source_root": "orc-src", "root_session": orc_root}},
        ],
    }
    path = base / f"project-{label}.json"
    path.write_text(json.dumps(held, indent=1))
    return path


def _run(command: list[str], args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [*command, "ingest-project", *args],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=300,
    )


def _normalize(text: str, shared: Path, label: str) -> str:
    text = text.replace(str(shared / f"{label}-out"), "ARCHIVE")
    text = text.replace(f"project-{label}.json", "project.json")
    text = text.replace(f"../{label}-out.sources", "../ARCHIVE.sources")
    text = text.replace(f"../{label}-out.build", "../ARCHIVE.build")
    text = text.replace(f"../{label}-out", "../ARCHIVE")
    text = text.replace(str(shared), "SHARED")
    text = re.sub(r"runs/[0-9A-Za-z-]+\.json", "runs/RUN.json", text)
    return text


def _scrub(value: object, shared: Path, label: str) -> object:
    if isinstance(value, dict):
        out: dict[str, object] = {}
        for key, item in value.items():
            if key in SHA_KEYS:
                out[key] = "<sha>"
            elif key in TS_KEYS:
                out[key] = "<ts>"
            elif key == "command" and isinstance(item, list):
                out[key] = [
                    _normalize(str(arg), shared, label) for arg in item
                ]
            else:
                out[key] = _scrub(item, shared, label)
        return out
    if isinstance(value, list):
        return [_scrub(item, shared, label) for item in value]
    if isinstance(value, str):
        value = _normalize(value, shared, label)
        return re.sub(r"\.objects/[0-9a-f]{2}/[0-9a-f]{64}\.db",
                      ".objects/HASH.db", value)
    return value


def _tree(root: Path, shared: Path, label: str) -> dict[str, object]:
    result: dict[str, object] = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(root).as_posix()
        if rel.startswith("runs/"):
            rel = "runs/RUN.json"
        if rel.endswith(".jsonl.gz"):
            # gzip bytes are the recorded zlib-generation
            # divergence; the content must agree.
            result[rel] = gzip.decompress(path.read_bytes())
        elif rel.endswith(".jsonl.gz.index.jsonl"):
            lines = [json.loads(line) for line in path.read_text().splitlines()]
            lines[0].pop("c_sha256", None)
            lines[0].pop("c_size", None)
            for member in lines[1:]:
                member.pop("c_len", None)
                member.pop("c_off", None)
            result[rel] = _scrub(lines, shared, label)
        elif rel == "extracted/transcripts/manifest.json":
            manifest = json.loads(path.read_text())
            for entry in manifest["files"].values():
                entry.pop("c_sha256", None)
                entry.pop("c_bytes", None)
            result[rel] = _scrub(manifest, shared, label)
        elif path.suffix == ".json":
            result[rel] = _scrub(json.loads(path.read_text()), shared, label)
        elif path.suffix == ".db":
            con = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
            try:
                result[rel] = "\n".join(con.iterdump())
            finally:
                con.close()
        else:
            result[rel] = path.read_bytes()
    objects = {key: value for key, value in result.items()
               if "/.objects/" in key}
    for key in objects:
        del result[key]
    dumps = [value for value in objects.values() if isinstance(value, str)]
    for index, dump in enumerate(sorted(dumps)):
        result[f"objects/DB-{index}.db"] = dump
    return result


def _archives(shared: Path, label: str) -> dict[str, object]:
    combined: dict[str, object] = {}
    for name in (f"{label}-out", f"{label}-out.build", f"{label}-out.sources"):
        if not (shared / name).exists():
            continue
        for rel, value in _tree(shared / name, shared, label).items():
            combined[f"OUT{name[len(label):]}/{rel}"] = value
    return combined


def _compare(tmp_path: Path, rust_wrkviz: Path, args: list[str], *,
             orc_root: str = ORC_ROOT_ID,
             ) -> tuple[subprocess.CompletedProcess[str],
                        subprocess.CompletedProcess[str]]:
    shared = tmp_path / "shared"
    shared.mkdir(parents=True, exist_ok=True)
    _codex_sessions(shared)
    _orc_source(shared)
    outcomes = {}
    for label, cmd in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                       ("rs", [str(rust_wrkviz)])):
        cfg = _config(shared, label, orc_root=orc_root)
        outcomes[label] = _run(cmd, ["--config", str(cfg), *args])
    oracle, result = outcomes["py"], outcomes["rs"]
    assert result.returncode == oracle.returncode, (result.stderr, oracle.stderr)
    assert _normalize(result.stdout, shared, "rs") == \
        _normalize(oracle.stdout, shared, "py")
    assert _normalize(result.stderr, shared, "rs") == \
        _normalize(oracle.stderr, shared, "py")
    assert _archives(shared, "rs") == _archives(shared, "py")
    return oracle, result


def test_project_mixed_teams(tmp_path: Path, rust_wrkviz: Path) -> None:
    oracle, result = _compare(tmp_path, rust_wrkviz, [])
    assert oracle.returncode == 0, oracle.stderr
    assert "code-team (codex): ingest:" in result.stdout
    assert "orc-team (orc): ingest:" in result.stdout
    assert "teams: 2 succeeded, 0 failed" in result.stdout


def test_project_second_run(tmp_path: Path, rust_wrkviz: Path) -> None:
    oracle, result = _compare(tmp_path, rust_wrkviz, [])
    assert oracle.returncode == 0
    oracle, result = _compare(tmp_path, rust_wrkviz, [])
    assert oracle.returncode == 0


def test_project_team_filter(tmp_path: Path, rust_wrkviz: Path) -> None:
    oracle, result = _compare(tmp_path, rust_wrkviz,
                              ["--team", "orc-team"])
    assert oracle.returncode == 0
    assert "code-team" not in result.stdout
    assert "teams: 1 succeeded, 0 failed" in result.stdout


def test_project_failing_team(tmp_path: Path, rust_wrkviz: Path) -> None:
    oracle, result = _compare(
        tmp_path, rust_wrkviz, [],
        orc_root="00000000-0000-0000-0000-000000000000")
    assert oracle.returncode == 2
    assert "teams: 1 succeeded, 1 failed" in result.stdout
    assert "team orc-team (orc):" in result.stderr


def test_project_bad_configs(tmp_path: Path, rust_wrkviz: Path) -> None:
    shared = tmp_path / "shared"
    shared.mkdir(parents=True)
    cases = {
        "unknown-field": {"schema_version": 1, "output": "x", "teams": [],
                          "bogus": 1},
        "bad-version": {"schema_version": 7, "output": "x", "teams": [{}]},
        "no-teams": {"schema_version": 1, "output": "x", "teams": []},
        "absolute-output": {"schema_version": 1, "output": "/tmp/x",
                            "teams": [{}]},
    }
    for name, document in cases.items():
        cfg = shared / f"{name}.json"
        cfg.write_text(json.dumps(document))
        seen = []
        for cmd in ([sys.executable, str(PYTHON_WRKVIZ)], [str(rust_wrkviz)]):
            done = _run(cmd, ["--config", str(cfg)])
            assert done.returncode == 2, (name, done.stdout, done.stderr)
            seen.append(done.stderr.replace(str(shared), "SHARED"))
        assert seen[0] == seen[1], name


def test_project_accept_selection_errors(tmp_path: Path,
                                         rust_wrkviz: Path) -> None:
    for extra in (["--accept-orc-prefix-rewrite", "nocolon"],
                  ["--accept-orc-prefix-rewrite", "code-team:xyz"],
                  ["--accept-orc-prefix-rewrite", "ghost-team:xyz"],
                  ["--team", "ghost-team"],
                  ["--team", "orc-team", "--team", "orc-team"]):
        oracle, result = _compare(tmp_path, rust_wrkviz, list(extra))
        assert oracle.returncode == 2, extra
