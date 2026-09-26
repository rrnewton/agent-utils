"""Process census evidence that does not appear where a naive walk looks for it.

Each test here pairs the evidence the census must find with the nearest case it
must not report, and several run real processes and the real GNU find rather
than fixtures: the zombie-leader case in particular exists because /proc
readdir hides the very entries that hold the files.
"""

from __future__ import annotations

import errno
import inspect
import os
import subprocess
import sys
import time
from collections.abc import Iterator, Sequence
from contextlib import contextmanager
from pathlib import Path

import pytest

from wrkslots import cli


# The main thread calls pthread_exit and becomes a zombie leader, while the
# other thread keeps a descriptor open into the target directory.
_ZOMBIE_LEADER_CHILD = """
import ctypes, os, sys, threading, time

def hold():
    fd = os.open(sys.argv[1], os.O_RDONLY | os.O_DIRECTORY)
    sys.stdout.write(f"{threading.get_native_id()} {fd}\\n")
    sys.stdout.flush()
    while True:
        time.sleep(60)

threading.Thread(target=hold, daemon=True).start()
sys.stdin.readline()
ctypes.CDLL(None).pthread_exit(None)
"""


def _state(pid: int) -> str:
    stat = (Path("/proc") / str(pid) / "stat").read_text(encoding="ascii")
    return stat.rsplit(")", 1)[1].split()[0]


def _wait_for_state(pid: int, state: str) -> None:
    deadline = time.monotonic() + 10
    while _state(pid) != state:
        assert time.monotonic() < deadline, f"PID {pid} never reached state {state}"
        time.sleep(0.01)


def _observation(pid: int) -> cli._AbsentProcessObservation:
    pid_dir = Path("/proc") / str(pid)
    ticks = cli._process_start_ticks(pid_dir)
    assert ticks is not None
    namespace = cli._mount_namespace(pid_dir)
    assert namespace is not None
    return cli._AbsentProcessObservation(pid, ticks, "", namespace)


def _budget(input_limit: int = 256 * 1024 * 1024) -> cli._ReadOnlyCommandBudget:
    return cli._ReadOnlyCommandBudget.start(
        timeout_seconds=60,
        stdout_limit=16 * 1024 * 1024,
        stderr_limit=64 * 1024,
        input_limit=input_limit,
    )


def _run_as_this_user(
    program: Path,
    arguments: Sequence[str],
    *,
    input_data: bytes | None = None,
    budget: cli._ReadOnlyCommandBudget | None = None,
) -> tuple[int, bytes, bytes]:
    """Stand in for sudo: this test's own processes are readable without it."""

    return cli._run_same_uid_command(
        program, arguments, budget=budget or _budget(), input_data=input_data
    )


@contextmanager
def _zombie_leader(target: Path) -> Iterator[tuple[int, int, int]]:
    """Yield ``(leader, thread, fd)`` for a zombie leader whose thread holds ``target``."""

    child = subprocess.Popen(
        [sys.executable, "-c", _ZOMBIE_LEADER_CHILD, str(target)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        assert child.stdout is not None and child.stdin is not None
        thread, fd = (int(field) for field in child.stdout.readline().split())
        leader = _observation(child.pid)
        child.stdin.write("exit leader\n")
        child.stdin.flush()
        _wait_for_state(child.pid, "Z")
        assert cli._process_start_ticks(Path("/proc") / str(child.pid)) == leader.start_ticks
        yield child.pid, thread, fd
    finally:
        child.kill()
        child.wait(timeout=10)


@contextmanager
def _unreaped_zombie() -> Iterator[int]:
    """Yield the PID of an exited, unreaped single-threaded child."""

    child = subprocess.Popen([sys.executable, "-c", "pass"])
    try:
        _wait_for_state(child.pid, "Z")
        yield child.pid
    finally:
        child.wait(timeout=10)


@contextmanager
def _sleeper() -> Iterator[int]:
    child = subprocess.Popen(["/bin/sleep", "60"])
    try:
        yield child.pid
    finally:
        child.kill()
        child.wait(timeout=10)


def test_zombie_leader_threads_are_observed_in_its_place(tmp_path: Path) -> None:
    """The leader's own entries are gone; the thread still holds the target."""

    target = tmp_path / "slot"
    target.mkdir()
    with _zombie_leader(target) as (leader, thread, fd):
        leader_dir = Path("/proc") / str(leader)
        thread_dir = Path("/proc") / str(thread)
        # The premise: readdir hides the thread, and only the thread shows the file.
        assert str(thread) not in os.listdir("/proc")
        assert os.readlink(thread_dir / "fd" / str(fd)) == str(target)

        assert cli._zombie_leader_thread_dirs(leader_dir) == (thread_dir,)
        assert cli._zombie_leader_has_live_threads(leader_dir)
        # Only a leader stands for its threads; a thread names its whole group.
        assert cli._zombie_leader_thread_dirs(thread_dir) == ()

        for snapshot in (
            cli._absent_validate_process_snapshot(include_owner_cgroups=False),
            cli._same_uid_process_observations(_budget()),
        ):
            pids = {process.pid for process in snapshot}
            assert thread in pids
            assert leader not in pids

        observed = [_observation(thread)]
        matches, indeterminate = cli._same_uid_batched_path_matches(
            observed, {target: "slot"}, _budget()
        )
        assert indeterminate == ()
        assert [(pid, slot, kind) for pid, slot, kind, _detail in matches] == [
            (thread, "slot", "link")
        ]


def test_a_leader_that_exits_under_running_threads_discards_the_attempt(
    tmp_path: Path,
) -> None:
    """An observed leader is neither live nor released while its threads run."""

    target = tmp_path / "slot"
    target.mkdir()
    child = subprocess.Popen(
        [sys.executable, "-c", _ZOMBIE_LEADER_CHILD, str(target)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
    )
    try:
        assert child.stdout is not None and child.stdin is not None
        child.stdout.readline()
        observed = _observation(child.pid)
        assert cli._process_observation_is_active(observed)
        child.stdin.write("exit leader\n")
        child.stdin.flush()
        _wait_for_state(child.pid, "Z")

        with pytest.raises(cli._ProcessEvidenceChanged, match="threads still run"):
            cli._process_observation_is_active(observed)
        with pytest.raises(cli._ProcessEvidenceChanged, match="threads still run"):
            cli._excluded_same_uid_live_processes((observed,), _budget())
    finally:
        child.kill()
        child.wait(timeout=10)


def test_an_exited_process_is_released_and_stands_for_nothing() -> None:
    """The negative control: a whole-process zombie has no threads to observe."""

    with _unreaped_zombie() as pid:
        pid_dir = Path("/proc") / str(pid)
        observed = cli._AbsentProcessObservation(
            pid, cli._process_start_ticks(pid_dir) or 0, "", "mnt:[0]"
        )
        assert cli._zombie_leader_thread_dirs(pid_dir) == ()
        assert not cli._zombie_leader_has_live_threads(pid_dir)
        assert not cli._process_observation_is_active(observed)
        assert cli._excluded_same_uid_live_processes((observed,), _budget()) == ()
        snapshot = cli._same_uid_process_observations(_budget())
        assert pid not in {process.pid for process in snapshot}


def test_a_vanished_same_uid_entry_is_complete_only_for_a_released_process(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    with _unreaped_zombie() as zombie:
        released = cli._AbsentProcessObservation(
            zombie, cli._process_start_ticks(Path("/proc") / str(zombie)) or 0, "", "mnt:[0]"
        )
        assert cli._same_uid_direct_path_processes((released,), _budget()) == ((), ())

    live = _observation(os.getpid())
    assert cli._same_uid_direct_path_processes((live,), _budget()) == ((live,), ())

    real_readlink = os.readlink

    def vanishing(path: str | os.PathLike[str]) -> str:
        if str(path).startswith(f"/proc/{os.getpid()}/"):
            raise FileNotFoundError(errno.ENOENT, "fixture: entry vanished", str(path))
        return real_readlink(path)

    monkeypatch.setattr(os, "readlink", vanishing)
    # A live generation's missing entry is an incomplete listing, never absence.
    assert cli._same_uid_direct_path_processes((live,), _budget()) == ((), (live,))


def test_processes_sharing_one_mount_view_read_one_table(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    reads: list[int] = []
    real = cli._mountinfo_path_references

    def counting(
        pid: int,
        budget: cli._ReadOnlyCommandBudget,
        cache: dict[str, tuple[tuple[str, str], ...]] | None = None,
    ) -> tuple[tuple[str, str], ...]:
        reads.append(pid)
        return real(pid, budget, cache)

    monkeypatch.setattr(cli, "_mountinfo_path_references", counting)
    with _sleeper() as sibling:
        processes = (_observation(os.getpid()), _observation(sibling))
        views = cli._mount_view_keys(processes, _budget())
        assert views[os.getpid()] == views[sibling]
        assert views[sibling] == (
            os.readlink(f"/proc/{sibling}/ns/mnt"), os.readlink(f"/proc/{sibling}/root"),
        )
        # A mounted path of the shared table is reported once, for the reader.
        reported = cli._absent_validate_mount_matches(processes, {Path("/proc"): "proc"}, _budget())
    assert reads == [os.getpid()]
    assert {(pid, slot, kind) for pid, slot, kind, _detail in reported} == {
        (os.getpid(), "proc", "mount")
    }


def test_a_mount_view_that_moves_during_the_census_discards_it(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    processes = (
        cli._AbsentProcessObservation(101, 1, "", "mnt:[7]"),
        cli._AbsentProcessObservation(102, 1, "", "mnt:[7]"),
        cli._AbsentProcessObservation(103, 1, "", "mnt:[9]"),
    )
    listings: list[tuple[int, ...]] = []

    def keys(
        observed: Sequence[cli._AbsentProcessObservation], _budget: cli._ReadOnlyCommandBudget,
    ) -> dict[int, tuple[str, str]]:
        listings.append(tuple(process.pid for process in observed))
        moved = len(listings) > 1
        return {
            101: ("mnt:[7]", "/"),
            102: ("mnt:[8]" if moved else "mnt:[7]", "/"),
            103: ("mnt:[9]", "/"),
        }

    monkeypatch.setattr(cli, "_mount_view_keys", keys)
    monkeypatch.setattr(cli, "_mountinfo_path_references", lambda _pid, _budget, _cache=None: ())
    monkeypatch.setattr(cli, "_process_start_ticks", lambda _path: 1)
    active = {102: True}
    monkeypatch.setattr(
        cli, "_process_observation_is_active", lambda process: active[process.pid]
    )

    with pytest.raises(cli._ProcessEvidenceChanged, match="PID 102 mount view changed"):
        cli._absent_validate_mount_matches(processes, {Path("/slot"): "slot"}, _budget())
    # Only members that relied on another's table are listed again.
    assert listings[1] == (101, 102)

    # An exited member's later view is irrelevant: it holds nothing now.
    listings.clear()
    active[102] = False
    assert cli._absent_validate_mount_matches(processes, {Path("/slot"): "slot"}, _budget()) == ()


def test_a_reader_that_moved_and_exited_cannot_stand_for_its_view(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # 101 reads for the view it shared with 102, but its table came from a view
    # it moved to first; it then exits, so only 102 is left showing the view.
    processes = (
        cli._AbsentProcessObservation(101, 1, "", "mnt:[7]"),
        cli._AbsentProcessObservation(102, 1, "", "mnt:[7]"),
    )
    reader_after: dict[int, tuple[str, str]] = {}
    listings: list[tuple[int, ...]] = []
    reads: list[int] = []

    def keys(
        observed: Sequence[cli._AbsentProcessObservation], _budget: cli._ReadOnlyCommandBudget,
    ) -> dict[int, tuple[str, str]]:
        listings.append(tuple(process.pid for process in observed))
        if len(listings) == 1:
            return {101: ("mnt:[7]", "/"), 102: ("mnt:[7]", "/")}
        return {**reader_after, 102: ("mnt:[7]", "/")}

    def mountinfo(
        pid: int, _budget: cli._ReadOnlyCommandBudget,
        _cache: dict[str, tuple[tuple[str, str], ...]] | None = None,
    ) -> tuple[tuple[str, str], ...]:
        reads.append(pid)
        return ()

    monkeypatch.setattr(cli, "_mount_view_keys", keys)
    monkeypatch.setattr(cli, "_mountinfo_path_references", mountinfo)
    monkeypatch.setattr(cli, "_process_start_ticks", lambda _path: 1)
    monkeypatch.setattr(
        cli, "_process_observation_is_active", lambda process: process.pid != 101
    )

    with pytest.raises(cli._ProcessEvidenceChanged, match="PID 101 mount view changed"):
        cli._absent_validate_mount_matches(processes, {Path("/slot"): "slot"}, _budget())
    assert reads == [101]
    assert listings == [(101, 102), (101, 102)]

    # The same exit is harmless when the reader still showed the view after
    # its read: that table was the view's, and it names no target.
    listings.clear()
    reads.clear()
    reader_after[101] = ("mnt:[7]", "/")
    assert cli._absent_validate_mount_matches(processes, {Path("/slot"): "slot"}, _budget()) == ()
    assert reads == [101]


def test_each_distinct_mount_view_reads_its_own_table(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # 103 differs from 101 only in its root and 104 only in its namespace;
    # each must read its own table, while 102 relies on 101's.
    processes = tuple(
        cli._AbsentProcessObservation(pid, 1, "", "mnt:[7]") for pid in (101, 102, 103, 104)
    )
    views = {
        101: ("mnt:[7]", "/"),
        102: ("mnt:[7]", "/"),
        103: ("mnt:[7]", "/chroot"),
        104: ("mnt:[8]", "/"),
    }
    reads: list[int] = []

    def mountinfo(
        pid: int, _budget: cli._ReadOnlyCommandBudget,
        _cache: dict[str, tuple[tuple[str, str], ...]] | None = None,
    ) -> tuple[tuple[str, str], ...]:
        reads.append(pid)
        return (("/slot/work", "/slot/work"),) if pid in (103, 104) else ()

    monkeypatch.setattr(cli, "_mount_view_keys", lambda observed, _budget: {
        process.pid: views[process.pid] for process in observed
    })
    monkeypatch.setattr(cli, "_mountinfo_path_references", mountinfo)
    monkeypatch.setattr(cli, "_process_start_ticks", lambda _path: 1)

    matches = cli._absent_validate_mount_matches(processes, {Path("/slot"): "slot"}, _budget())
    assert reads == [101, 103, 104]
    assert [(pid, slot, kind) for pid, slot, kind, _detail in matches] == [
        (103, "slot", "mount"), (104, "slot", "mount"),
    ]


def test_a_reader_whose_pid_was_reused_in_its_old_view_cannot_stand_for_it(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # 101 reads its table, then its PID is reused by a process that shows the
    # view 101 was listed in; the second listing alone cannot tell them apart.
    processes = (
        cli._AbsentProcessObservation(101, 1, "", "mnt:[7]"),
        cli._AbsentProcessObservation(102, 1, "", "mnt:[7]"),
    )
    listings: list[tuple[int, ...]] = []
    reused: set[int] = set()

    def keys(
        observed: Sequence[cli._AbsentProcessObservation], _budget: cli._ReadOnlyCommandBudget,
    ) -> dict[int, tuple[str, str]]:
        listings.append(tuple(process.pid for process in observed))
        return {process.pid: ("mnt:[7]", "/") for process in observed}

    def start_ticks(path: Path) -> int:
        return 2 if len(listings) > 1 and int(path.name) in reused else 1

    monkeypatch.setattr(cli, "_mount_view_keys", keys)
    monkeypatch.setattr(cli, "_mountinfo_path_references", lambda _pid, _budget, _cache=None: ())
    monkeypatch.setattr(cli, "_process_start_ticks", start_ticks)
    monkeypatch.setattr(cli, "_process_observation_is_active", lambda _process: True)

    reused.add(101)
    with pytest.raises(cli._ProcessEvidenceChanged, match="PID 101 mount view changed"):
        cli._absent_validate_mount_matches(processes, {Path("/slot"): "slot"}, _budget())
    assert listings == [(101, 102), (101, 102)]

    # A member's generation is not what the table stands on, so a reused
    # member PID that shows the same view does not discard the attempt.
    listings.clear()
    reused.clear()
    reused.add(102)
    assert cli._absent_validate_mount_matches(processes, {Path("/slot"): "slot"}, _budget()) == ()
    assert listings == [(101, 102), (101, 102)]


@pytest.mark.parametrize("moved_pid", (101, 102))
def test_a_root_that_moves_during_the_census_discards_it(
    monkeypatch: pytest.MonkeyPatch, moved_pid: int,
) -> None:
    # The namespace stays the same; only the root changes, by chroot, in the
    # reader (101) or in a live member relying on its table (102).
    processes = (
        cli._AbsentProcessObservation(101, 1, "", "mnt:[7]"),
        cli._AbsentProcessObservation(102, 1, "", "mnt:[7]"),
    )
    listings: list[tuple[int, ...]] = []

    def keys(
        observed: Sequence[cli._AbsentProcessObservation], _budget: cli._ReadOnlyCommandBudget,
    ) -> dict[int, tuple[str, str]]:
        listings.append(tuple(process.pid for process in observed))
        moved = len(listings) > 1
        return {
            process.pid: (
                "mnt:[7]", "/chroot" if moved and process.pid == moved_pid else "/",
            )
            for process in observed
        }

    monkeypatch.setattr(cli, "_mount_view_keys", keys)
    monkeypatch.setattr(cli, "_mountinfo_path_references", lambda _pid, _budget, _cache=None: ())
    monkeypatch.setattr(cli, "_process_start_ticks", lambda _path: 1)
    monkeypatch.setattr(cli, "_process_observation_is_active", lambda _process: True)

    with pytest.raises(
        cli._ProcessEvidenceChanged, match=f"PID {moved_pid} mount view changed"
    ):
        cli._absent_validate_mount_matches(processes, {Path("/slot"): "slot"}, _budget())
    assert listings == [(101, 102), (101, 102)]


def test_a_member_that_failed_to_read_and_exited_is_released(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # 101's read fails and it exits, so 102 reads for the view. 101 is absent
    # from the second listing, but it never stood for the view and holds
    # nothing now, so the attempt stands.
    processes = (
        cli._AbsentProcessObservation(101, 1, "", "mnt:[7]"),
        cli._AbsentProcessObservation(102, 1, "", "mnt:[7]"),
    )
    listings: list[tuple[int, ...]] = []
    reads: list[int] = []

    def keys(
        observed: Sequence[cli._AbsentProcessObservation], _budget: cli._ReadOnlyCommandBudget,
    ) -> dict[int, tuple[str, str]]:
        listings.append(tuple(process.pid for process in observed))
        if len(listings) == 1:
            return {101: ("mnt:[7]", "/"), 102: ("mnt:[7]", "/")}
        return {102: ("mnt:[7]", "/")}

    def mountinfo(
        pid: int, _budget: cli._ReadOnlyCommandBudget,
        _cache: dict[str, tuple[tuple[str, str], ...]] | None = None,
    ) -> tuple[tuple[str, str], ...]:
        reads.append(pid)
        if pid == 101:
            raise cli.Refusal("mount evidence is indeterminate for PID 101")
        return ()

    monkeypatch.setattr(cli, "_mount_view_keys", keys)
    monkeypatch.setattr(cli, "_mountinfo_path_references", mountinfo)
    monkeypatch.setattr(
        cli, "_process_start_ticks", lambda path: None if path.name == "101" else 1
    )
    monkeypatch.setattr(
        cli, "_process_observation_is_active", lambda process: process.pid != 101
    )

    assert cli._absent_validate_mount_matches(processes, {Path("/slot"): "slot"}, _budget()) == ()
    assert reads == [101, 102]
    assert listings == [(101, 102), (101, 102)]


def test_privileged_mount_views_come_from_the_real_unfollowed_find(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(cli, "_run_root_owned_command", _run_as_this_user)
    with _sleeper() as sibling:
        observed = {pid: _observation(pid) for pid in (os.getpid(), sibling)}
        assert cli._privileged_mount_view_keys(observed, _budget()) == {
            pid: (os.readlink(f"/proc/{pid}/ns/mnt"), os.readlink(f"/proc/{pid}/root"))
            for pid in observed
        }


def _records(*fields: str) -> bytes:
    return b"".join(field.encode() + b"\0" for field in fields)


_GOOD_VIEW = ("/proc/5/ns/mnt", "mnt:[4026531841]", "/proc/5/root", "/")


@pytest.mark.parametrize(
    ("stdout", "message"),
    (
        (_records(*_GOOD_VIEW, "/proc/5/cwd", "/"), "unexpected path"),
        (_records(*_GOOD_VIEW, "/proc/6/root", "/"), "unexpected path"),
        (_records(*_GOOD_VIEW, "/proc/5/root", "/"), "repeated"),
        (_records("/proc/5/ns/mnt", "mnt:[4026531841]"), "incomplete view"),
        (_records("/proc/5/ns/mnt", "net:[4026531841]", "/proc/5/root", "/"), "incomplete view"),
        (_records("/proc/5/ns/mnt", "mnt:[1]", "/proc/5/root", ""), "incomplete view"),
        (_records("/proc/5/ns/mnt", "mnt:[1]", "/proc/5/root"), "mount view find"),
    ),
)
def test_privileged_mount_views_refuse_output_they_cannot_attribute(
    monkeypatch: pytest.MonkeyPatch, stdout: bytes, message: str,
) -> None:
    monkeypatch.setattr(
        cli, "_run_root_owned_command", lambda *_args, **_kwargs: (0, stdout, b"")
    )
    observed = {5: cli._AbsentProcessObservation(5, 1, "", "pid:5")}
    with pytest.raises(cli.Refusal, match=message):
        cli._privileged_mount_view_keys(observed, _budget())


def test_privileged_mount_views_accept_well_formed_output(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(
        cli, "_run_root_owned_command", lambda *_args, **_kwargs: (0, _records(*_GOOD_VIEW), b"")
    )
    observed = {5: cli._AbsentProcessObservation(5, 1, "", "pid:5")}
    assert cli._privileged_mount_view_keys(observed, _budget()) == {
        5: ("mnt:[4026531841]", "/")
    }


def test_a_vanished_view_across_argument_batches_keeps_its_own_group(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """One PID's two links may land in different find invocations.

    Its namespace link vanishes in the first, while its root still prints in the
    second: the view is unknown, so the process keeps its own group rather than
    being keyed by half a view -- and a diagnostic for any other PID refuses.
    """

    def one_root_per_batch(
        prefix: Sequence[str], items: Sequence[str], suffix: Sequence[str], **_kwargs: int,
    ) -> Iterator[list[str]]:
        for item in items:
            yield [*prefix, item, *suffix]

    outputs = {
        "/proc/5/ns/mnt": (
            1, _records("/proc/5/ns/mnt", ""),
            b"/usr/bin/find: '/proc/5/ns/mnt': No such file or directory\n",
        ),
        "/proc/5/root": (0, _records("/proc/5/root", "/"), b""),
        "/proc/6/ns/mnt": (0, _records("/proc/6/ns/mnt", "mnt:[2]"), b""),
        "/proc/6/root": (0, _records("/proc/6/root", "/srv"), b""),
    }

    def run(
        _program: Path, arguments: Sequence[str], **_kwargs: cli._ReadOnlyCommandBudget | None,
    ) -> tuple[int, bytes, bytes]:
        return outputs[arguments[1]]

    monkeypatch.setattr(cli, "_argument_batches", one_root_per_batch)
    monkeypatch.setattr(cli, "_run_root_owned_command", run)
    monkeypatch.setattr(cli, "_process_start_ticks", lambda _path: None)
    observed = {
        pid: cli._AbsentProcessObservation(pid, 1, "", f"pid:{pid}") for pid in (5, 6)
    }
    assert cli._privileged_mount_view_keys(observed, _budget()) == {6: ("mnt:[2]", "/srv")}

    outputs["/proc/6/root"] = (
        1, b"", b"/usr/bin/find: '/proc/7/root': No such file or directory\n",
    )
    with pytest.raises(cli.Refusal):
        cli._privileged_mount_view_keys(observed, _budget())


def test_batch_link_matching_reports_a_held_descendant_and_not_a_sibling_prefix(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path,
) -> None:
    """Real GNU find over this process's own descriptors, with sudo stood in.

    ``slot-sibling`` shares the target's string prefix without lying inside it,
    which is exactly what a prefix match on link text would get wrong.
    """

    monkeypatch.setattr(cli, "_run_root_owned_command", _run_as_this_user)
    target = tmp_path / "slot"
    sibling = tmp_path / "slot-sibling"
    (target / "inner").mkdir(parents=True)
    sibling.mkdir()
    process = _observation(os.getpid())
    targets = {target: "slot"}

    sibling_fd = os.open(sibling, os.O_RDONLY | os.O_DIRECTORY)
    try:
        assert cli._batch_link_matches((process,), targets, _budget()) == ()
        held = os.open(target / "inner", os.O_RDONLY | os.O_DIRECTORY)
        try:
            matches = cli._batch_link_matches((process,), targets, _budget())
        finally:
            os.close(held)
    finally:
        os.close(sibling_fd)
    assert [(pid, slot, kind) for pid, slot, kind, _detail in matches] == [
        (os.getpid(), "slot", "link")
    ]
    assert str(target / "inner") in matches[0][3]
    assert cli._batch_link_matches((process,), targets, _budget()) == ()


def test_the_same_uid_input_bound_spans_every_retry(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path,
) -> None:
    """A discarded attempt's reads still count against the one operation bound.

    The first attempt reads this process's mount table and is then discarded;
    the retry reads it again. Exactly two tables' worth of input completes, and
    one byte less refuses rather than granting each attempt a fresh allowance.
    The table must not change between the reads; a test process's does not.
    """

    own = _observation(os.getpid())
    monkeypatch.setattr(cli, "_same_uid_process_observations", lambda _budget: (own,))
    monkeypatch.setattr(
        cli, "_observable_process_generations", lambda processes, _budget: tuple(processes)
    )
    monkeypatch.setattr(
        cli, "_same_uid_batched_path_matches", lambda _observable, _targets, _budget: ((), ())
    )
    monkeypatch.setattr(cli, "_absent_validate_find_matches", lambda *_args: ())
    monkeypatch.setattr(cli, "_absent_validate_maps_matches", lambda *_args: ())
    real = cli._absent_validate_mount_matches
    attempts: list[int] = []

    def discarding_first(
        observable: Sequence[cli._AbsentProcessObservation],
        targets: dict[Path, str],
        budget: cli._ReadOnlyCommandBudget,
    ) -> tuple[tuple[int, str, str, str], ...]:
        result = real(observable, targets, budget)
        attempts.append(budget.input_remaining)
        if len(attempts) == 1:
            raise cli._ProcessEvidenceChanged("fixture: first attempt discarded")
        return result

    monkeypatch.setattr(cli, "_absent_validate_mount_matches", discarding_first)
    table = len((Path("/proc") / str(os.getpid()) / "mountinfo").read_bytes())

    census = cli._capture_same_uid_process_path_census(
        (tmp_path / "slot",), budget=_budget(input_limit=2 * table)
    )
    assert census.matches == ()
    assert attempts == [table, 0]

    attempts.clear()
    with pytest.raises(cli.Refusal) as refused:
        cli._capture_same_uid_process_path_census(
            (tmp_path / "slot",), budget=_budget(input_limit=2 * table - 1)
        )
    assert attempts == [table - 1]
    # The retry's read is capped at what the discarded attempt left, not granted
    # a fresh operation allowance.
    assert f"exceeds the {table - 1}-byte safety bound" in str(refused.value)


def test_every_process_path_census_names_its_budget() -> None:
    """No caller can fall back to an implicit bound the operation did not choose."""

    parameter = inspect.signature(cli._capture_process_path_census).parameters["budget"]
    assert parameter.kind is inspect.Parameter.KEYWORD_ONLY
    assert parameter.default is inspect.Parameter.empty
    with pytest.raises(TypeError, match="budget"):
        cli._capture_process_path_census((Path("/nonexistent-slot"),))  # type: ignore[call-arg]
