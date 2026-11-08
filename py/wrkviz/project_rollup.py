"""One weekly summary for a whole project, rolled up from its teams' own weekly summaries.

Calendar rollups are per team: a week of one team is summarized from that team's days, and
nothing above that combines teams. A project weekly is the next level of the same bottom-up
tree. Its only evidence is each named team's already-written weekly *technical* rollup for the
week, run through the same weekly prompt the team rollups use; a plain-language weekly is then
written from that technical result, exactly as a team's is. No transcript, phase or other team is
read, so the project weekly cannot say anything its inputs did not.

The inputs are explicit and checked. Every named team must have a weekly rollup for the week in
the selected agent scope, all for the same interval; a missing one is refused by name rather than
silently leaving that team out. Each output lists its inputs (team, rollup path, input hash).

Outputs live under ``summary_scopes/<scope>/project/<name>/`` in the archive: their own cache,
``rollups/weekly/<week>.json``, Markdown under ``summaries/weekly/``, and a run record per run
under ``runs/``. Nothing in a team's tree, in ``summary_data/``, or in the website is touched.
"""

from __future__ import annotations

import json
import re
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from zoneinfo import ZoneInfo

from wrkviz.agent_scope import ALL_AGENTS, validate_agent_scope
from wrkviz.archive import (
    JsonValue,
    as_int,
    as_object,
    as_string,
    read_json,
    write_json_if_changed,
    write_text_if_changed,
)
from wrkviz.pipeline import (
    _PRESENTATION_FALLBACK_VERSION,
    _result_artifact_key,
    _result_line,
    _summary_from_json,
    _summary_json,
    _summary_root,
    _validate_team_slug,
    archive_writer_lock,
    utc_now,
)
from wrkviz.summarize import (
    SummaryJob,
    SummaryResult,
    SummaryRunStats,
    cached_summary,
    clean_summary_prose,
    summarize_jobs,
)
from wrkviz.summary_registry import (
    PLAIN_LANGUAGE_ROLLUP_STYLE,
    TECHNICAL_ROLLUP_STYLE,
    ContextComponent,
    ContextCoverage,
)
from wrkviz.terminology import plain_language_context_text

PROJECT_WEEKLY_SCHEMA_VERSION = 1

#: The line a project weekly built from coordinator-only team weeklies carries under its heading.
PROJECT_COORDINATOR_SCOPE_NOTE = (
    "**Scope: coordinator only.** Every input is a team weekly built from that team's root "
    "coordinator session and its declared continuations. In-session child agents are excluded "
    "except as their coordinator saw them, so this is not full-team coverage."
)
_WEEK = re.compile(r"[0-9]{4}-W[0-9]{2}")


@dataclass(frozen=True)
class ProjectWeeklyInput:
    """One team's weekly technical rollup, as the project weekly reads it."""

    team_slug: str
    rollup_path: str
    start_ms: int
    end_ms: int
    partial: bool
    technical: SummaryResult


@dataclass(frozen=True)
class ProjectWeeklyReport:
    """What one project weekly run read, called and wrote."""

    name: str
    week: str
    agent_scope: str
    inputs: tuple[ProjectWeeklyInput, ...]
    technical: SummaryResult
    plain: SummaryResult
    rollup_path: Path
    technical_markdown: Path
    plain_markdown: Path
    run_path: Path
    cache_hits: int
    cache_misses: int
    backend_batches: int


def project_root(archive: Path, agent_scope: str, name: str) -> Path:
    """Where one named project rollup keeps its cache, data, Markdown and run records."""

    _validate_team_slug(name)
    return archive / "summary_scopes" / validate_agent_scope(agent_scope) / "project" / name


def load_weekly_inputs(
    archive: Path, team_slugs: Sequence[str], week: str, agent_scope: str
) -> tuple[ProjectWeeklyInput, ...]:
    """Read every named team's weekly rollup for *week*; refuse, by name, any that is missing."""

    if not _WEEK.fullmatch(week):
        raise ValueError(f"week must be an ISO week key like 2026-W45, not {week!r}")
    if not team_slugs:
        raise ValueError("a project weekly needs at least one team")
    if len(set(team_slugs)) != len(team_slugs):
        raise ValueError("a team is named more than once")
    missing: list[str] = []
    inputs: list[ProjectWeeklyInput] = []
    for slug in sorted(team_slugs):
        path = _summary_root(archive, slug, agent_scope) / "rollups" / "weekly" / f"{week}.json"
        if not path.is_file():
            missing.append(f"{slug} ({path.relative_to(archive)})")
            continue
        obj = as_object(read_json(path), str(path))
        if as_string(obj.get("kind"), f"{path}.kind") != "weekly" or as_string(
            obj.get("key"), f"{path}.key"
        ) != week:
            raise ValueError(f"{path}: not the weekly rollup for {week}")
        technical = _summary_from_json(obj.get("technical_summary"), f"{path}.technical_summary")
        if technical.prompt_version == _PRESENTATION_FALLBACK_VERSION:
            missing.append(f"{slug} ({path.relative_to(archive)} holds no model summary)")
            continue
        inputs.append(
            ProjectWeeklyInput(
                team_slug=slug,
                rollup_path=str(path.relative_to(archive)),
                start_ms=as_int(obj.get("start_ms"), f"{path}.start_ms"),
                end_ms=as_int(obj.get("end_ms"), f"{path}.end_ms"),
                partial=_flag(obj.get("partial"), f"{path}.partial"),
                technical=technical,
            )
        )
    if missing:
        raise ValueError(
            f"no {agent_scope}-scope weekly rollup for {week} from: "
            + "; ".join(missing)
            + ". Summarize those teams with --rollup-kind weekly first, or leave them out."
        )
    intervals = {(item.start_ms, item.end_ms) for item in inputs}
    if len(intervals) != 1:
        raise ValueError(
            f"the {week} rollups cover different intervals (different timezones?): "
            + ", ".join(f"{item.team_slug} {item.start_ms}..{item.end_ms}" for item in inputs)
        )
    return tuple(inputs)


def _flag(value: JsonValue, where: str) -> bool:
    if not isinstance(value, bool):
        raise ValueError(f"{where}: expected true or false")
    return value


def project_weekly_jobs(
    name: str, week: str, inputs: Sequence[ProjectWeeklyInput]
) -> SummaryJob:
    """The technical weekly job: the teams' weekly technical summaries and nothing else."""

    start_ms, end_ms = inputs[0].start_ms, inputs[0].end_ms
    return SummaryJob(
        key=f"project-rollup:weekly:{week}",
        team_slug=name,
        agent_label=f"Technical weekly super-summary · {week} · {len(inputs)} teams",
        start_ms=start_ms,
        end_ms=end_ms,
        prior_context="",
        transcript="\n\n".join(
            _result_line(item.technical, f"Team {item.team_slug} weekly", start_ms)
            for item in inputs
        ),
        glossary="",
        stats={},
        summary_style=TECHNICAL_ROLLUP_STYLE,
        context_coverage=ContextCoverage(
            components=(
                ContextComponent("lower_level_summaries", len(inputs), len(inputs), "summaries"),
            ),
        ),
        dependency_keys=tuple(
            dict.fromkeys(_result_artifact_key(item.technical) for item in inputs)
        ),
    )


def _plain_job(technical_job: SummaryJob, technical: SummaryResult, week: str) -> SummaryJob:
    return SummaryJob(
        key=f"project-rollup-plain:weekly:{week}",
        team_slug=technical_job.team_slug,
        agent_label=technical_job.agent_label.replace("Technical", "Plain-language", 1),
        start_ms=technical_job.start_ms,
        end_ms=technical_job.end_ms,
        prior_context="",
        transcript=technical_job.transcript,
        glossary=plain_language_context_text("", []),
        stats={},
        summary_style=PLAIN_LANGUAGE_ROLLUP_STYLE,
        factual_context=_result_line(
            technical, "Authoritative technical weekly summary", technical_job.start_ms
        ),
        context_coverage=ContextCoverage(
            components=technical_job.context_coverage.components
            + (ContextComponent("technical_summary", 1, 1, "summaries"),),
        ),
        dependency_keys=technical_job.dependency_keys + (_result_artifact_key(technical),),
    )


def _local(at_ms: int, display_timezone: str) -> str:
    return (
        datetime.fromtimestamp(at_ms / 1000, tz=timezone.utc)
        .astimezone(ZoneInfo(display_timezone))
        .strftime("%Y-%m-%d %H:%M %Z")
    )


def _markdown(
    name: str,
    week: str,
    agent_scope: str,
    audience: str,
    summary: SummaryResult,
    inputs: Sequence[ProjectWeeklyInput],
    display_timezone: str,
) -> str:
    scope = "coordinator-only " if agent_scope != ALL_AGENTS else ""
    partial = any(item.partial for item in inputs)
    lines = [
        f"# {week} {name} project weekly {scope}{audience} summary",
        "",
        *([f"> {PROJECT_COORDINATOR_SCOPE_NOTE}", ""] if agent_scope != ALL_AGENTS else []),
        f"> Rolled up from {len(inputs)} team weekly technical summaries "
        f"({_local(inputs[0].start_ms, display_timezone)} to "
        f"{_local(inputs[0].end_ms, display_timezone)}"
        f"{'; the inputs cover only part of the week' if partial else ''}). "
        "It adds no evidence of its own.",
        "",
        f"**{summary.phrase}.** {clean_summary_prose(summary.paragraph)}",
        "",
        "## Work summary",
        "",
    ]
    lines.extend(
        f"- **{_local(item.at_ms, display_timezone)}** — {item.text}"
        for item in summary.work_summary
    )
    if not summary.work_summary:
        lines.append("_No substantive work item was recorded in this range._")
    lines.extend(["", "## Inputs", ""])
    lines.extend(
        f"- `{item.team_slug}`: `{item.rollup_path}` (input `{item.technical.input_hash}`)"
        for item in inputs
    )
    lines.extend(
        [
            "",
            "## Summary provenance",
            "",
            f"Model `{summary.model}` · prompt `{summary.prompt_version}` · "
            f"input `{summary.input_hash}`.",
        ]
    )
    return "\n".join(lines).rstrip() + "\n"


def plan_project_weekly(
    archive: Path,
    name: str,
    team_slugs: Sequence[str],
    week: str,
    backend: str,
    rollup_model: str,
    *,
    agent_scope: str = ALL_AGENTS,
    reasoning_effort: str | None = None,
    service_tier: str | None = None,
) -> tuple[tuple[ProjectWeeklyInput, ...], bool]:
    """Return the inputs and whether the technical weekly is already cached; no model call."""

    inputs = load_weekly_inputs(archive, team_slugs, week, agent_scope)
    job = project_weekly_jobs(name, week, inputs)
    cache = project_root(archive, agent_scope, name) / "cache"
    hit = cached_summary(job, cache, backend, rollup_model, reasoning_effort, service_tier)
    return inputs, hit is not None


def summarize_project_weekly(
    archive: Path,
    name: str,
    team_slugs: Sequence[str],
    week: str,
    backend: str,
    rollup_model: str,
    *,
    agent_scope: str = ALL_AGENTS,
    display_timezone: str = "UTC",
    reasoning_effort: str | None = None,
    service_tier: str | None = None,
    codex_command: Sequence[str] = ("codex",),
    claude_command: Sequence[str] = ("claude",),
    muse_command: Sequence[str] = ("muse",),
    backend_timeout_seconds: float | None = None,
    command: Sequence[str] = (),
) -> ProjectWeeklyReport:
    """Write the project's technical and plain-language weekly from its teams' weeklies.

    Holds the archive writer lock, so it neither reads a team rollup a summarize run is still
    writing nor races another project rollup.
    """

    ZoneInfo(display_timezone)
    with archive_writer_lock(archive):
        return _summarize_project_weekly_locked(
            archive,
            name,
            team_slugs,
            week,
            backend,
            rollup_model,
            agent_scope=agent_scope,
            display_timezone=display_timezone,
            reasoning_effort=reasoning_effort,
            service_tier=service_tier,
            codex_command=codex_command,
            claude_command=claude_command,
            muse_command=muse_command,
            backend_timeout_seconds=backend_timeout_seconds,
            command=command,
        )


def _summarize_project_weekly_locked(
    archive: Path,
    name: str,
    team_slugs: Sequence[str],
    week: str,
    backend: str,
    rollup_model: str,
    *,
    agent_scope: str,
    display_timezone: str,
    reasoning_effort: str | None,
    service_tier: str | None,
    codex_command: Sequence[str],
    claude_command: Sequence[str],
    muse_command: Sequence[str],
    backend_timeout_seconds: float | None,
    command: Sequence[str],
) -> ProjectWeeklyReport:
    started_at = utc_now()
    inputs = load_weekly_inputs(archive, team_slugs, week, agent_scope)
    root = project_root(archive, agent_scope, name)
    cache = root / "cache"
    def generate(job: SummaryJob) -> tuple[SummaryResult, SummaryRunStats]:
        results, stats = summarize_jobs(
            (job,),
            cache,
            backend,
            rollup_model,
            max_workers=1,
            batch_size=1,
            codex_command=codex_command,
            claude_command=claude_command,
            muse_command=muse_command,
            reasoning_effort=reasoning_effort,
            service_tier=service_tier,
            backend_timeout_seconds=backend_timeout_seconds,
        )
        return results[job.key], stats

    technical_job = project_weekly_jobs(name, week, inputs)
    technical, technical_stats = generate(technical_job)
    plain, plain_stats = generate(_plain_job(technical_job, technical, week))

    input_records: list[JsonValue] = [
        {
            "team": item.team_slug,
            "rollup": item.rollup_path,
            "input_hash": item.technical.input_hash,
        }
        for item in inputs
    ]
    rollup_path = root / "rollups" / "weekly" / f"{week}.json"
    write_json_if_changed(
        rollup_path,
        {
            "schema_version": PROJECT_WEEKLY_SCHEMA_VERSION,
            "kind": "weekly",
            "key": week,
            "name": name,
            "agent_scope": agent_scope,
            "start_ms": inputs[0].start_ms,
            "end_ms": inputs[0].end_ms,
            "partial": any(item.partial for item in inputs),
            "inputs": input_records,
            "technical_summary": _summary_json(technical),
            "plain_language_summary": _summary_json(plain),
        },
    )
    year = week.split("-W", 1)[0]
    markdown_root = root / "summaries" / "weekly" / year
    technical_markdown = markdown_root / f"{week}-{name}-weekly.md"
    plain_markdown = markdown_root / f"{week}-{name}-weekly-plain-language.md"
    for path, summary, audience in (
        (technical_markdown, technical, "technical"),
        (plain_markdown, plain, "plain-language"),
    ):
        write_text_if_changed(
            path,
            _markdown(name, week, agent_scope, audience, summary, inputs, display_timezone),
        )
    hits = technical_stats.hits + plain_stats.hits
    misses = technical_stats.misses + plain_stats.misses
    batches = technical_stats.batches + plain_stats.batches
    completed_at = utc_now()
    stamp = re.sub(r"[^0-9TZ]", "", completed_at.replace("+00:00", "Z"))
    run_path = root / "runs" / f"{stamp}-{week}.json"
    run: dict[str, JsonValue] = {
        "schema_version": 1,
        "command": list(command),
        "started_at": started_at,
        "completed_at": completed_at,
        "name": name,
        "week": week,
        "agent_scope": agent_scope,
        "backend": backend,
        "model": rollup_model,
        "reasoning_effort": reasoning_effort,
        "inputs": input_records,
        "cache_hits": hits,
        "cache_misses": misses,
        "backend_batches": batches,
        "outputs": [
            str(path.relative_to(archive))
            for path in (rollup_path, technical_markdown, plain_markdown)
        ],
    }
    run_path.parent.mkdir(parents=True, exist_ok=True)
    run_path.write_text(json.dumps(run, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    return ProjectWeeklyReport(
        name=name,
        week=week,
        agent_scope=agent_scope,
        inputs=inputs,
        technical=technical,
        plain=plain,
        rollup_path=rollup_path,
        technical_markdown=technical_markdown,
        plain_markdown=plain_markdown,
        run_path=run_path,
        cache_hits=hits,
        cache_misses=misses,
        backend_batches=batches,
    )
