"""Gated fast subset of the seeded lifecycle fuzz (#2043).

Seeds 1..4 at bounded steps; every wait is condition-based
(decode polling inside the runner), never a sleep-as-sync.
The long soak is opt-in by hand:
    python3 py/wrkslots/tests/lifecycle_fuzz.py --seed N --steps 60
"""

from __future__ import annotations

import os
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


def test_sigterm_sweeps_run_images(tmp_path: Path) -> None:
    """SIGTERM mid-run still detaches the run's images (#2048).

    Regression for the leaked loop devices: a killed harness
    deleted image files under live mounts. The runner's signal
    handler sweeps its own run directory before exiting, so no
    mount naming that directory survives.
    """
    import glob
    import signal
    import time

    runner = Path(__file__).with_name("lifecycle_fuzz.py")
    before = set(glob.glob("/tmp/wrkslots-fuzz-*"))
    env = dict(os.environ)
    # conftest defaults new projects to plain worktrees; this test
    # needs real image mounts to sweep.
    env["WRKSLOTS_INIT_REPRESENTATION"] = "image"
    proc = subprocess.Popen(
        [sys.executable, str(runner), "--seed", "2", "--steps", "60"],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        env=env,
    )
    run_dir: Path | None = None
    deadline = time.monotonic() + 480
    while time.monotonic() < deadline and run_dir is None:
        for candidate in set(glob.glob("/tmp/wrkslots-fuzz-*")) - before:
            if (Path(candidate) / "project" / "worktrees").is_dir():
                run_dir = Path(candidate)
                break
        if run_dir is None:
            time.sleep(0.2)
    assert run_dir is not None, "fuzz run never created its project"
    # Wait until a slot image is really mounted under the run
    # directory, so the sweep has something to detach.
    deadline = time.monotonic() + 480
    mounted = ""
    while time.monotonic() < deadline and str(run_dir) not in mounted:
        mounted = subprocess.run(
            ["findmnt", "-rn", "-o", "TARGET"], text=True,
            capture_output=True, check=False,
        ).stdout
        if str(run_dir) not in mounted:
            time.sleep(0.2)
    assert str(run_dir) in mounted, "no slot image ever mounted"
    proc.send_signal(signal.SIGTERM)
    proc.wait(timeout=240)
    assert proc.returncode not in (0, None)
    mounts = subprocess.run(
        ["findmnt", "-rn", "-o", "TARGET"], text=True,
        capture_output=True, check=False,
    ).stdout
    assert str(run_dir) not in mounts, f"mounts remain under {run_dir}"


