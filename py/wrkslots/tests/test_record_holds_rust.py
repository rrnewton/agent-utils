"""Differential: wrkslotsd parse-holds against the Python hold loader.

#562 rust-primary-wrkslots slice 19. The Python oracle is
`_load_record_holds` over per-machine snapshot states
(`_load_active_snapshot(..., require_repository=False)`) on fixture
projects; the Rust command must report the same per-record hold (or
captured error) read-only.
Snapshot mode only: a machine with a complete EVENTS log derives its
holds, and the Rust command refuses that case toward `rebuild` (the
Python oracle would replay, so that case asserts the Rust refusal
only). A log holding only in-progress `.tmp.` files stays in snapshot
mode on both sides.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path
from typing import cast

import pytest
from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]
SHA = "a" * 40
DIGEST = "b" * 64


def _root(tmp_path: Path) -> Path:
    root = tmp_path / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
    )
    return root


def _identity(pid: int) -> dict[str, object]:
    return {
        "pid": pid,
        "start_ticks": 1000 + pid,
        "boot_id": "boot-1",
        "host_id": "host-1",
        "cgroup_path": "/user.slice/session.scope",
    }


def _record(slot: str, agent: str, machine: str) -> dict[str, object]:
    return {
        "slot": slot,
        "agent": agent,
        "task": "do work",
        "purpose": "testing",
        "machine": machine,
        "generation": 1,
        "created_at": "2026-10-26T00:00:00Z",
        "heartbeat_at": "2026-10-26T01:00:00Z",
        "heartbeat_ttl_seconds": 60,
        "owner": _identity(123),
        "coordinator_lease": _identity(456),
        "coordinator_recovery_note": None,
        "handoff": None,
        "checkouts": [
            {
                "name": "repo",
                "path": f"worktrees/{slot}/repo",
                "repository": "../upstream-repo",
                "branch": "suite/work",
                "start_point": SHA,
                "remote": "origin",
                "remote_url_sha256": DIGEST,
                "landed_ref": "refs/remotes/origin/main",
                "head": SHA,
                "containing_remote_refs": ["refs/remotes/origin/main"],
                "vcs": "git",
            }
        ],
    }


def _write_machine(
    root: Path, machine: str, records: list[dict[str, object]]
) -> None:
    control = root / "worktrees"
    (control / f"ACTIVE.{machine}.json").write_text(
        json.dumps({"schema": 2, "machine": machine, "revision": 7, "slots": records})
    )
    (control / f"ARCHIVED.{machine}.json").write_text(
        json.dumps({"schema": 2, "machine": machine, "revision": 3, "records": []})
    )


def _hold(root: Path, machine: str, slot: str, reason: str = "waiting") -> None:
    payload = {
        "schema": 1,
        "machine": machine,
        "slot": slot,
        "held_at": "2026-10-26T03:00:00Z",
        "reason": reason,
    }
    name = f"HOLD.{len(machine)}.{machine}.{slot}.json"
    (root / "worktrees" / name).write_text(json.dumps(payload))


def _expected(root: Path) -> dict[str, object]:
    config = wrkslots_cli._load_config(str(root), None)
    states = [
        wrkslots_cli._load_active_snapshot(config, machine, require_repository=False)
        for machine in wrkslots_cli._state_machines(config)
    ]
    results = wrkslots_cli._load_record_holds(config, states)
    holds: list[dict[str, object]] = []
    for state in states:
        for record in state.slots:
            result = results[(record.machine, record.slot)]
            holds.append(
                {
                    "machine": record.machine,
                    "slot": record.slot,
                    "hold": result.value,
                    "error": result.error,
                }
            )
    return {"machines": [state.machine for state in states], "holds": holds}


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "parse-holds", "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def _fixture(root: Path) -> None:
    _write_machine(
        root,
        "node-a",
        [_record("slot-b", "agent-b", "node-a"), _record("slot-a", "agent-a", "node-a")],
    )
    _write_machine(root, "node-b", [_record("slot-c", "agent-c", "node-b")])
    _hold(root, "node-a", "slot-b")
    _hold(root, "node-b", "slot-c", reason="paused")


def test_parse_holds_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_parse_holds_invalid_hold_is_captured_like_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    _hold(root, "node-a", "slot-a", reason="  ")
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    expected = _expected(root)
    assert json.loads(result.stdout) == expected
    entry = next(
        item
        for item in cast(list[dict[str, object]], expected["holds"])
        if item["slot"] == "slot-a"
    )
    assert entry["hold"] is None
    assert "empty reason" in str(entry["error"])


def test_parse_holds_partial_event_write_stays_snapshot(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    events = root / "worktrees" / "EVENTS.node-a"
    events.mkdir()
    (events / "00000000000000000001.json.tmp.abc").write_text("{}")
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_parse_holds_invalid_event_log_is_captured_like_python(
    tmp_path: Path,
) -> None:
    root = _root(tmp_path)
    _fixture(root)
    events = root / "worktrees" / "EVENTS.node-a"
    events.mkdir()
    (events / "notes.txt").write_text("hi")
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    expected = _expected(root)
    assert json.loads(result.stdout) == expected
    errors = {
        str(item["error"])
        for item in cast(list[dict[str, object]], expected["holds"])
    }
    assert any("unexpected entry" in error for error in errors)


def _valid_event(root: Path, machine: str) -> None:
    events = root / "worktrees" / f"EVENTS.{machine}"
    events.mkdir(exist_ok=True)
    core: dict[str, object] = {
        "schema": 1,
        "machine": machine,
        "sequence": 1,
        "previous_sha256": "0" * 64,
        "recorded_at": "2026-10-26T03:00:00Z",
        "kind": "slot-held",
        "payload": {"slot": "slot-b", "reason": "waiting"},
    }
    event = {**core, "sha256": wrkslots_cli._event_digest(core)}
    (events / "00000000000000000001.json").write_text(json.dumps(event))


def test_parse_holds_refuses_event_derived_machine(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    _valid_event(root, "node-a")
    result = _rust(root)
    assert result.returncode == 1
    assert "is derived from the append-only event log" in result.stderr
    assert "use rebuild" in result.stderr


@pytest.mark.parametrize("name", ["00000000000000000001.json", "00000000000000000002.json"])
def test_parse_holds_corrupt_event_log_is_captured_like_python(
    tmp_path: Path, name: str
) -> None:
    root = _root(tmp_path)
    _fixture(root)
    events = root / "worktrees" / "EVENTS.node-a"
    events.mkdir()
    (events / name).write_text("{}")
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_parse_holds_shard_error_precedes_derived_refusal(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    _valid_event(root, "node-a")
    (root / "worktrees" / "ACTIVE.node-b.json").write_text("{")
    fragment = "active state is malformed JSON"
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr


@pytest.mark.parametrize("machine", ["node-a", "node-b"])
def test_parse_holds_machines_without_records(tmp_path: Path, machine: str) -> None:
    root = _root(tmp_path)
    _write_machine(root, machine, [])
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)
