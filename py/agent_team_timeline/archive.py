"""Small, deterministic primitives for the version-controllable timeline archive."""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
from collections.abc import Mapping, Sequence
from pathlib import Path

JsonScalar = str | int | float | bool | None
JsonValue = JsonScalar | list["JsonValue"] | dict[str, "JsonValue"]


def canonical_json(value: JsonValue) -> str:
    """Return the archive's stable JSON representation."""

    return json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"


def content_hash(text: str) -> str:
    """Hash UTF-8 text for cache keys and provenance."""

    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def write_text_if_changed(path: Path, text: str, *, executable: bool = False) -> bool:
    """Atomically replace *path* only when its bytes differ.

    Avoiding an identical rewrite is important for archives checked into Git: a formatting-only
    rebuild neither churns mtimes nor obscures which source or summary actually changed.
    """

    if path.is_file() and path.read_text(encoding="utf-8") == text:
        if executable:
            path.chmod(path.stat().st_mode | 0o111)
        return False
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, raw_tmp = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    tmp = Path(raw_tmp)
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="") as handle:
            handle.write(text)
            handle.flush()
            os.fsync(handle.fileno())
        if executable:
            tmp.chmod(0o755)
        os.replace(tmp, path)
    finally:
        if tmp.exists():
            tmp.unlink()
    return True


def write_json_if_changed(path: Path, value: JsonValue) -> bool:
    """Write deterministic JSON through :func:`write_text_if_changed`."""

    return write_text_if_changed(path, canonical_json(value))


def read_json(path: Path) -> JsonValue:
    """Read JSON while narrowing it to the archive's recursive value type."""

    raw: object = json.loads(path.read_text(encoding="utf-8"))
    return narrow_json(raw, str(path))


def narrow_json(raw: object, where: str = "JSON") -> JsonValue:
    """Reject non-JSON values and non-string object keys."""

    if raw is None or isinstance(raw, (str, bool, int, float)):
        return raw
    if isinstance(raw, list):
        return [narrow_json(item, where) for item in raw]
    if isinstance(raw, dict):
        result: dict[str, JsonValue] = {}
        for key, item in raw.items():
            if not isinstance(key, str):
                raise ValueError(f"{where}: object key is not a string")
            result[key] = narrow_json(item, where)
        return result
    raise ValueError(f"{where}: unsupported JSON value {type(raw).__name__}")


def as_object(value: JsonValue, where: str) -> dict[str, JsonValue]:
    if not isinstance(value, dict):
        raise ValueError(f"{where}: expected an object")
    return value


def as_array(value: JsonValue, where: str) -> list[JsonValue]:
    if not isinstance(value, list):
        raise ValueError(f"{where}: expected an array")
    return value


def as_string(value: JsonValue, where: str) -> str:
    if not isinstance(value, str):
        raise ValueError(f"{where}: expected a string")
    return value


def as_int(value: JsonValue, where: str) -> int:
    if not isinstance(value, int) or isinstance(value, bool):
        raise ValueError(f"{where}: expected an integer")
    return value


def string_map(values: Mapping[str, str]) -> dict[str, JsonValue]:
    return {key: value for key, value in values.items()}


def json_sequence(values: Sequence[JsonValue]) -> list[JsonValue]:
    return list(values)
