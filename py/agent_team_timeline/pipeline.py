"""Idempotent ingest -> summarize -> format pipeline for timeline archives."""

from __future__ import annotations

import fcntl
import hashlib
import os
import re
from collections.abc import Iterator, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import dataclass, replace
from datetime import datetime, timezone
from pathlib import Path

from agent_team_timeline import __version__
from agent_team_timeline.archive import (
    JsonValue,
    as_array,
    as_int,
    as_object,
    as_string,
    narrow_json,
    read_json,
    write_json_if_changed,
    write_text_if_changed,
)
from agent_team_timeline.codex import (
    CodexParseError,
    CodexSourceCopy,
    load_codex_team,
    snapshot_codex_lineage,
)
from agent_team_timeline.claude import (
    ClaudeParseError,
    ClaudeSourceCopy,
    load_claude_team,
    snapshot_claude_lineage,
)
from agent_team_timeline.model import TeamData, ToolCall, source_digest
from agent_team_timeline.model_io import team_from_json_obj
from agent_team_timeline.naming import (
    AgentNameJob,
    AgentNameResult,
    name_agents,
)
from agent_team_timeline.github_metadata import load_pull_request_metadata_cache
from agent_team_timeline.github_enrich import pull_metadata_path
from agent_team_timeline.periods import Period, periods_for_range
from agent_team_timeline.phases import (
    PhaseStats,
    PhaseWindow,
    aggregate_stats,
    build_phases,
    phase_agent_ids,
)
from agent_team_timeline.render import render_archive
from agent_team_timeline.summarize import (
    SummaryJob,
    SummaryResult,
    SummaryRunStats,
    WorkBullet,
    summarize_jobs,
)
from agent_team_timeline.token_usage import TokenUsage
from agent_team_timeline.terminology import (
    GlossaryTerm,
    TermSource,
    glossary_prompt_text,
    scan_terminology,
)
from agent_team_timeline.window import DateWindow, apply_date_window


_TEAM_SLUG = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")
_ARCHIVE_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}")
_ARCHIVE_MARKER = ".agent-team-timeline.json"
_ARCHIVE_LOCK = ".agent-team-timeline.lock"


def _validate_team_slug(team_slug: str) -> None:
    if len(team_slug) > 64 or _TEAM_SLUG.fullmatch(team_slug) is None:
        raise ValueError(
            "team slug must be 1-64 lowercase letters/digits separated by single hyphens"
        )


def _validate_archive_id(value: str, label: str) -> None:
    if _ARCHIVE_ID.fullmatch(value) is None:
        raise ValueError(f"{label} is not a safe archive identifier: {value!r}")


@contextmanager
def _archive_writer_lock(archive: Path) -> Iterator[None]:
    """Serialize raw archive transactions across processes using Linux ``flock``."""

    if archive.exists() and not archive.is_dir():
        raise ValueError(f"archive output is not a directory: {archive}")
    archive.mkdir(parents=True, exist_ok=True)
    lock_path = archive / _ARCHIVE_LOCK
    try:
        descriptor = os.open(
            lock_path,
            os.O_RDWR | os.O_CREAT | os.O_CLOEXEC | os.O_NOFOLLOW,
            0o600,
        )
    except OSError as exc:
        raise ValueError(f"cannot safely open archive writer lock {lock_path}: {exc}") from exc
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX)
        yield
    finally:
        try:
            fcntl.flock(descriptor, fcntl.LOCK_UN)
        finally:
            os.close(descriptor)


def _ensure_archive(archive: Path, team_slug: str, *, create: bool) -> None:
    """Reject non-archive, non-empty output directories before root files can be replaced."""

    _validate_team_slug(team_slug)
    marker_path = archive / _ARCHIVE_MARKER
    if marker_path.is_file():
        marker = as_object(read_json(marker_path), str(marker_path))
        if marker.get("tool") != "agent-team-timeline" or marker.get("schema_version") != 1:
            raise ValueError(f"invalid agent-team-timeline archive marker at {marker_path}")
        return
    legacy_raw = archive / "teams" / team_slug / "raw" / "team.json"
    if legacy_raw.is_file():
        legacy_marker: dict[str, JsonValue] = {
            "schema_version": 1,
            "tool": "agent-team-timeline",
        }
        write_json_if_changed(marker_path, legacy_marker)
        return
    if archive.exists():
        if not archive.is_dir():
            raise ValueError(f"archive output is not a directory: {archive}")
        if any(path.name != _ARCHIVE_LOCK for path in archive.iterdir()):
            raise ValueError(
                f"refusing non-empty non-archive output directory {archive}; "
                "choose a new directory"
            )
    if not create:
        raise ValueError(f"not an agent-team-timeline archive: {archive}")
    new_marker: dict[str, JsonValue] = {
        "schema_version": 1,
        "tool": "agent-team-timeline",
    }
    write_json_if_changed(marker_path, new_marker)


@dataclass(frozen=True)
class IngestReport:
    """Counts and source identity produced by one archive ingest."""

    team_slug: str
    source_digest: str
    sources: int
    source_bytes: int
    agents: int
    events: int
    tool_calls: int
    edges: int
    files_changed: int

    def to_json_obj(self) -> dict[str, JsonValue]:
        """Return the ingest report as a JSON-serializable object."""

        return {
            "team_slug": self.team_slug,
            "source_digest": self.source_digest,
            "sources": self.sources,
            "source_bytes": self.source_bytes,
            "agents": self.agents,
            "events": self.events,
            "tool_calls": self.tool_calls,
            "edges": self.edges,
            "files_changed": self.files_changed,
        }


@dataclass(frozen=True)
class SummarizeReport:
    """Cache, backend, and output counts from one summarization run."""

    backend: str
    model: str
    reasoning_effort: str | None
    phases: int
    rollups: int
    agent_names: int
    glossary_terms: int
    cache_hits: int
    cache_misses: int
    backend_batches: int
    newly_spent_usage: TokenUsage
    newly_spent_unknown_receipts: int
    artifact_generation_usage: TokenUsage
    artifact_generation_unknown_receipts: int
    unknown_legacy_artifacts: int
    usage_run_paths: tuple[str, ...]
    files_changed: int

    def to_json_obj(self) -> dict[str, JsonValue]:
        """Return the summarization report as a JSON-serializable object."""

        return {
            "backend": self.backend,
            "model": self.model,
            "reasoning_effort": self.reasoning_effort,
            "phases": self.phases,
            "rollups": self.rollups,
            "agent_names": self.agent_names,
            "glossary_terms": self.glossary_terms,
            "cache_hits": self.cache_hits,
            "cache_misses": self.cache_misses,
            "backend_batches": self.backend_batches,
            "newly_spent_usage": self.newly_spent_usage.to_json(),
            "newly_spent_unknown_receipts": self.newly_spent_unknown_receipts,
            "artifact_generation_usage": self.artifact_generation_usage.to_json(),
            "artifact_generation_unknown_receipts": (
                self.artifact_generation_unknown_receipts
            ),
            "unknown_legacy_artifacts": self.unknown_legacy_artifacts,
            "usage_run_paths": list(self.usage_run_paths),
            "files_changed": self.files_changed,
        }


def utc_now() -> str:
    """Return the current UTC time in ISO 8601 form."""

    return datetime.now(tz=timezone.utc).isoformat().replace("+00:00", "Z")


def _write_json_durable(path: Path, value: JsonValue) -> bool:
    """Atomically write JSON and persist the replaced directory entry before returning."""

    changed = write_json_if_changed(path, value)
    if not changed:
        return False
    try:
        parent_fd = os.open(
            path.parent,
            os.O_RDONLY | os.O_DIRECTORY | os.O_CLOEXEC | os.O_NOFOLLOW,
        )
    except OSError as exc:
        raise OSError(f"cannot open parent directory for durable JSON write {path}: {exc}") from exc
    try:
        os.fsync(parent_fd)
    finally:
        os.close(parent_fd)
    return True


def _archive_team(team: TeamData) -> TeamData:
    """Keep messages verbatim while dropping bulky tool commands and outputs.

    Tool timing/name/count metadata is sufficient for the requested condensed transcript. The
    original Codex JSONL remains the authority for command stdout and patch bodies.
    """

    tools = tuple(
        replace(tool, input_text=None, output_text=None)
        for tool in team.tool_calls
    )
    return replace(team, tool_calls=tools)


def _raw_team_path(archive: Path, team_slug: str) -> Path:
    _validate_team_slug(team_slug)
    return archive / "teams" / team_slug / "raw" / "team.json"


def _summary_root(archive: Path, team_slug: str) -> Path:
    _validate_team_slug(team_slug)
    return archive / "teams" / team_slug / "summary_data"


def _source_snapshot_root(archive: Path, team_slug: str) -> Path:
    _validate_team_slug(team_slug)
    return archive / "teams" / team_slug / "source_snapshots"


def _source_manifest_path(archive: Path, team_slug: str) -> Path:
    _validate_team_slug(team_slug)
    return archive / "teams" / team_slug / "raw" / "source-manifest.json"


def _ensure_source_snapshots_ignored(archive: Path) -> bool:
    path = archive / ".gitignore"
    required = (f"/{_ARCHIVE_LOCK}", "/teams/*/source_snapshots/")
    if path.exists() and not path.is_file():
        raise ValueError(f"archive .gitignore is not a file: {path}")
    existing = path.read_text(encoding="utf-8") if path.is_file() else ""
    existing_lines = set(existing.splitlines())
    missing = [line for line in required if line not in existing_lines]
    if not missing:
        return False
    prefix = existing
    if prefix and not prefix.endswith("\n"):
        prefix += "\n"
    return write_text_if_changed(path, prefix + "\n".join(missing) + "\n")


def _load_source_manifest(
    archive: Path,
    team_slug: str,
    root_thread_id: str,
    date_window: DateWindow | None,
) -> tuple[CodexSourceCopy, ...]:
    path = _source_manifest_path(archive, team_slug)
    if not path.is_file():
        return ()
    obj = as_object(read_json(path), str(path))
    if obj.get("schema_version") != 1 or obj.get("provider") != "codex":
        raise CodexParseError(f"invalid Codex source manifest at {path}")
    recorded_root = as_string(obj.get("root_thread_id"), f"{path}: root_thread_id")
    if recorded_root != root_thread_id:
        raise CodexParseError(
            f"source manifest belongs to root {recorded_root!r}, not {root_thread_id!r}"
        )
    expected_window: object = (
        date_window.to_json_obj() if date_window is not None else None
    )
    if obj.get("date_window") != expected_window:
        raise CodexParseError(
            "archive date window differs from this ingest; choose a new output directory"
        )
    raw_sources = as_array(obj.get("sources"), f"{path}: sources")
    result: list[CodexSourceCopy] = []
    for index, raw_source in enumerate(raw_sources):
        source = as_object(raw_source, f"{path}: sources[{index}]")
        result.append(CodexSourceCopy.from_json_obj(source, f"{path}: sources[{index}]"))
    return tuple(result)


def _load_claude_source_manifest(
    archive: Path,
    team_slug: str,
    root_thread_id: str,
    date_window: DateWindow | None,
) -> tuple[ClaudeSourceCopy, ...]:
    path = _source_manifest_path(archive, team_slug)
    if not path.is_file():
        return ()
    obj = as_object(read_json(path), str(path))
    if obj.get("schema_version") != 1 or obj.get("provider") != "claude":
        raise ClaudeParseError(f"invalid Claude source manifest at {path}")
    recorded_root = as_string(obj.get("root_thread_id"), f"{path}: root_thread_id")
    if recorded_root != root_thread_id:
        raise ClaudeParseError(
            f"source manifest belongs to root {recorded_root!r}, not {root_thread_id!r}"
        )
    expected_window: object = (
        date_window.to_json_obj() if date_window is not None else None
    )
    if obj.get("date_window") != expected_window:
        raise ClaudeParseError(
            "archive date window differs from this ingest; choose a new output directory"
        )
    raw_sources = as_array(obj.get("sources"), f"{path}: sources")
    result: list[ClaudeSourceCopy] = []
    for index, raw_source in enumerate(raw_sources):
        source = as_object(raw_source, f"{path}: sources[{index}]")
        result.append(
            ClaudeSourceCopy.from_json_obj(source, f"{path}: sources[{index}]")
        )
    return tuple(result)


def _write_ingested_team(
    archive: Path,
    team_slug: str,
    team: TeamData,
    date_window: DateWindow | None,
    files_changed: int,
) -> tuple[TeamData, IngestReport]:
    """Write provider-neutral normalized data after a provider snapshot is durable."""

    changed = files_changed
    archived = _archive_team(team)
    _validate_archive_id(archived.root_thread_id, "root thread id")
    for agent in archived.agents:
        _validate_archive_id(agent.thread_id, "thread id")
    changed += int(
        _write_json_durable(
            _raw_team_path(archive, team_slug), narrow_json(archived.to_json_obj())
        )
    )
    by_thread = {agent.thread_id: agent for agent in archived.agents}
    for thread_id, agent in by_thread.items():
        obj: dict[str, object] = {
            "agent": agent.to_json_obj(),
            "turns": [
                turn.to_json_obj()
                for turn in archived.turns
                if turn.thread_id == thread_id
            ],
            "messages": [
                event.to_json_obj()
                for event in archived.events
                if event.thread_id == thread_id
            ],
            "tools": [
                tool.to_json_obj()
                for tool in archived.tool_calls
                if tool.thread_id == thread_id
            ],
            "edges": [
                edge.to_json_obj()
                for edge in archived.edges
                if edge.from_thread_id == thread_id or edge.to_thread_id == thread_id
            ],
        }
        changed += int(
            _write_json_durable(
                archive
                / "teams"
                / team_slug
                / "raw"
                / "messages"
                / f"{thread_id}.json",
                narrow_json(obj),
            )
        )
    digest = source_digest(team)
    snapshot: dict[str, object] = {
        "provider": team.provider,
        "root_thread_id": team.root_thread_id,
        "team_slug": team.team_slug,
        "display_timezone": team.display_timezone,
        "date_window": date_window.to_json_obj() if date_window is not None else None,
        "source_digest": digest,
        "sources": [source.to_json_obj() for source in team.sources],
    }
    changed += int(
        _write_json_durable(
            archive / "teams" / team_slug / "raw" / "source-snapshot.json",
            narrow_json(snapshot),
        )
    )
    report = IngestReport(
        team_slug=team_slug,
        source_digest=digest,
        sources=len(team.sources),
        source_bytes=sum(source.complete_bytes for source in team.sources),
        agents=len(team.agents),
        events=len(team.events),
        tool_calls=len(team.tool_calls),
        edges=len(team.edges),
        files_changed=changed,
    )
    return archived, report


def _ingest_codex_locked(
    archive: Path,
    sessions_root: Path,
    root_thread_id: str,
    team_slug: str,
    display_timezone: str,
    date_window: DateWindow | None,
) -> tuple[TeamData, IngestReport]:
    """Normalize one complete Codex lineage and write canonical raw JSON."""

    _ensure_archive(archive, team_slug, create=True)
    changed = int(_ensure_source_snapshots_ignored(archive))
    previous_sources = _load_source_manifest(
        archive, team_slug, root_thread_id, date_window
    )
    snapshot_root = _source_snapshot_root(archive, team_slug)
    source_copies = snapshot_codex_lineage(
        sessions_root,
        root_thread_id,
        snapshot_root,
        previous_sources,
        utc_now(),
    )
    changed += source_copies.files_changed
    # Parsing deliberately starts only after the original logs have been closed. Everything from
    # this point onward consumes the archive-local, newline-complete backup.
    allowed_sources = tuple(source.snapshot_path for source in source_copies.sources)
    team = apply_date_window(
        load_codex_team(
            snapshot_root,
            root_thread_id,
            team_slug,
            display_timezone,
            source_paths=allowed_sources,
        ),
        date_window,
    )
    if tuple(sorted(source.path for source in team.sources)) != tuple(sorted(allowed_sources)):
        raise CodexParseError(
            "parsed source set differs from the just-validated source snapshot manifest"
        )
    source_manifest: dict[str, object] = {
        "schema_version": 1,
        "provider": "codex",
        "root_thread_id": root_thread_id,
        "source_root": str(sessions_root.resolve()),
        "snapshot_root": f"teams/{team_slug}/source_snapshots",
        "date_window": date_window.to_json_obj() if date_window is not None else None,
        "sources": [source.to_json_obj() for source in source_copies.sources],
    }
    changed += int(
        _write_json_durable(
            _source_manifest_path(archive, team_slug), narrow_json(source_manifest)
        )
    )
    return _write_ingested_team(archive, team_slug, team, date_window, changed)


def ingest_codex(
    archive: Path,
    sessions_root: Path,
    root_thread_id: str,
    team_slug: str,
    display_timezone: str,
    date_window: DateWindow | None = None,
) -> tuple[TeamData, IngestReport]:
    """Snapshot and normalize one Codex lineage as one serialized raw-data transaction."""

    with _archive_writer_lock(archive):
        return _ingest_codex_locked(
            archive,
            sessions_root,
            root_thread_id,
            team_slug,
            display_timezone,
            date_window,
        )


def _ingest_claude_locked(
    archive: Path,
    session_file: Path,
    team_slug: str,
    display_timezone: str,
    date_window: DateWindow | None,
) -> tuple[TeamData, IngestReport]:
    """Snapshot and normalize one Claude coordinator lineage."""

    root_thread_id = session_file.stem
    _ensure_archive(archive, team_slug, create=True)
    changed = int(_ensure_source_snapshots_ignored(archive))
    previous_sources = _load_claude_source_manifest(
        archive, team_slug, root_thread_id, date_window
    )
    snapshot_root = _source_snapshot_root(archive, team_slug)
    source_copies = snapshot_claude_lineage(
        session_file,
        snapshot_root,
        previous_sources,
        utc_now(),
    )
    changed += source_copies.files_changed
    allowed_sources = tuple(source.snapshot_path for source in source_copies.sources)
    snapshot_session = snapshot_root / session_file.name
    team = apply_date_window(
        load_claude_team(
            snapshot_session,
            team_slug,
            display_timezone,
            source_paths=allowed_sources,
        ),
        date_window,
    )
    if tuple(sorted(source.path for source in team.sources)) != tuple(
        sorted(allowed_sources)
    ):
        raise ClaudeParseError(
            "parsed source set differs from the just-validated source snapshot manifest"
        )
    source_manifest: dict[str, object] = {
        "schema_version": 1,
        "provider": "claude",
        "root_thread_id": root_thread_id,
        "source_root": str(session_file.parent.resolve()),
        "root_session_file": str(session_file.resolve()),
        "snapshot_root": f"teams/{team_slug}/source_snapshots",
        "date_window": date_window.to_json_obj() if date_window is not None else None,
        "sources": [source.to_json_obj() for source in source_copies.sources],
    }
    changed += int(
        _write_json_durable(
            _source_manifest_path(archive, team_slug), narrow_json(source_manifest)
        )
    )
    return _write_ingested_team(archive, team_slug, team, date_window, changed)


def ingest_claude(
    archive: Path,
    session_file: Path,
    team_slug: str,
    display_timezone: str,
    date_window: DateWindow | None = None,
) -> tuple[TeamData, IngestReport]:
    """Snapshot and normalize one Claude lineage as one serialized transaction."""

    with _archive_writer_lock(archive):
        return _ingest_claude_locked(
            archive,
            session_file,
            team_slug,
            display_timezone,
            date_window,
        )


def load_archived_team(archive: Path, team_slug: str) -> TeamData:
    """Load and validate the normalized team snapshot stored in *archive*."""

    _ensure_archive(archive, team_slug, create=False)
    path = _raw_team_path(archive, team_slug)
    if not path.is_file():
        raise ValueError(f"no ingested team {team_slug!r}; run `agent-team-timeline ingest`")
    team = team_from_json_obj(read_json(path))
    if team.team_slug != team_slug:
        raise ValueError(
            f"archived team slug {team.team_slug!r} does not match requested {team_slug!r}"
        )
    _validate_team_slug(team.team_slug)
    _validate_archive_id(team.root_thread_id, "root thread id")
    for agent in team.agents:
        _validate_archive_id(agent.thread_id, "thread id")
    return team


def _glossary_terms(team: TeamData) -> tuple[GlossaryTerm, ...]:
    sources = [
        TermSource(event.timestamp_ms, event.text or "")
        for event in team.events
        if event.thread_id == team.root_thread_id
        and event.kind == "user_prompt"
        and event.text
        and (team.window_end_ms is None or event.timestamp_ms < team.window_end_ms)
    ]
    return scan_terminology(sources, team.display_timezone)


def _phase_jobs(
    team: TeamData, phases: Sequence[PhaseWindow], terms: Sequence[GlossaryTerm]
) -> tuple[SummaryJob, ...]:
    jobs: list[SummaryJob] = []
    for phase in phases:
        chronological_terms = [term for term in terms if term.introduced_at_ms <= phase.end_ms]
        jobs.append(
            SummaryJob(
                key=phase.summary_key,
                team_slug=team.team_slug,
                agent_label=phase.agent_label,
                start_ms=phase.start_ms,
                end_ms=phase.end_ms,
                prior_context=phase.prior_context,
                transcript=phase.transcript_text,
                glossary=glossary_prompt_text(chronological_terms),
                stats=phase.stats.to_mapping(),
            )
        )
    return tuple(jobs)


def _summary_json(summary: SummaryResult) -> dict[str, JsonValue]:
    return {
        "key": summary.key,
        "phrase": summary.phrase,
        "paragraph": summary.paragraph,
        "work_summary": [
            {"at_ms": item.at_ms, "text": item.text} for item in summary.work_summary
        ],
        "model": summary.model,
        "prompt_version": summary.prompt_version,
        "input_hash": summary.input_hash,
        "generated_at": summary.generated_at,
    }


def _summary_from_json(value: JsonValue, where: str) -> SummaryResult:
    obj = as_object(value, where)
    bullets = tuple(
        WorkBullet(
            at_ms=as_int(as_object(item, f"{where}.work_summary[]").get("at_ms"), "at_ms"),
            text=as_string(as_object(item, f"{where}.work_summary[]").get("text"), "text"),
        )
        for item in as_array(obj.get("work_summary"), f"{where}.work_summary")
    )
    return SummaryResult(
        key=as_string(obj.get("key"), f"{where}.key"),
        phrase=as_string(obj.get("phrase"), f"{where}.phrase"),
        paragraph=as_string(obj.get("paragraph"), f"{where}.paragraph"),
        work_summary=bullets,
        model=as_string(obj.get("model"), f"{where}.model"),
        prompt_version=as_string(obj.get("prompt_version"), f"{where}.prompt_version"),
        input_hash=as_string(obj.get("input_hash"), f"{where}.input_hash"),
        generated_at=as_string(obj.get("generated_at"), f"{where}.generated_at"),
    )


def _write_phase_data(
    archive: Path,
    team_slug: str,
    phases: Sequence[PhaseWindow],
    results: Mapping[str, SummaryResult],
) -> int:
    changed = 0
    for phase in phases:
        obj: dict[str, JsonValue] = {
            "phase_id": phase.phase_id,
            "agent_id": phase.agent_id,
            "start_ms": phase.start_ms,
            "end_ms": phase.end_ms,
            "summary": _summary_json(results[phase.summary_key]),
        }
        changed += int(
            write_json_if_changed(
                _summary_root(archive, team_slug) / "phases" / f"{phase.phase_id}.json",
                obj,
            )
        )
    return changed


def _period_key(period: Period) -> str:
    return period.key + ":" + period.kind


def _phases_in(period: Period, phases: Sequence[PhaseWindow]) -> list[PhaseWindow]:
    return [
        phase
        for phase in phases
        if phase.start_ms < period.end_ms and phase.end_ms > period.start_ms
    ]


def _result_line(result: SummaryResult, label: str, at_ms: int) -> str:
    bullets = " ".join(item.text for item in result.work_summary)
    return f"[{at_ms}] {label}: {result.phrase}. {result.paragraph} {bullets}".strip()


def _agent_name_jobs(
    team: TeamData,
    phases: Sequence[PhaseWindow],
    phase_results: Mapping[str, SummaryResult],
) -> tuple[AgentNameJob, ...]:
    """Build hindsight naming inputs after all phase-level work has been summarized."""

    agents_by_id = {agent.thread_id: agent for agent in team.agents}
    phases_by_agent: dict[str, list[PhaseWindow]] = {}
    for phase in phases:
        phases_by_agent.setdefault(phase.agent_id, []).append(phase)
    jobs: list[AgentNameJob] = []
    selected_ids = phase_agent_ids(team, phases)
    for agent in team.agents:
        if agent.thread_id not in selected_ids:
            continue
        own_phases = sorted(
            phases_by_agent.get(agent.thread_id, []),
            key=lambda phase: (phase.start_ms, phase.phase_id),
        )
        work_lines: list[str] = []
        for phase in own_phases:
            result = phase_results[phase.summary_key]
            work_lines.append(
                _result_line(result, phase.agent_label, phase.start_ms)
            )
        parent = (
            agents_by_id.get(agent.parent_thread_id)
            if agent.parent_thread_id is not None
            else None
        )
        jobs.append(
            AgentNameJob(
                key=f"agent-name:{agent.thread_id}",
                thread_id=agent.thread_id,
                official_path=agent.agent_path,
                coordinator_nickname=agent.nickname,
                role=agent.role,
                depth=agent.depth,
                parent_official_path=parent.agent_path if parent is not None else None,
                prior_context=own_phases[0].prior_context if own_phases else "",
                work_summary=(
                    "\n\n".join(work_lines)
                    if work_lines
                    else "No substantive phase summary was available for this thread."
                ),
            )
        )
    return tuple(jobs)


def _agent_name_json(
    job: AgentNameJob, result: AgentNameResult
) -> dict[str, JsonValue]:
    return {
        "schema_version": 1,
        "agent": {
            "thread_id": job.thread_id,
            "official_path": job.official_path,
            "coordinator_nickname": job.coordinator_nickname,
            "role": job.role,
            "depth": job.depth,
            "parent_official_path": job.parent_official_path,
        },
        "name": {
            "thread_id": result.thread_id,
            "short_name": result.short_name,
            "rationale": result.rationale,
            "model": result.model,
            "prompt_version": result.prompt_version,
            "input_hash": result.input_hash,
            "generated_at": result.generated_at,
        },
    }


def _write_agent_name_data(
    archive: Path,
    team_slug: str,
    jobs: Sequence[AgentNameJob],
    results: Mapping[str, AgentNameResult],
) -> int:
    changed = 0
    for job in jobs:
        result = results[job.thread_id]
        changed += int(
            write_json_if_changed(
                _summary_root(archive, team_slug)
                / "agents"
                / f"{job.thread_id}.json",
                _agent_name_json(job, result),
            )
        )
    return changed


def _nonempty_string(value: JsonValue, where: str) -> str:
    result = as_string(value, where).strip()
    if not result:
        raise ValueError(f"{where}: must not be empty")
    return result


def _load_agent_names(
    archive: Path, team: TeamData, selected_ids: frozenset[str]
) -> dict[str, AgentNameResult]:
    results: dict[str, AgentNameResult] = {}
    for agent in team.agents:
        if agent.thread_id not in selected_ids:
            continue
        path = (
            _summary_root(archive, team.team_slug)
            / "agents"
            / f"{agent.thread_id}.json"
        )
        if not path.is_file():
            raise ValueError(
                f"missing hindsight name for {agent.agent_path}; run `agent-team-timeline summarize`"
            )
        root = as_object(read_json(path), str(path))
        if root.get("schema_version") != 1:
            raise ValueError(f"unsupported agent-name schema at {path}")
        identity = as_object(root.get("agent"), f"{path}.agent")
        recorded_thread = _nonempty_string(
            identity.get("thread_id"), f"{path}.agent.thread_id"
        )
        recorded_path = _nonempty_string(
            identity.get("official_path"), f"{path}.agent.official_path"
        )
        recorded_depth = as_int(identity.get("depth"), f"{path}.agent.depth")
        if (
            recorded_thread != agent.thread_id
            or recorded_path != agent.agent_path
            or recorded_depth != agent.depth
        ):
            raise ValueError(
                f"stale hindsight name metadata for {agent.agent_path}; run summarize"
            )
        raw_name = as_object(root.get("name"), f"{path}.name")
        result = AgentNameResult(
            thread_id=_nonempty_string(
                raw_name.get("thread_id"), f"{path}.name.thread_id"
            ),
            short_name=_nonempty_string(
                raw_name.get("short_name"), f"{path}.name.short_name"
            ),
            rationale=_nonempty_string(
                raw_name.get("rationale"), f"{path}.name.rationale"
            ),
            model=_nonempty_string(raw_name.get("model"), f"{path}.name.model"),
            prompt_version=_nonempty_string(
                raw_name.get("prompt_version"), f"{path}.name.prompt_version"
            ),
            input_hash=_nonempty_string(
                raw_name.get("input_hash"), f"{path}.name.input_hash"
            ),
            generated_at=_nonempty_string(
                raw_name.get("generated_at"), f"{path}.name.generated_at"
            ),
        )
        if result.thread_id != agent.thread_id:
            raise ValueError(f"agent-name result thread mismatch at {path}")
        results[agent.thread_id] = result
    return results


def _rollup_jobs_for_level(
    team: TeamData,
    periods: Sequence[Period],
    phases: Sequence[PhaseWindow],
    phase_results: Mapping[str, SummaryResult],
    lower_periods: Sequence[Period],
    lower_results: Mapping[str, SummaryResult],
    completed_same_level: Sequence[tuple[Period, SummaryResult]],
    terms: Sequence[GlossaryTerm],
) -> tuple[SummaryJob, ...]:
    jobs: list[SummaryJob] = []
    for period in periods:
        own_phases = _phases_in(period, phases)
        lower = [
            item
            for item in lower_periods
            if item.start_ms >= period.start_ms and item.end_ms <= period.end_ms
        ]
        transcript_parts = [
            (
                item.start_ms,
                _result_line(lower_results[_period_key(item)], item.label, item.start_ms),
            )
            for item in lower
        ]
        # Calendar levels do not nest perfectly: an ISO week can straddle a month. Summarize only
        # fully-contained lower periods and fill uncovered boundary time from phase summaries, so
        # January's monthly report never imports work from December or February.
        uncovered_phases = [
            phase
            for phase in own_phases
            if not any(
                item.start_ms <= phase.start_ms and phase.end_ms <= item.end_ms
                for item in lower
            )
        ]
        transcript_parts.extend(
            (
                phase.start_ms,
                _result_line(
                    phase_results[phase.summary_key], phase.agent_label, phase.start_ms
                ),
            )
            for phase in uncovered_phases
        )
        transcript = "\n\n".join(
            line for _, line in sorted(transcript_parts, key=lambda item: item[0])
        )
        prior = [
            (item, result)
            for item, result in completed_same_level
            if item.end_ms <= period.start_ms
        ][-10:]
        prior_context = "\n\n".join(
            _result_line(result, item.label, item.start_ms) for item, result in prior
        )
        stats = aggregate_stats(own_phases)
        jobs.append(
            SummaryJob(
                key=f"rollup:{period.kind}:{period.key}",
                team_slug=team.team_slug,
                agent_label=f"{period.kind.title()} super-summary · {period.label}",
                start_ms=period.start_ms,
                end_ms=period.end_ms,
                prior_context=prior_context,
                transcript=transcript,
                glossary=glossary_prompt_text(
                    [term for term in terms if term.introduced_at_ms < period.end_ms]
                ),
                stats=stats.to_mapping(),
            )
        )
    return tuple(jobs)


def _accumulate_stats(stats: Sequence[SummaryRunStats]) -> SummaryRunStats:
    newly_spent = TokenUsage()
    artifact_generation = TokenUsage()
    for item in stats:
        newly_spent += item.newly_spent_usage
        artifact_generation += item.artifact_generation_usage
    return SummaryRunStats(
        hits=sum(item.hits for item in stats),
        misses=sum(item.misses for item in stats),
        batches=sum(item.batches for item in stats),
        newly_spent_usage=newly_spent,
        newly_spent_unknown_receipts=sum(
            item.newly_spent_unknown_receipts for item in stats
        ),
        artifact_generation_usage=artifact_generation,
        artifact_generation_unknown_receipts=sum(
            item.artifact_generation_unknown_receipts for item in stats
        ),
        unknown_legacy_artifacts=sum(
            item.unknown_legacy_artifacts for item in stats
        ),
    )


def _period_range(
    team: TeamData, phases: Sequence[PhaseWindow]
) -> tuple[int, int]:
    """Return an inclusive range for calendar rollups from half-open phase/window bounds."""

    phase_start = min((phase.start_ms for phase in phases), default=None)
    phase_end = max((phase.end_ms for phase in phases), default=None)
    start_ms = team.window_start_ms if team.window_start_ms is not None else phase_start
    end_ms = team.window_end_ms if team.window_end_ms is not None else phase_end
    if start_ms is None or end_ms is None:
        raise ValueError("selected timeline range contains no activity and lacks both date bounds")
    if end_ms <= start_ms:
        raise ValueError("selected timeline range is empty")
    return start_ms, end_ms - 1


def _summarize_archive_locked(
    archive: Path,
    team_slug: str,
    backend: str,
    model: str,
    *,
    max_workers: int = 3,
    batch_size: int = 6,
    name_batch_size: int = 12,
    phase_minutes: int = 30,
    context_chars: int = 16_000,
    transcript_chars: int = 30_000,
    codex_command: Sequence[str] = ("codex",),
    reasoning_effort: str | None = None,
) -> SummarizeReport:
    """Fill only missing/changed structured summaries; never format the website."""

    team = load_archived_team(archive, team_slug)
    phases = build_phases(
        team,
        phase_minutes=phase_minutes,
        context_chars=context_chars,
        transcript_chars=transcript_chars,
    )
    terms = _glossary_terms(team)
    cache = _summary_root(archive, team_slug) / "cache"
    phase_results, phase_stats = summarize_jobs(
        _phase_jobs(team, phases, terms),
        cache,
        backend,
        model,
        max_workers=max_workers,
        batch_size=batch_size,
        codex_command=codex_command,
        reasoning_effort=reasoning_effort,
    )
    changed = _write_phase_data(archive, team_slug, phases, phase_results)
    name_jobs = _agent_name_jobs(team, phases, phase_results)
    agent_names, name_stats = name_agents(
        name_jobs,
        _summary_root(archive, team_slug) / "name_cache",
        backend,
        model,
        max_workers=max_workers,
        batch_size=name_batch_size,
        codex_command=codex_command,
        reasoning_effort=reasoning_effort,
    )
    changed += _write_agent_name_data(
        archive, team_slug, name_jobs, agent_names
    )

    start_ms, end_ms = _period_range(team, phases)
    periods = periods_for_range(start_ms, end_ms, team.display_timezone, team.team_slug)
    by_kind = {
        kind: [period for period in periods if period.kind == kind]
        for kind in ("daily", "weekly", "monthly", "quarterly")
    }
    all_results: dict[str, SummaryResult] = {}
    backend_stats: list[SummaryRunStats] = [phase_stats, name_stats]
    previous_periods: list[Period] = []
    previous_results: dict[str, SummaryResult] = {}
    for kind in ("daily", "weekly", "monthly", "quarterly"):
        current = by_kind[kind]
        completed: list[tuple[Period, SummaryResult]] = []
        # Same-level context is an intentional chronology dependency: day N reads up to ten
        # earlier daily summaries (and likewise for weeks/months/quarters). Generate those jobs
        # one at a time so the actual prior summaries—not empty placeholders—enter the next
        # content hash and terminology context.
        for period in current:
            jobs = _rollup_jobs_for_level(
                team,
                (period,),
                phases,
                phase_results,
                previous_periods,
                previous_results,
                completed,
                terms,
            )
            results, stats = summarize_jobs(
                jobs,
                cache,
                backend,
                model,
                max_workers=max_workers,
                batch_size=batch_size,
                codex_command=codex_command,
                reasoning_effort=reasoning_effort,
            )
            backend_stats.append(stats)
            result = results[jobs[0].key]
            all_results[_period_key(period)] = result
            completed.append((period, result))
        previous_periods = current
        previous_results = {
            _period_key(period): all_results[_period_key(period)] for period in current
        }

    for period in periods:
        result = all_results[_period_key(period)]
        obj: dict[str, JsonValue] = {
            "kind": period.kind,
            "key": period.key,
            "start_ms": period.start_ms,
            "end_ms": period.end_ms,
            "partial": period.partial,
            "summary": _summary_json(result),
        }
        changed += int(
            write_json_if_changed(
                _summary_root(archive, team_slug)
                / "rollups"
                / period.kind
                / f"{period.key}.json",
                obj,
            )
        )
    glossary_obj: dict[str, JsonValue] = {
        "terms": [
            {
                "term": term.term,
                "introduced_at_ms": term.introduced_at_ms,
                "occurrences": term.occurrences,
                "context": term.context,
                "week": term.week,
            }
            for term in terms
        ]
    }
    changed += int(
        write_json_if_changed(
            _summary_root(archive, team_slug) / "glossary.json", glossary_obj
        )
    )
    combined = _accumulate_stats(backend_stats)
    return SummarizeReport(
        backend=backend,
        model=model,
        reasoning_effort=reasoning_effort,
        phases=len(phases),
        rollups=len(periods),
        agent_names=len(agent_names),
        glossary_terms=len(terms),
        cache_hits=combined.hits,
        cache_misses=combined.misses,
        backend_batches=combined.batches,
        newly_spent_usage=combined.newly_spent_usage,
        newly_spent_unknown_receipts=combined.newly_spent_unknown_receipts,
        artifact_generation_usage=combined.artifact_generation_usage,
        artifact_generation_unknown_receipts=(
            combined.artifact_generation_unknown_receipts
        ),
        unknown_legacy_artifacts=combined.unknown_legacy_artifacts,
        usage_run_paths=tuple(
            str(item.usage_run_path.relative_to(archive))
            for item in backend_stats
            if item.usage_run_path is not None
        ),
        files_changed=changed,
    )


def summarize_archive(
    archive: Path,
    team_slug: str,
    backend: str,
    model: str,
    *,
    max_workers: int = 3,
    batch_size: int = 6,
    name_batch_size: int = 12,
    phase_minutes: int = 30,
    context_chars: int = 16_000,
    transcript_chars: int = 30_000,
    codex_command: Sequence[str] = ("codex",),
    reasoning_effort: str | None = None,
) -> SummarizeReport:
    """Fill structured summaries/names while serializing token-spending cache misses."""

    with _archive_writer_lock(archive):
        return _summarize_archive_locked(
            archive,
            team_slug,
            backend,
            model,
            max_workers=max_workers,
            batch_size=batch_size,
            name_batch_size=name_batch_size,
            phase_minutes=phase_minutes,
            context_chars=context_chars,
            transcript_chars=transcript_chars,
            codex_command=codex_command,
            reasoning_effort=reasoning_effort,
        )


def _load_phase_summaries(
    archive: Path, team_slug: str, phases: Sequence[PhaseWindow]
) -> dict[str, SummaryResult]:
    result: dict[str, SummaryResult] = {}
    for phase in phases:
        path = _summary_root(archive, team_slug) / "phases" / f"{phase.phase_id}.json"
        if not path.is_file():
            raise ValueError(
                f"missing summary for {phase.phase_id}; run `agent-team-timeline summarize`"
            )
        obj = as_object(read_json(path), str(path))
        result[phase.summary_key] = _summary_from_json(obj.get("summary"), str(path))
    return result


def _load_glossary(archive: Path, team_slug: str) -> tuple[GlossaryTerm, ...]:
    path = _summary_root(archive, team_slug) / "glossary.json"
    obj = as_object(read_json(path), str(path))
    result: list[GlossaryTerm] = []
    for raw in as_array(obj.get("terms"), f"{path}.terms"):
        item = as_object(raw, f"{path}.terms[]")
        result.append(
            GlossaryTerm(
                term=as_string(item.get("term"), "term"),
                introduced_at_ms=as_int(item.get("introduced_at_ms"), "introduced_at_ms"),
                occurrences=as_int(item.get("occurrences"), "occurrences"),
                context=as_string(item.get("context"), "context"),
                week=as_string(item.get("week"), "week"),
            )
        )
    return tuple(result)


def build_archive(
    archive: Path, team_slug: str, *, phase_minutes: int = 30
) -> dict[str, int]:
    """Regenerate Markdown/HTML/JSON exclusively from cached structured data."""

    team = load_archived_team(archive, team_slug)
    phases = build_phases(team, phase_minutes=phase_minutes)
    phase_results = _load_phase_summaries(archive, team_slug, phases)
    agent_names = _load_agent_names(archive, team, phase_agent_ids(team, phases))
    start_ms, end_ms = _period_range(team, phases)
    periods = periods_for_range(start_ms, end_ms, team.display_timezone, team.team_slug)
    rollup_results: dict[str, SummaryResult] = {}
    rollup_stats: dict[str, PhaseStats] = {}
    for period in periods:
        path = (
            _summary_root(archive, team_slug)
            / "rollups"
            / period.kind
            / f"{period.key}.json"
        )
        if not path.is_file():
            raise ValueError(f"missing {period.kind} summary {period.key}; run summarize")
        obj = as_object(read_json(path), str(path))
        key = _period_key(period)
        rollup_results[key] = _summary_from_json(obj.get("summary"), str(path))
        rollup_stats[key] = aggregate_stats(_phases_in(period, phases))
    terms = _load_glossary(archive, team_slug)
    pull_cache = load_pull_request_metadata_cache(
        pull_metadata_path(archive, team_slug)
    )
    pull_metadata = {record.key: record for record in pull_cache.records}
    return render_archive(
        archive,
        team,
        phases,
        phase_results,
        periods,
        rollup_results,
        rollup_stats,
        terms,
        agent_names,
        pull_metadata,
    )


def record_run(
    archive: Path,
    command: Sequence[str],
    started_at: str,
    status: str,
    team_slug: str,
    ingest: IngestReport | None,
    summaries: SummarizeReport | None,
    build: Mapping[str, int] | None,
    error: str | None = None,
) -> Path:
    """Append immutable run provenance and update the small archive manifest."""

    _ensure_archive(archive, team_slug, create=True)
    completed_at = utc_now()
    stamp = completed_at.replace("-", "").replace(":", "").replace(".", "")
    stamp = stamp.replace("+0000", "Z")
    material = "\0".join(command) + "\0" + completed_at
    run_id = stamp + "-" + hashlib.sha256(material.encode("utf-8")).hexdigest()[:8]
    run_obj: dict[str, JsonValue] = {
        "schema_version": 1,
        "run_id": run_id,
        "tool_version": __version__,
        "started_at": started_at,
        "completed_at": completed_at,
        "status": status,
        "team_slug": team_slug,
        "command": list(command),
        "ingest": ingest.to_json_obj() if ingest else None,
        "summaries": summaries.to_json_obj() if summaries else None,
        "build": {key: value for key, value in sorted((build or {}).items())},
        "error": error,
    }
    path = archive / "runs" / f"{run_id}.json"
    write_json_if_changed(path, run_obj)
    manifest_path = archive / "manifest.json"
    created_at = started_at
    run_count = 0
    old: dict[str, JsonValue] = {}
    if manifest_path.is_file():
        old = as_object(read_json(manifest_path), str(manifest_path))
        old_created = old.get("created_at")
        if isinstance(old_created, str):
            created_at = old_created
        old_count = old.get("run_count")
        if isinstance(old_count, int) and not isinstance(old_count, bool):
            run_count = old_count
    manifest: dict[str, JsonValue] = dict(old)
    old_teams = old.get("teams")
    teams = (
        {value for value in old_teams if isinstance(value, str)}
        if isinstance(old_teams, list)
        else set()
    )
    teams.add(team_slug)
    team_values: list[JsonValue] = []
    for team in sorted(teams):
        team_values.append(team)
    manifest.update({
        "schema_version": 1,
        "tool_version": __version__,
        "created_at": created_at,
        "last_run_at": completed_at,
        "last_run_id": run_id,
        "last_run_status": status,
        "run_count": run_count + 1,
        "teams": team_values,
    })
    if ingest is not None:
        manifest["latest_source_digest"] = ingest.source_digest
    write_json_if_changed(manifest_path, manifest)
    return path


__all__ = [
    "IngestReport",
    "SummarizeReport",
    "build_archive",
    "ingest_claude",
    "ingest_codex",
    "load_archived_team",
    "record_run",
    "source_digest",
    "summarize_archive",
    "utc_now",
]
