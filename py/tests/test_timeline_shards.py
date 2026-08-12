from __future__ import annotations

import gzip
import hashlib
from datetime import datetime, timezone
from pathlib import Path

from agent_team_timeline.archive import JsonValue, as_array, as_object, read_json
from agent_team_timeline.timeline_shards import (
    SCHEMA_2_BOOTSTRAP_PATH,
    write_timeline_shards,
)


def _ms(value: str) -> int:
    return int(datetime.fromisoformat(value).astimezone(timezone.utc).timestamp() * 1000)


def _timeline() -> dict[str, JsonValue]:
    start = _ms("2026-08-11T23:30:00+00:00")
    midnight = _ms("2026-08-12T00:00:00+00:00")
    end = _ms("2026-08-12T00:30:00+00:00")
    return {
        "schema_version": 1,
        "generated_at": "2026-08-12T00:30:00Z",
        "source_digest": "source-digest",
        "display_timezone": "America/New_York",
        "display_timezone_source": "explicit",
        "range": {"start_ms": start, "end_ms": end},
        "teams": [{"slug": "test-team", "label": "Test team"}],
        "agents": [
            {
                "id": "root",
                "team": "test-team",
                "parent_id": None,
                "start_ms": start,
                "end_ms": end,
            }
        ],
        "phases": [
            {
                "id": "phase-spanning-midnight",
                "agent_id": "root",
                "start_ms": start,
                "end_ms": end,
                "states": [],
            },
            {
                "id": "phase-ending-at-midnight",
                "agent_id": "root",
                "start_ms": start,
                "end_ms": midnight,
                "states": [],
            },
        ],
        "activity_bins": [
            {
                "team": "test-team",
                "role": "coordinator",
                "resolution": "hourly",
                "start_ms": start,
                "end_ms": end,
                "avg_active_concurrency": 1.0,
            }
        ],
        "edges": [
            {
                "id": "edge-spanning-midnight",
                "source_id": "root",
                "target_id": "root",
                "source_ms": start,
                "target_ms": end,
            },
            {
                "id": "edge-ending-at-midnight",
                "source_id": "root",
                "target_id": "root",
                "source_ms": start,
                "target_ms": midnight,
            },
            {
                "id": "structural-result",
                "kind": "result",
                "source_id": "root",
                "target_id": "root",
                "source_ms": midnight,
                "target_ms": midnight,
            }
        ],
        "events": [
            {"agent_id": "root", "at_ms": start, "kind": "user_prompt"},
            {"agent_id": "root", "at_ms": midnight, "kind": "tool_call"},
        ],
        "rollups": [],
        "glossary": [],
        "summary_files": [],
        "artifact_catalog_path": "data/artifacts.json",
        "project_overview": {"text": "Test project. " * 500},
    }


def _object(root: Path, reference: dict[str, JsonValue]) -> dict[str, JsonValue]:
    url = reference["url"]
    digest = reference["sha256"]
    assert isinstance(url, str)
    assert isinstance(digest, str)
    path = root / url
    assert path.name == f"{digest}.json"
    assert hashlib.sha256(path.read_bytes()).hexdigest() == digest
    if (path.with_name(path.name + ".gz")).is_file():
        assert gzip.decompress(path.with_name(path.name + ".gz").read_bytes()) == (
            path.read_bytes()
        )
    return as_object(read_json(path), str(path))


def test_schema_2_projection_is_content_addressed_range_sharded_and_idempotent(
    tmp_path: Path,
) -> None:
    schema_1 = tmp_path / "data" / "timeline.json"
    schema_1.parent.mkdir(parents=True)
    schema_1.write_text("schema-one-is-owned-by-the-caller\n", encoding="utf-8")

    first = write_timeline_shards(tmp_path, _timeline())
    assert first.files_changed > 0
    assert first.detail_shards == 2
    assert first.bootstrap_bytes < first.object_bytes
    assert first.bootstrap_gzip_bytes is not None
    assert first.bootstrap_gzip_bytes < first.bootstrap_bytes
    assert first.object_gzip_bytes < first.object_bytes
    assert schema_1.read_text(encoding="utf-8") == "schema-one-is-owned-by-the-caller\n"

    bootstrap_path = tmp_path / SCHEMA_2_BOOTSTRAP_PATH
    bootstrap = as_object(read_json(bootstrap_path), str(bootstrap_path))
    assert bootstrap["schema_version"] == 2
    assert bootstrap["kind"] == "timeline-bootstrap"
    assert bootstrap["teams"] == [{"label": "Test team", "slug": "test-team"}]
    assert len(as_array(bootstrap["activity_bins"], "activity bins")) == 1
    assert bootstrap["search"] == {
        "fields": ["agent", "phase", "edge"],
        "strategy": "load-all-detail-shards-on-first-query",
    }

    global_reference = as_object(bootstrap["global"], "global reference")
    global_object = _object(tmp_path, global_reference)
    assert global_object["kind"] == "timeline-global"
    assert len(as_array(global_object["agents"], "global agents")) == 1
    assert [
        as_object(value, "structural edge")["id"]
        for value in as_array(global_object["edges"], "global edges")
    ] == ["structural-result"]
    assert "phases" not in global_object
    assert "events" not in global_object
    assert "activity_bins" not in global_object

    catalog = [
        as_object(value, "detail shard")
        for value in as_array(bootstrap["detail_shards"], "detail shards")
    ]
    assert [value["day"] for value in catalog] == ["2026-08-11", "2026-08-12"]
    first_day = _object(tmp_path, catalog[0])
    second_day = _object(tmp_path, catalog[1])
    assert [
        as_object(value, "phase")["id"]
        for value in as_array(first_day["phases"], "first phases")
    ] == ["phase-spanning-midnight", "phase-ending-at-midnight"]
    assert [
        as_object(value, "phase")["id"]
        for value in as_array(second_day["phases"], "second phases")
    ] == ["phase-spanning-midnight"]
    assert len(as_array(first_day["edges"], "first edges")) == 2
    assert len(as_array(second_day["edges"], "second edges")) == 2
    assert len(as_array(first_day["events"], "first events")) == 1
    assert len(as_array(second_day["events"], "second events")) == 1

    assert bootstrap_path.stat().st_mtime_ns >= max(
        (tmp_path / relative).stat().st_mtime_ns
        for relative in first.generated_files
        if relative != SCHEMA_2_BOOTSTRAP_PATH and not relative.endswith(".gz")
    )
    second = write_timeline_shards(tmp_path, _timeline())
    assert second.files_changed == 0
    assert second.generated_files == first.generated_files


def test_schema_2_projection_can_skip_sidecars_for_temporary_builds(
    tmp_path: Path,
) -> None:
    report = write_timeline_shards(tmp_path, _timeline(), precompress=False)

    assert all(not path.endswith(".gz") for path in report.generated_files)
    assert report.object_gzip_bytes == report.object_bytes
