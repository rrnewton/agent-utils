"""Content-addressed, range-addressable projection of timeline schema 1.

Schema 1 remains the compatibility and command-line projection.  This module adds a browser
projection whose stable bootstrap is deliberately small enough to load before any detailed day.
Every referenced object is written before the bootstrap that publishes its URL.
"""

from __future__ import annotations

import hashlib
from bisect import bisect_left
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path

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
from agent_team_timeline.static_assets import (
    gzip_sidecar_path,
    sync_gzip_sidecar,
    write_text_with_gzip_invalidation,
)


SCHEMA_2_BOOTSTRAP_PATH = "data/timeline-v2.json"
_OBJECT_ROOT = "data/timeline-v2/objects"
_OBJECT_MANIFEST_PATH = "data/timeline-v2/manifest.json"
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


@dataclass(frozen=True)
class _PreviousObjects:
    current: frozenset[str]
    retained: frozenset[str]
    scope: _ShardScope | None


@dataclass(frozen=True)
class _ShardScope:
    teams: tuple[str, ...]
    start_ms: int
    end_ms: int


def _object_path(digest: str) -> str:
    return f"{_OBJECT_ROOT}/{digest}.json"


def _is_object_file(relative: str) -> bool:
    base = relative.removesuffix(".gz")
    path = Path(base)
    return (
        len(path.parts) == 4
        and tuple(path.parts[:3]) == ("data", "timeline-v2", "objects")
        and len(path.stem) == 64
        and all(character in "0123456789abcdef" for character in path.stem)
        and path.suffix == ".json"
    )


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


def _object_files_on_disk(output: Path) -> frozenset[str]:
    probe = _safe_output_path(output, f"{_OBJECT_ROOT}/{'0' * 64}.json")
    root = probe.parent
    if not root.exists():
        return frozenset()
    if not root.is_dir():
        raise ValueError(f"timeline shard object root is not a directory: {root}")
    found: set[str] = set()
    for path in root.iterdir():
        relative = path.relative_to(output).as_posix()
        if not _is_object_file(relative):
            continue
        safe = _safe_output_path(output, relative)
        if safe.is_file():
            found.add(relative)
    return frozenset(found)


def _manifest_object_files(
    output: Path, root: dict[str, JsonValue], field: str
) -> frozenset[str]:
    values: set[str] = set()
    for index, raw in enumerate(as_array(root.get(field), f"{field}")):
        relative = as_string(raw, f"{field}[{index}]")
        if not _is_object_file(relative):
            raise ValueError(
                f"invalid timeline shard object path in {field}: {relative!r}"
            )
        _safe_output_path(output, relative)
        if relative in values:
            raise ValueError(f"duplicate timeline shard object path in {field}: {relative!r}")
        values.add(relative)
    return frozenset(values)


def _shard_scope(root: dict[str, JsonValue], where: str) -> _ShardScope:
    teams: list[str] = []
    for index, raw in enumerate(as_array(root.get("teams"), where + ".teams")):
        team = as_object(raw, f"{where}.teams[{index}]")
        teams.append(as_string(team.get("slug"), f"{where}.teams[{index}].slug"))
    time_range = as_object(root.get("range"), where + ".range")
    start_ms = as_int(time_range.get("start_ms"), where + ".range.start_ms")
    end_ms = as_int(time_range.get("end_ms"), where + ".range.end_ms")
    if end_ms <= start_ms:
        raise ValueError(f"{where}.range: end must be after start")
    return _ShardScope(tuple(sorted(set(teams))), start_ms, end_ms)


def _scope_from_bootstrap(output: Path) -> _ShardScope | None:
    path = _safe_output_path(output, SCHEMA_2_BOOTSTRAP_PATH)
    if not path.is_file():
        return None
    try:
        root = as_object(read_json(path), str(path))
        if root.get("schema_version") != 2 or root.get("kind") != "timeline-bootstrap":
            return None
        return _shard_scope(root, str(path))
    except (OSError, ValueError):
        return None


def _manifest_scope(root: dict[str, JsonValue], where: str) -> _ShardScope | None:
    raw_scope = root.get("scope")
    if raw_scope is None:
        return None
    return _shard_scope(as_object(raw_scope, where + ".scope"), where + ".scope")


def _scope_json(scope: _ShardScope) -> dict[str, JsonValue]:
    teams: list[JsonValue] = []
    for slug in scope.teams:
        teams.append({"slug": slug})
    return {
        "teams": teams,
        "range": {"start_ms": scope.start_ms, "end_ms": scope.end_ms},
    }


def _scope_allows_retention(previous: _ShardScope | None, current: _ShardScope) -> bool:
    return (
        previous is not None
        and previous.teams == current.teams
        and current.start_ms <= previous.start_ms
        and current.end_ms >= previous.end_ms
    )


def _previous_objects(output: Path) -> _PreviousObjects:
    manifest_path = _safe_output_path(output, _OBJECT_MANIFEST_PATH)
    if not manifest_path.is_file():
        # Upgrade path: preserve every exact content-addressed object for one distinct generation.
        # Older code did not record reachability, so treating the whole set as current avoids a
        # destructive guess while still allowing a later changed generation to collect it.
        return _PreviousObjects(
            _object_files_on_disk(output),
            frozenset(),
            _scope_from_bootstrap(output),
        )
    root = as_object(read_json(manifest_path), str(manifest_path))
    if root.get("schema_version") != 1 or root.get("kind") != "timeline-shard-files":
        raise ValueError(f"unsupported timeline shard manifest at {manifest_path}")
    current = _manifest_object_files(output, root, "current_objects")
    retained = _manifest_object_files(output, root, "retained_objects")
    if current & retained:
        raise ValueError(f"overlapping current and retained objects at {manifest_path}")
    existing = _object_files_on_disk(output)
    recorded = current | retained
    # A crash can leave newly written immutable objects outside the last completed manifest. Keep
    # them for one generation; exact-name scanning never grants authority over any other file.
    return _PreviousObjects(
        current & existing,
        (retained & existing) | (existing - recorded),
        _manifest_scope(root, str(manifest_path)) or _scope_from_bootstrap(output),
    )


def _finish_object_generation(
    output: Path,
    previous: _PreviousObjects,
    current: frozenset[str],
    current_scope: _ShardScope,
) -> tuple[int, frozenset[str]]:
    retained_candidates = (
        (
            previous.retained
            if current == previous.current
            else previous.current - current
        )
        if _scope_allows_retention(previous.scope, current_scope)
        else frozenset()
    )
    retained = frozenset(
        relative
        for relative in retained_candidates
        if _safe_output_path(output, relative).is_file()
    )
    changed = 0
    for relative in sorted((previous.current | previous.retained) - current - retained):
        path = _safe_output_path(output, relative)
        if path.is_file():
            path.unlink()
            changed += 1
    current_values: list[JsonValue] = []
    retained_values: list[JsonValue] = []
    for relative in sorted(current):
        current_values.append(relative)
    for relative in sorted(retained):
        retained_values.append(relative)
    manifest: dict[str, JsonValue] = {
        "schema_version": 1,
        "kind": "timeline-shard-files",
        "scope": _scope_json(current_scope),
        "current_objects": current_values,
        "retained_objects": retained_values,
    }
    changed += int(
        write_text_if_changed(
            _safe_output_path(output, _OBJECT_MANIFEST_PATH),
            canonical_json(manifest),
        )
    )
    return changed, retained


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


def _minimum_activity_range(
    bounds_start: int,
    bounds_end: int,
    intervals: list[tuple[int, int]],
    points: list[int],
) -> tuple[int, int]:
    extent_start = bounds_end
    extent_end = bounds_start
    for start, end in intervals:
        clipped_start = max(bounds_start, start)
        clipped_end = min(bounds_end, end)
        if clipped_end > clipped_start:
            extent_start = min(extent_start, clipped_start)
            extent_end = max(extent_end, clipped_end)
    first_point = bisect_left(points, bounds_start)
    if first_point < len(points) and points[first_point] < bounds_end:
        last_point = bisect_left(points, bounds_end) - 1
        extent_start = min(extent_start, points[first_point])
        extent_end = max(extent_end, min(bounds_end, points[last_point] + 1))
    if extent_end <= extent_start:
        extent_start, extent_end = bounds_start, bounds_end
    minimum = min(bounds_end - bounds_start, 1000)
    if extent_end - extent_start < minimum:
        missing = minimum - max(0, extent_end - extent_start)
        extent_start = max(bounds_start, extent_start - missing // 2)
        extent_end = extent_start + minimum
        if extent_end > bounds_end:
            extent_end = bounds_end
            extent_start = extent_end - minimum
    return extent_start, extent_end


def _record_id(record: dict[str, JsonValue], field: str) -> str:
    value = record.get(field)
    return value if isinstance(value, str) else ""


def _phase_intervals(record: dict[str, JsonValue], where: str) -> list[tuple[int, int]]:
    start = _record_int(record, "start_ms", where)
    end = _record_int(record, "end_ms", where)
    intervals: list[tuple[int, int]] = []
    valid_state = False
    for index, raw in enumerate(as_array(record.get("states"), where + ".states")):
        state = as_object(raw, f"{where}.states[{index}]")
        state_start = _record_int(state, "start_ms", f"{where}.states[{index}]")
        state_end = _record_int(state, "end_ms", f"{where}.states[{index}]")
        if state_end <= state_start:
            continue
        valid_state = True
        kind = state.get("kind")
        if not isinstance(kind, str) or kind.lower() != "idle":
            intervals.append((state_start, state_end))
    if not valid_state and end > start:
        intervals.append((start, end))
    return intervals


def _activity_bounds(
    timeline: dict[str, JsonValue],
) -> tuple[
    dict[str, tuple[int, int]],
    dict[str, tuple[int, int]],
    tuple[tuple[int, int], ...],
]:
    phase_intervals: dict[str, list[tuple[int, int]]] = {}
    agent_intervals: dict[str, list[tuple[int, int]]] = {}
    global_intervals: list[tuple[int, int]] = []
    phase_records: list[dict[str, JsonValue]] = []
    for index, raw in enumerate(as_array(timeline.get("phases"), "timeline.phases")):
        record = as_object(raw, f"timeline.phases[{index}]")
        phase_id = _record_id(record, "id")
        agent_id = _record_id(record, "agent_id")
        intervals = _phase_intervals(record, f"timeline.phases[{index}]")
        phase_intervals[phase_id] = intervals
        agent_intervals.setdefault(agent_id, []).extend(intervals)
        global_intervals.extend(intervals)
        phase_records.append(record)

    agent_points: dict[str, list[int]] = {}
    global_points: list[int] = []
    for index, raw in enumerate(as_array(timeline.get("events"), "timeline.events")):
        record = as_object(raw, f"timeline.events[{index}]")
        at_ms = _record_int(record, "at_ms", f"timeline.events[{index}]")
        agent_points.setdefault(_record_id(record, "agent_id"), []).append(at_ms)
        global_points.append(at_ms)
    for index, raw in enumerate(as_array(timeline.get("edges"), "timeline.edges")):
        record = as_object(raw, f"timeline.edges[{index}]")
        source_ms = _record_int(record, "source_ms", f"timeline.edges[{index}]")
        target_ms = _record_int(record, "target_ms", f"timeline.edges[{index}]")
        agent_points.setdefault(_record_id(record, "source_id"), []).append(source_ms)
        agent_points.setdefault(_record_id(record, "target_id"), []).append(target_ms)
        global_points.extend((source_ms, target_ms))
    for points in agent_points.values():
        points.sort()
    global_points.sort()

    phase_bounds: dict[str, tuple[int, int]] = {}
    for record in phase_records:
        phase_id = _record_id(record, "id")
        agent_id = _record_id(record, "agent_id")
        start = _record_int(record, "start_ms", f"phase {phase_id}")
        end = _record_int(record, "end_ms", f"phase {phase_id}")
        phase_bounds[phase_id] = _minimum_activity_range(
            start,
            end,
            phase_intervals.get(phase_id, []),
            agent_points.get(agent_id, []),
        )

    agent_bounds: dict[str, tuple[int, int]] = {}
    for index, raw in enumerate(as_array(timeline.get("agents"), "timeline.agents")):
        record = as_object(raw, f"timeline.agents[{index}]")
        agent_id = _record_id(record, "id")
        start = _record_int(record, "start_ms", f"timeline.agents[{index}]")
        end = _record_int(record, "end_ms", f"timeline.agents[{index}]")
        agent_bounds[agent_id] = _minimum_activity_range(
            start,
            end,
            agent_intervals.get(agent_id, []),
            agent_points.get(agent_id, []),
        )

    rollup_bounds: list[tuple[int, int]] = []
    for index, raw in enumerate(as_array(timeline.get("rollups"), "timeline.rollups")):
        record = as_object(raw, f"timeline.rollups[{index}]")
        start = _record_int(record, "start_ms", f"timeline.rollups[{index}]")
        end = _record_int(record, "end_ms", f"timeline.rollups[{index}]")
        rollup_bounds.append(
            _minimum_activity_range(start, end, global_intervals, global_points)
        )
    return phase_bounds, agent_bounds, tuple(rollup_bounds)


def _with_activity_bounds(
    record: dict[str, JsonValue], bounds: tuple[int, int]
) -> dict[str, JsonValue]:
    return {
        **record,
        "activity_start_ms": bounds[0],
        "activity_end_ms": bounds[1],
    }


def _detail_days(
    timeline: dict[str, JsonValue], phase_bounds: dict[str, tuple[int, int]]
) -> dict[int, dict[str, list[JsonValue]]]:
    days: dict[int, dict[str, list[JsonValue]]] = {}
    for index, raw in enumerate(as_array(timeline.get("phases"), "timeline.phases")):
        record = as_object(raw, f"timeline.phases[{index}]")
        projected = _with_activity_bounds(
            record,
            phase_bounds.get(
                _record_id(record, "id"),
                (
                    _record_int(record, "start_ms", f"timeline.phases[{index}]"),
                    _record_int(record, "end_ms", f"timeline.phases[{index}]"),
                ),
            ),
        )
        _append_interval_record(
            days,
            "phases",
            projected,
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


def _global_object(
    timeline: dict[str, JsonValue],
    agent_bounds: dict[str, tuple[int, int]],
    rollup_bounds: tuple[tuple[int, int], ...],
) -> dict[str, JsonValue]:
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
    agents: list[JsonValue] = []
    for index, raw in enumerate(as_array(timeline.get("agents"), "timeline.agents")):
        record = as_object(raw, f"timeline.agents[{index}]")
        agents.append(
            _with_activity_bounds(
                record,
                agent_bounds.get(
                    _record_id(record, "id"),
                    (
                        _record_int(record, "start_ms", f"timeline.agents[{index}]"),
                        _record_int(record, "end_ms", f"timeline.agents[{index}]"),
                    ),
                ),
            )
        )
    rollups: list[JsonValue] = []
    for index, raw in enumerate(as_array(timeline.get("rollups"), "timeline.rollups")):
        record = as_object(raw, f"timeline.rollups[{index}]")
        rollups.append(_with_activity_bounds(record, rollup_bounds[index]))
    projected["agents"] = agents
    projected["rollups"] = rollups
    return {
        "schema_version": 2,
        "kind": "timeline-global",
        "edges": structural_edges,
        **projected,
    }


def _phase_index_object(
    timeline: dict[str, JsonValue], phase_bounds: dict[str, tuple[int, int]]
) -> dict[str, JsonValue]:
    card_fields = frozenset(
        {
            "id",
            "agent_id",
            "start_ms",
            "end_ms",
            "phrase",
            "paragraph",
            "summary_available",
            "detail_path",
        }
    )
    phases: list[JsonValue] = []
    for index, raw in enumerate(as_array(timeline.get("phases"), "timeline.phases")):
        record = as_object(raw, f"timeline.phases[{index}]")
        card = {key: value for key, value in record.items() if key in card_fields}
        phase_id = _record_id(record, "id")
        phases.append(
            _with_activity_bounds(
                card,
                phase_bounds.get(
                    phase_id,
                    (
                        _record_int(record, "start_ms", f"timeline.phases[{index}]"),
                        _record_int(record, "end_ms", f"timeline.phases[{index}]"),
                    ),
                ),
            )
        )
    return {
        "schema_version": 2,
        "kind": "timeline-phase-index",
        "phases": phases,
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
    current_scope = _shard_scope(timeline, "timeline")
    previous_objects = _previous_objects(output)
    phase_bounds, agent_bounds, rollup_bounds = _activity_bounds(timeline)
    days = _detail_days(timeline, phase_bounds)
    generated_files: set[str] = {SCHEMA_2_BOOTSTRAP_PATH}
    changed = 0
    object_bytes = 0
    object_gzip_bytes = 0

    global_object = _store_object(
        output,
        _global_object(timeline, agent_bounds, rollup_bounds),
        precompress=precompress,
    )
    changed += global_object.changed
    generated_files.update(global_object.generated_files)
    object_bytes += global_object.bytes
    object_gzip_bytes += global_object.gzip_bytes or global_object.bytes

    phase_index = _store_object(
        output,
        _phase_index_object(timeline, phase_bounds),
        precompress=precompress,
    )
    changed += phase_index.changed
    generated_files.update(phase_index.generated_files)
    object_bytes += phase_index.bytes
    object_gzip_bytes += phase_index.gzip_bytes or phase_index.bytes

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
        "phase_index": phase_index.catalog_obj(),
        "detail_shards": shard_catalog,
        "search": {
            "strategy": "load-all-detail-shards-on-first-query",
            "fields": ["agent", "phase", "edge"],
        },
    }
    current_objects = frozenset(
        relative for relative in generated_files if _is_object_file(relative)
    )
    generation_changed, retained_objects = _finish_object_generation(
        output, previous_objects, current_objects, current_scope
    )
    changed += generation_changed
    generated_files.add(_OBJECT_MANIFEST_PATH)
    generated_files.update(retained_objects)

    # Publication point: every immutable URL above and the internal reachability manifest exist
    # before the stable browser bootstrap begins referring to the new generation.
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
