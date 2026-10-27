"""Differential: wrkslotsd hold against the Python hold mutation.

#562 rust-primary-wrkslots slice 27, the second writer slice. The
Python oracle is `_cmd_hold` on a copy of the fixture registry; the
Rust writer runs on the original. Resulting HOLD records and
`slot-held` events must match apart from the wall-clock stamps (and
the event digest, re-verified with the Python digest function).
Temporary fixture registries only: writers are never pointed at a
live registry before the edition cutover.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]
DIGEST = "b" * 64
HOLD_NAME = "HOLD.6.node-a.slot-a.json"


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
    _git(path, "init", "-q")
    (path / "file.txt").write_text("one\n")
    _git(path, "add", "file.txt")
    _git(path, "commit", "-q", "-m", "one")
    return _git(path, "rev-parse", "HEAD")


def _record(head: str) -> dict[str, object]:
    identity = {
        "pid": 123,
        "start_ticks": 1123,
        "boot_id": "boot-1",
        "host_id": "host-1",
        "cgroup_path": "/user.slice/session.scope",
    }
    return {
        "slot": "slot-a",
        "agent": "agent-slot-a",
        "task": "do work",
        "purpose": "testing",
        "machine": "node-a",
        "generation": 1,
        "created_at": "2026-10-26T00:00:00Z",
        "heartbeat_at": "2026-10-26T01:00:00Z",
        "heartbeat_ttl_seconds": 60,
        "owner": identity,
        "coordinator_lease": identity,
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
                "remote_url_sha256": DIGEST,
                "landed_ref": "refs/remotes/origin/main",
                "head": head,
                "containing_remote_refs": ["refs/remotes/origin/main"],
                "vcs": "git",
            }
        ],
    }


def _fixture(base: Path) -> Path:
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
    _git(
        upstream, "worktree", "add", "-q", "-b", "suite/work",
        str(root / "worktrees" / "slot-a" / "repo"), "HEAD",
    )
    (root / "worktrees" / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 7, "slots": [_record(head)]})
    )
    (root / "worktrees" / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": []})
    )
    return root


def _rust(root: Path, slot: str, reason: str, *, fixture: bool = True) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "hold", slot, "--reason", reason, "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python(root: Path, slot: str, reason: str) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot=slot, reason=reason, wait_lock=0.0
    )
    return wrkslots_cli._cmd_hold(args)


def _pair(tmp_path: Path) -> tuple[Path, Path]:
    return _fixture(tmp_path / "rust"), _fixture(tmp_path / "oracle")


def _hold_record(root: Path) -> dict[str, object] | None:
    path = root / "worktrees" / HOLD_NAME
    if not path.exists():
        return None
    record: dict[str, object] = json.loads(path.read_text())
    record.pop("held_at")
    return record


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
        # Each fixture pair has its own Git history, so commit ids differ.
        payload = event.get("payload")
        if isinstance(payload, dict):
            active = payload.get("active")
            if isinstance(active, dict):
                for record in active.get("slots", []):
                    for checkout in record.get("checkouts", []):
                        checkout["head"] = "HEAD"
                        checkout["start_point"] = "HEAD"
        events.append(event)
    return events


def test_hold_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    assert _python(oracle_root, "slot-a", "keep it") == 0
    result = _rust(rust_root, "slot-a", "keep it")
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["already_held"] is False
    assert _hold_record(rust_root) == _hold_record(oracle_root)
    assert _events(rust_root) == _events(oracle_root)
    assert (rust_root / "worktrees" / "ACTIVE.node-a.json").read_bytes() == (
        oracle_root / "worktrees" / "ACTIVE.node-a.json"
    ).read_bytes()


def test_hold_already_held_same_reason_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    assert _python(oracle_root, "slot-a", "keep it") == 0
    assert _rust(rust_root, "slot-a", "keep it").returncode == 0
    assert _python(oracle_root, "slot-a", "keep it") == 0
    result = _rust(rust_root, "slot-a", "keep it")
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["already_held"] is True
    assert _events(rust_root) == _events(oracle_root)
    assert len(_events(rust_root)) == 2


def test_hold_already_held_different_reason_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    assert _python(oracle_root, "slot-a", "first") == 0
    assert _rust(rust_root, "slot-a", "first").returncode == 0
    with pytest.raises(Exception) as caught:
        _python(oracle_root, "slot-a", "second")
    assert "already held for a different reason" in str(caught.value)
    result = _rust(rust_root, "slot-a", "second")
    assert result.returncode == 1
    assert "already held for a different reason" in result.stderr
    assert len(_events(rust_root)) == 2


def test_hold_missing_slot_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    with pytest.raises(Exception) as caught:
        _python(oracle_root, "slot-z", "keep it")
    assert "is not active on machine node-a" in str(caught.value)
    result = _rust(rust_root, "slot-z", "keep it")
    assert result.returncode == 1
    assert "is not active on machine node-a" in result.stderr
    assert _hold_record(rust_root) is None


def test_hold_empty_reason_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    with pytest.raises(Exception) as caught:
        _python(oracle_root, "slot-a", "  ")
    assert "non-empty --reason" in str(caught.value)
    result = _rust(rust_root, "slot-a", "  ")
    assert result.returncode == 1
    assert "non-empty --reason" in result.stderr
    assert _hold_record(rust_root) is None


def test_hold_partial_state_refuses_without_writing(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    for root in (rust_root, oracle_root):
        (root / "worktrees" / "ACTIVE.node-a.json.tmp.1").write_text("{}")
    with pytest.raises(Exception) as caught:
        _python(oracle_root, "slot-a", "keep it")
    assert "partial atomic update found" in str(caught.value)
    result = _rust(rust_root, "slot-a", "keep it")
    assert result.returncode == 1
    assert "partial atomic update found" in result.stderr
    assert _hold_record(rust_root) is None
    assert _events(rust_root) == []


def test_hold_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "plain")
    result = _rust(rust_root, "slot-a", "keep it", fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
    assert _hold_record(rust_root) is None


def test_hold_seal_journal_refuses_without_writing(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    for root in (rust_root, oracle_root):
        (root / "worktrees" / "VALIDATE-BATCH-SEAL.node-a.journal").write_text("{}")
    with pytest.raises(Exception) as caught:
        _python(oracle_root, "slot-a", "keep it")
    assert "interrupted validation-batch seal" in str(caught.value)
    result = _rust(rust_root, "slot-a", "keep it")
    assert result.returncode == 1
    assert "interrupted validation-batch seal" in result.stderr
    assert _hold_record(rust_root) is None


def test_hold_derived_log_decides_already_held(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    assert _python(oracle_root, "slot-a", "keep it") == 0
    assert _rust(rust_root, "slot-a", "keep it").returncode == 0
    for root in (rust_root, oracle_root):
        (root / "worktrees" / HOLD_NAME).unlink()
    assert _python(oracle_root, "slot-a", "keep it") == 0
    result = _rust(rust_root, "slot-a", "keep it")
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["already_held"] is True
    assert _events(rust_root) == _events(oracle_root)
    assert len(_events(rust_root)) == 2


def test_hold_active_archived_overlap_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    archive_record = {
        "archive_id": "node-a:slot-a:1:2026-10-26T02:00:00Z",
        "slot": "slot-a",
        "agent": "agent-slot-a",
        "task": "do work",
        "purpose": "testing",
        "machine": "node-a",
        "generation": 1,
        "created_at": "2026-10-26T00:00:00Z",
        "finished_at": "2026-10-26T02:00:00Z",
        "mode": "remove",
        "actor": "coordinator",
        "physical_storage": "removed",
        "validation": ["gate ok"],
        "limitations": [],
        "continuation": "none needed",
        "checkouts": [],
    }
    for root in (rust_root, oracle_root):
        active = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
        archive_record["checkouts"] = active["slots"][0]["checkouts"]
        (root / "worktrees" / "ARCHIVED.node-a.json").write_text(
            json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": [archive_record]})
        )
    with pytest.raises(Exception) as caught:
        _python(oracle_root, "slot-a", "keep it")
    assert "is active on node-a and archived on node-a" in str(caught.value)
    result = _rust(rust_root, "slot-a", "keep it")
    assert result.returncode == 1
    assert "is active on node-a and archived on node-a" in result.stderr
    assert _hold_record(rust_root) is None
