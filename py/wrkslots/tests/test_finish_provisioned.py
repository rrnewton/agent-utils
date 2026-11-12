"""Finish tolerance for provisioning output (#1369), fixture repos only."""

from __future__ import annotations

from pathlib import Path

from wrkslots.tests.test_lifecycle import (
    add_recursive_submodules,
    checkout,
    create,
    finish,
    git,
    make_project,
    update_configuration,
)


def _ignore(repository: Path, *names: str) -> None:
    (repository / ".gitignore").write_text("".join(f"{name}\n" for name in names))
    git(repository, "add", ".gitignore")
    git(repository, "commit", "-q", "-m", "ignore provisioning output")
    git(repository, "push", "-q", "origin", "main")


def test_hook_created_ignored_symlink_finishes(tmp_path: Path) -> None:
    project, repository, _remote = make_project(
        tmp_path, post_provision_hooks=("ln -s seed.txt provisioned-link",)
    )
    _ignore(repository, "provisioned-link")
    assert create(project).returncode == 0
    tree = checkout(project)
    assert (tree / "provisioned-link").is_symlink()
    finished = finish(project)
    assert finished.returncode == 0, finished.stderr
    assert "accounting" in finished.stdout


def test_hook_created_symlink_modified_refuses(tmp_path: Path) -> None:
    project, repository, _remote = make_project(
        tmp_path, post_provision_hooks=("ln -s seed.txt provisioned-link",)
    )
    _ignore(repository, "provisioned-link")
    assert create(project).returncode == 0
    tree = checkout(project)
    (tree / "provisioned-link").unlink()
    (tree / "provisioned-link").symlink_to("other.txt")
    refused = finish(project)
    assert refused.returncode == 3
    assert "provisioned path changed" in refused.stderr


def test_unrelated_ignored_file_refuses(tmp_path: Path) -> None:
    project, repository, _remote = make_project(
        tmp_path, post_provision_hooks=("ln -s seed.txt provisioned-link",)
    )
    _ignore(repository, "provisioned-link", "stray.log")
    assert create(project).returncode == 0
    tree = checkout(project)
    (tree / "stray.log").write_text("stray\n", encoding="utf-8")
    refused = finish(project)
    assert refused.returncode == 3
    assert "is dirty or has untracked/ignored files" in refused.stderr


def test_hook_created_directory_in_submodule_finishes(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    update_configuration(
        project,
        post_provision_hooks=[
            "git -c protocol.file.allow=always submodule update --init --recursive",
            "mkdir -p component/shim/third-party && printf 'shim\\n' > component/shim/third-party/file.txt",
        ],
    )
    made = create(project)
    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    assert (tree / "component" / "shim" / "third-party" / "file.txt").is_file()
    finished = finish(project)
    assert finished.returncode == 0, finished.stderr


def test_existing_slot_without_record_uses_allowlist(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    _ignore(repository, "legacy.log")
    assert create(project).returncode == 0
    tree = checkout(project)
    # A slot whose record predates snapshots: the file appears after
    # create, so nothing recorded it.
    (tree / "legacy.log").write_text("legacy\n", encoding="utf-8")
    refused = finish(project)
    assert refused.returncode == 3
    update_configuration(project, provisioned_allowlist=["legacy.log"])
    finished = finish(project)
    assert finished.returncode == 0, finished.stderr
