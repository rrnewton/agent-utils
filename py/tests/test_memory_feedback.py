"""The profile-to-admission path must never learn a smaller cap from a censored sample.

A peak recorded while a ``memory.max`` was clamping proves only that the step used all it was
allowed. Deriving a cap from it re-derives the cap that produced it, so the workload is pinned
at whatever ceiling it was first given. These tests pin the three ways that is prevented: a
censored sample is a floor and never a central estimate, an unprovenanced sample is not
evidence at all, and with no uncensored evidence the authored hint survives untouched.
"""

from __future__ import annotations

import csv
from pathlib import Path

import pytest

from safe_ci_dag_runner.memory_feedback import (
    DEFAULT_MARGIN_PCT,
    Censoring,
    apply_memory_admissions,
    load_memory_admissions,
    memory_admission_from_rows,
    peak_observation_from_row,
)
from safe_ci_dag_runner.model import DagConfig, ResourceHint, Step

GIB = 1024**3


def _row(
    step: str = "g.build",
    *,
    peak: str = "1073741824",
    cap: str = "8589934592",
    reclaim: str = "0",
    oom: str = "0",
    high: str = "0",
    oom_invoked: str = "0",
    **extra: str,
) -> dict[str, str]:
    row = {
        "step": step,
        "peak_bytes": peak,
        "memory_max_bytes": cap,
        "memory_events_high": high,
        "memory_events_max": reclaim,
        "memory_events_oom": oom_invoked,
        "memory_events_oom_kill": oom,
    }
    row.update(extra)
    return row


def _step(tag: str, *, rss: int | None, hard: int | None = None) -> Step:
    group, job = tag.split(".", 1)
    return Step(
        group,
        job,
        "",
        "true",
        hint=ResourceHint(rss_baseline_bytes=rss, hard_mem_max_bytes=hard),
    )


# ---------------------------------------------------------------------------
# Classifying one recorded peak.
# ---------------------------------------------------------------------------


def test_a_peak_under_a_known_quiet_cap_is_a_real_observation() -> None:
    observed = peak_observation_from_row(_row(peak=str(2 * GIB), cap=str(8 * GIB)))

    assert observed.verdict is Censoring.UNCENSORED
    assert observed.peak_bytes == 2 * GIB
    assert observed.applied_cap_bytes == 8 * GIB


@pytest.mark.parametrize(
    ("why", "kwargs"),
    [
        ("peak reached the cap", {"peak": str(8 * GIB), "cap": str(8 * GIB)}),
        ("peak above a page-rounded cap", {"peak": str(8 * GIB), "cap": str(8 * GIB - 4096)}),
        ("held at the ceiling by reclaim", {"reclaim": "17"}),
        ("killed at the ceiling", {"oom": "2"}),
        ("throttled at a soft ceiling", {"high": "3"}),
        ("the oom killer was invoked without a recorded kill", {"oom_invoked": "1"}),
        ("cut short by the wall guard", {"timed_out": "true"}),
        ("cut short by the cpu guard", {"cpu_timed_out": "true"}),
    ],
)
def test_a_peak_that_met_a_ceiling_is_censored(why: str, kwargs: dict[str, str]) -> None:
    assert peak_observation_from_row(_row(**kwargs)).verdict is Censoring.CENSORED, why


def test_every_pressure_counter_the_writer_records_is_read() -> None:
    """#34 persists five ``memory.events`` counters. A reader that consults only some of them
    calls a throttled or OOM-invoked step comfortable, which is the exact failure this path
    exists to prevent.

    ``low`` is deliberately NOT censoring: it counts reclaim that breached a ``memory.low``
    PROTECTION, which does not bound the cgroup's own peak.
    """
    censoring = {"memory_events_high", "memory_events_max", "memory_events_oom",
                 "memory_events_oom_kill"}
    for column in censoring:
        row = _row()
        row[column] = "1"
        assert peak_observation_from_row(row).verdict is Censoring.CENSORED, column
    low = _row()
    low["memory_events_low"] = "9"
    assert peak_observation_from_row(low).verdict is Censoring.UNCENSORED


@pytest.mark.parametrize(
    ("why", "kwargs"),
    [
        ("no applied cap recorded", {"cap": ""}),
        ("no event counters recorded", {"reclaim": ""}),
        ("no peak recorded", {"peak": ""}),
        ("a cap cell that is neither max nor a number", {"cap": "unbounded"}),
    ],
)
def test_a_row_that_cannot_answer_is_unknown_not_uncensored(
    why: str, kwargs: dict[str, str]
) -> None:
    """Silence must not read as comfort. An unprovenanced row is excluded, never assumed safe."""
    assert peak_observation_from_row(_row(**kwargs)).verdict is Censoring.UNKNOWN, why


def test_an_unbounded_step_that_never_reclaimed_is_a_real_observation() -> None:
    """``max`` is a KNOWN ceiling of none, so its peak is genuine demand."""
    observed = peak_observation_from_row(_row(peak=str(3 * GIB), cap="max"))

    assert observed.cap_known
    assert observed.cap_unbounded
    assert observed.verdict is Censoring.UNCENSORED


def test_the_legacy_oom_column_still_censors_when_the_event_counter_is_absent() -> None:
    """A store written before the event counters existed still records ``oom_kills``."""
    row = _row(peak=str(8 * GIB), oom="")
    row["oom_kills"] = "1"

    assert peak_observation_from_row(row).verdict is Censoring.CENSORED


# ---------------------------------------------------------------------------
# Aggregating into an admission.
# ---------------------------------------------------------------------------


def test_enough_quiet_samples_produce_a_percentile_estimate_with_margin() -> None:
    """The expected bytes are written out LITERALLY. Recomputing them from
    ``DEFAULT_MARGIN_PCT`` would pin the arithmetic to whatever the constant happens to be, so
    changing 20 to 0 would leave the test green while every learned cap lost its headroom."""
    rows = [_row(peak=str(2 * GIB)) for _ in range(6)]

    admission = memory_admission_from_rows(rows)["g.build"]

    assert admission.source == "profile"
    assert admission.uncensored_samples == 6
    assert admission.margin_pct == 20
    # Every sample is exactly 2147483648 B, so the 9/10 percentile is 2147483648 B and the whole
    # difference is the 20% margin: 2147483648 + 429496729 = 2576980377.
    assert admission.rss_baseline_bytes == 2576980377
    assert not admission.censoring_excluded_samples()


def test_the_default_margin_is_twenty_percent() -> None:
    """Named once, here, so a change to the shipped headroom has to be made deliberately."""
    assert DEFAULT_MARGIN_PCT == 20


def test_the_estimate_never_falls_below_the_largest_censored_peak() -> None:
    """A censored peak proves demand was AT LEAST that large, so it can only raise the cap.

    Five quiet 1 GiB samples would otherwise justify 1.2 GiB. One sample that hit a 8 GiB
    ceiling says the step has wanted 8 GiB at least once; the recommendation must not be a
    number that step has already exceeded.
    """
    rows = [_row(peak=str(GIB), cap=str(8 * GIB)) for _ in range(5)]
    rows.append(_row(peak=str(8 * GIB), cap=str(8 * GIB), oom="1"))

    admission = memory_admission_from_rows(rows)["g.build"]

    assert admission.source == "profile"
    assert admission.uncensored_samples == 5
    assert admission.censored_samples == 1
    assert admission.censored_floor_bytes == 8 * GIB
    assert admission.rss_baseline_bytes == 8 * GIB
    assert admission.censoring_excluded_samples()


def test_the_estimate_never_falls_below_the_largest_uncensored_peak() -> None:
    """The percentile can sit under the maximum; the maximum is a thing that really happened."""
    rows = [_row(peak=str(GIB)) for _ in range(9)]
    rows.append(_row(peak=str(6 * GIB)))

    admission = memory_admission_from_rows(rows)["g.build"]

    assert admission.observed_peak_bytes == 6 * GIB
    assert admission.rss_baseline_bytes is not None
    assert admission.rss_baseline_bytes >= 6 * GIB


def test_every_peak_censored_keeps_the_static_hint_and_says_so() -> None:
    """This is the DeepScry case: a step whose whole history sits on its own ceiling."""
    rows = [_row(peak=str(8 * GIB), cap=str(8 * GIB)) for _ in range(30)]

    admission = memory_admission_from_rows(rows)["g.build"]

    assert admission.source == "hint"
    assert admission.rss_baseline_bytes is None
    assert admission.censored_samples == 30
    assert admission.uncensored_samples == 0
    assert "censored by its applied cap" in admission.reason


def test_unprovenanced_samples_alone_keep_the_static_hint_and_say_so() -> None:
    """A store written before the applied cap was recorded cannot license a smaller cap."""
    rows = [_row(peak=str(GIB), cap="", reclaim="") for _ in range(30)]

    admission = memory_admission_from_rows(rows)["g.build"]

    assert admission.source == "hint"
    assert admission.rss_baseline_bytes is None
    assert admission.unknown_samples == 30
    assert "no applied-cap or event provenance" in admission.reason


def test_too_few_uncensored_samples_keeps_the_static_hint_and_says_how_many() -> None:
    rows = [_row(peak=str(GIB)) for _ in range(2)]

    admission = memory_admission_from_rows(rows, min_uncensored_samples=5)["g.build"]

    assert admission.source == "hint"
    assert admission.rss_baseline_bytes is None
    assert "only 2 uncensored sample(s); 5 required" in admission.reason


def test_samples_from_another_revision_are_excluded_and_counted() -> None:
    """A cap learned across a change that moved memory is a cap learned from two workloads."""
    rows = [_row(peak=str(GIB), profile_base_sha="new") for _ in range(5)]
    rows += [_row(peak=str(64 * GIB), profile_base_sha="old") for _ in range(20)]

    admission = memory_admission_from_rows(rows, profile_base_sha="new")["g.build"]

    assert admission.source == "profile"
    assert admission.uncensored_samples == 5
    assert admission.rss_baseline_bytes is not None
    # The 20 huge samples belong to the other revision and must not raise this cap.
    assert admission.rss_baseline_bytes < 2 * GIB
    assert "20 sample(s) excluded as recorded against another source revision" in admission.reason


def test_a_step_with_no_rows_at_all_is_absent_rather_than_zero() -> None:
    assert memory_admission_from_rows([]) == {}


# ---------------------------------------------------------------------------
# Applying an admission to a DAG.
# ---------------------------------------------------------------------------


def _cfg(*steps: Step) -> DagConfig:
    return DagConfig(steps=tuple(steps))


def test_only_a_profile_backed_admission_replaces_the_authored_hint() -> None:
    rows = [_row("g.learned", peak=str(2 * GIB)) for _ in range(6)]
    rows += [_row("g.censored", peak=str(8 * GIB), cap=str(8 * GIB)) for _ in range(6)]
    admissions = memory_admission_from_rows(rows)
    cfg = _cfg(_step("g.learned", rss=99), _step("g.censored", rss=99), _step("g.absent", rss=99))

    applied = apply_memory_admissions(cfg, admissions)

    by_tag = {step.tag: step for step in applied.steps}
    assert by_tag["g.learned"].hint.rss_baseline_bytes == admissions["g.learned"].rss_baseline_bytes
    assert by_tag["g.censored"].hint.rss_baseline_bytes == 99
    assert by_tag["g.absent"].hint.rss_baseline_bytes == 99


def test_an_explicit_hard_cap_is_never_rewritten_by_the_profile_path() -> None:
    """A hard cap is an instruction. Learning may move the baseline under it, never the cap."""
    rows = [_row("g.learned", peak=str(2 * GIB)) for _ in range(6)]
    cfg = _cfg(_step("g.learned", rss=99, hard=5 * GIB))

    applied = apply_memory_admissions(cfg, memory_admission_from_rows(rows))

    step = applied.steps[0]
    assert step.hint.hard_mem_max_bytes == 5 * GIB
    assert step.hint.rss_baseline_bytes != 99


def test_applying_admissions_carries_every_other_step_field(tmp_path: Path) -> None:
    """The clone-and-override rule: a newly added Step field must not silently reset here."""
    rows = [_row("g.learned", peak=str(2 * GIB)) for _ in range(6)]
    original = Step(
        "g",
        "learned",
        "a description",
        "make build",
        deps=[],
        timeout=1234,
        cpu_timeout=567,
        hint=ResourceHint(rss_baseline_bytes=99, est_duration_s=42.0, preferred_inner_jobs=8),
    )

    applied = apply_memory_admissions(_cfg(original), memory_admission_from_rows(rows))

    step = applied.steps[0]
    assert step.timeout == 1234
    assert step.cpu_timeout == 567
    assert step.desc == "a description"
    assert step.hint.est_duration_s == 42.0
    assert step.hint.preferred_inner_jobs == 8


# ---------------------------------------------------------------------------
# Reading a real store.
# ---------------------------------------------------------------------------


def _write_store(store: Path, machine: str, container: str, rows: list[dict[str, str]]) -> None:
    path = store / f"step_profiles_{machine}_{container}.csv"
    fieldnames = sorted({key for row in rows for key in row})
    with path.open("w", newline="", encoding="utf-8") as handle:
        writer = csv.DictWriter(handle, fieldnames=fieldnames, lineterminator="\n")
        writer.writeheader()
        writer.writerows(rows)


def test_a_store_is_read_for_the_named_identity_only(tmp_path: Path) -> None:
    """A cap learned on one container class does not transfer to another."""
    _write_store(tmp_path, "hostA", "affinity8_cpu-max-max", [_row(peak=str(GIB)) for _ in range(6)])

    mine = load_memory_admissions(tmp_path, "hostA", "affinity8_cpu-max-max")
    theirs = load_memory_admissions(tmp_path, "hostA", "affinity64_cpu-max-max")

    assert mine["g.build"].source == "profile"
    assert theirs == {}


def test_a_missing_store_means_keep_every_hint_rather_than_no_memory(tmp_path: Path) -> None:
    admissions = load_memory_admissions(tmp_path, "hostA", "affinity8_cpu-max-max")
    cfg = _cfg(_step("g.build", rss=7 * GIB))

    assert admissions == {}
    assert apply_memory_admissions(cfg, admissions).steps[0].hint.rss_baseline_bytes == 7 * GIB


# ---------------------------------------------------------------------------
# The opt-in CLI surface.
# ---------------------------------------------------------------------------


def _run_cli(args: list[str], env: dict[str, str]) -> tuple[int, str, str]:
    import os
    import subprocess
    import sys

    child = os.environ.copy()
    child.update(env)
    child["PYTHONPATH"] = str(Path(__file__).resolve().parents[1])
    proc = subprocess.run(
        [sys.executable, "-m", "safe_ci_dag_runner", *args],
        text=True,
        capture_output=True,
        check=False,
        env=child,
    )
    return proc.returncode, proc.stdout, proc.stderr


def test_the_flag_is_off_by_default_and_reports_every_decision_when_on(tmp_path: Path) -> None:
    """The report must name the steps that did NOT move, or silence reads as agreement."""
    machine, container = "synthhost", "affinity4_cpu-max-max"
    store = tmp_path / "store"
    store.mkdir()
    rows = [_row("g.learned", peak=str(2 * GIB)) for _ in range(6)]
    rows += [_row("g.pinned", peak=str(8 * GIB), cap=str(8 * GIB)) for _ in range(6)]
    _write_store(store, machine, container, rows)
    dag = tmp_path / "dag.json"
    dag.write_text(
        '{"steps": ['
        '{"group": "g", "job": "learned", "cmd": "true", "deps": []},'
        '{"group": "g", "job": "pinned", "cmd": "true", "deps": []}]}',
        encoding="utf-8",
    )
    env = {
        "SAFE_CI_DAG_RUNNER_MACHINE_ID": machine,
        "SAFE_CI_DAG_RUNNER_CONTAINER_CLASS": container,
    }
    common = ["run", "--dag", str(dag), "--perf-dir", str(store), "--no-profile",
              "--unsafe-no-cgroups", "-q"]

    silent_rc, _out, silent_err = _run_cli(common, env)
    loud_rc, _out2, loud_err = _run_cli([*common, "--profile-memory-feedback"], env)

    assert silent_rc == 0, silent_err
    assert loud_rc == 0, loud_err
    assert "--profile-memory-feedback" not in silent_err
    assert "g.learned: rss_baseline_bytes=" in loud_err
    assert "g.pinned: keeping the authored hint" in loud_err
    assert "censored by its applied cap" in loud_err


def test_the_derived_baseline_reaches_max_mem_admission(tmp_path: Path) -> None:
    """The point of the whole path: the number it derives must change what admission does.

    One step, so the modeled worst case is that step's own cap and is independent of this host's
    CPU count. Its six uncensored samples peaked at 21474836480 B under a 107374182400 B cap.

    * Without the flag, the ordinary feedback takes the peak at face value: 21474836480 B, and
      ``--max-mem`` models a worst case of 26843545600 B.
    * With it, the censoring-aware estimate adds the 20% margin (25769803776 B) and the modeled
      worst case rises to 32212254720 B.

    Both numbers are named literally. A wiring change that applied the estimate AFTER admission
    had already read the config would leave the second equal to the first.
    """
    machine, container = "synthhost", "affinity4_cpu-max-max"
    store = tmp_path / "store"
    store.mkdir()
    _write_store(store, machine, container,
                 [_row("g.learned", peak="21474836480", cap="107374182400") for _ in range(6)])
    dag = tmp_path / "dag.json"
    dag.write_text(
        '{"steps": [{"group": "g", "job": "learned", "cmd": "true", "deps": [],'
        ' "hint": {"rss_baseline_bytes": 104857600}}]}',
        encoding="utf-8",
    )
    env = {
        "SAFE_CI_DAG_RUNNER_MACHINE_ID": machine,
        "SAFE_CI_DAG_RUNNER_CONTAINER_CLASS": container,
    }
    common = ["run", "--dag", str(dag), "--perf-dir", str(store), "--no-profile",
              "--unsafe-no-cgroups", "-q", "--max-mem", "400G"]

    off_rc, _o1, off_err = _run_cli(common, env)
    on_rc, _o2, on_err = _run_cli([*common, "--profile-memory-feedback"], env)

    assert off_rc == 0, off_err
    assert on_rc == 0, on_err
    assert "worst-case 26843545600 bytes" in off_err, off_err
    assert "rss_baseline_bytes=25769803776" in on_err, on_err
    assert "worst-case 32212254720 bytes" in on_err, on_err


def test_asking_for_memory_feedback_with_reading_disabled_says_it_is_inert(
    tmp_path: Path,
) -> None:
    """``--no-profile-feedback`` turns the store reader off, which makes
    ``--profile-memory-feedback`` a no-op. Doing nothing quietly is the worst answer available:
    the caller asked for a learned cap by name and would run with the authored one believing
    otherwise. Say it, and keep exit 0 — the combination is legal, just empty.
    """
    machine, container = "synthhost", "affinity4_cpu-max-max"
    store = tmp_path / "store"
    store.mkdir()
    _write_store(store, machine, container, [_row("g.learned", peak=str(2 * GIB))
                                             for _ in range(6)])
    dag = tmp_path / "dag.json"
    dag.write_text(
        '{"steps": [{"group": "g", "job": "learned", "cmd": "true", "deps": []}]}',
        encoding="utf-8",
    )
    env = {
        "SAFE_CI_DAG_RUNNER_MACHINE_ID": machine,
        "SAFE_CI_DAG_RUNNER_CONTAINER_CLASS": container,
    }
    args = ["run", "--dag", str(dag), "--perf-dir", str(store), "--no-profile",
            "--unsafe-no-cgroups", "-q", "--profile-memory-feedback", "--no-profile-feedback"]

    rc, _out, err = _run_cli(args, env)

    assert rc == 0, err
    assert "--no-profile-feedback" in err
    assert "no estimate is derived" in err
    # And it really did not derive one.
    assert "rss_baseline_bytes=" not in err
