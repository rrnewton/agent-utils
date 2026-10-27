"""Differential: wrkslotsd parse-doctor against the Python doctor.

#562 rust-primary-wrkslots slice 23. The Python oracle composes
`_status_record(..., tolerate_hold_error=True)` rows with
`_slot_findings` on fixture projects; the Rust command must agree on
the rows and on the snapshot-scope findings (invalid hold, dead
owner, unfinished journal, partial atomic update, cap breach).
Registry storage, slot-image, and owner-release findings stay with
Python and are filtered out of the oracle here. Heartbeat age is
compared within ten seconds.
"""

from __future__ import annotations

import datetime as dt
import json
import os
import subprocess
from pathlib import Path
from typing import cast

from wrkslots import cli as wrkslots_cli


AU_ROOT = Path(__file__).resolve().parents[3]
SHA = "a" * 40
DIGEST = "b" * 64
KINDS = {"invalid-hold", "dead-owner", "unfinished-journal", "partial-atomic-update", "cap-breach"}


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
        "max_active_slots: 1\n"
        "liveness_command: live.py\n"
    )
    return root


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


def _identity(pid: int, start: int) -> dict[str, object]:
    live = wrkslots_cli._read_process_identity(os.getpid())
    return {
        "pid": pid,
        "start_ticks": start,
        "boot_id": live.boot_id,
        "host_id": live.host_id,
        "cgroup_path": live.cgroup_path,
    }


def _record(
    slot: str, agent: str, owner: dict[str, object], machine: str = "node-a"
) -> dict[str, object]:
    heartbeat = dt.datetime.now(dt.timezone.utc) - dt.timedelta(seconds=3600)
    lease = _identity(os.getpid(), wrkslots_cli._read_process_identity(os.getpid()).start_ticks)
    return {
        "slot": slot,
        "agent": agent,
        "task": "do work",
        "purpose": "testing",
        "machine": machine,
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


def _fixture(root: Path) -> None:
    live_pid = os.getpid()
    live = _identity(live_pid, wrkslots_cli._read_process_identity(live_pid).start_ticks)
    dead = _identity(_unused_pid(), 12345)
    records = [_record("slot-a", "agent-a", live), _record("slot-b", "agent-b", dead)]
    control = root / "worktrees"
    (control / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 7, "slots": records})
    )
    (control / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": []})
    )
    other = [_record("slot-z", "agent-z", dead, machine="node-b")]
    (control / "ACTIVE.node-b.json").write_text(
        json.dumps({"schema": 2, "machine": "node-b", "revision": 7, "slots": other})
    )
    (control / "ARCHIVED.node-b.json").write_text(
        json.dumps({"schema": 2, "machine": "node-b", "revision": 3, "records": []})
    )
    bad_hold = {
        "schema": 1,
        "machine": "node-a",
        "slot": "slot-b",
        "held_at": "2026-10-26T03:00:00Z",
        "reason": "  ",
    }
    (control / "HOLD.6.node-a.slot-b.json").write_text(json.dumps(bad_hold))
    (control / "ACTIVE.node-a.journal").write_text("{}")
    (control / "ACTIVE.node-a.json.tmp.1").write_text("{}")
    events = control / "EVENTS.node-a"
    events.mkdir()
    (events / "00000000000000000001.json.tmp.x").write_text("{}")


def _expected(root: Path, *, all_machines: bool = False) -> dict[str, object]:
    config = wrkslots_cli._load_config(str(root), None)
    all_states = [
        wrkslots_cli._load_active_snapshot(config, machine, require_repository=False)
        for machine in wrkslots_cli._state_machines(config)
    ]
    states = (
        all_states
        if all_machines
        else [state for state in all_states if state.machine == config.machine]
    )
    hold_results = wrkslots_cli._load_record_holds(config, all_states)
    rows = [
        wrkslots_cli._status_record(
            config,
            record,
            hold_result=hold_results[(record.machine, record.slot)],
            tolerate_hold_error=True,
        )
        for state in states
        for record in state.slots
    ]
    findings = [
        item
        for item in wrkslots_cli._slot_findings(config, all_states, hold_results)
        if item["kind"] in KINDS
        and (all_machines or item["machine"] is None or item["machine"] == config.machine)
    ]
    return {
        "machines": [state.machine for state in states],
        "active": rows,
        "max_active_slots": config.max_active_slots,
        "findings": findings,
    }


def _rust(root: Path, *, all_machines: bool = False) -> subprocess.CompletedProcess[str]:
    extra = ["--all-machines"] if all_machines else []
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkslotsd", "--", "parse-doctor", "--root", str(root), *extra],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def test_parse_doctor_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    result = _rust(root)
    assert result.returncode == 0, result.stderr
    actual = json.loads(result.stdout)
    expected = _expected(root)
    got_rows = cast(list[dict[str, object]], actual.pop("active"))
    want_rows = cast(list[dict[str, object]], expected.pop("active"))
    assert len(got_rows) == len(want_rows)
    for got, want in zip(got_rows, want_rows, strict=True):
        got_age = cast(int, got.pop("heartbeat_age_seconds"))
        want_age = cast(int, want.pop("heartbeat_age_seconds"))
        assert abs(got_age - want_age) <= 10
        assert got == want
    assert actual == expected
    kinds = [str(item["kind"]) for item in cast(list[dict[str, object]], actual["findings"])]
    assert kinds == [
        "invalid-hold",
        "dead-owner",
        "unfinished-journal",
        "partial-atomic-update",
        "partial-atomic-update",
        "cap-breach",
    ]


def test_parse_doctor_all_machines_matches_python(tmp_path: Path) -> None:
    root = _root(tmp_path)
    _fixture(root)
    result = _rust(root, all_machines=True)
    assert result.returncode == 0, result.stderr
    actual = json.loads(result.stdout)
    expected = _expected(root, all_machines=True)
    got_rows = cast(list[dict[str, object]], actual.pop("active"))
    want_rows = cast(list[dict[str, object]], expected.pop("active"))
    assert len(got_rows) == len(want_rows) == 3
    for got, want in zip(got_rows, want_rows, strict=True):
        got.pop("heartbeat_age_seconds")
        want.pop("heartbeat_age_seconds")
        assert got == want
    assert actual == expected
    kinds = [str(item["kind"]) for item in cast(list[dict[str, object]], actual["findings"])]
    assert kinds.count("dead-owner") == 2


def test_parse_doctor_clean_registry(tmp_path: Path) -> None:
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
    actual = json.loads(result.stdout)
    assert actual["findings"] == []
    assert actual["active"] == []
