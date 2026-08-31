"""Strict YAML-1.2-core loading for the project policy file.

PyYAML defaults to YAML 1.1, while the native package uses a YAML 1.2 parser.  This loader pins
plain scalar resolution to the same core schema and rejects duplicate/non-string/merge keys so a
reviewed allowlist cannot change meaning across implementations or parser quirks.
"""

from __future__ import annotations

import math
import re
from collections.abc import Hashable
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    import yaml
    from yaml.nodes import ScalarNode

__all__ = ["core_load"]

_RE_INT_DEC = re.compile(r"^[-+]?(0|[1-9][0-9]*)$")
_RE_INT_OCT = re.compile(r"^[-+]?0o[0-7]+$")
_RE_INT_HEX = re.compile(r"^[-+]?0x[0-9a-fA-F]+$")
_RE_INT_BIN = re.compile(r"^[-+]?0b[01]+$")
_RE_FLOAT = re.compile(
    r"^[-+]?(\.[0-9]+|[0-9]+\.[0-9]*)([eE][-+]?[0-9]+)?$"
    r"|^[-+]?[0-9]+[eE][-+]?[0-9]+$"
)
_NULL_TOKENS = frozenset({"", "~", "null", "Null", "NULL"})
_TRUE_TOKENS = frozenset({"true", "True", "TRUE"})
_FALSE_TOKENS = frozenset({"false", "False", "FALSE"})
_NONFINITE_TOKENS = frozenset(
    {
        ".inf",
        ".Inf",
        ".INF",
        "+.inf",
        "+.Inf",
        "+.INF",
        "-.inf",
        "-.Inf",
        "-.INF",
        ".nan",
        ".NaN",
        ".NAN",
    }
)
_CORE_TAG = "tag:herdr-run,2026:core-scalar"


def _resolve_core_scalar(value: str) -> object:
    if value in _NULL_TOKENS:
        return None
    if value in _TRUE_TOKENS:
        return True
    if value in _FALSE_TOKENS:
        return False
    if _RE_INT_DEC.fullmatch(value):
        return int(value, 10)
    if _RE_INT_OCT.fullmatch(value) or _RE_INT_HEX.fullmatch(value) or _RE_INT_BIN.fullmatch(value):
        return int(value, 0)
    if value in _NONFINITE_TOKENS:
        return None
    if _RE_FLOAT.fullmatch(value):
        parsed = float(value)
        return parsed if math.isfinite(parsed) else value
    return value


def _construct_core_scalar(loader: yaml.SafeLoader, node: ScalarNode) -> object:
    raw = loader.construct_scalar(node)
    if raw in _NONFINITE_TOKENS:
        import yaml

        raise yaml.constructor.ConstructorError(
            None, None, "non-finite YAML numbers are not supported", node.start_mark
        )
    return _resolve_core_scalar(raw)


_CORE_LOADER: type[yaml.SafeLoader] | None = None


def _core_loader() -> type[yaml.SafeLoader]:
    global _CORE_LOADER
    if _CORE_LOADER is not None:
        return _CORE_LOADER
    import yaml

    class _CoreLoader(yaml.SafeLoader):
        def construct_mapping(
            self, node: yaml.MappingNode, deep: bool = False
        ) -> dict[Hashable, object]:
            result: dict[Hashable, object] = {}
            for key_node, value_node in node.value:
                key = self.construct_object(key_node, deep=deep)
                if not isinstance(key, str):
                    raise yaml.constructor.ConstructorError(
                        None, None, "mapping keys must be strings", key_node.start_mark
                    )
                if key == "<<":
                    raise yaml.constructor.ConstructorError(
                        None, None, "YAML merge keys are not supported", key_node.start_mark
                    )
                if key in result:
                    raise yaml.constructor.ConstructorError(
                        None, None, f"duplicate mapping key {key!r}", key_node.start_mark
                    )
                result[key] = self.construct_object(value_node, deep=deep)
            return result

    _CoreLoader.yaml_implicit_resolvers = {
        None: [(_CORE_TAG, re.compile(r".*", re.DOTALL))]
    }
    _CoreLoader.add_constructor(_CORE_TAG, _construct_core_scalar)
    _CORE_LOADER = _CoreLoader
    return _CoreLoader


def core_load(text: str) -> object:
    """Parse strict YAML-1.2-core text and return an untrusted object tree."""
    import yaml

    raw: object = yaml.load(text, Loader=_core_loader())
    return raw
