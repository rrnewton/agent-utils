"""Differential: declared-submodule private history in wrkslotsd remove.

#1255/#1257: a declared submodule whose own repository holds
commits nothing durable reaches refuses removal by name, before
salvage, with the Python edition's exact text. When the recorded
remote advertises a tag reaching those commits, the private
history refusal must not fire (recursive submodule salvage
itself stays Python in this slice).
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd


AU_ROOT = Path(__file__).resolve().parents[3]


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
            "-c", "commit.gpgsign=false", "-c", "protocol.file.allow=always",
            "-C", str(cwd), *args,
        ],
        capture_output=True, text=True, check=True, env=env, timeout=120,
    )
    return result.stdout.strip()


def _commit(repo: Path, name: str, text: str) -> str:
    (repo / name).write_text(text)
    _git(repo, "add", name)
    _git(repo, "commit", "-q", "-m", name)
    return _git(repo, "rev-parse", "HEAD")


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


def _fixture(base: Path, *, cover_with_tag: bool) -> Path:
    leaf = base / "leaf-repo"
    leaf.mkdir(parents=True)
    _git(leaf, "init", "-q", "-b", "main")
    leaf_head = _commit(leaf, "leaf.txt", "leaf\n")

    root = base
    upstream = root / "upstream-repo"
    upstream.mkdir(parents=True)
    _git(upstream, "init", "-q", "-b", "main")
    (upstream / "file.txt").write_text("one\n")
    _git(upstream, "add", "file.txt")
    _git(upstream, "commit", "-q", "-m", "one")
    _git(
        upstream, "-c", "protocol.file.allow=always",
        "submodule", "add", "-q", "../leaf-repo", "leaf",
    )
    _git(upstream, "commit", "-q", "-m", "add leaf")
    head = _git(upstream, "rev-parse", "HEAD")
    _git(upstream, "remote", "add", "origin", str(upstream))
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")

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
    checkout = root / "worktrees" / "slot-a" / "repo"
    _git(
        upstream, "worktree", "add", "-q", "-b", "suite/work", str(checkout), "HEAD"
    )
    _git(checkout, "-c", "protocol.file.allow=always", "submodule", "update", "--init")
    sub = checkout / "leaf"
    # Two local-only commits on a local branch and an annotated tag.
    _git(sub, "checkout", "-q", "-b", "side")
    _commit(sub, "side1.txt", "side1\n")
    side_head = _commit(sub, "side2.txt", "side2\n")
    _git(sub, "tag", "-a", "kept", "-m", "kept", side_head)
    _git(sub, "checkout", "-q", "main")
    if cover_with_tag:
        _git(sub, "push", "-q", "origin", "refs/tags/kept")
    dead = _dead_identity()
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
        "owner": dead,
        "coordinator_lease": dead,
        "coordinator_recovery_note": None,
        "handoff": None,
        "checkouts": [
            {
                "name": "repo",
                "path": "worktrees/slot-a/repo",
                "repository": "upstream-repo",
                "branch": "suite/work",
                "start_point": head,
                "remote": "origin",
                "remote_url_sha256": hashlib.sha256(
                    str(upstream).encode()
                ).hexdigest(),
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
    assert leaf_head
    return root


def _rust_remove(root: Path) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    return subprocess.run(
        [
            str(rust_wrkslotsd()), "remove", "slot-a",
            "--coordinator-pid", str(os.getpid()), "--expected-generation", "1",
            "--root", str(root),
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def _python_refusal(root: Path) -> str | None:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot="slot-a",
        coordinator_authorized=False, validate_complete=False,
        coordinator_pid=os.getpid(), expected_generation=1,
        salvage_archive_root=None, validation_proof_manifest=None,
        completed_record=None, attest_agent_stopped=None,
        attest_agent_never_started=None, attest_note=None,
        archive_drifted_checkouts=False, adopt_legacy_change=False, wait_lock=0.0,
        no_lock_budget=True,
        archive_private_history=False,
    )
    try:
        wrkslots_cli._cmd_remove(args)
    except wrkslots_cli.Refusal as exc:
        return str(exc)
    return None


def test_remove_refuses_unpushed_private_history_like_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust", cover_with_tag=False)
    oracle_root = _fixture(tmp_path / "oracle", cover_with_tag=False)
    refusal = _python_refusal(oracle_root)
    assert refusal is not None and "unpushed private history in submodule repo/leaf" in refusal
    result = _rust_remove(rust_root)
    assert result.returncode == 1
    assert refusal in result.stderr
    assert (rust_root / "worktrees" / "slot-a").is_dir()


def test_remove_advertised_tag_cover_skips_private_history_refusal(
    tmp_path: Path,
) -> None:
    rust_root = _fixture(tmp_path / "rust", cover_with_tag=True)
    oracle_root = _fixture(tmp_path / "oracle", cover_with_tag=True)
    refusal = _python_refusal(oracle_root)
    assert refusal is None or "unpushed private history" not in refusal
    result = _rust_remove(rust_root)
    assert result.returncode == 1
    assert "unpushed private history" not in result.stderr
    assert "submodule salvage stays with the Python edition" in result.stderr
