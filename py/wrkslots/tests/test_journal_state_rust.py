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


FENCED_TOKEN = "c" * 32


def _ownerless_journal(root: Path, with_exclusion: bool) -> dict[str, object]:
    raw_fenced = f"worktrees/validate/.slot-a.ownerless-validate.{FENCED_TOKEN}"
    journal: dict[str, object] = {
        "schema": 2,
        "kind": "ownerless-validate-remove",
        "machine": "node-a",
        "slot": "slot-a",
        "phase": "prepared",
        "fenced": raw_fenced,
        "authorization": {
            "target_kind": "checkout",
            "path": "worktrees/validate/slot-a",
            "identity": [1, 2, 3],
            "actor": _identity(789),
            "evidence": {},
            "no_authored_work": True,
            "no_live_use": True,
            "repository": None,
            "head": None,
            "remote_url_sha256": None,
            "parent_identity": None,
        },
    }
    if with_exclusion:
        config = wrkslots_cli._load_config(str(root), None)
        guard_name = wrkslots_cli._ownerless_validation_exclusion_name(
            config, config.root / raw_fenced
        )
        journal["exclusion"] = {
            "schema": 1,
            "phase": "guard-created",
            "root": "/data/misc",
            "root_identity": [1, 2, 3],
            "guard": f"/data/misc/{guard_name}",
            "guard_identity": [4, 5, 6],
            "source_alias": "/data/misc/alias-source",
            "payload_identity": [1, 2],
        }
    return journal


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


def _absent_journal() -> dict[str, object]:
    record = _record("slot-a", "agent-a")
    record["slot_type"] = "validate"
    record_obj = wrkslots_cli._record_from_obj(record, "fixture record")
    row = wrkslots_cli.AbsentValidateRow(
        machine="node-a",
        slot="slot-a",
        generation=1,
        record_sha256=wrkslots_cli._record_sha256(record_obj),
    )
    return {
        "schema": 2,
        "kind": "recover-absent-validate-rows",
        "machine": "node-a",
        "host_id": "host-1",
        "slot": "absent-validate-rows",
        "input_sha256": DIGEST,
        "cursor": 0,
        "actor": _identity(789),
        "items": [
            {
                "input": {
                    "machine": "node-a",
                    "slot": "slot-a",
                    "generation": 1,
                    "record_sha256": row.record_sha256,
                },
                "record": record,
                "archive_entry": wrkslots_cli._absent_validate_archive_entry(
                    record_obj, row, "2026-10-26T02:00:00Z"
                ),
            }
        ],
    }


def test_parse_journal_absent_rows_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    name = "ACTIVE.node-a.journal"
    path = _write(root, name, _absent_journal())
    result = _rust(root, name)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root, path)


@pytest.mark.parametrize(
    ("mutate", "fragment"),
    [
        ("cursor", "cursor exceeds its item count"),
        ("archive", "differs from its exact input row"),
        ("agent", "is not a validation row"),
    ],
)
def test_parse_journal_absent_rows_refusals(tmp_path: Path, mutate: str, fragment: str) -> None:
    root = _root(tmp_path)
    name = "ACTIVE.node-a.journal"
    journal = _absent_journal()
    if mutate == "cursor":
        journal["cursor"] = 5
    elif mutate == "archive":
        items = journal["items"]
        assert isinstance(items, list) and isinstance(items[0], dict)
        entry = items[0]["archive_entry"]
        assert isinstance(entry, dict)
        entry["continuation"] = "tampered"
    else:
        items = journal["items"]
        assert isinstance(items, list) and isinstance(items[0], dict)
        record = items[0]["record"]
        assert isinstance(record, dict)
        record["slot_type"] = "agent"
    path = _write(root, name, journal)
    with pytest.raises(Exception) as caught:
        _expected(root, path)
    assert fragment in str(caught.value)
    result = _rust(root, name)
    assert result.returncode == 1
    assert fragment in result.stderr


@pytest.mark.parametrize("with_exclusion", [False, True])
def test_parse_journal_ownerless_matches_python(tmp_path: Path, with_exclusion: bool) -> None:
    root = _root(tmp_path)
    name = "ACTIVE.node-a.journal"
    path = _write(root, name, _ownerless_journal(root, with_exclusion))
    result = _rust(root, name)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root, path)


@pytest.mark.parametrize(
    ("with_exclusion", "fragment"),
    [(False, "unknown ownerless validation phase"), (True, "paths are not canonical")],
)
def test_parse_journal_ownerless_refusals(tmp_path: Path, with_exclusion: bool, fragment: str) -> None:
    root = _root(tmp_path)
    name = "ACTIVE.node-a.journal"
    journal = _ownerless_journal(root, with_exclusion)
    if with_exclusion:
        exclusion = journal["exclusion"]
        assert isinstance(exclusion, dict)
        exclusion["guard"] = "/data/misc/.wrkslots-ownerless-validation.nope"
    else:
        journal["phase"] = "bogus"
    path = _write(root, name, journal)
    with pytest.raises(Exception) as caught:
        _expected(root, path)
    assert fragment in str(caught.value)
    result = _rust(root, name)
    assert result.returncode == 1
    assert fragment in result.stderr


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
