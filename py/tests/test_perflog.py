"""Tests for the CSV-backed metrics sink (perflog.CsvMetricsSink).

Guards the 0.1 fix that made ``run_dag(cfg, metrics=CsvMetricsSink(dir))`` actually write
its per-step and whole-run CSVs instead of raising: the scheduler's per-step row keys did
not match the writer's fieldnames (a ``ValueError``) and the whole-run appender opened a
not-yet-existent file for reading (a ``FileNotFoundError``).
"""

from __future__ import annotations

import csv
import tempfile
from pathlib import Path

from safe_ci_dag_runner import DagConfig, Step, run_dag
from safe_ci_dag_runner.perflog import CsvMetricsSink, machine_id

# The standard per-step columns the scheduler always emits (via the stamped run context and
# the row it builds in _run_step). Dynamic ``cpu.*`` columns may be appended after these.
_EXPECTED_STEP_COLUMNS = {
    "timestamp", "machine_id", "container_class", "git_sha", "outer_jobs",
    "profile_base_sha", "enforcement_kind", "runner_name",
    "step", "classification", "inner_jobs", "elapsed_s", "returncode", "ok",
    "timed_out", "oom_kills", "peak_bytes", "thread_peak",
}


def _tiny_dag() -> DagConfig:
    return DagConfig(
        steps=(
            Step("build", "app", "compile", "true"),
            Step("test", "unit", "unit tests", "true", deps=["build.app"]),
            Step("lint", "fmt", "format check", "true"),
        )
    )


def test_csv_metrics_sink_writes_per_step_and_whole_run() -> None:
    """A run with a CsvMetricsSink writes a non-empty per-step CSV (one row per step, with
    the expected columns) plus a whole-run CSV, and raises nothing."""
    with tempfile.TemporaryDirectory() as d:
        sink = CsvMetricsSink(d, git_sha="deadbeef")
        result = run_dag(_tiny_dag(), jobs=4, metrics=sink, verbosity=0)
        assert result.ok

        step_csvs = list(Path(d).glob("step_profiles_*.csv"))
        assert len(step_csvs) == 1, f"expected exactly one per-step CSV, got {step_csvs}"
        step_csv = step_csvs[0]
        assert step_csv.stat().st_size > 0

        with step_csv.open(newline="") as f:
            reader = csv.DictReader(f)
            header = set(reader.fieldnames or [])
            rows = list(reader)
        assert _EXPECTED_STEP_COLUMNS <= header, (
            f"missing columns: {_EXPECTED_STEP_COLUMNS - header}"
        )
        assert len(rows) == 3  # one row per step
        assert {row["step"] for row in rows} == {"build.app", "test.unit", "lint.fmt"}
        assert all(row["git_sha"] == "deadbeef" for row in rows)
        assert all(row["outer_jobs"] == "4" for row in rows)

        whole_run_csv = Path(d) / f"{machine_id()}.csv"
        assert whole_run_csv.exists() and whole_run_csv.stat().st_size > 0
        with whole_run_csv.open(newline="") as f:
            whole_rows = list(csv.DictReader(f))
        assert len(whole_rows) == 1
        assert whole_rows[0]["result"] == "pass"
        assert whole_rows[0]["n_steps"] == "3"

        # The flock sidecar must not be left behind as stray output.
        assert not list(Path(d).glob("*.lock")), "a stray *.lock file was left in --perf-dir"


def test_csv_metrics_sink_appends_across_runs() -> None:
    """A second run into the same directory appends (does not clobber): the per-step CSV
    accumulates rows and the header stays intact."""
    with tempfile.TemporaryDirectory() as d:
        for _ in range(2):
            run_dag(
                _tiny_dag(),
                jobs=2,
                metrics=CsvMetricsSink(d, git_sha="cafef00d"),
                verbosity=0,
            )
        step_csv = next(iter(Path(d).glob("step_profiles_*.csv")))
        with step_csv.open(newline="") as f:
            rows = list(csv.DictReader(f))
        assert len(rows) == 6  # 3 steps x 2 runs, single header
