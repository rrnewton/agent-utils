"""Idempotent ingest -> summarize -> format pipeline for timeline archives."""

from __future__ import annotations

import fcntl
import hashlib
import os
import re
import stat
from collections.abc import Iterator, Mapping, Sequence
from contextlib import ExitStack, contextmanager
from dataclasses import dataclass, replace
from datetime import datetime, timezone
from pathlib import Path
from typing import TYPE_CHECKING

from agent_team_timeline import __version__
from agent_team_timeline.archive import (
    JsonValue,
    as_array,
    as_int,
    as_object,
    as_string,
    canonical_json,
    narrow_json,
    read_json,
    write_json_if_changed,
    write_text_if_changed,
)
from agent_team_timeline.artifacts import (
    ArtifactCatalog,
    artifact_catalog_from_json,
    canonical_repository_url,
    extract_artifacts,
)
from agent_team_timeline.codex import (
    CodexContinuationLink,
    CodexParseError,
    CodexSourceCopy,
    codex_identity_metadata,
    load_codex_team,
    snapshot_codex_lineage,
)
from agent_team_timeline.claude import (
    ClaudeParseError,
    ClaudeSourceCopy,
    load_claude_team,
    snapshot_claude_lineage,
)
from agent_team_timeline.model import Agent, Event, TeamData, ToolCall, source_digest
from agent_team_timeline.model_io import team_from_json_obj
from agent_team_timeline.naming import (
    AgentNameJob,
    AgentNameResult,
    input_hash_for_provenance as agent_name_input_hash_for_provenance,
    name_agents,
)
from agent_team_timeline.github_metadata import load_pull_request_metadata_cache
from agent_team_timeline.github_enrich import pull_metadata_path
from agent_team_timeline.identity import (
    HostIdentity,
    IdentityOverrides,
    ProjectIdentity,
    SiteIdentity,
    infer_structured_identity,
    merge_site_identity,
    site_identity_from_json_obj,
)
from agent_team_timeline.orc import (
    OrcContinuationLink,
    OrcContinuationSpec,
    OrcParseError,
    OrcSourceCopy,
    load_orc_team,
    prune_orc_staging,
    prune_orc_snapshot_objects,
    snapshot_orc_lineage,
)
from agent_team_timeline.periods import (
    DEFAULT_ROLLUP_KINDS,
    ROLLUP_KINDS,
    Period,
    periods_for_range,
)
from agent_team_timeline.phases import (
    PhaseStats,
    PhaseWindow,
    aggregate_stats,
    build_phases,
    phase_agent_ids,
)
from agent_team_timeline.render import render_archive
from agent_team_timeline.summarize import (
    GLOSSARY_DEFINITION_STYLE,
    PLAIN_LANGUAGE_ROLLUP_STYLE,
    PROJECT_OVERVIEW_STYLE,
    SummaryJob,
    SummaryResult,
    SummaryRunStats,
    TECHNICAL_ROLLUP_STYLE,
    WorkBullet,
    clean_summary_prose,
    clean_summary_result,
    input_hash_for_provenance as summary_input_hash_for_provenance,
    knowledge_text_has_link,
    summarize_jobs,
)
from agent_team_timeline.summary_registry import (
    AGENT_LIFETIME_SUMMARIZER,
    ContextComponent,
    ContextCoverage,
    GLOSSARY_DEFINITION_SUMMARIZER,
    PROJECT_OVERVIEW_SUMMARIZER,
    registry_json_obj,
    summarizer_change_for_prompt,
)
from agent_team_timeline.summary_artifacts import (
    ARTIFACT_ENVELOPE_FORMAT,
    ARTIFACT_ENVELOPE_VERSION,
    SummaryArtifactProvenance,
)
from agent_team_timeline.summary_catalog import (
    SummaryArtifactCatalog,
    SummaryArtifactReference,
    load_summary_catalog,
    merge_summary_catalog,
    select_summary_artifact,
)
from agent_team_timeline.token_usage import TokenUsage, resolve_service_tier
from agent_team_timeline.terminology import (
    GlossaryTerm,
    TermSource,
    glossary_prompt_text,
    glossary_term_id,
    plain_language_context_text,
    scan_terminology,
)
from agent_team_timeline.window import DateWindow, apply_date_window

if TYPE_CHECKING:
    from agent_team_timeline.transcript_export import (
        PromptAuthorshipRule,
        TranscriptExportReport,
    )


_TEAM_SLUG = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")
_ARCHIVE_ID = re.compile(r"[A-Za-z0-9][A-Za-z0-9._-]{0,127}")
_ARCHIVE_MARKER = ".agent-team-timeline.json"
_ARCHIVE_LOCK = ".agent-team-timeline.lock"
_PROJECT_OVERVIEW_SCHEMA_VERSION = 3
_GLOSSARY_SCHEMA_VERSION = 3
_ORC_NORMALIZER_SCHEMA_VERSION = 3
_OVERVIEW_CONTEXT_CHARS = 48_000
_TERM_EVIDENCE_LIMIT = 6


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


@contextmanager
def _archive_writer_locks(*archives: Path) -> Iterator[None]:
    """Acquire several archive locks in canonical order without alias deadlocks."""

    ordered = {
        str(archive.resolve()): archive.resolve()
        for archive in archives
    }
    with ExitStack() as stack:
        for key in sorted(ordered):
            stack.enter_context(_archive_writer_lock(ordered[key]))
        yield


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
    artifacts: int = 0
    projects: int = 0

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
            "artifacts": self.artifacts,
            "projects": self.projects,
        }


@dataclass(frozen=True)
class SummarizeReport:
    """Cache, backend, and output counts from one summarization run."""

    backend: str
    model: str
    reasoning_effort: str | None
    service_tier: str | None
    phases: int
    rollups: int
    agent_names: int
    glossary_terms: int
    project_overviews: int
    glossary_definitions: int
    catalog_artifacts: int
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
            "service_tier": self.service_tier,
            "phases": self.phases,
            "rollups": self.rollups,
            "plain_language_rollups": self.rollups,
            "rollup_summary_artifacts": self.rollups * 2,
            "agent_names": self.agent_names,
            "glossary_terms": self.glossary_terms,
            "project_overviews": self.project_overviews,
            "glossary_definitions": self.glossary_definitions,
            "catalog_artifacts": self.catalog_artifacts,
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
    sources = tuple(
        replace(
            source,
            working_directory=None,
            repository_url=canonical_repository_url(source.repository_url),
        )
        for source in team.sources
    )
    return replace(team, sources=sources, tool_calls=tools)


def _raw_team_path(archive: Path, team_slug: str) -> Path:
    _validate_team_slug(team_slug)
    return archive / "teams" / team_slug / "raw" / "team.json"


def _artifact_catalog_path(archive: Path, team_slug: str) -> Path:
    _validate_team_slug(team_slug)
    return archive / "teams" / team_slug / "raw" / "artifacts.json"


def _summary_root(archive: Path, team_slug: str) -> Path:
    _validate_team_slug(team_slug)
    return archive / "teams" / team_slug / "summary_data"


def _summary_projection_missing(path: Path) -> bool:
    """Return true only for an absent optional projection, not a malformed path."""

    try:
        mode = path.lstat().st_mode
    except FileNotFoundError:
        return True
    if not stat.S_ISREG(mode):
        raise ValueError(f"{path}: summary projection is not a regular file")
    return False


def _source_snapshot_root(archive: Path, team_slug: str) -> Path:
    _validate_team_slug(team_slug)
    return archive / "teams" / team_slug / "source_snapshots"


def _source_manifest_path(archive: Path, team_slug: str) -> Path:
    _validate_team_slug(team_slug)
    return archive / "teams" / team_slug / "raw" / "source-manifest.json"


def _normalized_generation_path(archive: Path, team_slug: str) -> Path:
    _validate_team_slug(team_slug)
    return archive / "teams" / team_slug / "raw" / "normalized-generation.json"


def _file_sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        while True:
            chunk = handle.read(1024 * 1024)
            if not chunk:
                return digest.hexdigest()
            digest.update(chunk)


def _canonical_json_file_sha256(path: Path) -> str:
    encoded = canonical_json(read_json(path)).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def _site_identity_path(archive: Path, team_slug: str) -> Path:
    _validate_team_slug(team_slug)
    return archive / "teams" / team_slug / "raw" / "site-identity.json"


def load_site_identity(
    archive: Path, team: TeamData, *, required: bool = False
) -> SiteIdentity:
    """Load standalone site identity, with a team-data fallback when absent."""

    path = _site_identity_path(archive, team.team_slug)
    if not path.is_file():
        if required:
            raise ValueError(f"missing site identity at {path}")
        return SiteIdentity(
            team_slug=team.team_slug,
            projects=(),
            hosts=(),
            display_timezone=team.display_timezone,
            display_timezone_source="legacy_team_data",
        )
    identity = site_identity_from_json_obj(read_json(path), str(path))
    if identity.team_slug != team.team_slug:
        raise ValueError(
            f"site identity team {identity.team_slug!r} does not match {team.team_slug!r}"
        )
    if identity.display_timezone != team.display_timezone:
        raise ValueError(
            "site identity display timezone differs from normalized team data; rerun ingest"
        )
    return identity


def _record_site_identity(
    archive: Path,
    team: TeamData,
    inferred: tuple[tuple[ProjectIdentity, ...], tuple[HostIdentity, ...]],
    overrides: IdentityOverrides | None,
) -> int:
    """Merge durable identity evidence and atomically update its standalone record."""

    path = _site_identity_path(archive, team.team_slug)
    previous = (
        site_identity_from_json_obj(read_json(path), str(path))
        if path.is_file()
        else None
    )
    if previous is not None and previous.team_slug != team.team_slug:
        raise ValueError(
            f"site identity team {previous.team_slug!r} does not match {team.team_slug!r}"
        )
    selected = overrides or IdentityOverrides()
    identity = merge_site_identity(
        team.team_slug,
        team.display_timezone,
        selected.display_timezone_source,
        inferred[0],
        inferred[1],
        selected.projects,
        selected.hosts,
        previous,
    )
    return int(_write_json_durable(path, narrow_json(identity.to_json_obj())))


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


def _manifest_window_value(date_window: DateWindow | None) -> JsonValue:
    if date_window is None:
        return None
    return narrow_json(date_window.to_json_obj())


def _manifest_window_bounds(
    value: JsonValue, where: str
) -> tuple[int | None, int | None]:
    if value is None:
        return None, None
    obj = as_object(value, where)
    base_fields = {"start_date", "end_date", "start_ms", "end_ms"}
    exact_fields = base_fields | {"start_time", "end_time"}
    if set(obj) not in (base_fields, exact_fields):
        raise ValueError(f"{where}: invalid date-window fields")
    start_value = obj.get("start_ms")
    end_value = obj.get("end_ms")
    start_ms = (
        None
        if start_value is None
        else as_int(start_value, f"{where}.start_ms")
    )
    end_ms = (
        None if end_value is None else as_int(end_value, f"{where}.end_ms")
    )
    return start_ms, end_ms


def _window_is_same_or_narrower(
    recorded: JsonValue,
    requested: JsonValue,
    where: str,
) -> bool:
    """Return whether a new half-open ingest window is a subset of the old one."""

    recorded_start, recorded_end = _manifest_window_bounds(
        recorded, where + ".recorded"
    )
    requested_start, requested_end = _manifest_window_bounds(
        requested, where + ".requested"
    )
    start_is_narrower = (
        recorded_start is None
        or (
            requested_start is not None
            and requested_start >= recorded_start
        )
    )
    end_is_narrower = (
        recorded_end is None
        or (
            requested_end is not None
            and requested_end <= recorded_end
        )
    )
    return start_is_narrower and end_is_narrower


def _validate_manifest_window(
    recorded: JsonValue,
    date_window: DateWindow | None,
    where: str,
    error_type: type[ValueError],
) -> None:
    requested = _manifest_window_value(date_window)
    if recorded == requested:
        return
    try:
        if _window_is_same_or_narrower(recorded, requested, where):
            return
    except ValueError as error:
        raise error_type(f"invalid archived date window: {error}") from error
    raise error_type(
        "archive date window may only stay unchanged or become narrower; "
        "choose a new output directory to widen it"
    )


@dataclass(frozen=True)
class _CodexManifestState:
    sources: tuple[CodexSourceCopy, ...]
    continuation_links: tuple[CodexContinuationLink, ...]
    continuation_thread_ids: tuple[str, ...]


def _load_source_manifest(
    archive: Path,
    team_slug: str,
    root_thread_id: str,
    date_window: DateWindow | None,
    requested_continuation_thread_ids: Sequence[str] = (),
) -> _CodexManifestState:
    path = _source_manifest_path(archive, team_slug)
    if not path.is_file():
        return _CodexManifestState(
            (), (), tuple(requested_continuation_thread_ids)
        )
    obj = as_object(read_json(path), str(path))
    if obj.get("schema_version") != 1 or obj.get("provider") != "codex":
        raise CodexParseError(f"invalid Codex source manifest at {path}")
    recorded_root = as_string(obj.get("root_thread_id"), f"{path}: root_thread_id")
    if recorded_root != root_thread_id:
        raise CodexParseError(
            f"source manifest belongs to root {recorded_root!r}, not {root_thread_id!r}"
        )
    _validate_manifest_window(
        obj.get("date_window"), date_window, str(path), CodexParseError
    )
    raw_sources = as_array(obj.get("sources"), f"{path}: sources")
    result: list[CodexSourceCopy] = []
    for index, raw_source in enumerate(raw_sources):
        source = as_object(raw_source, f"{path}: sources[{index}]")
        result.append(CodexSourceCopy.from_json_obj(source, f"{path}: sources[{index}]"))
    raw_continuations = obj.get("continuation_sessions")
    links: list[CodexContinuationLink] = []
    if raw_continuations is not None:
        for index, raw_link in enumerate(
            as_array(raw_continuations, f"{path}: continuation_sessions")
        ):
            link = as_object(
                raw_link, f"{path}: continuation_sessions[{index}]"
            )
            links.append(
                CodexContinuationLink.from_json_obj(
                    link, f"{path}: continuation_sessions[{index}]"
                )
            )
    recorded_ids = tuple(link.thread_id for link in links)
    requested_ids = tuple(requested_continuation_thread_ids)
    if requested_ids:
        if recorded_ids != requested_ids[: len(recorded_ids)]:
            raise CodexParseError(
                "requested continuation sessions do not extend the recorded ordered prefix"
            )
        effective_ids = requested_ids
    else:
        effective_ids = recorded_ids
    return _CodexManifestState(tuple(result), tuple(links), effective_ids)


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
    _validate_manifest_window(
        obj.get("date_window"), date_window, str(path), ClaudeParseError
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
    artifact_catalog = extract_artifacts(team)
    changed += int(
        _write_json_durable(
            _artifact_catalog_path(archive, team_slug),
            narrow_json(artifact_catalog.to_json_obj()),
        )
    )
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
        "sources": [source.to_json_obj() for source in archived.sources],
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
        artifacts=len(artifact_catalog.artifacts),
        projects=len(artifact_catalog.projects),
    )
    return archived, report


def _normalized_generation_value(
    archive: Path, team_slug: str, team: TeamData
) -> dict[str, JsonValue]:
    """Describe the complete normalized Orc generation committed by the marker."""

    return {
        "schema_version": 1,
        "tool": "agent-team-timeline",
        "normalizer_schema_version": _ORC_NORMALIZER_SCHEMA_VERSION,
        "provider": "orc",
        "source_manifest_sha256": _canonical_json_file_sha256(
            _source_manifest_path(archive, team_slug)
        ),
        "team_sha256": _file_sha256(_raw_team_path(archive, team_slug)),
        "artifact_catalog_sha256": _file_sha256(
            _artifact_catalog_path(archive, team_slug)
        ),
        "source_digest": source_digest(team),
    }


def _validate_normalized_generation(
    archive: Path, team_slug: str, team: TeamData
) -> None:
    marker_path = _normalized_generation_path(archive, team_slug)
    if marker_path.is_symlink() or not marker_path.is_file():
        raise ValueError(
            f"incomplete Orc normalized generation for {team_slug!r}; rerun ingest"
        )
    marker = as_object(read_json(marker_path), str(marker_path))
    expected_fields = {
        "schema_version",
        "tool",
        "normalizer_schema_version",
        "provider",
        "source_manifest_sha256",
        "team_sha256",
        "artifact_catalog_sha256",
        "source_digest",
    }
    if set(marker) != expected_fields:
        raise ValueError(
            f"invalid Orc normalized generation marker at {marker_path}"
        )
    expected = _normalized_generation_value(archive, team_slug, team)
    if marker != expected:
        raise ValueError(
            f"stale or incomplete Orc normalized generation for {team_slug!r}; "
            "rerun ingest"
        )


def _ingest_codex_locked(
    archive: Path,
    sessions_root: Path,
    root_thread_id: str,
    team_slug: str,
    display_timezone: str,
    date_window: DateWindow | None,
    identity_overrides: IdentityOverrides | None,
    continuation_thread_ids: Sequence[str],
) -> tuple[TeamData, IngestReport]:
    """Normalize one complete Codex lineage and write canonical raw JSON."""

    _ensure_archive(archive, team_slug, create=True)
    changed = int(_ensure_source_snapshots_ignored(archive))
    manifest_state = _load_source_manifest(
        archive,
        team_slug,
        root_thread_id,
        date_window,
        continuation_thread_ids,
    )
    snapshot_root = _source_snapshot_root(archive, team_slug)
    source_copies = (
        snapshot_codex_lineage(
            sessions_root,
            root_thread_id,
            snapshot_root,
            manifest_state.sources,
            utc_now(),
            manifest_state.continuation_thread_ids,
            manifest_state.continuation_links,
        )
        if manifest_state.continuation_thread_ids
        else snapshot_codex_lineage(
            sessions_root,
            root_thread_id,
            snapshot_root,
            manifest_state.sources,
            utc_now(),
        )
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
            continuation_links=source_copies.continuations,
        ),
        date_window,
    )
    if tuple(sorted(source.path for source in team.sources)) != tuple(sorted(allowed_sources)):
        raise CodexParseError(
            "parsed source set differs from the just-validated source snapshot manifest"
        )
    changed += _record_site_identity(
        archive,
        team,
        infer_structured_identity(
            codex_identity_metadata(
                snapshot_root,
                allowed_sources,
                root_thread_id,
                manifest_state.continuation_thread_ids,
            )
        ),
        identity_overrides,
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
    if source_copies.continuations:
        source_manifest["continuation_sessions"] = [
            link.to_json_obj() for link in source_copies.continuations
        ]
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
    identity_overrides: IdentityOverrides | None = None,
    continuation_thread_ids: Sequence[str] = (),
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
            identity_overrides,
            continuation_thread_ids,
        )


def _ingest_claude_locked(
    archive: Path,
    session_file: Path,
    team_slug: str,
    display_timezone: str,
    date_window: DateWindow | None,
    identity_overrides: IdentityOverrides | None,
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
    changed += _record_site_identity(
        archive, team, ((), ()), identity_overrides
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
    identity_overrides: IdentityOverrides | None = None,
) -> tuple[TeamData, IngestReport]:
    """Snapshot and normalize one Claude lineage as one serialized transaction."""

    with _archive_writer_lock(archive):
        return _ingest_claude_locked(
            archive,
            session_file,
            team_slug,
            display_timezone,
            date_window,
            identity_overrides,
        )


@dataclass(frozen=True)
class _OrcManifestState:
    sources: tuple[OrcSourceCopy, ...]
    continuation_links: tuple[OrcContinuationLink, ...]
    continuation_specs: tuple[OrcContinuationSpec, ...]


def _orc_continuation_specs(
    values: Sequence[str | OrcContinuationSpec], where: str
) -> tuple[OrcContinuationSpec, ...]:
    result = tuple(
        OrcContinuationSpec.from_value(value, f"{where}[{index}]")
        for index, value in enumerate(values)
    )
    session_ids = tuple(spec.session_id for spec in result)
    if len(set(session_ids)) != len(session_ids):
        raise OrcParseError(f"{where}: duplicate session ids are not allowed")
    return result


def _load_orc_source_manifest(
    archive: Path,
    team_slug: str,
    root_session_id: str,
    date_window: DateWindow | None,
    requested_continuations: Sequence[str | OrcContinuationSpec] = (),
) -> _OrcManifestState:
    requested_specs = _orc_continuation_specs(
        requested_continuations, "requested continuation sessions"
    )
    path = _source_manifest_path(archive, team_slug)
    if not path.is_file():
        return _OrcManifestState((), (), requested_specs)
    obj = as_object(read_json(path), str(path))
    schema_version = as_int(obj.get("schema_version"), f"{path}: schema_version")
    if schema_version not in (1, 2, 3, 4) or obj.get("provider") != "orc":
        raise OrcParseError(f"invalid Orc source manifest at {path}")
    if schema_version in (2, 3, 4):
        expected_fields = {
            "schema_version",
            "provider",
            "root_session_id",
            "source_root",
            "snapshot_root",
            "date_window",
            "sources",
        }
        if schema_version in (3, 4):
            expected_fields.add("continuation_sessions")
        if set(obj) != expected_fields:
            missing = sorted(expected_fields - set(obj))
            unknown = sorted(set(obj) - expected_fields)
            raise OrcParseError(
                f"invalid Orc source manifest fields at {path}: "
                f"missing={missing!r}, unknown={unknown!r}"
            )
        recorded_source_root = as_string(
            obj.get("source_root"), f"{path}: source_root"
        )
        if not Path(recorded_source_root).is_absolute():
            raise OrcParseError(f"{path}: source_root must be absolute")
        expected_snapshot_root = f"teams/{team_slug}/source_snapshots"
        recorded_snapshot_root = as_string(
            obj.get("snapshot_root"), f"{path}: snapshot_root"
        )
        if recorded_snapshot_root != expected_snapshot_root:
            raise OrcParseError(
                f"{path}: snapshot_root must be {expected_snapshot_root!r}"
            )
    recorded_root = as_string(obj.get("root_session_id"), f"{path}: root_session_id")
    if recorded_root != root_session_id:
        raise OrcParseError(
            f"source manifest belongs to root {recorded_root!r}, not {root_session_id!r}"
        )
    _validate_manifest_window(
        obj.get("date_window"), date_window, str(path), OrcParseError
    )
    result: list[OrcSourceCopy] = []
    for index, raw_source in enumerate(
        as_array(obj.get("sources"), f"{path}: sources")
    ):
        source = as_object(raw_source, f"{path}: sources[{index}]")
        result.append(
            OrcSourceCopy.from_json_obj(
                source,
                f"{path}: sources[{index}]",
                2 if schema_version in (3, 4) else schema_version,
            )
        )
    links: list[OrcContinuationLink] = []
    raw_continuations = obj.get("continuation_sessions")
    if raw_continuations is not None:
        if schema_version not in (3, 4):
            raise OrcParseError(
                f"{path}: continuation_sessions requires manifest schema version 3 or 4"
            )
        for index, raw_link in enumerate(
            as_array(raw_continuations, f"{path}: continuation_sessions")
        ):
            link = as_object(
                raw_link, f"{path}: continuation_sessions[{index}]"
            )
            has_bounded_fields = (
                "start_message_id" in link or "start_source_line" in link
            )
            if schema_version == 3 and has_bounded_fields:
                raise OrcParseError(
                    f"{path}: schema-v3 continuation record cannot contain bounded fields"
                )
            if schema_version == 4 and not (
                "start_message_id" in link and "start_source_line" in link
            ):
                raise OrcParseError(
                    f"{path}: schema-v4 continuation record lacks bounded fields"
                )
            links.append(
                OrcContinuationLink.from_json_obj(
                    link, f"{path}: continuation_sessions[{index}]"
                )
            )
    recorded_specs = tuple(
        OrcContinuationSpec.from_value(
            {
                "session_id": link.session_id,
                "start_message_id": link.start_message_id,
            },
            f"{path}: continuation_sessions[{index}]",
        )
        for index, link in enumerate(links)
    )
    if requested_specs:
        if recorded_specs != requested_specs[: len(recorded_specs)]:
            raise OrcParseError(
                "requested continuation sessions do not extend the recorded ordered prefix"
            )
        effective_specs = requested_specs
    else:
        effective_specs = recorded_specs
    return _OrcManifestState(tuple(result), tuple(links), effective_specs)


def _ingest_orc_locked(
    archive: Path,
    source_root: Path,
    root_session_id: str,
    team_slug: str,
    display_timezone: str,
    date_window: DateWindow | None,
    identity_overrides: IdentityOverrides | None,
    continuation_specs: Sequence[str | OrcContinuationSpec],
) -> tuple[TeamData, IngestReport]:
    """Snapshot and normalize one Orc coordinator lineage."""

    _ensure_archive(archive, team_slug, create=True)
    changed = int(_ensure_source_snapshots_ignored(archive))
    manifest_state = _load_orc_source_manifest(
        archive,
        team_slug,
        root_session_id,
        date_window,
        continuation_specs,
    )
    snapshot_root = _source_snapshot_root(archive, team_slug)
    changed += prune_orc_staging(snapshot_root)
    snapshot = snapshot_orc_lineage(
        source_root,
        root_session_id,
        snapshot_root,
        manifest_state.sources,
        utc_now(),
        manifest_state.continuation_specs,
        manifest_state.continuation_links,
    )
    changed += snapshot.files_changed
    resolved_specs = tuple(
        OrcContinuationSpec.from_value(
            {
                "session_id": link.session_id,
                "start_message_id": link.start_message_id,
            },
            f"resolved continuation sessions[{index}]",
        )
        for index, link in enumerate(snapshot.continuations)
    )
    if resolved_specs != manifest_state.continuation_specs:
        raise OrcParseError(
            "resolved continuation sessions differ from the requested ordered specs"
        )
    team = apply_date_window(
        load_orc_team(
            snapshot_root,
            root_session_id,
            team_slug,
            display_timezone,
            snapshot.sources,
            snapshot.continuations,
        ),
        date_window,
    )
    parsed_paths = tuple(sorted(source.path for source in team.sources))
    expected_paths = tuple(
        sorted(source.source_path for source in snapshot.sources)
    )
    if parsed_paths != expected_paths:
        raise OrcParseError(
            "parsed source set differs from the validated Orc source snapshots"
        )
    changed += _record_site_identity(
        archive, team, ((), ()), identity_overrides
    )
    source_manifest: dict[str, object] = {
        "schema_version": 4 if snapshot.continuations else 2,
        "provider": "orc",
        "root_session_id": root_session_id,
        "source_root": str(source_root.resolve()),
        "snapshot_root": f"teams/{team_slug}/source_snapshots",
        "date_window": date_window.to_json_obj() if date_window is not None else None,
        "sources": [source.to_json_obj() for source in snapshot.sources],
    }
    if snapshot.continuations:
        source_manifest["continuation_sessions"] = [
            link.to_json_obj() for link in snapshot.continuations
        ]
    changed += int(
        _write_json_durable(
            _source_manifest_path(archive, team_slug),
            narrow_json(source_manifest),
        )
    )
    archived, report = _write_ingested_team(
        archive, team_slug, team, date_window, changed
    )
    marker_changed = int(
        _write_json_durable(
            _normalized_generation_path(archive, team_slug),
            _normalized_generation_value(archive, team_slug, archived),
        )
    )
    gc_changed = prune_orc_snapshot_objects(snapshot_root, snapshot.sources)
    return archived, replace(
        report,
        files_changed=report.files_changed + marker_changed + gc_changed,
    )


def ingest_orc(
    archive: Path,
    source_root: Path,
    root_session_id: str,
    team_slug: str,
    display_timezone: str,
    date_window: DateWindow | None = None,
    identity_overrides: IdentityOverrides | None = None,
    continuation_specs: Sequence[str | OrcContinuationSpec] = (),
) -> tuple[TeamData, IngestReport]:
    """Snapshot and normalize one Orc lineage as a serialized raw-data transaction."""

    with _archive_writer_lock(archive):
        return _ingest_orc_locked(
            archive,
            source_root,
            root_session_id,
            team_slug,
            display_timezone,
            date_window,
            identity_overrides,
            continuation_specs,
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
    source_manifest_path = _source_manifest_path(archive, team_slug)
    if source_manifest_path.is_file():
        source_manifest = as_object(
            read_json(source_manifest_path), str(source_manifest_path)
        )
        if (
            source_manifest.get("provider") == "orc"
            and source_manifest.get("schema_version") in (2, 3, 4)
        ):
            _validate_normalized_generation(archive, team_slug, team)
    return team


def extract_transcripts_archive(
    archive: Path,
    team_slugs: Sequence[str] = (),
    authorship_rules: Sequence[PromptAuthorshipRule] | None = None,
) -> TranscriptExportReport:
    """Mechanically export coordinator prompts/responses for selected ingested teams.

    An empty selection means every team with normalized raw data in this archive. The archive
    writer lock makes the multi-file JSONL generation serial with provider ingestion and site
    builds; no summarizer or model adapter is reachable from this operation.
    """

    from agent_team_timeline.transcript_export import export_transcripts

    with _archive_writer_lock(archive):
        selected = tuple(team_slugs)
        if not selected:
            teams_root = archive / "teams"
            selected = tuple(
                sorted(
                    path.name
                    for path in teams_root.iterdir()
                    if path.is_dir()
                    and not path.is_symlink()
                    and (path / "raw" / "team.json").is_file()
                )
            ) if teams_root.is_dir() else ()
        if not selected:
            raise ValueError(f"no ingested teams found in {archive}")
        if len(set(selected)) != len(selected):
            raise ValueError("transcript extraction team selection contains duplicates")
        teams = tuple(load_archived_team(archive, team_slug) for team_slug in selected)
        return export_transcripts(archive, teams, authorship_rules)


def load_artifact_catalog(
    archive: Path, team_slug: str, team: TeamData | None = None
) -> ArtifactCatalog:
    """Load mechanical artifact data, accepting pre-artifact archives as empty.

    Existing summary caches remain valid because artifact extraction is independent of model
    inputs. Re-running ingest creates the catalog from source snapshots; until then, an archive
    without a catalog builds with an empty one rather than demanding summary regeneration.
    """

    archived_team = team if team is not None else load_archived_team(archive, team_slug)
    path = _artifact_catalog_path(archive, team_slug)
    if not path.is_file():
        return ArtifactCatalog(source_digest(archived_team), (), ())
    catalog = artifact_catalog_from_json(read_json(path))
    expected_digest = source_digest(archived_team)
    if catalog.source_digest != expected_digest:
        raise ValueError(
            f"stale artifact catalog for {team_slug!r}; rerun ingest before building"
        )
    return catalog


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


@dataclass(frozen=True)
class _KnowledgeEpoch:
    """An immutable source frontier for reusable generated knowledge."""

    epoch_id: str
    cutoff_ms: int
    cutoff_reason: str

    def to_json_obj(self) -> dict[str, JsonValue]:
        """Return stable epoch metadata for durable summary provenance."""

        return {
            "epoch_id": self.epoch_id,
            "cutoff_ms": self.cutoff_ms,
            "cutoff_reason": self.cutoff_reason,
        }


@dataclass(frozen=True)
class _ProjectOverviewInput:
    """Bounded early/root evidence used by the durable project-overview job."""

    epoch: _KnowledgeEpoch
    start_ms: int
    end_ms: int
    transcript: str
    event_ids: tuple[str, ...]
    context_sha256: str


class _ProjectOverviewSourceSetChanged(ValueError):
    """Signal a verified append/backfill that needs a fresh knowledge epoch."""

    def __init__(self, path: Path, previous_epoch: _KnowledgeEpoch) -> None:
        super().__init__(
            f"{path}.source: frozen overview source set changed with prior evidence intact"
        )
        self.previous_epoch = previous_epoch


@dataclass(frozen=True)
class _GlossaryEvidence:
    """One bounded source occurrence supplied to a glossary-definition job."""

    event_id: str
    thread_id: str
    at_ms: int
    kind: str
    context: str

    def to_json_obj(self) -> dict[str, JsonValue]:
        """Return stable provenance for the structured glossary artifact."""

        return {
            "event_id": self.event_id,
            "thread_id": self.thread_id,
            "at_ms": self.at_ms,
            "kind": self.kind,
            "context": self.context,
        }


@dataclass(frozen=True)
class _TermKnowledge:
    """Frozen definition evidence and chronological availability for one term."""

    evidence: tuple[_GlossaryEvidence, ...]
    available_at_ms: int
    definition_cutoff_ms: int
    definition_epoch_id: str


def _one_line(value: str) -> str:
    return " ".join(value.strip().split())


def _current_knowledge_epoch(team: TeamData) -> _KnowledgeEpoch:
    if team.window_end_ms is not None:
        cutoff_ms = team.window_end_ms
        reason = "archive-window-end"
    else:
        latest_ms = max((event.timestamp_ms for event in team.events), default=0)
        cutoff_ms = latest_ms + 1
        reason = "first-summary-source-frontier"
    digest = hashlib.sha256(
        f"{team.team_slug}\0{cutoff_ms}\0{reason}".encode("utf-8")
    ).hexdigest()[:20]
    return _KnowledgeEpoch(f"knowledge-{digest}", cutoff_ms, reason)


def _root_overview_input(
    team: TeamData, epoch: _KnowledgeEpoch | None = None
) -> _ProjectOverviewInput:
    selected_epoch = epoch if epoch is not None else _current_knowledge_epoch(team)
    events = sorted(
        (
            event
            for event in team.events
            if event.thread_id == team.root_thread_id
            and event.kind in {"user_prompt", "assistant_message"}
            and event.text
            and event.timestamp_ms < selected_epoch.cutoff_ms
        ),
        key=lambda event: (event.timestamp_ms, event.event_id),
    )
    return _overview_input_from_events(team, selected_epoch, events)


def _overview_input_from_events(
    team: TeamData,
    epoch: _KnowledgeEpoch,
    events: Sequence[Event],
) -> _ProjectOverviewInput:
    """Build overview evidence from an already ordered root-event sequence."""

    parts: list[str] = []
    event_ids: list[str] = []
    used = 0
    retained: list[Event] = []
    for event in events:
        assert event.text is not None
        prefix = f"[{event.timestamp_ms}] {event.kind}: "
        remaining = _OVERVIEW_CONTEXT_CHARS - used
        if remaining <= len(prefix):
            break
        line = prefix + _one_line(event.text)
        excerpt = line[:remaining]
        if excerpt:
            parts.append(excerpt)
            event_ids.append(event.event_id)
            retained.append(event)
            used += len(excerpt) + 2
        if len(excerpt) < len(line) or used >= _OVERVIEW_CONTEXT_CHARS:
            break
    if retained:
        start_ms = retained[0].timestamp_ms
        end_ms = retained[-1].timestamp_ms
    else:
        fallback = team.window_start_ms
        if fallback is None or fallback >= epoch.cutoff_ms:
            fallback = min(
                (
                    event.timestamp_ms
                    for event in team.events
                    if event.timestamp_ms < epoch.cutoff_ms
                ),
                default=max(0, epoch.cutoff_ms - 1),
            )
        start_ms = fallback
        end_ms = fallback
        parts.append("No root user or assistant transcript text was retained.")
    transcript = "\n\n".join(parts)
    return _ProjectOverviewInput(
        epoch=epoch,
        start_ms=start_ms,
        end_ms=end_ms,
        transcript=transcript,
        event_ids=tuple(event_ids),
        context_sha256=hashlib.sha256(transcript.encode("utf-8")).hexdigest(),
    )


def _recorded_overview_input(
    team: TeamData,
    epoch: _KnowledgeEpoch,
    event_ids: tuple[str, ...],
) -> _ProjectOverviewInput | None:
    """Reconstruct prior evidence by ID so new early events do not hide mutation."""

    if len(event_ids) != len(set(event_ids)):
        return None
    by_id = {event.event_id: event for event in team.events}
    events: list[Event] = []
    for event_id in event_ids:
        event = by_id.get(event_id)
        if (
            event is None
            or event.thread_id != team.root_thread_id
            or not event.text
            or event.timestamp_ms >= epoch.cutoff_ms
        ):
            return None
        if event.kind not in {"user_prompt", "assistant_message"}:
            # Claude ingestion formerly treated provider-authored user-role envelopes as owner
            # prompts. Reconstruct that prior prefix to prove their text/timestamp stayed intact
            # while allowing the corrected classification to start a new source-set epoch.
            if (
                event.kind != "system_input"
                or event.role != "user"
                or event.ingress_kind != "claude_system"
                or event.author_kind != "system"
            ):
                return None
            event = replace(event, kind="user_prompt")
        events.append(event)
    result = _overview_input_from_events(team, epoch, events)
    return result if result.event_ids == event_ids else None


def _renewed_project_overview_input(
    team: TeamData, previous_epoch: _KnowledgeEpoch
) -> _ProjectOverviewInput:
    """Create a deterministic new epoch for verified append/backfill evidence."""

    current = _root_overview_input(team)
    digest = hashlib.sha256(
        canonical_json(
            {
                "team_slug": team.team_slug,
                "previous_epoch_id": previous_epoch.epoch_id,
                "cutoff_ms": current.epoch.cutoff_ms,
                "cutoff_reason": current.epoch.cutoff_reason,
                "event_ids": list(current.event_ids),
                "context_sha256": current.context_sha256,
            }
        ).encode("utf-8")
    ).hexdigest()[:20]
    epoch = _KnowledgeEpoch(
        epoch_id=f"knowledge-{digest}",
        cutoff_ms=current.epoch.cutoff_ms,
        cutoff_reason=current.epoch.cutoff_reason,
    )
    return replace(current, epoch=epoch)


def _project_overview_job(
    team: TeamData, source: _ProjectOverviewInput
) -> SummaryJob:
    return SummaryJob(
        key=f"project-overview:{team.team_slug}",
        team_slug=team.team_slug,
        agent_label=f"{team.team_slug} project overview",
        start_ms=source.start_ms,
        end_ms=source.end_ms,
        prior_context="",
        transcript=source.transcript,
        glossary="",
        stats={
            "source_events": len(source.event_ids),
            "source_characters": len(source.transcript),
        },
        summary_style=PROJECT_OVERVIEW_STYLE,
        context_coverage=ContextCoverage(
            components=(
                ContextComponent(
                    "early_root_transcript",
                    _OVERVIEW_CONTEXT_CHARS,
                    min(_OVERVIEW_CONTEXT_CHARS, len(source.transcript)),
                    "characters",
                ),
            )
        ),
    )


def _term_context(text: str, term: str) -> str:
    clean = _one_line(text)
    position = clean.find(term)
    if position < 0:
        return clean[:520]
    start = max(0, position - 180)
    end = min(len(clean), position + len(term) + 320)
    return clean[start:end].strip()


def _definition_evidence(
    team: TeamData, term: GlossaryTerm, cutoff_ms: int | None = None
) -> tuple[_GlossaryEvidence, ...]:
    effective_cutoff = (
        _current_knowledge_epoch(team).cutoff_ms
        if cutoff_ms is None
        else cutoff_ms
    )
    result: list[_GlossaryEvidence] = []
    for event in sorted(team.events, key=lambda item: (item.timestamp_ms, item.event_id)):
        if (
            not event.text
            or term.term not in event.text
            or event.timestamp_ms >= effective_cutoff
        ):
            continue
        result.append(
            _GlossaryEvidence(
                event_id=event.event_id,
                thread_id=event.thread_id,
                at_ms=event.timestamp_ms,
                kind=event.kind,
                context=_term_context(event.text, term.term),
            )
        )
        if len(result) == _TERM_EVIDENCE_LIMIT:
            break
    return tuple(result)


def _definition_epoch_id(
    term_id: str,
    project_epoch_id: str,
    cutoff_ms: int,
    evidence: Sequence[_GlossaryEvidence],
) -> str:
    fields = [term_id, project_epoch_id, str(cutoff_ms)]
    fields.extend(
        f"{item.thread_id}:{item.event_id}:{item.at_ms}:{item.kind}:{item.context}"
        for item in evidence
    )
    digest = hashlib.sha256("\0".join(fields).encode("utf-8")).hexdigest()[:20]
    return f"definition-{digest}"


def _glossary_definition_job(
    team: TeamData,
    term: GlossaryTerm,
    evidence: Sequence[_GlossaryEvidence],
    project_overview: SummaryResult,
) -> SummaryJob:
    transcript_lines = [f"Glossary term (exact spelling): {term.term}", "Source occurrences:"]
    transcript_lines.extend(
        f"- [{item.at_ms}] {item.kind} in {item.thread_id}: {item.context}"
        for item in evidence
    )
    if not evidence:
        transcript_lines.append("- No retained source occurrence was found.")
    start_ms = evidence[0].at_ms if evidence else term.introduced_at_ms
    end_ms = evidence[-1].at_ms if evidence else term.introduced_at_ms
    return SummaryJob(
        key=f"glossary-definition:{term.term_id}",
        team_slug=team.team_slug,
        agent_label=f"Glossary definition · {term.term}",
        start_ms=start_ms,
        end_ms=end_ms,
        prior_context=(
            "Durable source-bounded project overview:\n" + project_overview.paragraph
        ),
        transcript="\n".join(transcript_lines),
        glossary=f"Exact glossary name: {term.term}",
        stats={"source_occurrences": len(evidence)},
        summary_style=GLOSSARY_DEFINITION_STYLE,
        context_coverage=ContextCoverage(
            components=(
                ContextComponent(
                    "source_occurrences",
                    _TERM_EVIDENCE_LIMIT,
                    min(_TERM_EVIDENCE_LIMIT, len(evidence)),
                    "occurrences",
                ),
                ContextComponent("project_overview", 1, 1, "artifacts"),
            )
        ),
        dependency_keys=(
            (
                project_overview.artifact_provenance.artifact_id
                if project_overview.artifact_provenance is not None
                else project_overview.input_hash
            ),
        ),
    )


def _definition_status(summary: SummaryResult) -> str:
    if summary.phrase == "Definition supported":
        return "supported"
    if summary.phrase == "Insufficient evidence":
        return "insufficient-evidence"
    raise ValueError(
        f"glossary definition {summary.key!r} has invalid evidence status {summary.phrase!r}"
    )


def _phase_jobs(
    team: TeamData,
    phases: Sequence[PhaseWindow],
    terms: Sequence[GlossaryTerm],
    context_chars: int,
) -> tuple[SummaryJob, ...]:
    jobs: list[SummaryJob] = []
    for phase in phases:
        chronological_terms = [
            term
            for term in terms
            if term.summary_available_at_ms < phase.end_ms
        ]
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
                context_coverage=ContextCoverage(
                    components=(
                        ContextComponent(
                            "ancestor_transcript",
                            context_chars,
                            min(context_chars, len(phase.prior_context)),
                            "characters",
                        ),
                    )
                ),
            )
        )
    return tuple(jobs)


def _summary_json(summary: SummaryResult) -> dict[str, JsonValue]:
    cleaned = clean_summary_result(summary)
    result: dict[str, JsonValue] = {
        "key": cleaned.key,
        "phrase": cleaned.phrase,
        "paragraph": cleaned.paragraph,
        "work_summary": [
            {"at_ms": item.at_ms, "text": item.text} for item in cleaned.work_summary
        ],
        "model": cleaned.model,
        "prompt_version": cleaned.prompt_version,
        "input_hash": cleaned.input_hash,
        "generated_at": cleaned.generated_at,
    }
    if cleaned.artifact_provenance is not None:
        result["artifact_provenance"] = cleaned.artifact_provenance.to_json_obj()
    return result


def _require_exact_keys(
    obj: Mapping[str, JsonValue], expected: set[str], where: str
) -> None:
    actual = set(obj)
    if actual == expected:
        return
    missing = sorted(expected - actual)
    extra = sorted(actual - expected)
    details: list[str] = []
    if missing:
        details.append("missing " + ", ".join(missing))
    if extra:
        details.append("extra " + ", ".join(extra))
    raise ValueError(f"{where}: schema mismatch ({'; '.join(details)})")


def _summary_from_json(value: JsonValue, where: str) -> SummaryResult:
    obj = as_object(value, where)
    base_keys = {
        "key",
        "phrase",
        "paragraph",
        "work_summary",
        "model",
        "prompt_version",
        "input_hash",
        "generated_at",
    }
    actual_keys = set(obj)
    if actual_keys not in (base_keys, base_keys | {"artifact_provenance"}):
        _require_exact_keys(obj, base_keys, where)
    raw_bullets = as_array(obj.get("work_summary"), f"{where}.work_summary")
    for index, raw_bullet in enumerate(raw_bullets):
        bullet = as_object(raw_bullet, f"{where}.work_summary[{index}]")
        _require_exact_keys(
            bullet, {"at_ms", "text"}, f"{where}.work_summary[{index}]"
        )
    bullets = tuple(
        WorkBullet(
            at_ms=as_int(as_object(item, f"{where}.work_summary[]").get("at_ms"), "at_ms"),
            text=as_string(
                as_object(item, f"{where}.work_summary[]").get("text"), "text"
            ),
        )
        for item in raw_bullets
    )
    return clean_summary_result(
        SummaryResult(
            key=as_string(obj.get("key"), f"{where}.key"),
            phrase=as_string(obj.get("phrase"), f"{where}.phrase"),
            paragraph=as_string(obj.get("paragraph"), f"{where}.paragraph"),
            work_summary=bullets,
            model=as_string(obj.get("model"), f"{where}.model"),
            prompt_version=as_string(
                obj.get("prompt_version"), f"{where}.prompt_version"
            ),
            input_hash=as_string(obj.get("input_hash"), f"{where}.input_hash"),
            generated_at=as_string(obj.get("generated_at"), f"{where}.generated_at"),
            artifact_provenance=(
                SummaryArtifactProvenance.from_json_obj(
                    obj.get("artifact_provenance"), f"{where}.artifact_provenance"
                )
                if "artifact_provenance" in obj
                else None
            ),
        )
    )


def _summary_from_catalog_reference(
    summary_root: Path, reference: SummaryArtifactReference
) -> SummaryResult:
    path = summary_root / reference.cache_path
    root = as_object(read_json(path), str(path))
    provenance = reference.provenance
    if root.get("format") == ARTIFACT_ENVELOPE_FORMAT:
        if as_int(root.get("schema_version"), f"{path}.schema_version") != (
            ARTIFACT_ENVELOPE_VERSION
        ):
            raise ValueError(f"{path}: unsupported model-artifact envelope")
        cached_provenance = SummaryArtifactProvenance.from_json_obj(
            root.get("artifact"), f"{path}.artifact"
        )
        if cached_provenance != provenance:
            raise ValueError(f"{path}: cache provenance differs from artifact catalog")
    elif not provenance.legacy_storage:
        raise ValueError(f"{path}: cataloged artifact lacks its common envelope")
    result = _summary_from_json(root.get("result"), f"{path}.result")
    if (
        result.input_hash != provenance.input_hash
        or result.model != provenance.model
        or result.prompt_version != provenance.prompt_version
    ):
        raise ValueError(f"{path}: cached result differs from artifact provenance")
    return replace(result, artifact_provenance=provenance)


def _select_catalog_summary(
    summary_root: Path,
    catalog: SummaryArtifactCatalog,
    logical_key: str,
    summarizer_id: str,
    fallback: SummaryResult,
) -> SummaryResult:
    reference = select_summary_artifact(
        catalog,
        logical_key,
        summarizer_id,
    )
    if reference is None:
        return fallback
    return _summary_from_catalog_reference(summary_root, reference)


def _select_catalog_summary_for_range(
    summary_root: Path,
    catalog: SummaryArtifactCatalog,
    logical_key: str,
    summarizer_id: str,
    start_ms: int,
    end_ms: int,
    fallback: SummaryResult,
) -> SummaryResult:
    """Use a catalog result only when its source interval exactly matches the view."""

    reference = select_summary_artifact(catalog, logical_key, summarizer_id)
    if reference is None or (
        reference.provenance.start_ms != start_ms
        or reference.provenance.end_ms != end_ms
    ):
        return fallback
    return _summary_from_catalog_reference(summary_root, reference)


def _select_catalog_summary_for_job(
    summary_root: Path,
    catalog: SummaryArtifactCatalog,
    job: SummaryJob,
    summarizer_id: str,
    fallback: SummaryResult,
) -> SummaryResult:
    """Select the strongest catalog artifact whose full source identity is current."""

    compatible = SummaryArtifactCatalog(
        team_slug=catalog.team_slug,
        records=tuple(
            reference
            for reference in catalog.records
            if reference.provenance.summarizer_id == summarizer_id
            and _summary_provenance_matches_job(reference.provenance, job)
        ),
    )
    reference = select_summary_artifact(
        compatible,
        job.key,
        summarizer_id,
    )
    if reference is None:
        return fallback
    return _summary_from_catalog_reference(summary_root, reference)


def _prior_catalog_rollups(
    summary_root: Path,
    catalog: SummaryArtifactCatalog,
    kind: str,
    summary_style: str,
    before_ms: int,
) -> list[tuple[Period, SummaryResult]]:
    plain_language = summary_style == PLAIN_LANGUAGE_ROLLUP_STYLE
    summarizer_id = (
        "plain-language-rollup" if plain_language else "technical-rollup"
    )
    key_prefix = "rollup-plain" if plain_language else "rollup"
    logical_prefix = f"{key_prefix}:{kind}:"
    logical_keys = sorted(
        {
            record.provenance.logical_key
            for record in catalog.records
            if record.provenance.summarizer_id == summarizer_id
            and record.provenance.logical_key.startswith(logical_prefix)
            and record.provenance.end_ms <= before_ms
        }
    )
    result: list[tuple[Period, SummaryResult]] = []
    for logical_key in logical_keys:
        reference = select_summary_artifact(
            catalog,
            logical_key,
            summarizer_id,
        )
        if reference is None:
            continue
        provenance = reference.provenance
        key = logical_key[len(logical_prefix) :]
        result.append(
            (
                Period(
                    kind=kind,
                    key=key,
                    label=f"Prior {kind} · {key}",
                    start_ms=provenance.start_ms,
                    end_ms=provenance.end_ms,
                    relative_path="",
                    partial=False,
                ),
                _summary_from_catalog_reference(summary_root, reference),
            )
        )
    result.sort(key=lambda item: (item[0].end_ms, item[0].key))
    return result[-10:]


def _validate_project_overview(summary: SummaryResult, where: str) -> None:
    try:
        summarizer_change_for_prompt(
            PROJECT_OVERVIEW_SUMMARIZER, summary.prompt_version
        )
    except ValueError as error:
        raise ValueError(f"{where}: unregistered project overview version") from error
    if summary.phrase not in {"Project overview supported", "Insufficient evidence"}:
        raise ValueError(f"{where}: invalid project-overview evidence status")
    if summary.work_summary:
        raise ValueError(f"{where}: project overview must not contain work bullets")
    if summary.phrase == "Insufficient evidence" and not summary.paragraph.startswith(
        "Insufficient evidence:"
    ):
        raise ValueError(f"{where}: insufficient overview lacks an evidence limit")
    if knowledge_text_has_link(summary.paragraph):
        raise ValueError(f"{where}: project overview contains an unverified link")


def _validate_glossary_definition(summary: SummaryResult, where: str) -> None:
    try:
        summarizer_change_for_prompt(
            GLOSSARY_DEFINITION_SUMMARIZER, summary.prompt_version
        )
    except ValueError as error:
        raise ValueError(f"{where}: unregistered glossary definition version") from error
    _definition_status(summary)
    if summary.work_summary:
        raise ValueError(f"{where}: glossary definition must not contain work bullets")
    if summary.phrase == "Insufficient evidence" and not summary.paragraph.startswith(
        "Insufficient evidence:"
    ):
        raise ValueError(f"{where}: insufficient definition lacks an evidence limit")
    if knowledge_text_has_link(summary.paragraph):
        raise ValueError(f"{where}: glossary definition contains an unverified link")


def _write_project_overview_data(
    archive: Path,
    team_slug: str,
    source: _ProjectOverviewInput,
    summary: SummaryResult,
) -> int:
    obj: dict[str, JsonValue] = {
        "schema_version": _PROJECT_OVERVIEW_SCHEMA_VERSION,
        "style": PROJECT_OVERVIEW_STYLE,
        "knowledge_epoch": source.epoch.to_json_obj(),
        "source": {
            "event_ids": list(source.event_ids),
            "start_ms": source.start_ms,
            "end_ms": source.end_ms,
            "context_sha256": source.context_sha256,
        },
        "summary": _summary_json(summary),
    }
    return int(
        write_json_if_changed(
            _summary_root(archive, team_slug) / "project_overview.json", obj
        )
    )


def _knowledge_epoch_from_json(value: JsonValue, where: str) -> _KnowledgeEpoch:
    obj = as_object(value, where)
    _require_exact_keys(obj, {"epoch_id", "cutoff_ms", "cutoff_reason"}, where)
    epoch_id = as_string(obj.get("epoch_id"), f"{where}.epoch_id")
    if not epoch_id.startswith("knowledge-"):
        raise ValueError(f"{where}.epoch_id: invalid knowledge epoch ID")
    cutoff_ms = as_int(obj.get("cutoff_ms"), f"{where}.cutoff_ms")
    cutoff_reason = as_string(obj.get("cutoff_reason"), f"{where}.cutoff_reason")
    if cutoff_reason not in {"archive-window-end", "first-summary-source-frontier"}:
        raise ValueError(f"{where}.cutoff_reason: invalid cutoff reason")
    return _KnowledgeEpoch(epoch_id, cutoff_ms, cutoff_reason)


def _frozen_project_overview_input(
    archive: Path, team: TeamData
) -> _ProjectOverviewInput | None:
    """Load and validate the projected immutable project evidence epoch."""

    path = _summary_root(archive, team.team_slug) / "project_overview.json"
    if not path.is_file():
        return None
    obj = as_object(read_json(path), str(path))
    if "schema_version" not in obj:
        return None
    version = as_int(obj.get("schema_version"), f"{path}.schema_version")
    if version != _PROJECT_OVERVIEW_SCHEMA_VERSION:
        # Older schemas did not have enough immutable provenance to distinguish append-only
        # growth from a historical mutation.
        return None
    _require_exact_keys(
        obj,
        {"schema_version", "style", "knowledge_epoch", "source", "summary"},
        str(path),
    )
    if as_string(obj.get("style"), f"{path}.style") != PROJECT_OVERVIEW_STYLE:
        raise ValueError(f"{path}: invalid project-overview style")
    epoch = _knowledge_epoch_from_json(
        obj.get("knowledge_epoch"), f"{path}.knowledge_epoch"
    )
    source = as_object(obj.get("source"), f"{path}.source")
    _require_exact_keys(
        source,
        {"event_ids", "start_ms", "end_ms", "context_sha256"},
        f"{path}.source",
    )
    event_ids = tuple(
        as_string(item, f"{path}.source.event_ids[]")
        for item in as_array(source.get("event_ids"), f"{path}.source.event_ids")
    )
    start_ms = as_int(source.get("start_ms"), f"{path}.source.start_ms")
    end_ms = as_int(source.get("end_ms"), f"{path}.source.end_ms")
    expected_sha256 = as_string(
        source.get("context_sha256"), f"{path}.source.context_sha256"
    )
    if end_ms < start_ms or end_ms >= epoch.cutoff_ms:
        raise ValueError(f"{path}.source: timestamps escape the knowledge epoch")
    outside_window = (
        team.window_end_ms is not None and epoch.cutoff_ms > team.window_end_ms
    )
    validation_team = (
        load_archived_team(archive, team.team_slug) if outside_window else team
    )
    current = _root_overview_input(validation_team, epoch)
    exact_match = (
        current.event_ids == event_ids
        and current.start_ms == start_ms
        and current.end_ms == end_ms
        and current.context_sha256 == expected_sha256
    )
    if not exact_match:
        reconstructed = _recorded_overview_input(
            validation_team, epoch, event_ids
        )
        recorded_evidence_unchanged = reconstructed is not None and (
            reconstructed.start_ms == start_ms
            and reconstructed.end_ms == end_ms
            and reconstructed.context_sha256 == expected_sha256
        )
        if recorded_evidence_unchanged and current.event_ids != event_ids:
            raise _ProjectOverviewSourceSetChanged(path, epoch)
        raise ValueError(
            f"{path}.source: frozen overview evidence was mutated or truncated"
        )
    return None if outside_window else current


def _glossary_evidence_from_json(
    value: JsonValue, where: str
) -> _GlossaryEvidence:
    obj = as_object(value, where)
    _require_exact_keys(
        obj, {"event_id", "thread_id", "at_ms", "kind", "context"}, where
    )
    return _GlossaryEvidence(
        event_id=as_string(obj.get("event_id"), f"{where}.event_id"),
        thread_id=as_string(obj.get("thread_id"), f"{where}.thread_id"),
        at_ms=as_int(obj.get("at_ms"), f"{where}.at_ms"),
        kind=as_string(obj.get("kind"), f"{where}.kind"),
        context=as_string(obj.get("context"), f"{where}.context"),
    )


def _frozen_term_knowledge(
    archive: Path,
    team: TeamData,
    project_epoch_id: str,
    current_cutoff_ms: int,
) -> tuple[int | None, dict[str, _TermKnowledge]]:
    """Load immutable per-term evidence from the preceding glossary generation."""

    path = _summary_root(archive, team.team_slug) / "glossary.json"
    if not path.is_file():
        return None, {}
    obj = as_object(read_json(path), str(path))
    if "schema_version" not in obj:
        return None, {}
    version = as_int(obj.get("schema_version"), f"{path}.schema_version")
    if version != _GLOSSARY_SCHEMA_VERSION:
        return None, {}
    _require_exact_keys(
        obj,
        {
            "schema_version",
            "project_overview_input_hash",
            "project_overview_epoch_id",
            "observed_through_ms",
            "terms",
        },
        str(path),
    )
    recorded_project_epoch = as_string(
        obj.get("project_overview_epoch_id"),
        f"{path}.project_overview_epoch_id",
    )
    observed_through_ms = as_int(
        obj.get("observed_through_ms"), f"{path}.observed_through_ms"
    )
    if recorded_project_epoch != project_epoch_id:
        return None, {}
    if observed_through_ms > current_cutoff_ms:
        raise ValueError(f"{path}: knowledge source frontier moved backwards")
    result: dict[str, _TermKnowledge] = {}
    for index, raw_term in enumerate(as_array(obj.get("terms"), f"{path}.terms")):
        where = f"{path}.terms[{index}]"
        term = as_object(raw_term, where)
        _require_exact_keys(
            term,
            {
                "term",
                "introduced_at_ms",
                "occurrences",
                "context",
                "week",
                "term_id",
                "available_at_ms",
                "definition",
                "definition_status",
                "definition_cutoff_ms",
                "definition_epoch_id",
                "definition_summary",
                "evidence",
            },
            where,
        )
        term_name = as_string(term.get("term"), f"{where}.term")
        term_id = as_string(term.get("term_id"), f"{where}.term_id")
        if term_id != glossary_term_id(term_name):
            raise ValueError(f"{where}.term_id: does not match the exact term")
        available_at_ms = as_int(
            term.get("available_at_ms"), f"{where}.available_at_ms"
        )
        definition_cutoff_ms = as_int(
            term.get("definition_cutoff_ms"), f"{where}.definition_cutoff_ms"
        )
        definition_epoch_id = as_string(
            term.get("definition_epoch_id"), f"{where}.definition_epoch_id"
        )
        if definition_cutoff_ms > current_cutoff_ms:
            raise ValueError(f"{where}: definition source frontier moved backwards")
        evidence = tuple(
            _glossary_evidence_from_json(item, f"{where}.evidence[{evidence_index}]")
            for evidence_index, item in enumerate(
                as_array(term.get("evidence"), f"{where}.evidence")
            )
        )
        if any(item.at_ms >= definition_cutoff_ms for item in evidence):
            raise ValueError(f"{where}.evidence: occurrence escapes definition cutoff")
        expected_epoch_id = _definition_epoch_id(
            term_id,
            project_epoch_id,
            definition_cutoff_ms,
            evidence,
        )
        if definition_epoch_id != expected_epoch_id:
            raise ValueError(f"{where}.definition_epoch_id: provenance mismatch")
        current_evidence = _definition_evidence(
            team,
            GlossaryTerm(
                term=term_name,
                introduced_at_ms=0,
                occurrences=0,
                context="",
                week="",
                term_id=term_id,
            ),
            cutoff_ms=definition_cutoff_ms,
        )
        if current_evidence != evidence:
            raise ValueError(
                f"{where}.evidence: frozen source was mutated or truncated"
            )
        if term_id in result:
            raise ValueError(f"{where}.term_id: duplicate glossary term")
        result[term_id] = _TermKnowledge(
            evidence=evidence,
            available_at_ms=available_at_ms,
            definition_cutoff_ms=definition_cutoff_ms,
            definition_epoch_id=definition_epoch_id,
        )
    return observed_through_ms, result


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


def _result_artifact_key(result: SummaryResult) -> str:
    provenance = result.artifact_provenance
    return provenance.artifact_id if provenance is not None else result.input_hash


def _summary_provenance_matches_job(
    provenance: SummaryArtifactProvenance, job: SummaryJob
) -> bool:
    if (
        provenance.logical_key != job.key
        or provenance.team_slug != job.team_slug
        or provenance.start_ms != job.start_ms
        or provenance.end_ms != job.end_ms
        or provenance.dependency_keys != job.dependency_keys
        or provenance.context_coverage != job.context_coverage
    ):
        return False
    return summary_input_hash_for_provenance(job, provenance) == provenance.input_hash


def _summary_matches_job(result: SummaryResult, job: SummaryJob) -> bool:
    """Return whether a cached result was derived from the current mechanical input."""

    provenance = result.artifact_provenance
    return (
        provenance is not None
        and provenance.input_hash == result.input_hash
        and _summary_provenance_matches_job(provenance, job)
    )


def _agent_name_provenance_matches_job(
    provenance: SummaryArtifactProvenance, job: AgentNameJob
) -> bool:
    if (
        provenance.logical_key != job.key
        or provenance.team_slug != job.team_slug
        or provenance.start_ms != job.start_ms
        or provenance.end_ms != job.end_ms
        or provenance.dependency_keys != job.dependency_keys
        or provenance.context_coverage != job.context_coverage
    ):
        return False
    return agent_name_input_hash_for_provenance(job, provenance) == provenance.input_hash


def _agent_name_matches_job(result: AgentNameResult, job: AgentNameJob) -> bool:
    """Return whether a cached lifetime summary covers the current agent input."""

    provenance = result.artifact_provenance
    return (
        provenance is not None
        and provenance.input_hash == result.input_hash
        and _agent_name_provenance_matches_job(provenance, job)
    )


def _summary_catalog_reference(
    result: SummaryResult | AgentNameResult, cache_directory: str
) -> SummaryArtifactReference:
    provenance = result.artifact_provenance
    if provenance is None:
        raise ValueError(
            f"model artifact {result.input_hash!r} lacks common provenance"
        )
    return SummaryArtifactReference(
        provenance=provenance,
        cache_path=f"{cache_directory}/{result.input_hash}.json",
    )


def _agent_name_jobs(
    team: TeamData,
    phases: Sequence[PhaseWindow],
    phase_results: Mapping[str, SummaryResult],
    context_chars: int = 16_000,
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
        if own_phases:
            job_start_ms = own_phases[0].start_ms
            job_end_ms = own_phases[-1].end_ms
        else:
            job_start_ms = max(
                agent.started_at_ms,
                team.window_start_ms or agent.started_at_ms,
            )
            natural_end_ms = agent.ended_at_ms or team.window_end_ms or job_start_ms
            job_end_ms = max(
                job_start_ms,
                min(natural_end_ms, team.window_end_ms or natural_end_ms),
            )
        jobs.append(
            AgentNameJob(
                key=f"agent-name:{agent.thread_id}",
                team_slug=team.team_slug,
                thread_id=agent.thread_id,
                start_ms=job_start_ms,
                end_ms=job_end_ms,
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
                context_coverage=ContextCoverage(
                    components=(
                        ContextComponent(
                            "ancestor_transcript",
                            context_chars,
                            min(
                                context_chars,
                                len(own_phases[0].prior_context) if own_phases else 0,
                            ),
                            "characters",
                        ),
                        ContextComponent(
                            "phase_work_summaries",
                            len(own_phases),
                            len(own_phases),
                            "summaries",
                        ),
                    )
                ),
                dependency_keys=tuple(
                    _result_artifact_key(phase_results[phase.summary_key])
                    for phase in own_phases
                ),
            )
        )
    return tuple(jobs)


def _agent_name_json(
    job: AgentNameJob, result: AgentNameResult
) -> dict[str, JsonValue]:
    name: dict[str, JsonValue] = {
        "thread_id": result.thread_id,
        "short_name": result.short_name,
        "rationale": result.rationale,
        "lifetime_summary": result.lifetime_summary,
        "model": result.model,
        "prompt_version": result.prompt_version,
        "input_hash": result.input_hash,
        "generated_at": result.generated_at,
    }
    if result.artifact_provenance is not None:
        name["artifact_provenance"] = result.artifact_provenance.to_json_obj()
    return {
        "schema_version": 3,
        "agent": {
            "thread_id": job.thread_id,
            "official_path": job.official_path,
            "coordinator_nickname": job.coordinator_nickname,
            "role": job.role,
            "depth": job.depth,
            "parent_official_path": job.parent_official_path,
        },
        "name": name,
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


def _select_catalog_agent_name_for_job(
    summary_root: Path,
    catalog: SummaryArtifactCatalog,
    agent: Agent,
    job: AgentNameJob,
) -> AgentNameResult:
    compatible = SummaryArtifactCatalog(
        team_slug=catalog.team_slug,
        records=tuple(
            reference
            for reference in catalog.records
            if reference.provenance.summarizer_id
            == AGENT_LIFETIME_SUMMARIZER.summarizer_id
            and _agent_name_provenance_matches_job(reference.provenance, job)
        ),
    )
    reference = select_summary_artifact(
        compatible,
        job.key,
        AGENT_LIFETIME_SUMMARIZER.summarizer_id,
        minimum_output_schema=2,
    )
    if reference is None:
        return _fallback_agent_name(agent)
    return _agent_name_from_catalog_reference(summary_root, reference, agent)


def _load_agent_names(
    archive: Path,
    team: TeamData,
    selected_ids: frozenset[str],
    jobs: Sequence[AgentNameJob],
    catalog: SummaryArtifactCatalog | None = None,
) -> dict[str, AgentNameResult]:
    selected_catalog = catalog or load_summary_catalog(
        _summary_root(archive, team.team_slug) / "artifacts.json",
        team.team_slug,
    )
    jobs_by_thread = {job.thread_id: job for job in jobs}
    if len(jobs_by_thread) != len(jobs):
        raise ValueError("duplicate agent-name job thread")
    results: dict[str, AgentNameResult] = {}
    for agent in team.agents:
        if agent.thread_id not in selected_ids:
            continue
        job = jobs_by_thread.get(agent.thread_id)
        if job is None:
            raise ValueError(f"missing agent-name job for {agent.thread_id!r}")
        path = (
            _summary_root(archive, team.team_slug)
            / "agents"
            / f"{agent.thread_id}.json"
        )
        if _summary_projection_missing(path):
            candidate = _select_catalog_agent_name_for_job(
                _summary_root(archive, team.team_slug),
                selected_catalog,
                agent,
                job,
            )
            results[agent.thread_id] = candidate
            continue
        root = as_object(read_json(path), str(path))
        schema_version = as_int(
            root.get("schema_version"), f"{path}.schema_version"
        )
        if schema_version not in {1, 2, 3}:
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
        prompt_version = _nonempty_string(
            raw_name.get("prompt_version"), f"{path}.name.prompt_version"
        )
        try:
            prompt_contract = summarizer_change_for_prompt(
                AGENT_LIFETIME_SUMMARIZER, prompt_version
            )
        except ValueError as error:
            raise ValueError(f"{path}: unregistered agent-lifetime version") from error
        if schema_version == 1:
            if prompt_contract.output_schema_version != 1:
                raise ValueError(
                    f"{path}: agent-name wrapper and prompt schemas disagree"
                )
            lifetime_summary: str | None = None
        else:
            if prompt_contract.output_schema_version < 2:
                raise ValueError(
                    f"{path}: agent-name wrapper and prompt schemas disagree"
                )
            lifetime_summary = _nonempty_string(
                clean_summary_prose(
                    _nonempty_string(
                        raw_name.get("lifetime_summary"),
                        f"{path}.name.lifetime_summary",
                    )
                ),
                f"{path}.name.lifetime_summary",
            )
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
            lifetime_summary=lifetime_summary,
            model=_nonempty_string(raw_name.get("model"), f"{path}.name.model"),
            prompt_version=prompt_version,
            input_hash=_nonempty_string(
                raw_name.get("input_hash"), f"{path}.name.input_hash"
            ),
            generated_at=_nonempty_string(
                raw_name.get("generated_at"), f"{path}.name.generated_at"
            ),
            artifact_provenance=(
                SummaryArtifactProvenance.from_json_obj(
                    raw_name.get("artifact_provenance"),
                    f"{path}.name.artifact_provenance",
                )
                if "artifact_provenance" in raw_name
                else None
            ),
        )
        if result.thread_id != agent.thread_id:
            raise ValueError(f"agent-name result thread mismatch at {path}")
        provenance = result.artifact_provenance
        if provenance is not None and (
            provenance.summarizer_id != AGENT_LIFETIME_SUMMARIZER.summarizer_id
            or provenance.logical_key != f"agent-name:{agent.thread_id}"
            or provenance.team_slug != team.team_slug
            or provenance.input_hash != result.input_hash
            or provenance.model != result.model
            or provenance.prompt_version != result.prompt_version
        ):
            raise ValueError(f"{path}: agent-name provenance differs from result")
        if result.lifetime_summary is None:
            results[agent.thread_id] = result
        elif not _agent_name_matches_job(result, job):
            results[agent.thread_id] = _select_catalog_agent_name_for_job(
                _summary_root(archive, team.team_slug),
                selected_catalog,
                agent,
                job,
            )
        else:
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
    summary_style: str = TECHNICAL_ROLLUP_STYLE,
    project_overview: SummaryResult | None = None,
    same_period_technical: Mapping[str, SummaryResult] | None = None,
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
        dependency_results = [lower_results[_period_key(item)] for item in lower]
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
        dependency_results.extend(
            phase_results[phase.summary_key] for phase in uncovered_phases
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
        dependency_results.extend(result for _, result in prior)
        stats = aggregate_stats(own_phases)
        plain_language = summary_style == PLAIN_LANGUAGE_ROLLUP_STYLE
        key_prefix = "rollup-plain" if plain_language else "rollup"
        audience = "Plain-language" if plain_language else "Technical"
        technical_result = (
            same_period_technical.get(_period_key(period))
            if plain_language and same_period_technical is not None
            else None
        )
        if plain_language and technical_result is None:
            raise ValueError(
                f"plain-language {period.kind} rollup {period.key!r} lacks "
                "its same-period technical summary"
            )
        glossary = glossary_prompt_text(
            [term for term in terms if term.summary_available_at_ms < period.end_ms]
        )
        if plain_language:
            glossary = plain_language_context_text(
                project_overview.paragraph if project_overview is not None else "",
                [
                    term
                    for term in terms
                    if term.summary_available_at_ms < period.end_ms
                ],
            )
            if project_overview is not None:
                dependency_results.append(project_overview)
            if technical_result is not None:
                dependency_results.append(technical_result)
        earliest_activity = min(
            (event.timestamp_ms for event in team.events),
            default=period.start_ms,
        )
        previous_period = completed_same_level[-1][0] if completed_same_level else None
        if period.start_ms <= earliest_activity < period.end_ms:
            frontier_status = "project-start"
        elif previous_period is not None and previous_period.end_ms == period.start_ms:
            frontier_status = "contiguous-extension"
        else:
            frontier_status = "isolated-backfill"
        expected_prior_summaries = 0
        if earliest_activity < period.start_ms:
            expected_prior_summaries = min(
                10,
                len(
                    periods_for_range(
                        earliest_activity,
                        period.start_ms - 1,
                        team.display_timezone,
                        team.team_slug,
                        (period.kind,),
                    )
                ),
            )
        jobs.append(
            SummaryJob(
                key=f"{key_prefix}:{period.kind}:{period.key}",
                team_slug=team.team_slug,
                agent_label=(
                    f"{audience} {period.kind} super-summary · {period.label}"
                ),
                start_ms=period.start_ms,
                end_ms=period.end_ms,
                prior_context=prior_context,
                transcript=transcript,
                glossary=glossary,
                stats=stats.to_mapping(),
                summary_style=summary_style,
                factual_context=(
                    _result_line(
                        technical_result,
                        f"Authoritative technical {period.kind} summary",
                        period.start_ms,
                    )
                    if technical_result is not None
                    else ""
                ),
                context_coverage=ContextCoverage(
                    components=(
                        ContextComponent(
                            "lower_level_summaries",
                            len(transcript_parts),
                            len(transcript_parts),
                            "summaries",
                        ),
                        ContextComponent(
                            "prior_same_level_summaries",
                            expected_prior_summaries,
                            min(expected_prior_summaries, len(prior)),
                            "summaries",
                        ),
                    )
                    + (
                        (ContextComponent("project_overview", 1, 1, "artifacts"),)
                        if plain_language and project_overview is not None
                        else ()
                    )
                    + (
                        (ContextComponent("technical_summary", 1, 1, "summaries"),)
                        if technical_result is not None
                        else ()
                    ),
                    frontier_status=frontier_status,
                    predecessor_keys=tuple(_period_key(item) for item, _ in prior),
                ),
                dependency_keys=tuple(
                    dict.fromkeys(_result_artifact_key(result) for result in dependency_results)
                ),
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
    claude_command: Sequence[str] = ("claude",),
    reasoning_effort: str | None = None,
    service_tier: str | None = None,
    summary_window: DateWindow | None = None,
    rollup_kinds: tuple[str, ...] = DEFAULT_ROLLUP_KINDS,
) -> SummarizeReport:
    """Fill only missing/changed structured summaries; never format the website."""

    service_tier = resolve_service_tier(backend, service_tier)
    team = load_archived_team(archive, team_slug)
    if summary_window is not None:
        team = apply_date_window(team, summary_window)
    summary_root = _summary_root(archive, team_slug)
    existing_summary_catalog = load_summary_catalog(
        summary_root / "artifacts.json", team_slug
    )
    changed = int(
        write_json_if_changed(
            summary_root / "summarizers.json",
            registry_json_obj(),
        )
    )
    phases = build_phases(
        team,
        phase_minutes=phase_minutes,
        context_chars=context_chars,
        transcript_chars=transcript_chars,
    )
    # Legacy schema-3 glossary records contain mechanically selected strings, not a supported
    # semantic project ontology. Until the bounded semantic discovery pipeline is complete, fail
    # closed: no candidate enters a model prompt and no glossary-specific model job can run.
    supported_terms: tuple[GlossaryTerm, ...] = ()
    cache = summary_root / "cache"
    try:
        overview_source = _frozen_project_overview_input(archive, team)
    except _ProjectOverviewSourceSetChanged as change:
        overview_source = _renewed_project_overview_input(
            team, change.previous_epoch
        )
    if overview_source is None:
        overview_source = _root_overview_input(team)
    phase_results, phase_stats = summarize_jobs(
        _phase_jobs(team, phases, supported_terms, context_chars),
        cache,
        backend,
        model,
        max_workers=max_workers,
        batch_size=batch_size,
        codex_command=codex_command,
        claude_command=claude_command,
        reasoning_effort=reasoning_effort,
        service_tier=service_tier,
    )
    changed += _write_phase_data(archive, team_slug, phases, phase_results)
    name_jobs = _agent_name_jobs(team, phases, phase_results, context_chars)
    agent_names, name_stats = name_agents(
        name_jobs,
        summary_root / "name_cache",
        backend,
        model,
        max_workers=max_workers,
        batch_size=name_batch_size,
        codex_command=codex_command,
        claude_command=claude_command,
        reasoning_effort=reasoning_effort,
        service_tier=service_tier,
    )
    changed += _write_agent_name_data(
        archive, team_slug, name_jobs, agent_names
    )
    overview_job = _project_overview_job(team, overview_source)
    overview_results, overview_stats = summarize_jobs(
        (overview_job,),
        cache,
        backend,
        model,
        max_workers=max_workers,
        batch_size=batch_size,
        codex_command=codex_command,
        claude_command=claude_command,
        reasoning_effort=reasoning_effort,
        service_tier=service_tier,
    )
    project_overview = overview_results[overview_job.key]
    _validate_project_overview(project_overview, "generated project overview")
    changed += _write_project_overview_data(
        archive, team_slug, overview_source, project_overview
    )
    definition_jobs: tuple[SummaryJob, ...] = ()
    definition_results: dict[str, SummaryResult] = {}
    enriched_terms = supported_terms

    start_ms, end_ms = _period_range(team, phases)
    periods = periods_for_range(
        start_ms,
        end_ms,
        team.display_timezone,
        team.team_slug,
        rollup_kinds,
    )
    selected_kinds = tuple(kind for kind in ROLLUP_KINDS if kind in rollup_kinds)
    by_kind = {
        kind: [period for period in periods if period.kind == kind]
        for kind in selected_kinds
    }
    all_results: dict[str, SummaryResult] = {}
    all_plain_results: dict[str, SummaryResult] = {}
    backend_stats: list[SummaryRunStats] = [
        phase_stats,
        name_stats,
        overview_stats,
    ]
    previous_periods: list[Period] = []
    previous_results: dict[str, SummaryResult] = {}
    previous_plain_results: dict[str, SummaryResult] = {}
    for kind in selected_kinds:
        current = by_kind[kind]
        first_start_ms = current[0].start_ms if current else start_ms
        completed = _prior_catalog_rollups(
            summary_root,
            existing_summary_catalog,
            kind,
            TECHNICAL_ROLLUP_STYLE,
            first_start_ms,
        )
        completed_plain = _prior_catalog_rollups(
            summary_root,
            existing_summary_catalog,
            kind,
            PLAIN_LANGUAGE_ROLLUP_STYLE,
            first_start_ms,
        )
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
                supported_terms,
                TECHNICAL_ROLLUP_STYLE,
            )
            results, stats = summarize_jobs(
                jobs,
                cache,
                backend,
                model,
                max_workers=max_workers,
                batch_size=batch_size,
                codex_command=codex_command,
                claude_command=claude_command,
                reasoning_effort=reasoning_effort,
                service_tier=service_tier,
            )
            backend_stats.append(stats)
            result = results[jobs[0].key]
            all_results[_period_key(period)] = result
            completed.append((period, result))
            plain_jobs = _rollup_jobs_for_level(
                team,
                (period,),
                phases,
                phase_results,
                previous_periods,
                previous_plain_results,
                completed_plain,
                enriched_terms,
                PLAIN_LANGUAGE_ROLLUP_STYLE,
                project_overview,
                {_period_key(period): result},
            )
            plain_results, plain_stats = summarize_jobs(
                plain_jobs,
                cache,
                backend,
                model,
                max_workers=max_workers,
                batch_size=batch_size,
                codex_command=codex_command,
                claude_command=claude_command,
                reasoning_effort=reasoning_effort,
                service_tier=service_tier,
            )
            backend_stats.append(plain_stats)
            plain_result = plain_results[plain_jobs[0].key]
            all_plain_results[_period_key(period)] = plain_result
            completed_plain.append((period, plain_result))
        previous_periods = current
        previous_results = {
            _period_key(period): all_results[_period_key(period)] for period in current
        }
        previous_plain_results = {
            _period_key(period): all_plain_results[_period_key(period)]
            for period in current
        }

    for period in periods:
        result = all_results[_period_key(period)]
        plain_result = all_plain_results[_period_key(period)]
        obj: dict[str, JsonValue] = {
            "schema_version": 2,
            "kind": period.kind,
            "key": period.key,
            "start_ms": period.start_ms,
            "end_ms": period.end_ms,
            "partial": period.partial,
            "technical_summary": _summary_json(result),
            "plain_language_summary": _summary_json(plain_result),
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
    # Preserve historical schema-3 glossary bytes for provenance. The build path ignores that
    # mechanically selected projection, and no new legacy projection is written.
    summary_results = (
        tuple(phase_results.values())
        + (project_overview,)
        + tuple(definition_results.values())
        + tuple(all_results.values())
        + tuple(all_plain_results.values())
    )
    catalog_additions = tuple(
        _summary_catalog_reference(result, "cache") for result in summary_results
    ) + tuple(
        _summary_catalog_reference(result, "name_cache")
        for result in agent_names.values()
    )
    for reference in catalog_additions:
        cache_path = summary_root / reference.cache_path
        if not cache_path.is_file():
            raise ValueError(
                f"summary artifact cache entry is missing: {cache_path}"
            )
    catalog, catalog_changed = merge_summary_catalog(
        summary_root / "artifacts.json",
        team_slug,
        catalog_additions,
    )
    changed += int(catalog_changed)
    combined = _accumulate_stats(backend_stats)
    return SummarizeReport(
        backend=backend,
        model=model,
        reasoning_effort=reasoning_effort,
        service_tier=service_tier,
        phases=len(phases),
        rollups=len(periods),
        agent_names=len(agent_names),
        glossary_terms=len(supported_terms),
        project_overviews=1,
        glossary_definitions=len(definition_jobs),
        catalog_artifacts=len(catalog.records),
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
    claude_command: Sequence[str] = ("claude",),
    reasoning_effort: str | None = None,
    service_tier: str | None = None,
    summary_window: DateWindow | None = None,
    rollup_kinds: tuple[str, ...] = DEFAULT_ROLLUP_KINDS,
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
            claude_command=claude_command,
            reasoning_effort=reasoning_effort,
            service_tier=service_tier,
            summary_window=summary_window,
            rollup_kinds=rollup_kinds,
        )


def _load_phase_summaries(
    archive: Path,
    team_slug: str,
    phases: Sequence[PhaseWindow],
    jobs: Sequence[SummaryJob],
    catalog: SummaryArtifactCatalog,
) -> dict[str, SummaryResult]:
    result: dict[str, SummaryResult] = {}
    summary_root = _summary_root(archive, team_slug)
    jobs_by_key = {job.key: job for job in jobs}
    if len(jobs_by_key) != len(jobs):
        raise ValueError("duplicate phase summary job key")
    for phase in phases:
        path = _summary_root(archive, team_slug) / "phases" / f"{phase.phase_id}.json"
        job = jobs_by_key.get(phase.summary_key)
        if job is None:
            raise ValueError(f"missing phase summary job {phase.summary_key!r}")
        fallback = _unavailable_summary(
            phase.summary_key,
            phase.end_ms,
            "work phase",
        )
        if _summary_projection_missing(path):
            candidate = _select_catalog_summary_for_job(
                summary_root,
                catalog,
                job,
                "phase-work-summary",
                fallback,
            )
        else:
            obj = as_object(read_json(path), str(path))
            _require_exact_keys(
                obj,
                {"phase_id", "agent_id", "start_ms", "end_ms", "summary"},
                str(path),
            )
            recorded_phase_id = as_string(obj.get("phase_id"), f"{path}.phase_id")
            recorded_agent_id = as_string(obj.get("agent_id"), f"{path}.agent_id")
            recorded_start_ms = as_int(obj.get("start_ms"), f"{path}.start_ms")
            recorded_end_ms = as_int(obj.get("end_ms"), f"{path}.end_ms")
            if (
                recorded_phase_id != phase.phase_id
                or recorded_agent_id != phase.agent_id
            ):
                raise ValueError(f"{path}: phase projection identity mismatch")
            projected = _summary_from_json(obj.get("summary"), str(path))
            if (
                recorded_start_ms != phase.start_ms
                or recorded_end_ms != phase.end_ms
            ):
                candidate = fallback
            else:
                candidate = projected
        if not _summary_matches_job(candidate, job):
            candidate = _select_catalog_summary_for_job(
                summary_root,
                catalog,
                job,
                "phase-work-summary",
                fallback,
            )
        result[phase.summary_key] = candidate
    return result


_PRESENTATION_FALLBACK_VERSION = "agent-team-timeline-presentation-fallback-v1"


def _fallback_generated_at(at_ms: int) -> str:
    return datetime.fromtimestamp(at_ms / 1000, tz=timezone.utc).isoformat().replace(
        "+00:00", "Z"
    )


def _unavailable_summary(key: str, at_ms: int, subject: str) -> SummaryResult:
    """Return an ephemeral build-only placeholder; never persist it as model output."""

    identity = hashlib.sha256(
        f"{_PRESENTATION_FALLBACK_VERSION}\0{key}\0{subject}".encode("utf-8")
    ).hexdigest()
    return SummaryResult(
        key=key,
        phrase="Summary unavailable",
        paragraph=(
            f"Summary unavailable: no cached model summary exists for this {subject}. "
            "The transcript and statistics shown here come directly from normalized logs."
        ),
        work_summary=(),
        model="none",
        prompt_version=_PRESENTATION_FALLBACK_VERSION,
        input_hash=identity,
        generated_at=_fallback_generated_at(at_ms),
        summary_available=False,
    )


def _fallback_agent_name(agent: Agent) -> AgentNameResult:
    leaf = agent.agent_path.rstrip("/").rsplit("/", 1)[-1]
    if agent.parent_thread_id is None:
        short_name = "Coordinator"
    else:
        readable = re.sub(r"[-_]+", " ", leaf).strip()
        short_name = (readable or "Unnamed agent")[:48].rstrip()
    identity = hashlib.sha256(
        (
            f"{_PRESENTATION_FALLBACK_VERSION}\0{agent.thread_id}\0"
            f"{agent.agent_path}"
        ).encode("utf-8")
    ).hexdigest()
    return AgentNameResult(
        thread_id=agent.thread_id,
        short_name=short_name,
        rationale=(
            "Mechanical name from the official agent path; no cached hindsight name "
            "is available."
        ),
        lifetime_summary=(
            "Summary unavailable: no cached agent-lifetime summary exists. "
            "Open a work phase to inspect normalized transcript and statistics."
        ),
        model="none",
        prompt_version=_PRESENTATION_FALLBACK_VERSION,
        input_hash=identity,
        generated_at=_fallback_generated_at(agent.ended_at_ms or agent.started_at_ms),
        summary_available=False,
    )


def _agent_name_from_catalog_reference(
    summary_root: Path,
    reference: SummaryArtifactReference,
    agent: Agent,
) -> AgentNameResult:
    """Recover a compatible paid lifetime result when its projection is absent."""

    path = summary_root / reference.cache_path
    root = as_object(read_json(path), str(path))
    provenance = reference.provenance
    if root.get("format") == ARTIFACT_ENVELOPE_FORMAT:
        _require_exact_keys(
            root,
            {"format", "schema_version", "artifact", "result"},
            str(path),
        )
        if as_int(root.get("schema_version"), f"{path}.schema_version") != (
            ARTIFACT_ENVELOPE_VERSION
        ):
            raise ValueError(f"{path}: unsupported model-artifact envelope")
        cached_provenance = SummaryArtifactProvenance.from_json_obj(
            root.get("artifact"), f"{path}.artifact"
        )
        if cached_provenance != provenance:
            raise ValueError(f"{path}: cache provenance differs from artifact catalog")
    else:
        if not provenance.legacy_storage:
            raise ValueError(f"{path}: cataloged artifact lacks its common envelope")
        _require_exact_keys(
            root,
            {"cache_version", "backend", "usage_receipt_id", "result"},
            str(path),
        )
        if as_string(root.get("backend"), f"{path}.backend") != provenance.backend:
            raise ValueError(f"{path}: legacy cache backend differs from catalog")

    where = f"{path}.result"
    raw = as_object(root.get("result"), where)
    _require_exact_keys(
        raw,
        {
            "thread_id",
            "short_name",
            "rationale",
            "lifetime_summary",
            "model",
            "prompt_version",
            "input_hash",
            "generated_at",
        },
        where,
    )
    prompt_version = _nonempty_string(
        raw.get("prompt_version"), f"{where}.prompt_version"
    )
    prompt_contract = summarizer_change_for_prompt(
        AGENT_LIFETIME_SUMMARIZER, prompt_version
    )
    if prompt_contract.output_schema_version < 2:
        raise ValueError(f"{path}: agent-lifetime artifact lacks a lifetime summary")
    result = AgentNameResult(
        thread_id=_nonempty_string(raw.get("thread_id"), f"{where}.thread_id"),
        short_name=_nonempty_string(raw.get("short_name"), f"{where}.short_name"),
        rationale=_nonempty_string(raw.get("rationale"), f"{where}.rationale"),
        lifetime_summary=_nonempty_string(
            clean_summary_prose(
                _nonempty_string(
                    raw.get("lifetime_summary"), f"{where}.lifetime_summary"
                )
            ),
            f"{where}.lifetime_summary",
        ),
        model=_nonempty_string(raw.get("model"), f"{where}.model"),
        prompt_version=prompt_version,
        input_hash=_nonempty_string(raw.get("input_hash"), f"{where}.input_hash"),
        generated_at=_nonempty_string(
            raw.get("generated_at"), f"{where}.generated_at"
        ),
        artifact_provenance=provenance,
    )
    if (
        result.thread_id != agent.thread_id
        or result.input_hash != provenance.input_hash
        or result.model != provenance.model
        or result.prompt_version != provenance.prompt_version
    ):
        raise ValueError(f"{path}: cached agent lifetime differs from provenance")
    return result


def _validate_legacy_project_overview(
    root: Mapping[str, JsonValue], path: Path, version: int
) -> None:
    """Distinguish a complete old projection from a truncated/corrupt file."""

    root_keys = {"schema_version", "style", "source", "summary"}
    if version == 2:
        root_keys.add("knowledge_epoch")
    _require_exact_keys(root, root_keys, str(path))
    if as_string(root.get("style"), f"{path}.style") != PROJECT_OVERVIEW_STYLE:
        raise ValueError(f"{path}: invalid project-overview style")
    if version == 2:
        _knowledge_epoch_from_json(
            root.get("knowledge_epoch"), f"{path}.knowledge_epoch"
        )
    source = as_object(root.get("source"), f"{path}.source")
    source_keys = {"event_ids", "start_ms", "end_ms", "context_sha256"}
    if version == 2:
        source_keys.add("transcript")
    _require_exact_keys(source, source_keys, f"{path}.source")
    for index, value in enumerate(
        as_array(source.get("event_ids"), f"{path}.source.event_ids")
    ):
        as_string(value, f"{path}.source.event_ids[{index}]")
    as_int(source.get("start_ms"), f"{path}.source.start_ms")
    as_int(source.get("end_ms"), f"{path}.source.end_ms")
    expected_sha256 = _nonempty_string(
        source.get("context_sha256"), f"{path}.source.context_sha256"
    )
    if version == 2:
        transcript = as_string(
            source.get("transcript"), f"{path}.source.transcript"
        )
        actual_sha256 = hashlib.sha256(transcript.encode("utf-8")).hexdigest()
        if actual_sha256 != expected_sha256:
            raise ValueError(f"{path}.source: transcript digest mismatch")
    summary = _summary_from_json(root.get("summary"), f"{path}.summary")
    _validate_project_overview(summary, f"{path}.summary")


def _load_project_overview(
    archive: Path,
    team: TeamData,
) -> tuple[SummaryResult, _ProjectOverviewInput | None]:
    path = _summary_root(archive, team.team_slug) / "project_overview.json"
    if _summary_projection_missing(path):
        fallback_source = _root_overview_input(team)
        return (
            _unavailable_summary(
                f"project-overview:{team.team_slug}",
                fallback_source.end_ms,
                "project overview",
            ),
            fallback_source,
        )
    root = as_object(read_json(path), str(path))
    version = as_int(root.get("schema_version"), f"{path}.schema_version")
    if version in {1, 2}:
        _validate_legacy_project_overview(root, path, version)
        fallback_source = _root_overview_input(team)
        return (
            _unavailable_summary(
                f"project-overview:{team.team_slug}",
                fallback_source.end_ms,
                "project overview stored with a legacy schema",
            ),
            fallback_source,
        )
    if version != _PROJECT_OVERVIEW_SCHEMA_VERSION:
        raise ValueError(f"{path}: unsupported project-overview schema; run summarize")
    summary = _summary_from_json(root.get("summary"), f"{path}.summary")
    _validate_project_overview(summary, f"{path}.summary")
    try:
        source = _frozen_project_overview_input(archive, team)
    except _ProjectOverviewSourceSetChanged:
        fallback_source = _root_overview_input(team)
        return (
            _unavailable_summary(
                f"project-overview:{team.team_slug}",
                fallback_source.end_ms,
                "stale project overview",
            ),
            fallback_source,
        )
    except ValueError as error:
        if "frozen overview evidence was mutated or truncated" not in str(error):
            raise
        fallback_source = _root_overview_input(team)
        return (
            _unavailable_summary(
                f"project-overview:{team.team_slug}",
                fallback_source.end_ms,
                "stale project overview",
            ),
            fallback_source,
        )
    if source is None:
        fallback_source = _root_overview_input(team)
        return (
            _unavailable_summary(
                f"project-overview:{team.team_slug}",
                fallback_source.end_ms,
                "project overview for this time slice",
            ),
            fallback_source,
        )
    return summary, source


def _validate_legacy_glossary(
    archive: Path,
    team_slug: str,
    project_overview: SummaryResult,
    project_epoch_id: str | None,
) -> tuple[GlossaryTerm, ...]:
    """Validate the retired schema-3 projection without granting publication authority."""

    suppress = project_overview.prompt_version == _PRESENTATION_FALLBACK_VERSION
    path = _summary_root(archive, team_slug) / "glossary.json"
    if _summary_projection_missing(path):
        return ()
    obj = as_object(read_json(path), str(path))
    _require_exact_keys(
        obj,
        {
            "schema_version",
            "project_overview_input_hash",
            "project_overview_epoch_id",
            "observed_through_ms",
            "terms",
        },
        str(path),
    )
    if as_int(obj.get("schema_version"), f"{path}.schema_version") != (
        _GLOSSARY_SCHEMA_VERSION
    ):
        raise ValueError(f"{path}: unsupported glossary schema; run summarize")
    recorded_overview_hash = as_string(
        obj.get("project_overview_input_hash"),
        f"{path}.project_overview_input_hash",
    )
    if not suppress and recorded_overview_hash != project_overview.input_hash:
        raise ValueError(f"{path}: glossary used a stale project overview; run summarize")
    recorded_project_epoch_id = as_string(
        obj.get("project_overview_epoch_id"),
        f"{path}.project_overview_epoch_id",
    )
    if (
        not suppress
        and project_epoch_id is not None
        and recorded_project_epoch_id != project_epoch_id
    ):
        raise ValueError(f"{path}: glossary used a different knowledge epoch")
    project_epoch_id = recorded_project_epoch_id
    as_int(obj.get("observed_through_ms"), f"{path}.observed_through_ms")
    for index, raw in enumerate(as_array(obj.get("terms"), f"{path}.terms")):
        where = f"{path}.terms[{index}]"
        item = as_object(raw, where)
        _require_exact_keys(
            item,
            {
                "term",
                "introduced_at_ms",
                "occurrences",
                "context",
                "week",
                "term_id",
                "available_at_ms",
                "definition",
                "definition_status",
                "definition_cutoff_ms",
                "definition_epoch_id",
                "definition_summary",
                "evidence",
            },
            where,
        )
        term = as_string(item.get("term"), f"{where}.term")
        expected_id = glossary_term_id(term)
        term_id = as_string(item.get("term_id"), f"{where}.term_id")
        if term_id != expected_id:
            raise ValueError(
                f"{path}: glossary ID {term_id!r} does not match term {term!r}"
            )
        definition = as_string(item.get("definition"), f"{where}.definition")
        definition_status = as_string(
            item.get("definition_status"), f"{where}.definition_status"
        )
        if definition_status not in {"supported", "insufficient-evidence"}:
            raise ValueError(f"{where}: invalid definition status")
        as_int(item.get("available_at_ms"), f"{where}.available_at_ms")
        as_int(item.get("introduced_at_ms"), f"{where}.introduced_at_ms")
        as_int(item.get("occurrences"), f"{where}.occurrences")
        as_string(item.get("context"), f"{where}.context")
        as_string(item.get("week"), f"{where}.week")
        definition_cutoff_ms = as_int(
            item.get("definition_cutoff_ms"), f"{where}.definition_cutoff_ms"
        )
        definition_epoch_id = as_string(
            item.get("definition_epoch_id"), f"{where}.definition_epoch_id"
        )
        definition_summary = _summary_from_json(
            item.get("definition_summary"), f"{where}.definition_summary"
        )
        _validate_glossary_definition(
            definition_summary, f"{where}.definition_summary"
        )
        if (
            definition_summary.key != f"glossary-definition:{term_id}"
            or definition_summary.paragraph != definition
            or _definition_status(definition_summary) != definition_status
        ):
            raise ValueError(f"{where}: definition provenance does not match entry")
        parsed_evidence: list[_GlossaryEvidence] = []
        for evidence_index, raw_evidence in enumerate(
            as_array(item.get("evidence"), f"{where}.evidence")
        ):
            evidence_where = f"{where}.evidence[{evidence_index}]"
            evidence = as_object(raw_evidence, evidence_where)
            _require_exact_keys(
                evidence,
                {"event_id", "thread_id", "at_ms", "kind", "context"},
                evidence_where,
            )
            as_string(evidence.get("event_id"), f"{evidence_where}.event_id")
            as_string(evidence.get("thread_id"), f"{evidence_where}.thread_id")
            as_int(evidence.get("at_ms"), f"{evidence_where}.at_ms")
            as_string(evidence.get("kind"), f"{evidence_where}.kind")
            as_string(evidence.get("context"), f"{evidence_where}.context")
            parsed_evidence.append(
                _glossary_evidence_from_json(raw_evidence, evidence_where)
            )
        if any(
            evidence.at_ms >= definition_cutoff_ms for evidence in parsed_evidence
        ):
            raise ValueError(f"{where}.evidence: occurrence escapes definition cutoff")
        expected_definition_epoch = _definition_epoch_id(
            term_id,
            project_epoch_id,
            definition_cutoff_ms,
            parsed_evidence,
        )
        if definition_epoch_id != expected_definition_epoch:
            raise ValueError(f"{where}: definition provenance does not match evidence")
    # Schema 3 proves only that a model could define each mechanically selected string. It does
    # not classify the string as a durable project concept, so publishing it would turn ordinary
    # prose and workflow language into site-wide links. Keep validating the immutable projection
    # above for provenance/integrity, but fail closed until a semantic discovery projection exists.
    return ()


def _load_glossary(
    archive: Path,
    team_slug: str,
    project_overview: SummaryResult,
    project_epoch_id: str | None,
) -> tuple[GlossaryTerm, ...]:
    """Return supported semantic concepts, never legacy mechanical candidates."""

    # Keep these arguments in the stable loader boundary for the eventual semantic projection,
    # whose provenance will be checked against the project overview and knowledge epoch.
    _ = project_overview, project_epoch_id
    path = _summary_root(archive, team_slug) / "semantic_glossary.json"
    if _summary_projection_missing(path):
        return ()
    raise ValueError(f"{path}: semantic glossary projection is not supported by this version")


def _load_rollup_projection(
    path: Path, period: Period
) -> tuple[SummaryResult, SummaryResult, bool]:
    """Strictly validate one presentation projection before it can be suppressed."""

    obj = as_object(read_json(path), str(path))
    _require_exact_keys(
        obj,
        {
            "schema_version",
            "kind",
            "key",
            "start_ms",
            "end_ms",
            "partial",
            "technical_summary",
            "plain_language_summary",
        },
        str(path),
    )
    if as_int(obj.get("schema_version"), f"{path}.schema_version") != 2:
        raise ValueError(
            f"{path}: rollup lacks a plain-language summary; run summarize"
        )
    if (
        as_string(obj.get("kind"), f"{path}.kind") != period.kind
        or as_string(obj.get("key"), f"{path}.key") != period.key
        or as_int(obj.get("start_ms"), f"{path}.start_ms") != period.start_ms
        or as_int(obj.get("end_ms"), f"{path}.end_ms") != period.end_ms
    ):
        raise ValueError(f"{path}: rollup projection identity mismatch")
    recorded_partial = obj.get("partial")
    if not isinstance(recorded_partial, bool):
        raise ValueError(f"{path}.partial: expected a boolean")
    return (
        _summary_from_json(
            obj.get("technical_summary"), f"{path}.technical_summary"
        ),
        _summary_from_json(
            obj.get("plain_language_summary"),
            f"{path}.plain_language_summary",
        ),
        recorded_partial,
    )


def _validate_rollup_inputs(
    team: TeamData,
    periods: Sequence[Period],
    phases: Sequence[PhaseWindow],
    phase_results: Mapping[str, SummaryResult],
    project_overview: SummaryResult,
    terms: Sequence[GlossaryTerm],
    technical_candidates: Mapping[str, SummaryResult],
    plain_candidates: Mapping[str, SummaryResult],
    suppressed_keys: frozenset[str],
    summary_root: Path,
    catalog: SummaryArtifactCatalog,
) -> tuple[dict[str, SummaryResult], dict[str, SummaryResult]]:
    """Fail closed when current rollup inputs differ from cached model inputs."""

    selected_kinds = tuple(
        kind for kind in ROLLUP_KINDS if any(period.kind == kind for period in periods)
    )
    by_kind = {
        kind: [period for period in periods if period.kind == kind]
        for kind in selected_kinds
    }
    validated: dict[str, SummaryResult] = {}
    validated_plain: dict[str, SummaryResult] = {}
    previous_periods: list[Period] = []
    previous_results: dict[str, SummaryResult] = {}
    previous_plain_results: dict[str, SummaryResult] = {}
    for kind in selected_kinds:
        current = by_kind[kind]
        first_start_ms = current[0].start_ms
        completed = _prior_catalog_rollups(
            summary_root,
            catalog,
            kind,
            TECHNICAL_ROLLUP_STYLE,
            first_start_ms,
        )
        completed_plain = _prior_catalog_rollups(
            summary_root,
            catalog,
            kind,
            PLAIN_LANGUAGE_ROLLUP_STYLE,
            first_start_ms,
        )
        for period in current:
            key = _period_key(period)
            technical_job = _rollup_jobs_for_level(
                team,
                (period,),
                phases,
                phase_results,
                previous_periods,
                previous_results,
                completed,
                terms,
                TECHNICAL_ROLLUP_STYLE,
            )[0]
            technical = technical_candidates[key]
            if not _summary_matches_job(technical, technical_job):
                technical_fallback = _unavailable_summary(
                    technical_job.key,
                    period.end_ms,
                    f"stale {period.kind} rollup",
                )
                technical = (
                    technical_fallback
                    if key in suppressed_keys
                    else _select_catalog_summary_for_job(
                        summary_root,
                        catalog,
                        technical_job,
                        "technical-rollup",
                        technical_fallback,
                    )
                )
            validated[key] = technical
            completed.append((period, technical))

            plain_job = _rollup_jobs_for_level(
                team,
                (period,),
                phases,
                phase_results,
                previous_periods,
                previous_plain_results,
                completed_plain,
                terms,
                PLAIN_LANGUAGE_ROLLUP_STYLE,
                project_overview,
                {key: technical},
            )[0]
            plain = plain_candidates[key]
            if not _summary_matches_job(plain, plain_job):
                plain_fallback = _unavailable_summary(
                    plain_job.key,
                    period.end_ms,
                    f"stale plain-language {period.kind} rollup",
                )
                plain = (
                    plain_fallback
                    if key in suppressed_keys
                    else _select_catalog_summary_for_job(
                        summary_root,
                        catalog,
                        plain_job,
                        "plain-language-rollup",
                        plain_fallback,
                    )
                )
            validated_plain[key] = plain
            completed_plain.append((period, plain))
        previous_periods = current
        previous_results = {
            _period_key(period): validated[_period_key(period)] for period in current
        }
        previous_plain_results = {
            _period_key(period): validated_plain[_period_key(period)]
            for period in current
        }
    return validated, validated_plain


def _build_archive_locked(
    archive: Path,
    team_slug: str,
    *,
    phase_minutes: int = 30,
    display_window: DateWindow | None = None,
    rollup_kinds: tuple[str, ...] = DEFAULT_ROLLUP_KINDS,
    output: Path | None = None,
    _precompress: bool = True,
    _write_shards: bool = True,
) -> dict[str, int]:
    """Regenerate Markdown/HTML/JSON exclusively from cached structured data."""

    target = output or archive
    _ensure_archive(target, team_slug, create=output is not None)
    team = load_archived_team(archive, team_slug)
    if display_window is not None:
        team = apply_date_window(team, display_window)
    summary_root = _summary_root(archive, team_slug)
    summary_catalog = load_summary_catalog(
        summary_root / "artifacts.json", team_slug
    )
    phases = build_phases(team, phase_minutes=phase_minutes)
    phase_jobs = _phase_jobs(team, phases, (), 16_000)
    phase_results = _load_phase_summaries(
        archive, team_slug, phases, phase_jobs, summary_catalog
    )
    agent_name_jobs = _agent_name_jobs(team, phases, phase_results)
    agent_names = _load_agent_names(
        archive,
        team,
        phase_agent_ids(team, phases),
        agent_name_jobs,
        summary_catalog,
    )
    project_overview, project_overview_source = _load_project_overview(archive, team)
    start_ms, end_ms = _period_range(team, phases)
    periods = periods_for_range(
        start_ms,
        end_ms,
        team.display_timezone,
        team.team_slug,
        rollup_kinds,
    )
    rollup_results: dict[str, SummaryResult] = {}
    plain_rollup_results: dict[str, SummaryResult] = {}
    rollup_stats: dict[str, PhaseStats] = {}
    suppressed_rollup_keys: set[str] = set()
    for period in periods:
        path = (
            _summary_root(archive, team_slug)
            / "rollups"
            / period.kind
            / f"{period.key}.json"
        )
        key = _period_key(period)
        rollup_stats[key] = aggregate_stats(_phases_in(period, phases))
        projection_missing = _summary_projection_missing(path)
        if display_window is not None and period.partial:
            suppressed_rollup_keys.add(key)
            if not projection_missing:
                _load_rollup_projection(path, period)
            rollup_results[key] = _unavailable_summary(
                f"rollup:{period.kind}:{period.key}",
                period.end_ms,
                f"partial {period.kind} rollup",
            )
            plain_rollup_results[key] = _unavailable_summary(
                f"rollup-plain:{period.kind}:{period.key}",
                period.end_ms,
                f"partial plain-language {period.kind} rollup",
            )
            continue
        if projection_missing:
            rollup_results[key] = _select_catalog_summary(
                summary_root,
                summary_catalog,
                f"rollup:{period.kind}:{period.key}",
                "technical-rollup",
                _unavailable_summary(
                    f"rollup:{period.kind}:{period.key}",
                    period.end_ms,
                    f"{period.kind} rollup",
                ),
            )
            plain_rollup_results[key] = _select_catalog_summary(
                summary_root,
                summary_catalog,
                f"rollup-plain:{period.kind}:{period.key}",
                "plain-language-rollup",
                _unavailable_summary(
                    f"rollup-plain:{period.kind}:{period.key}",
                    period.end_ms,
                    f"plain-language {period.kind} rollup",
                ),
            )
            continue
        projected_technical, projected_plain, recorded_partial = (
            _load_rollup_projection(path, period)
        )
        if recorded_partial != period.partial:
            suppressed_rollup_keys.add(key)
            rollup_results[key] = _unavailable_summary(
                f"rollup:{period.kind}:{period.key}",
                period.end_ms,
                f"stale {period.kind} rollup",
            )
            plain_rollup_results[key] = _unavailable_summary(
                f"rollup-plain:{period.kind}:{period.key}",
                period.end_ms,
                f"stale plain-language {period.kind} rollup",
            )
            continue
        rollup_results[key] = _select_catalog_summary(
            summary_root,
            summary_catalog,
            f"rollup:{period.kind}:{period.key}",
            "technical-rollup",
            projected_technical,
        )
        plain_rollup_results[key] = _select_catalog_summary(
            summary_root,
            summary_catalog,
            f"rollup-plain:{period.kind}:{period.key}",
            "plain-language-rollup",
            projected_plain,
        )
    terms = _load_glossary(
        archive,
        team_slug,
        project_overview,
        (
            project_overview_source.epoch.epoch_id
            if project_overview_source is not None
            else None
        ),
    )
    rollup_results, plain_rollup_results = _validate_rollup_inputs(
        team,
        periods,
        phases,
        phase_results,
        project_overview,
        terms,
        rollup_results,
        plain_rollup_results,
        frozenset(suppressed_rollup_keys),
        summary_root,
        summary_catalog,
    )
    artifact_catalog = load_artifact_catalog(archive, team_slug, team)
    pull_cache = load_pull_request_metadata_cache(
        pull_metadata_path(archive, team_slug)
    )
    pull_metadata = {record.key: record for record in pull_cache.records}
    site_identity = load_site_identity(archive, team)
    return render_archive(
        target,
        team,
        phases,
        phase_results,
        periods,
        rollup_results,
        plain_rollup_results,
        rollup_stats,
        terms,
        project_overview,
        agent_names,
        pull_metadata,
        artifact_catalog,
        site_identity,
        _precompress=_precompress,
        _write_shards=_write_shards,
        _export_window=display_window,
    )


def build_archive(
    archive: Path,
    team_slug: str,
    *,
    phase_minutes: int = 30,
    display_window: DateWindow | None = None,
    rollup_kinds: tuple[str, ...] = DEFAULT_ROLLUP_KINDS,
    output: Path | None = None,
    _precompress: bool = True,
    _write_shards: bool = True,
) -> dict[str, int]:
    """Regenerate one site while serializing its source snapshot and output files."""

    target = output or archive
    with _archive_writer_locks(archive, target):
        return _build_archive_locked(
            archive,
            team_slug,
            phase_minutes=phase_minutes,
            display_window=display_window,
            rollup_kinds=rollup_kinds,
            output=output,
            _precompress=_precompress,
            _write_shards=_write_shards,
        )


def _record_run_locked(
    archive: Path,
    command: Sequence[str],
    started_at: str,
    status: str,
    team_slug: str,
    ingest: IngestReport | None,
    summaries: SummarizeReport | None,
    build: Mapping[str, int] | None,
    error: str | None = None,
    *,
    team_slugs: Sequence[str] = (),
    mechanical: Mapping[str, JsonValue] | None = None,
) -> Path:
    """Append immutable run provenance and update the small archive manifest."""

    recorded_teams = tuple(sorted(set(team_slugs or (team_slug,))))
    if team_slug not in recorded_teams:
        raise ValueError("primary run team must be present in team_slugs")
    for recorded_team in recorded_teams:
        _validate_team_slug(recorded_team)
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
    if mechanical is not None:
        run_obj["mechanical"] = {
            key: value for key, value in sorted(mechanical.items())
        }
    if len(recorded_teams) > 1:
        run_obj["team_slugs"] = list(recorded_teams)
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
    teams.update(recorded_teams)
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
    *,
    team_slugs: Sequence[str] = (),
    mechanical: Mapping[str, JsonValue] | None = None,
) -> Path:
    """Append run provenance as one serialized manifest transaction."""

    with _archive_writer_locks(archive):
        return _record_run_locked(
            archive,
            command,
            started_at,
            status,
            team_slug,
            ingest,
            summaries,
            build,
            error,
            team_slugs=team_slugs,
            mechanical=mechanical,
        )


__all__ = [
    "IngestReport",
    "SummarizeReport",
    "build_archive",
    "extract_transcripts_archive",
    "ingest_claude",
    "ingest_codex",
    "ingest_orc",
    "load_archived_team",
    "record_run",
    "source_digest",
    "summarize_archive",
    "utc_now",
]
