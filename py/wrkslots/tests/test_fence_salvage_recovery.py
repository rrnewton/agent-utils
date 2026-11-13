"""Fence-rename and stale-receipt salvage recovery (#1483, #1484).

#1483: after the slot is renamed to its fenced name, `git status` in a
linked worktree inside the slot fails in its submodule, whose
`core.worktree` still names the original slot path; salvage's recheck
must repair that instead of refusing. #1484: a salvage receipt left by
an interrupted removal that disagrees with the current checkout must be
superseded (kept aside), never a permanent wedge. Temporary fixtures
only; no wall-clock waits.
"""

from __future__ import annotations

import json
import os
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.test_lifecycle import create, git, make_project


def _git(path: Path, *args: str) -> str:
    done = subprocess.run(
        ["git", "-C", str(path), *args],
        check=True, capture_output=True, text=True, timeout=120,
    )
    return done.stdout.strip()


def _linked_worktree_with_submodule(tmp_path: Path) -> tuple[Path, Path, bytes]:
    """A linked worktree inside a slot, with a dirty submodule (#1483).

    Also returns the submodule gitdir config path and its bytes, so the
    test can prove salvage's reads rewrite no on-disk Git file.
    """
    subsrc = tmp_path / "subsrc"
    subsrc.mkdir()
    _git(tmp_path, "init", "-q", str(subsrc))
    (subsrc / "f").write_text("hi\n", encoding="utf-8")
    _git(subsrc, "add", "f")
    _git(subsrc, "commit", "-qm", "init")
    main = tmp_path / "main"
    main.mkdir()
    _git(tmp_path, "init", "-q", str(main))
    (main / "top").write_text("top\n", encoding="utf-8")
    _git(main, "add", "top")
    _git(main, "commit", "-qm", "init")
    _git(main, "-c", "protocol.file.allow=always", "submodule", "add", "-q", str(subsrc), "sub")
    _git(main, "commit", "-qm", "addsub")
    _git(main, "-c", "protocol.file.allow=always", "submodule", "update", "-q", "--init")
    slot = tmp_path / "slot"
    slot.mkdir()
    _git(main, "worktree", "add", "-q", str(slot / "proj"), "HEAD")
    proj = slot / "proj"
    _git(proj, "-c", "protocol.file.allow=always", "submodule", "update", "-q", "--init")
    wt = proj / "temp" / "wt"
    _git(proj, "worktree", "add", "-q", str(wt), "HEAD")
    _git(wt, "-c", "protocol.file.allow=always", "submodule", "update", "-q", "--init")
    (wt / "sub" / "f").write_text("hi\nchange\n", encoding="utf-8")
    assert "M sub" in _git(wt, "status", "--porcelain")
    gitdir = Path(_git(wt / "sub", "rev-parse", "--absolute-git-dir"))
    config_path = gitdir / "config"
    config_bytes = config_path.read_bytes()
    fenced = tmp_path / ".slot.fenced.1.test"
    os.rename(slot, fenced)
    return fenced / "proj" / "temp" / "wt", config_path, config_bytes


def test_fenced_linked_worktree_submodule_status_is_repaired(tmp_path: Path) -> None:
    moved, config_path, config_bytes = _linked_worktree_with_submodule(tmp_path)
    vcs = wrkslots_cli._GitVcs()
    status = vcs.status(moved, (), include_ignored=False)
    assert "sub" in status
    assert config_path.read_bytes() == config_bytes


def test_fenced_nested_ignored_checkout_submodule_status(tmp_path: Path) -> None:
    """Shape B (#1483): a nested checkout under `ignored/` with a submodule.

    The nested checkout is a standalone clone inside the slot; its
    submodule's `core.worktree` is an absolute path at the original
    slot location, stale after the fence rename.
    """
    subsrc = tmp_path / "leaf-src"
    subsrc.mkdir()
    _git(tmp_path, "init", "-q", str(subsrc))
    (subsrc / "leaf.txt").write_text("leaf\n", encoding="utf-8")
    _git(subsrc, "add", "leaf.txt")
    _git(subsrc, "commit", "-qm", "leaf")
    slot = tmp_path / "slot"
    nested = slot / "ignored" / "nested-checkout"
    nested.parent.mkdir(parents=True)
    _git(tmp_path, "clone", "-q", str(subsrc), str(nested.with_name("seed")))
    seed = nested.with_name("seed")
    _git(seed, "-c", "protocol.file.allow=always", "submodule", "add", "-q", str(subsrc), "leaf")
    _git(seed, "commit", "-qm", "add leaf")
    _git(tmp_path, "clone", "-q", str(seed), str(nested))
    _git(nested, "-c", "protocol.file.allow=always", "submodule", "update", "-q", "--init")
    import shutil

    shutil.rmtree(seed)
    (nested / "leaf" / "leaf.txt").write_text("leaf\nchange\n", encoding="utf-8")
    gitdir = Path(_git(nested / "leaf", "rev-parse", "--absolute-git-dir"))
    config_path = gitdir / "config"
    _git(nested / "leaf", "config", "core.worktree", str(nested / "leaf"))
    config_bytes = config_path.read_bytes()
    assert "M leaf" in _git(nested, "status", "--porcelain")
    fenced = tmp_path / ".slot.fenced.1.test"
    os.rename(slot, fenced)
    moved = fenced / "ignored" / "nested-checkout"
    vcs = wrkslots_cli._GitVcs()
    status = vcs.status(moved, (), include_ignored=False)
    assert "leaf" in status
    moved_config = fenced / config_path.relative_to(slot)
    assert moved_config.read_bytes() == config_bytes


def test_stale_partial_salvage_receipt_is_superseded(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    made = create(project)
    assert made.returncode == 0, made.stderr
    config = wrkslots_cli._load_config(str(project), "testhost")
    record = wrkslots_cli._load_active(config).slots[0]
    checkout = record.checkouts[0]
    path = wrkslots_cli._stored_path(config, checkout.path, "checkout path")
    (path / "task.txt").write_text("unpublished\n", encoding="utf-8")
    vcs = wrkslots_cli._GitVcs()
    facts = wrkslots_cli._salvage_candidate(config, record, checkout, vcs)
    archive_root = tmp_path / "archive"
    archive_root.mkdir()
    first = wrkslots_cli._archive_salvage_locally(
        config, record.machine, record.slot, record.generation,
        checkout, path, facts, archive_root, None, vcs,
    )
    receipt_path = Path(str(first["archive_receipt"]))
    payload = json.loads(receipt_path.read_text(encoding="utf-8"))
    payload["status_sha256"] = "0" * 64
    receipt_path.write_text(json.dumps(payload), encoding="utf-8")
    second = wrkslots_cli._archive_salvage_locally(
        config, record.machine, record.slot, record.generation,
        checkout, path, facts, archive_root, None, vcs,
    )
    assert second["archive_receipt"] == str(receipt_path)
    fresh = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert fresh["status_sha256"] == facts.status_digest
    second_aside = second["superseded_archive"]
    assert isinstance(second_aside, dict)
    assert fresh["superseded_archive"]["directory"] == second_aside["directory"]
    aside = Path(str(second_aside["directory"]))
    old = json.loads((aside / "receipt.json").read_text(encoding="utf-8"))
    assert old["status_sha256"] == "0" * 64
    assert (aside / "repository.bundle").is_file() or list(aside.glob("*.bundle"))
    note = json.loads((aside / "superseded.json").read_text(encoding="utf-8"))
    assert "interrupted" in note["reason"]
