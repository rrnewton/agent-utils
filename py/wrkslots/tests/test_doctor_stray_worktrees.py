"""Doctor stray-worktree census (#1930).

A scratch registry with one managed slot checkout, one worktree
created outside wrkslots, and nested worktrees under the slot.
The census is read-only: it reports path, branch, age, bounded
size and unpushed state, and deletes nothing. Temporary fixtures
only.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import os
from pathlib import Path

from wrkslots import cli as wrkslots_cli

from wrkslots.tests.test_create_text_rust import _fixture, _git, _repo


def _create_slot(root: Path) -> Path:
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", slot="slot-b", slot_type="agent",
        coordinator_authorized=True, agent="agent-new", task="task-1",
        purpose="testing", owner_pid=os.getpid(), coordinator_pid=os.getpid(),
        task_scope_unit=None, task_scope_invocation_id=None,
        repo=["repo=../upstream-repo"], remote=[], remote_url=[],
        branch=["repo=suite/work"], start=[], override_disk_floor=False,
        representation="config", format="human", wait_lock=0.0,
        offline_reason=None,
    )
    with contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(io.StringIO()):
        assert wrkslots_cli._cmd_create(args) == 0
    return root / "worktrees" / "slot-b" / "repo"


def _run(root: Path, fmt: str = "json") -> tuple[int, str]:
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", stray_worktrees=True,
        quick=False, repository=[], format=fmt,
    )
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        rc = wrkslots_cli._cmd_doctor_scan(args)
    return rc, buffer.getvalue()


def test_doctor_stray_worktrees_reports_unmanaged_only(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    managed = _create_slot(root)
    upstream = tmp_path / "case" / "upstream-repo"
    slot_dir = root / "worktrees" / "slot-b"

    outside = tmp_path / "case" / "outside-stray"
    _git(upstream, "worktree", "add", "-q", "-b", "outside-work", str(outside), "HEAD")

    nested = slot_dir / "nested-stray"
    _git(upstream, "worktree", "add", "-q", "-b", "nested-work", str(nested), "HEAD")
    (nested / "new.txt").write_text("unpushed\n")
    _git(nested, "add", "new.txt")
    _git(nested, "commit", "-q", "-m", "unpushed nested work")

    other_repo = tmp_path / "case" / "other-repo"
    _repo(other_repo)
    other_nested = slot_dir / "other-nested"
    _git(other_repo, "worktree", "add", "-q", "-b", "other-work", str(other_nested), "HEAD")

    rc, out = _run(root)
    assert rc == 0
    rows = {row["path"]: row for row in json.loads(out)["stray_worktrees"]}
    assert set(rows) == {str(outside), str(nested), str(other_nested)}
    assert str(managed) not in rows
    assert rows[str(outside)]["kind"] == "outside-slot"
    assert rows[str(outside)]["branch"] == "outside-work"
    assert rows[str(outside)]["has_unpushed_commits"] is False
    assert rows[str(nested)]["kind"] == "nested-in-slot"
    assert rows[str(nested)]["slot"] == "slot-b"
    assert rows[str(nested)]["has_unpushed_commits"] is True
    assert rows[str(other_nested)]["kind"] == "nested-in-slot"
    for row in rows.values():
        assert isinstance(row["age_seconds"], int) and row["age_seconds"] >= 0
        assert row["size_bytes"] > 0
        assert row["size_truncated"] is False
    # Read-only: nothing was deleted.
    assert outside.exists() and nested.exists() and other_nested.exists()


def test_rust_doctor_text_stray_worktrees_matches(tmp_path: Path) -> None:
    import subprocess

    from wrkslots.tests.rust_binary import rust_wrkslotsd

    root = _fixture(tmp_path / "rust")
    _create_slot(root)
    upstream = tmp_path / "rust" / "upstream-repo"
    stray = tmp_path / "rust" / "stray"
    _git(upstream, "worktree", "add", "-q", "-b", "stray-work", str(stray), "HEAD")
    env = dict(os.environ)
    result = subprocess.run(
        [
            str(rust_wrkslotsd()),
            "doctor-text",
            "--stray-worktrees",
            "--machine",
            "node-a",
            "--root",
            str(root),
            "--format",
            "json",
        ],
        capture_output=True,
        text=True,
        check=False,
        timeout=900,
        env=env,
    )
    assert result.returncode == 0, result.stderr
    rows = json.loads(result.stdout)["stray_worktrees"]
    assert [row["path"] for row in rows] == [str(stray)]
    assert rows[0]["kind"] == "outside-slot"
    assert rows[0]["branch"] == "stray-work"
    assert rows[0]["has_unpushed_commits"] is False
    assert rows[0]["size_bytes"] > 0
    assert stray.exists()


def test_doctor_stray_worktrees_human_form(tmp_path: Path) -> None:
    root = _fixture(tmp_path / "case")
    _create_slot(root)
    upstream = tmp_path / "case" / "upstream-repo"
    stray = tmp_path / "case" / "stray"
    _git(upstream, "worktree", "add", "-q", "-b", "stray-work", str(stray), "HEAD")
    rc, out = _run(root, fmt="human")
    assert rc == 0
    assert "stray_worktrees=1" in out
    assert f"STRAY-WORKTREE: path={stray}" in out
    assert stray.exists()
