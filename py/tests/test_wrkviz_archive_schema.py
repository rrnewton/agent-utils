"""Python mirror validation against the generated wrkviz schemas.

`jsonschema` is not vendored in this environment, so these tests use a
minimal in-repo validator for the subset of JSON Schema the generated
schemas use. Test-only: no runtime Python code gains a dependency.
The schemas are generated from `rs/wrkviz/src/formats.rs`
(#723 rust-primary-schema-collapse); Python never redefines the fields.
"""

from __future__ import annotations

import json
from pathlib import Path
from typing import cast

ROOT = Path(__file__).resolve().parents[2]
SCHEMAS = ROOT / "validation" / "schemas"

_TYPE_CHECKS = {
    "string": lambda v: isinstance(v, str),
    "integer": lambda v: isinstance(v, int) and not isinstance(v, bool),
    "number": lambda v: isinstance(v, (int, float)) and not isinstance(v, bool),
    "boolean": lambda v: isinstance(v, bool),
    "object": lambda v: isinstance(v, dict),
    "array": lambda v: isinstance(v, list),
    "null": lambda v: v is None,
}


def _resolve(schema: dict[str, object], root: dict[str, object]) -> dict[str, object]:
    ref = schema.get("$ref")
    if isinstance(ref, str) and ref.startswith("#/$defs/"):
        defs = cast(dict[str, object], root["$defs"])
        return cast(dict[str, object], defs[ref.removeprefix("#/$defs/")])
    return schema


def validate(instance: object, schema: dict[str, object], root: dict[str, object]) -> list[str]:
    schema = _resolve(schema, root)
    errors: list[str] = []
    if "const" in schema and instance != schema["const"]:
        errors.append(f"{instance!r} != const {schema['const']!r}")
    for key in ("anyOf", "oneOf"):
        if key in schema:
            branches = [validate(instance, cast(dict[str, object], branch), root) for branch in cast(list[object], schema[key])]
            if not any(not branch for branch in branches):
                errors.append(f"no {key} branch matched {instance!r}")
            return errors
    expected = schema.get("type")
    if expected is not None:
        types = expected if isinstance(expected, list) else [expected]
        if not any(_TYPE_CHECKS[t](instance) for t in cast(list[str], types)):
            return [f"{instance!r} is not of type {expected}"]
    if isinstance(instance, str) and "minLength" in schema:
        if len(instance) < cast(int, schema["minLength"]):
            errors.append("string shorter than minLength")
    if isinstance(instance, str) and "maxLength" in schema:
        if len(instance) > cast(int, schema["maxLength"]):
            errors.append("string longer than maxLength")
    if isinstance(instance, list):
        if "minItems" in schema and len(instance) < cast(int, schema["minItems"]):
            errors.append("fewer than minItems")
        if "maxItems" in schema and len(instance) > cast(int, schema["maxItems"]):
            errors.append("more than maxItems")
    if isinstance(instance, (int, float)) and "minimum" in schema:
        if instance < cast(float, schema["minimum"]):
            errors.append("below minimum")
    if isinstance(instance, dict):
        for name in cast(list[object], schema.get("required", [])):
            if name not in instance:
                errors.append(f"missing required {name}")
        properties = cast(dict[str, object], schema.get("properties", {}))
        for name, value in instance.items():
            if name in properties:
                errors.extend(validate(value, cast(dict[str, object], properties[name]), root))
            elif schema.get("additionalProperties") is False:
                errors.append(f"additional property {name}")
            elif isinstance(schema.get("additionalProperties"), dict):
                errors.extend(
                    validate(value, cast(dict[str, object], schema["additionalProperties"]), root)
                )
    if isinstance(instance, list) and "items" in schema:
        for value in instance:
            errors.extend(validate(value, cast(dict[str, object], schema["items"]), root))
    return errors


def load(stem: str) -> dict[str, object]:
    return cast(dict[str, object], json.loads((SCHEMAS / f"{stem}.schema.json").read_text(encoding="utf-8")))


def _shard(stream: str) -> dict[str, object]:
    return {
        "stream": stream,
        "team": "alpha",
        "day": "2026-01-01",
        "path": f"data/timeline-v3/{stream}/alpha/2026-01-01.jsonl.gz",
        "index_path": f"data/timeline-v3/{stream}/alpha/2026-01-01.jsonl.gz.index.jsonl",
        "records": 2,
        "members": 1,
        "c_bytes": 100,
        "u_bytes": 200,
        "c_sha256": "a" * 64,
        "u_sha256": "b" * 64,
        "timestamps_sorted": True,
        "t0": 1000,
        "t1": 2000,
        "t_end_exclusive": 3000,
        "line_ranges": {"event": [0, 2]},
    }


def _section(stream: str) -> dict[str, object]:
    return {
        "addressing": "per-team-utc-day",
        "sort": "at_ms, record_kind, encoded line",
        "record_kinds": ["event"],
        "shards": [_shard(stream)],
    }


BOOTSTRAP: dict[str, object] = {
    "schema_version": 3,
    "kind": "timeline-v3-bootstrap",
    "codec": {
        "container": "multi-member-gzip",
        "level": 6,
        "target_chunk_bytes": 1048576,
        "timestamp_key": "at_ms",
        "record_kind_key": "record_kind",
        "index_suffix": ".index.jsonl",
    },
    "teams": [{"slug": "alpha"}],
    "streams": {
        "timeline": _section("timeline"),
        "spine": _section("spine"),
        "bins": _section("bins"),
    },
}


def test_bootstrap_validates_and_refuses_drift() -> None:
    schema = load("wrkviz-timeline-v3-bootstrap")
    assert validate(BOOTSTRAP, schema, schema) == []
    wrong = json.loads(json.dumps(BOOTSTRAP))
    wrong["schema_version"] = 2
    assert validate(wrong, schema, schema)
    wrong = json.loads(json.dumps(BOOTSTRAP))
    wrong["codec"]["container"] = "single-gzip"
    assert validate(wrong, schema, schema)
    wrong = json.loads(json.dumps(BOOTSTRAP))
    wrong["streams"]["timeline"]["shards"][0]["surprise"] = 1
    assert validate(wrong, schema, schema)
    wrong = json.loads(json.dumps(BOOTSTRAP))
    wrong["streams"]["timeline"]["shards"][0]["records"] = -1
    assert validate(wrong, schema, schema)
    wrong = json.loads(json.dumps(BOOTSTRAP))
    wrong["teams"] = []
    assert validate(wrong, schema, schema)
    wrong = json.loads(json.dumps(BOOTSTRAP))
    wrong["streams"]["timeline"]["shards"][0]["line_ranges"] = {"event": [-1, 2]}
    assert validate(wrong, schema, schema)
    wrong = json.loads(json.dumps(BOOTSTRAP))
    wrong["streams"]["search"] = None
    assert validate(wrong, schema, schema)
