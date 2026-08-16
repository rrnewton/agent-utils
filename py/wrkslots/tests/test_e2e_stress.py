"""Required real-process end-to-end smoke test."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path


def test_real_process_and_git_invariants() -> None:
    runner = Path(__file__).with_name("e2e_stress.py")
    completed = subprocess.run(
        [sys.executable, str(runner), "--seed", "7", "--workers", "2", "--seconds", "0.15"],
        text=True,
        capture_output=True,
        check=False,
        timeout=600,
    )

    assert completed.returncode == 0, f"{completed.stdout}\n{completed.stderr}"
    assert "wrkslots e2e passed; seed=7" in completed.stdout
