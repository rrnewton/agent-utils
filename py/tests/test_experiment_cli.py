"""Tests for the CLI parsing helpers, the breach-precedence classifier, and the dry plan-round
path (which needs no cgroups). Exercises requirement 4 (a clean kill that NAMES the breach)."""

from __future__ import annotations

from pathlib import Path

import pytest

from safe_ci_dag_runner import StepOutcome

from parallel_experiment_runner.cli import main, parse_seeds
from parallel_experiment_runner.execute import _classify_outcome
from parallel_experiment_runner.model import (
    STATUS_CANCELLED,
    STATUS_CPU_TIMEOUT,
    STATUS_HIT,
    STATUS_MEMORY_CAP,
    STATUS_TIMEOUT,
    ExperimentSpec,
    HitCondition,
    WorkerLimits,
)


def test_parse_seeds_ranges_singletons_order() -> None:
    assert parse_seeds("0-4,10,20-22") == (0, 1, 2, 3, 4, 10, 20, 21, 22)


def test_parse_seeds_single() -> None:
    assert parse_seeds("7") == (7,)


def test_parse_seeds_rejects_reversed_range() -> None:
    with pytest.raises(ValueError):
        parse_seeds("5-1")


def test_parse_seeds_rejects_empty() -> None:
    with pytest.raises(ValueError):
        parse_seeds(" , ")


def _spec(**kw: object) -> ExperimentSpec:
    base: dict[str, object] = {
        "name": "s",
        "command": ("run", "{seed}"),
        "worker_limits": WorkerLimits(cpu_cores=1, memory_bytes=4096, cpu_timeout_s=3, wall_timeout_s=60),
        "hit": HitCondition(hit_exit_codes=(0,)),
    }
    base.update(kw)
    return ExperimentSpec(**base)  # type: ignore[arg-type]


def _outcome(returncode: int = 0, *, aborted: bool = False) -> StepOutcome:
    return StepOutcome(
        tag="seed.3", ok=(returncode == 0 and not aborted), duration_s=1.0,
        summary="", returncode=returncode, aborted=aborted,
    )


def test_breach_cpu_timeout_named(tmp_path: Path) -> None:
    row = {"cpu_timed_out": True, "cpu.usage_usec": 3_000_000}
    res = _classify_outcome(_spec(), _outcome(returncode=1), row, tmp_path / "seed-3.log")
    assert res.status == STATUS_CPU_TIMEOUT
    assert res.cpu_s == 3.0
    assert "CPU-TIMEOUT" in res.breach and "budget 3s" in res.breach


def test_breach_memory_cap_named(tmp_path: Path) -> None:
    row = {"oom_kills": 1, "peak_bytes": 5000}
    res = _classify_outcome(_spec(), _outcome(returncode=137), row, tmp_path / "seed-3.log")
    assert res.status == STATUS_MEMORY_CAP
    assert "MEMORY-CAP" in res.breach


def test_breach_wall_timeout_named(tmp_path: Path) -> None:
    row = {"timed_out": True, "elapsed_s": 60.0}
    res = _classify_outcome(_spec(), _outcome(returncode=1), row, tmp_path / "seed-3.log")
    assert res.status == STATUS_TIMEOUT
    assert "TIMEOUT" in res.breach


def test_breach_precedence_cancel_beats_cpu_timeout(tmp_path: Path) -> None:
    # An eager-cancel outranks a same-round cpu-timeout signal: a killed sibling is CANCELLED.
    row = {"cpu_timed_out": True, "cpu.usage_usec": 3_000_000}
    res = _classify_outcome(_spec(), _outcome(returncode=1, aborted=True), row, tmp_path / "s.log")
    assert res.status == STATUS_CANCELLED


def test_clean_zero_is_hit_when_default(tmp_path: Path) -> None:
    res = _classify_outcome(_spec(), _outcome(returncode=0), {}, tmp_path / "seed-3.log")
    assert res.status == STATUS_HIT  # default hit condition is exit 0
    assert res.breach == ""


def test_plan_round_dry_runs_without_cgroups(capsys: pytest.CaptureFixture[str]) -> None:
    code = main([
        "plan-round", "--name", "demo", "--seeds", "0-3",
        "--cpu-cores", "1", "--memory", "1G", "--max-concurrency", "2",
        "--slice-cpu", "8", "--slice-memory", "16G", "--slice-disk", "100G",
        "--", "/bin/true", "{seed}",
    ])
    assert code == 0
    out = capsys.readouterr().out
    assert "resolved width:" in out
    assert "profile key:" in out


def test_leading_double_dash_is_stripped_from_command(tmp_path: Path) -> None:
    # argparse REMAINDER keeps a leading "--"; plan-round must not treat it as argv[0].
    code = main([
        "plan-round", "--seeds", "0", "--slice-cpu", "4", "--slice-memory", "8G",
        "--slice-disk", "50G", "--", "echo", "{seed}",
    ])
    assert code == 0


def test_quickstart_and_version() -> None:
    assert main(["quickstart"]) == 0
    with pytest.raises(SystemExit) as exc:
        main(["--version"])
    assert exc.value.code == 0
