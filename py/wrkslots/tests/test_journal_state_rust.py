"""Differential: wrkslotsd parse-journal against `_validate_journal_shape`.

#562 rust-primary-wrkslots slice 7. Fixture projects carry one recovery
journal; the Python shape validator (which inspects no journal targets)
and the Rust command must agree, including refusal messages. The shape
validator returns nothing, so the expected output is the journal as stored.
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


def _record(slot: str, agent: str) -> dict[str, object]:
    return {
        "slot": slot,
        "agent": agent,
        "task": "do work",
        "purpose": "testing",
        "machine": "node-a",
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


def _create_journal() -> dict[str, object]:
    return {
        "schema": 2,
        "kind": "create",
        "machine": "node-a",
        "slot": "slot-a",
        "agent": "agent-a",
        "task": "do work",
        "purpose": "testing",
        "created_at": "2026-10-26T00:00:00Z",
        "heartbeat_at": "2026-10-26T01:00:00Z",
        "heartbeat_ttl_seconds": 60,
        "owner": _identity(123),
        "coordinator_lease": _identity(456),
        "planned": [
            {
                "name": "repo",
                "destination": "worktrees/slot-a/repo",
                "repository": "../upstream-repo",
                "branch": "suite/work",
                "start_point": SHA,
                "remote": "origin",
                "remote_url_sha256": DIGEST,
                "landed_ref": "refs/remotes/origin/main",
            }
        ],
        "created": [],
    }


def _finish_journal() -> dict[str, object]:
    return {
        "schema": 2,
        "kind": "finish",
        "machine": "node-a",
        "slot": "slot-a",
        "mode": "remove",
        "actor": "coordinator",
        "finished_at": "2026-10-26T02:00:00Z",
        "archive_id": "node-a:slot-a:1:2026-10-26T02:00:00Z",
        "phase": "prepared",
        "fenced": "yes",
        "removed": [],
        "record": _record("slot-a", "agent-a"),
    }


def _import_journal() -> dict[str, object]:
    return {
        "schema": 2,
        "kind": "import-existing",
        "machine": "node-a",
        "slot": "slot-a",
        "record": _record("slot-a", "agent-a"),
    }


def _legacy_journal() -> dict[str, object]:
    return {
        "schema": 2,
        "kind": "legacy-validate-remove",
        "machine": "node-a",
        "slot": "slot-a",
        "phase": "prepared",
        "checkout": "worktrees/validate/slot-a/repo",
        "repository": "../upstream-repo",
        "completed_record": "record",
        "completed_record_sha256": DIGEST,
        "head": SHA,
        "actor": _identity(789),
        "coordinator_authorized": True,
    }


def _write(root: Path, name: str, payload: dict[str, object]) -> Path:
    path = root / "worktrees" / name
    path.write_text(json.dumps(payload))
    return path


def _expected(root: Path, path: Path) -> dict[str, object]:
    config = wrkslots_cli._load_config(str(root), None)
    raw = json.loads(path.read_text())
    wrkslots_cli._validate_journal_shape(config, path, raw)
    return cast(dict[str, object], raw)


def _rust(root: Path, name: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "parse-journal", "--root", str(root), "--name", name],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


@pytest.mark.parametrize(
    ("name", "payload"),
    [
        ("CREATE.6.node-a.6.slot-a.journal", _create_journal()),
        ("ACTIVE.node-a.journal", _import_journal()),
        ("ACTIVE.node-a.journal", _finish_journal()),
        ("ACTIVE.node-a.journal", _legacy_journal()),
    ],
)
def test_parse_journal_matches_python(
    tmp_path: Path, name: str, payload: dict[str, object]
) -> None:
    root = _root(tmp_path)
    path = _write(root, name, payload)
    result = _rust(root, name)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root, path)


@pytest.mark.parametrize(
    ("name", "payload", "fragment"),
    [
        (
            "CREATE.6.node-a.6.slot-a.journal",
            {**_create_journal(), "planned": []},
            "contains no planned checkouts",
        ),
        (
            "CREATE.6.node-a.6.slot-b.journal",
            _create_journal(),
            "filename does not match its machine, operation, and slot",
        ),
        (
            "ACTIVE.node-a.journal",
            {**_finish_journal(), "archive_id": "node-a:slot-a:9:later"},
            "archive_id does not match its record",
        ),
        (
            "ACTIVE.node-a.journal",
            {**_import_journal(), "slot": "slot-b"},
            "does not match its shard or slot",
        ),
        (
            "ACTIVE.node-a.journal",
            {**_legacy_journal(), "coordinator_authorized": "yes"},
            "coordinator_authorized must be boolean",
        ),
        (
            "ACTIVE.node-a.journal",
            {**_legacy_journal(), "actor": None},
            "has no actor",
        ),
        (
            "ACTIVE.node-a.journal",
            {"schema": 2, "kind": "teleport", "machine": "node-a", "slot": "slot-a"},
            "unsupported recovery journal kind",
        ),
    ],
)
def test_parse_journal_refusals_name_the_same_problem(
    tmp_path: Path,
    name: str,
    payload: dict[str, object],
    fragment: str,
) -> None:
    root = _root(tmp_path)
    path = _write(root, name, payload)
    with pytest.raises(Exception) as caught:
        _expected(root, path)
    assert fragment in str(caught.value)
    result = _rust(root, name)
    assert result.returncode == 1
    assert fragment in result.stderr
