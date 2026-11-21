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


def test_salvage_index_trees_survive_fence_with_nested_worktree(tmp_path: Path) -> None:
    """Salvage index trees after a fence rename with a nested worktree (#2026).

    The nested worktree holds a submodule whose admin ``core.worktree``
    is a relative path naming the slot's unfenced location; Git cannot
    chdir into it after the fence. Salvage must still read the nested
    worktree's staged and worktree trees, per-invocation, without
    rewriting any Git file.
    """

    import dataclasses
    import subprocess

    from wrkslots.tests.test_lifecycle import checkout, create, make_project

    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    target = checkout(project)

    def run(cwd: Path, *args: str) -> subprocess.CompletedProcess[str]:
        return subprocess.run(
            list(args), cwd=cwd, check=False, capture_output=True, text=True, timeout=120
        )

    nested = target / "ignored" / "nested" / "agent-utils"
    added = run(
        target, "git", "worktree", "add", "--relative-paths", "-b", "nested-branch",
        str(nested),
    )
    assert added.returncode == 0, added.stderr
    leaf_src = tmp_path / "leaf-src"
    _repo(leaf_src)
    sub = run(
        nested, "git", "-c", "protocol.file.allow=always",
        "submodule", "add", "-q", str(leaf_src), "leafmod",
    )
    assert sub.returncode == 0, sub.stderr
    init = run(
        nested, "git", "-c", "protocol.file.allow=always",
        "submodule", "update", "--init",
    )
    assert init.returncode == 0, init.stderr
    for args in (
        ["config", "user.email", "t@example.invalid"],
        ["config", "user.name", "T"],
        ["add", "-A"],
        ["commit", "-q", "-m", "nested setup"],
    ):
        committed = run(nested, "git", *args)
        assert committed.returncode == 0, committed.stderr

    slot_dir = target.parent
    fenced = slot_dir.with_name(".slot01.fenced.test")
    os.rename(slot_dir, fenced)
    moved_nested = fenced / target.name / "ignored" / "nested" / "agent-utils"

    config = wrkslots_cli._load_config(str(project), "testhost")
    record = wrkslots_cli._find_record(wrkslots_cli._load_active(config), "slot01")
    nested_checkout = dataclasses.replace(
        record.checkouts[0], name="product/ignored/nested/agent-utils"
    )
    vcs = wrkslots_cli._GitVcs()
    trees = wrkslots_cli._index_state_trees(vcs, config, nested_checkout, moved_nested)
    assert trees
    assert all(len(tree) == 40 for tree in trees)
    # The worktree tree still carries the submodule's gitlink, at the
    # submodule's current HEAD, exactly as a healthy `git add` would.
    gitdir = wrkslots_cli._worktree_gitdir(moved_nested / "leafmod")
    assert gitdir is not None
    head = vcs._run(
        moved_nested,
        [f"--git-dir={gitdir}", "rev-parse", "--verify", "HEAD^{commit}"],
    ).stdout.strip()
    listing = run(moved_nested, "git", "ls-tree", trees[-1], "leafmod").stdout
    assert listing.startswith(f"160000 commit {head}\t")
