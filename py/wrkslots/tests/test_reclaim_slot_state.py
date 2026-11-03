"""Slot-state home reclamation (#796).

Removing a slot also reclaims `slot-state/<slot_type>/<slot>` under the
control directory, after the same liveness and unpushed-git safety checks
as the slot itself; `reclaim-orphan-state` plans (and with --apply,
performs) the same reclaim for homes whose slot is already archived.
"""

from __future__ import annotations

import argparse
import os
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.test_remove_write_rust import _fixture, _git, _python_remove


def _home(root: Path, slot: str = "slot-a") -> Path:
    home = root / "worktrees" / "slot-state" / "agent" / slot
    home.mkdir(parents=True, exist_ok=True)
    (home / "notes.txt").write_text("private state\n")
    return home


def _unpushed_repo(home: Path) -> str:
    repo = home / "scratch"
    repo.mkdir(parents=True)
    _git(repo, "init", "-q", "-b", "main")
    (repo / "draft.txt").write_text("unpushed\n")
    _git(repo, "add", "draft.txt")
    _git(repo, "commit", "-q", "-m", "draft")
    return _git(repo, "rev-parse", "HEAD")


def test_remove_reclaims_state_home_and_bundles_unpushed_git(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    home = _home(root)
    head = _unpushed_repo(home)
    assert _python_remove(root) == 0
    assert not home.exists()
    bundles = list((root / "worktrees" / "wrkslots-salvage").rglob("*.bundle"))
    assert bundles, "unpushed state-home git was not salvaged as a bundle"
    listing = subprocess.run(
        ["git", "bundle", "list-heads", str(bundles[0])],
        capture_output=True, text=True, check=True, timeout=60,
    ).stdout
    assert head in listing


def test_remove_refuses_when_a_process_uses_the_state_home(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    home = _home(root)
    proc = subprocess.Popen(["sleep", "60"], cwd=home)
    try:
        with pytest.raises(wrkslots_cli.Refusal, match="state home"):
            _python_remove(root)
    finally:
        proc.kill()
        proc.wait()
    assert home.exists()


def _orphan_args(root: Path, *, apply: bool) -> argparse.Namespace:
    return argparse.Namespace(
        project_root=str(root), machine=None, apply=apply, format="human",
    )


def _make_orphan(root: Path) -> Path:
    """Remove slot-a through the real path, then re-plant its state home."""
    assert _python_remove(root) == 0
    return _home(root)


def test_reclaim_orphan_state_plan_then_apply(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    home = _make_orphan(root)
    rc = wrkslots_cli._cmd_reclaim_orphan_state(_orphan_args(root, apply=False))
    assert rc == 0
    assert home.exists(), "plan must not delete"
    rc = wrkslots_cli._cmd_reclaim_orphan_state(_orphan_args(root, apply=True))
    assert rc == 0
    assert not home.exists()


def test_reclaim_orphan_state_apply_refuses_under_freeze(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    home = _make_orphan(root)
    config = wrkslots_cli._load_config(str(root), "node-a")
    (config.control / "RETIRE_FREEZE").write_text("migration\n")
    with pytest.raises(wrkslots_cli.Refusal, match="retire-freeze"):
        wrkslots_cli._cmd_reclaim_orphan_state(_orphan_args(root, apply=True))
    assert home.exists()
