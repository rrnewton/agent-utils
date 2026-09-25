from __future__ import annotations

import ctypes
import importlib.util
import json
import os
import select
import shutil
import signal
import subprocess
import sys
import time
from collections.abc import Callable, Iterable, Iterator
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
RUNNER_PATH = ROOT / "scripts" / "run_lifecycle_shards.py"
SPEC = importlib.util.spec_from_file_location("run_lifecycle_shards", RUNNER_PATH)
assert SPEC is not None and SPEC.loader is not None
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


def test_partition_nodeids_is_stable_balanced_disjoint_and_complete() -> None:
    nodeids = tuple(f"test.py::test_case[{index}]" for index in range(11))

    shards = runner.partition_nodeids(nodeids, 4)

    assert shards == (
        nodeids[0:3],
        nodeids[3:6],
        nodeids[6:9],
        nodeids[9:11],
    )
    assert set().union(*(set(shard) for shard in shards)) == set(nodeids)
    assert sum(len(shard) for shard in shards) == len(nodeids)
    assert max(map(len, shards)) - min(map(len, shards)) == 1


def test_partition_nodeids_refuses_invalid_or_empty_work() -> None:
    with pytest.raises(ValueError, match="positive"):
        runner.partition_nodeids(("test.py::test_one",), 0)
    with pytest.raises(ValueError, match="empty"):
        runner.partition_nodeids((), 4)


def test_run_commands_reports_every_shard_and_propagates_failure(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    commands = (
        (sys.executable, "-c", "print('first passed')"),
        (sys.executable, "-c", "print('second failed'); raise SystemExit(7)"),
    )

    returncode = runner.run_commands(commands, tmp_path)

    output = capsys.readouterr().out
    assert returncode == 1
    assert "shard 1/2 rc=0" in output
    assert "first passed" in output
    assert "shard 2/2 rc=7" in output
    assert "second failed" in output


def test_mapped_shard_command_preserves_namespace_selection_and_host_tools() -> None:
    python = Path("/tmp/wrkslots-test-venv/bin/python3")
    init_python = Path("/usr/bin/python3.12")
    tool_directory = Path("/tmp/wrkslots-test-tools")
    command = runner.mapped_shard_command(
        python,
        ROOT,
        ("wrkslots/tests/test_lifecycle.py::test_one",),
        init_python=init_python,
        tool_directory=tool_directory,
        durations=10,
    )

    assert command[:7] == [
        "/usr/bin/unshare",
        "--user",
        "--map-root-user",
        "--pid",
        "--fork",
        "--kill-child=SIGKILL",
        "--mount-proc",
    ]
    # The resolved interpreter is PID 1; pytest keeps the invoking
    # interpreter's path so a virtual environment's packages stay visible.
    assert command[7:10] == [
        str(init_python),
        str(ROOT / "scripts" / "pid_namespace_init.py"),
        "--",
    ]
    assert command[13:16] == [str(python), "-m", "pytest"]
    assert [
        "/usr/bin/env",
        f"--chdir={ROOT / 'py'}",
        f"PATH={tool_directory}:{os.environ['PATH']}",
    ] == command[10:13]
    assert command[-2:] == [
        "--durations=10",
        "wrkslots/tests/test_lifecycle.py::test_one",
    ]


def test_prepare_tool_directory_pins_only_stock_git(tmp_path: Path) -> None:
    tools = tmp_path / "tools"

    runner.prepare_tool_directory(tools)

    assert list(tools.iterdir()) == [tools / "git"]
    assert (tools / "git").readlink() == Path("/usr/bin/git")


def test_main_runs_pytest_with_the_unresolved_interpreter(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    link = tmp_path / "venv-python3"
    link.symlink_to(sys.executable)
    collected: list[Path] = []
    built: list[tuple[Path, Path]] = []

    def collect(python: Path, _root: Path, selection: str) -> tuple[str, ...]:
        assert selection == "not ordinary_environment"
        collected.append(python)
        return ("wrkslots/tests/test_lifecycle.py::test_one",)

    def build(python: Path, *_args: object, init_python: Path, **_kwargs: object) -> list[str]:
        built.append((python, init_python))
        return [sys.executable, "-c", "pass"]

    monkeypatch.setattr(runner.sys, "executable", str(link))
    monkeypatch.setattr(runner, "collect_nodeids", collect)
    monkeypatch.setattr(runner, "mapped_shard_command", build)

    assert runner.main(["--shards", "1"]) == 0

    assert collected == [link]
    assert built == [(link, Path(os.path.realpath(sys.executable)))]


def test_pinned_git_version_names_the_substituted_git(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    runner.prepare_tool_directory(tools)

    reported = runner.pinned_git_version(tools)

    assert reported.startswith("Git pinned to /usr/bin/git (git version ")


def test_partition_by_weight_isolates_heavy_tests_and_keeps_collection_order() -> None:
    nodeids = tuple(f"test.py::test_case[{index}]" for index in range(10))
    weights = {nodeids[3]: 50.0, nodeids[7]: 30.0}

    shards = runner.partition_by_weight(nodeids, 3, weights, 1.0)

    # Longest first: 50 and 30 open two shards, and the eight unit tests all
    # fill the third until it reaches 8, still below the other two.
    assert shards == (
        (nodeids[3],),
        (nodeids[7],),
        tuple(nodeids[index] for index in (0, 1, 2, 4, 5, 6, 8, 9)),
    )
    assert runner.partition_by_weight(nodeids, 3, weights, 1.0) == shards


def test_partition_by_weight_is_disjoint_complete_and_bounded_by_shards() -> None:
    nodeids = tuple(f"test.py::test_case[{index}]" for index in range(11))

    shards = runner.partition_by_weight(nodeids, 4, {}, 1.0)
    scheduled = [nodeid for shard in shards for nodeid in shard]

    assert sorted(scheduled) == sorted(nodeids)
    assert len(scheduled) == len(set(scheduled))
    assert max(map(len, shards)) - min(map(len, shards)) == 1
    assert all(list(shard) == sorted(shard, key=nodeids.index) for shard in shards)
    assert runner.partition_by_weight(nodeids[:2], 4, {}, 1.0) == (
        (nodeids[0],),
        (nodeids[1],),
    )


def test_partition_by_weight_refuses_invalid_duplicate_or_stale_input() -> None:
    with pytest.raises(ValueError, match="positive"):
        runner.partition_by_weight(("test.py::test_one",), 0, {}, 1.0)
    with pytest.raises(ValueError, match="empty"):
        runner.partition_by_weight((), 4, {}, 1.0)
    with pytest.raises(ValueError, match="duplicate"):
        runner.partition_by_weight(("test.py::a", "test.py::a"), 2, {}, 1.0)
    with pytest.raises(ValueError, match="test.py::renamed"):
        runner.partition_by_weight(("test.py::a",), 2, {"test.py::renamed": 9.0}, 1.0)


def test_ordinary_duration_hints_name_collected_ordinary_tests() -> None:
    nodeids = runner.collect_nodeids(
        Path(sys.executable), ROOT / "py", "ordinary_environment"
    )

    shards = runner.partition_by_weight(
        nodeids, 4, runner._ORDINARY_DURATION_HINTS, runner._ORDINARY_DEFAULT_DURATION
    )

    # Each hinted test outweighs a fair share of everything else, so each one
    # lands in a different shard and none waits behind another.
    hinted = set(runner._ORDINARY_DURATION_HINTS)
    assert [len(hinted.intersection(shard)) for shard in shards] == [1, 1, 1, 1]
    assert sorted(nodeid for shard in shards for nodeid in shard) == sorted(nodeids)


def test_ordinary_shard_command_uses_host_processes_and_caller_tools() -> None:
    python = Path("/tmp/wrkslots-test-venv/bin/python3")

    command = runner.ordinary_shard_command(
        python, ("wrkslots/tests/test_lifecycle.py::test_one",), durations=5
    )

    assert command == [
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
        "ordinary_environment",
        "--durations=5",
        "wrkslots/tests/test_lifecycle.py::test_one",
    ]


def test_run_commands_labels_shards_and_honours_cwd(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    workdir = tmp_path / "work"
    workdir.mkdir()
    logs = tmp_path / "logs"
    logs.mkdir()
    commands = ((sys.executable, "-c", "import os; print(os.getcwd())"),)

    assert runner.run_commands(commands, logs, label="ordinary lifecycle", cwd=workdir) == 0

    output = capsys.readouterr().out
    assert "=== ordinary lifecycle shard 1/1 rc=0 wall=" in output
    assert str(workdir) in output


_SHARD_THAT_RECORDS_HOW_IT_ENDED = """
import pathlib, signal, sys, time
marker = pathlib.Path(sys.argv[1])
def on_term(*_):
    marker.write_text("terminated")
    sys.exit(143)
signal.signal(signal.SIGTERM, on_term)
signal.signal(signal.SIGINT, signal.SIG_IGN)
print("shard started", flush=True)
time.sleep(float(sys.argv[2]))
marker.write_text("finished")
"""


def _shard(marker: Path, seconds: float) -> tuple[tuple[str, ...], ...]:
    return ((sys.executable, "-c", _SHARD_THAT_RECORDS_HOW_IT_ENDED, str(marker), str(seconds)),)


def _interrupt_the_poll_loop(
    monkeypatch: pytest.MonkeyPatch, exceptions: list[BaseException], started: Path
) -> None:
    """Raise each exception from a successive sleep once the shard is running."""

    real_sleep = time.sleep
    pending = list(exceptions)

    def sleep(seconds: float) -> None:
        if pending and started.exists() and b"shard started" in started.read_bytes():
            raise pending.pop(0)
        real_sleep(seconds)

    monkeypatch.setattr(runner.time, "sleep", sleep)


def test_interrupt_lets_shards_finish_their_teardown_and_reports_them(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    marker = tmp_path / "ended"
    logs = tmp_path / "logs"
    logs.mkdir()
    _interrupt_the_poll_loop(monkeypatch, [KeyboardInterrupt()], logs / "shard-0.log")

    started = time.monotonic()
    with pytest.raises(KeyboardInterrupt):
        runner.run_commands(_shard(marker, 1.5), logs, label="ordinary lifecycle")

    # The grace ends when the shards do, not when the grace runs out.
    assert time.monotonic() - started < runner._INTERRUPT_GRACE_SECONDS / 3
    assert marker.read_text() == "finished"
    output = capsys.readouterr().out
    assert "=== ordinary lifecycle shard 1/1 INTERRUPTED rc=0 ===" in output
    assert "shard started" in output


def test_second_interrupt_during_the_grace_stops_shards_instead_of_abandoning_them(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    marker = tmp_path / "ended"
    logs = tmp_path / "logs"
    logs.mkdir()
    _interrupt_the_poll_loop(
        monkeypatch, [KeyboardInterrupt(), KeyboardInterrupt()], logs / "shard-0.log"
    )

    started = time.monotonic()
    with pytest.raises(KeyboardInterrupt):
        runner.run_commands(_shard(marker, 60), logs)

    assert time.monotonic() - started < runner._INTERRUPT_GRACE_SECONDS / 3
    assert marker.read_text() == "terminated"
    assert "=== mapped lifecycle shard 1/1 INTERRUPTED rc=143 ===" in capsys.readouterr().out


def test_interrupt_grace_is_bounded_and_other_failures_stop_shards_at_once(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    marker = tmp_path / "ended"
    logs = tmp_path / "logs"
    logs.mkdir()
    _interrupt_the_poll_loop(monkeypatch, [RuntimeError("runner fault")], logs / "shard-0.log")

    started = time.monotonic()
    with pytest.raises(RuntimeError, match="runner fault"):
        runner.run_commands(_shard(marker, 60), logs)

    assert time.monotonic() - started < runner._INTERRUPT_GRACE_SECONDS / 3
    assert marker.read_text() == "terminated"
    assert "=== mapped lifecycle shard 1/1 INTERRUPTED rc=143 ===" in capsys.readouterr().out

    marker.unlink()
    logs.joinpath("shard-0.log").unlink()
    monkeypatch.setattr(runner, "_INTERRUPT_GRACE_SECONDS", 0.3)
    _interrupt_the_poll_loop(monkeypatch, [KeyboardInterrupt()], logs / "shard-0.log")
    with pytest.raises(KeyboardInterrupt):
        runner.run_commands(_shard(marker, 60), logs)
    assert marker.read_text() == "terminated"


def test_a_failed_interrupt_report_does_not_replace_the_original_exception(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    marker = tmp_path / "ended"
    logs = tmp_path / "logs"
    logs.mkdir()
    _interrupt_the_poll_loop(monkeypatch, [RuntimeError("runner fault")], logs / "shard-0.log")

    def closed_pipe(_header: str, _log: Path) -> None:
        raise BrokenPipeError("reader went away")

    monkeypatch.setattr(runner, "_report_shard", closed_pipe)
    with pytest.raises(RuntimeError, match="runner fault"):
        runner.run_commands(_shard(marker, 60), logs)
    assert marker.read_text() == "terminated"


# A real runner process, signalled on its own the way `kill -INT <runner>` or a deadline does, so
# nothing reaches the shards unless the runner forwards it.
_RUNNER_PROCESS = """
import importlib.util, pathlib, signal, sys
signal.signal(signal.SIGINT, signal.default_int_handler)  # even if started with SIGINT ignored
spec = importlib.util.spec_from_file_location("run_lifecycle_shards", sys.argv[1])
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)
commands = ((sys.executable, "-c", *sys.argv[3:]),)
sys.exit(runner.run_commands(commands, pathlib.Path(sys.argv[2]), label="ordinary lifecycle"))
"""

# A shard whose test left a process running.  "leaky" tears down on SIGINT but forgets the
# grandchild; "stubborn" ignores SIGINT and SIGTERM.  The grandchild ignores SIGINT, and SIGTERM too
# when "stubborn", so only a signal to the whole process group can stop it.
_SHARD_WITH_A_GRANDCHILD = """
import os, pathlib, signal, subprocess, sys, time
state, shard_mode, grandchild_mode, grandchild = pathlib.Path(sys.argv[1]), *sys.argv[2:5]
def tear_down_but_forget_the_grandchild(*_):
    (state / "shard-interrupted").write_text("")
    sys.exit(130)
if shard_mode == "stubborn":
    signal.signal(signal.SIGINT, signal.SIG_IGN)
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
else:
    signal.signal(signal.SIGINT, tear_down_but_forget_the_grandchild)
subprocess.Popen([sys.executable, "-c", grandchild, str(state), grandchild_mode])
while not (state / "grandchild.pid").exists():
    time.sleep(0.01)
(state / "shard.tmp").write_text(str(os.getpid()))
(state / "shard.tmp").rename(state / "shard.pid")
print("shard started", flush=True)
time.sleep(120)
"""

_GRANDCHILD = """
import os, pathlib, signal, sys, time
state = pathlib.Path(sys.argv[1])
signal.signal(signal.SIGINT, signal.SIG_IGN)
if sys.argv[2] == "stubborn":
    signal.signal(signal.SIGTERM, signal.SIG_IGN)
(state / "grandchild.tmp").write_text(str(os.getpid()))
(state / "grandchild.tmp").rename(state / "grandchild.pid")
time.sleep(120)
"""


class _RealRunner:
    def __init__(self, tmp_path: Path, shard_mode: str, grandchild_mode: str) -> None:
        self.state = tmp_path / "state"
        self.state.mkdir()
        logs = tmp_path / "logs"
        logs.mkdir()
        self.process = subprocess.Popen(
            [
                sys.executable, "-c", _RUNNER_PROCESS, str(RUNNER_PATH), str(logs),
                _SHARD_WITH_A_GRANDCHILD, str(self.state), shard_mode, grandchild_mode, _GRANDCHILD,
            ],
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        self.pidfds: dict[str, int] = {}
        deadline = time.monotonic() + 30
        while not (self.state / "shard.pid").exists():
            assert self.process.poll() is None, self.process.communicate()[0]
            assert time.monotonic() < deadline, "the shard never started"
            time.sleep(0.02)
        for name in ("shard", "grandchild"):
            self.pidfds[name] = os.pidfd_open(int((self.state / f"{name}.pid").read_text()))

    def signal(self, signum: int) -> None:
        os.kill(self.process.pid, signum)

    def finish(self) -> tuple[int, str]:
        output, _ = self.process.communicate(timeout=60)
        return self.process.returncode, output.decode()

    def exited(self, name: str) -> bool:
        poller = select.poll()
        poller.register(self.pidfds[name], select.POLLIN)
        return bool(poller.poll(0))

    def close(self) -> None:
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait()
        for pidfd in self.pidfds.values():
            try:
                signal.pidfd_send_signal(pidfd, signal.SIGKILL)
            except ProcessLookupError:
                pass
            os.close(pidfd)


@pytest.fixture
def real_runner(tmp_path: Path) -> Iterator[Callable[[str, str], _RealRunner]]:
    started: list[_RealRunner] = []

    def start(shard_mode: str, grandchild_mode: str) -> _RealRunner:
        started.append(_RealRunner(tmp_path, shard_mode, grandchild_mode))
        return started[-1]

    yield start
    for runner_process in started:
        runner_process.close()


def test_runner_only_interrupt_reaches_the_shard_and_kills_the_grandchild_it_left(
    real_runner: Callable[[str, str], _RealRunner],
) -> None:
    run = real_runner("leaky", "stubborn")

    started = time.monotonic()
    run.signal(signal.SIGINT)
    returncode, output = run.finish()

    assert returncode == -signal.SIGINT
    assert (run.state / "shard-interrupted").exists(), "the SIGINT was not forwarded to the shard"
    assert "=== ordinary lifecycle shard 1/1 INTERRUPTED rc=130 ===" in output
    # The runner returned only after the grandchild, which ignores SIGINT and SIGTERM, was gone.
    assert run.exited("shard") and run.exited("grandchild")
    # The grace ended with the shard; SIGTERM then waited its bound, and SIGKILL finished the group.
    assert time.monotonic() - started < runner._INTERRUPT_GRACE_SECONDS / 3


def test_repeated_interrupts_escalate_to_sigkill_and_leave_no_process_behind(
    real_runner: Callable[[str, str], _RealRunner],
) -> None:
    run = real_runner("stubborn", "stubborn")

    started = time.monotonic()
    for _ in range(12):  # Past the grace and the SIGTERM wait, and into the SIGKILL step.
        if run.process.poll() is not None:
            break
        run.signal(signal.SIGINT)
        time.sleep(0.25)
    returncode, output = run.finish()

    assert returncode == -signal.SIGINT
    assert run.exited("shard") and run.exited("grandchild")
    assert f"INTERRUPTED rc={-signal.SIGKILL} ===" in output
    # Each further interrupt skipped a step instead of waiting it out, or escaping it.
    assert time.monotonic() - started < runner._TERMINATE_SECONDS


@pytest.mark.parametrize(
    ("signum", "shard_mode", "grandchild_mode", "shard_rc"),
    (
        (signal.SIGHUP, "leaky", "killable", -signal.SIGTERM),
        (signal.SIGTERM, "stubborn", "stubborn", -signal.SIGKILL),
        (signal.SIGQUIT, "leaky", "killable", -signal.SIGTERM),  # Ctrl-\ at a terminal
    ),
)
def test_a_stop_signal_to_the_runner_stops_every_shard_group_without_a_grace(
    real_runner: Callable[[str, str], _RealRunner], signum: int, shard_mode: str, grandchild_mode: str,
    shard_rc: int,
) -> None:
    run = real_runner(shard_mode, grandchild_mode)

    started = time.monotonic()
    run.signal(signum)
    returncode, output = run.finish()
    elapsed = time.monotonic() - started

    assert returncode == 128 + signum
    assert f"INTERRUPTED rc={shard_rc} ===" in output
    assert run.exited("shard") and run.exited("grandchild")
    assert not (run.state / "shard-interrupted").exists()
    if shard_rc == -signal.SIGTERM:
        assert elapsed < runner._TERMINATE_SECONDS  # SIGTERM sufficed; no SIGKILL wait
    else:
        assert runner._TERMINATE_SECONDS <= elapsed < runner._INTERRUPT_GRACE_SECONDS / 3


_IGNORES_SIGTERM = (
    "import signal, time\n"
    "signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
    "print('ready', flush=True)\n"
    "time.sleep(60)\n"
)


@pytest.mark.parametrize(
    "signum", (signal.SIGINT, signal.SIGTERM), ids=("keyboard", "stop-signal")
)
def test_an_interrupt_inside_the_first_sigkill_sweep_still_kills_every_later_group(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str], signum: int
) -> None:
    """The SIGKILL sweep is one killpg per shard, and a signal can arrive between any two.  The
    kill step must still signal every later group: a signal is only recorded, so the one sweep
    runs to its end."""

    processes = [
        subprocess.Popen(
            (sys.executable, "-c", _IGNORES_SIGTERM),
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            start_new_session=True,
        )
        for _ in range(2)
    ]
    signals = runner._Signals()
    previous = signal.signal(signum, signals.record)
    try:
        for process in processes:
            assert process.stdout is not None and process.stdout.readline() == b"ready\n"
        real_killpg = os.killpg
        kills: list[int] = []

        def killpg(pgid: int, sent: int) -> None:
            real_killpg(pgid, sent)
            if sent == signal.SIGKILL:
                kills.append(pgid)
                if len(kills) == 1:  # Delivered to the first group; the later one not yet.
                    os.kill(os.getpid(), signum)

        monkeypatch.setattr(runner.os, "killpg", killpg)
        monkeypatch.setattr(runner, "_TERMINATE_SECONDS", 0.3)

        runner._stop_shards(processes, runner._Groups(), signals, 0.0, False)

        first, later = (process.pid for process in processes)
        assert signals.received == [signum]  # It really arrived inside the sweep.
        assert kills == [first, later]
        assert [process.returncode for process in processes] == [-signal.SIGKILL] * 2
        assert "still present after SIGKILL" not in capsys.readouterr().out
    finally:
        signal.signal(signum, previous)
        for process in processes:
            if process.poll() is None:
                process.kill()
                process.wait()
            if process.stdout is not None:
                process.stdout.close()


_IGNORES_INT_AND_TERM = (
    "import signal, time\n"
    "signal.signal(signal.SIGINT, signal.SIG_IGN)\n"
    "signal.signal(signal.SIGTERM, signal.SIG_IGN)\n"
    "print('shard started', flush=True)\n"
    "time.sleep(60)\n"
)


class _StartedShards:
    """Replaces the runner's Popen to keep a handle on every shard it starts, recorded or not."""

    def __init__(self, monkeypatch: pytest.MonkeyPatch) -> None:
        self.started: list[subprocess.Popen[bytes]] = []
        self.after_start: Callable[[int], None] = lambda _count: None
        # Hold each start until the shard has logged this, so its signal dispositions are set.
        self.wait_for: bytes | None = None
        shards = self

        class Popen(subprocess.Popen[bytes]):
            def __init__(self, *args: object, **kwargs: object) -> None:
                super().__init__(*args, **kwargs)  # type: ignore[call-overload]
                if shards.wait_for is not None:
                    log = Path(getattr(kwargs["stdout"], "name"))
                    deadline = time.monotonic() + 30
                    while shards.wait_for not in log.read_bytes():
                        assert self.poll() is None and time.monotonic() < deadline
                        time.sleep(0.01)
                shards.started.append(self)
                shards.after_start(len(shards.started))

        monkeypatch.setattr(runner.subprocess, "Popen", Popen)

    def assert_every_group_gone(self) -> None:
        for process in self.started:
            assert process.returncode is not None, f"shard {process.pid} was never reaped"
            with pytest.raises(ProcessLookupError):
                os.killpg(process.pid, 0)

    def close(self) -> None:
        for process in self.started:
            if process.poll() is None:
                process.kill()
                process.wait()


@pytest.fixture
def started_shards(monkeypatch: pytest.MonkeyPatch) -> Iterator[_StartedShards]:
    shards = _StartedShards(monkeypatch)
    yield shards
    shards.close()


def _runner_handles(signum: int) -> bool:
    """Whether the runner's recording handler would receive ``signum`` now; while the runner
    holds it blocked, it would reach whatever handler is restored next."""

    return isinstance(
        getattr(signal.getsignal(signum), "__self__", None), runner._Signals
    ) and signum not in signal.pthread_sigmask(signal.SIG_BLOCK, ())


@pytest.mark.parametrize("signum", (signal.SIGINT, signal.SIGTERM))
@pytest.mark.parametrize("from_shard", (1, 2), ids=("inside-startup", "after-startup"))
def test_a_stop_signal_at_every_line_of_the_start_and_stop_paths_cannot_escape_them(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    started_shards: _StartedShards,
    signum: int,
    from_shard: int,
) -> None:
    """A signal handler runs between any two bytecodes.  Deliver a real signal at every line the
    runner executes once ``from_shard`` shards have started, until its handlers are restored: at
    each one a raising handler would have left a started shard unrecorded or a group unsignalled.
    The shards ignore SIGINT and SIGTERM, so only the SIGKILL step stops them."""

    monkeypatch.setattr(runner, "_TERMINATE_SECONDS", 0.3)
    started_shards.wait_for = b"shard started"
    commands = ((sys.executable, "-c", _IGNORES_INT_AND_TERM),) * 2
    me = os.getpid()
    delivered: list[int] = []

    def line(frame: object, event: str, _arg: object) -> object:
        if (
            event == "line"
            and os.getpid() == me  # Not in a forked child before its exec.
            and len(started_shards.started) >= from_shard
            and _runner_handles(signum)
        ):
            delivered.append(signum)
            os.kill(me, signum)
        return line

    def call(frame: object, _event: str, _arg: object) -> object:
        code = getattr(frame, "f_code")
        if getattr(frame, "f_globals") is vars(runner) and code.co_name != "record":
            return line
        return None

    logs = tmp_path / "logs"
    logs.mkdir()
    previous_trace = sys.gettrace()
    expected = KeyboardInterrupt if signum == signal.SIGINT else SystemExit
    with pytest.raises(expected) as raised:
        sys.settrace(call)  # type: ignore[arg-type]
        try:
            runner.run_commands(commands, logs)
        finally:
            sys.settrace(previous_trace)

    if signum == signal.SIGTERM:
        assert isinstance(raised.value, SystemExit) and raised.value.code == 128 + signal.SIGTERM
    assert len(delivered) > 10
    assert len(started_shards.started) == from_shard
    started_shards.assert_every_group_gone()
    output = capsys.readouterr().out
    assert output.count(f"INTERRUPTED rc={-signal.SIGKILL} ===") == from_shard
    assert ("shards 2-2/2 NOT STARTED" in output) == (from_shard == 1)
    assert "still present after SIGKILL" not in output


def test_a_stop_signal_inside_popen_neither_loses_that_shard_nor_starts_another(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    started_shards: _StartedShards,
) -> None:
    """The review's reproduction: the signal is handled after the second shard is running and
    before Popen has returned it to the runner."""

    monkeypatch.setattr(runner, "_TERMINATE_SECONDS", 0.3)

    def signal_the_runner(count: int) -> None:
        if count == 2:
            os.kill(os.getpid(), signal.SIGTERM)

    started_shards.after_start = signal_the_runner
    started_shards.wait_for = b"shard started"
    logs = tmp_path / "logs"
    logs.mkdir()
    commands = ((sys.executable, "-c", _IGNORES_INT_AND_TERM),) * 3

    with pytest.raises(SystemExit) as raised:
        runner.run_commands(commands, logs)

    assert raised.value.code == 128 + signal.SIGTERM
    assert len(started_shards.started) == 2
    started_shards.assert_every_group_gone()
    output = capsys.readouterr().out
    assert "=== mapped lifecycle shard 2/3 INTERRUPTED rc=-9 ===" in output
    assert "=== mapped lifecycle shards 3-3/3 NOT STARTED ===" in output


def test_a_finished_shard_group_proven_empty_is_never_signalled_again(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, started_shards: _StartedShards
) -> None:
    """Once a shard's group is empty its number may be reused, possibly by an unrelated group."""

    real_killpg = os.killpg
    calls: list[tuple[int, int]] = []
    proven_empty: list[int] = []

    def killpg(pgid: int, signum: int) -> None:
        calls.append((pgid, signum))
        try:
            real_killpg(pgid, signum)
        except ProcessLookupError:
            if signum == 0:
                proven_empty.append(pgid)
            raise

    faulted: list[bool] = []
    real_sleep = time.sleep

    def sleep(seconds: float) -> None:
        # Fault the runner once the finished shard has been proven empty, which stops the rest.
        if not faulted and started_shards.started and started_shards.started[0].pid in proven_empty:
            faulted.append(True)
            raise RuntimeError("runner fault")
        real_sleep(seconds)

    monkeypatch.setattr(runner.os, "killpg", killpg)
    monkeypatch.setattr(runner.time, "sleep", sleep)
    marker = tmp_path / "ended"
    logs = tmp_path / "logs"
    logs.mkdir()
    commands = ((sys.executable, "-c", "pass"), *_shard(marker, 60))

    with pytest.raises(RuntimeError, match="runner fault"):
        runner.run_commands(commands, logs)

    finished, running = (process.pid for process in started_shards.started)
    after_proof = calls[calls.index((finished, 0)) + 1 :]
    assert [call for call in after_proof if call[0] == finished] == []
    assert (running, signal.SIGTERM) in after_proof
    assert marker.read_text() == "terminated"


_EXITS_LEAVING_A_CHILD = (
    "import subprocess, sys\n"
    "child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(0.3)'])\n"
    "print('left', child.pid, flush=True)\n"
)


def test_a_group_that_empties_after_its_leader_ended_is_proven_empty_before_its_number_is_reused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, started_shards: _StartedShards
) -> None:
    """A shard can end while a process it left keeps its group alive.  The poll loop must keep
    looking at that group, so that it is known empty from the first look after its last member
    goes, rather than from the first stop signal, by which time the number may lead another
    group.  Here the number is reused, in effect, one poll after the group really emptied."""

    real_killpg = os.killpg
    real_sleep = time.sleep
    reused: list[bool] = []
    sent_after_reuse: list[tuple[int, int]] = []
    seen_empty: list[bool] = []

    def finished() -> int:
        return started_shards.started[0].pid

    def killpg(pgid: int, signum: int) -> None:
        if reused and pgid == finished():
            sent_after_reuse.append((pgid, signum))
            return  # An unrelated group now answers to this number.
        real_killpg(pgid, signum)

    def sleep(seconds: float) -> None:
        if reused or len(started_shards.started) < 2 or started_shards.started[0].poll() is None:
            real_sleep(seconds)
            return
        if seen_empty:
            # The runner has looked at every shard since the group was seen empty.  Reuse the
            # number, then fault the runner so that it stops the running shard.
            reused.append(True)
            raise RuntimeError("runner fault")
        try:
            real_killpg(finished(), 0)
        except ProcessLookupError:
            seen_empty.append(True)
        real_sleep(seconds)

    monkeypatch.setattr(runner.os, "killpg", killpg)
    monkeypatch.setattr(runner.time, "sleep", sleep)
    marker = tmp_path / "ended"
    logs = tmp_path / "logs"
    logs.mkdir()
    commands = ((sys.executable, "-c", _EXITS_LEAVING_A_CHILD), *_shard(marker, 60))

    with pytest.raises(RuntimeError, match="runner fault"):
        runner.run_commands(commands, logs)

    assert (logs / "shard-0.log").read_bytes().startswith(b"left ")
    assert reused, "the finished shard's group never emptied"
    assert sent_after_reuse == []
    assert marker.read_text() == "terminated"


def test_a_shard_whose_runner_died_before_it_asked_for_a_parent_death_signal_exits() -> None:
    """PR_SET_PDEATHSIG only fires for a parent that dies after it is set.  A shard whose runner
    died first has already been adopted, so it must notice and exit instead of running unwatched.
    """

    read_end, write_end = os.pipe()
    child = os.fork()
    if child == 0:  # The runner that dies.
        try:
            os.close(read_end)
            dying_runner = os.getpid()
            if os.fork() == 0:  # Its shard, before exec.
                try:
                    deadline = time.monotonic() + 10
                    while os.getppid() == dying_runner and time.monotonic() < deadline:
                        time.sleep(0.01)
                    runner._signal_on_parent_death(dying_runner, signal.SIGTERM)()
                    os.write(write_end, b"ran on unwatched")
                finally:
                    os._exit(0)
        finally:
            os._exit(0)
    os.close(write_end)
    os.waitpid(child, 0)
    with os.fdopen(read_end, "rb") as shard_output:
        assert shard_output.read() == b""  # EOF once the orphaned shard exited.

    # Control: with its runner alive the shard carries on, and the request is in force.
    read_end, write_end = os.pipe()
    parent = os.getpid()
    child = os.fork()
    if child == 0:
        try:
            os.close(read_end)
            runner._signal_on_parent_death(parent, signal.SIGTERM)()
            requested = ctypes.c_int()
            runner._LIBC.prctl(2, ctypes.byref(requested), 0, 0, 0)  # PR_GET_PDEATHSIG
            os.write(write_end, str(requested.value).encode())
        finally:
            os._exit(0)
    os.close(write_end)
    os.waitpid(child, 0)
    with os.fdopen(read_end, "rb") as shard_output:
        assert shard_output.read() == str(int(signal.SIGTERM)).encode()


class _CallerStop(Exception):
    pass


@pytest.mark.parametrize("while_", ("installing", "restoring"))
def test_a_signal_while_handlers_change_leaves_the_caller_with_all_its_own_handlers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, while_: str
) -> None:
    """If a signal reached a half-changed set, the caller's raising handler could leave the
    runner's recorder installed for good, silently swallowing that signal in the caller."""

    def caller_stop(_signum: int, _frame: object) -> None:
        raise _CallerStop

    real_signal = signal.signal
    stop_signals = runner._STOP_SIGNALS
    previous_term = real_signal(signal.SIGTERM, caller_stop)
    try:
        own = {signum: signal.getsignal(signum) for signum in stop_signals}
        sent: list[bool] = []

        def changing(signum: int, handler: object) -> object:
            result = real_signal(signum, handler)  # type: ignore[arg-type]
            recorder = isinstance(getattr(handler, "__self__", None), runner._Signals)
            if not sent and signum == signal.SIGINT and recorder == (while_ == "installing"):
                sent.append(True)
                # Installing: the next handler is still the caller's.  Restoring: the rest are
                # still the recorder.
                os.kill(os.getpid(), signal.SIGTERM if while_ == "installing" else signal.SIGINT)
            return result

        monkeypatch.setattr(runner.signal, "signal", changing)
        logs = tmp_path / "logs"
        logs.mkdir()
        with pytest.raises(SystemExit if while_ == "installing" else KeyboardInterrupt) as raised:
            runner.run_commands(((sys.executable, "-c", "pass"),), logs)
        monkeypatch.undo()

        assert sent
        if while_ == "installing":  # Held back until the recorder had it: nothing started.
            assert isinstance(raised.value, SystemExit)
            assert raised.value.code == 128 + signal.SIGTERM
        assert {signum: signal.getsignal(signum) for signum in stop_signals} == own
        assert not set(stop_signals) & signal.pthread_sigmask(signal.SIG_BLOCK, ())
    finally:
        real_signal(signal.SIGTERM, previous_term)


def test_a_stop_signal_after_the_last_shard_ended_is_still_honoured(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """A signal that arrives after the poll loop's last look and before the handlers are
    restored reaches only the record.  The run still ends as that signal asks."""

    real_mask = signal.pthread_sigmask
    blocks: list[int] = []

    def pthread_sigmask(how: int, mask: Iterable[int]) -> set[int | signal.Signals]:
        if how == signal.SIG_BLOCK:
            blocks.append(how)
            if len(blocks) == 2:  # The restoring, not the installing.
                os.kill(os.getpid(), signal.SIGHUP)
        return real_mask(how, mask)

    monkeypatch.setattr(runner.signal, "pthread_sigmask", pthread_sigmask)
    logs = tmp_path / "logs"
    logs.mkdir()

    with pytest.raises(SystemExit) as raised:
        runner.run_commands(((sys.executable, "-c", "print('done')"),), logs)

    assert raised.value.code == 128 + signal.SIGHUP
    output = capsys.readouterr().out
    assert "=== mapped lifecycle shard 1/1 rc=0 wall=" in output
    assert "done" in output


def test_another_interrupt_during_the_report_ends_the_report_not_the_exit_status(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    marker = tmp_path / "ended"
    logs = tmp_path / "logs"
    logs.mkdir()
    real_sleep = time.sleep

    sent: list[bool] = []

    def sleep(seconds: float) -> None:
        if not sent and b"shard started" in (logs / "shard-0.log").read_bytes():
            sent.append(True)
            os.kill(os.getpid(), signal.SIGTERM)
        real_sleep(seconds)

    reports: list[str] = []

    def interrupted_report(header: str, _log: Path) -> None:
        reports.append(header)
        raise KeyboardInterrupt  # Ctrl-C, now under the caller's own handler

    monkeypatch.setattr(runner.time, "sleep", sleep)
    monkeypatch.setattr(runner, "_report_shard", interrupted_report)
    commands = (*_shard(marker, 60), *_shard(tmp_path / "second", 60))

    try:
        with pytest.raises(SystemExit) as raised:
            runner.run_commands(commands, logs)
    except KeyboardInterrupt:
        pytest.fail("the Ctrl-C during the report replaced the exit status")

    assert raised.value.code == 128 + signal.SIGTERM
    assert reports == ["=== mapped lifecycle shard 1/2 INTERRUPTED rc=143 ==="]
    assert marker.read_text() == "terminated"


def test_a_runner_killed_outright_interrupts_its_ordinary_shard_but_not_what_the_shard_leaked(
    real_runner: Callable[[str, str], _RealRunner],
) -> None:
    """Nothing survives the runner to escalate, so an ordinary shard gets one SIGINT from the
    kernel and its own teardown must do the rest.  What the teardown leaves in the group stays:
    the limitation stated at _ORDINARY_PARENT_DEATH_SIGNAL, pinned here so it changes knowingly."""

    run = real_runner("leaky", "killable")

    run.process.kill()
    returncode, _ = run.finish()
    deadline = time.monotonic() + 10
    while not run.exited("shard"):
        assert time.monotonic() < deadline, "the shard outlived its runner"
        time.sleep(0.02)

    assert returncode == -signal.SIGKILL
    assert (run.state / "shard-interrupted").exists(), "the shard's teardown never ran"
    time.sleep(0.5)
    assert not run.exited("grandchild")


def _namespaces_available() -> bool:
    unshare = shutil.which("unshare")
    if unshare is None:
        return False
    probe = subprocess.run(
        [unshare, "--user", "--map-root-user", "--pid", "--fork", "--kill-child=SIGKILL",
         "--mount-proc", "true"],
        capture_output=True,
        check=False,
    )
    return probe.returncode == 0


def _host_processes_naming(token: bytes) -> list[int]:
    pids = []
    for entry in Path("/proc").iterdir():
        if entry.name.isdigit():
            try:
                if token in (entry / "cmdline").read_bytes():
                    pids.append(int(entry.name))
            except OSError:
                pass  # Gone meanwhile.
    return pids


_RUNNER_OF_ONE_COMMAND = """
import importlib.util, json, pathlib, sys
spec = importlib.util.spec_from_file_location("run_lifecycle_shards", sys.argv[1])
runner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(runner)
command = json.loads(sys.argv[3])
sys.exit(runner.run_commands((command,), pathlib.Path(sys.argv[2]), parent_death_signal=int(sys.argv[4])))
"""


@pytest.mark.skipif(not _namespaces_available(), reason="needs unprivileged user and PID namespaces")
def test_a_runner_killed_outright_takes_its_mapped_shard_namespace_with_it(tmp_path: Path) -> None:
    """The namespace holds everything a mapped shard started, whatever it ignores; its PID 1
    dies with `unshare`, which dies with the runner."""

    state = tmp_path / "state"
    state.mkdir()
    logs = tmp_path / "logs"
    logs.mkdir()
    unshare = shutil.which("unshare")
    assert unshare is not None
    command = [
        *runner.namespace_prefix(unshare, ROOT, Path(os.path.realpath(sys.executable))),
        sys.executable, "-c", _SHARD_WITH_A_GRANDCHILD, str(state), "stubborn", "stubborn",
        _GRANDCHILD,
    ]
    token = str(state).encode()
    process = subprocess.Popen(
        [
            sys.executable, "-c", _RUNNER_OF_ONE_COMMAND, str(RUNNER_PATH), str(logs),
            json.dumps(command), str(int(runner._MAPPED_PARENT_DEATH_SIGNAL)),
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    try:
        deadline = time.monotonic() + 30
        while not (state / "shard.pid").exists():
            assert process.poll() is None, "the runner ended before the shard started"
            assert time.monotonic() < deadline, "the shard never started"
            time.sleep(0.02)
        # The runner, unshare, PID 1, the shard and its grandchild.
        assert len(_host_processes_naming(token)) >= 5

        process.kill()
        process.wait()
        deadline = time.monotonic() + 10
        while survivors := _host_processes_naming(token):
            assert time.monotonic() < deadline, f"processes outlived the runner: {survivors}"
            time.sleep(0.05)
    finally:
        if process.poll() is None:
            process.kill()
            process.wait()
        for pid in _host_processes_naming(token):
            try:
                pidfd = os.pidfd_open(pid)
            except ProcessLookupError:
                continue
            try:
                if token in Path(f"/proc/{pid}/cmdline").read_bytes():
                    signal.pidfd_send_signal(pidfd, signal.SIGKILL)
            except (OSError, ProcessLookupError):
                pass
            finally:
                os.close(pidfd)


def test_main_runs_ordinary_partition_with_the_unresolved_interpreter(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    link = tmp_path / "venv-python3"
    link.symlink_to(sys.executable)
    selections: list[tuple[Path, str]] = []
    ran: list[tuple[tuple[list[str], ...], str, Path | None]] = []
    nodeids = tuple(
        f"wrkslots/tests/test_lifecycle.py::test_{index}" for index in range(3)
    )

    def collect(python: Path, _root: Path, selection: str) -> tuple[str, ...]:
        selections.append((python, selection))
        return nodeids

    death_signals: list[int] = []

    def run(
        commands: tuple[list[str], ...],
        _logs: Path,
        *,
        label: str,
        cwd: Path | None = None,
        parent_death_signal: int,
    ) -> int:
        ran.append((commands, label, cwd))
        death_signals.append(parent_death_signal)
        return 0

    monkeypatch.setattr(runner.sys, "executable", str(link))
    monkeypatch.setattr(runner, "collect_nodeids", collect)
    monkeypatch.setattr(runner, "_ORDINARY_DURATION_HINTS", {})
    monkeypatch.setattr(runner, "run_commands", run)

    assert runner.main(["--partition", "ordinary", "--shards", "2"]) == 0

    assert selections == [(link, "ordinary_environment")]
    [(commands, label, cwd)] = ran
    assert label == "ordinary lifecycle"
    assert cwd == ROOT / "py"
    assert [command[0] for command in commands] == [str(link), str(link)]
    assert [command[command.index("ordinary_environment") + 1 :] for command in commands] == [
        [nodeids[0], nodeids[2]],
        [nodeids[1]],
    ]
    assert "unshare" not in " ".join(" ".join(command) for command in commands)
    assert death_signals == [signal.SIGINT]


def test_main_runs_mapped_partition_with_a_sigkill_parent_death_signal(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    ran: list[tuple[tuple[list[str], ...], int]] = []
    nodeids = tuple(f"wrkslots/tests/test_lifecycle.py::test_{index}" for index in range(2))

    def run(commands: tuple[list[str], ...], _logs: Path, *, parent_death_signal: int) -> int:
        ran.append((commands, parent_death_signal))
        return 0

    monkeypatch.setattr(runner, "collect_nodeids", lambda *_: nodeids)
    monkeypatch.setattr(runner, "run_commands", run)
    monkeypatch.setattr(runner.tempfile, "tempdir", str(tmp_path))
    monkeypatch.setattr(runner.shutil, "which", lambda _name: "/usr/bin/unshare")

    assert runner.main(["--shards", "2"]) == 0

    [(commands, death_signal)] = ran
    assert death_signal == signal.SIGKILL
    assert [command[5] for command in commands] == ["--kill-child=SIGKILL"] * 2
