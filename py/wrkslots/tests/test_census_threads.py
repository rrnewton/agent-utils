"""A zombie thread-group leader whose threads still run holds what they hold.

When a process's main thread calls pthread_exit, the leader is left in State Z
while its other threads keep running.  The leader's own cwd, fd, maps and
mountinfo then show nothing, /proc readdir never lists the survivors, and
lsof +D reports nothing (measured 2026-10-14).  Every census here must still
report the slot such a survivor uses.  These tests run a real child whose
main thread calls pthread_exit while another thread keeps its working
directory, an open descriptor or a mapping inside the slot, and they run each
census over the real /proc entries of that child: the proc root handed to the
walks holds links to the leader (and, where a test needs one, to this test
process), exactly the view readdir gives.
"""

from __future__ import annotations

import errno
import os
import re
import subprocess
import sys
import threading
import time
from collections.abc import Callable, Iterator, Sequence
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path

import pytest

from wrkslots import cli


# argv: slot, then what the surviving thread holds inside it (cwd, fd, both,
# or map: a mapping whose descriptor it closed), then optionally "nodump".
# The main thread waits for one line on stdin, makes the process
# non-dumpable if asked (so this user may no longer read its links), and
# then calls pthread_exit.  The surviving thread then reads stdin: "exec"
# makes it execute a new program, which the kernel runs under the leader's
# PID with the leader's start ticks, keeping the thread's working directory.
_ZOMBIE_LEADER_CHILD = """
import ctypes, os, sys, threading, time

slot, hold = sys.argv[1], sys.argv[2]
PR_SET_DUMPABLE = 4
PROT_READ, MAP_PRIVATE = 1, 2
leader_exiting = threading.Event()
libc = ctypes.CDLL(None, use_errno=True)
libc.mmap.restype = ctypes.c_void_p
libc.mmap.argtypes = (
    ctypes.c_void_p, ctypes.c_size_t, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_long,
)

def survive():
    if hold in ("cwd", "both"):
        os.chdir(slot)
    fd = os.open(os.path.join(slot, "held"), os.O_RDONLY) if hold in ("fd", "both") else -1
    if hold == "map":
        # libc's mmap, not Python's, which keeps a duplicate descriptor.
        mapped = os.open(os.path.join(slot, "held"), os.O_RDONLY)
        address = libc.mmap(None, 4096, PROT_READ, MAP_PRIVATE, mapped, 0)
        assert address not in (None, ctypes.c_void_p(-1).value), ctypes.get_errno()
        os.close(mapped)
    sys.stdout.write(f"{threading.get_native_id()} {fd}\\n")
    sys.stdout.flush()
    leader_exiting.wait()
    for line in sys.stdin:
        if line == "exec\\n":
            program = "import time\\nwhile True:\\n    time.sleep(60)\\n"
            os.execv(sys.executable, [sys.executable, "-I", "-c", program])
    while True:
        time.sleep(60)

threading.Thread(target=survive, daemon=True).start()
sys.stdin.readline()
if sys.argv[3:] == ["nodump"]:
    ctypes.CDLL(None).prctl(PR_SET_DUMPABLE, 0, 0, 0, 0)
leader_exiting.set()
ctypes.CDLL(None).pthread_exit(None)
"""

# The censuses that run find and grep trust them only as root-owned
# executables in the initial identity user namespace; in the repository's
# mapped-root namespace /usr/bin/find is not root-owned and they refuse.
_REAL_CENSUS_COMMANDS = pytest.mark.skipif(
    not cli._has_identity_user_namespace(),
    reason="the census commands are trusted only in the initial identity user namespace",
)


@dataclass(frozen=True)
class _Child:
    leader: int
    thread: int
    fd: int
    slot: Path
    proc_root: Path
    """A proc root that lists only the leader, as /proc readdir shows the group."""
    process: subprocess.Popen[str]


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


def _proc_root(tmp_path: Path, *pids: int) -> Path:
    proc_root = tmp_path / "proc"
    proc_root.mkdir()
    for pid in pids:
        (proc_root / str(pid)).symlink_to(Path("/proc") / str(pid))
    return proc_root


@contextmanager
def _started(
    slot: Path, hold: str, *extra: str,
) -> Iterator[subprocess.Popen[str]]:
    # As a context manager, Popen closes both pipes on the way out. Killing and reaping alone
    # leaves them to the Popen object's finalizer, and a test whose frame a reference cycle keeps
    # alive (pytest.raises(...) as caught) then holds both descriptors until a full garbage
    # collection, which may run inside a later test that compares descriptor snapshots.
    with subprocess.Popen(
        [sys.executable, "-I", "-c", _ZOMBIE_LEADER_CHILD, str(slot), hold, *extra],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
        cwd="/",
    ) as child:
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


def _exec_from_thread(child: subprocess.Popen[str], thread: int) -> None:
    """Make the surviving thread execute a new program, and wait until it runs."""

    assert child.stdin is not None
    child.stdin.write("exec\n")
    child.stdin.flush()
    deadline = time.monotonic() + 10
    while (Path("/proc") / str(thread)).exists() or _state(child.pid) in {"Z", "X"}:
        assert time.monotonic() < deadline, "the surviving thread never executed"
        time.sleep(0.01)


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
        proc_root = _proc_root(tmp_path, child.pid)
        yield _Child(child.pid, thread, fd, slot, proc_root, child)


def _use_of(child: _Child, hold: str) -> str:
    """The refusal of a walk that found the surviving thread's ``hold``."""

    return (
        rf"^live thread {child.thread} of process {child.leader} "
        rf"\(whose main thread has exited\) uses slot {re.escape(str(child.slot))}: {hold}[=/]"
    )


def _require_lsof() -> None:
    if not any(
        candidate.is_file() and os.access(candidate, os.X_OK)
        for candidate in (Path("/usr/bin/lsof"), Path("/usr/sbin/lsof"))
    ):
        pytest.skip("the lsof branch needs an lsof executable")


def _lsof_reports_nothing(
    _executable: Path, _slot_path: Path,
) -> tuple[cli._LsofUseDiagnostic | None, bool, int]:
    """lsof +D over such a child, measured: no output, exit 1 (lsof 4.94)."""

    return None, False, 1


def _root_owned_as_same_uid(
    hook: Callable[[Path], None] = lambda _program: None,
) -> Callable[[Path, Sequence[str]], tuple[int, bytes, bytes]]:
    """Stand in for sudo: these entries are this user's, so this user's find reads them."""

    def run(
        program: Path,
        arguments: Sequence[str],
        *,
        input_data: bytes | None = None,
        input_limit: int = 1024 * 1024,
        budget: cli._ReadOnlyCommandBudget | None = None,
    ) -> tuple[int, bytes, bytes]:
        del input_limit  # this user's command takes the batch budget's bound
        hook(program)
        return cli._run_same_uid_command(
            program, arguments, budget=budget or _budget(), input_data=input_data
        )

    return run


def test_started_closes_both_pipes_however_long_its_popen_lives(tmp_path: Path) -> None:
    """A frame a reference cycle keeps alive keeps its Popen; the pipes must not stay open with it.

    `pytest.raises(...) as caught` is such a cycle. Pipes left to the Popen finalizer stay open
    until a full garbage collection, which may run inside a later test that compares descriptor
    snapshots. `#360 census-fd-snapshot`.
    """
    with _started(_slot(tmp_path), "cwd") as child:
        _ready(child)
    assert child.stdin is not None and child.stdin.closed
    assert child.stdout is not None and child.stdout.closed


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


def _snapshot(lane: str, proc_root: Path) -> tuple[cli._AbsentProcessObservation, ...]:
    if lane == "privileged":
        return cli._absent_validate_process_snapshot(proc_root, include_owner_cgroups=False)
    return cli._same_uid_process_observations(_budget(), proc_root)


@pytest.mark.parametrize("lane", ("privileged", "same-uid"))
def test_each_snapshot_observes_the_thread_in_its_leader_place(
    tmp_path: Path, lane: str,
) -> None:
    with _zombie_leader(tmp_path, "both") as child:
        leader_ticks = cli._process_start_ticks(Path("/proc") / str(child.leader))
        snapshot = _snapshot(lane, child.proc_root)
        assert [process.pid for process in snapshot] == [child.thread]
        assert snapshot[0].start_ticks == cli._process_start_ticks(
            Path("/proc") / str(child.thread)
        )
        # The thread carries its leader's identity, for the exec check.
        assert (snapshot[0].leader_pid, snapshot[0].leader_start_ticks) == (
            child.leader, leader_ticks
        )


@pytest.mark.parametrize("lane", ("privileged", "same-uid"))
def test_a_leader_that_exits_after_the_terminal_check_still_yields_its_thread(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, lane: str,
) -> None:
    """The second place each snapshot adds threads: a leader that exits between
    its terminal check and its mount-namespace read, which then reads absent."""

    with _zombie_leader(tmp_path, "both") as child:
        leader_dir = child.proc_root / str(child.leader)
        real = cli._process_generation_is_terminal
        raced: list[Path] = []

        def alive_once(pid_dir: Path, generation: cli._ProcessStat) -> bool:
            if pid_dir == leader_dir and not raced:
                raced.append(pid_dir)
                return False
            return real(pid_dir, generation)

        monkeypatch.setattr(cli, "_process_generation_is_terminal", alive_once)
        snapshot = _snapshot(lane, child.proc_root)
        assert raced == [leader_dir]
        assert [(process.pid, process.leader_pid) for process in snapshot] == [
            (child.thread, child.leader)
        ]


@pytest.mark.parametrize("lane", ("privileged", "same-uid"))
def test_an_unreadable_thread_listing_refuses_each_snapshot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, lane: str,
) -> None:
    with _zombie_leader(tmp_path, "cwd") as child:
        task = child.proc_root / str(child.leader) / "task"
        real = os.scandir

        def failing(path: int | str | os.PathLike[str] = ".") -> Iterator[os.DirEntry[str]]:
            # Other readers pass directory descriptors, which name no path.
            if not isinstance(path, int) and Path(path) == task:
                raise OSError(errno.EIO, "injected", str(path))
            return real(path)

        monkeypatch.setattr(os, "scandir", failing)
        with pytest.raises(cli.Refusal, match="thread group is indeterminate"):
            _snapshot(lane, child.proc_root)


@_REAL_CENSUS_COMMANDS
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
        # It names the thread, and as a thread of its process.
        with pytest.raises(cli.Refusal, match=_use_of(child, "link")):
            census.assert_slot_unused(child.slot, None)
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


@_REAL_CENSUS_COMMANDS
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
        monkeypatch.setattr(cli, "_run_root_owned_command", _root_owned_as_same_uid())
        census = cli._capture_process_path_census(
            (child.slot,), budget=_budget(), include_owner_cgroups=False
        )
    assert [process.pid for process in census.processes] == [child.thread]
    assert {(pid, slot) for pid, slot, _kind, _detail in census.matches} == {
        (child.thread, str(child.slot))
    }


@_REAL_CENSUS_COMMANDS
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
        with pytest.raises(cli._ProcessEvidenceChanged, match="threads still run"):
            cli._recheck_thread_group_observations((observed,), _budget())

        proc_root = _proc_root(tmp_path, child.pid)
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
    # The third inventory is the closing one: it adds no generation.
    assert snapshots == [(child.pid,), (thread,), (thread,)]
    assert {(pid, slot_name) for pid, slot_name, _kind, _detail in census.matches} == {
        (thread, str(slot))
    }


@_REAL_CENSUS_COMMANDS
@pytest.mark.parametrize("lane", ("privileged", "same-uid"))
def test_a_leader_that_exits_between_the_link_and_maps_readers_discards_the_attempt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, lane: str,
) -> None:
    """Exiting after find and before grep leaves no diagnostic to catch.

    The surviving thread holds only a mapping (its descriptor closed, its
    working directory /), so find has nothing to report while the leader
    lives, and grep then reads the zombie leader's maps as empty, without an
    error.  This test process, listed first and in the same mount namespace,
    is the one whose mount table the mount census reads, so that census never
    reads the leader either.  Only the check after every reader catches the
    exit, and the retry reports the mapping the thread holds.
    """

    slot = _slot(tmp_path)
    with _started(slot, "map") as child:
        thread, _fd = _ready(child)
        if os.getpid() > child.pid:
            pytest.skip("PIDs wrapped: this test process is not listed first")
        assert cli._mount_namespace(Path("/proc/self")) == cli._mount_namespace(
            Path("/proc") / str(child.pid)
        )
        proc_root = _proc_root(tmp_path, os.getpid(), child.pid)
        greps: list[Path] = []

        def exit_before_first_grep(program: Path) -> None:
            if program.name == "grep" and not greps:
                _exit_leader(child)
            if program.name == "grep":
                greps.append(program)

        snapshots: list[tuple[int, ...]] = []
        if lane == "same-uid":
            real_observations = cli._same_uid_process_observations

            def observations(
                budget: cli._ReadOnlyCommandBudget,
            ) -> tuple[cli._AbsentProcessObservation, ...]:
                taken = real_observations(budget, proc_root)
                snapshots.append(tuple(process.pid for process in taken))
                return taken

            real_run = cli._run_same_uid_command

            def run(
                program: Path,
                arguments: Sequence[str],
                *,
                budget: cli._ReadOnlyCommandBudget,
                input_data: bytes | None = None,
            ) -> tuple[int, bytes, bytes]:
                exit_before_first_grep(program)
                return real_run(program, arguments, budget=budget, input_data=input_data)

            monkeypatch.setattr(cli, "_same_uid_process_observations", observations)
            monkeypatch.setattr(cli, "_run_same_uid_command", run)
            census = cli._capture_same_uid_process_path_census((slot,), budget=_budget())
        else:
            real_snapshot = cli._absent_validate_process_snapshot

            def snapshot(
                include_owner_cgroups: bool = True,
            ) -> tuple[cli._AbsentProcessObservation, ...]:
                taken = real_snapshot(proc_root, include_owner_cgroups=include_owner_cgroups)
                snapshots.append(tuple(process.pid for process in taken))
                return taken

            monkeypatch.setattr(cli, "_absent_validate_process_snapshot", snapshot)
            monkeypatch.setattr(
                cli, "_run_root_owned_command", _root_owned_as_same_uid(exit_before_first_grep)
            )
            census = cli._capture_process_path_census(
                (slot,), budget=_budget(), include_owner_cgroups=False
            )
    # The same-UID census ends with its closing inventory, which adds nothing.
    closing = [(os.getpid(), thread)] if lane == "same-uid" else []
    assert snapshots == [(os.getpid(), child.pid), (os.getpid(), thread), *closing]
    assert len(greps) == 2
    assert {(pid, kind) for pid, _slot, kind, _detail in census.matches} == {(thread, "map")}


@_REAL_CENSUS_COMMANDS
@pytest.mark.parametrize("lane", ("privileged", "same-uid"))
def test_a_thread_that_executes_after_the_snapshot_discards_the_attempt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, lane: str,
) -> None:
    """execve from the thread moves it to its leader's PID and start ticks.

    The thread's own ID then vanishes from every reader while the new
    program, at the leader's PID, keeps the thread's working directory in the
    slot.  The attempt that observed the thread is discarded, and the retry
    reports the new program.
    """

    with _zombie_leader(tmp_path, "cwd") as child:
        if os.getpid() > child.leader:
            pytest.skip("PIDs wrapped: this test process is not listed first")
        (child.proc_root / str(os.getpid())).symlink_to(Path("/proc") / str(os.getpid()))
        snapshots: list[tuple[int, ...]] = []

        def then_execute(
            taken: tuple[cli._AbsentProcessObservation, ...],
        ) -> tuple[cli._AbsentProcessObservation, ...]:
            snapshots.append(tuple(process.pid for process in taken))
            if len(snapshots) == 1:
                _exec_from_thread(child.process, child.thread)
            return taken

        if lane == "same-uid":
            real_observations = cli._same_uid_process_observations
            monkeypatch.setattr(
                cli, "_same_uid_process_observations",
                lambda budget: then_execute(real_observations(budget, child.proc_root)),
            )
            census = cli._capture_same_uid_process_path_census(
                (child.slot,), budget=_budget()
            )
        else:
            real_snapshot = cli._absent_validate_process_snapshot
            monkeypatch.setattr(
                cli, "_absent_validate_process_snapshot",
                lambda include_owner_cgroups=True: then_execute(
                    real_snapshot(child.proc_root, include_owner_cgroups=include_owner_cgroups)
                ),
            )
            monkeypatch.setattr(cli, "_run_root_owned_command", _root_owned_as_same_uid())
            census = cli._capture_process_path_census(
                (child.slot,), budget=_budget(), include_owner_cgroups=False
            )
    closing = [(os.getpid(), child.leader)] if lane == "same-uid" else []
    assert snapshots == [
        (os.getpid(), child.thread), (os.getpid(), child.leader), *closing
    ]
    assert {(pid, slot, kind, detail) for pid, slot, kind, detail in census.matches} == {
        (child.leader, str(child.slot), "link", str(child.slot))
    }


def test_an_executed_thread_is_neither_vanished_nor_active(tmp_path: Path) -> None:
    with _zombie_leader(tmp_path, "cwd") as child:
        (observed,) = cli._same_uid_process_observations(_budget(), child.proc_root)
        assert cli._process_observation_is_active(observed)
        cli._recheck_thread_group_observations((observed,), _budget())
        _exec_from_thread(child.process, child.thread)
        with pytest.raises(cli._ProcessEvidenceChanged, match="executed a new program"):
            cli._process_observation_is_active(observed)
        with pytest.raises(cli._ProcessEvidenceChanged, match="executed a new program"):
            cli._recheck_thread_group_observations((observed,), _budget())


@pytest.mark.parametrize("program", ("find", "grep"))
@pytest.mark.parametrize("lane", ("privileged", "same-uid"))
def test_thread_group_motion_does_not_skip_validating_later_diagnostics(
    tmp_path: Path, program: str, lane: str,
) -> None:
    """A diagnostic after the one that found motion still refuses outright."""

    slot = _slot(tmp_path)
    with _started(slot, "cwd") as child:
        _ready(child)
        observed = _observation(child.pid)
        _exit_leader(child)
        vanished = (
            f"/usr/bin/find: '/proc/{child.pid}/cwd': No such file or directory"
            if program == "find"
            else f"/usr/bin/grep: /proc/{child.pid}/maps: No such file or directory"
        )
        stderr = f"{vanished}\n/usr/bin/{program}: something unexpected\n".encode("ascii")
        returncode = 1 if program == "find" else 2
        processes = {child.pid: observed}
        with pytest.raises(cli.Refusal, match="something unexpected") as refused:
            if lane == "privileged":
                cli._allow_only_vanished_process_diagnostics(
                    program, returncode, stderr, processes
                )
            else:
                cli._same_uid_indeterminate_processes(program, returncode, stderr, processes)
        assert not isinstance(refused.value, cli._ProcessEvidenceChanged)
        # Alone, the vanished line is motion, and the attempt is discarded.
        with pytest.raises(cli._ProcessEvidenceChanged, match="threads still run"):
            if lane == "privileged":
                cli._allow_only_vanished_process_diagnostics(
                    program, returncode, f"{vanished}\n".encode("ascii"), processes
                )
            else:
                cli._same_uid_indeterminate_processes(
                    program, returncode, f"{vanished}\n".encode("ascii"), processes
                )


def test_the_lsof_fallback_refuses_a_thread_it_cannot_see(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """lsof -p TID prints nothing for a thread (lsof 4.94), which reads as no use."""

    def unexpected(*_args: object, **_kwargs: object) -> tuple[int, bytes, bytes]:
        raise AssertionError("lsof must not run for a thread")

    monkeypatch.setattr(cli, "_run_same_uid_command", unexpected)
    thread = cli._AbsentProcessObservation(
        4242, 7, "", "mnt:[1]", leader_pid=4241, leader_start_ticks=5
    )
    slot = tmp_path / "slot"
    with pytest.raises(cli.Refusal, match=r"cannot inspect thread 4242 of process 4241"):
        cli._batch_lsof_fallback((thread,), {slot: str(slot)}, _budget())


def test_a_census_ignores_a_thread_of_an_ancestor_and_names_any_other(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """The ancestry holds thread-group IDs; a thread is matched by its leader's."""

    slot = tmp_path / "slot"
    thread = cli._AbsentProcessObservation(
        4242, 7, "", "mnt:[1]", leader_pid=4241, leader_start_ticks=5
    )
    census = cli._ProcessPathCensus(
        (thread,), ((4242, str(slot), "link", str(slot)),), owner_cgroup_complete=False
    )
    parents = {os.getpid(): 4241}
    monkeypatch.setattr(cli, "_read_process_parent", lambda pid: parents.get(pid))
    census.assert_slot_unused(slot, None, ignore_invoking_ancestry=True)
    with pytest.raises(
        cli.Refusal,
        match=r"^live thread 4242 of process 4241 \(whose main thread has exited\) uses slot",
    ):
        census.assert_slot_unused(slot, None)


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

    if use_lsof:
        _require_lsof()
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


def test_an_unreadable_leader_state_refuses_the_slot_use_walk(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    """A walk that cannot read a process's state cannot rule out hidden threads."""

    _require_lsof()
    monkeypatch.setattr(cli, "_lsof_slot_use", _lsof_reports_nothing)
    with _zombie_leader(tmp_path, "cwd") as child:
        leader_stat = child.proc_root / str(child.leader) / "stat"
        real = Path.read_bytes

        def failing(path: Path) -> bytes:
            if path == leader_stat:
                raise OSError(errno.EIO, "injected", str(path))
            return real(path)

        monkeypatch.setattr(Path, "read_bytes", failing)
        with pytest.raises(cli.Refusal, match="process state is indeterminate"):
            cli._assert_slot_unused(child.slot, proc_root=child.proc_root)
        with pytest.raises(cli.Refusal, match="process state is indeterminate"):
            cli._agent_batch_fresh_use(
                child.slot, None, capture_generation=False, proc_root=child.proc_root
            )


def test_an_unreadable_thread_listing_refuses_the_slot_use_walks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    _require_lsof()
    monkeypatch.setattr(cli, "_lsof_slot_use", _lsof_reports_nothing)
    with _zombie_leader(tmp_path, "cwd") as child:
        task = child.proc_root / str(child.leader) / "task"
        real = os.scandir

        def failing(path: int | str | os.PathLike[str] = ".") -> Iterator[os.DirEntry[str]]:
            # Other readers pass directory descriptors, which name no path.
            if not isinstance(path, int) and Path(path) == task:
                raise OSError(errno.EIO, "injected", str(path))
            return real(path)

        monkeypatch.setattr(os, "scandir", failing)
        with pytest.raises(cli.Refusal, match="thread group is indeterminate"):
            cli._assert_slot_unused(child.slot, proc_root=child.proc_root)
        with pytest.raises(cli.Refusal, match="thread group is indeterminate"):
            cli._agent_batch_fresh_use(
                child.slot, None, capture_generation=False, proc_root=child.proc_root
            )
        try:
            fenced = cli._agent_batch_fenced_use(
                child.slot, child.slot, None, capture_generation=False,
                proc_root=child.proc_root,
            )
        except cli.Refusal as exc:
            assert "thread group is indeterminate" in str(exc)
        else:
            # Only a filesystem the fenced scan does not compare may stop it
            # before it walks a process.
            assert isinstance(fenced, cli._UndecidedFencedUse), fenced
            assert "filesystem" in fenced.reason, fenced.reason


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

    monkeypatch.setattr(cli, "_lsof_slot_use", _lsof_reports_nothing)
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
        _require_lsof()
        cli._assert_slot_unused(child.slot, proc_root=child.proc_root)
    assert survivor in mount_reads


def test_an_unreadable_entry_after_a_found_use_keeps_the_use(tmp_path: Path) -> None:
    """Under lsof's rule each unreadable read is skipped, never the uses found."""

    slot = tmp_path / "slot"
    slot.mkdir()
    entries = tmp_path / "entries"
    entries.mkdir()
    (entries / "cwd").symlink_to(slot)
    (entries / "root").symlink_to("/")
    (entries / "fd").mkdir()
    (entries / "maps").write_bytes(f"00400000-00401000 r--p 0 00:01 1 {slot}/held\n".encode())
    (entries / "fd").chmod(0)
    (entries / "maps").chmod(0)
    try:
        if os.access(entries / "fd", os.R_OK):
            pytest.skip("this user is not denied an unreadable directory")
        with pytest.raises(cli.Refusal, match="is unreadable"):
            cli._process_uses_slot(entries, slot)
        assert cli._process_uses_slot(entries, slot, unreadable_is_no_evidence=True) == [
            f"cwd={slot}"
        ]
    finally:
        (entries / "fd").chmod(0o700)
        (entries / "maps").chmod(0o600)


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


def test_the_live_use_recheck_waits_for_a_reported_thread_to_exit(tmp_path: Path) -> None:
    """A plain pidfd names only a leader; the recheck waits on the thread itself."""

    with _zombie_leader(tmp_path, "cwd") as child:
        try:
            os.close(os.pidfd_open(child.thread, cli._PIDFD_THREAD))
        except OSError:
            pytest.skip("this kernel cannot open a pidfd for one thread")
        ticks = cli._process_start_ticks(Path("/proc") / str(child.thread))
        timer = threading.Timer(0.3, child.process.kill)
        timer.start()
        try:
            started = time.monotonic()
            assert cli._LiveUseRecheckBudget(20).wait_for_exit(child.thread, start_ticks=ticks)
            assert time.monotonic() - started >= 0.2
        finally:
            timer.cancel()


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
        cli._recheck_thread_group_observations((observed,), _budget())
        proc_root = _proc_root(tmp_path, exited.pid)
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
        cli._recheck_thread_group_observations((_observation(child.pid),), _budget())
