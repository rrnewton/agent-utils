"""The machine-readable output contract: pure HEALTH / ACTION / NOTE / ERROR line formatters.

Every tick emits one of four line types on stdout; a caller parses by the leading token:

    HEALTH: <check> <ok|stale|missing> age_secs=<N|NA> threshold_secs=<N> detail="..."
    ACTION: <skill> [key=value ...] title="..."
    NOTE:   <free text>
    ERROR:  <text>

Lines are independent; parse by the leading token. These formatters are pure (no I/O) so the exact
bytes are unit-testable and stable. Field VALUES are bare when safe and double-quoted (with ``\\``
and ``"`` escaped) when they contain whitespace, a quote, or are empty; the ``title`` is always
quoted. This mirrors the well-worn contract that this tool generalizes.
"""

from __future__ import annotations

from collections.abc import Mapping

HEALTH_STATUS_OK = "ok"
HEALTH_STATUS_STALE = "stale"
HEALTH_STATUS_MISSING = "missing"


def _quote(value: str) -> str:
    """Double-quote a value, escaping backslashes and quotes."""
    escaped = value.replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}"'


def _needs_quote(value: str) -> bool:
    return value == "" or any(ch.isspace() for ch in value) or '"' in value or "\\" in value


def _fmt_value(value: str) -> str:
    return _quote(value) if _needs_quote(value) else value


def format_action(skill: str, fields: Mapping[str, str], title: str) -> str:
    """``ACTION: <skill> [k=v ...] [title="..."]`` (title appended only when non-empty)."""
    parts = [f"ACTION: {skill}"]
    for key, value in fields.items():
        parts.append(f"{key}={_fmt_value(value)}")
    if title:
        parts.append(f"title={_quote(title)}")
    return " ".join(parts)


def format_note(text: str) -> str:
    """Format an informational ``NOTE`` record."""
    return f"NOTE: {text}"


def format_no_result(name: str, detail: str = "") -> str:
    """Format a gate that ran but could not determine its condition.

    Built from the gate NAME alone and never through :func:`render_emit`. A gate
    that cannot determine its condition is exactly the gate least likely to have
    produced a usable ``summary=``, so rendering this through the normal
    interpolation path would raise ``UnresolvedPlaceholderError`` and report a
    templating fault instead of the real one. This line must be emittable when
    the gate printed nothing at all.
    """
    detail = detail.strip()
    tail = f" ({detail})" if detail else ""
    return f"NO_RESULT: {name} could not determine its condition; this is not a pass{tail}"


def format_error(text: str) -> str:
    """Format an operational ``ERROR`` record."""
    return f"ERROR: {text}"


def format_health(
    name: str, status: str, age_secs: int | None, threshold_secs: int, detail: str
) -> str:
    """``HEALTH: <name> <status> age_secs=<N|NA> threshold_secs=<N> detail="..."``."""
    age = "NA" if age_secs is None else str(age_secs)
    escaped_detail = detail.replace("\\", "\\\\").replace('"', '\\"')
    return (
        f"HEALTH: {name} {status} age_secs={age} "
        f'threshold_secs={threshold_secs} detail="{escaped_detail}"'
    )
