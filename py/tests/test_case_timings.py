"""One normalized per-case timing schema across every validation runner.

Gate-level profile rows say how long a whole gate took. They cannot say which
case inside it was slow, and each runner that does know its cases (pytest,
libtest, Playwright, node:test, the cross differential, the package and
example harnesses) reports them in a different shape. ``scripts/case_timings.py``
is the single schema those shapes are normalized into, plus the verifier that
proves a run's cases match its collected inventory exactly — without counting
a nested alternate-environment rerun of the same case twice.

These tests pin the schema and every adapter with small fixtures, then prove
two live emissions (the pytest shard runner and a real libtest binary) so the
fixtures cannot drift from what the runners actually print.
"""

from __future__ import annotations

import importlib.util
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path
from types import ModuleType

import pytest


REPO_ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = REPO_ROOT / "scripts"
sys.path.insert(0, str(SCRIPTS))

import case_timings  # noqa: E402


def _case(
    case_id: str, outcome: str = "pass", duration: float = 0.5
) -> case_timings.CaseRecord:
    return case_timings.CaseRecord(
        runner="pytest",
        gate="python.dagrun.shard-0",
        case_id=case_id,
        outcome=outcome,
        duration_s=duration,
    )


def test_write_and_verify_exact_inventory(tmp_path: Path) -> None:
    inventory = ["tests/test_a.py::test_one", "tests/test_a.py::test_two"]
    path = case_timings.write_records(
        tmp_path,
        runner="pytest",
        gate="python.dagrun.shard-0",
        inventory=inventory,
        cases=[_case(inventory[0]), _case(inventory[1], duration=1.25)],
    )
    record = json.loads(path.read_text(encoding="utf-8"))
    assert record["schema"] == case_timings.SCHEMA
    assert record["runner"] == "pytest"
    assert record["gate"] == "python.dagrun.shard-0"

    report = case_timings.verify_directory(tmp_path)
    assert report.problems == []
    assert report.primary_count == 2
    assert report.nested_count == 0
    lines = case_timings.render_report(report, slowest=1)
    assert any("2 primary case(s)" in line for line in lines)
    assert any("test_two" in line for line in lines)
    assert not any("test_one" in line and "slowest" in line for line in lines)


@pytest.mark.parametrize(
    ("inventory", "cases", "fragment"),
    [
        (["a", "b"], [_case("a")], "missing"),
        (["a"], [_case("a"), _case("surprise")], "not in its inventory"),
        (["a"], [_case("a"), _case("a")], "duplicate"),
        (["a", "a"], [_case("a")], "duplicate inventory"),
    ],
)
def test_verify_rejects_inventory_mismatch(
    tmp_path: Path,
    inventory: list[str],
    cases: list[case_timings.CaseRecord],
    fragment: str,
) -> None:
    path = tmp_path / "pytest-gate-1.json"
    path.write_text(
        json.dumps(
            {
                "schema": case_timings.SCHEMA,
                "runner": "pytest",
                "gate": "python.dagrun.shard-0",
                "nested": False,
                "inventory": inventory,
                "cases": [
                    {
                        "case_id": case.case_id,
                        "outcome": case.outcome,
                        "duration_s": case.duration_s,
                    }
                    for case in cases
                ],
            }
        ),
        encoding="utf-8",
    )
    report = case_timings.verify_directory(tmp_path)
    assert any(fragment in problem for problem in report.problems), report.problems


def test_nested_and_alternate_env_runs_are_not_double_counted(tmp_path: Path) -> None:
    case_timings.write_records(
        tmp_path,
        runner="pytest",
        gate="python.dagrun.shard-0",
        inventory=["tests/test_a.py::test_one"],
        cases=[_case("tests/test_a.py::test_one")],
    )
    # The same case rerun by a nested alternate-environment leg (for example a
    # namespaced retry of a host-visible case) is evidence, not a second case.
    case_timings.write_records(
        tmp_path,
        runner="pytest",
        gate="python.dagrun.shard-0",
        inventory=["tests/test_a.py::test_one"],
        cases=[_case("tests/test_a.py::test_one", duration=2.0)],
        nested=True,
    )
    report = case_timings.verify_directory(tmp_path)
    assert report.problems == []
    assert report.primary_count == 1
    assert report.nested_count == 1


def test_malformed_and_oversize_records_are_problems_not_crashes(tmp_path: Path) -> None:
    (tmp_path / "broken.json").write_text("{not json", encoding="utf-8")
    (tmp_path / "huge.json").write_bytes(b"x" * (case_timings.MAX_FILE_BYTES + 1))
    report = case_timings.verify_directory(tmp_path)
    assert any("broken.json" in problem for problem in report.problems)
    assert any("huge.json" in problem for problem in report.problems)


JUNIT = """<?xml version="1.0"?>
<testsuites>
  <testsuite name="pytest" tests="4">
    <testcase classname="tests.test_a" name="test_one" time="0.25">
      <properties><property name="nodeid" value="tests/test_a.py::test_one"/></properties>
    </testcase>
    <testcase classname="tests.test_a" name="test_two" time="1.5">
      <failure message="boom">trace</failure>
    </testcase>
    <testcase classname="tests.test_a" name="test_three" time="0.0">
      <skipped message="no"/>
    </testcase>
    <testcase classname="tests.test_a" name="test_four" time="0.1">
      <error message="setup"/>
    </testcase>
  </testsuite>
</testsuites>
"""


def test_junit_parser_prefers_nodeid_and_normalizes_outcomes() -> None:
    cases = case_timings.cases_from_junit(JUNIT, runner="pytest", gate="g")
    by_id = {case.case_id: case for case in cases}
    assert by_id["tests/test_a.py::test_one"].outcome == "pass"
    assert by_id["tests/test_a.py::test_one"].duration_s == 0.25
    assert by_id["tests.test_a::test_two"].outcome == "fail"
    assert by_id["tests.test_a::test_three"].outcome == "skip"
    assert by_id["tests.test_a::test_four"].outcome == "error"
    assert all(case.gate == "g" and case.runner == "pytest" for case in cases)


def test_libtest_json_events_get_arrival_durations() -> None:
    events: list[tuple[float, dict[str, object]]] = [
        (10.0, {"type": "suite", "event": "started", "test_count": 3}),
        (10.0, {"type": "test", "event": "started", "name": "alpha"}),
        (10.5, {"type": "test", "name": "alpha", "event": "ok"}),
        (10.5, {"type": "test", "event": "started", "name": "beta"}),
        (12.0, {"type": "test", "name": "beta", "event": "failed"}),
        (12.0, {"type": "test", "name": "gamma", "event": "ignored"}),
        (12.0, {"type": "suite", "event": "failed", "exec_time": 2.0}),
    ]
    cases = case_timings.cases_from_libtest_events(events, gate="rust.dagrun.test")
    by_id = {case.case_id: case for case in cases}
    assert by_id["alpha"].outcome == "pass"
    assert by_id["alpha"].duration_s == pytest.approx(0.5)
    assert by_id["beta"].outcome == "fail"
    assert by_id["beta"].duration_s == pytest.approx(1.5)
    assert by_id["gamma"].outcome == "skip"
    assert all(case.runner == "libtest" for case in cases)


def test_playwright_json_parser() -> None:
    payload = {
        "suites": [
            {
                "title": "timeline-interactions.spec.js",
                "file": "timeline-interactions.spec.js",
                "specs": [
                    {
                        "title": "pans",
                        "file": "timeline-interactions.spec.js",
                        "line": 12,
                        "tests": [
                            {
                                "results": [
                                    {"status": "passed", "duration": 321}
                                ]
                            }
                        ],
                    },
                    {
                        "title": "zooms",
                        "file": "timeline-interactions.spec.js",
                        "line": 40,
                        "tests": [
                            {"results": [{"status": "skipped", "duration": 0}]}
                        ],
                    },
                ],
                "suites": [],
            }
        ]
    }
    cases = case_timings.cases_from_playwright(payload, gate="timeline.browser.functional")
    by_id = {case.case_id: case for case in cases}
    assert by_id["timeline-interactions.spec.js:12 pans"].outcome == "pass"
    assert by_id["timeline-interactions.spec.js:12 pans"].duration_s == pytest.approx(0.321)
    assert by_id["timeline-interactions.spec.js:40 zooms"].outcome == "skip"
    assert all(case.runner == "playwright" for case in cases)


NODE_TAP = """TAP version 13
# Subtest: records every metric
ok 1 - records every metric
  ---
  duration_ms: 12.5
  ...
# Subtest: rejects a missing metric
not ok 2 - rejects a missing metric
  ---
  duration_ms: 3.25
  ...
ok 3 - later # SKIP no browser
  ---
  duration_ms: 0.5
  ...
1..3
"""


def test_node_tap_parser() -> None:
    cases = case_timings.cases_from_node_tap(NODE_TAP, gate="timeline.browser.benchmark-instrument")
    by_id = {case.case_id: case for case in cases}
    assert by_id["records every metric"].outcome == "pass"
    assert by_id["records every metric"].duration_s == pytest.approx(0.0125)
    assert by_id["rejects a missing metric"].outcome == "fail"
    assert by_id["later"].outcome == "skip"
    assert all(case.runner == "node-test" for case in cases)


def test_report_rendering_is_bounded(tmp_path: Path) -> None:
    inventory = [f"tests/test_a.py::test_{index}" for index in range(50)]
    case_timings.write_records(
        tmp_path,
        runner="pytest",
        gate="g",
        inventory=inventory,
        cases=[_case(case_id, duration=float(index)) for index, case_id in enumerate(inventory)],
    )
    report = case_timings.verify_directory(tmp_path)
    lines = case_timings.render_report(report, slowest=5)
    slowest_lines = [line for line in lines if "test_" in line]
    assert len(slowest_lines) == 5
    assert any("test_49" in line for line in slowest_lines)
    assert not any("test_0 " in line or line.rstrip().endswith("test_0") for line in slowest_lines)


def test_case_timings_self_test() -> None:
    completed = subprocess.run(
        [sys.executable, str(SCRIPTS / "case_timings.py"), "--self-test"],
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stderr
    assert "PASSED" in completed.stdout


def test_pytest_shard_runner_emits_case_timings(tmp_path: Path) -> None:
    test_file = tmp_path / "test_timed.py"
    test_file.write_text(
        "import pytest\n\n\ndef test_fast():\n    assert True\n\n\n"
        "@pytest.mark.skip(reason='fixture')\ndef test_skipped():\n    pass\n",
        encoding="utf-8",
    )
    timing_dir = tmp_path / "timings"
    environment = os.environ.copy()
    environment[case_timings.TIMING_DIR_ENV] = str(timing_dir)
    environment["DAGRUN_OUTER_RUN"] = "python.fixture.shard-0"
    completed = subprocess.run(
        [
            sys.executable,
            str(SCRIPTS / "run_pytest_shard.py"),
            "--shard",
            "0/1",
            "--",
            "-q",
            str(test_file),
        ],
        cwd=tmp_path,
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )
    assert completed.returncode == 0, completed.stdout + completed.stderr
    report = case_timings.verify_directory(timing_dir)
    assert report.problems == []
    assert report.primary_count == 2
    outcomes = {case.case_id: case.outcome for case in report.primary_cases}
    assert outcomes == {
        "test_timed.py::test_fast": "pass",
        "test_timed.py::test_skipped": "skip",
    }
    assert all(case.gate == "python.fixture.shard-0" for case in report.primary_cases)
    assert all(case.duration_s is not None and case.duration_s >= 0 for case in report.primary_cases)


def _differential() -> ModuleType:
    cross = str(REPO_ROOT / "cross")
    if cross not in sys.path:
        sys.path.insert(0, cross)
    spec = importlib.util.spec_from_file_location(
        "_cross_differential_case_timings", REPO_ROOT / "cross" / "differential.py"
    )
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def test_cross_report_emits_case_timings(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    differential = _differential()
    timing_dir = tmp_path / "timings"
    monkeypatch.setenv(case_timings.TIMING_DIR_ENV, str(timing_dir))
    monkeypatch.setenv("DAGRUN_OUTER_RUN", "cross.fixture.differential")
    monkeypatch.delenv("AGENT_UTILS_CROSS_COVERAGE_DIR", raising=False)

    report = differential.Report()
    report.ok("help-text")
    report.bad("json-bytes", "diverged")
    report.skip("boxed-leg", "boxing", "no box in this fixture")
    differential.record_coverage("fixture", "FAILED", report, frozenset())

    verified = case_timings.verify_directory(timing_dir)
    assert verified.problems == []
    outcomes = {case.case_id: case.outcome for case in verified.primary_cases}
    assert outcomes == {
        "fixture/help-text": "pass",
        "fixture/json-bytes": "fail",
        "fixture/boxed-leg": "skip",
    }
    assert all(case.runner == "cross" for case in verified.primary_cases)
    assert all(case.duration_s is not None and case.duration_s >= 0 for case in verified.primary_cases)


def test_validate_summary_reports_timing_problems(tmp_path: Path) -> None:
    spec = importlib.util.spec_from_file_location(
        "_validate_case_timings", SCRIPTS / "validate.py"
    )
    assert spec is not None and spec.loader is not None
    validate = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = validate
    spec.loader.exec_module(validate)

    case_timings.write_records(
        tmp_path,
        runner="pytest",
        gate="g",
        inventory=["a"],
        cases=[_case("a")],
    )
    lines, problems = validate.case_timing_report(tmp_path)
    assert problems == []
    assert any("case timings" in line for line in lines)

    (tmp_path / "broken.json").write_text("[]", encoding="utf-8")
    _lines, problems = validate.case_timing_report(tmp_path)
    assert problems


@pytest.mark.skipif(shutil.which("rustc") is None, reason="rustc is required")
def test_libtest_ingest_against_a_real_test_binary(tmp_path: Path) -> None:
    source = tmp_path / "timed.rs"
    source.write_text(
        "#[test] fn alpha() {}\n#[test] #[ignore] fn beta() {}\n",
        encoding="utf-8",
    )
    binary = tmp_path / "timed"
    subprocess.run(
        ["rustc", "--test", str(source), "-o", str(binary)],
        check=True,
        capture_output=True,
        text=True,
    )
    environment = os.environ.copy()
    environment["RUSTC_BOOTSTRAP"] = "1"
    raw = subprocess.run(
        [str(binary), "-Z", "unstable-options", "--format", "json"],
        env=environment,
        capture_output=True,
        text=True,
        check=False,
    )
    assert raw.returncode == 0, raw.stderr
    timing_dir = tmp_path / "timings"
    ingested = subprocess.run(
        [
            sys.executable,
            str(SCRIPTS / "case_timings.py"),
            "ingest-libtest-json",
            "--gate",
            "rust.fixture.test",
            "--out",
            str(timing_dir),
        ],
        input=raw.stdout,
        capture_output=True,
        text=True,
        check=False,
    )
    assert ingested.returncode == 0, ingested.stderr
    report = case_timings.verify_directory(timing_dir)
    assert report.problems == []
    outcomes = {case.case_id: case.outcome for case in report.primary_cases}
    assert outcomes == {"alpha": "pass", "beta": "skip"}
