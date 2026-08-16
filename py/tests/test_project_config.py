"""Tests for strict, zero-model project ingestion manifests."""

from __future__ import annotations

from collections.abc import Callable, Sequence
import json
from pathlib import Path
from typing import cast

import pytest

import agent_team_timeline.cli as cli_module
import agent_team_timeline.project_config as project_config_module
from agent_team_timeline.cli import main as timeline_main
from agent_team_timeline.identity import IdentityOverrides
from agent_team_timeline.model import TeamData
from agent_team_timeline.orc import OrcContinuationSpec
from agent_team_timeline.pipeline import IngestReport
from agent_team_timeline.project_config import (
    ClaudeProjectSource,
    CodexProjectSource,
    OrcProjectSource,
    ProjectIngestReport,
    ProjectTeamIngestResult,
    ingest_project,
    load_project_ingest_config,
)
from agent_team_timeline.transcript_export import TranscriptExportReport
from agent_team_timeline.transcript_export import PromptAuthorshipRule
from agent_team_timeline.window import DateWindow


def _manifest(output: str = "../summary/hermit") -> dict[str, object]:
    return {
        "schema_version": 1,
        "output": output,
        "timezone": "America/New_York",
        "projects": [
            {
                "label": "dev-hermit",
                "repository_url": "https://github.com/rrnewton/dev-hermit.git",
            }
        ],
        "source_hosts": ["shared.example.com"],
        "window": {"start_date": "2026-08-01", "end_date": "2026-08-13"},
        "teams": [
            {
                "slug": "codex-team",
                "provider": "codex",
                "source_hosts": ["codex.example.com"],
                "source": {
                    "sessions_root": "../raw/devbig014/.codex/sessions",
                    "root_session": "codex-root",
                    "continuation_sessions": ["codex-next", "codex-last"],
                },
            },
            {
                "slug": "claude-team",
                "provider": "claude",
                "timezone": "UTC",
                "source": {
                    "session_file": "../raw/devbig176/.claude/projects/p/root.jsonl"
                },
            },
            {
                "slug": "orc-team",
                "provider": "orc",
                "projects": [],
                "window": {},
                "prompt_authorship_rules": [
                    {
                        "id": "legacy-owner-web",
                        "ingress_kind": "submitted_web",
                        "author_kind": "owner_human",
                        "reason": "This audited legacy interval was owner-only.",
                        "end_time": "2026-08-10T00:00:00Z",
                    }
                ],
                "source": {
                    "source_root": "../raw/devbig014",
                    "root_session": "orc-root",
                    "continuation_sessions": [
                        "orc-next",
                        {
                            "session_id": "orc-last",
                            "start_message_id": "restart-message",
                        },
                    ],
                },
            },
        ],
    }


def _write_manifest(tmp_path: Path, value: dict[str, object]) -> Path:
    path = tmp_path / "configs" / "hermit.json"
    path.parent.mkdir(parents=True)
    path.write_text(json.dumps(value), encoding="utf-8")
    return path


def _report(team_slug: str) -> IngestReport:
    return IngestReport(team_slug, "a" * 64, 1, 10, 2, 3, 4, 1, 1)


def _orc_spec(value: str | OrcContinuationSpec) -> OrcContinuationSpec:
    return (
        value
        if isinstance(value, OrcContinuationSpec)
        else OrcContinuationSpec.from_value(value, "test")
    )


def _set_new_schema(value: dict[str, object]) -> None:
    value["schema_version"] = 2


def _add_unknown_root(value: dict[str, object]) -> None:
    value["unexpected"] = True


def _set_absolute_output(value: dict[str, object]) -> None:
    value["output"] = "/tmp/unsafe"


def _add_unknown_source(value: dict[str, object]) -> None:
    teams = cast(list[dict[str, object]], value["teams"])
    source = cast(dict[str, object], teams[0]["source"])
    source["mystery"] = "x"


def test_load_project_config_resolves_paths_and_provider_schema(tmp_path: Path) -> None:
    config_path = _write_manifest(tmp_path, _manifest())

    config = load_project_ingest_config(config_path)

    assert config.output == (tmp_path / "summary" / "hermit").resolve()
    assert tuple(team.slug for team in config.teams) == (
        "codex-team",
        "claude-team",
        "orc-team",
    )
    codex, claude, orc = config.teams
    assert isinstance(codex.source, CodexProjectSource)
    assert codex.source.sessions_root == (
        tmp_path / "raw" / "devbig014" / ".codex" / "sessions"
    ).resolve()
    assert codex.source.continuation_sessions == ("codex-next", "codex-last")
    assert codex.date_window is not None
    assert codex.date_window.start_date == "2026-08-01"
    assert codex.identity_overrides.projects[0].repository_url == (
        "https://github.com/rrnewton/dev-hermit"
    )
    assert codex.identity_overrides.hosts[0].hostname == "codex.example.com"
    assert isinstance(claude.source, ClaudeProjectSource)
    assert claude.timezone == "UTC"
    assert isinstance(orc.source, OrcProjectSource)
    assert orc.source.continuation_sessions == (
        OrcContinuationSpec("orc-next"),
        OrcContinuationSpec("orc-last", "restart-message"),
    )
    assert orc.date_window is None
    assert orc.identity_overrides.projects == ()
    assert orc.prompt_authorship_rules[0].rule_id == "legacy-owner-web"
    assert orc.prompt_authorship_rules[0].end_ms == 1786320000000


@pytest.mark.parametrize(
    ("mutate", "match"),
    (
        (_set_new_schema, "unsupported schema_version"),
        (_add_unknown_root, "unknown=\\['unexpected'\\]"),
        (_set_absolute_output, "output must be relative"),
        (_add_unknown_source, "unknown=\\['mystery'\\]"),
    ),
)
def test_project_config_rejects_schema_drift(
    tmp_path: Path,
    mutate: Callable[[dict[str, object]], None],
    match: str,
) -> None:
    value = _manifest()
    mutate(value)
    path = _write_manifest(tmp_path, value)
    with pytest.raises(ValueError, match=match):
        load_project_ingest_config(path)


def test_project_config_rejects_root_repeated_as_orc_continuation(
    tmp_path: Path,
) -> None:
    value = _manifest()
    teams = cast(list[dict[str, object]], value["teams"])
    source = cast(dict[str, object], teams[2]["source"])
    source["continuation_sessions"] = ["orc-next", "orc-root"]

    with pytest.raises(ValueError, match="root_session cannot also be a continuation"):
        load_project_ingest_config(_write_manifest(tmp_path, value))


def test_project_config_keeps_orc_continuations_optional(tmp_path: Path) -> None:
    value = _manifest()
    teams = cast(list[dict[str, object]], value["teams"])
    source = cast(dict[str, object], teams[2]["source"])
    del source["continuation_sessions"]

    config = load_project_ingest_config(_write_manifest(tmp_path, value))
    orc = config.teams[2].source

    assert isinstance(orc, OrcProjectSource)
    assert orc.continuation_sessions == ()


@pytest.mark.parametrize(
    ("continuations", "match"),
    (
        (
            [
                "orc-next",
                {
                    "session_id": "orc-next",
                    "start_message_id": "restart-message",
                },
            ],
            "duplicate session ids",
        ),
        (
            [{"session_id": "orc-next", "start_message_id": None}],
            "expected a non-empty string",
        ),
    ),
)
def test_project_config_rejects_noncanonical_orc_continuation_specs(
    tmp_path: Path,
    continuations: list[object],
    match: str,
) -> None:
    value = _manifest()
    teams = cast(list[dict[str, object]], value["teams"])
    source = cast(dict[str, object], teams[2]["source"])
    source["continuation_sessions"] = continuations

    with pytest.raises(ValueError, match=match):
        load_project_ingest_config(_write_manifest(tmp_path, value))


@pytest.mark.parametrize("command", ("ingest-orc", "refresh-orc"))
def test_orc_cli_accepts_ordered_repeatable_continuation_sessions(
    command: str,
) -> None:
    namespace = cli_module._parser().parse_args(
        [
            command,
            "--output",
            "/tmp/archive",
            "--team",
            "orc-test",
            "--source-root",
            "/tmp/source",
            "--root-session",
            "orc-root",
            "--continuation-session",
            "orc-next",
            "--continuation-session",
            "orc-last",
        ]
    )

    assert namespace.continuation_session == ["orc-next", "orc-last"]


def test_orc_cli_parses_legacy_and_bounded_continuation_specs() -> None:
    assert cli_module._orc_continuation_arg("orc-next") == OrcContinuationSpec(
        "orc-next"
    )
    assert cli_module._orc_continuation_arg(
        '{"session_id":"orc-last","start_message_id":"restart-message"}'
    ) == OrcContinuationSpec("orc-last", "restart-message")
    with pytest.raises(ValueError, match="expected a non-empty string"):
        cli_module._orc_continuation_arg(
            '{"session_id":"orc-last","start_message_id":null}'
        )


def test_orc_cli_dispatches_ordered_continuation_sessions(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    received: list[tuple[str, ...]] = []

    def fake_orc(
        archive: Path,
        source_root: Path,
        root_session_id: str,
        team_slug: str,
        display_timezone: str,
        date_window: DateWindow | None = None,
        identity_overrides: IdentityOverrides | None = None,
        continuation_specs: Sequence[str | OrcContinuationSpec] = (),
    ) -> tuple[TeamData, IngestReport]:
        normalized = tuple(_orc_spec(spec) for spec in continuation_specs)
        received.append(
            tuple(
                spec.session_id
                + (
                    "=" + spec.start_message_id
                    if spec.start_message_id is not None
                    else ""
                )
                for spec in normalized
            )
        )
        return cast(TeamData, object()), _report(team_slug)

    monkeypatch.setattr(cli_module, "ingest_orc", fake_orc)

    assert timeline_main(
        [
            "ingest-orc",
            "--output",
            str(tmp_path / "archive"),
            "--team",
            "orc-test",
            "--source-root",
            str(tmp_path / "source"),
            "--root-session",
            "orc-root",
            "--continuation-session",
            "orc-next",
            "--continuation-session",
            '{"session_id":"orc-last","start_message_id":"restart-message"}',
        ]
    ) == 0
    assert received == [("orc-next", "orc-last=restart-message")]


def test_project_ingest_dispatches_all_providers_then_extracts_all(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = load_project_ingest_config(_write_manifest(tmp_path, _manifest()))
    calls: list[tuple[str, ...]] = []

    def fake_codex(
        archive: Path,
        sessions_root: Path,
        root_thread_id: str,
        team_slug: str,
        display_timezone: str,
        date_window: DateWindow | None = None,
        identity_overrides: IdentityOverrides | None = None,
        continuation_thread_ids: Sequence[str] = (),
    ) -> tuple[TeamData, IngestReport]:
        calls.append(
            (
                "codex",
                str(archive),
                str(sessions_root),
                root_thread_id,
                team_slug,
                display_timezone,
                *continuation_thread_ids,
            )
        )
        return cast(TeamData, object()), _report(team_slug)

    def fake_claude(
        archive: Path,
        session_file: Path,
        team_slug: str,
        display_timezone: str,
        date_window: DateWindow | None = None,
        identity_overrides: IdentityOverrides | None = None,
    ) -> tuple[TeamData, IngestReport]:
        calls.append(
            (
                "claude",
                str(archive),
                str(session_file),
                team_slug,
                display_timezone,
            )
        )
        return cast(TeamData, object()), _report(team_slug)

    def fake_orc(
        archive: Path,
        source_root: Path,
        root_session_id: str,
        team_slug: str,
        display_timezone: str,
        date_window: DateWindow | None = None,
        identity_overrides: IdentityOverrides | None = None,
        continuation_specs: Sequence[str | OrcContinuationSpec] = (),
    ) -> tuple[TeamData, IngestReport]:
        normalized = tuple(_orc_spec(spec) for spec in continuation_specs)
        calls.append(
            (
                "orc",
                str(archive),
                str(source_root),
                root_session_id,
                team_slug,
                display_timezone,
                *(
                    spec.session_id
                    + (
                        "=" + spec.start_message_id
                        if spec.start_message_id is not None
                        else ""
                    )
                    for spec in normalized
                ),
            )
        )
        return cast(TeamData, object()), _report(team_slug)

    def fake_extract(
        archive: Path,
        team_slugs: Sequence[str] = (),
        authorship_rules: Sequence[PromptAuthorshipRule] | None = None,
    ) -> TranscriptExportReport:
        calls.append(
            (
                "extract",
                str(archive),
                *(rule.rule_id for rule in authorship_rules or ()),
                *team_slugs,
            )
        )
        return TranscriptExportReport(3, 20, 30, 4, 0, 2)

    monkeypatch.setattr(project_config_module, "ingest_codex", fake_codex)
    monkeypatch.setattr(project_config_module, "ingest_claude", fake_claude)
    monkeypatch.setattr(project_config_module, "ingest_orc", fake_orc)
    monkeypatch.setattr(project_config_module, "extract_transcripts_archive", fake_extract)

    result = ingest_project(config)

    assert tuple(item.team_slug for item in result.teams) == (
        "codex-team",
        "claude-team",
        "orc-team",
    )
    assert [call[0] for call in calls] == ["codex", "claude", "orc", "extract"]
    assert calls[2][-2:] == ("orc-next", "orc-last=restart-message")
    assert calls[-1] == ("extract", str(config.output), "legacy-owner-web")
    assert result.to_json_obj()["model_calls"] == 0
    assert result.to_json_obj()["website_build_performed"] is False


def test_project_config_team_filter_is_strict(tmp_path: Path) -> None:
    config = load_project_ingest_config(_write_manifest(tmp_path, _manifest()))
    assert tuple(
        team.slug
        for team in config.select_teams(("orc-team", "codex-team"))
    ) == ("codex-team", "orc-team")
    with pytest.raises(ValueError, match="not registered"):
        config.select_teams(("missing-team",))
    with pytest.raises(ValueError, match="duplicates"):
        config.select_teams(("codex-team", "codex-team"))


def test_ingest_project_cli_records_zero_model_contract(
    tmp_path: Path,
    monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    config_path = _write_manifest(tmp_path, _manifest())

    def fake_ingest(
        config: project_config_module.ProjectIngestConfig,
        requested_teams: Sequence[str] = (),
    ) -> ProjectIngestReport:
        assert requested_teams == ("codex-team",)
        return ProjectIngestReport(
            config.config_sha256,
            (
                ProjectTeamIngestResult(
                    "codex-team", "codex", _report("codex-team")
                ),
            ),
            TranscriptExportReport(1, 7, 8, 1, 0, 4),
        )

    monkeypatch.setattr(cli_module, "ingest_project", fake_ingest)

    assert timeline_main(
        ["ingest-project", "--config", str(config_path), "--team", "codex-team"]
    ) == 0
    output = capsys.readouterr().out
    assert "7 prompts" in output
    assert "website: not built" in output
    run_paths = tuple((tmp_path / "summary" / "hermit" / "runs").glob("*.json"))
    assert len(run_paths) == 1
    run = json.loads(run_paths[0].read_text(encoding="utf-8"))
    assert run["mechanical"]["project_ingest"]["model_calls"] == 0
    assert run["mechanical"]["project_ingest"]["website_build_performed"] is False
