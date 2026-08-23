"""Profile rows must expose run overlap and the cap a peak was measured under.

A ``peak_bytes`` recorded while a ``memory.max`` was clamping is a CENSORED observation: it
proves the step used everything it was allowed, not what it wanted. A reader that treats such
a sample as an observed maximum under-estimates the workload permanently, and before these
columns existed nothing in the row could tell the two apart. These tests pin the distinctions
end to end, through a real scheduler run and a CSV round trip.
"""

from __future__ import annotations

import contextlib
import csv
import io
from collections.abc import Mapping, Sequence
from pathlib import Path

from dagrun.cgroup import Cgroups, NoopCgroups, _sanitize
from dagrun.cli import main
from dagrun.model import DagConfig, ResourceHint, Step
from dagrun.perflog import CsvMetricsSink, new_run_id
from dagrun.protocols import CgroupManager
from dagrun.scheduler import run_dag


def _step(job: str, cmd: str, *, deps: Sequence[str] = ()) -> Step:
    return Step(
        "g",
        job,
        "",
        cmd,
        deps=list(deps),
        hint=ResourceHint(),
    )


class _PlantedCgroups:
    """A cgroup manager that reports planted per-step memory measurements.

    ``enabled`` is ``True`` so the scheduler takes exactly the boxed measurement path it takes
    on a real cgroup-v2 host, while the numbers come from the test rather than the kernel.
    """

    enabled: bool = True

    def __init__(self, planted: Mapping[str, tuple[int, str, Mapping[str, int]]]) -> None:
        #: tag -> (peak_bytes, applied memory.max verbatim, memory.events counters)
        self.planted = dict(planted)

    def prepare_command(
        self,
        tag: str,
        cmd: str,
        mem_max: int | None = None,
        cpu_count: int | None = None,
    ) -> str:
        return cmd

    def kill(self, tag: str) -> bool:
        return False

    def cleanup(self, tag: str) -> None:
        return None

    def set_worker_pids_max(self, limit: int | None) -> None:
        return None

    def pids_events(self, tag: str) -> int:
        return 0

    def oom_kills(self, tag: str) -> int:
        events = self.memory_events(tag)
        return 0 if events is None else events.get("oom_kill", 0)

    def memory_events(self, tag: str) -> Mapping[str, int] | None:
        entry = self.planted.get(tag)
        return None if entry is None else entry[2]

    def applied_memory_max(self, tag: str) -> str | None:
        entry = self.planted.get(tag)
        return None if entry is None else entry[1]

    def peak_bytes(self, tag: str) -> int | None:
        entry = self.planted.get(tag)
        return None if entry is None else entry[0]

    def cpu_stats(self, tag: str) -> Mapping[str, int] | None:
        return None

    def cpu_pressure(self, tag: str) -> Mapping[str, float] | None:
        return None

    def thread_count(self, tag: str) -> int | None:
        return None

    def kill_all_remaining(self) -> int:
        return 0


def _rows(store: Path) -> list[dict[str, str]]:
    paths = sorted(store.glob("step_profiles_*.csv"))
    assert len(paths) == 1, f"expected exactly one per-step store, found {paths}"
    with paths[0].open(newline="", encoding="utf-8") as handle:
        return list(csv.DictReader(handle))


def _by_step(store: Path) -> dict[str, dict[str, str]]:
    return {row["step"]: row for row in _rows(store)}


def _record(store: Path, cfg: DagConfig, *, jobs: int, cgroups: CgroupManager) -> None:
    """Run the DAG once into ``store``. ``run_dag`` flushes the rows through the sink itself."""
    run_dag(
        cfg,
        jobs=jobs,
        cgroups=cgroups,
        metrics=CsvMetricsSink(store, git_sha="deadbee"),
        verbosity=0,
    )


# ---------------------------------------------------------------------------
# Overlap.
# ---------------------------------------------------------------------------


def test_two_concurrent_steps_are_provably_overlapping_after_a_csv_round_trip(
    tmp_path: Path,
) -> None:
    cfg = DagConfig(steps=(_step("one", "sleep 0.5"), _step("two", "sleep 0.5")))

    _record(tmp_path, cfg, jobs=2, cgroups=NoopCgroups())

    rows = _by_step(tmp_path)
    one, two = rows["g.one"], rows["g.two"]
    assert one["run_id"] == two["run_id"] != ""
    one_start, one_end = float(one["started_offset_s"]), float(one["finished_offset_s"])
    two_start, two_end = float(two["started_offset_s"]), float(two["finished_offset_s"])
    # Overlap is the interval test, computed from the CSV alone.
    assert one_start < two_end and two_start < one_end
    # Both steps slept half a second, so an offset pair that merely happened to satisfy the
    # interval test (both zero, say) would not describe the run. Each must span its own sleep.
    assert one_end - one_start >= 0.4
    assert two_end - two_start >= 0.4


def test_two_dependent_steps_are_provably_disjoint_after_a_csv_round_trip(
    tmp_path: Path,
) -> None:
    """The same reconstruction must be able to say NO.

    A test that only ever sees overlapping steps cannot tell a real measurement from a
    constant, so the serialized case is what proves the offsets carry the ordering.
    """
    cfg = DagConfig(
        steps=(
            _step("first", "sleep 0.3"),
            _step("second", "sleep 0.3", deps=["g.first"]),
        )
    )

    _record(tmp_path, cfg, jobs=2, cgroups=NoopCgroups())

    rows = _by_step(tmp_path)
    first, second = rows["g.first"], rows["g.second"]
    assert first["run_id"] == second["run_id"]
    assert float(first["finished_offset_s"]) <= float(second["started_offset_s"])


def test_each_execution_gets_its_own_run_id_in_a_shared_store(tmp_path: Path) -> None:
    cfg = DagConfig(steps=(_step("only", "true"),))

    _record(tmp_path, cfg, jobs=1, cgroups=NoopCgroups())
    _record(tmp_path, cfg, jobs=1, cgroups=NoopCgroups())

    rows = _rows(tmp_path)
    assert len(rows) == 2
    assert rows[0]["run_id"] != rows[1]["run_id"]


def test_one_sink_recording_two_batches_keeps_them_in_one_run(tmp_path: Path) -> None:
    """A run that flushes its rows in more than one call is still ONE run.

    Minting the id per batch instead of per sink would split a single execution into groups
    that never overlapped by construction — the exact reconstruction error this column exists
    to prevent.
    """
    sink = CsvMetricsSink(tmp_path, git_sha="deadbee")

    sink.record_step_profiles([{"step": "g.early"}], jobs=1)
    sink.record_step_profiles([{"step": "g.late"}], jobs=1)

    rows = _by_step(tmp_path)
    assert rows["g.early"]["run_id"] == rows["g.late"]["run_id"]


def test_a_sweep_gives_each_iteration_its_own_run_id(tmp_path: Path) -> None:
    """Every sweep iteration is a SEPARATE DAG execution and must be its own run.

    The three iterations below run strictly one after another, but each gets a fresh Runner
    with a fresh monotonic origin, so all three rows report ``started_offset_s`` at (near)
    zero. Stamping one shared ``run_id`` across them therefore makes the documented rule —
    two rows of the same run_id overlap iff their [started, finished] intervals do — declare a
    strictly sequential sweep fully concurrent, which is precisely the reconstruction error the
    column exists to prevent.
    """
    dag = tmp_path / "dag.json"
    dag.write_text('{"steps": [{"group": "g", "job": "j", "cmd": "true"}]}', encoding="utf-8")
    store = tmp_path / "perf"

    out, err = io.StringIO(), io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
        rc = main(
            [
                "sweep",
                "--dag",
                str(dag),
                "--step",
                "g.j",
                "--jobs",
                "1..3",
                "--perf-dir",
                str(store),
                "--unsafe-no-cgroups",
            ]
        )
    assert rc == 0, err.getvalue()

    rows = _rows(store)
    assert len(rows) == 3
    # The offsets really do all restart, so the ids are the only thing separating the runs.
    assert all(float(row["started_offset_s"]) < 0.5 for row in rows)
    assert len({row["run_id"] for row in rows}) == 3, (
        "three sequential executions sharing one run_id would read as three concurrent steps: "
        f"{[row['run_id'] for row in rows]}"
    )


def test_an_explicit_run_id_is_recorded_verbatim(tmp_path: Path) -> None:
    chosen = new_run_id()
    sink = CsvMetricsSink(tmp_path, git_sha="deadbee", run_id=chosen)

    sink.record_step_profiles([{"step": "g.only"}], jobs=1)

    assert _by_step(tmp_path)["g.only"]["run_id"] == chosen


# ---------------------------------------------------------------------------
# Censoring.
# ---------------------------------------------------------------------------


def test_an_exact_cap_pass_and_an_oom_kill_stay_distinguishable_after_a_csv_round_trip(
    tmp_path: Path,
) -> None:
    """Both steps peak exactly at their cap. Only the row's provenance separates them.

    ``clamped`` exits 0 while pinned at its ceiling by reclaim; ``killed`` is OOM-killed there.
    ``peak_bytes`` is identical, so a reader with only the peak sees one population. The event
    counters are what say that one was evicted into finishing and the other was shot.
    """
    cap = 8 * 1024**3
    planted = {
        "g.clamped": (cap, str(cap), {"low": 0, "high": 4, "max": 17, "oom": 0, "oom_kill": 0}),
        "g.killed": (cap, str(cap), {"low": 0, "high": 1, "max": 3, "oom": 2, "oom_kill": 2}),
    }
    cfg = DagConfig(steps=(_step("clamped", "true"), _step("killed", "true")))

    _record(tmp_path, cfg, jobs=2, cgroups=_PlantedCgroups(planted))

    rows = _by_step(tmp_path)
    clamped, killed = rows["g.clamped"], rows["g.killed"]
    assert clamped["peak_bytes"] == killed["peak_bytes"] == str(cap)
    assert clamped["memory_max_bytes"] == killed["memory_max_bytes"] == str(cap)
    # Reclaim-at-cap: held at the ceiling, never killed.
    assert clamped["memory_events_max"] == "17"
    assert clamped["memory_events_oom_kill"] == "0"
    assert clamped["oom_kills"] == "0"
    # OOM: the kernel killed it at the same ceiling.
    assert killed["memory_events_max"] == "3"
    assert killed["memory_events_oom_kill"] == "2"
    assert killed["oom_kills"] == "2"


def test_a_peak_below_its_cap_is_not_reported_as_touching_it(tmp_path: Path) -> None:
    """The uncensored case must be recognisable, or every sample looks censored."""
    cap = 8 * 1024**3
    planted = {
        "g.roomy": (
            2 * 1024**3,
            str(cap),
            {"low": 0, "high": 0, "max": 0, "oom": 0, "oom_kill": 0},
        )
    }
    cfg = DagConfig(steps=(_step("roomy", "true"),))

    _record(tmp_path, cfg, jobs=1, cgroups=_PlantedCgroups(planted))

    row = _by_step(tmp_path)["g.roomy"]
    assert int(row["peak_bytes"]) < int(row["memory_max_bytes"])
    assert row["memory_events_max"] == "0"


def test_an_unbounded_step_says_max_and_an_unmeasured_one_says_nothing(
    tmp_path: Path,
) -> None:
    """``max`` (no ceiling the runner can see) and blank (unknown) are different answers.

    Collapsing them is what makes a store dangerous to learn from: an unknown cap cannot rule
    out censoring at all, while ``max`` rules out censoring by the runner's own caps — see
    :meth:`Cgroups.applied_memory_max`, which reports the tightest of the step's own cap and the
    delegated scope's precisely so ``max`` can mean that much. ``unbounded`` ran with no such
    ceiling; ``unmeasured`` was never contained, so nothing is known about its ceiling.
    """
    planted = {
        "g.unbounded": (
            1024,
            "max",
            {"low": 0, "high": 0, "max": 0, "oom": 0, "oom_kill": 0},
        )
    }
    cfg = DagConfig(steps=(_step("unbounded", "true"), _step("unmeasured", "true")))

    _record(tmp_path, cfg, jobs=2, cgroups=_PlantedCgroups(planted))

    rows = _by_step(tmp_path)
    assert rows["g.unbounded"]["memory_max_bytes"] == "max"
    assert rows["g.unmeasured"]["memory_max_bytes"] == ""
    # An unmeasured step reports no event counters at all, rather than zeroes that would read
    # as "we looked and nothing happened".
    assert rows["g.unmeasured"]["memory_events_max"] == ""
    assert rows["g.unmeasured"]["memory_events_oom_kill"] == ""
    assert rows["g.unbounded"]["memory_events_max"] == "0"


def test_an_unboxed_run_leaves_every_censoring_column_blank(tmp_path: Path) -> None:
    cfg = DagConfig(steps=(_step("only", "true"),))

    _record(tmp_path, cfg, jobs=1, cgroups=NoopCgroups())

    row = _by_step(tmp_path)["g.only"]
    assert row["memory_max_bytes"] == ""
    for counter in ("low", "high", "max", "oom", "oom_kill"):
        assert row[f"memory_events_{counter}"] == ""
    # The run context still identifies the execution and its timing, which do not need a cgroup.
    assert row["run_id"] != ""
    assert float(row["finished_offset_s"]) >= float(row["started_offset_s"])


# ---------------------------------------------------------------------------
# The cgroupfs reading layer.
# ---------------------------------------------------------------------------


def _planted_cgroupfs(root: Path, tag: str) -> tuple[Cgroups, Path]:
    """A :class:`Cgroups` pointed at a planted directory tree instead of a real hierarchy.

    Returns the manager and the step's directory, which the manager names itself, so the test
    plants files where the production code will actually look for them.
    """
    cgroups = Cgroups()
    cgroups.enabled = True
    cgroups.root = root
    cgroups._made.add(tag)
    child = root / _sanitize(tag)
    child.mkdir(parents=True, exist_ok=True)
    return cgroups, child


def test_applied_memory_max_reads_back_the_cap_the_kernel_holds(tmp_path: Path) -> None:
    """Read back, not echoed: the case that matters is a cap the kernel did NOT accept."""
    cgroups, child = _planted_cgroupfs(tmp_path, "g.job")
    (child / "memory.max").write_text("8589934592\n")

    assert cgroups.applied_memory_max("g.job") == "8589934592"


def test_applied_memory_max_distinguishes_unbounded_from_unreadable(tmp_path: Path) -> None:
    cgroups, child = _planted_cgroupfs(tmp_path, "g.job")

    # No memory.max file at all: the cap is UNKNOWN.
    assert cgroups.applied_memory_max("g.job") is None

    (child / "memory.max").write_text("max\n")
    # Now it is known, and known to be unbounded at every level this manager can see (the
    # planted scope root below carries no memory.max of its own).
    assert cgroups.applied_memory_max("g.job") == "max"


def test_an_uncapped_step_reports_the_outer_scope_cap_it_actually_ran_under(
    tmp_path: Path,
) -> None:
    """An uncharacterised step gets no INNER cap, but it is never actually unbounded.

    The runner refuses to run without an outer scope ``memory.max``, and cgroup-v2 limits are
    hierarchical, so such a step is held at the scope's ceiling for its whole life. Reporting
    the child's own ``max`` would label it "known unbounded" — reclaim at the ancestor's limit
    is not charged to the child's ``memory.events`` either, so the row would read as a
    comfortable fit for exactly the population being profiled to learn its footprint.
    """
    cgroups, child = _planted_cgroupfs(tmp_path, "g.job")
    (tmp_path / "memory.max").write_text("8589934592\n")
    (child / "memory.max").write_text("max\n")

    assert cgroups.applied_memory_max("g.job") == "8589934592"


def test_applied_memory_max_reports_the_tighter_of_the_step_and_scope_caps(
    tmp_path: Path,
) -> None:
    """Both directions, so the answer is a minimum and not "whichever we happened to read"."""
    cgroups, child = _planted_cgroupfs(tmp_path, "g.job")
    (tmp_path / "memory.max").write_text("8589934592\n")

    (child / "memory.max").write_text("2147483648\n")
    assert cgroups.applied_memory_max("g.job") == "2147483648"

    (child / "memory.max").write_text("17179869184\n")
    assert cgroups.applied_memory_max("g.job") == "8589934592"


def test_an_unbounded_scope_leaves_the_step_cap_untouched(tmp_path: Path) -> None:
    cgroups, child = _planted_cgroupfs(tmp_path, "g.job")
    (tmp_path / "memory.max").write_text("max\n")
    (child / "memory.max").write_text("2147483648\n")

    assert cgroups.applied_memory_max("g.job") == "2147483648"


def test_memory_events_carries_every_counter_and_agrees_with_oom_kills(
    tmp_path: Path,
) -> None:
    cgroups, child = _planted_cgroupfs(tmp_path, "g.job")
    (child / "memory.events").write_text("low 0\nhigh 12\nmax 41\noom 3\noom_kill 2\n")

    events = cgroups.memory_events("g.job")

    assert events == {"low": 0, "high": 12, "max": 41, "oom": 3, "oom_kill": 2}
    # One parse, one answer: the OOM count cannot drift from the recorded counters.
    assert cgroups.oom_kills("g.job") == 2


def test_one_unparsable_memory_events_line_does_not_discard_the_others(
    tmp_path: Path,
) -> None:
    """The kernel may add counters, and a blank trailing line costs nothing to skip.

    Discarding the whole file would blank all five CSV cells AND drop the recorded OOM count to
    zero, so a step that was actually killed would read as one that was never pressured. The
    Rust build skips per line, so both builds must, or the same ``/sys/fs/cgroup`` file yields
    two different CSV rows depending on which binary wrote it.
    """
    cgroups, child = _planted_cgroupfs(tmp_path, "g.job")
    (child / "memory.events").write_text(
        "low 0\nhigh not-a-number\n\nmax 41\nfuture_counter 9\noom 3\noom_kill 2\n"
    )

    events = cgroups.memory_events("g.job")

    assert events == {"low": 0, "max": 41, "future_counter": 9, "oom": 3, "oom_kill": 2}
    assert cgroups.oom_kills("g.job") == 2


def test_memory_events_is_none_rather_than_empty_when_unreadable(tmp_path: Path) -> None:
    cgroups, _child = _planted_cgroupfs(tmp_path, "g.job")

    assert cgroups.memory_events("g.job") is None
    assert cgroups.oom_kills("g.job") == 0
