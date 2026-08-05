"""Adversarial tests for tick-hub's subprocess boundary."""

from __future__ import annotations

import os
import shlex
import signal
import time
from pathlib import Path

import pytest

from tick_hub.probes import SubprocessGateRunner


def test_gate_runner_replaces_invalid_utf8() -> None:
    result = SubprocessGateRunner(timeout=2).run("printf '\\377'")
    assert result.ok is True
    assert result.returncode == 0
    assert result.stdout == "\ufffd"


@pytest.mark.skipif(os.name != "posix", reason="process-group cleanup is POSIX-specific")
def test_gate_timeout_kills_background_descendants(tmp_path: Path) -> None:
    pid_file = tmp_path / "child.pid"
    command = f"sleep 30 & echo $! > {shlex.quote(str(pid_file))}; sleep 30"
    started = time.monotonic()
    result = SubprocessGateRunner(timeout=1).run(command)
    elapsed = time.monotonic() - started

    assert result.ok is False
    assert result.error == "timed out after 1s"
    assert elapsed < 3
    child_pid = int(pid_file.read_text(encoding="utf-8"))

    def still_running() -> bool:
        try:
            stat = Path(f"/proc/{child_pid}/stat").read_text(encoding="utf-8")
        except OSError:
            return False
        return stat[stat.rfind(")") + 2 :].split()[0] != "Z"

    deadline = time.monotonic() + 1
    while still_running() and time.monotonic() < deadline:
        time.sleep(0.01)
    if still_running():
        os.kill(child_pid, signal.SIGKILL)
        raise AssertionError("timed-out gate left a live background descendant")
