"""Differential: wrkslotsd image-set-default against Python.

#1116 image glue batch. The Python oracle is
`imagecmd._cmd_image_set_default` in-process on a fixture project;
the Rust side runs the built binary with the fixture-writer gate
on. Both rewrite the configuration in its own format and save the
previous bytes to `.wrkslots.yml.bak`; stdout and the resulting
file bytes must agree.
"""

from __future__ import annotations

import argparse
import os
import subprocess
from pathlib import Path

import pytest
from wrkslots import imagecmd

from wrkslots.tests.rust_binary import rust_wrkslotsd


def _root(tmp_path: Path, representation: str | None) -> Path:
    root = tmp_path / "project"
    (root / "worktrees").mkdir(parents=True)
    (root / "live.py").write_text("# probe\n")
    extra = "" if representation is None else f"slot_representation: {representation}\n"
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
    return root


def _python(root: Path, representation: str, capsys: pytest.CaptureFixture[str]) -> str:
    args = argparse.Namespace(
        project_root=str(root), machine="node-a",
        representation=representation, wait_lock=0,
    )
    assert imagecmd._cmd_image_set_default(args) == 0
    return capsys.readouterr().out


def _rust(root: Path, representation: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(rust_wrkslotsd()), "image", "set-default", representation],
        cwd=root, capture_output=True, text=True, check=False, timeout=900,
        env={**os.environ, "WRKSLOTS_PYCOMPAT": "1", "WRKSLOTSD_FIXTURE_WRITER": "1"},
    )


@pytest.mark.parametrize("start,target", [(None, "image"), ("image", "worktree")])
def test_image_set_default_matches_python(
    tmp_path: Path, capsys: pytest.CaptureFixture[str], start: str | None, target: str
) -> None:
    python_root = _root(tmp_path / "py", start)
    rust_root = _root(tmp_path / "rs", start)
    expected = _python(python_root, target, capsys)
    result = _rust(rust_root, target)
    assert result.returncode == 0, result.stderr
    assert result.stdout == expected
    assert result.stderr == ""
    python_text = (python_root / ".wrkslots.yml").read_text()
    rust_text = (rust_root / ".wrkslots.yml").read_text()
    # The fixture roots differ, and the canonical rendering embeds none.
    assert rust_text == python_text
    assert (rust_root / ".wrkslots.yml.bak").exists()
    assert (python_root / ".wrkslots.yml.bak").exists()
