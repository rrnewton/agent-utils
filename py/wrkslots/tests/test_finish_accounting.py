"""Finish commit accounting (#1364): patch-id classification, no network.

Fixture registries only, mirroring test_finish_write_rust's harness but
driving the Python ``_cmd_finish`` directly. The fixture remote is a
local path, so every fetch is a local transport.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import subprocess
from collections.abc import Mapping, Sequence
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli


def _git(cwd: Path, *args: str, env_extra: dict[str, str] | None = None) -> str:
    env = {k: v for k, v in os.environ.items() if not k.startswith("GIT_")}
    env.update(
        {
            "LC_ALL": "C",
            "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": "/dev/null",
            "GIT_CONFIG_SYSTEM": "/dev/null",
        }
    )
    if env_extra:
        env.update(env_extra)
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


def _live_identity() -> dict[str, object]:
    identity = wrkslots_cli._read_process_identity(os.getpid())
    return {
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    }


def _fixture(base: Path) -> tuple[Path, Path, Path]:
    """(project root, upstream repo, slot checkout)."""
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
    # A bare origin separate from the source repository: the slot branch
    # exists in the source but reaches the origin only when pushed.
    bare = base / "origin.git"
    _git(base, "clone", "-q", "--bare", str(upstream), str(bare))
    _git(upstream, "remote", "add", "origin", str(bare))
    _git(upstream, "fetch", "-q", "origin", "+refs/heads/*:refs/remotes/origin/*")
    checkout = root / "worktrees" / "slot-a" / "repo"
    _git(
        upstream, "worktree", "add", "-q", "-b", "suite/work",
        str(checkout), "HEAD",
    )
    identity = _live_identity()
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
                    str(bare).encode()
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
    return root, upstream, checkout


def _finish(root: Path, *, discard: bool = False) -> int:
    args = argparse.Namespace(
        project_root=str(root), machine=None, slot="slot-a", agent="agent-slot-a",
        owner_pid=os.getpid(), expected_generation=1,
        validation=["cargo test: pass"], limitation=[], wait_lock=0.0,
        discard_unaccounted=discard,
    )
    return wrkslots_cli._cmd_finish(args)


def _commit(checkout: Path, name: str, content: str, message: str) -> str:
    (checkout / name).write_text(content)
    _git(checkout, "add", name)
    _git(checkout, "commit", "-q", "-m", message)
    return _git(checkout, "rev-parse", "HEAD")


def _handoff(root: Path) -> dict[str, object]:
    doc = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
    handoff: dict[str, object] = doc["slots"][0]["handoff"]
    return handoff


def test_landed_by_exact_sha(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    root, upstream, checkout = _fixture(tmp_path)
    commit = _commit(checkout, "a.txt", "a\n", "landed exact")
    _git(upstream, "merge", "-q", "--ff-only", commit)
    _git(upstream, "push", "-q", "origin", "main")
    assert _finish(root) == 0
    out = capsys.readouterr().out
    assert "accounting landed=1 open=0 unaccounted=0" in out
    assert _handoff(root)["validation"] == ["cargo test: pass"]


def test_landed_by_replay_same_patch(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    root, upstream, checkout = _fixture(tmp_path)
    commit = _commit(checkout, "a.txt", "a\n", "landed replay")
    _git(upstream, "cherry-pick", commit)
    _git(upstream, "commit", "-q", "--amend", "-m", "landed replay (replayed)")
    replayed = _git(upstream, "rev-parse", "HEAD")
    assert replayed != commit
    _git(upstream, "push", "-q", "origin", "main")
    assert _finish(root) == 0
    out = capsys.readouterr().out
    assert "accounting landed=1 open=0 unaccounted=0" in out


def test_open_on_published_ref(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    root, _upstream, checkout = _fixture(tmp_path)
    _commit(checkout, "a.txt", "a\n", "open change")
    _git(checkout, "push", "-q", "origin", "HEAD:refs/heads/pr/suite-work")
    assert _finish(root) == 0
    out = capsys.readouterr().out
    assert "accounting landed=0 open=1 unaccounted=0" in out


def test_unaccounted_refuses(tmp_path: Path) -> None:
    root, _upstream, checkout = _fixture(tmp_path)
    commit = _commit(checkout, "a.txt", "a\n", "secret wip")
    with pytest.raises(wrkslots_cli.Refusal) as caught:
        _finish(root)
    message = str(caught.value)
    assert "accounting landed=0 open=0 unaccounted=1" in message
    assert commit in message
    assert "secret wip" in message
    assert _handoff(root) is None


def test_discard_unaccounted_finishes_and_records(tmp_path: Path) -> None:
    root, _upstream, checkout = _fixture(tmp_path)
    commit = _commit(checkout, "a.txt", "a\n", "throwaway")
    assert _finish(root, discard=True) == 0
    assert _handoff(root)["discarded_unaccounted"] == [commit]
    bundles = list((root / "worktrees" / "wrkslots-salvage").glob("*.bundle"))
    assert len(bundles) == 1
    _git(checkout, "bundle", "verify", str(bundles[0]))


def test_open_by_replay_on_published_ref(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root, upstream, checkout = _fixture(tmp_path)
    commit = _commit(checkout, "a.txt", "a\n", "open replay")
    _git(upstream, "cherry-pick", commit)
    _git(upstream, "commit", "-q", "--amend", "-m", "open replay (replayed)")
    replayed = _git(upstream, "rev-parse", "HEAD")
    assert replayed != commit
    _git(upstream, "push", "-q", "origin", "HEAD:refs/heads/pr/open-replay")
    _git(upstream, "reset", "-q", "--hard", "HEAD~1")
    assert _finish(root) == 0
    out = capsys.readouterr().out
    assert "accounting landed=0 open=1 unaccounted=0" in out


def _classify_repo(base: Path) -> tuple[Path, str, str, str]:
    """(repo, base, candidate, head) with candidate on branch work."""
    repo = base / "classify-repo"
    base_sha = _repo(repo)
    _git(repo, "checkout", "-q", "-b", "work")
    candidate = _commit(repo, "a.txt", "a\n", "candidate change")
    return repo, base_sha, candidate, candidate


def _checkout_for(base_sha: str, head: str) -> wrkslots_cli.Checkout:
    return wrkslots_cli.Checkout(
        name="repo",
        path="repo",
        repository="repo",
        branch="work",
        start_point=base_sha,
        remote="origin",
        remote_url_sha256="0" * 64,
        landed_ref="refs/remotes/origin/main",
        head=head,
    )


def _classify(
    repo: Path, base_sha: str, head: str
) -> dict[str, object]:
    return wrkslots_cli._classify_checkout_commits(
        _checkout_for(base_sha, head), repo, head, wrkslots_cli._GitVcs(), base=base_sha
    )


def _replay_on(base: Path, repo: Path, base_sha: str, candidate: str, ref: str) -> str:
    _git(repo, "checkout", "-q", "-b", "replay-tmp", base_sha)
    _git(repo, "cherry-pick", candidate)
    _git(repo, "commit", "-q", "--amend", "-m", "candidate change (replayed)")
    replayed = _git(repo, "rev-parse", "HEAD")
    assert replayed != candidate
    _git(repo, "update-ref", ref, replayed)
    _git(repo, "checkout", "-q", "work")
    _git(repo, "branch", "-q", "-D", "replay-tmp")
    return replayed


def test_classify_published_replay_is_open(tmp_path: Path) -> None:
    repo, base_sha, candidate, head = _classify_repo(tmp_path)
    _git(repo, "update-ref", "refs/remotes/origin/main", base_sha)
    _replay_on(tmp_path, repo, base_sha, candidate, "refs/remotes/origin/pr")
    entry = _classify(repo, base_sha, head)
    assert entry["open"] == [(candidate, "candidate change")]
    assert entry["landed"] == []
    assert entry["unaccounted"] == []


def test_classify_landed_replay_is_landed(tmp_path: Path) -> None:
    repo, base_sha, candidate, head = _classify_repo(tmp_path)
    _replay_on(tmp_path, repo, base_sha, candidate, "refs/remotes/origin/main")
    entry = _classify(repo, base_sha, head)
    assert entry["landed"] == [(candidate, "candidate change")]
    assert entry["open"] == []
    assert entry["unaccounted"] == []


def test_classify_no_counterpart_is_unaccounted(tmp_path: Path) -> None:
    repo, base_sha, candidate, head = _classify_repo(tmp_path)
    _git(repo, "update-ref", "refs/remotes/origin/main", base_sha)
    entry = _classify(repo, base_sha, head)
    assert entry["unaccounted"] == [(candidate, "candidate change")]
    assert entry["landed"] == []
    assert entry["open"] == []


def test_classify_git_invocations_constant_in_remote_refs(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    repo, base_sha, candidate, head = _classify_repo(tmp_path)
    _git(repo, "update-ref", "refs/remotes/origin/main", base_sha)
    _replay_on(tmp_path, repo, base_sha, candidate, "refs/remotes/origin/pr")
    calls: list[tuple[str, ...]] = []
    original = wrkslots_cli._GitVcs._run

    def counting(
        repository: Path,
        args: Sequence[str],
        *,
        check: bool = True,
        input_text: str | None = None,
        env_overrides: Mapping[str, str] | None = None,
        timeout_seconds: float | None = None,
        timeout_refusal: str | None = None,
        config: Sequence[tuple[str, str]] = (),
    ) -> subprocess.CompletedProcess[str]:
        calls.append(tuple(args))
        return original(
            repository,
            args,
            check=check,
            input_text=input_text,
            env_overrides=env_overrides,
            timeout_seconds=timeout_seconds,
            timeout_refusal=timeout_refusal,
            config=config,
        )

    monkeypatch.setattr(wrkslots_cli._GitVcs, "_run", staticmethod(counting))
    first = _classify(repo, base_sha, head)
    first_count = len(calls)
    assert first["open"] == [(candidate, "candidate change")]
    # Thousands of extra published refs naming the already-accounted base
    # must not add a single Git invocation (#1488).
    for index in range(2000):
        _git(repo, "update-ref", f"refs/remotes/origin/rescue/r{index:04d}", base_sha)
    calls.clear()
    second = _classify(repo, base_sha, head)
    assert second == first
    assert len(calls) == first_count
    assert first_count < 60


def test_nested_worktree_unaccounted_refuses(tmp_path: Path) -> None:
    root, _upstream, checkout = _fixture(tmp_path)
    nested = checkout / "nested-wt"
    _git(checkout, "worktree", "add", "-q", "-b", "nested/work", str(nested), "HEAD")
    commit = _commit(nested, "n.txt", "n\n", "nested wip")
    with pytest.raises(wrkslots_cli.Refusal) as caught:
        _finish(root)
    assert commit in str(caught.value)
    assert _handoff(root) is None
