"""Differential: wrkslotsd recover-ownerless-agent-worktree plans.

#562 rust-primary-wrkslots slice 41, the sixteenth writer slice.
The Python oracle is `_cmd_recover_ownerless_agent_worktree` on a
copy of the fixture registry; the Rust writer runs on the original.
The Rust slice covers the read-only plan outcomes and refusals;
the destructive apply path stays with the Python edition and
refuses explicitly. Temporary fixture registries only: writers are
never pointed at a live registry before the edition cutover.
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


def _repo(path: Path) -> str:
    path.mkdir(parents=True)
    _git(path, "init", "-q", "-b", "main")
    (path / "file.txt").write_text("one\n")
    _git(path, "add", "file.txt")
    _git(path, "commit", "-q", "-m", "one")
    return _git(path, "rev-parse", "HEAD")


def _fixture(base: Path) -> tuple[Path, Path, str, str]:
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
    upstream = base / "upstream-repo"
    head = _repo(upstream)
    _git(upstream, "remote", "add", "origin", str(upstream))
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")
    orphan = root / "worktrees" / "orphan"
    _git(upstream, "worktree", "add", "-q", "-b", "suite/orphan", str(orphan), "HEAD")
    (root / "worktrees" / f"ACTIVE.{MACHINE}.json").write_text(
        json.dumps({"schema": 2, "machine": MACHINE, "revision": 7, "slots": []})
    )
    (root / "worktrees" / f"ARCHIVED.{MACHINE}.json").write_text(
        json.dumps({"schema": 2, "machine": MACHINE, "revision": 3, "records": []})
    )
    digest = hashlib.sha256(str(upstream).encode()).hexdigest()
    return root, orphan, head, digest


def _rust_args(root: Path, orphan: Path, head: str, digest: str) -> list[str]:
    return [
        "cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
        "-p", "wrkslotsd", "--", "recover-ownerless-agent-worktree",
        "worktrees/orphan", "--repository", "../upstream-repo", "--head", head,
        "--branch", "suite/orphan", "--remote", "origin",
        "--remote-url-sha256", digest, "--root", str(root),
    ]


def _rust(
    root: Path, orphan: Path, head: str, digest: str, *, fixture: bool = True,
    extra: list[str] | None = None,
) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if fixture:
        env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    else:
        env.pop("WRKSLOTSD_FIXTURE_WRITER", None)
    args = _rust_args(root, orphan, head, digest)
    if extra:
        args = args[:-2] + extra + args[-2:]
    return subprocess.run(
        args, cwd=AU_ROOT, capture_output=True, text=True, check=False,
        timeout=900, env=env,
    )


def _python(
    root: Path, orphan: Path, head: str, digest: str, *, detached: bool = False,
    handoff_sha256: str | None = None,
) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, path="worktrees/orphan",
        repository="../upstream-repo", head=head, branch=None if detached else "suite/orphan",
        detached=detached, remote="origin", remote_url_sha256=digest,
        handoff_sha256=handoff_sha256, apply=False, coordinator_authorized=False,
        coordinator_pid=None, wait_lock=0.0,
    )
    return wrkslots_cli._cmd_recover_ownerless_agent_worktree(args)


def test_ownerless_worktree_plan_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    rust_root, rust_orphan, head, digest = _fixture(tmp_path / "rust")
    oracle_root, oracle_orphan, oracle_head, oracle_digest = _fixture(tmp_path / "oracle")
    assert _python(oracle_root, oracle_orphan, oracle_head, oracle_digest) == 0
    expected = capsys.readouterr().out.strip()
    result = _rust(rust_root, rust_orphan, head, digest)
    assert result.returncode == 0, result.stderr
    outcome = json.loads(result.stdout)
    assert outcome["outcome"] == "planned"
    assert outcome["detail"] == "ownerless-agent"
    assert expected.endswith("outcome=planned detail=ownerless-agent")


def test_ownerless_worktree_handoff_digest_required(tmp_path: Path) -> None:
    rust_root, rust_orphan, head, digest = _fixture(tmp_path / "rust")
    oracle_root, oracle_orphan, oracle_head, oracle_digest = _fixture(tmp_path / "oracle")
    for orphan in (rust_orphan, oracle_orphan):
        (orphan / "HANDOFF.md").write_text("# Notes\n\nread me\n")
    observed = hashlib.sha256(b"# Notes\n\nread me\n").hexdigest()
    with pytest.raises(Exception) as caught:
        _python(oracle_root, oracle_orphan, oracle_head, oracle_digest)
    assert "rerun with --handoff-sha256" in str(caught.value)
    result = _rust(rust_root, rust_orphan, head, digest)
    assert result.returncode == 1
    assert f"rerun with --handoff-sha256 {observed}" in result.stderr
    ok = _rust(
        rust_root, rust_orphan, head, digest,
        extra=["--handoff-sha256", observed],
    )
    assert ok.returncode == 0, ok.stderr


def test_ownerless_worktree_refuses_without_fixture_attestation(
    tmp_path: Path,
) -> None:
    rust_root, rust_orphan, head, digest = _fixture(tmp_path / "rust")
    result = _rust(rust_root, rust_orphan, head, digest, fixture=False)
    assert result.returncode == 1
    assert "fixture-only" in result.stderr


def test_ownerless_worktree_grafts_refuse_plan(tmp_path: Path) -> None:
    rust_root, rust_orphan, head, digest = _fixture(tmp_path / "rust")
    oracle_root, oracle_orphan, oracle_head, oracle_digest = _fixture(tmp_path / "oracle")
    for base, orphan in ((tmp_path / "rust", rust_orphan), (tmp_path / "oracle", oracle_orphan)):
        common = Path(_git(orphan, "rev-parse", "--git-common-dir"))
        if not common.is_absolute():
            common = orphan / common
        grafts = common / "info" / "grafts"
        grafts.parent.mkdir(parents=True, exist_ok=True)
        grafts.write_text(f"{oracle_head if base.name == 'oracle' else head}\n")
    with pytest.raises(Exception) as caught:
        _python(oracle_root, oracle_orphan, oracle_head, oracle_digest)
    assert "graft" in str(caught.value) or "ordinary" in str(caught.value)
    result = _rust(rust_root, rust_orphan, head, digest)
    assert result.returncode == 1
    assert "graft" in result.stderr


def test_ownerless_worktree_pushurl_refuses_plan(tmp_path: Path) -> None:
    rust_root, rust_orphan, head, digest = _fixture(tmp_path / "rust")
    _git(rust_orphan, "remote", "set-url", "--push", "origin", "https://example.invalid/x")
    result = _rust(rust_root, rust_orphan, head, digest)
    assert result.returncode == 1
    assert "push URL" in result.stderr


def test_ownerless_worktree_absolute_path_refuses(tmp_path: Path) -> None:
    rust_root, rust_orphan, head, digest = _fixture(tmp_path / "rust")
    env = dict(os.environ)
    env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    result = subprocess.run(
        [
            "cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
            "-p", "wrkslotsd", "--", "recover-ownerless-agent-worktree",
            str(rust_orphan), "--repository", "../upstream-repo", "--head", head,
            "--branch", "suite/orphan", "--remote", "origin",
            "--remote-url-sha256", digest, "--root", str(rust_root),
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )
    assert result.returncode == 1
    assert "must be a relative path" in result.stderr


def test_ownerless_worktree_minimal_journal_refuses(tmp_path: Path) -> None:
    rust_root, rust_orphan, head, digest = _fixture(tmp_path / "rust")
    journal = {
        "kind": "ownerless-agent-remove",
        "authorization": {"path": "worktrees/orphan"},
    }
    (rust_root / "worktrees" / f"ACTIVE.{MACHINE}.journal").write_text(
        json.dumps(journal)
    )
    result = _rust(rust_root, rust_orphan, head, digest)
    assert result.returncode == 1
    assert "journal is invalid" in result.stderr
