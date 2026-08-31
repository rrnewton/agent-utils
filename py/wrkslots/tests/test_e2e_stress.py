"""Required real-process end-to-end smoke test."""

from __future__ import annotations

import subprocess
import sys
from pathlib import Path


def test_real_process_and_git_invariants() -> None:
    runner = Path(__file__).with_name("e2e_stress.py")
    command = [
        sys.executable,
        str(runner),
        "--seed",
        "7",
        "--workers",
        "2",
        "--seconds",
        "0.15",
    ]
    namespace = subprocess.run(
        ["unshare", "--user", "--map-root-user", "--pid", "--fork", "--mount-proc", "true"],
        text=True,
        capture_output=True,
        check=False,
    )
    if namespace.returncode == 0:
        command = [
            "unshare",
            "--user",
            "--map-root-user",
            "--pid",
            "--fork",
            "--mount-proc",
            *command,
        ]
    completed = subprocess.run(
        command,
        text=True,
        capture_output=True,
        check=False,
        timeout=600,
    )

    assert completed.returncode == 0, f"{completed.stdout}\n{completed.stderr}"
    assert "wrkslots e2e passed; seed=7" in completed.stdout
