from __future__ import annotations

import importlib.util
import os
import select
import signal
import subprocess
import sys
import time
from collections.abc import Callable, Iterator
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

    assert command[:6] == [
        "/usr/bin/unshare",
        "--user",
        "--map-root-user",
        "--pid",
        "--fork",
        "--mount-proc",
    ]
    # The resolved interpreter is PID 1; pytest keeps the invoking
    # interpreter's path so a virtual environment's packages stay visible.
    assert command[6:9] == [
        str(init_python),
        str(ROOT / "scripts" / "pid_namespace_init.py"),
        "--",
    ]
    assert command[12:15] == [str(python), "-m", "pytest"]
    assert [
        "/usr/bin/env",
        f"--chdir={ROOT / 'py'}",
        f"PATH={tool_directory}:{os.environ['PATH']}",
    ] == command[9:12]
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

    def run(
        commands: tuple[list[str], ...],
        _logs: Path,
        *,
        label: str,
        cwd: Path | None = None,
    ) -> int:
        ran.append((commands, label, cwd))
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
