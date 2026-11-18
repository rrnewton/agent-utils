"""Lock waits block with a deadline instead of poll-refusing (#1808-C).

A contender used to poll ``LOCK_NB`` every 50 ms until ``--wait-lock``
expired, so under churn a waiter could starve behind later arrivals and
every default (0) caller refused instantly. Waits are now a blocking
``flock`` on a duplicated descriptor bounded by the caller's deadline: the
kernel queues the waiter, and a waiter that acquires after its deadline
releases at once and never acts. These tests count flock attempts and wait
on events, never on elapsed-time verdicts.
"""

from __future__ import annotations

import argparse
import contextlib
import fcntl
import os
import threading
from collections.abc import Iterator
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_lifecycle import create, make_project


@pytest.fixture(autouse=True)
def fixture_machine(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("WRKSLOTS_MACHINE", "testhost")


@contextlib.contextmanager
def _held_config_lock(project: Path) -> Iterator[threading.Event]:
    """Hold the project-root flock from another thread until released."""

    entered = threading.Event()
    release = threading.Event()

    def hold() -> None:
        fd = os.open(project, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX)
            entered.set()
            release.wait(30)
            fcntl.flock(fd, fcntl.LOCK_UN)
        finally:
            os.close(fd)

    thread = threading.Thread(target=hold, daemon=True)
    thread.start()
    assert entered.wait(30)
    try:
        yield release
    finally:
        release.set()
        thread.join(30)


def test_a_waiter_blocks_instead_of_polling_and_acquires_on_release(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    real_flock = fcntl.flock
    nonblocking_attempts: list[int] = []
    third_attempt = threading.Event()

    def counting_flock(fd: int, operation: int) -> None:
        if operation & fcntl.LOCK_NB:
            nonblocking_attempts.append(operation)
            if len(nonblocking_attempts) >= 3:
                third_attempt.set()
        return real_flock(fd, operation)

    monkeypatch.setattr(fcntl, "flock", counting_flock)
    config_path = project / ".wrkslots.yml"
    with _held_config_lock(project) as release:
        acquired = threading.Event()

        def waiter() -> None:
            with cli._locked_config(config_path, 30.0):
                acquired.set()

        thread = threading.Thread(target=waiter, daemon=True)
        thread.start()
        # Poll-refusing would rack up non-blocking attempts while it waits;
        # a blocking wait makes one, then sleeps in the kernel.
        assert third_attempt.wait(5) is False or len(nonblocking_attempts) <= 2
        release.set()
        assert acquired.wait(30)
        thread.join(30)
    assert len(nonblocking_attempts) <= 2


def test_a_waiter_past_its_deadline_never_acts_and_leaves_the_lock_free(
    tmp_path: Path,
) -> None:
    project, _repository, _remote = make_project(tmp_path)
    config_path = project / ".wrkslots.yml"
    with _held_config_lock(project) as release:
        with pytest.raises(cli._LockBusy):
            with cli._locked_config(config_path, 0.2):
                pass
        # The deadline passed while the holder still holds: any late
        # acquisition by the waiter must have been released at once.
        release.set()
    # Condition, not a clock: the lock is free now and stays free.
    fd = os.open(project, os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC)
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        fcntl.flock(fd, fcntl.LOCK_UN)
    finally:
        os.close(fd)


def test_heartbeat_wait_is_bounded_well_below_the_ttl(tmp_path: Path) -> None:
    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    config = cli._load_config(str(project), "testhost")
    assert config.heartbeat_ttl_seconds == 600

    # The bound comes from the slot record's TTL (600 s in fixtures).
    defaulted = argparse.Namespace(wait_lock=30.0, wait_lock_defaulted=True)
    assert cli._heartbeat_wait_seconds(defaulted, 600.0) == pytest.approx(10.0)
    assert cli._heartbeat_wait_seconds(defaulted, 24.0) == pytest.approx(8.0)
    explicit = argparse.Namespace(wait_lock=0.0, wait_lock_defaulted=False)
    assert cli._heartbeat_wait_seconds(explicit, 600.0) == 0.0
    explicit.wait_lock = 45.0
    assert cli._heartbeat_wait_seconds(explicit, 600.0) == 45.0


def test_the_timeout_race_never_leaks_the_duplicate_descriptor(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Force the interleaving where the helper acquires just after the
    caller's wait times out (#1808 review): ownership was already decided,
    so the helper releases and nothing stays registered or held."""
    import time as time_module

    project, _repository, _remote = make_project(tmp_path)
    lock_path = tmp_path / "intent.lock"
    holder = os.open(lock_path, os.O_RDWR | os.O_CREAT, 0o600)
    fcntl.flock(holder, fcntl.LOCK_EX)
    class SlowEvent(threading.Event):
        def wait(self, timeout: float | None = None) -> bool:
            got: bool = super().wait(timeout)
            if not got and timeout is not None:
                time_module.sleep(0.5)
            return got

    class Shim:
        def __getattr__(self, name: str) -> object:
            return SlowEvent if name == "Event" else getattr(threading, name)

    monkeypatch.setattr(cli, "threading", Shim())
    threading.Timer(0.3, lambda: fcntl.flock(holder, fcntl.LOCK_UN)).start()
    fd = os.open(lock_path, os.O_RDWR)
    try:
        acquired = cli._flock_until(fd, fcntl.LOCK_EX, time_module.monotonic() + 0.2)
        assert acquired is False
        assert cli._FLOCK_DUPS.get(fd) is None
    finally:
        os.close(fd)
        os.close(holder)
    # Condition, not a clock: the lock becomes free (the helper's late
    # acquisition was released) within a bounded number of probes.
    probe = os.open(lock_path, os.O_RDWR)
    try:
        free = False
        for _attempt in range(200):
            try:
                fcntl.flock(probe, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                time_module.sleep(0.01)
                continue
            fcntl.flock(probe, fcntl.LOCK_UN)
            free = True
            break
        assert free, "the timed-out wait leaked its duplicate's lock"
    finally:
        os.close(probe)
