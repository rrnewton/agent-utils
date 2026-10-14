"""A zombie thread-group leader whose threads still run holds what they hold.

When a process's main thread calls pthread_exit, the leader is left in State Z
while its other threads keep running.  The leader's own cwd, fd, maps and
mountinfo then show nothing, /proc readdir never lists the survivors, and
lsof +D reports nothing (measured 2026-10-14).  Every census here must still
report the slot such a survivor uses.  These tests run a real child whose
main thread calls pthread_exit while another thread keeps its working
directory or an open descriptor inside the slot, and they run each census
over the real /proc entries of that child alone: the proc root handed to the
walks holds only a link to the leader, exactly the view readdir gives.
"""

from __future__ import annotations

import os
import re
import subprocess
import sys
import time
from collections.abc import Iterator
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path

import pytest

from wrkslots import cli


# argv: slot, then what the surviving thread holds inside it: cwd, fd or both,
# then optionally "nodump".  The main thread waits for one line on stdin, makes
# the process non-dumpable if asked (so this user may no longer read its
# links), and then calls pthread_exit.
_ZOMBIE_LEADER_CHILD = """
import ctypes, os, sys, threading, time

slot, hold = sys.argv[1], sys.argv[2]
PR_SET_DUMPABLE = 4

def survive():
    if hold in ("cwd", "both"):
        os.chdir(slot)
    fd = os.open(os.path.join(slot, "held"), os.O_RDONLY) if hold in ("fd", "both") else -1
    sys.stdout.write(f"{threading.get_native_id()} {fd}\\n")
    sys.stdout.flush()
    while True:
        time.sleep(60)

threading.Thread(target=survive, daemon=True).start()
sys.stdin.readline()
if sys.argv[3:] == ["nodump"]:
    ctypes.CDLL(None).prctl(PR_SET_DUMPABLE, 0, 0, 0, 0)
ctypes.CDLL(None).pthread_exit(None)
"""


@dataclass(frozen=True)
class _Child:
    leader: int
    thread: int
    fd: int
    slot: Path
    proc_root: Path
    """A proc root that lists only the leader, as /proc readdir shows the group."""


def _state(pid: int) -> str:
    stat = (Path("/proc") / str(pid) / "stat").read_bytes()
    return stat.rsplit(b") ", 1)[1].split()[0].decode("ascii")


def _wait_for_state(pid: int, state: str) -> None:
    deadline = time.monotonic() + 10
    while _state(pid) != state:
        assert time.monotonic() < deadline, f"PID {pid} never reached state {state}"
        time.sleep(0.01)


def _status_field(pid: int, name: str) -> str:
    status = (Path("/proc") / str(pid) / "status").read_text(encoding="ascii", errors="replace")
    (value,) = (line.split()[1] for line in status.split("\n") if line.startswith(f"{name}:"))
    return value


def _budget() -> cli._ReadOnlyCommandBudget:
    return cli._ReadOnlyCommandBudget.start(
        timeout_seconds=60,
        stdout_limit=16 * 1024 * 1024,
        stderr_limit=64 * 1024,
        input_limit=cli._MOUNTINFO_CENSUS_BYTES_LIMIT,
    )


def _observation(pid: int) -> cli._AbsentProcessObservation:
    pid_dir = Path("/proc") / str(pid)
    ticks = cli._process_start_ticks(pid_dir)
    namespace = cli._mount_namespace(pid_dir)
    assert ticks is not None and namespace is not None
    return cli._AbsentProcessObservation(pid, ticks, "", namespace)


def _slot(tmp_path: Path) -> Path:
    slot = tmp_path / "slot"
    slot.mkdir()
    (slot / "held").write_bytes(b"held by the surviving thread\n")
    return slot


@contextmanager
def _started(
    slot: Path, hold: str, *extra: str,
) -> Iterator[subprocess.Popen[str]]:
    child = subprocess.Popen(
        [sys.executable, "-I", "-c", _ZOMBIE_LEADER_CHILD, str(slot), hold, *extra],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
        cwd="/",
    )
    try:
        yield child
    finally:
        child.kill()
        child.wait(timeout=10)


def _exit_leader(child: subprocess.Popen[str]) -> None:
    assert child.stdin is not None
    child.stdin.write("exit leader\n")
    child.stdin.flush()
    _wait_for_state(child.pid, "Z")


def _ready(child: subprocess.Popen[str]) -> tuple[int, int]:
    assert child.stdout is not None
    thread, fd = (int(field) for field in child.stdout.readline().split())
    return thread, fd


@contextmanager
def _zombie_leader(tmp_path: Path, hold: str, *extra: str) -> Iterator[_Child]:
    """Yield a child whose leader is a zombie and whose thread holds ``hold``."""

    slot = _slot(tmp_path)
    with _started(slot, hold, *extra) as child:
        thread, fd = _ready(child)
        leader_ticks = cli._process_start_ticks(Path("/proc") / str(child.pid))
        _exit_leader(child)
        # The same generation, now a zombie: not a reaped and reused PID.
        assert cli._process_start_ticks(Path("/proc") / str(child.pid)) == leader_ticks
        proc_root = tmp_path / "proc"
        proc_root.mkdir()
        (proc_root / str(child.pid)).symlink_to(Path("/proc") / str(child.pid))
        yield _Child(child.pid, thread, fd, slot, proc_root)


def _use_of(child: _Child, hold: str) -> str:
    """The refusal of a walk that found the surviving thread's ``hold``."""

    return rf"^live process {child.thread} uses slot {re.escape(str(child.slot))}: {hold}[=/]"


def test_the_premise_a_zombie_leader_shows_nothing_its_thread_holds(tmp_path: Path) -> None:
    """What every census used to see: nothing at the leader, no thread to visit."""

    with _zombie_leader(tmp_path, "both") as child:
        leader = Path("/proc") / str(child.leader)
        survivor = leader / "task" / str(child.thread)
        assert _status_field(child.leader, "State") == "Z"
        assert int(_status_field(child.leader, "Threads")) == 2
        assert str(child.thread) not in os.listdir("/proc")
        for name in ("cwd", "root", "exe", "ns/mnt"):
            with pytest.raises(FileNotFoundError):
                os.readlink(leader / name)
        # Its fd directory belongs to root once its memory is gone: EACCES
        # here, and empty even to a reader allowed in.
        try:
            assert os.listdir(leader / "fd") == []
        except PermissionError:
            pass
        assert (leader / "maps").read_bytes() == b""
        assert cli._read_process_mountinfo(leader) is None
        # The surviving thread holds both, and /proc/TID is the same task.
        assert os.readlink(survivor / "cwd") == str(child.slot)
        assert os.readlink(survivor / "fd" / str(child.fd)) == str(child.slot / "held")
        top_level = Path("/proc") / str(child.thread)
        assert os.readlink(top_level / "cwd") == os.readlink(survivor / "cwd")
        assert os.readlink(top_level / "fd" / str(child.fd)) == str(child.slot / "held")


def test_the_zombie_leader_stands_for_its_running_thread(tmp_path: Path) -> None:
    with _zombie_leader(tmp_path, "both") as child:
        leader = Path("/proc") / str(child.leader)
        survivor = leader / "task" / str(child.thread)
        assert cli._zombie_leader_thread_dirs(leader) == (survivor,)
        assert cli._zombie_leader_has_live_threads(leader)
        assert cli._walked_zombie_leader_threads(leader) == (survivor,)
        # Only a leader stands for its threads; a thread names its whole group.
        assert cli._zombie_leader_thread_dirs(Path("/proc") / str(child.thread)) == ()
        assert cli._walked_zombie_leader_threads(survivor) == ()


@pytest.mark.parametrize("lane", ("privileged", "same-uid"))
def test_each_snapshot_observes_the_thread_in_its_leader_place(
    tmp_path: Path, lane: str,
) -> None:
    with _zombie_leader(tmp_path, "both") as child:
        snapshot = (
            cli._absent_validate_process_snapshot(child.proc_root, include_owner_cgroups=False)
            if lane == "privileged"
            else cli._same_uid_process_observations(_budget(), child.proc_root)
        )
        assert [process.pid for process in snapshot] == [child.thread]
        assert snapshot[0].start_ticks == cli._process_start_ticks(
            Path("/proc") / str(child.thread)
        )


@pytest.mark.parametrize("hold", ("cwd", "fd"))
def test_the_same_uid_census_reports_the_slot_the_thread_holds(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, hold: str,
) -> None:
    with _zombie_leader(tmp_path, hold) as child:
        real = cli._same_uid_process_observations
        monkeypatch.setattr(
            cli, "_same_uid_process_observations",
            lambda budget: real(budget, child.proc_root),
        )
        census = cli._capture_same_uid_process_path_census((child.slot,), budget=_budget())
    assert [process.pid for process in census.processes] == [child.thread]
    assert {(pid, slot) for pid, slot, _kind, _detail in census.matches} == {
        (child.thread, str(child.slot))
    }
    # A link match names the link's target: the slot itself for the cwd, the
    # held file for the descriptor.
    held = child.slot if hold == "cwd" else child.slot / "held"
    assert {(kind, detail) for _pid, _slot, kind, detail in census.matches} == {
        ("link", str(held))
    }


@pytest.mark.parametrize("hold", ("cwd", "fd"))
def test_the_privileged_census_reports_the_slot_the_thread_holds(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, hold: str,
) -> None:
    """The all-process census with sudo stood in: these entries are this user's."""

    with _zombie_leader(tmp_path, hold) as child:
        real = cli._absent_validate_process_snapshot
        monkeypatch.setattr(
            cli, "_absent_validate_process_snapshot",
            lambda include_owner_cgroups=True: real(
                child.proc_root, include_owner_cgroups=include_owner_cgroups
            ),
        )
        monkeypatch.setattr(
            cli, "_run_root_owned_command",
            lambda program, arguments, *, budget=None, input_data=None, **_kwargs: (
                cli._run_same_uid_command(
                    program, arguments, budget=budget or _budget(), input_data=input_data
                )
            ),
        )
        census = cli._capture_process_path_census(
            (child.slot,), budget=_budget(), include_owner_cgroups=False
        )
    assert [process.pid for process in census.processes] == [child.thread]
    assert {(pid, slot) for pid, slot, _kind, _detail in census.matches} == {
        (child.thread, str(child.slot))
    }


def test_a_leader_that_exits_under_running_threads_discards_the_attempt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """An observed leader is neither live nor released once only its threads run.

    The first attempt holds the leader as it was observed before it exited;
    the census discards that attempt, and its fresh snapshot finds the thread.
    """

    slot = _slot(tmp_path)
    with _started(slot, "fd") as child:
        thread, _fd = _ready(child)
        observed = _observation(child.pid)
        assert cli._process_observation_is_active(observed)
        _exit_leader(child)

        with pytest.raises(cli._ProcessEvidenceChanged, match="threads still run"):
            cli._process_observation_is_active(observed)
        with pytest.raises(cli._ProcessEvidenceChanged, match="threads still run"):
            cli._preflight_process_observations((observed,), _budget())

        proc_root = tmp_path / "proc"
        proc_root.mkdir()
        (proc_root / str(child.pid)).symlink_to(Path("/proc") / str(child.pid))
        real = cli._same_uid_process_observations
        snapshots: list[tuple[int, ...]] = []

        def stale_then_fresh(
            budget: cli._ReadOnlyCommandBudget,
        ) -> tuple[cli._AbsentProcessObservation, ...]:
            fresh = (observed,) if not snapshots else real(budget, proc_root)
            snapshots.append(tuple(process.pid for process in fresh))
            return fresh

        monkeypatch.setattr(cli, "_same_uid_process_observations", stale_then_fresh)
        census = cli._capture_same_uid_process_path_census((slot,), budget=_budget())
    assert snapshots == [(child.pid,), (thread,)]
    assert {(pid, slot_name) for pid, slot_name, _kind, _detail in census.matches} == {
        (thread, str(slot))
    }


@pytest.mark.parametrize("use_lsof", (True, False))
@pytest.mark.parametrize("hold", ("cwd", "fd"))
def test_the_slot_use_walk_reports_the_slot_the_thread_holds(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, use_lsof: bool, hold: str,
) -> None:
    """lsof proves only direct absence among the processes it lists.

    lsof +D, run for real over this child, reported nothing (exit 1, no
    output; lsof 4.94, 2026-10-14), so the stub returns exactly that.  A full
    host lsof scan takes seconds, which is why it is not repeated here.
    """

    if use_lsof and not any(
        candidate.is_file() and os.access(candidate, os.X_OK)
        for candidate in (Path("/usr/bin/lsof"), Path("/usr/sbin/lsof"))
    ):
        pytest.skip("the lsof branch needs an lsof executable")
    scans: list[Path] = []

    def lsof_reports_nothing(
        _executable: Path, slot_path: Path,
    ) -> tuple[cli._LsofUseDiagnostic | None, bool, int]:
        scans.append(slot_path)
        return None, False, 1

    monkeypatch.setattr(cli, "_lsof_slot_use", lsof_reports_nothing)
    with _zombie_leader(tmp_path, hold) as child:
        with pytest.raises(cli.Refusal, match=_use_of(child, hold)):
            cli._assert_slot_unused(child.slot, use_lsof=use_lsof, proc_root=child.proc_root)
    assert scans == ([child.slot] if use_lsof else [])


def test_a_thread_this_user_may_not_read_is_no_more_evidence_than_to_lsof(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """Another user's surviving thread cannot make every slot-use check refuse.

    lsof run as this user reads no link of a process this user may not read,
    and the walk after it holds the threads it never listed to the same rule:
    such a link is no evidence, and the thread's mount table still is.  The
    child here makes itself non-dumpable, which hides its links from its own
    user exactly as another user's are hidden.
    """

    monkeypatch.setattr(cli, "_lsof_slot_use", lambda _executable, _slot: (None, False, 1))
    mount_reads: list[Path] = []
    real_uses = cli._MountTableVerdicts.uses

    def recorded(self: cli._MountTableVerdicts, pid_dir: Path) -> list[str]:
        mount_reads.append(pid_dir)
        return real_uses(self, pid_dir)

    monkeypatch.setattr(cli._MountTableVerdicts, "uses", recorded)
    with _zombie_leader(tmp_path, "cwd", "nodump") as child:
        survivor = child.proc_root / str(child.leader) / "task" / str(child.thread)
        try:
            os.readlink(survivor / "cwd")
        except PermissionError:
            pass
        else:
            pytest.skip("this user may read a non-dumpable task's links")
        if not any(
            candidate.is_file() and os.access(candidate, os.X_OK)
            for candidate in (Path("/usr/bin/lsof"), Path("/usr/sbin/lsof"))
        ):
            pytest.skip("the lsof branch needs an lsof executable")
        cli._assert_slot_unused(child.slot, proc_root=child.proc_root)
    assert survivor in mount_reads


@pytest.mark.parametrize("hold", ("cwd", "fd"))
def test_the_agent_batch_scans_report_the_slot_the_thread_holds(
    tmp_path: Path, hold: str,
) -> None:
    with _zombie_leader(tmp_path, hold) as child:
        with pytest.raises(cli.Refusal, match=_use_of(child, hold)):
            cli._agent_batch_fresh_use(
                child.slot, None, capture_generation=False, proc_root=child.proc_root
            )
        fresh = cli._agent_batch_fresh_use(
            child.slot, None, capture_generation=True, proc_root=child.proc_root
        )
        assert isinstance(fresh, cli._LiveUseObservation)
        assert fresh.pid == child.thread
        assert fresh.start_ticks == cli._process_start_ticks(Path("/proc") / str(child.thread))

        fenced = cli._agent_batch_fenced_use(
            child.slot, child.slot, None, capture_generation=True, proc_root=child.proc_root
        )
        if isinstance(fenced, cli._UndecidedFencedUse) and "filesystem" in fenced.reason:
            pytest.skip(f"the fenced scan does not compare this filesystem: {fenced.reason}")
        assert isinstance(fenced, cli._LiveUseObservation), fenced
        assert fenced.pid == child.thread
        assert re.match(_use_of(child, hold), str(fenced.refusal))


def test_a_whole_process_zombie_and_a_live_leader_stand_for_nothing(tmp_path: Path) -> None:
    """The negative controls: no thread hides behind either."""

    exited = subprocess.Popen([sys.executable, "-I", "-c", "pass"])
    try:
        _wait_for_state(exited.pid, "Z")
        pid_dir = Path("/proc") / str(exited.pid)
        assert cli._zombie_leader_thread_dirs(pid_dir) == ()
        assert not cli._zombie_leader_has_live_threads(pid_dir)
        assert cli._walked_zombie_leader_threads(pid_dir) == ()
        observed = cli._AbsentProcessObservation(
            exited.pid, cli._process_start_ticks(pid_dir) or 0, "", "mnt:[0]"
        )
        assert not cli._process_observation_is_active(observed)
        assert cli._preflight_process_observations((observed,), _budget()) == ()
        proc_root = tmp_path / "proc"
        proc_root.mkdir()
        (proc_root / str(exited.pid)).symlink_to(pid_dir)
        assert cli._same_uid_process_observations(_budget(), proc_root) == ()
    finally:
        exited.wait(timeout=10)

    # A running leader with a running thread answers for itself: the walks
    # add nothing, and its own entries show the slot.
    (tmp_path / "live").mkdir()
    slot = _slot(tmp_path / "live")
    with _started(slot, "both") as child:
        thread, _fd = _ready(child)
        pid_dir = Path("/proc") / str(child.pid)
        assert int(_status_field(child.pid, "Threads")) == 2
        assert thread != child.pid
        assert cli._walked_zombie_leader_threads(pid_dir) == ()
        assert os.readlink(pid_dir / "cwd") == str(slot)
