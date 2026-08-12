"""Content-addressed, range-addressable projection of timeline schema 1.

Schema 1 remains the compatibility and command-line projection.  This module adds a browser
projection whose stable bootstrap is deliberately small enough to load before any detailed day.
Every referenced object is written before the bootstrap that publishes its URL.
"""

from __future__ import annotations

import hashlib
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

from agent_team_timeline.archive import (
    JsonValue,
    as_array,
    as_int,
    as_object,
    canonical_json,
    narrow_json,
    write_text_if_changed,
)
from agent_team_timeline.static_assets import (
    gzip_sidecar_path,
    sync_gzip_sidecar,
    write_text_with_gzip_invalidation,
)


SCHEMA_2_BOOTSTRAP_PATH = "data/timeline-v2.json"
_OBJECT_ROOT = "data/timeline-v2/objects"
_DAY_MS = 24 * 60 * 60 * 1000
_DETAIL_FIELDS = frozenset({"phases", "edges", "events"})
_STRUCTURAL_EDGE_KINDS = frozenset({"spawn", "continuation", "result"})
_BOOTSTRAP_FIELDS = frozenset(
    {
        "generated_at",
        "source_digest",
        "display_timezone",
        "display_timezone_source",
        "range",
        "teams",
        "activity_bins",
    }
)


@dataclass(frozen=True)
class TimelineShardReport:
    """Files and byte counts produced by one schema-2 projection."""

    files_changed: int
    generated_files: tuple[str, ...]
    detail_shards: int
    bootstrap_bytes: int
    bootstrap_gzip_bytes: int | None
    object_bytes: int
    object_gzip_bytes: int


@dataclass(frozen=True)
class _StoredObject:
    digest: str
    relative_path: str
    bytes: int
    gzip_bytes: int | None
    changed: int
    generated_files: tuple[str, ...]

    def catalog_obj(self) -> dict[str, JsonValue]:
        return {
            "url": self.relative_path,
            "sha256": self.digest,
            "bytes": self.bytes,
            "gzip_bytes": self.gzip_bytes,
        }


def _object_path(digest: str) -> str:
    return f"{_OBJECT_ROOT}/{digest}.json"


def _safe_output_path(output: Path, relative: str) -> Path:
    path = Path(relative)
    if path.is_absolute() or not path.parts or any(
        part in ("", ".", "..") for part in path.parts
    ):
        raise ValueError(f"unsafe timeline shard output path: {relative!r}")
    cursor = output
    for part in path.parts[:-1]:
        cursor /= part
        if cursor.is_symlink():
            raise ValueError(f"refusing timeline shard parent symlink: {cursor}")
    candidate = output.joinpath(*path.parts)
    try:
        candidate.parent.resolve().relative_to(output.resolve())
    except ValueError as error:
        raise ValueError(
            f"timeline shard output path escapes through a symlink: {relative!r}"
        ) from error
    if candidate.is_symlink():
        raise ValueError(f"refusing timeline shard output symlink: {candidate}")
    return candidate


def _store_object(
    output: Path, value: dict[str, JsonValue], *, precompress: bool
) -> _StoredObject:
    content = canonical_json(value)
    encoded = content.encode("utf-8")
    digest = hashlib.sha256(encoded).hexdigest()
    relative = _object_path(digest)
    path = _safe_output_path(output, relative)
    _safe_output_path(output, relative + ".gz")
    changed = int(write_text_if_changed(path, content))
    if precompress:
        changed += int(sync_gzip_sidecar(path))
    sidecar = gzip_sidecar_path(path)
    generated = [relative]
    gzip_bytes: int | None = None
    if precompress and sidecar.is_file():
        generated.append(relative + ".gz")
        gzip_bytes = sidecar.stat().st_size
    return _StoredObject(
        digest=digest,
        relative_path=relative,
        bytes=len(encoded),
        gzip_bytes=gzip_bytes,
        changed=changed,
        generated_files=tuple(generated),
    )


def _utc_day_start(timestamp_ms: int) -> int:
    instant = datetime.fromtimestamp(timestamp_ms / 1000, tz=timezone.utc)
    midnight = datetime(instant.year, instant.month, instant.day, tzinfo=timezone.utc)
    return int(midnight.timestamp() * 1000)


def _day_starts(start_ms: int, end_ms: int) -> tuple[int, ...]:
    """Return UTC days intersecting the half-open interval, or the point at ``start_ms``."""

    if end_ms < start_ms:
        raise ValueError("timeline detail interval ends before it starts")
    final_ms = start_ms if end_ms == start_ms else end_ms - 1
    first = _utc_day_start(start_ms)
    final = _utc_day_start(final_ms)
    return tuple(range(first, final + _DAY_MS, _DAY_MS))


def _record_int(record: dict[str, JsonValue], field: str, where: str) -> int:
    return as_int(record.get(field), f"{where}.{field}")


def _append_interval_record(
    days: dict[int, dict[str, list[JsonValue]]],
    field: str,
    record: dict[str, JsonValue],
    start_ms: int,
    end_ms: int,
) -> None:
    for day_start in _day_starts(start_ms, end_ms):
        day = days.setdefault(
            day_start,
            {"phases": [], "edges": [], "events": []},
        )
        day[field].append(record)


def _detail_days(timeline: dict[str, JsonValue]) -> dict[int, dict[str, list[JsonValue]]]:
    days: dict[int, dict[str, list[JsonValue]]] = {}
    for index, raw in enumerate(as_array(timeline.get("phases"), "timeline.phases")):
        record = as_object(raw, f"timeline.phases[{index}]")
        _append_interval_record(
            days,
            "phases",
            record,
            _record_int(record, "start_ms", f"timeline.phases[{index}]"),
            _record_int(record, "end_ms", f"timeline.phases[{index}]"),
        )
    for index, raw in enumerate(as_array(timeline.get("edges"), "timeline.edges")):
        record = as_object(raw, f"timeline.edges[{index}]")
        if record.get("kind") in _STRUCTURAL_EDGE_KINDS:
            continue
        source_ms = _record_int(record, "source_ms", f"timeline.edges[{index}]")
        target_ms = _record_int(record, "target_ms", f"timeline.edges[{index}]")
        _append_interval_record(
            days,
            "edges",
            record,
            min(source_ms, target_ms),
            max(source_ms, target_ms) + 1,
        )
    for index, raw in enumerate(as_array(timeline.get("events"), "timeline.events")):
        record = as_object(raw, f"timeline.events[{index}]")
        at_ms = _record_int(record, "at_ms", f"timeline.events[{index}]")
        _append_interval_record(days, "events", record, at_ms, at_ms)
    return days


def _global_object(timeline: dict[str, JsonValue]) -> dict[str, JsonValue]:
    projected = {
        key: value
        for key, value in timeline.items()
        if key not in _DETAIL_FIELDS
        and key not in _BOOTSTRAP_FIELDS
        and key != "schema_version"
    }
    structural_edges: list[JsonValue] = []
    for index, raw in enumerate(as_array(timeline.get("edges"), "timeline.edges")):
        record = as_object(raw, f"timeline.edges[{index}]")
        if record.get("kind") in _STRUCTURAL_EDGE_KINDS:
            structural_edges.append(record)
    return {
        "schema_version": 2,
        "kind": "timeline-global",
        "edges": structural_edges,
        **projected,
    }


def _required_value(timeline: dict[str, JsonValue], field: str) -> JsonValue:
    if field not in timeline:
        raise ValueError(f"timeline.{field}: missing required schema-1 field")
    return timeline[field]


def write_timeline_shards(
    output: Path,
    raw_timeline: dict[str, JsonValue],
    *,
    precompress: bool = True,
) -> TimelineShardReport:
    """Publish schema-2 objects and then atomically publish their stable bootstrap.

    The caller continues to own ``data/timeline.json``.  Keeping that schema-1 file makes both old
    browsers and the archive-local query CLI valid throughout the migration.
    """

    if as_int(raw_timeline.get("schema_version"), "timeline.schema_version") != 1:
        raise ValueError("schema-2 sharding requires a schema-1 source timeline")
    bootstrap_path = _safe_output_path(output, SCHEMA_2_BOOTSTRAP_PATH)
    _safe_output_path(output, SCHEMA_2_BOOTSTRAP_PATH + ".gz")
    _safe_output_path(output, f"{_OBJECT_ROOT}/object.json")
    timeline = as_object(narrow_json(raw_timeline), "timeline")
    days = _detail_days(timeline)
    generated_files: set[str] = {SCHEMA_2_BOOTSTRAP_PATH}
    changed = 0
    object_bytes = 0
    object_gzip_bytes = 0

    global_object = _store_object(
        output, _global_object(timeline), precompress=precompress
    )
    changed += global_object.changed
    generated_files.update(global_object.generated_files)
    object_bytes += global_object.bytes
    object_gzip_bytes += global_object.gzip_bytes or global_object.bytes

    shard_catalog: list[JsonValue] = []
    for day_start, records in sorted(days.items()):
        day_end = day_start + _DAY_MS
        detail_object: dict[str, JsonValue] = {
            "schema_version": 2,
            "kind": "timeline-detail-day",
            "range": {"start_ms": day_start, "end_ms": day_end},
            "phases": records["phases"],
            "edges": records["edges"],
            "events": records["events"],
        }
        stored = _store_object(output, detail_object, precompress=precompress)
        changed += stored.changed
        generated_files.update(stored.generated_files)
        object_bytes += stored.bytes
        object_gzip_bytes += stored.gzip_bytes or stored.bytes
        day_label = datetime.fromtimestamp(day_start / 1000, tz=timezone.utc).date()
        shard_catalog.append(
            {
                "kind": "utc-day",
                "day": day_label.isoformat(),
                "start_ms": day_start,
                "end_ms": day_end,
                **stored.catalog_obj(),
                "counts": {
                    "phases": len(records["phases"]),
                    "edges": len(records["edges"]),
                    "events": len(records["events"]),
                },
            }
        )

    bootstrap: dict[str, JsonValue] = {
        "schema_version": 2,
        "kind": "timeline-bootstrap",
        "generated_at": _required_value(timeline, "generated_at"),
        "source_digest": _required_value(timeline, "source_digest"),
        "display_timezone": _required_value(timeline, "display_timezone"),
        "display_timezone_source": timeline.get(
            "display_timezone_source", "legacy_team_data"
        ),
        "range": _required_value(timeline, "range"),
        "teams": _required_value(timeline, "teams"),
        "activity_bins": _required_value(timeline, "activity_bins"),
        "global": global_object.catalog_obj(),
        "detail_shards": shard_catalog,
        "search": {
            "strategy": "load-all-detail-shards-on-first-query",
            "fields": ["agent", "phase", "edge"],
        },
    }
    # Publication point: every immutable URL above exists (with its gzip representation, when
    # useful) before the stable manifest begins referring to it.
    bootstrap_text = canonical_json(bootstrap)
    changed += write_text_with_gzip_invalidation(bootstrap_path, bootstrap_text)
    bootstrap_sidecar = gzip_sidecar_path(bootstrap_path)
    if precompress:
        changed += int(sync_gzip_sidecar(bootstrap_path))
    elif bootstrap_sidecar.is_file():
        bootstrap_sidecar.unlink()
        changed += 1
    bootstrap_gzip_bytes = (
        bootstrap_sidecar.stat().st_size if bootstrap_sidecar.is_file() else None
    )
    if bootstrap_gzip_bytes is not None:
        generated_files.add(SCHEMA_2_BOOTSTRAP_PATH + ".gz")
    bootstrap_bytes = len(bootstrap_text.encode("utf-8"))
    return TimelineShardReport(
        files_changed=changed,
        generated_files=tuple(sorted(generated_files)),
        detail_shards=len(shard_catalog),
        bootstrap_bytes=bootstrap_bytes,
        bootstrap_gzip_bytes=bootstrap_gzip_bytes,
        object_bytes=object_bytes,
        object_gzip_bytes=object_gzip_bytes,
    )


__all__ = [
    "SCHEMA_2_BOOTSTRAP_PATH",
    "TimelineShardReport",
    "write_timeline_shards",
]
