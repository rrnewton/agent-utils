"""The global git config guard keeps tests off the real ``~/.gitconfig``.

#1474 tests-rewrite-global-gitconfig. These cases never modify the real
file: unit cases fingerprint temp files, the redirect case writes through
``git config --global`` (which the session guard has already pointed at the
per-session sentinel), and the end-to-end case runs a culprit test in a
subprocess whose ``HOME`` is a temp directory.
"""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest

from gitconfig_guard import GlobalGitConfigGuard, fingerprint, real_gitconfig_path


def test_guard_redirect_is_active_for_this_session() -> None:
    sentinel = os.environ.get("GIT_CONFIG_GLOBAL")
    assert sentinel, "the conftest guard did not set GIT_CONFIG_GLOBAL"
    assert Path(sentinel).is_file()
    assert Path(sentinel).resolve() != real_gitconfig_path().resolve()
    assert os.environ.get("GIT_CONFIG_NOSYSTEM") == "1"


def test_global_write_lands_in_sentinel_not_real_file() -> None:
    real = real_gitconfig_path()
    before = fingerprint(real)
    subprocess.run(
        ["git", "config", "--global", "user.email", "guard-probe@example.invalid"],
        check=True,
        timeout=30,
    )
    assert fingerprint(real) == before
    sentinel = Path(os.environ["GIT_CONFIG_GLOBAL"])
    assert "guard-probe@example.invalid" in sentinel.read_text(encoding="utf-8")


def test_check_ok_warn_fail(tmp_path: Path) -> None:
    real = tmp_path / ".gitconfig"
    real.write_text("[user]\n\temail = a@example.invalid\n", encoding="utf-8")
    # A throwaway environ: start() redirects it, never this process's env.
    guard = GlobalGitConfigGuard.start(environ={"HOME": str(tmp_path)})
    assert guard.check()[0] == "ok"
    # Identical bytes at a new inode (a sync copy) warn, not fail.
    replacement = tmp_path / "replacement"
    replacement.write_bytes(real.read_bytes())
    os.replace(replacement, real)
    assert guard.check()[0] == "warn"
    real.write_text("[user]\n\temail = b@example.invalid\n", encoding="utf-8")
    assert guard.check()[0] == "fail"


def test_cleanup_removes_the_sentinel_directory(tmp_path: Path) -> None:
    guard = GlobalGitConfigGuard.start(environ={"HOME": str(tmp_path)})
    directory = guard.sentinel.parent
    assert directory.name.startswith("agent-utils-gitconfig-global-")
    assert guard.sentinel.is_file()
    guard.cleanup()
    assert not directory.exists()
    guard.cleanup()  # idempotent: a second call at exit must not raise


def test_context_manager_removes_the_sentinel_directory(tmp_path: Path) -> None:
    with GlobalGitConfigGuard.start(environ={"HOME": str(tmp_path)}) as guard:
        directory = guard.sentinel.parent
        assert guard.sentinel.is_file()
    assert not directory.exists()


def test_cleanup_keeps_the_directory_when_a_write_landed(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    guard = GlobalGitConfigGuard.start(environ={"HOME": str(tmp_path)})
    guard.sentinel.write_bytes(b"[user]\n\temail = caught@example.invalid\n")
    guard.cleanup()
    assert guard.sentinel.is_file(), "cleanup deleted the caught write"
    assert str(guard.sentinel.parent) in capsys.readouterr().err
    guard.cleanup()  # still idempotent; the kept directory stays
    import shutil

    shutil.rmtree(guard.sentinel.parent)  # test hygiene: drop the kept evidence


def test_sigterm_mid_session_removes_the_sentinel_directory(
    tmp_path: Path,
) -> None:
    # A worker killed with SIGTERM mid-session runs the guard's signal
    # handler: the seed-only directory is removed before the process dies.
    import signal as signal_module

    py_root = Path(__file__).resolve().parents[1]
    script = (
        "import sys, time\n"
        f"sys.path.insert(0, {str(py_root)!r})\n"
        "from gitconfig_guard import GlobalGitConfigGuard\n"
        "guard = GlobalGitConfigGuard.start()\n"
        "print(guard.sentinel.parent, flush=True)\n"
        "time.sleep(60)\n"
    )
    proc = subprocess.Popen(
        [sys.executable, "-c", script],
        env={**os.environ, "HOME": str(tmp_path)},
        stdout=subprocess.PIPE,
        text=True,
    )
    assert proc.stdout is not None
    try:
        directory = Path(proc.stdout.readline().strip())
        assert directory.name.startswith("agent-utils-gitconfig-global-")
        proc.send_signal(signal_module.SIGTERM)
        proc.wait(timeout=30)
    finally:
        if proc.poll() is None:
            proc.kill()
            proc.wait(timeout=30)
    assert not directory.exists()


def test_atexit_removes_the_sentinel_directory_on_failure_exit(
    tmp_path: Path,
) -> None:
    # A failing interpreter exit still runs atexit: start a guard in a
    # subprocess, exit nonzero without cleanup, and assert no entry survives.
    py_root = Path(__file__).resolve().parents[1]
    script = (
        "import sys\n"
        f"sys.path.insert(0, {str(py_root)!r})\n"
        "from gitconfig_guard import GlobalGitConfigGuard\n"
        "guard = GlobalGitConfigGuard.start()\n"
        "print(guard.sentinel.parent, flush=True)\n"
        "sys.exit(3)\n"
    )
    run = subprocess.run(
        [sys.executable, "-c", script],
        env={**os.environ, "HOME": str(tmp_path)},
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert run.returncode == 3, run.stderr
    directory = Path(run.stdout.strip().splitlines()[-1])
    assert directory.name.startswith("agent-utils-gitconfig-global-")
    assert not directory.exists(), run.stderr


def test_session_fails_when_a_test_writes_the_real_file(tmp_path: Path) -> None:
    home = tmp_path / "home"
    home.mkdir()
    (home / ".gitconfig").write_text(
        "[user]\n\temail = owner@example.invalid\n", encoding="utf-8"
    )
    py_root = Path(__file__).resolve().parents[1]
    (tmp_path / "conftest.py").write_text(
        "import sys\n"
        f"sys.path.insert(0, {str(py_root)!r})\n"
        "import pytest\n"
        "from gitconfig_guard import GlobalGitConfigGuard\n"
        "_guard = None\n"
        "def pytest_sessionstart(session):\n"
        "    global _guard\n"
        "    _guard = GlobalGitConfigGuard.start()\n"
        "def pytest_sessionfinish(session, exitstatus):\n"
        "    status, message = _guard.check()\n"
        "    session.config._guard_result = (status, message)\n"
        "    if status == 'fail':\n"
        "        session.exitstatus = pytest.ExitCode.TESTS_FAILED\n"
        "def pytest_terminal_summary(terminalreporter):\n"
        "    status, message = getattr(terminalreporter.config, '_guard_result', ('ok', ''))\n"
        "    if status != 'ok':\n"
        "        terminalreporter.write_line(f'guard {status}: {message}')\n",
        encoding="utf-8",
    )
    (tmp_path / "test_culprit.py").write_text(
        "import os\n"
        "import subprocess\n"
        "def test_writes_real_global_config():\n"
        "    target = os.path.join(os.environ['HOME'], '.gitconfig')\n"
        "    subprocess.run(['git', 'config', '--file', target, 'user.email', "
        "'culprit@example.invalid'], check=True)\n",
        encoding="utf-8",
    )
    env = {k: v for k, v in os.environ.items() if k != "GIT_CONFIG_GLOBAL"}
    env["HOME"] = str(home)
    # The fake HOME hides the operator's user site-packages, where pytest
    # lives; name it explicitly so the subprocess interpreter finds pytest.
    import site

    env["PYTHONPATH"] = os.pathsep.join(
        [site.getusersitepackages(), env.get("PYTHONPATH", "")]
    )
    run = subprocess.run(
        [sys.executable, "-m", "pytest", "-q", "-p", "no:cacheprovider", str(tmp_path)],
        env=env,
        capture_output=True,
        text=True,
        timeout=120,
    )
    output = run.stdout + run.stderr
    assert run.returncode != 0, output
    assert "changed content during the test session" in output, output
    # The culprit really did write the fake home's file; the guard caught it.
    assert "culprit@example.invalid" in (home / ".gitconfig").read_text(encoding="utf-8")
