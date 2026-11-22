"""Host-local machine-id aliases after a reprovision (#1933)."""

from __future__ import annotations

import argparse
import dataclasses
import datetime as dt
import json
import os
import stat
from pathlib import Path

import pytest
from wrkslots import cli


MACHINE_A = "machine-a"
MACHINE_B = "machine-b"
MACHINE_C = "machine-c"
BOOT_A = "boot-a"
BOOT_B = "boot-b"
HOSTNAME = "node-a"


def _identity(host: str, boot: str, *, pid: int = 4194300) -> cli.ProcessIdentity:
    return cli.ProcessIdentity(
        pid=pid,
        start_ticks=12345,
        boot_id=boot,
        host_id=host,
        cgroup_path="/user.slice/session.scope",
    )


def _entry(
    previous: str,
    current: str,
    *,
    hostname: str = HOSTNAME,
    previous_boots: list[str] | None = None,
    current_boot: str = BOOT_B,
) -> dict[str, object]:
    return {
        "previous_host_id": previous,
        "current_host_id": current,
        "hostname": hostname,
        "recorded_at": "2026-11-19T00:00:00+00:00",
        "recorded_by": {
            "pid": 123,
            "start_ticks": 456,
            "boot_id": current_boot,
            "host_id": current,
        },
        "previous_boot_ids": list(previous_boots or []),
        "current_boot_id": current_boot,
    }


def _write_aliases(path: Path, entries: list[dict[str, object]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps({"schema": 1, "aliases": entries}), encoding="utf-8")


def _host_at_b(monkeypatch: pytest.MonkeyPatch, alias_path: Path) -> None:
    monkeypatch.setenv("WRKSLOTS_HOST_ALIAS_PATH", str(alias_path))
    monkeypatch.setenv("WRKSLOTS_HOST_ALIAS_TEST", "1")
    monkeypatch.setattr(cli, "_host_id", lambda: MACHINE_B)
    monkeypatch.setattr(cli, "_boot_id", lambda proc_root: BOOT_B)
    monkeypatch.setattr(cli, "_short_hostname", lambda: HOSTNAME)


def test_aliased_old_boot_owner_is_dead_and_foreign_owner_stays_indeterminate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    _host_at_b(monkeypatch, alias_path)
    _write_aliases(alias_path, [_entry(MACHINE_A, MACHINE_B, previous_boots=[BOOT_A])])

    assert cli._process_state(_identity(MACHINE_A, BOOT_A)) == (
        "dead",
        "the recorded machine boot ended",
    )
    assert cli._process_state(_identity(MACHINE_C, BOOT_A)) == (
        "indeterminate",
        "the recorded owner belongs to a different machine identity",
    )


def test_same_boot_alias_never_counts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    _host_at_b(monkeypatch, alias_path)
    _write_aliases(
        alias_path,
        [_entry(MACHINE_A, MACHINE_B, previous_boots=[BOOT_B], current_boot=BOOT_B)],
    )

    assert cli._process_state(_identity(MACHINE_A, BOOT_A))[0] == "indeterminate"


def test_current_boot_aliased_owner_is_never_dead(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    _host_at_b(monkeypatch, alias_path)
    _write_aliases(alias_path, [_entry(MACHINE_A, MACHINE_B, previous_boots=[BOOT_A])])
    live = cli._read_process_identity(os.getpid())
    recorded_live = dataclasses.replace(live, host_id=MACHINE_A, boot_id=BOOT_B)
    recorded_gone = _identity(MACHINE_A, BOOT_B)

    # Probe 3d: a live owner's true generation, recorded under the old
    # identity in the current boot, must not read as a reused PID.
    assert cli._process_state(recorded_live) == (
        "indeterminate",
        "the recorded owner belongs to a previous machine identity in the current boot",
    )
    # Probe 3b2: the same record with a gone PID is not proven dead either.
    assert cli._process_state(recorded_gone) == (
        "indeterminate",
        "the recorded owner belongs to a previous machine identity in the current boot",
    )


def test_group_writable_alias_file_is_ignored(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    _host_at_b(monkeypatch, alias_path)
    _write_aliases(alias_path, [_entry(MACHINE_A, MACHINE_B, previous_boots=[BOOT_A])])
    os.chmod(alias_path, 0o664)

    assert cli._process_state(_identity(MACHINE_A, BOOT_A))[0] == "indeterminate"


def test_judge_ignores_alias_override_without_test_marker(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    _host_at_b(monkeypatch, alias_path)
    monkeypatch.delenv("WRKSLOTS_HOST_ALIAS_TEST")
    monkeypatch.setenv("XDG_STATE_HOME", str(tmp_path / "xdg"))
    _write_aliases(alias_path, [_entry(MACHINE_A, MACHINE_B, previous_boots=[BOOT_A])])

    assert cli._process_state(_identity(MACHINE_A, BOOT_A))[0] == "indeterminate"


def test_transitive_alias_counts_and_cycles_stop(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    _host_at_b(monkeypatch, alias_path)
    entries = [
        _entry(MACHINE_A, "machine-mid", previous_boots=[BOOT_A]),
        _entry("machine-mid", MACHINE_B, previous_boots=["boot-mid"]),
    ]
    assert cli._host_alias_counts(
        MACHINE_A,
        current_host_id=MACHINE_B,
        hostname=HOSTNAME,
        current_boot_id=BOOT_B,
        aliases=tuple(entries),
    )
    cycle = [
        _entry(MACHINE_A, "machine-mid", previous_boots=[BOOT_A]),
        _entry("machine-mid", MACHINE_A, previous_boots=["boot-mid"]),
    ]
    assert not cli._host_alias_counts(
        MACHINE_A,
        current_host_id=MACHINE_B,
        hostname=HOSTNAME,
        current_boot_id=BOOT_B,
        aliases=tuple(cycle),
    )


def test_relative_xdg_state_home_is_ignored(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("HOME", str(tmp_path))
    monkeypatch.setenv("XDG_STATE_HOME", "relative/state")
    monkeypatch.delenv("WRKSLOTS_HOST_ALIAS_PATH", raising=False)
    expected = tmp_path / ".local" / "state" / "wrkslots" / "host-aliases.json"

    assert cli._host_alias_path(judge=True) == expected
    assert cli._host_alias_path() == expected


def test_malformed_alias_file_fails_closed(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    alias_path.write_text("not json", encoding="utf-8")
    _host_at_b(monkeypatch, alias_path)

    assert cli._load_host_aliases(alias_path) == ()
    assert cli._process_state(_identity(MACHINE_A, BOOT_A))[0] == "indeterminate"


def _record(owner: cli.ProcessIdentity | None, lease: cli.ProcessIdentity) -> cli.ActiveRecord:
    return cli.ActiveRecord(
        slot="slot-a",
        agent="agent-a",
        task="task",
        purpose="testing",
        slot_type="agent",
        machine=HOSTNAME,
        generation=1,
        created_at="2026-11-19T00:00:00+00:00",
        heartbeat_at="2026-11-19T00:00:00+00:00",
        heartbeat_ttl_seconds=60,
        owner=owner,
        coordinator_lease=lease,
        coordinator_recovery_note=None,
        handoff=None,
        checkouts=(),
    )


def _verb_args(previous: str) -> argparse.Namespace:
    return argparse.Namespace(
        previous_host_id=previous,
        coordinator_pid=os.getpid(),
        coordinator_authorized=True,
        project_root=None,
        machine=None,
    )


def _verb_host(
    monkeypatch: pytest.MonkeyPatch,
    alias_path: Path,
    records: list[cli.ActiveRecord],
    *,
    machine: str = HOSTNAME,
) -> None:
    _host_at_b(monkeypatch, alias_path)
    coordinator = _identity(MACHINE_B, BOOT_B, pid=os.getpid())
    monkeypatch.setattr(cli, "_capture_caller_process", lambda pid, label: coordinator)
    config = dataclasses.replace(
        cli.Config(
            root=Path("/tmp"),
            config_path=Path("/tmp/.wrkslots.yml"),
            worktrees=Path("/tmp/worktrees"),
            control=Path("/tmp/worktrees"),
            machine=machine,
            default_remote="origin",
            default_landed_ref="refs/remotes/origin/main",
            heartbeat_ttl_seconds=60,
            liveness_command=Path("/tmp/live.py"),
        )
    )
    monkeypatch.setattr(cli, "_load_config", lambda root, override: config)
    state = cli.ActiveState(machine=HOSTNAME, revision=1, slots=tuple(records))
    monkeypatch.setattr(cli, "_load_active", lambda cfg: state)


def test_host_alias_add_unblocks_old_owner_only(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, capsys: pytest.CaptureFixture[str]
) -> None:
    alias_path = tmp_path / "state" / "host-aliases.json"
    old_owner = _identity(MACHINE_A, BOOT_A)
    old_lease = _identity(MACHINE_A, BOOT_A, pid=4194301)
    _verb_host(monkeypatch, alias_path, [_record(old_owner, old_lease)])

    assert cli._process_state(old_owner)[0] == "indeterminate"
    assert cli._cmd_host_alias_add(_verb_args(MACHINE_A)) == 0
    assert "recorded host alias" in capsys.readouterr().out
    payload = json.loads(alias_path.read_text(encoding="utf-8"))
    assert stat.S_IMODE(alias_path.stat().st_mode) == 0o600
    assert stat.S_IMODE(alias_path.parent.stat().st_mode) == 0o700
    assert payload["aliases"][0]["previous_boot_ids"] == [BOOT_A]
    assert cli._process_state(old_owner) == ("dead", "the recorded machine boot ended")
    assert cli._process_state(_identity(MACHINE_C, BOOT_A))[0] == "indeterminate"
    with pytest.raises(cli.Refusal, match="already recorded"):
        cli._cmd_host_alias_add(_verb_args(MACHINE_A))


def test_host_alias_add_refuses_current_identity_and_current_boot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    current_boot_owner = _identity(MACHINE_A, BOOT_B)
    lease = _identity(MACHINE_B, BOOT_B, pid=4194301)
    _verb_host(monkeypatch, alias_path, [_record(current_boot_owner, lease)])

    with pytest.raises(cli.Refusal, match="current machine identity"):
        cli._cmd_host_alias_add(_verb_args(MACHINE_B))
    with pytest.raises(cli.Refusal, match="current boot .* is recorded"):
        cli._cmd_host_alias_add(_verb_args(MACHINE_A))
    assert not alias_path.exists()


def test_host_alias_add_refuses_hostname_mismatch(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    _verb_host(monkeypatch, alias_path, [], machine="other-host")

    with pytest.raises(cli.Refusal, match="selected shard is 'other-host'"):
        cli._cmd_host_alias_add(_verb_args(MACHINE_A))
    assert not alias_path.exists()


def test_host_alias_add_refuses_live_coordinator_under_previous_identity(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    live = cli._read_process_identity(os.getpid())
    lease = dataclasses.replace(live, host_id=MACHINE_A, boot_id=BOOT_B)
    _verb_host(monkeypatch, alias_path, [_record(None, lease)])

    with pytest.raises(cli.Refusal, match="is live now"):
        cli._cmd_host_alias_add(_verb_args(MACHINE_A))
    assert not alias_path.exists()


BOOT_START = dt.datetime(2026, 11, 20, 9, 0, tzinfo=dt.timezone.utc)
OLD_HEARTBEAT = "2026-11-19T00:00:00+00:00"


def _validate_record(
    owner: cli.ProcessIdentity | None, lease: cli.ProcessIdentity, *, heartbeat: str = OLD_HEARTBEAT
) -> cli.ActiveRecord:
    return dataclasses.replace(
        _record(owner, lease), slot_type="validate", heartbeat_at=heartbeat
    )


def _host_at_b_with_boot_start(monkeypatch: pytest.MonkeyPatch, alias_path: Path) -> None:
    _host_at_b(monkeypatch, alias_path)
    monkeypatch.setattr(cli, "_boot_started_at", lambda: BOOT_START)


def test_aliased_validate_row_is_local_and_recoverable(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    _host_at_b_with_boot_start(monkeypatch, alias_path)
    _write_aliases(alias_path, [_entry(MACHINE_A, MACHINE_B, previous_boots=[BOOT_A])])
    record = _validate_record(_identity(MACHINE_A, BOOT_A), _identity(MACHINE_A, BOOT_A, pid=4194301))

    cli._assert_absent_validate_row_is_local(record)
    assert (
        cli._absent_validate_owner_refusal(
            record, current_host=MACHINE_B, current_boot=BOOT_B
        )
        is None
    )

    foreign = _validate_record(
        _identity(MACHINE_C, BOOT_A), _identity(MACHINE_C, BOOT_A, pid=4194301)
    )
    with pytest.raises(cli.Refusal, match="belongs to stable host"):
        cli._assert_absent_validate_row_is_local(foreign)
    assert cli._absent_validate_owner_refusal(
        foreign, current_host=MACHINE_B, current_boot=BOOT_B
    )


def test_aliased_absent_agent_row_passes_locality_gate(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Absent agent rows share the validate locality gate (#2100)."""

    alias_path = tmp_path / "host-aliases.json"
    _host_at_b_with_boot_start(monkeypatch, alias_path)
    _write_aliases(alias_path, [_entry(MACHINE_A, MACHINE_B, previous_boots=[BOOT_A])])
    record = _record(_identity(MACHINE_A, BOOT_A), _identity(MACHINE_A, BOOT_A, pid=4194301))
    assert record.slot_type == "agent"

    cli._assert_absent_validate_row_is_local(record)

    foreign = _record(_identity(MACHINE_C, BOOT_A), _identity(MACHINE_C, BOOT_A, pid=4194301))
    with pytest.raises(cli.Refusal, match="host-alias add --previous-host-id"):
        cli._assert_absent_validate_row_is_local(foreign)


def test_aliased_row_passes_pre_lock_and_locked_locality_checks(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Both recover checks pass for a re-homed row (#2100, #2088).

    The #2088 pre-lock pre-check and the locked recovery path call
    the same `_assert_absent_validate_row_is_local` on the snapshot
    record and on the locked reload; an aliased row passes both, a
    foreign row fails both with the host-alias remedy.
    """

    alias_path = tmp_path / "host-aliases.json"
    _host_at_b_with_boot_start(monkeypatch, alias_path)
    _write_aliases(alias_path, [_entry(MACHINE_A, MACHINE_B, previous_boots=[BOOT_A])])
    snapshot = _validate_record(
        _identity(MACHINE_A, BOOT_A), _identity(MACHINE_A, BOOT_A, pid=4194301)
    )
    locked_reload = dataclasses.replace(snapshot)

    cli._assert_absent_validate_row_is_local(snapshot)
    cli._assert_absent_validate_row_is_local(locked_reload)

    foreign = _validate_record(
        _identity(MACHINE_C, BOOT_A), _identity(MACHINE_C, BOOT_A, pid=4194301)
    )
    with pytest.raises(cli.Refusal, match="host-alias add"):
        cli._assert_absent_validate_row_is_local(foreign)
    with pytest.raises(cli.Refusal, match="host-alias add"):
        cli._assert_absent_validate_row_is_local(dataclasses.replace(foreign))


def test_aliased_validate_row_seen_live_after_boot_stays_refused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    _host_at_b_with_boot_start(monkeypatch, alias_path)
    _write_aliases(alias_path, [_entry(MACHINE_A, MACHINE_B, previous_boots=[BOOT_A])])
    live_heartbeat = (BOOT_START + dt.timedelta(seconds=60)).isoformat()
    record = _validate_record(
        _identity(MACHINE_A, BOOT_A),
        _identity(MACHINE_A, BOOT_A, pid=4194301),
        heartbeat=live_heartbeat,
    )

    with pytest.raises(cli.Refusal, match="belongs to stable host"):
        cli._assert_absent_validate_row_is_local(record)

    current_boot_owner = _validate_record(
        _identity(MACHINE_A, BOOT_B), _identity(MACHINE_A, BOOT_A, pid=4194301)
    )
    with pytest.raises(cli.Refusal, match="belongs to stable host"):
        cli._assert_absent_validate_row_is_local(current_boot_owner)


def test_heartbeat_margin_boundary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    _host_at_b_with_boot_start(monkeypatch, alias_path)
    _write_aliases(alias_path, [_entry(MACHINE_A, MACHINE_B, previous_boots=[BOOT_A])])
    at_margin = _validate_record(
        _identity(MACHINE_A, BOOT_A),
        _identity(MACHINE_A, BOOT_A, pid=4194301),
        heartbeat=(BOOT_START - dt.timedelta(seconds=5)).isoformat(),
    )
    cli._assert_absent_validate_row_is_local(at_margin)

    inside_margin = _validate_record(
        _identity(MACHINE_A, BOOT_A),
        _identity(MACHINE_A, BOOT_A, pid=4194301),
        heartbeat=(BOOT_START - dt.timedelta(seconds=2)).isoformat(),
    )
    with pytest.raises(cli.Refusal, match="belongs to stable host"):
        cli._assert_absent_validate_row_is_local(inside_margin)


def test_host_alias_add_refuses_heartbeat_after_boot(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    _host_at_b_with_boot_start(monkeypatch, alias_path)
    record = _validate_record(
        _identity(MACHINE_A, BOOT_A),
        _identity(MACHINE_A, BOOT_A, pid=4194301),
        heartbeat=(BOOT_START + dt.timedelta(seconds=60)).isoformat(),
    )
    _verb_host(monkeypatch, alias_path, [record])

    with pytest.raises(cli.Refusal, match="may have been seen live since the boot"):
        cli._cmd_host_alias_add(_verb_args(MACHINE_A))
    assert not alias_path.exists()


def test_host_alias_add_refuses_differing_paths(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    _host_at_b_with_boot_start(monkeypatch, alias_path)
    base = _validate_record(_identity(MACHINE_A, BOOT_A), _identity(MACHINE_A, BOOT_A, pid=4194301))
    checkout = cli.Checkout(
        name="main",
        path="elsewhere/not-the-slot-path",
        repository="repo",
        branch="main",
        start_point="HEAD",
        remote="origin",
        remote_url_sha256="0" * 64,
        landed_ref="refs/remotes/origin/main",
        head="abc123",
    )
    record = dataclasses.replace(base, checkouts=(checkout,))
    _verb_host(monkeypatch, alias_path, [record])

    with pytest.raises(cli.Refusal, match="names paths that differ"):
        cli._cmd_host_alias_add(_verb_args(MACHINE_A))
    assert not alias_path.exists()


def test_host_alias_add_refuses_empty_evidence(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Zero-evidence aliases refuse (#2100 cold review).

    This replaces #1933's `test_host_alias_add_allows_empty_boot_evidence`:
    with no row in this shard under the previous id, the proofs loop
    over nothing and any id would be recorded.
    """

    alias_path = tmp_path / "host-aliases.json"
    _verb_host(monkeypatch, alias_path, [])

    with pytest.raises(cli.Refusal, match="no row in shard"):
        cli._cmd_host_alias_add(_verb_args(MACHINE_A))
    assert not alias_path.exists()


def test_judge_refuses_when_boot_start_unknown(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    _host_at_b_with_boot_start(monkeypatch, alias_path)
    monkeypatch.setattr(cli, "_boot_started_at", lambda: None)
    _write_aliases(alias_path, [_entry(MACHINE_A, MACHINE_B, previous_boots=[BOOT_A])])
    record = _validate_record(
        _identity(MACHINE_A, BOOT_A), _identity(MACHINE_A, BOOT_A, pid=4194301)
    )

    with pytest.raises(cli.Refusal, match="belongs to stable host"):
        cli._assert_absent_validate_row_is_local(record)


def test_ownerless_row_with_current_boot_lease_refuses(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    _host_at_b_with_boot_start(monkeypatch, alias_path)
    _write_aliases(alias_path, [_entry(MACHINE_A, MACHINE_B, previous_boots=[BOOT_A])])
    record = _validate_record(None, _identity(MACHINE_A, BOOT_B, pid=4194301))

    with pytest.raises(cli.Refusal, match="belongs to stable host"):
        cli._assert_absent_validate_row_is_local(record)


def test_journal_host_aliased_accepted_foreign_refused(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    alias_path = tmp_path / "host-aliases.json"
    _host_at_b_with_boot_start(monkeypatch, alias_path)
    _write_aliases(alias_path, [_entry(MACHINE_A, MACHINE_B, previous_boots=[BOOT_A])])

    assert cli._journal_host_is_local(MACHINE_A)
    assert cli._journal_host_is_local(MACHINE_B)
    assert not cli._journal_host_is_local(MACHINE_C)


def test_wrongly_recorded_alias_still_refuses_live_rows(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """An alias recorded for a foreign id does not free its live rows."""

    alias_path = tmp_path / "host-aliases.json"
    _host_at_b_with_boot_start(monkeypatch, alias_path)
    _write_aliases(alias_path, [_entry(MACHINE_C, MACHINE_B, previous_boots=[BOOT_A])])

    current_boot = _validate_record(
        _identity(MACHINE_C, BOOT_B), _identity(MACHINE_C, BOOT_A, pid=4194301)
    )
    with pytest.raises(cli.Refusal, match="belongs to stable host"):
        cli._assert_absent_validate_row_is_local(current_boot)

    after_boot = _validate_record(
        _identity(MACHINE_C, BOOT_A),
        _identity(MACHINE_C, BOOT_A, pid=4194301),
        heartbeat=(BOOT_START + dt.timedelta(seconds=60)).isoformat(),
    )
    with pytest.raises(cli.Refusal, match="belongs to stable host"):
        cli._assert_absent_validate_row_is_local(after_boot)
