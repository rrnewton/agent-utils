"""Nested submodule repair after a fence rename (#788).

A nested repository whose own `.git` directory moved with the slot
and whose `core.worktree` is an absolute path to the pre-rename
location is repointed at its current location, so a following
`git status` works. A pinned path that still exists (a live other
worktree) still refuses. Temporary fixtures only.
"""

from __future__ import annotations

import os
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.test_create_text_rust import _git, _repo


def _nested(tmp_path: Path) -> tuple[Path, Path]:
    leaf = tmp_path / "leaf-src"
    _repo(leaf)
    component = tmp_path / "component-src"
    _repo(component)
    _git(component, "-c", "protocol.file.allow=always", "submodule", "add", "-q", str(leaf), "leaf")
    _git(component, "commit", "-q", "-m", "add leaf")
    top = tmp_path / "slot"
    _repo(top)
    _git(top, "-c", "protocol.file.allow=always", "submodule", "add", "-q", str(component), "component")
    _git(top, "commit", "-q", "-m", "add component")
    _git(top, "-c", "protocol.file.allow=always", "submodule", "update", "--init", "--recursive")
    nested_leaf = top / "component" / "leaf"
    # Replace the absorbed leaf with a standalone clone: its own .git
    # directory (never absorbed) pinned to the absolute location, as
    # older submodule layouts write it.
    import shutil
    import subprocess

    shutil.rmtree(nested_leaf)
    subprocess.run(
        ["git", "clone", "-q", str(leaf), str(nested_leaf)],
        check=True, capture_output=True, text=True, timeout=120,
    )
    subprocess.run(
        [
            "git", "config", "--file", str(nested_leaf / ".git" / "config"),
            "core.worktree", str(nested_leaf),
        ],
        check=True, capture_output=True, text=True, timeout=120,
    )
    return top, nested_leaf


def test_linked_worktree_nested_absolute_core_worktree(tmp_path: Path) -> None:
    """The devbig014 layout (#788): a linked worktree whose nested
    submodule gitdirs live under the main repo, with absolute
    core.worktree, repaired after a fence rename."""

    from wrkslots.tests.test_lifecycle import (
        add_recursive_submodules,
        checkout,
        create,
        git,
        make_project,
    )

    project, _repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, _repository)
    made = create(project)
    assert made.returncode == 0, made.stderr
    target = checkout(project)
    for sub in (target / "component", target / "component" / "leaf"):
        git(sub, "config", "core.worktree", str(sub))
    slot_dir = target.parent
    fenced = slot_dir.with_name(".slot01.fenced.test")
    os.rename(slot_dir, fenced)
    moved = fenced / target.name
    vcs = wrkslots_cli._GitVcs()
    wrkslots_cli._repair_nested_repositories(moved, vcs)
    assert vcs.status(moved) == ""
    for sub in (moved / "component", moved / "component" / "leaf"):
        assert _git(sub, "config", "--get", "core.worktree") == str(sub)


def test_repair_refuses_live_pinned_worktree(tmp_path: Path) -> None:
    top, nested_leaf = _nested(tmp_path)
    vcs = wrkslots_cli._GitVcs()
    with pytest.raises(wrkslots_cli.Refusal, match="refusing to move it"):
        vcs.repair_nested_repository(nested_leaf)
