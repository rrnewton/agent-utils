"""Summarize plans the whole tree before spending, holds a budget, and resumes a partial run."""

from __future__ import annotations

import json
import stat
from collections.abc import Callable
from dataclasses import replace
from pathlib import Path

import pytest

from wrkviz.budget import (
    BudgetExceeded,
    BudgetStop,
    CostModel,
    JobUsage,
    Prices,
    SpendGuard,
    StageCount,
    SummaryPlan,
    load_state,
)
from wrkviz.cli import main as timeline_main
from wrkviz.pipeline import SummarizeReport, build_archive, plan_summaries, summarize_archive
from wrkviz.periods import periods_for_range
from wrkviz.window import DateWindow
from tests.test_timeline_pipeline import ROOT, START, _event, _two_day_team, _write_team
from tests.timeline_projection import schema_1_timeline_text

# A stand-in for the Codex CLI that answers summary and naming prompts, logs the keys it was
# asked for, and reports token usage taken from the environment.
_FAKE = r'''#!/usr/bin/env python3
import json
import os
import sys
from pathlib import Path

args = sys.argv[1:]
prompt = sys.stdin.read()
output = Path(args[args.index("--output-last-message") + 1])
if "BEGIN_AGENT_NAME_JOBS_JSON" in prompt:
    jobs = json.loads(prompt.split("BEGIN_AGENT_NAME_JOBS_JSON\n", 1)[1].split("END_AGENT_NAME_JOBS_JSON", 1)[0])
    names = [{
        "key": job["key"],
        "thread_id": job["thread_id"],
        "short_name": "Coordinator" if job["official_path"].rstrip("/") == "/root" else "Receipt Binding Work",
        "rationale": "Named for its work.",
        "lifetime_summary": "Bound receipts to the archive and verified them.",
    } for job in jobs]
    if os.environ.get("FAKE_NAME_INVENT") and len(names) > 1:
        names[-1]["key"] += "_dup_placeholder"
    answer = {"names": names}
else:
    jobs = json.loads(prompt.split("BEGIN_JOBS_JSON\n", 1)[1].split("END_JOBS_JSON", 1)[0])
    summaries = []
    for job in jobs:
        if job.get("summary_style") == "project-overview":
            summaries.append({"key": job["key"], "phrase": "Project overview supported",
                              "paragraph": "Widget archives agent work.", "work_summary": []})
        else:
            summaries.append({"key": job["key"], "phrase": "Built " + job["key"],
                              "paragraph": "Implemented durable output.",
                              "work_summary": [{"at_ms": job["start_ms"], "text": "Implemented it."}]})
    answer = {"summaries": summaries}
output.write_text(json.dumps(answer), encoding="utf-8")
with open(os.environ["FAKE_LOG"], "a", encoding="utf-8") as handle:
    handle.write(json.dumps([job["key"] for job in jobs]) + "\n")
print(json.dumps({"type": "turn.completed", "usage": {
    "input_tokens": int(os.environ.get("FAKE_INPUT_TOKENS", "1000")),
    "cached_input_tokens": 0,
    "output_tokens": int(os.environ.get("FAKE_OUTPUT_TOKENS", "100")),
}}))
'''

PRICES = Prices.parse("input=2,output=10")


@pytest.fixture
def fake(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    path = tmp_path / "fake-codex"
    path.write_text(_FAKE, encoding="utf-8")
    path.chmod(path.stat().st_mode | stat.S_IXUSR)
    monkeypatch.setenv("FAKE_LOG", str(tmp_path / "calls.log"))
    return path


def _calls(tmp_path: Path) -> list[list[str]]:
    log = tmp_path / "calls.log"
    if not log.exists():
        return []
    return [json.loads(line) for line in log.read_text(encoding="utf-8").splitlines()]


def _archive(tmp_path: Path) -> tuple[Path, str]:
    archive = tmp_path / "archive"
    team = _two_day_team("Implemented alpha receipt binding; 101 focused tests passed.")
    _write_team(archive, team)
    return archive, team.team_slug


def _summarize(
    archive: Path,
    slug: str,
    fake: Path,
    *,
    budget_usd: float | None = None,
    prices: Prices | None = None,
    on_plan: Callable[[SummaryPlan], None] | None = None,
    name_batch_size: int = 1,
) -> SummarizeReport:
    return summarize_archive(
        archive,
        slug,
        "codex",
        "fake-model",
        codex_command=(str(fake),),
        max_workers=1,
        batch_size=1,
        name_batch_size=name_batch_size,
        rollup_kinds=("daily", "weekly"),
        budget_usd=budget_usd,
        prices=prices,
        on_plan=on_plan,
    )


def test_plan_counts_the_whole_tree_before_any_model_call(tmp_path: Path, fake: Path) -> None:
    archive, slug = _archive(tmp_path)
    plan = plan_summaries(
        archive, slug, "codex", "fake-model", rollup_kinds=("daily", "weekly"), prices=PRICES
    )
    by_name = {stage.name: stage for stage in plan.stages}
    assert by_name["phase summaries"].to_generate == by_name["phase summaries"].total > 0
    assert by_name["agent names"].to_generate == by_name["agent names"].total > 0
    assert by_name["project overview"].to_generate == 1
    days = by_name["daily rollups (technical)"]
    assert days.to_generate == days.total >= 2
    assert by_name["daily rollups (plain-language)"].to_generate == days.total
    assert by_name["weekly rollups (technical)"].to_generate >= 1
    assert plan.estimated_usd is not None and plan.estimated_usd > 0
    assert any("total:" in line for line in plan.lines())
    assert _calls(tmp_path) == []

    _summarize(archive, slug, fake)
    after = plan_summaries(
        archive, slug, "codex", "fake-model", rollup_kinds=("daily", "weekly"), prices=PRICES
    )
    assert after.to_generate == 0
    assert after.estimated_usd == 0
    # The estimate is now calibrated from this cache's own receipts.
    assert after.costs.source("phase").startswith("mean of")


def test_over_budget_plan_refuses_to_start_and_spends_nothing(
    tmp_path: Path, fake: Path
) -> None:
    archive, slug = _archive(tmp_path)
    plans: list[SummaryPlan] = []
    with pytest.raises(BudgetExceeded, match="refusing to start"):
        _summarize(archive, slug, fake, budget_usd=0.01, prices=PRICES, on_plan=plans.append)
    assert len(plans) == 1 and plans[0].over_budget
    assert _calls(tmp_path) == []
    assert load_state(archive / "teams" / slug / "summary_data") is None


def test_budget_without_prices_is_refused(tmp_path: Path, fake: Path) -> None:
    archive, slug = _archive(tmp_path)
    with pytest.raises(ValueError, match="--price"):
        _summarize(archive, slug, fake, budget_usd=5.0)
    assert _calls(tmp_path) == []


def test_guard_stop_leaves_marked_partial_state_and_next_run_resumes(
    tmp_path: Path, fake: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive, slug = _archive(tmp_path)
    summary_root = archive / "teams" / slug / "summary_data"
    plan = plan_summaries(
        archive, slug, "codex", "fake-model", rollup_kinds=("daily", "weekly"), prices=PRICES
    )
    assert plan.estimated_usd is not None
    # The plan fits the budget, but the model turns out far more expensive than estimated:
    # the first call alone costs more than the whole budget.
    monkeypatch.setenv("FAKE_INPUT_TOKENS", "50000000")
    with pytest.raises(BudgetStop, match="budget reached"):
        _summarize(archive, slug, fake, budget_usd=plan.estimated_usd * 1.5, prices=PRICES)
    first_calls = _calls(tmp_path)
    assert len(first_calls) == 1
    state = load_state(summary_root)
    assert state is not None
    assert state["status"] == "partial"
    stopped = state["stopped"]
    assert isinstance(stopped, dict)
    assert stopped["reason"] == "budget"
    assert stopped["stage"] == "phase summaries"
    assert isinstance(state["spent_usd"], float) and state["spent_usd"] > plan.estimated_usd

    # The resumed plan says so, and counts the paid leaf as cached.
    resumed = plan_summaries(
        archive, slug, "codex", "fake-model", rollup_kinds=("daily", "weekly"), prices=PRICES
    )
    assert any(line.strip().startswith("resuming:") for line in resumed.lines())
    by_name = {stage.name: stage for stage in resumed.stages}
    assert by_name["phase summaries"].cached == 1

    monkeypatch.setenv("FAKE_INPUT_TOKENS", "1000")
    _summarize(archive, slug, fake)
    state = load_state(summary_root)
    assert state is not None
    assert state["status"] == "complete"
    resumed_from = state["resumed_from"]
    assert isinstance(resumed_from, dict) and resumed_from["status"] == "partial"
    asked = [key for call in _calls(tmp_path) for key in call]
    assert len(asked) == len(set(asked)), "a paid job was asked for again"
    assert first_calls[0][0] in asked


def test_interrupted_run_is_reported_and_build_warns(
    tmp_path: Path, fake: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    archive, slug = _archive(tmp_path)
    _summarize(archive, slug, fake)
    state_path = archive / "teams" / slug / "summary_data" / "summarize_state.json"
    state = json.loads(state_path.read_text(encoding="utf-8"))
    state["status"] = "running"  # what a killed run leaves behind
    state_path.write_text(json.dumps(state), encoding="utf-8")

    plan = plan_summaries(archive, slug, "codex", "fake-model", rollup_kinds=("daily", "weekly"))
    assert any("was interrupted" in line for line in plan.lines())
    build_archive(archive, slug)
    assert timeline_main(["build", "--output", str(archive), "--team", slug]) == 0
    assert "summaries are partial" in capsys.readouterr().err


def test_cli_plan_prints_estimate_and_exits_3_over_budget(
    tmp_path: Path, fake: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    archive, slug = _archive(tmp_path)
    common = [
        "summarize", "--output", str(archive), "--team", slug, "--backend", "codex",
        "--model", "fake-model", "--codex-command", str(fake), "--rollup-kind", "daily",
        "--price", "input=2,output=10",
    ]
    assert timeline_main([*common, "--plan"]) == 0
    out = capsys.readouterr().out
    assert "summary plan for" in out and "phase summaries" in out and "~$" in out
    assert timeline_main([*common, "--plan", "--budget-usd", "0.0001"]) == 3
    assert "(over budget)" in capsys.readouterr().out
    assert timeline_main([*common, "--budget-usd", "0.0001"]) == 3
    assert "refusing to start" in capsys.readouterr().err
    assert _calls(tmp_path) == []
    assert not (archive / "teams" / slug / "summary_data" / "summarize_state.json").exists()


def test_name_batch_answer_under_an_invented_key_is_retried(
    tmp_path: Path, fake: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    archive, slug = _archive(tmp_path)
    monkeypatch.setenv("FAKE_NAME_INVENT", "1")
    report = _summarize(archive, slug, fake, name_batch_size=12)
    assert report.agent_names >= 2
    name_calls = [call for call in _calls(tmp_path) if call[0].startswith("agent-name:")]
    # One batch answered with an invented key, then each of its jobs alone.
    assert len(name_calls) == 1 + report.agent_names
    assert sorted(name_calls[0]) == sorted(key for call in name_calls[1:] for key in call)


def test_guard_admits_every_batch_of_a_plan_that_exactly_meets_the_budget() -> None:
    costs = CostModel(per_job={"phase": JobUsage(333.3333, 0, 0, 33.3333, samples=3)})
    prices = Prices.parse("input=3,output=15")
    plan = SummaryPlan(
        team_slug="t",
        stages=[StageCount(name="phase summaries", kind="phase", total=7, to_generate=7)],
        costs=costs,
        prices=prices,
        budget_usd=None,
    )
    budget = plan.estimated_usd
    assert budget is not None
    guard = SpendGuard(budget, prices, costs)
    for _ in range(7):
        reservation = guard.reserve("phase", 1)
        assert reservation is not None
        guard.settle(reservation, None)
    assert guard.reserve("phase", 1) is None


def test_windowed_summary_rollups_are_current_in_a_full_range_build(tmp_path: Path) -> None:
    """A rollup summarized inside a window reads different prior context than the full build
    gives it (earlier days are unsummarized there); what it summarizes is the same, so the
    build shows it instead of marking it stale."""

    base = _two_day_team("Implemented exact receipt binding and verified the focused tests.")
    # A third day, so the summarized middle day is a whole period in the full build too (the
    # first and last days of a team's range are partial periods there).
    later = 2 * 26 * 60 * 60 * 1000
    root = ROOT
    third = (
        _event("day-three-prompt", root, later, "user_prompt", "Check the report again."),
        _event("day-three-reply", root, later + 2_000, "assistant_message", "Still verified."),
    )
    team = replace(
        base,
        events=base.events + third,
        agents=tuple(
            replace(agent, ended_at_ms=START + later + 3_000)
            if agent.thread_id == root
            else agent
            for agent in base.agents
        ),
    )
    archive = tmp_path / "archive"
    _write_team(archive, team)
    days = [
        period
        for period in periods_for_range(
            min(event.timestamp_ms for event in team.events),
            max(event.timestamp_ms for event in team.events),
            team.display_timezone,
            team.team_slug,
        )
        if period.kind == "daily"
    ]
    assert len(days) >= 3
    summarize_archive(
        archive,
        team.team_slug,
        "heuristic",
        "offline",
        summary_window=DateWindow(None, None, days[1].start_ms, days[1].end_ms),
        rollup_kinds=("daily",),
    )
    output = tmp_path / "site"
    build_archive(archive, team.team_slug, rollup_kinds=("daily",), output=output)
    rollups = {
        item["start_ms"]: item["summary_available"]
        for item in json.loads(schema_1_timeline_text(output))["rollups"]
        if item["kind"] == "daily"
    }
    assert rollups[days[1].start_ms] is True
    assert rollups[days[0].start_ms] is False
