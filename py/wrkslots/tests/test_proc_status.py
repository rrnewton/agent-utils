"""Ownership samples must distinguish a vanished proc inode from a live PID."""

from __future__ import annotations

import contextlib
import dataclasses
import errno
import json
import re
import os
import selectors
import shutil
import signal
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import textwrap
import threading
import time
from collections.abc import Callable, Iterator, Sequence
from pathlib import Path
from typing import Literal, TextIO, TypedDict, cast

import pytest

from wrkslots import cli, sockdiag


def scripted_status(
    monkeypatch: pytest.MonkeyPatch,
    pid_dir: Path,
    samples: Sequence[str | bytes | BaseException],
) -> list[Path]:
    pending = iter(samples)
    calls: list[Path] = []
    original_read = Path.read_bytes

    def read_bytes(path: Path) -> bytes:
        if path == pid_dir / "stat":
            # The /proc walks read stat to find zombie thread-group leaders;
            # only status is scripted here.
            return original_read(path)
        assert path == pid_dir / "status"
        calls.append(path)
        sample = next(pending)
        if isinstance(sample, BaseException):
            raise sample
        return sample if isinstance(sample, bytes) else sample.encode("ascii")

    monkeypatch.setattr(Path, "read_bytes", read_bytes)
    return calls


def opaque_stat(
    pid: int, parent: int, *, flags: int = 0, start_ticks: int = 17
) -> bytes:
    """One kernel-shaped stat record whose comm is deliberately not text."""
    fields = [b"S", str(parent).encode(), str(pid).encode(), str(pid).encode()]
    fields.extend([b"0", b"0", str(flags).encode()])
    fields.extend([b"0"] * 12)
    fields.append(str(start_ticks).encode())
    return str(pid).encode() + b" (p\xff) Z\n(\x80) " + b" ".join(fields) + b"\n"


def test_process_stat_and_ancestry_treat_comm_as_opaque_bytes(tmp_path: Path) -> None:
    proc_root = tmp_path / "proc"
    pid_dir = proc_root / "123"
    pid_dir.mkdir(parents=True)
    (pid_dir / "stat").write_bytes(opaque_stat(123, 42, flags=9, start_ticks=99))

    assert cli._read_process_stat(pid_dir) == cli._ProcessStat(99, 9)
    assert cli._read_process_parent(123, proc_root=proc_root) == 42


def test_global_process_snapshot_accepts_opaque_comm_bytes(tmp_path: Path) -> None:
    """One valid opaque process name cannot make the all-process census unavailable."""
    proc_root = tmp_path / "proc"
    pid_dir = proc_root / "123"
    (pid_dir / "ns").mkdir(parents=True)
    (pid_dir / "stat").write_bytes(opaque_stat(123, 42, flags=0, start_ticks=99))
    uid = os.getuid()
    (pid_dir / "status").write_bytes(
        b"Name:\tp\xff\rUid:\t0\vState:\tZ\\n(\x80\nState:\tS (sleeping)\nUid:\t"
        + f"{uid}\t{uid}\t{uid}\t{uid}\n".encode()
    )
    (pid_dir / "cgroup").write_text("0::/fixture\n", encoding="ascii")
    (pid_dir / "ns" / "mnt").symlink_to("mnt:[123]")

    assert cli._absent_validate_process_snapshot(proc_root) == (
        cli._AbsentProcessObservation(123, 99, "/fixture", "mnt:[123]"),
    )


@contextlib.contextmanager
def waiting_child(cwd: Path | None = None) -> Iterator[subprocess.Popen[bytes]]:
    child = subprocess.Popen(
        [sys.executable, "-B", "-c", "import sys; print('ready', flush=True); sys.stdin.read()"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        cwd=cwd,
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
    b"Name:\topaque\xff\n",
])
def test_process_uids_refuses_missing_malformed_or_incomplete_status(
    monkeypatch: pytest.MonkeyPatch, after_esrch: bool,
    status: str | bytes | BaseException,
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
    monkeypatch.setattr(
        cli, "_process_uses_slot", lambda _pid, _slot, **_kw: [f"cwd={slot}"]
    )
    with pytest.raises(cli.Refusal, match="live process .* uses slot"):
        cli._assert_slot_unused(slot, use_lsof=False, proc_root=proc_root)
    assert len(calls) == 2


def test_lsof_guard_refuses_a_real_live_user_with_attributed_evidence(
    tmp_path: Path,
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    empty_proc = tmp_path / "empty-proc"
    empty_proc.mkdir()
    assert Path("/usr/bin/lsof").is_file(), "the slot guard requires the installed lsof"

    with waiting_child(cwd=slot) as child:
        with pytest.raises(cli.Refusal) as refused:
            cli._assert_slot_unused(slot, proc_root=empty_proc)

    rendered = str(refused.value)
    assert f"live process {child.pid} uses slot {slot}" in rendered
    assert "command=" in rendered
    assert 'fd="cwd"' in rendered
    assert f'file="{slot}"' in rendered
    assert len(rendered) < 1024


def scripted_lsof(
    monkeypatch: pytest.MonkeyPatch,
    slot: Path,
    responses: list[subprocess.CompletedProcess[str]],
    *,
    events: list[str] | None = None,
) -> list[list[str]]:
    pending = iter(responses)
    calls: list[list[str]] = []

    def run(
        args: list[str], *, text: bool, capture_output: bool, check: bool
    ) -> subprocess.CompletedProcess[str]:
        assert args == ["/usr/bin/lsof", "-nP", "-Fpcfn", "+D", str(slot)]
        assert text is True and capture_output is True and check is False
        calls.append(args)
        if events is not None:
            events.append("lsof")
        return next(pending)

    monkeypatch.setattr(subprocess, "run", run)
    return calls


def lsof_holder(
    pid: int, slot: Path, *, returncode: int = 0
) -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(
        ["lsof"],
        returncode,
        f"p{pid}\ncpython\nfcwd\nn{slot}\n",
        "",
    )


def lsof_unused() -> subprocess.CompletedProcess[str]:
    return subprocess.CompletedProcess(["lsof"], 1, "", "")


def test_reclaim_restarts_lsof_and_proc_after_exact_lsof_generation_exits(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    empty_proc = tmp_path / "empty-proc"
    empty_proc.mkdir()
    with waiting_child(cwd=slot) as child:
        events: list[str] = []
        calls = scripted_lsof(
            monkeypatch,
            slot,
            [
                lsof_holder(child.pid, slot),
                lsof_holder(child.pid, slot),
                lsof_unused(),
            ],
            events=events,
        )
        real_read_process_stat = cli._read_process_stat
        real_pidfd_open = os.pidfd_open
        generation_reads = 0

        def release_after_pidfd_generation_check(
            path: Path,
        ) -> cli._ProcessStat | None:
            nonlocal generation_reads
            observed = real_read_process_stat(path)
            if path == Path("/proc") / str(child.pid):
                events.append("stat")
                generation_reads += 1
                if generation_reads == 2:
                    assert child.stdin is not None
                    child.stdin.close()
            return observed

        monkeypatch.setattr(
            cli, "_read_process_stat", release_after_pidfd_generation_check
        )

        def record_pidfd_open(pid: int) -> int:
            assert pid == child.pid
            events.append("pidfd")
            return real_pidfd_open(pid)

        monkeypatch.setattr(os, "pidfd_open", record_pidfd_open)
        budget = cli._LiveUseRecheckBudget()

        cli._assert_slot_unused(
            slot,
            proc_root=empty_proc,
            live_use_recheck=budget,
        )

        assert child.wait(timeout=1) == 0
        assert len(calls) == 3
        assert generation_reads == 2
        assert events == ["lsof", "stat", "pidfd", "lsof", "stat", "lsof"]
        assert budget.remaining_seconds < cli._RECLAIM_LIVE_USE_RECHECK_SECONDS


def test_reclaim_restarts_proc_after_exact_proc_generation_exits(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    proc_root = tmp_path / "proc"
    with waiting_child(cwd=slot) as child:
        pid_dir = proc_root / str(child.pid)
        pid_dir.mkdir(parents=True)
        uid = os.getuid()
        (pid_dir / "status").write_text(
            f"Uid:\t{uid}\t{uid}\t{uid}\t{uid}\n", encoding="ascii"
        )
        real_generation = cli._read_process_stat(Path("/proc") / str(child.pid))
        assert real_generation is not None
        generation_reads = 0
        use_reads = 0

        def stable_generation(path: Path) -> cli._ProcessStat | None:
            nonlocal generation_reads
            assert path == pid_dir
            generation_reads += 1
            if generation_reads == 2:
                assert child.stdin is not None
                child.stdin.close()
            return real_generation

        def transient_use(_pid_dir: Path, _slot: Path, **_kw: object) -> list[str]:
            nonlocal use_reads
            use_reads += 1
            return [f"cwd={slot}"] if use_reads == 1 else []

        monkeypatch.setattr(cli, "_read_process_stat", stable_generation)
        monkeypatch.setattr(cli, "_process_uses_slot", transient_use)
        budget = cli._LiveUseRecheckBudget()

        cli._assert_slot_unused(
            slot,
            use_lsof=False,
            proc_root=proc_root,
            live_use_recheck=budget,
        )

        assert child.wait(timeout=1) == 0
        assert generation_reads == 3
        assert use_reads == 2
        assert budget.remaining_seconds < cli._RECLAIM_LIVE_USE_RECHECK_SECONDS


def test_reclaim_recheck_catches_replacement_without_signaling_any_holder(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    empty_proc = tmp_path / "empty-proc"
    empty_proc.mkdir()
    with (
        waiting_child(cwd=slot) as first,
        waiting_child(cwd=slot) as replacement,
        waiting_child() as unrelated,
    ):
        calls = scripted_lsof(
            monkeypatch,
            slot,
            [
                lsof_holder(first.pid, slot),
                lsof_holder(first.pid, slot),
                lsof_holder(replacement.pid, slot),
                lsof_holder(replacement.pid, slot),
            ],
        )
        real_read_process_stat = cli._read_process_stat
        first_reads = 0

        def release_first_after_generation_check(
            path: Path,
        ) -> cli._ProcessStat | None:
            nonlocal first_reads
            observed = real_read_process_stat(path)
            if path == Path("/proc") / str(first.pid):
                first_reads += 1
                if first_reads == 2:
                    assert first.stdin is not None
                    first.stdin.close()
            return observed

        monkeypatch.setattr(
            cli, "_read_process_stat", release_first_after_generation_check
        )
        budget = cli._LiveUseRecheckBudget(0.05)

        with pytest.raises(cli.Refusal) as refused:
            cli._assert_slot_unused(
                slot,
                proc_root=empty_proc,
                live_use_recheck=budget,
            )

        assert f"live process {replacement.pid} uses slot {slot}" in str(refused.value)
        assert len(calls) == 4
        assert budget.remaining_seconds == 0
        assert first.poll() == 0
        assert replacement.poll() is None
        assert unrelated.poll() is None


def test_reclaim_recheck_persistent_holder_exhausts_one_deadline(
    tmp_path: Path,
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    with waiting_child(cwd=slot) as child:
        generation = cli._read_process_stat(Path("/proc") / str(child.pid))
        assert generation is not None
        budget = cli._LiveUseRecheckBudget(0.02)

        assert budget.wait_for_exit(
            child.pid,
            start_ticks=generation.start_ticks,
        ) is False

        assert budget.remaining_seconds == 0
        assert child.poll() is None


def test_reclaim_recheck_deadline_is_shared_across_later_checks(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    samples = iter((10.0, 10.01, 10.06))
    opened: list[int] = []

    def process_exited(pid: int) -> int:
        opened.append(pid)
        raise ProcessLookupError(errno.ESRCH, "fixture generation exited")

    monkeypatch.setattr(time, "monotonic", lambda: next(samples))
    monkeypatch.setattr(os, "pidfd_open", process_exited)
    budget = cli._LiveUseRecheckBudget(0.05)

    assert budget.wait_for_exit(111, start_ticks=17) is True
    assert budget.wait_for_exit(222, start_ticks=23) is False
    assert opened == [111]
    assert budget.remaining_seconds == 0


@pytest.mark.parametrize("failure", ["pid-reuse", "pidfd-failure", "incomplete"])
def test_reclaim_never_retries_unbound_process_generation(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    failure: str,
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    empty_proc = tmp_path / "empty-proc"
    empty_proc.mkdir()
    calls = scripted_lsof(
        monkeypatch,
        slot,
        [
            lsof_holder(123, slot),
            *(
                [lsof_holder(123, slot)]
                if failure == "pid-reuse"
                else []
            ),
        ],
    )
    opened: list[int] = []
    read_fd, write_fd = os.pipe()
    generations = iter(
        (
            (None,)
            if failure == "incomplete"
            else (cli._ProcessStat(17, 0), cli._ProcessStat(18, 0))
        )
    )

    def generation(_path: Path) -> cli._ProcessStat | None:
        return next(generations)

    def pidfd_open(pid: int) -> int:
        opened.append(pid)
        if failure == "pidfd-failure":
            raise PermissionError(errno.EPERM, "fixture pidfd denial")
        return os.dup(read_fd)

    monkeypatch.setattr(cli, "_read_process_stat", generation)
    monkeypatch.setattr(os, "pidfd_open", pidfd_open)
    try:
        with pytest.raises(cli.Refusal, match="live process 123 uses slot"):
            cli._assert_slot_unused(
                slot,
                proc_root=empty_proc,
                live_use_recheck=cli._LiveUseRecheckBudget(0.25),
            )
    finally:
        os.close(read_fd)
        os.close(write_fd)

    assert len(calls) == (2 if failure == "pid-reuse" else 1)
    assert opened == ([] if failure == "incomplete" else [123])


def test_reclaim_never_retries_lsof_pid_reused_before_generation_binding(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    empty_proc = tmp_path / "empty-proc"
    empty_proc.mkdir()
    events: list[str] = []
    calls = scripted_lsof(
        monkeypatch,
        slot,
        [lsof_holder(123, slot), lsof_unused()],
        events=events,
    )

    def replacement_generation(_path: Path) -> cli._ProcessStat:
        events.append("stat")
        return cli._ProcessStat(18, 0)

    monkeypatch.setattr(cli, "_read_process_stat", replacement_generation)
    opened: list[int] = []
    read_fd, write_fd = os.pipe()

    def pidfd_open(pid: int) -> int:
        opened.append(pid)
        events.append("pidfd")
        return os.dup(read_fd)

    monkeypatch.setattr(os, "pidfd_open", pidfd_open)
    try:
        with pytest.raises(cli.Refusal, match="live process 123 uses slot"):
            cli._assert_slot_unused(
                slot,
                proc_root=empty_proc,
                live_use_recheck=cli._LiveUseRecheckBudget(0.25),
            )
    finally:
        os.close(read_fd)
        os.close(write_fd)

    assert len(calls) == 2
    assert opened == [123]
    assert events == ["lsof", "stat", "pidfd", "lsof"]


def test_reclaim_never_retries_lsof_holder_gone_before_pidfd_binding(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    empty_proc = tmp_path / "empty-proc"
    empty_proc.mkdir()
    calls = scripted_lsof(monkeypatch, slot, [lsof_holder(123, slot)])
    monkeypatch.setattr(
        cli, "_read_process_stat", lambda _path: cli._ProcessStat(17, 0)
    )
    opened: list[int] = []

    def exited_before_pidfd(pid: int) -> int:
        opened.append(pid)
        raise ProcessLookupError(errno.ESRCH, "fixture lsof holder exited")

    monkeypatch.setattr(os, "pidfd_open", exited_before_pidfd)

    with pytest.raises(cli.Refusal, match="live process 123 uses slot"):
        cli._assert_slot_unused(
            slot,
            proc_root=empty_proc,
            live_use_recheck=cli._LiveUseRecheckBudget(0.25),
        )

    assert len(calls) == 1
    assert opened == [123]


@pytest.mark.parametrize(
    ("response", "message"),
    [
        (
            subprocess.CompletedProcess(["lsof"], 0, "", "fatal lsof error\n"),
            "lsof reported",
        ),
        (
            subprocess.CompletedProcess(["lsof"], 2, "", ""),
            "lsof exited 2",
        ),
        (
            subprocess.CompletedProcess(["lsof"], 0, "unexpected\n", ""),
            "malformed field output",
        ),
        (
            subprocess.CompletedProcess(
                ["lsof"], 2, "p123\ncgit\nfcwd\nn/slot\n", ""
            ),
            "live process 123 uses slot",
        ),
        (
            subprocess.CompletedProcess(
                ["lsof"], 0, "p123\ncgit\nxunknown\nfcwd\nn/slot\n", ""
            ),
            "live process 123 uses slot",
        ),
        (
            subprocess.CompletedProcess(["lsof"], 0, "p123\n", ""),
            "live process 123 uses slot",
        ),
        (
            subprocess.CompletedProcess(
                ["lsof"], 0, "p123\ncgit\nfcwd\n", ""
            ),
            "live process 123 uses slot",
        ),
        (
            subprocess.CompletedProcess(
                ["lsof"], 0, "p123\nfcwd\nn/slot\n", ""
            ),
            "live process 123 uses slot",
        ),
        (
            subprocess.CompletedProcess(
                ["lsof"],
                0,
                "p123\ncgit\nfcwd\nn/slot\np124\n",
                "",
            ),
            "live process 123 uses slot",
        ),
        (
            subprocess.CompletedProcess(
                ["lsof"], 0, "p123\nfcwd\ncgit\nn/slot\n", ""
            ),
            "live process 123 uses slot",
        ),
        (
            subprocess.CompletedProcess(
                ["lsof"], 0, "p123\nfcwd\nn/slot\ncgit\n", ""
            ),
            "live process 123 uses slot",
        ),
        (
            subprocess.CompletedProcess(
                ["lsof"],
                0,
                (
                    "p123\ncgit\nfcwd\nn/slot\n"
                    "p123\ncgit\nfcwd\nn/slot\n"
                ),
                "",
            ),
            "live process 123 uses slot",
        ),
    ],
)
def test_reclaim_never_retries_indeterminate_lsof_evidence(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    response: subprocess.CompletedProcess[str],
    message: str,
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    empty_proc = tmp_path / "empty-proc"
    empty_proc.mkdir()
    if "n/slot\n" in response.stdout:
        response = subprocess.CompletedProcess(
            response.args,
            response.returncode,
            response.stdout.replace("/slot", str(slot)),
            response.stderr,
        )
    calls = scripted_lsof(monkeypatch, slot, [response])
    budget = cli._LiveUseRecheckBudget(0.25)
    opened: list[int] = []

    def unexpected_pidfd_open(pid: int) -> int:
        opened.append(pid)
        raise AssertionError("indeterminate lsof evidence must not reach pidfd")

    monkeypatch.setattr(os, "pidfd_open", unexpected_pidfd_open)

    with pytest.raises(cli.Refusal, match=message):
        cli._assert_slot_unused(
            slot,
            proc_root=empty_proc,
            live_use_recheck=budget,
        )

    assert len(calls) == 1
    assert opened == []
    assert budget.remaining_seconds == 0.25


def test_reclaim_never_retries_incomplete_proc_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    proc_root = tmp_path / "proc"
    fixture_pid = os.getpid() + 1_000
    pid_dir = proc_root / str(fixture_pid)
    pid_dir.mkdir(parents=True)
    uid = os.getuid()
    (pid_dir / "status").write_text(
        f"Uid:\t{uid}\t{uid}\t{uid}\t{uid}\n", encoding="ascii"
    )
    monkeypatch.setattr(
        cli, "_read_process_stat", lambda _path: cli._ProcessStat(17, 0)
    )
    monkeypatch.setattr(
        cli,
        "_process_uses_slot",
        lambda _pid, _slot, **_kw: (_ for _ in ()).throw(
            cli.Refusal("fixture proc evidence is incomplete")
        ),
    )
    waits: list[int] = []

    def unexpected_wait(
        _budget: cli._LiveUseRecheckBudget,
        pid: int,
        *,
        start_ticks: int | None,
        proc_root: Path = Path("/proc"),
        revalidate: Callable[[], bool] | None = None,
    ) -> bool:
        del start_ticks, proc_root, revalidate
        waits.append(pid)
        return True

    monkeypatch.setattr(cli._LiveUseRecheckBudget, "wait_for_exit", unexpected_wait)

    with pytest.raises(cli.Refusal, match="fixture proc evidence is incomplete"):
        cli._assert_slot_unused(
            slot,
            use_lsof=False,
            fallback_census=cli._ProcessPathCensus((), ()),
            proc_root=proc_root,
            live_use_recheck=cli._LiveUseRecheckBudget(),
        )

    assert waits == []


def test_reclaim_keeps_first_proc_holder_refusal_when_later_evidence_is_incomplete(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    proc_root = tmp_path / "proc"
    holder_pid = os.getpid() + 1_000
    incomplete_pid = holder_pid + 1
    for pid in (holder_pid, incomplete_pid):
        (proc_root / str(pid)).mkdir(parents=True)
    monkeypatch.setattr(
        cli, "_read_process_stat", lambda _path: cli._ProcessStat(17, 0)
    )

    def process_uids(pid_dir: Path) -> tuple[int, int, int, int]:
        if int(pid_dir.name) == incomplete_pid:
            raise cli.Refusal("later fixture ownership is incomplete")
        uid = os.getuid()
        return uid, uid, uid, uid

    monkeypatch.setattr(cli, "_process_uids", process_uids)
    monkeypatch.setattr(
        cli,
        "_process_uses_slot",
        lambda pid_dir, _slot, **_kw: (
            [f"cwd={slot}"] if int(pid_dir.name) == holder_pid else []
        ),
    )
    opened: list[int] = []

    def unexpected_pidfd_open(pid: int) -> int:
        opened.append(pid)
        raise AssertionError("incomplete census must suppress the pidfd wait")

    monkeypatch.setattr(os, "pidfd_open", unexpected_pidfd_open)

    with pytest.raises(
        cli.Refusal,
        match=rf"live process {holder_pid} uses slot",
    ):
        cli._assert_slot_unused(
            slot,
            use_lsof=False,
            proc_root=proc_root,
            live_use_recheck=cli._LiveUseRecheckBudget(),
        )

    assert opened == []


def test_reclaim_never_retries_before_an_unbound_second_proc_holder(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    proc_root = tmp_path / "proc"
    first_pid = os.getpid() + 1_000
    unbound_pid = first_pid + 1
    for pid in (first_pid, unbound_pid):
        (proc_root / str(pid)).mkdir(parents=True)

    def generation(pid_dir: Path) -> cli._ProcessStat | None:
        return (
            cli._ProcessStat(17, 0)
            if int(pid_dir.name) == first_pid
            else None
        )

    uid = os.getuid()
    monkeypatch.setattr(cli, "_read_process_stat", generation)
    monkeypatch.setattr(
        cli, "_process_uids", lambda _path: (uid, uid, uid, uid)
    )
    monkeypatch.setattr(
        cli, "_process_uses_slot", lambda _pid, _slot, **_kw: [f"cwd={slot}"]
    )
    opened: list[int] = []

    def unexpected_pidfd_open(pid: int) -> int:
        opened.append(pid)
        raise AssertionError("an unbound holder must suppress the pidfd wait")

    monkeypatch.setattr(os, "pidfd_open", unexpected_pidfd_open)

    with pytest.raises(cli.Refusal, match=rf"live process {first_pid} uses slot"):
        cli._assert_slot_unused(
            slot,
            use_lsof=False,
            proc_root=proc_root,
            live_use_recheck=cli._LiveUseRecheckBudget(),
        )

    assert opened == []


def test_reclaim_fallback_exit_restarts_the_original_proc_census(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    slot = tmp_path / "slot"
    slot.mkdir()
    proc_root = tmp_path / "proc"
    fixture_pid_dir = proc_root / str(os.getpid() + 1_000)
    fixture_pid_dir.mkdir(parents=True)
    with waiting_child(cwd=slot) as child:
        lsof_calls = scripted_lsof(
            monkeypatch,
            slot,
            [lsof_holder(child.pid, slot), lsof_holder(child.pid, slot)],
        )
        real_read_process_stat = cli._read_process_stat
        generation_reads = 0
        proc_reads = 0

        def generations(path: Path) -> cli._ProcessStat | None:
            nonlocal generation_reads
            if path == fixture_pid_dir:
                return cli._ProcessStat(17, 0)
            observed = real_read_process_stat(path)
            if path == Path("/proc") / str(child.pid):
                generation_reads += 1
                if generation_reads == 2:
                    assert child.stdin is not None
                    child.stdin.close()
            return observed

        def process_uids(_pid_dir: Path) -> tuple[int, int, int, int]:
            uid = os.getuid()
            return uid, uid, uid, uid

        def indeterminate_then_clear(
            _pid_dir: Path, _slot: Path, **_kw: object
        ) -> list[str]:
            nonlocal proc_reads
            proc_reads += 1
            if proc_reads == 1:
                raise cli.Refusal("fixture direct proc evidence is incomplete")
            return []

        monkeypatch.setattr(cli, "_read_process_stat", generations)
        monkeypatch.setattr(cli, "_process_uids", process_uids)
        monkeypatch.setattr(cli, "_process_uses_slot", indeterminate_then_clear)

        cli._assert_slot_unused(
            slot,
            use_lsof=False,
            proc_root=proc_root,
            live_use_recheck=cli._LiveUseRecheckBudget(),
        )

        assert child.wait(timeout=1) == 0
        assert len(lsof_calls) == 2
        assert generation_reads == 2
        assert proc_reads == 2


def test_lsof_diagnostic_associates_process_command_and_first_file_record() -> None:
    evidence, malformed = cli._parse_lsof_use_diagnostic(
        "\n".join(
            (
                "p900",
                "cother",
                "f9",
                "n/other/file",
                "p123",
                "cgit maintenance",
                "fcwd",
                "n/selected/slot",
                "f7",
                "n/selected/slot/later",
                "p456",
                "clater",
                "fcwd",
                "n/later/slot",
            )
        )
        + "\n"
    )

    assert malformed is False
    assert evidence == cli._LsofUseDiagnostic(
        pid=123,
        command_excerpt='"git maintenance"',
        descriptor_excerpt='"cwd"',
        name_excerpt='"/selected/slot"',
    )
    assert cli._parse_lsof_use_diagnostic("") == (None, False)


def test_lsof_diagnostic_is_bounded_terminal_safe_and_malformed_safe() -> None:
    long_command = "x" * 10_000 + "\x1b[31m"
    long_descriptor = "9" * 10_000
    long_name = "/slot/" + "y" * 10_000
    evidence, malformed = cli._parse_lsof_use_diagnostic(
        f"p123\nc{long_command}\nf{long_descriptor}\nn{long_name}\n"
    )
    assert evidence is not None
    assert malformed is False
    rendered = cli._format_lsof_use_diagnostic(evidence, malformed=malformed)
    assert len(rendered) < 600
    assert rendered.count(cli._LSOF_DIAGNOSTIC_TRUNCATION) == 3
    assert "\x1b" not in rendered
    assert "\n" not in rendered

    forged, forged_malformed = cli._parse_lsof_use_diagnostic(
        f"p123\nc{long_command}\nf{long_descriptor}\nn{long_name}\nforged refusal\n"
    )
    assert forged is not None
    assert forged_malformed is True
    forged_rendered = cli._format_lsof_use_diagnostic(
        forged, malformed=forged_malformed
    )
    assert "malformed lsof fields omitted" in forged_rendered
    assert len(forged_rendered) < 600
    assert "\x1b" not in forged_rendered
    assert "\n" not in forged_rendered

    for broken in (
        "corphan\nfcwd\nn/slot\n",
        f"p{'9' * 10_000}\ncbogus\nfcwd\nn/slot\n",
        "p123\ncright\nn/orphan\nf1\nn/slot/file\n",
        "p123\ncright\nxunknown\nn/slot/file\n",
    ):
        parsed, parse_malformed = cli._parse_lsof_use_diagnostic(broken)
        assert parse_malformed is True
        if parsed is not None:
            diagnostic = cli._format_lsof_use_diagnostic(
                parsed, malformed=parse_malformed
            )
            assert "malformed lsof fields omitted" in diagnostic
            assert len(diagnostic) < 600


def test_the_machines_init_is_refused_as_a_process_identity() -> None:
    """PID 1 must never be recorded as an owner, coordinator or actor.

    ⚠️ THE READ-TIME CHECKS CANNOT CATCH THIS, which is why the guard is at
    write time. Liveness compares the recorded generation against the live one,
    and for the machine's init that comparison answers "alive" until reboot --
    so a slot registered this way is unreclaimable and nothing downstream can
    tell it from a genuine owner.

    This runs against the real /proc rather than a fixture, because the
    property being tested is about this machine's actual init.
    """
    if Path("/proc/1/cgroup").read_text(encoding="ascii").strip() != "0::/init.scope":
        pytest.skip("this machine's init is not in /init.scope")
    with pytest.raises(cli.Refusal) as refused:
        cli._read_process_identity(1)
    rendered = str(refused.value)
    assert "the machine's init process" in rendered
    assert "is not live" not in rendered


def test_a_namespaced_pid_1_is_still_recorded(monkeypatch: pytest.MonkeyPatch) -> None:
    """The guard asks what the process IS, not what it is numbered.

    ⚠️ THIS IS THE CONTROL, AND IT IS NOT HYPOTHETICAL. A first version of this
    guard refused PID 1 by number and broke the process-and-git stress suite,
    which runs its whole harness under a PID namespace where the runner
    legitimately is PID 1. Inside such a namespace PID 1 reports an ordinary
    session scope, and it must still be recordable.
    """
    real = cli._read_process_cgroup

    def session_scope(pid_dir: Path) -> str:
        if pid_dir.name == "1":
            return "/user.slice/user-1000.slice/session-3.scope"
        return real(pid_dir)

    monkeypatch.setattr(cli, "_read_process_cgroup", session_scope)
    identity = cli._read_process_identity(1)
    assert identity.pid == 1
    assert identity.cgroup_path == "/user.slice/user-1000.slice/session-3.scope"


def test_the_kernel_thread_daemon_is_refused_as_a_process_identity() -> None:
    if Path("/proc/2/cgroup").read_text(encoding="ascii").strip() != "0::/":
        pytest.skip("this machine's PID 2 is not the kernel thread daemon")
    with pytest.raises(cli.Refusal) as refused:
        cli._read_process_identity(2)
    assert "the kernel thread daemon" in str(refused.value)


def test_an_ordinary_live_pid_is_still_recorded() -> None:
    """The guard must refuse two specific processes, not narrow registration."""
    identity = cli._read_process_identity(os.getpid())
    assert identity.pid == os.getpid()
    assert identity.start_ticks > 0


def test_a_live_owner_cgroup_is_still_evidence() -> None:
    """The narrowing must be conditional, not a deletion.

    ⚠️ THIS IS THE CONTROL FOR THE CGROUP NARROWING. The change it guards makes
    a shared owner cgroup stop blocking reclaim once the recorded owner
    generation is proven dead. If the condition were dropped rather than
    narrowed, this test is what notices: a live recorded owner must still make
    its cgroup count as evidence.
    """
    record = cli.ActiveRecord.__new__(cli.ActiveRecord)
    object.__setattr__(record, "owner", cli._read_process_identity(os.getpid()))
    assert cli._owner_cgroup_is_evidence(record) is True


def test_a_dead_owner_cgroup_is_not_evidence() -> None:
    live = cli._read_process_identity(os.getpid())
    record = cli.ActiveRecord.__new__(cli.ActiveRecord)
    # A boot that has ended is the cheapest proof of death available here, and
    # it is the same one the lifecycle fixtures use.
    object.__setattr__(record, "owner", dataclasses.replace(live, boot_id="finished-boot"))
    assert cli._owner_cgroup_is_evidence(record) is False


def test_no_recorded_owner_is_not_evidence() -> None:
    record = cli.ActiveRecord.__new__(cli.ActiveRecord)
    object.__setattr__(record, "owner", None)
    assert cli._owner_cgroup_is_evidence(record) is False
    assert cli._owner_cgroup_is_evidence(None) is False


def _record(owner: "cli.ProcessIdentity | None") -> "cli.ActiveRecord":
    record = cli.ActiveRecord.__new__(cli.ActiveRecord)
    object.__setattr__(record, "owner", owner)
    object.__setattr__(record, "slot", "slot01")
    return record


def test_an_absent_owner_record_no_longer_preserves_a_slot() -> None:
    assert cli._owner_record_is_absent(_record(None)) is True


def test_the_machines_init_as_owner_counts_as_absent() -> None:
    """A degenerate owner is an error on its face, not a fact to preserve."""
    live = cli._read_process_identity(os.getpid())
    init = dataclasses.replace(live, pid=1, cgroup_path="/init.scope")
    assert cli._owner_record_is_absent(_record(init)) is True


def test_a_real_owner_is_not_treated_as_absent() -> None:
    """⚠️ THE CONTROL. If this ever returns True the unblocking has escaped to
    every slot, which would drop the proven-dead requirement for rows whose
    owner is perfectly well recorded."""
    live = cli._read_process_identity(os.getpid())
    assert cli._owner_record_is_absent(_record(live)) is False
    # PID 1 inside its own namespace is a real process in an ordinary scope and
    # must NOT be swept in.
    namespaced = dataclasses.replace(live, pid=1, cgroup_path="/user.slice/session-3.scope")
    assert cli._owner_record_is_absent(_record(namespaced)) is False


def test_the_audit_publishes_each_slot_once_even_from_several_routes() -> None:
    """One slot described by two routes must be published once, not twice.

    ⚠️ A DEADLOCK TEST, NOT A TIDINESS TEST. The audit's `rows` is built from
    FOUR separate append sites, so a slot present both in the registry and as an
    on-disk worktree legitimately produces two rows -- which is why
    `owner_state == "unregistered"` is itself an attention condition. The
    published list carried both, and the downstream contract then refused the
    WHOLE census with "audit.attention_slots contains duplicates". Nothing
    downstream could clear that: the duplicate is produced inside the audit
    every time two routes describe one slot, so no later participant could
    complete the operation from the record.

    ⚠️ AND IT CALLS THE REAL FUNCTION. An earlier version of this test
    reimplemented the dedup inline and passed whatever the production code did,
    which is the exact shape of a check that cannot fail for the reason it
    names.
    """
    # Two routes to one slot, plus the same NAME on a second machine -- the case
    # a (slot, machine) key could not have caught, because the published list
    # carries the name alone.
    rows = ["slot01", "slot01", "slot02", "slot01"]
    assert cli._first_seen_names(rows) == ["slot01", "slot02"]

    published = cli._first_seen_names(rows)
    assert len(published) == len(set(published))
    # First-seen order, so the report is stable between runs.
    assert cli._first_seen_names(["b", "a", "b"]) == ["b", "a"]
    assert cli._first_seen_names([]) == []


def exiting_init_proc(
    proc_root: Path, pid: int = 2991802, *, wchan: str = "zap_pid_ns_processes",
    nspid: str = "2991802\t1", cwd: str | None = None, root: str | None = None,
    threads: int = 1,
) -> Path:
    """The measured shape of a PID-namespace init being torn down.

    Its wait channel is zap_pid_ns_processes and its cwd, root and ns/mnt
    are gone (ENOENT): the kernel released them before it began to wait.
    """
    pid_dir = proc_root / str(pid)
    (pid_dir / "ns").mkdir(parents=True)
    (pid_dir / "stat").write_bytes(opaque_stat(pid, 1, flags=0, start_ticks=99))
    uid = os.getuid()
    (pid_dir / "status").write_text(
        f"Name:\tpidns_recheck\nState:\tS (sleeping)\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"
        f"NSpid:\t{nspid}\nThreads:\t{threads}\n", encoding="ascii")
    (pid_dir / "cgroup").write_text("0::/fixture\n", encoding="ascii")
    (pid_dir / "wchan").write_text(wchan, encoding="ascii")
    if cwd is not None:
        (pid_dir / "cwd").symlink_to(cwd)
    if root is not None:
        (pid_dir / "root").symlink_to(root)
    return pid_dir


def test_an_exiting_namespace_init_holds_no_checkout(
        tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    # 2026-10-13: every `wrkslots remove <validate-fresh-...> --validate-complete`
    # refused "mount namespace is indeterminate for live PID 2991802", a
    # namespace init stuck in zap_pid_ns_processes, and 49 GB of finished
    # validation checkouts could not be removed.
    proc_root = tmp_path / "proc"
    exiting_init_proc(proc_root)
    assert cli._absent_validate_process_snapshot(proc_root) == ()
    budget = cli._ReadOnlyCommandBudget.start(timeout_seconds=30, stdout_limit=1 << 20,
                                              stderr_limit=1 << 20)
    assert cli._same_uid_process_observations(budget, proc_root) == ()
    err = capsys.readouterr().err
    assert "PID 2991802 (pidns_recheck) is a PID-namespace init being torn down" in err


class _NearMiss(TypedDict, total=False):
    """One way to differ from an exiting namespace init; keyword arguments of exiting_init_proc."""

    wchan: str
    nspid: str
    cwd: str
    root: str
    threads: int


@pytest.mark.parametrize("near_miss", [
    {"wchan": "do_wait"},                   # same name, not tearing down: still refused
    {"nspid": "2991802\t7"},               # not the init of its namespace
    {"nspid": "1\t7"},                     # PID 1 only at an OUTER level: not the init
    {"cwd": "/home/somewhere/checkout"},   # still holds a working directory
    {"root": "/"},                          # still holds a root
    {"threads": 2},                         # another thread may still hold files
], ids=["other-wait-channel", "not-a-namespace-init", "init-only-at-an-outer-level",
        "cwd-still-held", "root-still-held", "two-threads"])
def test_a_near_miss_of_an_exiting_namespace_init_is_still_refused(
        tmp_path: Path, near_miss: _NearMiss) -> None:
    proc_root = tmp_path / "proc"
    exiting_init_proc(proc_root, **near_miss)
    with pytest.raises(cli.Refusal, match="mount namespace is indeterminate for live PID 2991802"):
        cli._absent_validate_process_snapshot(proc_root)
    budget = cli._ReadOnlyCommandBudget.start(timeout_seconds=30, stdout_limit=1 << 20,
                                              stderr_limit=1 << 20)
    with pytest.raises(cli.Refusal, match="mount namespace is indeterminate for live PID 2991802"):
        cli._same_uid_process_observations(budget, proc_root)



def exiting_proc(
    proc_root: Path, pid: int = 2991803, *, threads: int = 1,
    cwd: str | None = None,
) -> Path:
    """One single-threaded task met inside do_exit on a loaded host.

    Its stat still reads S and its wait channel is unremarkable, but
    cwd, root and ns/mnt are already gone (ENOENT): the kernel
    released its files, filesystem context and namespaces before
    exit_notify made it a zombie.
    """
    pid_dir = proc_root / str(pid)
    (pid_dir / "ns").mkdir(parents=True)
    (pid_dir / "fd").mkdir(parents=True)
    (pid_dir / "stat").write_bytes(opaque_stat(pid, 1, flags=0, start_ticks=99))
    uid = os.getuid()
    (pid_dir / "status").write_text(
        f"Name:\texiting_recheck\nState:\tS (sleeping)\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"
        f"NSpid:\t{pid}\nThreads:\t{threads}\n", encoding="ascii")
    (pid_dir / "cgroup").write_text("0::/fixture\n", encoding="ascii")
    (pid_dir / "wchan").write_text("0", encoding="ascii")
    if cwd is not None:
        (pid_dir / "cwd").symlink_to(cwd)
    return pid_dir


def test_an_exiting_process_with_released_context_holds_no_checkout(
        tmp_path: Path) -> None:
    # 2026-11-02: the audit census on a busy host met such a task
    # (a short-lived `timeout` wrapper in exit teardown) and refused
    # "mount namespace is indeterminate for live PID N", flipping
    # every audit row to UNKNOWN while the Rust edition, which walks
    # cwd/exe/fd links only, reported the rows BLOCKED.
    proc_root = tmp_path / "proc"
    exiting_proc(proc_root)
    assert cli._absent_validate_process_snapshot(proc_root) == ()
    budget = cli._ReadOnlyCommandBudget.start(timeout_seconds=30, stdout_limit=1 << 20,
                                              stderr_limit=1 << 20)
    assert cli._same_uid_process_observations(budget, proc_root) == ()


@pytest.mark.parametrize("near_miss", [
    {"threads": 2},                        # another thread may still hold files
    {"cwd": "/home/somewhere/checkout"},   # still holds a working directory
], ids=["two-threads", "cwd-still-held"])
def test_a_near_miss_of_an_exiting_process_is_still_refused(
        tmp_path: Path, near_miss: dict[str, object]) -> None:
    proc_root = tmp_path / "proc"
    exiting_proc(proc_root, **near_miss)  # type: ignore[arg-type]
    with pytest.raises(cli.Refusal, match="mount namespace is indeterminate for live PID 2991803"):
        cli._absent_validate_process_snapshot(proc_root)
    budget = cli._ReadOnlyCommandBudget.start(timeout_seconds=30, stdout_limit=1 << 20,
                                              stderr_limit=1 << 20)
    with pytest.raises(cli.Refusal, match="mount namespace is indeterminate for live PID 2991803"):
        cli._same_uid_process_observations(budget, proc_root)


def _live_proc(proc_root: Path, pid: int = 2991804) -> Path:
    pid_dir = proc_root / str(pid)
    (pid_dir / "ns").mkdir(parents=True)
    (pid_dir / "stat").write_bytes(opaque_stat(pid, 1, flags=0, start_ticks=99))
    uid = os.getuid()
    (pid_dir / "status").write_text(
        f"Name:\tlive\nState:\tS (sleeping)\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"
        f"NSpid:\t{pid}\nThreads:\t1\n", encoding="ascii")
    (pid_dir / "cgroup").write_text("0::/fixture\n", encoding="ascii")
    return pid_dir


def _mark_zombie(pid_dir: Path, pid: int = 2991804) -> None:
    uid = os.getuid()
    (pid_dir / "status").write_text(
        f"Name:\tlive\nState:\tZ (zombie)\nUid:\t{uid}\t{uid}\t{uid}\t{uid}\n"
        f"NSpid:\t{pid}\nThreads:\t1\n", encoding="ascii")


def _budget() -> cli._ReadOnlyCommandBudget:
    return cli._ReadOnlyCommandBudget.start(
        timeout_seconds=30, stdout_limit=1 << 20, stderr_limit=1 << 20)


def test_namespace_read_fails_once_then_zombie_census_continues(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # #875: a PID past the terminal check but mid-exit has an unreadable
    # ns/mnt while start_ticks still reads; a moment later it is a zombie.
    for lane in ("snapshot", "same_uid"):
        proc_root = tmp_path / lane
        pid_dir = _live_proc(proc_root)
        calls = 0

        def flaky(_pid_dir: Path) -> str | None:
            nonlocal calls
            calls += 1
            if calls == 1:
                raise cli.Refusal("mount namespace is indeterminate because ns/mnt is unreadable: simulated")
            _mark_zombie(pid_dir)
            return None

        monkeypatch.setattr(cli, "_mount_namespace", flaky)
        if lane == "snapshot":
            assert cli._absent_validate_process_snapshot(proc_root) == ()
        else:
            assert cli._same_uid_process_observations(_budget(), proc_root) == ()
        assert calls >= 2


def test_namespace_absent_then_disappears_census_continues(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # #875: ns/mnt reads as absent, then the PID disappears on re-probe.
    for lane in ("snapshot", "same_uid"):
        proc_root = tmp_path / lane
        pid_dir = _live_proc(proc_root)
        calls = 0

        def vanishing(_pid_dir: Path) -> str | None:
            nonlocal calls
            calls += 1
            if calls >= 2:
                (pid_dir / "stat").unlink(missing_ok=True)
            return None

        monkeypatch.setattr(cli, "_mount_namespace", vanishing)
        if lane == "snapshot":
            assert cli._absent_validate_process_snapshot(proc_root) == ()
        else:
            assert cli._same_uid_process_observations(_budget(), proc_root) == ()
        assert calls >= 2


def test_namespace_absent_then_generation_changes_census_continues(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # #875: a changed start time on re-probe means the original PID exited.
    for lane in ("snapshot", "same_uid"):
        proc_root = tmp_path / lane
        pid_dir = _live_proc(proc_root)
        calls = 0

        def changed(_pid_dir: Path) -> str | None:
            nonlocal calls
            calls += 1
            if calls >= 2:
                (pid_dir / "stat").write_bytes(opaque_stat(2991804, 1, flags=0, start_ticks=100))
            return None

        monkeypatch.setattr(cli, "_mount_namespace", changed)
        if lane == "snapshot":
            assert cli._absent_validate_process_snapshot(proc_root) == ()
        else:
            assert cli._same_uid_process_observations(_budget(), proc_root) == ()
        assert calls >= 2


def test_namespace_stays_live_and_unreadable_is_still_refused(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # #875: a genuinely live, unreadable namespace is refused after retries.
    for lane in ("snapshot", "same_uid"):
        proc_root = tmp_path / lane
        _live_proc(proc_root)

        def unreadable(_pid_dir: Path) -> str | None:
            raise cli.Refusal("mount namespace is indeterminate because ns/mnt is unreadable: simulated")

        monkeypatch.setattr(cli, "_mount_namespace", unreadable)
        with pytest.raises(cli.Refusal, match="mount namespace is indeterminate"):
            if lane == "snapshot":
                cli._absent_validate_process_snapshot(proc_root)
            else:
                cli._same_uid_process_observations(_budget(), proc_root)


def test_a_link_that_is_unreadable_rather_than_released_is_still_refused(
        tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # Only ENOENT means released; a permission error on cwd says nothing.
    proc_root = tmp_path / "proc"
    pid_dir = exiting_init_proc(proc_root)
    real_readlink = os.readlink

    def readlink(path: str | os.PathLike[str], *, dir_fd: int | None = None) -> str:
        if Path(path) == pid_dir / "cwd":
            raise PermissionError(errno.EACCES, "Permission denied", str(path))
        return real_readlink(path, dir_fd=dir_fd)

    # cli reads links through the os module, so patching os.readlink reaches it.
    monkeypatch.setattr(os, "readlink", readlink)
    assert not cli._process_is_exiting_namespace_init(pid_dir)


# #1173 census-vanished-process: a /proc read that finds its process exited
# (ENOENT, ESRCH) is no evidence and the census completes; EACCES of a live
# process still refuses.  The children and the injected reads mirror the
# other edition's census_gone_tests, which assert the same diagnostics.


@dataclasses.dataclass
class _GoneSlot:
    slot: Path
    outside: Path


@pytest.fixture
def gone_slot(tmp_path: Path) -> _GoneSlot:
    (tmp_path / "slot").mkdir()
    (tmp_path / "outside").mkdir()
    return _GoneSlot((tmp_path / "slot").resolve(), (tmp_path / "outside").resolve())


def _settled(child: subprocess.Popen[bytes], entry: str, target: Path) -> subprocess.Popen[bytes]:
    link = Path(f"/proc/{child.pid}/{entry}")
    for _ in range(500):
        with contextlib.suppress(OSError):
            if os.readlink(link) == str(target):
                return child
        time.sleep(0.01)
    child.kill()
    raise AssertionError(f"{link} never pointed at {target}")


def _in_cwd(slot: _GoneSlot) -> subprocess.Popen[bytes]:
    child = subprocess.Popen(["sleep", "600"], cwd=slot.slot, stdin=subprocess.DEVNULL)
    return _settled(child, "cwd", slot.slot)


def _holding_fd(slot: _GoneSlot) -> subprocess.Popen[bytes]:
    held = slot.slot / "held"
    held.write_bytes(b"x")
    child = subprocess.Popen(
        ["sh", "-c", 'exec sleep 600 3<"$0"', str(held)],
        cwd=slot.outside, stdin=subprocess.DEVNULL,
    )
    return _settled(child, "fd/3", held)


def _running_from(slot: _GoneSlot) -> subprocess.Popen[bytes]:
    program = slot.slot / "sleep"
    program.write_bytes(Path("/bin/sleep").read_bytes())
    program.chmod(0o755)
    child = subprocess.Popen([str(program), "600"], cwd=slot.outside, stdin=subprocess.DEVNULL)
    return _settled(child, "exe", program)


def _inject(monkeypatch: pytest.MonkeyPatch, failing: dict[Path, int]) -> None:
    """Fail the census's reads of exactly these /proc paths with these errnos."""

    def fail(path: os.PathLike[str] | str) -> None:
        number = failing.get(Path(path))
        if number is not None:
            exc_type = {errno.ENOENT: FileNotFoundError, errno.ESRCH: ProcessLookupError,
                        errno.EACCES: PermissionError}.get(number, OSError)
            raise exc_type(number, os.strerror(number), str(path))

    original_readlink = os.readlink
    original_iterdir = Path.iterdir
    original_read_text = Path.read_text

    def readlink(path: os.PathLike[str] | str, *args: object, **kwargs: object) -> str:
        fail(path)
        return original_readlink(path, *args, **kwargs)  # type: ignore[arg-type]

    def iterdir(path: Path) -> Iterator[Path]:
        fail(path)
        return original_iterdir(path)

    def read_text(path: Path, *args: object, **kwargs: object) -> str:
        fail(path)
        return original_read_text(path, *args, **kwargs)  # type: ignore[arg-type]

    monkeypatch.setattr(os, "readlink", readlink)
    monkeypatch.setattr(Path, "iterdir", iterdir)
    monkeypatch.setattr(Path, "read_text", read_text)


def _uses_with(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
    child: subprocess.Popen[bytes], slot: _GoneSlot, entries: Sequence[str], number: int,
) -> tuple[list[str], list[str]]:
    pid_dir = Path(f"/proc/{child.pid}")
    with monkeypatch.context() as patch:
        patch.setenv("WRKSLOTS_CENSUS_DIAGNOSTICS", "1")
        _inject(patch, {pid_dir / entry: number for entry in entries})
        capsys.readouterr()
        uses = cli._process_uses_slot(pid_dir, slot.slot)
    return uses, capsys.readouterr().err.splitlines()


def test_census_skips_and_notes_a_process_gone_on_each_read(
    gone_slot: _GoneSlot, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    children = [
        (_in_cwd(gone_slot), ["cwd"]),
        (_holding_fd(gone_slot), ["fd/3"]),
        (_holding_fd(gone_slot), ["fd"]),
        (_running_from(gone_slot), ["exe", "maps"]),
    ]
    try:
        for child, entries in children:
            uses, notes = _uses_with(monkeypatch, capsys, child, gone_slot, [], errno.ESRCH)
            assert uses, f"{entries} must be a use uninjected"
            assert notes == []
            for number in (errno.ENOENT, errno.ESRCH):
                uses, notes = _uses_with(monkeypatch, capsys, child, gone_slot, entries, number)
                assert uses == [], (entries, number)
                if number == errno.ESRCH:
                    assert notes == [
                        f"census: process {child.pid} exited during the census "
                        f"(/proc/{child.pid}/{entries[0]}: ESRCH); skipped"
                    ]
                else:
                    # Its stat is still there: an absent entry, not an exit.
                    assert notes == []
        # root has no positive case without privilege; a gone root read still
        # completes the census with the other evidence.
        for number in (errno.ENOENT, errno.ESRCH):
            uses, _notes = _uses_with(monkeypatch, capsys, children[0][0], gone_slot, ["root"], number)
            assert len(uses) == 1
    finally:
        for child, _entries in children:
            child.kill()
            child.wait()


@pytest.mark.parametrize("entry", ["cwd", "root", "exe", "fd", "fd/3", "maps"])
def test_census_still_refuses_an_unreadable_read_of_a_live_process(
    gone_slot: _GoneSlot, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
    entry: str,
) -> None:
    child = _holding_fd(gone_slot)
    try:
        with pytest.raises(cli.Refusal) as refused:
            _uses_with(monkeypatch, capsys, child, gone_slot, [entry], errno.EACCES)
        assert "process use is indeterminate because" in str(refused.value)
        assert f"/proc/{child.pid}/{entry} is unreadable" in str(refused.value)
        assert capsys.readouterr().err == ""
    finally:
        child.kill()
        child.wait()


def test_census_counts_an_unreaped_zombie_as_gone(
    gone_slot: _GoneSlot, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    child = subprocess.Popen(["true"], cwd=gone_slot.slot)
    pid_dir = Path(f"/proc/{child.pid}")
    try:
        while not cli._process_has_exited(pid_dir):
            time.sleep(0.005)
        uses, notes = _uses_with(monkeypatch, capsys, child, gone_slot, [], errno.ESRCH)
        assert uses == []
        assert notes == [
            f"census: process {child.pid} exited during the census "
            f"(/proc/{child.pid}/cwd: ENOENT); skipped"
        ]
    finally:
        child.wait()


def test_census_status_read_skips_a_gone_process(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    pid_dir = Path("/proc/123")
    monkeypatch.setenv("WRKSLOTS_CENSUS_DIAGNOSTICS", "1")
    scripted_status(
        monkeypatch, pid_dir,
        [ProcessLookupError(errno.ESRCH, "old inode"),
         ProcessLookupError(errno.ESRCH, "fresh inode also exited")],
    )
    assert cli._census_process_uids(pid_dir) is None
    assert capsys.readouterr().err.splitlines() == [
        "census: process 123 exited during the census (/proc/123/status: ESRCH); skipped"
    ]


def test_census_cgroup_read_skips_only_a_gone_process(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    monkeypatch.setenv("WRKSLOTS_CENSUS_DIAGNOSTICS", "1")
    pid_dir = Path("/proc/123")
    with monkeypatch.context() as patch:
        _inject(patch, {pid_dir / "cgroup": errno.ESRCH})
        assert cli._census_process_cgroup(pid_dir) is None
    assert capsys.readouterr().err.splitlines() == [
        "census: process 123 exited during the census (/proc/123/cgroup: ESRCH); skipped"
    ]
    # A live process's unreadable or failing cgroup still refuses.
    live = Path(f"/proc/{os.getpid()}")
    for number in (errno.EACCES, errno.EIO, errno.ENOENT):
        with monkeypatch.context() as patch:
            _inject(patch, {live / "cgroup": number})
            with pytest.raises(cli.Refusal, match="cannot read process cgroup"):
                cli._census_process_cgroup(live)


def test_census_of_processes_exiting_meanwhile_never_refuses(gone_slot: _GoneSlot) -> None:
    for _ in range(200):
        child = subprocess.Popen(["true"], cwd=gone_slot.slot)
        pid_dir = Path(f"/proc/{child.pid}")
        reaper = threading.Thread(target=child.wait)
        reaper.start()
        try:
            for _ in range(20):
                cli._process_uses_slot(pid_dir, gone_slot.slot)
                cli._census_process_uids(pid_dir)
        finally:
            reaper.join()


def _exiting_stat(stat: bytes) -> bytes:
    """``stat`` with State D and PF_EXITING set: a task stuck in do_exit."""
    closing = stat.rfind(b") ")
    fields = stat[closing + 2:].split()
    fields[0] = b"D"
    fields[6] = str(int(fields[6]) | cli._PF_EXITING).encode()
    return stat[:closing] + b") " + b" ".join(fields) + b"\n"


def _stat_says_exiting(
    monkeypatch: pytest.MonkeyPatch, pid_dir: Path, *, until_exit: bool,
) -> None:
    """Make the census read ``pid_dir`` as in do_exit (until it really exits)."""
    original = Path.read_bytes
    exiting = _exiting_stat(original(pid_dir / "stat"))

    def read_bytes(path: Path) -> bytes:
        if path != pid_dir / "stat":
            return original(path)
        if until_exit:
            real = original(path)
            parsed = cli._parse_proc_stat(real)
            if parsed is not None and parsed.state in cli._ZOMBIE_STATES:
                return real
        return exiting

    monkeypatch.setattr(Path, "read_bytes", read_bytes)


def test_census_unreadable_entry_of_a_process_that_exits_within_the_wait_is_gone(
    gone_slot: _GoneSlot, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    # Not in do_exit: an unreadable entry of a live process refuses at once.
    live = _holding_fd(gone_slot)
    try:
        started = time.monotonic()
        with pytest.raises(cli.Refusal, match="is unreadable"):
            _uses_with(monkeypatch, capsys, live, gone_slot, ["fd"], errno.EACCES)
        assert time.monotonic() - started < cli._CENSUS_EXIT_WAIT_SECONDS
        assert capsys.readouterr().err == ""
    finally:
        live.kill()
        live.wait()
    # In do_exit (its stat says so until it really is a zombie): it counts as
    # gone once it exits within the wait.
    child = subprocess.Popen(["sleep", "0.02"], cwd=gone_slot.outside)
    pid_dir = Path(f"/proc/{child.pid}")
    try:
        with monkeypatch.context() as patch:
            _stat_says_exiting(patch, pid_dir, until_exit=True)
            uses, notes = _uses_with(patch, capsys, child, gone_slot, ["fd"], errno.EACCES)
        assert uses == []
        assert notes == [
            f"census: process {child.pid} exited during the census "
            f"({pid_dir / 'fd'}: EACCES); skipped"
        ]
    finally:
        child.wait()


def test_census_unreadable_entry_of_a_process_still_exiting_after_the_wait_refuses(
    gone_slot: _GoneSlot, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    child = _in_cwd(gone_slot)
    pid_dir = Path(f"/proc/{child.pid}")
    try:
        with monkeypatch.context() as patch:
            _stat_says_exiting(patch, pid_dir, until_exit=False)
            started = time.monotonic()
            with pytest.raises(cli.Refusal) as refused:
                _uses_with(patch, capsys, child, gone_slot, ["fd"], errno.EACCES)
            assert time.monotonic() - started >= cli._CENSUS_EXIT_WAIT_SECONDS
        assert f"{pid_dir / 'fd'} is unreadable" in str(refused.value)
        assert re.search(
            rf"; process {child.pid} is still exiting \(PF_EXITING, its memory already "
            r"released\) after a \d+ ms wait \(this census waits 2000 ms in all",
            str(refused.value),
        )
        [note] = capsys.readouterr().err.splitlines()
        assert re.fullmatch(
            rf"census: process {child.pid} still exiting after \d+ ms "
            rf"\({re.escape(str(pid_dir / 'fd'))}: EACCES\); indeterminate",
            note,
        )
    finally:
        child.kill()
        child.wait()


class _FakeClock:
    """A monotonic clock that moves only when the census sleeps."""

    def __init__(self) -> None:
        self.now = 0.0
        self.sleeps: list[float] = []

    def clock(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.sleeps.append(seconds)
        self.now += seconds


def _fake_exiting_process(root: Path, pid: int) -> Path:
    """A /proc/PID stand-in under ``root`` whose stat says it is in do_exit."""
    pid_dir = root / str(pid)
    pid_dir.mkdir(parents=True)
    (pid_dir / "stat").write_bytes(_exiting_stat(Path("/proc/self/stat").read_bytes()))
    return pid_dir


def _zombie_stat(stat: bytes) -> bytes:
    closing = stat.rfind(b") ")
    fields = stat[closing + 2:].split()
    fields[0] = b"Z"
    return stat[:closing] + b") " + b" ".join(fields) + b"\n"


def test_census_exit_wait_ends_as_soon_as_the_process_exits(tmp_path: Path) -> None:
    """An ordinary exit costs one short recheck, not the whole wait (#1174)."""
    pid_dir = _fake_exiting_process(tmp_path, 4242)
    fake = _FakeClock()

    def sleep_then_exit(seconds: float) -> None:
        fake.sleep(seconds)
        stat = pid_dir / "stat"
        stat.write_bytes(_zombie_stat(stat.read_bytes()))

    exit_wait = cli._CensusExitWait(clock=fake.clock, sleep=sleep_then_exit)
    denied = PermissionError(errno.EACCES, "Permission denied")
    assert cli._unreadable_entry_exited(pid_dir, pid_dir / "fd", denied, exit_wait)
    assert fake.sleeps == [cli._CENSUS_EXIT_FIRST_POLL_SECONDS]
    assert exit_wait.still_exiting == {}


def test_census_exiting_processes_share_one_deadline(tmp_path: Path) -> None:
    """Ten processes stuck in do_exit cost one wait in all, then refuse (#1174)."""
    pid_dirs = [_fake_exiting_process(tmp_path, 5000 + index) for index in range(10)]
    fake = _FakeClock()
    exit_wait = cli._CensusExitWait(clock=fake.clock, sleep=fake.sleep)
    denied = PermissionError(errno.EACCES, "Permission denied")
    for pid_dir in pid_dirs:
        assert not cli._unreadable_entry_exited(pid_dir, pid_dir / "fd", denied, exit_wait)
    assert fake.now == pytest.approx(cli._CENSUS_EXIT_WAIT_SECONDS)
    # Rechecks back off from the first poll to the steady one.
    assert fake.sleeps[:2] == [cli._CENSUS_EXIT_FIRST_POLL_SECONDS] + [
        2 * cli._CENSUS_EXIT_FIRST_POLL_SECONDS
    ]
    assert max(fake.sleeps) == cli._CENSUS_EXIT_POLL_SECONDS
    assert exit_wait.still_exiting == {
        pid_dir: (2000 if index == 0 else 0) for index, pid_dir in enumerate(pid_dirs)
    }
    refusal = cli._unreadable_entry_refusal(
        pid_dirs[3], pid_dirs[3] / "fd", denied, exit_wait
    )
    assert str(refusal) == (
        f"process use is indeterminate because {pid_dirs[3] / 'fd'} is unreadable: "
        f"{denied}; process 5003 is still exiting (PF_EXITING, its memory already "
        "released) after a 0 ms wait (this census waits 2000 ms in all for exiting "
        "processes)"
    )


_MAP_ONLY_CHILD = """
import ctypes, os, sys, time
libc = ctypes.CDLL(None, use_errno=True)
libc.mmap.restype = ctypes.c_void_p
libc.mmap.argtypes = (
    ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long,
)
fd = os.open(sys.argv[1], os.O_RDONLY)
assert libc.mmap(None, 4096, 1, 2, fd, 0) not in (None, ctypes.c_void_p(-1).value)
os.close(fd)
print("ready", flush=True)
time.sleep(float(sys.argv[2]))
"""


def _mapping_only(slot: _GoneSlot, seconds: str) -> subprocess.Popen[bytes]:
    """A child outside the slot whose only use of it is a libc mapping (#1174)."""
    held = slot.slot / "mapped"
    held.write_bytes(b"x" * 4096)
    child = subprocess.Popen(
        [sys.executable, "-c", _MAP_ONLY_CHILD, str(held), seconds],
        cwd=slot.outside, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
    )
    assert child.stdout is not None and child.stdout.readline().strip() == b"ready"
    return child


def test_census_unreadable_maps_follow_the_exit_and_lsof_rules(
    gone_slot: _GoneSlot, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str],
) -> None:
    """Paired with the other edition's snapshot case for #1174 (same child, same faults)."""
    child = _mapping_only(gone_slot, "600")
    pid_dir = Path(f"/proc/{child.pid}")
    maps = pid_dir / "maps"
    try:
        assert cli._process_uses_slot(pid_dir, gone_slot.slot) == [
            f"map={gone_slot.slot / 'mapped'}"
        ]
        # Still in do_exit past the wait: indeterminate.
        with monkeypatch.context() as patch:
            _stat_says_exiting(patch, pid_dir, until_exit=False)
            started = time.monotonic()
            with pytest.raises(cli.Refusal, match=f"{maps} is unreadable"):
                _uses_with(patch, capsys, child, gone_slot, ["maps"], errno.EACCES)
            assert time.monotonic() - started >= cli._CENSUS_EXIT_WAIT_SECONDS
        # A protected process that is not exiting, under the remove census's
        # lsof rule: no evidence.
        with monkeypatch.context() as patch:
            _inject(patch, {maps: errno.EACCES})
            assert cli._process_uses_slot(
                pid_dir, gone_slot.slot, unreadable_is_no_evidence=True
            ) == []
    finally:
        child.kill()
        child.wait()
    # In do_exit and exiting within the wait: gone.
    child = _mapping_only(gone_slot, "0.05")
    pid_dir = Path(f"/proc/{child.pid}")
    try:
        with monkeypatch.context() as patch:
            _stat_says_exiting(patch, pid_dir, until_exit=True)
            uses, notes = _uses_with(patch, capsys, child, gone_slot, ["maps"], errno.EACCES)
        assert uses == []
        assert notes == [
            f"census: process {child.pid} exited during the census "
            f"({pid_dir / 'maps'}: EACCES); skipped"
        ]
    finally:
        child.wait()


_CHROOT_ONLY_CHILD = """
import os, sys, time
slot, outside = sys.argv[1], sys.argv[2]
away = os.open(outside, os.O_RDONLY)
os.chroot(slot)
os.fchdir(away)
os.close(away)
print("ready", flush=True)
time.sleep(600)
"""


def _rooted_in(slot: _GoneSlot) -> subprocess.Popen[bytes] | None:
    """A child whose only use of the slot is its root (chroot in its own user namespace)."""
    child = subprocess.Popen(
        ["unshare", "-r", sys.executable, "-c", _CHROOT_ONLY_CHILD, str(slot.slot),
         str(slot.outside)],
        cwd=slot.outside, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    assert child.stdout is not None
    if child.stdout.readline().strip() != b"ready":
        child.wait()
        return None
    return child


def _fresh_verdict(
    read: Callable[[], cli._LiveUseObservation | None],
) -> tuple[str, int | None, str]:
    try:
        observation = read()
    except cli.Refusal as exc:
        return ("refused", None, str(exc))
    if observation is None:
        return ("unused", None, "")
    return ("live", observation.pid, str(observation.refusal))


def test_one_shared_walk_answers_every_slot_as_its_own_walk(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """#1175 census-single-pass: the batch's one /proc walk equals one walk per slot.

    Real children use six slots in every way the walk reads (working directory,
    root, executable, descriptor, mapping, and the owner cgroup through the
    record), a seventh slot is unused, and one listed process has exited.  The
    walk reads a proc root that lists exactly those processes.  For each slot,
    record and generation mode, the shared walk's lookup returns or raises
    exactly what the slot's own `_agent_batch_fresh_use` does.
    """

    (tmp_path / "outside").mkdir()
    outside = (tmp_path / "outside").resolve()

    def slot(name: str) -> _GoneSlot:
        (tmp_path / name).mkdir()
        return _GoneSlot((tmp_path / name).resolve(), outside)

    slots = {name: slot(name) for name in ("cwd", "root", "exe", "fd", "map", "free")}
    children: list[subprocess.Popen[bytes]] = []
    try:
        children.append(_in_cwd(slots["cwd"]))
        children.append(_running_from(slots["exe"]))
        children.append(_holding_fd(slots["fd"]))
        children.append(_mapping_only(slots["map"], "600"))
        rooted = _rooted_in(slots["root"])
        if rooted is not None:
            children.append(rooted)
        vanished = subprocess.Popen(["/bin/true"])
        vanished.wait()
        proc_root = tmp_path / "proc"
        proc_root.mkdir()
        for pid in [child.pid for child in children] + [vanished.pid]:
            (proc_root / str(pid)).symlink_to(f"/proc/{pid}")
        targets = [entry.slot for entry in slots.values()]
        shared = cli._agent_batch_fresh_uses(targets, proc_root=proc_root)
        # A record whose owner cgroup is evidence and is the children's cgroup.
        own_cgroup = cli._read_process_cgroup(Path(f"/proc/{children[0].pid}"))
        monkeypatch.setattr(cli, "_owner_cgroup_is_evidence", lambda record: record is not None)
        owner = dataclasses.make_dataclass("Owner", [("cgroup_path", str)])(own_cgroup)
        record_type = dataclasses.make_dataclass("Record", [("owner", object)])
        records = [None, record_type(owner)]
        verdicts: dict[str, str] = {}
        for name, entry in slots.items():
            for record in records:
                for capture_generation in (False, True):
                    def own() -> cli._LiveUseObservation | None:
                        return cli._agent_batch_fresh_use(
                            entry.slot, record,
                            capture_generation=capture_generation, proc_root=proc_root,
                        )

                    def looked_up() -> cli._LiveUseObservation | None:
                        return shared.lookup(
                            entry.slot, record,
                            capture_generation=capture_generation,
                        )

                    expected = _fresh_verdict(own)
                    assert _fresh_verdict(looked_up) == expected, (name, record, capture_generation)
                    if record is None:
                        verdicts[name] = expected[0]
        expected_verdicts = {"cwd": "live", "exe": "live", "fd": "live", "map": "live",
                             "free": "unused", "root": "live" if rooted is not None else "unused"}
        assert verdicts == expected_verdicts, verdicts
    finally:
        for child in children:
            child.kill()
            child.wait()


_SOCKET_CHILD = """
import socket, sys, time
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(sys.argv[1])
print("ready", flush=True)
time.sleep(600)
"""

_TWO_TREES_CHILD = """
import os, sys, time
held = open(sys.argv[2], "rb")
os.chdir(sys.argv[1])
print("ready", flush=True)
time.sleep(600)
"""


def _ready_child(argv: list[str], cwd: Path) -> subprocess.Popen[bytes]:
    child = subprocess.Popen(argv, cwd=cwd, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE)
    assert child.stdout is not None and child.stdout.readline().strip() == b"ready"
    return child


def _group_outcome(
    read: Callable[[], cli._LiveUseObservation | cli._UndecidedFencedUse | None],
) -> tuple[str, int | None, str]:
    try:
        observed = read()
    except cli.Refusal as exc:
        return ("refused", None, str(exc))
    if observed is None:
        return ("unused", None, "")
    if isinstance(observed, cli._UndecidedFencedUse):
        return ("undecided", None, observed.reason)
    return ("live", observed.pid, str(observed.refusal))


def test_one_group_scan_decides_each_member_as_its_own_fenced_scan(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """#1201 batch-grouped-fencing: one group scan equals one fenced scan per member.

    Real children use fenced trees by working directory, descriptor,
    executable, mapping only, a socket bound below the tree, and a hard
    link outside it (identity only); one child holds two trees at once; one
    tree is unused, one has an unreadable directory (undecided, unless root
    reads it), and one is an image-style member that matches by device.
    A record whose owner cgroup is the children's refuses its member.  The
    proc root lists those children and one exited process.  For each
    member and generation mode, the group verdict returns or raises exactly
    what `_agent_batch_fenced_use` does for that member alone.
    """

    (tmp_path / "outside").mkdir()
    outside = (tmp_path / "outside").resolve()
    names = ("cwd", "fd", "exe", "map", "sock", "link", "both-cwd", "both-fd", "free",
             "unreadable", "device", "cgroup")
    trees: dict[str, _GoneSlot] = {}
    for name in names:
        (tmp_path / f".{name}.fenced.1.x").mkdir()
        trees[name] = _GoneSlot((tmp_path / f".{name}.fenced.1.x").resolve(), outside)
    children: list[subprocess.Popen[bytes]] = []
    blocked = trees["unreadable"].slot / "blocked"
    try:
        children.append(_in_cwd(trees["cwd"]))
        children.append(_holding_fd(trees["fd"]))
        children.append(_running_from(trees["exe"]))
        children.append(_mapping_only(trees["map"], "600"))
        children.append(_ready_child(
            [sys.executable, "-c", _SOCKET_CHILD, str(trees["sock"].slot / "s")], outside))
        (trees["link"].slot / "original").write_bytes(b"l")
        os.link(trees["link"].slot / "original", outside / "alias")
        children.append(_ready_child(
            [sys.executable, "-c", _TWO_TREES_CHILD.replace(
                "os.chdir(sys.argv[1])", "pass"), str(outside), str(outside / "alias")],
            outside))
        (trees["both-fd"].slot / "held").write_bytes(b"b")
        children.append(_ready_child(
            [sys.executable, "-c", _TWO_TREES_CHILD, str(trees["both-cwd"].slot),
             str(trees["both-fd"].slot / "held")], outside))
        blocked.mkdir()
        blocked.chmod(0)
        vanished = subprocess.Popen(["/bin/true"])
        vanished.wait()
        proc_root = tmp_path / "proc"
        proc_root.mkdir()
        for pid in [child.pid for child in children] + [vanished.pid]:
            (proc_root / str(pid)).symlink_to(f"/proc/{pid}")
        own_cgroup = cli._read_process_cgroup(Path(f"/proc/{children[0].pid}"))
        monkeypatch.setattr(cli, "_owner_cgroup_is_evidence", lambda record: record is not None)
        owner = dataclasses.make_dataclass("Owner", [("cgroup_path", str)])(own_cgroup)
        record = cast(cli.ActiveRecord, dataclasses.make_dataclass("Record", [("owner", object)])(owner))
        members = [
            cli._FencedGroupMember(
                trees[name].slot,
                tmp_path / name,
                record if name == "cgroup" else None,
                whole_device=name == "device",
            )
            for name in names
        ]
        group = cli._agent_batch_group_fenced_uses(members, proc_root=proc_root)
        assert set(group) == {member.canonical_path for member in members}
        kinds: dict[str, str] = {}
        for name, member in zip(names, members):
            for capture_generation in (False, True):
                def own() -> cli._LiveUseObservation | cli._UndecidedFencedUse | None:
                    return cli._agent_batch_fenced_use(
                        member.fenced_path, member.canonical_path, member.record,
                        capture_generation=capture_generation, proc_root=proc_root,
                        whole_device=member.whole_device,
                    )

                def grouped() -> cli._LiveUseObservation | cli._UndecidedFencedUse | None:
                    return cli._fenced_group_result(
                        group[member.canonical_path], capture_generation=capture_generation
                    )

                expected = _group_outcome(own)
                assert _group_outcome(grouped) == expected, (name, capture_generation)
                kinds[name] = expected[0]
        root_reads = os.geteuid() == 0 or os.access(blocked, os.R_OK)
        assert kinds == {
            "cwd": "live", "fd": "live", "exe": "live", "map": "live", "sock": "live",
            "link": "live", "both-cwd": "live", "both-fd": "live", "free": "unused",
            "unreadable": "unused" if root_reads else "undecided", "device": "live",
            "cgroup": "refused",
        }, kinds
        # One member alone is the same scan as a group of one.
        alone = cli._agent_batch_group_fenced_uses(members[:1], proc_root=proc_root)
        assert _group_outcome(lambda: cli._fenced_group_result(
            alone[members[0].canonical_path], capture_generation=True)) == _group_outcome(
            lambda: cli._fenced_group_result(
                group[members[0].canonical_path], capture_generation=True))
    finally:
        with contextlib.suppress(OSError):
            blocked.chmod(0o700)
        for child in children:
            child.kill()
            child.wait()


_SEAL_THROUGH_BIND = """
import os, subprocess, sys
from pathlib import Path
sys.path.insert(0, sys.argv[1])
from wrkslots import cli
source, mount_point = Path(sys.argv[2]), Path(sys.argv[3])
subprocess.run(["mount", "--bind", str(source), str(mount_point)], check=True)
cli._seal_group_fence(mount_point)
sealed = (oct(os.stat(source).st_mode & 0o7777), oct(os.stat(mount_point).st_mode & 0o7777))
os.chmod(mount_point, 0o755)
print(*sealed, oct(os.stat(source).st_mode & 0o7777))
"""


def test_seal_through_a_bind_mount_point_seals_the_mounted_root(tmp_path: Path) -> None:
    """#1201: chmod on a fenced mount point seals the mounted tree's root.

    A worktrees-image slot's fence is a bind mount of its subvolume, and an
    image slot's fence a mount of its file system; either way the fenced
    path is a mount point.  In a user and mount namespace of its own, a bind
    mount's point is sealed: the bound directory itself shows mode 0, and
    restoring through the point restores it.
    """

    if shutil.which("unshare") is None or shutil.which("mount") is None:
        pytest.skip("needs unshare and mount")
    source = tmp_path / "subvolume"
    mount_point = tmp_path / ".slot01.fenced.1.x"
    source.mkdir(mode=0o755)
    mount_point.mkdir(mode=0o755)
    (source / "file").write_text("x")
    run = subprocess.run(
        ["unshare", "-rm", sys.executable, "-I", "-c", _SEAL_THROUGH_BIND,
         str(Path(cli.__file__).resolve().parents[1]), str(source), str(mount_point)],
        capture_output=True, text=True, timeout=60, check=False,
    )
    if run.returncode != 0 and "Operation not permitted" in run.stderr:
        pytest.skip(f"no unprivileged mount namespace here: {run.stderr.strip()}")
    assert run.returncode == 0, run.stderr
    assert run.stdout.split() == ["0o0", "0o0", "0o755"]


# --- #1230 socket-liveness-proof -------------------------------------------------------------

_SEQ, _PORT = 77, 4242


def _nl(kind: int, flags: int, body: bytes = b"", *, seq: int = _SEQ, port: int = _PORT) -> bytes:
    length = 16 + len(body)
    return struct.pack("=IHHII", length, kind, flags, seq, port) + body + b"\0" * (-length % 4)


def _nlattr(kind: int, payload: bytes) -> bytes:
    length = 4 + len(payload)
    return struct.pack("=HH", length, kind) + payload + b"\0" * (-length % 4)


def _udiag(
    ino: int, *, vfs: tuple[int, int] | None = None, name: bytes | None = None,
    family: int = socket.AF_UNIX, extra: bytes = b"",
) -> bytes:
    body = struct.pack("=BBBBI2I", family, socket.SOCK_STREAM, 7, 0, ino, 5, 0)
    if name is not None:
        body += _nlattr(sockdiag.UNIX_DIAG_NAME, name)
    if vfs is not None:
        body += _nlattr(sockdiag.UNIX_DIAG_VFS, struct.pack("=II", *vfs))
    return body + extra


def _rec(ino: int, **kwargs: object) -> bytes:
    return _nl(sockdiag.SOCK_DIAG_BY_FAMILY, 2, _udiag(ino, **kwargs))  # type: ignore[arg-type]


def _done(status: int = 0) -> bytes:
    return _nl(sockdiag.NLMSG_DONE, 2, struct.pack("=i", status))


def _chunk(*messages: bytes, flags: int = 0, sender: tuple[int, int] = (0, 0)) -> tuple[bytes, int, tuple[int, int]]:
    return (b"".join(messages), flags, sender)


def test_sockdiag_parses_a_clean_multipart_dump() -> None:
    sockets, total = sockdiag.parse_dump(
        [_chunk(_rec(10, vfs=(5, 99), name=b"/x")), _chunk(_rec(11), _done())],
        seq=_SEQ, port=_PORT,
    )
    assert [(item.sockfs_ino, item.vfs, item.name) for item in sockets] == [
        (10, (5, 99), b"/x"), (11, None, None),
    ]
    assert total > 0


@pytest.mark.parametrize(
    ("chunks", "reason"),
    [
        ([_chunk(_rec(1))], "without NLMSG_DONE"),
        ([_chunk(_nl(sockdiag.SOCK_DIAG_BY_FAMILY, 2 | sockdiag.NLM_F_DUMP_INTR, _udiag(1)), _done())], "interrupted"),
        ([_chunk(_nl(sockdiag.NLMSG_ERROR, 0, struct.pack("=i", -1)))], "refused with error 1"),
        ([_chunk(_nl(sockdiag.SOCK_DIAG_BY_FAMILY, 2, _udiag(1), seq=_SEQ + 1), _done())], "sequence"),
        ([_chunk(_nl(sockdiag.SOCK_DIAG_BY_FAMILY, 2, _udiag(1), port=_PORT + 1), _done())], "port"),
        ([_chunk(_rec(1), _done(), sender=(99, 0))], "not the kernel"),
        ([_chunk(_rec(1), _done(), sender=(0, 1))], "not the kernel"),
        ([_chunk(_rec(1), _done(), flags=socket.MSG_TRUNC)], "MSG_TRUNC"),
        ([_chunk(_nl(sockdiag.SOCK_DIAG_BY_FAMILY, 2, _udiag(1, extra=struct.pack("=HH", 64, 1))), _done())], "overruns its record"),
        ([_chunk(_rec(1, vfs=(1, 2), extra=_nlattr(sockdiag.UNIX_DIAG_VFS, b"\0" * 8)), _done())], "malformed UNIX_DIAG_VFS"),
        ([_chunk(_nl(sockdiag.SOCK_DIAG_BY_FAMILY, 2, _udiag(1, extra=_nlattr(sockdiag.UNIX_DIAG_VFS, b"\0" * 4))), _done())], "malformed UNIX_DIAG_VFS"),
        ([_chunk(_nl(sockdiag.SOCK_DIAG_BY_FAMILY, 2, b"\1\1")), _done()], "shorter than its header"),
        ([_chunk(_rec(1, family=socket.AF_INET), _done())], "family"),
        ([_chunk(_rec(1), _done(-5))], "ended with error 5"),
        ([_chunk(_rec(1), _done(), _rec(2))], "data after NLMSG_DONE"),
        ([_chunk(_rec(1), _done()), _chunk(_rec(2))], "data after NLMSG_DONE"),
        ([_chunk(_nl(99, 2, b""), _done())], "unexpected netlink message type 99"),
        ([_chunk(_rec(1)[:-4]), _chunk(_done())], "overruns its datagram"),
        ([_chunk(struct.pack("=IHHII", 400, 20, 2, _SEQ, _PORT))], "overruns its datagram"),
        ([_chunk(b"\0" * 8)], "truncated netlink message header"),
    ],
)
def test_sockdiag_refuses_an_unauthenticated_or_incomplete_dump(
    chunks: list[tuple[bytes, int, tuple[int, int]]], reason: str,
) -> None:
    with pytest.raises(sockdiag.DumpError, match=reason):
        sockdiag.parse_dump(chunks, seq=_SEQ, port=_PORT)


def test_sockdiag_refuses_a_dump_beyond_its_record_limit(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(sockdiag, "DUMP_RECORD_LIMIT", 1)
    with pytest.raises(sockdiag.DumpError, match="record limit"):
        sockdiag.parse_dump([_chunk(_rec(1), _rec(2), _done())], seq=_SEQ, port=_PORT)


class _ScriptedTransport(sockdiag.Transport):
    """A netlink peer that answers the request with scripted datagrams."""

    def __init__(self, replies: Callable[[int], list[tuple[bytes, int, tuple[int, int]]]]) -> None:
        self.replies = replies
        self.pending: list[tuple[bytes, int, tuple[int, int]]] = []

    def port(self) -> int:
        return _PORT

    def send(self, data: bytes) -> None:
        seq = struct.unpack_from("=IHHII", data)[3]
        self.pending = self.replies(seq)

    def recv(self) -> tuple[bytes, int, tuple[int, int]]:
        if not self.pending:
            raise TimeoutError("no reply")
        return self.pending.pop(0)

    def close(self) -> None:
        pass


def test_sockdiag_dump_uses_its_own_random_sequence_and_refuses_a_stale_reply() -> None:
    def good(seq: int) -> list[tuple[bytes, int, tuple[int, int]]]:
        return [_chunk(_nl(20, 2, _udiag(3, vfs=(9, 8)), seq=seq), _nl(3, 2, b"\0" * 4, seq=seq))]

    dump = sockdiag.dump_own_namespace(
        deadline=time.monotonic() + 5, transport_factory=lambda _t: _ScriptedTransport(good)
    )
    assert [item.vfs for item in dump.sockets] == [(9, 8)]
    with pytest.raises(sockdiag.DumpError, match="sequence"):
        sockdiag.dump_own_namespace(
            deadline=time.monotonic() + 5,
            transport_factory=lambda _t: _ScriptedTransport(lambda seq: good(seq + 1)),
        )
    with pytest.raises(sockdiag.DumpError, match="timed out before NLMSG_DONE"):
        sockdiag.dump_own_namespace(
            deadline=time.monotonic() + 5,
            transport_factory=lambda _t: _ScriptedTransport(lambda seq: good(seq)[:0] + [_chunk(_nl(20, 2, _udiag(1), seq=seq))]),
        )


def _synthetic_node(root: Path, ino: int, dev: int = 50) -> cli._SocketNode:
    return cli._SocketNode(root / "node", root, "slot", dev, ino, 1, stat.S_IFSOCK | 0o755, (1, 2))


def _synthetic_evidence(
    *sockets: sockdiag.UnixSocket,
    unenterable: dict[int, str] | None = None,
    representatives: dict[int, list[int]] | None = None,
    gaps: Sequence[str] = (),
) -> cli._SocketEvidence:
    dump = sockdiag.Dump(1, tuple(sockets), len(sockets), 0, 0.0)
    return cli._SocketEvidence(
        {1: dump}, dict(unenterable or {}), dict(representatives or {}), list(gaps), 0.0
    )


def _bound(ino: int, vfs: tuple[int, int] | None) -> sockdiag.UnixSocket:
    return sockdiag.UnixSocket(ino, 0, socket.SOCK_STREAM, 7, vfs, None, None)


def test_judge_treats_a_low32_inode_match_as_a_candidate_never_a_nonmatch(tmp_path: Path) -> None:
    node = _synthetic_node(tmp_path, (3 << 32) | 7)
    calls: list[cli._SocketNode] = []

    def calibrate(seen: cli._SocketNode) -> int | None:
        calls.append(seen)
        return 900

    live = cli._judge_socket_nodes([node], _synthetic_evidence(_bound(1, (7, 900))), calibrate=calibrate)
    assert [verdict.verdict for verdict in live] == ["LIVE"] and calls == [node]
    other_device = cli._judge_socket_nodes([node], _synthetic_evidence(_bound(1, (7, 901))), calibrate=calibrate)
    assert [verdict.verdict for verdict in other_device] == ["NO-OWNER-EVIDENCE"]


@pytest.mark.parametrize("calibration", ["skipped", "failed"])
def test_judge_without_a_calibrated_device_leaves_a_candidate_unknown(tmp_path: Path, calibration: str) -> None:
    node = _synthetic_node(tmp_path, 7)
    verdicts = cli._judge_socket_nodes(
        [node], _synthetic_evidence(_bound(1, (7, 900))),
        calibrate=None if calibration == "skipped" else (lambda _n: None),
    )
    assert [verdict.verdict for verdict in verdicts] == ["UNKNOWN"]
    assert "could not be calibrated" in verdicts[0].reason


def test_judge_calibrates_only_on_a_candidate(tmp_path: Path) -> None:
    calls: list[cli._SocketNode] = []

    def calibrate(node: cli._SocketNode) -> int | None:
        calls.append(node)
        return 900

    verdicts = cli._judge_socket_nodes(
        [_synthetic_node(tmp_path, 7)], _synthetic_evidence(_bound(1, (8, 900)), _bound(2, None)),
        calibrate=calibrate,
    )
    assert [verdict.verdict for verdict in verdicts] == ["NO-OWNER-EVIDENCE"] and calls == []


def test_judge_names_every_visibility_gap_as_unknown(tmp_path: Path) -> None:
    verdicts = cli._judge_socket_nodes(
        [_synthetic_node(tmp_path, 7)],
        _synthetic_evidence(gaps=["process 9's network namespace cannot be read", "another"]),
        calibrate=None,
    )
    assert verdicts[0].verdict == "UNKNOWN"
    assert verdicts[0].reason == "process 9's network namespace cannot be read (and 1 more visibility gaps)"


def test_judge_unenterable_namespace_proves_live_by_name_but_never_dead(tmp_path: Path) -> None:
    node = _synthetic_node(tmp_path, 7)
    evidence = _synthetic_evidence(unenterable={5: "EPERM"}, representatives={5: [123]})
    header = b"Num       RefCount Protocol Flags    Type St Inode Path\n"

    def table(listing: bytes | None) -> Callable[[int], bytes | None]:
        return lambda pid: listing if pid == 123 else None

    named = header + b"0000000000000000: 00000002 00000000 00000000 0001 01 4242 " + str(node.path).encode() + b"\n"
    assert cli._judge_socket_nodes([node], evidence, calibrate=None, table_reader=table(named))[0].verdict == "LIVE"
    other = header + b"0000000000000000: 00000002 00000000 00000000 0001 01 4242 /elsewhere\n"
    for listing in (other, None):
        verdict = cli._judge_socket_nodes([node], evidence, calibrate=None, table_reader=table(listing))[0]
        assert verdict.verdict == "UNKNOWN", listing
        assert "cannot be entered" in verdict.reason or "cannot be read" in verdict.reason


@contextlib.contextmanager
def _bound_socket(path: Path, kind: int = socket.SOCK_STREAM, *, listen: bool = False) -> Iterator[socket.socket]:
    sock = socket.socket(socket.AF_UNIX, kind)
    try:
        sock.bind(str(path))
        if listen:
            sock.listen()
        yield sock
    finally:
        sock.close()


def _dead_node(path: Path) -> None:
    with _bound_socket(path):
        pass


def _only_node(root: Path) -> cli._SocketNode:
    nodes = cli._socket_node_inventory({root: "slot"})
    assert len(nodes) == 1
    return nodes[0]


@pytest.mark.parametrize(
    ("kind", "listen", "expected"),
    [
        (socket.SOCK_STREAM, True, "listening"),
        (socket.SOCK_STREAM, False, "a seqpacket connect() found a bound socket of another type"),
        (socket.SOCK_SEQPACKET, False, "a stream connect() found a bound socket of another type"),
        (socket.SOCK_SEQPACKET, True, "a stream connect() found a bound socket of another type"),
        (socket.SOCK_DGRAM, False, "a stream connect() found a bound socket of another type"),
    ],
)
def test_probe_finds_every_bound_socket_type(tmp_path: Path, kind: int, listen: bool, expected: str) -> None:
    with _bound_socket(tmp_path / "s", kind, listen=listen):
        outcome, detail = cli._probe_socket_node(_only_node(tmp_path))
    assert outcome == "LIVE" and expected in detail


def test_probe_refuses_only_a_node_no_socket_is_bound_to(tmp_path: Path) -> None:
    _dead_node(tmp_path / "s")
    assert cli._probe_socket_node(_only_node(tmp_path))[0] == "REFUSED"


def test_probe_needs_the_parent_unchanged_across_the_whole_proof(tmp_path: Path) -> None:
    _dead_node(tmp_path / "s")
    node = _only_node(tmp_path)
    parent_ctimes: dict[tuple[int, int], int] = {}
    assert cli._probe_socket_node(node, parent_ctimes)[0] == "REFUSED"
    time.sleep(0.01)
    (tmp_path / "sibling").write_bytes(b"")
    outcome, detail = cli._probe_socket_node(node, parent_ctimes)
    assert outcome == "UNKNOWN" and "directory was replaced or changed" in detail


@pytest.mark.parametrize("change", ["unlinked", "rebound", "replaced-by-file", "parent-replaced", "ctime"])
def test_probe_never_refuses_a_changed_node(tmp_path: Path, change: str) -> None:
    tree = tmp_path / "tree"
    tree.mkdir()
    _dead_node(tree / "s")
    node = _only_node(tree)
    (tree / "s").unlink()
    if change == "rebound":
        with _bound_socket(tree / "s"):
            outcome = cli._probe_socket_node(node)[0]
    else:
        if change == "replaced-by-file":
            (tree / "s").write_bytes(b"")
        elif change == "ctime":
            _dead_node(tree / "s")
            node = dataclasses.replace(_only_node(tree), ctime_ns=node.ctime_ns - 1)
        elif change == "parent-replaced":
            tree.rename(tmp_path / "old")
            tree.mkdir()
            _dead_node(tree / "s")
        outcome = cli._probe_socket_node(node)[0]
    assert outcome == ("DISAPPEARED" if change == "unlinked" else "UNKNOWN")


@pytest.fixture
def socket_world(monkeypatch: pytest.MonkeyPatch) -> set[int]:
    """Only these PIDs count as same-UID processes, so host load cannot add gaps."""

    allowed = {os.getpid()}
    original = cli._process_uids
    monkeypatch.setattr(
        cli, "_process_uids",
        lambda pid_dir: original(pid_dir) if pid_dir.name.isdigit() and int(pid_dir.name) in allowed else None,
    )
    return allowed


def _prove(root: Path, *, probe: bool = True) -> list[cli._SocketVerdict]:
    verdicts, _evidence = cli._prove_socket_nodes(
        {root: "slot"}, deadline=time.monotonic() + 60, probe=probe, allow_calibration=True,
    )
    return verdicts


def test_proof_calls_a_stale_node_dead_only_with_the_probe(tmp_path: Path, socket_world: set[int]) -> None:
    _dead_node(tmp_path / "s")
    assert [verdict.verdict for verdict in _prove(tmp_path, probe=False)] == ["NO-OWNER-EVIDENCE"]
    dead = _prove(tmp_path)
    assert [(verdict.verdict, verdict.evidence) for verdict in dead] == [("DEAD", "active")]
    assert "refused before and after the second dump" in dead[0].reason
    assert sorted(path.name for path in tmp_path.iterdir()) == ["s"]


@pytest.mark.parametrize("dump_evidence", [True, False])
def test_proof_bound_stream_socket_that_refuses_connect_is_live(
    tmp_path: Path, socket_world: set[int], monkeypatch: pytest.MonkeyPatch, dump_evidence: bool,
) -> None:
    if not dump_evidence:
        # The dump can skip a live socket; the dual-type probe still finds it.
        monkeypatch.setattr(cli._SocketEvidence, "bound", lambda _self: {})
    with _bound_socket(tmp_path / "s"):
        verdicts = _prove(tmp_path)
    assert [verdict.verdict for verdict in verdicts] == ["LIVE"]
    assert ("bound to this node's identity" if dump_evidence else "another type") in verdicts[0].reason


def test_proof_socket_held_only_in_flight_is_live(tmp_path: Path, socket_world: set[int]) -> None:
    sender, receiver = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    try:
        with _bound_socket(tmp_path / "s") as bound:
            sender.sendmsg([b"x"], [(socket.SOL_SOCKET, socket.SCM_RIGHTS, struct.pack("=i", bound.fileno()))])
        verdicts = _prove(tmp_path)
        assert [verdict.verdict for verdict in verdicts] == ["LIVE"], verdicts
    finally:
        sender.close()
        receiver.close()


def test_proof_reruns_once_when_a_node_disappears(
    tmp_path: Path, socket_world: set[int], monkeypatch: pytest.MonkeyPatch,
) -> None:
    _dead_node(tmp_path / "gone")
    inventories: list[int] = []
    original_inventory = cli._socket_node_inventory

    def inventory(targets: dict[Path, str]) -> list[cli._SocketNode]:
        found = original_inventory(targets)
        inventories.append(len(found))
        return found

    original_probe = cli._probe_socket_node

    def probe(node: cli._SocketNode, *rest: dict[tuple[int, int], int]) -> tuple[str, str]:
        node.path.unlink()
        return original_probe(node, *rest)

    monkeypatch.setattr(cli, "_socket_node_inventory", inventory)
    monkeypatch.setattr(cli, "_probe_socket_node", probe)
    assert _prove(tmp_path) == [] and inventories == [1, 0]


def test_proof_second_disappearance_is_never_dead(
    tmp_path: Path, socket_world: set[int], monkeypatch: pytest.MonkeyPatch,
) -> None:
    _dead_node(tmp_path / "s")
    monkeypatch.setattr(cli, "_probe_socket_node", lambda *_a: ("DISAPPEARED", "gone"))
    assert [verdict.verdict for verdict in _prove(tmp_path)] == ["DISAPPEARED"]


def test_require_dead_refuses_any_verdict_but_dead(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    node = _synthetic_node(tmp_path, 7)
    budget = cli._ReadOnlyCommandBudget.start(timeout_seconds=22, stdout_limit=4096, stderr_limit=4096)
    evidence = _synthetic_evidence()
    for verdict in ("LIVE", "UNKNOWN", "NO-OWNER-EVIDENCE", "DISAPPEARED", "DEAD"):
        monkeypatch.setattr(
            cli, "_prove_socket_nodes",
            lambda *_a, value=verdict, **_k: ([cli._SocketVerdict(node, value, "why")], evidence),
        )
        if verdict == "DEAD":
            receipts, _resolver, low = cli._require_dead_socket_nodes({tmp_path: "slot"}, budget)
            assert [receipt["verdict"] for receipt in receipts] == ["DEAD"] and low == {7}
        else:
            with pytest.raises(cli.Refusal, match=f"not proven dead: .*: {verdict}: why"):
                cli._require_dead_socket_nodes({tmp_path: "slot"}, budget)


def _unshare_holder(code: str, *args: str) -> subprocess.Popen[str]:
    process = subprocess.Popen(
        ["/usr/bin/unshare", "--user", "--map-root-user", "--net", "--", sys.executable, "-I", "-c", code, *args],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, cwd="/",
    )
    assert process.stdout is not None
    if process.stdout.readline() != "ready\n":
        _stdout, stderr = process.communicate("q", timeout=10)
        if process.returncode and re.fullmatch(
            r"unshare: (unshare failed|write failed /proc/self/uid_map): "
            r"(Operation not permitted|Permission denied)\n", stderr,
        ):
            pytest.skip(f"no user and network namespace for this test: {stderr.strip()}")
        pytest.fail(f"namespace holder failed: rc={process.returncode} stderr={stderr!r}")
    return process


_HOLDER = textwrap.dedent('''
    import os, socket, sys
    os.chdir(sys.argv[1])
    sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    # A relative name: the holder's table names no path this proof could match.
    sock.bind("held")
    print("ready", flush=True)
    sys.stdin.read(1)
    sock.close()
    print("closed", flush=True)
    sys.stdin.read(1)
''')


def _release(holder: subprocess.Popen[str]) -> None:
    assert holder.stdin is not None and holder.stdout is not None
    holder.stdin.write("c")
    holder.stdin.flush()
    assert holder.stdout.readline() == "closed\n"


@pytest.mark.parametrize("enterable", [True, False])
def test_proof_socket_held_only_in_another_network_namespace(
    tmp_path: Path, socket_world: set[int], monkeypatch: pytest.MonkeyPatch, enterable: bool,
) -> None:
    """The PrivateNetwork-style case: NOT DEAD while held, DEAD once released."""

    _dead_node(tmp_path / "dead")
    holder = _unshare_holder(_HOLDER, str(tmp_path))
    try:
        socket_world.add(holder.pid)
        theirs = os.stat(f"/proc/{holder.pid}/ns/net").st_ino
        assert theirs != sockdiag.own_netns()
        if not enterable:
            def refuse(_fd: int, netns: int, _deadline: float) -> sockdiag.Dump:
                raise sockdiag.DumpError(f"cannot enter network namespace {netns}: [Errno 1] Operation not permitted")

            monkeypatch.setattr(cli, "_dump_entered_namespace", refuse)
        verdicts, evidence = cli._prove_socket_nodes(
            {tmp_path: "slot"}, deadline=time.monotonic() + 60, probe=True, allow_calibration=True,
        )
        by_name = {verdict.node.path.name: verdict for verdict in verdicts}
        assert by_name["held"].verdict == "LIVE"
        if enterable:
            assert theirs in evidence.dumps and f"network namespace {theirs}" in by_name["held"].reason
            assert by_name["dead"].verdict == "DEAD" and by_name["dead"].gaps_overridden == ()
        else:
            # Only the seqpacket probe can see it: EPROTOTYPE, never DEAD.
            assert theirs in evidence.unenterable
            assert "seqpacket connect() found a bound socket of another type" in by_name["held"].reason
            assert by_name["dead"].verdict == "DEAD"
            assert any("cannot be entered" in gap for gap in by_name["dead"].gaps_overridden)
            receipt = by_name["dead"].receipt(evidence)
            assert any(
                str(line).startswith("gap overridden by dual-type probe: network namespace")
                for line in cast(list[object], receipt["gaps_overridden"])
            )
            budget = cli._ReadOnlyCommandBudget.start(timeout_seconds=60, stdout_limit=4096, stderr_limit=4096)
            monkeypatch.setattr(cli, "_run_root_owned_command", lambda *_a, **_k: (1, b"", b"stubbed"))
            with pytest.raises(cli.Refusal, match=r"not proven dead: .*/held: LIVE: "):
                cli._require_dead_socket_nodes({tmp_path: "slot"}, budget)
        _release(holder)
        released = {verdict.node.path.name: verdict for verdict in _prove(tmp_path)}
        assert {name: verdict.verdict for name, verdict in released.items()} == {"dead": "DEAD", "held": "DEAD"}
        if not enterable:
            assert all(verdict.gaps_overridden for verdict in released.values())
    finally:
        holder.communicate("q", timeout=10)


def test_failed_probe_self_test_keeps_a_gap_unknown(
    tmp_path: Path, socket_world: set[int], monkeypatch: pytest.MonkeyPatch,
) -> None:
    _dead_node(tmp_path / "dead")
    monkeypatch.setattr(cli, "_socket_probe_self_test", lambda *_a: (False, "rigged answer"))
    original = cli._socket_namespace_handles

    def with_gap(gaps: list[str], privileged: object = None) -> cli._NamespaceScan:
        gaps.append("same-UID process 9's network namespace cannot be read")
        return original(gaps, None)

    monkeypatch.setattr(cli, "_socket_namespace_handles", with_gap)
    verdicts, evidence = cli._prove_socket_nodes(
        {tmp_path: "slot"}, deadline=time.monotonic() + 60, probe=True, allow_calibration=True,
    )
    assert [verdict.verdict for verdict in verdicts] == ["UNKNOWN"]
    assert "self-test failed: rigged answer" in verdicts[0].reason
    assert verdicts[0].gaps_overridden == () and evidence.self_test == "failed: rigged answer"
    monkeypatch.setattr(cli, "_socket_probe_self_test", lambda *_a: (True, "ok"))
    verdicts, _evidence = cli._prove_socket_nodes(
        {tmp_path: "slot"}, deadline=time.monotonic() + 60, probe=True, allow_calibration=True,
    )
    assert [verdict.verdict for verdict in verdicts] == ["DEAD"]
    assert verdicts[0].gaps_overridden == ("same-UID process 9's network namespace cannot be read",)


def test_probe_self_test_passes_on_this_kernel_and_is_kept_per_boot(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(cli, "_SOCKET_SELF_TEST_CACHE", {})
    passed, detail = cli._socket_probe_self_test()
    if not passed and "could not be made" in detail:
        pytest.skip(f"no user and network namespace: {detail}")
    assert passed, detail
    assert detail == (
        "bound in another namespace: stream ECONNREFUSED/seqpacket EPROTOTYPE; "
        "released: stream ECONNREFUSED/seqpacket ECONNREFUSED"
    )
    monkeypatch.setattr(cli, "_run_socket_probe_self_test", lambda *_a: (False, "not run again"))
    assert cli._socket_probe_self_test() == (passed, detail)


@pytest.mark.parametrize("ending", ["pass", "rigged-failure", "holder-never-starts", "holder-dies"])
def test_probe_self_test_directory_is_0700_and_removed_on_every_path(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, request: pytest.FixtureRequest, ending: str,
) -> None:
    # A short parent, as /run/user/<uid> is: a socket path is limited to 108 bytes.
    parent = Path(tempfile.mkdtemp(prefix="wst-", dir="/tmp"))
    request.addfinalizer(lambda: shutil.rmtree(parent, ignore_errors=True))
    if ending == "rigged-failure":
        monkeypatch.setattr(cli, "_socket_connect_errno", lambda *_a: 0)
    elif ending == "holder-never-starts":
        monkeypatch.setattr(shutil, "which", lambda _name: str(tmp_path / "no-such-unshare"))
    elif ending == "holder-dies":
        monkeypatch.setattr(shutil, "which", lambda _name: "/bin/false")
    made: list[tuple[str, int]] = []
    original_mkdir = os.mkdir

    def mkdir(path: str | os.PathLike[str], mode: int = 0o777, **kwargs: int) -> None:
        original_mkdir(path, mode, **kwargs)
        if Path(path).parent == parent:
            made.append((Path(path).name, stat.S_IMODE(os.stat(path).st_mode)))

    monkeypatch.setattr(os, "mkdir", mkdir)
    passed, detail = cli._run_socket_probe_self_test(parent)
    assert list(parent.iterdir()) == [], detail
    assert len(made) == 1 and made[0][1] == 0o700
    assert cli._SOCKET_SELF_TEST_RE.fullmatch(made[0][0])
    if ending == "pass":
        if not passed and "could not be made" in detail:
            pytest.skip(f"no user and network namespace: {detail}")
        assert passed, detail
    else:
        assert not passed
        assert {
            "rigged-failure": "bound in another namespace: stream connected",
            "holder-never-starts": "could not be started",
            "holder-dies": "could not be made",
        }[ending] in detail


def test_probe_self_test_ignores_tmpdir_and_never_runs_inside_a_target(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    target = tmp_path / "target"
    (target / "tmp").mkdir(parents=True)
    monkeypatch.setenv("TMPDIR", str(target / "tmp"))
    chosen = cli._socket_self_test_parent()
    assert chosen in (Path(f"/run/user/{os.getuid()}"), Path("/tmp"))
    assert not os.path.realpath(chosen).startswith(os.path.realpath(target))
    monkeypatch.setattr(cli, "_socket_self_test_parent", lambda: target / "tmp")
    monkeypatch.setattr(cli, "_SOCKET_SELF_TEST_CACHE", {})

    def never(_parent: Path) -> tuple[bool, str]:
        raise AssertionError("the self-test ran inside a census target")

    monkeypatch.setattr(cli, "_run_socket_probe_self_test", never)
    passed, detail = cli._socket_probe_self_test((tmp_path / "other", target))
    assert not passed and f"is inside census target {target}" in detail
    assert cli._SOCKET_SELF_TEST_CACHE == {} and list((target / "tmp").iterdir()) == []


def test_probe_self_test_sweeps_only_stale_directories(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    ticks = cli._quiet_start_ticks(Path("/proc/self"))
    assert ticks is not None

    def leftover(name_ticks: int, tag: str, *, extra: bool = False) -> Path:
        directory = tmp_path / f"wrkslots-probe-selftest.{os.getpid()}.{name_ticks}.{tag * 16}"
        directory.mkdir(mode=0o700)
        os.mknod(directory / "s", stat.S_IFSOCK | 0o600)
        if extra:
            (directory / "keep").write_bytes(b"")
        return directory

    stale = leftover(ticks + 1, "a")
    alive = leftover(ticks, "b")
    foreign = leftover(ticks + 1, "c", extra=True)
    other = tmp_path / "wrkslots-probe-selftest-legacy"
    other.mkdir()
    monkeypatch.setattr(shutil, "which", lambda _name: str(tmp_path / "no-such-unshare"))
    passed, _detail = cli._run_socket_probe_self_test(tmp_path)
    assert not passed
    assert not stale.exists() and alive.exists() and foreign.exists() and other.exists()


def test_finish_journal_records_every_overridden_gap(tmp_path: Path) -> None:
    def receipt(slot: str, gap: str) -> dict[str, object]:
        return {"slot": slot, "verdict": "DEAD", "gaps_overridden": [gap]}

    receipts = tuple(
        receipt(str(tmp_path), f"gap overridden by dual-type probe: g{n}") for n in range(12)
    ) + (receipt("/elsewhere", "other"),)
    census = cli._ProcessPathCensus((), (), owner_cgroup_complete=False, socket_receipts=receipts)
    extension = cli._socket_proof_extension(census, tmp_path)
    assert extension is not None
    assert extension["nodes"] == 12 and extension["gaps_overridden_count"] == 12
    assert len(extension["gaps_overridden"]) == 8  # type: ignore[arg-type]
    assert cli._socket_proof_extension(census, tmp_path / "none") is None
    assert len(json.dumps(extension)) < cli._JOURNAL_EXTENSION_LIMIT_BYTES


def test_namespace_rounds_dump_a_late_namespace_and_refuse_endless_churn(
    socket_world: set[int], monkeypatch: pytest.MonkeyPatch,
) -> None:
    original = cli._socket_namespace_handles
    reads: list[int] = []

    def scan(gaps: list[str], privileged: object = None) -> cli._NamespaceScan:
        result = original(gaps, None)
        reads.append(1)
        for fake in range(1, len(reads) if endless else min(len(reads), 2)):
            result.handles.setdefault(10**12 + fake, [Path("/proc/self/ns/net")])
        return result

    monkeypatch.setattr(cli, "_socket_namespace_handles", scan)
    endless = False
    evidence = cli._collect_socket_evidence(deadline=time.monotonic() + 60)
    assert evidence.rounds == 2 and 10**12 + 1 in evidence.unenterable
    reads.clear()
    endless = True
    with pytest.raises(cli.Refusal, match="kept appearing after 4 rounds"):
        cli._collect_socket_evidence(deadline=time.monotonic() + 60)


def test_namespace_read_refuses_a_reused_pid(
    socket_world: set[int], monkeypatch: pytest.MonkeyPatch,
) -> None:
    original = cli._quiet_start_ticks
    seen: list[Path] = []

    def ticks(pid_dir: Path) -> int | None:
        value = original(pid_dir)
        if pid_dir == Path(f"/proc/{os.getpid()}"):
            seen.append(pid_dir)
            return None if value is None else value + (len(seen) > 1)
        return value

    monkeypatch.setattr(cli, "_quiet_start_ticks", ticks)
    with pytest.raises(cli.Refusal, match=f"PID {os.getpid()} was reused"):
        cli._collect_socket_evidence(deadline=time.monotonic() + 60)


def test_held_unix_socket_absent_from_every_dump_is_a_gap() -> None:
    left, right = socket.socketpair(socket.AF_UNIX, socket.SOCK_STREAM)
    tcp = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    try:
        held = {
            os.fstat(sock.fileno()).st_ino: f"/proc/self/fd/{sock.fileno()}" for sock in (left, tcp)
        }
        unix_inode = os.fstat(left.fileno()).st_ino
        empty: dict[int, sockdiag.Dump] = {1: sockdiag.Dump(1, (), 0, 0, 0.0)}
        gaps = cli._orphaned_held_unix_sockets(held, held, empty)
        assert len(gaps) == 1 and f"socket {unix_inode}" in gaps[0] and "1 held Unix" in gaps[0]
        listed = {1: sockdiag.Dump(1, (_bound(unix_inode, None),), 1, 0, 0.0)}
        assert cli._orphaned_held_unix_sockets(held, held, listed) == []
        # A socket held before but not after the dumps may have been made or closed meanwhile.
        assert cli._orphaned_held_unix_sockets(held, {}, empty) == []
    finally:
        for sock in (left, right, tcp):
            sock.close()


def test_calibration_matches_a_real_bound_socket_and_leaves_nothing(tmp_path: Path) -> None:
    with _bound_socket(tmp_path / "s") as live:
        node = _only_node(tmp_path)
        before = sorted(os.listdir(tmp_path))
        kernel_dev = cli._calibrate_socket_device(node, deadline=time.monotonic() + 30)
        dump = sockdiag.dump_own_namespace(deadline=time.monotonic() + 30)
        mine = [item for item in dump.sockets if item.sockfs_ino == os.fstat(live.fileno()).st_ino]
    assert kernel_dev is not None and mine and mine[0].vfs == (sockdiag.low32(node.st_ino), kernel_dev)
    assert sorted(os.listdir(tmp_path)) == before


def test_stale_calibration_directories_are_found_and_removed_only_when_stale(tmp_path: Path) -> None:
    ticks = cli._quiet_start_ticks(Path("/proc/self"))
    assert ticks is not None

    def calibration(name_ticks: int, tag: str, *, extra: bool = False) -> Path:
        directory = tmp_path / f".wrkslots-calib.{os.getpid()}.{name_ticks}.{tag * 16}"
        directory.mkdir()
        os.mknod(directory / "s", stat.S_IFSOCK | 0o600)
        if extra:
            (directory / "keep").write_bytes(b"")
        return directory

    stale = calibration(ticks + 1, "a")
    alive = calibration(ticks, "b")
    foreign = calibration(ticks + 1, "c", extra=True)
    (tmp_path / ".wrkslots-calib.not-a-match").mkdir()
    assert cli._stale_socket_calibrations(tmp_path) == [stale]
    assert cli._remove_stale_socket_calibrations(tmp_path) == [str(stale)]
    assert not stale.exists() and alive.exists() and foreign.exists()


def test_privileged_namespace_links_drop_a_batch_with_any_diagnostic(monkeypatch: pytest.MonkeyPatch) -> None:
    budget = cli._ReadOnlyCommandBudget.start(timeout_seconds=22, stdout_limit=4096, stderr_limit=4096)
    records = (
        b"/proc/7/ns/net\0net:[11]\0/proc/7/fd\0\0/proc/7/fd/3\0net:[12]\0/proc/7/fd/4\0socket:[9]\0"
        b"/proc/8/ns/net\0garbage\0"
    )
    for rc, stderr, expected in (
        (0, b"", {7: (11, {12})}),
        (1, b"", {}),
        (0, b"find: '/proc/9/fd': No such file or directory\n", {}),
    ):
        monkeypatch.setattr(cli, "_run_root_owned_command", lambda *_a, value=(rc, records, stderr), **_k: value)
        assert cli._privileged_namespace_links([7, 8], budget) == expected


def _census(paths: Sequence[Path], *extra: str, tmp_path: Path) -> tuple[int, dict[str, object], Path]:
    stub = tmp_path / "stubbin"
    stub.mkdir(exist_ok=True)
    log = tmp_path / "sudo-calls.log"
    (stub / "sudo").write_text(f"#!/bin/sh\necho \"$@\" >> {log}\nexit 1\n")
    (stub / "sudo").chmod(0o755)
    env = dict(os.environ, PATH=f"{stub}:{os.environ.get('PATH', '/usr/bin:/bin')}",
               PYTHONPATH=str(Path(cli.__file__).resolve().parents[1]))
    run = subprocess.run(
        [sys.executable, "-m", "wrkslots", "socket-census", "--format", "json", *extra, *map(str, paths)],
        capture_output=True, text=True, env=env, timeout=300, check=False,
    )
    return run.returncode, (json.loads(run.stdout) if run.stdout else {"stderr": run.stderr}), log


def _census_verdicts(report: dict[str, object]) -> dict[str, object]:
    nodes = report["nodes"]
    assert isinstance(nodes, list)
    return {Path(str(node["path"])).name: node["verdict"] for node in nodes}


@pytest.mark.load_quarantine  # #1267 wrkslots-load-robust
def test_socket_census_is_read_only_passive_and_runs_no_sudo(tmp_path: Path) -> None:
    tree = tmp_path / "tree"
    tree.mkdir()
    _dead_node(tree / "dead")
    with _bound_socket(tree / "live", listen=True):
        before = sorted(os.listdir(tree))
        rc, report, log = _census([tree], tmp_path=tmp_path)
        assert sorted(os.listdir(tree)) == before
        verdicts = _census_verdicts(report)
        assert verdicts["live"] == "LIVE" and verdicts["dead"] in ("NO-OWNER-EVIDENCE", "UNKNOWN")
        assert report["mode"] == "passive" and report["counts"]["DEAD"] == 0  # type: ignore[index]
        assert rc == 1 and report["dumps"] and not log.exists()
        rc, report, log = _census([tree], "--probe", tmp_path=tmp_path)
        assert sorted(os.listdir(tree)) == before and not log.exists()
        verdicts = _census_verdicts(report)
        assert verdicts["live"] == "LIVE" and verdicts["dead"] in ("DEAD", "UNKNOWN")


def test_socket_census_takes_no_project_root(tmp_path: Path) -> None:
    run = subprocess.run(
        [sys.executable, "-m", "wrkslots", "--project-root", str(tmp_path), "socket-census", str(tmp_path)],
        capture_output=True, text=True, timeout=60, check=False,
        env=dict(os.environ, PYTHONPATH=str(Path(cli.__file__).resolve().parents[1])),
    )
    assert run.returncode == 2 and "takes no --project-root" in run.stderr
