"""Gated fast subset of the seeded lifecycle fuzz (#2043).

Seeds 1..4 at bounded steps; every wait is condition-based
(decode polling inside the runner), never a sleep-as-sync.
The long soak is opt-in by hand:
    python3 py/wrkslots/tests/lifecycle_fuzz.py --seed N --steps 60
"""

from __future__ import annotations

import resource
import subprocess
import sys
from pathlib import Path


def _limits() -> None:
    resource.setrlimit(resource.RLIMIT_CPU, (600, 600))
    resource.setrlimit(resource.RLIMIT_AS, (4 * 1024**3, 4 * 1024**3))


def test_lifecycle_fuzz_fast_seeds(tmp_path: Path) -> None:
    runner = Path(__file__).with_name("lifecycle_fuzz.py")
    for seed in (1, 2, 3, 4):
        done = subprocess.run(
            [sys.executable, str(runner), "--seed", str(seed), "--steps", "16"],
            text=True, capture_output=True, check=False,
            timeout=600, preexec_fn=_limits, cwd=tmp_path,
        )
        assert done.returncode == 0, (
            f"seed {seed}:\n{done.stdout}\n{done.stderr}"
        )
        assert f"lifecycle fuzz passed; seed={seed}" in done.stdout
