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
from types import FrameType

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


def _taken_if_pending(*signums: int) -> list[int]:
    """Which of signums are pending for this process, each taken so that it is never delivered."""

    pending = signal.sigpending()
    taken = [signum for signum in signums if signum in pending]
    for signum in taken:
        signal.sigtimedwait((signum,), 0)
    return taken


def _stop_signal_check() -> Callable[[], None]:
    """A check for each wait of the runner, failing once if a stop signal is blocked or pending.

    It waits only with the caller's mask, which here blocks none of them, so one held back would
    act only after every shard had run out its sleep: a runner that left them blocked would take
    minutes to fail.  A pending one is taken, so that it never reaches pytest's own handler.  Only
    the first failure raises, so that the runner still stops its shards.
    """

    failed: list[bool] = []

    def check() -> None:
        if failed:
            return
        blocked = set(runner._STOP_SIGNALS) & signal.pthread_sigmask(signal.SIG_BLOCK, ())
        taken = _taken_if_pending(*runner._STOP_SIGNALS)
        if blocked or taken:
            failed.append(True)
        assert not failed, f"stop signals blocked while shards ran: {blocked} {taken}"

    return check


def _interrupt_the_poll_loop(
    monkeypatch: pytest.MonkeyPatch, exceptions: list[BaseException], started: Path
) -> None:
    """Raise each exception from a successive sleep once the shard is running."""

    real_sleep = time.sleep
    pending = list(exceptions)
    check = _stop_signal_check()

    def sleep(seconds: float) -> None:
        if pending and started.exists() and b"shard started" in started.read_bytes():
            # Taken first, so that none is raised later, from inside the stopping of the shards.
            exception = pending.pop(0)
            check()
            raise exception
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


def test_a_fault_stops_shards_at_once_even_after_a_recorded_interrupt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """A fault means the runner cannot be trusted to wait out a grace, whatever else arrived."""

    marker = tmp_path / "ended"
    logs = tmp_path / "logs"
    logs.mkdir()
    real_sleep = time.sleep
    recorded: list[int] = []
    sent: list[bool] = []

    def sleep(seconds: float) -> None:
        log = logs / "shard-0.log"
        if not sent and log.exists() and b"shard started" in log.read_bytes():
            sent.append(True)
            os.kill(os.getpid(), signal.SIGINT)  # Ctrl-C: recorded, not raised.
            assert not _taken_if_pending(signal.SIGINT), "stop signals blocked while shards ran"
            recorder = getattr(signal.getsignal(signal.SIGINT), "__self__", None)
            assert isinstance(recorder, runner._Signals)
            recorded.extend(recorder.received)
            raise RuntimeError("runner fault")
        real_sleep(seconds)

    monkeypatch.setattr(runner.time, "sleep", sleep)
    started = time.monotonic()
    with pytest.raises(RuntimeError, match="runner fault"):
        runner.run_commands(_shard(marker, 60), logs)

    assert recorded == [signal.SIGINT]
    assert time.monotonic() - started < runner._INTERRUPT_GRACE_SECONDS / 3
    assert marker.read_text() == "terminated"
    assert "=== mapped lifecycle shard 1/1 INTERRUPTED rc=143 ===" in capsys.readouterr().out


# Takes its signals one at a time rather than in handlers, which Python may nest: a SIGTERM
# handler that exits inside the SIGINT one would lose the SIGINT's note.
_SHARD_THAT_LISTS_ITS_SIGNALS = """
import signal, sys
wanted = {signal.SIGINT, signal.SIGTERM}
signal.pthread_sigmask(signal.SIG_BLOCK, wanted)
print("shard started", flush=True)
with open(sys.argv[1], "a") as out:
    while True:
        signum = signal.sigwait(wanted)
        out.write(signal.Signals(signum).name + "\\n")
        out.flush()
        if signum == signal.SIGTERM:
            sys.exit(143)
"""


@pytest.mark.parametrize(
    ("first", "second"),
    ((signal.SIGINT, signal.SIGTERM), (signal.SIGTERM, signal.SIGINT)),
    ids=("sigint-then-sigterm", "sigterm-then-sigint"),
)
def test_the_first_of_two_stop_signals_decides_the_grace_and_the_exit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, first: signal.Signals, second: signal.Signals
) -> None:
    """Two stop signals recorded before the runner acts: the first decides.  A Ctrl-C followed
    by a SIGTERM forwards the Ctrl-C (the SIGTERM then cuts its grace short), and the run exits as
    for a Ctrl-C.  A SIGTERM followed by a Ctrl-C forwards nothing and exits as for the SIGTERM."""

    received = tmp_path / "received"
    logs = tmp_path / "logs"
    logs.mkdir()
    real_sleep = time.sleep
    recorded: list[int] = []
    sent: list[bool] = []

    def sleep(seconds: float) -> None:
        log = logs / "shard-0.log"
        if not sent and log.exists() and b"shard started" in log.read_bytes():
            sent.append(True)
            os.kill(os.getpid(), first)
            os.kill(os.getpid(), second)
            recorder = getattr(signal.getsignal(signal.SIGINT), "__self__", None)
            assert isinstance(recorder, runner._Signals)
            recorded.extend(recorder.received)
            # Sent once: a runner that left them blocked would otherwise be sent them at every
            # poll, while the shard waits for them forever.  Taken, they fail this test at once
            # instead of ending pytest when the caller's handlers go back.
            assert not _taken_if_pending(first, second), "stop signals blocked while shards ran"
        real_sleep(seconds)

    monkeypatch.setattr(runner.time, "sleep", sleep)
    command = (sys.executable, "-c", _SHARD_THAT_LISTS_ITS_SIGNALS, str(received))
    started = time.monotonic()
    with pytest.raises((KeyboardInterrupt, SystemExit)) as raised:
        runner.run_commands((command,), logs)

    assert recorded == [first, second]
    assert time.monotonic() - started < runner._INTERRUPT_GRACE_SECONDS / 3
    if first == signal.SIGINT:
        assert raised.type is KeyboardInterrupt
        assert received.read_text().split() == ["SIGINT", "SIGTERM"]
    else:
        assert isinstance(raised.value, SystemExit)
        assert raised.value.code == 128 + signal.SIGTERM
        # The Ctrl-C is not forwarded; it only cuts the SIGTERM's wait short, so a SIGKILL follows
        # at once and may land before the shard has noted the SIGTERM.
        noted = received.read_text().split() if received.exists() else []
        assert noted in ([], ["SIGTERM"])


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


def _held_back(pid: int) -> set[int]:
    """The signals that process pid has pending and blocked, as its /proc status shows them."""

    try:
        status = Path(f"/proc/{pid}/status").read_text()
    except OSError:
        return set()
    fields = dict(line.split(":", 1) for line in status.splitlines() if ":" in line)
    pending = int(fields["SigPnd"], 16) | int(fields["ShdPnd"], 16)
    held = pending & int(fields["SigBlk"], 16)
    return {signum for signum in range(1, 65) if held >> (signum - 1) & 1}


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
        self.sent: set[int] = set()
        deadline = time.monotonic() + 30
        while not (self.state / "shard.pid").exists():
            assert self.process.poll() is None, self.process.communicate()[0]
            assert time.monotonic() < deadline, "the shard never started"
            time.sleep(0.02)
        for name in ("shard", "grandchild"):
            self.pidfds[name] = os.pidfd_open(int((self.state / f"{name}.pid").read_text()))

    def signal(self, signum: int) -> None:
        self.sent.add(signum)
        os.kill(self.process.pid, signum)

    def finish(self) -> tuple[int, str]:
        deadline = time.monotonic() + 60
        held_since: float | None = None
        while True:
            try:
                output, _ = self.process.communicate(timeout=0.05)
            except subprocess.TimeoutExpired:
                assert time.monotonic() < deadline, "the runner never ended"
            else:
                return self.process.returncode, output.decode()
            # The runner holds the stop signals back only while its handlers change, which takes
            # microseconds.  One held at every look for a second was left blocked, and would act
            # only once the shard had slept out its two minutes.
            held = self.sent & _held_back(self.process.pid)
            now = time.monotonic()
            if not held:
                held_since = None
            elif held_since is None:
                held_since = now
            assert held_since is None or now - held_since < 1, f"the runner holds back {held}"

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
    "signum", (signal.SIGINT, signal.SIGTERM), ids=("sigint", "sigterm")
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
                    try:
                        while shards.wait_for not in log.read_bytes():
                            assert self.poll() is None and time.monotonic() < deadline
                            time.sleep(0.01)
                    except BaseException:
                        # Nothing else has a handle on this shard yet: the runner gets none when
                        # Popen raises, and close() sees only the recorded ones.
                        self.kill()
                        self.wait()
                        raise
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

    real_sleep = time.sleep
    check = _stop_signal_check()

    def sleep(seconds: float) -> None:
        # A runner that left the signal blocked would give no line a chance to deliver it, and
        # would wait out the shards' minute before the test could fail.
        check()
        real_sleep(seconds)

    monkeypatch.setattr(runner.time, "sleep", sleep)
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
            assert not _taken_if_pending(signal.SIGTERM), "stop signals blocked while shards ran"

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
        # Fault the runner once the finished shard has been proven empty, which stops the rest;
        # but not before the running shard has set the SIGTERM handler that writes its marker.
        if (
            not faulted
            and started_shards.started
            and started_shards.started[0].pid in proven_empty
            and b"shard started" in (logs / "shard-1.log").read_bytes()
        ):
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
        if (
            reused
            or len(started_shards.started) < 2
            or started_shards.started[0].poll() is None
            # The running shard's SIGTERM handler, which writes its marker, must be set first.
            or b"shard started" not in (logs / "shard-1.log").read_bytes()
        ):
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


def test_a_group_is_looked_at_again_just_before_it_is_signalled(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """A finished shard's group can empty, and its number be reused, after the last look and
    before a signal step.  The step looks again first, so a group that emptied in between is
    only probed, never sent the signal."""

    process = subprocess.Popen([sys.executable, "-c", "pass"], start_new_session=True)
    process.wait()  # The leader is reaped and started nothing: its group is empty.
    groups = runner._Groups()  # No look yet since the group emptied.
    sent: list[int] = []
    real_killpg = os.killpg

    def killpg(pgid: int, signum: int) -> None:
        sent.append(signum)
        real_killpg(pgid, signum)

    monkeypatch.setattr(runner.os, "killpg", killpg)
    groups.signal([process], signal.SIGTERM)
    groups.signal([process], signal.SIGKILL)

    assert sent == [0]  # One probe proved it empty; neither signal was sent.


def test_a_group_that_vanishes_as_it_is_signalled_is_never_signalled_again(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    """A group can empty between the fresh look and the signal.  Its number may then be reused,
    so the failed signal proves it empty, and the error must not end the stop steps early."""

    process = subprocess.Popen([sys.executable, "-c", "pass"], start_new_session=True)
    process.wait()
    groups = runner._Groups()
    sent: list[int] = []

    def killpg(_pgid: int, signum: int) -> None:
        sent.append(signum)
        if signum != 0:  # The look finds a member, which leaves before the signal lands.
            raise ProcessLookupError

    monkeypatch.setattr(runner.os, "killpg", killpg)
    groups.signal([process], signal.SIGTERM)
    groups.signal([process], signal.SIGKILL)

    assert sent == [0, signal.SIGTERM]
    assert groups.ended([process])


@pytest.mark.parametrize("denied", ("look", "signal"))
def test_a_group_this_user_may_not_signal_is_never_taken_for_empty(
    monkeypatch: pytest.MonkeyPatch, denied: str
) -> None:
    """EPERM means a member exists that this user may not signal, so the group is not empty: it
    must not end the wait, nor be skipped by a later step, nor end the stop steps early."""

    process = subprocess.Popen([sys.executable, "-c", "pass"], start_new_session=True)
    process.wait()
    groups = runner._Groups()
    sent: list[int] = []

    def killpg(_pgid: int, signum: int) -> None:
        sent.append(signum)
        if (signum == 0) == (denied == "look"):
            raise PermissionError

    monkeypatch.setattr(runner.os, "killpg", killpg)
    assert not groups.ended([process])
    groups.signal([process], signal.SIGTERM)
    groups.signal([process], signal.SIGKILL)

    assert sent == [0, 0, signal.SIGTERM, 0, signal.SIGKILL]
    assert not groups.ended([process])


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


class _CallerExit(BaseException):
    """What a caller's handler raises that an `except Exception` would miss, as sys.exit does."""


_LATER = "Then, before that reached the caller, this was raised too:\n"


def _attached(raised: BaseException, stderr: str) -> list[str]:
    """What the runner attached to raised, each by the last line of its traceback, in order.

    From Python 3.11 each later exception is attached to the first as a note.  Python 3.10 has no
    notes, so the runner prints each to stderr instead.
    """

    if sys.version_info >= (3, 11):
        assert _LATER not in stderr
        notes: list[str] = getattr(raised, "__notes__", [])
    else:
        assert not hasattr(raised, "__notes__")
        notes = [_LATER + part for part in stderr.split(_LATER)[1:]]
    assert all(note.startswith(_LATER + "Traceback (most recent call last):\n") for note in notes)
    return [note.rstrip("\n").splitlines()[-1] for note in notes]


def _line(kind: type[BaseException], message: str) -> str:
    """The last line of the traceback of kind(message)."""

    return f"{kind.__module__}.{kind.__qualname__}: {message}"


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


@pytest.mark.parametrize("while_", ("installing", "restoring"))
def test_a_handler_raising_as_the_stop_signals_are_blocked_leaves_the_caller_as_it_was(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, while_: str
) -> None:
    """CPython changes the mask first and then runs pending handlers, so a caller's handler (the
    default SIGINT one when installing; any other signal's when restoring) can raise out of the
    very call that blocked the stop signals.  The caller must not be left with them blocked, or
    with the runner's recorder installed in place of its own handlers."""

    real_sigmask = signal.pthread_sigmask
    stop_signals = set(runner._STOP_SIGNALS)
    own = {signum: signal.getsignal(signum) for signum in stop_signals}
    caller_mask = real_sigmask(signal.SIG_BLOCK, ())
    assert not stop_signals & caller_mask
    blocks: list[int] = []

    def sigmask(how: int, mask: Iterable[int]) -> set[int | signal.Signals]:
        result: set[int | signal.Signals] = set(real_sigmask(how, mask))
        if how == signal.SIG_BLOCK and stop_signals <= set(mask):
            blocks.append(how)
            if len(blocks) == (1 if while_ == "installing" else 2):
                raise _CallerStop
        return result

    monkeypatch.setattr(runner.signal, "pthread_sigmask", sigmask)
    started = tmp_path / "started"
    logs = tmp_path / "logs"
    logs.mkdir()
    command = (sys.executable, "-c", "import pathlib, sys; pathlib.Path(sys.argv[1]).touch()")
    try:
        with pytest.raises(_CallerStop):
            runner.run_commands(((*command, str(started)),), logs)
        monkeypatch.undo()
        left_mask = real_sigmask(signal.SIG_BLOCK, ())
        left_handlers = {signum: signal.getsignal(signum) for signum in stop_signals}
    finally:  # Whatever the result, the rest of the suite gets this process back as it was.
        for signum, handler in own.items():
            signal.signal(signum, handler)
        real_sigmask(signal.SIG_SETMASK, caller_mask)

    # Restoring, the call that raised is made again: it cannot tell whether that call blocked them.
    assert len(blocks) == (1 if while_ == "installing" else 3)
    assert started.exists() == (while_ == "restoring")
    assert left_mask == caller_mask
    assert left_handlers == own


@pytest.mark.parametrize("changes", (False, True), ids=("before-changing", "after-changing"))
@pytest.mark.parametrize("failing", range(len(runner._STOP_SIGNALS)))
def test_a_handler_raising_while_the_recorders_are_installed_puts_back_those_already_installed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, failing: int, changes: bool
) -> None:
    """A caller's handler for another signal may raise out of any of the calls that install the
    recorders, and such a call may already have installed its recorder.  The ones already
    installed go back, that one included, all while the stop signals are held back, so the caller
    is never left with a Ctrl-C that is only recorded, and no shard starts."""

    real_signal = signal.signal
    real_sigmask = signal.pthread_sigmask
    stop_signals = runner._STOP_SIGNALS
    own = {signum: signal.getsignal(signum) for signum in stop_signals}
    caller_mask = real_sigmask(signal.SIG_BLOCK, ())
    assert not set(stop_signals) & caller_mask
    calls: list[int] = []
    masks: list[set[int | signal.Signals]] = []

    def set_handler(
        signum: int, handler: Callable[[int, FrameType | None], object] | int | None
    ) -> Callable[[int, FrameType | None], object] | int | None:
        calls.append(signum)
        masks.append(real_sigmask(signal.SIG_BLOCK, ()))
        if len(calls) == failing + 1:
            if changes:
                real_signal(signum, handler)
            raise _CallerStop
        return real_signal(signum, handler)

    monkeypatch.setattr(runner.signal, "signal", set_handler)
    started = tmp_path / "started"
    logs = tmp_path / "logs"
    logs.mkdir()
    command = (sys.executable, "-c", "import pathlib, sys; pathlib.Path(sys.argv[1]).touch()")
    try:
        with pytest.raises(_CallerStop):
            runner.run_commands(((*command, str(started)),), logs)
        monkeypatch.undo()
        left_mask = real_sigmask(signal.SIG_BLOCK, ())
        left_handlers = {signum: signal.getsignal(signum) for signum in stop_signals}
    finally:  # Whatever the result, the rest of the suite gets this process back as it was.
        monkeypatch.undo()
        for signum, handler in own.items():
            real_signal(signum, handler)
        real_sigmask(signal.SIG_SETMASK, caller_mask)

    # The installs up to the failing one, then each of those put back in order, the failing one
    # too: whether or not its call changed it, the caller's own handler is what it gets back.
    assert calls == [*stop_signals[: failing + 1], *stop_signals[: failing + 1]]
    assert all(mask == caller_mask | set(stop_signals) for mask in masks)
    assert left_handlers == own
    assert left_mask == caller_mask
    assert not started.exists()


def test_a_handler_raising_while_the_handlers_are_put_back_still_unblocks_the_stop_signals(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The stop signals are blocked while the caller's handlers go back, and a caller's handler for
    another signal may raise between any two of those calls.  Whatever handlers that leaves, the
    stop signals must not stay blocked: a caller that could no longer be interrupted is worse."""

    real_signal = signal.signal
    real_sigmask = signal.pthread_sigmask
    stop_signals = runner._STOP_SIGNALS
    own = {signum: signal.getsignal(signum) for signum in stop_signals}
    caller_mask = real_sigmask(signal.SIG_BLOCK, ())
    assert not set(stop_signals) & caller_mask
    calls: list[int] = []
    masks: list[set[int | signal.Signals]] = []

    def set_handler(
        signum: int, handler: Callable[[int, FrameType | None], object] | int | None
    ) -> Callable[[int, FrameType | None], object] | int | None:
        calls.append(signum)
        if len(calls) == len(stop_signals) + 1:  # The first handler put back.
            masks.append(real_sigmask(signal.SIG_BLOCK, ()))
            raise _CallerStop
        return real_signal(signum, handler)

    monkeypatch.setattr(runner.signal, "signal", set_handler)
    logs = tmp_path / "logs"
    logs.mkdir()
    try:
        with pytest.raises(_CallerStop):
            runner.run_commands(((sys.executable, "-c", "pass"),), logs)
        monkeypatch.undo()
        left_mask = real_sigmask(signal.SIG_BLOCK, ())
    finally:  # Whatever the result, the rest of the suite gets this process back as it was.
        monkeypatch.undo()
        for signum, handler in own.items():
            real_signal(signum, handler)
        real_sigmask(signal.SIG_SETMASK, caller_mask)

    # Every install, every handler put back, then the first one once more (the rest still went
    # back: see the test after this one).
    assert calls == [*stop_signals, *stop_signals, stop_signals[0]]
    assert masks == [caller_mask | set(stop_signals)]  # It did raise with them blocked.
    assert left_mask == caller_mask


@pytest.mark.parametrize("stop", (_CallerStop, _CallerExit), ids=("exception", "base-exception"))
@pytest.mark.parametrize("changes", (False, True), ids=("before-changing", "after-changing"))
@pytest.mark.parametrize("while_", ("installing", "restoring"))
@pytest.mark.parametrize("failing", range(len(runner._STOP_SIGNALS)))
def test_a_handler_raising_as_any_one_handler_is_put_back_still_puts_back_every_one(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    failing: int,
    while_: str,
    changes: bool,
    stop: type[BaseException],
) -> None:
    """Putting the caller's handlers back, after an install raised or after the run, must not stop
    at the first call that raises: every handler after it would stay the runner's recorder, and a
    caller whose SIGTERM is only recorded no longer stops for it.  The one whose call raised is
    tried again, whether or not that call had changed it, so every handler comes back.  The same
    holds when what the caller's handler raises is not an Exception.  What raised reaches the
    caller, but never in front of an install that raised first."""

    real_signal = signal.signal
    real_sigmask = signal.pthread_sigmask
    stop_signals = runner._STOP_SIGNALS
    own = {signum: signal.getsignal(signum) for signum in stop_signals}
    caller_mask = real_sigmask(signal.SIG_BLOCK, ())
    assert not set(stop_signals) & caller_mask
    # Installing: the last install raises after installing its recorder, so all of them go back.
    install_raises_at = len(stop_signals) if while_ == "installing" else None
    put_back_raises_at = len(stop_signals) + failing + 1
    calls: list[int] = []
    masks: list[set[int | signal.Signals]] = []

    def set_handler(
        signum: int, handler: Callable[[int, FrameType | None], object] | int | None
    ) -> Callable[[int, FrameType | None], object] | int | None:
        calls.append(signum)
        masks.append(real_sigmask(signal.SIG_BLOCK, ()))
        if len(calls) == install_raises_at:
            real_signal(signum, handler)
            raise stop("install")
        if len(calls) == put_back_raises_at:
            if changes:
                real_signal(signum, handler)
            raise stop("put back")
        return real_signal(signum, handler)

    monkeypatch.setattr(runner.signal, "signal", set_handler)
    started = tmp_path / "started"
    logs = tmp_path / "logs"
    logs.mkdir()
    command = (sys.executable, "-c", "import pathlib, sys; pathlib.Path(sys.argv[1]).touch()")
    try:
        with pytest.raises(stop) as raised:
            runner.run_commands(((*command, str(started)),), logs)
        monkeypatch.undo()
        left_mask = real_sigmask(signal.SIG_BLOCK, ())
        left_handlers = {signum: signal.getsignal(signum) for signum in stop_signals}
    finally:  # Whatever the result, the rest of the suite gets this process back as it was.
        monkeypatch.undo()
        for signum, handler in own.items():
            real_signal(signum, handler)
        real_sigmask(signal.SIG_SETMASK, caller_mask)

    # Every install, every handler put back, then the one whose call raised, once more.
    assert calls == [*stop_signals, *stop_signals, stop_signals[failing]]
    assert all(mask == caller_mask | set(stop_signals) for mask in masks)
    assert left_handlers == own
    assert left_mask == caller_mask
    assert type(raised.value) is stop
    captured = capsys.readouterr()
    later = _attached(raised.value, captured.err)
    if while_ == "installing":
        assert str(raised.value) == "install"
        assert later == [_line(stop, "put back")]  # Not swallowed: it still reaches the caller.
    else:
        assert str(raised.value) == "put back"
        assert later == []
        assert "=== mapped lifecycle shard" not in captured.out  # Raised before any report.
    assert started.exists() == (while_ == "restoring")


@pytest.mark.parametrize("stop", (_CallerStop, _CallerExit), ids=("exception", "base-exception"))
def test_a_handler_raising_as_the_stop_signals_are_unblocked_after_installing_puts_every_one_back(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, stop: type[BaseException]
) -> None:
    """Unblocking the stop signals once every recorder is installed also runs pending handlers, and
    a caller's handler for another signal may raise out of it.  All four recorders are installed
    by then, so all four go back, with the stop signals held back again, and no shard starts."""

    real_signal = signal.signal
    real_sigmask = signal.pthread_sigmask
    stop_signals = runner._STOP_SIGNALS
    own = {signum: signal.getsignal(signum) for signum in stop_signals}
    caller_mask = real_sigmask(signal.SIG_BLOCK, ())
    assert not set(stop_signals) & caller_mask
    calls: list[int] = []
    masks: list[set[int | signal.Signals]] = []
    unblocks: list[int] = []

    def set_handler(
        signum: int, handler: Callable[[int, FrameType | None], object] | int | None
    ) -> Callable[[int, FrameType | None], object] | int | None:
        calls.append(signum)
        masks.append(real_sigmask(signal.SIG_BLOCK, ()))
        return real_signal(signum, handler)

    def sigmask(how: int, mask: Iterable[int]) -> set[int | signal.Signals]:
        result: set[int | signal.Signals] = set(real_sigmask(how, mask))
        if how == signal.SIG_SETMASK:
            unblocks.append(len(calls))
            if len(unblocks) == 1:
                raise stop("unblock")
        return result

    monkeypatch.setattr(runner.signal, "signal", set_handler)
    monkeypatch.setattr(runner.signal, "pthread_sigmask", sigmask)
    started = tmp_path / "started"
    logs = tmp_path / "logs"
    logs.mkdir()
    command = (sys.executable, "-c", "import pathlib, sys; pathlib.Path(sys.argv[1]).touch()")
    try:
        with pytest.raises(stop) as raised:
            runner.run_commands(((*command, str(started)),), logs)
        monkeypatch.undo()
        left_mask = real_sigmask(signal.SIG_BLOCK, ())
        left_handlers = {signum: signal.getsignal(signum) for signum in stop_signals}
    finally:  # Whatever the result, the rest of the suite gets this process back as it was.
        monkeypatch.undo()
        for signum, handler in own.items():
            real_signal(signum, handler)
        real_sigmask(signal.SIG_SETMASK, caller_mask)

    # It raised once all four were installed; the second unblock came after all four went back.
    assert unblocks == [len(stop_signals), 2 * len(stop_signals)]
    assert calls == [*stop_signals, *stop_signals]
    assert all(mask == caller_mask | set(stop_signals) for mask in masks)
    assert left_handlers == own
    assert left_mask == caller_mask
    assert str(raised.value) == "unblock"
    assert not started.exists()


def test_a_handler_that_can_never_be_put_back_is_tried_a_bounded_number_of_times(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """A call that raises every time cannot put its handler back, however often it is tried, and
    trying it forever would never return to the caller.  It is tried _PUT_BACK_ATTEMPTS times in
    all; every other handler and the mask still come back, and the first exception is raised, with
    the later two attached."""

    real_signal = signal.signal
    real_sigmask = signal.pthread_sigmask
    stop_signals = runner._STOP_SIGNALS
    stuck = signal.SIGTERM
    own = {signum: signal.getsignal(signum) for signum in stop_signals}
    caller_mask = real_sigmask(signal.SIG_BLOCK, ())
    assert not set(stop_signals) & caller_mask
    calls: list[int] = []

    def set_handler(
        signum: int, handler: Callable[[int, FrameType | None], object] | int | None
    ) -> Callable[[int, FrameType | None], object] | int | None:
        calls.append(signum)
        if len(calls) > len(stop_signals) and signum == stuck:
            raise _CallerStop(f"call {len(calls)}")
        return real_signal(signum, handler)

    monkeypatch.setattr(runner.signal, "signal", set_handler)
    logs = tmp_path / "logs"
    logs.mkdir()
    try:
        with pytest.raises(_CallerStop) as raised:
            runner.run_commands(((sys.executable, "-c", "pass"),), logs)
        monkeypatch.undo()
        left_mask = real_sigmask(signal.SIG_BLOCK, ())
        left_handlers = {signum: signal.getsignal(signum) for signum in stop_signals}
    finally:  # Whatever the result, the rest of the suite gets this process back as it was.
        monkeypatch.undo()
        for signum, handler in own.items():
            real_signal(signum, handler)
        real_sigmask(signal.SIG_SETMASK, caller_mask)

    assert runner._PUT_BACK_ATTEMPTS == 3
    assert calls == [*stop_signals, *stop_signals, stuck, stuck]
    assert str(raised.value) == f"call {len(stop_signals) + 1 + stop_signals.index(stuck)}"
    assert _attached(raised.value, capsys.readouterr().err) == [
        _line(_CallerStop, f"call {2 * len(stop_signals) + 1}"),
        _line(_CallerStop, f"call {2 * len(stop_signals) + 2}"),
    ]
    assert left_mask == caller_mask
    assert {signum: left_handlers[signum] for signum in stop_signals if signum != stuck} == {
        signum: own[signum] for signum in stop_signals if signum != stuck
    }
    # The stuck one could not be put back: it is still the runner's recorder.
    assert isinstance(getattr(left_handlers[stuck], "__self__", None), runner._Signals)


@pytest.mark.parametrize(
    "cleanup", ("put-back", "mask-before-changing", "mask-after-changing", "both")
)
@pytest.mark.parametrize("first", ("install", "unblock", "work", "work-then-stopping", "signal"))
def test_the_first_failure_reaches_the_caller_with_what_putting_things_back_raised_attached(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    first: str,
    cleanup: str,
) -> None:
    """Python raises the last of several failures, keeping the first as its __context__ at best,
    though the first is what went wrong: an install, the unblocking after the installs, the run
    itself, or a stop signal.  The caller gets that, with each later exception attached, in order,
    and still gets back every handler and its mask, which is put back again if a call raised
    before it changed the mask, as a pending handler can, even with what an `except Exception`
    would miss, as sys.exit raises."""

    real_signal = signal.signal
    real_sigmask = signal.pthread_sigmask
    stop_signals = runner._STOP_SIGNALS
    own = {signum: signal.getsignal(signum) for signum in stop_signals}
    caller_mask = real_sigmask(signal.SIG_BLOCK, ())
    assert not set(stop_signals) & caller_mask
    calls: list[int] = []
    masks: list[set[int | signal.Signals]] = []
    mask_back: list[int] = []  # The calls made so far at each attempt to put the mask back.

    def set_handler(
        signum: int, handler: Callable[[int, FrameType | None], object] | int | None
    ) -> Callable[[int, FrameType | None], object] | int | None:
        calls.append(signum)
        masks.append(real_sigmask(signal.SIG_BLOCK, ()))
        if first == "install" and len(calls) == len(stop_signals):
            real_signal(signum, handler)
            raise _CallerStop("install")
        if cleanup in ("put-back", "both") and len(calls) == len(stop_signals) + 1:
            raise _CallerStop("put back")  # Before changing it: only the retry puts it back.
        return real_signal(signum, handler)

    def sigmask(how: int, mask: Iterable[int]) -> set[int | signal.Signals]:
        if how != signal.SIG_SETMASK:
            return set(real_sigmask(how, mask))
        if len(calls) == len(stop_signals) and first != "install":  # Unblocking after installing.
            result = set(real_sigmask(how, mask))
            if first == "unblock":
                raise _CallerStop("unblock")
            return result
        mask_back.append(len(calls))
        if len(mask_back) == 1 and cleanup in ("mask-before-changing", "both"):
            raise _CallerExit("mask back")
        result = set(real_sigmask(how, mask))
        if len(mask_back) == 1 and cleanup == "mask-after-changing":
            raise _CallerExit("mask back")
        return result

    def fail_to_start(_parent: int, _signum: int) -> Callable[[], None]:
        raise _CallerStop("work")

    def fail_to_stop(*_args: object) -> None:
        raise _CallerStop("stopping")

    real_sleep = time.sleep

    def sleep(seconds: float) -> None:
        # Held back, the shard's one SIGTERM would wait out its minute and then reach pytest.
        assert not _taken_if_pending(signal.SIGTERM), "stop signals blocked while shards ran"
        real_sleep(seconds)

    monkeypatch.setattr(runner.time, "sleep", sleep)
    monkeypatch.setattr(runner.signal, "signal", set_handler)
    monkeypatch.setattr(runner.signal, "pthread_sigmask", sigmask)
    if first.startswith("work"):
        monkeypatch.setattr(runner, "_signal_on_parent_death", fail_to_start)
    if first == "work-then-stopping":
        monkeypatch.setattr(runner, "_stop_shards", fail_to_stop)
    logs = tmp_path / "logs"
    logs.mkdir()
    # The shard stops the runner while it is still running, so the signal is what came first.
    stops_its_runner = (
        "import os, signal, time; os.kill(os.getppid(), signal.SIGTERM); time.sleep(60)"
    )
    started = time.monotonic()
    try:
        with pytest.raises((_CallerStop, SystemExit)) as raised:
            runner.run_commands(((sys.executable, "-c", stops_its_runner),), logs)
        monkeypatch.undo()
        left_mask = real_sigmask(signal.SIG_BLOCK, ())
        left_handlers = {signum: signal.getsignal(signum) for signum in stop_signals}
    finally:  # Whatever the result, the rest of the suite gets this process back as it was.
        monkeypatch.undo()
        for signum, handler in own.items():
            real_signal(signum, handler)
        real_sigmask(signal.SIG_SETMASK, caller_mask)
    captured = capsys.readouterr()

    if first == "signal":
        assert type(raised.value) is SystemExit
        assert raised.value.code == 128 + signal.SIGTERM
        assert "=== mapped lifecycle shard 1/1 INTERRUPTED rc=-15 ===" in captured.out
        assert time.monotonic() - started < 30  # SIGTERM gets the shard no grace.
    else:
        assert type(raised.value) is _CallerStop
        assert str(raised.value) == ("work" if first.startswith("work") else first)
    later = {
        "put-back": [_line(_CallerStop, "put back")],
        "mask-before-changing": [_line(_CallerExit, "mask back")],
        "mask-after-changing": [_line(_CallerExit, "mask back")],
        "both": [_line(_CallerStop, "put back"), _line(_CallerExit, "mask back")],
    }[cleanup]
    if first == "work-then-stopping":
        later = [_line(_CallerStop, "stopping"), *later]
    assert _attached(raised.value, captured.err) == later
    assert left_handlers == own
    assert left_mask == caller_mask
    # Every handler went back with the stop signals held back, and the mask went back once more
    # after a call that raised, whether or not that call had changed it.
    assert all(mask == caller_mask | set(stop_signals) for mask in masks)
    retried = cleanup in ("put-back", "both")
    put_back = 2 * len(stop_signals) + int(retried)
    assert calls == [*stop_signals, *stop_signals, *stop_signals[:int(retried)]]
    assert mask_back == ([put_back] if cleanup == "put-back" else [put_back, put_back])


@pytest.mark.parametrize("stop", (_CallerStop, _CallerExit), ids=("exception", "base-exception"))
@pytest.mark.parametrize("changes", (False, True), ids=("before-changing", "after-changing"))
def test_holding_the_stop_signals_back_again_is_retried_before_any_handler_goes_back(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
    changes: bool,
    stop: type[BaseException],
) -> None:
    """A pending handler may raise as the call that holds the stop signals back is entered, before
    it changes the mask.  That call is made again, so every handler still goes back with the stop
    signals held back, and the mask comes back; what raised still reaches the caller, before the
    shards' report, which could raise in front of it."""

    real_signal = signal.signal
    real_sigmask = signal.pthread_sigmask
    stop_signals = runner._STOP_SIGNALS
    own = {signum: signal.getsignal(signum) for signum in stop_signals}
    caller_mask = real_sigmask(signal.SIG_BLOCK, ())
    assert not set(stop_signals) & caller_mask
    calls: list[int] = []
    masks: list[set[int | signal.Signals]] = []
    blocks: list[int] = []

    def set_handler(
        signum: int, handler: Callable[[int, FrameType | None], object] | int | None
    ) -> Callable[[int, FrameType | None], object] | int | None:
        calls.append(signum)
        masks.append(real_sigmask(signal.SIG_BLOCK, ()))
        return real_signal(signum, handler)

    def sigmask(how: int, mask: Iterable[int]) -> set[int | signal.Signals]:
        if how == signal.SIG_BLOCK and set(stop_signals) <= set(mask):
            blocks.append(len(calls))
            if len(blocks) == 2:  # The first attempt to hold them back again, after the run.
                if changes:
                    real_sigmask(how, mask)
                raise stop("block")
        return set(real_sigmask(how, mask))

    monkeypatch.setattr(runner.signal, "signal", set_handler)
    monkeypatch.setattr(runner.signal, "pthread_sigmask", sigmask)
    logs = tmp_path / "logs"
    logs.mkdir()
    try:
        with pytest.raises(stop) as raised:
            runner.run_commands(((sys.executable, "-c", "pass"),), logs)
        monkeypatch.undo()
        left_mask = real_sigmask(signal.SIG_BLOCK, ())
        left_handlers = {signum: signal.getsignal(signum) for signum in stop_signals}
    finally:  # Whatever the result, the rest of the suite gets this process back as it was.
        monkeypatch.undo()
        for signum, handler in own.items():
            real_signal(signum, handler)
        real_sigmask(signal.SIG_SETMASK, caller_mask)

    assert blocks == [0, len(stop_signals), len(stop_signals)]
    assert calls == [*stop_signals, *stop_signals]
    assert all(mask == caller_mask | set(stop_signals) for mask in masks)
    assert left_handlers == own
    assert left_mask == caller_mask
    assert type(raised.value) is stop
    assert str(raised.value) == "block"
    captured = capsys.readouterr()
    assert _attached(raised.value, captured.err) == []
    assert "=== mapped lifecycle shard" not in captured.out


@pytest.mark.parametrize("python", ("3.11", "3.10"))
def test_each_later_failure_is_attached_to_the_first_once_with_its_traceback(
    monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str], python: str
) -> None:
    """The first failure is returned to be raised; each later one, however often it was raised,
    is attached once, in order, as a note carrying its traceback, or on Python 3.10, which has no
    notes, printed to stderr at once."""

    if python == "3.10":
        monkeypatch.setattr(sys, "version_info", (3, 10, 0))
    elif sys.version_info < (3, 11):
        pytest.skip("exception notes need Python 3.11")
    failures: list[BaseException] = []
    for message in ("first", "second", "third"):
        try:
            raise _CallerStop(message)
        except _CallerStop as exc:
            failures.append(exc)
    first, second, third = failures

    result = runner._first_of([first, second, third, second])
    later = _attached(result, capsys.readouterr().err)
    notes: list[str] = getattr(result, "__notes__", [])
    monkeypatch.undo()

    assert result is first
    assert later == [_line(_CallerStop, "second"), _line(_CallerStop, "third")]
    if python == "3.11":
        assert all('raise _CallerStop(message)' in note for note in notes)
    assert runner._first_of([third]) is third


def test_a_caller_that_already_blocks_signals_keeps_them_blocked_during_and_after_the_run(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The runner holds the stop signals back only while handlers change.  Whatever the caller
    had already blocked, a stop signal among them included, stays blocked throughout."""

    real_sigmask = signal.pthread_sigmask
    stop_signals = runner._STOP_SIGNALS
    own = {signum: signal.getsignal(signum) for signum in stop_signals}
    original_mask = real_sigmask(signal.SIG_BLOCK, ())
    caller_mask = original_mask | {signal.SIGUSR1, signal.SIGINT}
    real_sleep = time.sleep
    during: list[set[int | signal.Signals]] = []

    def sleep(seconds: float) -> None:
        during.append(real_sigmask(signal.SIG_BLOCK, ()))
        real_sleep(seconds)

    monkeypatch.setattr(runner.time, "sleep", sleep)
    logs = tmp_path / "logs"
    logs.mkdir()
    real_sigmask(signal.SIG_SETMASK, caller_mask)
    try:
        rc = runner.run_commands(((sys.executable, "-c", "import time; time.sleep(0.5)"),), logs)
        after = real_sigmask(signal.SIG_BLOCK, ())
    finally:
        real_sigmask(signal.SIG_SETMASK, original_mask)

    assert rc == 0
    assert during
    assert all(mask == caller_mask for mask in during)
    assert after == caller_mask
    assert {signum: signal.getsignal(signum) for signum in stop_signals} == own


def test_a_stop_signal_after_the_last_shard_ended_is_still_honoured(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    """A signal that arrives after the poll loop's last look and before the handlers are
    restored reaches only the record.  The run still ends as that signal asks."""

    real_mask = signal.pthread_sigmask
    blocks: list[int] = []

    def pthread_sigmask(how: int, mask: Iterable[int]) -> set[int | signal.Signals]:
        mask = set(mask)
        # Only the calls that block the stop signals count; reading the mask blocks nothing.
        if how == signal.SIG_BLOCK and set(runner._STOP_SIGNALS) <= mask:
            blocks.append(how)
            if len(blocks) == 2:  # The restoring, not the installing.
                os.kill(os.getpid(), signal.SIGHUP)
                # Not yet blocked, so recorded already; held back, it would end pytest at restore.
                assert not _taken_if_pending(signal.SIGHUP), "stop signals blocked after the run"
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
            assert not _taken_if_pending(signal.SIGTERM), "stop signals blocked while shards ran"
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
    shard = int((run.state / "shard.pid").read_text())
    deadline = time.monotonic() + 10
    while not run.exited("shard"):
        assert time.monotonic() < deadline, "the shard outlived its runner"
        # The shard never blocks SIGINT itself, so one held back came blocked from the runner.
        assert signal.SIGINT not in _held_back(shard), "the shard holds back its SIGINT"
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
