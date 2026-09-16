"""Ownership samples must distinguish a vanished proc inode from a live PID."""

from __future__ import annotations

import contextlib
import errno
import os
import selectors
import signal
import subprocess
import sys
from collections.abc import Iterator
from pathlib import Path
from typing import Literal, TextIO

import pytest

from wrkslots import cli


def scripted_status(
    monkeypatch: pytest.MonkeyPatch,
    pid_dir: Path,
    samples: list[str | BaseException],
) -> list[Path]:
    pending = iter(samples)
    calls: list[Path] = []

    def read_text(
        path: Path, encoding: str | None = None, errors: str | None = None,
    ) -> str:
        assert path == pid_dir / "status"
        assert encoding == "ascii" and errors is None
        calls.append(path)
        sample = next(pending)
        if isinstance(sample, BaseException):
            raise sample
        return sample

    monkeypatch.setattr(Path, "read_text", read_text)
    return calls


@contextlib.contextmanager
def waiting_child() -> Iterator[subprocess.Popen[bytes]]:
    child = subprocess.Popen(
        [sys.executable, "-B", "-c", "import sys; print('ready', flush=True); sys.stdin.read()"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
    )
    pidfd: int | None = None
    try:
        pidfd = os.pidfd_open(child.pid)
        assert child.stdout is not None
        with selectors.DefaultSelector() as selector:
            selector.register(child.stdout, selectors.EVENT_READ)
            assert selector.select(5), "owned child did not report readiness"
            assert child.stdout.readline() == b"ready\n"
        yield child
    finally:
        assert child.stdin is not None
        if not child.stdin.closed:
            child.stdin.close()
        try:
            child.wait(timeout=3)
        except subprocess.TimeoutExpired:
            if pidfd is None:
                # Acquisition failed before this direct child was reaped.
                child.kill()
            else:
                signal.pidfd_send_signal(pidfd, signal.SIGKILL)
            child.wait(timeout=5)
            raise
        finally:
            for stream in (child.stdout, child.stderr):
                if stream is not None:
                    stream.close()
            if pidfd is not None:
                os.close(pidfd)


def test_process_uids_reopens_after_real_status_read_races_reap(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    original_open = Path.open
    with waiting_child() as child:
        pid_dir = Path("/proc") / str(child.pid)
        status_path = pid_dir / "status"
        opens = 0

        def open_then_reap(
            path: Path, mode: Literal["r"] = "r", buffering: int = -1,
            encoding: str | None = None, errors: str | None = None,
            newline: str | None = None,
        ) -> TextIO:
            nonlocal opens
            if path != status_path:
                return original_open(path, mode, buffering, encoding, errors, newline)
            opens += 1
            stream = original_open(path, mode, buffering, encoding, errors, newline)
            try:
                assert opens == 1, "the fresh path should be absent after reap"
                assert child.stdin is not None
                child.stdin.close()
                assert child.wait(timeout=3) == 0
            except BaseException:
                stream.close()
                raise
            return stream

        monkeypatch.setattr(Path, "open", open_then_reap)
        assert cli._process_uids(pid_dir) is None
        assert opens == 2, "ESRCH must lead to one fresh pathname observation"


def test_process_uids_keeps_fresh_replacement_and_zombie_leader_credentials(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    pid_dir = Path("/proc/123")
    calls = scripted_status(
        monkeypatch,
        pid_dir,
        [ProcessLookupError(errno.ESRCH, "old generation exited"),
         "State:\tZ (zombie)\nThreads:\t2\nUid:\t10\t11\t12\t13\n"],
    )
    assert cli._process_uids(pid_dir) == (10, 11, 12, 13)
    assert len(calls) == 2


@pytest.mark.parametrize("after_esrch", [False, True])
@pytest.mark.parametrize("number", [errno.EACCES, errno.EPERM, errno.EIO])
def test_process_uids_refuses_unexpected_errors_without_more_resampling(
    monkeypatch: pytest.MonkeyPatch, after_esrch: bool, number: int
) -> None:
    error = OSError(number, os.strerror(number))
    samples: list[str | BaseException] = [error]
    if after_esrch:
        samples.insert(0, ProcessLookupError(errno.ESRCH, "old inode"))
    calls = scripted_status(monkeypatch, Path("/proc/123"), samples)
    with pytest.raises(cli.Refusal, match="process ownership is indeterminate") as refused:
        cli._process_uids(Path("/proc/123"))
    assert refused.value.__cause__ is error
    assert len(calls) == 1 + int(after_esrch)


def test_process_uids_refuses_a_second_esrch(monkeypatch: pytest.MonkeyPatch) -> None:
    second = ProcessLookupError(errno.ESRCH, "fresh inode also exited")
    calls = scripted_status(
        monkeypatch, Path("/proc/123"),
        [ProcessLookupError(errno.ESRCH, "old inode"), second],
    )
    with pytest.raises(cli.Refusal, match="process ownership is indeterminate") as refused:
        cli._process_uids(Path("/proc/123"))
    assert refused.value.__cause__ is second
    assert len(calls) == 2


@pytest.mark.parametrize("after_esrch", [False, True])
@pytest.mark.parametrize("status", [
    "State:\tS (sleeping)\n",
    "Uid:\t1\t2\t3\n",
    "Uid:\t1\tbroken\t3\t4\n",
    UnicodeDecodeError("ascii", b"\xff", 0, 1, "not ASCII"),
])
def test_process_uids_refuses_missing_malformed_or_undecodable_status(
    monkeypatch: pytest.MonkeyPatch, after_esrch: bool, status: str | BaseException
) -> None:
    samples = [status]
    if after_esrch:
        samples.insert(0, ProcessLookupError(errno.ESRCH, "old inode"))
    calls = scripted_status(monkeypatch, Path("/proc/123"), samples)
    with pytest.raises(cli.Refusal, match="process ownership is indeterminate"):
        cli._process_uids(Path("/proc/123"))
    assert len(calls) == 1 + int(after_esrch)


@pytest.mark.parametrize("same_uid", [False, True])
def test_fresh_status_does_not_hide_changed_process_generation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, same_uid: bool
) -> None:
    proc_root = tmp_path / "proc"
    pid_dir = proc_root / "123"
    pid_dir.mkdir(parents=True)
    uid = os.getuid()
    calls = scripted_status(
        monkeypatch, pid_dir,
        [ProcessLookupError(errno.ESRCH, "old inode"), f"Uid:\t{uid}\t{uid}\t{uid}\t{uid}\n"],
    )
    generations = iter([cli._ProcessStat(17, 0), cli._ProcessStat(18, 0)])
    monkeypatch.setattr(cli, "_read_process_stat", lambda _path: next(generations))
    monkeypatch.setattr(cli, "_mount_namespace", lambda _path: "mnt:[123]")
    monkeypatch.setattr(cli, "_read_process_cgroup", lambda _path: "/fixture")
    budget = cli._ReadOnlyCommandBudget.start(
        timeout_seconds=30, stdout_limit=1024, stderr_limit=1024,
    )
    with pytest.raises(cli._ProcessEvidenceChanged, match="process generation changed"):
        if same_uid:
            cli._same_uid_process_observations(budget, proc_root)
        else:
            cli._absent_validate_process_snapshot(proc_root)
    assert len(calls) == 2


def test_direct_census_refuses_a_live_holder_after_fresh_status(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    proc_root = tmp_path / "proc"
    pid_dir = proc_root / str(os.getpid() + 1000)
    pid_dir.mkdir(parents=True)
    uid = os.getuid()
    calls = scripted_status(
        monkeypatch, pid_dir,
        [ProcessLookupError(errno.ESRCH, "old inode"), f"Uid:\t{uid}\t{uid}\t{uid}\t{uid}\n"],
    )
    monkeypatch.setattr(cli, "_process_uses_slot", lambda _pid, _slot: [f"cwd={slot}"])
    with pytest.raises(cli.Refusal, match="live process .* uses slot"):
        cli._assert_slot_unused(slot, use_lsof=False, proc_root=proc_root)
    assert len(calls) == 2
