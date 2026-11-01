"""Differential: Rust `wrkviz discover-agents` vs the oracle.

One archived home per host side holds every harness's sessions
plus an agentctl registry, so the tree exercises native links
(Claude subagents, Codex spawns, Orc parents), manager launches,
restarts from queued prompts, shell-exec links, creator links,
unresolved launches, ignored mentions, and the orphan sweep.
The command is read-only, so both editions run against the same
fixture and stdout/stderr/exit (and the --json file) must agree
byte-for-byte.
"""

from __future__ import annotations

import json
import os
import sqlite3
import subprocess
from datetime import datetime, timezone
from pathlib import Path

import pytest

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkviz"

T0 = datetime(2026, 1, 5, 12, 0, tzinfo=timezone.utc).timestamp()


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


def _iso(seconds: float) -> str:
    return datetime.fromtimestamp(T0 + seconds, timezone.utc).isoformat().replace(
        "+00:00", "Z"
    )


def _at(seconds: float) -> float:
    return T0 + seconds


def _write_jsonl(path: Path, records: list[dict[str, object]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        "".join(json.dumps(record) + "\n" for record in records), encoding="utf-8"
    )


def _write_json(path: Path, record: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(record), encoding="utf-8")


def _user(seconds: float, text: str, cwd: str = "/proj") -> dict[str, object]:
    return {"type": "user", "cwd": cwd, "timestamp": _iso(seconds),
            "message": {"role": "user", "content": text}}


def _bash(seconds: float, command: str) -> dict[str, object]:
    # The launch/exec scanners read raw log lines, so the command
    # rides a plain text record the way a shell transcript holds it.
    return {"type": "assistant", "timestamp": _iso(seconds),
            "message": {"role": "assistant", "content": command}}


def _fixture(tmp_path: Path) -> Path:
    home = tmp_path / "homeA"
    projects = home / ".claude" / "projects" / "-proj"
    _write_jsonl(projects / "coord.jsonl", [
        _user(0, "coordinate the work"),
        _bash(60, "agentctl start worker-a"),
        _bash(110, "agentctl start worker-b"),
        _bash(130, "export A=/home/x/agentctl; $A start ghost-name"),
        _bash(180, "codex exec --full-auto review this"),
        _bash(240, "agentctl start late-agent"),
    ])
    _write_jsonl(projects / "coord" / "subagents" / "agent-sub1.jsonl", [
        _user(30, "subtask"),
    ])
    _write_jsonl(projects / "lane1.jsonl", [_user(70, "You are worker-a, do it")])
    _write_jsonl(projects / "lane1b.jsonl", [_user(320, "worker-a: continue with part two")])
    _write_jsonl(projects / "stray.jsonl", [_user(800, "unlinked work")])
    codex = home / ".codex" / "sessions" / "2026" / "01" / "05"
    _write_jsonl(codex / "rollout-2026-01-05T12-03-10-threadchild.jsonl", [
        {"type": "session_meta", "payload": {
            "id": "thread-child", "timestamp": _iso(190), "cwd": "/proj",
            "source": "cli"}},
        {"type": "response_item", "timestamp": _iso(195), "payload": {
            "type": "message", "role": "user",
            "content": [{"type": "input_text", "text": "review this"}]}},
    ])
    _write_jsonl(codex / "rollout-2026-01-05T12-03-20-threadspawn.jsonl", [
        {"type": "session_meta", "payload": {
            "id": "thread-spawn", "timestamp": _iso(200), "cwd": "/proj",
            "source": {"subagent": {"thread_spawn": {"parent_thread_id": "thread-child"}}}}},
    ])
    muse = home / ".local" / "share" / "muse" / "sessions" / "2026" / "01" / "05"
    _write_jsonl(muse / "sess-muse1" / "session.jsonl", [
        {"recorded_at": int(_at(400) * 1e6),
         "payload": {"kind": "route_facts", "record": {"cwd": "/proj/"}}},
    ])
    _write_jsonl(muse / "sess-muse2" / "session.jsonl", [
        {"recorded_at": int(_at(500) * 1e6),
         "payload": {"kind": "route_facts", "record": {"cwd": "/proj/sub"}}},
    ])
    orc = home / ".orc"
    orc.mkdir(parents=True, exist_ok=True)
    connection = sqlite3.connect(orc / "index.db")
    connection.execute(
        "CREATE TABLE sessions (id TEXT, parent_id TEXT, name TEXT, created_at TEXT, cwd TEXT)"
    )
    connection.executemany(
        "INSERT INTO sessions VALUES (?, ?, ?, ?, ?)",
        [
            ("orc-root", None, "orc-coord", _iso(0), "/proj"),
            ("orc-kid", "orc-root", None, _iso(100), "/proj"),
        ],
    )
    connection.commit()
    connection.close()
    registry = home / ".agentctl"
    _write_json(registry / "agents" / "worker-a" / "agent.json",
                {"name": "worker-a", "harness": "claude", "cwd": "/proj",
                 "created_at": _at(65)})
    _write_json(registry / "agents" / "worker-a" / "queue" / "inbox" / "msg1.json",
                {"text": "worker-a: continue with part two", "queued_at": _at(300)})
    _write_json(registry / "worker-b" / "agent.json",
                {"name": "worker-b", "harness": "codex", "cwd": "/proj",
                 "created_at": _at(120),
                 "native_session": {"value": "thread-child"}})
    _write_json(registry / "late-agent" / "agent.json",
                {"name": "late-agent", "harness": "claude", "cwd": "/proj",
                 "created_at": _at(5240)})
    _write_json(registry / "archive" / "2026" / "worker-old" / "agent.json",
                {"name": "worker-old", "harness": "muse", "cwd": "/proj",
                 "created_at": _at(390), "created_by": {"agent": "worker-a"}})
    return home


def _run(binary: Path, args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(binary), *args], cwd=AU_ROOT, capture_output=True, text=True,
        check=False, timeout=300,
    )


def _both(
    rust_wrkviz: Path, args: list[str]
) -> tuple[subprocess.CompletedProcess[str], subprocess.CompletedProcess[str]]:
    oracle = _run(PYTHON_WRKVIZ, args)
    rust = _run(rust_wrkviz, args)
    assert (rust.returncode, rust.stdout, rust.stderr) == (
        oracle.returncode, oracle.stdout, oracle.stderr
    ), (args, oracle, rust)
    return oracle, rust


def test_discover_tree_report_matches(
    rust_wrkviz: Path, tmp_path: Path
) -> None:
    home = _fixture(tmp_path)
    base = [
        "discover-agents", "--home", f"hostA={home}",
        "--registry", f"hostA={home / '.agentctl'}",
        "--cwd-prefix", "/proj",
        "--root", "claude:coord", "--root", "orc:orc-root",
    ]
    oracle, _ = _both(rust_wrkviz, base)
    assert "| claude:coord | worker-a |" not in oracle.stdout  # root name stays empty
    assert "claude-subagent 1" in oracle.stdout
    assert "manager-launch 2" in oracle.stdout
    assert "manager-restart 1" in oracle.stdout
    assert "manager-created-by 1" in oracle.stdout
    assert "codex-spawn 1" in oracle.stdout
    assert "orc-parent 1" in oracle.stdout
    assert "orphans (under a --cwd-prefix, reached by no root): 2" in oracle.stdout
    assert "no resolvable child: 1; launch-like text naming no registered agent (ignored): 1" in oracle.stdout
    oracle_json, _ = _both(rust_wrkviz, [*base, "--format", "json"])
    report = json.loads(oracle_json.stdout)
    keys = {row["key"] for row in report["tree"]}
    assert keys == {
        "claude-subagent:coord/sub1", "claude:lane1", "claude:lane1b",
        "codex:thread-child", "codex:thread-spawn", "muse:sess-muse1",
        "orc:orc-kid",
    }
    assert {row["key"] for row in report["orphans"]} == {
        "claude:stray", "muse:sess-muse2",
    }
    assert report["unresolved_launches"][0]["name"] == "late-agent"
    assert report["unresolved_launches"][0]["reason"] == "no record created by this launch"


def test_discover_json_file_matches(
    rust_wrkviz: Path, tmp_path: Path
) -> None:
    home = _fixture(tmp_path)
    oracle_file = tmp_path / "oracle-tree.json"
    rust_file = tmp_path / "rust-tree.json"
    base = [
        "discover-agents", "--home", f"hostA={home}",
        "--cwd-prefix", "/proj", "--root", "claude:coord",
    ]
    oracle = _run(PYTHON_WRKVIZ, [*base, "--json", str(oracle_file)])
    rust = _run(rust_wrkviz, [*base, "--json", str(rust_file)])
    assert (rust.returncode, rust.stdout, rust.stderr) == (
        oracle.returncode, oracle.stdout, oracle.stderr
    )
    assert rust_file.read_bytes() == oracle_file.read_bytes()


def test_discover_since_filter_matches(
    rust_wrkviz: Path, tmp_path: Path
) -> None:
    home = _fixture(tmp_path)
    stray = home / ".claude" / "projects" / "-proj" / "stray.jsonl"
    os.utime(stray, (_at(100), _at(100)))
    base = [
        "discover-agents", "--home", f"hostA={home}",
        "--registry", f"hostA={home / '.agentctl'}",
        "--cwd-prefix", "/proj", "--root", "claude:coord",
        "--since", _iso(900), "--format", "json",
    ]
    oracle, _ = _both(rust_wrkviz, base)
    report = json.loads(oracle.stdout)
    assert {row["key"] for row in report["orphans"]} == {"muse:sess-muse2"}
    day = [
        "discover-agents", "--home", f"hostA={home}",
        "--cwd-prefix", "/proj", "--root", "claude:coord",
        "--since", "2026-01-04", "--format", "json",
    ]
    _both(rust_wrkviz, day)


def test_discover_duplicate_registry_records_match(
    rust_wrkviz: Path, tmp_path: Path
) -> None:
    # Two records with identical fields collapse into one
    # `resolved` entry in the oracle (it keys by record value);
    # the child must be claimed once, not attached twice.
    home = _fixture(tmp_path)
    duplicate = home / ".agentctl" / "archive" / "copy" / "agent.json"
    _write_json(duplicate, {"name": "worker-a", "harness": "claude",
                            "cwd": "/proj", "created_at": _at(65)})
    args = [
        "discover-agents", "--home", f"hostA={home}",
        "--registry", f"hostA={home / '.agentctl'}",
        "--cwd-prefix", "/proj",
        "--root", "claude:coord", "--root", "orc:orc-root",
        "--format", "json",
    ]
    oracle, _ = _both(rust_wrkviz, args)
    report = json.loads(oracle.stdout)
    everywhere = list(report["tree"]) + list(report["orphans"])
    lane1 = [row for row in everywhere if row["key"] == "claude:lane1"]
    assert len(lane1) == 1
    assert lane1[0]["parent"] is None
    assert all(row["link"] != "manager-restart" for row in everywhere)


def test_discover_source_flags_match(
    rust_wrkviz: Path, tmp_path: Path
) -> None:
    home = _fixture(tmp_path)
    args = [
        "discover-agents",
        "--claude-projects", f"hostA={home / '.claude' / 'projects'}",
        "--codex-sessions", f"hostA={home / '.codex' / 'sessions'}",
        "--orc-root", f"hostA={home}",
        "--muse-sessions", f"hostA={home / '.local' / 'share' / 'muse' / 'sessions'}",
        "--registry", f"hostA={home / '.agentctl'}",
        "--cwd-prefix", "/proj/", "--root", "orc:orc-root",
        "--format", "json",
    ]
    _both(rust_wrkviz, args)


def test_discover_errors_match(
    rust_wrkviz: Path, tmp_path: Path
) -> None:
    home = _fixture(tmp_path)
    base = ["discover-agents", "--home", f"hostA={home}"]
    for args in (
        [*base, "--root", "claude:missing"],
        [*base, "--root", "claude:coord", "--since", "yesterday"],
        [*base, "--root", "claude:coord", "--since", "2026-01-05T12:00:00"],
        ["discover-agents", "--home", "noequals", "--root", "claude:coord"],
        ["discover-agents", "--home", "=x", "--root", "claude:coord"],
    ):
        oracle, _ = _both(rust_wrkviz, list(args))
        assert oracle.returncode == 2, args


def test_discover_help_and_argparse_match(rust_wrkviz: Path) -> None:
    for args in (
        ["discover-agents", "--help"],
        ["discover-agents", "-h"],
        ["discover-agents"],
        ["discover-agents", "--format", "xml", "--root", "claude:x"],
        ["discover-agents", "--bogus", "--root", "claude:x"],
        ["discover-agents", "--bogus"],
        ["discover-agents", "extra", "--root", "claude:x"],
        ["discover-agents", "--root", "claude:x", "--bogus"],
        ["discover-agents", "--h", "--root", "claude:x"],
        ["discover-agents", "--he"],
        ["discover-agents", "--help=x", "--root", "claude:x"],
        ["discover-agents", "--root=claude:x"],
        ["discover-agents", "--root"],
    ):
        _both(rust_wrkviz, list(args))
