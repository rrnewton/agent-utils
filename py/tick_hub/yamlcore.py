"""Shared YAML-1.2-core scalar resolution, used by both the config and the ops-state loaders.

PyYAML defaults to YAML 1.1 implicit typing, which resolves UNQUOTED scalars differently from a
YAML-1.2 core schema: 1.1 turns ``no``/``yes``/``on``/``off`` into booleans (the "Norway problem"),
``0755`` into octal, ``1_000`` into a number, and ``2024-01-01`` into a date. A future Rust port
would use ``serde_norway`` (YAML 1.2 core), so both tick-hub YAML surfaces resolve plain scalars with
the SAME core-schema rules here — quoted and block scalars always stay strings. This keeps a given
``.yaml`` building the identical model in either language and never resurrects the Norway problem.

``core_load`` pins ``yaml.load``'s ``Any`` to ``object`` at the boundary; callers re-validate every
field's type, so no ``Any`` leaks past this module.
"""

from __future__ import annotations

import math
import re

import yaml
from yaml.nodes import ScalarNode

__all__ = ["core_load", "core_dump", "resolve_core_scalar"]

_RE_INT_DEC = re.compile(r"^[-+]?(0|[1-9][0-9]*)$")
_RE_INT_OCT = re.compile(r"^0o[0-7]+$")
_RE_INT_HEX = re.compile(r"^0x[0-9a-fA-F]+$")
_RE_INT_BIN = re.compile(r"^0b[01]+$")
_RE_FLOAT = re.compile(
    r"^[-+]?(\.[0-9]+|[0-9]+\.[0-9]*)([eE][-+]?[0-9]+)?$" r"|^[-+]?[0-9]+[eE][-+]?[0-9]+$"
)
_NULL_TOKENS = frozenset({"", "~", "null", "Null", "NULL"})
_TRUE_TOKENS = frozenset({"true", "True", "TRUE"})
_FALSE_TOKENS = frozenset({"false", "False", "FALSE"})
_NONFINITE_TOKENS = frozenset(
    {
        ".inf", ".Inf", ".INF", "+.inf", "+.Inf", "+.INF", "-.inf", "-.Inf", "-.INF",
        ".nan", ".NaN", ".NAN",
    }
)

_CORE_TAG = "tag:agent-utils,2026:core-scalar"


def resolve_core_scalar(value: str) -> object:
    """Resolve one PLAIN YAML scalar to the type a YAML-1.2 core schema (serde_norway) would.

    Returns ``None`` / ``bool`` / ``int`` / ``float`` / ``str``. Leading-zero decimals, underscore
    grouping, sexagesimal, and timestamps stay STRINGS; ``0o``/``0x``/``0b`` and dotted/exponent
    floats parse; a float overflowing to infinity stays a string, and an explicit ``.inf``/``.nan``
    becomes null."""
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
    return resolve_core_scalar(loader.construct_scalar(node))


class _CoreLoader(yaml.SafeLoader):
    """A ``SafeLoader`` that resolves PLAIN scalars with :func:`resolve_core_scalar`."""


_CoreLoader.yaml_implicit_resolvers = {None: [(_CORE_TAG, re.compile(r".*", re.DOTALL))]}
_CoreLoader.add_constructor(_CORE_TAG, _construct_core_scalar)


def _represent_core_str(dumper: yaml.SafeDumper, data: str) -> ScalarNode:
    """Force-quote any string that :func:`resolve_core_scalar` would re-read as a NON-string."""
    style = "'" if not isinstance(resolve_core_scalar(data), str) else None
    return dumper.represent_scalar("tag:yaml.org,2002:str", data, style=style)


class _CoreDumper(yaml.SafeDumper):
    """SafeDumper whose string representer round-trips through :class:`_CoreLoader`."""


_CoreDumper.add_representer(str, _represent_core_str)


def core_load(text: str) -> object:
    """Parse YAML with core-schema plain-scalar resolution; result pinned to ``object``."""
    raw: object = yaml.load(text, Loader=_CoreLoader)
    return raw


def core_dump(obj: object) -> str:
    """Dump ``obj`` to a YAML document that round-trips back through :func:`core_load`."""
    return yaml.dump(
        obj,
        Dumper=_CoreDumper,
        sort_keys=False,
        allow_unicode=True,
        default_flow_style=False,
    )
