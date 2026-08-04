"""Tests for safe_ci_dag_runner.sizing (synthetic, deterministic)."""

from __future__ import annotations

from safe_ci_dag_runner.model import DagConfig, ResourceHint, Step
from safe_ci_dag_runner.sizing import (
    PER_BUILD_JOB_MEM_BYTES,
    derive_build_jobs,
    jobs_footprint_bytes,
    jobs_for_budget,
    parse_size,
    schedulable_peak_mem_bytes,
    step_mem_cap_bytes,
    transitive_deps,
)

GIB = 1024**3


def test_derive_build_jobs_caps_the_284_leak() -> None:
    """Three-part bracket for the CARGO_BUILD_JOBS/NUM_JOBS quota leak (hermit#1584)."""
    # CAUGHT: an unpinned wide-quota step under an 8 GiB cap must be bounded by memory,
    # never the granted core count (the observed NUM_JOBS=284 -> OOM-killed linker).
    assert derive_build_jobs(284, 8 * GIB) == 8
    assert 8 == 8 * GIB // PER_BUILD_JOB_MEM_BYTES  # the bound is the #1584-safe j8

    # LEGITIMATE, N=3 (a mechanism that clamps EVERYTHING to 1 would pass the CAUGHT
    # assertion too, so these prove it does not over-constrain honest configs):
    assert derive_build_jobs(4, 64 * GIB) == 4  # (1) cpu-bound small box, unharmed
    assert derive_build_jobs(8, None) == 8  # (2) no mem cap -> cpu-bound, unharmed
    assert derive_build_jobs(32, 8 * GIB) == 8  # (3) mem-bound == #1584 safe width

    # FLOOR: never 0 / never negative, even when a cap fits less than one job.
    assert derive_build_jobs(284, 512 * 1024**2) == 1
    assert derive_build_jobs(1, 1) == 1


def _cfg() -> DagConfig:
    steps = (
        Step("g", "A", "", "true", hint=ResourceHint(rss_baseline_bytes=3 * GIB)),
        Step("g", "B", "", "true", deps=["g.A"], hint=ResourceHint(rss_baseline_bytes=2 * GIB)),
        Step("g", "C", "", "true", hint=ResourceHint(rss_baseline_bytes=4 * GIB, resources={"gpu": 1})),
        Step("g", "D", "", "true", hint=ResourceHint(rss_baseline_bytes=1 * GIB, resources={"gpu": 1})),
    )
    return DagConfig(
        steps=steps,
        resource_caps={"gpu": 1},
        mem_cap_factor=1.0,
        outer_mem_safety_factor=1.0,
        mem_cap_floor_bytes=0,
    )


def test_parse_size() -> None:
    assert parse_size("8G") == 8 * GIB
    assert parse_size("4096M") == 4096 * 1024**2
    assert parse_size("2048K") == 2048 * 1024
    assert parse_size("12345") == 12345
    assert parse_size(None) is None
    assert parse_size("nonsense") is None


def test_transitive_deps() -> None:
    deps = transitive_deps(list(_cfg().steps))
    assert deps["g.B"] == {"g.A"}
    assert deps["g.A"] == set()


def test_step_mem_cap_hard_override_wins() -> None:
    step = Step(
        "g",
        "X",
        "",
        "true",
        hint=ResourceHint(rss_baseline_bytes=2 * GIB, hard_mem_max_bytes=9 * GIB),
    )
    assert step_mem_cap_bytes(step, mem_cap_factor=1.25) == 9 * GIB


def test_schedulable_peak_picks_best_feasible_set() -> None:
    # gpu cap 1 forbids C+D together; A (3G) + C (4G) are independent -> 7G is the max.
    total, chosen = schedulable_peak_mem_bytes(_cfg(), jobs=4)
    assert total == 7 * GIB
    assert set(chosen) == {"g.A", "g.C"}


def test_jobs_for_budget_monotonic_and_at_least_one() -> None:
    cfg = _cfg()
    # -j1 footprint is the single largest cap (C = 4G).
    assert jobs_footprint_bytes(cfg, 1) == 4 * GIB
    # budget below the -j2 peak (7G) but >= -j1 (4G) yields exactly 1.
    assert jobs_for_budget(cfg, 6 * GIB) == (1, 4 * GIB)
    # a budget below even -j1 still returns 1 (a WAIT/abort decision for the caller).
    assert jobs_for_budget(cfg, 1 * GIB)[0] == 1
