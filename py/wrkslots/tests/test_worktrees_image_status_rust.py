"""Differential: wrkslotsd worktrees-image-status against Python.

#1141 worktrees-image family, status step. The fixture has a
worktrees_image section and no image file, so neither edition
mounts anything; stdout and exit code must agree byte for byte
(each edition's load warning goes to its own stderr).
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli
from wrkslots import imagecmd, slotimage

from wrkslots.tests.rust_binary import rust_wrkslotsd


def _root(tmp_path: Path, section: bool = True) -> Path:
    root = tmp_path / "project"
    control = root / "worktrees"
    control.mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    extra = (
        "worktrees_image:\n  path: wt.img\n  size_bytes: 1073741824\n"
        if section
        else ""
    )
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
        + extra
    )
    (control / "ACTIVE.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 0, "slots": []})
    )
    (control / "ARCHIVED.node-a.json").write_text(
        json.dumps({"schema": 2, "machine": "node-a", "revision": 0, "records": []})
    )
    return root


@pytest.fixture(autouse=True)
def _no_mount_activation(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(slotimage, "ensure_all_mounted", lambda *a, **k: ([], []))
    monkeypatch.setattr(wrkslots_cli, "_OWNED_REPRESENTATION_MOUNTS", frozenset())


def _python(root: Path, capsys: pytest.CaptureFixture[str], fmt: str) -> tuple[int, str, str]:
    args = argparse.Namespace(project_root=str(root), machine="node-a", format=fmt)
    try:
        rc = imagecmd._cmd_wt_image_status(args)
        err = ""
    except wrkslots_cli.Refusal as exc:
        rc = 3
        remedy = wrkslots_cli._refusal_remedy(
            argparse.Namespace(command="worktrees-image"), exc
        )
        err = f"REFUSED: {exc}\nREMEDY: {remedy}\n"
    captured = capsys.readouterr()
    return rc, captured.out, err or captured.err


def _rust(root: Path, fmt: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(rust_wrkslotsd()), "worktrees-image", "status", "--format", fmt],
        cwd=root, capture_output=True, text=True, check=False, timeout=900,
        env={**os.environ, "WRKSLOTS_PYCOMPAT": "1"},
    )


def test_wt_status_matches_python_stdout(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path)
    for fmt in ("human", "json"):
        rc, out, _err = _python(root, capsys, fmt)
        result = _rust(root, fmt)
        assert result.returncode == rc
        assert result.stdout == out


def test_wt_status_unconfigured_refusal_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path, section=False)
    expected = _python(root, capsys, "human")
    result = _rust(root, "human")
    assert (result.returncode, result.stdout, result.stderr) == expected


def test_wt_migrate_stays_refused(tmp_path: Path) -> None:
    root = _root(tmp_path)
    result = subprocess.run(
        [str(rust_wrkslotsd()), "worktrees-image", "migrate", "slot-a"],
        cwd=root, capture_output=True, text=True, check=False, timeout=900,
        env={**os.environ, "WRKSLOTS_PYCOMPAT": "1"},
    )
    assert result.returncode == 1
    assert "'worktrees-image' stays with the Python edition in this slice" in result.stderr
