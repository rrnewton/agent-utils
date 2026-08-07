"""Contract tests for registered model-backed timeline computations."""

from __future__ import annotations

import pytest

from agent_team_timeline.summary_registry import (
    ContextComponent,
    ContextCoverage,
    PHASE_STYLE,
    PHASE_SUMMARIZER,
    SUMMARIZER_REGISTRY,
    registry_json_obj,
    summarizer_for_style,
)


def test_registry_has_unique_complete_current_contracts() -> None:
    assert len(SUMMARIZER_REGISTRY) == 6
    assert len({item.summarizer_id for item in SUMMARIZER_REGISTRY}) == 6
    assert len({item.summary_style for item in SUMMARIZER_REGISTRY}) == 6
    for spec in SUMMARIZER_REGISTRY:
        assert spec.current_version == spec.changelog[-1].version
        assert spec.prompt_version == spec.changelog[-1].prompt_version
        assert spec.input_fields
        assert spec.output_fields
        assert summarizer_for_style(spec.summary_style) is spec

    manifest = registry_json_obj()
    assert manifest["schema_version"] == 1
    summarizers = manifest["summarizers"]
    assert isinstance(summarizers, list)
    assert len(summarizers) == 6
    assert summarizer_for_style(PHASE_STYLE) is PHASE_SUMMARIZER


def test_unknown_summary_style_fails_closed() -> None:
    with pytest.raises(ValueError, match="unregistered summary style"):
        summarizer_for_style("future-unversioned-task")


def test_context_coverage_averages_channels_without_mixing_units() -> None:
    coverage = ContextCoverage(
        components=(
            ContextComponent("ancestor_transcript", 16_000, 8_000, "characters"),
            ContextComponent("prior_days", 10, 8, "summaries"),
        ),
        frontier_status="isolated-backfill",
        predecessor_keys=("daily:2026-08-05",),
    )

    assert coverage.coverage_basis_points == 6_500
    assert coverage.coverage_percent == 65
    assert coverage.missing_percent == 35
    assert coverage.to_json_obj()["frontier_status"] == "isolated-backfill"


def test_context_coverage_rejects_impossible_provenance() -> None:
    with pytest.raises(ValueError, match="must not exceed"):
        ContextComponent("prior_weeks", 4, 5, "summaries")
    with pytest.raises(ValueError, match="unique"):
        ContextCoverage(
            components=(
                ContextComponent("prior_days", 1, 1, "summaries"),
                ContextComponent("prior_days", 1, 1, "summaries"),
            )
        )
