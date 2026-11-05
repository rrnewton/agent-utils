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
