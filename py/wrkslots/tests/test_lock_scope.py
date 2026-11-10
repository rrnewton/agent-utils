"""Configuration lock scope (#1194 wrkslots-lock-scope).

Slow salvage facts and the process census are gathered before the
configuration lock; under the lock only a cheap fingerprint is compared.
read-handoff reads under a shared lock after a lock-free gather.
"""

from __future__ import annotations

import subprocess
import threading
import time
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli


def _git(repo: Path, *args: str) -> str:
    return subprocess.run(
        ["git", "-C", str(repo), *args],
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()


def _repo(tmp_path: Path) -> Path:
    repo = tmp_path / "repo"
    repo.mkdir()
    _git(repo, "init", "-q")
    _git(repo, "config", "user.email", "test@example.com")
    _git(repo, "config", "user.name", "Test")
    (repo / "file.txt").write_text("one\n")
    _git(repo, "add", "file.txt")
    _git(repo, "commit", "-qm", "first")
    return repo


def _config_and_record(tmp_path: Path, repo: Path) -> tuple[object, object]:
    control = tmp_path / "worktrees"
    control.mkdir(exist_ok=True)
    config_path = tmp_path / ".wrkslots.yml"
    config_path.write_text("machine: node-a\n")
    (control / "ACTIVE.node-a.json").write_text("{}\n")
    config = wrkslots_cli.Config(
        root=tmp_path,
        config_path=config_path,
        worktrees=control,
        control=control,
        machine="node-a",
        default_remote="origin",
        default_landed_ref="refs/remotes/origin/main",
        heartbeat_ttl_seconds=60,
        liveness_command=tmp_path / "liveness",
    )
    head = _git(repo, "rev-parse", "HEAD")
    checkout = wrkslots_cli.Checkout(
        name="repo",
        path="repo",
        repository="repo",
        branch=_git(repo, "branch", "--show-current") or "master",
        start_point=head,
        remote="origin",
        remote_url_sha256="0" * 64,
        landed_ref="refs/remotes/origin/main",
        head=head,
    )
    record = wrkslots_cli.ActiveRecord(
        slot="slot-a",
        agent="agent-a",
        task="task",
        purpose="test",
        slot_type="agent",
        machine="node-a",
        generation=1,
        created_at="2026-01-01T00:00:00Z",
        heartbeat_at="2026-01-01T00:00:00Z",
        heartbeat_ttl_seconds=60,
        owner=None,
        coordinator_lease=wrkslots_cli.ProcessIdentity(
            pid=1, start_ticks=1, boot_id="b", host_id="h", cgroup_path="/"
        ),
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=(checkout,),
    )
    return config, record


def _probe_exclusive(root: Path) -> bool:
    """True when an exclusive flock on the project root succeeds at once."""
    import fcntl
    import os

    fd = os.open(root, os.O_RDONLY | os.O_DIRECTORY)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return False
        fcntl.flock(fd, fcntl.LOCK_UN)
        return True
    finally:
        import os as _os

        _os.close(fd)


def test_slow_gather_does_not_hold_the_configuration_lock(tmp_path: Path) -> None:
    config_path = tmp_path / ".wrkslots.yml"
    config_path.write_text("machine: node-a\n")
    observed: dict[str, object] = {}

    def gather_then_lock() -> None:
        time.sleep(1.5)  # fake slow salvage/census gather, before the lock
        observed["probe_during_gather"] = _probe_exclusive(tmp_path)
        with wrkslots_cli._locked_config(config_path, 5.0):
            observed["probe_under_lock"] = _probe_exclusive(tmp_path)
            time.sleep(0.2)

    started = time.monotonic()
    gather_then_lock()
    observed["elapsed"] = time.monotonic() - started
    assert observed["probe_during_gather"] is True
    assert observed["probe_under_lock"] is False


def test_shared_config_lock_admits_a_second_reader(tmp_path: Path) -> None:
    config_path = tmp_path / ".wrkslots.yml"
    config_path.write_text("machine: node-a\n")
    entered = threading.Event()

    def hold_shared() -> None:
        with wrkslots_cli._locked_config(config_path, 5.0, exclusive=False):
            entered.set()
            time.sleep(1.0)

    thread = threading.Thread(target=hold_shared)
    thread.start()
    assert entered.wait(5.0)
    started = time.monotonic()
    with wrkslots_cli._locked_config(config_path, 5.0, exclusive=False):
        pass
    assert time.monotonic() - started < 0.5
    thread.join(5.0)


def test_fingerprint_revalidation_refuses_when_head_changes(tmp_path: Path) -> None:
    repo = _repo(tmp_path)
    config, record = _config_and_record(tmp_path, repo)
    vcs = wrkslots_cli._GitVcs()
    before = wrkslots_cli._lock_scope_fingerprint(config, record, vcs)  # type: ignore[arg-type]
    wrkslots_cli._assert_lock_scope_fingerprint(before, before, "slot-a")
    (repo / "file.txt").write_text("two\n")
    _git(repo, "add", "file.txt")
    _git(repo, "commit", "-qm", "second")
    after = wrkslots_cli._lock_scope_fingerprint(config, record, vcs)  # type: ignore[arg-type]
    with pytest.raises(wrkslots_cli.Refusal, match="HEAD changed"):
        wrkslots_cli._assert_lock_scope_fingerprint(before, after, "slot-a")


def test_lock_free_gather_runs_while_config_lock_is_held(tmp_path: Path) -> None:
    repo = _repo(tmp_path)
    config, record = _config_and_record(tmp_path, repo)
    config_path = config.config_path  # type: ignore[attr-defined]
    with wrkslots_cli._locked_config(config_path, 5.0):
        started = time.monotonic()
        fingerprint = wrkslots_cli._lock_scope_fingerprint(
            config, record, wrkslots_cli._GitVcs()  # type: ignore[arg-type]
        )
        assert time.monotonic() - started < 2.0
        assert fingerprint.heads[0][1] == _git(repo, "rev-parse", "HEAD")
        with pytest.raises(wrkslots_cli._LockBusy):
            with wrkslots_cli._locked_config(config_path, 0.2, exclusive=False):
                pass
