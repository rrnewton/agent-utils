"""Differential: wrkslotsd heartbeat against the Python heartbeat mutation.

#562 rust-primary-wrkslots slice 29, the fourth writer slice. The
Python oracle is `_cmd_heartbeat` on a copy of the fixture registry;
the Rust writer runs on the original. Resulting ACTIVE shards and
`active-state-recorded` events must match apart from the wall-clock
stamps (and digests, re-verified with the Python digest function).
Temporary fixture registries only: writers are never pointed at a
live registry before the edition cutover.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
from pathlib import Path
from typing import cast

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


def _live_identity() -> dict[str, object]:
    identity = wrkslots_cli._read_process_identity(os.getpid())
    return {
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    }


def _record(head: str) -> dict[str, object]:
    identity = _live_identity()
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




def _rust_hb(
    root: Path,
    slot: str,
    agent: str,
    owner_pid: int,
    generation: int,
    *,
    fixture: bool = True,
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "heartbeat", slot, "--agent", agent,
         "--owner-pid", str(owner_pid), "--expected-generation", str(generation),
         "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python_hb(root: Path, slot: str, agent: str, owner_pid: int, generation: int) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot=slot, agent=agent,
        owner_pid=owner_pid, expected_generation=generation, wait_lock=0.0,
    )
    return wrkslots_cli._cmd_heartbeat(args)


def _active(root: Path) -> dict[str, object]:
    doc: dict[str, object] = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
    for record in doc["slots"]:  # type: ignore[attr-defined]
        record.pop("heartbeat_at")
    return doc


def test_heartbeat_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    assert _python_hb(oracle_root, "slot-a", "agent-slot-a", os.getpid(), 1) == 0
    result = _rust_hb(rust_root, "slot-a", "agent-slot-a", os.getpid(), 1)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["generation"] == 1
    assert _active(rust_root) == _active(oracle_root)
    assert _active(rust_root)["revision"] == 8
    rust_events = _events(rust_root)
    oracle_events = _events(oracle_root)
    assert [event["kind"] for event in rust_events] == [
        "state-imported",
        "active-state-recorded",
    ]
    assert [event["kind"] for event in oracle_events] == [
        "state-imported",
        "active-state-recorded",
    ]
    for event in (*rust_events, *oracle_events):
        payload = cast(dict[str, object], event["payload"])
        if event["kind"] == "active-state-recorded":
            record = cast(dict[str, object], payload["record"])
            previous = dict(record)
            record.pop("heartbeat_at")
            previous["heartbeat_at"] = "2026-10-26T01:00:00Z"
            assert payload["previous_record_sha256"] == wrkslots_cli._event_digest(previous)
            payload.pop("previous_record_sha256")
            evidence = cast(dict[str, object], payload["evidence"])
            assert set(evidence) == {"heartbeat_at"}
            payload["evidence"] = {}
            for checkout in cast(list[dict[str, object]], record["checkouts"]):
                checkout["head"] = "HEAD"
                checkout["start_point"] = "HEAD"
    assert rust_events == oracle_events


def test_heartbeat_stale_generation_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    with pytest.raises(Exception) as caught:
        _python_hb(oracle_root, "slot-a", "agent-slot-a", os.getpid(), 2)
    assert "stale generation for slot slot-a" in str(caught.value)
    result = _rust_hb(rust_root, "slot-a", "agent-slot-a", os.getpid(), 2)
    assert result.returncode == 1
    assert "stale generation for slot slot-a" in result.stderr
    assert _active(rust_root)["revision"] == 7


def test_heartbeat_agent_mismatch_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    with pytest.raises(Exception) as caught:
        _python_hb(oracle_root, "slot-a", "other-agent", os.getpid(), 1)
    assert "ownership mismatch for slot slot-a" in str(caught.value)
    result = _rust_hb(rust_root, "slot-a", "other-agent", os.getpid(), 1)
    assert result.returncode == 1
    assert "ownership mismatch for slot slot-a" in result.stderr


def test_heartbeat_owner_mismatch_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    with pytest.raises(Exception) as caught:
        _python_hb(oracle_root, "slot-a", "agent-slot-a", os.getppid(), 1)
    assert "owner process generation mismatch for slot slot-a" in str(caught.value)
    result = _rust_hb(rust_root, "slot-a", "agent-slot-a", os.getppid(), 1)
    assert result.returncode == 1
    assert "owner process generation mismatch for slot slot-a" in result.stderr


def test_heartbeat_dead_owner_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    pid = 4194300
    while True:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            break
        except PermissionError:
            pid += 1
            continue
        pid += 1
    with pytest.raises(Exception) as caught:
        _python_hb(oracle_root, "slot-a", "agent-slot-a", pid, 1)
    assert "is not live" in str(caught.value)
    result = _rust_hb(rust_root, "slot-a", "agent-slot-a", pid, 1)
    assert result.returncode == 1
    assert "is not live" in result.stderr


def test_heartbeat_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "plain")
    result = _rust_hb(rust_root, "slot-a", "agent-slot-a", os.getpid(), 1, fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
    assert _active(rust_root)["revision"] == 7
