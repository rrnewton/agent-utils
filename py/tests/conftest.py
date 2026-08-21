"""Shared pytest fixtures for the safe-ci-dag-runner Python tests.

The default auto-logging profile store (Feature D) writes CSVs to ``./.safe-ci-dag-runner/profiles/``
relative to the CWD whenever a ``run``/``sweep`` executes without ``--perf-dir``/``--no-profile``.
To keep the test run hermetic (no writes into the repo checkout), an autouse fixture points
``$SAFE_CI_DAG_RUNNER_PROFILE_DIR`` at a throwaway temp directory for every test. Tests that
specifically exercise the true default location or ``--no-profile`` unset this env var themselves.

A second autouse fixture makes the operator build width ambient-proof. ``select_build_jobs``
consults an intent captured from ``$CARGO_BUILD_JOBS`` at IMPORT, so a developer who happens to
have that variable exported turned ``test_build_job_cap.py`` and ``test_sizing.py`` red — a suite
whose verdict depends on the shell that launched it is not a suite. Every test therefore starts
from "the operator stated nothing", and the handful of tests that are ABOUT operator intent set it
for themselves.
"""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest

from safe_ci_dag_runner.cli import PROFILE_DIR_ENV
from safe_ci_dag_runner.sizing import BUILD_JOBS_ENV, OPERATOR_BUILD_JOBS_ENV


@pytest.fixture(autouse=True)
def _isolated_profile_store(
    tmp_path_factory: pytest.TempPathFactory, monkeypatch: pytest.MonkeyPatch
) -> Iterator[Path]:
    """Redirect the default profile store to a per-test temp dir (no repo writes)."""
    store = tmp_path_factory.mktemp("profile_store")
    monkeypatch.setenv(PROFILE_DIR_ENV, str(store))
    yield store


@pytest.fixture(autouse=True)
def _no_ambient_operator_build_width(monkeypatch: pytest.MonkeyPatch) -> None:
    """Start every test from "the operator stated no build width".

    Both the module-level capture and the two environment variables are cleared, because they are
    read at different moments: the capture is what ``select_build_jobs`` consults in THIS
    interpreter, and the variables are what a subprocess or a re-exec would inherit. Leaving
    either behind lets a developer's shell decide whether the containment default is allowed to
    refine a step's width downward, which is the property ``test_build_job_cap.py`` exists to
    hold.
    """
    monkeypatch.delenv(BUILD_JOBS_ENV, raising=False)
    monkeypatch.delenv(OPERATOR_BUILD_JOBS_ENV, raising=False)
    monkeypatch.setattr("safe_ci_dag_runner.sizing._OPERATOR_BUILD_JOBS", None)
