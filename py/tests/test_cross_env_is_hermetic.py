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

from agentctl import agent as agentctl_agent
from agentctl.client import HerdrClient

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




# The Python edition with the host-wide target locks moved to private files. A fresh interpreter
# does not inherit conftest's in-process patch, and `start` would otherwise wait on the account's
# real lock files, which live agents hold. The locks are still real flocks.
_PRIVATE_LOCK_AGENTCTL = """import os, sys
from agentctl import agent
roots = tuple(os.path.join(sys.argv[1], part) for part in ("account", "legacy"))
def private_lock_paths(name):
    lock_name = agent._target_lock_name(name)
    return os.path.join(roots[0], lock_name), os.path.join(roots[1], lock_name)
agent._target_lock_paths = private_lock_paths
sys.argv = ["agentctl", *sys.argv[2:]]
from agentctl.cli import main
raise SystemExit(main())
"""


def _python_agentctl(lock_root: Path) -> list[str]:
    for part in ("account", "legacy"):
        (lock_root / part).mkdir(parents=True, mode=0o700, exist_ok=True)
    lock_root.chmod(0o700)
    # A just-recorded refresh of the legacy lock files, so a lock lookup does not scan.
    (lock_root / "account" / agentctl_agent._LEGACY_REFRESH_MARKER).touch(mode=0o600)
    return [sys.executable, "-c", _PRIVATE_LOCK_AGENTCTL, str(lock_root)]


def _private_locks_taken(lock_root: Path) -> list[str]:
    return sorted(
        path.name for path in (lock_root / "account").iterdir()
        if path.name != agentctl_agent._LEGACY_REFRESH_MARKER
    )


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
        python, rust = harness.invoke(case, (program, *herdr_agent.FIXTURE_HERDR))
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


def test_the_production_client_ignores_path_for_the_default_herdr(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Why the harness refuses instead of stubbing Herdr: Python never consults a PATH stub."""
    _ambient_tool(tmp_path, monkeypatch, "herdr")
    home = tmp_path / "home"
    (home / "bin").mkdir(parents=True)
    installed = home / "bin" / "herdr"
    installed.write_text("#!/bin/sh\nexit 1\n", encoding="utf-8")
    installed.chmod(0o700)
    monkeypatch.setattr(HerdrClient, "_account_home", staticmethod(lambda: str(home)))

    resolved = HerdrClient()._executable()

    on_this_host = {
        os.path.realpath(path) for path in ("/usr/local/bin/herdr", "/usr/bin/herdr")
        if os.path.isfile(path) and os.access(path, os.X_OK)
    }
    assert resolved in on_this_host | {str(installed)}
    assert resolved != str(tmp_path / "ambient-bin" / "herdr")


def _recording_edition(tmp_path: Path) -> tuple[list[str], Path]:
    started = tmp_path / "edition-started"
    edition = [
        sys.executable, "-c",
        f"open({str(started)!r}, 'a', encoding='utf-8').write('started\\n')",
    ]
    return edition, started


@pytest.mark.parametrize("arguments", (
    ("start", "worker"),
    ("unknown",),
    ("status", "--pane", "--help"),
    ("--registry", "<ROOT>/registry", "send", "worker", "text"),
    ("start", "worker", "--herdr-bin", "herdr"),
    ("start", "worker", "--herdr-bin=/usr/local/bin/herdr"),
    ("start", "worker", "--herdr-bin", "<ROOT>/../outside/herdr"),
    # After a `--` terminator these are message text and a positional name, not options.
    ("send", "worker", "--", "--herdr-bin=<HERDR>"),
    ("send", "worker", "--", "--herdr-bin", "<HERDR>"),
    ("--", "start", "worker"),
    ("--", "start", "--help"),
    ("start", "--", "--help"),
    # After a root-level `--`, the Python subcommand parser reads options again.
    ("--herdr-bin", "<HERDR>", "--", "start", "worker", "--herdr-bin", "herdr"),
    ("--herdr-bin", "<HERDR>", "--", "start", "worker", "--herdr-bin=/usr/local/bin/herdr"),
    # The last --herdr-bin wins in both parsers.
    ("start", "worker", "--herdr-bin", "<HERDR>", "--herdr-bin", "herdr"),
    # The Python Chat bridge builds a default client, so naming the fixture cannot redirect it.
    ("chat", "launch", "--config", "<ROOT>/chat.json", "--harness-arg", "--herdr-bin=<HERDR> x"),
    ("chat", "init", "--config", "<ROOT>/chat.json", "--herdr-bin", "<HERDR>"),
    ("--herdr-bin", "<HERDR>", "chat", "status"),
))
def test_an_invocation_that_could_reach_the_installed_herdr_is_never_started(
    tmp_path: Path, arguments: tuple[str, ...]
) -> None:
    herdr_agent = _cross_module("herdr_agent_differential")
    edition, started = _recording_edition(tmp_path)
    harness = herdr_agent.Harness(tmp_path / "cross", edition, edition)
    try:
        case = harness.case("installed-herdr")
        python, rust = harness.invoke(case, arguments)
        report = herdr_agent.Report()
        harness.require_no_host_cli(report)
    finally:
        harness.close()

    assert not started.exists()
    assert python == rust
    assert python.returncode == 127
    assert python.stderr.startswith("cross harness host CLI guard: refused to run the editions")
    calls = harness.host_cli_calls()
    assert len(calls) == 2
    assert all('"program": "herdr"' in call and '"refused": ' in call for call in calls)
    assert [failure.split(":", 1)[0] for failure in report.failures] == ["harness/no-host-cli"]


@pytest.mark.parametrize("arguments", (
    (),
    ("--help",),
    ("--version",),
    ("--userguide", "unknown"),
    ("userguide",),
    ("quickstart",),
    ("capabilities",),
    ("profiles", "--cwd", "<ROOT>"),
    ("skill", "install", "--harness", "codex"),
    ("stop", "--help"),
    ("--registry", "--help"),
    ("start", "worker", "--herdr-bin", "<HERDR>"),
    ("start", "worker", "--herdr-bin=<HERDR>"),
    ("start", "worker", "--herdr-bin", "./fake-herdr"),
    ("--herdr-bin", "<HERDR>", "start", "worker"),
    ("send", "worker", "--herdr-bin", "<HERDR>", "--", "--text-that-looks-like-an-option"),
    ("chat", "--help"),
))
def test_herdr_free_commands_and_fixture_herdr_invocations_still_run(
    tmp_path: Path, arguments: tuple[str, ...]
) -> None:
    herdr_agent = _cross_module("herdr_agent_differential")
    edition, started = _recording_edition(tmp_path)
    harness = herdr_agent.Harness(tmp_path / "cross", edition, edition)
    try:
        case = harness.case("fixture-herdr")
        python, rust = harness.invoke(case, arguments)
    finally:
        harness.close()

    assert (python.returncode, rust.returncode) == (0, 0)
    assert started.read_text(encoding="utf-8") == "started\n" * 2
    assert harness.host_cli_calls() == []


@pytest.mark.parametrize("link", ("linked-herdr", "linked-directory/herdr"))
def test_a_case_symlink_to_an_outside_herdr_is_never_started(tmp_path: Path, link: str) -> None:
    """Both production clients canonicalize the executable, so a link names its target."""
    herdr_agent = _cross_module("herdr_agent_differential")
    outside = tmp_path / "outside"
    outside.mkdir()
    (outside / "herdr").write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    (outside / "herdr").chmod(0o700)
    edition, started = _recording_edition(tmp_path)
    harness = herdr_agent.Harness(tmp_path / "cross", edition, edition)
    try:
        case = harness.case("linked-herdr")
        for root in (case.python_root, case.rust_root):
            (root / "linked-herdr").symlink_to(outside / "herdr")
            (root / "linked-directory").symlink_to(outside, target_is_directory=True)
        python, rust = harness.invoke(case, ("start", "worker", "--herdr-bin", f"<ROOT>/{link}"))
        relative, _ = harness.invoke(case, ("start", "worker", "--herdr-bin", f"./{link}"))
    finally:
        harness.close()

    assert not started.exists()
    assert python == rust
    assert python.returncode == relative.returncode == 127
    assert "is not a fixture inside the case directory" in python.stderr
    assert len(harness.host_cli_calls()) == 4


def _executable_file(path: Path) -> None:
    path.write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    path.chmod(0o700)


@pytest.mark.parametrize("layout", ("python-escapes", "kernel-escapes"))
def test_a_symlink_before_dotdot_must_stay_inside_under_both_readings(
    tmp_path: Path, layout: str
) -> None:
    """`link/../herdr`: the Python client drops `..` before following links, the kernel after.

    Each layout keeps one reading inside the case and sends the other outside, so the guard must
    check both. The Python reading is taken from the production resolver, which only inspects
    the file; nothing is executed.
    """
    herdr_agent = _cross_module("herdr_agent_differential")
    from agentctl import client as agentctl_client

    outside = tmp_path / "outside"
    (outside / "deeper").mkdir(parents=True)
    _executable_file(outside / "herdr")
    edition, started = _recording_edition(tmp_path)
    harness = herdr_agent.Harness(tmp_path / "cross", edition, edition)
    try:
        case = harness.case("dotdot-herdr")
        for root in (case.python_root, case.rust_root):
            (root / "nested" / "deeper").mkdir(parents=True)
            _executable_file(root / "nested" / "herdr")
            if layout == "python-escapes":
                (root / "link").symlink_to(root / "nested" / "deeper", target_is_directory=True)
                (root / "herdr").symlink_to(outside / "herdr")
            else:
                (root / "link").symlink_to(outside / "deeper", target_is_directory=True)
                _executable_file(root / "herdr")
        root = case.python_root
        python_reading = agentctl_client._validated_executable(
            str(root / "link" / ".." / "herdr"), "Herdr"
        )
        kernel_reading = os.path.realpath(root / "link" / ".." / "herdr")
        python, rust = harness.invoke(
            case, ("start", "worker", "--herdr-bin", "<ROOT>/link/../herdr")
        )
        relative, _ = harness.invoke(case, ("start", "worker", "--herdr-bin", "./link/../herdr"))
    finally:
        harness.close()

    inside = os.path.realpath(root / "nested" / "herdr")
    escaped = os.path.realpath(outside / "herdr")
    if layout == "python-escapes":
        assert (python_reading, kernel_reading) == (escaped, inside)
    else:
        assert (python_reading, kernel_reading) == (os.path.realpath(root / "herdr"), escaped)
    assert not started.exists()
    assert python == rust
    assert python.returncode == relative.returncode == 127
    assert "is not a fixture inside the case directory" in python.stderr
    assert "is not a fixture inside the case directory" in relative.stderr
    assert len(harness.host_cli_calls()) == 4


@pytest.mark.parametrize(
    "variable", sorted(_cross_module("herdr_agent_differential").AMBIENT_EXECUTABLE_OVERRIDES)
)
def test_an_ambient_executable_override_is_not_inherited(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, variable: str
) -> None:
    """An absolute override in the caller's environment would bypass every PATH stub."""
    herdr_agent = _cross_module("herdr_agent_differential")
    program = herdr_agent.AMBIENT_EXECUTABLE_OVERRIDES[variable]
    ran = tmp_path / "override-ran"
    override = tmp_path / "override-bin" / program
    override.parent.mkdir()
    override.write_text(f"#!/bin/sh\necho $$ > {ran}\n", encoding="utf-8")
    override.chmod(0o700)
    monkeypatch.setenv(variable, str(override))
    # Each "edition" resolves the tool as agentctl/foreign/lib.py does: the override, else the name.
    edition = [
        sys.executable, "-c",
        "import os, subprocess, sys; "
        "done = subprocess.run([os.environ.get(sys.argv[1], sys.argv[2]), 'probe'], "
        "capture_output=True, text=True); print(done.returncode, end='')",
    ]
    harness = herdr_agent.Harness(tmp_path / "cross", edition, edition)
    try:
        case = harness.case("ambient-override")
        python, rust = harness.invoke(case, (variable, program, *herdr_agent.FIXTURE_HERDR))
        report = herdr_agent.Report()
        harness.require_no_host_cli(report)
    finally:
        harness.close()

    assert not ran.exists()
    assert python == rust
    assert python.stdout == "127"
    calls = harness.host_cli_calls()
    assert len(calls) == 2
    assert all(f'"program": "{program}", "argv": ["probe"]' in call for call in calls)
    assert [failure.split(":", 1)[0] for failure in report.failures] == ["harness/no-host-cli"]


# Invocations whose refusal is checked against what the Python parsers actually select.
_PARSER_PROBES: tuple[tuple[str, ...], ...] = (
    ("start", "worker"),
    ("start", "worker", "--herdr-bin", "<HERDR>"),
    ("--herdr-bin", "<HERDR>", "start", "worker"),
    ("--herdr-bin=<HERDR>", "status", "--pane", "w1:p1"),
    ("--registry", "<ROOT>/registry", "send", "worker", "text"),
    ("send", "worker", "--", "--herdr-bin=<HERDR>"),
    ("send", "worker", "--", "--herdr-bin", "<HERDR>"),
    ("send", "--pane", "w1:p1", "--", "--herdr-bin=<HERDR>"),
    ("--", "start", "worker"),
    ("--", "start", "--help"),
    ("start", "--", "--help"),
    ("start", "--help"),
    ("start", "worker", "--herdr-bin", "<HERDR>", "--herdr-bin", "herdr"),
    ("start", "worker", "--herdr-bin", "herdr", "--herdr-bin", "<HERDR>"),
    ("--herdr-bin", "<HERDR>", "--", "start", "worker", "--herdr-bin", "herdr"),
    ("--herdr-bin", "<HERDR>", "--", "start", "worker", "--herdr-bin=/usr/local/bin/herdr"),
    ("--herdr-bin", "<HERDR>", "--", "start", "worker"),
    ("--pane", "w1:p1", "status"),
    ("status", "--pane", "w1:p1", "--herdr-bin", "<HERDR>"),
    ("--queue", "<ROOT>/queue", "userguide"),
    ("userguide", "status"),
    ("status", "userguide"),
    ("--userguide",),
    ("--version",),
    ("capabilities",),
    ("profiles", "--cwd", "<ROOT>"),
    ("quickstart",),
    ("skill", "install", "--harness", "codex"),
    ("goal", "worker", "--herdr-bin", "<HERDR>"),
    ("goal", "worker"),
    ("stop", "--help"),
    (),
)


def _parsed_herdr(parse: object, arguments: list[str]) -> tuple[str | None, bool] | None:
    """The Herdr a parser selects and whether its command is Herdr-free; None if it exits."""
    import argparse
    import contextlib
    import io

    assert callable(parse)
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        try:
            namespace = parse(arguments)
        except SystemExit:
            return None
    assert isinstance(namespace, argparse.Namespace)
    herdr = getattr(namespace, "herdr_bin", None)
    command = getattr(namespace, "command", None)
    free = bool(getattr(namespace, "userguide", False)) or command in (
        None, "capabilities", "profiles", "quickstart", "skill", "userguide",
    )
    return (herdr if isinstance(herdr, str) else None), free


@pytest.mark.parametrize("arguments", _PARSER_PROBES)
def test_an_invocation_the_guard_admits_never_selects_an_installed_herdr(
    tmp_path: Path, arguments: tuple[str, ...]
) -> None:
    """Check the syntactic guard against both Python parsers, without constructing a client.

    The Rust edition searches PATH for a bare `herdr`, where the stub catches it; the Python
    edition does not, so its parsers are the ones the guard must never under-approximate.
    """
    herdr_agent = _cross_module("herdr_agent_differential")
    from agentctl import cli as agentctl_cli
    from agentctl import legacy_cli

    root = tmp_path / "case"
    root.mkdir()
    (root / "fake-herdr").write_text("#!/bin/sh\nexit 0\n", encoding="utf-8")
    expanded = [
        value.replace("<ROOT>", str(root)).replace("<HERDR>", str(root / "fake-herdr"))
        for value in arguments
    ]
    refusal = herdr_agent.herdr_refusal(root, expanded)
    parsers = {
        "agentctl": agentctl_cli.parser().parse_args,
        "herdr-agent": legacy_cli._parser().parse_intermixed_args,
    }
    for name, parse in parsers.items():
        parsed = _parsed_herdr(parse, expanded)
        if parsed is None:
            continue  # a usage error, help, or version output exits before any client exists
        herdr, free = parsed
        if free:
            continue
        # Judged independently of the guard, the way client._validated_executable resolves it.
        real_root = os.path.realpath(root)
        installed = herdr is None or os.sep not in herdr or os.path.commonpath(
            (os.path.realpath(os.path.abspath(herdr)), real_root)
        ) != real_root
        assert not (installed and refusal is None), (
            f"{name} would run {herdr!r} for {arguments!r}, and the guard admitted it"
        )


@pytest.mark.parametrize(("parser_name", "arguments"), (
    ("agentctl", ("send", "worker", "--", "--herdr-bin=<HERDR>")),
    ("agentctl", ("--", "start", "worker")),
    ("agentctl", ("start", "--", "--help")),
    ("agentctl", ("--herdr-bin", "<HERDR>", "--", "start", "worker", "--herdr-bin", "herdr")),
    ("herdr-agent", ("send", "--pane", "w1:p1", "--", "--herdr-bin=<HERDR>")),
    ("herdr-agent", ("--", "start", "--help")),
    ("herdr-agent", ("--pane", "w1:p1", "status")),
))
def test_the_parser_probes_include_invocations_that_select_the_installed_herdr(
    tmp_path: Path, parser_name: str, arguments: tuple[str, ...]
) -> None:
    """The probes above are not vacuous: these reach the installed Herdr unless refused."""
    herdr_agent = _cross_module("herdr_agent_differential")
    from agentctl import cli as agentctl_cli
    from agentctl import legacy_cli

    root = tmp_path / "case"
    root.mkdir()
    expanded = [value.replace("<HERDR>", str(root / "fake-herdr")) for value in arguments]
    parse = (
        agentctl_cli.parser().parse_args if parser_name == "agentctl"
        else legacy_cli._parser().parse_intermixed_args
    )
    assert arguments in _PARSER_PROBES
    assert _parsed_herdr(parse, expanded) == ("herdr", False)
    assert herdr_agent.herdr_refusal(root, expanded) is not None


class _ClientConstructed(Exception):
    """Raised in place of a production Herdr client, so the command stops before any Herdr use."""


@pytest.mark.parametrize("suffix", (("--herdr-bin", "herdr"), ("--herdr-bin=/usr/local/bin/herdr",)))
def test_a_root_terminator_does_not_hide_a_subcommand_herdr_from_the_guard(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, suffix: tuple[str, ...]
) -> None:
    """`--herdr-bin FIXTURE -- start worker --herdr-bin herdr` reaches the client as `herdr`.

    The CLI runs for real up to the point where it constructs its HerdrClient, which is replaced
    by a recorder that raises. Process creation is blocked as well, so nothing can execute.
    """
    import contextlib
    import io
    import subprocess

    herdr_agent = _cross_module("herdr_agent_differential")
    from agentctl import cli as agentctl_cli

    root = tmp_path / "case"
    root.mkdir()
    fixture = root / "fake-herdr"
    _executable_file(fixture)
    constructed: list[str] = []

    def recording_client(*, herdr_bin: str = "herdr", **_: object) -> object:
        constructed.append(herdr_bin)
        raise _ClientConstructed

    def no_process(*_: object, **__: object) -> object:
        raise AssertionError("the CLI tried to start a process")

    monkeypatch.setattr(agentctl_cli, "HerdrClient", recording_client)
    monkeypatch.setattr(subprocess, "Popen", no_process)
    arguments = [
        "--registry", str(root / "registry"), "--herdr-bin", str(fixture),
        "--", "start", "worker", *suffix,
    ]
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        with contextlib.suppress(_ClientConstructed):
            agentctl_cli.main(arguments)

    assert constructed == [suffix[-1].removeprefix("--herdr-bin=")]
    assert not herdr_agent._names_case_file(root, constructed[0])
    assert herdr_agent.herdr_refusal(root, arguments) is not None


def test_the_chat_bridge_ignores_a_fixture_herdr_the_guard_would_otherwise_see(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Why `chat` is refused outright: its `--herdr-bin`-looking token is a harness argument.

    The bridge's launch builds a default HerdrClient, which runs the installed Herdr (see
    test_the_production_client_ignores_path_for_the_default_herdr), so a fixture path in the
    argument list redirects nothing. The launch itself is replaced, so no client is built here.
    """
    herdr_agent = _cross_module("herdr_agent_differential")
    from agentctl import chat as agentctl_chat
    from agentctl import cli as agentctl_cli

    root = tmp_path / "case"
    root.mkdir()
    fixture = f"--herdr-bin={root / 'fake-herdr'} x"
    launched: list[tuple[str, ...]] = []

    def record_launch(state: Path, config_path: Path, *, harness_args: tuple[str, ...] | list[str],
                      **_: object) -> int:
        launched.append(tuple(harness_args))
        return 0

    monkeypatch.setattr(agentctl_chat, "_launch_here", record_launch)
    arguments = [
        "--registry", str(root / "registry"), "chat", "launch",
        "--config", str(root / "chat.json"), "--harness-arg", fixture,
    ]

    assert agentctl_cli.main(arguments) == 0
    assert launched == [(fixture,)]
    assert herdr_agent.herdr_refusal(root, arguments) is not None


def test_the_unfixed_retirement_goal_query_reached_the_host_codex(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The case as it stood: a goal query with no goal command uses `codex` from PATH."""
    agentctl = _cross_module("agentctl_differential")
    ran = _ambient_tool(tmp_path, monkeypatch, "codex")
    lock_root = tmp_path / "target-locks"
    edition = _python_agentctl(lock_root)
    harness = agentctl.Harness(tmp_path / "cross", edition, edition)
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
    # `start` held its target lock in the private root, not the account's.
    assert _private_locks_taken(lock_root)
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
    lock_root = tmp_path / "target-locks"
    edition = _python_agentctl(lock_root)
    harness = agentctl.Harness(tmp_path / "cross", edition, edition)
    try:
        report = agentctl.Report()
        agentctl._workspace_retirement(harness, report)
        harness.require_no_host_cli(report)
    finally:
        harness.close()

    assert not ran.exists()
    assert _private_locks_taken(lock_root)
    assert harness.host_cli_calls() == []
    assert report.failures == []
    # start, status, attach, goal-query, goal-query-native, wait, stop, and the guard.
    assert report.checks == 8
