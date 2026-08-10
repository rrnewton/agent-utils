"""Read-only, machine-friendly navigation over a built timeline archive."""

from __future__ import annotations

import argparse
import json
import re
import sys
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath
from typing import Protocol

QUERY_SCHEMA_VERSION = 1
TIMELINE_SCHEMA_VERSION = 1
_WHITESPACE = re.compile(r"\s+")
_RFC3339_INSTANT = re.compile(
    r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}"
    r"(?:\.[0-9]{1,6})?(?:Z|[+-][0-9]{2}:[0-9]{2})$"
)

JsonScalar = str | int | float | bool | None
JsonValue = JsonScalar | list["JsonValue"] | dict[str, "JsonValue"]


class TimeWindow(Protocol):
    """The two interval operations required by query filtering."""

    def contains(self, timestamp_ms: int) -> bool:
        """Return whether a point falls inside this half-open window."""

    def overlaps(self, start_ms: int, end_ms: int | None) -> bool:
        """Return whether an interval overlaps this half-open window."""


def _narrow_json(raw: object, where: str) -> JsonValue:
    if raw is None or isinstance(raw, (str, bool, int, float)):
        return raw
    if isinstance(raw, list):
        return [_narrow_json(item, where) for item in raw]
    if isinstance(raw, dict):
        result: dict[str, JsonValue] = {}
        for key, value in raw.items():
            if not isinstance(key, str):
                raise ValueError(f"{where}: object key is not a string")
            result[key] = _narrow_json(value, where)
        return result
    raise ValueError(f"{where}: unsupported JSON value {type(raw).__name__}")


def read_json(path: Path) -> JsonValue:
    """Read and recursively narrow a JSON document."""

    raw: object = json.loads(path.read_text(encoding="utf-8"))
    return _narrow_json(raw, str(path))


def as_object(value: JsonValue, where: str) -> dict[str, JsonValue]:
    if not isinstance(value, dict):
        raise ValueError(f"{where}: expected an object")
    return value


def as_array(value: JsonValue, where: str) -> list[JsonValue]:
    if not isinstance(value, list):
        raise ValueError(f"{where}: expected an array")
    return value


def as_string(value: JsonValue, where: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{where}: expected a string")
    return value


def as_int(value: JsonValue, where: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise ValueError(f"{where}: expected an integer")
    return value


def canonical_json(value: JsonValue) -> str:
    return json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"


@dataclass(frozen=True)
class QueryFilters:
    """Provider-neutral filters shared by list and search operations."""

    teams: tuple[str, ...] = ()
    window: TimeWindow | None = None
    rollup_kinds: tuple[str, ...] = ()
    agent_ref: str | None = None


@dataclass(frozen=True)
class _IndexEntry:
    kind: str
    record: dict[str, JsonValue]


def _record_array(
    timeline: dict[str, JsonValue], key: str
) -> tuple[dict[str, JsonValue], ...]:
    return tuple(
        as_object(value, f"timeline.{key}[{index}]")
        for index, value in enumerate(
            as_array(timeline.get(key), f"timeline.{key}")
        )
    )


def _team(record: dict[str, JsonValue], where: str) -> str:
    return as_string(record.get("team"), f"{where}.team")


def _local_identifier(team: str, identifier: str) -> str:
    prefix = f"{team}::"
    return identifier[len(prefix) :] if identifier.startswith(prefix) else identifier


def team_ref(team: str) -> str:
    """Return the canonical reference for one team."""

    return f"team:{team}"


def agent_ref(record: dict[str, JsonValue], where: str = "agent") -> str:
    """Return a canonical reference independent of single- versus multi-team export IDs."""

    team = _team(record, where)
    identifier = as_string(record.get("id"), f"{where}.id")
    return f"agent:{team}::{_local_identifier(team, identifier)}"


def phase_ref(record: dict[str, JsonValue], where: str = "phase") -> str:
    """Return the canonical reference for one work phase."""

    team = _team(record, where)
    identifier = as_string(record.get("id"), f"{where}.id")
    return f"phase:{team}::{_local_identifier(team, identifier)}"


def rollup_ref(record: dict[str, JsonValue], where: str = "rollup") -> str:
    """Return the canonical reference for one calendar rollup."""

    team = _team(record, where)
    kind = as_string(record.get("kind"), f"{where}.kind")
    start_ms = as_int(record.get("start_ms"), f"{where}.start_ms")
    return f"rollup:{team}::{kind}::{start_ms}"


def _timestamp(timestamp_ms: int) -> str:
    return (
        datetime.fromtimestamp(timestamp_ms / 1000, tz=timezone.utc)
        .isoformat(timespec="milliseconds")
        .replace("+00:00", "Z")
    )


def _interval(record: dict[str, JsonValue], where: str) -> tuple[int, int]:
    start_ms = as_int(record.get("start_ms"), f"{where}.start_ms")
    end_ms = as_int(record.get("end_ms"), f"{where}.end_ms")
    return start_ms, end_ms


def _overlaps(record: dict[str, JsonValue], where: str, window: TimeWindow | None) -> bool:
    if window is None:
        return True
    start_ms, end_ms = _interval(record, where)
    return window.overlaps(start_ms, end_ms)


def _with_times(
    result: dict[str, JsonValue], record: dict[str, JsonValue], where: str
) -> None:
    start_ms, end_ms = _interval(record, where)
    result["start_time"] = _timestamp(start_ms)
    result["end_time"] = _timestamp(end_ms)


def _optional_string(record: dict[str, JsonValue], key: str) -> str | None:
    value = record.get(key)
    return value if isinstance(value, str) else None


def _optional_int(record: dict[str, JsonValue], key: str) -> int | None:
    value = record.get(key)
    return value if isinstance(value, int) and not isinstance(value, bool) else None


def _copy_fields(
    record: dict[str, JsonValue], keys: tuple[str, ...]
) -> dict[str, JsonValue]:
    return {key: record[key] for key in keys if key in record}


class TimelineQuery:
    """Validated, read-only view of a built single- or multi-team timeline."""

    def __init__(self, root: Path) -> None:
        self.root = root.resolve()
        timeline_path = self.root / "data" / "timeline.json"
        if not timeline_path.is_file():
            raise ValueError(f"no built timeline at {timeline_path}")
        self.timeline = as_object(read_json(timeline_path), str(timeline_path))
        schema_version = as_int(
            self.timeline.get("schema_version"), "timeline.schema_version"
        )
        if schema_version != TIMELINE_SCHEMA_VERSION:
            raise ValueError(
                f"unsupported timeline schema version {schema_version}; "
                f"expected {TIMELINE_SCHEMA_VERSION}"
            )
        self.teams = _record_array(self.timeline, "teams")
        self.agents = _record_array(self.timeline, "agents")
        self.phases = _record_array(self.timeline, "phases")
        self.rollups = _record_array(self.timeline, "rollups")
        self._entries = self._build_index()

    def _build_index(self) -> dict[str, _IndexEntry]:
        entries: dict[str, _IndexEntry] = {}
        for index, record in enumerate(self.teams):
            slug = as_string(record.get("slug"), f"timeline.teams[{index}].slug")
            self._add_entry(entries, team_ref(slug), "team", record)
        for index, record in enumerate(self.agents):
            self._add_entry(
                entries,
                agent_ref(record, f"timeline.agents[{index}]"),
                "agent",
                record,
            )
        for index, record in enumerate(self.phases):
            self._add_entry(
                entries,
                phase_ref(record, f"timeline.phases[{index}]"),
                "phase",
                record,
            )
        for index, record in enumerate(self.rollups):
            self._add_entry(
                entries,
                rollup_ref(record, f"timeline.rollups[{index}]"),
                "rollup",
                record,
            )
        return entries

    @staticmethod
    def _add_entry(
        entries: dict[str, _IndexEntry],
        reference: str,
        kind: str,
        record: dict[str, JsonValue],
    ) -> None:
        if reference in entries:
            raise ValueError(f"duplicate stable reference {reference!r}")
        entries[reference] = _IndexEntry(kind, record)

    def _selected_team(self, team: str, filters: QueryFilters) -> bool:
        return not filters.teams or team in filters.teams

    def _validated_agent_filter(self, filters: QueryFilters) -> str | None:
        if filters.agent_ref is None:
            return None
        entry = self._entries.get(filters.agent_ref)
        if entry is None:
            raise ValueError(f"unknown stable reference {filters.agent_ref!r}")
        if entry.kind != "agent":
            raise ValueError("--agent must be an agent:... reference")
        return filters.agent_ref

    def list_records(
        self, resource: str, filters: QueryFilters
    ) -> list[dict[str, JsonValue]]:
        """Return concise, stable projections for one resource kind."""

        selected_agent = self._validated_agent_filter(filters)
        if resource == "teams":
            records = self._list_teams(filters)
        elif resource == "agents":
            records = self._list_agents(filters, selected_agent)
        elif resource == "phases":
            records = self._list_phases(filters, selected_agent)
        elif resource == "rollups":
            records = self._list_rollups(filters)
        else:
            raise ValueError(f"unsupported query resource {resource!r}")
        return sorted(
            records,
            key=lambda item: (
                _optional_int(item, "start_ms") or -1,
                _optional_string(item, "ref") or "",
            ),
        )

    def _team_range(self, slug: str) -> tuple[int, int] | None:
        intervals = [
            _interval(record, f"agent {agent_ref(record)}")
            for record in self.agents
            if _team(record, "agent") == slug
        ]
        if not intervals:
            intervals = [
                _interval(record, f"rollup {rollup_ref(record)}")
                for record in self.rollups
                if _team(record, "rollup") == slug
            ]
        if not intervals:
            return None
        return min(start for start, _end in intervals), max(
            end for _start, end in intervals
        )

    def _list_teams(self, filters: QueryFilters) -> list[dict[str, JsonValue]]:
        results: list[dict[str, JsonValue]] = []
        for index, record in enumerate(self.teams):
            where = f"timeline.teams[{index}]"
            slug = as_string(record.get("slug"), f"{where}.slug")
            interval = self._team_range(slug)
            if not self._selected_team(slug, filters):
                continue
            if interval is not None and filters.window is not None:
                if not filters.window.overlaps(interval[0], interval[1]):
                    continue
            result = _copy_fields(
                record,
                ("slug", "label", "provider", "projects", "hosts", "stats"),
            )
            result["ref"] = team_ref(slug)
            if interval is not None:
                result["start_ms"], result["end_ms"] = interval
                result["start_time"] = _timestamp(interval[0])
                result["end_time"] = _timestamp(interval[1])
            results.append(result)
        return results

    def _list_agents(
        self, filters: QueryFilters, selected_agent: str | None
    ) -> list[dict[str, JsonValue]]:
        results: list[dict[str, JsonValue]] = []
        for index, record in enumerate(self.agents):
            where = f"timeline.agents[{index}]"
            reference = agent_ref(record, where)
            team = _team(record, where)
            if not self._selected_team(team, filters):
                continue
            if selected_agent is not None and reference != selected_agent:
                continue
            if not _overlaps(record, where, filters.window):
                continue
            result = _copy_fields(
                record,
                (
                    "team",
                    "short_name",
                    "official_name",
                    "nickname",
                    "lifetime_summary",
                    "depth",
                    "status",
                    "start_ms",
                    "end_ms",
                ),
            )
            result["ref"] = reference
            parent = record.get("parent_id")
            result["parent_ref"] = (
                None
                if parent is None
                else self._agent_reference_for_id(
                    team, as_string(parent, f"{where}.parent_id")
                )
            )
            _with_times(result, record, where)
            results.append(result)
        return results

    def _list_phases(
        self, filters: QueryFilters, selected_agent: str | None
    ) -> list[dict[str, JsonValue]]:
        results: list[dict[str, JsonValue]] = []
        for index, record in enumerate(self.phases):
            where = f"timeline.phases[{index}]"
            team = _team(record, where)
            current_agent = self._agent_reference_for_id(
                team, as_string(record.get("agent_id"), f"{where}.agent_id")
            )
            if not self._selected_team(team, filters):
                continue
            if selected_agent is not None and current_agent != selected_agent:
                continue
            if not _overlaps(record, where, filters.window):
                continue
            result = _copy_fields(
                record,
                (
                    "team",
                    "phrase",
                    "paragraph",
                    "stats",
                    "start_ms",
                    "end_ms",
                ),
            )
            result["ref"] = phase_ref(record, where)
            result["agent_ref"] = current_agent
            _with_times(result, record, where)
            results.append(result)
        return results

    def _list_rollups(self, filters: QueryFilters) -> list[dict[str, JsonValue]]:
        results: list[dict[str, JsonValue]] = []
        for index, record in enumerate(self.rollups):
            where = f"timeline.rollups[{index}]"
            team = _team(record, where)
            kind = as_string(record.get("kind"), f"{where}.kind")
            if not self._selected_team(team, filters):
                continue
            if filters.rollup_kinds and kind not in filters.rollup_kinds:
                continue
            if not _overlaps(record, where, filters.window):
                continue
            result = _copy_fields(
                record,
                (
                    "team",
                    "kind",
                    "label",
                    "start_ms",
                    "end_ms",
                    "technical_path",
                    "plain_language_path",
                ),
            )
            result["ref"] = rollup_ref(record, where)
            _with_times(result, record, where)
            results.append(result)
        return results

    def _agent_reference_for_id(self, team: str, identifier: str) -> str:
        return f"agent:{team}::{_local_identifier(team, identifier)}"

    @staticmethod
    def _same_agent_identifier(team: str, left: str, right: str) -> bool:
        return _local_identifier(team, left) == _local_identifier(team, right)

    def _safe_file(self, relative: str) -> tuple[Path, PurePosixPath]:
        pure = PurePosixPath(relative)
        if pure.is_absolute() or not pure.parts or any(
            part in {"", ".", ".."} for part in pure.parts
        ):
            raise ValueError(f"archive path escapes root: {relative!r}")
        path = self.root.joinpath(*pure.parts).resolve()
        try:
            path.relative_to(self.root)
        except ValueError as error:
            raise ValueError(f"archive path escapes root: {relative!r}") from error
        if not path.is_file():
            raise ValueError(f"archive file is missing: {relative}")
        return path, pure

    def _detail_file(self, relative: str) -> Path:
        path, pure = self._safe_file(relative)
        if (
            len(pure.parts) < 3
            or pure.parts[:2] != ("data", "details")
            or pure.suffix != ".json"
        ):
            raise ValueError(f"phase detail path is outside data/details: {relative!r}")
        return path

    def _rollup_file(self, relative: str, team: str) -> Path:
        path, pure = self._safe_file(relative)
        if (
            len(pure.parts) < 5
            or pure.parts[:3] != ("teams", team, "summaries")
            or pure.suffix != ".md"
        ):
            raise ValueError(
                f"rollup path is outside teams/{team}/summaries: {relative!r}"
            )
        return path

    def show(self, reference: str, *, transcript: bool = False) -> dict[str, JsonValue]:
        """Resolve one stable reference and include useful relationship links."""

        entry = self._entries.get(reference)
        if entry is None:
            raise ValueError(f"unknown stable reference {reference!r}")
        result = dict(entry.record)
        result["ref"] = reference
        result["record_type"] = entry.kind
        if entry.kind == "team":
            self._expand_team(result, entry.record)
        elif entry.kind == "agent":
            self._expand_agent(result, entry.record)
        elif entry.kind == "phase":
            self._expand_phase(result, entry.record, transcript)
        elif entry.kind == "rollup":
            self._expand_rollup(result, entry.record)
        return result

    def _expand_team(
        self, result: dict[str, JsonValue], record: dict[str, JsonValue]
    ) -> None:
        slug = as_string(record.get("slug"), "team.slug")
        result["agent_refs"] = [
            agent_ref(agent)
            for agent in self.agents
            if _team(agent, "agent") == slug
        ]
        result["rollup_refs"] = [
            rollup_ref(rollup)
            for rollup in self.rollups
            if _team(rollup, "rollup") == slug
        ]
        interval = self._team_range(slug)
        if interval is not None:
            result["start_ms"], result["end_ms"] = interval
            result["start_time"] = _timestamp(interval[0])
            result["end_time"] = _timestamp(interval[1])

    def _expand_agent(
        self, result: dict[str, JsonValue], record: dict[str, JsonValue]
    ) -> None:
        team = _team(record, "agent")
        identifier = as_string(record.get("id"), "agent.id")
        parent = record.get("parent_id")
        result["parent_ref"] = (
            None
            if parent is None
            else self._agent_reference_for_id(
                team, as_string(parent, "agent.parent_id")
            )
        )
        result["child_refs"] = [
            agent_ref(candidate)
            for candidate in self.agents
            if _team(candidate, "agent") == team
            and isinstance(candidate.get("parent_id"), str)
            and self._same_agent_identifier(
                team,
                as_string(candidate.get("parent_id"), "agent.parent_id"),
                identifier,
            )
        ]
        result["phase_refs"] = [
            phase_ref(phase)
            for phase in self.phases
            if _team(phase, "phase") == team
            and self._same_agent_identifier(
                team,
                as_string(phase.get("agent_id"), "phase.agent_id"),
                identifier,
            )
        ]
        _with_times(result, record, "agent")

    def _expand_phase(
        self,
        result: dict[str, JsonValue],
        record: dict[str, JsonValue],
        transcript: bool,
    ) -> None:
        team = _team(record, "phase")
        identifier = as_string(record.get("agent_id"), "phase.agent_id")
        result["agent_ref"] = self._agent_reference_for_id(team, identifier)
        relative = as_string(record.get("detail_path"), "phase.detail_path")
        detail = as_object(read_json(self._detail_file(relative)), relative)
        result["detail"] = (
            detail
            if transcript
            else {key: value for key, value in detail.items() if key != "transcript"}
        )
        _with_times(result, record, "phase")

    def _expand_rollup(
        self, result: dict[str, JsonValue], record: dict[str, JsonValue]
    ) -> None:
        technical_path = as_string(
            record.get("technical_path"), "rollup.technical_path"
        )
        plain_path = as_string(
            record.get("plain_language_path"), "rollup.plain_language_path"
        )
        team = _team(record, "rollup")
        result["technical_markdown"] = self._rollup_file(technical_path, team).read_text(
            encoding="utf-8"
        )
        result["plain_language_markdown"] = self._rollup_file(plain_path, team).read_text(
            encoding="utf-8"
        )
        _with_times(result, record, "rollup")

    def search(
        self,
        needle: str,
        *,
        scope: str,
        filters: QueryFilters,
        case_sensitive: bool,
        limit: int,
    ) -> list[dict[str, JsonValue]]:
        """Search summaries and/or condensed transcript messages without an index."""

        if not needle:
            raise ValueError("search text must not be empty")
        if scope not in {"summaries", "transcripts", "all"}:
            raise ValueError(f"unsupported search scope {scope!r}")
        if limit < 1:
            raise ValueError("--limit must be at least 1")
        selected_agent = self._validated_agent_filter(filters)
        matches: list[dict[str, JsonValue]] = []
        if scope in {"summaries", "all"}:
            matches.extend(
                self._search_summaries(
                    needle, filters, selected_agent, case_sensitive
                )
            )
        if scope in {"transcripts", "all"}:
            matches.extend(
                self._search_transcripts(
                    needle, filters, selected_agent, case_sensitive
                )
            )
        matches.sort(
            key=lambda item: (
                _optional_int(item, "at_ms")
                or _optional_int(item, "start_ms")
                or -1,
                _optional_string(item, "ref") or "",
                _optional_string(item, "field") or "",
            )
        )
        return matches[:limit]

    @staticmethod
    def _contains(text: str, needle: str, case_sensitive: bool) -> bool:
        if case_sensitive:
            return needle in text
        return needle.casefold() in text.casefold()

    @staticmethod
    def _excerpt(text: str, needle: str, case_sensitive: bool) -> str:
        compact = _WHITESPACE.sub(" ", text).strip()
        haystack = compact if case_sensitive else compact.casefold()
        sought = needle if case_sensitive else needle.casefold()
        position = haystack.find(sought)
        if position < 0:
            return compact[:240]
        start = max(0, position - 100)
        end = min(len(compact), position + len(needle) + 140)
        prefix = "…" if start else ""
        suffix = "…" if end < len(compact) else ""
        return f"{prefix}{compact[start:end]}{suffix}"

    def _match(
        self,
        results: list[dict[str, JsonValue]],
        *,
        needle: str,
        case_sensitive: bool,
        reference: str,
        record_type: str,
        team: str,
        field: str,
        text: str,
        start_ms: int | None = None,
        at_ms: int | None = None,
    ) -> None:
        if not self._contains(text, needle, case_sensitive):
            return
        result: dict[str, JsonValue] = {
            "ref": reference,
            "record_type": record_type,
            "team": team,
            "field": field,
            "excerpt": self._excerpt(text, needle, case_sensitive),
        }
        if start_ms is not None:
            result["start_ms"] = start_ms
            result["start_time"] = _timestamp(start_ms)
        if at_ms is not None:
            result["at_ms"] = at_ms
            result["at_time"] = _timestamp(at_ms)
        results.append(result)

    def _search_summaries(
        self,
        needle: str,
        filters: QueryFilters,
        selected_agent: str | None,
        case_sensitive: bool,
    ) -> list[dict[str, JsonValue]]:
        results: list[dict[str, JsonValue]] = []
        for record in self.agents:
            team = _team(record, "agent")
            reference = agent_ref(record)
            if not self._selected_team(team, filters):
                continue
            if selected_agent is not None and reference != selected_agent:
                continue
            if not _overlaps(record, reference, filters.window):
                continue
            start_ms, _end_ms = _interval(record, reference)
            for field in (
                "short_name",
                "official_name",
                "nickname",
                "lifetime_summary",
                "naming_rationale",
            ):
                text = _optional_string(record, field)
                if text:
                    self._match(
                        results,
                        needle=needle,
                        case_sensitive=case_sensitive,
                        reference=reference,
                        record_type="agent",
                        team=team,
                        field=field,
                        text=text,
                        start_ms=start_ms,
                    )
        for record in self.phases:
            team = _team(record, "phase")
            current_agent = self._agent_reference_for_id(
                team, as_string(record.get("agent_id"), "phase.agent_id")
            )
            if not self._selected_team(team, filters):
                continue
            if selected_agent is not None and current_agent != selected_agent:
                continue
            reference = phase_ref(record)
            if not _overlaps(record, reference, filters.window):
                continue
            start_ms, _end_ms = _interval(record, reference)
            for field in ("phrase", "paragraph"):
                text = _optional_string(record, field)
                if text:
                    self._match(
                        results,
                        needle=needle,
                        case_sensitive=case_sensitive,
                        reference=reference,
                        record_type="phase",
                        team=team,
                        field=field,
                        text=text,
                        start_ms=start_ms,
                    )
        if selected_agent is None:
            results.extend(
                self._search_rollups(needle, filters, case_sensitive)
            )
        return results

    def _search_rollups(
        self, needle: str, filters: QueryFilters, case_sensitive: bool
    ) -> list[dict[str, JsonValue]]:
        results: list[dict[str, JsonValue]] = []
        for record in self.rollups:
            team = _team(record, "rollup")
            kind = as_string(record.get("kind"), "rollup.kind")
            reference = rollup_ref(record)
            if not self._selected_team(team, filters):
                continue
            if filters.rollup_kinds and kind not in filters.rollup_kinds:
                continue
            if not _overlaps(record, reference, filters.window):
                continue
            start_ms, _end_ms = _interval(record, reference)
            fields = (
                (
                    "technical_markdown",
                    as_string(record.get("technical_path"), "rollup.technical_path"),
                ),
                (
                    "plain_language_markdown",
                    as_string(
                        record.get("plain_language_path"),
                        "rollup.plain_language_path",
                    ),
                ),
            )
            for field, relative in fields:
                text = self._rollup_file(relative, team).read_text(encoding="utf-8")
                self._match(
                    results,
                    needle=needle,
                    case_sensitive=case_sensitive,
                    reference=reference,
                    record_type="rollup",
                    team=team,
                    field=field,
                    text=text,
                    start_ms=start_ms,
                )
        return results

    def _search_transcripts(
        self,
        needle: str,
        filters: QueryFilters,
        selected_agent: str | None,
        case_sensitive: bool,
    ) -> list[dict[str, JsonValue]]:
        results: list[dict[str, JsonValue]] = []
        for record in self.phases:
            team = _team(record, "phase")
            current_agent = self._agent_reference_for_id(
                team, as_string(record.get("agent_id"), "phase.agent_id")
            )
            if not self._selected_team(team, filters):
                continue
            if selected_agent is not None and current_agent != selected_agent:
                continue
            reference = phase_ref(record)
            if not _overlaps(record, reference, filters.window):
                continue
            relative = as_string(record.get("detail_path"), "phase.detail_path")
            detail = as_object(read_json(self._detail_file(relative)), relative)
            transcript = as_array(detail.get("transcript"), f"{relative}.transcript")
            for index, raw_message in enumerate(transcript):
                message = as_object(raw_message, f"{relative}.transcript[{index}]")
                at_ms = as_int(
                    message.get("at_ms"),
                    f"{relative}.transcript[{index}].at_ms",
                )
                if filters.window is not None and not filters.window.contains(at_ms):
                    continue
                text = as_string(
                    message.get("text"), f"{relative}.transcript[{index}].text"
                )
                if not self._contains(text, needle, case_sensitive):
                    continue
                role = as_string(
                    message.get("role"),
                    f"{relative}.transcript[{index}].role",
                )
                self._match(
                    results,
                    needle=needle,
                    case_sensitive=case_sensitive,
                    reference=reference,
                    record_type="transcript",
                    team=team,
                    field=role,
                    text=text,
                    at_ms=at_ms,
                )
        return results


def query_envelope(command: str, items: list[dict[str, JsonValue]]) -> dict[str, JsonValue]:
    """Wrap records in a small versioned response for default JSON output."""

    widened_items: list[JsonValue] = [item for item in items]
    return {
        "schema_version": QUERY_SCHEMA_VERSION,
        "command": command,
        "count": len(items),
        "items": widened_items,
    }


def _markdown_title(item: dict[str, JsonValue]) -> str:
    for key in ("short_name", "label", "phrase", "slug", "ref"):
        value = _optional_string(item, key)
        if value:
            return value.replace("\n", " ")
    return "Result"


def _markdown_item(item: dict[str, JsonValue]) -> str:
    lines = [f"## {_markdown_title(item)}", ""]
    for key in (
        "ref",
        "record_type",
        "team",
        "provider",
        "kind",
        "agent_ref",
        "parent_ref",
        "start_time",
        "end_time",
        "at_time",
        "field",
    ):
        value = item.get(key)
        if isinstance(value, (str, int, float, bool)):
            lines.append(f"- {key.replace('_', ' ').title()}: `{value}`")
    lines.append("")
    for key in (
        "lifetime_summary",
        "paragraph",
        "excerpt",
        "technical_markdown",
        "plain_language_markdown",
    ):
        text = _optional_string(item, key)
        if not text:
            continue
        if key in {"technical_markdown", "plain_language_markdown"}:
            lines.extend((f"### {key.replace('_', ' ').title()}", ""))
        lines.extend((text.rstrip(), ""))
    detail_value = item.get("detail")
    if isinstance(detail_value, dict):
        transcript_value = detail_value.get("transcript")
        if isinstance(transcript_value, list):
            lines.extend(("### Transcript", ""))
            for index, raw_message in enumerate(transcript_value):
                message = as_object(raw_message, f"detail.transcript[{index}]")
                role = as_string(message.get("role"), f"detail.transcript[{index}].role")
                at_ms = as_int(message.get("at_ms"), f"detail.transcript[{index}].at_ms")
                text = as_string(message.get("text"), f"detail.transcript[{index}].text")
                lines.extend((f"#### {role} · {_timestamp(at_ms)}", "", text.rstrip(), ""))
    return "\n".join(lines).rstrip()


def format_query(
    command: str, items: list[dict[str, JsonValue]], output_format: str
) -> str:
    """Render query records as versioned JSON, streaming JSONL, or compact Markdown."""

    if output_format == "json":
        return canonical_json(query_envelope(command, items))
    if output_format == "jsonl":
        return "".join(
            json.dumps(item, ensure_ascii=False, sort_keys=True) + "\n"
            for item in items
        )
    if output_format == "markdown":
        body = "\n\n".join(_markdown_item(item) for item in items)
        suffix = f"\n\n_{len(items)} result(s)._\n"
        return f"# agent-team-timeline query: {command}\n\n{body}{suffix}"
    raise ValueError(f"unsupported query output format {output_format!r}")


@dataclass(frozen=True)
class _StandaloneWindow:
    start_ms: int | None
    end_ms: int | None

    def contains(self, timestamp_ms: int) -> bool:
        if self.start_ms is not None and timestamp_ms < self.start_ms:
            return False
        return self.end_ms is None or timestamp_ms < self.end_ms

    def overlaps(self, start_ms: int, end_ms: int | None) -> bool:
        effective_end = end_ms if end_ms is not None else start_ms + 1
        if effective_end <= start_ms:
            effective_end = start_ms + 1
        if self.end_ms is not None and start_ms >= self.end_ms:
            return False
        return self.start_ms is None or effective_end > self.start_ms


def _parse_instant(raw: str, label: str) -> int:
    if _RFC3339_INSTANT.fullmatch(raw) is None:
        raise ValueError(
            f"{label} must be an RFC3339 timestamp with an explicit offset or Z"
        )
    normalized = raw[:-1] + "+00:00" if raw.endswith("Z") else raw
    try:
        value = datetime.fromisoformat(normalized)
    except ValueError as error:
        raise ValueError(
            f"{label} must be an RFC3339 timestamp with an explicit offset or Z"
        ) from error
    if value.tzinfo is None or value.utcoffset() is None:
        raise ValueError(f"{label} must include an explicit offset or Z")
    return int(value.timestamp() * 1000)


def _standalone_add_filters(parser: argparse.ArgumentParser) -> None:
    parser.add_argument("--team", action="append", default=[])
    parser.add_argument("--start-time")
    parser.add_argument("--end-time")
    parser.add_argument(
        "--kind",
        action="append",
        choices=("hourly", "daily", "weekly", "monthly", "quarterly"),
        default=[],
    )
    parser.add_argument("--agent")


def _standalone_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="query.py",
        description="Read-only navigation over this built agent-team timeline.",
    )
    parser.add_argument("--output", default=".")
    parser.add_argument(
        "--format", choices=("json", "jsonl", "markdown"), default="json"
    )
    sub = parser.add_subparsers(dest="action", required=True)
    listing = sub.add_parser("list")
    listing.add_argument("resource", choices=("teams", "agents", "phases", "rollups"))
    _standalone_add_filters(listing)
    showing = sub.add_parser("show")
    showing.add_argument("reference")
    showing.add_argument("--transcript", action="store_true")
    searching = sub.add_parser("search")
    searching.add_argument("text")
    searching.add_argument(
        "--scope", choices=("summaries", "transcripts", "all"), default="summaries"
    )
    searching.add_argument("--case-sensitive", action="store_true")
    searching.add_argument("--limit", type=int, default=50)
    _standalone_add_filters(searching)
    return parser


def _standalone_filters(ns: argparse.Namespace) -> QueryFilters:
    raw_teams: object = ns.team
    raw_kinds: object = ns.kind
    raw_agent: object = ns.agent
    if not isinstance(raw_teams, list) or not all(
        isinstance(item, str) for item in raw_teams
    ):
        raise ValueError("--team values must be strings")
    if not isinstance(raw_kinds, list) or not all(
        isinstance(item, str) for item in raw_kinds
    ):
        raise ValueError("--kind values must be strings")
    if raw_agent is not None and not isinstance(raw_agent, str):
        raise ValueError("--agent must be a string")
    start_ms = (
        _parse_instant(str(ns.start_time), "start time")
        if ns.start_time is not None
        else None
    )
    end_ms = (
        _parse_instant(str(ns.end_time), "end time")
        if ns.end_time is not None
        else None
    )
    if start_ms is not None and end_ms is not None and start_ms >= end_ms:
        raise ValueError("start bound must be earlier than end bound")
    window = (
        None
        if start_ms is None and end_ms is None
        else _StandaloneWindow(start_ms, end_ms)
    )
    return QueryFilters(tuple(raw_teams), window, tuple(raw_kinds), raw_agent)


def _standalone_main(argv: Sequence[str] | None = None) -> int:
    parser = _standalone_parser()
    ns = parser.parse_args(list(argv) if argv is not None else None)
    try:
        query = TimelineQuery(Path(str(ns.output)).expanduser())
        action = str(ns.action)
        if action == "list":
            resource = str(ns.resource)
            command = f"list {resource}"
            items = query.list_records(resource, _standalone_filters(ns))
        elif action == "show":
            reference = str(ns.reference)
            command = f"show {reference}"
            items = [query.show(reference, transcript=bool(ns.transcript))]
        elif action == "search":
            needle = str(ns.text)
            command = f"search {needle}"
            items = query.search(
                needle,
                scope=str(ns.scope),
                filters=_standalone_filters(ns),
                case_sensitive=bool(ns.case_sensitive),
                limit=int(ns.limit),
            )
        else:
            raise ValueError(f"unsupported query action {action!r}")
        print(format_query(command, items, str(ns.format)), end="")
        return 0
    except (OSError, ValueError) as error:
        print(f"query.py: {error}", file=sys.stderr)
        return 2


__all__ = [
    "QueryFilters",
    "TimelineQuery",
    "agent_ref",
    "format_query",
    "phase_ref",
    "query_envelope",
    "rollup_ref",
    "team_ref",
]


if __name__ == "__main__":
    raise SystemExit(_standalone_main())
