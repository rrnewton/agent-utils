"""`summarize-project`: a project weekly rolled up from its teams' own weekly rollups only."""

from __future__ import annotations

import json
from dataclasses import replace
from pathlib import Path

import pytest

from tests.test_timeline_pipeline import _team, _write_team
from wrkviz.cli import main as timeline_main
from wrkviz.pipeline import summarize_archive
from wrkviz.project_rollup import (
    load_weekly_inputs,
    project_root,
    project_weekly_jobs,
    summarize_project_weekly,
)

TEAMS = ("alpha-team", "beta-team")


def _archive(tmp_path: Path) -> str:
    """Two teams summarized coordinator-only through weekly; return the week key."""

    for slug, extra in zip(TEAMS, ("Alpha shipped the parser.", "Beta fixed the scheduler.")):
        _write_team(tmp_path, replace(_team(extra), team_slug=slug))
        summarize_archive(
            tmp_path,
            slug,
            "heuristic",
            "test-model",
            agent_scope="coordinator",
            rollup_kinds=("daily", "weekly"),
        )
    weeks = sorted(
        path.stem
        for path in (
            tmp_path / "teams" / TEAMS[0] / "summary_scopes" / "coordinator" / "rollups" / "weekly"
        ).glob("*.json")
    )
    assert len(weeks) == 1
    return weeks[0]


def test_project_weekly_reads_only_the_team_weeklies_and_lists_them(tmp_path: Path) -> None:
    week = _archive(tmp_path)

    inputs = load_weekly_inputs(tmp_path, TEAMS, week, "coordinator")
    job = project_weekly_jobs("example-project", week, inputs)
    # The model sees one line per team weekly and nothing else: no transcript, no phase.
    lines = job.transcript.split("\n\n")
    assert len(lines) == 2
    for line, slug in zip(lines, TEAMS):
        assert f"Team {slug} weekly:" in line
    assert "Alpha shipped the parser." in job.transcript
    assert "Beta fixed the scheduler." in job.transcript

    report = summarize_project_weekly(
        tmp_path,
        "example-project",
        TEAMS,
        week,
        "heuristic",
        "test-model",
        agent_scope="coordinator",
        display_timezone="America/New_York",
    )

    root = project_root(tmp_path, "coordinator", "example-project")
    assert report.rollup_path == root / "rollups" / "weekly" / f"{week}.json"
    data = json.loads(report.rollup_path.read_text(encoding="utf-8"))
    assert [item["team"] for item in data["inputs"]] == list(TEAMS)
    for item in data["inputs"]:
        assert (tmp_path / item["rollup"]).is_file()
    assert data["agent_scope"] == "coordinator"
    # Nothing invented: every work item the summary states comes from the team weeklies.
    for bullet in report.technical.work_summary:
        assert bullet.text.rstrip(".") in job.transcript
    for path in (report.technical_markdown, report.plain_markdown):
        text = path.read_text(encoding="utf-8")
        assert "coordinator-only" in text.splitlines()[0]
        inputs_section = text.split("## Inputs", 1)[1]
        for slug in TEAMS:
            assert f"`{slug}`" in inputs_section
    run = json.loads(report.run_path.read_text(encoding="utf-8"))
    assert [item["team"] for item in run["inputs"]] == list(TEAMS)
    # A second run is served from the project rollup's own cache.
    again = summarize_project_weekly(
        tmp_path, "example-project", TEAMS, week, "heuristic", "test-model",
        agent_scope="coordinator",
    )
    assert again.cache_misses == 0 and again.cache_hits == 2


def test_a_team_without_a_weekly_is_refused_by_name(tmp_path: Path) -> None:
    week = _archive(tmp_path)

    with pytest.raises(ValueError, match="ghost-team"):
        summarize_project_weekly(
            tmp_path, "example-project", (*TEAMS, "ghost-team"), week, "heuristic",
            "test-model", agent_scope="coordinator",
        )
    # The full-team scope was never summarized, so every team is missing there.
    with pytest.raises(ValueError, match="alpha-team.*beta-team"):
        load_weekly_inputs(tmp_path, TEAMS, week, "all")
    with pytest.raises(ValueError, match="ISO week"):
        load_weekly_inputs(tmp_path, TEAMS, "last-week", "coordinator")
    with pytest.raises(ValueError, match="more than once"):
        load_weekly_inputs(tmp_path, (TEAMS[0], TEAMS[0]), week, "coordinator")
    assert not project_root(tmp_path, "coordinator", "example-project").exists()


def test_cli_plans_then_writes(tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    week = _archive(tmp_path)
    common = [
        "summarize-project", "--output", str(tmp_path), "--name", "example-project",
        "--team", TEAMS[0], "--team", TEAMS[1], "--week", week,
        "--agent-scope", "coordinator", "--backend", "heuristic",
    ]

    assert timeline_main([*common, "--plan"]) == 0
    plan = capsys.readouterr().out
    assert f"input {TEAMS[0]}:" in plan and "to generate" in plan
    assert not project_root(tmp_path, "coordinator", "example-project").exists()

    assert timeline_main(common) == 0
    out = capsys.readouterr().out
    assert "2 team inputs" in out and "run metadata:" in out
    assert timeline_main([*common, "--team", "ghost-team"]) == 2
    assert "ghost-team" in capsys.readouterr().err
    for slug in TEAMS:
        assert not (tmp_path / "teams" / slug / "summary_data").exists()


def test_a_rollup_recording_another_scope_is_refused(tmp_path: Path) -> None:
    week = _archive(tmp_path)
    path = (
        tmp_path / "teams" / TEAMS[0] / "summary_scopes" / "coordinator" / "rollups" / "weekly"
        / f"{week}.json"
    )
    data = json.loads(path.read_text(encoding="utf-8"))
    assert data["agent_scope"] == "coordinator"
    path.write_text(json.dumps(dict(data, agent_scope="all")), encoding="utf-8")

    with pytest.raises(ValueError, match="records agent scope 'all'"):
        load_weekly_inputs(tmp_path, TEAMS, week, "coordinator")
