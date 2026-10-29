"""Differential: wrkslotsd unpushed against the Python unpushed diagnosis.

#562 rust-primary-wrkslots slice 48. The Python oracle is
`_cmd_unpushed` on a fixture registry with real Git worktrees; the
Rust reader must print the identical JSON payload (schema, slot
rows, states, containing refs, two-readings evidence). Temporary
fixture registries only.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import os
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]


def _git(cwd: Path, *args: str) -> str:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(
        {
            "LC_ALL": "C",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_SYSTEM": "/dev/null",
            "GIT_AUTHOR_DATE": "2026-10-26T00:00:00+00:00",
            "GIT_COMMITTER_DATE": "2026-10-26T00:00:00+00:00",
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
    # The remote is a separate bare repository: if the source repo
    # were its own remote, a worktree commit would move the remote's
    # branch ref directly and nothing could ever be unpushed.
    remote = base / "remote.git"
    _git(base, "init", "-q", "--bare", "-b", "main", str(remote))
    _git(upstream, "remote", "add", "origin", str(remote))
    _git(upstream, "push", "-q", "origin", "main")
    _git(
        upstream, "worktree", "add", "-q", "-b", "suite/work",
        str(root / "worktrees" / "slot-a" / "repo"), "HEAD",
    )
    _git(upstream, "push", "-q", "origin", "suite/work")
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")
    identity = {
        "pid": 123,
        "start_ticks": 1123,
        "boot_id": "boot-1",
        "host_id": "host-1",
        "cgroup_path": "/user.slice/session.scope",
    }
    record = {
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
                "remote_url_sha256": hashlib.sha256(
                    str(remote).encode()
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
    return root


def _python(root: Path, slot: str | None = None) -> dict[str, object]:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot=slot,
        format="json", wait_lock=0.0,
    )
    stream = io.StringIO()
    with contextlib.redirect_stdout(stream):
        assert wrkslots_cli._cmd_unpushed(args) == 0
    payload: dict[str, object] = json.loads(stream.getvalue())
    return payload


def _rust(root: Path, slot: str | None = None) -> subprocess.CompletedProcess[str]:
    extra = ["--slot", slot] if slot else []
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "unpushed", "--root", str(root), *extra],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def test_unpushed_published_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    actual = json.loads(result.stdout)
    assert actual == _python(root)
    rows = actual["slots"]
    assert len(rows) == 1
    assert rows[0]["state"] == "PUBLISHED"
    assert rows[0]["containing_remote_refs"]


def test_unpushed_unpublished_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    checkout = root / "worktrees" / "slot-a" / "repo"
    (checkout / "file.txt").write_text("two\n")
    _git(checkout, "add", "file.txt")
    _git(checkout, "commit", "-q", "-m", "two")
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    actual = json.loads(result.stdout)
    assert actual == _python(root)
    row = actual["slots"][0]
    assert row["state"] == "UNPUSHED-OR-STALE"
    assert row["same_named_remote_ref"] == "refs/remotes/origin/suite/work"
    assert row["touched_files"] == ["file.txt"]
    assert row["diagnostic_command"] is not None


def test_unpushed_slot_filter_matches_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    result = _rust(root, "slot-a")
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == _python(root, "slot-a")
    missing = _rust(root, "slot-b")
    assert missing.returncode == 0, missing.stderr
    assert json.loads(missing.stdout) == _python(root, "slot-b")


def test_unpushed_branch_changed_refuses_like_python(tmp_path: Path) -> None:
    root = _fixture(tmp_path)
    checkout = root / "worktrees" / "slot-a" / "repo"
    _git(checkout, "checkout", "-q", "-b", "suite/other")
    with pytest.raises(Exception) as caught:
        _python(root)
    assert "branch changed" in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert "branch changed" in result.stderr
