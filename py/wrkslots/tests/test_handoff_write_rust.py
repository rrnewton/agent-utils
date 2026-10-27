"""Differential: wrkslotsd heartbeat against the Python heartbeat mutation.

#562 rust-primary-wrkslots slice 30, the fifth writer slice. The
Python oracle is `_cmd_write_handoff` on a copy of the fixture registry;
the Rust writer runs on the original. Resulting handoff sidecars and publication events must match apart
from the wall-clock stamps (and digests, re-verified with the Python
digest function).
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




SIDECAR_NAME = "HANDOFF.6.node-a.6.slot-a.1.json"


def _source(base: Path) -> Path:
    base.mkdir(parents=True, exist_ok=True)
    path = base / "notes.md"
    path.write_text("# Handoff\n\nAll done.\n")
    return path


def _rust_wh(
    root: Path, source: Path, *, fixture: bool = True
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "write-handoff", "slot-a", "--agent", "agent-slot-a",
         "--owner-pid", str(os.getpid()), "--expected-generation", "1",
         "--from-file", str(source), "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python_wh(root: Path, source: Path) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot="slot-a", agent="agent-slot-a",
        owner_pid=os.getpid(), expected_generation=1, from_file=str(source),
        wait_lock=0.0,
    )
    return wrkslots_cli._cmd_write_handoff(args)


def _sidecar(root: Path) -> dict[str, object] | None:
    path = root / "worktrees" / SIDECAR_NAME
    if not path.exists():
        return None
    doc: dict[str, object] = json.loads(path.read_text())
    doc.pop("recorded_at")
    identity = cast(dict[str, object], doc["source_identity"])
    identity.pop("identity")
    return doc


def test_write_handoff_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    rust_source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    assert _python_wh(oracle_root, oracle_source) == 0
    result = _rust_wh(rust_root, rust_source)
    assert result.returncode == 0, result.stderr
    rust_doc = _sidecar(rust_root)
    oracle_doc = _sidecar(oracle_root)
    assert rust_doc is not None and oracle_doc is not None
    assert rust_doc.pop("source_path") is not None
    oracle_doc.pop("source_path")
    assert rust_doc == oracle_doc
    assert [event["kind"] for event in _events(rust_root)] == [
        "state-imported",
        "handoff-write-intended",
        "handoff-write-completed",
        "handoff-written",
    ]
    assert [event["kind"] for event in _events(oracle_root)] == [
        "state-imported",
        "handoff-write-intended",
        "handoff-write-completed",
        "handoff-written",
    ]


def test_write_handoff_repeat_same_bytes_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    rust_source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    assert _python_wh(oracle_root, oracle_source) == 0
    assert _rust_wh(rust_root, rust_source).returncode == 0
    assert _python_wh(oracle_root, oracle_source) == 0
    result = _rust_wh(rust_root, rust_source)
    assert result.returncode == 0, result.stderr
    assert len(_events(rust_root)) == len(_events(oracle_root))


def test_write_handoff_different_bytes_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    rust_source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    assert _python_wh(oracle_root, oracle_source) == 0
    assert _rust_wh(rust_root, rust_source).returncode == 0
    (tmp_path / "rust-src" / "notes.md").write_text("# Changed\n")
    (tmp_path / "oracle-src" / "notes.md").write_text("# Changed\n")
    with pytest.raises(Exception) as caught:
        _python_wh(oracle_root, oracle_source)
    assert "immutable handoff sidecar" in str(caught.value)
    result = _rust_wh(rust_root, rust_source)
    assert result.returncode == 1
    assert "immutable handoff sidecar" in result.stderr


def test_write_handoff_non_utf8_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    rust_source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    rust_source.write_bytes(b"\xff\xfe")
    oracle_source.write_bytes(b"\xff\xfe")
    with pytest.raises(Exception) as caught:
        _python_wh(oracle_root, oracle_source)
    assert "is not UTF-8" in str(caught.value)
    result = _rust_wh(rust_root, rust_source)
    assert result.returncode == 1
    assert "is not UTF-8" in result.stderr
    assert _sidecar(rust_root) is None


def test_write_handoff_inside_git_tree_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    source = rust_root / "worktrees" / "slot-a" / "repo" / "notes.md"
    source.write_text("# Inside\n")
    result = _rust_wh(rust_root, source)
    assert result.returncode == 1
    assert "inside Git working tree" in result.stderr
    assert _sidecar(rust_root) is None


def test_write_handoff_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "plain")
    source = _source(tmp_path / "rust-src")
    result = _rust_wh(rust_root, source, fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
    assert _sidecar(rust_root) is None


def test_write_handoff_schema1_sidecar_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    sidecar = {
        "schema": 1,
        "machine": "node-a",
        "slot": "slot-a",
        "generation": 1,
        "recorded_at": "2026-10-26T01:00:00+00:00",
        "source": "write-handoff",
        "source_identity": {"identity": [1, 2], "size": 3, "sha256": "0" * 64},
        "contents_utf8": "abc",
        "sha256": __import__("hashlib").sha256(b"abc").hexdigest(),
    }
    sidecar["source_identity"] = {
        "identity": [1, 2],
        "size": 3,
        "sha256": sidecar["sha256"],
    }
    for root in (rust_root, oracle_root):
        (root / "worktrees" / SIDECAR_NAME).write_text(json.dumps(sidecar))
    source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    with pytest.raises(Exception) as caught:
        _python_wh(oracle_root, oracle_source)
    assert "schema 1 lacks auditable source provenance" in str(caught.value)
    result = _rust_wh(rust_root, source)
    assert result.returncode == 1
    assert "schema 1 lacks auditable source provenance" in result.stderr


def test_write_handoff_legacy_file_disagreeing_refuses(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    for root in (rust_root, oracle_root):
        (root / "worktrees" / "slot-a" / "HANDOFF.md").write_text("# Other bytes\n")
    source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    with pytest.raises(Exception) as caught:
        _python_wh(oracle_root, oracle_source)
    assert "legacy HANDOFF.md disagrees" in str(caught.value)
    result = _rust_wh(rust_root, source)
    assert result.returncode == 1
    assert "legacy HANDOFF.md disagrees" in result.stderr
    assert _sidecar(rust_root) is None


def test_write_handoff_legacy_file_agreeing_matches_python(tmp_path: Path) -> None:
    rust_root, oracle_root = _pair(tmp_path)
    for root in (rust_root, oracle_root):
        (root / "worktrees" / "slot-a" / "HANDOFF.md").write_text("# Handoff\n\nAll done.\n")
    source = _source(tmp_path / "rust-src")
    oracle_source = _source(tmp_path / "oracle-src")
    assert _python_wh(oracle_root, oracle_source) == 0
    result = _rust_wh(rust_root, source)
    assert result.returncode == 0, result.stderr
    assert _sidecar(rust_root) is not None
