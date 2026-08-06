"""Human-readable accounting bundled into each generated timeline archive."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import agent_team_timeline.run_stats as run_stats


def _usage(input_tokens: int, output_tokens: int) -> dict[str, object]:
    return {
        "input_tokens": input_tokens,
        "cached_input_tokens": 0,
        "cache_write_input_tokens": 0,
        "output_tokens": output_tokens,
        "reasoning_output_tokens": 0,
        "total_tokens": input_tokens + output_tokens,
    }


def _write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")


def _run(
    run_id: str,
    completed_at: str,
    status: str,
    summaries: dict[str, object] | None,
) -> dict[str, object]:
    return {
        "schema_version": 1,
        "run_id": run_id,
        "tool_version": "test",
        "started_at": "2026-08-06T00:00:00Z",
        "completed_at": completed_at,
        "status": status,
        "team_slug": "codex-coord",
        "command": ["agent-team-timeline", "refresh", "--team", "codex-coord"],
        "ingest": {
            "sources": 2,
            "source_bytes": 4096,
            "agents": 3,
            "events": 9,
            "tool_calls": 4,
            "edges": 2,
            "files_changed": 1,
        },
        "summaries": summaries,
        "build": {"agents": 3, "phases": 5, "files_changed": 7},
        "error": "backend stopped" if status == "failed" else None,
    }


def _summary() -> dict[str, object]:
    return {
        "backend": "codex",
        "model": "gpt-5.6-sol",
        "reasoning_effort": "xhigh",
        "phases": 5,
        "agent_names": 3,
        "rollups": 2,
        "project_overviews": 1,
        "glossary_definitions": 4,
        "cache_hits": 6,
        "cache_misses": 9,
        "backend_batches": 2,
        "newly_spent_usage": _usage(100, 10),
        "newly_spent_unknown_receipts": 0,
        "artifact_generation_usage": _usage(140, 14),
        "artifact_generation_unknown_receipts": 1,
        "unknown_legacy_artifacts": 2,
        "usage_run_paths": ["teams/codex-coord/summary_data/cache/_usage/runs/a.json"],
        "files_changed": 9,
    }


def _receipt(
    receipt_id: str, status: str, usage: dict[str, object] | None
) -> dict[str, object]:
    return {
        "schema_version": 1,
        "receipt_id": receipt_id,
        "backend": "codex",
        "model": "gpt-5.6-sol",
        "reasoning_effort": "xhigh",
        "status": status,
        "started_at": "2026-08-06T00:00:01Z",
        "completed_at": "2026-08-06T00:00:02Z",
        "input_hashes": ["abc"],
        "usage": usage,
        "error": None if status == "completed" else "failed",
    }


def _archive(tmp_path: Path) -> Path:
    archive = tmp_path / "archive"
    _write_json(
        archive / "runs" / "001.json",
        _run("001", "2026-08-06T00:01:00Z", "completed", _summary()),
    )
    _write_json(
        archive / "runs" / "002.json",
        _run("002", "2026-08-06T00:02:00Z", "failed", None),
    )
    receipts = (
        ("successful", "completed", _usage(100, 10)),
        ("failed-known", "failed", _usage(7, 3)),
        ("failed-unknown", "failed", None),
    )
    receipt_root = (
        archive
        / "teams"
        / "codex-coord"
        / "summary_data"
        / "cache"
        / "_usage"
        / "receipts"
    )
    for receipt_id, status, usage in receipts:
        _write_json(receipt_root / f"{receipt_id}.json", _receipt(receipt_id, status, usage))
    return archive


def test_report_separates_run_spend_from_complete_backend_ledger(
    tmp_path: Path,
) -> None:
    archive = _archive(tmp_path)
    report = run_stats.render_run_stats(archive)

    assert "Top-level runs: 2 (1 completed, 1 failed, 0 other; 1 with summarization)" in report
    assert "Receipts: 3; usage known=2; usage unknown=1" in report
    assert "Known tokens spent: total=120; input=107" in report
    assert "attempts=3 (1 completed, 2 failed, 0 other)" in report
    assert "Newly spent: total=110; input=100" in report
    assert "Ledger difference: +10 known tokens and +1 unknown-usage receipts" in report
    assert "tokens newly spent: total=110" in report
    assert "returned-artifact generation cost (not new spend): total=154" in report
    assert "[001] 2026-08-06T00:01:00Z  COMPLETED" in report
    assert "[002] 2026-08-06T00:02:00Z  FAILED" in report
    assert "run log: runs/001.json" in report


def test_bundled_script_defaults_to_its_own_archive_directory(
    tmp_path: Path,
) -> None:
    archive = _archive(tmp_path)
    source_path = Path(run_stats.__file__)
    script = archive / "run_stats.py"
    script.write_text(source_path.read_text(encoding="utf-8"), encoding="utf-8")

    completed = subprocess.run(
        [sys.executable, str(script)],
        text=True,
        capture_output=True,
        check=False,
        cwd=tmp_path,
    )

    assert completed.returncode == 0, completed.stderr
    assert f"Archive: {archive.resolve()}" in completed.stdout
    assert "Known tokens spent: total=120" in completed.stdout
