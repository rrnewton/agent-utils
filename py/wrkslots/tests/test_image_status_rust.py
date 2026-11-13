"""Differential: wrkslotsd image-status against Python `image status`.

#1095/#1116 image glue batch. The Python oracle is
`imagecmd._cmd_image_status` in-process with mount activation on
config load stubbed out (the Rust loader never mounts; that is the
recorded divergence), on fixture projects where nothing is mounted,
so both editions read the same post-load state and must agree byte
for byte on stdout and exit code. The worktrees-image case compares
stdout only: each edition's load-time not-mounted warning goes to
stderr with its own reason text.
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


AU_ROOT = Path(__file__).resolve().parents[3]
GIB = 1024 ** 3


def _root(tmp_path: Path, extra: str = "") -> Path:
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
        "liveness_command: live.py\n"
        "slot_representation: image\n"
        + extra
    )
    return root


def _record(root: Path, slot: str = "slot-a", with_images: bool = True) -> Path:
    control = root / "worktrees"
    directory = control / "slot-images" / "agent" / slot
    directory.mkdir(parents=True)
    location = control / slot
    (directory / slotimage.RECORD_NAME).write_text(
        json.dumps(
            {
                "schema": 1,
                "slot": slot,
                "slot_type": "agent",
                "location": str(location),
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
    if with_images:
        for name, size in (("slot.img", 3 * GIB), ("state.img", GIB)):
            path = directory / name
            with path.open("wb") as handle:
                handle.truncate(size)
                if name == "slot.img":
                    handle.write(b"x" * (1024 * 1024))
    return directory


@pytest.fixture(autouse=True)
def _no_mount_activation(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(slotimage, "ensure_all_mounted", lambda *a, **k: ([], []))
    monkeypatch.setattr(wrkslots_cli, "_OWNED_REPRESENTATION_MOUNTS", frozenset())


def _python(root: Path, capsys: pytest.CaptureFixture[str], fmt: str) -> tuple[int, str, str]:
    args = argparse.Namespace(project_root=str(root), machine="node-a", format=fmt)
    try:
        rc = imagecmd._cmd_image_status(args)
        err_text = ""
    except wrkslots_cli.Refusal as exc:
        rc = 3
        remedy = wrkslots_cli._refusal_remedy(argparse.Namespace(command="image"), exc)
        err_text = f"REFUSED: {exc}\nREMEDY: {remedy}\n"
    captured = capsys.readouterr()
    return rc, captured.out, err_text or captured.err


def _rust(root: Path, fmt: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(rust_wrkslotsd()), "image", "status", "--format", fmt],
        cwd=root, capture_output=True, text=True, check=False, timeout=900,
        env={**os.environ, "WRKSLOTS_PYCOMPAT": "1"},
    )


def test_image_status_unmounted_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path)
    _record(root)
    for fmt in ("human", "json"):
        expected = _python(root, capsys, fmt)
        result = _rust(root, fmt)
        assert (result.returncode, result.stdout, result.stderr) == expected


def test_image_status_empty_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path)
    for fmt in ("human", "json"):
        expected = _python(root, capsys, fmt)
        result = _rust(root, fmt)
        assert (result.returncode, result.stdout, result.stderr) == expected


def test_image_status_missing_image_files_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path)
    _record(root, with_images=False)
    for fmt in ("human", "json"):
        expected = _python(root, capsys, fmt)
        result = _rust(root, fmt)
        assert (result.returncode, result.stdout, result.stderr) == expected


def test_image_status_bad_record_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path)
    directory = _record(root)
    (directory / slotimage.RECORD_NAME).write_text("{}")
    for fmt in ("human", "json"):
        expected = _python(root, capsys, fmt)
        result = _rust(root, fmt)
        assert (result.returncode, result.stdout, result.stderr) == expected


def test_image_status_worktrees_image_matches_python_stdout(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(
        tmp_path,
        "worktrees_image:\n  path: wt.img\n  size_bytes: 1073741824\n",
    )
    for fmt in ("human", "json"):
        rc, out, _err = _python(root, capsys, fmt)
        result = _rust(root, fmt)
        assert result.returncode == rc
        assert result.stdout == out


def test_image_other_actions_stay_refused(tmp_path: Path) -> None:
    root = _root(tmp_path)
    result = subprocess.run(
        [str(rust_wrkslotsd()), "image", "convert", "slot-a", "--to", "worktree"],
        cwd=root, capture_output=True, text=True, check=False, timeout=900,
        env={**os.environ, "WRKSLOTS_PYCOMPAT": "1"},
    )
    assert result.returncode == 1
    assert "fixture-only" in result.stderr
