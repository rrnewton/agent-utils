"""CLI contract for exact summary token-cost reporting."""

from __future__ import annotations

from pathlib import Path

import pytest

import agent_team_timeline.cli as timeline_cli
from agent_team_timeline.pipeline import SummarizeReport
from agent_team_timeline.token_usage import TokenUsage


def _report() -> SummarizeReport:
    return SummarizeReport(
        backend="codex",
        model="gpt-5.6-sol",
        reasoning_effort="xhigh",
        service_tier="priority",
        phases=4,
        rollups=1,
        agent_names=2,
        glossary_terms=3,
        project_overviews=1,
        glossary_definitions=3,
        cache_hits=1,
        cache_misses=6,
        backend_batches=2,
        newly_spent_usage=TokenUsage(
            input_tokens=1_000,
            cached_input_tokens=600,
            cache_write_input_tokens=10,
            output_tokens=80,
            reasoning_output_tokens=30,
        ),
        newly_spent_unknown_receipts=1,
        artifact_generation_usage=TokenUsage(
            input_tokens=1_400,
            cached_input_tokens=800,
            cache_write_input_tokens=10,
            output_tokens=100,
            reasoning_output_tokens=40,
        ),
        artifact_generation_unknown_receipts=2,
        unknown_legacy_artifacts=3,
        usage_run_paths=("teams/test/summary_data/cache/_usage/runs/one.json",),
        files_changed=7,
    )


def test_summary_output_separates_new_spend_from_artifact_cost(
    capsys: pytest.CaptureFixture[str],
) -> None:
    timeline_cli._print_summaries(_report())
    output = capsys.readouterr().out
    assert (
        "tokens newly spent: input=1000, cached_input=600, cache_write_input=10, "
        "output=80, reasoning_output=30, total=1080; unknown_receipts=1"
    ) in output
    assert (
        "tokens behind returned cached artifacts: input=1400, cached_input=800, "
        "cache_write_input=10, output=100, reasoning_output=40, total=1500; "
        "unknown_receipts=2"
    ) in output
    assert "3 legacy artifact(s) have no usage receipt" in output


def test_reasoning_effort_and_service_tier_reach_pipeline(
    monkeypatch: pytest.MonkeyPatch, tmp_path: Path
) -> None:
    seen: dict[str, object] = {}

    def fake_summarize_archive(
        archive: Path,
        team_slug: str,
        backend: str,
        model: str,
        **kwargs: object,
    ) -> SummarizeReport:
        seen.update(
            archive=archive,
            team_slug=team_slug,
            backend=backend,
            model=model,
            reasoning_effort=kwargs.get("reasoning_effort"),
            service_tier=kwargs.get("service_tier"),
        )
        return _report()

    monkeypatch.setattr(timeline_cli, "summarize_archive", fake_summarize_archive)
    parser = timeline_cli._parser()
    ns = parser.parse_args(
        [
            "summarize",
            "--output",
            str(tmp_path),
            "--team",
            "test-team",
            "--model",
            "model-under-test",
            "--reasoning-effort",
            "high",
            "--service-tier",
            "priority",
        ]
    )
    timeline_cli._summary_call(ns)
    assert seen["model"] == "model-under-test"
    assert seen["reasoning_effort"] == "high"
    assert seen["service_tier"] == "priority"
    default_ns = parser.parse_args(
        ["summarize", "--output", str(tmp_path), "--team", "test-team"]
    )
    assert default_ns.service_tier is None
