"""Differential: wrkslotsd recover-unbound-owner against Python.

#562 rust-primary-wrkslots slice 38, the thirteenth writer slice.
The Python oracle is `_cmd_recover_unbound_owner` on a copy of the
fixture registry; the Rust writer runs on the original. Resulting
ACTIVE shards and event payloads must match apart from wall-clock
stamps and each fixture's own Git history. Temporary fixture
registries only: writers are never pointed at a live registry
before the edition cutover.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path
from typing import cast

import pytest
from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]
NOTE = "owner evidence lost in a reboot; coordinator inspected the slot"


def _git(cwd: Path, *args: str) -> str:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(
        {
            "LC_ALL": "C",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_SYSTEM": "/dev/null",
        }
    )
    result = subprocess.run(
        [
            "git", "-c", "user.email=test@example.invalid", "-c", "user.name=Test",
            "-c", "commit.gpgsign=false", "-C", str(cwd), *args,
        ],
        capture_output=True, text=True, check=True, env=env, timeout=120,
    )
    return result.stdout.strip()


def _repo(path: Path) -> str:
    path.mkdir(parents=True)
    _git(path, "init", "-q", "-b", "main")
    (path / "file.txt").write_text("one\n")
    _git(path, "add", "file.txt")
    _git(path, "commit", "-q", "-m", "one")
    return _git(path, "rev-parse", "HEAD")


def _live_identity() -> dict[str, object]:
    identity = wrkslots_cli._read_process_identity(os.getpid())
    return {
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    }


def _fixture(base: Path, owner: dict[str, object] | None) -> Path:
    root = base / "project"
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
    upstream = base / "upstream-repo"
    head = _repo(upstream)
    _git(upstream, "remote", "add", "origin", str(upstream))
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")
    _git(
        upstream, "worktree", "add", "-q", "-b", "suite/work",
        str(root / "worktrees" / "slot-a" / "repo"), "HEAD",
    )
    record: dict[str, object] = {
        "slot": "slot-a",
        "agent": "agent-slot-a",
        "task": "do work",
        "purpose": "testing",
        "machine": "node-a",
        "generation": 1,
        "created_at": "2026-10-26T00:00:00Z",
        "heartbeat_at": "2026-10-26T01:00:00Z",
        "heartbeat_ttl_seconds": 60,
        "owner": owner,
        "coordinator_lease": _live_identity(),
        "coordinator_recovery_note": None,
        "handoff": None,
        "checkouts": [
            {
                "name": "repo",
                "path": "worktrees/slot-a/repo",
                "repository": "../upstream-repo",
                "branch": "suite/work",
                "start_point": head,
                "remote": "origin",
                "remote_url_sha256": hashlib.sha256(str(upstream).encode()).hexdigest(),
                "landed_ref": "refs/remotes/origin/main",
                "head": head,
                "containing_remote_refs": ["refs/remotes/origin/main"],
                "vcs": "git",
            }
        ],
    }
    (root / "worktrees" / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 7, "slots": [record]})
    )
    (root / "worktrees" / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": []})
    )
    return root


def _pair(tmp_path: Path, owner: dict[str, object] | None) -> tuple[Path, Path]:
    return _fixture(tmp_path / "rust", owner), _fixture(tmp_path / "oracle", owner)


def _rust(root: Path, *, fixture: bool = True) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        [
            "cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
            "-p", "wrkslotsd", "--", "recover-unbound-owner", "slot-a",
            "--coordinator-pid", str(os.getpid()), "--expected-generation", "1",
            "--recovery-note", NOTE, "--validation", "checked the slot by hand",
            "--limitation", "owner identity unrecoverable",
            "--root", str(root),
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python(root: Path) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot="slot-a",
        coordinator_pid=os.getpid(), expected_generation=1,
        recovery_note=NOTE, validation=["checked the slot by hand"],
        limitation=["owner identity unrecoverable"], wait_lock=0.0,
    )
    return wrkslots_cli._cmd_recover_unbound_owner(args)


def _scrub_record(record: dict[str, object]) -> None:
    record.pop("heartbeat_at", None)
    handoff = record.get("handoff")
    if isinstance(handoff, dict):
        handoff.pop("recorded_at", None)
    for checkout in cast(list[dict[str, object]], record.get("checkouts", [])):
        checkout["head"] = "HEAD"
        checkout["start_point"] = "HEAD"
        checkout["remote_url_sha256"] = "URLSHA"


def _active(root: Path) -> dict[str, object]:
    doc: dict[str, object] = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
    for record in cast(list[dict[str, object]], doc["slots"]):
        _scrub_record(record)
    return doc


def _events(root: Path) -> list[dict[str, object]]:
    directory = root / "worktrees" / "EVENTS.node-a"
    if not directory.is_dir():
        return []
    events: list[dict[str, object]] = []
    for path in sorted(directory.iterdir()):
        event: dict[str, object] = json.loads(path.read_text())
        assert event["sha256"] == wrkslots_cli._event_digest(
            {k: v for k, v in event.items() if k != "sha256"}
        )
        event.pop("recorded_at")
        event.pop("sha256")
        event.pop("previous_sha256")
        payload = event.get("payload")
        if isinstance(payload, dict):
            # Digests the pre-update record, whose checkout heads are
            # each fixture's own Git history.
            if "previous_record_sha256" in payload:
                payload["previous_record_sha256"] = "DIGEST"
            records: list[dict[str, object]] = []
            record = payload.get("record")
            if isinstance(record, dict):
                records.append(record)
            active = payload.get("active")
            if isinstance(active, dict):
                records.extend(cast(list[dict[str, object]], active.get("slots", [])))
            for item in records:
                _scrub_record(item)
        events.append(event)
    return events


def test_recover_unbound_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path, None)
    assert _python(oracle_root) == 0
    result = _rust(rust_root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["outcome"] == "recorded coordinator recovery evidence"
    assert _active(rust_root) == _active(oracle_root)
    assert _events(rust_root) == _events(oracle_root)


def test_recover_unbound_historical_owner_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path, _live_identity())
    with pytest.raises(Exception) as caught:
        _python(oracle_root)
    assert "has a historical owner" in str(caught.value)
    result = _rust(rust_root)
    assert result.returncode == 1
    assert "has a historical owner" in result.stderr
    assert _active(rust_root)["revision"] == 7


def test_recover_unbound_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust", None)
    result = _rust(rust_root, fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
    assert _active(rust_root)["revision"] == 7
