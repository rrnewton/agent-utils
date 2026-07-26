"""Canonical JSON and YAML (de)serialization for a :class:`~safe_ci_dag_runner.model.DagConfig`.

This is the on-disk / interchange form the CLI loads via ``--dag FILE`` and the shared
fixture format for cross-language (Python vs Rust) tests. The schema mirrors the dataclasses
field-for-field. Parsing is STRICT and fails loudly on a malformed document
(:class:`DagJsonError`), never silently defaulting a wrong-typed field.

YAML (:func:`dag_from_yaml` / :func:`dag_to_yaml`) is ISOMORPHIC to the JSON schema: the parsed
YAML object is funneled through the SAME strict narrowing as JSON, so both syntaxes build an
identical model. YAML additionally allows comments and multi-line block scalars (handy for
``description`` fields), which is why "literate" DAGs are written in YAML.

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
import math
import re
from collections.abc import Mapping
from typing import NoReturn

import yaml
from yaml.nodes import ScalarNode

from safe_ci_dag_runner.model import (
    DEFAULT_JOBS_FLAG,
    DEFAULT_STEP_TIMEOUT,
    DagConfig,
    ResourceHint,
    Step,
    StepClass,
)

__all__ = [
    "dag_from_json",
    "dag_from_yaml",
    "dag_to_json",
    "dag_to_yaml",
    "DagJsonError",
]

_DEFAULT_MEM_CAP_FLOOR = 8 * 1024**3


class DagJsonError(ValueError):
    """Raised when a DAG JSON document is malformed."""


# Integer fields map to Rust `i64`, and the Rust build reads them via serde_json's `as_i64`, which
# rejects anything outside this range (including u64 values above i64::MAX). Python ints are
# arbitrary-precision, so bound them here too or a config the Python build accepts would be
# unreadable by the Rust build.
_I64_MIN = -(2**63)
_I64_MAX = 2**63 - 1


def _check_i64(n: int, where: str) -> int:
    if not (_I64_MIN <= n <= _I64_MAX):
        raise DagJsonError(f"{where}: integer {n} does not fit a signed 64-bit range")
    return n


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
    return _check_i64(val, f"field '{key}'")


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
    return _check_i64(val, f"field '{key}'")


def _opt_float(m: Mapping[str, object], key: str, default: float) -> float:
    val = m.get(key, default)
    if isinstance(val, bool) or not isinstance(val, (int, float)):
        raise DagJsonError(f"field '{key}' must be a number")
    result = float(val)
    # Reject NaN / +-inf: the Rust build's serde_json::Value cannot represent a non-finite number
    # (it rejects such input), and a non-finite value re-emits as invalid JSON (`Infinity`/`NaN`),
    # so both builds must refuse it to stay isomorphic and round-trippable.
    if not math.isfinite(result):
        raise DagJsonError(f"field '{key}' must be a finite number")
    return result


def _opt_float_or_none(m: Mapping[str, object], key: str) -> float | None:
    val = m.get(key)
    if val is None:
        return None
    if isinstance(val, bool) or not isinstance(val, (int, float)):
        raise DagJsonError(f"field '{key}' must be a number or null")
    result = float(val)
    if not math.isfinite(result):
        raise DagJsonError(f"field '{key}' must be a finite number or null")
    return result


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
        out[name] = _check_i64(num, f"{where}.{key}.{name}")
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


def _reject_json_constant(token: str) -> NoReturn:
    """Reject JSON's non-standard ``Infinity`` / ``-Infinity`` / ``NaN`` literals.

    Python's ``json`` accepts these by default, but the Rust build's ``serde_json`` rejects them
    ("expected value"), so accepting them here would break byte-for-byte parity and produce
    non-round-trippable output. Wired in as ``json.loads(parse_constant=...)``.
    """
    raise DagJsonError(f"invalid JSON: non-finite float literal {token!r} is not allowed")


def dag_from_json(text: str) -> DagConfig:
    """Parse a DAG JSON document into a :class:`DagConfig`. Raises :class:`DagJsonError`."""
    try:
        raw: object = json.loads(text, parse_constant=_reject_json_constant)
    except json.JSONDecodeError as exc:
        raise DagJsonError(f"invalid JSON: {exc}") from exc
    return _dag_from_obj(raw)


# --- YAML plain-scalar resolution matched to the Rust build (serde_norway / YAML 1.2 core) ---
#
# PyYAML defaults to YAML 1.1 implicit typing, which resolves UNQUOTED scalars differently from
# serde_norway (the Rust build's YAML parser, ~YAML 1.2 core schema). YAML 1.1 turns `no`/`yes`/
# `on`/`off` into booleans (the "Norway problem"), `0755` into octal, `1_000`/`1:20` into numbers,
# and `2024-01-01` into a date — none of which serde_norway does. Left unaddressed, the SAME .yaml
# would load to a different model in one build, or be accepted by one and rejected by the other:
# exactly the desync a two-language runner must never have. So PLAIN (unquoted) scalars are
# resolved here with the SAME core-schema rules serde_norway uses; quoted and block scalars always
# stay strings. cross/differential.py pins this against the Rust build token-for-token.

_RE_INT_DEC = re.compile(r"^[-+]?(0|[1-9][0-9]*)$")
_RE_INT_OCT = re.compile(r"^0o[0-7]+$")
_RE_INT_HEX = re.compile(r"^0x[0-9a-fA-F]+$")
_RE_INT_BIN = re.compile(r"^0b[01]+$")
# A float needs a '.' or an exponent; a bare digit run is an int candidate, and a leading-zero
# decimal like `0755` matches nothing here and stays a STRING — matching serde_norway (which, like
# JSON, rejects leading-zero decimals) rather than YAML 1.1 (which reads it as octal).
_RE_FLOAT = re.compile(
    r"^[-+]?(\.[0-9]+|[0-9]+\.[0-9]*)([eE][-+]?[0-9]+)?$"
    r"|^[-+]?[0-9]+[eE][-+]?[0-9]+$"
)
_NULL_TOKENS = frozenset({"", "~", "null", "Null", "NULL"})
_TRUE_TOKENS = frozenset({"true", "True", "TRUE"})
_FALSE_TOKENS = frozenset({"false", "False", "FALSE"})
# serde_norway recognizes these as non-finite floats, but serde_json::Value cannot hold a
# non-finite number, so it stores null. We mirror that: a non-finite scalar becomes null (not an
# error), so a non-nullable float field rejects it on BOTH sides and a nullable one reads null on
# both — never an accepted infinity.
_NONFINITE_TOKENS = frozenset(
    {
        ".inf", ".Inf", ".INF", "+.inf", "+.Inf", "+.INF", "-.inf", "-.Inf", "-.INF",
        ".nan", ".NaN", ".NAN",
    }
)

_CORE_TAG = "tag:agent-utils,2026:core-scalar"


def _resolve_core_scalar(value: str) -> object:
    """Resolve one PLAIN YAML scalar to the same type serde_norway (YAML 1.2 core) would.

    Returns ``None`` / ``bool`` / ``int`` / ``float`` / ``str``. Leading-zero decimals (``0755``),
    underscore grouping (``1_000``), sexagesimal (``1:20``), and timestamps (``2024-01-01``) stay
    STRINGS; ``0o`` / ``0x`` / ``0b`` integer forms and dotted/exponent floats are parsed; a float
    that overflows to infinity (``1e400``) stays a string and an explicit non-finite token
    (``.inf`` / ``.nan``) becomes null — exactly as serde_norway does.
    """
    if value in _NULL_TOKENS:
        return None
    if value in _TRUE_TOKENS:
        return True
    if value in _FALSE_TOKENS:
        return False
    if _RE_INT_DEC.match(value):
        return int(value, 10)
    if _RE_INT_OCT.match(value):
        return int(value[2:], 8)
    if _RE_INT_HEX.match(value):
        return int(value[2:], 16)
    if _RE_INT_BIN.match(value):
        return int(value[2:], 2)
    if value in _NONFINITE_TOKENS:
        return None
    if _RE_FLOAT.match(value):
        parsed = float(value)
        return parsed if math.isfinite(parsed) else value
    return value


def _construct_core_scalar(loader: yaml.SafeLoader, node: ScalarNode) -> object:
    return _resolve_core_scalar(loader.construct_scalar(node))


class _CoreLoader(yaml.SafeLoader):
    """A ``SafeLoader`` that resolves PLAIN scalars with :func:`_resolve_core_scalar`.

    Every PLAIN (unquoted) scalar is routed to :func:`_construct_core_scalar` via a single
    catch-all implicit resolver (installed below) that REPLACES PyYAML's default YAML-1.1 implicit
    typing. Quoted and block scalars keep the default string tag, so a quoted ``"no"`` stays the
    string ``"no"`` and a plain ``no`` also stays a string — matching serde_norway and never
    resurrecting the Norway problem.
    """


# Replace PyYAML's implicit resolvers with ONE catch-all under the wildcard (``None``) first-char
# key: every plain scalar therefore resolves to ``_CORE_TAG`` and is handed to our core-schema
# constructor, while quoted/block scalars fall through to the default string tag. Assigned directly
# (not via the untyped ``add_implicit_resolver`` classmethod) so this stays mypy-strict clean, and
# only on the subclass so PyYAML's shared ``SafeLoader`` table is left untouched.
_CoreLoader.yaml_implicit_resolvers = {None: [(_CORE_TAG, re.compile(r".*", re.DOTALL))]}
_CoreLoader.add_constructor(_CORE_TAG, _construct_core_scalar)


def dag_from_yaml(text: str) -> DagConfig:
    """Parse a DAG YAML document into a :class:`DagConfig`. Raises :class:`DagJsonError`.

    YAML is ISOMORPHIC to the JSON schema: the parsed object is funneled through the SAME typed
    narrowing (:func:`_dag_from_obj`) that :func:`dag_from_json` uses, AND plain scalars are
    resolved with the SAME YAML-1.2 core-schema rules the Rust build (serde_norway) uses (see
    :class:`_CoreLoader`), so a given ``.yaml`` builds the identical model — or is rejected — in
    both builds. The only surface differences are comments and multi-line block scalars.
    """
    try:
        # yaml.load returns Any; pin it to `object` at the parse boundary so no Any leaks past here
        # (the strict narrowing in _dag_from_obj re-validates every field's type anyway).
        raw: object = yaml.load(text, Loader=_CoreLoader)
    except yaml.YAMLError as exc:
        # Match the Rust build, which returns a load error (exit 2) on malformed YAML rather than
        # letting an exception escape.
        raise DagJsonError(f"invalid YAML: {exc}") from exc
    return _dag_from_obj(raw)


def _dag_from_obj(raw: object) -> DagConfig:
    """Build a :class:`DagConfig` from an already-parsed JSON/YAML object.

    The shared strict narrowing behind both :func:`dag_from_json` and :func:`dag_from_yaml`, so the
    two syntaxes cannot drift in how they construct the model.
    """
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
                description=_opt_str(sm, "description", ""),
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
        description=_opt_str(doc, "description", ""),
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
        "description": step.description,
        "cmd": step.cmd,
        "deps": list(step.deps),
        "env": dict(sorted(step.env.items())),
        "networkonly": step.networkonly,
        "engine_only": step.engine_only,
        "timeout": step.timeout,
        "jobs_flag": step.jobs_flag,
        "hint": _hint_to_json(step.hint),
    }


def _dag_to_obj(cfg: DagConfig) -> dict[str, object]:
    """Build the canonical document dict shared by :func:`dag_to_json` and :func:`dag_to_yaml`.

    A single source of truth for the field set + key order, so the two output formats cannot drift.
    """
    return {
        "description": cfg.description,
        "resource_caps": dict(sorted(cfg.resource_caps.items())),
        "mem_cap_factor": cfg.mem_cap_factor,
        "mem_cap_floor_bytes": cfg.mem_cap_floor_bytes,
        "outer_mem_safety_factor": cfg.outer_mem_safety_factor,
        "default_step_timeout": cfg.default_step_timeout,
        "default_jobs_flag": cfg.default_jobs_flag,
        "steps": [_step_to_json(s) for s in cfg.steps],
    }


def dag_to_json(cfg: DagConfig) -> str:
    """Serialize a :class:`DagConfig` to canonical, deterministic JSON (2-space indent).

    ``ensure_ascii=False`` emits non-ASCII characters as raw UTF-8 (not ``\\uXXXX`` escapes),
    which makes this output BYTE-IDENTICAL to the Rust build's hand-rolled serializer for every
    input — including multi-line, quote/backslash-laden, and unicode descriptions. Both sides
    still escape the JSON control set (``"``, ``\\``, ``\\n``, ``\\t``, ``\\r``, ``\\b``, ``\\f``,
    and ``\\u00XX`` for other code points < 0x20) identically.
    """
    return json.dumps(_dag_to_obj(cfg), indent=2, ensure_ascii=False)


def _represent_core_str(dumper: yaml.SafeDumper, data: str) -> yaml.nodes.ScalarNode:
    """Force-quote any string that :func:`_resolve_core_scalar` would re-read as a NON-string.

    PyYAML's default dumper decides quoting from its YAML-1.1 resolver, so it leaves e.g. ``1e3``
    or ``0o17`` (and the empty string) UNQUOTED — but :class:`_CoreLoader` (YAML-1.2 core) would
    then re-read those as a float / int / null. Quoting exactly those strings keeps
    :func:`dag_to_yaml` output round-trippable back through :func:`dag_from_yaml`.
    """
    style = "'" if not isinstance(_resolve_core_scalar(data), str) else None
    return dumper.represent_scalar("tag:yaml.org,2002:str", data, style=style)


class _CoreDumper(yaml.SafeDumper):
    """SafeDumper whose string representer round-trips through :class:`_CoreLoader`."""


_CoreDumper.add_representer(str, _represent_core_str)


def dag_to_yaml(cfg: DagConfig) -> str:
    """Serialize a :class:`DagConfig` to a YAML document.

    YAML byte-output need NOT match the Rust build (only YAML *loading* is isomorphic across the
    two languages); the emitted document round-trips back through :func:`dag_from_yaml` to an
    identical :class:`DagConfig` — including exotic string values like ``"1e3"`` or ``"0o17"``,
    which :class:`_CoreDumper` force-quotes so the core-schema loader reads them back as strings.
    Built from the same canonical document dict as :func:`dag_to_json`.
    """
    return yaml.dump(
        _dag_to_obj(cfg),
        Dumper=_CoreDumper,
        sort_keys=False,  # preserve our deliberate key order
        allow_unicode=True,  # keep unicode raw rather than \\xNN escapes
        default_flow_style=False,  # block style, human-readable
    )
