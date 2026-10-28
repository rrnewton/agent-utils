"""Differential: wrkslotsd transfer-owner against the Python mutation.

#562 rust-primary-wrkslots slice 37, the twelfth writer slice. The
Python oracle is `_cmd_transfer_owner` on a copy of the fixture
registry; the Rust writer runs on the original. Resulting ACTIVE
shards and event kinds must match apart from wall-clock stamps.
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


def _dead_identity() -> dict[str, object]:
    proc = subprocess.Popen(["sleep", "30"])
    try:
        identity = wrkslots_cli._read_process_identity(proc.pid)
        return {
            "pid": identity.pid,
            "start_ticks": identity.start_ticks,
            "boot_id": identity.boot_id,
            "host_id": identity.host_id,
            "cgroup_path": identity.cgroup_path,
        }
    finally:
        proc.kill()
        proc.wait()


def _live_identity(pid: int) -> dict[str, object]:
    identity = wrkslots_cli._read_process_identity(pid)
    return {
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    }


def _record(head: str, owner: dict[str, object] | None) -> dict[str, object]:
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
        "owner": owner,
        "coordinator_lease": owner,
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
    _git(
        upstream, "worktree", "add", "-q", "-b", "suite/work",
        str(root / "worktrees" / "slot-a" / "repo"), "HEAD",
    )
    record = _record(head, owner)
    record["coordinator_lease"] = _live_identity(os.getpid())
    (root / "worktrees" / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 7, "slots": [record]})
    )
    (root / "worktrees" / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": []})
    )
    return root


def _pair(tmp_path: Path, owner: dict[str, object] | None) -> tuple[Path, Path]:
    return _fixture(tmp_path / "rust", owner), _fixture(tmp_path / "oracle", owner)


def _rust(root: Path, owner_pid: int, *, fixture: bool = True) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        [
            "cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
            "-p", "wrkslotsd", "--", "transfer-owner", "slot-a",
            "--agent", "agent-slot-a", "--owner-pid", str(owner_pid),
            "--expected-generation", "1", "--root", str(root),
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python(root: Path, owner_pid: int) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot="slot-a", agent="agent-slot-a",
        owner_pid=owner_pid, expected_generation=1, wait_lock=0.0,
    )
    return wrkslots_cli._cmd_transfer_owner(args)


def _active(root: Path) -> dict[str, object]:
    doc: dict[str, object] = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
    for record in cast(list[dict[str, object]], doc["slots"]):
        record.pop("heartbeat_at")
        for checkout in cast(list[dict[str, object]], record["checkouts"]):
            checkout["head"] = "HEAD"
            checkout["start_point"] = "HEAD"
    return doc


def _kinds(root: Path) -> list[str]:
    return [str(event["kind"]) for event in _events(root)]


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
            record = payload.get("record")
            if isinstance(record, dict):
                record.pop("heartbeat_at", None)
                for checkout in cast(list[dict[str, object]], record.get("checkouts", [])):
                    checkout["head"] = "HEAD"
                    checkout["start_point"] = "HEAD"
        events.append(event)
    return events


def test_transfer_dead_owner_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path, _dead_identity())
    assert _python(oracle_root, os.getpid()) == 0
    result = _rust(rust_root, os.getpid())
    assert result.returncode == 0, result.stderr
    outcome = json.loads(result.stdout)
    assert outcome["already_transferred"] is False
    assert outcome["basis"] == "caller-ancestry"
    assert outcome["previous_owner_state"] == "dead"
    assert _active(rust_root) == _active(oracle_root)
    assert _events(rust_root) == _events(oracle_root)


def test_transfer_unowned_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path, None)
    assert _python(oracle_root, os.getpid()) == 0
    result = _rust(rust_root, os.getpid())
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["previous_owner_state"] == "indeterminate"
    assert _active(rust_root) == _active(oracle_root)
    assert _events(rust_root) == _events(oracle_root)


def test_transfer_repeat_is_already_transferred(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path, _dead_identity())
    assert _python(oracle_root, os.getpid()) == 0
    assert _rust(rust_root, os.getpid()).returncode == 0
    assert _python(oracle_root, os.getpid()) == 0
    result = _rust(rust_root, os.getpid())
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout)["already_transferred"] is True
    assert _active(rust_root) == _active(oracle_root)
    assert _events(rust_root) == _events(oracle_root)


def test_transfer_live_owner_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path, _live_identity(os.getpid()))
    with pytest.raises(Exception) as caught:
        _python(oracle_root, os.getppid())
    assert "only replaces an owner that is provably dead" in str(caught.value)
    result = _rust(rust_root, os.getppid())
    assert result.returncode == 1
    assert "only replaces an owner that is provably dead" in result.stderr
    assert _active(rust_root)["revision"] == 7


def test_transfer_outsider_successor_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path, _dead_identity())
    outsider = subprocess.Popen(["sleep", "60"])
    try:
        with pytest.raises(Exception) as caught:
            _python(oracle_root, outsider.pid)
        assert "neither in the invoking process ancestry" in str(caught.value)
        result = _rust(rust_root, outsider.pid)
        assert result.returncode == 1
        assert "neither in the invoking process ancestry" in result.stderr
    finally:
        outsider.kill()
        outsider.wait()
    assert _active(rust_root)["revision"] == 7


def test_transfer_stale_generation_and_wrong_agent_refuse(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust", _dead_identity())
    result = _rust(rust_root, os.getpid())
    assert result.returncode == 0
    again = subprocess.run(
        [
            "cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
            "-p", "wrkslotsd", "--", "transfer-owner", "slot-a",
            "--agent", "agent-slot-a", "--owner-pid", str(os.getpid()),
            "--expected-generation", "9", "--root", str(rust_root),
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
        env={**os.environ, "WRKSLOTSD_FIXTURE_WRITER": "1"},
    )
    assert again.returncode == 1
    assert "stale generation" in again.stderr


def test_transfer_indeterminate_owner_refuses(tmp_path: Path) -> None:
    owner = _dead_identity()
    owner["host_id"] = "another-host"
    rust_root, oracle_root = _pair(tmp_path, owner)
    with pytest.raises(Exception) as caught:
        _python(oracle_root, os.getpid())
    assert "only replaces an owner that is provably dead" in str(caught.value)
    result = _rust(rust_root, os.getpid())
    assert result.returncode == 1
    assert "only replaces an owner that is provably dead" in result.stderr


def _release_with_dying_owner(root: Path, source_root: Path) -> None:
    """Publish + release from a helper whose exit makes the owner dead."""
    source = source_root / "notes.md"
    source_root.mkdir(parents=True, exist_ok=True)
    source.write_text("# Handoff\n\nAll done.\n")
    helper = (
        "import argparse, json, os, sys\n"
        "from pathlib import Path\n"
        "from wrkslots import cli\n"
        "root = sys.argv[1]; source = sys.argv[2]\n"
        "active = Path(root) / 'worktrees' / 'ACTIVE.node-a.json'\n"
        "doc = json.loads(active.read_text())\n"
        "identity = cli._read_process_identity(os.getpid())\n"
        "me = {'pid': identity.pid, 'start_ticks': identity.start_ticks,"
        " 'boot_id': identity.boot_id, 'host_id': identity.host_id,"
        " 'cgroup_path': identity.cgroup_path}\n"
        "doc['slots'][0]['owner'] = me\n"
        "doc['slots'][0]['coordinator_lease'] = me\n"
        "active.write_text(json.dumps(doc))\n"
        "base = dict(project_root=root, machine=None, slot='slot-a',"
        " agent='agent-slot-a', owner_pid=os.getpid(), expected_generation=1,"
        " wait_lock=0.0)\n"
        "cli._cmd_write_handoff(argparse.Namespace(**base, from_file=source))\n"
        "cli._cmd_release(argparse.Namespace(**base))\n"
    )
    env = dict(os.environ)
    env["PYTHONPATH"] = str(AU_ROOT / "py")
    proc = subprocess.run(
        ["python3", "-c", helper, str(root), str(source)],
        capture_output=True, text=True, check=False, timeout=300, env=env,
    )
    assert proc.returncode == 0, proc.stderr


def test_transfer_released_generation_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path, None)
    _release_with_dying_owner(rust_root, tmp_path / "rust-src")
    _release_with_dying_owner(oracle_root, tmp_path / "oracle-src")
    with pytest.raises(Exception) as caught:
        _python(oracle_root, os.getpid())
    assert "was released by its owner" in str(caught.value)
    result = _rust(rust_root, os.getpid())
    assert result.returncode == 1
    assert "was released by its owner" in result.stderr


def test_transfer_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust", _dead_identity())
    result = _rust(rust_root, os.getpid(), fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
    assert _active(rust_root)["revision"] == 7
