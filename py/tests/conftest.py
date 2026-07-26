"""Shared pytest fixtures for the safe-ci-dag-runner Python tests.

The default auto-logging profile store (Feature D) writes CSVs to ``./.safe-ci-dag-runner/profiles/``
relative to the CWD whenever a ``run``/``sweep`` executes without ``--perf-dir``/``--no-profile``.
To keep the test run hermetic (no writes into the repo checkout), an autouse fixture points
``$SAFE_CI_DAG_RUNNER_PROFILE_DIR`` at a throwaway temp directory for every test. Tests that
specifically exercise the true default location or ``--no-profile`` unset this env var themselves.
"""

from __future__ import annotations

from collections.abc import Iterator
from pathlib import Path

import pytest

from safe_ci_dag_runner.cli import PROFILE_DIR_ENV


@pytest.fixture(autouse=True)
def _isolated_profile_store(
    tmp_path_factory: pytest.TempPathFactory, monkeypatch: pytest.MonkeyPatch
) -> Iterator[Path]:
    """Redirect the default profile store to a per-test temp dir (no repo writes)."""
    store = tmp_path_factory.mktemp("profile_store")
    monkeypatch.setenv(PROFILE_DIR_ENV, str(store))
    yield store
