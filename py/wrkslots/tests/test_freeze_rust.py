"""Differential: wrkslotsd freeze against the Python freeze (#780)."""

from __future__ import annotations

import os
import subprocess
from pathlib import Path

from wrkslots.tests.rust_binary import rust_wrkslotsd
from wrkslots.tests.test_retire_freeze import _freeze
from wrkslots.tests.test_create_text_rust import _fixture


AU_ROOT = Path(__file__).resolve().parents[3]


def _rust(root: Path, *argv: str) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(rust_wrkslotsd()), "freeze", *argv, "--root", str(root), "--machine", "node-a"],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=dict(os.environ),
    )


def test_freeze_rust_matches_python(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    oracle_root = _fixture(tmp_path / "oracle")
    _, expected = _freeze(oracle_root, "set", "isolation", "migration")
    result = _rust(rust_root, "set", "isolation", "migration")
    assert result.returncode == 0, result.stderr
    assert result.stdout == expected
    _, expected = _freeze(oracle_root, "status")
    result = _rust(rust_root, "status")
    assert result.stdout == expected
    _, expected = _freeze(oracle_root, "clear")
    result = _rust(rust_root, "clear")
    assert result.stdout == expected


def test_freeze_rust_remove_refuses_when_frozen(tmp_path: Path) -> None:
    rust_root = _fixture(tmp_path / "rust")
    result = _rust(rust_root, "set", "planned", "migration")
    assert result.returncode == 0, result.stderr
    env = dict(os.environ)
    env["WRKSLOTSD_FIXTURE_WRITER"] = "1"
    removed = subprocess.run(
        [
            str(rust_wrkslotsd()), "remove", "slot-a",
            "--coordinator-pid", str(os.getpid()), "--expected-generation", "1",
            "--root", str(rust_root), "--machine", "node-a",
        ],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900, env=env,
    )
    assert removed.returncode == 1
    assert "retire-freeze:" in removed.stderr
    assert "planned migration" in removed.stderr
