"""Canonical JSON (de)serialization for a :class:`~safe_ci_dag_runner.model.DagConfig`.

This is the on-disk / interchange form the CLI loads via ``--dag FILE`` and the shared
fixture format for cross-language (Python vs Rust) tests. The schema mirrors the dataclasses
field-for-field. Parsing is STRICT and fails loudly on a malformed document
(:class:`DagJsonError`), never silently defaulting a wrong-typed field.

Example document::

    {
      "resource_caps": {"browser": 2},
      "mem_cap_factor": 1.25,
      "steps": [
        {"group": "build", "job": "app", "desc": "build", "cmd": "make build",
         "hint": {"est_duration_s": 90, "classification": "cpu-bound"}},
        {"group": "test", "job": "unit", "desc": "tests", "cmd": "make test",
         "deps": ["build.app"]}
      ]
    }
"""

from __future__ import annotations

import json
from collections.abc import Mapping

from safe_ci_dag_runner.model import (
    DEFAULT_JOBS_FLAG,
    DEFAULT_STEP_TIMEOUT,
    DagConfig,
    ResourceHint,
    Step,
    StepClass,
)

__all__ = ["dag_from_json", "dag_to_json", "DagJsonError"]

_DEFAULT_MEM_CAP_FLOOR = 8 * 1024**3


class DagJsonError(ValueError):
    """Raised when a DAG JSON document is malformed."""


# --- typed narrowing helpers (json.loads yields Any; narrow explicitly, no Any leaks) ---


def _as_obj(value: object, where: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise DagJsonError(f"{where}: expected an object, got {type(value).__name__}")
    out: dict[str, object] = {}
    for key, val in value.items():
        out[str(key)] = val
    return out


def _req_str(m: Mapping[str, object], key: str, where: str) -> str:
    val = m.get(key)
    if not isinstance(val, str):
        raise DagJsonError(f"{where}: field '{key}' must be a string")
    return val


def _opt_str(m: Mapping[str, object], key: str, default: str) -> str:
    val = m.get(key, default)
    if not isinstance(val, str):
        raise DagJsonError(f"field '{key}' must be a string")
    return val


def _opt_int(m: Mapping[str, object], key: str, default: int) -> int:
    val = m.get(key, default)
    if isinstance(val, bool) or not isinstance(val, int):
        raise DagJsonError(f"field '{key}' must be an integer")
    return val


def _opt_str_or_none(m: Mapping[str, object], key: str) -> str | None:
    val = m.get(key)
    if val is None:
        return None
    if not isinstance(val, str):
        raise DagJsonError(f"field '{key}' must be a string or null")
    return val


def _opt_int_or_none(m: Mapping[str, object], key: str) -> int | None:
    val = m.get(key)
    if val is None:
        return None
    if isinstance(val, bool) or not isinstance(val, int):
        raise DagJsonError(f"field '{key}' must be an integer or null")
    return val


def _opt_float(m: Mapping[str, object], key: str, default: float) -> float:
    val = m.get(key, default)
    if isinstance(val, bool) or not isinstance(val, (int, float)):
        raise DagJsonError(f"field '{key}' must be a number")
    return float(val)


def _opt_float_or_none(m: Mapping[str, object], key: str) -> float | None:
    val = m.get(key)
    if val is None:
        return None
    if isinstance(val, bool) or not isinstance(val, (int, float)):
        raise DagJsonError(f"field '{key}' must be a number or null")
    return float(val)


def _opt_bool(m: Mapping[str, object], key: str, default: bool) -> bool:
    val = m.get(key, default)
    if not isinstance(val, bool):
        raise DagJsonError(f"field '{key}' must be a boolean")
    return val


def _opt_str_list(m: Mapping[str, object], key: str) -> list[str]:
    val = m.get(key)
    if val is None:
        return []
    if not isinstance(val, list):
        raise DagJsonError(f"field '{key}' must be a list of strings")
    out: list[str] = []
    for item in val:
        if not isinstance(item, str):
            raise DagJsonError(f"field '{key}' must contain only strings")
        out.append(item)
    return out


def _opt_str_int_map(m: Mapping[str, object], key: str, where: str) -> dict[str, int]:
    val = m.get(key)
    if val is None:
        return {}
    obj = _as_obj(val, f"{where}.{key}")
    out: dict[str, int] = {}
    for name, num in obj.items():
        if isinstance(num, bool) or not isinstance(num, int):
            raise DagJsonError(f"{where}.{key}.{name}: must be an integer")
        out[name] = num
    return out


def _opt_str_str_map(m: Mapping[str, object], key: str, where: str) -> dict[str, str]:
    val = m.get(key)
    if val is None:
        return {}
    obj = _as_obj(val, f"{where}.{key}")
    out: dict[str, str] = {}
    for name, text in obj.items():
        if not isinstance(text, str):
            raise DagJsonError(f"{where}.{key}.{name}: must be a string")
        out[name] = text
    return out


def _hint_from(value: object, where: str) -> ResourceHint:
    if value is None:
        return ResourceHint()
    obj = _as_obj(value, where)
    cls_name = _opt_str(obj, "classification", StepClass.LIGHT.value)
    try:
        classification = StepClass(cls_name)
    except ValueError as exc:
        raise DagJsonError(f"{where}.classification: unknown value {cls_name!r}") from exc
    return ResourceHint(
        resources=_opt_str_int_map(obj, "resources", where),
        est_duration_s=_opt_float(obj, "est_duration_s", 0.0),
        rss_baseline_bytes=_opt_int_or_none(obj, "rss_baseline_bytes"),
        hard_mem_max_bytes=_opt_int_or_none(obj, "hard_mem_max_bytes"),
        classification=classification,
        preferred_inner_jobs=_opt_int_or_none(obj, "preferred_inner_jobs"),
        measured_effective_cores=_opt_float_or_none(obj, "measured_effective_cores"),
        measured_cpu_utilization=_opt_float_or_none(obj, "measured_cpu_utilization"),
    )


def dag_from_json(text: str) -> DagConfig:
    """Parse a DAG JSON document into a :class:`DagConfig`. Raises :class:`DagJsonError`."""
    try:
        raw: object = json.loads(text)
    except json.JSONDecodeError as exc:
        raise DagJsonError(f"invalid JSON: {exc}") from exc
    doc = _as_obj(raw, "<root>")
    # The document-level default_step_timeout is the per-step default for any step that omits
    # its own `timeout` (falling back to the module constant only when the document omits it
    # too). Parse it BEFORE the step loop so it can be threaded in as each step's default.
    default_step_timeout = _opt_int(doc, "default_step_timeout", DEFAULT_STEP_TIMEOUT)
    steps_raw = doc.get("steps")
    if not isinstance(steps_raw, list):
        raise DagJsonError("<root>: 'steps' must be a list")
    steps: list[Step] = []
    for i, entry in enumerate(steps_raw):
        where = f"steps[{i}]"
        sm = _as_obj(entry, where)
        steps.append(
            Step(
                group=_req_str(sm, "group", where),
                job=_req_str(sm, "job", where),
                desc=_opt_str(sm, "desc", ""),
                cmd=_req_str(sm, "cmd", where),
                deps=_opt_str_list(sm, "deps"),
                env=_opt_str_str_map(sm, "env", where),
                hint=_hint_from(sm.get("hint"), f"{where}.hint"),
                networkonly=_opt_bool(sm, "networkonly", False),
                engine_only=_opt_bool(sm, "engine_only", False),
                timeout=_opt_int(sm, "timeout", default_step_timeout),
                jobs_flag=_opt_str_or_none(sm, "jobs_flag"),
            )
        )
    return DagConfig(
        steps=tuple(steps),
        resource_caps=_opt_str_int_map(doc, "resource_caps", "<root>"),
        mem_cap_factor=_opt_float(doc, "mem_cap_factor", 1.25),
        mem_cap_floor_bytes=_opt_int(doc, "mem_cap_floor_bytes", _DEFAULT_MEM_CAP_FLOOR),
        outer_mem_safety_factor=_opt_float(doc, "outer_mem_safety_factor", 1.0),
        default_step_timeout=default_step_timeout,
        default_jobs_flag=_opt_str(doc, "default_jobs_flag", DEFAULT_JOBS_FLAG),
    )


def _hint_to_json(hint: ResourceHint) -> dict[str, object]:
    return {
        "resources": dict(sorted(hint.resources.items())),
        "est_duration_s": hint.est_duration_s,
        "rss_baseline_bytes": hint.rss_baseline_bytes,
        "hard_mem_max_bytes": hint.hard_mem_max_bytes,
        "classification": hint.classification.value,
        "preferred_inner_jobs": hint.preferred_inner_jobs,
        "measured_effective_cores": hint.measured_effective_cores,
        "measured_cpu_utilization": hint.measured_cpu_utilization,
    }


def _step_to_json(step: Step) -> dict[str, object]:
    return {
        "group": step.group,
        "job": step.job,
        "desc": step.desc,
        "cmd": step.cmd,
        "deps": list(step.deps),
        "env": dict(sorted(step.env.items())),
        "networkonly": step.networkonly,
        "engine_only": step.engine_only,
        "timeout": step.timeout,
        "jobs_flag": step.jobs_flag,
        "hint": _hint_to_json(step.hint),
    }


def dag_to_json(cfg: DagConfig) -> str:
    """Serialize a :class:`DagConfig` to canonical, deterministic JSON (2-space indent)."""
    doc: dict[str, object] = {
        "resource_caps": dict(sorted(cfg.resource_caps.items())),
        "mem_cap_factor": cfg.mem_cap_factor,
        "mem_cap_floor_bytes": cfg.mem_cap_floor_bytes,
        "outer_mem_safety_factor": cfg.outer_mem_safety_factor,
        "default_step_timeout": cfg.default_step_timeout,
        "default_jobs_flag": cfg.default_jobs_flag,
        "steps": [_step_to_json(s) for s in cfg.steps],
    }
    return json.dumps(doc, indent=2)
