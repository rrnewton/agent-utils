"""Differential: wrkslotsd create-text against the Python create output.

#562 rust-primary-wrkslots: the soak `create` human form. The Python
oracle is `_cmd_create` on a copy of the fixture registry; the Rust
writer runs on the original. Stdout (human lines or the sorted-key
JSON payload) and stderr (retained-storage warnings, setup-hook
lines) must match byte for byte apart from the fixture roots and
commit ids. Temporary fixture registries only.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
import re
import subprocess
from pathlib import Path

from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd


AU_ROOT = Path(__file__).resolve().parents[3]
HEX40 = re.compile(r"\b[0-9a-f]{40}\b")


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


def _repo(path: Path, *, hooks_script: bool = False) -> str:
    path.mkdir(parents=True)
    _git(path, "init", "-q", "-b", "main")
    (path / "file.txt").write_text("one\n")
    _git(path, "add", "file.txt")
    if hooks_script:
        script = path / "scripts" / "setup-hooks.sh"
        script.parent.mkdir(parents=True)
        script.write_text("#!/bin/sh\necho hooks-installed\n")
        script.chmod(0o755)
        _git(path, "add", "scripts/setup-hooks.sh")
    _git(path, "commit", "-q", "-m", "one")
    return _git(path, "rev-parse", "HEAD")


def _fixture(base: Path, *, hooks_script: bool = False) -> Path:
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
    _repo(upstream, hooks_script=hooks_script)
    _git(upstream, "remote", "add", "origin", str(upstream))
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")
    return root


def _norm(text: str, root: Path) -> str:
    text = text.replace(str(root), "ROOT").replace(str(root.parent), "BASE")
    return HEX40.sub("HEAD", text)


def _python_create(root: Path, *, fmt: str, bound: bool = True) -> tuple[int, str, str]:
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", slot="slot-b", slot_type="agent",
        coordinator_authorized=True, agent="agent-new", task="task-1",
        purpose="testing",
        owner_pid=os.getpid() if bound else None,
        coordinator_pid=os.getpid(),
        task_scope_unit=None, task_scope_invocation_id=None,
        repo=["repo=../upstream-repo"], remote=[], remote_url=[],
        branch=["repo=suite/new"], start=[], override_disk_floor=False,
        representation="config", format=fmt, wait_lock=0.0,
    )
    stdout, stderr = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
        rc = wrkslots_cli._cmd_create(args)
    return rc, stdout.getvalue(), stderr.getvalue()


def _rust_create_text(
    root: Path, *, fmt: str, bound: bool = True
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    args = [
        str(rust_wrkslotsd()), "create-text", "slot-b", "--slot-type", "agent",
        "--coordinator-authorized",
        "--agent", "agent-new", "--task", "task-1", "--purpose", "testing",
        "--coordinator-pid", str(os.getpid()),
        "--repo", "repo=../upstream-repo", "--branch", "repo=suite/new",
        "--machine", "node-a", "--root", str(root), "--format", fmt,
    ]
    if bound:
        args += ["--owner-pid", str(os.getpid())]
    return subprocess.run(
        args, cwd=AU_ROOT, capture_output=True, text=True, check=False,
        timeout=900, env=env,
    )


def test_create_text_human_matches_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    rc, out, err = _python_create(oracle_root, fmt="human")
    assert rc == 0
    result = _rust_create_text(rust_root, fmt="human")
    assert result.returncode == 0, result.stderr
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)
    assert _norm(result.stderr, rust_root) == _norm(err, oracle_root)


def test_create_text_json_matches_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    rc, out, err = _python_create(oracle_root, fmt="json")
    assert rc == 0
    result = _rust_create_text(rust_root, fmt="json")
    assert result.returncode == 0, result.stderr
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)
    assert _norm(result.stderr, rust_root) == _norm(err, oracle_root)
    payload = json.loads(result.stdout)
    assert payload["slot"] == "slot-b"
    assert payload["setup_hooks"]["repositories_checked"] == 1


def test_create_text_unbound_owner_matches_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    rc, out, err = _python_create(oracle_root, fmt="human", bound=False)
    assert rc == 0
    result = _rust_create_text(rust_root, fmt="human", bound=False)
    assert result.returncode == 0, result.stderr
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)
    assert "owner_process=unbound" in result.stdout


def test_create_text_shared_hooks_script_matches_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust", hooks_script=True)
    oracle_root = _fixture(tmp_path / "oracle", hooks_script=True)
    rc, out, err = _python_create(oracle_root, fmt="human")
    assert rc == 0
    result = _rust_create_text(rust_root, fmt="human")
    assert result.returncode == 0, result.stderr
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)
    assert _norm(result.stderr, rust_root) == _norm(err, oracle_root)
    assert "not-run-shared-git-config" in result.stdout or "not run" in result.stdout
