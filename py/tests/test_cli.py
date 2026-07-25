"""Tests for the safe-ci-dag-runner CLI."""

from __future__ import annotations

import subprocess
import sys

from safe_ci_dag_runner import __version__
from safe_ci_dag_runner.cli import PROG, main


def test_main_no_args_returns_zero() -> None:
    assert main([]) == 0


def test_version_via_module() -> None:
    result = subprocess.run(
        [sys.executable, "-m", "safe_ci_dag_runner", "--version"],
        capture_output=True,
        text=True,
        check=True,
    )
    assert result.stdout.strip() == f"{PROG} {__version__}"
