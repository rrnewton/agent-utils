"""Tests for the profile-store feedback reader + the planner (safe_ci_dag_runner.estimates)."""

from __future__ import annotations

from pathlib import Path

from safe_ci_dag_runner import (
    DagConfig,
    ResourceHint,
    Step,
    apply_plan_to_config,
    build_plan,
    load_step_samples,
    plan_to_json,
    plan_to_text,
)
from safe_ci_dag_runner.estimates import Planner

_HEADER = (
    "timestamp,machine_id,container_class,git_sha,outer_jobs,profile_base_sha,enforcement_kind,"
    "runner_name,step,classification,inner_jobs,elapsed_s,returncode,ok,timed_out,oom_kills,"
    "peak_bytes,thread_peak,pct_other\n"
)


def _write_store(tmp_path: Path, rows: list[str], *, machine: str = "m", container: str = "c") -> Path:
    store = tmp_path / "store"
    store.mkdir()
    (store / f"step_profiles_{machine}_{container}.csv").write_text(_HEADER + "".join(rows))
    return store


def _row(step: str, elapsed: str, peak: str = "", pct_other: str = "0.0") -> str:
    return (
        f"2026-07-26T10:00:00,m,c,abc,1,abc,unverified,local,{step},light,1,{elapsed},0,True,"
        f"False,0,{peak},,{pct_other}\n"
    )


def test_no_store_file_returns_empty(tmp_path: Path) -> None:
    assert load_step_samples(tmp_path, "m", "c") == {}


def test_median_duration_and_percentile_rss(tmp_path: Path) -> None:
    store = _write_store(
        tmp_path,
        [
            _row("g.a", "2.0", "1000"),
            _row("g.a", "4.0", "3000"),
            _row("g.a", "3.0", "2000"),
        ],
    )
    samples = load_step_samples(store, "m", "c")
    assert samples["g.a"].samples == 3
    # median of [2,3,4] == 3.0 (no contention -> no discount).
    assert samples["g.a"].est_duration_s == 3.0
    # p90 nearest-rank of [1000,2000,3000] (n=3, rank=3) == 3000.
    assert samples["g.a"].rss_estimate_bytes == 3000


def test_contention_discount_recovers_intrinsic_duration(tmp_path: Path) -> None:
    # A 10s sample under 50% other-work contention has intrinsic ~5s, matching the two quiet 5s
    # samples; the discounted median must be 5.0, not dragged up by the contended sample.
    store = _write_store(
        tmp_path,
        [
            _row("g.a", "5.0", "100", pct_other="0.0"),
            _row("g.a", "5.0", "100", pct_other="0.0"),
            _row("g.a", "10.0", "100", pct_other="50.0"),
        ],
    )
    samples = load_step_samples(store, "m", "c")
    assert samples["g.a"].est_duration_s == 5.0


def test_mad_trim_ignores_outlier(tmp_path: Path) -> None:
    store = _write_store(
        tmp_path,
        [_row("g.a", v) for v in ("3.0", "3.0", "3.0", "3.0", "100.0")],
    )
    samples = load_step_samples(store, "m", "c")
    assert samples["g.a"].est_duration_s == 3.0


def test_missing_columns_degrade_gracefully(tmp_path: Path) -> None:
    # No peak_bytes recorded -> rss_estimate is None (fall back to hint elsewhere).
    store = _write_store(tmp_path, [_row("g.a", "2.0", peak=""), _row("g.a", "2.0", peak="")])
    samples = load_step_samples(store, "m", "c")
    assert samples["g.a"].est_duration_s == 2.0
    assert samples["g.a"].rss_estimate_bytes is None


def _dag() -> DagConfig:
    return DagConfig(
        steps=(
            Step("g", "prep", "prep", "true", hint=ResourceHint(est_duration_s=1.0)),
            Step("g", "heavy", "heavy", "true", deps=["g.prep"], hint=ResourceHint(est_duration_s=10.0)),
            Step("g", "solo", "solo", "true", hint=ResourceHint(est_duration_s=5.0)),
        )
    )


def test_planner_orders_differ_and_are_deterministic() -> None:
    cfg = _dag()
    lpt = build_plan(cfg, {}, planner=Planner.GREEDY_LPT)
    cp = build_plan(cfg, {}, planner=Planner.CRITICAL_PATH)
    # greedy-lpt: by single est desc; critical-path: by bottom-level desc.
    assert list(lpt.order) == ["g.heavy", "g.solo", "g.prep"]
    assert list(cp.order) == ["g.prep", "g.heavy", "g.solo"]
    assert lpt.order != cp.order
    # bottom_level(prep) = 1 + bottom_level(heavy)=10 -> 11; critical path prep -> heavy.
    assert list(cp.critical_path) == ["g.prep", "g.heavy"]
    assert cp.critical_path_length_s == 11.0


def test_store_wins_over_hint_and_feeds_config(tmp_path: Path) -> None:
    store = _write_store(
        tmp_path,
        [_row("g.heavy", "8.0", "6000"), _row("g.heavy", "8.0", "6000")],
    )
    samples = load_step_samples(store, "m", "c")
    cfg = _dag()
    plan = build_plan(cfg, samples, planner=Planner.CRITICAL_PATH)
    by_tag = plan.by_tag()
    # g.heavy learned 8.0s + 6000 bytes from the store; g.solo has neither -> hint / none.
    assert by_tag["g.heavy"].est_source == "store"
    assert by_tag["g.heavy"].est_duration_s == 8.0
    assert by_tag["g.heavy"].rss_source == "store"
    assert by_tag["g.heavy"].rss_estimate_bytes == 6000
    assert by_tag["g.solo"].est_source == "hint"
    assert by_tag["g.prep"].rss_source == "none"
    # apply_plan_to_config threads the learned values into the cfg hints (feeds sizing + ordering).
    applied = apply_plan_to_config(cfg, plan)
    heavy = applied.by_tag()["g.heavy"]
    assert heavy.hint.est_duration_s == 8.0
    assert heavy.hint.rss_baseline_bytes == 6000


def test_min_samples_threshold_falls_back_to_hint(tmp_path: Path) -> None:
    store = _write_store(tmp_path, [_row("g.heavy", "8.0", "6000")])
    samples = load_step_samples(store, "m", "c")
    cfg = _dag()
    # With min_samples=2 the single sample is not enough; the DAG hint (10.0) wins.
    plan = build_plan(cfg, samples, planner=Planner.GREEDY_LPT, min_samples=2)
    entry = plan.by_tag()["g.heavy"]
    assert entry.est_source == "hint"
    assert entry.est_duration_s == 10.0


def test_plan_json_and_text_are_stable(tmp_path: Path) -> None:
    cfg = _dag()
    plan = build_plan(cfg, {}, planner=Planner.GREEDY_LPT)
    js = plan_to_json(plan)
    assert '"planner": "greedy-lpt"' in js
    assert js.count('"tag":') == 3
    assert js.startswith("{\n") and js.endswith("}")
    text = plan_to_text(plan)
    assert text.startswith("plan: greedy-lpt\n")
    assert "scheduled order: g.heavy, g.solo, g.prep" in text
    assert "critical path (" in text


def test_empty_dag_plan() -> None:
    plan = build_plan(DagConfig(steps=()), {}, planner=Planner.CRITICAL_PATH)
    assert list(plan.order) == []
    assert list(plan.critical_path) == []
    assert plan_to_json(plan).endswith('"steps": []\n}')
