"""The wrkslots tests' git stand-in (#2065): nothing a git call leaves behind outlives it.

Each case compiles the stand-in around a fake "real git" script and runs it for real.
"""

from __future__ import annotations

import json
import os
import select
import signal
import subprocess
import sys
from pathlib import Path

import pytest

from wrkslots.tests import git_stand_in

#: A generous backstop for a condition the kernel settles at once; never a timing budget.
BACKSTOP_S = 60


def _build(tmp_path: Path, fake: str) -> tuple[Path, Path]:
    compiler = git_stand_in._compiler()
    if compiler is None:
        pytest.skip("no C compiler (cc, gcc, or clang) is on PATH")
    real = tmp_path / "real-git"
    real.write_text("#!/bin/sh\n" + fake, encoding="utf-8")
    real.chmod(0o755)
    log = tmp_path / "calls.log"
    binary = tmp_path / "git"
    subprocess.run(
        [compiler, "-O2", "-Wall", "-Wextra", "-Werror",
         "-DREAL_GIT=" + json.dumps(str(real)), "-DCALL_LOG=" + json.dumps(str(log)),
         "-o", str(binary), str(git_stand_in.SOURCE)],
        check=True, capture_output=True, text=True, timeout=120,
    )
    return binary, log


def _require_namespace(log: Path) -> None:
    first = log.read_text(encoding="utf-8").split() if log.exists() else []
    if first[:1] != ["ns"]:
        pytest.skip(f"unprivileged user namespaces are refused here ({' '.join(first)})")


def test_a_detached_grandchild_is_gone_when_the_call_returns(tmp_path: Path) -> None:
    """The fake git leaves a detached sleeper holding the write end of a pipe and
    exits 7. When the stand-in returns, every writer is gone, so the pipe reads
    EOF at once; the sleeper would hold it for 300 s."""
    binary, log = _build(tmp_path, "setsid sleep 300 &\nexit 7\n")
    read_end, write_end = os.pipe()
    try:
        done = subprocess.run([str(binary)], stdout=write_end, timeout=BACKSTOP_S, check=False)
        os.close(write_end)
        write_end = -1
        _require_namespace(log)
        assert done.returncode == 7
        ready, _, _ = select.select([read_end], [], [], BACKSTOP_S)
        assert ready, "a process the git call left behind still holds its output"
        assert os.read(read_end, 1) == b""
    finally:
        os.close(read_end)
        if write_end >= 0:
            os.close(write_end)
    assert log.read_text(encoding="utf-8").split() == ["ns", "0"]


def test_a_signal_death_and_the_arguments_pass_through(tmp_path: Path) -> None:
    binary, log = _build(tmp_path, 'printf "%s|" "$@"\nkill -USR1 $$\n')
    done = subprocess.run(
        [str(binary), "log", "--format=%H x", ""], capture_output=True, text=True,
        timeout=BACKSTOP_S, check=False,
    )
    _require_namespace(log)
    assert done.stdout == "log|--format=%H x||"
    assert done.returncode == -signal.SIGUSR1


def test_sigterm_reaches_git_and_its_own_exit_status_comes_back(tmp_path: Path) -> None:
    """A SIGTERM to the call reaches the real git, whose own handler decides the outcome."""
    binary, log = _build(tmp_path, "trap 'exit 42' TERM\necho ready\nsleep 300 & wait\n")
    call = subprocess.Popen([str(binary)], stdout=subprocess.PIPE, text=True)
    try:
        assert call.stdout is not None
        assert call.stdout.readline() == "ready\n"
        _require_namespace(log)
        call.send_signal(signal.SIGTERM)
        assert call.wait(timeout=BACKSTOP_S) == 42
    finally:
        if call.poll() is None:
            call.kill()
            call.wait()


def test_the_session_stand_in_is_first_on_path() -> None:
    """The suite's own session runs this stand-in, or says why not."""
    if not git_stand_in.GitStandIn.start().on:  # nested start: reports the outer stand-in
        pytest.skip("the session's git stand-in is off; the terminal summary says why")
    active = os.environ[git_stand_in.ACTIVE_ENV]
    found = subprocess.run(
        [sys.executable, "-c", "import shutil; print(shutil.which('git'))"],
        capture_output=True, text=True, check=True, timeout=BACKSTOP_S,
    ).stdout.strip()
    assert Path(found).parent == Path(active)
