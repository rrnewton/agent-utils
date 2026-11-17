"""Validator result adapter: validator output to receipt fields.

The validator is project-owned; this adapter only carries its
declared totals into the record vocabulary. Project meaning —
which profile qualifies, which payload is current — stays in
project configuration, never in this mapping.
"""
from __future__ import annotations

import json
from pathlib import Path

_INT_FIELDS = ("executed_tests", "passed_tests", "filtered_tests", "failures",
               "gates_run", "gates_expected", "intentional_skipped_nodes")


def _count(value: object) -> int | None:
    """One node count: an int, or a list whose length is the count."""
    if isinstance(value, bool):
        return None
    if isinstance(value, int):
        return value
    if isinstance(value, list):
        return len(value)
    return None


def adapt(raw: dict[str, object]) -> dict[str, object]:
    """Map one validator result object to receipt record fields.

    Unknown or malformed values are dropped, never invented: a
    missing total stays missing, and the predicate treats missing
    evidence as no evidence.
    """
    out: dict[str, object] = {}
    totals_raw = raw.get("test_results")
    totals: dict[str, object] = totals_raw if isinstance(totals_raw, dict) else {}
    nested_raw = totals.get("totals")
    nested: dict[str, object] = nested_raw if isinstance(nested_raw, dict) else {}
    for name in _INT_FIELDS:
        value = _count(raw.get(name))
        if value is None or value < 0:
            value = _count(nested.get(name))
        if value is not None and value >= 0:
            out[name] = value
    for name in ("profile", "selection_mode", "release_builder"):
        text: object = raw.get(name)
        if isinstance(text, str) and text:
            out[name] = text
    for name in ("e2e_payload", "coverage", "cell_results"):
        obj: object = raw.get(name)
        if isinstance(obj, dict):
            out[name] = obj
    known = _count(raw.get("known_gaps", nested.get("known_gaps")))
    if known is not None and known >= 0:
        out["known_gaps"] = known
    gates = raw.get("gates")
    if isinstance(gates, list):
        out["gates"] = gates
    return out


def from_result_file(path: Path) -> dict[str, object]:
    """Adapt a validator-written result file, or empty when absent."""
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return {}
    return adapt(raw) if isinstance(raw, dict) else {}
