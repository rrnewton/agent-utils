"""Deterministic, zero-model exports of coordinator prompts and responses."""

from __future__ import annotations

import hashlib
import json
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Iterable, Sequence
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
_AUTHORSHIP_RULES_FILE = "authorship-rules.json"
_UNCLASSIFIED_AUTHOR_KINDS = frozenset({"external_or_unknown", "unknown"})
_RULE_AUTHOR_KINDS = frozenset(
    {"owner_human", "other_human", "agent", "system"}
)


@dataclass(frozen=True)
class PromptAuthorshipRule:
    """Auditable correction for an ingress interval without sender identity."""

    rule_id: str
    team_slug: str
    ingress_kind: str
    author_kind: str
    reason: str
    start_ms: int | None = None
    end_ms: int | None = None
    source_native_ids: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        for label, value in (
            ("rule_id", self.rule_id),
            ("team_slug", self.team_slug),
            ("ingress_kind", self.ingress_kind),
            ("reason", self.reason),
        ):
            if not value.strip() or "\0" in value:
                raise ValueError(f"prompt authorship {label} must be non-empty")
        if self.author_kind not in _RULE_AUTHOR_KINDS:
            raise ValueError(
                "prompt authorship author_kind must be owner_human, other_human, "
                "agent, or system"
            )
        if self.start_ms is not None and self.start_ms < 0:
            raise ValueError("prompt authorship start_ms must be non-negative")
        if self.end_ms is not None and self.end_ms < 0:
            raise ValueError("prompt authorship end_ms must be non-negative")
        if (
            self.start_ms is not None
            and self.end_ms is not None
            and self.start_ms >= self.end_ms
        ):
            raise ValueError("prompt authorship interval must be non-empty")
        if len(set(self.source_native_ids)) != len(self.source_native_ids):
            raise ValueError("prompt authorship source_native_ids contain duplicates")
        if any(not value or "\0" in value for value in self.source_native_ids):
            raise ValueError(
                "prompt authorship source_native_ids must contain non-empty strings"
            )

    def to_json_obj(self) -> dict[str, JsonValue]:
        """Return the complete, deterministic rule representation."""

        native_ids: list[JsonValue] = list(self.source_native_ids)
        return {
            "rule_id": self.rule_id,
            "team_slug": self.team_slug,
            "ingress_kind": self.ingress_kind,
            "author_kind": self.author_kind,
            "reason": self.reason,
            "start_ms": self.start_ms,
            "end_ms": self.end_ms,
            "source_native_ids": native_ids,
        }

    def matches(self, record: dict[str, JsonValue]) -> bool:
        """Return whether this rule selects one prompt occurrence."""

        if record.get("team_slug") != self.team_slug:
            return False
        if record.get("ingress_kind") != self.ingress_kind:
            return False
        timestamp_ms = as_int(record.get("timestamp_ms"), "prompt.timestamp_ms")
        if self.start_ms is not None and timestamp_ms < self.start_ms:
            return False
        if self.end_ms is not None and timestamp_ms >= self.end_ms:
            return False
        if not self.source_native_ids:
            return True
        native_id = record.get("source_native_id")
        return isinstance(native_id, str) and native_id in self.source_native_ids


@dataclass(frozen=True)
class TranscriptExportReport:
    """Counts produced by one mechanical transcript export."""

    teams: int
    prompts: int
    responses: int
    system_inputs: int
    carried_forward: int
    files_changed: int
    reclassified: int = 0

    def to_json_obj(self) -> dict[str, JsonValue]:
        """Return a run-receipt-compatible JSON object."""

        return {
            "teams": self.teams,
            "prompts": self.prompts,
            "responses": self.responses,
            "system_inputs": self.system_inputs,
            "carried_forward": self.carried_forward,
            "reclassified": self.reclassified,
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


def _prompt_authorship_rule_from_json(
    value: JsonValue, where: str
) -> PromptAuthorshipRule:
    obj = as_object(value, where)
    expected = {
        "rule_id",
        "team_slug",
        "ingress_kind",
        "author_kind",
        "reason",
        "start_ms",
        "end_ms",
        "source_native_ids",
    }
    if set(obj) != expected:
        raise ValueError(
            f"{where}: invalid fields; missing={sorted(expected - set(obj))!r}, "
            f"unknown={sorted(set(obj) - expected)!r}"
        )
    start_value = obj.get("start_ms")
    end_value = obj.get("end_ms")
    start_ms = None if start_value is None else as_int(start_value, where + ".start_ms")
    end_ms = None if end_value is None else as_int(end_value, where + ".end_ms")
    native_ids = tuple(
        as_string(item, f"{where}.source_native_ids[{index}]")
        for index, item in enumerate(
            as_array(obj.get("source_native_ids"), where + ".source_native_ids")
        )
    )
    return PromptAuthorshipRule(
        as_string(obj.get("rule_id"), where + ".rule_id"),
        as_string(obj.get("team_slug"), where + ".team_slug"),
        as_string(obj.get("ingress_kind"), where + ".ingress_kind"),
        as_string(obj.get("author_kind"), where + ".author_kind"),
        as_string(obj.get("reason"), where + ".reason"),
        start_ms,
        end_ms,
        native_ids,
    )


def _rules_overlap(left: PromptAuthorshipRule, right: PromptAuthorshipRule) -> bool:
    if left.team_slug != right.team_slug or left.ingress_kind != right.ingress_kind:
        return False
    if (
        left.end_ms is not None
        and right.start_ms is not None
        and left.end_ms <= right.start_ms
    ):
        return False
    if (
        right.end_ms is not None
        and left.start_ms is not None
        and right.end_ms <= left.start_ms
    ):
        return False
    if not left.source_native_ids or not right.source_native_ids:
        return True
    return bool(set(left.source_native_ids) & set(right.source_native_ids))


def _validate_rules(
    rules: Sequence[PromptAuthorshipRule], team_slugs: set[str]
) -> tuple[PromptAuthorshipRule, ...]:
    ordered = tuple(
        sorted(
            rules,
            key=lambda rule: (
                rule.team_slug,
                rule.ingress_kind,
                rule.start_ms if rule.start_ms is not None else -1,
                rule.end_ms if rule.end_ms is not None else 2**63,
                rule.rule_id,
            ),
        )
    )
    ids: set[str] = set()
    for rule in ordered:
        if rule.rule_id in ids:
            raise ValueError(f"duplicate prompt authorship rule {rule.rule_id!r}")
        ids.add(rule.rule_id)
        if rule.team_slug not in team_slugs:
            raise ValueError(
                f"prompt authorship rule {rule.rule_id!r} selects unknown team "
                f"{rule.team_slug!r}"
            )
    for index, left in enumerate(ordered):
        for right in ordered[index + 1 :]:
            if _rules_overlap(left, right):
                raise ValueError(
                    "overlapping prompt authorship rules "
                    f"{left.rule_id!r} and {right.rule_id!r}"
                )
    return ordered


def _rules_text(rules: Sequence[PromptAuthorshipRule]) -> str:
    values: list[JsonValue] = [rule.to_json_obj() for rule in rules]
    return canonical_json({"schema_version": 1, "rules": values})


def _load_rules(path: Path) -> tuple[PromptAuthorshipRule, ...]:
    if not path.exists():
        return ()
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"prompt authorship rules are not a regular file: {path}")
    root = as_object(read_json(path), str(path))
    if set(root) != {"schema_version", "rules"}:
        raise ValueError(f"invalid prompt authorship rules document at {path}")
    if as_int(root.get("schema_version"), f"{path}.schema_version") != 1:
        raise ValueError(f"unsupported prompt authorship rules schema at {path}")
    return tuple(
        _prompt_authorship_rule_from_json(value, f"{path}.rules[{index}]")
        for index, value in enumerate(as_array(root.get("rules"), f"{path}.rules"))
    )


def _apply_authorship_rules(
    records: Sequence[dict[str, JsonValue]],
    rules: Sequence[PromptAuthorshipRule],
) -> dict[str, int]:
    applied = {rule.rule_id: 0 for rule in rules}
    for record in records:
        if record.get("record_type") != "prompt":
            continue
        current_kind = record.get("source_author_kind", record.get("author_kind"))
        source_kind = current_kind if isinstance(current_kind, str) else None
        source_version = record.get(
            "source_classification_version", record.get("classification_version")
        )
        record["source_author_kind"] = source_kind
        record["source_classification_version"] = source_version
        record["author_kind"] = source_kind
        record["classification_version"] = source_version
        record.pop("authorship_rule_id", None)
        record.pop("authorship_rule_reason", None)
        if source_kind not in _UNCLASSIFIED_AUTHOR_KINDS:
            continue
        matches = [rule for rule in rules if rule.matches(record)]
        if len(matches) > 1:
            raise ValueError(
                "multiple prompt authorship rules matched record "
                f"{record.get('record_id')!r}"
            )
        if not matches:
            continue
        rule = matches[0]
        record["author_kind"] = rule.author_kind
        base_version = source_version if isinstance(source_version, str) else "unknown"
        record["classification_version"] = f"{base_version}+rule:{rule.rule_id}"
        record["authorship_rule_id"] = rule.rule_id
        record["authorship_rule_reason"] = rule.reason
        applied[rule.rule_id] += 1
    return applied


def _immutable_projection(record: dict[str, JsonValue]) -> dict[str, JsonValue]:
    mutable = {
        "ordinal",
        "ingress_kind",
        "author_kind",
        "classification_version",
        "source_author_kind",
        "source_classification_version",
        "authorship_rule_id",
        "authorship_rule_reason",
        "in_reply_to_prompt_id",
    }
    return {key: value for key, value in record.items() if key not in mutable}


def _source_occurrence_id(record: dict[str, JsonValue]) -> str:
    """Return the provider occurrence identity without its projected message class."""

    source_event_ids = tuple(
        as_string(value, "record.source_event_ids[]")
        for value in as_array(record.get("source_event_ids"), "record.source_event_ids")
    )
    native_id = record.get("source_native_id")
    return _stable_id(
        "source-occurrence",
        (
            as_string(record.get("team_slug"), "record.team_slug"),
            as_string(record.get("provider"), "record.provider"),
            as_string(record.get("thread_id"), "record.thread_id"),
            as_string(record.get("source_path"), "record.source_path"),
            native_id if isinstance(native_id, str) else "",
            str(as_int(record.get("source_line"), "record.source_line")),
            str(as_int(record.get("timestamp_ms"), "record.timestamp_ms")),
            as_string(record.get("content_sha256"), "record.content_sha256"),
            *source_event_ids,
        ),
    )


def _reclassification_projection(
    record: dict[str, JsonValue],
) -> dict[str, JsonValue]:
    """Return fields that must survive a prompt/system/response refinement."""

    projected = _immutable_projection(record)
    for key in ("record_type", "record_id", "logical_record_id"):
        projected.pop(key, None)
    return projected


def _monotonic_union(
    old: list[dict[str, JsonValue]], new: list[dict[str, JsonValue]]
) -> tuple[list[dict[str, JsonValue]], int, int]:
    merged: dict[str, dict[str, JsonValue]] = {}
    old_by_source: dict[str, list[dict[str, JsonValue]]] = {}
    for record in old:
        record_id = as_string(record.get("record_id"), "old record.record_id")
        if record_id in merged:
            raise ValueError(f"duplicate existing transcript record {record_id!r}")
        merged[record_id] = record
        old_by_source.setdefault(_source_occurrence_id(record), []).append(record)
    new_ids: set[str] = set()
    new_source_ids: set[str] = set()
    reclassified = 0
    for record in new:
        record_id = as_string(record.get("record_id"), "new record.record_id")
        if record_id in new_ids:
            raise ValueError(f"duplicate new transcript record {record_id!r}")
        new_ids.add(record_id)
        source_id = _source_occurrence_id(record)
        if source_id in new_source_ids:
            raise ValueError(
                f"duplicate new transcript source occurrence {source_id!r}"
            )
        new_source_ids.add(source_id)
        for previous_class in old_by_source.get(source_id, ()):
            previous_id = as_string(
                previous_class.get("record_id"), "old record.record_id"
            )
            if previous_id == record_id:
                continue
            if _reclassification_projection(
                previous_class
            ) != _reclassification_projection(record):
                raise ValueError(
                    "immutable transcript occurrence changed while its message class "
                    f"was refined for {source_id!r}"
                )
            del merged[previous_id]
            reclassified += 1
        previous = merged.get(record_id)
        if previous is not None and _immutable_projection(previous) != _immutable_projection(
            record
        ):
            raise ValueError(
                f"immutable transcript occurrence changed for {record_id!r}"
            )
        merged[record_id] = record
    carried = len(set(merged) - new_ids)
    return sorted(merged.values(), key=_record_sort_key), carried, reclassified


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
        attributed = [
            item
            for item in ordered
            if isinstance(item.get("author_kind"), str)
            and item.get("author_kind") not in _UNCLASSIFIED_AUTHOR_KINDS
        ]
        attributed_kinds = {
            item.get("author_kind") for item in attributed
        }
        if len(attributed_kinds) > 1:
            raise ValueError(
                f"conflicting authorship classifications for {logical_id!r}: "
                f"{sorted(str(value) for value in attributed_kinds)!r}"
            )
        representative = attributed[0] if attributed else ordered[0]
        record = dict(representative)
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
    rules_entry = files.get(_AUTHORSHIP_RULES_FILE)
    if rules_entry is not None:
        entry = as_object(
            rules_entry, f"{path}.files.{_AUTHORSHIP_RULES_FILE}"
        )
        expected = as_string(
            entry.get("sha256"),
            f"{path}.files.{_AUTHORSHIP_RULES_FILE}.sha256",
        )
        rules_path = root / _AUTHORSHIP_RULES_FILE
        if not rules_path.is_file() or rules_path.is_symlink():
            raise ValueError(
                f"transcript export file is missing or unsafe: {rules_path}"
            )
        actual = hashlib.sha256(rules_path.read_bytes()).hexdigest()
        if actual != expected:
            raise ValueError(
                "transcript export generation is incomplete: "
                f"{rules_path} digest mismatch"
            )
    return manifest


def export_transcripts(
    archive: Path,
    teams: Iterable[TeamData],
    prompt_authorship_rules: Sequence[PromptAuthorshipRule] | None = None,
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
    configured_rules = (
        tuple(prompt_authorship_rules)
        if prompt_authorship_rules is not None
        else _load_rules(root / _AUTHORSHIP_RULES_FILE)
    )
    rules = _validate_rules(
        configured_rules, {team.team_slug for team in ordered_teams}
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
    occurrences, carried, reclassified = _monotonic_union(
        _load_jsonl(root / "occurrences.jsonl"), current_occurrences
    )
    rule_counts = _apply_authorship_rules(occurrences, rules)
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
    rules_text = _rules_text(rules)
    changed = 0
    for name in _MANAGED_FILES:
        changed += int(write_text_if_changed(root / name, texts[name]))
    changed += int(
        write_text_if_changed(root / _AUTHORSHIP_RULES_FILE, rules_text)
    )

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
    file_manifest[_AUTHORSHIP_RULES_FILE] = {
        "sha256": _sha256_text(rules_text),
        "bytes": len(rules_text.encode("utf-8")),
        "records": len(rules),
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
    rule_count_values: dict[str, JsonValue] = {
        rule_id: count for rule_id, count in rule_counts.items()
    }
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
            "reclassified": reclassified,
            "prompt_authorship_rules": len(rules),
        },
        "prompt_authorship": {
            "rule_application_counts": rule_count_values,
            "unclassified_prompts": sum(
                1
                for prompt in prompts
                if prompt.get("author_kind") in _UNCLASSIFIED_AUTHOR_KINDS
            ),
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
        reclassified=reclassified,
    )


__all__ = [
    "PromptAuthorshipRule",
    "TRANSCRIPT_EXPORT_SCHEMA_VERSION",
    "TranscriptExportReport",
    "export_transcripts",
]
