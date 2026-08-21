"""The differential must drive both engines from an environment IT chose.

``cross/differential.py`` builds every child environment from ``os.environ``, so anything the
developer exported is inherited by both engines. That is deliberate for most variables and wrong
for the ones a case is asserting about: an ambient ``CARGO_BUILD_JOBS`` makes the
``operator-build-width:unstated`` leg see "honouring CARGO_BUILD_JOBS=200" where it requires "no
CARGO_BUILD_JOBS in the environment", and ``make cross`` goes red on a difference that exists in
neither engine. The harness already pops ``SAFE_CI_DAG_RUNNER_LOG_DIR`` for exactly this reason;
the build-width pair belongs beside it.

Running the whole differential here would cost minutes, so this pins ``_env`` itself, which is the
single place every child environment is built.
"""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path
from types import ModuleType

import pytest

REPO_ROOT = Path(__file__).resolve().parents[2]


def _differential() -> ModuleType:
    # differential.py imports its sibling modules by bare name, as the harness runs it by path.
    cross = str(REPO_ROOT / "cross")
    if cross not in sys.path:
        sys.path.insert(0, cross)
    spec = importlib.util.spec_from_file_location(
        "_cross_differential_under_test", REPO_ROOT / "cross" / "differential.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_an_ambient_build_width_does_not_reach_either_engine(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setenv("CARGO_BUILD_JOBS", "200")
    monkeypatch.setenv("SAFE_CI_OPERATOR_BUILD_JOBS", "200")
    env = _differential()._env()
    assert "CARGO_BUILD_JOBS" not in env
    assert "SAFE_CI_OPERATOR_BUILD_JOBS" not in env


def test_a_case_that_is_about_intent_can_still_state_it(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # The scrubbing must not disarm the `stated` leg: `extra` is applied after the pops, so a case
    # that deliberately asks for a width still gets it.
    monkeypatch.delenv("CARGO_BUILD_JOBS", raising=False)
    env = _differential()._env({"CARGO_BUILD_JOBS": "200"})
    assert env["CARGO_BUILD_JOBS"] == "200"
