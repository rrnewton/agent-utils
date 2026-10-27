"""Differential: wrkslotsd parse-handoff against the Python sidecar loader.

#562 rust-primary-wrkslots slice 5. Fixture projects carry an ACTIVE shard
plus a generation-bound handoff sidecar; `_load_handoff_sidecar` bound to
the active record (serialized with `_handoff_artifact_to_obj`) and the
Rust command must agree, including refusal messages.
"""

from __future__ import annotations

import hashlib
import json
import subprocess
from collections.abc import Callable
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]
SHA = "a" * 40
DIGEST = "b" * 64
CONTENTS = "hello handoff\n"
CONTENT_DIGEST = hashlib.sha256(CONTENTS.encode()).hexdigest()


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


def _write_active(root: Path) -> None:
    state = {"schema": 2, "machine": "node-a", "revision": 1, "slots": [_record("slot-a", "agent-a")]}
    (root / "worktrees" / "ACTIVE.node-a.json").write_text(json.dumps(state))


def _sidecar() -> dict[str, object]:
    return {
        "schema": 2,
        "machine": "node-a",
        "slot": "slot-a",
        "generation": 1,
        "recorded_at": "2026-10-26T02:00:00Z",
        "source": "write-handoff",
        "source_path": "/tmp/handoff-source.md",
        "source_identity": {
            "identity": [11, 22],
            "size": len(CONTENTS.encode()),
            "sha256": CONTENT_DIGEST,
        },
        "contents_utf8": CONTENTS,
        "sha256": CONTENT_DIGEST,
    }


def _write_sidecar(root: Path, payload: dict[str, object] | str) -> Path:
    path = root / "worktrees" / "HANDOFF.6.node-a.6.slot-a.1.json"
    path.write_text(payload if isinstance(payload, str) else json.dumps(payload))
    return path


def _expected(root: Path) -> dict[str, object] | None:
    config = wrkslots_cli._load_config(str(root), None)
    state = wrkslots_cli._load_active_snapshot(config, require_repository=False)
    record = next(item for item in state.slots if item.slot == "slot-a")
    artifact = wrkslots_cli._load_handoff_sidecar(config, record)
    if artifact is None:
        return None
    return wrkslots_cli._handoff_artifact_to_obj(artifact)


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "parse-handoff", "--root", str(root), "--slot", "slot-a"],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def test_parse_handoff_absent_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _write_active(root)
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) is None
    assert _expected(root) is None


def test_parse_handoff_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _write_active(root)
    _write_sidecar(root, _sidecar())
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _expected(root)


@pytest.mark.parametrize(
    ("mutate", "fragment"),
    [
        (lambda sidecar: sidecar.update(sha256=DIGEST), "digest does not match its content"),
        (lambda sidecar: sidecar.update(slot="slot-b"), "identity does not match slot generation"),
        (lambda sidecar: sidecar.update(source_identity=None), "lacks source file identity"),
        (lambda sidecar: sidecar.update(source="telepathy"), ".source is invalid"),
    ],
)
def test_parse_handoff_refusals_name_the_same_problem(
    tmp_path: Path, mutate: Callable[[dict[str, object]], None], fragment: str
) -> None:
    root = _root(tmp_path)
    _write_active(root)
    sidecar = _sidecar()
    mutate(sidecar)
    _write_sidecar(root, sidecar)
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr


def test_parse_handoff_duplicate_key_refused(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _write_active(root)
    payload = json.dumps(_sidecar())
    _write_sidecar(root, payload.replace('"schema": 2', '"schema": 2, "schema": 2', 1))
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert "is malformed JSON" in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert "is malformed JSON" in result.stderr


def test_parse_handoff_legacy_sidecar_needs_provenance_upgrade(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _write_active(root)
    legacy = _sidecar()
    legacy["schema"] = 1
    legacy["source"] = "legacy-slot-file"
    del legacy["source_path"]
    payload = {key: value for key, value in legacy.items()}
    _write_sidecar(root, payload)
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert "must be provenance-upgraded" in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert "must be provenance-upgraded" in result.stderr
