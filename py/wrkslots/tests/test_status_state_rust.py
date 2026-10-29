"""Differential: wrkslotsd parse-status against the Python status record.

#562 rust-primary-wrkslots slice 22. The Python oracle is
`_status_record` over snapshot states with `_load_record_holds`
results on fixture projects; the Rust command must agree on the
record, owner-process diagnosis, expiry, and hold outcome. Owner
identities are built from this host's live `/proc` data, so the live,
reused, and not-live branches are all exercised. Heartbeat age is the
one wall-clock field: it is compared within ten seconds instead of
exactly. `storage_inconsistencies` is empty on both sides here (the
oracle's default), since registry storage checks stay with Python.
"""

from __future__ import annotations

import datetime as dt
import json
import os
import subprocess
from pathlib import Path
from typing import cast

import pytest
from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]
SHA = "a" * 40
DIGEST = "b" * 64


def _root(tmp_path: Path) -> Path:
    root = tmp_path / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
    )
    return root


def _record(
    slot: str, agent: str, owner: dict[str, object] | None, *, heartbeat_age: int = 3600
) -> dict[str, object]:
    heartbeat = dt.datetime.now(dt.timezone.utc) - dt.timedelta(seconds=heartbeat_age)
    lease = owner if owner is not None else _live_identity()
    return {
        "slot": slot,
        "agent": agent,
        "task": "do work",
        "purpose": "testing",
        "machine": "node-a",
        "generation": 1,
        "created_at": "2026-10-26T00:00:00Z",
        "heartbeat_at": heartbeat.strftime("%Y-%m-%dT%H:%M:%SZ"),
        "heartbeat_ttl_seconds": 60,
        "owner": owner,
        "coordinator_lease": lease,
        "coordinator_recovery_note": None,
        "handoff": None,
        "checkouts": [
            {
                "name": "repo",
                "path": f"worktrees/{slot}/repo",
                "repository": "../upstream-repo",
                "branch": "suite/work",
                "start_point": SHA,
                "remote": "origin",
                "remote_url_sha256": DIGEST,
                "landed_ref": "refs/remotes/origin/main",
                "head": SHA,
                "containing_remote_refs": ["refs/remotes/origin/main"],
                "vcs": "git",
            }
        ],
    }


def _live_identity() -> dict[str, object]:
    identity = wrkslots_cli._read_process_identity(os.getpid())
    return {
        "pid": identity.pid,
        "start_ticks": identity.start_ticks,
        "boot_id": identity.boot_id,
        "host_id": identity.host_id,
        "cgroup_path": identity.cgroup_path,
    }


def _unused_pid() -> int:
    pid = 4194300
    while True:
        try:
            os.kill(pid, 0)
        except ProcessLookupError:
            return pid
        except PermissionError:
            pid += 1
            continue
        pid += 1


def _fixture(root: Path) -> None:
    live = _live_identity()
    reused = {**live, "start_ticks": cast(int, live["start_ticks"]) + 1}
    dead = {**live, "pid": _unused_pid(), "start_ticks": 12345}
    foreign = {**live, "host_id": "some-other-host"}
    init = {**live, "pid": 1, "start_ticks": 1, "cgroup_path": "/init.scope"}
    records = [
        _record("slot-live", "agent-a", live),
        _record("slot-reused", "agent-b", reused),
        _record("slot-dead", "agent-c", dead),
        _record("slot-foreign", "agent-d", foreign),
        _record("slot-none", "agent-e", None),
        _record("slot-init", "agent-f", init),
    ]
    control = root / "worktrees"
    (control / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 7, "slots": records})
    )
    (control / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": []})
    )
    hold = {
        "schema": 1,
        "machine": "node-a",
        "slot": "slot-live",
        "held_at": "2026-10-26T03:00:00Z",
        "reason": "waiting",
    }
    (control / "HOLD.6.node-a.slot-live.json").write_text(json.dumps(hold))


def _expected(root: Path) -> list[dict[str, object]]:
    config = wrkslots_cli._load_config(str(root), None)
    states = [
        wrkslots_cli._load_active_snapshot(config, machine, require_repository=False)
        for machine in wrkslots_cli._state_machines(config)
    ]
    results = wrkslots_cli._load_record_holds(config, states)
    records: list[dict[str, object]] = []
    for state in states:
        for record in state.slots:
            records.append(
                wrkslots_cli._status_record(
                    config,
                    record,
                    hold_result=results[(record.machine, record.slot)],
                )
            )
    return records


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "parse-status", "--root", str(root)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def test_parse_status_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    actual = cast(list[dict[str, object]], json.loads(result.stdout)["records"])
    expected = _expected(root)
    assert len(actual) == len(expected)
    for got, want in zip(actual, expected, strict=True):
        got_age = cast(int, got.pop("heartbeat_age_seconds"))
        want_age = cast(int, want.pop("heartbeat_age_seconds"))
        assert abs(got_age - want_age) <= 10
        assert got == want
    by_slot = {str(item["slot"]): item for item in actual}
    assert by_slot["slot-live"]["owner_state"] == "live"
    assert by_slot["slot-live"]["held"] is True
    assert by_slot["slot-reused"]["owner_state"] == "dead"
    assert by_slot["slot-dead"]["owner_state"] == "dead"
    assert by_slot["slot-foreign"]["owner_state"] == "indeterminate"
    assert by_slot["slot-none"]["owner_state"] == "indeterminate"
    assert by_slot["slot-none"]["owner_detail"] == (
        "no owner process generation is recorded"
    )


def test_parse_status_invalid_hold_refuses_like_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    hold = {
        "schema": 1,
        "machine": "node-a",
        "slot": "slot-dead",
        "held_at": "2026-10-26T03:00:00Z",
        "reason": "  ",
    }
    (root / "worktrees" / "HOLD.6.node-a.slot-dead.json").write_text(json.dumps(hold))
    with pytest.raises(Exception) as caught:
        _expected(root)
    assert "empty reason" in str(caught.value)
    result = _rust(root)
    assert result.returncode == 1
    assert "empty reason" in result.stderr


def test_parse_status_no_records(tmp_path: Path) -> None:
    root = _root(tmp_path)
    control = root / "worktrees"
    (control / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 0, "slots": []})
    )
    (control / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 0, "records": []})
    )
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == {"machines": ["node-a"], "records": []}


def test_parse_status_reads_event_derived_state_like_python(
    tmp_path: Path,
) -> None:
    """Regression for the live-registry trial stop: a machine whose
    state is derived from the append-only event log must read through
    the chain fold (active document and holds), not refuse toward
    rebuild. The fixture copies the live registry's shape: a stale
    snapshot shard beside a complete EVENTS chain."""
    root = _root(tmp_path)
    live = _record("slot-live", "agent-live", _live_identity())
    stale = _record("slot-stale", "agent-stale", None)
    (root / "worktrees" / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 7, "slots": [stale]})
    )
    (root / "worktrees" / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": []})
    )
    hold = {
        "schema": 1,
        "machine": "node-a",
        "slot": "slot-live",
        "held_at": "2026-10-26T03:00:00Z",
        "reason": "waiting",
    }
    core: dict[str, object] = {
        "schema": 1,
        "machine": "node-a",
        "sequence": 1,
        "previous_sha256": "0" * 64,
        "recorded_at": "2026-10-26T03:00:00Z",
        "kind": "state-imported",
        "payload": {
            "active": {
                "schema": 2, "machine": "node-a", "revision": 9, "slots": [live],
            },
            "archive": {
                "schema": 2, "machine": "node-a", "revision": 3, "records": [],
            },
            "holds": [hold],
        },
    }
    event = {**core, "sha256": wrkslots_cli._event_digest(core)}
    events = root / "worktrees" / "EVENTS.node-a"
    events.mkdir()
    (events / "00000000000000000001.json").write_text(json.dumps(event))

    result = _rust(root)
    assert result.returncode == 0, result.stderr
    doc = json.loads(result.stdout)
    records = doc["records"]
    assert [record["slot"] for record in records] == ["slot-live"]
    assert records[0]["held"] is True
    assert records[0]["hold_reason"] == "waiting"
