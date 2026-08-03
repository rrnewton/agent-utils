"""Canonical JSON and YAML (de)serialization for a :class:`~tick_hub.model.TickConfig`.

This is the on-disk / interchange form the CLI loads via ``--config FILE`` and the shared fixture
format for the (future) cross-language Python-vs-Rust differential. The schema mirrors the
dataclasses field-for-field. Parsing is STRICT and fails loudly on a malformed document
(:class:`TickConfigError`), never silently defaulting a wrong-typed field.

YAML (:func:`config_from_yaml` / :func:`config_to_yaml`) is ISOMORPHIC to the JSON schema: the
parsed YAML object is funneled through the SAME strict narrowing as JSON, so both syntaxes build an
identical model. YAML additionally allows comments and multi-line block scalars (handy for
``description`` fields). Plain (unquoted) YAML scalars are resolved with YAML-1.2 core-schema rules
(so ``no``/``yes``/``on``/``off`` stay strings, not booleans — the "Norway problem"), matching the
maintained Rust YAML parser (``serde_norway``) a future Rust port will use, so a ``.yaml`` loads to
the same model in both builds.

``json.loads`` / ``yaml.load`` yield ``Any``; both are pinned to ``object`` at the parse boundary
and every field's type is re-validated here, so no ``Any`` leaks into the model.
"""

from __future__ import annotations

import json
from collections.abc import Mapping
from typing import TYPE_CHECKING, NoReturn

if TYPE_CHECKING:
    # PyYAML is optional: only config_from_yaml catches yaml.YAMLError, and it imports yaml lazily
    # there. Keeping the import out of module scope means the JSON paths and `--help` never need it.
    import yaml

from tick_hub.model import (
    Emit,
    EmitKind,
    Gate,
    GateWhen,
    HealthCheck,
    Reminder,
    TickConfig,
)
from tick_hub.yamlcore import _MISSING_YAML_MSG, core_dump, core_load

__all__ = [
    "config_from_json",
    "config_from_yaml",
    "config_to_json",
    "config_to_yaml",
    "TickConfigError",
]

# Integer fields map to a Rust `i64` in the planned port; bound Python's arbitrary-precision ints
# to that range so a config the Python build accepts is also readable by the Rust build.
_I64_MIN = -(2**63)
_I64_MAX = 2**63 - 1


class TickConfigError(ValueError):
    """Raised when a tick-hub config document is malformed."""


def _check_i64(n: int, where: str) -> int:
    if not (_I64_MIN <= n <= _I64_MAX):
        raise TickConfigError(f"{where}: integer {n} does not fit a signed 64-bit range")
    return n


# --- typed narrowing helpers (json.loads yields Any; narrow explicitly, no Any leaks) ---


def _as_obj(value: object, where: str) -> dict[str, object]:
    if not isinstance(value, dict):
        raise TickConfigError(f"{where}: expected an object, got {type(value).__name__}")
    out: dict[str, object] = {}
    for key, val in value.items():
        out[str(key)] = val
    return out


def _req_str(m: Mapping[str, object], key: str, where: str) -> str:
    val = m.get(key)
    if not isinstance(val, str):
        raise TickConfigError(f"{where}: field '{key}' must be a string")
    return val


def _opt_str(m: Mapping[str, object], key: str, default: str, where: str) -> str:
    val = m.get(key, default)
    if not isinstance(val, str):
        raise TickConfigError(f"{where}: field '{key}' must be a string")
    return val


def _opt_int(m: Mapping[str, object], key: str, default: int, where: str) -> int:
    val = m.get(key, default)
    if isinstance(val, bool) or not isinstance(val, int):
        raise TickConfigError(f"{where}: field '{key}' must be an integer")
    return _check_i64(val, f"{where}.{key}")


def _opt_bool(m: Mapping[str, object], key: str, default: bool, where: str) -> bool:
    val = m.get(key, default)
    if not isinstance(val, bool):
        raise TickConfigError(f"{where}: field '{key}' must be a boolean")
    return val


def _opt_str_list(m: Mapping[str, object], key: str, where: str) -> tuple[str, ...]:
    val = m.get(key)
    if val is None:
        return ()
    if not isinstance(val, list):
        raise TickConfigError(f"{where}: field '{key}' must be a list of strings")
    out: list[str] = []
    for item in val:
        if not isinstance(item, str):
            raise TickConfigError(f"{where}: field '{key}' must contain only strings")
        out.append(item)
    return tuple(out)


def _opt_str_str_map(m: Mapping[str, object], key: str, where: str) -> dict[str, str]:
    val = m.get(key)
    if val is None:
        return {}
    obj = _as_obj(val, f"{where}.{key}")
    out: dict[str, str] = {}
    for name, text in obj.items():
        if not isinstance(text, str):
            raise TickConfigError(f"{where}.{key}.{name}: must be a string")
        out[name] = text
    return out


def _gate_from(value: object, where: str) -> Gate | None:
    if value is None:
        return None
    obj = _as_obj(value, where)
    when_name = _opt_str(obj, "when", GateWhen.SUCCESS.value, where)
    try:
        when = GateWhen(when_name)
    except ValueError as exc:
        raise TickConfigError(
            f"{where}.when: unknown value {when_name!r} "
            f"(allowed: {[w.value for w in GateWhen]})"
        ) from exc
    return Gate(
        cmd=_req_str(obj, "cmd", where),
        when=when,
        capture=_opt_bool(obj, "capture", False, where),
    )


def _emit_from(value: object, where: str) -> Emit:
    obj = _as_obj(value, where)
    kind_name = _opt_str(obj, "kind", EmitKind.ACTION.value, where)
    try:
        kind = EmitKind(kind_name)
    except ValueError as exc:
        raise TickConfigError(
            f"{where}.kind: unknown value {kind_name!r} "
            f"(allowed: {[k.value for k in EmitKind]})"
        ) from exc
    emit = Emit(
        kind=kind,
        title=_opt_str(obj, "title", "", where),
        skill=_opt_str(obj, "skill", "", where),
        fields=_opt_str_str_map(obj, "fields", where),
    )
    if emit.kind is EmitKind.ACTION and not emit.skill:
        raise TickConfigError(f"{where}: an ACTION emit requires a non-empty 'skill'")
    return emit


def _reminder_from(value: object, where: str) -> Reminder:
    obj = _as_obj(value, where)
    emit_raw = obj.get("emit")
    if emit_raw is None:
        raise TickConfigError(f"{where}: field 'emit' is required")
    return Reminder(
        name=_req_str(obj, "name", where),
        emit=_emit_from(emit_raw, f"{where}.emit"),
        cadence_secs=_opt_int(obj, "cadence_secs", 0, where),
        requires_flags=_opt_str_list(obj, "requires_flags", where),
        gate=_gate_from(obj.get("gate"), f"{where}.gate"),
    )


def _health_from(value: object, where: str) -> HealthCheck:
    obj = _as_obj(value, where)
    return HealthCheck(
        name=_req_str(obj, "name", where),
        glob=_req_str(obj, "glob", where),
        threshold_secs=_opt_int(obj, "threshold_secs", 0, where),
        detail=_opt_str(obj, "detail", "", where),
    )


def _reject_json_constant(token: str) -> NoReturn:
    """Reject JSON's non-standard ``Infinity`` / ``-Infinity`` / ``NaN`` literals.

    Python's ``json`` accepts these by default; the planned Rust ``serde_json`` build does not, so
    accepting them here would break future byte-for-byte parity."""
    raise TickConfigError(f"invalid JSON: non-finite float literal {token!r} is not allowed")


def config_from_json(text: str) -> TickConfig:
    """Parse a tick-hub config JSON document into a :class:`TickConfig`."""
    try:
        raw: object = json.loads(text, parse_constant=_reject_json_constant)
    except json.JSONDecodeError as exc:
        raise TickConfigError(f"invalid JSON: {exc}") from exc
    return _config_from_obj(raw)


def config_from_yaml(text: str) -> TickConfig:
    """Parse a tick-hub config YAML document into a :class:`TickConfig`.

    YAML is ISOMORPHIC to the JSON schema: the parsed object funnels through the same typed
    narrowing (:func:`_config_from_obj`), and plain scalars resolve with YAML-1.2 core-schema rules
    (:mod:`tick_hub.yamlcore`).

    PyYAML is optional; if it is not installed this raises :class:`TickConfigError` with an
    actionable install hint (JSON works without it)."""
    try:
        import yaml
    except ModuleNotFoundError as exc:
        raise TickConfigError(_MISSING_YAML_MSG) from exc
    try:
        raw: object = core_load(text)
    except yaml.YAMLError as exc:
        raise TickConfigError(f"invalid YAML: {exc}") from exc
    return _config_from_obj(raw)


def _config_from_obj(raw: object) -> TickConfig:
    doc = _as_obj(raw, "<root>")
    reminders_raw = doc.get("reminders")
    reminders: list[Reminder] = []
    if reminders_raw is not None:
        if not isinstance(reminders_raw, list):
            raise TickConfigError("<root>: 'reminders' must be a list")
        for i, entry in enumerate(reminders_raw):
            reminders.append(_reminder_from(entry, f"reminders[{i}]"))
    health_raw = doc.get("health_checks")
    health: list[HealthCheck] = []
    if health_raw is not None:
        if not isinstance(health_raw, list):
            raise TickConfigError("<root>: 'health_checks' must be a list")
        for i, entry in enumerate(health_raw):
            health.append(_health_from(entry, f"health_checks[{i}]"))
    return TickConfig(
        reminders=tuple(reminders),
        health_checks=tuple(health),
        description=_opt_str(doc, "description", "", "<root>"),
    )


# --- serialization (canonical, deterministic) ---


def _gate_to_obj(gate: Gate | None) -> object:
    if gate is None:
        return None
    return {"cmd": gate.cmd, "when": gate.when.value, "capture": gate.capture}


def _emit_to_obj(emit: Emit) -> dict[str, object]:
    return {
        "kind": emit.kind.value,
        "title": emit.title,
        "skill": emit.skill,
        "fields": dict(sorted(emit.fields.items())),
    }


def _reminder_to_obj(rem: Reminder) -> dict[str, object]:
    return {
        "name": rem.name,
        "cadence_secs": rem.cadence_secs,
        "requires_flags": list(rem.requires_flags),
        "gate": _gate_to_obj(rem.gate),
        "emit": _emit_to_obj(rem.emit),
    }


def _health_to_obj(hc: HealthCheck) -> dict[str, object]:
    return {
        "name": hc.name,
        "glob": hc.glob,
        "threshold_secs": hc.threshold_secs,
        "detail": hc.detail,
    }


def _config_to_obj(cfg: TickConfig) -> dict[str, object]:
    return {
        "description": cfg.description,
        "health_checks": [_health_to_obj(h) for h in cfg.health_checks],
        "reminders": [_reminder_to_obj(r) for r in cfg.reminders],
    }


def config_to_json(cfg: TickConfig) -> str:
    """Serialize a :class:`TickConfig` to canonical, deterministic JSON (2-space indent)."""
    return json.dumps(_config_to_obj(cfg), indent=2, ensure_ascii=False)


def config_to_yaml(cfg: TickConfig) -> str:
    """Serialize a :class:`TickConfig` to a YAML document that round-trips through
    :func:`config_from_yaml`.

    PyYAML is optional; if it is not installed this raises :class:`TickConfigError` with an
    actionable install hint (JSON works without it)."""
    try:
        return core_dump(_config_to_obj(cfg))
    except ModuleNotFoundError as exc:
        raise TickConfigError(_MISSING_YAML_MSG) from exc
