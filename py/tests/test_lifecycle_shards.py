from __future__ import annotations

import importlib.util
import os
import sys
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
RUNNER_PATH = ROOT / "scripts" / "run_mapped_lifecycle_shards.py"
SPEC = importlib.util.spec_from_file_location("run_mapped_lifecycle_shards", RUNNER_PATH)
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

    def collect(python: Path, _root: Path) -> tuple[str, ...]:
        collected.append(python)
        return ("wrkslots/tests/test_lifecycle.py::test_one",)

    def build(python: Path, *_args: object, init_python: Path, **_kwargs: object) -> list[str]:
        built.append((python, init_python))
        return [sys.executable, "-c", "pass"]

    monkeypatch.setattr(runner.sys, "executable", str(link))
    monkeypatch.setattr(runner, "collect_mapped_nodeids", collect)
    monkeypatch.setattr(runner, "mapped_shard_command", build)

    assert runner.main(["--shards", "1"]) == 0

    assert collected == [link]
    assert built == [(link, Path(os.path.realpath(sys.executable)))]


def test_pinned_git_version_names_the_substituted_git(tmp_path: Path) -> None:
    tools = tmp_path / "tools"
    runner.prepare_tool_directory(tools)

    reported = runner.pinned_git_version(tools)

    assert reported.startswith("Git pinned to /usr/bin/git (git version ")
