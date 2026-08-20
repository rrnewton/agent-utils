"""The terminal step record must carry what the step actually consumed.

``step_end`` exists so the journal alone can answer "what was this run doing" without the
end-of-run profile rows, which a hard kill destroys.  It said how long the step took and whether
it passed, but not what it consumed nor what it was allowed to consume — so the one record
guaranteed to survive a kill could not be used to judge a budget.

The cgroup CPU counters are already read before ``cleanup()`` removes the step's cgroup, so
journalling them costs nothing.  A counter the kernel does not publish stays ABSENT rather than
becoming a measured zero.
"""

from __future__ import annotations

import json
import os
from pathlib import Path

from safe_ci_dag_runner import DagConfig, Runner, Step
from safe_ci_dag_runner.attribution import LOG_DIR_ENV
from safe_ci_dag_runner.cgroup import NoopCgroups
from safe_ci_dag_runner.model import DEFAULT_SMALL_CPU_TIMEOUT
from safe_ci_dag_runner.scheduler import _cpu_journal_fields


class _FullCounters(NoopCgroups):
    enabled = True

    def cpu_stats(self, tag: str) -> dict[str, int]:
        return {
            "usage_usec": 259_926_893,
            "nr_throttled": 994,
            "throttled_usec": 431_942_000,
            "user_usec": 1,
        }


class _PartialCounters(NoopCgroups):
    enabled = True

    def cpu_stats(self, tag: str) -> dict[str, int]:
        return {"usage_usec": 7}


def _step_end(
    log_dir: Path,
    cgroups: NoopCgroups,
    cpu_timeout: int,
    default_cpu_timeout: int = DEFAULT_SMALL_CPU_TIMEOUT,
) -> dict[str, str]:
    cfg = DagConfig(
        steps=(Step("g", "quick", "does a little", "true", timeout=45, cpu_timeout=cpu_timeout),),
        default_step_cpu_timeout=default_cpu_timeout,
    )
    previous = os.environ.get(LOG_DIR_ENV)
    os.environ[LOG_DIR_ENV] = str(log_dir)
    try:
        Runner(cfg, max_steps=1, max_cpus=1, cgroups=cgroups).run()
    finally:
        if previous is None:
            os.environ.pop(LOG_DIR_ENV, None)
        else:
            os.environ[LOG_DIR_ENV] = previous

    records = [
        json.loads(line)
        for line in (log_dir / "journal.jsonl").read_text().splitlines()
        if line.strip()
    ]
    ends = [r for r in records if r.get("event") == "step_end"]
    assert len(ends) == 1, f"expected exactly one step_end record, got {ends}"
    record: dict[str, str] = ends[0]
    return record


def test_absent_counters_stay_absent_and_present_ones_keep_their_units() -> None:
    assert _cpu_journal_fields(None) == []
    assert _cpu_journal_fields(
        {"usage_usec": 259_926_893, "nr_throttled": 994, "throttled_usec": 431_942_000}
    ) == [
        ("cpu_usage_usec", "259926893"),
        ("cpu_nr_throttled", "994"),
        ("cpu_throttled_usec", "431942000"),
    ]
    # A kernel that publishes only some counters contributes only those. Inventing the rest as 0
    # would put a measurement in the record that was never measured.
    assert _cpu_journal_fields({"usage_usec": 7}) == [("cpu_usage_usec", "7")]


def test_a_boxed_step_journals_what_it_consumed_and_what_it_was_allowed(
    tmp_path: Path,
) -> None:
    record = _step_end(tmp_path / "full", _FullCounters(), cpu_timeout=30)

    assert record["cpu_usage_usec"] == "259926893"
    assert record["cpu_nr_throttled"] == "994"
    assert record["cpu_throttled_usec"] == "431942000"

    # Both ceilings, each named for the quantity it bounds. Without them the consumption figures
    # above are unjudgeable: a number with no bound beside it is not evidence about a budget.
    assert record["cpu_limit_s"] == "30"
    assert record["wall_limit_s"] == "45"

    # The step's own duration keeps its unit in its name, like everything else beside a limit.
    assert float(record["wall_elapsed_s"]) >= 0.0
    assert "elapsed_s" not in record


def test_a_partially_reporting_kernel_contributes_only_what_it_reported(
    tmp_path: Path,
) -> None:
    record = _step_end(tmp_path / "partial", _PartialCounters(), cpu_timeout=30)
    assert record["cpu_usage_usec"] == "7"
    assert "cpu_nr_throttled" not in record
    assert "cpu_throttled_usec" not in record


def test_an_unboxed_step_journals_no_cpu_counters_at_all(tmp_path: Path) -> None:
    # Unboxed there are no cgroup counters to read. Absent is the honest record; zeroes would be
    # a claim that the step consumed nothing.
    record = _step_end(tmp_path / "unboxed", NoopCgroups(), cpu_timeout=0)
    assert "cpu_usage_usec" not in record
    # The budget recorded is the one actually IN FORCE, so a step that declares none still shows
    # the DAG's small default rather than looking unbounded.
    assert record["cpu_limit_s"] == str(DEFAULT_SMALL_CPU_TIMEOUT)
    assert record["wall_limit_s"] == "45"


def test_a_genuinely_disabled_cpu_budget_is_absent_not_zero(tmp_path: Path) -> None:
    record = _step_end(
        tmp_path / "nobudget", NoopCgroups(), cpu_timeout=0, default_cpu_timeout=0
    )
    # No budget was in force at all. A `cpu_limit_s` of 0 would read as "bounded at zero seconds",
    # which is the opposite of unbounded.
    assert "cpu_limit_s" not in record
