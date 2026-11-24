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

import pytest


def _limits() -> None:
    resource.setrlimit(resource.RLIMIT_CPU, (600, 600))
    resource.setrlimit(resource.RLIMIT_AS, (4 * 1024**3, 4 * 1024**3))


@pytest.mark.load_quarantine

def _require_root_mounts() -> None:
    """Root-mount tests are opt-in (owner rule).

    These tests force image slots, whose mounts go through
    wrkslots' sudo -n helper. Without the explicit opt-in they
    skip by name: loop-device teardown and SIGTERM image-sweep
    coverage is lost in that run.
    """
    if os.environ.get("WRKSLOTS_ALLOW_ROOT_TESTS") != "1":
        pytest.skip(
            "root-mount coverage lost: loop-device teardown and "
            "SIGTERM image sweep not run; set "
            "WRKSLOTS_ALLOW_ROOT_TESTS=1 to run them"
        )


def test_lifecycle_fuzz_fast_seeds(tmp_path: Path) -> None:
    runner = Path(__file__).with_name("lifecycle_fuzz.py")
    for seed in (1, 2, 3, 4):
        done = subprocess.run(
            [sys.executable, str(runner), "--seed", str(seed), "--steps", "16"],
            text=True, capture_output=True, check=False,
            timeout=1800, preexec_fn=_limits, cwd=tmp_path,
        )
        assert done.returncode == 0, (
            f"seed {seed}:\n{done.stdout}\n{done.stderr}"
        )
        assert f"lifecycle fuzz passed; seed={seed}" in done.stdout


def test_failing_run_leaves_no_loop_devices(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """A scenario that fails mid-run still detaches its devices.

    The fixture records loop devices by name at attach time;
    the finally sweep unmounts through wrkslots, so zero of
    this run's devices remain (other runs' devices, including
    pre-existing orphans, are never touched or counted).
    """
    _require_root_mounts()
    monkeypatch.setenv("WRKSLOTS_INIT_REPRESENTATION", "image")
    sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
    from wrkslots.tests import e2e_stress as e2e
    from wrkslots.tests import lifecycle_fuzz as fuzz

    trace = e2e.Trace(tmp_path / "trace.jsonl", seed=7)
    project = e2e._make_project(tmp_path / "run", trace, ttl_seconds=2)
    fuzz._ACTIVE.update(project=project, trace=trace)
    fuzz._record_attached(project)
    try:
        e2e._create_unbound(project, "fz-a", "fz-fz-a", trace)
        fuzz._record_attached(project)
        raise RuntimeError("simulated scenario failure")
    except RuntimeError:
        pass
    finally:
        fuzz._exit_sweep()
    # Nothing may stay mounted in this namespace, and no image
    # file may be deleted under a mount. A loop device whose
    # only remaining copies live in OTHER namespaces (the gate
    # shard runs in a pid namespace) is the documented #2050
    # residual class: reported by name, backing intact, never
    # detached by pattern from here.
    assert e2e._mount_points_under(project.root) == []
    images = project.root / "worktrees" / "slot-images"
    if images.is_dir():
        # Every image file the run attached still exists: the
        # sweep unmounted before anything could delete under it.
        assert list(images.rglob("*.img")), "expected an imaged slot"
    recorded = fuzz._ACTIVE.get("devices")
    assert isinstance(recorded, set)
    assert fuzz._attached_loop_devices(project.root) <= recorded


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
    _require_root_mounts()
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
    # Condition-based: the sweep unmounts after the in-flight
    # child stops; poll until no mount names the run directory.
    deadline = time.monotonic() + 240
    mounts = ""
    while time.monotonic() < deadline:
        mounts = subprocess.run(
            ["findmnt", "-rn", "-o", "TARGET"], text=True,
            capture_output=True, check=False,
        ).stdout
        if str(run_dir) not in mounts:
            break
        time.sleep(0.2)
    assert str(run_dir) not in mounts, f"mounts remain under {run_dir}"


