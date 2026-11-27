"""Finish tolerance for provisioning output (#1369), fixture repos only."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

import pytest

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


#: `#2299 finish-archive-remove-budget`. A HANG backstop, not a budget: the
#: four tests that remove a finished slot spend most of their time in the
#: remove census's host-wide process scan (`lsof +D` over every process on the
#: host), whose cost follows the host's process count and load, not the test.
#: Measured 2026-11-26 on main, 5 solo runs at load1 130-184: 64-201 s per test
#: (the 70-file bounded case 131-201 s); in a gate at load1 ~500-600 three went
#: past the 600 s default and one met the remove batch's 60 s budget. No pass or
#: fail depends on this limit; it only reports a hang. The real fix is
#: `#1896 wrkslots-batch-census-without-lsof` and `#2219
#: wrkslots-cpu-time-budgets`; until they land these tests are slow, which the
#: fast-tests rule (owner reply 346) does not allow, so they must come back down
#: or leave the default selection.
_HOST_CENSUS_BACKSTOP = pytest.mark.timeout(1800)


def _unbudgeted_remove(project: Path) -> subprocess.CompletedProcess[str]:
    """Remove the finished slot without the batch item's wall-clock budget
    (`#2299`): that budget is host-load timing, not what these tests prove."""
    return command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--no-lock-budget",
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


@_HOST_CENSUS_BACKSTOP
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
    removed = _unbudgeted_remove(project)
    assert removed.returncode == 0, removed.stderr
    assert not tree.exists()


@_HOST_CENSUS_BACKSTOP
def test_many_ignored_files_finish_bounded_and_remove_enforces(
    tmp_path: Path,
) -> None:
    """#1772: finish records a summary plus manifest, remove enforces it."""

    project, repository, _remote = make_project(tmp_path)
    _ignore(repository, "ignored/")
    assert create(project).returncode == 0
    tree = checkout(project)
    bulk = tree / "ignored" / "bulk"
    bulk.mkdir(parents=True)
    for index in range(70):
        (bulk / f"file-{index:03d}.bin").write_text(f"bulk-{index}\n")
    finished = _finish_with(project, "--discard-ignored")
    assert finished.returncode == 0, finished.stderr
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    handoff = row["handoff"]
    assert isinstance(handoff, dict)
    recorded = handoff["ignored_files"]
    assert isinstance(recorded, list)
    assert len(recorded) == 64
    summary = handoff["ignored_summary"]
    assert isinstance(summary, dict)
    assert summary["count"] == 70
    assert summary["dispositions"] == {"discarded": 70}
    # A changed file after finish still refuses removal, judged through
    # the manifest the summary names. The batch item budget is host-load
    # timing, not what this test proves, so remove runs unbudgeted.
    (bulk / "file-000.bin").write_text("changed\n")
    mark_owner_dead(project)
    set_liveness(project, "dead")

    def unbudgeted_remove() -> subprocess.CompletedProcess[str]:
        return command(
            project,
            "remove",
            "slot01",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
            "--no-lock-budget",
        )

    refused = unbudgeted_remove()
    assert refused.returncode != 0
    assert "changed after finish" in (refused.stdout + refused.stderr)
    (bulk / "file-000.bin").write_text("bulk-0\n")
    removed = unbudgeted_remove()
    assert removed.returncode == 0, removed.stderr
    assert not tree.exists()


@_HOST_CENSUS_BACKSTOP
def test_many_ignored_files_below_writer_ceiling_stay_legacy(
    tmp_path: Path,
) -> None:
    """#1772 at writer_ceiling 2: the full list stays inline (#1788).

    ignored_summary is record schema level 3; below the ceiling the
    writer emits the equally checked older form (every file inline, no
    summary, no manifest) so a pinned older reader keeps working.
    """

    project, repository, _remote = make_project(tmp_path)
    _ignore(repository, "ignored/")
    update_configuration(project, writer_ceiling=2)
    assert create(project).returncode == 0
    tree = checkout(project)
    bulk = tree / "ignored" / "bulk"
    bulk.mkdir(parents=True)
    for index in range(70):
        (bulk / f"file-{index:03d}.bin").write_text(f"bulk-{index}\n")
    finished = _finish_with(project, "--discard-ignored")
    assert finished.returncode == 0, finished.stderr
    row = active_slots(project)[0]
    assert isinstance(row, dict)
    handoff = row["handoff"]
    assert isinstance(handoff, dict)
    assert "ignored_summary" not in handoff
    recorded = handoff["ignored_files"]
    assert isinstance(recorded, list)
    assert len(recorded) == 70
    mark_owner_dead(project)
    set_liveness(project, "dead")
    removed = command(
        project,
        "remove",
        "slot01",
        "--coordinator-pid",
        str(os.getpid()),
        "--expected-generation",
        "1",
        "--no-lock-budget",
    )
    assert removed.returncode == 0, removed.stderr
    assert not tree.exists()


@_HOST_CENSUS_BACKSTOP
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
    removed = _unbudgeted_remove(project)
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
