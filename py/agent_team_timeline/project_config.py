"""Strict, versioned configuration for zero-model multi-team ingestion."""

from __future__ import annotations

from collections.abc import Sequence
from dataclasses import dataclass
import hashlib
import json
from pathlib import Path
import re
from typing import Literal, cast

from agent_team_timeline.archive import (
    JsonValue,
    as_array,
    as_int,
    as_object,
    as_string,
    narrow_json,
)
from agent_team_timeline.identity import IdentityOverrides, parse_identity_overrides
from agent_team_timeline.pipeline import (
    IngestReport,
    extract_transcripts_archive,
    ingest_claude,
    ingest_codex,
    ingest_orc,
)
from agent_team_timeline.transcript_export import TranscriptExportReport
from agent_team_timeline.window import DateWindow, parse_date_window


PROJECT_INGEST_SCHEMA_VERSION = 1
_TEAM_SLUG = re.compile(r"[a-z0-9]+(?:-[a-z0-9]+)*")
Provider = Literal["codex", "claude", "orc"]


@dataclass(frozen=True)
class CodexProjectSource:
    """One Codex coordinator root and its explicitly ordered continuations."""

    sessions_root: Path
    root_session: str
    continuation_sessions: tuple[str, ...]


@dataclass(frozen=True)
class ClaudeProjectSource:
    """One canonical Claude coordinator JSONL file."""

    session_file: Path


@dataclass(frozen=True)
class OrcProjectSource:
    """One Orc state root and coordinator session identifier."""

    source_root: Path
    root_session: str


ProjectSource = CodexProjectSource | ClaudeProjectSource | OrcProjectSource


@dataclass(frozen=True)
class ProjectTeamConfig:
    """Validated provider inputs and archive identity for one registered team."""

    slug: str
    provider: Provider
    source: ProjectSource
    timezone: str
    date_window: DateWindow | None
    identity_overrides: IdentityOverrides


@dataclass(frozen=True)
class ProjectIngestConfig:
    """A validated schema-v1 project manifest resolved relative to its file."""

    config_path: Path
    config_sha256: str
    output: Path
    teams: tuple[ProjectTeamConfig, ...]

    def select_teams(self, requested: Sequence[str] = ()) -> tuple[ProjectTeamConfig, ...]:
        """Return configured teams in manifest order, validating an optional filter."""

        if len(set(requested)) != len(requested):
            raise ValueError("--team selection contains duplicates")
        by_slug = {team.slug: team for team in self.teams}
        unknown = sorted(set(requested) - set(by_slug))
        if unknown:
            raise ValueError(
                "team selection is not registered in project config: "
                + ", ".join(unknown)
            )
        if not requested:
            return self.teams
        selected = set(requested)
        return tuple(team for team in self.teams if team.slug in selected)


@dataclass(frozen=True)
class ProjectTeamIngestResult:
    """Mechanical ingest result for one configured team."""

    team_slug: str
    provider: Provider
    ingest: IngestReport

    def to_json_obj(self) -> dict[str, JsonValue]:
        """Return this result as run-receipt metadata."""

        return {
            "team_slug": self.team_slug,
            "provider": self.provider,
            "ingest": self.ingest.to_json_obj(),
        }


@dataclass(frozen=True)
class ProjectIngestReport:
    """Results of configured team ingests and the global transcript projection."""

    config_sha256: str
    teams: tuple[ProjectTeamIngestResult, ...]
    transcripts: TranscriptExportReport

    def to_json_obj(self) -> dict[str, JsonValue]:
        """Return an explicit zero-model run receipt payload."""

        team_values: list[JsonValue] = [team.to_json_obj() for team in self.teams]
        return {
            "schema_version": PROJECT_INGEST_SCHEMA_VERSION,
            "config_sha256": self.config_sha256,
            "teams": team_values,
            "transcript_extraction": self.transcripts.to_json_obj(),
            "model_calls": 0,
            "model_tokens": 0,
            "website_build_performed": False,
        }


@dataclass(frozen=True)
class _WindowSpec:
    start_date: str | None = None
    start_time: str | None = None
    end_date: str | None = None
    end_time: str | None = None

    def resolve(self, timezone: str) -> DateWindow | None:
        return parse_date_window(
            self.start_date,
            self.end_date,
            timezone,
            start_time=self.start_time,
            end_time=self.end_time,
        )


def _check_fields(
    value: dict[str, JsonValue],
    where: str,
    required: frozenset[str],
    optional: frozenset[str] = frozenset(),
) -> None:
    missing = sorted(required - set(value))
    unknown = sorted(set(value) - required - optional)
    if missing or unknown:
        raise ValueError(
            f"{where}: invalid fields; missing={missing!r}, unknown={unknown!r}"
        )


def _required_string(value: JsonValue, where: str) -> str:
    result = as_string(value, where).strip()
    if not result or "\0" in result:
        raise ValueError(f"{where}: expected a non-empty string")
    return result


def _optional_string(
    value: dict[str, JsonValue], key: str, where: str
) -> str | None:
    if key not in value:
        return None
    return _required_string(value[key], f"{where}.{key}")


def _string_array(value: JsonValue, where: str) -> tuple[str, ...]:
    result = tuple(
        _required_string(item, f"{where}[{index}]")
        for index, item in enumerate(as_array(value, where))
    )
    if len(set(result)) != len(result):
        raise ValueError(f"{where}: duplicate values are not allowed")
    return result


def _relative_output(config_path: Path, raw: str) -> Path:
    candidate = Path(raw).expanduser()
    if candidate.is_absolute():
        raise ValueError("project config output must be relative to the config file")
    return (config_path.parent / candidate).resolve()


def _source_path(config_path: Path, raw: str) -> Path:
    candidate = Path(raw).expanduser()
    if not candidate.is_absolute():
        candidate = config_path.parent / candidate
    return candidate.resolve()


def _project_values(value: JsonValue, where: str) -> tuple[str, ...]:
    result: list[str] = []
    for index, raw in enumerate(as_array(value, where)):
        item_where = f"{where}[{index}]"
        item = as_object(raw, item_where)
        _check_fields(
            item,
            item_where,
            frozenset({"label", "repository_url"}),
        )
        label = _required_string(item.get("label"), item_where + ".label")
        repository_url = _required_string(
            item.get("repository_url"), item_where + ".repository_url"
        )
        result.append(f"{label}={repository_url}")
    return tuple(result)


def _window_spec(value: JsonValue, where: str) -> _WindowSpec:
    obj = as_object(value, where)
    _check_fields(
        obj,
        where,
        frozenset(),
        frozenset({"start_date", "start_time", "end_date", "end_time"}),
    )
    start_date = _optional_string(obj, "start_date", where)
    start_time = _optional_string(obj, "start_time", where)
    end_date = _optional_string(obj, "end_date", where)
    end_time = _optional_string(obj, "end_time", where)
    if start_date is not None and start_time is not None:
        raise ValueError(f"{where}: choose start_date or start_time, not both")
    if end_date is not None and end_time is not None:
        raise ValueError(f"{where}: choose end_date or end_time, not both")
    return _WindowSpec(start_date, start_time, end_date, end_time)


def _codex_source(
    value: dict[str, JsonValue], where: str, config_path: Path
) -> CodexProjectSource:
    _check_fields(
        value,
        where,
        frozenset({"sessions_root", "root_session"}),
        frozenset({"continuation_sessions"}),
    )
    root_session = _required_string(value.get("root_session"), where + ".root_session")
    continuations = (
        _string_array(
            value["continuation_sessions"], where + ".continuation_sessions"
        )
        if "continuation_sessions" in value
        else ()
    )
    if root_session in continuations:
        raise ValueError(f"{where}: root_session cannot also be a continuation")
    return CodexProjectSource(
        _source_path(
            config_path,
            _required_string(value.get("sessions_root"), where + ".sessions_root"),
        ),
        root_session,
        continuations,
    )


def _claude_source(
    value: dict[str, JsonValue], where: str, config_path: Path
) -> ClaudeProjectSource:
    _check_fields(value, where, frozenset({"session_file"}))
    return ClaudeProjectSource(
        _source_path(
            config_path,
            _required_string(value.get("session_file"), where + ".session_file"),
        )
    )


def _orc_source(
    value: dict[str, JsonValue], where: str, config_path: Path
) -> OrcProjectSource:
    _check_fields(value, where, frozenset({"source_root", "root_session"}))
    return OrcProjectSource(
        _source_path(
            config_path,
            _required_string(value.get("source_root"), where + ".source_root"),
        ),
        _required_string(value.get("root_session"), where + ".root_session"),
    )


def _team_config(
    value: JsonValue,
    where: str,
    config_path: Path,
    default_timezone: str,
    default_projects: tuple[str, ...],
    default_hosts: tuple[str, ...],
    default_window: _WindowSpec,
) -> ProjectTeamConfig:
    obj = as_object(value, where)
    _check_fields(
        obj,
        where,
        frozenset({"slug", "provider", "source"}),
        frozenset({"timezone", "projects", "source_hosts", "window"}),
    )
    slug = _required_string(obj.get("slug"), where + ".slug")
    if len(slug) > 64 or _TEAM_SLUG.fullmatch(slug) is None:
        raise ValueError(
            f"{where}.slug: expected 1-64 lowercase letters/digits separated by hyphens"
        )
    raw_provider = _required_string(obj.get("provider"), where + ".provider")
    if raw_provider not in ("codex", "claude", "orc"):
        raise ValueError(f"{where}.provider: expected codex, claude, or orc")
    provider = cast(Provider, raw_provider)
    source_value = as_object(obj.get("source"), where + ".source")
    source: ProjectSource
    if provider == "codex":
        source = _codex_source(source_value, where + ".source", config_path)
    elif provider == "claude":
        source = _claude_source(source_value, where + ".source", config_path)
    else:
        source = _orc_source(source_value, where + ".source", config_path)
    timezone = (
        _required_string(obj["timezone"], where + ".timezone")
        if "timezone" in obj
        else default_timezone
    )
    project_values = (
        _project_values(obj["projects"], where + ".projects")
        if "projects" in obj
        else default_projects
    )
    host_values = (
        _string_array(obj["source_hosts"], where + ".source_hosts")
        if "source_hosts" in obj
        else default_hosts
    )
    projects, hosts = parse_identity_overrides(project_values, host_values)
    window_spec = (
        _window_spec(obj["window"], where + ".window")
        if "window" in obj
        else default_window
    )
    return ProjectTeamConfig(
        slug,
        provider,
        source,
        timezone,
        window_spec.resolve(timezone),
        IdentityOverrides(projects, hosts, "config"),
    )


def load_project_ingest_config(path: Path) -> ProjectIngestConfig:
    """Load and strictly validate one project-ingest schema-v1 JSON file."""

    config_path = path.expanduser().resolve()
    config_bytes = config_path.read_bytes()
    raw: object = json.loads(config_bytes.decode("utf-8"))
    root = as_object(narrow_json(raw, str(config_path)), str(config_path))
    _check_fields(
        root,
        str(config_path),
        frozenset({"schema_version", "output", "teams"}),
        frozenset({"timezone", "projects", "source_hosts", "window"}),
    )
    schema_version = as_int(root.get("schema_version"), str(config_path) + ".schema_version")
    if schema_version != PROJECT_INGEST_SCHEMA_VERSION:
        raise ValueError(
            f"{config_path}: unsupported schema_version {schema_version}; "
            f"expected {PROJECT_INGEST_SCHEMA_VERSION}"
        )
    output = _relative_output(
        config_path,
        _required_string(root.get("output"), str(config_path) + ".output"),
    )
    timezone = (
        _required_string(root["timezone"], str(config_path) + ".timezone")
        if "timezone" in root
        else "America/New_York"
    )
    projects = (
        _project_values(root["projects"], str(config_path) + ".projects")
        if "projects" in root
        else ()
    )
    source_hosts = (
        _string_array(root["source_hosts"], str(config_path) + ".source_hosts")
        if "source_hosts" in root
        else ()
    )
    window = (
        _window_spec(root["window"], str(config_path) + ".window")
        if "window" in root
        else _WindowSpec()
    )
    raw_teams = as_array(root.get("teams"), str(config_path) + ".teams")
    if not raw_teams:
        raise ValueError(f"{config_path}.teams: expected at least one team")
    teams = tuple(
        _team_config(
            raw,
            f"{config_path}.teams[{index}]",
            config_path,
            timezone,
            projects,
            source_hosts,
            window,
        )
        for index, raw in enumerate(raw_teams)
    )
    slugs = tuple(team.slug for team in teams)
    if len(set(slugs)) != len(slugs):
        raise ValueError(f"{config_path}.teams: duplicate team slugs are not allowed")
    config_sha256 = hashlib.sha256(config_bytes).hexdigest()
    return ProjectIngestConfig(config_path, config_sha256, output, teams)


def ingest_project(
    config: ProjectIngestConfig, requested_teams: Sequence[str] = ()
) -> ProjectIngestReport:
    """Ingest selected registered teams, then refresh the global transcript JSONL.

    This operation invokes provider importers and transcript extraction only. It cannot invoke a
    summarizer and deliberately does not build or rewrite the website presentation.
    """

    selected = config.select_teams(requested_teams)
    results: list[ProjectTeamIngestResult] = []
    for team in selected:
        source = team.source
        if isinstance(source, CodexProjectSource):
            _, report = ingest_codex(
                config.output,
                source.sessions_root,
                source.root_session,
                team.slug,
                team.timezone,
                team.date_window,
                team.identity_overrides,
                source.continuation_sessions,
            )
        elif isinstance(source, ClaudeProjectSource):
            _, report = ingest_claude(
                config.output,
                source.session_file,
                team.slug,
                team.timezone,
                team.date_window,
                team.identity_overrides,
            )
        else:
            _, report = ingest_orc(
                config.output,
                source.source_root,
                source.root_session,
                team.slug,
                team.timezone,
                team.date_window,
                team.identity_overrides,
            )
        results.append(ProjectTeamIngestResult(team.slug, team.provider, report))
    # Always project every normalized team already in the durable archive. The projection is a
    # monotonic global database and therefore cannot safely omit teams merely because this run's
    # ingest filter selected a subset.
    transcripts = extract_transcripts_archive(config.output)
    return ProjectIngestReport(config.config_sha256, tuple(results), transcripts)


__all__ = [
    "ClaudeProjectSource",
    "CodexProjectSource",
    "ingest_project",
    "load_project_ingest_config",
    "OrcProjectSource",
    "ProjectIngestConfig",
    "ProjectIngestReport",
    "ProjectTeamConfig",
    "ProjectTeamIngestResult",
]
