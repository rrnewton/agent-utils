"""CLI behavior for optional GitHub pull metadata."""

from __future__ import annotations

from pathlib import Path

import pytest

import wrkviz.cli as timeline_cli
from wrkviz.github_enrich import PullMetadataReport


def test_partial_metadata_failure_still_rebuilds_successes(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    builds: list[tuple[Path, str, int]] = []

    def fake_enrich(
        archive: Path,
        team_slug: str,
        *,
        timeout_seconds: float,
    ) -> PullMetadataReport:
        del timeout_seconds
        return PullMetadataReport(
            references=2,
            distinct_pulls=2,
            fetched=1,
            not_modified=0,
            failures=("owner/repository#2: GitHub API returned HTTP 404",),
            cache_path=str(archive / "pulls.json"),
        )

    def fake_build(archive: Path, team_slug: str, phase_minutes: int) -> dict[str, int]:
        builds.append((archive, team_slug, phase_minutes))
        return {"files_changed": 1}

    monkeypatch.setattr(timeline_cli, "enrich_pull_request_metadata", fake_enrich)
    monkeypatch.setattr(timeline_cli, "build_archive", fake_build)

    result = timeline_cli.main(
        [
            "github-metadata",
            "--output",
            str(tmp_path),
            "--team",
            "test-team",
            "--phase-minutes",
            "45",
        ]
    )

    assert result == 2
    assert builds == [(tmp_path, "test-team", 45)]


def test_the_token_option_is_gone(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    # gh supplies the credential; wrkviz reads no token (2026-10-19).
    called = False

    def fake_enrich(archive: Path, team_slug: str, *, timeout_seconds: float) -> PullMetadataReport:
        nonlocal called
        called = True
        raise AssertionError("metadata fetch must not start")

    monkeypatch.setattr(timeline_cli, "enrich_pull_request_metadata", fake_enrich)

    with pytest.raises(SystemExit) as raised:
        timeline_cli.main(
            [
                "github-metadata",
                "--output",
                str(tmp_path),
                "--team",
                "test-team",
                "--github-token-env",
                "SOME_TOKEN",
            ]
        )

    assert raised.value.code == 2
    assert "--github-token-env" in capsys.readouterr().err
    assert not called
