#!/usr/bin/env python3
"""Print human-readable run and model-token statistics for a timeline archive.

This file is deliberately standard-library-only because ``render_archive`` copies it into every
generated archive.  The copied script remains useful on a machine where agent-team-timeline is not
installed.
"""

from __future__ import annotations

import argparse
import json
import shlex
import sys
from collections.abc import Sequence
from dataclasses import dataclass
from datetime import datetime
from pathlib import Path


JsonScalar = str | int | float | bool | None
JsonValue = JsonScalar | list["JsonValue"] | dict[str, "JsonValue"]


def _narrow_json(raw: object, where: str) -> JsonValue:
    if raw is None or isinstance(raw, (str, bool, int, float)):
        return raw
    if isinstance(raw, list):
        return [_narrow_json(item, where) for item in raw]
    if isinstance(raw, dict):
        result: dict[str, JsonValue] = {}
        for key, item in raw.items():
            if not isinstance(key, str):
                raise ValueError(f"{where}: object key is not a string")
            result[key] = _narrow_json(item, where)
        return result
    raise ValueError(f"{where}: unsupported JSON value {type(raw).__name__}")


def _read_object(path: Path) -> dict[str, JsonValue]:
    raw: object = json.loads(path.read_text(encoding="utf-8"))
    value = _narrow_json(raw, str(path))
    if not isinstance(value, dict):
        raise ValueError(f"{path}: expected a JSON object")
    return value


def _required_string(obj: dict[str, JsonValue], key: str, where: str) -> str:
    value = obj.get(key)
    if not isinstance(value, str):
        raise ValueError(f"{where}.{key}: expected a string")
    return value


def _optional_string(obj: dict[str, JsonValue], key: str, where: str) -> str | None:
    value = obj.get(key)
    if value is None:
        return None
    if not isinstance(value, str):
        raise ValueError(f"{where}.{key}: expected a string or null")
    return value


def _required_int(obj: dict[str, JsonValue], key: str, where: str) -> int:
    value = obj.get(key)
    if not isinstance(value, int) or isinstance(value, bool):
        raise ValueError(f"{where}.{key}: expected an integer")
    return value


def _optional_int(obj: dict[str, JsonValue], key: str, where: str) -> int | None:
    value = obj.get(key)
    if value is None:
        return None
    if not isinstance(value, int) or isinstance(value, bool):
        raise ValueError(f"{where}.{key}: expected an integer or null")
    return value


def _optional_object(
    obj: dict[str, JsonValue], key: str, where: str
) -> dict[str, JsonValue] | None:
    value = obj.get(key)
    if value is None:
        return None
    if not isinstance(value, dict):
        raise ValueError(f"{where}.{key}: expected an object or null")
    return value


@dataclass(frozen=True)
class Usage:
    """One additive model-token breakdown."""

    input_tokens: int = 0
    cached_input_tokens: int = 0
    cache_write_input_tokens: int = 0
    output_tokens: int = 0
    reasoning_output_tokens: int = 0

    @property
    def total_tokens(self) -> int:
        """Return billable input plus output tokens without double-counting subsets."""

        return self.input_tokens + self.output_tokens

    def __add__(self, other: Usage) -> Usage:
        return Usage(
            input_tokens=self.input_tokens + other.input_tokens,
            cached_input_tokens=self.cached_input_tokens + other.cached_input_tokens,
            cache_write_input_tokens=(
                self.cache_write_input_tokens + other.cache_write_input_tokens
            ),
            output_tokens=self.output_tokens + other.output_tokens,
            reasoning_output_tokens=(
                self.reasoning_output_tokens + other.reasoning_output_tokens
            ),
        )

    @classmethod
    def from_json(cls, value: JsonValue, where: str) -> Usage:
        """Parse and validate one serialized token-usage breakdown."""

        if not isinstance(value, dict):
            raise ValueError(f"{where}: expected an object")
        usage = cls(
            input_tokens=_required_int(value, "input_tokens", where),
            cached_input_tokens=_required_int(value, "cached_input_tokens", where),
            cache_write_input_tokens=_required_int(
                value, "cache_write_input_tokens", where
            ),
            output_tokens=_required_int(value, "output_tokens", where),
            reasoning_output_tokens=_required_int(
                value, "reasoning_output_tokens", where
            ),
        )
        if min(
            usage.input_tokens,
            usage.cached_input_tokens,
            usage.cache_write_input_tokens,
            usage.output_tokens,
            usage.reasoning_output_tokens,
        ) < 0:
            raise ValueError(f"{where}: token counts must be non-negative")
        if usage.cached_input_tokens > usage.input_tokens:
            raise ValueError(f"{where}: cached input exceeds input")
        if usage.reasoning_output_tokens > usage.output_tokens:
            raise ValueError(f"{where}: reasoning output exceeds output")
        recorded_total = _required_int(value, "total_tokens", where)
        if recorded_total != usage.total_tokens:
            raise ValueError(f"{where}.total_tokens: inconsistent token total")
        return usage


@dataclass(frozen=True)
class RunRecord:
    """The top-level provenance record for one pipeline command."""

    path: Path
    run_id: str
    started_at: str
    completed_at: str
    status: str
    team_slug: str
    command: tuple[str, ...]
    ingest: dict[str, JsonValue] | None
    summaries: dict[str, JsonValue] | None
    build: dict[str, JsonValue] | None
    error: str | None

    @classmethod
    def from_path(cls, path: Path) -> RunRecord:
        """Load one immutable top-level pipeline run record."""

        obj = _read_object(path)
        where = str(path)
        raw_command = obj.get("command")
        if not isinstance(raw_command, list) or not all(
            isinstance(item, str) for item in raw_command
        ):
            raise ValueError(f"{where}.command: expected an array of strings")
        return cls(
            path=path,
            run_id=_required_string(obj, "run_id", where),
            started_at=_required_string(obj, "started_at", where),
            completed_at=_required_string(obj, "completed_at", where),
            status=_required_string(obj, "status", where),
            team_slug=_required_string(obj, "team_slug", where),
            command=tuple(item for item in raw_command if isinstance(item, str)),
            ingest=_optional_object(obj, "ingest", where),
            summaries=_optional_object(obj, "summaries", where),
            build=_optional_object(obj, "build", where),
            error=_optional_string(obj, "error", where),
        )


@dataclass(frozen=True)
class BatchReceipt:
    """One actual backend attempt from the immutable receipt ledger."""

    receipt_id: str
    backend: str
    model: str
    reasoning_effort: str | None
    status: str
    usage: Usage | None

    @classmethod
    def from_path(cls, path: Path) -> BatchReceipt:
        """Load one immutable backend-attempt receipt."""

        obj = _read_object(path)
        where = str(path)
        raw_usage = obj.get("usage")
        return cls(
            receipt_id=_required_string(obj, "receipt_id", where),
            backend=_required_string(obj, "backend", where),
            model=_required_string(obj, "model", where),
            reasoning_effort=_optional_string(obj, "reasoning_effort", where),
            status=_required_string(obj, "status", where),
            usage=(
                None
                if raw_usage is None
                else Usage.from_json(raw_usage, f"{where}.usage")
            ),
        )


@dataclass(frozen=True)
class ModelLedger:
    """Aggregated receipt counts and usage for one backend/model/effort."""

    attempts: int = 0
    completed: int = 0
    failed: int = 0
    other_status: int = 0
    unknown_usage: int = 0
    usage: Usage = Usage()

    def add(self, receipt: BatchReceipt) -> ModelLedger:
        """Return this ledger with one backend receipt incorporated."""

        return ModelLedger(
            attempts=self.attempts + 1,
            completed=self.completed + int(receipt.status == "completed"),
            failed=self.failed + int(receipt.status == "failed"),
            other_status=self.other_status
            + int(receipt.status not in ("completed", "failed")),
            unknown_usage=self.unknown_usage + int(receipt.usage is None),
            usage=self.usage + (receipt.usage or Usage()),
        )


def _load_runs(archive: Path) -> tuple[list[RunRecord], list[str]]:
    records: list[RunRecord] = []
    warnings: list[str] = []
    for path in sorted((archive / "runs").glob("*.json")):
        try:
            records.append(RunRecord.from_path(path))
        except (OSError, ValueError, json.JSONDecodeError) as error:
            warnings.append(f"could not read run log {path.relative_to(archive)}: {error}")
    records.sort(key=lambda item: (item.completed_at, item.run_id))
    return records, warnings


def _receipt_paths(archive: Path) -> list[Path]:
    teams = archive / "teams"
    if not teams.is_dir():
        return []
    return sorted(
        path
        for path in teams.rglob("*.json")
        if path.parent.name == "receipts" and path.parent.parent.name == "_usage"
    )


def _load_receipts(archive: Path) -> tuple[list[BatchReceipt], list[str]]:
    by_id: dict[str, BatchReceipt] = {}
    warnings: list[str] = []
    for path in _receipt_paths(archive):
        try:
            receipt = BatchReceipt.from_path(path)
        except (OSError, ValueError, json.JSONDecodeError) as error:
            warnings.append(
                f"could not read backend receipt {path.relative_to(archive)}: {error}"
            )
            continue
        previous = by_id.get(receipt.receipt_id)
        if previous is not None and previous != receipt:
            warnings.append(f"conflicting duplicate backend receipt {receipt.receipt_id}")
            continue
        by_id[receipt.receipt_id] = receipt
    return sorted(by_id.values(), key=lambda item: item.receipt_id), warnings


def _format_usage(usage: Usage) -> str:
    return (
        f"total={usage.total_tokens:,}; input={usage.input_tokens:,} "
        f"(cached={usage.cached_input_tokens:,}, cache-write={usage.cache_write_input_tokens:,}); "
        f"output={usage.output_tokens:,} (reasoning={usage.reasoning_output_tokens:,})"
    )


def _duration(started_at: str, completed_at: str) -> str:
    try:
        started = datetime.fromisoformat(started_at.replace("Z", "+00:00"))
        completed = datetime.fromisoformat(completed_at.replace("Z", "+00:00"))
        seconds = max(0, round((completed - started).total_seconds()))
    except ValueError:
        return "unknown"
    hours, remainder = divmod(seconds, 3_600)
    minutes, final_seconds = divmod(remainder, 60)
    if hours:
        return f"{hours}h {minutes}m {final_seconds}s"
    if minutes:
        return f"{minutes}m {final_seconds}s"
    return f"{final_seconds}s"


def _object_counts(obj: dict[str, JsonValue]) -> str:
    parts: list[str] = []
    for key in sorted(obj):
        value = obj[key]
        if isinstance(value, bool) or not isinstance(value, (int, str)):
            continue
        parts.append(f"{key}={value:,}" if isinstance(value, int) else f"{key}={value}")
    return ", ".join(parts) if parts else "none"


def _summary_usage(
    summary: dict[str, JsonValue], key: str, where: str
) -> Usage | None:
    value = summary.get(key)
    if value is None:
        return None
    return Usage.from_json(value, f"{where}.{key}")


def _summary_int(
    summary: dict[str, JsonValue], key: str, where: str, default: int = 0
) -> int:
    value = _optional_int(summary, key, where)
    return default if value is None else value


def _format_run(record: RunRecord, archive: Path, index: int) -> list[str]:
    action = record.command[1] if len(record.command) > 1 else "command"
    lines = [
        f"[{index:03d}] {record.completed_at}  {record.status.upper()}  "
        f"{record.team_slug} / {action}",
        f"      duration: {_duration(record.started_at, record.completed_at)}",
        f"      command: {shlex.join(record.command)}",
    ]
    if record.ingest is not None:
        lines.append(f"      ingest: {_object_counts(record.ingest)}")
    if record.summaries is not None:
        where = str(record.path) + ".summaries"
        backend = _required_string(record.summaries, "backend", where)
        model = _required_string(record.summaries, "model", where)
        effort = _optional_string(record.summaries, "reasoning_effort", where) or "default"
        hits = _summary_int(record.summaries, "cache_hits", where)
        misses = _summary_int(record.summaries, "cache_misses", where)
        batches = _summary_int(record.summaries, "backend_batches", where)
        lines.extend(
            (
                f"      summarize: {backend} / {model} / effort={effort}; "
                f"jobs={hits + misses:,}, cache={hits:,} hit + {misses:,} miss, "
                f"backend_batches={batches:,}",
                "      products: "
                + ", ".join(
                    f"{key}={_summary_int(record.summaries, key, where):,}"
                    for key in (
                        "phases",
                        "agent_names",
                        "rollups",
                        "plain_language_rollups",
                        "rollup_summary_artifacts",
                        "glossary_terms",
                        "project_overviews",
                        "glossary_definitions",
                        "files_changed",
                    )
                ),
            )
        )
        new_usage = _summary_usage(record.summaries, "newly_spent_usage", where)
        new_unknown = _summary_int(
            record.summaries, "newly_spent_unknown_receipts", where
        )
        lines.append(
            "      tokens newly spent: "
            + (_format_usage(new_usage) if new_usage is not None else "unavailable")
            + f"; unknown receipts={new_unknown:,}"
        )
        artifact_usage = _summary_usage(
            record.summaries, "artifact_generation_usage", where
        )
        artifact_unknown = _summary_int(
            record.summaries, "artifact_generation_unknown_receipts", where
        )
        legacy_unknown = _summary_int(
            record.summaries, "unknown_legacy_artifacts", where
        )
        lines.append(
            "      returned-artifact generation cost (not new spend): "
            + (
                _format_usage(artifact_usage)
                if artifact_usage is not None
                else "unavailable"
            )
            + f"; unknown receipts={artifact_unknown:,}; legacy unknown={legacy_unknown:,}"
        )
        raw_paths = record.summaries.get("usage_run_paths")
        if isinstance(raw_paths, list):
            lines.append(f"      internal accounting records: {len(raw_paths):,}")
    if record.build is not None:
        lines.append(f"      build: {_object_counts(record.build)}")
    if record.error is not None:
        lines.append(f"      error: {record.error}")
    lines.append(f"      run log: {record.path.relative_to(archive)}")
    return lines


def render_run_stats(archive: Path) -> str:
    """Return a complete human-readable accounting report for *archive*."""

    root = archive.resolve()
    if not root.is_dir():
        raise ValueError(f"archive directory does not exist: {root}")
    runs, run_warnings = _load_runs(root)
    receipts, receipt_warnings = _load_receipts(root)

    model_ledgers: dict[tuple[str, str, str], ModelLedger] = {}
    receipt_usage = Usage()
    receipt_unknown = 0
    for receipt in receipts:
        effort = receipt.reasoning_effort or "default"
        key = (receipt.backend, receipt.model, effort)
        model_ledgers[key] = model_ledgers.get(key, ModelLedger()).add(receipt)
        if receipt.usage is None:
            receipt_unknown += 1
        else:
            receipt_usage += receipt.usage

    logged_usage = Usage()
    logged_unknown = 0
    summarized_runs = 0
    for run in runs:
        if run.summaries is None:
            continue
        summarized_runs += 1
        where = str(run.path) + ".summaries"
        usage = _summary_usage(run.summaries, "newly_spent_usage", where)
        if usage is not None:
            logged_usage += usage
        logged_unknown += _summary_int(
            run.summaries, "newly_spent_unknown_receipts", where
        )

    completed = sum(run.status == "completed" for run in runs)
    failed = sum(run.status == "failed" for run in runs)
    other = len(runs) - completed - failed
    lines = [
        "Agent Team Timeline run statistics",
        f"Archive: {root}",
        f"Top-level runs: {len(runs):,} "
        f"({completed:,} completed, {failed:,} failed, {other:,} other; "
        f"{summarized_runs:,} with summarization)",
        "",
        "Backend receipt ledger (archive-wide actual attempts)",
        f"  Receipts: {len(receipts):,}; usage known={len(receipts) - receipt_unknown:,}; "
        f"usage unknown={receipt_unknown:,}",
        f"  Known tokens spent: {_format_usage(receipt_usage)}",
    ]
    if model_ledgers:
        lines.append("  By backend / model / reasoning effort:")
        for key in sorted(model_ledgers):
            ledger = model_ledgers[key]
            statuses = (
                f"{ledger.completed:,} completed, {ledger.failed:,} failed, "
                f"{ledger.other_status:,} other"
            )
            lines.append(
                f"    {key[0]} / {key[1]} / {key[2]}: attempts={ledger.attempts:,} "
                f"({statuses}); unknown usage={ledger.unknown_usage:,}; "
                f"{_format_usage(ledger.usage)}"
            )
    else:
        lines.append("  No backend receipts found.")

    lines.extend(
        (
            "",
            "Top-level run-log accounting (successful summarization reports)",
            f"  Newly spent: {_format_usage(logged_usage)}; "
            f"unknown receipts={logged_unknown:,}",
        )
    )
    delta = receipt_usage.total_tokens - logged_usage.total_tokens
    if delta != 0 or receipt_unknown != logged_unknown:
        lines.append(
            f"  Ledger difference: {delta:+,} known tokens and "
            f"{receipt_unknown - logged_unknown:+,} unknown-usage receipts. "
            "This includes failed/unattributed backend attempts or missing top-level logs."
        )

    lines.extend(("", "Runs (oldest to newest)"))
    if not runs:
        lines.append("  No top-level run logs found.")
    for index, run in enumerate(runs, start=1):
        if index > 1:
            lines.append("")
        lines.extend(_format_run(run, root, index))

    warnings = run_warnings + receipt_warnings
    if warnings:
        lines.extend(("", f"Warnings ({len(warnings):,})"))
        lines.extend(f"  - {warning}" for warning in warnings)
    lines.extend(
        (
            "",
            "Notes: cached input is a subset of input; reasoning output is a subset of output.",
            "Returned-artifact generation cost is provenance, not additional spend, and is not "
            "added to totals.",
        )
    )
    return "\n".join(lines) + "\n"


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Print per-run and model-token statistics from a timeline archive."
    )
    parser.add_argument(
        "archive",
        nargs="?",
        type=Path,
        default=Path(__file__).resolve().parent,
        help="archive directory (default: directory containing this script)",
    )
    return parser


def _main(argv: Sequence[str] | None = None) -> int:
    args = list(argv) if argv is not None else sys.argv[1:]
    namespace = _parser().parse_args(args)
    archive = namespace.archive
    if not isinstance(archive, Path):
        raise TypeError("archive argument was not parsed as a path")
    try:
        print(render_run_stats(archive), end="")
    except (OSError, ValueError, json.JSONDecodeError) as error:
        print(f"run_stats.py: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
