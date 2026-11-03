"""Differential: wrkslotsd reclaim-orphan-state against Python (#796)."""

from __future__ import annotations

import argparse
import os
import subprocess
from pathlib import Path

from wrkslots import cli as wrkslots_cli
from wrkslots.tests.rust_binary import rust_wrkslotsd
from wrkslots.tests.test_reclaim_slot_state import _home, _make_orphan
from wrkslots.tests.test_remove_write_rust import _fixture


AU_ROOT = Path(__file__).resolve().parents[3]


def _rust(root: Path, *, apply: bool) -> subprocess.CompletedProcess[str]:
    args = [
        str(rust_wrkslotsd()), "reclaim-orphan-state",
        "--root", str(root), "--format", "json",
    ]
    if apply:
        args.append("--apply")
    return subprocess.run(
        args, cwd=AU_ROOT, capture_output=True, text=True,
        check=False, timeout=900, env=dict(os.environ),
    )


def _python(root: Path, *, apply: bool) -> str:
    import contextlib
    import io

    args = argparse.Namespace(
        project_root=str(root), machine=None, apply=apply, format="json",
    )
    buffer = io.StringIO()
    with contextlib.redirect_stdout(buffer):
        rc = wrkslots_cli._cmd_reclaim_orphan_state(args)
    assert rc == 0
    return buffer.getvalue()


def _norm(text: str, root: Path) -> str:
    return text.replace(str(root), "ROOT")


def test_reclaim_orphan_rust_plan_and_apply_match_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    rust_home = _make_orphan(rust_root)
    _make_orphan(oracle_root)

    planned = _rust(rust_root, apply=False)
    assert planned.returncode == 0, planned.stderr
    assert _norm(planned.stdout, rust_root) == _norm(
        _python(oracle_root, apply=False), oracle_root
    )
    assert rust_home.exists(), "plan must not delete"

    applied = _rust(rust_root, apply=True)
    assert applied.returncode == 0, applied.stderr
    _python(oracle_root, apply=True)
    assert not rust_home.exists()
    assert not (oracle_root / "worktrees" / "slot-state" / "agent" / "slot-a").exists()
