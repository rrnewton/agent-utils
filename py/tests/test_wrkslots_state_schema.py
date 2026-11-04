"""Python mirror validation against the generated wrkslots schemas.

`jsonschema` is not vendored in this environment, so these tests use a
minimal in-repo validator for the subset of JSON Schema the generated
schemas use. Test-only: no runtime Python code gains a dependency.
The schemas are generated from `rs/wrkslotsd/src/formats.rs`
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
    for stem in (
        "wrkslots-active-state",
        "wrkslots-archived-state",
        "wrkslots-handoff-sidecar",
        "wrkslots-handoff-sidecar-v1",
    ):
        assert (SCHEMAS / f"{stem}.schema.json").is_file()


# Fixtures transcribed from the Python writers (py/wrkslots/cli.py state
# records and _handoff_artifact_to_obj) and the Rust writers, which the
# differential corpus already holds at parity.
ACTIVE: dict[str, object] = {
    "schema": 2,
    "machine": "host1",
    "revision": 3,
    "slots": [
        {
            "slot": "alpha",
            "agent": "worker",
            "task": "task",
            "purpose": "purpose",
            "machine": "host1",
            "generation": 1,
            "created_at": "2026-01-01T00:00:00Z",
            "heartbeat_at": "2026-01-01T00:01:00Z",
            "heartbeat_ttl_seconds": 300,
            "owner": {
                "pid": 10,
                "start_ticks": 20,
                "boot_id": "boot",
                "host_id": "host1",
                "cgroup_path": "/user.slice/x",
            },
            "coordinator_lease": {
                "pid": 11,
                "start_ticks": 21,
                "boot_id": "boot",
                "host_id": "host1",
                "cgroup_path": "/user.slice/y",
            },
            "coordinator_recovery_note": None,
            "handoff": None,
            "checkouts": [
                {
                    "name": "repo",
                    "path": "repo",
                    "repository": "origin",
                    "branch": "main",
                    "start_point": "a" * 40,
                    "remote": "origin",
                    "remote_url_sha256": "b" * 64,
                    "landed_ref": "refs/heads/main",
                    "head": "c" * 40,
                    "containing_remote_refs": ["refs/remotes/origin/main"],
                    "vcs": "git",
                }
            ],
        }
    ],
}


def test_active_state_validates_and_unknown_fields_are_refused() -> None:
    schema = load("wrkslots-active-state")
    assert validate(ACTIVE, schema, schema) == []
    bogus = json.loads(json.dumps(ACTIVE))
    bogus["slots"][0]["surprise"] = 1
    assert validate(bogus, schema, schema)
    wrong = json.loads(json.dumps(ACTIVE))
    wrong["schema"] = 1
    assert validate(wrong, schema, schema)


def test_null_where_only_absence_is_allowed_is_refused() -> None:
    schema = load("wrkslots-active-state")
    for field in ("slot_type", "layout", "task_scope"):
        bogus = json.loads(json.dumps(ACTIVE))
        bogus["slots"][0][field] = None
        assert validate(bogus, schema, schema), field
    bogus = json.loads(json.dumps(ACTIVE))
    bogus["slots"][0]["coordinator_lease"] = None
    assert validate(bogus, schema, schema)
    bogus = json.loads(json.dumps(ACTIVE))
    bogus["slots"][0]["generation"] = 0
    assert validate(bogus, schema, schema)


def test_populated_optional_shapes_validate() -> None:
    schema = load("wrkslots-active-state")
    full = json.loads(json.dumps(ACTIVE))
    record = full["slots"][0]
    record["slot_type"] = "validate"
    record["layout"] = "flat"
    record["task_scope"] = {
        "unit": "wrkslots-validate-x.service",
        "invocation_id": "abc",
        "cgroup_path": "/user.slice/z",
        "boot_id": "boot",
        "leader_pid": 12,
        "leader_start_ticks": 22,
        "verification": "systemd-runtime-invocation-symlink-v1",
    }
    record["import_source"] = {
        "format": "worktree-state-v3",
        "path": "old/state.json",
        "file_sha256": "e" * 64,
        "row_sha256": "f" * 64,
        "status": "active",
        "row": {"status": "active"},
    }
    assert validate(full, schema, schema) == []
    record["checkouts"][0]["vcs"] = "svn"
    assert validate(full, schema, schema)


def test_sidecars_validate_by_version() -> None:
    digest = "d" * 64
    v2 = {
        "schema": 2,
        "machine": "host1",
        "slot": "alpha",
        "generation": 1,
        "recorded_at": "2026-01-01T00:02:00Z",
        "source": "write-handoff",
        "source_path": "/registry/agents/worker/HANDOFF.md",
        "source_identity": {"identity": [1, 2], "size": 5, "sha256": digest},
        "contents_utf8": "hello",
        "sha256": digest,
    }
    schema = load("wrkslots-handoff-sidecar")
    assert validate(v2, schema, schema) == []
    v1 = {k: v for k, v in v2.items() if k != "source_path"}
    v1["schema"] = 1
    legacy = load("wrkslots-handoff-sidecar-v1")
    assert validate(v1, legacy, legacy) == []
    assert validate(v1, schema, schema)


def test_archived_state_validates() -> None:
    schema = load("wrkslots-archived-state")
    record = cast(dict[str, object], json.loads(json.dumps(cast(list[object], ACTIVE["slots"])[0])))
    archived = {
        "schema": 2,
        "machine": "host1",
        "revision": 1,
        "records": [
            {
                "archive_id": "host1:alpha:1:2026-01-02T00:00:00Z",
                "slot": record["slot"],
                "agent": record["agent"],
                "task": record["task"],
                "purpose": record["purpose"],
                "machine": "host1",
                "generation": 1,
                "created_at": record["created_at"],
                "finished_at": "2026-01-02T00:00:00Z",
                "mode": "remove",
                "actor": "coordinator",
                "physical_storage": "removed",
                "validation": ["checked"],
                "limitations": [],
                "continuation": "none",
                "checkouts": record["checkouts"],
            }
        ],
    }
    assert validate(archived, schema, schema) == []
