"""Pure formatting of cached normalized data into Markdown and a static website."""

from __future__ import annotations

from collections.abc import Mapping, Sequence
from datetime import datetime, timezone
from importlib.resources import files
from pathlib import Path
from zoneinfo import ZoneInfo

from agent_team_timeline.archive import narrow_json, write_json_if_changed, write_text_if_changed
from agent_team_timeline.model import Agent, Edge, Event, TeamData, source_digest
from agent_team_timeline.periods import Period, period_heading
from agent_team_timeline.phases import PhaseStats, PhaseWindow, agent_label
from agent_team_timeline.summarize import SummaryResult
from agent_team_timeline.terminology import GlossaryTerm, glossary_markdown


def _iso(at_ms: int) -> str:
    return datetime.fromtimestamp(at_ms / 1000, tz=timezone.utc).isoformat().replace(
        "+00:00", "Z"
    )


def _local_time(at_ms: int, display_timezone: str) -> str:
    local = datetime.fromtimestamp(at_ms / 1000, tz=timezone.utc).astimezone(
        ZoneInfo(display_timezone)
    )
    return local.strftime("%Y-%m-%d %H:%M:%S %Z")


def _summary_obj(result: SummaryResult) -> dict[str, object]:
    return {
        "key": result.key,
        "phrase": result.phrase,
        "paragraph": result.paragraph,
        "work_summary": [
            {"at_ms": bullet.at_ms, "text": bullet.text}
            for bullet in result.work_summary
        ],
        "model": result.model,
        "prompt_version": result.prompt_version,
        "input_hash": result.input_hash,
        "generated_at": result.generated_at,
    }


def _phase_markdown(
    team: TeamData, phase: PhaseWindow, summary: SummaryResult
) -> str:
    stats = phase.stats
    lines = [
        f"# {summary.phrase}",
        "",
        f"Team: `{team.team_slug}`  ",
        f"Agent: `{phase.agent_label}`  ",
        f"Window: {_local_time(phase.start_ms, team.display_timezone)} to "
        f"{_local_time(phase.end_ms, team.display_timezone)}",
        "",
        f"> {summary.paragraph}",
        "",
        "## Agent work summary",
        "",
    ]
    if summary.work_summary:
        lines.extend(
            f"- **{_local_time(item.at_ms, team.display_timezone)}** — {item.text}"
            for item in summary.work_summary
        )
    else:
        lines.append("_No substantive work item survived the archival filter._")
    lines.extend(
        [
            "",
            "## Window statistics",
            "",
            f"- User prompts: {stats.user_prompts}",
            f"- Agent responses: {stats.agent_responses}",
            f"- Inter-agent messages: {stats.inter_agent_messages}",
            f"- Tool calls: {stats.tool_calls}",
            "",
            "## Summary provenance",
            "",
            f"- Model: `{summary.model}`",
            f"- Prompt: `{summary.prompt_version}`",
            f"- Input hash: `{summary.input_hash}`",
        ]
    )
    return "\n".join(lines).rstrip() + "\n"


def _rollup_markdown(
    team: TeamData,
    period: Period,
    summary: SummaryResult,
    stats: PhaseStats,
) -> str:
    lines = [
        f"# {period_heading(period, team.display_timezone)} {team.team_slug} {period.kind} summary",
        "",
        f"> **{summary.phrase}.** {summary.paragraph}",
        "",
        "## Agent work summary",
        "",
    ]
    if summary.work_summary:
        lines.extend(
            f"- **{_local_time(item.at_ms, team.display_timezone)}** — {item.text}"
            for item in summary.work_summary
        )
    else:
        lines.append("_No substantive work item was recorded in this range._")
    lines.extend(
        [
            "",
            "## Aggregate statistics",
            "",
            f"- User prompts: {stats.user_prompts}",
            f"- Agent responses: {stats.agent_responses}",
            f"- Inter-agent messages: {stats.inter_agent_messages}",
            f"- Tool calls: {stats.tool_calls}",
            "",
            "## Summary provenance",
            "",
            f"Model `{summary.model}` · prompt `{summary.prompt_version}` · "
            f"input `{summary.input_hash}`.",
        ]
    )
    return "\n".join(lines).rstrip() + "\n"


def _agent_markdown(
    team: TeamData,
    agent: Agent,
    phases: Sequence[PhaseWindow],
    summaries: Mapping[str, SummaryResult],
) -> str:
    lines = [
        f"# {agent_label(agent)}",
        "",
        f"Team: `{team.team_slug}` · thread `{agent.thread_id}` · path `{agent.agent_path}`",
        "",
    ]
    for phase in phases:
        result = summaries[phase.summary_key]
        lines.extend(
            [
                f"## {_local_time(phase.start_ms, team.display_timezone)} — {result.phrase}",
                "",
                result.paragraph,
                "",
            ]
        )
        lines.extend(
            f"- **{_local_time(item.at_ms, team.display_timezone)}** — {item.text}"
            for item in result.work_summary
        )
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def _time_range(team: TeamData, phases: Sequence[PhaseWindow]) -> tuple[int, int]:
    values: list[int] = []
    values.extend(agent.started_at_ms for agent in team.agents)
    values.extend(event.timestamp_ms for event in team.events)
    values.extend(phase.start_ms for phase in phases)
    values.extend(phase.end_ms for phase in phases)
    if not values:
        now = int(datetime.now(tz=timezone.utc).timestamp() * 1000)
        return now - 1000, now
    start = min(values)
    end = max(values)
    return start, max(start + 1000, end)


def _summary_for_agent_at(
    agent_id: str,
    at_ms: int,
    phases: Sequence[PhaseWindow],
    summaries: Mapping[str, SummaryResult],
) -> SummaryResult | None:
    own = [phase for phase in phases if phase.agent_id == agent_id]
    if not own:
        return None
    containing = [phase for phase in own if phase.start_ms <= at_ms <= phase.end_ms]
    selected = min(
        containing or own,
        key=lambda phase: (abs(phase.start_ms - at_ms), phase.start_ms),
    )
    return summaries.get(selected.summary_key)


def _edge_obj(
    edge: Edge,
    agents: Mapping[str, Agent],
    phases: Sequence[PhaseWindow],
    summaries: Mapping[str, SummaryResult],
) -> dict[str, object] | None:
    target = agents.get(edge.to_thread_id)
    if target is None or edge.from_thread_id not in agents:
        return None
    summary = _summary_for_agent_at(edge.to_thread_id, edge.timestamp_ms, phases, summaries)
    readable_kind = "message" if edge.kind == "followup" else edge.kind
    action = {
        "spawn": "Spawn",
        "message": "Message to",
        "interrupt": "Interrupt",
    }.get(readable_kind, readable_kind.replace("_", " ").title())
    phrase = f"{action} {agent_label(target)}"
    paragraph = summary.paragraph if summary is not None else phrase
    full_text = edge.message_text or ""
    status = ""
    if edge.content_availability == "encrypted":
        # Codex may leave a plaintext routing envelope next to an encrypted body. Do not
        # misrepresent that envelope as the user's full collaboration instruction.
        full_text = ""
        status = (
            "Codex persisted this collaboration payload encrypted. The exact plaintext is not "
            "available to an offline transcript parser; the paragraph is inferred from the "
            "receiving agent's work."
        )
    elif not full_text:
        status = "No plaintext message body was present in the source log."
    return {
        "id": edge.edge_id,
        "source_id": edge.from_thread_id,
        "target_id": edge.to_thread_id,
        "source_ms": edge.timestamp_ms,
        "target_ms": target.started_at_ms if readable_kind == "spawn" else edge.timestamp_ms,
        "kind": readable_kind if readable_kind in ("spawn", "message", "result") else "other",
        "phrase": phrase,
        "paragraph": paragraph,
        "full_text": full_text,
        "content_status": status,
    }


def _result_edge_objs(
    team: TeamData,
    agents: Mapping[str, Agent],
    phases: Sequence[PhaseWindow],
    summaries: Mapping[str, SummaryResult],
) -> list[dict[str, object]]:
    result: list[dict[str, object]] = []
    for agent in team.agents:
        if agent.parent_thread_id is None or agent.parent_thread_id not in agents:
            continue
        finals = [
            event
            for event in team.events
            if event.thread_id == agent.thread_id
            and event.kind == "assistant_message"
            and event.phase == "final_answer"
            and event.text
        ]
        for final in sorted(finals, key=lambda event: (event.timestamp_ms, event.event_id)):
            summary = _summary_for_agent_at(
                agent.thread_id, final.timestamp_ms, phases, summaries
            )
            result.append(
                {
                    "id": f"result-{final.event_id}",
                    "source_id": agent.thread_id,
                    "target_id": agent.parent_thread_id,
                    "source_ms": final.timestamp_ms,
                    "target_ms": final.timestamp_ms,
                    "kind": "result",
                    "phrase": f"{agent_label(agent)} reports results",
                    "paragraph": summary.paragraph if summary else (final.text or "")[:500],
                    "full_text": final.text or "",
                    "content_status": "",
                }
            )
    return result


def _event_objs(team: TeamData) -> list[dict[str, object]]:
    result: list[dict[str, object]] = []
    for event in team.events:
        if event.kind in ("user_prompt", "assistant_message", "inter_agent_message"):
            result.append(
                {"at_ms": event.timestamp_ms, "agent_id": event.thread_id, "kind": event.kind}
            )
    for tool in team.tool_calls:
        count = sum(value for _, value in tool.nested_tools) or 1
        result.extend(
            {"at_ms": tool.started_at_ms, "agent_id": tool.thread_id, "kind": "tool_call"}
            for _ in range(count)
        )
    result.sort(
        key=lambda item: (
            _object_int(item.get("at_ms")),
            _object_string(item.get("agent_id")),
            _object_string(item.get("kind")),
        )
    )
    return result


def _object_int(value: object) -> int:
    return value if isinstance(value, int) and not isinstance(value, bool) else 0


def _object_string(value: object) -> str:
    return value if isinstance(value, str) else ""


def _standalone_server() -> str:
    return '''#!/usr/bin/env python3
"""Serve this self-contained timeline on loopback and optionally open a browser."""
import argparse
import functools
import http.server
import pathlib
import threading
import webbrowser

parser = argparse.ArgumentParser()
parser.add_argument("--port", type=int, default=8765)
parser.add_argument("--open", action="store_true")
args = parser.parse_args()
root = pathlib.Path(__file__).resolve().parent
handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(root))
server = http.server.ThreadingHTTPServer(("127.0.0.1", args.port), handler)
url = f"http://127.0.0.1:{server.server_address[1]}/"
print(f"Serving {root} at {url}")
if args.open:
    threading.Timer(0.2, lambda: webbrowser.open(url)).start()
try:
    server.serve_forever()
except KeyboardInterrupt:
    pass
finally:
    server.server_close()
'''


def render_archive(
    archive: Path,
    team: TeamData,
    phases: Sequence[PhaseWindow],
    phase_summaries: Mapping[str, SummaryResult],
    periods: Sequence[Period],
    rollup_summaries: Mapping[str, SummaryResult],
    rollup_stats: Mapping[str, PhaseStats],
    glossary_terms: Sequence[GlossaryTerm],
) -> dict[str, int]:
    """Regenerate all presentation files without invoking a summary backend."""

    changed = 0
    phase_paths: dict[str, str] = {}
    for phase in phases:
        summary = phase_summaries[phase.summary_key]
        raw_path = f"teams/{team.team_slug}/summaries/phases/{phase.phase_id}.md"
        detail_path = f"data/details/{phase.phase_id}.json"
        phase_paths[phase.phase_id] = detail_path
        changed += int(
            write_text_if_changed(archive / raw_path, _phase_markdown(team, phase, summary))
        )
        detail: dict[str, object] = {
            "phrase": summary.phrase,
            "paragraph": summary.paragraph,
            "stats": phase.stats.to_mapping(),
            "work_summary": [
                {"at_ms": item.at_ms, "text": item.text} for item in summary.work_summary
            ],
            "transcript": [entry.to_json_obj() for entry in phase.transcript],
            "raw_summary_path": raw_path,
        }
        changed += int(write_json_if_changed(archive / detail_path, narrow_json(detail)))

    phases_by_agent: dict[str, list[PhaseWindow]] = {}
    for phase in phases:
        phases_by_agent.setdefault(phase.agent_id, []).append(phase)
    for agent in team.agents:
        own = sorted(phases_by_agent.get(agent.thread_id, []), key=lambda phase: phase.start_ms)
        if not own:
            continue
        agent_path = f"teams/{team.team_slug}/summaries/agents/{agent.thread_id}.md"
        changed += int(
            write_text_if_changed(
                archive / agent_path,
                _agent_markdown(team, agent, own, phase_summaries),
            )
        )

    for period in periods:
        summary = rollup_summaries[period.key + ":" + period.kind]
        stats = rollup_stats[period.key + ":" + period.kind]
        changed += int(
            write_text_if_changed(
                archive / period.relative_path,
                _rollup_markdown(team, period, summary, stats),
            )
        )

    weeks = sorted({term.week for term in glossary_terms})
    for week in weeks:
        year = week.split("-W", 1)[0]
        path = f"teams/{team.team_slug}/summaries/glossary/{year}/{week}-{team.team_slug}-glossary.md"
        changed += int(
            write_text_if_changed(
                archive / path, glossary_markdown(team.team_slug, week, glossary_terms)
            )
        )

    static_root = files("agent_team_timeline") / "static"
    for name in ("index.html", "app.js", "style.css"):
        text = (static_root / name).read_text(encoding="utf-8")
        changed += int(write_text_if_changed(archive / name, text))
    changed += int(write_text_if_changed(archive / "serve.py", _standalone_server(), executable=True))
    changed += int(
        write_text_if_changed(
            archive / "Makefile",
            ".PHONY: serve open\nPORT ?= 8765\n\nserve:\n\tpython3 serve.py --port $(PORT)\n\n"
            "open:\n\tpython3 serve.py --port $(PORT) --open\n",
        )
    )
    changed += int(
        write_text_if_changed(
            archive / "README.md",
            f"# {team.team_slug} agent-team timeline\n\n"
            "This directory is a self-contained, version-controllable timeline archive.\n\n"
            "```bash\nmake serve\n# open http://127.0.0.1:8765/\n```\n\n"
            "Use `make open` to ask Python to open the browser. Do not open `index.html` directly: "
            "browsers block the JSON fetch from `file://`.\n",
        )
    )

    summary_files: list[dict[str, object]] = []
    for summary_path_file in sorted(
        (archive / "teams" / team.team_slug / "summaries").rglob("*.md")
    ):
        relative = summary_path_file.relative_to(archive).as_posix()
        parts = summary_path_file.relative_to(
            archive / "teams" / team.team_slug / "summaries"
        ).parts
        kind = parts[0] if parts else "summary"
        summary_files.append(
            {
                "label": summary_path_file.stem.replace("-", " "),
                "kind": kind,
                "period": summary_path_file.stem,
                "path": relative,
            }
        )

    start_ms, end_ms = _time_range(team, phases)
    latest_ms = max((event.timestamp_ms for event in team.events), default=end_ms)
    agents_by_id = {agent.thread_id: agent for agent in team.agents}
    agent_objs: list[dict[str, object]] = []
    for agent in team.agents:
        own = phases_by_agent.get(agent.thread_id, [])
        own_end = max((phase.end_ms for phase in own), default=agent.ended_at_ms or end_ms)
        agent_objs.append(
            {
                "id": agent.thread_id,
                "team": team.team_slug,
                "parent_id": agent.parent_thread_id,
                "path": agent.agent_path,
                "label": agent_label(agent),
                "nickname": agent.nickname or "",
                "depth": agent.depth,
                "start_ms": agent.started_at_ms,
                "end_ms": max(agent.ended_at_ms or own_end, own_end),
                "status": agent.status,
            }
        )
    phase_objs = [
        {
            "id": phase.phase_id,
            "agent_id": phase.agent_id,
            "start_ms": phase.start_ms,
            "end_ms": phase.end_ms,
            "phrase": phase_summaries[phase.summary_key].phrase,
            "paragraph": phase_summaries[phase.summary_key].paragraph,
            "detail_path": phase_paths[phase.phase_id],
            "stats": phase.stats.to_mapping(),
            "states": [state.to_json_obj() for state in phase.states],
        }
        for phase in phases
    ]
    edge_objs = [
        value
        for edge in team.edges
        for value in [_edge_obj(edge, agents_by_id, phases, phase_summaries)]
        if value is not None
    ]
    edge_objs.extend(_result_edge_objs(team, agents_by_id, phases, phase_summaries))
    edge_objs.sort(
        key=lambda item: (
            _object_int(item.get("source_ms")),
            _object_string(item.get("id")),
        )
    )
    rollup_objs = [
        {
            "kind": period.kind,
            "label": period.label + (" · partial" if period.partial else ""),
            "start_ms": period.start_ms,
            "end_ms": period.end_ms,
            "path": period.relative_path,
        }
        for period in periods
    ]
    timeline: dict[str, object] = {
        "schema_version": 1,
        "generated_at": _iso(latest_ms),
        "source_digest": source_digest(team),
        "display_timezone": team.display_timezone,
        "range": {"start_ms": start_ms, "end_ms": end_ms},
        "teams": [{"slug": team.team_slug, "label": team.team_slug}],
        "agents": agent_objs,
        "phases": phase_objs,
        "edges": edge_objs,
        "events": _event_objs(team),
        "rollups": rollup_objs,
        "summary_files": summary_files,
    }
    changed += int(
        write_json_if_changed(archive / "data" / "timeline.json", narrow_json(timeline))
    )
    return {
        "files_changed": changed,
        "phases": len(phases),
        "agents": len(team.agents),
        "edges": len(edge_objs),
        "summary_files": len(summary_files),
    }


__all__ = ["render_archive"]
