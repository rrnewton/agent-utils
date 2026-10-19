"""The same-UID census closes over processes that fork after it lists /proc.

A process that holds a fenced checkout can fork after the census enumerated
/proc, hand its working directory, descriptors, mappings or mounts to the
child, and exit before its own entries are read.  The child is in no
snapshot the census took, so a census that probes only its snapshot reports
the checkout unused.  The census therefore lists /proc again after scanning
and scans every process generation, an exact (pid, start_ticks) identity,
that the listing adds, until a listing adds nothing.

Most tests here drive the census over a small scripted host: ``_Host`` holds
the live generations and which of them hold the slot, and events fire at
fixed points (after the Nth listing, or before one generation's entries are
read), so each race happens the same way every run.  The last test runs a
real child that forks after the listing and exits, over the real /proc and
the real find and grep.
"""

from __future__ import annotations

import os
import signal
import subprocess
import sys
import time
from collections.abc import Callable, Iterator, Mapping, Sequence
from contextlib import contextmanager
from pathlib import Path

import pytest

from wrkslots import cli

Match = tuple[int, str, str, str]
Generation = tuple[int, int]


def _budget() -> cli._ReadOnlyCommandBudget:
    return cli._ReadOnlyCommandBudget.start(
        timeout_seconds=60,
        stdout_limit=16 * 1024 * 1024,
        stderr_limit=64 * 1024,
        input_limit=cli._MOUNTINFO_CENSUS_BYTES_LIMIT,
    )


class _Host:
    """A scripted process table: live generations, holders, and timed events."""

    def __init__(self, slot: Path, *live: Generation, holders: Sequence[Generation] = ()) -> None:
        self.slot = slot
        self.live: dict[int, int] = dict(live)
        self.holders: set[Generation] = set(holders)
        self.inventories: list[tuple[Generation, ...]] = []
        self.scans: list[tuple[Generation, ...]] = []
        self.after_inventory: dict[int, Callable[[], None]] = {}
        self.before_scanning: dict[Generation, Callable[[], None]] = {}

    def fork(self, parent: Generation, child: Generation, *, parent_exits: bool) -> None:
        """``parent`` forks ``child``, which inherits what the parent holds."""

        assert self.live.get(parent[0]) == parent[1], parent
        self.live[child[0]] = child[1]
        if parent in self.holders:
            self.holders.add(child)
        if parent_exits:
            self.exit(parent)

    def exit(self, generation: Generation) -> None:
        assert self.live.get(generation[0]) == generation[1], generation
        del self.live[generation[0]]

    def spawn(self, generation: Generation) -> None:
        """An unrelated process starts, holding nothing."""

        assert generation[0] not in self.live
        self.live[generation[0]] = generation[1]

    # Stand-ins for the census's readers.

    def observations(
        self, _budget: cli._ReadOnlyCommandBudget,
    ) -> tuple[cli._AbsentProcessObservation, ...]:
        listed = tuple(
            cli._AbsentProcessObservation(pid, ticks, "", "mnt:[1]")
            for pid, ticks in sorted(self.live.items())
        )
        self.inventories.append(tuple((item.pid, item.start_ticks) for item in listed))
        event = self.after_inventory.pop(len(self.inventories), None)
        if event is not None:
            event()
        return listed

    def read_stat(self, pid_dir: Path) -> cli._ProcessStat | None:
        ticks = self.live.get(int(pid_dir.name))
        return None if ticks is None else cli._ProcessStat(ticks, 0)

    def path_matches(
        self,
        processes: Sequence[cli._AbsentProcessObservation],
        targets: Mapping[Path, str],
        _budget: cli._ReadOnlyCommandBudget,
    ) -> tuple[tuple[Match, ...], tuple[cli._AbsentProcessObservation, ...]]:
        """The link readers: what each listed PID holds now, as find reads it."""

        self.scans.append(tuple((item.pid, item.start_ticks) for item in processes))
        matches: list[Match] = []
        for process in processes:
            generation = (process.pid, process.start_ticks)
            event = self.before_scanning.pop(generation, None)
            if event is not None:
                event()
            if self.live.get(process.pid) != process.start_ticks:
                continue  # it exited: its entries read as missing
            if generation in self.holders:
                matches.append((process.pid, targets[self.slot], "link", str(self.slot)))
        return tuple(matches), ()


@pytest.fixture()
def slot(tmp_path: Path) -> Path:
    path = tmp_path / "fenced-slot"
    path.mkdir()
    return path


def _scripted(monkeypatch: pytest.MonkeyPatch, host: _Host) -> None:
    monkeypatch.setattr(cli, "_same_uid_process_observations", host.observations)
    monkeypatch.setattr(cli, "_read_process_stat", host.read_stat)
    monkeypatch.setattr(cli, "_process_is_zombie", lambda _path, **_kwargs: False)
    monkeypatch.setattr(cli, "_same_uid_batched_path_matches", host.path_matches)
    for name in (
        "_absent_validate_find_matches",
        "_absent_validate_maps_matches",
        "_absent_validate_mount_matches",
    ):
        monkeypatch.setattr(cli, name, lambda *_args: ())


def _census(host: _Host) -> cli._ProcessPathCensus:
    return cli._capture_same_uid_process_path_census((host.slot,), budget=_budget())


def _holders(census: cli._ProcessPathCensus) -> set[int]:
    return {pid for pid, _slot, _kind, _detail in census.matches}


def test_a_child_forked_after_the_listing_is_scanned(
    monkeypatch: pytest.MonkeyPatch, slot: Path,
) -> None:
    """The holder forks once the census has listed /proc, and exits before its scan."""

    parent, child = (100, 7), (200, 9)
    host = _Host(slot, parent, holders=[parent])
    host.after_inventory[1] = lambda: host.fork(parent, child, parent_exits=True)
    _scripted(monkeypatch, host)

    census = _census(host)

    assert host.inventories == [(parent,), (child,), (child,)]
    assert host.scans == [(parent,), (child,)]
    assert _holders(census) == {child[0]}
    with pytest.raises(cli.Refusal, match=rf"live process {child[0]} uses slot "):
        census.assert_slot_unused(slot, None, ignore_current_process=False)


def test_a_handoff_chain_is_followed_to_its_last_holder(
    monkeypatch: pytest.MonkeyPatch, slot: Path,
) -> None:
    """Each holder forks and exits just after the census lists it."""

    first, second, third = (100, 7), (200, 9), (300, 11)
    host = _Host(slot, first, holders=[first])
    host.after_inventory[1] = lambda: host.fork(first, second, parent_exits=True)
    host.after_inventory[2] = lambda: host.fork(second, third, parent_exits=True)
    _scripted(monkeypatch, host)

    census = _census(host)

    assert host.inventories == [(first,), (second,), (third,), (third,)]
    assert _holders(census) == {third[0]}


def test_replaced_pids_never_restart_the_host_wide_scan(
    monkeypatch: pytest.MonkeyPatch, slot: Path,
) -> None:
    """A PID reused by an unrelated process is a new generation, scanned alone.

    One PID is replaced after each of three listings.  A census that
    discarded its attempt on each replacement refused after three; this one
    omits each replaced generation and scans each newcomer once.
    """

    host = _Host(slot, (100, 7), (101, 7), (102, 7), (103, 7))
    for listing, pid in enumerate((100, 101, 102), start=1):

        def replace(pid: int = pid) -> None:
            host.exit((pid, 7))
            host.spawn((pid, 12))

        host.after_inventory[listing] = replace
    _scripted(monkeypatch, host)

    census = _census(host)

    assert host.scans == [
        ((101, 7), (102, 7), (103, 7)),
        ((100, 12),),
        ((101, 12),),
        ((102, 12),),
    ]
    assert host.inventories[-1] == ((100, 12), (101, 12), (102, 12), (103, 7))
    assert len(host.inventories) == 5
    assert census.matches == ()
    assert [(item.pid, item.start_ticks) for item in census.processes] == [
        (101, 7), (102, 7), (103, 7), (100, 12), (101, 12), (102, 12),
    ]


def test_a_newcomer_on_a_replaced_pid_is_scanned_for_use(
    monkeypatch: pytest.MonkeyPatch, slot: Path,
) -> None:
    """The holder exits once its PID's predecessor has; its child takes that PID."""

    bystander, holder, newcomer = (100, 7), (150, 3), (100, 12)

    def replace() -> None:
        host.exit(bystander)
        host.fork(holder, newcomer, parent_exits=True)

    host = _Host(slot, bystander, holder, holders=[holder])
    host.before_scanning[holder] = replace
    _scripted(monkeypatch, host)

    census = _census(host)

    assert host.scans == [(bystander, holder), (newcomer,)]
    assert _holders(census) == {newcomer[0]}


def test_a_use_found_before_a_discarded_scan_is_kept(
    monkeypatch: pytest.MonkeyPatch, slot: Path,
) -> None:
    """Thread-group motion discards the scan; the holder then exits for good.

    Before this census kept positive evidence across retries, the retry saw
    no holder and reported the slot unused.
    """

    holder = (100, 7)
    host = _Host(slot, holder, holders=[holder])
    rechecks: list[int] = []

    def recheck(
        processes: Sequence[cli._AbsentProcessObservation],
        _budget: cli._ReadOnlyCommandBudget | None = None,
    ) -> None:
        rechecks.append(len(processes))
        if len(rechecks) == 1:
            host.exit(holder)
            raise cli._ProcessEvidenceChanged("fixture thread-group motion")

    _scripted(monkeypatch, host)
    monkeypatch.setattr(cli, "_recheck_thread_group_observations", recheck)

    census = _census(host)

    assert host.inventories == [(holder,), ()]
    assert census.matches == ((holder[0], str(slot), "link", str(slot)),)
    with pytest.raises(cli.Refusal, match=rf"live process {holder[0]} uses slot "):
        census.assert_slot_unused(slot, None, ignore_current_process=False)


def test_a_link_match_survives_motion_in_the_maps_reader(
    monkeypatch: pytest.MonkeyPatch, slot: Path,
) -> None:
    """The real link and maps readers: find matched, then grep saw motion.

    grep reports the holder's maps missing while its generation still runs,
    which discards the scan; the holder exits before the retry.  The link
    match find authenticated is kept.
    """

    holder = cli._AbsentProcessObservation(100, 7, "", "mnt:[1]")
    listings = iter(((holder,),))
    inventories: list[tuple[cli._AbsentProcessObservation, ...]] = []

    def observations(
        _budget: cli._ReadOnlyCommandBudget,
    ) -> tuple[cli._AbsentProcessObservation, ...]:
        # Listed once; gone from every later listing.
        inventories.append(next(listings, ()))
        return inventories[-1]

    def run(
        program: Path,
        arguments: Sequence[str],
        *,
        budget: cli._ReadOnlyCommandBudget,
        input_data: bytes | None = None,
    ) -> tuple[int, bytes, bytes]:
        del arguments, budget, input_data
        if program.name == "find":
            return 0, os.fsencode(f"/proc/100/cwd\0{slot}\0"), b""
        return 2, b"", b"/usr/bin/grep: /proc/100/maps: No such file or directory\n"

    monkeypatch.setattr(cli, "_same_uid_process_observations", observations)
    monkeypatch.setattr(cli, "_read_process_stat", lambda _path: cli._ProcessStat(7, 0))
    monkeypatch.setattr(cli, "_process_is_zombie", lambda _path, **_kwargs: False)
    # Still running when grep's diagnostic is judged, so the scan is discarded.
    monkeypatch.setattr(cli, "_process_observation_is_active", lambda _process: True)
    monkeypatch.setattr(
        cli, "_same_uid_direct_path_processes", lambda processes, _budget: (processes, ())
    )
    monkeypatch.setattr(cli, "_run_same_uid_command", run)
    monkeypatch.setattr(cli, "_absent_validate_mount_matches", lambda *_args: ())

    census = cli._capture_same_uid_process_path_census((slot,), budget=_budget())

    assert inventories == [(holder,), ()]
    assert census.matches == ((100, str(slot), "link", str(slot)),)


def test_unrelated_births_converge(
    monkeypatch: pytest.MonkeyPatch, slot: Path,
) -> None:
    """Processes that start during the census are scanned once each, then it closes."""

    first, second, third = (100, 7), (200, 9), (300, 11)
    host = _Host(slot, first)
    host.after_inventory[1] = lambda: host.spawn(second)
    host.after_inventory[2] = lambda: host.spawn(third)
    _scripted(monkeypatch, host)

    census = _census(host)

    assert host.scans == [(first,), (second,), (third,)]
    assert len(host.inventories) == 4
    assert host.inventories[-1] == (first, second, third)
    assert census.matches == ()
    census.assert_slot_unused(slot, None, ignore_current_process=False)


def test_endless_births_refuse_within_the_bound(
    monkeypatch: pytest.MonkeyPatch, slot: Path,
) -> None:
    """Every listing finds a newcomer, so no listing ever closes the census."""

    host = _Host(slot, (100, 7))
    for listing in range(1, 40):

        def birth(pid: int = 1000 + listing) -> None:
            host.spawn((pid, pid))

        host.after_inventory[listing] = birth
    _scripted(monkeypatch, host)

    with pytest.raises(cli.Refusal, match="same-UID process census did not close") as refused:
        _census(host)

    assert len(host.inventories) == cli._SAME_UID_CENSUS_INVENTORIES
    assert len(host.scans) == cli._SAME_UID_CENSUS_INVENTORIES
    assert "PIDs 1007" in str(refused.value)


def test_persistent_motion_still_refuses_after_three_scans(
    monkeypatch: pytest.MonkeyPatch, slot: Path,
) -> None:
    host = _Host(slot, (100, 7))
    _scripted(monkeypatch, host)

    def motion(*_args: object) -> None:
        raise cli._ProcessEvidenceChanged("fixture thread-group motion")

    monkeypatch.setattr(cli, "_recheck_thread_group_observations", motion)

    with pytest.raises(cli.Refusal, match="three liveness attempts") as refused:
        _census(host)

    assert not isinstance(refused.value, cli._ProcessEvidenceChanged)
    assert host.scans == [((100, 7),)] * 3


# The real census commands are trusted only as root-owned executables in the
# initial identity user namespace.
_REAL_CENSUS_COMMANDS = pytest.mark.skipif(
    not cli._has_identity_user_namespace(),
    reason="the census commands are trusted only in the initial identity user namespace",
)

# argv: slot.  It changes into the slot, says "ready", and on any line of
# input forks a child that keeps that working directory, says the child's
# PID, and exits at once.
_FORKING_HOLDER = """
import os, sys, time

os.chdir(sys.argv[1])
sys.stdout.write("ready\\n")
sys.stdout.flush()
sys.stdin.readline()
if os.fork() == 0:
    sys.stdout.write(f"{os.getpid()}\\n")
    sys.stdout.flush()
    while True:
        time.sleep(60)
os._exit(0)
"""


@contextmanager
def _forking_holder(slot: Path) -> Iterator[tuple[subprocess.Popen[str], list[int]]]:
    holder = subprocess.Popen(
        [sys.executable, "-I", "-c", _FORKING_HOLDER, str(slot)],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        text=True,
        cwd="/",
    )
    children: list[int] = []
    try:
        assert holder.stdout is not None
        assert holder.stdout.readline() == "ready\n"
        yield holder, children
    finally:
        for pid in children:
            try:
                os.kill(pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        holder.kill()
        holder.wait(timeout=10)
        for stream in (holder.stdin, holder.stdout):
            if stream is not None:
                stream.close()


@_REAL_CENSUS_COMMANDS
def test_a_real_holder_that_forks_after_the_listing_and_exits(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, slot: Path,
) -> None:
    """Fork after enumeration, made deterministic: the fork happens inside the listing.

    The proc root lists only the holder until it forks; then it lists the
    child too, as /proc readdir would.  The holder is reaped before any of
    its entries are read, so every reader finds it gone.
    """

    proc_root = tmp_path / "proc"
    proc_root.mkdir()
    real_observations = cli._same_uid_process_observations
    inventories: list[tuple[int, ...]] = []
    with _forking_holder(slot) as (holder, children):
        (proc_root / str(holder.pid)).symlink_to(Path("/proc") / str(holder.pid))

        def observations(
            budget: cli._ReadOnlyCommandBudget,
        ) -> tuple[cli._AbsentProcessObservation, ...]:
            listed = real_observations(budget, proc_root)
            inventories.append(tuple(process.pid for process in listed))
            if len(inventories) == 1:
                assert holder.stdin is not None and holder.stdout is not None
                holder.stdin.write("fork\n")
                holder.stdin.flush()
                child = int(holder.stdout.readline())
                children.append(child)
                assert holder.wait(timeout=10) == 0
                (proc_root / str(child)).symlink_to(Path("/proc") / str(child))
                deadline = time.monotonic() + 10
                while os.readlink(Path("/proc") / str(child) / "cwd") != str(slot):
                    assert time.monotonic() < deadline, "the child never ran in the slot"
                    time.sleep(0.01)
            return listed

        def privileged_as_same_uid(
            program: Path,
            arguments: Sequence[str],
            *,
            input_data: bytes | None = None,
            input_limit: int = 1024 * 1024,
            budget: cli._ReadOnlyCommandBudget | None = None,
        ) -> tuple[int, bytes, bytes]:
            # The reaped holder is handed to the privileged fallback; stand in
            # for sudo, since these entries are this user's.
            del input_limit
            return cli._run_same_uid_command(
                program, arguments, budget=budget or _budget(), input_data=input_data
            )

        monkeypatch.setattr(cli, "_same_uid_process_observations", observations)
        monkeypatch.setattr(cli, "_run_root_owned_command", privileged_as_same_uid)
        census = cli._capture_same_uid_process_path_census((slot,), budget=_budget())
        (child,) = children
        assert inventories == [(holder.pid,), (child,), (child,)]
        assert {(pid, kind, detail) for pid, _slot, kind, detail in census.matches} == {
            (child, "link", str(slot))
        }
        with pytest.raises(cli.Refusal, match=rf"live process {child} uses slot "):
            census.assert_slot_unused(slot, None)

