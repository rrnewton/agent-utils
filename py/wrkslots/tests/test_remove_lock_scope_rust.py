"""wrkslotsd remove gathers before the registry locks (#1287 retire-lock-narrow).

The Rust edition's `remove` held its mutation locks -- including the
exclusive flock on the project root -- across the fleet validation,
the state-home reclaim preparation, the salvage gates and the
process-use census. It now validates and plans with no lock held (see
`prepare_remove` in `rs/wrkslotsd/src/remove_write.rs`) and takes the
locks only to re-check the slot's record and commit it, mirroring the
Python edition. This test holds the project-root flock itself and
proves the gather still ran: the state-home salvage bundle, written by
the lock-free preparation, exists after the removal was refused as
busy at the lock. Before the narrowing, the preparation ran after the
lock acquisition, so a busy lock left no bundle.
"""

from __future__ import annotations

import fcntl
import os
import subprocess
from pathlib import Path

import pytest

from wrkslots.tests.test_remove_write_rust import _fixture, _git, _own_fs


def test_remove_gathers_before_the_project_root_flock(tmp_path: Path) -> None:
    with _own_fs(tmp_path) as base:
        root = _fixture(base)
        # A slot-state home holding a Git repository no remote ref
        # contains: the lock-free preparation must bundle it.
        home = root / "worktrees" / "slot-state" / "agent" / "slot-a" / "notes"
        home.mkdir(parents=True)
        _git(home, "init", "-q", "-b", "main")
        (home / "notes.txt").write_text("keep me\n")
        _git(home, "add", "notes.txt")
        _git(home, "commit", "-q", "-m", "unpushed notes")

        fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
            env = dict(os.environ)
            env["WRKSLOTS_WAIT_LOCK"] = "1"
            result = _run_remove(root, env)
        finally:
            fcntl.flock(fd, fcntl.LOCK_UN)
            os.close(fd)

        # Refused at the lock, as any mutation must be while it is held...
        assert result.returncode != 0
        assert "configuration lock is busy" in result.stderr + result.stdout
        # ...but only after the lock-free preparation bundled the
        # state-home repository it would otherwise have lost.
        bundles = list(root.rglob("*.bundle"))
        assert bundles, "lock-free preparation did not run before the lock"


def _write_live_finish_journal(root: Path, pid: int) -> Path:
    import json

    from wrkslots.tests.test_transfer_owner_write_rust import _live_identity

    journal = root / "worktrees" / "ACTIVE.node-a.journal"
    journal.write_text(
        json.dumps(
            {
                "schema": 2,
                "kind": "finish",
                "machine": "node-a",
                "slot": "slot-a",
                "finished_at": "2026-10-26T02:00:00Z",
                "owner": _live_identity(pid),
            }
        )
        + "\n"
    )
    return journal


def test_remove_waits_for_a_live_finish_journal_then_proceeds(tmp_path: Path) -> None:
    """#1755 (Rust): remove waits out a live finish journal."""

    import threading

    with _own_fs(tmp_path) as base:
        root = _fixture(base)
        sleeper = subprocess.Popen(["sleep", "120"])
        try:
            journal = _write_live_finish_journal(root, sleeper.pid)
            env = dict(os.environ)
            env["WRKSLOTS_WAIT_LOCK"] = "30"
            outcome: dict[str, subprocess.CompletedProcess[str]] = {}

            def run() -> None:
                outcome["result"] = _run_remove(root, env)

            worker = threading.Thread(target=run, daemon=True)
            worker.start()
            worker.join(timeout=3)
            assert worker.is_alive(), "remove refused instead of waiting"
            assert journal.exists()
            journal.unlink()
            worker.join(timeout=900)
            assert not worker.is_alive()
            assert outcome["result"].returncode == 0, outcome["result"].stderr
        finally:
            sleeper.kill()
            sleeper.wait()


def test_remove_with_a_tiny_wait_refuses_and_names_the_holder(tmp_path: Path) -> None:
    """#1755 (Rust): an expired bound refuses busy, naming the pid."""

    with _own_fs(tmp_path) as base:
        root = _fixture(base)
        sleeper = subprocess.Popen(["sleep", "120"])
        try:
            _write_live_finish_journal(root, sleeper.pid)
            env = dict(os.environ)
            env["WRKSLOTS_WAIT_LOCK"] = "0.2"
            result = _run_remove(root, env)
            assert result.returncode != 0
            assert "busy: live 'finish' mutation" in result.stderr
            assert f"by pid {sleeper.pid}" in result.stderr
        finally:
            sleeper.kill()
            sleeper.wait()


def _run_remove(root: Path, env: dict[str, str]) -> subprocess.CompletedProcess[str]:
    from wrkslots.tests.rust_binary import rust_wrkslotsd

    full_env = dict(env)
    full_env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    return subprocess.run(
        [
            str(rust_wrkslotsd()),
            "remove",
            "slot-a",
            "--coordinator-pid",
            str(os.getpid()),
            "--expected-generation",
            "1",
            "--root",
            str(root),
        ],
        capture_output=True,
        text=True,
        check=False,
        timeout=900,
        env=full_env,
    )
