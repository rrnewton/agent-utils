"""The Rust-only meter's own contract (#542 rust-primary-rust-only-check)."""

from __future__ import annotations

import subprocess
from pathlib import Path


AU_ROOT = Path(__file__).resolve().parents[2]


def test_rust_only_self_test_passes() -> None:
    result = subprocess.run(
        ["python3", "scripts/check_rust_only.py", "--self-test"],
        cwd=AU_ROOT,
        capture_output=True,
        text=True,
        check=False,
        timeout=120,
    )
    assert result.returncode == 0, result.stderr
    assert "PASSED" in result.stdout
