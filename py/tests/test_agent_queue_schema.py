"""Python mirror validation against the generated agent-queue schemas.

`jsonschema` is not vendored in this environment, so these tests use a
minimal in-repo validator for the subset of JSON Schema the generated
schemas use. Test-only: no runtime Python code gains a dependency.
The schemas are generated from `rs/agent-queue/src/schema.rs`
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
    if isinstance(instance, list) and "items" in schema:
        for value in instance:
            errors.extend(validate(value, cast(dict[str, object], schema["items"]), root))
    return errors


def load(stem: str) -> dict[str, object]:
    return cast(dict[str, object], json.loads((SCHEMAS / f"{stem}.schema.json").read_text(encoding="utf-8")))


def test_generated_schemas_exist_for_each_format() -> None:
    for stem in ("queue-item", "tombstone", "quarantine-sidecar"):
        assert (SCHEMAS / f"{stem}.schema.json").is_file()


def test_python_mirror_item_validates_against_rust_schema() -> None:
    schema = load("queue-item")
    item = {
        "id": "m1",
        "text": "do the thing",
        "queued_at": 1700000000.5,
        "delivery_attempts": 0,
        "sender_specific": {"kept": True},
    }
    assert validate(item, schema, schema) == []


def test_empty_text_and_wrong_types_are_refused() -> None:
    schema = load("queue-item")
    assert validate({"text": ""}, schema, schema)
    assert validate({"queued_at": 1.0}, schema, schema)
    assert validate({"text": "x", "delivery_attempts": -1}, schema, schema)
    assert validate({"text": "x", "delivery_state": "bogus"}, schema, schema)


def test_lane_and_cancelled_items_validate() -> None:
    schema = load("queue-item")
    v2 = {"text": "x", "not_after": None, "misroutes": [{"at": 1.0, "error": "wrong pane"}]}
    assert validate(v2, schema, schema) == []
    cancelled = {
        "text": "x",
        "delivery_state": "cancelled",
        "cancel_reason": "expired",
        "cancel_detail": "too old",
        "cancelled_at": 3.0,
    }
    assert validate(cancelled, schema, schema) == []
    assert validate({"text": "x", "misroutes": [{"at": 1.0}]}, schema, schema)
    # #1156 queue-supersede-key: keyed and superseded items, as both editions write them.
    keyed = {"text": "x", "supersede_key": "status-request:lane:abc"}
    assert validate(keyed, schema, schema) == []
    superseded = {
        "text": "x",
        "supersede_key": "status-request:lane:abc",
        "delivery_state": "cancelled",
        "cancel_reason": "superseded",
        "cancel_detail": "superseded by m2",
        "cancelled_at": 3.0,
        "superseded_by": "m2",
    }
    assert validate(superseded, schema, schema) == []
    assert validate({"text": "x", "supersede_key": 7}, schema, schema)


def test_tombstone_and_sidecar_validate() -> None:
    tombstone = load("tombstone")
    assert validate(
        {"id": "m1", "reason": "superseded", "cancelled_at": 1.0},
        tombstone,
        tombstone,
    ) == []
    assert validate({"id": "m1"}, tombstone, tombstone)
    sidecar = load("quarantine-sidecar")
    assert validate(
        {
            "artifact": "m1.json",
            "outcome": "possibly_submitted",
            "error": "crashed",
            "failed_at": 2.0,
        },
        sidecar,
        sidecar,
    ) == []
    assert validate(
        {"artifact": "m1.json", "outcome": "bogus", "error": "x", "failed_at": 2.0},
        sidecar,
        sidecar,
    )
