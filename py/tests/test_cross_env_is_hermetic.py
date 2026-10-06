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

import importlib
import importlib.util
import os
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


@pytest.mark.parametrize(
    ("environment", "expected"),
    [
        ({}, 8),
        ({"AGENT_UTILS_VALIDATION_JOBS": "1"}, 1),
        ({"AGENT_UTILS_VALIDATION_JOBS": "4"}, 4),
        ({"AGENT_UTILS_VALIDATION_JOBS": "64"}, 8),
    ],
)
def test_boxed_bandwidth_width_follows_the_outer_validation_budget(
    environment: dict[str, str], expected: int
) -> None:
    assert _differential()._validation_jobs(environment, effective_jobs=16) == expected


@pytest.mark.parametrize("value", ("", "0", "-1", "four", " 4", "+4", "٤"))
def test_invalid_outer_validation_budget_is_refused(value: str) -> None:
    with pytest.raises(ValueError, match="must be a positive integer"):
        _differential()._validation_jobs(
            {"AGENT_UTILS_VALIDATION_JOBS": value}, effective_jobs=16
        )


def test_standalone_validation_width_obeys_effective_affinity_and_quota() -> None:
    differential = _differential()
    assert differential._validation_jobs({}, effective_jobs=4) == 4
    assert differential._validation_jobs({}, effective_jobs=64) == 8


def test_dagrun_main_forwards_the_admitted_validation_width(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    differential = _differential()
    observed: dict[str, int] = {}

    observed_skips: list[frozenset[str]] = []

    def compare(
        _random: int,
        _seed: int,
        *,
        validation_jobs: int = 8,
        allowed_skips: frozenset[str] = frozenset(),
    ) -> int:
        observed["validation_jobs"] = validation_jobs
        observed_skips.append(allowed_skips)
        return 0

    monkeypatch.setenv("AGENT_UTILS_VALIDATION_JOBS", "4")
    monkeypatch.delenv("AGENT_UTILS_CROSS_ALLOW_SKIP", raising=False)
    monkeypatch.setattr(differential, "_effective_validation_jobs", lambda: 16)
    monkeypatch.setattr(differential, "compare_dagrun", compare)

    assert differential.main(["--tool", "dagrun"]) == 0
    assert observed == {"validation_jobs": 4}
    assert observed_skips == [frozenset()]


def test_main_cannot_leave_a_record_in_an_outer_validate_run() -> None:
    """``scripts/validate.py`` exports the coverage directory to this suite's own node.

    ``main`` above writes one record per verdict into that directory, which the outer run would
    summarise as a cross node. The shared conftest removes the variable before every test.
    """

    assert "AGENT_UTILS_CROSS_COVERAGE_DIR" not in os.environ


def test_boxed_cpu_bandwidth_case_scales_every_width_observable() -> None:
    differential = _differential()
    dag, args, expected_facts, expected_quota = differential._boxed_cpu_bandwidth_case(
        4, dag_path="fixture.json", delegated=True
    )
    steps = dag["steps"]

    assert args[args.index("--dag") + 1] == "fixture.json"
    assert "-j4" in args
    assert len(steps) == 2
    assert all(step["hint"]["preferred_inner_jobs"] == 4 for step in steps)
    assert all("--cgroup-parent-levels 2" in step["cmd"] for step in steps)
    assert expected_facts == differential.CpuFootprintFacts(2, (4, 4), 2, 8)
    assert expected_quota == (4.0,)


def test_boxing_only_checks_skip_exact_unboxed_delegation_without_spawning(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    differential = _differential()
    monkeypatch.setenv("DAGRUN_DELEGATED_UNBOXED", "1")
    monkeypatch.setenv("DAGRUN_OUTER_RUN", "outer-run")

    def unexpected_run(*_args: object, **_kwargs: object) -> object:
        raise AssertionError("a boxing-only check spawned under unboxed delegation")

    monkeypatch.setattr(differential, "run", unexpected_run)
    report = differential.Report()
    differential.compare_profile_timeseries_trace(["python"], ["rust"], report)
    differential.compare_operator_build_width(["python"], ["rust"], report)
    differential.compare_boxed_cpu_bandwidth(["python"], ["rust"], report)

    # None of the four checks ran, so none of them may be counted as a pass.
    assert report.checks == 0
    assert report.failures == []
    assert [(skip.label, skip.kind) for skip in report.skipped] == [
        ("profile-timeseries", "boxing"),
        ("operator-build-width:stated", "boxing"),
        ("operator-build-width:unstated", "boxing"),
        ("boxed-cpu-bandwidth", "boxing"),
    ]


def test_delegated_cgroup_does_not_disable_boxing_only_checks(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    differential = _differential()
    monkeypatch.setenv("DAGRUN_DELEGATED_UNBOXED", "1")
    monkeypatch.setenv("DAGRUN_DELEGATED_CGROUP", "/delegated")
    monkeypatch.setenv("DAGRUN_OUTER_RUN", "outer-run")

    assert differential._inside_delegated_harness()
    assert not differential._parent_offers_only_unboxed_delegation()

    monkeypatch.delenv("DAGRUN_DELEGATED_CGROUP")
    monkeypatch.delenv("DAGRUN_OUTER_RUN")
    assert not differential._parent_offers_only_unboxed_delegation()


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


# --- Host command-line tools -------------------------------------------------------------------
#
# The agentctl and herdr-agent differentials run both editions with the developer's PATH. A case
# that lets an edition fall back to a tool on PATH, such as the default native goal transport
# `codex app-server proxy`, compares two runs of the host's real tool. Codex prints a
# per-process path, so the two runs differ and `make validate` went red on a machine with Codex
# installed while passing on one without it. The harness puts a guard ahead of PATH and reports
# every call that reaches it. These tests put an ambient tool that prints its own process ID
# first on the caller's PATH, which is the shape that diverged.


def _cross_module(name: str) -> ModuleType:
    cross = str(REPO_ROOT / "cross")
    if cross not in sys.path:
        sys.path.insert(0, cross)
    return importlib.import_module(name)


def _ambient_tool(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, program: str
) -> Path:
    """Put a host tool whose output depends on its process ID first on PATH."""
    directory = tmp_path / "ambient-bin"
    directory.mkdir(exist_ok=True)
    ran = tmp_path / f"ambient-{program}-ran"
    tool = directory / program
    tool.write_text(
        f"#!{sys.executable}\n"
        "import os, sys\n"
        f"with open({str(ran)!r}, 'a', encoding='utf-8') as stream:\n"
        "    stream.write(f'{os.getpid()}\\n')\n"
        "print(f'host state at /tmp/ambient/{os.getpid()}/arcrc', file=sys.stderr)\n"
        "raise SystemExit(1)\n",
        encoding="utf-8",
    )
    tool.chmod(0o700)
    monkeypatch.setenv("PATH", f"{directory}{os.pathsep}{os.environ.get('PATH', '')}")
    return ran


def _python_agentctl() -> list[str]:
    return [sys.executable, "-m", "agentctl"]


@pytest.mark.parametrize(
    "program", _cross_module("herdr_agent_differential").HOST_CLI_GUARDED
)
def test_an_edition_that_reaches_a_host_cli_is_refused_rather_than_compared(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, program: str
) -> None:
    herdr_agent = _cross_module("herdr_agent_differential")
    ran = _ambient_tool(tmp_path, monkeypatch, program)
    # Each "edition" runs the named tool from PATH exactly as a default transport would.
    edition = [
        sys.executable, "-c",
        "import subprocess, sys; "
        "done = subprocess.run([sys.argv[1], 'probe'], capture_output=True, text=True); "
        "print(done.returncode, done.stderr, end='')",
    ]
    harness = herdr_agent.Harness(tmp_path / "cross", edition, edition)
    try:
        case = harness.case("reaches-host-cli")
        python, rust = harness.invoke(case, (program,))
        report = herdr_agent.Report()
        harness.require_no_host_cli(report)
    finally:
        harness.close()

    # The ambient tool never ran, so its process ID cannot make the editions differ ...
    assert not ran.exists()
    assert python == rust
    assert python.stdout.startswith("127 cross harness host CLI guard")
    # ... and the call is a named failure, not a silent agreement.
    calls = harness.host_cli_calls()
    assert [call.split(": ", 1)[0] for call in calls] == [
        "001-reaches-host-cli/python/project", "001-reaches-host-cli/rust/project",
    ]
    assert all(f'"program": "{program}", "argv": ["probe"]' in call for call in calls)
    assert report.checks == 1
    assert len(report.failures) == 1
    assert report.failures[0].startswith("harness/no-host-cli: 2 call(s)")


def test_the_hostile_path_fixture_keeps_the_guard_first(tmp_path: Path) -> None:
    herdr_agent = _cross_module("herdr_agent_differential")
    edition = [sys.executable, "-c", "import os; print(os.environ['PATH'], end='')"]
    harness = herdr_agent.Harness(tmp_path / "cross", edition, edition)
    try:
        case = harness.case("hostile", {"hostile_path": True})
        python, _ = harness.invoke(case, ())
    finally:
        harness.close()
    assert python.stdout == os.pathsep.join(
        ("<ROOT>/" + herdr_agent.HOST_CLI_GUARD_DIRECTORY, "<ROOT>/hostile-bin")
    )


def test_the_unfixed_retirement_goal_query_reached_the_host_codex(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The case as it stood: a goal query with no goal command uses `codex` from PATH."""
    agentctl = _cross_module("agentctl_differential")
    ran = _ambient_tool(tmp_path, monkeypatch, "codex")
    harness = agentctl.Harness(tmp_path / "cross", _python_agentctl(), _python_agentctl())
    try:
        case = harness.case("unfixed-goal-query")
        common = ("--herdr-bin", "<HERDR>", "--registry", "<ROOT>/registry")
        started = harness.invoke(case, (
            "start", "worker", "--cwd", "<ROOT>", "--workspace-id", "w1", *common,
        ))
        assert [outcome.returncode for outcome in started] == [0, 0], started
        harness.invoke(case, ("goal", "worker", *common))
        report = agentctl.Report()
        harness.require_no_host_cli(report)
    finally:
        harness.close()

    assert not ran.exists()
    calls = harness.host_cli_calls()
    assert len(calls) == 2
    assert all('"program": "codex", "argv": ["app-server", "proxy"]' in call for call in calls)
    assert [failure.split(":", 1)[0] for failure in report.failures] == ["harness/no-host-cli"]


def test_the_retirement_goal_query_uses_the_fixture_transport(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The shipped case agrees with a PID-printing `codex` first on PATH and never runs it."""
    agentctl = _cross_module("agentctl_differential")
    ran = _ambient_tool(tmp_path, monkeypatch, "codex")
    harness = agentctl.Harness(tmp_path / "cross", _python_agentctl(), _python_agentctl())
    try:
        report = agentctl.Report()
        agentctl._workspace_retirement(harness, report)
        harness.require_no_host_cli(report)
    finally:
        harness.close()

    assert not ran.exists()
    assert harness.host_cli_calls() == []
    assert report.failures == []
    # start, status, attach, goal-query, goal-query-native, wait, stop, and the guard.
    assert report.checks == 8
