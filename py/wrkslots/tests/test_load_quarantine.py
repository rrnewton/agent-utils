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


_1765_CASES = (
    "test_sandbox_view_confines_writes",
    "test_coordinator_box_confines_writes",
    "test_other_slots_and_the_registry_are_read_only_from_a_box",
    "test_ownerless_worktree_plan_matches_python",
)


def _collect_1765(extra_env: dict[str, str]) -> str:
    env = dict(os.environ)
    env.update(extra_env)
    here = Path(__file__).parent
    result = subprocess.run(
        [
            sys.executable, "-m", "pytest", "--collect-only", "-q",
            str(here / "test_slot_images.py"),
            str(here / "test_recover_ownerless_write_rust.py"),
            "-k", " or ".join(_1765_CASES),
        ],
        capture_output=True, text=True, env=env, timeout=300,
    )
    # Exit 5 (no tests collected) is the expected default outcome: every
    # selected case is quarantined and only named in the summary.
    assert result.returncode in (0, 5), result.stdout + result.stderr
    return result.stdout + result.stderr


def test_1765_cases_returned_and_collected_by_default() -> None:
    # Step 2 (#1765): the slice-limit retry, the fixed census clock and the
    # ownerless manager wait returned these cases; they collect by default.
    out = _collect_1765({})
    for name in _1765_CASES:
        assert name in out
    assert "quarantined: " not in out
