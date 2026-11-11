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
import time
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


@pytest.mark.load_quarantine  # #1267 wrkslots-load-robust
def test_remove_reclaims_state_home_and_bundles_unpushed_git(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #1195: the reclaim, not host speed, is under test. A state-home census
    # slowed past the production item budget (the gate host's two ~48 s
    # censuses against 60 s, scaled down: 1.5 s against 1 s) still lets the
    # reclaim succeed under the test budget of _python_remove.
    monkeypatch.setattr(wrkslots_cli, "_AGENT_REMOVE_BATCH_ITEM_SECONDS", 1.0)
    _slow_census(monkeypatch, 1.5)
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


def _slow_census(monkeypatch: pytest.MonkeyPatch, seconds: float) -> None:
    """Make the state home's /proc census take ``seconds`` longer (#1188)."""

    census = wrkslots_cli._stray_live_uses

    def slow(path: Path) -> list[str]:
        time.sleep(seconds)
        return census(path)

    monkeypatch.setattr(wrkslots_cli, "_stray_live_uses", slow)


def _home_user(home: Path) -> subprocess.Popen[bytes]:
    # Outlives any test run: a `sleep 60` user exited before a census slowed
    # by gate load reached it, so the home looked unused (#1188).
    return subprocess.Popen(["sleep", "3600"], cwd=home)


@pytest.mark.load_quarantine  # #1267 wrkslots-load-robust
def test_remove_refuses_when_a_process_uses_the_state_home(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #1188: the refusal, not host speed, is under test. A census slower
    # than the item budget (the gate host's ~48 s census against 60 s,
    # scaled down) still reaches the state-home refusal without a budget.
    root = _fixture(tmp_path / "case")
    home = _home(root)
    monkeypatch.setattr(wrkslots_cli, "_AGENT_REMOVE_BATCH_ITEM_SECONDS", 1.0)
    _slow_census(monkeypatch, 1.5)
    proc = _home_user(home)
    try:
        with pytest.raises(wrkslots_cli.Refusal, match="state home .* in use"):
            _python_remove(root, no_lock_budget=True)
    finally:
        proc.kill()
        proc.wait()
    assert home.exists()


def test_remove_refuses_when_a_slow_census_spends_the_lock_budget(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    # #1188: the budget still applies when set. The state-home census,
    # slowed past the item budget, spends it, and default remove refuses
    # with the budget message, keeping the slot and its state home -- the
    # failure the gate host showed.
    if wrkslots_cli._lsof_executable() is None:
        pytest.skip("the item budget applies only where lsof is available")
    root = _fixture(tmp_path / "case")
    home = _home(root)
    monkeypatch.setattr(wrkslots_cli, "_AGENT_REMOVE_BATCH_ITEM_SECONDS", 1.0)
    _slow_census(monkeypatch, 1.5)
    with pytest.raises(wrkslots_cli.Refusal) as refused:
        _python_remove(root, budget_seconds=None)
    message = str(refused.value)
    assert "used its 1-second budget for work before deletion" in message
    assert "'wrkslots remove --no-lock-budget'" in message
    assert home.exists()
    assert (root / "worktrees" / "slot-a" / "repo").is_dir()


def _orphan_args(root: Path, *, apply: bool) -> argparse.Namespace:
    return argparse.Namespace(
        project_root=str(root), machine=None, apply=apply, format="human",
    )


def _make_orphan(root: Path) -> Path:
    """Remove slot-a through the real path, then re-plant its state home."""
    assert _python_remove(root) == 0
    return _home(root)


@pytest.mark.load_quarantine  # #1267 wrkslots-load-robust
def test_reclaim_orphan_state_plan_then_apply(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    home = _make_orphan(root)
    rc = wrkslots_cli._cmd_reclaim_orphan_state(_orphan_args(root, apply=False))
    assert rc == 0
    assert home.exists(), "plan must not delete"
    rc = wrkslots_cli._cmd_reclaim_orphan_state(_orphan_args(root, apply=True))
    assert rc == 0
    assert not home.exists()


@pytest.mark.load_quarantine  # #1267 wrkslots-load-robust
def test_reclaim_orphan_state_apply_refuses_under_freeze(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    home = _make_orphan(root)
    config = wrkslots_cli._load_config(str(root), "node-a")
    (config.control / "RETIRE_FREEZE").write_text("migration\n")
    with pytest.raises(wrkslots_cli.Refusal, match="retire-freeze"):
        wrkslots_cli._cmd_reclaim_orphan_state(_orphan_args(root, apply=True))
    assert home.exists()
