"""The differential must drive both engines from an environment IT chose.

``cross/differential.py`` builds every child environment from ``os.environ``, so anything the
developer exported is inherited by both engines. That is deliberate for most variables and wrong
for the ones a case is asserting about: an ambient ``CARGO_BUILD_JOBS`` makes the
``operator-build-width:unstated`` leg see "honouring CARGO_BUILD_JOBS=200" where it requires "no
CARGO_BUILD_JOBS in the environment".  More seriously, an outer validation DAG's delegated
cgroup and memory ceiling can turn large synthetic sizing cases into real nested-run refusals.
Both make ``make cross`` go red on a difference that exists in neither engine.  Runner authority
is retained only by the cases explicitly exercising containment; other case controls are stated
by each fixture.

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
    monkeypatch.setenv("DAGRUN_OPERATOR_BUILD_JOBS", "200")
    env = _differential()._env()
    assert "CARGO_BUILD_JOBS" not in env
    assert "DAGRUN_OPERATOR_BUILD_JOBS" not in env


def test_a_case_that_is_about_intent_can_still_state_it(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    # The scrubbing must not disarm the `stated` leg: `extra` is applied after the pops, so a case
    # that deliberately asks for a width still gets it.
    monkeypatch.delenv("CARGO_BUILD_JOBS", raising=False)
    env = _differential()._env({"CARGO_BUILD_JOBS": "200"})
    assert env["CARGO_BUILD_JOBS"] == "200"


def test_ambient_runner_authority_and_resource_policy_do_not_reach_synthetic_cases(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    differential = _differential()
    controlled = (
        differential._RUNNER_AUTHORITY_ENV
        | differential._AMBIENT_CASE_CONTROL_ENV
    )
    for name in controlled:
        monkeypatch.setenv(name, "ambient-value-must-not-become-fixture-input")
    monkeypatch.setenv("DAGRUN_STEP", "outer.owner")
    monkeypatch.setenv("DAGRUN_STEP_STARTED_MONOTONIC_NS", "1234")

    env = differential._env()

    assert controlled.isdisjoint(env)
    assert env["DAGRUN_STEP"] == "outer.owner"
    assert env["DAGRUN_STEP_STARTED_MONOTONIC_NS"] == "1234"


def test_boxed_cases_retain_authority_but_not_ambient_policy(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    differential = _differential()
    for name in differential._RUNNER_AUTHORITY_ENV:
        monkeypatch.setenv(name, f"authority:{name}")
    for name in differential._AMBIENT_CASE_CONTROL_ENV:
        monkeypatch.setenv(name, f"policy:{name}")

    env = differential._env(inherit_runner_authority=True)

    assert {
        name: env.get(name) for name in differential._RUNNER_AUTHORITY_ENV
    } == {
        name: f"authority:{name}" for name in differential._RUNNER_AUTHORITY_ENV
    }
    assert differential._AMBIENT_CASE_CONTROL_ENV.isdisjoint(env)


def test_explicit_fixture_values_win_after_the_ambient_scrub(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    differential = _differential()
    monkeypatch.setenv("DAGRUN_DELEGATED_CGROUP", "/ambient/delegation")
    monkeypatch.setenv("DAGRUN_OUTER_MEMORY_MAX_BYTES", "100")

    env = differential._env(
        {
            "DAGRUN_DELEGATED_CGROUP": "/fixture/delegation",
            "DAGRUN_OUTER_MEMORY_MAX_BYTES": "200",
        }
    )

    assert env["DAGRUN_DELEGATED_CGROUP"] == "/fixture/delegation"
    assert env["DAGRUN_OUTER_MEMORY_MAX_BYTES"] == "200"


def test_run_output_normalization_removes_only_terminal_elapsed_time() -> None:
    differential = _differential()
    python_output = (
        "plan: cpa\n"
        "scheduled order: p.bud, p.knee\n"
        "[p.bud] ✓ PASS   budget-capped (0s)\n"
        "[p.knee] ✗ FAIL   sub-linear knee (2s, exit 7)\n"
    )
    rust_output = python_output.replace("(0s)", "(1s)").replace("(2s, exit 7)", "(3s, exit 7)")

    normalized = differential._deterministic_run_output
    assert normalized(python_output) == normalized(rust_output)
    assert normalized(python_output) != normalized(
        rust_output.replace("scheduled order: p.bud, p.knee", "scheduled order: p.knee, p.bud")
    )
    assert normalized(python_output) != normalized(
        rust_output.replace("sub-linear knee", "different step")
    )
    assert normalized(python_output) != normalized(
        rust_output.replace("exit 7", "exit 8")
    )


def test_only_exact_transient_user_bus_refusals_are_retryable() -> None:
    differential = _differential()
    outcome = differential.Outcome

    assert differential._transient_user_bus_refusal(
        outcome(
            1,
            "",
            "Failed to connect to user scope bus via local transport: Connection refused\n",
        )
    )
    assert differential._transient_user_bus_refusal(
        outcome(0, "", ""),
        outcome(1, "", "Failed to connect to user scope bus: Connection refused"),
    )
    assert not differential._transient_user_bus_refusal(
        outcome(1, "", "Failed to connect to user scope bus: Connection refused"),
        outcome(1, "", "a real allocator failure"),
    )
    assert not differential._transient_user_bus_refusal(
        outcome(1, "", "Failed to connect to user scope bus: Permission denied")
    )
    assert not differential._transient_user_bus_refusal(
        outcome(1, "", "Connection refused")
    )
    assert not differential._transient_user_bus_refusal(
        outcome(0, "", "Failed to connect to user scope bus: Connection refused")
    )
