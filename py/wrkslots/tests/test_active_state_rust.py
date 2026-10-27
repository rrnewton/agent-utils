"""Differential: wrkslotsd parse-state against the Python shard loader.

#562 rust-primary-wrkslots slice 3. Fixture projects carry an
ACTIVE.<machine>.json shard; the Python loader's normalized state
(`_active_to_obj` over `_load_active_snapshot(..., require_repository=False)`)
and the Rust command must agree, including refusal messages.
"""

from __future__ import annotations

import json
import subprocess
from collections.abc import Callable
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
        "checkouts": [_checkout(slot)],
    }


def _write_state(root: Path, state: dict[str, object]) -> Path:
    path = root / "worktrees" / "ACTIVE.node-a.json"
    path.write_text(json.dumps(state))
    return path


def _state(*records: dict[str, object]) -> dict[str, object]:
    return {"schema": 2, "machine": "node-a", "revision": 7, "slots": list(records)}


def _expected(root: Path) -> dict[str, object]:
    config = wrkslots_cli._load_config(str(root), None)
    state = wrkslots_cli._load_active_snapshot(config, require_repository=False)
    return wrkslots_cli._active_to_obj(state)


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "parse-state", "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def test_parse_state_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _write_state(root, _state(_record("slot-b", "agent-b"), _record("slot-a", "agent-a")))
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def test_parse_state_imported_record_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    record = _record("slot-old", "agent-old")
    record["checkouts"] = []
    record["layout"] = "nested"
    owner = record["owner"]
    assert isinstance(owner, dict)
    row: dict[str, object] = {
        "status": "active",
        "allocated": record["created_at"],
        "task": record["task"],
        "purpose": record["purpose"],
        "agents": [{"name": "agent-old", "read_only": False, "task": record["task"]}],
        "owner_sidecar": {
            "supervisor_pid": owner["pid"],
            "start_ticks": owner["start_ticks"],
            "boot_id": owner["boot_id"],
            "cgroup_path": owner["cgroup_path"],
            "slot": "slot-old",
            "agent": "agent-old",
        },
    }
    record["import_source"] = {
        "format": "worktree-state-v3",
        "path": "worktree-state.json",
        "file_sha256": DIGEST,
        "row_sha256": wrkslots_cli._event_digest(row),
        "status": "active",
        "row": row,
    }
    _write_state(root, _state(record))
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


def _slots(state: dict[str, object]) -> list[dict[str, object]]:
    return cast(list[dict[str, object]], state["slots"])


def _checkout0(state: dict[str, object]) -> dict[str, object]:
    return cast(list[dict[str, object]], _slots(state)[0]["checkouts"])[0]


@pytest.mark.parametrize(
    ("mutate", "fragment"),
    [
        (lambda state: state.update(schema=3), "unsupported active state schema in"),
        (lambda state: _slots(state)[0].update(bogus=1), "has invalid fields"),
        (lambda state: _slots(state).append(_slots(state)[0]), "duplicate slot in"),
        (lambda state: _slots(state)[0].update(machine="node-b"), "expected shard"),
        (lambda state: _slots(state)[0].update(coordinator_lease=None), "has no coordinator lease"),
        (lambda state: _slots(state)[0].update(created_at="soon"), "invalid timestamp in"),
        (
            lambda state: _checkout0(state).update(path="worktrees/slot-a/other"),
            "path does not match its slot",
        ),
        (
            lambda state: _checkout0(state).update(remote_url_sha256="zz"),
            "must be a lowercase SHA-256",
        ),
    ],
)
def test_parse_state_refusals_name_the_same_problem(
    tmp_path: Path, mutate: Callable[[dict[str, object]], None], fragment: str
) -> None:
    root = _root(tmp_path)
    state = _state(_record("slot-a", "agent-a"))
    mutate(state)
    _write_state(root, state)
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr


def test_parse_state_missing_shard(tmp_path: Path) -> None:
    root = _root(tmp_path)
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert "missing active state" in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert "missing active state" in result.stderr
