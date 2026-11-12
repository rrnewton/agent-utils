"""Differential: wrkslotsd image mount/unmount/grow against Python.

#1116 mount family. Fixture registries only: nothing is mounted,
so neither edition runs a privileged step. Mount covers the empty
registry; unmount an unmounted image; grow covers the shrink
refusal (which precedes any mount in both editions).
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
        (control / "slot-a").mkdir()
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


def _oracle(
    capsys: pytest.CaptureFixture[str], handler: object, args: argparse.Namespace
) -> tuple[int, str, str]:
    try:
        rc = handler(args)  # type: ignore[operator]
        err = ""
    except wrkslots_cli.Refusal as exc:
        rc = 3
        remedy = wrkslots_cli._refusal_remedy(argparse.Namespace(command="image"), exc)
        err = f"REFUSED: {exc}\nREMEDY: {remedy}\n"
    captured = capsys.readouterr()
    return rc, captured.out, err or captured.err


def _rust(root: Path, *argv: str) -> subprocess.CompletedProcess[str]:
    env = {
        **os.environ,
        "WRKSLOTS_PYCOMPAT": "1",
        "WRKSLOTSD_FIXTURE_WRITER": "1",
        "WRKSLOTS_PYTHON_BIN": sys.executable,
        "WRKSLOTS_IMAGEHELPER": str(AU_ROOT / "py" / "wrkslots" / "imagehelper.py"),
    }
    return subprocess.run(
        [str(rust_wrkslotsd()), "image", *argv],
        cwd=root, capture_output=True, text=True, check=False, timeout=900, env=env,
    )


def test_image_mount_empty_registry_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path, with_record=False)
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", wait_lock=0,
        coordinator_authorized=False,
    )
    expected = _oracle(capsys, imagecmd._cmd_image_mount, args)
    result = _rust(root, "mount")
    assert (result.returncode, result.stdout, result.stderr) == expected


def test_image_unmount_unmounted_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path)
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", wait_lock=0,
        slot="slot-a", slot_type="agent",
    )
    expected = _oracle(capsys, imagecmd._cmd_image_unmount, args)
    result = _rust(root, "unmount", "slot-a")
    assert (result.returncode, result.stdout, result.stderr) == expected


def test_image_grow_shrink_refusal_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path)
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", wait_lock=0,
        slot="slot-a", slot_type="agent", ceiling_gib=1,
    )
    expected = _oracle(capsys, imagecmd._cmd_image_grow, args)
    result = _rust(root, "grow", "slot-a", "--ceiling-gib", "1")
    assert (result.returncode, result.stdout, result.stderr) == expected


def test_image_unmount_not_image_backed_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path, with_record=False)
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", wait_lock=0,
        slot="slot-a", slot_type="agent",
    )
    expected = _oracle(capsys, imagecmd._cmd_image_unmount, args)
    result = _rust(root, "unmount", "slot-a")
    assert (result.returncode, result.stdout, result.stderr) == expected
