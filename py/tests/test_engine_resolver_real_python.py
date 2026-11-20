"""The dispatcher's Python leg runs the real interpreter (#1964).

Where ``python3`` is a launcher that forks the interpreter and waits
with SIGTERM caught, a supervisor's signal never reaches the work. The
``common/bin`` dispatcher therefore resolves the real interpreter once
(cached safely) and execs it on the tracked script by path. These tests
pin: the bin/ process IS the resolved interpreter (same pid, direct
parent), SIGTERM to it works, a shadowing module in the caller's cwd
cannot hijack imports, no PYTHONPATH leaks into children, the classic
``python3 py/bin/<tool>`` call still works, and the fallback executes
when resolution fails. Waits are liveness bounds after an event (the
ready file), not performance budgets.
"""

from __future__ import annotations

import os
import shutil
import signal
import stat
import subprocess
import sys
import time
from pathlib import Path


AU_ROOT = Path(__file__).resolve().parents[2]
RESOLVER = AU_ROOT / "common" / "bin" / "engine-resolver"
TOOL = "wrkslots"
LIVENESS_SECONDS = 30.0

SCRUBBED = (
    "DAGRUN_ENGINE",
    "AGENT_UTILS_ENGINE",
    "WRKSLOTS_ENGINE",
    "WRKSLOTS_RUST_READERS",
    "WRKSLOTS_RUST_READERS_FILE",
    "WRKSLOTS_PYCOMPAT",
    "WRKSLOTS_SOAK_READERS_ONLY",
    "CI_HUB_PYTHON",
    "AGENT_UTILS_PYTHON",
    "PYTHONPATH",
)


def _chmod_x(path: Path) -> None:
    path.chmod(path.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def _env(root: Path, extra: dict[str, str] | None = None) -> dict[str, str]:
    env = {k: v for k, v in os.environ.items() if k not in SCRUBBED}
    env["WRKSLOTS_RUST_READERS_FILE"] = str(root / "no-readers-file")
    env.update(extra or {})
    return env


def _build_sandbox(tmp_path: Path, stub_body: str) -> Path:
    root = tmp_path / "au"
    common_bin = root / "common" / "bin"
    python_bin = root / "py" / "bin"
    rust_bin = root / "rs" / "bin"
    for directory in (common_bin, python_bin, rust_bin):
        directory.mkdir(parents=True)
    shutil.copy2(RESOLVER, common_bin / "engine-resolver")
    _chmod_x(common_bin / "engine-resolver")
    (common_bin / TOOL).symlink_to("engine-resolver")
    stub = python_bin / TOOL
    stub.write_text(stub_body)
    _chmod_x(stub)
    return root


def _resolved_real_interpreter() -> Path:
    completed = subprocess.run(
        ["python3", "-c", "import sys; print(sys.executable)"],
        capture_output=True, text=True, check=True, timeout=30,
    )
    return Path(os.path.realpath(completed.stdout.strip()))


def test_bin_path_process_is_the_real_interpreter(tmp_path: Path) -> None:
    stub = (
        "import os, sys, time\n"
        "from pathlib import Path\n"
        "Path(sys.argv[1]).write_text(f'{os.getpid()} {sys.executable}', encoding='utf-8')\n"
        "time.sleep(60)\n"
    )
    root = _build_sandbox(tmp_path, stub)
    link_bin = tmp_path / "linkbin"
    link_bin.mkdir()
    linked = link_bin / TOOL
    linked.symlink_to(root / "common" / "bin" / TOOL)
    ready = tmp_path / "ready"
    env = _env(root, {"XDG_STATE_HOME": str(tmp_path / "state")})
    proc = subprocess.Popen(
        [str(linked), str(ready)],
        cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True, env=env,
    )
    try:
        deadline = time.monotonic() + LIVENESS_SECONDS
        while not ready.exists():
            assert time.monotonic() < deadline, "stub never became ready"
            time.sleep(0.05)
        pid_text, exe = ready.read_text(encoding="utf-8").split(" ", 1)
        pid = int(pid_text)
        assert pid == proc.pid
        assert Path(f"/proc/{pid}/exe").resolve() == _resolved_real_interpreter()
        assert Path(exe.strip()).resolve() == _resolved_real_interpreter()
        fields = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8").rsplit(")", 1)[1].split()
        assert int(fields[1]) == os.getpid()
        proc.send_signal(signal.SIGTERM)
        assert proc.wait(timeout=LIVENESS_SECONDS) == -signal.SIGTERM
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=LIVENESS_SECONDS)


def test_pythonpath_is_not_exported_to_children(tmp_path: Path) -> None:
    stub = "import os\nprint(os.environ.get('PYTHONPATH', '<unset>'))\n"
    root = _build_sandbox(tmp_path, stub)
    env = _env(root, {"XDG_STATE_HOME": str(tmp_path / "state")})
    result = subprocess.run(
        [str(root / "common" / "bin" / TOOL)],
        cwd=root, capture_output=True, text=True, env=env, check=False, timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == "<unset>\n"


def test_fallback_executes_when_resolution_fails(tmp_path: Path) -> None:
    root = _build_sandbox(tmp_path, "print('PY-fallback')\n")
    fake = tmp_path / "au-python"
    fake.write_text(
        "#!/bin/sh\n"
        'if [ "$1" = "-c" ]; then exit 1; fi\n'
        f'exec "{sys.executable}" "$@"\n',
        encoding="utf-8",
    )
    _chmod_x(fake)
    env = _env(root, {
        "XDG_STATE_HOME": str(tmp_path / "state"),
        "AGENT_UTILS_PYTHON": str(fake),
    })
    result = subprocess.run(
        [str(root / "common" / "bin" / TOOL)],
        cwd=root, capture_output=True, text=True, env=env, check=False, timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert result.stdout == "PY-fallback\n"


def test_resolution_cache_is_written_and_reused(tmp_path: Path) -> None:
    root = _build_sandbox(tmp_path, "print('PY-cached')\n")
    state = tmp_path / "state"
    env = _env(root, {"XDG_STATE_HOME": str(state)})
    command = [str(root / "common" / "bin" / TOOL)]
    first = subprocess.run(command, cwd=root, capture_output=True, text=True, env=env, check=False, timeout=30)
    assert first.returncode == 0, first.stderr
    cache = state / "agent-utils" / "real-python.cache"
    assert cache.is_file()
    assert (cache.parent.stat().st_mode & 0o777) == 0o700
    lines = cache.read_text(encoding="utf-8").splitlines()
    assert len(lines) == 4
    assert Path(lines[3]).resolve() == _resolved_real_interpreter()
    before = cache.read_bytes()
    second = subprocess.run(command, cwd=root, capture_output=True, text=True, env=env, check=False, timeout=30)
    assert second.returncode == 0, second.stderr
    assert second.stdout == "PY-cached\n"
    assert cache.read_bytes() == before


def test_cwd_shadowing_modules_cannot_hijack_the_real_entry(tmp_path: Path) -> None:
    (tmp_path / "json.py").write_text("raise RuntimeError('shadowed json')\n", encoding="utf-8")
    (tmp_path / "wrkslots.py").write_text("raise RuntimeError('shadowed wrkslots')\n", encoding="utf-8")
    env = _env(AU_ROOT, {"XDG_STATE_HOME": str(tmp_path / "state")})
    result = subprocess.run(
        [str(AU_ROOT / "common" / "bin" / "wrkslots"), "--version"],
        cwd=tmp_path, capture_output=True, text=True, env=env, check=False, timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert "wrkslots" in result.stdout


def test_python3_pybin_invocation_still_works() -> None:
    result = subprocess.run(
        ["python3", str(AU_ROOT / "py" / "bin" / "wrkslots"), "--version"],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=30,
    )
    assert result.returncode == 0, result.stderr
    assert "wrkslots" in result.stdout
