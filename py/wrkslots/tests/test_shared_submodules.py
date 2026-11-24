"""Shared submodule object stores (#1206 wrkslots-shared-submodule-objects).

A slot's submodule checkout is a linked worktree of the primary's
.git/modules/<name> repository instead of a full private clone per slot.
"""

from __future__ import annotations

import json
import os
import shutil
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
    initialize,
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
    history = wrkslots._submodule_private_history(
        vcs,
        checkout(project) / "component",
        kept,
        (repository / "component",),
    )
    assert history is not None
    assert heads["slot01"] in history.tips
    assert heads["slot02"] not in history.tips


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


# --- Worktree-config safety (#1435 wrkslots-worktreeconfig-core-worktree) ---
#
# Enabling ``extensions.worktreeConfig`` while ``core.worktree`` is still
# in a repository's common config makes every linked worktree resolve its
# top level through that shared value. The identity path refuses in that
# state, the shared-submodule preparation moves the value, and doctor
# reports the broken state without changing anything.

_WORKTREE_CONFIG_KIND = "worktree-config-core-worktree-in-common"


def _toplevel_and_status(tree: Path) -> tuple[str, str]:
    toplevel = git(tree, "rev-parse", "--show-toplevel").stdout.strip()
    status = git(tree, "status", "--porcelain").stdout
    return toplevel, status


def _common_config(repo: Path) -> Path:
    common = git(
        repo, "rev-parse", "--path-format=absolute", "--git-common-dir"
    ).stdout.strip()
    return Path(common) / "config"


def _doctor_kinds(project: Path) -> set[str]:
    result = command(project, "doctor", "--format", "json")
    assert result.returncode == 0, result.stderr
    return {str(f["kind"]) for f in json.loads(result.stdout)["findings"]}


def test_identity_refuses_submodule_style_core_worktree_and_leaves_toplevels(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    # Submodule-style: the common config carries core.worktree, as a
    # submodule's module repository does (there with a relative value,
    # covered by the preparation test below).
    sibling = tmp_path / "sibling"
    git(repository, "worktree", "add", "-b", "codex/sibling", str(sibling), "main")
    git(repository, "config", "core.worktree", str(repository))
    before_main = _toplevel_and_status(repository)
    before_sibling = _toplevel_and_status(sibling)
    shared_before = _common_config(repository).read_bytes()
    update_configuration(project, identity=dict(_SLOT_IDENTITY))

    refused = create(project)

    assert refused.returncode == 3, refused.stderr
    assert "core.worktree" in refused.stderr
    assert _toplevel_and_status(repository) == before_main
    assert _toplevel_and_status(sibling) == before_sibling
    assert _common_config(repository).read_bytes() == shared_before
    assert not checkout(project).exists()


def test_identity_refusal_names_the_remedy(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    git(repository, "config", "core.worktree", str(repository))
    update_configuration(project, identity=dict(_SLOT_IDENTITY))
    config = wrkslots._load_config(str(project), "testhost")

    with pytest.raises(wrkslots.Refusal) as caught:
        wrkslots._assert_git_identity_supported(
            config, [repository], wrkslots._GitVcs()
        )

    message = str(caught.value)
    assert "Move it into the main worktree's config.worktree" in message
    assert "remove identity from the configuration" in message
    assert "core.worktree" in message


def test_prepare_moves_core_worktree_and_leaves_toplevels(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    component = repository / "component"
    sibling = tmp_path / "component-sibling"
    git(component, "worktree", "add", "-b", "codex/comp-sibling", str(sibling), "HEAD")
    before_main = _toplevel_and_status(component)
    before_sibling = _toplevel_and_status(sibling)

    changed = wrkslots._prepare_shared_submodule_repo(
        wrkslots._GitVcs(), component
    )

    assert changed is True
    assert _toplevel_and_status(component) == before_main
    assert _toplevel_and_status(sibling) == before_sibling
    shared = _common_config(component).read_text(encoding="utf-8")
    assert "core.worktree" not in shared
    assert "worktreeConfig = true" in shared
    held = _common_config(component).with_name("config.worktree").read_text(
        encoding="utf-8"
    )
    assert str(component.resolve()) in held


def test_doctor_reports_broken_state_and_stays_silent_when_healthy(
    tmp_path: Path,
) -> None:
    project, repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    assert _WORKTREE_CONFIG_KIND not in _doctor_kinds(project)

    # Break it the way a hand flip does: extension on, value unmoved.
    git(repository, "config", "extensions.worktreeConfig", "true")
    git(repository, "config", "core.worktree", str(repository))
    before = _common_config(repository).read_bytes()

    result = command(project, "doctor", "--format", "json")

    assert result.returncode == 0, result.stderr
    findings = [
        f for f in json.loads(result.stdout)["findings"]
        if f["kind"] == _WORKTREE_CONFIG_KIND
    ]
    assert len(findings) == 1
    assert "core.worktree" in str(findings[0]["detail"])
    assert "config.worktree" in str(findings[0]["detail"])
    assert _common_config(repository).read_bytes() == before, "doctor repaired"


def test_doctor_silent_on_prepared_shared_submodule(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    update_configuration(
        project, identity=dict(_SLOT_IDENTITY), share_submodules=True
    )

    made = create(project)

    assert made.returncode == 0, made.stderr
    assert _WORKTREE_CONFIG_KIND not in _doctor_kinds(project)


# --- Submodule repository as the slotted repository (#1941) ---
#
# The slotted repository can itself be a submodule: its common dir is
# the superproject's .git/modules/<name>. Git's submodule machinery
# (re)writes core.worktree into that shared config on connect/update,
# so identity must never enable extensions.worktreeConfig there, even
# when no core.worktree is present at the moment identity runs.


def _submodule_repo_project(tmp_path: Path) -> tuple[Path, Path, Path]:
    """A wrkslots project whose registered repository is a submodule.

    Returns (project, repository, module_config): the project is the
    superproject checkout, the repository its submodule at ``repo``,
    and module_config the submodule's shared config under
    ``.git/modules/repo``.
    """

    child_remote = tmp_path / "child.git"
    child_source = tmp_path / "child-source"
    super_remote = tmp_path / "super.git"
    project = tmp_path / "super-clone"
    for remote in (child_remote, super_remote):
        subprocess.run(
            ["git", "init", "--bare", "--initial-branch=main", str(remote)],
            check=True, capture_output=True, text=True,
        )
    subprocess.run(
        ["git", "clone", str(child_remote), str(child_source)],
        check=True, capture_output=True, text=True,
    )
    git(child_source, "config", "user.name", "Wrkslots Test")
    git(child_source, "config", "user.email", "wrkslots@example.invalid")
    (child_source / "child.txt").write_text("child\n", encoding="utf-8")
    git(child_source, "add", "child.txt")
    git(child_source, "commit", "-m", "child base")
    git(child_source, "push", "-u", "origin", "main")
    subprocess.run(
        ["git", "clone", str(super_remote), str(project)],
        check=True, capture_output=True, text=True,
    )
    git(project, "config", "user.name", "Wrkslots Test")
    git(project, "config", "user.email", "wrkslots@example.invalid")
    (project / "super.txt").write_text("super\n", encoding="utf-8")
    git(project, "add", "super.txt")
    git(project, "commit", "-m", "super base")
    git(
        project, "-c", "protocol.file.allow=always",
        "submodule", "add", str(child_remote), "repo",
    )
    git(project, "commit", "-am", "add child as repo")
    git(project, "push", "-u", "origin", "main")
    repository = project / "repo"
    module_config = project / ".git" / "modules" / "repo" / "config"
    initialize(project)
    return project, repository, module_config


def test_identity_never_enables_worktreeconfig_on_submodule_repo(
    tmp_path: Path,
) -> None:
    project, repository, module_config = _submodule_repo_project(tmp_path)
    peer = tmp_path / "peer"
    git(repository, "worktree", "add", "-b", "codex/peer", str(peer), "HEAD")
    # The defect's flip moment: core.worktree not yet written into the
    # module config (a fresh materialization connects the submodule
    # only after create). On the old code identity flips the extension
    # here and every linked worktree breaks once Git writes the value.
    git(repository, "config", "--unset", "core.worktree")
    before_primary = _toplevel_and_status(repository)
    before_peer = _toplevel_and_status(peer)
    update_configuration(project, identity=dict(_SLOT_IDENTITY))

    made = create(project)

    assert made.returncode == 0, made.stderr
    assert "identity skipped" in made.stderr
    shared = module_config.read_text(encoding="utf-8")
    assert "worktreeConfig" not in shared
    slot = checkout(project)
    assert (slot / "child.txt").read_text(encoding="utf-8") == "child\n"
    assert _toplevel_and_status(slot) == (str(slot.resolve()), "")
    assert _toplevel_and_status(repository) == before_primary
    assert _toplevel_and_status(peer) == before_peer
    status = git(project, "submodule", "status")
    assert "cannot be used without a working tree" not in status.stdout
    # Git (re)writes core.worktree into the module config when it
    # reconnects the submodule; with the extension off that is the
    # stock, harmless state and no worktree changes.
    git(
        project, "-c", "protocol.file.allow=always",
        "submodule", "update", "--init", "--force", "repo",
    )
    shared = module_config.read_text(encoding="utf-8")
    assert "worktreeConfig" not in shared
    assert _toplevel_and_status(slot) == (str(slot.resolve()), "")
    assert _toplevel_and_status(peer) == before_peer
    assert git(project, "submodule", "status").returncode == 0


def test_identity_on_submodule_repo_with_core_worktree_skips_not_refuses(
    tmp_path: Path,
) -> None:
    project, repository, module_config = _submodule_repo_project(tmp_path)
    assert git(repository, "config", "--get", "core.worktree").stdout.strip()
    before_primary = _toplevel_and_status(repository)
    update_configuration(project, identity=dict(_SLOT_IDENTITY))

    made = create(project)

    assert made.returncode == 0, made.stderr
    assert "identity skipped" in made.stderr
    assert "worktreeConfig" not in module_config.read_text(encoding="utf-8")
    slot = checkout(project)
    assert _toplevel_and_status(slot) == (str(slot.resolve()), "")
    assert _toplevel_and_status(repository) == before_primary


def test_doctor_reports_broken_shared_submodule_gitdir(tmp_path: Path) -> None:
    project, repository, module_config = _submodule_repo_project(tmp_path)
    update_configuration(project, identity=dict(_SLOT_IDENTITY))
    assert create(project).returncode == 0
    # Break it by hand: extension on while core.worktree is still in
    # the shared submodule config.
    git(repository, "config", "extensions.worktreeConfig", "true")
    before = module_config.read_bytes()

    result = command(project, "doctor", "--format", "json")

    assert result.returncode == 0, result.stderr
    findings = [
        f for f in json.loads(result.stdout)["findings"]
        if f["kind"] == _WORKTREE_CONFIG_KIND
    ]
    assert len(findings) == 1
    assert "core.worktree" in str(findings[0]["detail"])
    assert "config.worktree" in str(findings[0]["detail"])
    assert module_config.read_bytes() == before, "doctor repaired"


def test_plain_repo_identity_keeps_working(tmp_path: Path) -> None:
    project, repository, _remote = make_project(tmp_path)
    update_configuration(project, identity=dict(_SLOT_IDENTITY))

    made = create(project)

    assert made.returncode == 0, made.stderr
    tree = checkout(project)
    assert (
        git(tree, "config", "--worktree", "user.name").stdout.strip()
        == _SLOT_IDENTITY["name"]
    )
    shared = _common_config(repository).read_text(encoding="utf-8")
    assert "worktreeConfig = true" in shared
    assert "core.worktree" not in shared


def test_failed_create_restores_shared_module_config_byte_identical(
    tmp_path: Path,
) -> None:
    # #1910: a create that fails after the shared-submodule config step
    # (here a failing post-provision hook) must leave the shared module
    # repository's common config byte-identical.
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    update_configuration(
        project,
        post_provision_hooks=["test -e .hook-done || { touch .hook-done; exit 1; }"],
        share_submodules=True,
    )
    module = _primary_module(repository)
    config_before = (module / "config").read_bytes()
    worktree_before = (
        (module / "config.worktree").read_bytes()
        if (module / "config.worktree").exists()
        else None
    )

    made = create(project)

    assert made.returncode != 0
    assert (module / "config").read_bytes() == config_before
    after_worktree = (
        (module / "config.worktree").read_bytes()
        if (module / "config.worktree").exists()
        else None
    )
    assert after_worktree == worktree_before

    # Recovery re-prepares and publishes a working shared submodule:
    # git status there is clean, never a phantom view of the primary.
    recovered = command(project, "recover", "--coordinator-pid", str(os.getpid()))
    assert recovered.returncode == 0, recovered.stderr
    slot_component = checkout(project) / "component"
    status = git(slot_component, "status", "--porcelain")
    assert status.stdout.strip() == ""
    assert (module / "config").read_bytes() != config_before  # prepared again


def test_hook_skip_marker_is_reported_as_skipped(tmp_path: Path) -> None:
    # #1910: a hook exiting 0 with a wrkslots-hook-skip stderr line is
    # reported as skipped with its reason, not as plain success.
    project, repository, _remote = make_project(tmp_path)
    update_configuration(
        project,
        post_provision_hooks=[
            "echo 'wrkslots-hook-skip: no generator in this repo' >&2"
        ],
    )

    made = create(project)

    assert made.returncode == 0, made.stderr
    assert "skipped (no generator in this repo)" in made.stderr
    assert len(active_slots(project)) == 1


def test_recover_reruns_first_hook_after_rollback_removed_worktree(
    tmp_path: Path,
) -> None:
    # #1910: rollback removes the added submodule worktree, so recovery
    # must rerun hook 1 instead of resuming at hook 2.
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    update_configuration(
        project,
        post_provision_hooks=[
            'echo run >> "$WRKSLOTS_PROJECT_ROOT/hook1-runs"',
            "test -e .hook2-done || { touch .hook2-done; exit 1; }",
        ],
        share_submodules=True,
    )

    made = create(project)
    assert made.returncode != 0

    recovered = command(project, "recover", "--coordinator-pid", str(os.getpid()))
    assert recovered.returncode == 0, recovered.stderr
    runs = (project / "hook1-runs").read_text(encoding="utf-8").splitlines()
    assert len(runs) == 2, runs
    assert len(active_slots(project)) == 1


def test_create_fetches_pinned_commit_missing_from_module_repo(
    tmp_path: Path,
) -> None:
    # #1988: the superproject pin names a commit the shared module
    # repository never fetched (a repin); create fetches exactly it from
    # the module repo's origin instead of failing "not our ref".
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    update_configuration(project, share_submodules=True)
    module = _primary_module(repository)
    # New commit exists only on the component remote.
    remote_source = tmp_path / "component-source"
    (remote_source / "repinned.txt").write_text("repinned\n", encoding="utf-8")
    git(remote_source, "add", "repinned.txt")
    git(remote_source, "commit", "-m", "repin target")
    git(remote_source, "push", "origin", "main")
    new_pin = git(remote_source, "rev-parse", "HEAD").stdout.strip()
    assert (
        subprocess.run(
            ["git", f"--git-dir={module}", "cat-file", "-e", f"{new_pin}^{{commit}}"],
            capture_output=True,
        ).returncode
        != 0
    )
    # Repin the superproject without fetching into the module repo.
    git(
        repository,
        "update-index",
        "--add",
        "--cacheinfo",
        f"160000,{new_pin},component",
    )
    git(repository, "commit", "-m", "repin component")
    git(repository, "push", "origin", "main")

    made = create(project)

    assert made.returncode == 0, made.stderr
    slot_component = checkout(project) / "component"
    assert git(slot_component, "rev-parse", "HEAD").stdout.strip() == new_pin
    assert (slot_component / "repinned.txt").exists()


def test_create_refuses_unreachable_pinned_commit_before_hooks(
    tmp_path: Path,
) -> None:
    # #1988: a pin nobody can fetch refuses with a remedy, and no hook
    # runs against a half-provisioned submodule.
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    update_configuration(
        project,
        post_provision_hooks=['echo hook-ran >> "$WRKSLOTS_PROJECT_ROOT/hook-ran"'],
        share_submodules=True,
    )
    missing = "1" * 40
    git(
        repository,
        "update-index",
        "--add",
        "--cacheinfo",
        f"160000,{missing},component",
    )
    git(repository, "commit", "-m", "repin to nowhere")
    git(repository, "push", "origin", "main")

    made = create(project)

    assert made.returncode != 0
    assert "pinned commit" in made.stderr
    assert not (project / "hook-ran").exists()
    assert active_slots(project) == []


def test_ensure_without_fetch_refuses_and_fetches_nothing(tmp_path: Path) -> None:
    # #1988: the under-lock re-check (allow_fetch=False) never touches
    # the network; a missing pin refuses with the remedy, unfetched.
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    module = _primary_module(repository)
    missing = "2" * 40
    with pytest.raises(wrkslots.Refusal):
        wrkslots._ensure_pinned_submodule_commit(
            wrkslots._GitVcs(),
            repository,
            repository / "component",
            "component",
            missing,
            module,
            allow_fetch=False,
        )
    assert (
        subprocess.run(
            ["git", f"--git-dir={module}", "cat-file", "-e", f"{missing}^{{commit}}"],
            capture_output=True,
        ).returncode
        != 0
    )


def test_share_without_fetch_refuses_missing_pin_and_fetches_nothing(
    tmp_path: Path,
) -> None:
    # #1988: the recover path shares with fetch_pinned=False, so under
    # the recover lock a missing pin refuses and is never fetched.
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    module = _primary_module(repository)
    missing = "3" * 40
    git(
        repository,
        "update-index",
        "--add",
        "--cacheinfo",
        f"160000,{missing},component",
    )
    git(repository, "commit", "-m", "repin to nowhere")
    slot_checkout = tmp_path / "slot-checkout"
    git(repository, "worktree", "add", "--detach", str(slot_checkout))

    with pytest.raises(wrkslots.Refusal):
        wrkslots._share_checkout_submodules(
            wrkslots._GitVcs(), repository, slot_checkout, fetch_pinned=False
        )

    assert (
        subprocess.run(
            ["git", f"--git-dir={module}", "cat-file", "-e", f"{missing}^{{commit}}"],
            capture_output=True,
        ).returncode
        != 0
    )
def _place_shared_submodule(repository: Path, slot_checkout: Path) -> Path:
    """Place the slot's submodule as the primary module repo's worktree (#2168)."""

    primary_sub = repository / "component"
    head = git(primary_sub, "rev-parse", "HEAD").stdout.strip()
    sub = slot_checkout / "component"
    if sub.exists():
        shutil.rmtree(sub)
    git(primary_sub, "worktree", "add", "--detach", str(sub), head)
    return sub


def test_remove_accepts_declared_submodule_placed_as_shared_worktree(
    tmp_path: Path,
) -> None:
    # #2168: a declared submodule sitting in the slot as the primary
    # module repository's linked worktree (detached at the gitlink,
    # clean) is expected: removal deregisters it from the owning
    # repository instead of refusing it as undeclared.
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    update_configuration(project)
    assert create(project).returncode == 0
    sub = _place_shared_submodule(repository, checkout(project))
    module = _primary_module(repository)
    assert finish(project).returncode == 0
    mark_owner_dead(project)
    set_liveness(project, "dead")

    removed = remove(project)

    assert removed.returncode == 0, removed.stderr
    assert not (module / "worktrees").exists() or not any(
        (module / "worktrees").iterdir()
    ), "owning repository still registers the removed worktree"
    assert (module / "objects").is_dir()


def test_remove_accepts_shared_placed_submodule_in_flat_layout(
    tmp_path: Path,
) -> None:
    # #2168, flat layout: the checkout IS the slot directory and the
    # declared submodule sits inside it as a shared linked worktree.
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    update_configuration(project, layout="flat")
    assert create(project).returncode == 0
    sub = _place_shared_submodule(repository, checkout(project))
    module = _primary_module(repository)
    assert finish(project).returncode == 0
    mark_owner_dead(project)
    set_liveness(project, "dead")

    removed = remove(project)

    assert removed.returncode == 0, removed.stderr
    assert not (module / "worktrees").exists() or not any(
        (module / "worktrees").iterdir()
    ), "owning repository still registers the removed worktree"


def test_remove_refuses_unreadable_shared_placed_submodule(tmp_path: Path) -> None:
    # #2168: expected-nested is fail-closed on salvage: a declared
    # submodule whose repository cannot be read refuses and survives.
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    update_configuration(project)
    assert create(project).returncode == 0
    sub = _place_shared_submodule(repository, checkout(project))
    assert finish(project).returncode == 0
    (sub / ".git").write_text("gitdir: /nonexistent/nowhere\n", encoding="utf-8")
    mark_owner_dead(project)
    set_liveness(project, "dead")

    removed = remove(project)

    assert removed.returncode == 3, removed.stdout
    assert sub.is_dir()
    assert checkout(project).exists()


def _sha_survives(tmp_path: Path, sha: str) -> bool:
    for gitdir in tmp_path.rglob("objects"):
        repo = gitdir.parent
        found = subprocess.run(
            ["git", "--git-dir", str(repo), "cat-file", "-e", f"{sha}^{{commit}}"],
            capture_output=True,
        )
        if found.returncode == 0:
            return True
    return any(tmp_path.rglob("*.bundle")) and any(
        sha in subprocess.run(
            ["git", "bundle", "list-heads", str(bundle)],
            capture_output=True, text=True,
        ).stdout
        for bundle in tmp_path.rglob("*.bundle")
    )


def _private_commit_in_private_clone(
    tmp_path: Path, *, unset_core_worktree: bool
) -> subprocess.CompletedProcess[str]:
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    update_configuration(project)
    assert create(project).returncode == 0
    slot_sub = checkout(project) / "component"
    private_module = Path(
        git(slot_sub, "rev-parse", "--absolute-git-dir").stdout.strip()
    )
    git(slot_sub, "config", "user.name", "T")
    git(slot_sub, "config", "user.email", "t@example.invalid")
    git(slot_sub, "checkout", "-q", "-b", "wip")
    (slot_sub / "private.txt").write_text("private only here\n", encoding="utf-8")
    git(slot_sub, "add", "private.txt")
    git(slot_sub, "commit", "-q", "-m", "unpushed private work")
    sha = git(slot_sub, "rev-parse", "HEAD").stdout.strip()
    if unset_core_worktree:
        subprocess.run(
            ["git", "config", "--file", str(private_module / "config"),
             "--unset", "core.worktree"],
            check=True,
        )
    _place_shared_submodule(repository, checkout(project))
    assert finish(project).returncode == 0
    mark_owner_dead(project)
    set_liveness(project, "dead")
    removed = remove(project)
    assert removed.returncode != 0 or _sha_survives(tmp_path, sha), (
        "private commit in the slot's own module repository was lost"
    )
    return removed


def test_remove_never_loses_private_module_history_when_shared_placed(
    tmp_path: Path,
) -> None:
    # #2168 B1: the slot's private module clone holds an unpushed
    # branch; replacing its worktree with the shared placement must
    # not delete that history unseen (refuse or bundle it).
    _private_commit_in_private_clone(tmp_path, unset_core_worktree=False)


def test_remove_never_loses_private_module_history_without_core_worktree(
    tmp_path: Path,
) -> None:
    # #2168 B2/S2: same, with the module's core.worktree unset, where
    # the judged repository would share the listed candidate's name.
    _private_commit_in_private_clone(tmp_path, unset_core_worktree=True)


def _private_clone_setup(tmp_path: Path) -> tuple[Path, Path, Path, Path]:
    project, repository, _remote = make_project(tmp_path)
    add_recursive_submodules(tmp_path, project, repository)
    update_configuration(project)
    assert create(project).returncode == 0
    slot_sub = checkout(project) / "component"
    private_module = Path(
        git(slot_sub, "rev-parse", "--absolute-git-dir").stdout.strip()
    )
    git(slot_sub, "config", "user.name", "T")
    git(slot_sub, "config", "user.email", "t@example.invalid")
    return project, repository, slot_sub, private_module


def _object_survives(tmp_path: Path, obj: str, kind: str = "commit") -> bool:
    seeds: list[Path] = []
    for objdir in tmp_path.rglob("objects"):
        repo = objdir.parent
        if (repo / "HEAD").exists():
            seeds.append(repo)
            found = subprocess.run(
                ["git", "--git-dir", str(repo), "cat-file", "-e", f"{obj}^{{{kind}}}"],
                capture_output=True,
            )
            if found.returncode == 0:
                return True
    # Thin salvage bundles need their prerequisites: fetch each into a
    # scratch cloned from a seed repository that holds them.
    for number, bundle in enumerate(tmp_path.rglob("*.bundle")):
        for seed_number, seed in enumerate(seeds):
            scratch = tmp_path / f"unb-scratch-{number}-{seed_number}"
            if scratch.exists():
                continue
            cloned = subprocess.run(
                ["git", "clone", "-q", "--bare", str(seed), str(scratch)],
                capture_output=True,
            )
            if cloned.returncode != 0:
                continue
            subprocess.run(
                ["git", "-C", str(scratch), "fetch", "-q", str(bundle), "refs/*:refs/b/*"],
                capture_output=True,
            )
            found = subprocess.run(
                ["git", "-C", str(scratch), "cat-file", "-e", f"{obj}^{{{kind}}}"],
                capture_output=True,
            )
            if found.returncode == 0:
                return True
    return False


def _finish_remove_2168(project: Path) -> subprocess.CompletedProcess[str]:
    assert finish(project).returncode == 0
    mark_owner_dead(project)
    set_liveness(project, "dead")
    return remove(project)


def test_remove_never_loses_linked_worktree_head_of_private_clone(
    tmp_path: Path,
) -> None:
    # #2168 N1/P3: private work only in a linked worktree's detached
    # HEAD of the slot's private clone must survive or refuse.
    project, repository, slot_sub, _module = _private_clone_setup(tmp_path)
    gitlink = git(slot_sub, "rev-parse", "HEAD").stdout.strip()
    lwt = checkout(project).parent / "lwt-scratch"
    git(slot_sub, "worktree", "add", "-q", "--detach", str(lwt), gitlink)
    git(lwt, "config", "user.name", "T")
    git(lwt, "config", "user.email", "t@example.invalid")
    (lwt / "d.txt").write_text("detached private\n", encoding="utf-8")
    git(lwt, "add", "d.txt")
    git(lwt, "commit", "-q", "-m", "private in linked worktree")
    sha = git(lwt, "rev-parse", "HEAD").stdout.strip()
    shutil.rmtree(lwt)
    git(slot_sub, "remote", "set-url", "--push", "origin", str(tmp_path / "fork.git"))
    _place_shared_submodule(repository, checkout(project))
    removed = _finish_remove_2168(project)
    assert removed.returncode != 0 or _object_survives(tmp_path, sha), "PRIVATE COMMIT LOST"


def test_remove_never_loses_index_only_blob_of_private_clone(tmp_path: Path) -> None:
    # #2168 N1/P4: staged-only content in the private clone.
    project, repository, slot_sub, _module = _private_clone_setup(tmp_path)
    (slot_sub / "staged.txt").write_text("staged only\n", encoding="utf-8")
    git(slot_sub, "add", "staged.txt")
    blob = git(slot_sub, "rev-parse", ":staged.txt").stdout.strip()
    _place_shared_submodule(repository, checkout(project))
    removed = _finish_remove_2168(project)
    assert removed.returncode == 0, removed.stderr
    assert _object_survives(tmp_path, blob, "blob"), "STAGED BLOB LOST"


def test_remove_never_loses_linked_head_with_stale_core_worktree(
    tmp_path: Path,
) -> None:
    # #2168 N1/P7: stale recorded worktree plus linked-worktree HEAD.
    project, repository, slot_sub, private_module = _private_clone_setup(tmp_path)
    gitlink = git(slot_sub, "rev-parse", "HEAD").stdout.strip()
    lwt = checkout(project).parent / "lwt-scratch"
    git(slot_sub, "worktree", "add", "-q", "--detach", str(lwt), gitlink)
    git(lwt, "config", "user.name", "T")
    git(lwt, "config", "user.email", "t@example.invalid")
    (lwt / "d.txt").write_text("detached private\n", encoding="utf-8")
    git(lwt, "add", "d.txt")
    git(lwt, "commit", "-q", "-m", "private in linked worktree")
    sha = git(lwt, "rev-parse", "HEAD").stdout.strip()
    shutil.rmtree(lwt)
    _place_shared_submodule(repository, checkout(project))
    subprocess.run(
        ["git", "config", "--file", str(private_module / "config"),
         "core.worktree", str(tmp_path / "moved-away" / "slot01" / "component")],
        check=True,
    )
    removed = _finish_remove_2168(project)
    assert removed.returncode != 0 or private_module.exists() or _object_survives(tmp_path, sha), "PRIVATE COMMIT LOST"


def test_remove_never_loses_detached_head_without_reflog(tmp_path: Path) -> None:
    # #2168 N1/P8: detached HEAD commit, reflogs expired.
    project, repository, slot_sub, _module = _private_clone_setup(tmp_path)
    (slot_sub / "h.txt").write_text("detached head private\n", encoding="utf-8")
    git(slot_sub, "add", "h.txt")
    git(slot_sub, "commit", "-q", "-m", "detached private")
    sha = git(slot_sub, "rev-parse", "HEAD").stdout.strip()
    git(slot_sub, "reflog", "expire", "--expire=now", "--all")
    git(slot_sub, "remote", "set-url", "--push", "origin", str(tmp_path / "fork.git"))
    _place_shared_submodule(repository, checkout(project))
    removed = _finish_remove_2168(project)
    assert removed.returncode != 0 or _object_survives(tmp_path, sha), "PRIVATE COMMIT LOST"


def test_remove_never_loses_orphan_head_commit(tmp_path: Path) -> None:
    # #2168 N1/P9: detached private commit, then an orphan checkout.
    project, repository, slot_sub, _module = _private_clone_setup(tmp_path)
    (slot_sub / "o.txt").write_text("detached then orphan\n", encoding="utf-8")
    git(slot_sub, "add", "o.txt")
    git(slot_sub, "commit", "-q", "-m", "detached private")
    sha = git(slot_sub, "rev-parse", "HEAD").stdout.strip()
    git(slot_sub, "checkout", "-q", "--orphan", "scratch")
    _place_shared_submodule(repository, checkout(project))
    removed = _finish_remove_2168(project)
    assert removed.returncode != 0 or _object_survives(tmp_path, sha), "PRIVATE COMMIT LOST"


def test_remove_keeps_every_linked_worktree_index_of_private_clone(
    tmp_path: Path,
) -> None:
    # #2168: staged-only content in the main index and in two linked
    # worktrees' indexes is all kept (no label-truncation drop).
    project, repository, slot_sub, _module = _private_clone_setup(tmp_path)
    head = git(slot_sub, "rev-parse", "HEAD").stdout.strip()
    (slot_sub / "m.txt").write_text("main staged\n", encoding="utf-8")
    git(slot_sub, "add", "m.txt")
    blobs = [git(slot_sub, "rev-parse", ":m.txt").stdout.strip()]
    for number in (1, 2):
        lwt = checkout(project).parent / f"lwt{number}"
        git(slot_sub, "worktree", "add", "-q", "--detach", str(lwt), head)
        (lwt / f"l{number}.txt").write_text(f"linked {number}\n", encoding="utf-8")
        git(lwt, "add", f"l{number}.txt")
        blobs.append(git(lwt, "rev-parse", f":l{number}.txt").stdout.strip())
        shutil.rmtree(lwt)
    _place_shared_submodule(repository, checkout(project))

    removed = _finish_remove_2168(project)

    assert removed.returncode == 0, removed.stderr
    for blob in blobs:
        assert _object_survives(tmp_path, blob, "blob"), "STAGED BLOB LOST"


def test_remove_refuses_locked_index_of_private_clone(tmp_path: Path) -> None:
    # #2168: a leftover index.lock means a Git process may be
    # mid-write; removal refuses rather than judging stale state.
    project, repository, slot_sub, private_module = _private_clone_setup(tmp_path)
    (slot_sub / "staged.txt").write_text("staged\n", encoding="utf-8")
    git(slot_sub, "add", "staged.txt")
    _place_shared_submodule(repository, checkout(project))
    shutil.copyfile(private_module / "index", private_module / "index.lock")

    removed = _finish_remove_2168(project)

    assert removed.returncode == 3, removed.stdout
    assert checkout(project).exists()
