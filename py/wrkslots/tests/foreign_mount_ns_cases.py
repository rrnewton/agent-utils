"""#350 wrkslots-foreign-mount-ns: image mounts copied into unrelated mount namespaces.

An unrelated sandboxed process that creates a private mount namespace while slot images are
mounted keeps its own copy of every slot mount. These tests cover what wrkslots must do about
that without root: recognise a slot's own loop mount by device and file system identity rather
than a rendered backing path, treat an idle copy of it as storage, never attach an image to a
second loop device, keep Git away from an unmounted image checkout, and never let one checkout's
Git repair write into another checkout's directory. The root end-to-end checks live in
``e2e_foreign_namespace.sh``.

They run in the lifecycle suite's isolated shards: test_lifecycle.py collects the names in
COLLECTED. This module is deliberately not a test file of its own.
"""

from __future__ import annotations

import os
import shutil
import subprocess
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests import test_lifecycle as t


# --------------------------------------------------------------------------- Git repair isolation


def _git(path: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(path), *args], check=True, capture_output=True, text=True
    ).stdout


def _repository_with_two_worktrees(tmp_path: Path) -> tuple[Path, Path, Path]:
    repository = tmp_path / "repo"
    repository.mkdir(parents=True)
    _git(repository, "init", "-q", "--initial-branch=main")
    _git(repository, "config", "user.name", "Wrkslots Test")
    _git(repository, "config", "user.email", "wrkslots@example.invalid")
    (repository / "seed.txt").write_text("seed\n", encoding="utf-8")
    _git(repository, "add", "seed.txt")
    _git(repository, "commit", "-q", "-m", "seed")
    first = tmp_path / "slots" / "first"
    second = tmp_path / "slots" / "second"
    first.parent.mkdir()
    _git(repository, "worktree", "add", "-q", "-b", "first", str(first))
    _git(repository, "worktree", "add", "-q", "-b", "second", str(second))
    return repository, first, second


def _empty_like_an_unmounted_image(directory: Path) -> None:
    """Leave only the empty directory, as an unmounted image's mount point is."""

    for entry in directory.iterdir():
        if entry.is_dir() and not entry.is_symlink():
            shutil.rmtree(entry)
        else:
            entry.unlink()


def test_repairing_one_moved_checkout_never_writes_into_a_sibling(tmp_path: Path) -> None:
    repository, first, second = _repository_with_two_worktrees(tmp_path)
    _empty_like_an_unmounted_image(second)
    moved = first.with_name(".first.fenced.1")
    first.rename(moved)

    cli._GitVcs().repair_worktree(repository, moved)

    assert moved.absolute() in cli._GitVcs().listed_worktrees(repository)
    assert _git(moved, "rev-parse", "--show-toplevel").strip() == str(moved)
    assert list(second.iterdir()) == []


def test_git_worktree_repair_itself_writes_into_an_empty_sibling(tmp_path: Path) -> None:
    """The hazard the targeted repair avoids, pinned so a Git change is noticed."""

    repository, first, second = _repository_with_two_worktrees(tmp_path)
    _empty_like_an_unmounted_image(second)
    moved = first.with_name(".first.fenced.1")
    first.rename(moved)

    _git(repository, "worktree", "repair", str(moved))

    assert [entry.name for entry in second.iterdir()] == [".git"]


def test_targeted_repair_refuses_a_git_file_naming_another_repository(tmp_path: Path) -> None:
    repository, first, _second = _repository_with_two_worktrees(tmp_path)
    other, other_first, _other_second = _repository_with_two_worktrees(tmp_path / "other")
    (first / ".git").write_text(
        f"gitdir: {other / '.git' / 'worktrees' / other_first.name}\n", encoding="utf-8"
    )
    with pytest.raises(cli.Refusal, match="not a linked-worktree directory of"):
        cli._GitVcs().repair_worktree(repository, first)
    (first / ".git").write_text("not a pointer\n", encoding="utf-8")
    with pytest.raises(cli.Refusal, match="invalid .git file"):
        cli._GitVcs().repair_worktree(repository, first)
    (first / ".git").unlink()
    with pytest.raises(cli.Refusal, match="no safe .git file"):
        cli._GitVcs().repair_worktree(repository, first)


def _two_slots_with_an_emptied_sibling(tmp_path: Path) -> tuple[Path, Path]:
    project, repository, _remote = t.make_project(tmp_path)
    assert t.create(project).returncode == 0
    second = t.create(project, slot="slot02", agent="codex-2", branch="codex/second")
    assert second.returncode == 0, second.stderr
    t.commit_task(repository, t.checkout(project), "codex/task")
    sibling = t.checkout(project, "slot02")
    _empty_like_an_unmounted_image(sibling)
    t.mark_owner_dead(project, slot="slot01")
    t.set_liveness(project, "dead")
    return project, sibling


def test_removing_one_slot_never_writes_into_an_emptied_sibling_checkout(
    tmp_path: Path,
) -> None:
    project, sibling = _two_slots_with_an_emptied_sibling(tmp_path)

    removed = t.remove(project)

    assert removed.returncode == 0, removed.stderr
    assert not t.checkout(project).exists()
    assert list(sibling.iterdir()) == []


def test_a_rolled_back_fence_never_writes_into_an_emptied_sibling_checkout(
    tmp_path: Path,
) -> None:
    project, sibling = _two_slots_with_an_emptied_sibling(tmp_path)

    refused = t._remove_with_patch(
        project,
        """
        original = cli._assert_salvage_still_matches
        def refuse(*args, **kwargs):
            # Only the recheck after the path fence names the moved paths; a
            # refusal there rolls the fence back.
            if "paths" not in kwargs:
                return original(*args, **kwargs)
            raise cli.Refusal("injected refusal after the path fence")
        cli._assert_salvage_still_matches = refuse
        """,
    )

    assert refused.returncode == 3, refused.stdout + refused.stderr
    assert "injected refusal after the path fence" in refused.stderr
    assert t.checkout(project).is_dir()
    assert len(t.active_slots(project)) == 2
    assert list(sibling.iterdir()) == []


#: What test_lifecycle.py collects from this module.
COLLECTED: tuple[str, ...] = tuple(name for name in sorted(globals()) if name.startswith("test_"))
