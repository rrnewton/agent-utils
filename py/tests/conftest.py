"""Shared pytest fixtures for the dagrun Python tests.

The default auto-logging profile store (Feature D) writes CSVs to ``./.dagrun/profiles/``
relative to the CWD whenever a ``run``/``sweep`` executes without ``--perf-dir``/``--no-profile``.
To keep the test run hermetic (no writes into the repo checkout), an autouse fixture points
``$DAGRUN_PROFILE_DIR`` at a throwaway temp directory for every test. Tests that
specifically exercise the true default location or ``--no-profile`` unset this env var themselves.

A second autouse fixture makes the operator build width ambient-proof. ``select_build_jobs``
consults an intent captured from ``$CARGO_BUILD_JOBS`` at IMPORT, so a developer who happens to
have that variable exported turned ``test_build_job_cap.py`` and ``test_sizing.py`` red — a suite
whose verdict depends on the shell that launched it is not a suite. Every test therefore starts
from "the operator stated nothing", and the handful of tests that are ABOUT operator intent set it
for themselves.

A third isolates runner authority inherited when this suite is itself one delegated validation
node. Ordinary CLI unit cases still model fresh top-level invocations; the real nesting smokes
explicitly restore the captured authority to their subprocesses.
"""

from __future__ import annotations

import os
from collections.abc import Iterator, Mapping
from pathlib import Path

import pytest

from dagrun.cli import PROFILE_DIR_ENV
from dagrun.sizing import BUILD_JOBS_ENV, OPERATOR_BUILD_JOBS_ENV


_RUNNER_AUTHORITY_ENV = (
    "DAGRUN_DELEGATED_CGROUP",
    "DAGRUN_DELEGATED_UNBOXED",
    "DAGRUN_EXPECTED_OUTER_CPU_COUNT",
    "DAGRUN_EXPECTED_OUTER_MEMORY_MAX_BYTES",
    "DAGRUN_EXPECTED_RUNTIME_MAX_SEC",
    "DAGRUN_IN_SCOPE",
    "DAGRUN_OUTER_RUN",
    "DAGRUN_SCOPE_UNIT",
)
_INHERITED_RUNNER_AUTHORITY = {
    name: value for name in _RUNNER_AUTHORITY_ENV if (value := os.environ.get(name)) is not None
}


@pytest.fixture
def inherited_runner_authority_env() -> Mapping[str, str]:
    """Original outer-run authority for the few tests that deliberately nest real schedulers."""
    return dict(_INHERITED_RUNNER_AUTHORITY)


@pytest.fixture(autouse=True)
def _no_ambient_runner_authority(monkeypatch: pytest.MonkeyPatch) -> None:
    """Make CLI unit tests model a fresh invocation even under the delegated validation lane.

    The outer validator delegates its cgroup so real nested-scheduler smokes stay contained. That
    authority is not test input for hundreds of ordinary in-process CLI cases: inherited CPU and
    memory ceilings would otherwise rewrite their explicit fixtures. Tests that exercise actual
    nesting request ``inherited_runner_authority_env`` and restore it only to their subprocess.
    """
    for name in _RUNNER_AUTHORITY_ENV:
        monkeypatch.delenv(name, raising=False)


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
    monkeypatch.setattr("dagrun.sizing._OPERATOR_BUILD_JOBS", None)
