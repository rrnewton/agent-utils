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
