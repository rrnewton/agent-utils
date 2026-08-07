from __future__ import annotations

import hashlib
import json
import threading
import urllib.request
from dataclasses import replace
from pathlib import Path

import pytest

from agent_team_timeline.archive import narrow_json, write_json_if_changed
from agent_team_timeline.artifacts import extract_artifacts
from agent_team_timeline.cli import main as timeline_main
from agent_team_timeline.github_enrich import pull_metadata_path
from agent_team_timeline.github_metadata import (
    PullRequestKey,
    PullRequestMetadata,
    PullRequestMetadataCache,
    save_pull_request_metadata_cache,
)
from agent_team_timeline.identity import HostIdentity, ProjectIdentity, SiteIdentity
from agent_team_timeline.model import (
    Agent,
    Edge,
    Event,
    SourceSnapshot,
    TeamData,
    ToolCall,
    Turn,
    source_digest,
)
from agent_team_timeline.multi_team import build_combined_archive
from agent_team_timeline.periods import Period, periods_for_range
from agent_team_timeline.phases import PhaseStats, PhaseWindow, build_phases
from agent_team_timeline.pipeline import (
    IngestReport,
    _agent_name_jobs,
    _definition_evidence,
    _glossary_terms,
    _load_agent_names,
    _root_overview_input,
    _rollup_jobs_for_level,
    build_archive,
    load_archived_team,
    record_run,
    summarize_archive,
)
from agent_team_timeline.server import make_server
from agent_team_timeline.summarize import PLAIN_LANGUAGE_ROLLUP_STYLE, SummaryResult
from agent_team_timeline.terminology import GlossaryTerm, glossary_term_id
from agent_team_timeline.window import DateWindow


ROOT = "00000000-0000-0000-0000-000000000001"
CHILD = "00000000-0000-0000-0000-000000000002"
NESTED = "00000000-0000-0000-0000-000000000003"
START = 1_775_000_000_000


def _event(
    event_id: str,
    thread: str,
    offset: int,
    kind: str,
    text: str | None,
    *,
    phase: str | None = None,
) -> Event:
    return Event(
        event_id=event_id,
        thread_id=thread,
        turn_id="turn-" + thread[-1],
        timestamp_ms=START + offset,
        kind=kind,
        role="user" if kind == "user_prompt" else "assistant",
        phase=phase,
        text=text,
        content_availability="plaintext" if text else "none",
        encrypted_content=None,
        author=None,
        recipient=None,
        source_line=1,
    )


def _team(extra_root_text: str = "") -> TeamData:
    events = (
        _event(
            "prompt-1",
            ROOT,
            1_000,
            "user_prompt",
            "Investigate the safe-landing protocol and keep exact-head validation terminology.",
        ),
        _event(
            "root-response",
            ROOT,
            3_000,
            "assistant_message",
            "The coordinator assigned a focused safe-landing audit. " + extra_root_text,
        ),
        _event(
            "child-update",
            CHILD,
            8_000,
            "assistant_message",
            "Found that exact-head validation was not bound to the release receipt.",
        ),
        _event(
            "child-final",
            CHILD,
            18_000,
            "assistant_message",
            "Added receipt binding and proved six negative cases plus two positive cases.",
            phase="final_answer",
        ),
        _event(
            "root-final",
            ROOT,
            22_000,
            "assistant_message",
            "Receipt binding is complete with eight focused cases passing.",
        ),
    )
    return TeamData(
        team_slug="codex-test",
        provider="codex",
        root_thread_id=ROOT,
        display_timezone="America/New_York",
        sources=(SourceSnapshot("root.jsonl", ROOT, 100, 1, "a" * 64, 100, 10),),
        agents=(
            Agent(ROOT, None, "/root", None, None, 0, START, START + 23_000, "completed", "root"),
            Agent(
                CHILD,
                ROOT,
                "/root/release_receipt_audit",
                "Ada",
                None,
                1,
                START + 5_000,
                START + 19_000,
                "completed",
                "child",
            ),
        ),
        turns=(
            Turn("turn-1", ROOT, START, START + 23_000, "completed", 10, None, None),
            Turn("turn-2", CHILD, START + 5_000, START + 19_000, "completed", 10, None, None),
        ),
        events=events,
        tool_calls=(
            ToolCall(
                "tool-1",
                "item-1",
                CHILD,
                "turn-2",
                "exec",
                None,
                START + 10_000,
                START + 12_000,
                "completed",
                None,
                None,
                (("bash", 2), ("git", 1)),
                2,
            ),
        ),
        edges=(
            Edge(
                "spawn-1",
                "spawn-1",
                ROOT,
                CHILD,
                "spawn",
                START + 5_000,
                None,
                "encrypted",
                "gAAAA-test",
                3,
            ),
        ),
    )


def _write_team(archive: Path, team: TeamData) -> None:
    write_json_if_changed(
        archive / "teams" / team.team_slug / "raw" / "team.json",
        narrow_json(team.to_json_obj()),
    )


def test_knowledge_evidence_keeps_prior_context_but_excludes_post_window_events() -> None:
    team = _team()
    future = _event(
        "future-root",
        ROOT,
        30_000,
        "assistant_message",
        "FUTURE_ONLY_MARKER discussed exact-head after the selected day.",
    )
    bounded = replace(
        team,
        events=team.events + (future,),
        window_start_ms=START + 7_000,
        window_end_ms=START + 30_000,
    )

    overview = _root_overview_input(bounded)
    exact_head = next(term for term in _glossary_terms(bounded) if term.term == "exact-head")
    evidence = _definition_evidence(bounded, exact_head)

    assert "prompt-1" in overview.event_ids
    assert "future-root" not in overview.event_ids
    assert "Investigate the safe-landing protocol" in overview.transcript
    assert "FUTURE_ONLY_MARKER" not in overview.transcript
    assert evidence
    assert all(item.event_id != "future-root" for item in evidence)
    assert all("FUTURE_ONLY_MARKER" not in item.context for item in evidence)


def test_unversioned_legacy_glossary_is_regenerated(tmp_path: Path) -> None:
    team = _team()
    _write_team(tmp_path, team)
    glossary_path = (
        tmp_path
        / "teams"
        / team.team_slug
        / "summary_data"
        / "glossary.json"
    )
    write_json_if_changed(glossary_path, narrow_json({"terms": []}))

    summarize_archive(tmp_path, team.team_slug, "heuristic", "test-model")

    glossary = json.loads(glossary_path.read_text(encoding="utf-8"))
    assert glossary["schema_version"] == 3
    assert glossary["project_overview_epoch_id"].startswith("knowledge-")


def test_cached_pipeline_builds_self_contained_site_idempotently(tmp_path: Path) -> None:
    team = _team()
    _write_team(tmp_path, team)
    first = summarize_archive(
        tmp_path,
        team.team_slug,
        "heuristic",
        "test-model",
        reasoning_effort="high",
    )
    built = build_archive(tmp_path, team.team_slug)

    assert first.agent_names == 2
    assert first.project_overviews == 1
    assert first.glossary_definitions == first.glossary_terms
    assert first.cache_misses == (
        first.phases
        + (2 * first.rollups)
        + first.agent_names
        + first.project_overviews
        + first.glossary_definitions
    )
    assert first.catalog_artifacts == first.cache_misses
    assert first.backend == "heuristic"
    assert first.model == "test-model"
    assert first.reasoning_effort == "high"
    assert first.service_tier is None
    assert first.newly_spent_usage.total_tokens == 0
    assert first.newly_spent_unknown_receipts == 0
    assert first.artifact_generation_unknown_receipts == 0
    assert first.unknown_legacy_artifacts == 0
    assert len(first.usage_run_paths) == (
        (2 * first.rollups) + 3 + int(first.glossary_definitions > 0)
    )
    assert all((tmp_path / path).is_file() for path in first.usage_run_paths)
    assert built["agents"] == 2
    assert (tmp_path / "index.html").is_file()
    assert (tmp_path / "timeline-core.js").is_file()
    markdown_bundle = tmp_path / "vendor" / "markdown-it-15.0.0.min.js"
    assert hashlib.sha256(markdown_bundle.read_bytes()).hexdigest() == (
        "8d0f6aca8f4de3321b6d07e03286176c59ec19b7b84abb6eb31f0fa795e83abc"
    )
    index_text = (tmp_path / "index.html").read_text(encoding="utf-8")
    assert index_text.index("markdown-it-15.0.0.min.js") < index_text.index("timeline-core.js")
    assert index_text.index("timeline-core.js") < index_text.index("app.js")
    generated_makefile = (tmp_path / "Makefile").read_text(encoding="utf-8")
    assert generated_makefile.startswith(".PHONY: serve")
    assert "run-stats:\n\tpython3 run_stats.py\n" in generated_makefile
    assert (tmp_path / "run_stats.py").is_file()
    assert (tmp_path / "run_stats.py").stat().st_mode & 0o111
    timeline = json.loads((tmp_path / "data" / "timeline.json").read_text(encoding="utf-8"))
    assert len(timeline["agents"]) == 2
    child_track = next(agent for agent in timeline["agents"] if agent["id"] == CHILD)
    assert child_track["short_name"] == "Release receipt audit"
    assert child_track["official_name"] == "/root/release_receipt_audit"
    assert child_track["official_leaf"] == "release_receipt_audit"
    assert child_track["nickname"] == "Ada"
    assert "receipt binding" in child_track["lifetime_summary"]
    summary_prose = " ".join(
        [child_track["lifetime_summary"]]
        + [
            value
            for phase in timeline["phases"]
            for value in (phase["phrase"], phase["paragraph"])
        ]
    )
    assert "ASSISTANT:" not in summary_prose
    assert "TOOLS:" not in summary_prose
    assert "[2026-" not in summary_prose
    assert timeline["source_digest"] == source_digest(team)
    assert timeline["display_timezone_source"] == "legacy_team_data"
    assert timeline["teams"][0]["projects"] == []
    assert timeline["teams"][0]["hosts"] == []
    assert any(edge["kind"] == "spawn" for edge in timeline["edges"])
    result_edges = [edge for edge in timeline["edges"] if edge["kind"] == "result"]
    assert len(result_edges) == 1
    assert result_edges[0]["source_id"] == CHILD
    assert result_edges[0]["target_id"] == ROOT
    assert result_edges[0]["source_ms"] == child_track["end_ms"]
    assert result_edges[0]["target_ms"] == child_track["end_ms"]
    detail = json.loads(
        next((tmp_path / "data" / "details").glob("*.json")).read_text(
            encoding="utf-8"
        )
    )
    assert all("at_ms" in entry and "role" in entry for entry in detail["transcript"])
    rollup = timeline["rollups"][0]
    assert (tmp_path / rollup["technical_path"]).is_file()
    assert (tmp_path / rollup["plain_language_path"]).is_file()
    assert "plain-language summary" in (tmp_path / rollup["plain_language_path"]).read_text(
        encoding="utf-8"
    )
    rollup_record = json.loads(
        next(
            (tmp_path / "teams" / team.team_slug / "summary_data" / "rollups").rglob(
                "*.json"
            )
        ).read_text(encoding="utf-8")
    )
    assert rollup_record["schema_version"] == 2
    assert rollup_record["technical_summary"]["prompt_version"].endswith(
        "technical-rollup-v2"
    )
    assert rollup_record["plain_language_summary"]["prompt_version"].endswith(
        "plain-rollup-v2"
    )
    overview_record = json.loads(
        (
            tmp_path
            / "teams"
            / team.team_slug
            / "summary_data"
            / "project_overview.json"
        ).read_text(encoding="utf-8")
    )
    assert overview_record["schema_version"] == 3
    assert overview_record["knowledge_epoch"]["cutoff_reason"] == (
        "first-summary-source-frontier"
    )
    assert "transcript" not in overview_record["source"]
    assert overview_record["summary"]["prompt_version"].endswith(
        "project-overview-v2"
    )
    assert overview_record["summary"]["phrase"] == "Insufficient evidence"
    assert overview_record["summary"]["artifact_provenance"]["model"] == (
        "test-model"
    )
    summary_catalog = json.loads(
        (
            tmp_path
            / "teams"
            / team.team_slug
            / "summary_data"
            / "artifacts.json"
        ).read_text(encoding="utf-8")
    )
    assert summary_catalog["artifact_count"] == first.catalog_artifacts
    assert summary_catalog["logical_key_count"] == first.catalog_artifacts
    assert summary_catalog["model_counts"] == {"test-model": first.catalog_artifacts}
    assert all(
        (tmp_path / "teams" / team.team_slug / "summary_data" / item["cache_path"])
        .is_file()
        for item in summary_catalog["artifacts"]
    )
    glossary_record = json.loads(
        (
            tmp_path
            / "teams"
            / team.team_slug
            / "summary_data"
            / "glossary.json"
        ).read_text(encoding="utf-8")
    )
    assert glossary_record["schema_version"] == 3
    assert glossary_record["project_overview_input_hash"] == overview_record["summary"][
        "input_hash"
    ]
    assert all(
        item["definition_status"] == "insufficient-evidence"
        and item["definition"].startswith("Insufficient evidence:")
        and item["evidence"]
        and item["available_at_ms"] >= item["introduced_at_ms"]
        and item["definition_epoch_id"].startswith("definition-")
        for item in glossary_record["terms"]
    )
    assert timeline["glossary_path"].endswith("codex-test-glossary.md")
    assert all(item["url"] == "#glossary/" + item["id"] for item in timeline["glossary"])
    assert all("definition" in item for item in timeline["glossary"])
    assert timeline["project_overview"]["evidence_status"] == "insufficient-evidence"
    glossary_catalog = tmp_path / timeline["glossary_path"]
    catalog_text = glossary_catalog.read_text(encoding="utf-8")
    assert catalog_text.index("## Project overview") < catalog_text.index(
        "## Project terms"
    )
    assert "### Definition" in catalog_text
    assert "### First-use evidence" in catalog_text
    assert timeline["events"].count(
        {"agent_id": CHILD, "at_ms": START + 10_000, "kind": "tool_call"}
    ) == 3
    detail_path = tmp_path / timeline["phases"][0]["detail_path"]
    assert detail_path.is_file()
    name_path = (
        tmp_path
        / "teams"
        / team.team_slug
        / "summary_data"
        / "agents"
        / f"{CHILD}.json"
    )
    name_record = json.loads(name_path.read_text(encoding="utf-8"))
    assert name_record["schema_version"] == 3
    assert name_record["agent"]["official_path"] == "/root/release_receipt_audit"
    assert name_record["name"]["short_name"] == "Release receipt audit"
    assert "receipt binding" in name_record["name"]["lifetime_summary"]
    agent_markdown = (
        tmp_path
        / "teams"
        / team.team_slug
        / "summaries"
        / "agents"
        / f"{CHILD}.md"
    ).read_text(encoding="utf-8")
    assert "## Lifetime summary" in agent_markdown
    assert name_record["name"]["lifetime_summary"] in agent_markdown

    second = summarize_archive(
        tmp_path,
        team.team_slug,
        "heuristic",
        "test-model",
        reasoning_effort="high",
    )
    rebuilt = build_archive(tmp_path, team.team_slug)
    assert second.cache_misses == 0
    assert second.cache_hits == first.cache_misses
    assert second.files_changed == 0
    assert second.newly_spent_usage.total_tokens == 0
    assert second.artifact_generation_usage == first.artifact_generation_usage
    assert rebuilt["files_changed"] == 0

    run_path = record_run(
        tmp_path,
        ("agent-team-timeline", "summarize"),
        "2026-08-05T00:00:00Z",
        "completed",
        team.team_slug,
        None,
        second,
        None,
    )
    run = json.loads(run_path.read_text(encoding="utf-8"))
    assert run["summaries"]["reasoning_effort"] == "high"
    assert run["summaries"]["service_tier"] is None
    assert run["summaries"]["newly_spent_usage"]["total_tokens"] == 0
    assert run["summaries"]["usage_run_paths"] == list(second.usage_run_paths)


def test_append_catchup_keeps_completed_historical_knowledge_jobs_stable(
    tmp_path: Path,
) -> None:
    team = _team()
    _write_team(tmp_path, team)
    summarize_archive(tmp_path, team.team_slug, "heuristic", "test-model")

    daily_dir = (
        tmp_path
        / "teams"
        / team.team_slug
        / "summary_data"
        / "rollups"
        / "daily"
    )
    original_daily_path = next(daily_dir.glob("*.json"))
    original_daily = json.loads(original_daily_path.read_text(encoding="utf-8"))
    overview_path = (
        tmp_path
        / "teams"
        / team.team_slug
        / "summary_data"
        / "project_overview.json"
    )
    glossary_path = (
        tmp_path
        / "teams"
        / team.team_slug
        / "summary_data"
        / "glossary.json"
    )
    original_overview = json.loads(overview_path.read_text(encoding="utf-8"))
    original_glossary = json.loads(glossary_path.read_text(encoding="utf-8"))
    original_definitions = {
        item["term_id"]: item["definition_summary"]["input_hash"]
        for item in original_glossary["terms"]
    }

    later_offset = 2 * 24 * 60 * 60 * 1000
    late_events = (
        _event(
            "late-dbi-first",
            ROOT,
            later_offset,
            "user_prompt",
            "Investigate DBI behavior without changing earlier summaries.",
        ),
        _event(
            "late-dbi-second",
            ROOT,
            later_offset + 1_000,
            "user_prompt",
            "Use DBI consistently in this newly active workstream.",
        ),
        _event(
            "late-dbi-response",
            ROOT,
            later_offset + 2_000,
            "assistant_message",
            "DBI work belongs only to the later calendar period.",
        ),
    )
    later_end = START + later_offset + 3_000
    updated_agents = tuple(
        replace(agent, ended_at_ms=later_end)
        if agent.thread_id == ROOT
        else agent
        for agent in team.agents
    )
    updated_turns = tuple(
        replace(turn, ended_at_ms=later_end)
        if turn.thread_id == ROOT
        else turn
        for turn in team.turns
    )
    appended = replace(
        team,
        agents=updated_agents,
        turns=updated_turns,
        events=team.events + late_events,
    )
    _write_team(tmp_path, appended)

    summarize_archive(tmp_path, team.team_slug, "heuristic", "test-model")

    refreshed_daily = json.loads(original_daily_path.read_text(encoding="utf-8"))
    refreshed_overview = json.loads(overview_path.read_text(encoding="utf-8"))
    refreshed_glossary = json.loads(glossary_path.read_text(encoding="utf-8"))
    refreshed_definitions = {
        item["term_id"]: item["definition_summary"]["input_hash"]
        for item in refreshed_glossary["terms"]
    }
    dbi = next(item for item in refreshed_glossary["terms"] if item["term"] == "DBI")

    assert refreshed_daily["technical_summary"]["input_hash"] == (
        original_daily["technical_summary"]["input_hash"]
    )
    assert refreshed_daily["plain_language_summary"]["input_hash"] == (
        original_daily["plain_language_summary"]["input_hash"]
    )
    assert refreshed_overview["knowledge_epoch"] == original_overview["knowledge_epoch"]
    assert refreshed_overview["source"] == original_overview["source"]
    assert refreshed_overview["summary"]["input_hash"] == (
        original_overview["summary"]["input_hash"]
    )
    assert all(
        refreshed_definitions[term_id] == input_hash
        for term_id, input_hash in original_definitions.items()
    )
    assert dbi["available_at_ms"] == START + later_offset + 1_000
    assert dbi["available_at_ms"] >= original_daily["end_ms"]


def test_frozen_overview_rejects_historical_source_mutation(tmp_path: Path) -> None:
    team = _team()
    _write_team(tmp_path, team)
    summarize_archive(tmp_path, team.team_slug, "heuristic", "test-model")
    changed_events = tuple(
        replace(event, text="Historically rewritten coordinator response.")
        if event.event_id == "root-response"
        else event
        for event in team.events
    )
    _write_team(tmp_path, replace(team, events=changed_events))

    with pytest.raises(ValueError, match="overview evidence was mutated or truncated"):
        summarize_archive(tmp_path, team.team_slug, "heuristic", "test-model")


def test_frozen_definition_rejects_historical_source_mutation(tmp_path: Path) -> None:
    team = _team()
    _write_team(tmp_path, team)
    summarize_archive(tmp_path, team.team_slug, "heuristic", "test-model")
    changed_events = tuple(
        replace(event, text="Rewritten child evidence no longer names the term.")
        if event.event_id == "child-update"
        else event
        for event in team.events
    )
    _write_team(tmp_path, replace(team, events=changed_events))

    with pytest.raises(ValueError, match="frozen source was mutated or truncated"):
        summarize_archive(tmp_path, team.team_slug, "heuristic", "test-model")


def test_build_embeds_standalone_site_identity(tmp_path: Path) -> None:
    team = _team()
    _write_team(tmp_path, team)
    identity = SiteIdentity(
        team.team_slug,
        (
            ProjectIdentity(
                "dev-hermit",
                "https://github.com/rrnewton/dev-hermit",
                True,
                "session_metadata",
            ),
        ),
        (HostIdentity("devbig014", "explicit"),),
        team.display_timezone,
        "explicit",
    )
    write_json_if_changed(
        tmp_path / "teams" / team.team_slug / "raw" / "site-identity.json",
        narrow_json(identity.to_json_obj()),
    )
    summarize_archive(tmp_path, team.team_slug, "heuristic", "test-model")
    build_archive(tmp_path, team.team_slug)

    timeline = json.loads(
        (tmp_path / "data" / "timeline.json").read_text(encoding="utf-8")
    )
    assert timeline["display_timezone"] == "America/New_York"
    assert timeline["display_timezone_source"] == "explicit"
    assert timeline["teams"] == [
        {
            "slug": team.team_slug,
            "label": team.team_slug,
            "projects": [identity.projects[0].to_json_obj()],
            "hosts": [identity.hosts[0].to_json_obj()],
        }
    ]


def test_phase_details_emit_conservative_pull_request_link_spans(tmp_path: Path) -> None:
    team = _team(
        "Reviewed https://github.com/rrnewton/dev-hermit/pull/38 and "
        "sched-ext/scx#3668; naked #7 is ambiguous."
    )
    _write_team(tmp_path, team)
    summarize_archive(tmp_path, team.team_slug, "heuristic", "test-model")
    build_archive(tmp_path, team.team_slug)

    detail_objects = [
        json.loads(path.read_text(encoding="utf-8"))
        for path in (tmp_path / "data" / "details").glob("*.json")
    ]
    entry = next(
        transcript_entry
        for detail in detail_objects
        for transcript_entry in detail["transcript"]
        if "dev-hermit/pull/38" in transcript_entry["text"]
    )
    references = entry["pull_requests"]
    assert [reference["repository"] for reference in references] == [
        "rrnewton/dev-hermit",
        "sched-ext/scx",
    ]
    assert [reference["number"] for reference in references] == [38, 3668]
    assert all(
        entry["text"][reference["start"] : reference["end"]] == reference["text"]
        for reference in references
    )
    assert all(reference["text"] != "#7" for reference in references)

    pull = PullRequestMetadata(
        key=PullRequestKey("rrnewton/dev-hermit", 38),
        title="Repair archive refresh",
        state="closed",
        draft=False,
        merged_at="2026-08-05T10:00:00Z",
        body_excerpt="Makes refresh append-safe.",
        base_ref="main",
        head_label="rrnewton:archive-refresh",
        author="rrnewton",
        updated_at="2026-08-05T10:00:00Z",
        etag='W/"pull-38"',
        fetched_at="2026-08-05T11:00:00Z",
    )
    save_pull_request_metadata_cache(
        pull_metadata_path(tmp_path, team.team_slug),
        PullRequestMetadataCache((pull,)),
    )
    build_archive(tmp_path, team.team_slug)
    enriched_details = [
        json.loads(path.read_text(encoding="utf-8"))
        for path in (tmp_path / "data" / "details").glob("*.json")
    ]
    enriched_entry = next(
        transcript_entry
        for detail in enriched_details
        for transcript_entry in detail["transcript"]
        if "dev-hermit/pull/38" in transcript_entry["text"]
    )
    assert enriched_entry["pull_requests"][0]["title"] == "Repair archive refresh"
    assert enriched_entry["pull_requests"][0]["merged_at"] == "2026-08-05T10:00:00Z"


def test_reused_subagent_gets_one_structural_lifetime_result(tmp_path: Path) -> None:
    team = _team()
    second_final = _event(
        "child-final-again",
        CHILD,
        19_000,
        "assistant_message",
        "A resumed follow-up independently confirmed the receipt binding.",
        phase="final_answer",
    )
    updated = replace(team, events=team.events + (second_final,))
    _write_team(tmp_path, updated)
    summarize_archive(tmp_path, updated.team_slug, "heuristic", "test-model")
    build_archive(tmp_path, updated.team_slug)
    timeline = json.loads((tmp_path / "data" / "timeline.json").read_text(encoding="utf-8"))
    result_edges = [edge for edge in timeline["edges"] if edge["kind"] == "result"]
    assert len(result_edges) == 1
    lifetime_result = result_edges[0]
    assert lifetime_result["id"] == f"result-{CHILD}"
    assert lifetime_result["source_id"] == CHILD
    assert lifetime_result["target_id"] == ROOT
    assert lifetime_result["source_ms"] == START + 19_000
    assert lifetime_result["target_ms"] == START + 19_000
    assert lifetime_result["phrase"] == "Release receipt audit returns to Coordinator"
    assert lifetime_result["full_text"] == ""
    assert lifetime_result["content_status"].startswith("Structural lifetime join.")
    turn_results = {
        edge["id"]: edge
        for edge in timeline["edges"]
        if edge["id"].startswith("turn-result-")
    }
    assert set(turn_results) == {
        "turn-result-child-final",
        "turn-result-child-final-again",
    }
    assert {edge["kind"] for edge in turn_results.values()} == {"message"}
    assert {edge["target_id"] for edge in turn_results.values()} == {ROOT}


@pytest.mark.parametrize(
    ("status", "ended_at_ms", "expected_results"),
    (("interrupted", START + 19_000, 1), ("running", None, 0)),
)
def test_only_ended_agent_lifetimes_get_a_structural_join(
    tmp_path: Path,
    status: str,
    ended_at_ms: int | None,
    expected_results: int,
) -> None:
    team = _team()
    child = replace(team.agents[1], status=status, ended_at_ms=ended_at_ms)
    updated = replace(team, agents=(team.agents[0], child))
    _write_team(tmp_path, updated)
    summarize_archive(tmp_path, updated.team_slug, "heuristic", "test-model")
    build_archive(tmp_path, updated.team_slug)

    timeline = json.loads((tmp_path / "data" / "timeline.json").read_text(encoding="utf-8"))
    result_edges = [edge for edge in timeline["edges"] if edge["kind"] == "result"]
    assert len(result_edges) == expected_results
    if expected_results:
        assert result_edges[0]["source_id"] == CHILD
        assert result_edges[0]["target_id"] == ROOT
        assert result_edges[0]["phrase"].endswith(f"({status})")


def test_resumed_nested_agent_joins_parent_while_turn_results_reach_initiators(
    tmp_path: Path,
) -> None:
    team = _team()
    nested_initial = replace(
        _event(
            "nested-initial-final",
            NESTED,
            11_000,
            "assistant_message",
            "The nested agent completed its initial audit for its parent.",
            phase="final_answer",
        ),
        turn_id="nested-initial",
    )
    nested_resumed = replace(
        _event(
            "nested-resumed-final",
            NESTED,
            28_000,
            "assistant_message",
            "The root coordinator resumed the nested agent after its parent finished.",
            phase="final_answer",
        ),
        turn_id="nested-resumed",
    )
    agents = (
        replace(team.agents[0], ended_at_ms=START + 30_000),
        team.agents[1],
        Agent(
            NESTED,
            CHILD,
            "/root/release_receipt_audit/nested_review",
            "Emmy",
            None,
            2,
            START + 6_000,
            START + 29_000,
            "completed",
            "nested",
        ),
    )
    turns = team.turns + (
        Turn(
            "nested-initial",
            NESTED,
            START + 6_000,
            START + 12_000,
            "completed",
            10,
            None,
            None,
        ),
        Turn(
            "nested-resumed",
            NESTED,
            START + 24_000,
            START + 29_000,
            "completed",
            10,
            None,
            None,
        ),
    )
    edges = team.edges + (
        Edge(
            "spawn-nested",
            "spawn-nested",
            CHILD,
            NESTED,
            "spawn",
            START + 6_400,
            None,
            "encrypted",
            "gAAAA-nested",
            4,
        ),
        Edge(
            "resume-nested",
            "resume-nested",
            ROOT,
            NESTED,
            "followup",
            START + 24_600,
            None,
            "encrypted",
            "gAAAA-resume",
            5,
        ),
    )
    updated = replace(
        team,
        agents=agents,
        turns=turns,
        events=team.events + (nested_initial, nested_resumed),
        edges=edges,
    )
    _write_team(tmp_path, updated)
    summarize_archive(tmp_path, updated.team_slug, "heuristic", "test-model")
    build_archive(tmp_path, updated.team_slug)

    timeline = json.loads((tmp_path / "data" / "timeline.json").read_text(encoding="utf-8"))
    result_edges = {
        edge["id"]: edge["target_id"]
        for edge in timeline["edges"]
        if edge["kind"] == "result"
    }
    assert result_edges[f"result-{NESTED}"] == CHILD
    turn_result_targets = {
        edge["id"]: edge["target_id"]
        for edge in timeline["edges"]
        if edge["id"].startswith("turn-result-nested-")
    }
    assert turn_result_targets == {
        "turn-result-nested-initial-final": CHILD,
        "turn-result-nested-resumed-final": ROOT,
    }
    nested_track = next(agent for agent in timeline["agents"] if agent["id"] == NESTED)
    assert nested_track["parent_id"] == CHILD
    assert nested_track["end_ms"] > team.agents[1].ended_at_ms


def test_team_slug_and_archived_identity_cannot_escape_archive(tmp_path: Path) -> None:
    with pytest.raises(ValueError, match="team slug"):
        load_archived_team(tmp_path, "../../outside")

    team = replace(_team(), team_slug="other-team")
    target = tmp_path / "teams" / "codex-test" / "raw" / "team.json"
    write_json_if_changed(target, narrow_json(team.to_json_obj()))
    with pytest.raises(ValueError, match="does not match requested"):
        load_archived_team(tmp_path, "codex-test")


def test_build_refuses_to_clobber_non_archive_directory(tmp_path: Path) -> None:
    project = tmp_path / "real-project"
    project.mkdir()
    readme = project / "README.md"
    makefile = project / "Makefile"
    readme.write_text("valuable project readme\n", encoding="utf-8")
    makefile.write_text("all:\n\t@true\n", encoding="utf-8")

    with pytest.raises(ValueError, match="refusing non-empty non-archive"):
        build_archive(project, "codex-test")
    assert readme.read_text(encoding="utf-8") == "valuable project readme\n"
    assert makefile.read_text(encoding="utf-8") == "all:\n\t@true\n"


def test_build_only_run_preserves_source_digest_and_team_history(tmp_path: Path) -> None:
    ingest = IngestReport(
        team_slug="codex-test",
        source_digest="a" * 64,
        sources=1,
        source_bytes=10,
        agents=1,
        events=2,
        tool_calls=3,
        edges=1,
        files_changed=1,
    )
    record_run(
        tmp_path,
        ("agent-team-timeline", "refresh"),
        "2026-08-05T00:00:00Z",
        "completed",
        "codex-test",
        ingest,
        None,
        None,
    )
    record_run(
        tmp_path,
        ("agent-team-timeline", "build"),
        "2026-08-05T01:00:00Z",
        "completed",
        "other-team",
        None,
        None,
        {"files_changed": 0},
    )
    manifest = json.loads((tmp_path / "manifest.json").read_text(encoding="utf-8"))
    assert manifest["latest_source_digest"] == "a" * 64
    assert manifest["teams"] == ["codex-test", "other-team"]
    assert manifest["run_count"] == 2


def _two_day_team(child_result: str) -> TeamData:
    base = _team()
    changed_events = tuple(
        replace(event, text=child_result) if event.event_id == "child-final" else event
        for event in base.events
    )
    day = 26 * 60 * 60 * 1000
    later_events = (
        _event("day-two-prompt", ROOT, day, "user_prompt", "Verify the archived report."),
        _event(
            "day-two-response",
            ROOT,
            day + 2_000,
            "assistant_message",
            "The archived report remains verified.",
        ),
    )
    agents = tuple(
        replace(agent, ended_at_ms=START + day + 3_000)
        if agent.thread_id == ROOT
        else agent
        for agent in base.agents
    )
    return replace(base, agents=agents, events=changed_events + later_events)


def _daily_hashes(archive: Path) -> list[str]:
    paths = sorted(
        (archive / "teams" / "codex-test" / "summary_data" / "rollups" / "daily").glob(
            "*.json"
        )
    )
    return [
        json.loads(path.read_text(encoding="utf-8"))["technical_summary"]["input_hash"]
        for path in paths
    ]


def _daily_plain_hashes(archive: Path) -> list[str]:
    paths = sorted(
        (archive / "teams" / "codex-test" / "summary_data" / "rollups" / "daily").glob(
            "*.json"
        )
    )
    return [
        json.loads(path.read_text(encoding="utf-8"))["plain_language_summary"][
            "input_hash"
        ]
        for path in paths
    ]


def test_later_daily_rollup_hash_includes_prior_daily_summary(tmp_path: Path) -> None:
    first_team = _two_day_team(
        "Implemented alpha receipt binding; 101 focused tests passed and released the benchmark."
    )
    _write_team(tmp_path, first_team)
    summarize_archive(tmp_path, first_team.team_slug, "heuristic", "test-model")
    first = _daily_hashes(tmp_path)
    first_plain = _daily_plain_hashes(tmp_path)
    assert len(first) >= 2
    assert len(first_plain) == len(first)

    changed_team = _two_day_team(
        "Implemented beta scheduler isolation; 909 focused tests passed and released the benchmark."
    )
    _write_team(tmp_path, changed_team)
    summarize_archive(tmp_path, changed_team.team_slug, "heuristic", "test-model")
    second = _daily_hashes(tmp_path)
    second_plain = _daily_plain_hashes(tmp_path)
    assert first[0] != second[0]
    assert first[1] != second[1]
    assert first_plain[0] != second_plain[0]
    assert first_plain[1] != second_plain[1]


def test_isolated_day_backfill_loads_prior_day_from_artifact_catalog(
    tmp_path: Path,
) -> None:
    team = _two_day_team(
        "Implemented exact receipt binding and verified the focused tests."
    )
    _write_team(tmp_path, team)
    event_start = min(event.timestamp_ms for event in team.events)
    event_end = max(event.timestamp_ms for event in team.events)
    days = [
        period
        for period in periods_for_range(
            event_start,
            event_end,
            team.display_timezone,
            team.team_slug,
        )
        if period.kind == "daily"
    ]
    assert len(days) >= 2

    for period in days[:2]:
        summarize_archive(
            tmp_path,
            team.team_slug,
            "heuristic",
            "offline",
            summary_window=DateWindow(
                None,
                None,
                period.start_ms,
                period.end_ms,
            ),
            rollup_kinds=("daily",),
        )

    second_record = json.loads(
        (
            tmp_path
            / "teams"
            / team.team_slug
            / "summary_data"
            / "rollups"
            / "daily"
            / f"{days[1].key}.json"
        ).read_text(encoding="utf-8")
    )
    coverage = second_record["technical_summary"]["artifact_provenance"][
        "context_coverage"
    ]
    prior = next(
        item
        for item in coverage["components"]
        if item["name"] == "prior_same_level_summaries"
    )
    assert prior["requested"] == 1
    assert prior["provided"] == 1
    assert coverage["coverage_percent"] == 100
    assert coverage["frontier_status"] == "contiguous-extension"


def test_append_only_changed_window_and_rollups_invalidate(tmp_path: Path) -> None:
    team = _team()
    _write_team(tmp_path, team)
    first = summarize_archive(tmp_path, team.team_slug, "heuristic", "test-model")
    appended_event = _event(
        "root-followup",
        ROOT,
        25_000,
        "assistant_message",
        "A new root-cause sentence extends only the coordinator window.",
    )
    changed = replace(
        team,
        events=team.events + (appended_event,),
        agents=tuple(
            replace(agent, ended_at_ms=START + 26_000)
            if agent.thread_id == ROOT
            else agent
            for agent in team.agents
        ),
        turns=tuple(
            replace(turn, ended_at_ms=START + 26_000)
            if turn.thread_id == ROOT
            else turn
            for turn in team.turns
        ),
    )
    _write_team(tmp_path, changed)
    second = summarize_archive(tmp_path, changed.team_slug, "heuristic", "test-model")
    assert 0 < second.cache_misses < first.cache_misses
    assert second.cache_hits > 0


def test_cross_spawn_context_reaches_child_phase() -> None:
    team = _team()
    child_phase = next(phase for phase in build_phases(team) if phase.agent_id == CHILD)
    assert "safe-landing protocol" in child_phase.prior_context
    assert "exact-head validation" in child_phase.prior_context


def test_hindsight_name_job_combines_phase_summary_with_parent_context() -> None:
    team = _team()
    phases = build_phases(team)
    results = {
        phase.summary_key: SummaryResult(
            key=phase.summary_key,
            phrase="Receipt binding audit",
            paragraph="Verified exact-head release receipt binding with eight focused cases.",
            work_summary=(),
            model="test-model",
            prompt_version="test-prompt",
            input_hash="a" * 64,
            generated_at="2026-08-05T00:00:00Z",
        )
        for phase in phases
    }

    jobs = _agent_name_jobs(team, phases, results)
    child = next(job for job in jobs if job.thread_id == CHILD)

    assert child.official_path == "/root/release_receipt_audit"
    assert child.parent_official_path == "/root"
    assert child.depth == 1
    assert "safe-landing protocol" in child.prior_context
    assert "exact-head release receipt binding" in child.work_summary


def test_summary_window_can_backfill_one_hour_without_other_rollup_levels(
    tmp_path: Path,
) -> None:
    team = _team()
    _write_team(tmp_path, team)
    window = DateWindow(
        start_date=None,
        end_date=None,
        start_ms=START,
        end_ms=START + 20_000,
        start_time="2026-04-01T00:00:00.000Z",
        end_time="2026-04-01T00:00:20.000Z",
    )

    report = summarize_archive(
        tmp_path,
        team.team_slug,
        "heuristic",
        "offline",
        summary_window=window,
        rollup_kinds=("hourly",),
    )

    assert report.rollups == 1
    rollup_root = tmp_path / "teams" / team.team_slug / "summary_data" / "rollups"
    assert len(list((rollup_root / "hourly").glob("*.json"))) == 1
    assert not (rollup_root / "daily").exists()
    phase_records = list(
        (tmp_path / "teams" / team.team_slug / "summary_data" / "phases").glob(
            "*.json"
        )
    )
    assert phase_records
    assert all(
        START <= json.loads(path.read_text(encoding="utf-8"))["start_ms"]
        < START + 20_000
        for path in phase_records
    )
    export = tmp_path / "hour-export"
    first_export = build_archive(
        tmp_path,
        team.team_slug,
        display_window=window,
        rollup_kinds=("hourly",),
        output=export,
    )
    second_export = build_archive(
        tmp_path,
        team.team_slug,
        display_window=window,
        rollup_kinds=("hourly",),
        output=export,
    )
    assert first_export["files_changed"] > 0
    assert second_export["files_changed"] == 0
    assert (export / ".agent-team-timeline.json").is_file()
    timeline = json.loads(
        (export / "data" / "timeline.json").read_text(encoding="utf-8")
    )
    assert {item["kind"] for item in timeline["rollups"]} == {"hourly"}
    assert all(
        START <= phase["start_ms"] < START + 20_000
        for phase in timeline["phases"]
    )


def test_build_without_summary_cache_uses_presentation_only_fallbacks(
    tmp_path: Path,
) -> None:
    team = _team()
    _write_team(tmp_path, team)
    window = DateWindow(
        start_date=None,
        end_date=None,
        start_ms=START,
        end_ms=START + 20_000,
        start_time="2026-03-31T23:33:20.000Z",
        end_time="2026-03-31T23:33:40.000Z",
    )
    output = tmp_path / "zero-summary-site"
    source_summary_root = tmp_path / "teams" / team.team_slug / "summary_data"

    first = build_archive(
        tmp_path,
        team.team_slug,
        display_window=window,
        rollup_kinds=("hourly",),
        output=output,
    )
    second = build_archive(
        tmp_path,
        team.team_slug,
        display_window=window,
        rollup_kinds=("hourly",),
        output=output,
    )

    assert first["files_changed"] > 0
    assert second["files_changed"] == 0
    assert not source_summary_root.exists()
    timeline = json.loads(
        (output / "data" / "timeline.json").read_text(encoding="utf-8")
    )
    assert timeline["phases"]
    assert {phase["phrase"] for phase in timeline["phases"]} == {
        "Summary unavailable"
    }
    assert all(
        "normalized logs" in phase["paragraph"] for phase in timeline["phases"]
    )
    assert all(
        "no cached hindsight name" in agent["naming_rationale"]
        for agent in timeline["agents"]
    )
    assert "no cached model summary" in timeline["project_overview"]["text"]
    assert {rollup["kind"] for rollup in timeline["rollups"]} == {"hourly"}
    for phase in timeline["phases"]:
        detail = json.loads(
            (output / phase["detail_path"]).read_text(encoding="utf-8")
        )
        assert detail["transcript"]
        assert detail["stats"] == phase["stats"]
        assert detail["phrase"] == "Summary unavailable"
    rollup = timeline["rollups"][0]
    assert "Summary unavailable" in (output / rollup["technical_path"]).read_text(
        encoding="utf-8"
    )


def test_build_preserves_available_phase_summaries_in_patchy_archive(
    tmp_path: Path,
) -> None:
    team = _team()
    _write_team(tmp_path, team)
    summarize_archive(tmp_path, team.team_slug, "heuristic", "offline")
    phases = build_phases(team, phase_minutes=30)
    assert len(phases) >= 2
    missing = phases[-1]
    summary_root = tmp_path / "teams" / team.team_slug / "summary_data"
    (summary_root / "phases" / f"{missing.phase_id}.json").unlink()
    (summary_root / "artifacts.json").unlink()

    output = tmp_path / "patchy-site"
    build_archive(tmp_path, team.team_slug, output=output)
    timeline = json.loads(
        (output / "data" / "timeline.json").read_text(encoding="utf-8")
    )
    phrases = {phase["id"]: phase["phrase"] for phase in timeline["phases"]}

    assert phrases[missing.phase_id] == "Summary unavailable"
    assert any(
        phrase != "Summary unavailable"
        for phase_id, phrase in phrases.items()
        if phase_id != missing.phase_id
    )


def test_build_recovers_compatible_paid_name_from_catalog(tmp_path: Path) -> None:
    team = _team()
    _write_team(tmp_path, team)
    summarize_archive(tmp_path, team.team_slug, "heuristic", "offline")
    summary_root = tmp_path / "teams" / team.team_slug / "summary_data"
    name_path = summary_root / "agents" / f"{CHILD}.json"
    overview_path = summary_root / "project_overview.json"
    expected_name = json.loads(name_path.read_text(encoding="utf-8"))["name"]
    name_path.unlink()
    overview_path.unlink()

    output = tmp_path / "catalog-recovery-site"
    build_archive(tmp_path, team.team_slug, output=output)
    timeline = json.loads(
        (output / "data" / "timeline.json").read_text(encoding="utf-8")
    )
    child = next(agent for agent in timeline["agents"] if agent["id"] == CHILD)
    assert child["short_name"] == expected_name["short_name"]
    assert child["lifetime_summary"] == expected_name["lifetime_summary"]
    assert "no cached model summary" in timeline["project_overview"]["text"]
    assert timeline["glossary"] == []


def test_build_does_not_recover_future_catalog_knowledge_into_slice(
    tmp_path: Path,
) -> None:
    team = _team()
    _write_team(tmp_path, team)
    summarize_archive(tmp_path, team.team_slug, "heuristic", "offline")
    window = DateWindow(
        start_date=None,
        end_date=None,
        start_ms=START,
        end_ms=START + 10_000,
        start_time="2026-03-31T23:33:20.000Z",
        end_time="2026-03-31T23:33:30.000Z",
    )

    output = tmp_path / "bounded-catalog-site"
    build_archive(
        tmp_path,
        team.team_slug,
        display_window=window,
        rollup_kinds=("hourly",),
        output=output,
    )
    timeline = json.loads(
        (output / "data" / "timeline.json").read_text(encoding="utf-8")
    )
    child = next(agent for agent in timeline["agents"] if agent["id"] == CHILD)
    assert "no cached hindsight name" in child["naming_rationale"]
    assert all(
        phase["phrase"] == "Summary unavailable" for phase in timeline["phases"]
    )
    assert all("final" not in phase["paragraph"].lower() for phase in timeline["phases"])
    assert all(
        "Summary unavailable"
        in (output / rollup["technical_path"]).read_text(encoding="utf-8")
        for rollup in timeline["rollups"]
    )
    assert "no cached model summary" in timeline["project_overview"]["text"]
    assert timeline["glossary"] == []


def test_build_validates_corrupt_partial_rollup_before_suppressing(
    tmp_path: Path,
) -> None:
    team = _team()
    _write_team(tmp_path, team)
    summarize_archive(
        tmp_path,
        team.team_slug,
        "heuristic",
        "offline",
        rollup_kinds=("hourly",),
    )
    period = periods_for_range(
        START,
        START + 9_999,
        team.display_timezone,
        team.team_slug,
        ("hourly",),
    )[0]
    rollup_path = (
        tmp_path
        / "teams"
        / team.team_slug
        / "summary_data"
        / "rollups"
        / "hourly"
        / f"{period.key}.json"
    )
    write_json_if_changed(rollup_path, {"schema_version": 2})
    window = DateWindow(
        start_date=None,
        end_date=None,
        start_ms=START,
        end_ms=START + 10_000,
        start_time="2026-03-31T23:33:20.000Z",
        end_time="2026-03-31T23:33:30.000Z",
    )

    with pytest.raises(ValueError, match="schema mismatch"):
        build_archive(
            tmp_path,
            team.team_slug,
            display_window=window,
            rollup_kinds=("hourly",),
            output=tmp_path / "corrupt-partial-rollup-site",
        )


def test_build_does_not_render_stale_partial_rollup_as_complete(
    tmp_path: Path,
) -> None:
    hour_ms = 60 * 60 * 1000
    hour_start = (START // hour_ms) * hour_ms
    hour_end = hour_start + hour_ms
    team = replace(
        _team(),
        window_start_ms=hour_start,
        window_end_ms=hour_end,
    )
    _write_team(tmp_path, team)
    summarize_archive(
        tmp_path,
        team.team_slug,
        "heuristic",
        "offline",
        rollup_kinds=("hourly",),
    )
    period = periods_for_range(
        hour_start,
        hour_end - 1,
        team.display_timezone,
        team.team_slug,
        ("hourly",),
    )[0]
    assert not period.partial
    rollup_path = (
        tmp_path
        / "teams"
        / team.team_slug
        / "summary_data"
        / "rollups"
        / "hourly"
        / f"{period.key}.json"
    )
    rollup = json.loads(rollup_path.read_text(encoding="utf-8"))
    rollup["partial"] = True
    write_json_if_changed(rollup_path, rollup)

    output = tmp_path / "stale-partial-rollup-site"
    build_archive(
        tmp_path,
        team.team_slug,
        rollup_kinds=("hourly",),
        output=output,
    )
    timeline = json.loads(
        (output / "data" / "timeline.json").read_text(encoding="utf-8")
    )
    assert len(timeline["rollups"]) == 1
    assert "Summary unavailable" in (
        output / timeline["rollups"][0]["technical_path"]
    ).read_text(encoding="utf-8")


def test_build_validates_out_of_window_overview_source(tmp_path: Path) -> None:
    team = _team()
    _write_team(tmp_path, team)
    summarize_archive(tmp_path, team.team_slug, "heuristic", "offline")
    overview_path = (
        tmp_path
        / "teams"
        / team.team_slug
        / "summary_data"
        / "project_overview.json"
    )
    overview = json.loads(overview_path.read_text(encoding="utf-8"))
    overview["source"]["context_sha256"] = "0" * 64
    write_json_if_changed(overview_path, overview)
    window = DateWindow(
        start_date=None,
        end_date=None,
        start_ms=START,
        end_ms=START + 10_000,
        start_time="2026-03-31T23:33:20.000Z",
        end_time="2026-03-31T23:33:30.000Z",
    )

    with pytest.raises(ValueError, match="mutated or truncated"):
        build_archive(
            tmp_path,
            team.team_slug,
            display_window=window,
            output=tmp_path / "corrupt-out-of-window-overview-site",
        )


def test_build_validates_out_of_window_overview_summary(tmp_path: Path) -> None:
    team = _team()
    _write_team(tmp_path, team)
    summarize_archive(tmp_path, team.team_slug, "heuristic", "offline")
    overview_path = (
        tmp_path
        / "teams"
        / team.team_slug
        / "summary_data"
        / "project_overview.json"
    )
    overview = json.loads(overview_path.read_text(encoding="utf-8"))
    overview["summary"]["phrase"] = "corrupt evidence status"
    write_json_if_changed(overview_path, overview)
    window = DateWindow(
        start_date=None,
        end_date=None,
        start_ms=START,
        end_ms=START + 10_000,
        start_time="2026-03-31T23:33:20.000Z",
        end_time="2026-03-31T23:33:30.000Z",
    )

    with pytest.raises(ValueError, match="invalid project-overview evidence status"):
        build_archive(
            tmp_path,
            team.team_slug,
            display_window=window,
            output=tmp_path / "corrupt-out-of-window-summary-site",
        )


def test_build_validates_legacy_v2_overview_transcript_digest(tmp_path: Path) -> None:
    team = _team()
    _write_team(tmp_path, team)
    summarize_archive(tmp_path, team.team_slug, "heuristic", "offline")
    overview_path = (
        tmp_path
        / "teams"
        / team.team_slug
        / "summary_data"
        / "project_overview.json"
    )
    overview = json.loads(overview_path.read_text(encoding="utf-8"))
    overview["schema_version"] = 2
    overview["source"]["transcript"] = _root_overview_input(team).transcript
    overview["source"]["context_sha256"] = "0" * 64
    write_json_if_changed(overview_path, overview)

    with pytest.raises(ValueError, match="transcript digest mismatch"):
        build_archive(
            tmp_path,
            team.team_slug,
            output=tmp_path / "corrupt-legacy-overview-site",
        )


def test_build_rejects_present_corrupt_overview_projection(tmp_path: Path) -> None:
    team = _team()
    _write_team(tmp_path, team)
    overview_path = (
        tmp_path
        / "teams"
        / team.team_slug
        / "summary_data"
        / "project_overview.json"
    )
    write_json_if_changed(overview_path, {"legacy": "corrupt-shape"})

    with pytest.raises(ValueError, match="schema_version"):
        build_archive(tmp_path, team.team_slug, output=tmp_path / "corrupt-site")


def test_build_rejects_non_regular_summary_projection(tmp_path: Path) -> None:
    team = _team()
    _write_team(tmp_path, team)
    overview_path = (
        tmp_path
        / "teams"
        / team.team_slug
        / "summary_data"
        / "project_overview.json"
    )
    overview_path.mkdir(parents=True)

    with pytest.raises(ValueError, match="not a regular file"):
        build_archive(tmp_path, team.team_slug, output=tmp_path / "directory-site")


def test_combined_export_namespaces_teams_and_is_byte_idempotent(
    tmp_path: Path,
) -> None:
    first_team = _team(
        "Reviewed https://github.com/example/project/pull/12 as part of the result."
    )
    second_team = replace(first_team, team_slug="claude-test", provider="claude")
    window = DateWindow(
        start_date=None,
        end_date=None,
        start_ms=START,
        end_ms=START + 20_000,
        start_time="2026-04-01T00:00:00.000Z",
        end_time="2026-04-01T00:00:20.000Z",
    )
    for team in (first_team, second_team):
        _write_team(tmp_path, team)
        write_json_if_changed(
            tmp_path / "teams" / team.team_slug / "raw" / "artifacts.json",
            narrow_json(extract_artifacts(team).to_json_obj()),
        )
        summarize_archive(
            tmp_path,
            team.team_slug,
            "heuristic",
            "offline",
            summary_window=window,
            rollup_kinds=("hourly",),
        )
    output = tmp_path / "combined"

    first = build_combined_archive(
        tmp_path,
        ("claude-test", first_team.team_slug),
        output=output,
        display_timezone="America/New_York",
        display_window=window,
        rollup_kinds=("hourly",),
    )
    second = build_combined_archive(
        tmp_path,
        (first_team.team_slug, "claude-test"),
        output=output,
        display_timezone="America/New_York",
        display_window=window,
        rollup_kinds=("hourly",),
    )

    assert first["teams"] == 2
    assert first["files_changed"] > 0
    assert second["files_changed"] == 0
    timeline = json.loads(
        (output / "data" / "timeline.json").read_text(encoding="utf-8")
    )
    assert timeline["range"] == {"start_ms": START, "end_ms": START + 20_000}
    assert [team["slug"] for team in timeline["teams"]] == [
        "claude-test",
        first_team.team_slug,
    ]
    assert {team["provider"] for team in timeline["teams"]} == {"claude", "codex"}
    agent_ids = {agent["id"] for agent in timeline["agents"]}
    assert len(agent_ids) == len(timeline["agents"])
    assert all("::" in agent_id for agent_id in agent_ids)
    assert all(phase["team"] in {"claude-test", first_team.team_slug} for phase in timeline["phases"])
    assert all(phase["agent_id"] in agent_ids for phase in timeline["phases"])
    assert all(edge["source_id"] in agent_ids for edge in timeline["edges"])
    assert all(edge["target_id"] in agent_ids for edge in timeline["edges"])
    assert {rollup["team"] for rollup in timeline["rollups"]} == {
        "claude-test",
        first_team.team_slug,
    }
    assert {rollup["kind"] for rollup in timeline["rollups"]} == {"hourly"}
    assert {item["team"] for item in timeline["summary_files"]} == {
        "claude-test",
        first_team.team_slug,
    }
    for phase in timeline["phases"]:
        detail_path = output / phase["detail_path"]
        assert detail_path.is_file()
        assert phase["detail_path"].startswith(f"data/details/{phase['team']}/")
    assert (output / "Makefile").is_file()
    assert (output / ".agent-team-timeline.json").is_file()
    export_manifest = json.loads(
        (output / "data" / "export.json").read_text(encoding="utf-8")
    )
    assert export_manifest["teams"] == ["claude-test", first_team.team_slug]
    artifact_catalog = json.loads(
        (output / "data" / "artifacts.json").read_text(encoding="utf-8")
    )
    artifact_ids = {
        artifact["artifact_id"] for artifact in artifact_catalog["artifacts"]
    }
    assert len(artifact_ids) == 2
    assert any(value.startswith("artifact-claude-test-") for value in artifact_ids)
    assert any(
        value.startswith(f"artifact-{first_team.team_slug}-")
        for value in artifact_ids
    )
    assert {artifact["team"] for artifact in artifact_catalog["artifacts"]} == {
        "claude-test",
        first_team.team_slug,
    }

    assert timeline_main(
        (
            "export",
            "--archive",
            str(tmp_path),
            "--output",
            str(output),
            "--team",
            first_team.team_slug,
            "--team",
            "claude-test",
            "--start-time",
            "2026-03-31T23:33:20Z",
            "--end-time",
            "2026-03-31T23:33:40Z",
            "--rollup-kind",
            "hourly",
            "--timezone",
            "America/New_York",
        )
    ) == 0
    manifest = json.loads((output / "manifest.json").read_text(encoding="utf-8"))
    assert manifest["teams"] == ["claude-test", first_team.team_slug]
    run = json.loads(
        (output / "runs" / f"{manifest['last_run_id']}.json").read_text(
            encoding="utf-8"
        )
    )
    assert run["team_slugs"] == ["claude-test", first_team.team_slug]


def test_combined_export_builds_two_zero_summary_teams(tmp_path: Path) -> None:
    codex_team = _team()
    claude_team = replace(codex_team, team_slug="claude-test", provider="claude")
    for team in (codex_team, claude_team):
        _write_team(tmp_path, team)
    window = DateWindow(
        start_date=None,
        end_date=None,
        start_ms=START,
        end_ms=START + 20_000,
        start_time="2026-03-31T23:33:20.000Z",
        end_time="2026-03-31T23:33:40.000Z",
    )
    output = tmp_path / "combined-zero-summary"

    first = build_combined_archive(
        tmp_path,
        (codex_team.team_slug, claude_team.team_slug),
        output=output,
        display_timezone="America/New_York",
        display_window=window,
        rollup_kinds=("hourly",),
    )
    second = build_combined_archive(
        tmp_path,
        (claude_team.team_slug, codex_team.team_slug),
        output=output,
        display_timezone="America/New_York",
        display_window=window,
        rollup_kinds=("hourly",),
    )

    assert first["teams"] == 2
    assert first["files_changed"] > 0
    assert second["files_changed"] == 0
    assert all(
        not (tmp_path / "teams" / team.team_slug / "summary_data").exists()
        for team in (codex_team, claude_team)
    )
    timeline = json.loads(
        (output / "data" / "timeline.json").read_text(encoding="utf-8")
    )
    assert {item["slug"] for item in timeline["teams"]} == {
        codex_team.team_slug,
        claude_team.team_slug,
    }
    assert timeline["phases"]
    assert all(phase["phrase"] == "Summary unavailable" for phase in timeline["phases"])
    assert all("::" in phase["agent_id"] for phase in timeline["phases"])
    assert {rollup["team"] for rollup in timeline["rollups"]} == {
        codex_team.team_slug,
        claude_team.team_slug,
    }


def test_combined_export_refuses_unmarked_nonempty_output(tmp_path: Path) -> None:
    team = _team()
    other = replace(team, team_slug="claude-test", provider="claude")
    for item in (team, other):
        _write_team(tmp_path, item)
    output = tmp_path / "not-an-archive"
    output.mkdir()
    (output / "valuable.txt").write_text("keep me\n", encoding="utf-8")

    with pytest.raises(ValueError, match="refusing non-empty non-archive"):
        build_combined_archive(
            tmp_path,
            (team.team_slug, other.team_slug),
            output=output,
            display_timezone="UTC",
        )
    assert (output / "valuable.txt").read_text(encoding="utf-8") == "keep me\n"


def test_agent_name_v1_projection_degrades_without_lifetime_summary(
    tmp_path: Path,
) -> None:
    team = _team()
    child = next(agent for agent in team.agents if agent.thread_id == CHILD)
    path = (
        tmp_path
        / "teams"
        / team.team_slug
        / "summary_data"
        / "agents"
        / f"{CHILD}.json"
    )
    write_json_if_changed(
        path,
        {
            "schema_version": 1,
            "agent": {
                "thread_id": CHILD,
                "official_path": child.agent_path,
                "coordinator_nickname": child.nickname,
                "role": child.role,
                "depth": child.depth,
                "parent_official_path": "/root",
            },
            "name": {
                "thread_id": CHILD,
                "short_name": "Release receipt audit",
                "rationale": "The work audited release receipt binding.",
                "model": "gpt-5.6-sol",
                "prompt_version": "agent-team-timeline-agent-name-v1",
                "input_hash": "legacy-name-hash",
                "generated_at": "2026-08-05T00:00:00Z",
            },
        },
    )

    loaded = _load_agent_names(tmp_path, team, frozenset({CHILD}))[CHILD]

    assert loaded.short_name == "Release receipt audit"
    assert loaded.lifetime_summary is None


def test_spanning_tool_is_not_repeated_before_later_phase_boundary() -> None:
    team = _team()
    long_tool = replace(
        team.tool_calls[0],
        started_at_ms=START + 10_000,
        ended_at_ms=START + 40 * 60 * 1000,
    )
    later = _event(
        "later-child-update",
        CHILD,
        35 * 60 * 1000,
        "assistant_message",
        "The long validation command is still running.",
    )
    updated = replace(team, events=team.events + (later,), tool_calls=(long_tool,))
    for phase in build_phases(updated):
        assert all(phase.start_ms <= entry.at_ms < phase.end_ms for entry in phase.transcript)


def _rollup_result(key: str, phrase: str) -> SummaryResult:
    return SummaryResult(key, phrase, phrase, (), "test", "v1", key, "now")


def test_monthly_rollup_excludes_cross_boundary_week_summary() -> None:
    team = _team()
    month = Period("monthly", "month", "Month", 100, 300, "month.md", False)
    crossing = Period("weekly", "cross", "Cross", 50, 150, "cross.md", False)
    contained = Period("weekly", "inside", "Inside", 150, 250, "inside.md", False)
    stats = PhaseStats(0, 0, 0, 0)

    def phase(key: str, start: int) -> PhaseWindow:
        return PhaseWindow(
            key,
            key,
            ROOT,
            "Coordinator",
            start,
            start + 10,
            stats,
            (),
            key,
            "",
            (),
        )

    boundary_start = phase("boundary-start", 110)
    covered = phase("covered", 160)
    boundary_end = phase("boundary-end", 260)
    phase_results = {
        item.summary_key: _rollup_result(item.summary_key, item.summary_key.upper())
        for item in (boundary_start, covered, boundary_end)
    }
    lower_results = {
        "cross:weekly": _rollup_result("cross", "CROSS_WEEK"),
        "inside:weekly": _rollup_result("inside", "CONTAINED_WEEK"),
    }
    job = _rollup_jobs_for_level(
        team,
        (month,),
        (boundary_start, covered, boundary_end),
        phase_results,
        (crossing, contained),
        lower_results,
        (),
        (),
    )[0]
    assert "CROSS_WEEK" not in job.transcript
    assert "CONTAINED_WEEK" in job.transcript
    assert "BOUNDARY-START" in job.transcript
    assert "BOUNDARY-END" in job.transcript
    assert "COVERED" not in job.transcript


def test_plain_rollup_gets_overview_and_supported_definitions_only() -> None:
    team = _team()
    period = Period("daily", "day", "Day", START, START + 30_000, "day.md", False)
    phases = build_phases(team)
    phase_results = {
        phase.summary_key: _rollup_result(phase.summary_key, phase.summary_key)
        for phase in phases
    }
    supported = GlossaryTerm(
        "exact-head",
        START,
        2,
        "First-use evidence for exact-head.",
        "2026-W13",
        glossary_term_id("exact-head"),
        "A release check that requires one exact revision.",
        "supported",
    )
    unsupported = GlossaryTerm(
        "DBI",
        START,
        2,
        "DBI first-use evidence.",
        "2026-W13",
        glossary_term_id("DBI"),
        "Insufficient evidence: DBI was never expanded.",
        "insufficient-evidence",
    )

    technical = _rollup_jobs_for_level(
        team,
        (period,),
        phases,
        phase_results,
        (),
        {},
        (),
        (supported, unsupported),
    )[0]
    plain = _rollup_jobs_for_level(
        team,
        (period,),
        phases,
        phase_results,
        (),
        {},
        (),
        (supported, unsupported),
        PLAIN_LANGUAGE_ROLLUP_STYLE,
        _rollup_result(
            "project-overview",
            "Hermit runs guest software in a repeatable environment.",
        ),
    )[0]
    without_definitions = tuple(
        replace(term, definition="", definition_status="unavailable")
        for term in (supported, unsupported)
    )
    technical_without_definitions = _rollup_jobs_for_level(
        team,
        (period,),
        phases,
        phase_results,
        (),
        {},
        (),
        without_definitions,
    )[0]

    assert technical.glossary == technical_without_definitions.glossary
    assert "Hermit runs guest software" in plain.glossary
    assert "A release check that requires one exact revision" in plain.glossary
    assert "DBI" not in plain.glossary


def test_build_rejects_pre_definition_glossary_schema(tmp_path: Path) -> None:
    team = _team()
    _write_team(tmp_path, team)
    summarize_archive(tmp_path, team.team_slug, "heuristic", "test-model")
    glossary_path = (
        tmp_path
        / "teams"
        / team.team_slug
        / "summary_data"
        / "glossary.json"
    )
    glossary = json.loads(glossary_path.read_text(encoding="utf-8"))
    glossary["schema_version"] = 1
    glossary_path.write_text(json.dumps(glossary), encoding="utf-8")

    with pytest.raises(ValueError, match="unsupported glossary schema; run summarize"):
        build_archive(tmp_path, team.team_slug)


def test_calendar_periods_use_local_dst_boundaries() -> None:
    # 2026-03-08 is the US spring-forward day: local midnight-to-midnight is 23 hours.
    first_ms = 1_772_946_000_000  # 2026-03-08T05:00:00Z == midnight EST
    last_ms = first_ms + 22 * 60 * 60 * 1000
    periods = periods_for_range(first_ms, last_ms, "America/New_York", "dst-test")
    daily = next(period for period in periods if period.kind == "daily")
    assert daily.end_ms - daily.start_ms == 23 * 60 * 60 * 1000


def test_loopback_server_serves_json_with_safe_headers(tmp_path: Path) -> None:
    (tmp_path / "index.html").write_text("ok", encoding="utf-8")
    server = make_server(tmp_path, port=0)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        port = int(server.server_address[1])
        with urllib.request.urlopen(f"http://127.0.0.1:{port}/", timeout=5) as response:
            assert response.read() == b"ok"
            assert response.headers["X-Content-Type-Options"] == "nosniff"
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=5)
