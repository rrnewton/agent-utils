"""Differential: Rust `wrkviz ingest-orc` against the Python oracle.

Both editions snapshot and normalize the same synthetic Orc
source tree (modern `messages` session databases, an index,
and a `.tg` task database) into twin archives; stdout,
stderr, exit code, and the file trees must agree. Run records
and manifests compare with timestamps, run ids, and snapshot
mtimes normalized.

Named normalizations: the CPython-vs-serde JSON syntax
wording; the backup-file digest fields (`sha256`,
`snapshot_path`, `source_manifest_sha256`, `source_digest`,
`team_sha256`), whose bytes differ by the SQLite
`SQLITE_VERSION_NUMBER` header stamp each library writes
during online backup (the environment-defined stamp class,
like zlib-generation bytes). Every payload digest -- append
prefixes, semantic identities, task projections -- compares
exactly, and snapshot databases compare by SQL dump.

Fixtures only; no live Orc tree.
"""

from __future__ import annotations

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
ROOT_ID = "11111111-2222-3333-4444-555555555555"
CHILD_ID = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
CONT_ID = "99999999-8888-7777-6666-555555555555"
T0 = 1767225600000  # 2026-01-01T00:00:00Z
SHA_KEYS = {"sha256", "snapshot_path", "source_manifest_sha256",
            "source_digest", "team_sha256", "superseded_sha256",
            "superseded_snapshot_path"}


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


def _message(mid: int, role: str, ts: int, text: str) -> str:
    return json.dumps({
        "id": f"m{mid}",
        "role": role,
        "created_at_ms": ts,
        "source": "",
        "blocks": [{"type": "text", "id": mid * 10, "text": text}],
    }, sort_keys=True)


def _session_db(path: Path, session_id: str, parent_id: str | None,
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
                     _message(mid, role, ts, text), text))
    con.commit()
    con.close()


def _task_db(path: Path) -> None:
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
    con.execute("INSERT INTO task_notes (id, task_id, content, created_at, "
                "author_unixname) VALUES (2, 'task-1', 'External update', "
                "'2026-01-01T00:20:00Z', 'ext-author')")
    con.commit()
    con.close()


def _upstream(base: Path, *, continuation: bool = False) -> Path:
    src = base / "upstream"
    _session_db(src / ".orc" / "sessions" / ROOT_ID / "session.db",
                ROOT_ID, None, "root", "maindb", "2026-01-01T00:00:00Z",
                [(1, "user", T0 + 60_000, "Build the widget"),
                 (2, "assistant", T0 + 120_000, "Widget built")])
    _session_db(src / ".orc" / "sessions" / CHILD_ID / "session.db",
                CHILD_ID, ROOT_ID, "child", None, "2026-01-01T00:05:00Z",
                [(1, "user", T0 + 300_000, "Check the widget"),
                 (2, "assistant", T0 + 360_000, "Widget checked")])
    if continuation:
        _session_db(src / ".orc" / "sessions" / CONT_ID / "session.db",
                    CONT_ID, None, "root-two", None, "2026-01-01T00:10:00Z",
                    [(1, "user", T0 + 660_000, "Continue the widget"),
                     (2, "assistant", T0 + 720_000, "Widget continued")])
    _task_db(src / ".tg" / "maindb.db")
    index = src / ".orc" / "index.db"
    con = sqlite3.connect(index)
    con.execute("CREATE TABLE sessions (id TEXT, parent_id TEXT)")
    con.execute("INSERT INTO sessions VALUES (?, NULL)", (ROOT_ID,))
    con.execute("INSERT INTO sessions VALUES (?, ?)", (CHILD_ID, ROOT_ID))
    if continuation:
        con.execute("INSERT INTO sessions VALUES (?, NULL)", (CONT_ID,))
    con.commit()
    con.close()
    return src


def _run(command: list[str], args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [*command, "ingest-orc", *args],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=120,
    )


def _normalize(text: str, root: Path) -> str:
    text = text.replace(str(root), "ROOT")
    text = re.sub(r"runs/[0-9A-Za-z-]+\.json", "runs/RUN.json", text)
    text = re.sub(r"invalid JSON: .*", "invalid JSON: <syntax>", text)
    text = re.sub(r"invalid JSON \(.*\)", "invalid JSON (<syntax>)", text)
    text = re.sub(r"orc-\d+-[0-9a-f]+\.db", "orc-TEMP.db", text)
    text = re.sub(r"\.objects/[0-9a-f]{2}/[0-9a-f]{64}\.db",
                  ".objects/HASH.db", text)
    return text


def _scrub(value: object, root: Path) -> object:
    if isinstance(value, dict):
        out: dict[str, object] = {}
        for key, item in value.items():
            if key in SHA_KEYS:
                out[key] = "<sha>"
            elif key in ("run_id", "started_at", "completed_at", "created_at",
                         "last_run_at", "last_run_id", "updated_at", "mtime_ns",
                         "captured_at", "upstream_retired_at", "accepted_at",
                         "last_rewrite_at"):
                out[key] = "<ts>"
            elif key == "command" and isinstance(item, list):
                out[key] = [
                    ("ROOT" if str(arg) == str(root)
                     or str(arg).startswith(str(root)) else arg)
                    for arg in item
                ]
            else:
                out[key] = _scrub(item, root)
        return out
    if isinstance(value, list):
        return [_scrub(item, root) for item in value]
    if isinstance(value, str):
        value = value.replace(str(root), "ROOT")
        return re.sub(r"\.objects/[0-9a-f]{2}/[0-9a-f]{64}\.db",
                      ".objects/HASH.db", value)
    return value


def _dump(path: Path) -> str:
    con = sqlite3.connect(f"file:{path}?mode=ro", uri=True)
    try:
        return "\n".join(con.iterdump())
    finally:
        con.close()


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
        elif path.suffix == ".db":
            result[rel] = _dump(path)
        else:
            result[rel] = path.read_bytes()
    # Snapshot object names are digests of backup bytes (see the
    # module docstring): pair the databases by their dumps.
    objects = {key: value for key, value in result.items()
               if "/.objects/" in key}
    for key in objects:
        del result[key]
    for index, dump in enumerate(sorted(objects.values())):
        result[f"objects/DB-{index}.db"] = dump
    return result


def _archives(root: Path) -> dict[str, object]:
    combined: dict[str, object] = {}
    for name in ("archive", "archive.build", "archive.sources"):
        if not (root / name).exists():
            continue
        for rel, value in _tree(root / name).items():
            combined[f"{name}/{rel}"] = value
    return combined


def _compare(tmp_path: Path, rust_wrkviz: Path, args: list[str], *,
             source_root: Path) -> tuple[subprocess.CompletedProcess[str],
                                          subprocess.CompletedProcess[str]]:
    py_root = tmp_path / "py"
    rs_root = tmp_path / "rs"
    shared = ["--team", "orc-team", "--source-root", str(source_root),
              "--root-session", ROOT_ID, *args]
    oracle = _run([sys.executable, str(PYTHON_WRKVIZ)],
                  ["--output", str(py_root / "archive"), *shared])
    result = _run([str(rust_wrkviz)],
                  ["--output", str(rs_root / "archive"), *shared])
    assert result.returncode == oracle.returncode, (result.stderr, oracle.stderr)
    assert _normalize(result.stdout, rs_root) == _normalize(oracle.stdout, py_root)
    assert _normalize(result.stderr, rs_root) == _normalize(oracle.stderr, py_root)
    assert _archives(rs_root) == _archives(py_root)
    return oracle, result


def _pair(tmp_path: Path, rust_wrkviz: Path, extra: list[str], *,
          second: bool = False,
          continuation: bool = False) -> tuple[subprocess.CompletedProcess[str],
                                               subprocess.CompletedProcess[str]]:
    source_root = _upstream(tmp_path / "shared", continuation=continuation)
    oracle, result = _compare(tmp_path, rust_wrkviz, extra,
                              source_root=source_root)
    if second:
        oracle, result = _compare(tmp_path, rust_wrkviz, extra,
                                  source_root=source_root)
    return oracle, result


def test_orc_fresh_and_second_run(tmp_path: Path, rust_wrkviz: Path) -> None:
    oracle, result = _pair(tmp_path / "fresh", rust_wrkviz, [])
    assert oracle.returncode == 0
    assert "3 agents" in result.stdout
    _pair(tmp_path / "second", rust_wrkviz, [], second=True)


def test_orc_missing_root_session(tmp_path: Path, rust_wrkviz: Path) -> None:
    source_root = _upstream(tmp_path / "shared")
    for label, root in (("py", tmp_path / "py"), ("rs", tmp_path / "rs")):
        cmd = ([sys.executable, str(PYTHON_WRKVIZ)] if label == "py"
               else [str(rust_wrkviz)])
        done = _run(cmd, [
            "--output", str(root / "archive"), "--team", "orc-team",
            "--source-root", str(source_root), "--root-session",
            "00000000-0000-0000-0000-000000000000"])
        assert done.returncode == 2, (label, done.stderr)
        assert "was not found" in done.stderr or "absent" in done.stderr


def test_orc_malformed_database(tmp_path: Path, rust_wrkviz: Path) -> None:
    source_root = _upstream(tmp_path / "shared")
    target = source_root / ".orc" / "sessions" / ROOT_ID / "session.db"
    target.write_bytes(b"this is not a sqlite database at all" * 8)
    oracle, result = _compare(tmp_path, rust_wrkviz, [], source_root=source_root)
    assert oracle.returncode == 2
    assert result.returncode == 2


def test_orc_append_rewrite_refused_and_accepted(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    source_root = _upstream(tmp_path / "shared")
    flow = tmp_path / "flow"
    oracle, result = _compare(flow, rust_wrkviz, [], source_root=source_root)
    assert oracle.returncode == 0
    # Rewrite one recorded message in place: same rows, new text.
    target = source_root / ".orc" / "sessions" / ROOT_ID / "session.db"
    con = sqlite3.connect(target)
    held = con.execute(
        "SELECT message_json FROM messages WHERE id = 2").fetchone()[0]
    con.execute("UPDATE messages SET message_json = ?, search_text = ? "
                "WHERE id = 2",
                (held.replace("Widget built", "Widget rebuilt"),
                 "Widget rebuilt"))
    con.commit()
    con.close()
    oracle, result = _compare(flow, rust_wrkviz, [], source_root=source_root)
    assert oracle.returncode == 2
    assert "existing append prefix was rewritten" in result.stderr
    oracle, result = _compare(
        flow, rust_wrkviz, ["--accept-orc-prefix-rewrite", ROOT_ID],
        source_root=source_root)
    assert oracle.returncode == 0
    receipt = json.loads(sorted(
        (flow / "rs" / "archive" / "runs").glob("*.json"))[-1].read_text())
    assert receipt["ingest"]["orc_prefix_overrides"]


def test_orc_upstream_retired(tmp_path: Path, rust_wrkviz: Path) -> None:
    source_root = _upstream(tmp_path / "shared")
    flow = tmp_path / "flow"
    oracle, result = _compare(flow, rust_wrkviz, [], source_root=source_root)
    assert oracle.returncode == 0
    for session_id in (ROOT_ID, CHILD_ID):
        (source_root / ".orc" / "sessions" / session_id
         / "session.db").unlink()
    oracle, result = _compare(flow, rust_wrkviz, [], source_root=source_root)
    assert oracle.returncode == 0
    receipt = json.loads(sorted(
        (flow / "rs" / "archive" / "runs").glob("*.json"))[-1].read_text())
    assert receipt["ingest"]["upstream_retired_sources"] == [
        f".orc/sessions/{ROOT_ID}/session.db",
        f".orc/sessions/{CHILD_ID}/session.db",
        ".tg/maindb.db",
    ]


def test_orc_continuation(tmp_path: Path, rust_wrkviz: Path) -> None:
    oracle, result = _pair(tmp_path, rust_wrkviz,
                           ["--continuation-session", CONT_ID],
                           continuation=True)
    assert oracle.returncode == 0
    manifest = json.loads(
        (tmp_path / "rs" / "archive.build" / "orc-team" / "raw"
         / "source-manifest.json").read_text())
    assert manifest["schema_version"] == 5
    assert manifest["continuation_sessions"][0]["session_id"] == CONT_ID


def test_orc_bounded_continuation(tmp_path: Path, rust_wrkviz: Path) -> None:
    spec = json.dumps({"session_id": CONT_ID, "start_message_id": "m1"})
    oracle, result = _pair(tmp_path, rust_wrkviz,
                           ["--continuation-session", spec],
                           continuation=True)
    assert oracle.returncode == 0
    manifest = json.loads(
        (tmp_path / "rs" / "archive.build" / "orc-team" / "raw"
         / "source-manifest.json").read_text())
    link = manifest["continuation_sessions"][0]
    assert link["start_message_id"] == "m1"
    assert link["start_source_line"] == 1


def test_orc_window_and_identity(tmp_path: Path, rust_wrkviz: Path) -> None:
    _pair(tmp_path, rust_wrkviz, [
        "--project", "Widget=https://github.com/example/widget.git",
        "--source-host", "Host01.Example",
        "--timezone", "UTC",
        "--start-time", "2026-01-01T00:03:00Z",
        "--end-time", "2026-01-01T00:21:00Z",
    ])
