#!/usr/bin/env python3
"""Read-only, machine-friendly navigation over a built timeline archive."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import re
import sys
from collections.abc import Iterable, Sequence
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path, PurePosixPath
from typing import Protocol

QUERY_SCHEMA_VERSION = 1
TIMELINE_SCHEMA_VERSION = 1
TRANSCRIPT_EXPORT_SCHEMA_VERSION = 1
_WHITESPACE = re.compile(r"\s+")
_TRIGRAM_BLOOM_ALGORITHM = "ascii-lower-utf8-trigram-fnv1a32-double-v1"
_TRIGRAM_BLOOM_FNV_OFFSET = 2_166_136_261
_TRIGRAM_BLOOM_FNV_PRIME = 16_777_619
_TRIGRAM_BLOOM_SECOND_SEED = 0x9E37_79B9
_UINT32_MASK = (1 << 32) - 1
_RFC3339_INSTANT = re.compile(
    r"^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}"
    r"(?:\.[0-9]{1,6})?(?:Z|[+-][0-9]{2}:[0-9]{2})$"
)
PROMPT_SELECTIONS = ("human", "bot", "all")
_HUMAN_AUTHOR_KINDS = frozenset(("owner_human", "other_human"))
_BOT_AUTHOR_KINDS = frozenset(("agent", "system"))
SEARCH_CORPORA = (
    "owner-prompts",
    "agent-responses",
    "all-transcript",
)
SEARCH_MATCH_MODES = ("smart", "phrase", "literal")
SEARCH_SORTS = ("relevance", "newest", "oldest")
SEARCH_PROMPT_AUTHORS = ("any", "owner", "agent", "unclassified")
SEARCH_LINKAGES = ("any", "linked", "unlinked")
SEARCH_ROLES = (
    "user",
    "assistant",
    "agent",
    "system",
    "external",
    "goal",
    "tool",
    "event",
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
    """Require one JSON object and retain its recursively narrowed type."""

    if not isinstance(value, dict):
        raise ValueError(f"{where}: expected an object")
    return value


def as_array(value: JsonValue, where: str) -> list[JsonValue]:
    """Require one JSON array and retain its recursively narrowed type."""

    if not isinstance(value, list):
        raise ValueError(f"{where}: expected an array")
    return value


def as_string(value: JsonValue, where: str) -> str:
    """Require one JSON string with a location-aware validation error."""

    if not isinstance(value, str):
        raise ValueError(f"{where}: expected a string")
    return value


def as_int(value: JsonValue, where: str) -> int:
    """Require one JSON integer while rejecting booleans."""

    if not isinstance(value, int) or isinstance(value, bool):
        raise ValueError(f"{where}: expected an integer")
    return value


def canonical_json(value: JsonValue) -> str:
    """Serialize a narrowed JSON value deterministically for CLI output."""

    return json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"


@dataclass(frozen=True)
class QueryFilters:
    """Provider-neutral filters shared by list and search operations."""

    teams: tuple[str, ...] = ()
    window: TimeWindow | None = None
    rollup_kinds: tuple[str, ...] = ()
    agent_ref: str | None = None


@dataclass(frozen=True)
class SearchResults:
    """One ranked search page plus complete result-set metadata."""

    items: tuple[dict[str, JsonValue], ...]
    total_matches: int
    offset: int
    limit: int
    corpus: str
    match_mode: str
    sort: str

    @property
    def truncated(self) -> bool:
        """Return whether matches exist outside this page."""

        return self.offset > 0 or self.offset + len(self.items) < self.total_matches


@dataclass(frozen=True)
class _TextMatch:
    compact: str
    ranges: tuple[tuple[int, int], ...]
    score: int


@dataclass(frozen=True)
class _SearchMatcher:
    compact_needle: str
    patterns: tuple[re.Pattern[str], ...]
    bloom_terms: tuple[str, ...]
    case_sensitive: bool

    def match(self, text: str) -> _TextMatch | None:
        """Return match ranges and a deterministic relevance score."""

        compact = _compact_text(text)
        collected: list[tuple[int, int]] = []
        for pattern in self.patterns:
            candidate = _pattern_ranges(pattern, compact)
            if not candidate:
                return None
            collected.extend(candidate)
        ranges = tuple(sorted(set(collected)))[:64]
        first = min(start for start, _ in ranges)
        last = max(end for _, end in ranges)
        span = max(1, last - first)
        comparable = compact if self.case_sensitive else compact.casefold()
        comparable_needle = (
            self.compact_needle
            if self.case_sensitive
            else self.compact_needle.casefold()
        )
        exact = comparable == comparable_needle
        score = (
            (100_000 if exact else 0)
            + max(0, 20_000 - span)
            + len(ranges) * 100
            - min(first, 10_000)
        )
        return _TextMatch(compact, ranges, score)


@dataclass(frozen=True)
class _IndexEntry:
    kind: str
    record: dict[str, JsonValue]


@dataclass(frozen=True)
class OrdinalRange:
    """One-based inclusive prompt ordinal bounds."""

    first: int
    last: int

    def contains(self, ordinal: int) -> bool:
        """Return whether *ordinal* lies inside this inclusive range."""

        return self.first <= ordinal <= self.last


@dataclass(frozen=True)
class TextTotals:
    """Additive counts over record text, excluding serialization framing."""

    records: int = 0
    words: int = 0
    utf8_bytes: int = 0

    @classmethod
    def from_texts(cls, texts: Iterable[str]) -> TextTotals:
        """Count records, whitespace-delimited words, and UTF-8 text bytes."""

        records = 0
        words = 0
        utf8_bytes = 0
        for text in texts:
            records += 1
            words += len(text.split())
            utf8_bytes += len(text.encode("utf-8"))
        return cls(records, words, utf8_bytes)

    def __add__(self, other: TextTotals) -> TextTotals:
        return TextTotals(
            self.records + other.records,
            self.words + other.words,
            self.utf8_bytes + other.utf8_bytes,
        )

    def to_mapping(self) -> dict[str, JsonValue]:
        """Return these totals as a JSON-compatible object."""

        return {
            "records": self.records,
            "words": self.words,
            "utf8_bytes": self.utf8_bytes,
        }


@dataclass(frozen=True)
class SummaryKindStats:
    """Availability and generated-text totals for one summary surface."""

    available: int
    unavailable: int
    content: TextTotals

    def to_mapping(self) -> dict[str, JsonValue]:
        """Return coverage and content totals as a JSON-compatible object."""

        return {
            "available": self.available,
            "unavailable": self.unavailable,
            "total": self.available + self.unavailable,
            "content": self.content.to_mapping(),
        }


def parse_ordinal_range(raw: str) -> OrdinalRange:
    """Parse ``N`` or inclusive ``N-M`` prompt ordinals."""

    match = re.fullmatch(r"([1-9][0-9]*)(?:-([1-9][0-9]*))?", raw)
    if match is None:
        raise ValueError("--range must be a positive ordinal or inclusive N-M range")
    first = int(match.group(1))
    last = int(match.group(2) or match.group(1))
    if first > last:
        raise ValueError("--range first ordinal must not exceed its last ordinal")
    return OrdinalRange(first, last)


def _read_jsonl(path: Path) -> tuple[dict[str, JsonValue], ...]:
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"transcript export file is missing or unsafe: {path}")
    result: list[dict[str, JsonValue]] = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line:
            continue
        raw: object = json.loads(line)
        result.append(
            as_object(_narrow_json(raw, f"{path}:{line_number}"), f"{path}:{line_number}")
        )
    return tuple(result)


class TranscriptQuery:
    """Validated read-only access to the zero-model transcript projection."""

    _FILES = (
        "occurrences.jsonl",
        "prompts.jsonl",
        "messages.jsonl",
        "system-inputs.jsonl",
    )

    def __init__(self, root: Path) -> None:
        self.root = root.resolve()
        self.transcript_root = self.root / "extracted" / "transcripts"
        manifest_path = self.transcript_root / "manifest.json"
        if manifest_path.is_symlink() or not manifest_path.is_file():
            raise ValueError(
                f"no extracted transcripts at {manifest_path}; run extract-transcripts"
            )
        manifest = as_object(read_json(manifest_path), str(manifest_path))
        schema = as_int(manifest.get("schema_version"), "transcript manifest schema")
        if schema != TRANSCRIPT_EXPORT_SCHEMA_VERSION:
            raise ValueError(
                f"unsupported transcript export schema version {schema}; "
                f"expected {TRANSCRIPT_EXPORT_SCHEMA_VERSION}"
            )
        files = as_object(manifest.get("files"), "transcript manifest files")
        for name in self._FILES:
            entry = as_object(files.get(name), f"transcript manifest files.{name}")
            expected = as_string(entry.get("sha256"), f"{name}.sha256")
            path = self.transcript_root / name
            if path.is_symlink() or not path.is_file():
                raise ValueError(f"transcript export file is missing or unsafe: {path}")
            actual = hashlib.sha256(path.read_bytes()).hexdigest()
            if actual != expected:
                raise ValueError(f"transcript export generation is incomplete: {name}")
        self.prompts = _read_jsonl(self.transcript_root / "prompts.jsonl")
        self.messages = _read_jsonl(self.transcript_root / "messages.jsonl")
        self._prompt_ordinals = {
            as_string(record.get("record_id"), "prompt.record_id"): as_int(
                record.get("ordinal"), "prompt.ordinal"
            )
            for record in self.prompts
        }

    @staticmethod
    def _selected(record: dict[str, JsonValue], filters: QueryFilters) -> bool:
        team = as_string(record.get("team_slug"), "transcript record.team_slug")
        if filters.teams and team not in filters.teams:
            return False
        if filters.window is None:
            return True
        at_ms = as_int(record.get("timestamp_ms"), "transcript record.timestamp_ms")
        return filters.window.contains(at_ms)

    @staticmethod
    def _prompt_class(record: dict[str, JsonValue]) -> str:
        author_kind = as_string(record.get("author_kind"), "prompt.author_kind")
        if author_kind in _HUMAN_AUTHOR_KINDS:
            return "human"
        if author_kind in _BOT_AUTHOR_KINDS:
            return "bot"
        return "unclassified"

    @classmethod
    def _selected_prompt_authorship(
        cls, record: dict[str, JsonValue], which: str
    ) -> bool:
        if which not in PROMPT_SELECTIONS:
            raise ValueError("prompt selection must be human, bot, or all")
        return which == "all" or cls._prompt_class(record) == which

    def list_prompts(
        self,
        filters: QueryFilters,
        ordinal_range: OrdinalRange | None,
        which: str = "human",
    ) -> list[dict[str, JsonValue]]:
        """Return verbatim authored prompt records in global timestamp order."""

        result: list[dict[str, JsonValue]] = []
        for record in self.prompts:
            ordinal = as_int(record.get("ordinal"), "prompt.ordinal")
            if ordinal_range is not None and not ordinal_range.contains(ordinal):
                continue
            if not self._selected_prompt_authorship(record, which):
                continue
            if self._selected(record, filters):
                result.append(dict(record))
        return result

    def list_messages(
        self,
        filters: QueryFilters,
        ordinal_range: OrdinalRange | None,
        which: str = "human",
    ) -> list[dict[str, JsonValue]]:
        """Return prompts plus mechanically associated coordinator responses."""

        selected_prompts = self.list_prompts(filters, ordinal_range, which)
        selected_prompt_ids = {
            as_string(record.get("record_id"), "prompt.record_id")
            for record in selected_prompts
        }
        result: list[dict[str, JsonValue]] = []
        for record in self.messages:
            record_type = as_string(record.get("record_type"), "message.record_type")
            if record_type == "prompt":
                record_id = as_string(record.get("record_id"), "prompt.record_id")
                if record_id not in selected_prompt_ids:
                    continue
                ordinal: int | None = as_int(record.get("ordinal"), "prompt.ordinal")
            elif record_type == "response":
                prompt_id = record.get("in_reply_to_prompt_id")
                if not isinstance(prompt_id, str) or prompt_id not in selected_prompt_ids:
                    continue
                ordinal = self._prompt_ordinals.get(prompt_id)
            else:
                raise ValueError(f"unknown transcript message type {record_type!r}")
            if self._selected(record, filters):
                item = dict(record)
                if record_type == "response":
                    item["prompt_ordinal"] = ordinal
                result.append(item)
        return result

    def content_stats(
        self, filters: QueryFilters
    ) -> tuple[TextTotals, TextTotals, TextTotals, TextTotals, TextTotals]:
        """Count mechanically attributed prompt classes and responses in *filters*."""

        human_prompt_texts: list[str] = []
        bot_prompt_texts: list[str] = []
        unattributed_prompt_texts: list[str] = []
        for record in self.prompts:
            if not self._selected(record, filters):
                continue
            text = as_string(record.get("text"), "prompt.text")
            prompt_class = self._prompt_class(record)
            if prompt_class == "human":
                human_prompt_texts.append(text)
            elif prompt_class == "bot":
                bot_prompt_texts.append(text)
            else:
                unattributed_prompt_texts.append(text)
        linked_response_texts: list[str] = []
        unlinked_response_texts: list[str] = []
        for record in self.messages:
            if as_string(record.get("record_type"), "message.record_type") != "response":
                continue
            if not self._selected(record, filters):
                continue
            text = as_string(record.get("text"), "response.text")
            if isinstance(record.get("in_reply_to_prompt_id"), str):
                linked_response_texts.append(text)
            else:
                unlinked_response_texts.append(text)
        return (
            TextTotals.from_texts(human_prompt_texts),
            TextTotals.from_texts(bot_prompt_texts),
            TextTotals.from_texts(unattributed_prompt_texts),
            TextTotals.from_texts(linked_response_texts),
            TextTotals.from_texts(unlinked_response_texts),
        )


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


def _summary_available(record: dict[str, JsonValue], key: str) -> bool:
    if key not in record:
        return True
    value = record[key]
    if not isinstance(value, bool):
        raise ValueError(f"{key}: expected a boolean")
    return value


def _compact_text(value: str) -> str:
    return _WHITESPACE.sub(" ", value).strip()


def _pattern_ranges(
    pattern: re.Pattern[str], compact: str
) -> tuple[tuple[int, int], ...]:
    result: list[tuple[int, int]] = []
    for match in pattern.finditer(compact):
        result.append((match.start(), match.end()))
        if len(result) == 64:
            break
    return tuple(result)


def _smart_parts(needle: str) -> tuple[tuple[str, bool], ...]:
    parts: list[tuple[str, bool]] = []
    for match in re.finditer(r'"([^"\\]*(?:\\.[^"\\]*)*)"|(\S+)', needle):
        quoted = match.group(1)
        if quoted is not None:
            value = quoted.replace(r'\"', '"').replace(r"\\", "\\")
            if value:
                parts.append((value, True))
        else:
            value = match.group(2)
            if value:
                parts.append((value, False))
    return tuple(parts)


def _compile_search_matcher(
    needle: str,
    *,
    match_mode: str,
    case_sensitive: bool,
) -> _SearchMatcher:
    compact_needle = _compact_text(needle)
    if not compact_needle:
        raise ValueError("search text must not be empty")
    flags = 0 if case_sensitive else re.IGNORECASE
    patterns: tuple[re.Pattern[str], ...]
    bloom_terms: tuple[str, ...]
    if match_mode in {"literal", "phrase"}:
        patterns = (re.compile(re.escape(compact_needle), flags),)
        bloom_terms = (compact_needle,)
    elif match_mode == "smart":
        parts = _smart_parts(compact_needle)
        if not parts:
            raise ValueError("search text must contain a term")
        compiled: list[re.Pattern[str]] = []
        for part, quoted in parts:
            if quoted or re.fullmatch(r"[\w-]+", part, flags=re.UNICODE) is None:
                compiled.append(re.compile(re.escape(part), flags))
            else:
                compiled.append(
                    re.compile(rf"(?<!\w){re.escape(part)}(?!\w)", flags)
                )
        patterns = tuple(compiled)
        bloom_terms = tuple(part for part, _ in parts)
    else:
        raise ValueError(f"unsupported search match mode {match_mode!r}")
    return _SearchMatcher(compact_needle, patterns, bloom_terms, case_sensitive)


def _ascii_lower_utf8(value: str) -> bytes:
    encoded = value.encode("utf-8")
    return bytes(byte + 32 if 65 <= byte <= 90 else byte for byte in encoded)


def _trigram_positions(
    trigram: bytes, bit_count: int, hash_count: int
) -> tuple[int, ...]:
    def digest(seed: int) -> int:
        result = seed
        for byte in trigram:
            result ^= byte
            result = (result * _TRIGRAM_BLOOM_FNV_PRIME) & _UINT32_MASK
        return result

    first = digest(_TRIGRAM_BLOOM_FNV_OFFSET)
    second = digest(_TRIGRAM_BLOOM_FNV_OFFSET ^ _TRIGRAM_BLOOM_SECOND_SEED) | 1
    mask = bit_count - 1
    return tuple((first + index * second) & mask for index in range(hash_count))


def _bloom_might_match(
    raw: JsonValue, terms: tuple[str, ...], where: str
) -> bool:
    """Validate one catalog Bloom filter and reject only definite misses."""

    if raw is None:
        return True
    value = as_object(raw, where)
    algorithm = as_string(value.get("algorithm"), where + ".algorithm")
    if algorithm != _TRIGRAM_BLOOM_ALGORITHM:
        raise ValueError(f"{where}.algorithm: unsupported value {algorithm!r}")
    bit_count = as_int(value.get("bit_count"), where + ".bit_count")
    if bit_count < 64 or bit_count & (bit_count - 1):
        raise ValueError(f"{where}.bit_count: expected a power of two of at least 64")
    hash_count = as_int(value.get("hash_count"), where + ".hash_count")
    if hash_count <= 0:
        raise ValueError(f"{where}.hash_count: expected a positive integer")
    trigram_count = as_int(value.get("trigram_count"), where + ".trigram_count")
    if trigram_count < 0:
        raise ValueError(f"{where}.trigram_count: expected a non-negative integer")
    encoded = as_string(value.get("bits_base64"), where + ".bits_base64")
    try:
        bits = base64.b64decode(encoded, validate=True)
    except ValueError as error:
        raise ValueError(f"{where}.bits_base64: invalid base64") from error
    if len(bits) * 8 != bit_count:
        raise ValueError(
            f"{where}.bits_base64: decoded {len(bits) * 8} bits, expected {bit_count}"
        )
    for term in terms:
        compact = _compact_text(term)
        if not compact.isascii():
            continue
        term_bytes = _ascii_lower_utf8(compact)
        if len(term_bytes) < 3:
            continue
        for offset in range(len(term_bytes) - 2):
            trigram = term_bytes[offset : offset + 3]
            for position in _trigram_positions(trigram, bit_count, hash_count):
                if not bits[position // 8] & (1 << (position % 8)):
                    return False
    return True


def _search_excerpt(match: _TextMatch) -> dict[str, JsonValue]:
    first = min(start for start, _ in match.ranges)
    start = max(0, first - 120)
    end = min(len(match.compact), start + 480)
    excerpt = match.compact[start:end]
    ranges: list[JsonValue] = []
    for left, right in match.ranges:
        if right <= start or left >= end:
            continue
        ranges.append([max(0, left - start), min(end, right) - start])
    return {
        "text": excerpt,
        "full_characters": len(match.compact),
        "leading_omitted_characters": start,
        "trailing_omitted_characters": len(match.compact) - end,
        "is_truncated": start > 0 or end < len(match.compact),
        "match_ranges": ranges,
    }


def _copy_fields(
    record: dict[str, JsonValue], keys: tuple[str, ...]
) -> dict[str, JsonValue]:
    return {key: record[key] for key in keys if key in record}


class TimelineQuery:
    """Validated, read-only view of a built single- or multi-team timeline."""

    def __init__(self, root: Path) -> None:
        self.root = root.resolve()
        self._timeline_v2_bootstrap: dict[str, JsonValue] | None = None
        bootstrap_path = self.root / "data" / "timeline-v2.json"
        if bootstrap_path.is_file():
            bootstrap = as_object(read_json(bootstrap_path), str(bootstrap_path))
            bootstrap_schema = as_int(
                bootstrap.get("schema_version"), "timeline-v2.schema_version"
            )
            predates_phase_index = (
                bootstrap_schema == 2
                and bootstrap.get("kind") == "timeline-bootstrap"
                and "phase_index" not in bootstrap
            )
            if predates_phase_index:
                self.timeline = self._load_schema_1_timeline()
            else:
                self.timeline, self._timeline_v2_bootstrap = (
                    self._load_schema_2_timeline(bootstrap_path, bootstrap)
                )
        else:
            if bootstrap_path.exists():
                raise ValueError(
                    f"timeline schema-2 bootstrap is not a regular file: {bootstrap_path}"
                )
            self.timeline = self._load_schema_1_timeline()
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
        phase_intervals: dict[tuple[str, str], list[tuple[int, int, str]]] = {}
        for index, phase in enumerate(self.phases):
            where = f"timeline.phases[{index}]"
            team = _team(phase, where)
            agent_id = _local_identifier(
                team, as_string(phase.get("agent_id"), where + ".agent_id")
            )
            start_ms, end_ms = _interval(phase, where)
            phase_intervals.setdefault((team, agent_id), []).append(
                (start_ms, end_ms, phase_ref(phase, where))
            )
        self._search_phase_intervals = {
            key: tuple(sorted(values)) for key, values in phase_intervals.items()
        }

    def _load_schema_1_timeline(self) -> dict[str, JsonValue]:
        timeline_path = self.root / "data" / "timeline.json"
        if not timeline_path.is_file():
            raise ValueError(f"no built timeline at {timeline_path}")
        return as_object(read_json(timeline_path), str(timeline_path))

    def _content_addressed_object(
        self, raw_reference: JsonValue, where: str
    ) -> tuple[dict[str, JsonValue], str]:
        reference = as_object(raw_reference, where)
        digest = as_string(reference.get("sha256"), where + ".sha256")
        if re.fullmatch(r"[0-9a-f]{64}", digest) is None:
            raise ValueError(f"{where}.sha256: expected a lowercase SHA-256 digest")
        relative = as_string(reference.get("url"), where + ".url")
        expected_relative = f"data/timeline-v2/objects/{digest}.json"
        if relative != expected_relative:
            raise ValueError(
                f"{where}.url: expected content-addressed path {expected_relative!r}"
            )
        path, _pure = self._safe_file(relative)
        encoded = path.read_bytes()
        if hashlib.sha256(encoded).hexdigest() != digest:
            raise ValueError(f"{where}: object digest mismatch for {relative}")
        raw_bytes = reference.get("bytes")
        if raw_bytes is not None and as_int(raw_bytes, where + ".bytes") != len(encoded):
            raise ValueError(f"{where}: object byte count mismatch for {relative}")
        try:
            decoded: object = json.loads(encoded.decode("utf-8"))
        except (UnicodeDecodeError, json.JSONDecodeError) as error:
            raise ValueError(f"{where}: invalid JSON object {relative}") from error
        return as_object(_narrow_json(decoded, relative), relative), relative

    @staticmethod
    def _schema_2_range(
        record: dict[str, JsonValue], where: str
    ) -> tuple[dict[str, JsonValue], int, int]:
        time_range = as_object(record.get("range"), where + ".range")
        start_ms = as_int(time_range.get("start_ms"), where + ".range.start_ms")
        end_ms = as_int(time_range.get("end_ms"), where + ".range.end_ms")
        if end_ms <= start_ms:
            raise ValueError(f"{where}.range: end must be after start")
        return time_range, start_ms, end_ms

    @staticmethod
    def _schema_2_teams(
        record: dict[str, JsonValue], where: str
    ) -> tuple[list[JsonValue], frozenset[str]]:
        raw_teams = as_array(record.get("teams"), where + ".teams")
        if not raw_teams:
            raise ValueError(f"{where}.teams: must contain at least one team")
        team_slugs: set[str] = set()
        for index, raw_team in enumerate(raw_teams):
            team = as_object(raw_team, f"{where}.teams[{index}]")
            slug = as_string(team.get("slug"), f"{where}.teams[{index}].slug")
            if not slug:
                raise ValueError(f"{where}.teams[{index}].slug: must not be empty")
            if slug in team_slugs:
                raise ValueError(f"{where}.teams: duplicate team slug {slug!r}")
            team_slugs.add(slug)
        return raw_teams, frozenset(team_slugs)

    def _load_schema_2_timeline(
        self,
        bootstrap_path: Path,
        bootstrap: dict[str, JsonValue],
    ) -> tuple[dict[str, JsonValue], dict[str, JsonValue]]:
        if (
            as_int(bootstrap.get("schema_version"), "timeline-v2.schema_version") != 2
            or bootstrap.get("kind") != "timeline-bootstrap"
        ):
            raise ValueError(f"unsupported timeline schema-2 bootstrap at {bootstrap_path}")
        source_digest = as_string(
            bootstrap.get("source_digest"), "timeline-v2.source_digest"
        )
        if not source_digest:
            raise ValueError("timeline-v2.source_digest: must not be empty")
        teams, team_slugs = self._schema_2_teams(bootstrap, "timeline-v2")
        time_range, timeline_start_ms, timeline_end_ms = self._schema_2_range(
            bootstrap, "timeline-v2"
        )

        global_record, global_relative = self._content_addressed_object(
            bootstrap.get("global"), "timeline-v2.global"
        )
        if (
            as_int(global_record.get("schema_version"), global_relative + ".schema_version")
            != 2
            or global_record.get("kind") != "timeline-global"
        ):
            raise ValueError(f"unsupported timeline global object: {global_relative}")
        global_digest = global_record.get("source_digest")
        if global_digest is not None and as_string(
            global_digest, global_relative + ".source_digest"
        ) != source_digest:
            raise ValueError("timeline global object belongs to a different source generation")
        if "range" in global_record:
            global_range, _global_start, _global_end = self._schema_2_range(
                global_record, global_relative
            )
            if global_range != time_range:
                raise ValueError("timeline global range does not match bootstrap")
        if "teams" in global_record:
            _global_teams, global_team_slugs = self._schema_2_teams(
                global_record, global_relative
            )
            if global_team_slugs != team_slugs:
                raise ValueError("timeline global team set does not match bootstrap")

        agents = tuple(
            as_object(raw, f"{global_relative}.agents[{index}]")
            for index, raw in enumerate(
                as_array(global_record.get("agents"), global_relative + ".agents")
            )
        )
        agent_teams: dict[str, str] = {}
        for index, agent in enumerate(agents):
            where = f"{global_relative}.agents[{index}]"
            identifier = as_string(agent.get("id"), where + ".id")
            team = _team(agent, where)
            if team not in team_slugs:
                raise ValueError(f"{where}.team: unknown team {team!r}")
            agent_start_ms, agent_end_ms = _interval(agent, where)
            if (
                agent_end_ms <= agent_start_ms
                or agent_start_ms < timeline_start_ms
                or agent_end_ms > timeline_end_ms
            ):
                raise ValueError(f"{where}: agent interval is outside timeline range")
            if identifier in agent_teams:
                raise ValueError(
                    f"{global_relative}.agents: duplicate exact agent id {identifier!r}"
                )
            agent_teams[identifier] = team
        for key in ("rollups", "project_overviews"):
            raw_values = global_record.get(key)
            if raw_values is None:
                continue
            for index, raw in enumerate(as_array(raw_values, f"{global_relative}.{key}")):
                record = as_object(raw, f"{global_relative}.{key}[{index}]")
                team = _team(record, f"{global_relative}.{key}[{index}]")
                if team not in team_slugs:
                    raise ValueError(
                        f"{global_relative}.{key}[{index}].team: unknown team {team!r}"
                    )

        phase_index, phase_relative = self._content_addressed_object(
            bootstrap.get("phase_index"), "timeline-v2.phase_index"
        )
        if (
            as_int(phase_index.get("schema_version"), phase_relative + ".schema_version")
            != 2
            or phase_index.get("kind") != "timeline-phase-index"
        ):
            raise ValueError(f"unsupported timeline phase index: {phase_relative}")
        phase_digest = phase_index.get("source_digest")
        if phase_digest is not None and as_string(
            phase_digest, phase_relative + ".source_digest"
        ) != source_digest:
            raise ValueError("timeline phase index belongs to a different source generation")
        phases: list[JsonValue] = []
        seen_phase_ids: set[str] = set()
        for index, raw_phase in enumerate(
            as_array(phase_index.get("phases"), phase_relative + ".phases")
        ):
            where = f"{phase_relative}.phases[{index}]"
            phase = dict(as_object(raw_phase, where))
            phase_id = as_string(phase.get("id"), where + ".id")
            if phase_id in seen_phase_ids:
                raise ValueError(f"{phase_relative}.phases: duplicate id {phase_id!r}")
            seen_phase_ids.add(phase_id)
            agent_id = as_string(phase.get("agent_id"), where + ".agent_id")
            phase_start_ms, phase_end_ms = _interval(phase, where)
            if (
                phase_end_ms <= phase_start_ms
                or phase_start_ms < timeline_start_ms
                or phase_end_ms > timeline_end_ms
            ):
                raise ValueError(f"{where}: phase interval is outside timeline range")
            phase_team = agent_teams.get(agent_id)
            if phase_team is None:
                raise ValueError(
                    f"{where}.agent_id: no exact global agent match for {agent_id!r}"
                )
            existing_team = phase.get("team")
            if existing_team is not None and as_string(
                existing_team, where + ".team"
            ) != phase_team:
                raise ValueError(f"{where}.team: does not match phase agent")
            phase["team"] = phase_team
            phases.append(phase)

        for index, raw_bin in enumerate(
            as_array(bootstrap.get("activity_bins"), "timeline-v2.activity_bins")
        ):
            activity_bin = as_object(raw_bin, f"timeline-v2.activity_bins[{index}]")
            team = _team(activity_bin, f"timeline-v2.activity_bins[{index}]")
            if team not in team_slugs:
                raise ValueError(
                    f"timeline-v2.activity_bins[{index}].team: unknown team {team!r}"
                )

        timeline = {
            key: value
            for key, value in global_record.items()
            if key
            not in {
                "schema_version",
                "kind",
                "source_digest",
                "range",
                "teams",
            }
        }
        timeline.update(
            {
                "schema_version": TIMELINE_SCHEMA_VERSION,
                "generated_at": as_string(
                    bootstrap.get("generated_at"), "timeline-v2.generated_at"
                ),
                "source_digest": source_digest,
                "display_timezone": as_string(
                    bootstrap.get("display_timezone"), "timeline-v2.display_timezone"
                ),
                "display_timezone_source": as_string(
                    bootstrap.get(
                        "display_timezone_source", "legacy_team_data"
                    ),
                    "timeline-v2.display_timezone_source",
                ),
                "range": time_range,
                "teams": teams,
                "activity_bins": bootstrap.get("activity_bins"),
                "phases": phases,
                "events": [],
            }
        )
        return timeline, bootstrap

    def _project_overviews(self) -> tuple[dict[str, JsonValue], ...]:
        plural = self.timeline.get("project_overviews")
        if plural is not None:
            return tuple(
                as_object(value, f"timeline.project_overviews[{index}]")
                for index, value in enumerate(
                    as_array(plural, "timeline.project_overviews")
                )
            )
        singular = self.timeline.get("project_overview")
        if singular is None:
            return ()
        if len(self.teams) != 1:
            raise ValueError(
                "timeline.project_overview requires exactly one timeline team"
            )
        result = dict(as_object(singular, "timeline.project_overview"))
        result["team"] = as_string(self.teams[0].get("slug"), "timeline.teams[0].slug")
        return (result,)

    @staticmethod
    def _summary_kind_stats(texts: list[str], unavailable: int) -> SummaryKindStats:
        return SummaryKindStats(
            available=len(texts),
            unavailable=unavailable,
            content=TextTotals.from_texts(texts),
        )

    def summary_stats(
        self, filters: QueryFilters
    ) -> dict[str, SummaryKindStats]:
        """Count available and unavailable summaries in the presentation projection."""

        project_texts: list[str] = []
        project_unavailable = 0
        # Project overviews describe a whole team and have no honest time interval. Exclude
        # them from time-sliced statistics instead of pretending they belong to every window.
        if filters.window is None:
            for record in self._project_overviews():
                team = _team(record, "project_overview")
                if not self._selected_team(team, filters):
                    continue
                if _summary_available(record, "summary_available"):
                    project_texts.append(
                        as_string(record.get("text"), "project_overview.text")
                    )
                else:
                    project_unavailable += 1

        agent_texts: list[str] = []
        agent_unavailable = 0
        for record in self.agents:
            team = _team(record, "agent")
            if not self._selected_team(team, filters):
                continue
            if not _overlaps(record, agent_ref(record), filters.window):
                continue
            if _summary_available(record, "summary_available"):
                agent_texts.append(
                    as_string(record.get("lifetime_summary"), "agent.lifetime_summary")
                )
            else:
                agent_unavailable += 1

        phase_texts: list[str] = []
        phase_unavailable = 0
        for record in self.phases:
            team = _team(record, "phase")
            if not self._selected_team(team, filters):
                continue
            if not _overlaps(record, phase_ref(record), filters.window):
                continue
            if _summary_available(record, "summary_available"):
                phrase = as_string(record.get("phrase"), "phase.phrase")
                paragraph = as_string(record.get("paragraph"), "phase.paragraph")
                phase_texts.append(f"{phrase}\n{paragraph}")
            else:
                phase_unavailable += 1

        technical_texts: list[str] = []
        technical_unavailable = 0
        plain_texts: list[str] = []
        plain_unavailable = 0
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
            if _summary_available(record, "technical_summary_available"):
                relative = as_string(
                    record.get("technical_path"), "rollup.technical_path"
                )
                technical_texts.append(
                    self._rollup_file(relative, team).read_text(encoding="utf-8")
                )
            else:
                technical_unavailable += 1
            if _summary_available(record, "plain_language_summary_available"):
                relative = as_string(
                    record.get("plain_language_path"),
                    "rollup.plain_language_path",
                )
                plain_texts.append(
                    self._rollup_file(relative, team).read_text(encoding="utf-8")
                )
            else:
                plain_unavailable += 1

        return {
            "project_overviews": self._summary_kind_stats(
                project_texts, project_unavailable
            ),
            "agent_lifetimes": self._summary_kind_stats(
                agent_texts, agent_unavailable
            ),
            "work_phases": self._summary_kind_stats(phase_texts, phase_unavailable),
            "rollup_technical": self._summary_kind_stats(
                technical_texts, technical_unavailable
            ),
            "rollup_plain_language": self._summary_kind_stats(
                plain_texts, plain_unavailable
            ),
        }

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
                    "summary_available",
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
                    "summary_available",
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
                    "technical_summary_available",
                    "plain_language_summary_available",
                    "summary_available",
                    "stats",
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

    def _iter_search_records(
        self,
        filters: QueryFilters | None = None,
        *,
        bloom_terms: tuple[str, ...] = (),
    ) -> Iterable[dict[str, JsonValue]]:
        """Yield validated records while retaining at most one decoded day shard."""

        bootstrap = self._timeline_v2_bootstrap
        if bootstrap is None:
            raise ValueError(
                "this archive has no transcript search corpus; rebuild the website"
            )
        timeline_teams = sorted(
            as_string(team.get("slug"), "timeline team.slug") for team in self.teams
        )
        raw_search = bootstrap.get("search")
        if raw_search is None:
            raise ValueError(
                "this archive has no transcript search corpus; rebuild the website"
            )
        search = as_object(raw_search, "timeline-v2.search")
        if search.get("strategy") != "transcript-message-shards":
            raise ValueError(
                "this archive predates transcript search; rebuild the website"
            )
        if as_int(search.get("schema_version"), "timeline-v2.search.schema_version") != 1:
            raise ValueError("unsupported transcript search schema")
        source_digest = as_string(
            bootstrap.get("source_digest"), "timeline-v2.source_digest"
        )
        seen: set[str] = set()
        for shard_index, raw_shard in enumerate(
            as_array(search.get("shards"), "timeline-v2.search.shards")
        ):
            where = f"timeline-v2.search.shards[{shard_index}]"
            shard = as_object(raw_shard, where)
            shard_team = as_string(shard.get("team"), where + ".team")
            shard_start = as_int(shard.get("start_ms"), where + ".start_ms")
            shard_end = as_int(shard.get("end_ms"), where + ".end_ms")
            if shard_team not in timeline_teams or shard_end <= shard_start:
                raise ValueError(f"invalid transcript search shard scope at {where}")
            if filters is not None:
                if filters.teams and shard_team not in filters.teams:
                    continue
                if filters.window is not None and not filters.window.overlaps(
                    shard_start, shard_end
                ):
                    continue
            if bloom_terms and not _bloom_might_match(
                shard.get("trigram_bloom"),
                bloom_terms,
                where + ".trigram_bloom",
            ):
                continue
            root, relative = self._content_addressed_object(shard, where)
            if root.get("schema_version") != 1 or root.get("kind") != "timeline-search-day":
                raise ValueError(f"unsupported transcript search shard: {relative}")
            shard_digest = root.get("source_digest")
            if shard_digest is not None and as_string(
                shard_digest, relative + ".source_digest"
            ) != source_digest:
                raise ValueError(
                    f"transcript search shard belongs to a different source generation: {relative}"
                )
            if as_string(root.get("team"), relative + ".team") != shard_team:
                raise ValueError(f"transcript search shard team mismatch: {relative}")
            root_range = as_object(root.get("range"), relative + ".range")
            if root_range != {"start_ms": shard_start, "end_ms": shard_end}:
                raise ValueError(f"transcript search shard range mismatch: {relative}")
            raw_records = as_array(root.get("records"), relative + ".records")
            catalog_counts = as_object(shard.get("counts"), where + ".counts")
            if as_int(catalog_counts.get("records"), where + ".counts.records") != len(
                raw_records
            ):
                raise ValueError(f"transcript search shard count mismatch: {relative}")
            for record_index, raw_record in enumerate(
                raw_records
            ):
                record = as_object(
                    raw_record, f"{relative}.records[{record_index}]"
                )
                reference = as_string(
                    record.get("ref"), f"{relative}.records[{record_index}].ref"
                )
                if as_int(
                    record.get("schema_version"),
                    f"{relative}.records[{record_index}].schema_version",
                ) != 1:
                    raise ValueError(f"unsupported transcript search record: {reference}")
                record_team = as_string(
                    record.get("team"), f"{relative}.records[{record_index}].team"
                )
                at_ms = as_int(
                    record.get("at_ms"), f"{relative}.records[{record_index}].at_ms"
                )
                if record_team != shard_team or not shard_start <= at_ms < shard_end:
                    raise ValueError(f"transcript search record escapes shard: {reference}")
                message_prefix = f"message:{record_team}::"
                tool_prefix = f"tool:{record_team}::"
                if not reference.startswith((message_prefix, tool_prefix)):
                    raise ValueError(f"invalid transcript search reference {reference!r}")
                record_type = as_string(
                    record.get("record_type"),
                    f"{relative}.records[{record_index}].record_type",
                )
                if (record_type == "tool") != reference.startswith(tool_prefix):
                    raise ValueError(
                        f"transcript search reference kind mismatch: {reference}"
                    )
                agent_reference = as_string(
                    record.get("agent_ref"),
                    f"{relative}.records[{record_index}].agent_ref",
                )
                agent_identifier = as_string(
                    record.get("agent_id"),
                    f"{relative}.records[{record_index}].agent_id",
                )
                if (
                    self._agent_reference_for_id(record_team, agent_identifier)
                    != agent_reference
                ):
                    raise ValueError(
                        f"transcript search record agent identity mismatch: {reference}"
                    )
                agent_entry = self._entries.get(agent_reference)
                if agent_entry is None or agent_entry.kind != "agent":
                    raise ValueError(
                        f"transcript search record has unknown agent {agent_reference!r}"
                    )
                if _team(agent_entry.record, "search record agent") != record_team:
                    raise ValueError(
                        f"transcript search record agent belongs to another team: {reference}"
                    )
                prompt_reference = record.get("prompt_ref")
                if prompt_reference is not None and not as_string(
                    prompt_reference,
                    f"{relative}.records[{record_index}].prompt_ref",
                ).startswith(message_prefix):
                    raise ValueError(
                        f"transcript search prompt reference belongs to another team: {reference}"
                    )
                role = as_string(
                    record.get("role"), f"{relative}.records[{record_index}].role"
                )
                if role not in SEARCH_ROLES:
                    raise ValueError(f"unsupported transcript search role {role!r}")
                as_string(
                    record.get("text"), f"{relative}.records[{record_index}].text"
                )
                if reference in seen:
                    raise ValueError(f"duplicate transcript search reference {reference!r}")
                seen.add(reference)
                yield record

    def _phase_reference_for_search_record(
        self, record: dict[str, JsonValue]
    ) -> str | None:
        team = as_string(record.get("team"), "search record.team")
        agent_id = as_string(record.get("agent_id"), "search record.agent_id")
        at_ms = as_int(record.get("at_ms"), "search record.at_ms")
        candidates: list[tuple[int, str]] = []
        for start_ms, end_ms, reference in self._search_phase_intervals.get(
            (team, _local_identifier(team, agent_id)), ()
        ):
            if start_ms <= at_ms < end_ms:
                candidates.append((end_ms - start_ms, reference))
        return min(candidates)[1] if candidates else None

    def _show_search_record(self, reference: str) -> dict[str, JsonValue]:
        _, separator, remainder = reference.partition(":")
        team, team_separator, _ = remainder.partition("::")
        if not separator or not team_separator or not team:
            raise ValueError(f"invalid transcript search reference {reference!r}")
        record: dict[str, JsonValue] | None = None
        prompts: dict[str, dict[str, JsonValue]] = {}
        responses: list[dict[str, JsonValue]] = []
        for candidate in self._iter_search_records(QueryFilters(teams=(team,))):
            candidate_reference = as_string(
                candidate.get("ref"), "search record.ref"
            )
            candidate_type = as_string(
                candidate.get("record_type"), "search record.record_type"
            )
            if candidate_type in {"prompt", "inter_agent_prompt"}:
                prompts[candidate_reference] = dict(candidate)
            if candidate_reference == reference:
                record = dict(candidate)
            if (
                _optional_string(candidate, "prompt_ref") == reference
                and candidate_reference != reference
                and candidate_type in {"response", "inter_agent_response"}
            ):
                responses.append(dict(candidate))
        if record is None:
            raise ValueError(f"unknown stable reference {reference!r}")
        result = dict(record)
        result["at_time"] = _timestamp(
            as_int(record.get("at_ms"), "search record.at_ms")
        )
        phase_reference = self._phase_reference_for_search_record(record)
        if phase_reference is not None:
            result["phase_ref"] = phase_reference
        prompt_reference = _optional_string(record, "prompt_ref")
        if prompt_reference is not None and prompt_reference != reference:
            prompt = prompts.get(prompt_reference)
            result["linked_prompt"] = dict(prompt) if prompt is not None else None
        if record.get("record_type") in {"prompt", "inter_agent_prompt"}:
            responses.sort(
                key=lambda candidate: (
                    as_int(candidate.get("at_ms"), "search response.at_ms"),
                    as_string(candidate.get("ref"), "search response.ref"),
                )
            )
            linked_responses: list[JsonValue] = []
            linked_responses.extend(responses)
            result["linked_responses"] = linked_responses
        return result

    def show(self, reference: str, *, transcript: bool = False) -> dict[str, JsonValue]:
        """Resolve one stable reference and include useful relationship links."""

        entry = self._entries.get(reference)
        if entry is None:
            if reference.startswith(("message:", "tool:")):
                return self._show_search_record(reference)
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
        team = _team(record, "rollup")
        if _summary_available(record, "technical_summary_available"):
            technical_path = as_string(
                record.get("technical_path"), "rollup.technical_path"
            )
            result["technical_markdown"] = self._rollup_file(
                technical_path, team
            ).read_text(encoding="utf-8")
        if _summary_available(record, "plain_language_summary_available"):
            plain_path = as_string(
                record.get("plain_language_path"), "rollup.plain_language_path"
            )
            result["plain_language_markdown"] = self._rollup_file(
                plain_path, team
            ).read_text(encoding="utf-8")
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

    def search_v2(
        self,
        needle: str,
        *,
        corpus: str,
        filters: QueryFilters,
        case_sensitive: bool,
        match_mode: str,
        sort: str,
        prompt_author: str,
        linkage: str,
        roles: tuple[str, ...],
        offset: int,
        limit: int,
    ) -> SearchResults:
        """Search the canonical phase-independent transcript corpus."""

        if corpus not in SEARCH_CORPORA:
            raise ValueError(f"unsupported search corpus {corpus!r}")
        if match_mode not in SEARCH_MATCH_MODES:
            raise ValueError(f"unsupported search match mode {match_mode!r}")
        if sort not in SEARCH_SORTS:
            raise ValueError(f"unsupported search sort {sort!r}")
        if prompt_author not in SEARCH_PROMPT_AUTHORS:
            raise ValueError(f"unsupported prompt author filter {prompt_author!r}")
        if linkage not in SEARCH_LINKAGES:
            raise ValueError(f"unsupported linkage filter {linkage!r}")
        if any(role not in SEARCH_ROLES for role in roles):
            raise ValueError("unsupported transcript role filter")
        if offset < 0:
            raise ValueError("--offset must not be negative")
        if limit < 1:
            raise ValueError("--limit must be at least 1")
        selected_agent = self._validated_agent_filter(filters)
        matcher = _compile_search_matcher(
            needle,
            match_mode=match_mode,
            case_sensitive=case_sensitive,
        )
        prompt_texts: dict[str, str] = {}
        response_counts: dict[str, int] = {}
        candidates: list[
            tuple[int, int, str, dict[str, JsonValue], _TextMatch]
        ] = []
        for record in self._iter_search_records(
            filters, bloom_terms=matcher.bloom_terms
        ):
            prompt_reference = _optional_string(record, "prompt_ref")
            reference = as_string(record.get("ref"), "search record.ref")
            record_type = as_string(
                record.get("record_type"), "search record.record_type"
            )
            text = as_string(record.get("text"), "search record.text")
            if record_type in {"prompt", "inter_agent_prompt"}:
                prompt_texts[reference] = text
            team = as_string(record.get("team"), "search record.team")
            if not self._selected_team(team, filters):
                continue
            at_ms = as_int(record.get("at_ms"), "search record.at_ms")
            if filters.window is not None and not filters.window.contains(at_ms):
                continue
            agent_reference = as_string(
                record.get("agent_ref"), "search record.agent_ref"
            )
            if selected_agent is not None and agent_reference != selected_agent:
                continue
            if (
                record_type in {"response", "inter_agent_response"}
                and prompt_reference is not None
                and prompt_reference != reference
            ):
                response_counts[prompt_reference] = (
                    response_counts.get(prompt_reference, 0) + 1
                )
            role = as_string(record.get("role"), "search record.role")
            if roles and role not in roles:
                continue
            author_kind = _optional_string(record, "author_kind")
            prompt_kind = _optional_string(record, "prompt_author_kind")
            if corpus == "owner-prompts" and not (
                record_type == "prompt" and author_kind == "owner_human"
            ):
                continue
            if corpus == "agent-responses" and record_type not in {
                "response",
                "inter_agent_response",
            }:
                continue
            selected_prompt_kind = (
                author_kind
                if record_type in {"prompt", "inter_agent_prompt"}
                else prompt_kind
            )
            if prompt_author == "owner" and selected_prompt_kind != "owner_human":
                continue
            if prompt_author == "agent" and selected_prompt_kind not in _BOT_AUTHOR_KINDS:
                continue
            if prompt_author == "unclassified" and selected_prompt_kind in (
                _HUMAN_AUTHOR_KINDS | _BOT_AUTHOR_KINDS
            ):
                continue
            text_match = matcher.match(text)
            if text_match is None:
                continue
            candidates.append(
                (text_match.score, at_ms, reference, dict(record), text_match)
            )

        matches: list[tuple[int, int, str, dict[str, JsonValue]]] = []
        for score, at_ms, reference, record, text_match in candidates:
            record_type = as_string(
                record.get("record_type"), "search record.record_type"
            )
            prompt_reference = _optional_string(record, "prompt_ref")
            linked = (
                response_counts.get(reference, 0) > 0
                if record_type in {"prompt", "inter_agent_prompt"}
                else prompt_reference is not None and prompt_reference != reference
            )
            if linkage == "linked" and not linked:
                continue
            if linkage == "unlinked" and linked:
                continue
            team = as_string(record.get("team"), "search record.team")
            role = as_string(record.get("role"), "search record.role")
            agent_reference = as_string(
                record.get("agent_ref"), "search record.agent_ref"
            )
            prompt_kind = _optional_string(record, "prompt_author_kind")
            excerpt_details = _search_excerpt(text_match)
            excerpt = as_string(
                excerpt_details.get("text"), "search excerpt.text"
            )
            item: dict[str, JsonValue] = {
                "result_id": reference,
                "ref": reference,
                "record_type": record_type,
                "role": role,
                "team": team,
                "agent_ref": agent_reference,
                "agent_path": record.get("agent_path"),
                "at_ms": at_ms,
                "at_time": _timestamp(at_ms),
                "prompt_ref": prompt_reference,
                "prompt_at_ms": record.get("prompt_at_ms"),
                "prompt_author_kind": prompt_kind,
                "linked_response_count": response_counts.get(reference, 0),
                "score": score,
                "ranking_version": "search-rank-v1",
                "content_fidelity": record.get("content_fidelity"),
                "excerpt": excerpt,
                "excerpt_details": excerpt_details,
            }
            phase_reference = self._phase_reference_for_search_record(record)
            if phase_reference is not None:
                item["phase_ref"] = phase_reference
            if prompt_reference is not None and prompt_reference != reference:
                prompt_text = prompt_texts.get(prompt_reference)
                if prompt_text is not None:
                    item["prompt_excerpt"] = _compact_text(prompt_text)[:320]
            matches.append((score, at_ms, reference, item))
        if sort == "relevance":
            matches.sort(key=lambda item: (-item[0], -item[1], item[2]))
        elif sort == "newest":
            matches.sort(key=lambda item: (-item[1], -item[0], item[2]))
        else:
            matches.sort(key=lambda item: (item[1], -item[0], item[2]))
        total_matches = len(matches)
        page = tuple(item for _, _, _, item in matches[offset : offset + limit])
        return SearchResults(
            page,
            total_matches,
            offset,
            limit,
            corpus,
            match_mode,
            sort,
        )

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
            fields = [
                "short_name",
                "official_name",
                "nickname",
            ]
            if _summary_available(record, "summary_available"):
                fields.extend(("lifetime_summary", "naming_rationale"))
            for field in fields:
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
            if not _summary_available(record, "summary_available"):
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
            fields: list[tuple[str, str]] = []
            if _summary_available(record, "technical_summary_available"):
                fields.append(
                    (
                        "technical_markdown",
                        as_string(
                            record.get("technical_path"),
                            "rollup.technical_path",
                        ),
                    )
                )
            if _summary_available(record, "plain_language_summary_available"):
                fields.append(
                    (
                        "plain_language_markdown",
                        as_string(
                            record.get("plain_language_path"),
                            "rollup.plain_language_path",
                        ),
                    )
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


@dataclass(frozen=True)
class ArchiveStats:
    """Content and summary-coverage totals for one filtered archive view."""

    human_prompts: TextTotals
    bot_prompts: TextTotals
    unattributed_prompts: TextTotals
    linked_responses: TextTotals
    unlinked_responses: TextTotals
    summary_kinds: dict[str, SummaryKindStats]
    teams: tuple[str, ...]
    time_filtered: bool
    rollup_kinds: tuple[str, ...]

    @property
    def total_responses(self) -> TextTotals:
        """Return linked and unlinked response totals."""

        return self.linked_responses + self.unlinked_responses

    @property
    def all_prompts(self) -> TextTotals:
        """Return human, bot, and unattributed prompt totals."""

        return self.human_prompts + self.bot_prompts + self.unattributed_prompts

    @property
    def generated_summaries(self) -> TextTotals:
        """Return content totals across every summary surface."""

        total = TextTotals()
        for item in self.summary_kinds.values():
            total += item.content
        return total

    @property
    def prompts_and_responses(self) -> TextTotals:
        """Return all prompt and response content totals."""

        return self.all_prompts + self.total_responses

    @property
    def all_counted_content(self) -> TextTotals:
        """Return prompts, responses, and generated summaries together."""

        return self.prompts_and_responses + self.generated_summaries

    def to_mapping(self) -> dict[str, JsonValue]:
        """Return the full accounting report as a JSON-compatible object."""

        content: dict[str, JsonValue] = {
            "human_prompts": self.human_prompts.to_mapping(),
            "bot_prompts": self.bot_prompts.to_mapping(),
            "unattributed_prompts": self.unattributed_prompts.to_mapping(),
            "all_prompts": self.all_prompts.to_mapping(),
            "mechanically_linked_responses": self.linked_responses.to_mapping(),
            "unlinked_responses": self.unlinked_responses.to_mapping(),
            "total_responses": self.total_responses.to_mapping(),
            "generated_summaries": self.generated_summaries.to_mapping(),
            "prompts_and_responses": self.prompts_and_responses.to_mapping(),
            "all_counted_content": self.all_counted_content.to_mapping(),
        }
        by_kind: dict[str, JsonValue] = {
            key: value.to_mapping() for key, value in self.summary_kinds.items()
        }
        available = sum(item.available for item in self.summary_kinds.values())
        unavailable = sum(item.unavailable for item in self.summary_kinds.values())
        scope: dict[str, JsonValue] = {
            "teams": list(self.teams),
            "all_teams": not self.teams,
            "time_filtered": self.time_filtered,
            "rollup_kinds": list(self.rollup_kinds),
        }
        return {
            "schema_version": QUERY_SCHEMA_VERSION,
            "command": "stats",
            "scope": scope,
            "content": content,
            "summary_availability": {
                "available": available,
                "unavailable": unavailable,
                "total": available + unavailable,
                "by_kind": by_kind,
            },
            "counting_contract": {
                "words": "Unicode text split on whitespace",
                "utf8_bytes": "UTF-8 encoded text only; serialization framing excluded",
                "rollup_text": "referenced generated Markdown counted verbatim",
                "phase_summary_text": "phrase plus newline plus paragraph",
                "time_unbounded_project_overviews": (
                    "included only when no time filter is active"
                ),
                "human_prompts": "records labeled owner_human or other_human",
                "bot_prompts": "records labeled agent or system",
                "unattributed_prompts": "all remaining prompt authorship labels",
            },
        }


def archive_stats(root: Path, filters: QueryFilters) -> ArchiveStats:
    """Read verified projections and compute zero-model archive content totals."""

    transcripts = TranscriptQuery(root)
    (
        human_prompts,
        bot_prompts,
        unattributed_prompts,
        linked_responses,
        unlinked_responses,
    ) = transcripts.content_stats(filters)
    summaries = TimelineQuery(root).summary_stats(filters)
    return ArchiveStats(
        human_prompts=human_prompts,
        bot_prompts=bot_prompts,
        unattributed_prompts=unattributed_prompts,
        linked_responses=linked_responses,
        unlinked_responses=unlinked_responses,
        summary_kinds=summaries,
        teams=filters.teams,
        time_filtered=filters.window is not None,
        rollup_kinds=filters.rollup_kinds,
    )


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
        "summary_available",
        "technical_summary_available",
        "plain_language_summary_available",
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


def _text_item(item: dict[str, JsonValue]) -> str:
    record_type = _optional_string(item, "record_type") or "record"
    ordinal = _optional_int(item, "ordinal")
    if ordinal is None:
        ordinal = _optional_int(item, "prompt_ordinal")
    timestamp = (
        _optional_string(item, "timestamp_local")
        or _optional_string(item, "at_time")
        or _optional_string(item, "start_time")
        or "unknown time"
    )
    team = (
        _optional_string(item, "team_slug")
        or _optional_string(item, "team")
        or "unknown team"
    )
    number = f" #{ordinal}" if ordinal is not None else ""
    header = f"[{record_type}{number} · {timestamp} · {team}]"
    body = (
        _optional_string(item, "text")
        or _optional_string(item, "excerpt")
        or _optional_string(item, "paragraph")
        or canonical_json(item).rstrip()
    )
    return f"{header}\n{body.rstrip()}"


def format_query(
    command: str, items: list[dict[str, JsonValue]], output_format: str
) -> str:
    """Render query records as JSON, JSONL, Markdown, or scan-friendly plain text."""

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
    if output_format == "text":
        body = "\n\n".join(_text_item(item) for item in items)
        return body + ("\n" if body else "")
    raise ValueError(f"unsupported query output format {output_format!r}")


def format_search_results(
    command: str, results: SearchResults, output_format: str
) -> str:
    """Render a search-v2 page while preserving total/truncation metadata."""

    items = list(results.items)
    if output_format == "json":
        values: list[JsonValue] = [item for item in items]
        return canonical_json(
            {
                "schema_version": QUERY_SCHEMA_VERSION,
                "search_schema_version": 1,
                "command": command,
                "corpus": results.corpus,
                "match_mode": results.match_mode,
                "sort": results.sort,
                "total_matches": results.total_matches,
                "offset": results.offset,
                "returned": len(items),
                "truncated": results.truncated,
                "items": values,
            }
        )
    if output_format == "jsonl":
        return "".join(
            json.dumps(item, ensure_ascii=False, sort_keys=True) + "\n"
            for item in items
        )
    if output_format == "markdown":
        body = "\n\n".join(_markdown_item(item) for item in items)
        status = (
            f"Showing {len(items)} of {results.total_matches} result(s) "
            f"from offset {results.offset}."
        )
        return f"# agent-team-timeline query: {command}\n\n{body}\n\n_{status}_\n"
    if output_format == "text":
        status = (
            f"Showing {len(items)} of {results.total_matches} result(s) "
            f"from offset {results.offset}."
        )
        body = "\n\n".join(_text_item(item) for item in items)
        return status + "\n\n" + body + ("\n" if body else "")
    raise ValueError(f"unsupported query output format {output_format!r}")


def _stats_content_rows(stats: ArchiveStats) -> tuple[tuple[str, TextTotals], ...]:
    return (
        ("Identified human prompts", stats.human_prompts),
        ("Identified bot/agent prompts", stats.bot_prompts),
        ("Unattributed prompt records", stats.unattributed_prompts),
        ("All prompt records", stats.all_prompts),
        ("Mechanically linked responses", stats.linked_responses),
        ("Unlinked responses", stats.unlinked_responses),
        ("Total responses", stats.total_responses),
        ("Generated summaries", stats.generated_summaries),
        ("Prompts + responses", stats.prompts_and_responses),
        ("All counted content", stats.all_counted_content),
    )


def _stats_scope(stats: ArchiveStats) -> str:
    teams = ", ".join(stats.teams) if stats.teams else "all teams"
    time_scope = "filtered time window" if stats.time_filtered else "all time"
    rollups = (
        ", ".join(stats.rollup_kinds)
        if stats.rollup_kinds
        else "all rollup kinds"
    )
    return f"{teams}; {time_scope}; {rollups}"


def _format_stats_text(stats: ArchiveStats) -> str:
    lines = [
        "Archive content statistics",
        f"Scope: {_stats_scope(stats)}",
        "",
        f"{'Content':<34} {'Records':>12} {'Words':>14} {'UTF-8 bytes':>16}",
        f"{'-' * 34} {'-' * 12} {'-' * 14} {'-' * 16}",
    ]
    for label, totals in _stats_content_rows(stats):
        lines.append(
            f"{label:<34} {totals.records:>12,} {totals.words:>14,} "
            f"{totals.utf8_bytes:>16,}"
        )
    lines.extend(
        (
            "",
            "Summary availability",
            f"{'Kind':<34} {'Available':>12} {'Unavailable':>14} {'Total':>16}",
            f"{'-' * 34} {'-' * 12} {'-' * 14} {'-' * 16}",
        )
    )
    for key, item in stats.summary_kinds.items():
        label = key.replace("_", " ").title()
        lines.append(
            f"{label:<34} {item.available:>12,} {item.unavailable:>14,} "
            f"{item.available + item.unavailable:>16,}"
        )
    available = sum(item.available for item in stats.summary_kinds.values())
    unavailable = sum(item.unavailable for item in stats.summary_kinds.values())
    lines.append(
        f"{'All summary slots':<34} {available:>12,} {unavailable:>14,} "
        f"{available + unavailable:>16,}"
    )
    lines.extend(
        (
            "",
            "Words are whitespace-delimited Unicode text. Byte totals are UTF-8 text "
            "bytes and exclude CLI/JSON serialization framing; generated rollup Markdown "
            "is counted verbatim.",
        )
    )
    return "\n".join(lines) + "\n"


def _format_stats_markdown(stats: ArchiveStats) -> str:
    lines = [
        "# Archive content statistics",
        "",
        f"Scope: {_stats_scope(stats)}.",
        "",
        "| Content | Records | Words | UTF-8 bytes |",
        "| --- | ---: | ---: | ---: |",
    ]
    for label, totals in _stats_content_rows(stats):
        lines.append(
            f"| {label} | {totals.records:,} | {totals.words:,} | "
            f"{totals.utf8_bytes:,} |"
        )
    lines.extend(
        (
            "",
            "## Summary availability",
            "",
            "| Kind | Available | Unavailable | Total |",
            "| --- | ---: | ---: | ---: |",
        )
    )
    for key, item in stats.summary_kinds.items():
        label = key.replace("_", " ").title()
        lines.append(
            f"| {label} | {item.available:,} | {item.unavailable:,} | "
            f"{item.available + item.unavailable:,} |"
        )
    available = sum(item.available for item in stats.summary_kinds.values())
    unavailable = sum(item.unavailable for item in stats.summary_kinds.values())
    lines.extend(
        (
            f"| **All summary slots** | **{available:,}** | **{unavailable:,}** | "
            f"**{available + unavailable:,}** |",
            "",
            "Words are whitespace-delimited Unicode text. Byte totals are UTF-8 text "
            "bytes and exclude CLI/JSON serialization framing; generated rollup Markdown "
            "is counted verbatim.",
        )
    )
    return "\n".join(lines) + "\n"


def format_stats(stats: ArchiveStats, output_format: str) -> str:
    """Render content statistics without entering the generic record-list envelope."""

    if output_format == "json":
        return canonical_json(stats.to_mapping())
    if output_format == "jsonl":
        return json.dumps(
            stats.to_mapping(), ensure_ascii=False, sort_keys=True
        ) + "\n"
    if output_format == "markdown":
        return _format_stats_markdown(stats)
    if output_format == "text":
        return _format_stats_text(stats)
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
    parser.add_argument(
        "--team",
        action="append",
        default=[],
        metavar="SLUG",
        help="include one team slug; repeat to include multiple teams",
    )
    parser.add_argument(
        "--start-time",
        metavar="RFC3339",
        help="include records at or after this timestamp",
    )
    parser.add_argument(
        "--end-time",
        metavar="RFC3339",
        help="exclude records at or after this timestamp",
    )
    parser.add_argument(
        "--kind",
        action="append",
        choices=("hourly", "daily", "weekly", "monthly", "quarterly"),
        default=[],
        help="include one rollup kind; repeat to include several",
    )
    parser.add_argument(
        "--agent",
        metavar="REF",
        help="restrict results to one canonical agent:TEAM::ID reference",
    )


def _standalone_add_transcript_filters(parser: argparse.ArgumentParser) -> None:
    parser.add_argument(
        "--team",
        action="append",
        default=[],
        metavar="SLUG",
        help="include one team slug; repeat to include multiple teams",
    )
    parser.add_argument(
        "--start-time",
        metavar="RFC3339",
        help="include records at or after this timestamp",
    )
    parser.add_argument(
        "--end-time",
        metavar="RFC3339",
        help="exclude records at or after this timestamp",
    )


def _standalone_add_common_options(
    parser: argparse.ArgumentParser, default_format: str
) -> None:
    parser.add_argument(
        "--archive",
        "--output",
        dest="command_archive",
        default=None,
        metavar="PATH",
        help="read a different archive instead of the one containing this executable",
    )
    parser.add_argument(
        "--format",
        dest="command_format",
        choices=("json", "jsonl", "markdown", "text"),
        default=None,
        help=f"output format (default: {default_format})",
    )
    parser.set_defaults(default_format=default_format)


def _standalone_parser() -> argparse.ArgumentParser:
    program = Path(sys.argv[0]).name
    parser = argparse.ArgumentParser(
        prog=program,
        description=(
            "Read prompts, responses, agents, and summaries from this "
            "agent-team timeline archive."
        ),
        epilog=(
            "examples:\n"
            "  ./timeline prompts\n"
            "  ./timeline prompts --range 200-300\n"
            "  ./timeline prompts --which all --format jsonl > all-prompts.jsonl\n"
            "  ./timeline prompts --format jsonl > prompts.jsonl\n"
            "  ./timeline messages --range 200-300\n"
            "  ./timeline stats\n"
            "  ./timeline agents --team codex-coord-030\n"
            "  ./timeline search 'reproducible build' --scope all\n\n"
            "Prompt ranges are 1-based and inclusive. The default archive is the "
            "directory containing this executable."
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    parser.add_argument(
        "--archive",
        "--output",
        dest="global_archive",
        default=str(Path(__file__).resolve().parent),
        metavar="PATH",
        help="archive directory (default: directory containing this executable)",
    )
    parser.add_argument(
        "--format",
        dest="global_format",
        choices=("json", "jsonl", "markdown", "text"),
        default=None,
        help=argparse.SUPPRESS,
    )
    parser.add_argument(
        "--version",
        action="version",
        version=f"%(prog)s archive-query schema {QUERY_SCHEMA_VERSION}",
    )
    sub = parser.add_subparsers(dest="action", required=True, metavar="COMMAND")

    for resource, help_text in (
        ("teams", "list teams represented in the archive"),
        ("agents", "list coordinator and subagent lifetimes"),
        ("phases", "list summarized agent work phases"),
        ("rollups", "list hourly, daily, weekly, and longer summaries"),
    ):
        resource_parser = sub.add_parser(resource, help=help_text, description=help_text)
        _standalone_add_filters(resource_parser)
        _standalone_add_common_options(resource_parser, "json")

    prompts = sub.add_parser(
        "prompts",
        help="print chronological verbatim prompts",
        description=(
            "Print chronological verbatim prompts across the selected teams. "
            "Human-authored prompts are the default."
        ),
        epilog=(
            "examples:\n"
            "  ./timeline prompts\n"
            "  ./timeline prompts --range 200-300\n"
            "  ./timeline prompts --which all --format jsonl\n"
            "  ./timeline prompts --team orc-coord-014 --format jsonl"
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    prompts.add_argument(
        "--range",
        dest="ordinal_range",
        metavar="N[-M]",
        help="one prompt ordinal or an inclusive range, for example 200-300",
    )
    prompts.add_argument(
        "--which",
        choices=PROMPT_SELECTIONS,
        default="human",
        help="select human, bot, or all prompt authorship (default: %(default)s)",
    )
    _standalone_add_transcript_filters(prompts)
    _standalone_add_common_options(prompts, "text")

    messages = sub.add_parser(
        "messages",
        help="print prompts and their mechanically linked responses",
        description=(
            "Print chronological prompts plus coordinator responses mechanically "
            "associated with the selected prompts. Human-authored prompts are the default."
        ),
    )
    messages.add_argument(
        "--range",
        dest="ordinal_range",
        metavar="N[-M]",
        help="one prompt ordinal or an inclusive range, for example 200-300",
    )
    messages.add_argument(
        "--which",
        choices=PROMPT_SELECTIONS,
        default="human",
        help="select human, bot, or all prompt authorship (default: %(default)s)",
    )
    _standalone_add_transcript_filters(messages)
    _standalone_add_common_options(messages, "text")

    stats = sub.add_parser(
        "stats",
        help="count prompt, response, and generated-summary text",
        description=(
            "Count records, whitespace-delimited words, and UTF-8 text bytes for "
            "owner prompts, responses, and generated summaries. This is a read-only, "
            "zero-model operation."
        ),
        epilog=(
            "examples:\n"
            "  ./timeline stats\n"
            "  ./timeline stats --team codex-coord-030\n"
            "  ./timeline stats --start-time 2026-08-11T00:00:00-04:00 "
            "--end-time 2026-08-12T00:00:00-04:00\n"
            "  ./timeline stats --format json"
        ),
        formatter_class=argparse.RawDescriptionHelpFormatter,
    )
    _standalone_add_transcript_filters(stats)
    stats.add_argument(
        "--kind",
        action="append",
        choices=("hourly", "daily", "weekly", "monthly", "quarterly"),
        default=[],
        help="count one rollup kind; repeat to include several",
    )
    _standalone_add_common_options(stats, "text")

    listing = sub.add_parser(
        "list",
        help="compatibility form of teams/agents/phases/rollups",
        description="List one class of timeline record.",
    )
    listing.add_argument(
        "resource",
        choices=("teams", "agents", "phases", "rollups"),
        help="record class to list",
    )
    _standalone_add_filters(listing)
    _standalone_add_common_options(listing, "json")

    showing = sub.add_parser(
        "show",
        help="resolve one stable timeline or transcript reference",
        description="Show one record selected by a stable reference from a list or search.",
    )
    showing.add_argument(
        "reference",
        help="team:, agent:, phase:, rollup:, message:, or tool: reference",
    )
    showing.add_argument(
        "--transcript",
        action="store_true",
        help="include condensed transcript messages for a work phase",
    )
    _standalone_add_common_options(showing, "json")

    searching = sub.add_parser(
        "search",
        help="search prompts, responses, full transcript text, and summaries",
        description=(
            "Search the canonical transcript corpus with stable message references, or use "
            "--scope for the compatibility phase-transcript search."
        ),
    )
    searching.add_argument("text", help="text or pattern to find")
    searching.add_argument(
        "--in",
        dest="search_corpus",
        choices=SEARCH_CORPORA,
        help="search-v2 corpus: owner prompts, agent responses, or all transcript",
    )
    searching.add_argument(
        "--scope",
        choices=("summaries", "transcripts", "all"),
        default=None,
        help="compatibility search scope (default when --in is absent: summaries)",
    )
    searching.add_argument(
        "--match", choices=SEARCH_MATCH_MODES, default=None
    )
    searching.add_argument("--sort", choices=SEARCH_SORTS, default=None)
    searching.add_argument(
        "--prompt-author", choices=SEARCH_PROMPT_AUTHORS, default=None
    )
    searching.add_argument(
        "--linkage", choices=SEARCH_LINKAGES, default=None
    )
    searching.add_argument(
        "--role", action="append", choices=SEARCH_ROLES, default=[]
    )
    searching.add_argument(
        "--case-sensitive", action="store_true", help="preserve letter case"
    )
    searching.add_argument("--offset", type=int, default=None)
    searching.add_argument(
        "--limit", type=int, default=50, help="maximum matches (default: %(default)s)"
    )
    _standalone_add_filters(searching)
    _standalone_add_common_options(searching, "json")
    return parser


def _standalone_output_format(ns: argparse.Namespace) -> str:
    command_format: object = getattr(ns, "command_format", None)
    global_format: object = getattr(ns, "global_format", None)
    default_format: object = getattr(ns, "default_format", "json")
    selected = command_format or global_format or default_format
    if not isinstance(selected, str) or selected not in {
        "json",
        "jsonl",
        "markdown",
        "text",
    }:
        raise ValueError("output format must be json, jsonl, markdown, or text")
    return selected


def _standalone_filters(ns: argparse.Namespace) -> QueryFilters:
    raw_teams: object = ns.team
    raw_kinds: object = getattr(ns, "kind", [])
    raw_agent: object = getattr(ns, "agent", None)
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
        command_archive: object = getattr(ns, "command_archive", None)
        global_archive: object = getattr(ns, "global_archive", None)
        if command_archive is not None and not isinstance(command_archive, str):
            raise ValueError("--archive must be a path")
        if not isinstance(global_archive, str):
            raise ValueError("--archive must be a path")
        output = Path(command_archive or global_archive).expanduser()
        action = str(ns.action)
        if action in {"teams", "agents", "phases", "rollups"}:
            query = TimelineQuery(output)
            command = f"list {action}"
            items = query.list_records(action, _standalone_filters(ns))
        elif action == "list":
            query = TimelineQuery(output)
            resource = str(ns.resource)
            command = f"list {resource}"
            items = query.list_records(resource, _standalone_filters(ns))
        elif action == "show":
            query = TimelineQuery(output)
            reference = str(ns.reference)
            command = f"show {reference}"
            items = [query.show(reference, transcript=bool(ns.transcript))]
        elif action == "search":
            query = TimelineQuery(output)
            needle = str(ns.text)
            command = f"search {needle}"
            raw_corpus: object = ns.search_corpus
            raw_scope: object = ns.scope
            if raw_corpus is not None:
                if raw_scope is not None:
                    raise ValueError("--in and --scope cannot be combined")
                raw_roles: object = ns.role
                if not isinstance(raw_roles, list) or not all(
                    isinstance(role, str) for role in raw_roles
                ):
                    raise ValueError("--role values must be strings")
                search_results = query.search_v2(
                    needle,
                    corpus=str(raw_corpus),
                    filters=_standalone_filters(ns),
                    case_sensitive=bool(ns.case_sensitive),
                    match_mode=str(ns.match or "smart"),
                    sort=str(ns.sort or "relevance"),
                    prompt_author=str(ns.prompt_author or "any"),
                    linkage=str(ns.linkage or "any"),
                    roles=tuple(raw_roles),
                    offset=int(ns.offset or 0),
                    limit=int(ns.limit),
                )
                print(
                    format_search_results(
                        command, search_results, _standalone_output_format(ns)
                    ),
                    end="",
                )
                return 0
            if (
                ns.match is not None
                or ns.sort is not None
                or ns.prompt_author is not None
                or ns.linkage is not None
                or ns.offset is not None
                or ns.role
            ):
                raise ValueError("search-v2 options require --in")
            items = query.search(
                needle,
                scope=str(raw_scope or "summaries"),
                filters=_standalone_filters(ns),
                case_sensitive=bool(ns.case_sensitive),
                limit=int(ns.limit),
            )
        elif action in {"prompts", "messages"}:
            query_transcripts = TranscriptQuery(output)
            raw_range: object = ns.ordinal_range
            ordinal_range = (
                parse_ordinal_range(raw_range)
                if isinstance(raw_range, str)
                else None
            )
            command = action
            items = (
                query_transcripts.list_prompts(
                    _standalone_filters(ns), ordinal_range, str(ns.which)
                )
                if action == "prompts"
                else query_transcripts.list_messages(
                    _standalone_filters(ns), ordinal_range, str(ns.which)
                )
            )
        elif action == "stats":
            stats_result = archive_stats(output, _standalone_filters(ns))
            print(
                format_stats(stats_result, _standalone_output_format(ns)),
                end="",
            )
            return 0
        else:
            raise ValueError(f"unsupported query action {action!r}")
        print(format_query(command, items, _standalone_output_format(ns)), end="")
        return 0
    except (OSError, ValueError) as error:
        print(f"{Path(sys.argv[0]).name}: {error}", file=sys.stderr)
        return 2


__all__ = [
    "ArchiveStats",
    "OrdinalRange",
    "QueryFilters",
    "SEARCH_CORPORA",
    "SEARCH_LINKAGES",
    "SEARCH_MATCH_MODES",
    "SEARCH_PROMPT_AUTHORS",
    "SEARCH_ROLES",
    "SEARCH_SORTS",
    "SearchResults",
    "TimelineQuery",
    "TranscriptQuery",
    "archive_stats",
    "agent_ref",
    "format_query",
    "format_search_results",
    "format_stats",
    "phase_ref",
    "parse_ordinal_range",
    "query_envelope",
    "rollup_ref",
    "team_ref",
]


if __name__ == "__main__":
    raise SystemExit(_standalone_main())
