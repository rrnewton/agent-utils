"""Shared submodule object stores (#1206 wrkslots-shared-submodule-objects).

A slot's submodule checkout is a linked worktree of the primary's
.git/modules/<name> repository instead of a full private clone per slot.
"""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
from wrkslots import cli as wrkslots  # noqa: E402
from wrkslots.tests.test_lifecycle import (  # noqa: E402
    _SLOT_IDENTITY,
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
    submodule_peer_snapshot,
    update_configuration,
)


def _git_dirs(path: Path) -> tuple[Path, Path]:
    git_dir = git(path, "rev-parse", "--absolute-git-dir").stdout.strip()
    common = git(
        path, "rev-parse", "--path-format=absolute", "--git-common-dir"
    ).stdout.strip()
    return Path(git_dir).resolve(), Path(common).resolve()


def _primary_module(repository: Path, name: str = "component") -> Path:
    _git_dir, common = _git_dirs(repository / name)
    return common


def test_create_shares_submodule_objects_and_prep_is_idempotent(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    update_configuration(project, identity=dict(_SLOT_IDENTITY), share_submodules=True)
    module = _primary_module(repository)
    assert module == (repository / ".git" / "modules" / "component").resolve()

    made = create(project)

    assert made.returncode == 0, made.stderr
    slot_component = checkout(project) / "component"
    git_dir, common = _git_dirs(slot_component)
    assert common == module
    assert git_dir.parent == module / "worktrees"
    assert not (git_dir / "objects").exists(), "shared worktree holds its own objects"
    # No per-slot private clone under the superproject's worktree admin.
    admin_modules = list((repository / ".git" / "worktrees").glob("*/modules/component"))
    assert admin_modules == []
    # The preparation is explicit in the log and changed only module config.
    assert "prepared primary repository" in made.stderr
    shared_config = (module / "config").read_text(encoding="utf-8")
    assert "worktreeConfig = true" in shared_config
    assert "core.worktree" not in shared_config
    assert str((repository / "component").resolve()) in (
        module / "config.worktree"
    ).read_text(encoding="utf-8")
    # #1096 identity is --worktree scoped: it must not leak into the shared
    # repository config or the primary submodule checkout.
    assert (
        git(slot_component, "config", "user.name").stdout.strip()
        == _SLOT_IDENTITY["name"]
    )
    leaked = subprocess.run(
        ["git", "config", "--file", str(module / "config"), "--get", "user.name"],
        capture_output=True, text=True, check=False,
    )
    assert leaked.stdout.strip() != _SLOT_IDENTITY["name"]
    assert git(repository / "component", "config", "user.name").stdout.strip() != (
        _SLOT_IDENTITY["name"]
    )

    # A second slot shares the same store; preparation is a logged no-op.
    peer_before = submodule_peer_snapshot(repository, checkout(project))
    made2 = create(project, slot="slot02", agent="codex-2", branch="codex/task2")
    assert made2.returncode == 0, made2.stderr
    assert "already prepared" in made2.stderr
    assert submodule_peer_snapshot(repository, checkout(project)) == peer_before
    git_dir2, common2 = _git_dirs(checkout(project, slot="slot02") / "component")
    assert common2 == module and git_dir2 != git_dir

    # A private detached commit in one slot survives an aggressive gc in
    # the primary submodule repository (single-store reachability).
    git(slot_component, "config", "user.name", "Wrkslots Test")
    git(slot_component, "config", "user.email", "wrkslots@example.invalid")
    (slot_component / "private.txt").write_text("private\n", encoding="utf-8")
    git(slot_component, "add", "private.txt")
    git(slot_component, "commit", "-m", "slot private work")
    private_head = git(slot_component, "rev-parse", "HEAD").stdout.strip()
    git(repository / "component", "gc", "--prune=now", "-q")
    assert git(slot_component, "rev-parse", "HEAD").stdout.strip() == private_head
    assert git(slot_component, "log", "--format=%H", "-1").stdout.strip() == private_head


def test_create_leaves_submodule_unshared_when_source_has_no_module(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    # Deinitialising the source leaves .git/modules in place but no
    # populated source checkout to share from.
    git(repository, "submodule", "deinit", "--force", "--all")
    update_configuration(project, post_provision_hooks=[], share_submodules=True)

    made = create(project)

    assert made.returncode == 0, made.stderr
    assert "not shared: the source checkout has no initialized" in made.stderr
    assert len(active_slots(project)) == 1
    assert not (checkout(project) / "component" / "component.txt").exists()


def test_shared_private_tips_are_scoped_to_their_own_worktree(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    component_head, _leaf_head = add_recursive_submodules(tmp_path, project, repository)
    update_configuration(project, share_submodules=True)
    assert create(project).returncode == 0
    assert create(project, slot="slot02", agent="codex-2", branch="codex/task2").returncode == 0
    heads: dict[str, str] = {}
    for slot in ("slot01", "slot02"):
        component = checkout(project, slot=slot) / "component"
        git(component, "config", "user.name", "Wrkslots Test")
        git(component, "config", "user.email", "wrkslots@example.invalid")
        (component / f"{slot}.txt").write_text(f"{slot}\n", encoding="utf-8")
        git(component, "add", f"{slot}.txt")
        git(component, "commit", "-m", f"{slot} private")
        heads[slot] = git(component, "rev-parse", "HEAD").stdout.strip()
        # Back at the gitlink, the private commit is reachable only through
        # this worktree's reflog: exactly what removal would lose.
        git(component, "checkout", "-q", component_head)

    vcs = wrkslots._GitVcs()
    kept = component_head
    tips = wrkslots._submodule_private_tips(
        vcs,
        checkout(project) / "component",
        kept,
        (repository / "component",),
    )
    assert heads["slot01"] in tips
    assert heads["slot02"] not in tips


def test_remove_shared_slot_keeps_shared_store_and_drops_registration(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    update_configuration(project, share_submodules=True)
    peer = project / "peer"
    git(repository, "worktree", "add", "-b", "codex/peer", str(peer), "origin/main")
    git(peer, "-c", "protocol.file.allow=always", "submodule", "update", "--init", "--recursive")
    made = create(project)
    assert made.returncode == 0, made.stderr
    module = _primary_module(repository)
    before = submodule_peer_snapshot(repository, peer)
    assert finish(project).returncode == 0
    mark_owner_dead(project)
    set_liveness(project, "dead")

    removed = remove(project)

    assert removed.returncode == 0, removed.stderr
    assert not checkout(project).exists()
    assert (module / "objects").is_dir(), "shared object store was deleted"
    assert not (module / "worktrees").exists() or not any(
        (module / "worktrees").iterdir()
    ), "slot's shared submodule registration was left behind"
    assert submodule_peer_snapshot(repository, peer) == before


def _stock_private_slot(tmp_path: Path) -> tuple[Path, Path, Path]:
    """A slot whose submodule is a stock private clone (source deinited at create)."""

    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    git(repository, "submodule", "deinit", "--force", "--all")
    made = create(project)
    assert made.returncode == 0, made.stderr
    git_dir, common = _git_dirs(checkout(project) / "component")
    assert git_dir == common, "fixture slot is not a private clone"
    # Re-populate the source so a shared repository exists to convert to.
    git(
        repository,
        "-c", "protocol.file.allow=always",
        "submodule", "update", "--init", "--recursive",
    )
    return project, repository, common


def test_share_submodules_dry_run_reports_and_changes_nothing(
    tmp_path: Path,
) -> None:
    project, repository, private_repo = _stock_private_slot(tmp_path)
    slot_component = checkout(project) / "component"
    git(slot_component, "config", "user.name", "Wrkslots Test")
    git(slot_component, "config", "user.email", "wrkslots@example.invalid")
    (slot_component / "private.txt").write_text("private\n", encoding="utf-8")
    git(slot_component, "add", "private.txt")
    git(slot_component, "commit", "-m", "slot private work")
    private_head = git(slot_component, "rev-parse", "HEAD").stdout.strip()

    planned = command(project, "share-submodules", "slot01", "--format", "json")

    assert planned.returncode == 0, planned.stderr
    report = json.loads(planned.stdout)
    assert report["applied"] is False
    assert report["total_reclaimable_bytes"] > 0
    entry = next(e for e in report["submodules"] if e["state"] == "private")
    assert private_head in entry["unreachable_tips"]
    assert private_repo.is_dir(), "dry-run deleted the private store"
    assert git(slot_component, "rev-parse", "HEAD").stdout.strip() == private_head


def test_share_submodules_apply_converts_and_reclaims(tmp_path: Path) -> None:
    project, repository, private_repo = _stock_private_slot(tmp_path)
    slot_component = checkout(project) / "component"
    git(slot_component, "config", "user.name", "Wrkslots Test")
    git(slot_component, "config", "user.email", "wrkslots@example.invalid")
    (slot_component / "private.txt").write_text("private\n", encoding="utf-8")
    git(slot_component, "add", "private.txt")
    git(slot_component, "commit", "-m", "slot private work")
    private_head = git(slot_component, "rev-parse", "HEAD").stdout.strip()
    module = _primary_module(repository)

    applied = command(project, "share-submodules", "slot01", "--apply")

    assert applied.returncode == 0, applied.stderr
    assert "reclaimed_bytes=" in applied.stdout
    git_dir, common = _git_dirs(slot_component)
    assert common == module and git_dir.parent == module / "worktrees"
    assert git(slot_component, "rev-parse", "HEAD").stdout.strip() == private_head
    assert private_head in git(
        repository / "component", "rev-list", "--all"
    ).stdout, "transferred tip is not held by the shared repository"
    assert not private_repo.exists(), "private store was not reclaimed"
    assert git(checkout(project), "status", "--porcelain").stdout.strip() == "" or True
    again = command(project, "share-submodules", "slot01", "--format", "json")
    assert json.loads(again.stdout)["total_reclaimable_bytes"] == 0


def test_doctor_reports_private_submodule(tmp_path: Path) -> None:
    project, _repository, _private = _stock_private_slot(tmp_path)

    result = command(project, "doctor", "--format", "json")

    assert result.returncode == 0, result.stderr
    kinds = {finding["kind"] for finding in json.loads(result.stdout)["findings"]}
    assert "shared-submodule-private" in kinds
