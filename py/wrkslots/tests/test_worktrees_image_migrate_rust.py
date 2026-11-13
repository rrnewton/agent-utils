"""Differential: wrkslotsd worktrees-image migrate against Python.

#1291 migrate/convert. Fixture projects only: the image is never
mounted, so the cases are the state and configuration refusals,
plus the direct-layout boundary (Rust refuses it by name until a
direct handler exists; Python would run its direct path).
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


def _root(tmp_path: Path, section: bool = True, direct: bool = False) -> Path:
    root = tmp_path / "project"
    control = root / "worktrees"
    control.mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    extra = ""
    if section:
        extra = "worktrees_image:\n  path: wt.img\n  size_bytes: 1073741824\n"
        if direct:
            extra += "  mount_at: worktrees_dir\n"
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


def _strip_load_warning(text: str) -> str:
    lines = [
        line
        for line in text.splitlines()
        if not line.startswith("wrkslots: WARNING: worktrees image not mounted:")
    ]
    return "\n".join(lines) + ("\n" if text else "")


def _python(root: Path, capsys: pytest.CaptureFixture[str]) -> tuple[int, str, str]:
    args = argparse.Namespace(
        project_root=str(root), machine="node-a", wait_lock=0,
        slot="slot-a", slot_type="agent",
    )
    try:
        rc = imagecmd._cmd_migrate(args)
        err = ""
    except wrkslots_cli.Refusal as exc:
        rc = 3
        remedy = wrkslots_cli._refusal_remedy(
            argparse.Namespace(command="worktrees-image"), exc
        )
        err = f"REFUSED: {exc}\nREMEDY: {remedy}\n"
    captured = capsys.readouterr()
    return rc, captured.out, err or _strip_load_warning(captured.err)


def _rust(root: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(rust_wrkslotsd()), "worktrees-image", "migrate", "slot-a"],
        cwd=root, capture_output=True, text=True, check=False, timeout=900,
        env={
            **os.environ,
            "WRKSLOTS_PYCOMPAT": "1",
            "WRKSLOTSD_FIXTURE_WRITER": "1",
        },
    )


def test_migrate_not_mounted_refusal_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    # Separate roots: the Python loader creates the mount
    # directory during its own load, which would change the
    # state the Rust side then classifies on a shared root.
    expected = _python(_root(tmp_path / "oracle"), capsys)
    result = _rust(_root(tmp_path / "rust"))
    assert (result.returncode, result.stdout, _strip_load_warning(result.stderr)) == expected


def test_migrate_unconfigured_refusal_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str]
) -> None:
    expected = _python(_root(tmp_path / "oracle", section=False), capsys)
    result = _rust(_root(tmp_path / "rust", section=False))
    assert (result.returncode, result.stdout, _strip_load_warning(result.stderr)) == expected


def test_migrate_direct_layout_refused_by_name(tmp_path: Path) -> None:
    root = _root(tmp_path, direct=True)
    result = _rust(root)
    assert result.returncode == 3
    assert "direct) layout stays with the Python edition" in result.stderr
