"""The py/bin wrappers must run the real interpreter, not a launcher (#1964).

On hosts where ``python3`` is the fbpython launcher, the launcher forks
the real interpreter and waits with SIGTERM caught, so a supervisor's
signal never reaches the work. These tests drive the shipped shared
helper (py/bin/_real-python.sh) through a test wrapper and a sleeping
fixture module: the process that ends up sleeping must BE the resolved
interpreter (same pid the shell started, parented directly to this
test), and SIGTERM to that pid must end it. The waits below are
liveness bounds (the process is event-ready via its ready file first),
not performance budgets.
"""

from __future__ import annotations

import os
import signal
import subprocess
import sys
import time
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
HELPER = REPO_ROOT / "py" / "_real-python.sh"
LIVENESS_SECONDS = 30.0

SLEEPER = """
import os, sys, time
from pathlib import Path
Path(sys.argv[1]).write_text(f"{os.getpid()} {sys.executable}", encoding="utf-8")
time.sleep(120)
"""


def _resolved_real_interpreter() -> str:
    completed = subprocess.run(
        ["python3", "-c", "import sys; print(sys.executable)"],
        capture_output=True, text=True, check=True, timeout=30,
    )
    return os.path.realpath(completed.stdout.strip())


def _write_test_wrapper(directory: Path) -> Path:
    wrapper = directory / "au-test-wrapper"
    wrapper.write_text(
        "#!/bin/sh\n"
        f'_au_dir={REPO_ROOT / "py" / "bin"}\n'
        '. "$_au_dir/../_real-python.sh"\n'
        'exec_real_python_module au_sleeper "$@"\n',
        encoding="utf-8",
    )
    wrapper.chmod(0o755)
    return wrapper


def _wait_ready(ready: Path) -> tuple[int, str]:
    deadline = time.monotonic() + LIVENESS_SECONDS
    while time.monotonic() < deadline:
        if ready.exists():
            pid_text, exe = ready.read_text(encoding="utf-8").split(" ", 1)
            return int(pid_text), exe.strip()
        time.sleep(0.05)
    raise AssertionError(f"sleeper never became ready within {LIVENESS_SECONDS:g}s")


def test_wrapper_process_is_the_real_interpreter(tmp_path: Path) -> None:
    (tmp_path / "au_sleeper.py").write_text(SLEEPER, encoding="utf-8")
    wrapper = _write_test_wrapper(tmp_path)
    ready = tmp_path / "ready"
    env = dict(os.environ)
    env["PYTHONPATH"] = str(tmp_path)
    env["XDG_STATE_HOME"] = str(tmp_path / "state")
    proc = subprocess.Popen(
        [str(wrapper), str(ready)],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env,
    )
    try:
        pid, exe = _wait_ready(ready)
        # exec replaced the shell: the sleeper keeps the wrapper's pid and
        # is parented directly to this test — no launcher in between.
        assert pid == proc.pid
        assert Path(f"/proc/{pid}/exe").resolve() == Path(_resolved_real_interpreter())
        assert Path(exe).resolve() == Path(_resolved_real_interpreter())
        stat_fields = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8").rsplit(")", 1)[1].split()
        assert int(stat_fields[1]) == os.getpid()
        # SIGTERM to that pid ends it: the default disposition of the real
        # interpreter, unreachable when a launcher parent catches it.
        proc.send_signal(signal.SIGTERM)
        assert proc.wait(timeout=LIVENESS_SECONDS) == -signal.SIGTERM
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=LIVENESS_SECONDS)


def test_resolution_cache_is_reused_and_keyed(tmp_path: Path) -> None:
    env = dict(os.environ)
    env["XDG_STATE_HOME"] = str(tmp_path / "state")
    script = (
        f'. "{HELPER}"\n'
        "_au_resolve_real_python\n"
    )
    first = subprocess.run(
        ["sh", "-c", script], capture_output=True, text=True, check=True, env=env, timeout=30,
    )
    cache = tmp_path / "state" / "agent-utils" / "real-python.cache"
    assert cache.is_file()
    lines = cache.read_text(encoding="utf-8").splitlines()
    assert len(lines) == 3
    assert lines[2] == first.stdout.strip()
    before = cache.read_bytes()
    second = subprocess.run(
        ["sh", "-c", script], capture_output=True, text=True, check=True, env=env, timeout=30,
    )
    assert second.stdout.strip() == first.stdout.strip()
    assert cache.read_bytes() == before


def test_unresolvable_interpreter_resolves_empty(tmp_path: Path) -> None:
    env = dict(os.environ)
    env["XDG_STATE_HOME"] = str(tmp_path / "state")
    env["AGENT_UTILS_PYTHON"] = "/nonexistent-au-python"
    script = (
        f'. "{HELPER}"\n'
        "_au_resolve_real_python\n"
    )
    completed = subprocess.run(
        ["sh", "-c", script], capture_output=True, text=True, check=False, env=env, timeout=30,
    )
    assert completed.stdout.strip() == ""


def test_real_wrkslots_wrapper_runs() -> None:
    completed = subprocess.run(
        [str(REPO_ROOT / "py" / "bin" / "wrkslots"), "--version"],
        capture_output=True, text=True, check=False, timeout=30,
    )
    assert completed.returncode == 0, completed.stderr
    assert "wrkslots" in completed.stdout
