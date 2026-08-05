from __future__ import annotations

import hashlib
import json
import threading
import urllib.request
from dataclasses import replace
from pathlib import Path

import pytest

from agent_team_timeline.archive import narrow_json, write_json_if_changed
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
from agent_team_timeline.periods import Period, periods_for_range
from agent_team_timeline.phases import PhaseStats, PhaseWindow, build_phases
from agent_team_timeline.pipeline import (
    IngestReport,
    _agent_name_jobs,
    _rollup_jobs_for_level,
    build_archive,
    load_archived_team,
    record_run,
    summarize_archive,
)
from agent_team_timeline.render import _result_target
from agent_team_timeline.server import make_server
from agent_team_timeline.summarize import SummaryResult


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
    assert first.cache_misses == first.phases + (2 * first.rollups) + first.agent_names
    assert first.backend == "heuristic"
    assert first.model == "test-model"
    assert first.reasoning_effort == "high"
    assert first.newly_spent_usage.total_tokens == 0
    assert first.newly_spent_unknown_receipts == 0
    assert first.artifact_generation_unknown_receipts == 0
    assert first.unknown_legacy_artifacts == 0
    assert len(first.usage_run_paths) == (2 * first.rollups) + 2
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
    assert (tmp_path / "Makefile").read_text(encoding="utf-8").startswith(".PHONY: serve")
    timeline = json.loads((tmp_path / "data" / "timeline.json").read_text(encoding="utf-8"))
    assert len(timeline["agents"]) == 2
    child_track = next(agent for agent in timeline["agents"] if agent["id"] == CHILD)
    assert child_track["short_name"] == "Release receipt audit"
    assert child_track["official_name"] == "/root/release_receipt_audit"
    assert child_track["official_leaf"] == "release_receipt_audit"
    assert child_track["nickname"] == "Ada"
    assert "receipt binding" in child_track["lifetime_summary"]
    assert timeline["source_digest"] == source_digest(team)
    assert timeline["display_timezone_source"] == "legacy_team_data"
    assert timeline["teams"][0]["projects"] == []
    assert timeline["teams"][0]["hosts"] == []
    assert any(edge["kind"] == "spawn" for edge in timeline["edges"])
    assert any(edge["kind"] == "result" for edge in timeline["edges"])
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
        "plain-rollup-v1"
    )
    assert timeline["glossary_path"].endswith("codex-test-glossary.md")
    assert all(item["url"] == "#glossary/" + item["id"] for item in timeline["glossary"])
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
    assert name_record["schema_version"] == 2
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
    assert second.cache_hits == first.phases + (2 * first.rollups) + first.agent_names
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
    assert run["summaries"]["newly_spent_usage"]["total_tokens"] == 0
    assert run["summaries"]["usage_run_paths"] == list(second.usage_run_paths)


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


def test_every_completed_subagent_turn_gets_a_result_edge(tmp_path: Path) -> None:
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
    assert {edge["id"] for edge in result_edges} == {
        "result-child-final",
        "result-child-final-again",
    }


def test_resumed_nested_agent_reports_to_turn_initiator_not_finished_parent(
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
    result_targets = {
        edge["id"]: edge["target_id"]
        for edge in timeline["edges"]
        if edge["kind"] == "result"
    }
    assert result_targets["result-nested-initial-final"] == CHILD
    assert result_targets["result-nested-resumed-final"] == ROOT
    nested_track = next(agent for agent in timeline["agents"] if agent["id"] == NESTED)
    assert nested_track["parent_id"] == CHILD
    assert nested_track["end_ms"] > team.agents[1].ended_at_ms


def test_unmatched_result_falls_back_to_lineage_parent() -> None:
    team = replace(_team(), edges=())
    child = next(agent for agent in team.agents if agent.thread_id == CHILD)
    final = next(event for event in team.events if event.event_id == "child-final")

    assert _result_target(team, child, final) == ROOT


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


def test_only_changed_window_and_rollups_invalidate(tmp_path: Path) -> None:
    team = _team()
    _write_team(tmp_path, team)
    first = summarize_archive(tmp_path, team.team_slug, "heuristic", "test-model")
    changed = _team("A new root-cause sentence changes only the coordinator window.")
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
