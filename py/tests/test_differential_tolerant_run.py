"""The differential's hang-guard runner: progress-aware waits and group cleanup.

The guard reports only hangs (no CPU or output progress, excessive CPU,
or excessive wall time under a generous minutes-scale budget); it never
judges how fast a child finished (cross-differentials-robust).
"""
from __future__ import annotations

import os
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "cross"))

from herdr_agent_differential import (  # noqa: E402
    _tree_cpu_seconds,
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


def test_slow_but_progressing_child_is_not_a_hang() -> None:
    # A child slower than any micro-budget but steadily producing output
    # finishes normally: the guard waits on progress, not on speed
    # (cross-differentials-robust).
    child = (
        "import time\n"
        "for index in range(3):\n"
        "    print(f'step {index}', flush=True)\n"
        "    time.sleep(0.6)\n"
    )
    done = run_tolerant([PY, "-c", child], timeout_s=1)
    assert done.returncode == 0
    assert done.stdout.splitlines() == [f"step {index}" for index in range(3)]


def test_stdin_child_reads_its_payload_from_a_temp_file_fd() -> None:
    # `run_tolerant` takes a stdin fd, not `input=`: the MCP differential
    # feeds its payload through a rewound temp file, so pin that the child
    # reads exactly those bytes to EOF from its stdin (#1067).
    child = "import sys\nsys.stdout.write(sys.stdin.read().upper())"
    with tempfile.TemporaryFile(mode="w+", encoding="utf-8") as stdin_file:
        stdin_file.write("hello\nstdin\n")
        stdin_file.seek(0)
        done = run_tolerant([PY, "-c", child], timeout_s=5, stdin=stdin_file.fileno())
    assert (done.returncode, done.stdout) == (0, "HELLO\nSTDIN\n")


def test_sigterm_removes_the_fixture_temp_root(tmp_path: Path) -> None:
    # A CPU-timeout kill (SIGTERM) must not leak the differential's
    # fixture tree. Once the temp root appears (a filesystem condition),
    # terminate and assert nothing remains behind; no wall-clock verdict.
    cross_dir = Path(__file__).resolve().parents[2] / "cross"
    script = (
        f"import sys, time\nsys.path.insert(0, {str(cross_dir)!r})\n"
        "from herdr_agent_differential import _signal_cleaned_tempdir\n"
        "with _signal_cleaned_tempdir('agentctl-cross-test-') as root:\n"
        "    (root / 'marker').write_text('x')\n"
        "    print(root, flush=True)\n"
        "    time.sleep(600)\n"
    )
    proc = subprocess.Popen(
        [PY, "-c", script],
        stdout=subprocess.PIPE,
        text=True,
        env={**os.environ, "TMPDIR": str(tmp_path)},
    )
    assert proc.stdout is not None
    root = Path(proc.stdout.readline().strip())
    assert (root / "marker").exists()
    proc.send_signal(signal.SIGTERM)
    proc.wait(timeout=60)
    assert proc.returncode == 128 + signal.SIGTERM
    assert not root.exists()
    assert not list(tmp_path.glob("agentctl-cross-test-*"))
