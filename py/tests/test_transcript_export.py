"""Zero-model transcript extraction and prompt-range query contracts."""

from __future__ import annotations

import json
import stat
import subprocess
from dataclasses import replace
from pathlib import Path

import pytest

from agent_team_timeline.archive import JsonValue, as_object, narrow_json
from agent_team_timeline.cli import main as timeline_main
from agent_team_timeline.model import Agent, Event, TeamData
from agent_team_timeline.transcript_export import export_transcripts


def _event(
    event_id: str,
    turn_id: str,
    timestamp_ms: int,
    kind: str,
    text: str,
    source_line: int,
) -> Event:
    prompt = kind == "user_prompt"
    system = kind == "system_input"
    return Event(
        event_id=event_id,
        thread_id="root",
        turn_id=turn_id,
        timestamp_ms=timestamp_ms,
        kind=kind,
        role="user" if prompt else "system" if system else "assistant",
        phase=None,
        text=text,
        content_availability="plain",
        encrypted_content=None,
        author="user" if prompt else "system" if system else "root",
        recipient="root" if prompt or system else None,
        source_line=source_line,
        ingress_kind="typed" if prompt else "scheduled" if system else "agent",
        author_kind="owner_human" if prompt else "system" if system else "agent",
        source_native_id=event_id,
        classification_version="authorship-v1",
    )


def _team(events: tuple[Event, ...]) -> TeamData:
    return TeamData(
        team_slug="alpha",
        provider="codex",
        root_thread_id="root",
        display_timezone="America/New_York",
        sources=(),
        agents=(
            Agent(
                thread_id="root",
                parent_thread_id=None,
                agent_path="/root",
                nickname=None,
                role="coordinator",
                depth=0,
                started_at_ms=100,
                ended_at_ms=1_000,
                status="completed",
                source_path="root.jsonl",
            ),
        ),
        turns=(),
        events=events,
        tool_calls=(),
        edges=(),
    )


def _jsonl(path: Path) -> list[dict[str, JsonValue]]:
    result: list[dict[str, JsonValue]] = []
    for index, line in enumerate(path.read_text(encoding="utf-8").splitlines()):
        raw: object = json.loads(line)
        result.append(as_object(narrow_json(raw), f"record {index}"))
    return result


def test_export_is_idempotent_and_monotonic_across_missing_source_records(
    tmp_path: Path,
) -> None:
    first = _team(
        (
            _event("prompt-one", "turn-one", 200, "user_prompt", "One", 1),
            _event("response-one", "turn-one", 250, "assistant_message", "Done", 2),
            _event("scheduled", "turn-system", 300, "system_input", "Tick", 3),
            _event("prompt-two", "turn-two", 400, "user_prompt", "Two", 4),
        )
    )

    report = export_transcripts(tmp_path, (first,))
    assert report.prompts == 2
    assert report.responses == 1
    assert report.system_inputs == 1
    assert report.files_changed == 8
    assert export_transcripts(tmp_path, (first,)).files_changed == 0

    # Simulate a provider rewrite that drops an old prompt/response while exposing
    # newly discovered earlier history. The durable occurrence set only grows.
    second = _team(
        (
            _event("prompt-zero", "turn-zero", 100, "user_prompt", "Zero", 1),
            _event("prompt-two", "turn-two", 400, "user_prompt", "Two", 4),
        )
    )
    updated = export_transcripts(tmp_path, (second,))
    assert updated.prompts == 3
    assert updated.responses == 1
    assert updated.system_inputs == 1
    assert updated.carried_forward == 3

    root = tmp_path / "extracted" / "transcripts"
    prompts = _jsonl(root / "prompts.jsonl")
    assert [record["text"] for record in prompts] == ["Zero", "One", "Two"]
    assert [record["ordinal"] for record in prompts] == [1, 2, 3]
    messages = _jsonl(root / "messages.jsonl")
    response = next(record for record in messages if record["record_type"] == "response")
    prompt_one = next(record for record in prompts if record["text"] == "One")
    assert response["in_reply_to_prompt_id"] == prompt_one["record_id"]


def test_prompt_query_range_works_without_a_built_timeline(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    team = _team(
        tuple(
            _event(f"prompt-{index}", f"turn-{index}", index * 100, "user_prompt", text, index)
            for index, text in enumerate(("First", "Second", "Third", "Fourth"), 1)
        )
    )
    export_transcripts(tmp_path, (team,))
    assert not (tmp_path / "data" / "timeline.json").exists()

    assert (
        timeline_main(
            (
                "query",
                "--output",
                str(tmp_path),
                "--format",
                "text",
                "prompts",
                "--range",
                "2-3",
            )
        )
        == 0
    )
    output = capsys.readouterr().out
    assert "prompt #2" in output
    assert "Second" in output
    assert "Third" in output
    assert "First" not in output
    assert "Fourth" not in output

    assert (
        timeline_main(
            (
                "query",
                "--output",
                str(tmp_path),
                "--format",
                "jsonl",
                "prompts",
                "--range",
                "3",
            )
        )
        == 0
    )
    lines = capsys.readouterr().out.splitlines()
    assert len(lines) == 1
    record = as_object(narrow_json(json.loads(lines[0])), "query record")
    assert record["ordinal"] == 3
    assert record["text"] == "Third"


def test_archive_timeline_cli_is_discoverable_and_cwd_independent(
    tmp_path: Path,
) -> None:
    archive = tmp_path / "archive with spaces"
    archive.mkdir()
    team = _team(
        tuple(
            _event(f"prompt-{index}", f"turn-{index}", index * 100, "user_prompt", text, index)
            for index, text in enumerate(("First", "Second", "Third", "Fourth"), 1)
        )
    )
    export_transcripts(archive, (team,))
    launcher = archive / "timeline"
    assert launcher.is_file()
    assert launcher.stat().st_mode & stat.S_IXUSR

    unrelated = tmp_path / "unrelated working directory"
    unrelated.mkdir()
    top_help = subprocess.run(
        (str(launcher), "--help"),
        cwd=unrelated,
        check=False,
        capture_output=True,
        text=True,
    )
    assert top_help.returncode == 0, top_help.stderr
    assert "./timeline prompts --range 200-300" in top_help.stdout
    assert "directory containing this executable" in top_help.stdout
    for action in (
        "prompts",
        "messages",
        "teams",
        "agents",
        "phases",
        "rollups",
        "show",
        "search",
        "list",
    ):
        action_help = subprocess.run(
            (str(launcher), action, "--help"),
            cwd=unrelated,
            check=False,
            capture_output=True,
            text=True,
        )
        assert action_help.returncode == 0, action_help.stderr
        assert "options:" in action_help.stdout

    readable = subprocess.run(
        (str(launcher), "prompts", "--range", "2-3"),
        cwd=unrelated,
        check=False,
        capture_output=True,
        text=True,
    )
    assert readable.returncode == 0, readable.stderr
    assert readable.stderr == ""
    assert "prompt #2" in readable.stdout
    assert "Second" in readable.stdout
    assert "Third" in readable.stdout
    assert "First" not in readable.stdout
    assert "Fourth" not in readable.stdout

    machine = subprocess.run(
        (str(launcher), "prompts", "--range", "3", "--format", "jsonl"),
        cwd=unrelated,
        check=False,
        capture_output=True,
        text=True,
    )
    assert machine.returncode == 0, machine.stderr
    assert machine.stderr == ""
    lines = machine.stdout.splitlines()
    assert len(lines) == 1
    record = as_object(narrow_json(json.loads(lines[0])), "timeline JSONL record")
    assert record["ordinal"] == 3
    assert record["text"] == "Third"


def test_logical_prompt_report_deduplicates_identical_fork_occurrences(
    tmp_path: Path,
) -> None:
    event = _event("shared-native", "turn", 100, "user_prompt", "Shared", 1)
    first = _team((event,))
    second = replace(first, team_slug="beta")

    report = export_transcripts(tmp_path, (first, second))

    assert report.prompts == 1
    occurrences = _jsonl(
        tmp_path / "extracted" / "transcripts" / "occurrences.jsonl"
    )
    assert len(occurrences) == 2
    prompts = _jsonl(tmp_path / "extracted" / "transcripts" / "prompts.jsonl")
    assert prompts[0]["occurrence_count"] == 2
    assert prompts[0]["occurrence_teams"] == ["alpha", "beta"]


def test_prompt_query_fails_closed_on_digest_mismatch(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    export_transcripts(
        tmp_path,
        (_team((_event("prompt", "turn", 100, "user_prompt", "Original", 1),)),),
    )
    prompts = tmp_path / "extracted" / "transcripts" / "prompts.jsonl"
    prompts.write_text(prompts.read_text(encoding="utf-8") + "{}\n", encoding="utf-8")

    assert (
        timeline_main(
            ("query", "--output", str(tmp_path), "prompts")
        )
        == 2
    )
    assert "generation is incomplete" in capsys.readouterr().err
