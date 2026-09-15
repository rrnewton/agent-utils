"""Real namespace controls for the lifecycle suite's init process."""

from __future__ import annotations

import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import time

import pytest


ROOT = Path(__file__).resolve().parents[2]
INIT = ROOT / "scripts" / "pid_namespace_init.py"
PYTHON = str(Path(sys.executable).resolve())
NAMESPACE_FLAGS = ["--user", "--map-root-user", "--pid", "--fork", "--mount-proc"]


@pytest.fixture(scope="module")
def namespace_argv() -> list[str]:
    unshare = shutil.which("unshare")
    if unshare is None:
        pytest.skip("unshare is unavailable; the Makefile retains the ordinary full-suite path")
    available = subprocess.run(
        [unshare, *NAMESPACE_FLAGS, "true"], capture_output=True, text=True, timeout=10
    )
    if available.returncode != 0:
        pytest.skip(f"PID namespaces unavailable: {available.stderr.strip()}")
    return [unshare, *NAMESPACE_FLAGS, PYTHON, str(INIT), "--", PYTHON, "-c"]


def run_namespace(namespace_argv: list[str], code: str) -> subprocess.CompletedProcess[str]:
    argv = [*namespace_argv, code]
    process = subprocess.Popen(argv, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    try:
        stdout, stderr = process.communicate(timeout=10)
        return subprocess.CompletedProcess(argv, process.returncode, stdout, stderr)
    finally:
        finish_owned(process, None)


def direct_children(parent: int) -> list[int]:
    # /proc/PID/task/PID/children depends on a kernel configuration option.
    result = subprocess.run(
        ["ps", "--ppid", str(parent), "-o", "pid="], capture_output=True, text=True, timeout=5
    )
    assert result.returncode in (0, 1), result.stderr
    return [int(pid) for pid in result.stdout.split()]


def namespace_init_pid(process: subprocess.Popen[str]) -> int:
    children = direct_children(process.pid)
    assert len(children) == 1, children
    pid = children[0]
    status = Path(f"/proc/{pid}/status").read_text()
    assert f"PPid:\t{process.pid}\n" in status
    assert Path(f"/proc/{pid}/exe").resolve() == Path(PYTHON)
    return pid


def wait_ready(process: subprocess.Popen[str], ready: Path) -> None:
    deadline = time.monotonic() + 5
    while not ready.exists():
        assert process.poll() is None, process.communicate()
        assert time.monotonic() < deadline, "namespace command did not become ready"
        time.sleep(0.01)


def finish_owned(process: subprocess.Popen[str], init_pid: int | None) -> None:
    if process.poll() is None:
        if init_pid is not None:
            try:
                os.kill(init_pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
        else:
            children = direct_children(process.pid)
            for pid in children:
                try:
                    status = Path(f"/proc/{pid}/status").read_text()
                    assert f"PPid:\t{process.pid}\n" in status
                    os.kill(pid, signal.SIGKILL)
                except ProcessLookupError:
                    pass
                except FileNotFoundError:
                    pass
            if not children:
                process.kill()
        process.communicate(timeout=5)


def test_refuses_outside_namespace_pid_one(tmp_path: Path) -> None:
    marker = tmp_path / "must-not-run"
    result = subprocess.run(
        [PYTHON, str(INIT), "--", PYTHON, "-c", f"open({str(marker)!r}, 'w').close()"],
        capture_output=True,
        text=True,
        timeout=5,
    )
    assert result.returncode == 2
    assert "refused: must run as namespace PID 1" in result.stderr
    assert not marker.exists()


@pytest.mark.parametrize("exit_code", [0, 7, 42])
def test_preserves_primary_status(namespace_argv: list[str], exit_code: int) -> None:
    result = run_namespace(namespace_argv, f"raise SystemExit({exit_code})")
    assert result.returncode == exit_code, result.stderr


def test_preserves_primary_signal_failure(namespace_argv: list[str]) -> None:
    result = run_namespace(namespace_argv, "import os, signal; os.kill(os.getpid(), signal.SIGTERM)")
    assert result.returncode == 128 + signal.SIGTERM, result.stderr


def test_adopted_status_does_not_replace_primary_failure(namespace_argv: list[str]) -> None:
    result = run_namespace(
        namespace_argv,
        """
import os, time
parent = os.fork()
if parent == 0:
    if os.fork() == 0:
        os._exit(12)
    os._exit(0)
os.waitpid(parent, 0)
time.sleep(0.05)
raise SystemExit(7)
""",
    )
    assert result.returncode == 7, result.stderr


def test_reaps_adopted_orphans_during_primary_execution(namespace_argv: list[str]) -> None:
    result = run_namespace(
        namespace_argv,
        """
from pathlib import Path
import os, time
ancestors = set()
ancestor = os.getpid()
while ancestor != 1:
    ancestors.add(ancestor)
    status = (Path('/proc') / str(ancestor) / 'status').read_text()
    parent_line = next(line for line in status.splitlines() if line.startswith('PPid:'))
    ancestor = int(parent_line.split()[1])
for index in range(300):
    parent = os.fork()
    if parent == 0:
        if os.fork() == 0:
            os._exit(0)
        os._exit(0)
    _, status = os.waitpid(parent, 0)
    assert os.waitstatus_to_exitcode(status) == 0, ('intermediate child failed', index, status)
deadline = time.monotonic() + 3
while True:
    adopted = []
    for entry in Path('/proc').iterdir():
        if not entry.name.isdecimal() or int(entry.name) in ancestors:
            continue
        try:
            status = (entry / 'status').read_text()
        except (FileNotFoundError, ProcessLookupError):
            continue
        if 'PPid:\\t1\\n' in status:
            adopted.append(entry.name)
    if not adopted:
        break
    assert time.monotonic() < deadline, adopted
    time.sleep(0.01)
print('300 adopted children reaped while primary remained alive')
""",
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "300 adopted children reaped while primary remained alive"


def test_live_setsid_orphan_survives_until_primary_exits(
    namespace_argv: list[str], tmp_path: Path
) -> None:
    stopped = tmp_path / "orphan-stopped"
    result = run_namespace(
        namespace_argv,
        f"""
from pathlib import Path
import os, signal, time
read_fd, write_fd = os.pipe()
parent = os.fork()
if parent == 0:
    os.close(read_fd)
    if os.fork() == 0:
        os.setsid()
        def stop(_signum, _frame):
            Path({str(stopped)!r}).write_text('terminated after primary exit')
            os._exit(0)
        signal.signal(signal.SIGTERM, stop)
        os.write(write_fd, str(os.getpid()).encode())
        os.close(write_fd)
        while True:
            time.sleep(1)
    os.close(write_fd)
    os._exit(0)
os.close(write_fd)
orphan = int(os.read(read_fd, 64))
os.close(read_fd)
os.waitpid(parent, 0)
time.sleep(0.15)
os.kill(orphan, 0)
assert not Path({str(stopped)!r}).exists(), 'init killed a live fixture'
print('live setsid orphan survived until primary exit')
""",
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout.strip() == "live setsid orphan survived until primary exit"
    assert stopped.read_text() == "terminated after primary exit"


def test_immediate_primary_cancellation_cannot_become_success(namespace_argv: list[str]) -> None:
    result = run_namespace(namespace_argv, "import os, signal; os.kill(1, signal.SIGTERM)")
    assert result.returncode == 128 + signal.SIGTERM, result.stderr
    assert "cancellation signal 15" in result.stderr


@pytest.mark.parametrize("catches_as_zero", [False, True])
def test_cancellation_reaches_quiet_primary_and_cannot_become_success(
    namespace_argv: list[str], tmp_path: Path, catches_as_zero: bool
) -> None:
    ready = tmp_path / "ready"
    code = f"""
from pathlib import Path
import os, signal, time
signal.signal(signal.SIGTERM, (lambda _s, _f: os._exit(0)) if {catches_as_zero!r} else signal.SIG_IGN)
Path({str(ready)!r}).touch()
while True:
    time.sleep(1)
"""
    process = subprocess.Popen([*namespace_argv, code], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    init_pid: int | None = None
    try:
        wait_ready(process, ready)
        init_pid = namespace_init_pid(process)
        # Repeated cancellation must not restart the grace period. A quiet
        # process must terminate while signals continue, not after they stop.
        deadline = time.monotonic() + 5
        while process.poll() is None and time.monotonic() < deadline:
            try:
                os.kill(init_pid, signal.SIGTERM)
            except ProcessLookupError:
                break
            time.sleep(0.05)
        stdout, stderr = process.communicate(timeout=1)
        expected = 128 + (signal.SIGTERM if catches_as_zero else signal.SIGKILL)
        assert process.returncode == expected, (stdout, stderr)
        assert "cancellation signal 15" in stderr
    finally:
        finish_owned(process, init_pid)


def test_later_cancellation_preserves_primary_failure_during_orphan_cleanup(
    namespace_argv: list[str], tmp_path: Path
) -> None:
    ready = tmp_path / "ready"
    code = f"""
from pathlib import Path
import os, signal, time
primary_pid = os.getpid()
parent = os.fork()
if parent == 0:
    if os.fork() == 0:
        os.setsid()
        signal.signal(signal.SIGTERM, signal.SIG_IGN)
        Path({str(ready)!r}).write_text(str(primary_pid))
        while True:
            time.sleep(1)
    os._exit(0)
os.waitpid(parent, 0)
while not Path({str(ready)!r}).exists():
    time.sleep(0.01)
raise SystemExit(7)
"""
    process = subprocess.Popen([*namespace_argv, code], stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
    init_pid: int | None = None
    try:
        wait_ready(process, ready)
        init_pid = namespace_init_pid(process)
        # The orphan's readiness does not prove the foreground status was
        # collected. Observe its namespace PID disappear from this init's
        # direct children before testing cancellation during orphan cleanup.
        primary_pid = int(ready.read_text())
        deadline = time.monotonic() + 3
        while True:
            primary_visible = False
            for child in direct_children(init_pid):
                try:
                    status = Path(f"/proc/{child}/status").read_text()
                except FileNotFoundError:
                    continue
                ns_pid = next(line for line in status.splitlines() if line.startswith("NSpid:"))
                if int(ns_pid.split()[-1]) == primary_pid:
                    primary_visible = True
            if not primary_visible:
                break
            assert time.monotonic() < deadline, "primary status was not collected"
            time.sleep(0.01)
        assert process.poll() is None, "init completed before the late cancellation"
        os.kill(init_pid, signal.SIGTERM)
        stdout, stderr = process.communicate(timeout=5)
        assert process.returncode == 7, (stdout, stderr)
        assert "cancellation signal 15" in stderr
    finally:
        finish_owned(process, init_pid)
