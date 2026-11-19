"""#1878: submodule name != path must not lose a deinitialized private commit."""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.test_lifecycle import (
    _bundle_reaches,
    _drift_receipts,
    checkout,
    commit_local,
    commit_task,
    create,
    finish,
    git,
    make_project,
    mark_owner_dead,
    remove,
    set_liveness,
    update_configuration,
)


def _bare_with_commit(tmp_path: Path, name: str) -> Path:
    remote = tmp_path / f"{name}.git"
    source = tmp_path / f"{name}-source"
    subprocess.run(
        ["git", "init", "--bare", "--initial-branch=main", str(remote)],
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "clone", str(remote), str(source)], check=True, capture_output=True
    )
    git(source, "config", "user.name", "T")
    git(source, "config", "user.email", "t@example.invalid")
    (source / f"{name}.txt").write_text(name + "\n", encoding="utf-8")
    git(source, "add", ".")
    git(source, "commit", "-m", name)
    git(source, "push", "-u", "origin", "main")
    return remote


def test_declared_worktree_maps_module_name_to_path(tmp_path: Path) -> None:
    """The `.gitmodules` name-to-path mapping survives `deinit` (#1878)."""

    repository = tmp_path / "super"
    repository.mkdir()
    git(repository, "init", "-q", "-b", "main")
    (repository / ".gitmodules").write_text(
        '[submodule "y"]\n\tpath = x\n\turl = ../alpha.git\n'
        '[submodule "bname"]\n\tpath = y\n\turl = ../beta.git\n',
        encoding="utf-8",
    )
    vcs = wrkslots_cli._GitVcs()
    assert wrkslots_cli._module_declared_worktree(vcs, repository, ["y"]) == (
        repository / "x"
    )
    assert (
        wrkslots_cli._module_declared_worktree(vcs, repository, ["bname"])
        == repository / "y"
    )
    assert wrkslots_cli._module_declared_worktree(vcs, repository, ["gone"]) is None


@pytest.mark.parametrize("b_path", ("z", "y"))
def test_name_differs_from_path_collision(tmp_path: Path, b_path: str) -> None:
    """A: name y, path x (deinit'd, private commit in modules/y). B: path b_path."""

    project, repository, _remote = make_project(tmp_path)
    a_remote = _bare_with_commit(tmp_path, "alpha")
    b_remote = _bare_with_commit(tmp_path, "beta")
    git(
        repository,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--name",
        "y",
        str(a_remote),
        "x",
    )
    git(
        repository,
        "-c",
        "protocol.file.allow=always",
        "submodule",
        "add",
        "--name",
        "bname",
        str(b_remote),
        b_path,
    )
    git(repository, "commit", "-am", "two submodules")
    git(repository, "push", "origin", "main")
    update_configuration(
        project,
        post_provision_hooks=[
            "git -c protocol.file.allow=always submodule update --init --recursive"
        ],
    )
    assert create(project).returncode == 0
    tree = checkout(project)
    commit_task(repository, tree, "codex/task")
    assert finish(project).returncode == 0
    a = tree / "x"
    git(a, "config", "user.name", "T")
    git(a, "config", "user.email", "t@example.invalid")
    git(a, "checkout", "-q", "-b", "local-work")
    private = commit_local(a, "private.txt", "private\n", "private A work")
    gitdir = git(a, "rev-parse", "--absolute-git-dir").stdout.strip()
    git(tree, "-c", "protocol.file.allow=always", "submodule", "deinit", "-f", "x")
    assert git(Path(gitdir), "cat-file", "-e", private, check=False).returncode == 0
    mark_owner_dead(project)
    set_liveness(project, "dead")
    removed = remove(project)
    if removed.returncode != 0:
        # Refusing is safe; silently deleting the only copy is not.
        return
    receipts = _drift_receipts(project)
    reached = any(
        "archive_bundle" in r
        and _bundle_reaches(tmp_path, r, private, f"restored-{i}.git")
        for i, r in enumerate(receipts.values())
    )
    assert reached, f"private commit {private} of A lost; receipts {sorted(receipts)}"
