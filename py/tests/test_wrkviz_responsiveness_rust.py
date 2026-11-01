"""Differential: Rust `wrkviz responsiveness` vs the oracle.

One agentctl registry plus coordinator/child transcripts (modeled
on the oracle's own responsiveness fixture) exercises every
notification path, both child harnesses, idle-gap splitting, the
shell-command scanner (for-loops, heredocs, variable expansion,
wrkslots slot actions), window bounds, exclusions, and refusals.
The command is read-only, so both editions run against the same
fixture and stdout/stderr/exit must agree byte-for-byte; the
--episodes files must agree too.
"""

from __future__ import annotations

import json
import subprocess
import sys
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


def _iso(minutes: float) -> str:
    return datetime.fromtimestamp(T0 + minutes * 60, timezone.utc).isoformat().replace("+00:00", "Z")


def _at(minutes: float) -> float:
    return T0 + minutes * 60


def _write_jsonl(path: Path, records: list[dict[str, object]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("".join(json.dumps(record) + "\n" for record in records), encoding="utf-8")


def _write_json(path: Path, record: dict[str, object]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(record), encoding="utf-8")


def _user(minutes: float, text: str) -> dict[str, object]:
    return {"type": "user", "timestamp": _iso(minutes),
            "message": {"role": "user", "content": text}}


def _assistant(minutes: float, text: str = "", command: str | None = None) -> dict[str, object]:
    content: list[dict[str, object]] = []
    if text:
        content.append({"type": "text", "text": text})
    if command is not None:
        content.append({"type": "tool_use", "id": "t", "name": "Bash",
                        "input": {"command": command}})
    return {"type": "assistant", "timestamp": _iso(minutes),
            "message": {"role": "assistant", "content": content}}


def _turn_end(minutes: float) -> dict[str, object]:
    return {"type": "system", "subtype": "turn_duration", "timestamp": _iso(minutes)}


def _sweep(minutes: float) -> dict[str, object]:
    return {"type": "system", "subtype": "scheduled_task_fire", "timestamp": _iso(minutes),
            "prompt": "[sweep] read every agent"}


def _muse_run(minutes: float, event: dict[str, object]) -> dict[str, object]:
    return {"recorded_at": int(_at(minutes) * 1e6), "payload": {"kind": "run", "event": event}}


def _team(tmp_path: Path) -> dict[str, Path]:
    registry = tmp_path / "registry"
    _write_json(registry / "coordinator" / "agent.json",
                {"name": "coordinator", "harness": "claude"})
    _write_json(registry / "archive" / "cl-a-1" / "agent.json", {
        "name": "cl-a", "harness": "claude", "created_at": _at(0), "cwd": "/w/slot-a",
        "slot": "slot-a", "native_session": {"value": "sess-a"}})
    _write_json(registry / "mu-b" / "agent.json", {
        "name": "mu-b", "harness": "muse", "created_at": _at(0), "cwd": "/w/slot-b",
        "slot": "slot-b"})
    _write_json(registry / "coordinator" / "queue" / "processed" / "1.json", {
        "id": "01-1", "text": "cl-a: gate green, may I push?",
        "queued_at": _at(9.5), "confirmed_at": _at(10.5)})
    _write_json(registry / "coordinator" / "queue" / "processed" / "2.json", {
        "id": "agent-watch-abc",
        "text": "[agentctl watch-agents] 1 agent(s) need attention:\n- mu-b: idle",
        "queued_at": _at(50), "confirmed_at": _at(50.5)})
    _write_json(registry / "archive" / "cl-a-1" / "queue" / "processed" / "3.json", {
        "id": "01-3", "text": "[coordinator] push",
        "queued_at": _at(12), "confirmed_at": _at(12.2)})
    coordinator = tmp_path / "coord.jsonl"
    _write_jsonl(coordinator, [
        _user(0, "start the team"), _assistant(1, command="agentctl start cl-a"), _turn_end(2),
        _user(10.5, "cl-a: gate green, may I push?"),
        _assistant(11, command="agentctl read cl-a --lines 30"),
        _assistant(12, command="agentctl send cl-a --file brief"), _turn_end(12.5),
        _sweep(40), _assistant(40.2, command="agentctl stop cl-a; agentctl read mu-b"),
        _turn_end(41),
        _user(50.5, "[agentctl watch-agents] 1 agent(s) need attention:\n- mu-b: idle"),
        _assistant(51, command="agentctl stop mu-b"), _turn_end(52),
    ])
    projects = tmp_path / "projects"
    _write_jsonl(projects / "-w-slot-a" / "sess-a.jsonl", [
        _user(1, "brief"), _assistant(9, "Gate green; may I push?"), _turn_end(10),
        _user(12.2, "[coordinator] push"), _assistant(14, "Running the gate."), _turn_end(15),
        _user(30, "<task-notification>gate done</task-notification>"),
        _assistant(31, "DONE: landed"), _turn_end(32),
    ])
    sessions = tmp_path / "muse" / "sessions"
    _write_jsonl(sessions / "2026" / "01" / "05" / "s1" / "session.jsonl", [
        {"recorded_at": int(_at(0.1) * 1e6),
         "payload": {"kind": "route_facts", "record": {"cwd": "/w/slot-b/"}}},
        _muse_run(1, {"kind": "started", "prompt": "brief"}),
        _muse_run(20, {"kind": "assistant_message_committed", "text": "DONE: shipped"}),
        _muse_run(21, {"kind": "terminal"}),
    ])
    return {
        "registry": registry, "coordinator": coordinator, "projects": projects,
        "sessions": sessions,
    }


def _run(command: list[str], args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command + args, cwd=AU_ROOT, capture_output=True, text=True,
        check=False, timeout=300,
    )


def _both(args: list[str], expect: int = 0, episodes: bool = False,
          tmp_path: Path | None = None) -> dict[str, subprocess.CompletedProcess[str]]:
    outcomes = {}
    episode_bytes = {}
    root = tmp_path if tmp_path is not None else Path()
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(RUST_WRKVIZ)])):
        extra = []
        if episodes:
            target = root / f"episodes-{label}.json"
            extra = ["--episodes", str(target)]
        outcomes[label] = _run(command, ["responsiveness", *args, *extra])
        if episodes:
            episode_bytes[label] = (root / f"episodes-{label}.json").read_bytes()
    oracle, result = outcomes["py"], outcomes["rs"]
    assert oracle.returncode == expect, oracle.stderr
    assert result.returncode == oracle.returncode
    assert result.stdout == oracle.stdout
    assert result.stderr == oracle.stderr
    if episodes:
        assert episode_bytes["py"] == episode_bytes["rs"]
    return outcomes


def _args(team: dict[str, Path]) -> list[str]:
    return [
        "--registry", str(team["registry"]),
        "--coordinator-transcript", str(team["coordinator"]),
        "--claude-projects", str(team["projects"]),
        "--muse-sessions", str(team["sessions"]),
        "--sweep-marker", "[sweep]",
    ]


def test_responsiveness_markdown_json_and_episodes(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    team = _team(tmp_path)
    outcomes = _both(_args(team), episodes=True, tmp_path=tmp_path)
    assert "Parent-child responsiveness" in outcomes["py"].stdout
    assert "| path | child-message |" in outcomes["py"].stdout
    outcomes = _both([*_args(team), "--format", "json"], episodes=True, tmp_path=tmp_path)
    figures = json.loads(outcomes["py"].stdout)
    assert figures["episodes"] == 3
    assert figures["coordinator"]["inputs"] == {
        "child-message": 1, "other": 1, "sweep": 1, "watch": 1}


def test_responsiveness_selection_and_windows(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    team = _team(tmp_path)
    outcomes = _both([*_args(team), "--exclude", "mu-*", "--format", "json"])
    assert json.loads(outcomes["py"].stdout)["episodes"] == 2
    outcomes = _both([*_args(team), "--until", _iso(15), "--format", "json"])
    assert json.loads(outcomes["py"].stdout)["episodes"] == 1
    _both([*_args(team), "--since", _iso(11), "--until", _iso(35), "--format", "json"])
    # Without a sweep marker every scheduled-task fire is a sweep.
    args = [arg for arg in _args(team) if arg != "--sweep-marker" and arg != "[sweep]"]
    _both([*args, "--format", "json"])


def test_responsiveness_unconfirmed_message(tmp_path: Path, rust_wrkviz: Path) -> None:
    team = _team(tmp_path)
    _write_json(team["registry"] / "coordinator" / "queue" / "processed" / "1.json", {
        "id": "01-1", "text": "cl-a: gate green, may I push?", "queued_at": _at(9.5)})
    outcomes = _both([*_args(team), "--format", "json"])
    paths = json.loads(outcomes["py"].stdout)["act_latency_by_path"]
    assert paths["child-message"]["n"] == 1


def test_responsiveness_command_scanner(tmp_path: Path, rust_wrkviz: Path) -> None:
    team = _team(tmp_path)
    # A second coordinator transcript exercising the scanner: a
    # killed turn, a for-loop stop, a heredoc whose body names
    # another child, variable expansion into wrkslots, and a slot
    # handoff read that removes the slot's owner.
    _write_jsonl(team["coordinator"], [
        _user(0, "start the team"), _assistant(1, command="agentctl start cl-a"),
        _user(10.5, "cl-a: gate green, may I push?"),
        _assistant(11, command="agentctl send mu-b --file - <<'EOF'\nagentctl stop cl-a\nEOF"),
        _assistant(12, command="agentctl send cl-a --file brief"), _turn_end(12.5),
        _sweep(40),
        _assistant(40.2, command="WS=/x/wrkslots; $WS remove slot-a"),
        _assistant(41, command="for n in cl-a mu-b; do agentctl stop $n; done"),
        _turn_end(42),
        _user(50.5, "[agentctl watch-agents] 1 agent(s) need attention:\n- mu-b: idle"),
        _assistant(51, command="timeout 5 agentctl stop mu-b"), _turn_end(52),
    ])
    _both([*_args(team), "--format", "json"], episodes=True, tmp_path=tmp_path)
    _both(_args(team))


def test_responsiveness_refusals(tmp_path: Path, rust_wrkviz: Path) -> None:
    team = _team(tmp_path)
    _both(["--registry", str(team["registry"]),
           "--coordinator-transcript", str(tmp_path / "missing.jsonl")], expect=2)
    _both(["--registry", str(tmp_path / "missing-registry"),
           "--coordinator-transcript", str(team["coordinator"])], expect=2)
    _both([*_args(team), "--until", "2026-01-05T13:00:00"], expect=2)
    _both([*_args(team), "--since", "not-an-instant"], expect=2)
    _both(["--registry", str(team["registry"])], expect=2)
    empty = tmp_path / "empty.jsonl"
    _write_jsonl(empty, [_user(0, "hello")])
    _both(["--registry", str(team["registry"]),
           "--coordinator-transcript", str(empty),
           "--claude-projects", str(team["projects"])], expect=2)


def test_responsiveness_help(rust_wrkviz: Path) -> None:
    oracle = _run([sys.executable, str(PYTHON_WRKVIZ)], ["responsiveness", "--help"])
    result = _run([str(RUST_WRKVIZ)], ["responsiveness", "--help"])
    assert result.returncode == oracle.returncode == 0
    assert result.stdout == oracle.stdout
    assert result.stderr == oracle.stderr
