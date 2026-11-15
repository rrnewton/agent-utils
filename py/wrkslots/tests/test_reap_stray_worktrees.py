"""Reap stray Git worktree registrations (#775).

The fixture registry owns one slot checkout; a second worktree of
the same source repository belongs to no slot. Plan mode reports
it, apply mode preserves a rescue ref and removes it, and a stray
with unpushed commits is retained. Temporary fixtures only.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import subprocess
from pathlib import Path

import pytest

from wrkslots import cli as wrkslots_cli

from wrkslots.tests.test_create_text_rust import _fixture, _git


def _stray(base: Path) -> Path:
    upstream = base / "upstream-repo"
    stray = base / "stray"
    _git(upstream, "worktree", "add", "-q", "-b", "stray-work", str(stray), "HEAD")
    return stray


def _run(root: Path, *, apply: bool, fmt: str = "human") -> tuple[int, str]:
    args = argparse.Namespace(
        project_root=str(root), machine="node-a",
        repository=[str(root.parent / "upstream-repo")],
        apply=apply, format=fmt,
    )
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        rc = wrkslots_cli._cmd_reap_stray_worktrees(args)
    return rc, buffer.getvalue()


def test_reap_stray_plan_then_apply(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    stray = _stray(tmp_path / "case")
    rc, out = _run(root, apply=False)
    assert rc == 0
    assert f"STRAY: {stray}" in out
    assert stray.exists()
    rc, out = _run(root, apply=True, fmt="json")
    assert rc == 0
    payload = json.loads(out)
    assert payload["applied"] is True
    assert payload["stray"][0]["outcome"] == "reaped"
    assert payload["stray"][0]["rescue_ref"].startswith("refs/rescue/wrkslots/stray-")
    assert not stray.exists()
    upstream = tmp_path / "case" / "upstream-repo"
    refs = _git(upstream, "for-each-ref", "--format=%(refname)", "refs/rescue/wrkslots/")
    assert "stray-" in refs


def _archived_record(slot: str, *, generation: int = 1) -> dict[str, object]:
    """One record in the archive schema `_archive_to_obj` writes (#877)."""

    finished = "2026-10-26T01:00:00Z"
    return {
        "archive_id": f"node-a:{slot}:{generation}:{finished}",
        "slot": slot,
        "agent": f"agent-{slot}",
        "task": "do work",
        "purpose": "testing",
        "machine": "node-a",
        "generation": generation,
        "created_at": "2026-10-26T00:00:00Z",
        "finished_at": finished,
        "mode": "remove",
        "actor": "coordinator",
        "physical_storage": "removed",
        "validation": ["clean"],
        "limitations": [],
        "continuation": "none",
        "checkouts": [
            {
                "name": "repo",
                "path": f"worktrees/{slot}/repo",
                "repository": "../upstream-repo",
                "branch": "suite/work",
                "start_point": "a" * 40,
                "remote": "origin",
                "remote_url_sha256": "b" * 64,
                "landed_ref": "refs/remotes/origin/main",
                "head": "a" * 40,
                "containing_remote_refs": ["refs/remotes/origin/main"],
                "vcs": "git",
            }
        ],
    }


def test_reap_stray_reports_past_archived_records(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    stray = _stray(tmp_path / "case")
    control = root / "worktrees"
    valid = _archived_record("slot-old")
    malformed = _archived_record("slot-bad")
    # Not the archive schema's "remove": the record is genuinely
    # malformed, as an interrupted or hand-edited archive can be.
    malformed["mode"] = "forget"
    (control / "ARCHIVED.node-a.json").write_text(
        json.dumps(
            {
                "schema": 2,
                "machine": "node-a",
                "revision": 4,
                "records": [valid, malformed],
            }
        )
    )
    (control / "EVENTS.node-a").mkdir(exist_ok=True)
    (control / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 1, "slots": []})
    )

    rc, out = _run(root, apply=False, fmt="json")
    assert rc == 0
    payload = json.loads(out)
    # The malformed record is named, and the report still covers the
    # stray; the valid record's repository is among the sources.
    assert payload["malformed_archives"] == [
        {
            "machine": "node-a",
            "slot": "slot-bad",
            "reason": "archive state.records[1].mode is invalid: 'forget'",
        }
    ]
    assert [row["path"] for row in payload["stray"]] == [str(stray)]
    assert payload["stray"][0]["outcome"] == "would-reap"

    # A malformed record does not bear on this removal, so apply works.
    rc, out = _run(root, apply=True, fmt="json")
    assert rc == 0
    payload = json.loads(out)
    assert payload["stray"][0]["outcome"] == "reaped"
    assert payload["malformed_archives"][0]["slot"] == "slot-bad"
    assert not stray.exists()


def _gone_archived_record(slot: str) -> dict[str, object]:
    record = _archived_record(slot)
    checkouts = record["checkouts"]
    assert isinstance(checkouts, list)
    checkout = checkouts[0]
    assert isinstance(checkout, dict)
    checkout["repository"] = "../gone-source"
    return record


def _active_record(slot: str, repository: str) -> dict[str, object]:
    identity: dict[str, object] = {
        "pid": 123,
        "start_ticks": 1123,
        "boot_id": "boot-1",
        "host_id": "host-1",
        "cgroup_path": "/user.slice/session.scope",
    }
    return {
        "slot": slot,
        "agent": f"agent-{slot}",
        "task": "do work",
        "purpose": "testing",
        "slot_type": "agent",
        "machine": "node-a",
        "generation": 1,
        "created_at": "2026-10-26T00:00:00Z",
        "heartbeat_at": "2026-10-26T01:00:00Z",
        "heartbeat_ttl_seconds": 60,
        "owner": identity,
        "coordinator_lease": dict(identity, pid=456, start_ticks=1456),
        "coordinator_recovery_note": None,
        "handoff": None,
        "checkouts": [
            {
                "name": "repo",
                "path": f"worktrees/{slot}/repo",
                "repository": repository,
                "branch": "suite/work",
                "start_point": "a" * 40,
                "remote": "origin",
                "remote_url_sha256": "b" * 64,
                "landed_ref": "refs/remotes/origin/main",
                "head": "a" * 40,
                "containing_remote_refs": ["refs/remotes/origin/main"],
                "vcs": "git",
            }
        ],
    }


def _write_registry(
    root: Path,
    *,
    active: list[dict[str, object]],
    archived: list[dict[str, object]],
) -> None:
    control = root / "worktrees"
    control.mkdir(parents=True, exist_ok=True)
    (control / "ARCHIVED.node-a.json").write_text(
        json.dumps(
            {"schema": 2, "machine": "node-a", "revision": 4, "records": archived}
        )
    )
    (control / "EVENTS.node-a").mkdir(exist_ok=True)
    (control / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 1, "slots": active})
    )


def test_reap_stray_report_skips_archived_record_with_deleted_repository(
    tmp_path: Path,
) -> None:
    # #1433 (1): the report census names and skips an archived row
    # whose source repository is gone, and still completes.
    root = _fixture(tmp_path / "case")
    stray = _stray(tmp_path / "case")
    _write_registry(
        root,
        active=[],
        archived=[_archived_record("slot-old"), _gone_archived_record("slot-gone")],
    )

    rc, out = _run(root, apply=False, fmt="json")
    assert rc == 0
    payload = json.loads(out)
    skipped = payload["unresolvable_archives"]
    assert len(skipped) == 1
    assert skipped[0]["machine"] == "node-a"
    assert skipped[0]["slot"] == "slot-gone"
    assert skipped[0]["repository"] == "../gone-source"
    assert "does not exist or cannot be resolved" in skipped[0]["reason"]
    assert [row["path"] for row in payload["stray"]] == [str(stray)]
    assert stray.exists()

    rc, out = _run(root, apply=False)
    assert rc == 0
    assert "UNRESOLVABLE-ARCHIVE: machine=node-a slot=slot-gone" in out
    assert "repository=../gone-source" in out


def test_reap_stray_report_still_refuses_active_record_with_deleted_repository(
    tmp_path: Path,
) -> None:
    # #1433 (3): an active row with a missing repository refuses in
    # both modes.
    root = _fixture(tmp_path / "case")
    _stray(tmp_path / "case")
    _write_registry(
        root,
        active=[_active_record("slot-live", "../gone-source")],
        archived=[_gone_archived_record("slot-gone")],
    )

    with pytest.raises(wrkslots_cli.Refusal, match="does not exist"):
        _run(root, apply=False)
    with pytest.raises(wrkslots_cli.Refusal, match="does not exist"):
        _run(root, apply=True)


def test_reap_stray_apply_skips_archived_record_with_deleted_repository(
    tmp_path: Path,
) -> None:
    # #1433 (2): with --apply the archived row is skipped and named,
    # and the real strays are still reaped: a gone source repository
    # cannot own any live registration, so refusing would block the
    # reap forever for no safety gain.
    root = _fixture(tmp_path / "case")
    stray = _stray(tmp_path / "case")
    _write_registry(
        root,
        active=[],
        archived=[_archived_record("slot-old"), _gone_archived_record("slot-gone")],
    )

    rc, out = _run(root, apply=True, fmt="json")
    assert rc == 0
    payload = json.loads(out)
    assert [row["slot"] for row in payload["unresolvable_archives"]] == ["slot-gone"]
    assert payload["stray"][0]["outcome"] == "reaped"
    assert not stray.exists()


def test_reap_stray_unpushed_is_retained(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    stray = _stray(tmp_path / "case")
    (stray / "new.txt").write_text("unpushed\n")
    _git(stray, "add", "new.txt")
    _git(stray, "commit", "-q", "-m", "unpushed work")
    rc, out = _run(root, apply=True)
    assert rc == 1
    assert "retained-unpushed" in out
    assert stray.exists()
