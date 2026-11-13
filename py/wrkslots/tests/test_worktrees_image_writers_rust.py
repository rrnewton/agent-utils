"""Differential: wrkslotsd worktrees-image writers against Python.

#1141 worktrees-image family, writer steps. Fixture projects only:
no image is ever created or mounted, so every case is a refusal or
an early validation both editions must word identically.
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


GIB = 1024 ** 3


def _root(tmp_path: Path, direct: bool = False) -> Path:
    root = tmp_path / "project"
    control = root / "worktrees"
    control.mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    mount_at = "  mount_at: worktrees_dir\n" if direct else ""
    (root / ".wrkslots.yml").write_text(
        "schema: 2\n"
        "machine: node-a\n"
        "worktrees_dir: worktrees\n"
        "default_remote: origin\n"
        "default_landed_ref: refs/remotes/origin/main\n"
        "heartbeat_ttl_seconds: 60\n"
        "liveness_command: live.py\n"
        "worktrees_image:\n  path: wt.img\n  size_bytes: 1073741824\n"
        + mount_at
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


def _refusal(command: str) -> str:
    return f"REMEDY: run 'wrkslots {command} --help'"

def _strip_load_warning(text: str) -> str:
    # Each edition warns once that the image is not mounted on
    # load (#1148), with its own reason text; that line is not
    # part of the command output under comparison.
    return "\n".join(
        line
        for line in text.splitlines()
        if not line.startswith("wrkslots: WARNING: worktrees image not mounted:")
    ) + ("\n" if text else "")


def _oracle(
    capsys: pytest.CaptureFixture[str], handler: object, args: argparse.Namespace
) -> tuple[int, str, str]:
    try:
        rc = handler(args)  # type: ignore[operator]
        err = ""
    except wrkslots_cli.Refusal as exc:
        rc = 3
        remedy = wrkslots_cli._refusal_remedy(
            argparse.Namespace(command="worktrees-image"), exc
        )
        err = f"REFUSED: {exc}\nREMEDY: {remedy}\n"
    captured = capsys.readouterr()
    return rc, captured.out, err or _strip_load_warning(captured.err)


def _rust(root: Path, *argv: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(rust_wrkslotsd()), "worktrees-image", *argv],
        cwd=root, capture_output=True, text=True, check=False, timeout=900,
        env={
            **os.environ,
            "WRKSLOTS_PYCOMPAT": "1",
            "WRKSLOTSD_FIXTURE_WRITER": "1",
        },
    )


def _args(root: Path, **extra: object) -> argparse.Namespace:
    return argparse.Namespace(
        project_root=str(root), machine="node-a", wait_lock=0, **extra
    )


def test_wt_mount_unconfigured_refusal_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path)
    config_path = root / ".wrkslots.yml"
    config_path.write_text(
        config_path.read_text().replace(
            "worktrees_image:\n  path: wt.img\n  size_bytes: 1073741824\n", ""
        )
    )
    expected = _oracle(capsys, imagecmd._cmd_wt_image_mount, _args(root))
    result = _rust(root, "mount")
    assert (result.returncode, result.stdout, _strip_load_warning(result.stderr)) == expected


def test_wt_unmount_direct_refusal_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path, direct=True)
    expected = _oracle(capsys, imagecmd._cmd_wt_image_unmount, _args(root))
    result = _rust(root, "unmount")
    assert (result.returncode, result.stdout, _strip_load_warning(result.stderr)) == expected


def test_wt_grow_shrink_refusal_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path)
    image = root / "wt.img"
    with image.open("wb") as handle:
        handle.truncate(3 * GIB)
    expected = _oracle(
        capsys, imagecmd._cmd_wt_image_grow, _args(root, to_size_bytes=1)
    )
    result = _rust(root, "grow", "--to-size-bytes", "1")
    assert (result.returncode, result.stdout, _strip_load_warning(result.stderr)) == expected


def test_wt_create_pair_refusal_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    root = _root(tmp_path)
    expected = _oracle(
        capsys,
        imagecmd._cmd_wt_image_create,
        _args(root, path="wt2.img", size_bytes=None),
    )
    result = _rust(root, "create", "--path", "wt2.img")
    assert (result.returncode, result.stdout, _strip_load_warning(result.stderr)) == expected
