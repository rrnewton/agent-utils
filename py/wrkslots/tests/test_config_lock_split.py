"""Stage D: the dedicated config.lock file, dual-locked with the legacy lock (#1808-D).

Mutations flock <control>/config.lock IN ADDITION TO the legacy project-root
directory flock, in that fixed order, so an upgraded and a legacy process
stay mutually exclusive through the legacy lock while new processes also
exclude on the new file. These tests probe both objects with independent
descriptors and wait on events, never on elapsed-time verdicts.
"""

from __future__ import annotations

import fcntl
import os
import threading
import time
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import create, make_project


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


def _held(path: Path) -> bool:
    fd = os.open(path, os.O_RDONLY | os.O_CLOEXEC)
    try:
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return True
        fcntl.flock(fd, fcntl.LOCK_UN)
        return False
    finally:
        os.close(fd)


def test_mutation_dual_locks_legacy_directory_and_config_lock_file(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    config = cli._load_config(str(project), "testhost")
    lock_file = config.control / "config.lock"

    with cli._mutation_locks(config, 5.0):
        assert lock_file.is_file()
        assert _held(lock_file), "config.lock must be flocked during a mutation"
        assert _held(project), "the legacy directory flock must also be held"
    assert not _held(lock_file)
    assert not _held(project)
    assert oct(lock_file.stat().st_mode & 0o777) == "0o600"


def test_a_legacy_only_holder_still_blocks_an_upgraded_mutation(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    config = cli._load_config(str(project), "testhost")
    fd = os.open(project, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    fcntl.flock(fd, fcntl.LOCK_EX)
    try:
        with pytest.raises(cli._LockBusy):
            with cli._mutation_locks(config, 0.0):
                pass
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)


def test_a_config_lock_only_holder_blocks_an_upgraded_mutation(
    tmp_path: Path,
) -> None:
    """An upgraded coordinator holding only config.lock excludes mutations."""
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    config = cli._load_config(str(project), "testhost")
    lock_file = config.control / "config.lock"
    lock_file.touch(mode=0o600, exist_ok=True)
    fd = os.open(lock_file, os.O_RDWR | os.O_CLOEXEC)
    fcntl.flock(fd, fcntl.LOCK_EX)
    try:
        with pytest.raises(cli._LockBusy):
            with cli._mutation_locks(config, 0.0):
                pass
    finally:
        fcntl.flock(fd, fcntl.LOCK_UN)
        os.close(fd)


def test_no_ordering_cycle_between_the_two_locks(tmp_path: Path) -> None:
    """One process holds config.lock while another takes legacy-then-file:
    the second waits (no cycle, no refusal with a wait), then proceeds."""
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    config = cli._load_config(str(project), "testhost")
    lock_file = config.control / "config.lock"
    lock_file.touch(mode=0o600, exist_ok=True)
    holder_fd = os.open(lock_file, os.O_RDWR | os.O_CLOEXEC)
    fcntl.flock(holder_fd, fcntl.LOCK_EX)
    acquired = threading.Event()

    def contender() -> None:
        with cli._mutation_locks(config, 30.0):
            acquired.set()

    thread = threading.Thread(target=contender, daemon=True)
    thread.start()
    # Condition: the contender holds the legacy flock while it waits
    # for the file (bounded probes, never a timed verdict).
    for _attempt in range(1000):
        if _held(project):
            break
        time.sleep(0.005)
    else:
        raise AssertionError("contender never took the legacy flock")
    fcntl.flock(holder_fd, fcntl.LOCK_UN)
    os.close(holder_fd)
    assert acquired.wait(30)
    thread.join(30)
