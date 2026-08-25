"""Strict JSON and YAML serialization for :class:`DagConfig`.

Both input formats narrow through the same schema and reject malformed or wrong-typed
fields with :class:`DagJsonError`. YAML additionally supports comments and block scalars.
"""

from __future__ import annotations

import json
import math
import re
from collections.abc import Mapping, Sequence
from typing import TYPE_CHECKING, NoReturn

if TYPE_CHECKING:
    # PyYAML is a declared runtime dependency, but it is imported lazily so malformed installations
    # still retain working JSON paths and actionable `--help` / `--version` output. Under
    # `from __future__ import annotations` these type-only imports cost nothing at runtime.
    import yaml
    from yaml.nodes import ScalarNode

from dagrun.model import (
    DEFAULT_JOBS_FLAG,
    DagConfig,
    IntentionalSkipReason,
    ResourceHint,
    Step,
    StepClass,
    WriteDomainGuarantee,
    WriteDomainPolicy,
    write_domain_violations,
    resolve_jobs_env,
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


# Surfaced (as a DagJsonError, so the CLI prints it cleanly rather than dumping a traceback) when a
# YAML entry point is reached but the declared PyYAML dependency is not installed.
_MISSING_YAML_MSG = (
    "reading or writing YAML requires PyYAML, but that package is not installed. "
    "Repair this installation with: python3 -m pip install 'pyyaml>=6'. "
    "The JSON format remains available."
)


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


def _present_str_list(m: Mapping[str, object], key: str) -> list[str] | None:
    """Parse a presence-sensitive string list.

    ``None`` means the key was omitted; an explicit empty list remains ``[]``.
    Write-domain fail-closed policy depends on that distinction.
    """

    if key not in m:
        return None
    val = m[key]
    if not isinstance(val, list):
        raise DagJsonError(f"field '{key}' must be a list of strings")
    out: list[str] = []
    for item in val:
        if not isinstance(item, str):
            raise DagJsonError(f"field '{key}' must contain only strings")
        out.append(item)
    return out


def _write_domain_policy(value: object) -> WriteDomainPolicy:
    if value is None:
        return WriteDomainPolicy()
    obj = _as_obj(value, "write_domain_policy")
    allowed = _opt_str_list(obj, "allowed_domains")
    duplicates = sorted({name for name in allowed if allowed.count(name) > 1})
    if duplicates:
        raise DagJsonError(
            "write_domain_policy.allowed_domains contains duplicates: " + ", ".join(duplicates)
        )
    if any(not name for name in allowed):
        raise DagJsonError("write_domain_policy.allowed_domains must not contain empty names")
    return WriteDomainPolicy(
        require_explicit=_opt_bool(obj, "require_explicit", False),
        allowed_domains=frozenset(allowed),
    )


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


# Cache for the lazily-built loader subclass. PyYAML is imported and the subclass defined only on
# the first YAML load (see _core_loader), so JSON-only paths and `--help` never touch yaml.
_CORE_LOADER: type[yaml.SafeLoader] | None = None


def _core_loader() -> type[yaml.SafeLoader]:
    """Build (once, lazily) the ``SafeLoader`` that resolves PLAIN scalars via core-schema rules.

    Every PLAIN (unquoted) scalar is routed to :func:`_construct_core_scalar` via a single
    catch-all implicit resolver that REPLACES PyYAML's default YAML-1.1 implicit typing. Quoted and
    block scalars keep the default string tag, so a quoted ``"no"`` stays the string ``"no"`` and a
    plain ``no`` also stays a string — matching serde_norway and never resurrecting the Norway
    problem. Defining the subclass here avoids importing PyYAML on JSON-only paths.
    """
    global _CORE_LOADER
    if _CORE_LOADER is not None:
        return _CORE_LOADER
    import yaml

    class _CoreLoader(yaml.SafeLoader):
        pass

    # Replace PyYAML's implicit resolvers with ONE catch-all under the wildcard (``None``)
    # first-char key: every plain scalar therefore resolves to ``_CORE_TAG`` and is handed to our
    # core-schema constructor, while quoted/block scalars fall through to the default string tag.
    # Assigned directly (not via the untyped ``add_implicit_resolver`` classmethod) so this stays
    # mypy-strict clean, and only on the subclass so PyYAML's shared ``SafeLoader`` table is left
    # untouched.
    _CoreLoader.yaml_implicit_resolvers = {None: [(_CORE_TAG, re.compile(r".*", re.DOTALL))]}
    _CoreLoader.add_constructor(_CORE_TAG, _construct_core_scalar)
    _CORE_LOADER = _CoreLoader
    return _CoreLoader


def dag_from_yaml(text: str) -> DagConfig:
    """Parse a DAG YAML document into a :class:`DagConfig`.

    YAML uses the same strict schema as JSON, with YAML 1.2 core scalar resolution.
    Comments and multi-line block scalars are accepted. Malformed input and a missing
    declared YAML dependency raise :class:`DagJsonError`.
    """
    try:
        import yaml
    except ModuleNotFoundError as exc:
        raise DagJsonError(_MISSING_YAML_MSG) from exc
    try:
        # yaml.load returns Any; pin it to `object` at the parse boundary so no Any leaks past here
        # (the strict narrowing in _dag_from_obj re-validates every field's type anyway).
        raw: object = yaml.load(text, Loader=_core_loader())
    except yaml.YAMLError as exc:
        # Match the Rust build, which returns a load error (exit 2) on malformed YAML rather than
        # letting an exception escape.
        raise DagJsonError(f"invalid YAML: {exc}") from exc
    return _dag_from_obj(raw)


#: :class:`DagConfig` fields that the DOCUMENT FORMAT deliberately does not carry.
#:
#: Writing one of these at the top level of a DAG file today has no effect whatsoever: the parser
#: never looks at the key, the field takes its dataclass default, and nothing says so.  That is
#: the dropped-field bug from the reader's side — a configured cap silently replaced by a default,
#: with no report — so the loader REFUSES the key by name instead of ignoring it.
#:
#: Genuinely unknown keys stay tolerated: a key nobody has ever implemented cannot masquerade as a
#: setting that took effect, whereas one that names a real field reads exactly like one that did.
#: ``known_failures`` is listed although only this edition has the field, so both editions refuse
#: the same set of keys byte for byte.
UNCARRIED_CONFIG_KEYS: tuple[str, ...] = (
    "default_step_mem_cap_bytes",
    "default_step_cpu_count",
    "default_step_cpu_timeout",
    "cpu_timeout_multiplier",
    "cpu_timeout_platform",
    "known_failures",
)


def _refuse_uncarried_config_keys(doc: Mapping[str, object]) -> None:
    """Raise when the document sets a real config field the format cannot carry."""
    present = [key for key in UNCARRIED_CONFIG_KEYS if key in doc]
    if not present:
        return
    raise DagJsonError(
        f"<root>: {len(present)} top-level key(s) name a DagConfig field the DAG document "
        "format does not carry, so the value would be SILENTLY replaced by a default: "
        + ", ".join(present)
        + ". Set these on the DagConfig at the call site (they are caller/platform policy, not "
        "properties of the graph), or remove them."
    )


def _refuse_unusable_explains(steps: Sequence[Step]) -> None:
    """Refuse an ``explains`` declaration that cannot mean what it says.

    ``explains`` buys a step an exemption from eager-exit cancellation, so a declaration that is
    quietly wrong is worse than one that is missing: the step looks protected in the document and
    is reaped anyway, or protects itself for a reason nobody can audit. Each refusal below is a
    way that can happen.

    UNKNOWN TAG -- names a node that does not exist, so the exemption can never trigger. This is
    the misspelling case, and it is silent without a check: the field parses, the step schedules
    normally, and the protection simply never applies.

    SELF-REFERENCE -- a step cannot explain its own failure. Permitting it would let any step opt
    itself out of cancellation with a single self-naming line, which is precisely the blanket
    opt-out this relationship exists to avoid.

    CYCLE -- A explains B and B explains A (directly or through a chain). Each would then shield
    the other, so the pair becomes mutually uncancellable and eager-exit silently stops applying
    to it. Nothing reports that; the run just gets slower and the pact is invisible in the
    document because each individual line looks reasonable. Refusing cycles keeps the relation a
    strict "diagnoses" hierarchy, which is the only reading under which the exemption is bounded.
    """
    by_tag = {step.tag: step for step in steps}
    problems: list[str] = []
    for step in steps:
        for target in step.explains:
            if target == step.tag:
                problems.append(f"step {step.tag}: explains itself; a step cannot diagnose its own failure")
            elif target not in by_tag:
                problems.append(f"step {step.tag}: explains unknown node {target!r}")
    if problems:
        raise DagJsonError("; ".join(sorted(set(problems))))

    # Iterative three-colour DFS over the explains relation. Iterative rather than recursive to
    # match how this repository walks graphs elsewhere, so a deep chain cannot hit the recursion
    # limit and turn a validation error into a crash.
    WHITE, GREY, BLACK = 0, 1, 2
    colour = {tag: WHITE for tag in by_tag}
    for root in sorted(by_tag):
        if colour[root] != WHITE:
            continue
        stack: list[tuple[str, bool]] = [(root, False)]
        path: list[str] = []
        while stack:
            tag, leaving = stack.pop()
            if leaving:
                colour[tag] = BLACK
                path.pop()
                continue
            if colour[tag] == BLACK:
                continue
            if colour[tag] == GREY:
                cycle = path[path.index(tag):] + [tag]
                raise DagJsonError(
                    "explains cycle: " + " -> ".join(cycle) + ". Each step in the cycle would "
                    "exempt the next from eager-exit, so the whole cycle becomes uncancellable "
                    "and eager-exit silently stops applying to it. `explains` must describe a "
                    "one-way 'diagnoses' relation."
                )
            colour[tag] = GREY
            path.append(tag)
            stack.append((tag, True))
            for target in sorted(by_tag[tag].explains):
                if colour.get(target) != BLACK:
                    stack.append((target, False))


def _dag_from_obj(raw: object) -> DagConfig:
    """Build a :class:`DagConfig` from an already-parsed JSON/YAML object.

    The shared strict narrowing behind both :func:`dag_from_json` and :func:`dag_from_yaml`, so the
    two syntaxes cannot drift in how they construct the model.
    """
    doc = _as_obj(raw, "<root>")
    _refuse_uncarried_config_keys(doc)
    # The document-level default_step_timeout is the per-step default for any step that omits
    # its own `timeout`. Parse it BEFORE the step loop so it can be threaded in as each step's
    # default. ABSENT IS NOT 1800: an omitted default leaves both it and the step at the 0
    # sentinel, and `resolved_wall_timeout` derives the bound from the step's declared CPU budget
    # (or falls back to DEFAULT_STEP_TIMEOUT). Materializing 1800 here is what baked the
    # load-sensitive number into every graph.
    default_step_timeout = _opt_int(doc, "default_step_timeout", 0)
    steps_raw = doc.get("steps")
    if not isinstance(steps_raw, list):
        raise DagJsonError("<root>: 'steps' must be a list")
    policy = _write_domain_policy(doc.get("write_domain_policy"))
    steps: list[Step] = []
    for i, entry in enumerate(steps_raw):
        where = f"steps[{i}]"
        sm = _as_obj(entry, where)
        skip_text = _opt_str_or_none(sm, "skip_reason")
        try:
            skip_reason = IntentionalSkipReason(skip_text) if skip_text is not None else None
        except ValueError as exc:
            raise DagJsonError(
                f"{where}.skip_reason: unknown value {skip_text!r}"
            ) from exc
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
                cpu_timeout=_opt_int(sm, "cpu_timeout", 0),
                jobs_flag=_opt_str_or_none(sm, "jobs_flag"),
                skip_reason=skip_reason,
                explains=_opt_str_list(sm, "explains"),
                write_domains=_present_str_list(sm, "write_domains"),
                write_domain_guarantee=(
                    None
                    if sm.get("write_domain_guarantee") is None
                    else _parse_write_domain_guarantee(sm, where)
                ),
            )
        )
    intentional = {step.tag for step in steps if step.skip_reason is not None}
    for step in steps:
        blocked = sorted(set(step.deps) & intentional)
        if blocked:
            raise DagJsonError(
                f"step {step.tag}: dependency on intentionally skipped node(s) "
                f"{', '.join(blocked)} is undefined"
            )
    _refuse_unusable_explains(steps)
    cfg = DagConfig(
        steps=tuple(steps),
        description=_opt_str(doc, "description", ""),
        resource_caps=_opt_str_int_map(doc, "resource_caps", "<root>"),
        mem_cap_factor=_opt_float(doc, "mem_cap_factor", 1.25),
        mem_cap_floor_bytes=_opt_int(doc, "mem_cap_floor_bytes", _DEFAULT_MEM_CAP_FLOOR),
        outer_mem_safety_factor=_opt_float(doc, "outer_mem_safety_factor", 1.0),
        default_step_timeout=default_step_timeout,
        default_jobs_flag=_opt_str(doc, "default_jobs_flag", DEFAULT_JOBS_FLAG),
        # HOST-supplied, never read from the document: which env channel this machine
        # delivers inner width through. A graph must not be able to set it, because that is
        # exactly the per-host setting that does not belong in a description of the work.
        default_jobs_env=_opt_str(doc, "default_jobs_env", resolve_jobs_env()),
        write_domain_policy=policy,
    )
    violations = write_domain_violations(cfg)
    if violations:
        raise DagJsonError("write-domain policy refused DAG before execution: " + "; ".join(violations))
    return cfg


def _parse_write_domain_guarantee(
    step: Mapping[str, object], where: str
) -> WriteDomainGuarantee:
    raw = _req_str(step, "write_domain_guarantee", where)
    try:
        return WriteDomainGuarantee(raw)
    except ValueError as exc:
        raise DagJsonError(f"{where}.write_domain_guarantee: unknown value {raw!r}") from exc


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
    obj: dict[str, object] = {
        "group": step.group,
        "job": step.job,
        "desc": step.desc,
        "description": step.description,
        "cmd": step.cmd,
        "deps": list(step.deps),
        "env": dict(sorted(step.env.items())),
        "networkonly": step.networkonly,
        "engine_only": step.engine_only,
        # Both timeout fields are emitted only when SET. 0 is the "derive it" sentinel, and
        # writing it out would read as "no wall bound" — the opposite of what it means.
        "timeout": step.timeout,
        # Emitted only when set, so existing DAGs (all cpu_timeout=0) stay byte-for-byte
        # unchanged; absence parses back to 0, keeping round-trip stable. Positioned
        # immediately after `timeout` to match the Rust serializer's key order.
        "cpu_timeout": step.cpu_timeout,
        "jobs_flag": step.jobs_flag,
        "skip_reason": step.skip_reason.value if step.skip_reason is not None else None,
        "hint": _hint_to_json(step.hint),
    }
    if step.timeout == 0:
        del obj["timeout"]
    if step.cpu_timeout == 0:
        del obj["cpu_timeout"]
    if step.skip_reason is None:
        del obj["skip_reason"]
    # Emitted only when declared, like write_domains below: a graph that does not use the
    # relationship keeps a byte-identical document, so adding the field cannot churn every
    # existing DAG or the cross-build byte-comparison that pins the two editions together.
    if step.explains:
        obj["explains"] = list(step.explains)
    if step.write_domains is not None:
        obj["write_domains"] = list(step.write_domains)
    if step.write_domain_guarantee is not None:
        obj["write_domain_guarantee"] = step.write_domain_guarantee.value
    return obj


def _dag_to_obj(cfg: DagConfig) -> dict[str, object]:
    """Build the canonical document dict shared by :func:`dag_to_json` and :func:`dag_to_yaml`.

    A single source of truth for the field set + key order, so the two output formats cannot drift.
    """
    obj: dict[str, object] = {
        "description": cfg.description,
        "resource_caps": dict(sorted(cfg.resource_caps.items())),
        "mem_cap_factor": cfg.mem_cap_factor,
        "mem_cap_floor_bytes": cfg.mem_cap_floor_bytes,
        "outer_mem_safety_factor": cfg.outer_mem_safety_factor,
        "default_step_timeout": cfg.default_step_timeout,
        "default_jobs_flag": cfg.default_jobs_flag,
        "default_jobs_env": cfg.default_jobs_env,
        "steps": [_step_to_json(s) for s in cfg.steps],
    }
    if cfg.default_step_timeout == 0:
        del obj["default_step_timeout"]
    policy = cfg.write_domain_policy
    if policy.require_explicit or policy.allowed_domains:
        obj["write_domain_policy"] = {
            "require_explicit": policy.require_explicit,
            "allowed_domains": sorted(policy.allowed_domains),
        }
    return obj


def dag_to_json(cfg: DagConfig) -> str:
    """Serialize a :class:`DagConfig` to deterministic, two-space-indented JSON.

    Non-ASCII characters are emitted as UTF-8, while JSON control characters, quotes,
    and backslashes are escaped according to the JSON specification.
    """
    return json.dumps(_dag_to_obj(cfg), indent=2, ensure_ascii=False)


def _represent_core_str(dumper: yaml.SafeDumper, data: str) -> ScalarNode:
    """Force-quote any string that :func:`_resolve_core_scalar` would re-read as a NON-string.

    PyYAML's default dumper decides quoting from its YAML-1.1 resolver, so it leaves e.g. ``1e3``
    or ``0o17`` (and the empty string) UNQUOTED — but :func:`_core_loader` (YAML-1.2 core) would
    then re-read those as a float / int / null. Quoting exactly those strings keeps
    :func:`dag_to_yaml` output round-trippable back through :func:`dag_from_yaml`.
    """
    style = "'" if not isinstance(_resolve_core_scalar(data), str) else None
    return dumper.represent_scalar("tag:yaml.org,2002:str", data, style=style)


# Cache for the lazily-built dumper subclass; see _core_dumper.
_CORE_DUMPER: type[yaml.SafeDumper] | None = None


def _core_dumper() -> type[yaml.SafeDumper]:
    """Build (once, lazily) the ``SafeDumper`` whose string representer round-trips through
    :func:`_core_loader`. Defined here (not at module scope) to keep PyYAML optional."""
    global _CORE_DUMPER
    if _CORE_DUMPER is not None:
        return _CORE_DUMPER
    import yaml

    class _CoreDumper(yaml.SafeDumper):
        pass

    _CoreDumper.add_representer(str, _represent_core_str)
    _CORE_DUMPER = _CoreDumper
    return _CoreDumper


def dag_to_yaml(cfg: DagConfig) -> str:
    """Serialize a :class:`DagConfig` to round-trippable YAML.

    Ambiguous scalar-looking strings are quoted so the core-schema loader preserves
    them as strings. A missing declared YAML dependency raises :class:`DagJsonError`.
    """
    try:
        import yaml
    except ModuleNotFoundError as exc:
        raise DagJsonError(_MISSING_YAML_MSG) from exc
    return yaml.dump(
        _dag_to_obj(cfg),
        Dumper=_core_dumper(),
        sort_keys=False,  # preserve our deliberate key order
        allow_unicode=True,  # keep unicode raw rather than \\xNN escapes
        default_flow_style=False,  # block style, human-readable
    )
