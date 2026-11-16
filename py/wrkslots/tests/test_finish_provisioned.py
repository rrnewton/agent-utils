"""Finish tolerance for provisioning output (#1369), fixture repos only."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from wrkslots.tests.test_lifecycle import (
    active_slots,
    add_recursive_submodules,
    checkout,
    command,
    create,
    finish,
    git,
    make_project,
    mark_owner_dead,
    remove,
    set_liveness,
    update_configuration,
)


def _finish_with(
    project: Path, *extra: str
) -> subprocess.CompletedProcess[str]:
    return command(
        project,
        "finish",
        "slot01",
        "--agent",
        "codex-1",
        "--owner-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--validation",
        "pytest",
        *extra,
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


def test_ignored_only_discard_finishes_and_remove_deletes(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    _ignore(repository, "stray.log", "ignored/")
    assert create(project).returncode == 0
    tree = checkout(project)
    (tree / "stray.log").write_text("stray\n", encoding="utf-8")
    (tree / "ignored").mkdir()
    (tree / "ignored" / "build.o").write_text("object\n", encoding="utf-8")
    assert finish(project).returncode == 3
    finished = _finish_with(project, "--discard-ignored")
    assert finished.returncode == 0, finished.stderr
    assert "disposition=discarded" in finished.stdout
    # Finish never deletes: the files stay until remove.
    assert (tree / "stray.log").is_file()
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    handoff = row["handoff"]
    assert isinstance(handoff, dict)
    recorded = handoff["ignored_files"]
    assert [(item["path"], item["disposition"]) for item in recorded] == [
        ("ignored/build.o", "discarded"),
        ("stray.log", "discarded"),
    ]
    mark_owner_dead(project)
    set_liveness(project, "dead")
    removed = remove(project)
    assert removed.returncode == 0, removed.stderr
    assert not tree.exists()


def test_ignored_only_archive_finishes_with_verified_tar(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    _ignore(repository, "stray.log")
    assert create(project).returncode == 0
    tree = checkout(project)
    (tree / "stray.log").write_text("stray\n", encoding="utf-8")
    finished = _finish_with(project, "--archive-ignored")
    assert finished.returncode == 0, finished.stderr
    assert "disposition=archived" in finished.stdout
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    handoff = row["handoff"]
    assert isinstance(handoff, dict)
    archives = handoff["ignored_archives"]
    assert len(archives) == 1
    tar_path = Path(archives[0]["archive_root"]) / archives[0]["path"]
    assert tar_path.is_file()
    mark_owner_dead(project)
    set_liveness(project, "dead")
    removed = remove(project)
    assert removed.returncode == 0, removed.stderr
    assert not tree.exists()
    # The archive outlives the checkout.
    assert tar_path.is_file()


def test_untracked_nonignored_still_refuses_with_discard(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    _ignore(repository, "stray.log")
    assert create(project).returncode == 0
    tree = checkout(project)
    (tree / "stray.log").write_text("stray\n", encoding="utf-8")
    (tree / "untracked.txt").write_text("work\n", encoding="utf-8")
    refused = _finish_with(project, "--discard-ignored")
    assert refused.returncode == 3
    assert "cover gitignored files only" in refused.stderr


def test_tracked_dirty_still_refuses_with_discard(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    _ignore(repository, "stray.log")
    assert create(project).returncode == 0
    tree = checkout(project)
    (tree / "stray.log").write_text("stray\n", encoding="utf-8")
    (tree / "seed.txt").write_text("changed\n", encoding="utf-8")
    refused = _finish_with(project, "--discard-ignored")
    assert refused.returncode == 3
    assert "cover gitignored files only" in refused.stderr


def test_both_ignored_flags_refuse(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    _ignore(repository, "stray.log")
    assert create(project).returncode == 0
    refused = _finish_with(project, "--archive-ignored", "--discard-ignored")
    assert refused.returncode == 3
    assert "cannot be supplied together" in refused.stderr


def test_provisioned_symlink_deleted_names_restore(tmp_path: Path) -> None:
    project, repository, _remote = make_project(
        tmp_path, post_provision_hooks=("ln -s seed.txt provisioned-link",)
    )
    _ignore(repository, "provisioned-link")
    assert create(project).returncode == 0
    tree = checkout(project)
    (tree / "provisioned-link").unlink()
    refused = finish(project)
    assert refused.returncode == 3
    assert "provisioned path changed: provisioned-link" in refused.stderr
    assert "ln -s seed.txt provisioned-link" in refused.stderr


def test_ignored_change_after_finish_refuses_remove(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    _ignore(repository, "stray.log")
    assert create(project).returncode == 0
    tree = checkout(project)
    (tree / "stray.log").write_text("stray\n", encoding="utf-8")
    assert _finish_with(project, "--discard-ignored").returncode == 0
    (tree / "stray.log").write_text("changed after finish\n", encoding="utf-8")
    mark_owner_dead(project)
    set_liveness(project, "dead")
    refused = remove(project)
    assert refused.returncode == 3
    assert "changed after finish" in refused.stderr
    assert tree.exists()
