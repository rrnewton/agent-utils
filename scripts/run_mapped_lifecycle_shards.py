#!/usr/bin/env python3
"""Run mapped-namespace lifecycle tests in deterministic parallel shards."""

from __future__ import annotations

import argparse
import os
import shutil
import subprocess
import sys
import tempfile
from collections.abc import Sequence
from pathlib import Path


_LIFECYCLE_PATH = "wrkslots/tests/test_lifecycle.py"


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
        "not ordinary_environment",
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


def run_commands(commands: Sequence[Sequence[str]], log_directory: Path) -> int:
    """Run every shard concurrently and report every result before failing."""

    processes: list[subprocess.Popen[bytes]] = []
    logs: list[Path] = []
    try:
        for index, command in enumerate(commands):
            log = log_directory / f"shard-{index}.log"
            with log.open("wb") as output:
                processes.append(
                    subprocess.Popen(
                        list(command),
                        stdin=subprocess.DEVNULL,
                        stdout=output,
                        stderr=subprocess.STDOUT,
                    )
                )
            logs.append(log)
        returncodes = [process.wait() for process in processes]
    except BaseException:
        for process in processes:
            if process.poll() is None:
                process.terminate()
        for process in processes:
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
        raise
    for index, (returncode, log) in enumerate(zip(returncodes, logs, strict=True)):
        print(f"=== mapped lifecycle shard {index + 1}/{len(commands)} rc={returncode} ===")
        contents = log.read_bytes()
        sys.stdout.buffer.write(contents)
        if contents and not contents.endswith(b"\n"):
            print()
    return 0 if all(returncode == 0 for returncode in returncodes) else 1


def collect_mapped_nodeids(python: Path, python_root: Path) -> tuple[str, ...]:
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
            "not ordinary_environment",
        ],
        cwd=python_root,
        text=True,
        capture_output=True,
        check=False,
    )
    if completed.returncode != 0:
        sys.stdout.write(completed.stdout)
        sys.stderr.write(completed.stderr)
        raise RuntimeError("mapped lifecycle collection failed")
    prefix = f"{_LIFECYCLE_PATH}::"
    nodeids = tuple(line for line in completed.stdout.splitlines() if line.startswith(prefix))
    if not nodeids:
        raise RuntimeError("mapped lifecycle collection returned no node IDs")
    return nodeids


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--shards", type=int, default=4)
    parser.add_argument("--durations", type=int, default=0)
    arguments = parser.parse_args(argv)
    repository = Path(__file__).resolve().parents[1]
    python_root = repository / "py"
    python = Path(sys.executable)
    init_python = Path(os.path.realpath(sys.executable))
    nodeids = collect_mapped_nodeids(python, python_root)
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
