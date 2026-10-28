"""Differential: wrkslotsd recover-ownerless-agent-clone plans.

#562 rust-primary-wrkslots slice 42, the seventeenth writer slice.
The Python oracle is `_cmd_recover_ownerless_agent_clone` on a copy
of the fixture registry; the Rust writer runs on the original. The
Rust slice covers the plan outcomes that do not depend on the full
clone inventory (machine check, journal resume, registry/hold
collisions, target binding, root clone identity facts); the
inventory-bound plan and the destructive apply path stay with the
Python edition and refuse explicitly. Temporary fixture registries
only: writers are never pointed at a live registry before the
edition cutover.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import socket
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]
MACHINE = socket.gethostname().split(".", 1)[0]


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


def _fixture(base: Path) -> tuple[Path, Path]:
    root = base / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        f"machine: {MACHINE}\n"
        "worktrees_dir: worktrees\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
    )
    bare = base / "remote.git"
    bare.mkdir(parents=True)
    _git(bare, "init", "-q", "--bare", "-b", "main")
    clone = root / "worktrees" / "old-clone"
    clone.mkdir(parents=True)
    _git(clone, "init", "-q", "-b", "main")
    (clone / "file.txt").write_text("one\n")
    _git(clone, "add", "file.txt")
    _git(clone, "commit", "-q", "-m", "one")
    _git(clone, "remote", "add", "origin", str(bare))
    _git(clone, "push", "-q", "origin", "main")
    (root / "worktrees" / f"ACTIVE.{MACHINE}.json").write_text(
        json.dumps({"schema": 2, "machine": MACHINE, "revision": 7, "slots": []})
    )
    (root / "worktrees" / f"ARCHIVED.{MACHINE}.json").write_text(
        json.dumps({"schema": 2, "machine": MACHINE, "revision": 3, "records": []})
    )
    return root, clone


def _rust(root: Path, *, fixture: bool = True, extra: list[str] | None = None) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    args = [
        "cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
        "-p", "wrkslotsd", "--", "recover-ownerless-agent-clone",
        "worktrees/old-clone",
        "--remote-url", f".git={root.parent / 'remote.git'}",
        "--root", str(root),
    ]
    if extra:
        args = args[:-2] + extra + args[-2:]
    return subprocess.run(
        args, cwd=AU_ROOT, capture_output=True, text=True, check=False,
        timeout=900, env=env,
    )


def _python(root: Path) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, path="worktrees/old-clone",
        remote_urls=[f".git={root.parent / 'remote.git'}"],
        handoff_sha256s=[], discard_ignored=False, inventory_sha256=None,
        apply=False, coordinator_authorized=False, coordinator_pid=None,
        wait_lock=0.0,
    )
    return wrkslots_cli._cmd_recover_ownerless_agent_clone(args)


def _journal(root: Path, clone: Path, *, minimal: bool = False) -> None:
    if minimal:
        journal: dict[str, object] = {
            "kind": "ownerless-clone-remove",
            "authorization": {"path": "worktrees/old-clone"},
        }
    else:
        identity = list(wrkslots_cli._open_directory_identity(clone, "ownerless clone"))
        actor = wrkslots_cli._identity_to_obj(
            wrkslots_cli._read_process_identity(os.getpid())
        )
        journal = {
            "schema": 2,
            "kind": "ownerless-clone-remove",
            "machine": MACHINE,
            "slot": "old-clone",
            "phase": "prepared",
            "fenced": "worktrees/.old-clone.ownerless-clone." + "0" * 32,
            "authorization": {
                "path": "worktrees/old-clone",
                "identity": identity,
                "actor": actor,
                "remote_urls": {".git": str(root.parent / "remote.git")},
                "inventory_sha256": hashlib.sha256(b"inventory").hexdigest(),
                "handoff_sha256s": [],
                "discard_ignored": False,
                "recorded_at": wrkslots_cli._utc_now(),
            },
            "rescue": [],
            "record": None,
        }
    (root / "worktrees" / f"ACTIVE.{MACHINE}.journal").write_text(json.dumps(journal))


def test_ownerless_clone_journal_resume_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    rust_root, rust_clone = _fixture(tmp_path / "rust")
    oracle_root, oracle_clone = _fixture(tmp_path / "oracle")
    _journal(rust_root, rust_clone)
    _journal(oracle_root, oracle_clone)
    assert _python(oracle_root) == 0
    expected = capsys.readouterr().out.strip()
    result = _rust(rust_root)
    assert result.returncode == 0, result.stderr
    outcome = json.loads(result.stdout)
    assert outcome["outcome"] == "planned"
    assert outcome["detail"] == "resume"
    assert outcome["phase"] == "prepared"
    assert expected.endswith("outcome=planned detail=resume phase=prepared")


def test_ownerless_clone_minimal_journal_refuses(tmp_path: Path) -> None:
    rust_root, rust_clone = _fixture(tmp_path / "rust")
    oracle_root, oracle_clone = _fixture(tmp_path / "oracle")
    _journal(rust_root, rust_clone, minimal=True)
    _journal(oracle_root, oracle_clone, minimal=True)
    with pytest.raises(Exception):
        _python(oracle_root)
    result = _rust(rust_root)
    assert result.returncode == 1
    assert "journal is invalid" in result.stderr or "journal" in result.stderr


def test_ownerless_clone_dirty_refuses(tmp_path: Path) -> None:
    rust_root, rust_clone = _fixture(tmp_path / "rust")
    oracle_root, oracle_clone = _fixture(tmp_path / "oracle")
    for clone in (rust_clone, oracle_clone):
        (clone / "file.txt").write_text("changed\n")
    with pytest.raises(Exception) as caught:
        _python(oracle_root)
    assert "cannot be retired yet" in str(caught.value) or "not clean" in str(caught.value)
    result = _rust(rust_root)
    assert result.returncode == 1
    assert "not clean" in result.stderr


def test_ownerless_clone_linked_worktree_refuses(tmp_path: Path) -> None:
    rust_root, rust_clone = _fixture(tmp_path / "rust")
    dotgit = rust_clone / ".git"
    for entry in sorted(dotgit.rglob("*"), key=lambda p: len(p.parts), reverse=True):
        if entry.is_dir() and not entry.is_symlink():
            entry.rmdir()
        else:
            entry.unlink()
    dotgit.rmdir()
    dotgit.write_text("gitdir: /elsewhere\n")
    result = _rust(rust_root)
    assert result.returncode == 1
    assert "linked worktree" in result.stderr or "standalone" in result.stderr


def test_ownerless_clone_clean_plan_stays_python_owned(tmp_path: Path) -> None:
    rust_root, _clone = _fixture(tmp_path / "rust")
    result = _rust(rust_root)
    assert result.returncode == 1
    assert "stay with the Python edition" in result.stderr


def test_ownerless_clone_refuses_without_fixture_attestation(tmp_path: Path) -> None:
    rust_root, _clone = _fixture(tmp_path / "rust")
    result = _rust(rust_root, fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
