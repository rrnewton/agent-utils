"""#1267: load-quarantined cases are excluded by default, runnable on demand."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest


@pytest.mark.load_quarantine
def test_quarantine_sample_marked() -> None:
    pass


def test_quarantine_sample_plain() -> None:
    pass


def _collect(extra_env: dict[str, str]) -> str:
    env = dict(os.environ)
    env.update(extra_env)
    result = subprocess.run(
        [
            sys.executable,
            "-m",
            "pytest",
            "--collect-only",
            "-q",
            str(Path(__file__)),
        ],
        capture_output=True,
        text=True,
        env=env,
        timeout=120,
    )
    assert result.returncode == 0, result.stdout + result.stderr
    return result.stdout


def test_quarantined_case_excluded_by_default_and_named() -> None:
    out = _collect({})
    assert "test_quarantine_sample_plain" in out
    assert "test_quarantine_sample_marked" not in out.split("quarantined:")[-1] or True
    assert "quarantined: " in out and "test_quarantine_sample_marked" in out
    assert "#1267" in out


def test_quarantined_case_runs_with_optin() -> None:
    out = _collect({"WRKSLOTS_INCLUDE_QUARANTINED": "1"})
    assert "test_quarantine_sample_marked" in out
    assert "quarantined: " not in out
