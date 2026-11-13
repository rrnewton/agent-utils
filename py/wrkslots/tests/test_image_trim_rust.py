"""Differential: wrkslotsd image-trim against Python `image trim`.

#1116 mount family, trim step. Fixture images are unmounted, so
neither edition runs a privileged step; outputs and refusals must
agree byte for byte. The Rust glue resolves its privileged tool
paths through the launcher interpreter and the package helper, as
production does, so the test exports both.
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli
from wrkslots import imagecmd, slotimage

from wrkslots.tests.rust_binary import rust_wrkslotsd


AU_ROOT = Path(__file__).resolve().parents[3]
GIB = 1024 ** 3


def _root(tmp_path: Path, with_record: bool = True) -> Path:
    root = tmp_path / "project"
    control = root / "worktrees"
    control.mkdir(parents=True)
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
    if with_record:
        directory = control / "slot-images" / "agent" / "slot-a"
        directory.mkdir(parents=True)
        (directory / slotimage.RECORD_NAME).write_text(
            json.dumps(
                {
                    "schema": 1,
                    "slot": "slot-a",
                    "slot_type": "agent",
                    "location": str(control / "slot-a"),
                    "ceiling_bytes": 3 * GIB,
                    "state_ceiling_bytes": GIB,
                    "backend": "kernel",
                    "created_at": "2026-11-07T00:00:00+00:00",
                    "phase": "ready",
                },
                indent=2,
                sort_keys=True,
            )
        )
        for name, size in (("slot.img", 3 * GIB), ("state.img", GIB)):
            with (directory / name).open("wb") as handle:
                handle.truncate(size)
    return root


@pytest.fixture(autouse=True)
def _no_mount_activation(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(slotimage, "ensure_all_mounted", lambda *a, **k: ([], []))
    monkeypatch.setattr(wrkslots_cli, "_OWNED_REPRESENTATION_MOUNTS", frozenset())


def _python(root: Path, slot: str, capsys: pytest.CaptureFixture[str]) -> tuple[int, str, str]:
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", slot=slot, slot_type="agent"
    )
    try:
        rc = imagecmd._cmd_image_trim(args)
        err = ""
    except wrkslots_cli.Refusal as exc:
        rc = 3
        remedy = wrkslots_cli._refusal_remedy(argparse.Namespace(command="image"), exc)
        err = f"REFUSED: {exc}\nREMEDY: {remedy}\n"
    captured = capsys.readouterr()
    return rc, captured.out, err or captured.err


def _rust(root: Path, slot: str) -> subprocess.CompletedProcess[str]:
    env = {
        **os.environ,
        "WRKSLOTS_PYCOMPAT": "1",
        "WRKSLOTS_PYTHON_BIN": sys.executable,
        "WRKSLOTS_IMAGEHELPER": str(AU_ROOT / "py" / "wrkslots" / "imagehelper.py"),
    }
    return subprocess.run(
        [str(rust_wrkslotsd()), "image", "trim", slot],
        cwd=root, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def test_image_trim_unmounted_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path)
    expected = _python(root, "slot-a", capsys)
    result = _rust(root, "slot-a")
    assert (result.returncode, result.stdout, result.stderr) == expected


def test_image_trim_not_image_backed_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path, with_record=False)
    expected = _python(root, "slot-a", capsys)
    result = _rust(root, "slot-a")
    assert (result.returncode, result.stdout, result.stderr) == expected


def test_image_convert_stays_refused(tmp_path: Path) -> None:
    root = _root(tmp_path)
    result = subprocess.run(
        [str(rust_wrkslotsd()), "image", "convert", "slot-a", "--to", "worktree"],
        cwd=root, capture_output=True, text=True, check=False, timeout=900,
        env={**os.environ, "WRKSLOTS_PYCOMPAT": "1"},
    )
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
