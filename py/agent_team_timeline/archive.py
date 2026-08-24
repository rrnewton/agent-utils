"""Small, deterministic primitives for the version-controllable timeline archive."""

from __future__ import annotations

import hashlib
import json
import os
import tempfile
from collections.abc import Iterable, Mapping, Sequence
from pathlib import Path

JsonScalar = str | int | float | bool | None
JsonValue = JsonScalar | list["JsonValue"] | dict[str, "JsonValue"]


def canonical_json(value: JsonValue) -> str:
    """Return the archive's stable JSON representation."""

    return json.dumps(value, ensure_ascii=False, indent=2, sort_keys=True) + "\n"


def canonical_jsonl(records: Iterable[Mapping[str, JsonValue]]) -> str:
    """Return one canonical JSON object per line, in the order given.

    The indented whole-file form above is what a human diffs; this one is what a reader can
    consume a record at a time without materializing the file. Both are sorted-key and
    ``ensure_ascii=False`` so the same record hashes the same either way, and both end in a
    newline so appending never merges two records into one line.
    """

    return "".join(
        json.dumps(dict(record), ensure_ascii=False, sort_keys=True, separators=(",", ":"))
        + "\n"
        for record in records
    )


def read_jsonl(path: Path) -> list[dict[str, JsonValue]]:
    """Read a line-delimited JSON record file, treating an absent file as empty.

    Absent means "nothing has written this yet", which every caller here handles by starting
    from an empty record set; a symlink or a non-regular file means something other than this
    tool owns the name, and that is refused rather than followed. Empty lines are skipped rather
    than refused, because a blank line carries no record and rejecting one would turn a harmless
    stray newline into an unreadable archive. Every line reports its own number, because "invalid
    JSON" without a line number in a several-hundred-thousand-line record file is not a
    diagnosis.

    **Split on ``"\\n"``, never with ``str.splitlines()``.** This is the reader for files
    :func:`canonical_jsonl` wrote, and that writer uses ``ensure_ascii=False``, so U+2028, U+2029
    and U+0085 -- which JSON does not escape and ``splitlines`` *does* treat as line terminators
    -- go to disk raw inside a string value. With ``splitlines`` one such character in one note's
    text turns one physical line into two JSON fragments, and the file becomes permanently
    unparseable: not a bad read, an archive that can no longer be loaded or re-ingested, poisoned
    by content the archive itself just wrote. There are no such characters in the corpus today;
    there is also nothing stopping the next agent message from containing one.
    """

    if not path.exists():
        return []
    if path.is_symlink() or not path.is_file():
        raise ValueError(f"line-delimited JSON record file is not a regular file: {path}")
    result: list[dict[str, JsonValue]] = []
    for line_number, line in enumerate(path.read_text(encoding="utf-8").split("\n"), 1):
        if not line:
            continue
        value = narrow_json(json.loads(line), f"{path}:{line_number}")
        result.append(as_object(value, f"{path}:{line_number}"))
    return result


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
    """Return *value* as a JSON object or raise a contextual error."""

    if not isinstance(value, dict):
        raise ValueError(f"{where}: expected an object")
    return value


def as_array(value: JsonValue, where: str) -> list[JsonValue]:
    """Return *value* as a JSON array or raise a contextual error."""

    if not isinstance(value, list):
        raise ValueError(f"{where}: expected an array")
    return value


def as_string(value: JsonValue, where: str) -> str:
    """Return *value* as a string or raise a contextual error."""

    if not isinstance(value, str):
        raise ValueError(f"{where}: expected a string")
    return value


def as_int(value: JsonValue, where: str) -> int:
    """Return *value* as a non-boolean integer or raise a contextual error."""

    if not isinstance(value, int) or isinstance(value, bool):
        raise ValueError(f"{where}: expected an integer")
    return value


def string_map(values: Mapping[str, str]) -> dict[str, JsonValue]:
    """Widen a string mapping to the recursive JSON value type."""

    return {key: value for key, value in values.items()}


def json_sequence(values: Sequence[JsonValue]) -> list[JsonValue]:
    """Copy a sequence into a mutable JSON array."""

    return list(values)
