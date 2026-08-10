"""Read-only CLI navigation for built timeline archives."""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

from agent_team_timeline.archive import (
    JsonValue,
    as_array,
    as_object,
    as_string,
    narrow_json,
)
from agent_team_timeline.cli import main as timeline_main


START = 1_786_068_000_000


def _write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value), encoding="utf-8")


def _site(tmp_path: Path) -> Path:
    root = tmp_path / "site"
    detail_path = "data/details/alpha/phase-work.json"
    technical_path = "teams/alpha/summaries/hourly/technical.md"
    plain_path = "teams/alpha/summaries/hourly/plain.md"
    timeline = {
        "schema_version": 1,
        "teams": [
            {
                "slug": "alpha",
                "label": "Alpha team",
                "provider": "codex",
                "projects": [],
                "hosts": [],
                "stats": {"active_agents": 2},
            },
            {
                "slug": "beta",
                "label": "Beta team",
                "provider": "claude",
                "projects": [],
                "hosts": [],
                "stats": {"active_agents": 1},
            },
        ],
        "agents": [
            {
                "id": "alpha::root",
                "team": "alpha",
                "parent_id": None,
                "short_name": "Coordinator",
                "official_name": "/root",
                "nickname": "",
                "lifetime_summary": "Coordinated the reproducible build investigation.",
                "naming_rationale": "Root coordinator",
                "depth": 0,
                "status": "completed",
                "start_ms": START,
                "end_ms": START + 3_600_000,
            },
            {
                "id": "alpha::child",
                "team": "alpha",
                "parent_id": "root",
                "short_name": "Build verifier",
                "official_name": "/root/build_verifier",
                "nickname": "Ada",
                "lifetime_summary": "Verified deterministic GHC build output.",
                "naming_rationale": "Completed verification work",
                "depth": 1,
                "status": "completed",
                "start_ms": START + 60_000,
                "end_ms": START + 1_800_000,
            },
            {
                "id": "beta::root",
                "team": "beta",
                "parent_id": None,
                "short_name": "Coordinator",
                "official_name": "/root",
                "nickname": "",
                "lifetime_summary": "Reviewed deployment logs.",
                "naming_rationale": "Root coordinator",
                "depth": 0,
                "status": "completed",
                "start_ms": START + 7_200_000,
                "end_ms": START + 10_800_000,
            },
        ],
        "phases": [
            {
                "id": "alpha::phase-work",
                "team": "alpha",
                "agent_id": "child",
                "detail_path": detail_path,
                "phrase": "Verified reproducible GHC builds.",
                "paragraph": "The verifier compared two builds and found identical artifacts.",
                "stats": {"tool_calls": 3},
                "start_ms": START + 60_000,
                "end_ms": START + 1_800_000,
            }
        ],
        "rollups": [
            {
                "team": "alpha",
                "kind": "hourly",
                "label": "Thu Aug 6 · 22:00",
                "technical_path": technical_path,
                "plain_language_path": plain_path,
                "start_ms": START,
                "end_ms": START + 3_600_000,
            }
        ],
    }
    _write_json(root / "data" / "timeline.json", timeline)
    _write_json(
        root / detail_path,
        {
            "team": "alpha",
            "phrase": "Verified reproducible GHC builds.",
            "paragraph": "The verifier compared two builds.",
            "stats": {"tool_calls": 3},
            "transcript": [
                {
                    "at_ms": START + 120_000,
                    "role": "assistant",
                    "text": "The GHC output hashes match exactly.",
                    "tools": [],
                    "pull_requests": [],
                },
                {
                    "at_ms": START + 180_000,
                    "role": "assistant",
                    "text": "The second GHC check is outside the requested boundary.",
                    "tools": [],
                    "pull_requests": [],
                }
            ],
        },
    )
    technical = root / technical_path
    technical.parent.mkdir(parents=True, exist_ok=True)
    technical.write_text("# Technical\n\nReproducible GHC builds passed.\n", encoding="utf-8")
    plain = root / plain_path
    plain.write_text("# Plain language\n\nTwo builds produced the same files.\n", encoding="utf-8")
    return root


def _response(capsys: pytest.CaptureFixture[str]) -> dict[str, JsonValue]:
    return as_object(narrow_json(json.loads(capsys.readouterr().out)), "response")


def _items(response: dict[str, JsonValue]) -> list[dict[str, JsonValue]]:
    return [
        as_object(value, f"response.items[{index}]")
        for index, value in enumerate(as_array(response["items"], "response.items"))
    ]


def test_list_uses_export_independent_stable_refs_and_filters(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _site(tmp_path)

    assert timeline_main(("query", "--output", str(root), "list", "teams")) == 0
    teams = _items(_response(capsys))
    assert [item["ref"] for item in teams] == ["team:alpha", "team:beta"]

    assert (
        timeline_main(
            (
                "query",
                "--output",
                str(root),
                "list",
                "agents",
                "--team",
                "alpha",
                "--end-time",
                "2026-08-07T03:00:00Z",
            )
        )
        == 0
    )
    agents = _items(_response(capsys))
    assert [item["ref"] for item in agents] == [
        "agent:alpha::root",
        "agent:alpha::child",
    ]
    assert agents[1]["parent_ref"] == "agent:alpha::root"

    assert (
        timeline_main(
            (
                "query",
                "--output",
                str(root),
                "list",
                "rollups",
                "--kind",
                "daily",
            )
        )
        == 0
    )
    assert _items(_response(capsys)) == []


def test_show_expands_relationships_rollups_and_optional_transcript(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _site(tmp_path)

    assert (
        timeline_main(
            ("query", "--output", str(root), "show", "agent:alpha::root")
        )
        == 0
    )
    agent = _items(_response(capsys))[0]
    assert agent["child_refs"] == ["agent:alpha::child"]

    phase_ref = "phase:alpha::phase-work"
    assert timeline_main(("query", "--output", str(root), "show", phase_ref)) == 0
    phase = _items(_response(capsys))[0]
    detail = phase["detail"]
    assert isinstance(detail, dict)
    assert "transcript" not in detail

    assert (
        timeline_main(
            (
                "query",
                "--output",
                str(root),
                "show",
                phase_ref,
                "--transcript",
            )
        )
        == 0
    )
    expanded = _items(_response(capsys))[0]
    expanded_detail = expanded["detail"]
    assert isinstance(expanded_detail, dict)
    assert "transcript" in expanded_detail

    rollup_ref = f"rollup:alpha::hourly::{START}"
    assert timeline_main(("query", "--output", str(root), "show", rollup_ref)) == 0
    rollup = _items(_response(capsys))[0]
    assert "Reproducible GHC" in str(rollup["technical_markdown"])
    assert "same files" in str(rollup["plain_language_markdown"])


def test_search_supports_summary_transcript_agent_and_jsonl_output(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _site(tmp_path)

    assert (
        timeline_main(
            (
                "query",
                "--output",
                str(root),
                "search",
                "ghc",
                "--scope",
                "all",
                "--team",
                "alpha",
                "--agent",
                "agent:alpha::child",
                "--start-time",
                "2026-08-07T02:00:00Z",
                "--end-time",
                "2026-08-07T02:03:00Z",
            )
        )
        == 0
    )
    matches = _items(_response(capsys))
    assert {item["record_type"] for item in matches} == {
        "agent",
        "phase",
        "transcript",
    }
    assert all(item["team"] == "alpha" for item in matches)
    assert sum(item["record_type"] == "transcript" for item in matches) == 1

    assert (
        timeline_main(
            (
                "query",
                "--output",
                str(root),
                "--format",
                "jsonl",
                "search",
                "same files",
            )
        )
        == 0
    )
    output = capsys.readouterr().out
    lines = output.splitlines()
    assert len(lines) == 1
    line = as_object(narrow_json(json.loads(lines[0])), "JSONL record")
    assert line["record_type"] == "rollup"


def test_query_fails_closed_on_unknown_refs_and_archive_path_escape(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _site(tmp_path)
    assert timeline_main(("query", "--output", str(root), "show", "agent:nope")) == 2
    error = capsys.readouterr().err
    assert "unknown stable reference" in error

    timeline_path = root / "data" / "timeline.json"
    timeline = as_object(
        narrow_json(json.loads(timeline_path.read_text(encoding="utf-8"))),
        "timeline",
    )
    rollups = as_array(timeline["rollups"], "timeline.rollups")
    first = as_object(rollups[0], "timeline.rollups[0]")
    first["technical_path"] = "../../outside.md"
    timeline_path.write_text(json.dumps(timeline), encoding="utf-8")
    reference = f"rollup:alpha::hourly::{START}"
    assert timeline_main(("query", "--output", str(root), "show", reference)) == 2
    error = capsys.readouterr().err
    assert "escapes root" in error


def test_query_rejects_unknown_timeline_schema_and_same_root_path_confusion(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _site(tmp_path)
    timeline_path = root / "data" / "timeline.json"
    original = as_object(
        narrow_json(json.loads(timeline_path.read_text(encoding="utf-8"))),
        "timeline",
    )
    invalid_schema = dict(original)
    invalid_schema["schema_version"] = 99
    timeline_path.write_text(json.dumps(invalid_schema), encoding="utf-8")

    assert timeline_main(("query", "--output", str(root), "list", "teams")) == 2
    assert "unsupported timeline schema version 99" in capsys.readouterr().err

    phases = as_array(original["phases"], "timeline.phases")
    first_phase = as_object(phases[0], "timeline.phases[0]")
    first_phase["detail_path"] = "data/timeline.json"
    timeline_path.write_text(json.dumps(original), encoding="utf-8")
    phase_id = as_string(first_phase["id"], "timeline.phases[0].id")
    team = as_string(first_phase["team"], "timeline.phases[0].team")
    reference = f"phase:{team}::{phase_id.removeprefix(team + '::')}"

    assert timeline_main(("query", "--output", str(root), "show", reference)) == 2
    assert "outside data/details" in capsys.readouterr().err


def test_bundled_query_source_runs_without_the_installed_package(tmp_path: Path) -> None:
    root = _site(tmp_path)
    source = Path(__file__).resolve().parents[1] / "agent_team_timeline" / "query.py"
    bundled = root / "query.py"
    shutil.copyfile(source, bundled)

    completed = subprocess.run(
        (sys.executable, str(bundled), "--output", str(root), "list", "teams"),
        cwd=tmp_path,
        check=False,
        capture_output=True,
        text=True,
    )

    assert completed.returncode == 0, completed.stderr
    response = as_object(
        narrow_json(json.loads(completed.stdout)), "standalone response"
    )
    assert response["count"] == 2
