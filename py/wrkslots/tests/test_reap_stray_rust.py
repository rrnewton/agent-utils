"""Differential: wrkslotsd reap-stray-worktrees against Python (#775)."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from wrkslots.tests.rust_binary import rust_wrkslotsd
from wrkslots.tests.test_reap_stray_worktrees import _run, _stray
from wrkslots.tests.test_create_text_rust import _fixture


AU_ROOT = Path(__file__).resolve().parents[3]


def _rust(root: Path, upstream: Path, *, apply: bool, fmt: str) -> subprocess.CompletedProcess[str]:
    args = [
        str(rust_wrkslotsd()), "reap-stray-worktrees",
        "--repository", str(upstream), "--root", str(root), "--format", fmt,
    ]
    if apply:
        args.append("--apply")
    return subprocess.run(args, cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=dict(os.environ))


def _norm(text: str, root: Path) -> str:
    return text.replace(str(root), "ROOT").replace(str(root.parent), "BASE")


def test_reap_stray_rust_plan_matches_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    rust_stray = _stray(tmp_path / "rust")
    _stray(tmp_path / "oracle")
    rc, out = _run(oracle_root, apply=False)
    assert rc == 0
    result = _rust(rust_root, tmp_path / "rust" / "upstream-repo", apply=False, fmt="human")
    assert result.returncode == 0, result.stderr
    assert _norm(result.stdout, rust_root) == _norm(out, oracle_root)
    assert rust_stray.exists()


def test_reap_stray_rust_apply_reaps(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    stray = _stray(tmp_path / "rust")
    upstream = tmp_path / "rust" / "upstream-repo"
    result = _rust(rust_root, upstream, apply=True, fmt="json")
    assert result.returncode == 0, result.stderr
    assert '"reaped"' in result.stdout
    assert not stray.exists()
