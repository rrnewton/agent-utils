"""Deterministic, zero-model exports of coordinator prompts and responses."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from agent_team_timeline.archive import (
    JsonValue,
    as_array,
    as_int,
    as_object,
    as_string,
    canonical_json,
    narrow_json,
    read_json,
    write_text_if_changed,
)
from agent_team_timeline.model import Event, TeamData, source_digest
from agent_team_timeline.render import archive_makefile, standalone_query_source


TRANSCRIPT_EXPORT_SCHEMA_VERSION = 1
_MANAGED_FILES = (
    "occurrences.jsonl",
    "prompts.jsonl",
    "messages.jsonl",
    "system-inputs.jsonl",
)


@dataclass(frozen=True)
class TranscriptExportReport:
    """Counts produced by one mechanical transcript export."""

    teams: int
    prompts: int
    responses: int
    system_inputs: int
    carried_forward: int
    files_changed: int

    def to_json_obj(self) -> dict[str, JsonValue]:
        """Return a run-receipt-compatible JSON object."""

        return {
            "teams": self.teams,
            "prompts": self.prompts,
            "responses": self.responses,
            "system_inputs": self.system_inputs,
            "carried_forward": self.carried_forward,
            "files_changed": self.files_changed,
            "model_calls": 0,
            "model_tokens": 0,
        }


@dataclass(frozen=True)
class _GroupedEvent:
    team: TeamData
    events: tuple[Event, ...]
    text: str

    @property
    def first(self) -> Event:
        return self.events[0]


def _sha256_text(text: str) -> str:
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def _stable_id(prefix: str, fields: Iterable[str]) -> str:
    material = "\0".join(fields)
    return f"{prefix}:{hashlib.sha256(material.encode('utf-8')).hexdigest()[:32]}"


def _instant(timestamp_ms: int, zone: timezone | ZoneInfo) -> str:
    return datetime.fromtimestamp(timestamp_ms / 1000, tz=zone).isoformat(
        timespec="milliseconds"
    )


def _timezone(team: TeamData) -> timezone | ZoneInfo:
    try:
        return ZoneInfo(team.display_timezone)
    except ZoneInfoNotFoundError as error:
        raise ValueError(
            f"team {team.team_slug!r} has unknown display timezone "
            f"{team.display_timezone!r}"
        ) from error


def _coordinator_threads(team: TeamData) -> set[str]:
    result = {team.root_thread_id}
    result.update(
        agent.thread_id for agent in team.agents if agent.role == "coordinator"
    )
    return result


def _group_key(event: Event) -> tuple[str, str, int, int, str, str]:
    native = event.source_native_id or ""
    fallback = "" if native else event.event_id.rsplit(":", 1)[0]
    return (
        event.thread_id,
        event.turn_id or "",
        event.timestamp_ms,
        event.source_line,
        event.kind,
        native or fallback,
    )


def _group_events(team: TeamData, kinds: set[str]) -> tuple[_GroupedEvent, ...]:
    coordinator_threads = _coordinator_threads(team)
    grouped: dict[tuple[str, str, int, int, str, str], list[Event]] = {}
    for event in team.events:
        if (
            event.thread_id not in coordinator_threads
            or event.kind not in kinds
            or event.text is None
            or not event.text.strip()
        ):
            continue
        grouped.setdefault(_group_key(event), []).append(event)
    result: list[_GroupedEvent] = []
    for events in grouped.values():
        ordered = tuple(sorted(events, key=lambda event: event.event_id))
        text = "\n\n".join(
            event.text.strip() for event in ordered if event.text is not None
        )
        result.append(_GroupedEvent(team, ordered, text))
    return tuple(
        sorted(
            result,
            key=lambda group: (
                group.first.timestamp_ms,
                group.first.source_line,
                group.first.event_id,
            ),
        )
    )


def _agent_fields(team: TeamData, thread_id: str) -> tuple[str, str]:
    for agent in team.agents:
        if agent.thread_id == thread_id:
            return agent.agent_path, agent.source_path
    return "/root", ""


def _base_record(group: _GroupedEvent, record_type: str) -> dict[str, JsonValue]:
    event = group.first
    team = group.team
    agent_path, source_path = _agent_fields(team, event.thread_id)
    digest = _sha256_text(group.text)
    event_ids: list[JsonValue] = [item.event_id for item in group.events]
    native_id = event.source_native_id
    occurrence_fields = (
        team.team_slug,
        team.provider,
        event.thread_id,
        source_path,
        native_id or "",
        str(event.source_line),
        str(event.timestamp_ms),
        digest,
        *[item.event_id for item in group.events],
    )
    logical_fields = (
        team.provider,
        event.thread_id,
        native_id or event.event_id,
        str(event.timestamp_ms),
        digest,
    )
    zone = _timezone(team)
    return {
        "schema_version": TRANSCRIPT_EXPORT_SCHEMA_VERSION,
        "record_type": record_type,
        "record_id": _stable_id(record_type, occurrence_fields),
        "logical_record_id": _stable_id(f"logical-{record_type}", logical_fields),
        "team_slug": team.team_slug,
        "provider": team.provider,
        "thread_id": event.thread_id,
        "turn_id": event.turn_id,
        "agent_path": agent_path,
        "timestamp_ms": event.timestamp_ms,
        "timestamp_utc": _instant(event.timestamp_ms, timezone.utc).replace(
            "+00:00", "Z"
        ),
        "timestamp_local": _instant(event.timestamp_ms, zone),
        "display_timezone": team.display_timezone,
        "text": group.text,
        "content_sha256": digest,
        "source_path": source_path,
        "source_line": event.source_line,
        "source_event_ids": event_ids,
        "source_native_id": native_id,
        "ingress_kind": event.ingress_kind,
        "author_kind": event.author_kind,
        "classification_version": event.classification_version,
    }


def _response_records(
    teams: tuple[TeamData, ...],
    prompts: list[dict[str, JsonValue]],
) -> list[dict[str, JsonValue]]:
    prompt_candidates: dict[tuple[str, str, str], list[dict[str, JsonValue]]] = {}
    for prompt in prompts:
        turn_id = prompt.get("turn_id")
        if not isinstance(turn_id, str):
            continue
        key = (
            as_string(prompt.get("team_slug"), "prompt.team_slug"),
            as_string(prompt.get("thread_id"), "prompt.thread_id"),
            turn_id,
        )
        prompt_candidates.setdefault(key, []).append(prompt)
    for values in prompt_candidates.values():
        values.sort(
            key=lambda item: (
                as_int(item.get("timestamp_ms"), "prompt.timestamp_ms"),
                as_int(item.get("source_line"), "prompt.source_line"),
            )
        )

    responses: list[dict[str, JsonValue]] = []
    for team in teams:
        for group in _group_events(team, {"assistant_message"}):
            record = _base_record(group, "response")
            turn_id = group.first.turn_id
            candidates = (
                prompt_candidates.get(
                    (team.team_slug, group.first.thread_id, turn_id), ()
                )
                if turn_id is not None
                else ()
            )
            eligible = [
                prompt
                for prompt in candidates
                if as_int(prompt.get("timestamp_ms"), "prompt.timestamp_ms")
                <= group.first.timestamp_ms
            ]
            linked = eligible[-1] if eligible else None
            record["in_reply_to_prompt_id"] = (
                as_string(linked.get("record_id"), "prompt.record_id")
                if linked is not None
                else None
            )
            responses.append(record)
    return responses


def _record_sort_key(record: dict[str, JsonValue]) -> tuple[int, int, str, str]:
    record_type = as_string(record.get("record_type"), "record.record_type")
    type_rank = 0 if record_type == "prompt" else 1
    return (
        as_int(record.get("timestamp_ms"), "record.timestamp_ms"),
        type_rank,
        as_string(record.get("team_slug"), "record.team_slug"),
        as_string(record.get("record_id"), "record.record_id"),
    )


def _jsonl(records: Iterable[dict[str, JsonValue]]) -> str:
    return "".join(
        json.dumps(record, ensure_ascii=False, sort_keys=True, separators=(",", ":"))
        + "\n"
        for record in records
    )


def _load_jsonl(path: Path) -> list[dict[str, JsonValue]]:
    if not path.exists():
        return []
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"managed transcript export is not a regular file: {path}")
    result: list[dict[str, JsonValue]] = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line:
            continue
        value = narrow_json(json.loads(line), f"{path}:{line_number}")
        result.append(as_object(value, f"{path}:{line_number}"))
    return result


def _immutable_projection(record: dict[str, JsonValue]) -> dict[str, JsonValue]:
    mutable = {
        "ordinal",
        "ingress_kind",
        "author_kind",
        "classification_version",
        "in_reply_to_prompt_id",
    }
    return {key: value for key, value in record.items() if key not in mutable}


def _monotonic_union(
    old: list[dict[str, JsonValue]], new: list[dict[str, JsonValue]]
) -> tuple[list[dict[str, JsonValue]], int]:
    merged: dict[str, dict[str, JsonValue]] = {}
    for record in old:
        record_id = as_string(record.get("record_id"), "old record.record_id")
        if record_id in merged:
            raise ValueError(f"duplicate existing transcript record {record_id!r}")
        merged[record_id] = record
    new_ids: set[str] = set()
    for record in new:
        record_id = as_string(record.get("record_id"), "new record.record_id")
        if record_id in new_ids:
            raise ValueError(f"duplicate new transcript record {record_id!r}")
        new_ids.add(record_id)
        previous = merged.get(record_id)
        if previous is not None and _immutable_projection(previous) != _immutable_projection(
            record
        ):
            raise ValueError(
                f"immutable transcript occurrence changed for {record_id!r}"
            )
        merged[record_id] = record
    carried = len(set(merged) - new_ids)
    return sorted(merged.values(), key=_record_sort_key), carried


def _logical_records(
    occurrences: Iterable[dict[str, JsonValue]], record_type: str
) -> list[dict[str, JsonValue]]:
    grouped: dict[str, list[dict[str, JsonValue]]] = {}
    for occurrence in occurrences:
        if occurrence.get("record_type") != record_type:
            continue
        logical_id = as_string(
            occurrence.get("logical_record_id"), "occurrence.logical_record_id"
        )
        grouped.setdefault(logical_id, []).append(occurrence)
    result: list[dict[str, JsonValue]] = []
    for logical_id, group in grouped.items():
        ordered = sorted(group, key=_record_sort_key)
        record = dict(ordered[0])
        record["record_id"] = logical_id
        occurrence_ids: list[JsonValue] = []
        for occurrence_id in sorted(
            as_string(item.get("record_id"), "occurrence.record_id")
            for item in ordered
        ):
            occurrence_ids.append(occurrence_id)
        occurrence_teams: list[JsonValue] = []
        for occurrence_team in sorted(
            {
                as_string(item.get("team_slug"), "occurrence.team_slug")
                for item in ordered
            }
        ):
            occurrence_teams.append(occurrence_team)
        record["occurrence_ids"] = occurrence_ids
        record["occurrence_teams"] = occurrence_teams
        record["occurrence_count"] = len(ordered)
        result.append(record)
    return sorted(result, key=_record_sort_key)


def _validate_previous_manifest(root: Path) -> dict[str, JsonValue] | None:
    path = root / "manifest.json"
    if not path.exists():
        return None
    manifest = as_object(read_json(path), str(path))
    if manifest.get("schema_version") != TRANSCRIPT_EXPORT_SCHEMA_VERSION:
        raise ValueError(f"unsupported transcript export manifest at {path}")
    files = as_object(manifest.get("files"), f"{path}.files")
    for name in _MANAGED_FILES:
        entry = as_object(files.get(name), f"{path}.files.{name}")
        expected = as_string(entry.get("sha256"), f"{path}.files.{name}.sha256")
        managed = root / name
        if not managed.is_file() or managed.is_symlink():
            raise ValueError(f"transcript export file is missing or unsafe: {managed}")
        actual = hashlib.sha256(managed.read_bytes()).hexdigest()
        if actual != expected:
            raise ValueError(
                f"transcript export generation is incomplete: {managed} digest mismatch"
            )
    return manifest


def export_transcripts(
    archive: Path, teams: Iterable[TeamData]
) -> TranscriptExportReport:
    """Update the archive's append-only coordinator transcript projection.

    This function performs no network requests and has no model integration. Existing source
    occurrences are retained when a provider's current snapshot no longer contains them, which
    protects the extracted corpus from provider-side history rewriting or log rotation.
    """

    ordered_teams = tuple(sorted(teams, key=lambda team: team.team_slug))
    if not ordered_teams:
        raise ValueError("transcript extraction requires at least one ingested team")
    if len({team.team_slug for team in ordered_teams}) != len(ordered_teams):
        raise ValueError("transcript extraction received duplicate team slugs")
    root = archive / "extracted" / "transcripts"
    previous_manifest = _validate_previous_manifest(root)
    if previous_manifest is not None:
        previous_teams = {
            as_string(value, "transcript manifest team")
            for value in as_array(previous_manifest.get("teams"), "transcript manifest teams")
        }
        selected_teams = {team.team_slug for team in ordered_teams}
        omitted = sorted(previous_teams - selected_teams)
        if omitted:
            raise ValueError(
                "monotonic transcript export cannot omit previously extracted teams: "
                + ", ".join(omitted)
            )

    current_prompts = [
        _base_record(group, "prompt")
        for team in ordered_teams
        for group in _group_events(team, {"user_prompt"})
    ]
    current_responses = _response_records(ordered_teams, current_prompts)
    current_system = [
        _base_record(group, "system_input")
        for team in ordered_teams
        for group in _group_events(team, {"system_input"})
    ]

    current_occurrences = [*current_prompts, *current_responses, *current_system]
    occurrences, carried = _monotonic_union(
        _load_jsonl(root / "occurrences.jsonl"), current_occurrences
    )
    prompts = _logical_records(occurrences, "prompt")
    for index, prompt in enumerate(prompts, 1):
        prompt["ordinal"] = index
    occurrence_to_logical_prompt = {
        as_string(occurrence_id, "prompt.occurrence_ids[]"): as_string(
            prompt.get("record_id"), "prompt.record_id"
        )
        for prompt in prompts
        for occurrence_id in as_array(
            prompt.get("occurrence_ids"), "prompt.occurrence_ids"
        )
    }
    responses = _logical_records(occurrences, "response")
    for response in responses:
        linked = response.get("in_reply_to_prompt_id")
        response["in_reply_to_prompt_id"] = (
            occurrence_to_logical_prompt.get(linked)
            if isinstance(linked, str)
            else None
        )
    system_inputs = _logical_records(occurrences, "system_input")
    messages = sorted([*prompts, *responses], key=_record_sort_key)

    texts = {
        "occurrences.jsonl": _jsonl(occurrences),
        "prompts.jsonl": _jsonl(prompts),
        "messages.jsonl": _jsonl(messages),
        "system-inputs.jsonl": _jsonl(system_inputs),
    }
    changed = 0
    for name in _MANAGED_FILES:
        changed += int(write_text_if_changed(root / name, texts[name]))

    file_manifest: dict[str, JsonValue] = {}
    for name in _MANAGED_FILES:
        text = texts[name]
        file_manifest[name] = {
            "sha256": _sha256_text(text),
            "bytes": len(text.encode("utf-8")),
            "records": len(
                occurrences
                if name == "occurrences.jsonl"
                else prompts
                if name == "prompts.jsonl"
                else messages
                if name == "messages.jsonl"
                else system_inputs
            ),
        }
    source_generations: list[JsonValue] = [
        {
            "team_slug": team.team_slug,
            "provider": team.provider,
            "root_thread_id": team.root_thread_id,
            "source_digest": source_digest(team),
            "sources": len(team.sources),
        }
        for team in ordered_teams
    ]
    manifest: dict[str, JsonValue] = {
        "schema_version": TRANSCRIPT_EXPORT_SCHEMA_VERSION,
        "kind": "mechanical-coordinator-transcript-export",
        "model_calls": 0,
        "model_tokens": 0,
        "teams": [team.team_slug for team in ordered_teams],
        "source_generations": source_generations,
        "counts": {
            "prompts": len(prompts),
            "responses": len(responses),
            "system_inputs": len(system_inputs),
            "occurrences": len(occurrences),
            "carried_forward": carried,
        },
        "ordinal_contract": (
            "Prompt ordinals are 1-based chronological projection indexes; stable record_id "
            "values, not ordinals, are durable identities."
        ),
        "files": file_manifest,
    }
    changed += int(write_text_if_changed(root / "manifest.json", canonical_json(manifest)))
    changed += int(
        write_text_if_changed(
            archive / "query.py", standalone_query_source(), executable=True
        )
    )
    changed += int(
        write_text_if_changed(
            archive / "timeline", standalone_query_source(), executable=True
        )
    )
    changed += int(write_text_if_changed(archive / "Makefile", archive_makefile()))
    return TranscriptExportReport(
        teams=len(ordered_teams),
        prompts=len(prompts),
        responses=len(responses),
        system_inputs=len(system_inputs),
        carried_forward=carried,
        files_changed=changed,
    )


__all__ = [
    "TRANSCRIPT_EXPORT_SCHEMA_VERSION",
    "TranscriptExportReport",
    "export_transcripts",
]
