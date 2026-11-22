"""Differential: wrkslotsd reads the same host-alias file as Python (#1933)."""

from __future__ import annotations

import datetime as dt
import json
import os
import subprocess
from pathlib import Path
from typing import cast

import pytest
from wrkslots import cli as wrkslots_cli

from wrkslots.tests.rust_binary import rust_wrkslotsd
from wrkslots.tests.test_status_state_rust import _live_identity, _record, _root


AU_ROOT = Path(__file__).resolve().parents[3]


def _alias_entry(previous: str, current: str, hostname: str, current_boot: str) -> dict[str, object]:
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
        "previous_boot_ids": ["old-boot"],
        "current_boot_id": current_boot,
    }


def test_rust_status_uses_host_alias_like_python(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    root = _root(tmp_path)
    live = _live_identity()
    old_owner = {
        **live,
        "pid": 4194300,
        "start_ticks": 12345,
        "boot_id": "old-boot",
        "host_id": "machine-a",
    }
    foreign_owner = {**old_owner, "host_id": "machine-c"}
    current_boot = wrkslots_cli._boot_id(Path("/proc"))
    live_current_old_host = {
        **live,
        "boot_id": current_boot,
        "host_id": "machine-a",
    }
    gone_current_old_host = {
        **old_owner,
        "boot_id": current_boot,
    }
    records = [
        _record("slot-old", "agent-a", old_owner),
        _record("slot-foreign", "agent-b", foreign_owner),
        _record("slot-live-current", "agent-c", live_current_old_host),
        _record("slot-gone-current", "agent-d", gone_current_old_host),
    ]
    control = root / "worktrees"
    (control / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 7, "slots": records})
    )
    (control / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": []})
    )
    alias_path = tmp_path / "host-aliases.json"
    alias_path.write_text(
        json.dumps(
            {
                "schema": 1,
                "aliases": [
                    _alias_entry(
                        "machine-a",
                        wrkslots_cli._host_id(),
                        wrkslots_cli._short_hostname(),
                        wrkslots_cli._boot_id(Path("/proc")),
                    )
                ],
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("WRKSLOTS_HOST_ALIAS_PATH", str(alias_path))
    monkeypatch.setenv("WRKSLOTS_HOST_ALIAS_TEST", "1")

    old_identity = wrkslots_cli.ProcessIdentity(
        pid=4194300,
        start_ticks=12345,
        boot_id="old-boot",
        host_id="machine-a",
        cgroup_path=str(live["cgroup_path"]),
    )
    assert wrkslots_cli._process_state(old_identity) == (
        "dead",
        "the recorded machine boot ended",
    )

    env = dict(os.environ)
    env["WRKSLOTS_HOST_ALIAS_PATH"] = str(alias_path)
    env["WRKSLOTS_HOST_ALIAS_TEST"] = "1"
    result = subprocess.run(
        [str(rust_wrkslotsd()), "parse-status", "--root", str(root)],
        cwd=AU_ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=900,
        env=env,
    )
    assert result.returncode == 0, result.stderr
    rows = cast(list[dict[str, object]], json.loads(result.stdout)["records"])
    by_slot = {str(row["slot"]): row for row in rows}
    assert by_slot["slot-old"]["owner_state"] == "dead"
    assert by_slot["slot-old"]["owner_detail"] == "the recorded machine boot ended"
    assert by_slot["slot-foreign"]["owner_state"] == "indeterminate"
    assert by_slot["slot-foreign"]["owner_detail"] == (
        "the recorded owner belongs to a different machine identity"
    )
    for slot in ("slot-live-current", "slot-gone-current"):
        assert by_slot[slot]["owner_state"] == "indeterminate"
        assert by_slot[slot]["owner_detail"] == (
            "the recorded owner belongs to a previous machine identity in the current boot"
        )


def test_rust_audit_treats_aliased_validate_row_as_local(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """Differential (#2100): a re-homed validation row is local in both editions."""

    root = _root(tmp_path)
    upstream = tmp_path / "upstream-repo"
    upstream.mkdir()
    subprocess.run(
        ["git", "init", "-q", "-b", "main", str(upstream)],
        check=True,
        capture_output=True,
        timeout=60,
    )
    live = _live_identity()
    old_owner = {
        **live,
        "pid": 4194300,
        "start_ticks": 12345,
        "boot_id": "old-boot",
        "host_id": "machine-a",
    }
    foreign_owner = {**old_owner, "host_id": "machine-c"}

    def validate_record(slot: str, agent: str, owner: dict[str, object]) -> dict[str, object]:
        record = _record(slot, agent, owner)
        record["slot_type"] = "validate"
        record["heartbeat_at"] = "2026-11-19T00:00:00Z"
        checkouts = cast(list[dict[str, object]], record["checkouts"])
        checkouts[0]["path"] = f"worktrees/validate/{slot}/repo"
        return record

    boot_start = wrkslots_cli._boot_started_at()
    assert boot_start is not None
    current_boot_owner = {
        **old_owner,
        "boot_id": wrkslots_cli._boot_id(Path("/proc")),
    }
    boundary_record = validate_record("slot-boundary", "validate-e", old_owner)
    boundary_record["heartbeat_at"] = (
        boot_start - dt.timedelta(seconds=4.5)
    ).isoformat()
    records = [
        validate_record("slot-old", "validate-a", old_owner),
        validate_record("slot-foreign", "validate-b", foreign_owner),
        validate_record("slot-curboot", "validate-c", current_boot_owner),
        validate_record(
            "slot-afterboot",
            "validate-d",
            old_owner,
        ),
        boundary_record,
    ]
    records[3]["heartbeat_at"] = dt.datetime.now(dt.timezone.utc).isoformat()
    control = root / "worktrees"
    (control / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 7, "slots": records})
    )
    (control / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 3, "records": []})
    )
    alias_path = tmp_path / "host-aliases.json"
    alias_path.write_text(
        json.dumps(
            {
                "schema": 1,
                "aliases": [
                    _alias_entry(
                        "machine-a",
                        wrkslots_cli._host_id(),
                        wrkslots_cli._short_hostname(),
                        wrkslots_cli._boot_id(Path("/proc")),
                    )
                ],
            }
        ),
        encoding="utf-8",
    )
    monkeypatch.setenv("WRKSLOTS_HOST_ALIAS_PATH", str(alias_path))
    monkeypatch.setenv("WRKSLOTS_HOST_ALIAS_TEST", "1")

    env = dict(os.environ)
    env["WRKSLOTS_HOST_ALIAS_PATH"] = str(alias_path)
    env["WRKSLOTS_HOST_ALIAS_TEST"] = "1"
    result = subprocess.run(
        [str(rust_wrkslotsd()), "parse-audit", "--root", str(root)],
        cwd=AU_ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=900,
        env=env,
    )
    assert result.returncode == 0, result.stderr
    rows = cast(list[dict[str, object]], json.loads(result.stdout)["slots"])
    by_slot = {str(row["slot"]): row for row in rows}
    old_reasons = " ".join(
        str(r) for r in cast(list[object], by_slot["slot-old"]["reasons"])
    )
    foreign_reasons = " ".join(
        str(r) for r in cast(list[object], by_slot["slot-foreign"]["reasons"])
    )
    assert "belongs to stable host" not in old_reasons
    assert "belongs to stable host machine-c" in foreign_reasons
    for slot in ("slot-curboot", "slot-afterboot", "slot-boundary"):
        reasons = " ".join(
            str(r) for r in cast(list[object], by_slot[slot]["reasons"])
        )
        assert "belongs to stable host machine-a" in reasons, slot
