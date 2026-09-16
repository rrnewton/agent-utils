"""Ownership samples must distinguish a vanished proc inode from a live PID."""

from __future__ import annotations

import contextlib
import dataclasses
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
