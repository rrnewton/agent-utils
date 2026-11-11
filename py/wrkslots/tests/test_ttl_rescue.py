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

    (entry,) = receipt["rescued"]  # type: ignore[index]
    assert entry["rescue_ref"] == "refs/heads/rescue/slot-a/20261114/feat/x"
    assert entry["pushed"] is True
    assert entry["wip_commit"] == _git(repo, "rev-parse", "HEAD")
    assert _remote_ref(remote, entry["rescue_ref"]) == entry["wip_commit"]
    assert receipt["bundle"] is None
    assert receipt["operations"] == 4  # status + add + commit + push


def test_detached_head_and_failed_push_fall_back_to_verified_bundle(tmp_path: Path) -> None:
    repo, _remote = _repo(tmp_path)
    head = _git(repo, "rev-parse", "HEAD")
    _git(repo, "checkout", "-q", head)

    receipt = cli._execute_ttl_rescue(
        repo, slot="slot-b", date_str="20261114", remote="missing-remote",
        bundle_path=tmp_path / "bundles" / "slot-b.bundle",
    )

    (entry,) = receipt["rescued"]  # type: ignore[index]
    assert entry["branch"] == f"detached-{_git(repo, 'rev-parse', '--short', 'HEAD')}"
    assert entry["pushed"] is False
    bundle = Path(str(receipt["bundle"]))
    assert bundle.is_file()
    listed = subprocess.run(
        ["git", "-C", str(repo), "bundle", "list-heads", str(bundle)],
        check=True, capture_output=True, text=True,
    ).stdout
    assert head in listed
