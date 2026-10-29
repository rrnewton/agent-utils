"""Differential: wrkslotsd parse-storage-git against the Python diagnosis.

#562 rust-primary-wrkslots slice 25. The Python oracle is
`_registry_storage_inconsistencies` (non-tolerating) over fixture
projects backed by real Git repositories and worktrees; the Rust
command must agree on the Git findings (registration and checkout
identity). Filesystem findings are covered by parse-storage and are
filtered out of the oracle here.
"""

from __future__ import annotations

import json
import os
import shutil
import subprocess
from pathlib import Path
from typing import cast

import pytest
from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]
DIGEST = "b" * 64
KINDS = {
    "repository-evidence-unavailable",
    "git-registration-without-directory",
    "unsafe-checkout-path",
    "checkout-without-git-registration",
    "git-worktree-unreadable",
    "git-worktree-mismatch",
    "git-registration-without-row",
}


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


def _record(slot: str, head: str) -> dict[str, object]:
    identity = {
        "pid": 123,
        "start_ticks": 1123,
        "boot_id": "boot-1",
        "host_id": "host-1",
        "cgroup_path": "/user.slice/session.scope",
    }
    return {
        "slot": slot,
        "agent": f"agent-{slot}",
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
                "path": f"worktrees/{slot}/repo",
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


def _fixture(tmp_path: Path) -> Path:
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
    upstream = tmp_path / "upstream-repo"
    head = _repo(upstream)
    worktrees = root / "worktrees"
    _git(upstream, "worktree", "add", "-q", "-b", "suite/work", str(worktrees / "slot-a" / "repo"), "HEAD")
    _git(upstream, "worktree", "add", "-q", "-b", "suite/stale", str(worktrees / "slot-c" / "repo"), "HEAD")
    shutil.rmtree(worktrees / "slot-c" / "repo")
    _repo(worktrees / "slot-b" / "repo")
    _git(upstream, "worktree", "add", "-q", "-b", "suite/orphan", str(worktrees / "orphan" / "repo"), "HEAD")
    records = [_record(slot, head) for slot in ("slot-a", "slot-b", "slot-c")]
    (worktrees / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 7, "slots": records})
    )
    (worktrees / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": []})
    )
    return root


def _expected(root: Path) -> dict[str, object]:
    config = wrkslots_cli._load_config(str(root), None)
    states = [
        wrkslots_cli._load_active_snapshot(config, machine, require_repository=False)
        for machine in wrkslots_cli._state_machines(config)
    ]
    findings = [
        item.to_obj()
        for item in wrkslots_cli._registry_storage_inconsistencies(config, states)
        if item.kind in KINDS
    ]
    return {"machines": [state.machine for state in states], "findings": findings}


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "parse-storage-git", "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def test_parse_storage_git_missing_repository_refuses_like_python(
    tmp_path: Path,
) -> None:
    root = _fixture(tmp_path)
    active_path = root / "worktrees" / "ACTIVE.node-a.json"
    active = json.loads(active_path.read_text())
    record = _record("slot-e", "0" * 40)
    checkout = cast(list[dict[str, object]], record["checkouts"])[0]
    record["checkouts"] = [
        {**checkout, "path": "worktrees/slot-e/repo", "repository": "../missing-repo"}
    ]
    active["slots"].append(record)
    active_path.write_text(json.dumps(active))
    (root / "worktrees" / "slot-e" / "repo").mkdir(parents=True)
    fragment = "source repository '../missing-repo' does not exist or cannot be resolved"
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert fragment in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert fragment in result.stderr


def test_parse_storage_git_unmounted_image_matches_python(
    tmp_path: Path,
) -> None:
    root = _fixture(tmp_path)
    control = root / "worktrees"
    # slot-b's checkout becomes a real registered worktree of the
    # upstream repository, inside what is now an unmounted image's
    # mount point: the census must still record it (no orphan
    # finding), while no Git inspection runs against it.
    shutil.rmtree(control / "slot-b" / "repo")
    _git(
        tmp_path / "upstream-repo", "worktree", "add", "-q", "-b",
        "suite/img", str(control / "slot-b" / "repo"), "HEAD",
    )
    image_dir = control / "slot-images" / "agent" / "slot-b"
    image_dir.mkdir(parents=True)
    (image_dir / "slot.img").write_bytes(b"")
    (image_dir / "IMAGE.json").write_text(
        json.dumps(
            {
                "schema": "wrkslots-slot-image/v1",
                "slot": "slot-b",
                "slot_type": "agent",
                "phase": "ready",
                "backend": "kernel",
                "location": str(control / "slot-b"),
                "created_at": "2026-10-26T00:00:00Z",
                "ceiling_bytes": 1024,
                "state_ceiling_bytes": 1024,
            }
        )
    )
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    actual = json.loads(result.stdout)
    assert actual == _expected(root)
    slot_b = [item["kind"] for item in actual["findings"] if item["slot"] == "slot-b"]
    assert slot_b == []


def test_parse_storage_git_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    actual = json.loads(result.stdout)
    assert actual == _expected(root)
    kinds = [(item["kind"], item["slot"]) for item in actual["findings"]]
    assert kinds == [
        ("git-registration-without-row", "orphan"),
        ("checkout-without-git-registration", "slot-b"),
        ("git-registration-without-directory", "slot-c"),
    ]
