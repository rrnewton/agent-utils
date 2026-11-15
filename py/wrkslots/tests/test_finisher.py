"""Finisher plan partitioning and refusal tests (#1568).

Candidate gathering is monkeypatched: these tests pin the finisher's
own contract (dry-run by default, bounded, delegation to
retire-pending only with a coordinator PID, skipped slots keep their
reasons).  The removal guards themselves are covered by the
lifecycle suite; the finisher adds none and removes none.
"""

from __future__ import annotations

import argparse
import datetime as dt
from typing import cast

import pytest

from wrkslots import cli

NOW = dt.datetime(2026, 11, 15, 12, 0, tzinfo=dt.timezone.utc)


def _candidate(
    slot: str, *, blocked: str | None = None, machine: str = "testhost"
) -> cli._RetirementCandidate:
    return cli._RetirementCandidate(
        machine=machine,
        slot=slot,
        generation=1,
        sha256="0" * 64,
        event_sequence=1,
        enqueued_at="2026-11-14T00:00:00+00:00",
        last_attempt_sequence=None,
        artifact_path=None,
        artifact_source=None,
        blocked_reason=blocked,
    )


def _args(**overrides: object) -> argparse.Namespace:
    values: dict[str, object] = {
        "project_root": None,
        "machine": None,
        "wait_lock": 0.0,
        "limit": 8,
        "apply": False,
        "coordinator_pid": None,
        "coordinator_authorized": False,
        "archive_private_history": False,
        "archive_drifted_checkouts": False,
        "now": None,
        "warn_after_seconds": cli._TTL_WARN_AFTER_SECONDS,
        "alarm_after_seconds": cli._TTL_ALARM_AFTER_SECONDS,
        "stop_after_seconds": cli._TTL_STOP_AFTER_SECONDS,
        "teardown_after_seconds": cli._TTL_TEARDOWN_AFTER_SECONDS,
        "format": "json",
    }
    values.update(overrides)
    return argparse.Namespace(**values)


def test_finisher_rejects_bad_limit_before_touching_state() -> None:
    with pytest.raises(cli.Refusal, match="--limit"):
        cli._cmd_finisher(_args(limit=0))
    with pytest.raises(cli.Refusal, match="--limit"):
        cli._cmd_finisher(_args(limit=cli.RETIRE_PENDING_LIMIT + 1))


def test_finisher_apply_requires_coordinator_pid() -> None:
    with pytest.raises(cli.Refusal, match="requires --coordinator-pid"):
        cli._cmd_finisher(_args(apply=True))


def test_finisher_plan_partitions_candidates(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    config = cli.Config.__new__(cli.Config)
    object.__setattr__(config, "machine", "testhost")
    monkeypatch.setattr(
        cli,
        "_current_retirement_candidates",
        lambda *_a, **_k: (
            _candidate("queued-ok"),
            _candidate("queued-held", blocked="slot is held: keep"),
            _candidate("queued-foreign", machine="otherhost"),
        ),
    )
    monkeypatch.setattr(
        cli,
        "_expired_unhanded_candidates",
        lambda *_a, **_k: (
            cli._ExpiredRetirementCandidate(
                machine="testhost",
                slot="expired-ok",
                generation=1,
                agent="agent-a",
                classification="attemptable",
                reason="",
            ),
            cli._ExpiredRetirementCandidate(
                machine="testhost",
                slot="expired-unread",
                generation=1,
                agent="agent-b",
                classification="action",
                reason="read the handoff first",
            ),
        ),
    )
    monkeypatch.setattr(
        cli,
        "_load_active_snapshot",
        lambda *_a, **_k: cli.ActiveState(machine="testhost", revision=1, slots=()),
    )
    monkeypatch.setattr(
        cli,
        "_ttl_report_rows",
        lambda *_a, **_k: [
            {
                "slot": "queued-ok",
                "agent": "agent-a",
                "stage": "teardown",
                "blocked": None,
                "teardown_at": NOW.isoformat(),
            },
            {
                "slot": "ttl-only",
                "agent": "agent-c",
                "stage": "teardown",
                "blocked": "blocked: held",
                "teardown_at": NOW.isoformat(),
            },
            {
                "slot": "ttl-early",
                "agent": "agent-d",
                "stage": "alarm",
                "blocked": None,
                "teardown_at": NOW.isoformat(),
            },
        ],
    )
    monkeypatch.setattr(cli, "_retire_freeze", lambda _config: None)
    thresholds = cli._ttl_thresholds(0, 86_400, 172_800, 259_200)
    plan = cli._finisher_plan(config, 1.0, thresholds, NOW)
    would = cast(list[dict[str, object]], plan["would_retire"])
    assert [item["slot"] for item in would] == ["queued-ok", "expired-ok"]
    skipped_rows = cast(list[dict[str, object]], plan["skipped"])
    skipped = {str(item["slot"]): str(item["reason"]) for item in skipped_rows}
    assert skipped["queued-held"] == "slot is held: keep"
    assert "foreign" in skipped["queued-foreign"]
    action_rows = cast(list[dict[str, object]], plan["actions"])
    assert [item["slot"] for item in action_rows] == ["expired-unread"]
    # A slot already planned for retirement is not listed twice as a
    # TTL candidate, and a pre-teardown rung is never a candidate.
    ttl_rows = cast(list[dict[str, object]], plan["ttl_teardown_candidates"])
    assert [item["slot"] for item in ttl_rows] == ["ttl-only"]
    assert plan["frozen"] is None


def test_finisher_captured_run_reports_refusal() -> None:
    code, payload = cli._finisher_run_captured(
        ["retire-pending", "--limit", "0", "--coordinator-pid", "1"]
    )
    assert code == 3
    assert payload is not None and payload["status"] == "refused"


def test_finisher_is_on_the_cli_surface(capsys: pytest.CaptureFixture[str]) -> None:
    with pytest.raises(SystemExit) as excinfo:
        cli.main(["finisher", "--help"])
    assert excinfo.value.code == 0
    out = capsys.readouterr().out
    assert "Dry-run by default" in out
    assert "--apply" in out
