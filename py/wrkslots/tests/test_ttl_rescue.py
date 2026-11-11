"""Deterministic TTL rescue against fixture repositories (#1293 slice 3).

Fixtures only: a scratch checkout and a local bare remote. Assertions name
refs, commits and bundles (and count rescue operations), never time.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

from wrkslots import cli


def _git(path: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(path), *args],
        check=True, capture_output=True, text=True,
    ).stdout.strip()


def _repo(tmp_path: Path) -> tuple[Path, Path]:
    remote = tmp_path / "remote.git"
    subprocess.run(["git", "init", "-q", "--bare", str(remote)], check=True)
    repo = tmp_path / "repo"
    repo.mkdir()
    _git(repo, "init", "-q")
    _git(repo, "-c", "user.name=fixture", "-c", "user.email=f@x.invalid",
         "commit", "-q", "--allow-empty", "-m", "base")
    _git(repo, "remote", "add", "origin", str(remote))
    return repo, remote


def _remote_ref(remote: Path, ref: str) -> str:
    return subprocess.run(
        ["git", f"--git-dir={remote}", "rev-parse", ref],
        check=True, capture_output=True, text=True,
    ).stdout.strip()


def test_dirty_branch_is_wip_committed_and_pushed_to_dated_rescue_ref(tmp_path: Path) -> None:
    repo, remote = _repo(tmp_path)
    _git(repo, "checkout", "-q", "-b", "feat/x")
    (repo / "work.txt").write_text("stranded\n", encoding="utf-8")

    receipt = cli._execute_ttl_rescue(
        repo, slot="slot-a", date_str="20261114", remote="origin",
        bundle_path=tmp_path / "bundles" / "slot-a.bundle",
    )

    entries = {entry["branch"]: entry for entry in receipt["rescued"]}
    entry = entries["feat/x"]
    assert entry["rescue_ref"] == "refs/heads/rescue/slot-a/20261114/feat/x"
    assert entry["pushed"] is True
    assert entry["wip_commit"] == _git(repo, "rev-parse", "HEAD")
    assert _remote_ref(remote, entry["rescue_ref"]) == entry["wip_commit"]
    assert receipt["bundle"] is None
    # status + add + commit + worktree push + the unchecked-out base branch.
    assert receipt["operations"] == 5


def test_detached_head_and_failed_push_fall_back_to_verified_bundle(tmp_path: Path) -> None:
    repo, _remote = _repo(tmp_path)
    head = _git(repo, "rev-parse", "HEAD")
    _git(repo, "checkout", "-q", head)

    receipt = cli._execute_ttl_rescue(
        repo, slot="slot-b", date_str="20261114", remote="missing-remote",
        bundle_path=tmp_path / "bundles" / "slot-b.bundle",
    )

    entries = {entry["branch"]: entry for entry in receipt["rescued"]}
    entry = next(item for item in receipt["rescued"] if item["branch"].startswith("detached-"))
    assert entry["branch"] == f"detached-{_git(repo, 'rev-parse', '--short', 'HEAD')}"
    assert entries  # every local branch was attempted too
    assert entry["pushed"] is False
    bundle = Path(str(receipt["bundle"]))
    assert bundle.is_file()
    listed = subprocess.run(
        ["git", "-C", str(repo), "bundle", "list-heads", str(bundle)],
        check=True, capture_output=True, text=True,
    ).stdout
    assert head in listed


def test_hook_renames_refs_and_words_the_wip_commit(tmp_path: Path) -> None:
    repo, remote = _repo(tmp_path)
    _git(repo, "checkout", "-q", "-b", "feat/x")
    (repo / "work.txt").write_text("stranded\n", encoding="utf-8")

    def hook(request: cli._TtlRescueHookRequest) -> cli._TtlRescueHookDecision:
        assert "feat/x" in request["branches"]
        return {"ref_names": {"feat/x": "kept-work"}, "wip_message": "hook WIP"}

    receipt = cli._execute_ttl_rescue(
        repo, slot="slot-h", date_str="20261114", remote="origin",
        bundle_path=tmp_path / "bundles" / "slot-h.bundle", hook=hook,
    )

    entries = {entry["branch"]: entry for entry in receipt["rescued"]}
    assert entries["feat/x"]["rescue_ref"] == "refs/heads/rescue/slot-h/20261114/kept-work"
    subject = subprocess.run(
        ["git", f"--git-dir={remote}", "log", "-1", "--format=%s",
         "refs/heads/rescue/slot-h/20261114/kept-work"],
        check=True, capture_output=True, text=True,
    ).stdout.strip()
    assert subject == "hook WIP"


def test_a_failing_hook_falls_back_to_deterministic_names(tmp_path: Path) -> None:
    repo, remote = _repo(tmp_path)
    _git(repo, "checkout", "-q", "-b", "feat/x")

    def hook(request: cli._TtlRescueHookRequest) -> cli._TtlRescueHookDecision:
        raise RuntimeError("hook unavailable")

    receipt = cli._execute_ttl_rescue(
        repo, slot="slot-i", date_str="20261114", remote="origin",
        bundle_path=tmp_path / "bundles" / "slot-i.bundle", hook=hook,
    )

    entries = {entry["branch"]: entry for entry in receipt["rescued"]}
    assert entries["feat/x"]["pushed"] is True
    assert _remote_ref(remote, "refs/heads/rescue/slot-i/20261114/feat/x")
