"""TTL live-owner / HOLD guards (#1491) and heartbeat nested warning (#1492).

Every clock is injected (``now``) and liveness is monkeypatched; no
wall-clock waits.  Scratch fixture roots only.
"""

from __future__ import annotations

import datetime as dt
import os
import subprocess
from collections.abc import Iterator
from pathlib import Path

import pytest

from wrkslots import cli
from wrkslots.tests.test_census_cost import _config
from wrkslots.tests.test_ttl_rescue import _git, _remote_ref, _repo

NOW = dt.datetime(2026, 11, 14, 12, 0, tzinfo=dt.timezone.utc)
THRESHOLDS = cli._ttl_thresholds(0, 86_400, 172_800, 259_200)


def _record(
    config: cli.Config,
    repo: Path,
    remote: Path,
    *,
    owner: cli.ProcessIdentity | None = None,
) -> cli.ActiveRecord:
    coordinator = cli.ProcessIdentity(1, 1, "boot", "host", "/fixture")
    if repo.exists():
        # Idempotent: the slot repo may already have origin.
        proc = subprocess.run(
            ["git", "-C", str(repo), "remote", "get-url", "origin"],
            capture_output=True, text=True, check=False,
        )
        if proc.returncode != 0:
            _git(repo, "remote", "add", "origin", str(remote))
    return cli.ActiveRecord(
        slot="slot-f",
        agent="agent-slot-f",
        task="task-slot-f",
        purpose="ttl guard fixture",
        slot_type="agent",
        machine="testhost",
        generation=1,
        created_at="2026-11-01T00:00:00+00:00",
        heartbeat_at="2026-11-10T00:00:00+00:00",
        heartbeat_ttl_seconds=3600,
        owner=owner,
        coordinator_lease=coordinator,
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=(
            cli.Checkout(
                name="repo",
                path=repo.relative_to(config.root).as_posix(),
                repository="repo",
                branch="feat/z",
                start_point="refs/remotes/origin/main",
                remote="origin",
                remote_url_sha256="0" * 64,
                landed_ref="refs/remotes/origin/main",
                head="1" * 40,
            ),
        ),
    )


def _live_owner() -> cli.ProcessIdentity:
    return cli.ProcessIdentity(os.getpid(), 1, "boot", "host", "/fixture")


def _patch_dead(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(cli, "_process_state", lambda _id: ("dead", "proven dead"))
    monkeypatch.setattr(
        cli,
        "_owner_liveness_states",
        lambda _cfg, records, deadline=None: {
            (r.machine, r.slot, r.generation): ("dead", "proven dead")
            for r in records
        },
    )
    monkeypatch.setattr(cli, "_load_hold", lambda *_a, **_k: None)


def _patch_live(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(cli, "_process_state", lambda _id: ("live", "PID live"))
    monkeypatch.setattr(
        cli,
        "_owner_liveness_states",
        lambda _cfg, records, deadline=None: {
            (r.machine, r.slot, r.generation): ("dead", "proven dead")
            for r in records
        },
    )
    monkeypatch.setattr(cli, "_load_hold", lambda *_a, **_k: None)


def _patch_held(monkeypatch: pytest.MonkeyPatch) -> None:
    _patch_dead(monkeypatch)
    monkeypatch.setattr(
        cli, "_load_hold", lambda *_a, **_k: {"reason": "do not touch"}
    )


def _make_slot(tmp_path: Path) -> tuple[cli.Config, cli.ActiveRecord, Path, Path]:
    config = _config(tmp_path)
    remote = tmp_path / "remote.git"
    subprocess.run(["git", "init", "-q", "--bare", str(remote)], check=True)
    repo = config.worktrees / "slot-f" / "repo"
    repo.mkdir(parents=True)
    _git(repo, "init", "-q")
    _git(repo, "-c", "user.name=fixture", "-c", "user.email=f@x.invalid",
         "commit", "-q", "--allow-empty", "-m", "base")
    _git(repo, "checkout", "-q", "-b", "feat/z")
    (repo / "work.txt").write_text("stranded\n", encoding="utf-8")
    record = _record(config, repo, remote, owner=_live_owner())
    return config, record, repo, remote


def test_live_owner_past_teardown_refused_without_rescue_side_effects(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config, record, repo, _remote = _make_slot(tmp_path)
    _patch_live(monkeypatch)
    slot_dir = config.worktrees / "slot-f"
    assert slot_dir.exists()
    with pytest.raises(cli.Refusal) as caught:
        cli._execute_ttl_slot_teardown(
            config, record, now=NOW, thresholds=THRESHOLDS
        )
    assert "slot-f" in str(caught.value)
    assert "blocked: owner live" in str(caught.value)
    assert slot_dir.exists()
    assert (repo / "work.txt").read_text(encoding="utf-8") == "stranded\n"
    # No rescue bundle was written.
    assert not list((config.control / "ttl-rescue").glob("*")) if (
        config.control / "ttl-rescue"
    ).exists() else True
    with pytest.raises(cli.Refusal):
        cli._execute_ttl_slot_rescue(
            config, record, now=NOW, thresholds=THRESHOLDS
        )
    assert slot_dir.exists()


def test_held_slot_refused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config, record, _repo_path, _remote = _make_slot(tmp_path)
    _patch_held(monkeypatch)
    with pytest.raises(cli.Refusal) as caught:
        cli._execute_ttl_slot_teardown(
            config, record, now=NOW, thresholds=THRESHOLDS
        )
    assert "slot-f" in str(caught.value)
    assert "blocked: held" in str(caught.value)
    with pytest.raises(cli.Refusal):
        cli._execute_ttl_slot_rescue(
            config, record, now=NOW, thresholds=THRESHOLDS
        )


def test_live_owner_refused_regardless_of_ttl_stage(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Liveness is checked first: even at warn/alarm, live refuses by name."""
    config, record, _repo_path, _remote = _make_slot(tmp_path)
    _patch_live(monkeypatch)
    warn_now = dt.datetime(2026, 11, 11, 12, 0, tzinfo=dt.timezone.utc)
    stage, _exp, _td = cli._ttl_slot_stage(record, warn_now, THRESHOLDS)
    assert stage in ("warn", "alarm", "stop")
    with pytest.raises(cli.Refusal) as caught:
        cli._execute_ttl_slot_rescue(
            config, record, now=warn_now, thresholds=THRESHOLDS
        )
    assert "blocked: owner live" in str(caught.value)
    with pytest.raises(cli.Refusal) as caught2:
        cli._execute_ttl_slot_teardown(
            config, record, now=warn_now, thresholds=THRESHOLDS
        )
    assert "blocked: owner live" in str(caught2.value)


def test_dead_owner_no_hold_tears_down_as_today(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config, record, _repo_path, remote = _make_slot(tmp_path)
    _patch_dead(monkeypatch)
    receipt = cli._execute_ttl_slot_teardown(
        config, record, now=NOW, thresholds=THRESHOLDS
    )
    assert receipt["removed"] is True
    assert not (config.worktrees / "slot-f").exists()
    entries = {
        e["branch"]: e
        for c in receipt["rescue"]["checkouts"]
        for e in c["rescued"]
    }
    assert entries["feat/z"]["pushed"] is True
    assert _remote_ref(remote, entries["feat/z"]["rescue_ref"])


def test_report_marks_live_and_held_as_blocked(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    config = _config(tmp_path)
    coordinator = cli.ProcessIdentity(1, 1, "boot", "host", "/fixture")

    def bare_record(
        slot: str, owner: cli.ProcessIdentity | None
    ) -> cli.ActiveRecord:
        return cli.ActiveRecord(
            slot=slot, agent=f"agent-{slot}", task=f"task-{slot}",
            purpose="report fixture", slot_type="agent", machine="testhost",
            generation=1, created_at="2026-11-01T00:00:00+00:00",
            heartbeat_at="2026-11-10T00:00:00+00:00",
            heartbeat_ttl_seconds=3600, owner=owner,
            coordinator_lease=coordinator, coordinator_recovery_note=None,
            handoff=None, checkouts=(),
        )

    state = cli.ActiveState(
        machine="testhost", revision=1,
        slots=(bare_record("live-slot", _live_owner()),
               bare_record("held-slot", None),
               bare_record("free-slot", None)),
    )
    holds = {"held-slot": {"reason": "keep"}}
    monkeypatch.setattr(
        cli, "_load_hold",
        lambda _cfg, slot, _m=None, **_k: holds.get(slot),
    )
    monkeypatch.setattr(
        cli, "_process_state",
        lambda ident: ("live", "PID live"),
    )

    def liveness(_cfg: object, records: object, deadline: object = None) -> dict:
        out = {}
        for r in records:  # type: ignore[attr-defined]
            out[(r.machine, r.slot, r.generation)] = ("dead", "dead")
        return out

    monkeypatch.setattr(cli, "_owner_liveness_states", liveness)
    rows = cli._ttl_report_rows(config, state, NOW, THRESHOLDS)
    by_slot = {row["slot"]: row for row in rows}
    assert by_slot["live-slot"]["stage"] == "teardown"
    # Past expiry a live owner reads "expired (owner live)" with no ladder
    # step; the blocked field still carries the teardown refusal label.
    assert by_slot["live-slot"]["next_action"] == "expired (owner live)"
    assert by_slot["live-slot"]["blocked"] == "blocked: owner live"
    assert by_slot["held-slot"]["next_action"] == "blocked: held"
    assert by_slot["held-slot"]["blocked"] == "blocked: held"
    assert by_slot["free-slot"]["blocked"] is None
    assert by_slot["free-slot"]["next_action"] not in (
        "blocked: owner live", "expired (owner live)"
    )


def test_owner_becoming_live_during_rescue_refused_at_recheck(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import argparse
    import contextlib

    config = _config(tmp_path)
    record = _record(
        config, config.worktrees / "slot-f" / "repo", tmp_path / "r.git",
        owner=_live_owner(),
    )
    state = cli.ActiveState(machine="testhost", revision=7, slots=(record,))
    calls = {"n": 0}
    monkeypatch.setattr(cli, "_load_config", lambda *_a: config)
    monkeypatch.setattr(cli, "_load_active_snapshot", lambda *_a, **_k: state)
    monkeypatch.setattr(cli, "_load_active", lambda *_a, **_k: state)
    monkeypatch.setattr(
        cli, "_load_archive",
        lambda *_a, **_k: cli.ArchiveState(machine="testhost", revision=1, records=()),
    )
    monkeypatch.setattr(cli, "_refuse_partial_state", lambda *_a, **_k: None)
    monkeypatch.setattr(cli, "_assert_no_journal", lambda *_a, **_k: None)

    def process_state(_ident: object) -> tuple[str, str]:
        calls["n"] += 1
        return ("live", "appeared") if calls["n"] > 1 else ("dead", "dead")

    monkeypatch.setattr(cli, "_process_state", process_state)
    monkeypatch.setattr(
        cli, "_owner_liveness_states",
        lambda _c, records, deadline=None: {
            (r.machine, r.slot, r.generation): ("dead", "dead") for r in records
        },
    )
    monkeypatch.setattr(cli, "_load_hold", lambda *_a, **_k: None)
    monkeypatch.setattr(
        cli, "_execute_ttl_slot_rescue",
        lambda *_a, **_k: {
            "slot": "slot-f", "stage": "teardown",
            "expired_at": "2026-11-10T01:00:00+00:00",
            "teardown_at": "2026-11-13T01:00:00+00:00", "checkouts": [],
        },
    )

    @contextlib.contextmanager
    def locks(*_a: object, **_k: object) -> Iterator[None]:
        yield

    monkeypatch.setattr(cli, "_mutation_locks", locks)
    monkeypatch.setattr(cli, "_append_archive_once", lambda *_a, **_k: state)
    monkeypatch.setattr(cli, "_write_active_state", lambda *_a, **_k: {})
    slot_dir = config.worktrees / "slot-f"
    slot_dir.mkdir(parents=True, exist_ok=True)
    with pytest.raises(cli.Refusal) as caught:
        cli._cmd_ttl_teardown(argparse.Namespace(
            project_root=None, machine=None, slot="slot-f",
            expected_generation=1, now=NOW.isoformat(),
            rescue_hook_command=None, warn_after_seconds=0,
            alarm_after_seconds=86_400, stop_after_seconds=172_800,
            teardown_after_seconds=259_200, wait_lock=0.0,
        ))
    assert "slot-f" in str(caught.value)
    assert "blocked: owner live" in str(caught.value)
    assert slot_dir.exists(), "re-check refusal must not delete the tree"


def test_hold_added_during_rescue_refused_at_recheck(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    import argparse
    import contextlib

    config = _config(tmp_path)
    record = _record(
        config, config.worktrees / "slot-f" / "repo", tmp_path / "r.git",
        owner=_live_owner(),
    )
    state = cli.ActiveState(machine="testhost", revision=7, slots=(record,))
    calls = {"n": 0}
    monkeypatch.setattr(cli, "_load_config", lambda *_a: config)
    monkeypatch.setattr(cli, "_load_active_snapshot", lambda *_a, **_k: state)
    monkeypatch.setattr(cli, "_load_active", lambda *_a, **_k: state)
    monkeypatch.setattr(
        cli, "_load_archive",
        lambda *_a, **_k: cli.ArchiveState(machine="testhost", revision=1, records=()),
    )
    monkeypatch.setattr(cli, "_refuse_partial_state", lambda *_a, **_k: None)
    monkeypatch.setattr(cli, "_assert_no_journal", lambda *_a, **_k: None)
    monkeypatch.setattr(cli, "_process_state", lambda _i: ("dead", "dead"))
    monkeypatch.setattr(
        cli, "_owner_liveness_states",
        lambda _c, records, deadline=None: {
            (r.machine, r.slot, r.generation): ("dead", "dead") for r in records
        },
    )

    def hold(*_a: object, **_k: object) -> dict[str, object] | None:
        calls["n"] += 1
        return {"reason": "placed during rescue"} if calls["n"] > 1 else None

    monkeypatch.setattr(cli, "_load_hold", hold)
    monkeypatch.setattr(
        cli, "_execute_ttl_slot_rescue",
        lambda *_a, **_k: {
            "slot": "slot-f", "stage": "teardown",
            "expired_at": "2026-11-10T01:00:00+00:00",
            "teardown_at": "2026-11-13T01:00:00+00:00", "checkouts": [],
        },
    )

    @contextlib.contextmanager
    def locks(*_a: object, **_k: object) -> Iterator[None]:
        yield

    monkeypatch.setattr(cli, "_mutation_locks", locks)
    monkeypatch.setattr(cli, "_append_archive_once", lambda *_a, **_k: state)
    monkeypatch.setattr(cli, "_write_active_state", lambda *_a, **_k: {})
    slot_dir = config.worktrees / "slot-f"
    slot_dir.mkdir(parents=True, exist_ok=True)
    with pytest.raises(cli.Refusal) as caught:
        cli._cmd_ttl_teardown(argparse.Namespace(
            project_root=None, machine=None, slot="slot-f",
            expected_generation=1, now=NOW.isoformat(),
            rescue_hook_command=None, warn_after_seconds=0,
            alarm_after_seconds=86_400, stop_after_seconds=172_800,
            teardown_after_seconds=259_200, wait_lock=0.0,
        ))
    assert "blocked: held" in str(caught.value)
    assert slot_dir.exists()


@pytest.mark.ordinary_environment
def test_heartbeat_succeeds_with_warning_for_nested_worktree(
    tmp_path: Path,
) -> None:
    """#1492: heartbeat warns and records despite git-registration-without-row."""
    from wrkslots.tests.test_lifecycle import (
        checkout,
        command,
        create,
        git,
        make_project,
    )

    project, _repository, _remote = make_project(tmp_path)
    assert create(project).returncode == 0
    nested = checkout(project) / "ignored" / "nested-a"
    nested.parent.mkdir(parents=True, exist_ok=True)
    git(checkout(project), "worktree", "add", "--detach", str(nested), "HEAD")
    (nested / "dirty.txt").write_text("live work\n", encoding="utf-8")
    before = (nested / "dirty.txt").read_text(encoding="utf-8")
    result = command(
        project, "heartbeat", "slot01", "--agent", "codex-1",
        "--owner-pid", str(os.getpid()), "--expected-generation", "1",
    )
    assert result.returncode == 0, result.stderr
    assert "nested" in result.stderr.lower()
    assert "heartbeat" in result.stdout
    assert (nested / "dirty.txt").read_text(encoding="utf-8") == before
    assert nested.is_dir()
