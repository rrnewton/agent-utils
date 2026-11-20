"""Create with --offline-reason against an unreachable remote (#1929).

A provenance-only/offline slot must not be refused by the
create-time remote probe (ls-remote + fetch in _create_plan), while
a normal slot still is. The reason is recorded only in the
slot-created event's evidence: no record or journal field carries
it, so old readers replay the slot unchanged. Temporary fixtures
only.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots_cli

from wrkslots.tests.test_create_text_rust import _fixture, _git


def _break_remote(base: Path) -> None:
    upstream = base / "upstream-repo"
    _git(upstream, "remote", "set-url", "origin", str(base / "no-such-remote"))


def _args(root: Path, **overrides: object) -> argparse.Namespace:
    values: dict[str, object] = {
        "project_root": str(root),
        "machine": "node-a",
        "slot": "slot-off",
        "slot_type": "agent",
        "coordinator_authorized": True,
        "agent": "agent-off",
        "task": "task-1",
        "purpose": "provenance only",
        "owner_pid": os.getpid(),
        "coordinator_pid": os.getpid(),
        "task_scope_unit": None,
        "task_scope_invocation_id": None,
        "repo": ["repo=../upstream-repo"],
        "remote": [],
        "remote_url": [],
        "branch": ["repo=suite/offline"],
        "start": [],
        "override_disk_floor": False,
        "representation": "config",
        "format": "human",
        "wait_lock": 0.0,
        "offline_reason": None,
    }
    values.update(overrides)
    return argparse.Namespace(**values)


def _create(root: Path, **overrides: object) -> int:
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(
        io.StringIO()
    ):
        return wrkslots_cli._cmd_create(_args(root, **overrides))


def _events(root: Path) -> list[dict[str, object]]:
    directory = root / "worktrees" / "EVENTS.node-a"
    return [
        json.loads(path.read_text())
        for path in sorted(directory.iterdir())
        if path.suffix == ".json"
    ]


def test_normal_create_refuses_unreachable_remote(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    _break_remote(tmp_path / "case")
    with pytest.raises(wrkslots_cli.Refusal):
        _create(root)


def test_offline_create_skips_probe_and_records_reason_in_evidence(
    tmp_path: Path,
) -> None:
    root = _fixture(tmp_path / "case")
    _break_remote(tmp_path / "case")
    assert _create(root, offline_reason="GHE unreachable; provenance only") == 0

    active = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
    (record,) = active["slots"]
    assert record["slot"] == "slot-off"
    assert "offline_reason" not in record
    assert record["checkouts"][0]["remote_url_sha256"]

    created = [
        event
        for event in _events(root)
        if event.get("kind") == "active-state-recorded"
        and event["payload"].get("action") == "slot-created"
    ]
    assert len(created) == 1
    evidence = created[0]["payload"]["evidence"]
    assert evidence["offline_reason"] == "GHE unreachable; provenance only"
    # Journals are cleared on success and carry no offline field.
    control = root / "worktrees"
    assert not list(control.glob("CREATE.*"))


def test_offline_create_still_requires_a_local_start_ref(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    _break_remote(tmp_path / "case")
    with pytest.raises(wrkslots_cli.Refusal):
        _create(
            root,
            offline_reason="offline",
            start=["repo=refs/remotes/origin/missing"],
        )


def test_offline_reason_must_be_non_empty(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    with pytest.raises(wrkslots_cli.Refusal, match="offline-reason"):
        _create(root, offline_reason="  ")


def test_rust_create_text_offline_skips_fetch(tmp_path: Path) -> None:
    import subprocess

    from wrkslots.tests.rust_binary import rust_wrkslotsd

    root = _fixture(tmp_path / "rust")
    _break_remote(tmp_path / "rust")
    env = dict(os.environ)
    env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    result = subprocess.run(
        [
            str(rust_wrkslotsd()),
            "create-text",
            "slot-rust",
            "--slot-type",
            "agent",
            "--coordinator-authorized",
            "--agent",
            "agent-rust",
            "--task",
            "task-1",
            "--purpose",
            "provenance only",
            "--coordinator-pid",
            str(os.getpid()),
            "--owner-pid",
            str(os.getpid()),
            "--repo",
            "repo=../upstream-repo",
            "--branch",
            "repo=suite/rust-offline",
            "--offline-reason",
            "GHE unreachable; provenance only",
            "--machine",
            "node-a",
            "--root",
            str(root),
            "--format",
            "human",
        ],
        capture_output=True,
        text=True,
        check=False,
        timeout=900,
        env=env,
    )
    assert result.returncode == 0, result.stderr
    active = json.loads((root / "worktrees" / "ACTIVE.node-a.json").read_text())
    (record,) = active["slots"]
    assert "offline_reason" not in record
    created = [
        event
        for event in _events(root)
        if event.get("kind") == "active-state-recorded"
        and event["payload"].get("action") == "slot-created"
    ]
    assert created[0]["payload"]["evidence"]["offline_reason"] == (
        "GHE unreachable; provenance only"
    )
