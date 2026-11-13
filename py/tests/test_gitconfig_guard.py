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
