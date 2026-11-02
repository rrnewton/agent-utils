"""The differential's tolerant runner: CPU-spent budgets and group cleanup."""
from __future__ import annotations

import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "cross"))

from herdr_agent_differential import run_tolerant  # noqa: E402

PY = sys.executable


def test_fast_child_passes_through() -> None:
    done = run_tolerant([PY, "-c", "print('hi')"], timeout_s=5)
    assert (done.returncode, done.stdout.strip()) == (0, "hi")


def test_idle_child_is_killed_near_its_budget() -> None:
    started = time.monotonic()
    try:
        run_tolerant([PY, "-c", "import time; time.sleep(30)"], timeout_s=1)
    except subprocess.TimeoutExpired:
        assert time.monotonic() - started < 10
        return
    raise AssertionError("idle child was not timed out")


def test_busy_grandchild_counts_as_progress() -> None:
    # The direct child waits on its own busy child: tree CPU is progress,
    # so the idle budget must not kill the call while the grandchild works.
    parent = (
        "import subprocess, sys\n"
        "busy = 'import time\\nstart = time.monotonic()\\n"
        "while time.monotonic() - start < 1.4:\\n    pass\\n'\n"
        "subprocess.run([sys.executable, '-c', busy], check=True)\n"
        "print('done')"
    )
    done = run_tolerant([PY, "-c", parent], timeout_s=1)
    assert (done.returncode, done.stdout.strip()) == (0, "done")


def test_busy_child_is_killed_by_cpu_not_instantly() -> None:
    started = time.monotonic()
    try:
        run_tolerant([PY, "-c", "x = 0\nwhile True:\n    x += 1"], timeout_s=1)
    except subprocess.TimeoutExpired:
        elapsed = time.monotonic() - started
        assert 1.0 <= elapsed < 10, elapsed
        return
    raise AssertionError("busy child was not timed out")
