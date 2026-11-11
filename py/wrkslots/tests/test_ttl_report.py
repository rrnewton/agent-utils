"""TTL escalation ladder and the read-only ``ttl-report`` (#1293).

Every clock here is a fixed value passed to the code; nothing reads the
wall clock, and the cost assertions count files and rows, never time.
"""

from __future__ import annotations

import datetime as dt
import json
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_census_cost import _config

NOW = dt.datetime(2026, 11, 14, 12, 0, tzinfo=dt.timezone.utc)


def _record(slot: str, heartbeat_at: str, *, branch: str | None = None) -> cli.ActiveRecord:
    coordinator = cli.ProcessIdentity(1, 1, "boot", "host", "/fixture")
    checkouts: tuple[cli.Checkout, ...] = ()
    if branch is not None:
        checkouts = (
            cli.Checkout(
                name="repo",
                path=f"worktrees/{slot}/repo",
                repository="repo",
                branch=branch,
                start_point="refs/remotes/origin/main",
                remote="origin",
                remote_url_sha256="0" * 64,
                landed_ref="refs/remotes/origin/main",
                head="1" * 40,
            ),
        )
    return cli.ActiveRecord(
        slot=slot,
        agent=f"agent-{slot}",
        task=f"task-{slot}",
        purpose="ttl report fixture",
        slot_type="agent",
        machine="testhost",
        generation=1,
        created_at="2026-11-01T00:00:00+00:00",
        heartbeat_at=heartbeat_at,
        heartbeat_ttl_seconds=3600,
        owner=None,
        coordinator_lease=coordinator,
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=checkouts,
    )


def test_stage_boundaries_use_the_fixed_clock() -> None:
    thresholds = cli._ttl_thresholds(0, 86_400, 172_800, 259_200)
    expired = dt.datetime(2026, 11, 11, 12, 0, tzinfo=dt.timezone.utc)
    assert cli._ttl_stage(expired, expired - dt.timedelta(seconds=1), thresholds) == "current"
    assert cli._ttl_stage(expired, expired, thresholds) == "warn"
    assert cli._ttl_stage(expired, expired + dt.timedelta(days=1), thresholds) == "alarm"
    assert cli._ttl_stage(expired, expired + dt.timedelta(days=2), thresholds) == "stop"
    assert cli._ttl_stage(expired, expired + dt.timedelta(days=3), thresholds) == "teardown"
    assert cli._ttl_stage(expired, expired + dt.timedelta(days=30), thresholds) == "teardown"


def test_thresholds_must_not_decrease() -> None:
    with pytest.raises(cli.Refusal):
        cli._ttl_thresholds(0, 100, 50, 200)


def _report(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, *extra: str
) -> tuple[int, str]:
    config = _config(tmp_path)
    state = cli.ActiveState(
        machine="testhost",
        revision=1,
        slots=(
            _record("fresh", "2026-11-14T11:30:00+00:00"),
            _record("stale", "2026-11-10T00:00:00+00:00", branch="feat/x"),
        ),
    )
    monkeypatch.setattr(cli, "_load_config", lambda *_args: config)
    monkeypatch.setattr(cli, "_load_active_snapshot", lambda *_args, **_kw: state)
    before = sorted(path.relative_to(tmp_path) for path in tmp_path.rglob("*"))
    result = cli.main(
        [
            "--project-root", str(config.root), "ttl-report",
            "--now", NOW.isoformat(), *extra,
        ]
    )
    after = sorted(path.relative_to(tmp_path) for path in tmp_path.rglob("*"))
    assert before == after, "ttl-report must not write anything"
    return result, config


def test_report_names_rescue_ref_bundle_and_teardown_time(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    result, config = _report(tmp_path, monkeypatch, "--format", "json")
    assert result == 0, capsys.readouterr().err
    payload = json.loads(capsys.readouterr().out)
    rows = {row["slot"]: row for row in payload["slots"]}
    assert sorted(rows) == ["fresh", "stale"]
    assert rows["fresh"]["stage"] == "current"
    stale = rows["stale"]
    # Heartbeat 2026-11-10T00:00Z + 1 h TTL expired 2026-11-10T01:00Z;
    # at the fixed now it is past the T+3 d teardown rung.
    assert stale["stage"] == "teardown"
    assert stale["expired_at"] == "2026-11-10T01:00:00+00:00"
    assert stale["teardown_at"] == "2026-11-13T01:00:00+00:00"
    (checkout,) = stale["checkouts"]
    assert checkout["rescue_ref"] == "refs/heads/rescue/stale/20261114/feat/x"
    assert checkout["bundle"] == str(config.control / "ttl-rescue" / "stale.bundle")


def test_only_overdue_keeps_expired_rows(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
    capsys: pytest.CaptureFixture[str],
) -> None:
    result, _config_used = _report(
        tmp_path, monkeypatch, "--format", "json", "--only-overdue"
    )
    assert result == 0, capsys.readouterr().err
    payload = json.loads(capsys.readouterr().out)
    assert [row["slot"] for row in payload["slots"]] == ["stale"]
