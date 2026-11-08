"""The differential's tolerant runner: CPU-spent budgets and group cleanup.

Also the in-run calibration that scales positive micro-budgets by measured
spawn latency (#1067 cross-differential-load-aware): load-inflated spawns
must pass, delays beyond the hard cap must still fail.
"""
from __future__ import annotations

import subprocess
import sys
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "cross"))

from herdr_agent_differential import (  # noqa: E402
    _tree_cpu_seconds,
    calibrated_budget,
    run_tolerant,
)

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


def test_waited_for_grandchild_cpu_stays_in_the_tree_total() -> None:
    # A grandchild that already exited must still count (cutime/cstime):
    # one fake-Herdr spawn lives ~50 ms, between 100 ms polls, and reading
    # only live processes would call that progress idleness (#1067).
    parent = (
        "import subprocess, sys, time\n"
        "busy = 'import time\\nstart = time.monotonic()\\n"
        "while time.monotonic() - start < 0.5:\\n    pass\\n'\n"
        "subprocess.run([sys.executable, '-c', busy], check=True)\n"
        "time.sleep(30)"
    )
    proc = subprocess.Popen([PY, "-c", parent], stdout=subprocess.DEVNULL)
    try:
        deadline = time.monotonic() + 10
        total = 0.0
        while time.monotonic() < deadline:
            measured = _tree_cpu_seconds(proc.pid)
            if measured is not None:
                total = measured
            if total >= 0.3:
                break
            time.sleep(0.05)
        assert total >= 0.3, f"exited grandchild CPU vanished from tree total: {total}"
    finally:
        proc.kill()
        proc.wait(timeout=5)


def test_output_growth_counts_as_progress() -> None:
    # Almost no CPU, steady output for ~2 s against a 1 s budget: only the
    # output-progress leg can keep this alive (wall cap is 4 s) (#1067).
    child = (
        "import time\n"
        "for index in range(5):\n"
        "    print(f'line {index}', flush=True)\n"
        "    time.sleep(0.4)"
    )
    done = run_tolerant([PY, "-c", child], timeout_s=1)
    assert done.returncode == 0
    assert done.stdout.splitlines() == [f"line {index}" for index in range(5)]


def test_calibrated_budget_scales_with_spawn_latency_but_never_past_the_cap() -> None:
    # No measurement keeps the strict nominal budget.
    assert calibrated_budget(0.1, [], probes=6, cap_s=5.0) == 0.1
    # Fast spawns keep the nominal floor.
    assert calibrated_budget(0.2, [0.001, 0.002], probes=6, cap_s=5.0) == 0.2
    # Slow spawns scale by probes x worst x margin (2.0): 6 x 0.25 x 2 = 3.
    assert calibrated_budget(0.1, [0.125, 0.25], probes=6, cap_s=5.0) == 3.0
    # The cap is hard however slow the host looks: an injected per-probe
    # delay beyond cap/probes (here 6 s against a 5 s cap) still expires,
    # so calibration cannot talk a real slowdown into a pass (#1067).
    assert calibrated_budget(0.1, [10.0], probes=6, cap_s=5.0) == 5.0
    assert 6 * 6.0 > calibrated_budget(0.1, [10.0], probes=6, cap_s=5.0)
