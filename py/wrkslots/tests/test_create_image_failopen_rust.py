"""Differential: create `config` -> `image` must never fail open in Rust.

#562 rust-primary-wrkslots, P7 image corner (mu-rsimg S1). The Python
oracle resolves `representation=config` against a configuration whose
`slot_representation` is `image` and would provision a slot image; the
Rust writer, until P8 provisions images, must refuse by name with
zero slot, journal, or ACTIVE change instead of silently creating a
plain worktree (the fail-open found in milestone 1). Temporary
fixture registries and temp directories only; no image is provisioned
and nothing is mounted.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
from pathlib import Path

from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd

AU_ROOT = Path(__file__).resolve().parents[3]
REFUSAL = "slot-image provisioning stays with the Python edition in this slice"


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


def _image_fixture(base: Path) -> Path:
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
        "slot_representation: image\n"
    )
    upstream = base / "upstream-repo"
    _repo(upstream)
    _git(upstream, "remote", "add", "origin", str(upstream))
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")
    return root


def _rust_create(root: Path, *, representation: str = "config") -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    args = [
        str(rust_wrkslotsd()), "create", "slot-b", "--slot-type", "agent",
        "--coordinator-authorized", "--agent", "agent-new", "--task", "task-1",
        "--purpose", "testing", "--owner-pid", str(os.getpid()),
        "--coordinator-pid", str(os.getpid()),
        "--repo", "repo=../upstream-repo", "--branch", "repo=suite/new",
        "--representation", representation,
        "--machine", "node-a", "--root", str(root),
    ]
    return subprocess.run(
        args, cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env
    )


def _assert_no_create_side_effects(root: Path) -> None:
    assert not (root / "worktrees" / "slot-b").exists()
    assert not list((root / "worktrees").glob("CREATE.*.journal"))
    active = root / "worktrees" / "ACTIVE.node-a.json"
    if active.exists():
        doc = json.loads(active.read_text())
        assert all(record["slot"] != "slot-b" for record in doc["slots"])


def test_python_resolves_config_to_image(tmp_path: Path) -> None:
    root = _image_fixture(tmp_path / "oracle")
    config = wrkslots_cli._load_config(str(root), "node-a")
    args = argparse.Namespace(representation="config")
    assert wrkslots_cli._requested_representation(config, args) == "image"


def test_rust_create_config_to_image_refuses_without_side_effects(tmp_path: Path) -> None:
    root = _image_fixture(tmp_path / "rust")
    result = _rust_create(root)
    assert result.returncode == 1, result.stdout
    assert REFUSAL in result.stderr
    _assert_no_create_side_effects(root)


def test_rust_create_explicit_image_refuses_without_side_effects(tmp_path: Path) -> None:
    root = _image_fixture(tmp_path / "rust")
    result = _rust_create(root, representation="image")
    assert result.returncode == 1, result.stdout
    assert REFUSAL in result.stderr
    _assert_no_create_side_effects(root)
