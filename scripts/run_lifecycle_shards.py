#!/usr/bin/env python3
"""Run one lifecycle-test partition in deterministic parallel shards.

``--partition mapped`` (the default) runs tests outside ``ordinary_environment``
in independent user/PID namespaces.  ``--partition ordinary`` runs the
``ordinary_environment`` tests as ordinary host processes, because they need
host-visible process identities or the initial identity user namespace.
"""

from __future__ import annotations

import argparse
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from collections.abc import Callable, Mapping, Sequence
from pathlib import Path


_LIFECYCLE_PATH = "wrkslots/tests/test_lifecycle.py"
_MAPPED_SELECTION = "not ordinary_environment"
_ORDINARY_SELECTION = "ordinary_environment"

# Measured wall seconds for the ordinary tests that dominate the partition: the
# 322-test serial run took 917.9 s, and these four took 254.2 s of it.  Each
# spends almost all of that in production's host-wide live-use scan (the mount
# walk, then `lsof +D`), whose cost grows with the number of processes on the
# host, so the absolute values are host-dependent.  They only steer scheduling:
# a stale value can make shards uneven, never skip or duplicate a test.
_ORDINARY_DURATION_HINTS: Mapping[str, float] = {
    f"{_LIFECYCLE_PATH}::test_failed_post_provision_hook_can_be_explicitly_aborted": 101.8,
    f"{_LIFECYCLE_PATH}::test_process_entering_after_final_scan_before_path_move_is_not_deleted": 75.6,
    f"{_LIFECYCLE_PATH}::test_root_lsof_sees_process_use_behind_a_sealed_guard": 46.2,
    f"{_LIFECYCLE_PATH}::test_remove_refuses_live_process_using_slot": 30.6,
}
# Mean wall seconds of the remaining 318 ordinary tests in the same run.
_ORDINARY_DEFAULT_DURATION = 2.1
# After Ctrl-C, how long shards get to finish their own teardown.  An ordinary shard's teardown is
# what stops the host processes its tests started: nothing else will, because unlike a mapped shard
# it has no PID namespace to collapse.  Each shard runs in its own session, so Ctrl-C at a terminal
# reaches only the runner, which forwards exactly one SIGINT to every shard's process group, as the
# terminal used to; a SIGINT sent to the runner alone now reaches the shards too.  Another Ctrl-C
# skips to the next step: the rest of the grace, then the SIGTERM wait.
_INTERRUPT_GRACE_SECONDS = 30.0
# How long each shard's process group gets after SIGTERM, then after SIGKILL.  Signalling the group,
# not only the leader, reaches what a shard's tests started and left behind, and reaches a mapped
# shard's namespace, whose `unshare --fork` leader ignores SIGTERM.  A process that starts its own
# session (as production hooks do) has left the group and is its test's teardown to stop.
_TERMINATE_SECONDS = 5.0
_KILL_SECONDS = 5.0
# Besides Ctrl-C, the signals that stop the runner and must not orphan shards, which no longer share
# its process group: a SIGTERM (a deadline, or a TERM sent to the caller's process group) and a
# terminal hangup.
_STOP_SIGNALS = (signal.SIGTERM, signal.SIGHUP)


class _StopSignal(BaseException):
    """The runner received one of ``_STOP_SIGNALS``."""

    def __init__(self, signum: int) -> None:
        super().__init__(f"received signal {signum}")
        self.signum = signum


def _raise_stop_signal(signum: int, _frame: object) -> None:
    raise _StopSignal(signum)


def partition_nodeids(
    nodeids: Sequence[str], shard_count: int
) -> tuple[tuple[str, ...], ...]:
    """Return stable, balanced, disjoint contiguous shards.

    Keeping adjacent tests together preserves intentional producer/consumer
    ordering for namespace-local process fixtures while each shard still owns
    a separate PID namespace.
    """

    if shard_count < 1:
        raise ValueError("shard count must be positive")
    if not nodeids:
        raise ValueError("cannot shard an empty test selection")
    selected_count = min(shard_count, len(nodeids))
    short_size, large_count = divmod(len(nodeids), selected_count)
    result: list[tuple[str, ...]] = []
    offset = 0
    for index in range(selected_count):
        size = short_size + int(index < large_count)
        result.append(tuple(nodeids[offset : offset + size]))
        offset += size
    return tuple(result)


def partition_by_weight(
    nodeids: Sequence[str],
    shard_count: int,
    weights: Mapping[str, float],
    default_weight: float,
) -> tuple[tuple[str, ...], ...]:
    """Return disjoint, complete shards balanced by expected duration.

    Longest-first greedy assignment to the least-loaded shard; ties go to the
    earlier test and the lower shard, so the result depends only on the input.
    Each shard keeps collection order.  Count-balanced contiguous shards put
    three of the four slowest ordinary tests in one shard, which simulates at
    2.2 times the length of the shortest.
    """

    if shard_count < 1:
        raise ValueError("shard count must be positive")
    if not nodeids:
        raise ValueError("cannot shard an empty test selection")
    if len(set(nodeids)) != len(nodeids):
        raise ValueError("cannot shard a selection containing duplicate node IDs")
    unknown = sorted(set(weights).difference(nodeids))
    if unknown:
        raise ValueError(
            "duration hints name tests outside the selection; update the hints: "
            + ", ".join(unknown)
        )
    selected_count = min(shard_count, len(nodeids))
    order = sorted(
        range(len(nodeids)),
        key=lambda index: (-weights.get(nodeids[index], default_weight), index),
    )
    loads = [0.0] * selected_count
    members: list[list[int]] = [[] for _ in range(selected_count)]
    for index in order:
        shard = min(range(selected_count), key=lambda candidate: (loads[candidate], candidate))
        loads[shard] += weights.get(nodeids[index], default_weight)
        members[shard].append(index)
    result = tuple(tuple(nodeids[index] for index in sorted(indices)) for indices in members)
    scheduled = [nodeid for shard_nodeids in result for nodeid in shard_nodeids]
    if len(scheduled) != len(nodeids) or set(scheduled) != set(nodeids):
        raise AssertionError("weighted partition lost or duplicated a node ID")
    return result


def ordinary_shard_command(
    python: Path, nodeids: Sequence[str], *, durations: int = 0
) -> list[str]:
    """Build one ordinary-environment pytest shard with the caller's tools.

    No namespace and no Git substitution: this partition exists to observe host
    process identities and the caller's environment exactly as the serial
    command did.
    """

    command = [
        str(python),
        "-m",
        "pytest",
        "-q",
        "-p",
        "no:cacheprovider",
        "-c",
        "pyproject.toml",
        "--rootdir=.",
        "-m",
        _ORDINARY_SELECTION,
    ]
    if durations:
        command.append(f"--durations={durations}")
    command.extend(nodeids)
    return command


def mapped_shard_command(
    python: Path,
    repository: Path,
    nodeids: Sequence[str],
    *,
    init_python: Path,
    tool_directory: Path,
    durations: int = 0,
) -> list[str]:
    """Build one independently isolated pytest shard command.

    ``python`` runs pytest exactly as invoked, so a virtual environment keeps
    its packages.  ``init_python`` is the resolved interpreter that serves as
    the namespace's PID 1; a launcher that forks cannot reap adopted children.
    """

    unshare = shutil.which("unshare")
    if unshare is None:
        raise RuntimeError("unshare disappeared after the caller's availability probe")
    pytest_command = [
        str(python),
        "-m",
        "pytest",
        "-q",
        "-p",
        "no:cacheprovider",
        "-c",
        "pyproject.toml",
        "--rootdir=.",
        "-m",
        _MAPPED_SELECTION,
    ]
    if durations:
        pytest_command.append(f"--durations={durations}")
    pytest_command.extend(nodeids)
    return [
        unshare,
        "--user",
        "--map-root-user",
        "--pid",
        "--fork",
        "--mount-proc",
        str(init_python),
        str(repository / "scripts" / "pid_namespace_init.py"),
        "--",
        "/usr/bin/env",
        f"--chdir={repository / 'py'}",
        # A site Git wrapper earlier on PATH may start an asynchronous
        # `git remote get-url` observer in the checkout.  Parallel lifecycle
        # tests can then correctly see that observer as live slot use after
        # the foreground Git command has returned.  Exercise the system Git
        # in these isolated fixtures so each command's lifetime is explicit.
        # This can change the Git version under test; main() prints the one
        # used, and the ordinary partition still runs the caller's Git.
        f"PATH={tool_directory}{os.pathsep}{os.environ['PATH']}",
        *pytest_command,
    ]


def prepare_tool_directory(path: Path) -> None:
    """Pin only Git while preserving the caller's remaining toolchain."""

    path.mkdir()
    (path / "git").symlink_to("/usr/bin/git")


def pinned_git_version(tool_directory: Path) -> str:
    """Name the Git every mapped shard runs, so the substitution is visible."""

    git = tool_directory / "git"
    completed = subprocess.run(
        [str(git), "--version"], text=True, capture_output=True, check=False
    )
    if completed.returncode != 0 or not completed.stdout.startswith("git version "):
        raise RuntimeError(f"pinned Git {git.readlink()} is not runnable")
    return f"Git pinned to {git.readlink()} ({completed.stdout.strip()})"


def run_commands(
    commands: Sequence[Sequence[str]],
    log_directory: Path,
    *,
    label: str = "mapped lifecycle",
    cwd: Path | None = None,
) -> int:
    """Run every shard concurrently and report every result before failing."""

    processes: list[subprocess.Popen[bytes]] = []
    logs: list[Path] = []
    started = time.monotonic()
    previous = {signum: signal.signal(signum, _raise_stop_signal) for signum in _STOP_SIGNALS}
    try:
        for index, command in enumerate(commands):
            log = log_directory / f"shard-{index}.log"
            logs.append(log)
            with log.open("wb") as output:
                processes.append(
                    subprocess.Popen(
                        list(command),
                        cwd=cwd,
                        stdin=subprocess.DEVNULL,
                        stdout=output,
                        stderr=subprocess.STDOUT,
                        start_new_session=True,
                    )
                )
        elapsed: list[float | None] = [None] * len(processes)
        while any(seconds is None for seconds in elapsed):
            for index, process in enumerate(processes):
                if elapsed[index] is None and process.poll() is not None:
                    elapsed[index] = time.monotonic() - started
            time.sleep(0.2)
        returncodes = [process.wait() for process in processes]
    except BaseException as exc:
        interrupted = isinstance(exc, KeyboardInterrupt)
        _stop_shards(processes, _INTERRUPT_GRACE_SECONDS if interrupted else 0.0, interrupted)
        try:
            for index, process in enumerate(processes):
                _report_shard(
                    f"=== {label} shard {index + 1}/{len(commands)} INTERRUPTED "
                    f"rc={process.returncode} ===",
                    logs[index],
                )
        except Exception:
            pass  # A failed report (a closed pipe) must not replace the original exception.
        if isinstance(exc, _StopSignal):
            raise SystemExit(128 + exc.signum) from exc
        raise
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)
    for index, (returncode, log) in enumerate(zip(returncodes, logs, strict=True)):
        _report_shard(
            f"=== {label} shard {index + 1}/{len(commands)} rc={returncode} "
            f"wall={elapsed[index]:.1f}s ===",
            log,
        )
    return 0 if all(returncode == 0 for returncode in returncodes) else 1


def _stop_shards(
    processes: Sequence[subprocess.Popen[bytes]], grace: float, forward_interrupt: bool
) -> None:
    """Stop every shard's process group: grace, then SIGTERM, then SIGKILL, each bounded.

    A further Ctrl-C or stop signal moves on to the next step; during the SIGKILL step it only
    repeats the SIGKILL, so no number of interrupts can leave a group unsignalled.
    """

    try:
        if forward_interrupt:
            _signal_groups(processes, signal.SIGINT)
        _wait_until(lambda: all(p.poll() is not None for p in processes), grace)
    except (KeyboardInterrupt, _StopSignal):
        pass  # Stop now, not abandon the shards.
    try:
        _signal_groups(processes, signal.SIGTERM)
        if _wait_until(lambda: _groups_ended(processes), _TERMINATE_SECONDS):
            return
    except (KeyboardInterrupt, _StopSignal):
        pass
    deadline = time.monotonic() + _KILL_SECONDS
    while True:
        try:
            _signal_groups(processes, signal.SIGKILL)
            if not _wait_until(lambda: _groups_ended(processes), deadline - time.monotonic()):
                survivors = [str(p.pid) for p in processes if not _groups_ended([p])]
                print(f"shard process groups still present after SIGKILL: {', '.join(survivors)}")
            return
        except (KeyboardInterrupt, _StopSignal):
            continue  # Repeating SIGKILL is harmless, and the deadline does not move.


def _signal_groups(processes: Sequence[subprocess.Popen[bytes]], signum: int) -> None:
    # Each shard leads its own session, so its PID names its process group.  The group outlives
    # the reaped leader while any member remains, and the kernel does not reuse that PID meanwhile.
    for process in processes:
        try:
            os.killpg(process.pid, signum)
        except (ProcessLookupError, PermissionError):
            pass  # The group is empty, or holds nothing this user may signal.


def _groups_ended(processes: Sequence[subprocess.Popen[bytes]]) -> bool:
    for process in processes:
        if process.poll() is None:
            return False
        try:
            os.killpg(process.pid, 0)
        except ProcessLookupError:
            continue
        except PermissionError:
            pass  # A member exists that this user may not signal.
        return False
    return True


def _wait_until(done: Callable[[], bool], seconds: float) -> bool:
    deadline = time.monotonic() + seconds
    while not done():
        if time.monotonic() >= deadline:
            return False
        time.sleep(0.05)
    return True


def _report_shard(header: str, log: Path) -> None:
    print(header)
    contents = log.read_bytes()
    sys.stdout.buffer.write(contents)
    if contents and not contents.endswith(b"\n"):
        print()


def collect_nodeids(
    python: Path, python_root: Path, selection: str
) -> tuple[str, ...]:
    completed = subprocess.run(
        [
            str(python),
            "-m",
            "pytest",
            "--collect-only",
            "-q",
            "-p",
            "no:cacheprovider",
            "-c",
            "pyproject.toml",
            "--rootdir=.",
            _LIFECYCLE_PATH,
            "-m",
            selection,
        ],
        cwd=python_root,
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        sys.stdout.write(completed.stdout)
        sys.stderr.write(completed.stderr)
        raise RuntimeError(f"lifecycle collection for {selection!r} failed")
    prefix = f"{_LIFECYCLE_PATH}::"
    nodeids = tuple(line for line in completed.stdout.splitlines() if line.startswith(prefix))
    if not nodeids:
        raise RuntimeError(f"lifecycle collection for {selection!r} returned no node IDs")
    return nodeids


def run_ordinary(python: Path, python_root: Path, shard_count: int, durations: int) -> int:
    nodeids = collect_nodeids(python, python_root, _ORDINARY_SELECTION)
    shards = partition_by_weight(
        nodeids, shard_count, _ORDINARY_DURATION_HINTS, _ORDINARY_DEFAULT_DURATION
    )
    print(
        f"ordinary lifecycle: {len(nodeids)} tests across {len(shards)} "
        "duration-balanced host-process shards",
        flush=True,
    )
    with tempfile.TemporaryDirectory(prefix="wrkslots-ordinary-lifecycle-shards-") as raw:
        commands = tuple(
            ordinary_shard_command(python, shard, durations=durations) for shard in shards
        )
        return run_commands(
            commands, Path(raw), label="ordinary lifecycle", cwd=python_root
        )


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--partition", choices=("mapped", "ordinary"), default="mapped")
    parser.add_argument("--shards", type=int, default=4)
    parser.add_argument("--durations", type=int, default=0)
    arguments = parser.parse_args(argv)
    repository = Path(__file__).resolve().parents[1]
    python_root = repository / "py"
    python = Path(sys.executable)
    if arguments.partition == "ordinary":
        return run_ordinary(python, python_root, arguments.shards, arguments.durations)
    init_python = Path(os.path.realpath(sys.executable))
    nodeids = collect_nodeids(python, python_root, _MAPPED_SELECTION)
    shards = partition_nodeids(nodeids, arguments.shards)
    print(
        f"mapped lifecycle: {len(nodeids)} tests across {len(shards)} "
        "independent user/PID namespaces",
        flush=True,
    )
    with tempfile.TemporaryDirectory(prefix="wrkslots-lifecycle-shards-") as raw:
        temporary = Path(raw)
        tool_directory = temporary / "tools"
        prepare_tool_directory(tool_directory)
        print(f"mapped lifecycle: {pinned_git_version(tool_directory)}", flush=True)
        commands = tuple(
            mapped_shard_command(
                python,
                repository,
                shard,
                init_python=init_python,
                tool_directory=tool_directory,
                durations=arguments.durations,
            )
            for shard in shards
        )
        return run_commands(commands, temporary)


if __name__ == "__main__":
    raise SystemExit(main())
