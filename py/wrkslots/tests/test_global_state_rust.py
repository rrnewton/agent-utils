"""Differential: wrkslotsd parse-global against the Python global loader.

#562 rust-primary-wrkslots slice 18. The Python oracle is
`_validate_global_state(..., require_repository=False)` over fixture
projects with shards for two machines; the Rust command must report the
same registry read-only, including cross-machine refusal messages.
Snapshot mode only: a machine with a complete EVENTS log derives its
state, and the Rust command refuses that case toward `rebuild` (the
Python oracle would replay, so that case asserts the Rust refusal only).
A log holding only in-progress `.tmp.` files stays in snapshot mode on
both sides, and any other log entry refuses on both sides.
"""

from __future__ import annotations

import json
import subprocess
from collections.abc import Callable
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]
SHA = "a" * 40
DIGEST = "b" * 64
FINISHED = "2026-10-26T02:00:00Z"


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


def _checkout(slot: str) -> dict[str, object]:
    return {
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


def _active_record(slot: str, agent: str, machine: str) -> dict[str, object]:
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
        "checkouts": [_checkout(slot)],
    }


def _archive_record(slot: str, agent: str, machine: str) -> dict[str, object]:
    return {
        "archive_id": f"{machine}:{slot}:1:{FINISHED}",
        "slot": slot,
        "agent": agent,
        "task": "do work",
        "purpose": "testing",
        "machine": machine,
        "generation": 1,
        "created_at": "2026-10-26T00:00:00Z",
        "finished_at": FINISHED,
        "mode": "remove",
        "actor": "coordinator",
        "physical_storage": "removed",
        "validation": ["gate ok"],
        "limitations": [],
        "continuation": "none needed",
        "checkouts": [_checkout(slot)],
    }


def _write(
    root: Path,
    machine: str,
    active: list[dict[str, object]] | None,
    archive: list[dict[str, object]] | None,
) -> None:
    control = root / "worktrees"
    if active is not None:
        (control / f"ACTIVE.{machine}.json").write_text(
            json.dumps(
                {"schema": 2, "machine": machine, "revision": 7, "slots": active}
            )
        )
    if archive is not None:
        (control / f"ARCHIVED.{machine}.json").write_text(
            json.dumps(
                {"schema": 2, "machine": machine, "revision": 3, "records": archive}
            )
        )


def _expected(root: Path) -> dict[str, object]:
    config = wrkslots_cli._load_config(str(root), None)
    states, archives = wrkslots_cli._validate_global_state(
        config, require_repository=False
    )
    by_machine = {
        state.machine: (
            wrkslots_cli._active_to_obj(state),
            wrkslots_cli._archive_to_obj(archive),
        )
        for state, archive in zip(states, archives, strict=True)
    }
    machines = sorted(by_machine)
    return {
        "machines": machines,
        "states": [
            {
                "machine": machine,
                "active": by_machine[machine][0],
                "archive": by_machine[machine][1],
            }
            for machine in machines
        ],
    }


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "parse-global", "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def test_parse_global_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _write(
        root,
        "node-a",
        [_active_record("slot-a", "agent-a", "node-a")],
        [_archive_record("slot-old", "agent-old", "node-a")],
    )
    _write(root, "node-b", [_active_record("slot-b", "agent-b", "node-b")], [])
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_parse_global_empty_machines_match_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _write(root, "node-a", [], [])
    _write(root, "node-b", [], [])
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def _pair(root: Path) -> None:
    _write(root, "node-a", [], [])
    _write(root, "node-b", [], [])


def _setup_active_in_both(root: Path) -> None:
    _write(root, "node-a", [_active_record("slot-a", "agent-a", "node-a")], [])
    _write(root, "node-b", [_active_record("slot-a", "agent-b", "node-b")], [])


def _setup_agent_owns_both(root: Path) -> None:
    _write(root, "node-a", [_active_record("slot-a", "agent-a", "node-a")], [])
    _write(root, "node-b", [_active_record("slot-b", "agent-a", "node-b")], [])


def _setup_archived_in_both(root: Path) -> None:
    _write(root, "node-a", [], [_archive_record("slot-old", "agent-a", "node-a")])
    _write(root, "node-b", [], [_archive_record("slot-old", "agent-b", "node-b")])


def _setup_active_and_archived(root: Path) -> None:
    _write(root, "node-a", [_active_record("slot-a", "agent-a", "node-a")], [])
    _write(root, "node-b", [], [_archive_record("slot-a", "agent-b", "node-b")])


def _setup_lone_active(root: Path) -> None:
    _write(root, "node-a", [], None)


def _setup_bad_active_name(root: Path) -> None:
    _pair(root)
    (root / "worktrees" / "ACTIVE.bad!.json").write_text("{}")


def _setup_events_not_directory(root: Path) -> None:
    _pair(root)
    (root / "worktrees" / "EVENTS.node-c").write_text("not a directory")


@pytest.mark.parametrize(
    ("setup", "fragment"),
    [
        (_setup_active_in_both, "slot 'slot-a' is active in both node-a and node-b"),
        (_setup_agent_owns_both, "agent 'agent-a' owns both slot-a and slot-b"),
        (
            _setup_archived_in_both,
            "slot 'slot-old' is archived in both node-a and node-b",
        ),
        (
            _setup_active_and_archived,
            "slot 'slot-a' is active on node-a and archived on node-b",
        ),
        (
            _setup_lone_active,
            "ACTIVE and ARCHIVED machine shards differ without append-only history",
        ),
        (_setup_bad_active_name, "unexpected active-state filename"),
        (_setup_events_not_directory, "append-only event log is not a real directory"),
    ],
)
def test_parse_global_refusals_name_the_same_problem(
    tmp_path: Path,
    setup: Callable[[Path], None],
    fragment: str,
) -> None:
    root = _root(tmp_path)
    setup(root)
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr


def test_parse_global_derived_machine_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _pair(root)
    control = root / "worktrees"
    active = json.loads((control / "ACTIVE.node-a.json").read_text())
    archive = json.loads((control / "ARCHIVED.node-a.json").read_text())
    core: dict[str, object] = {
        "schema": 1,
        "machine": "node-a",
        "sequence": 1,
        "previous_sha256": "0" * 64,
        "recorded_at": "2026-10-26T03:00:00Z",
        "kind": "state-imported",
        "payload": {"active": active, "archive": archive, "holds": []},
    }
    event = {**core, "sha256": wrkslots_cli._event_digest(core)}
    events = control / "EVENTS.node-a"
    events.mkdir()
    (events / "00000000000000000001.json").write_text(json.dumps(event))
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_parse_global_corrupt_event_log_refuses_like_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _pair(root)
    events = root / "worktrees" / "EVENTS.node-c"
    events.mkdir()
    (events / "00000000000000000001.json").write_text("{}")
    result = _rust(root)
    assert result.returncode == 1
    assert "append-only event" in result.stderr


def test_parse_global_partial_event_write_stays_snapshot(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _pair(root)
    events = root / "worktrees" / "EVENTS.node-a"
    events.mkdir()
    (events / "00000000000000000001.json.tmp.abc").write_text("{}")
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_parse_global_unexpected_event_entry_refuses_like_python(
    tmp_path: Path,
) -> None:
    root = _root(tmp_path)
    _pair(root)
    events = root / "worktrees" / "EVENTS.node-a"
    events.mkdir()
    (events / "notes.txt").write_text("hi")
    fragment = "append-only event log contains an unexpected entry"
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr


def test_parse_global_shard_error_precedes_derived_refusal(
    tmp_path: Path,
) -> None:
    root = _root(tmp_path)
    _pair(root)
    (root / "worktrees" / "ACTIVE.node-a.json").write_text("{")
    events = root / "worktrees" / "EVENTS.node-b"
    events.mkdir()
    (events / "00000000000000000001.json").write_text("{}")
    fragment = "active state is malformed JSON"
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr


def test_parse_global_first_conflict_follows_shard_order(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _write(
        root,
        "node-a",
        [
            _active_record("slot-z", "agent-g1", "node-a"),
            _active_record("slot-a", "agent-g2", "node-a"),
        ],
        [],
    )
    _write(
        root,
        "node-b",
        [
            _active_record("slot-z", "agent-g3", "node-b"),
            _active_record("slot-a", "agent-g4", "node-b"),
        ],
        [],
    )
    fragment = "slot 'slot-z' is active in both node-a and node-b"
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr
