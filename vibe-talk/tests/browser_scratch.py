"""Per-run scratch for the browser walks, kept out of the host temp dir (`#2382 summary-link-test-host-tmp`).

A walk that starts the real server and Chromium writes a config, a store, a message cache,
Chromium's profile and Playwright's artifacts. With `TMPDIR` unset, `tempfile` and Playwright's
driver put all of that in the host's `/tmp`, and a run that dagrun stops at its timeout is
killed with `cgroup.kill` (SIGKILL), so no cleanup runs: one run left 747 MB there.

`run_scratch` gives a run one private directory under the vibe-talk build area
(`target/test-scratch/`), named with the run's PID, and points `TMPDIR` at its `tmp/` for the
run, so the server and Playwright's driver (and through it Chromium) keep their files there
too. The directory is removed when the run ends, passed or failed, including on SIGTERM. A
SIGKILL can still leave it behind, but only inside the worktree's build area, which goes with
the build output; the next run also removes such leftovers once their PID is gone and they
have stood untouched for an hour.

New walks: use `run_scratch` in place of `tempfile.TemporaryDirectory`, start the server
inside it, and wait for the server to exit before the block ends.
"""

from __future__ import annotations

import contextlib
import os
import shutil
import signal
import stat
import sys
import tempfile
import time
from collections.abc import Iterator
from pathlib import Path
from types import FrameType

ROOT = Path(__file__).resolve().parent.parent
SCRATCH_ROOT = ROOT / "target" / "test-scratch"
"""Where every run directory lives: inside the build area, never the host temp dir."""

STALE_SECONDS = 3600
"""A leftover is removed only after standing untouched this long (and its PID is gone)."""


def sweep_stale(prefix: str, *, root: Path = SCRATCH_ROOT, now: float | None = None) -> list[Path]:
    """Remove run directories under `root` named `prefix<PID>-...` that killed runs left
    behind: this user's, untouched for `STALE_SECONDS`, and whose PID is gone. A live run's
    directory is never touched, whatever its age."""
    moment = time.time() if now is None else now
    removed: list[Path] = []
    for path in sorted(root.glob(f"{prefix}*")):
        try:
            info = path.lstat()
        except OSError:
            continue
        if (
            not stat.S_ISDIR(info.st_mode)
            or info.st_uid != os.getuid()
            or moment - info.st_mtime < STALE_SECONDS
        ):
            continue
        owner = path.name[len(prefix):].split("-", 1)[0]
        if owner.isdigit() and Path("/proc", owner).exists():
            continue
        shutil.rmtree(path, ignore_errors=True)
        if not path.exists():
            removed.append(path)
    return removed


def _terminate(signum: int, _frame: FrameType | None) -> None:
    raise SystemExit(128 + signum)


@contextlib.contextmanager
def run_scratch(prefix: str, *, root: Path = SCRATCH_ROOT) -> Iterator[Path]:
    """One run's private directory under `root`, with `TMPDIR` pointing at its `tmp/` for the
    run; removed when the block ends, passed, failed or SIGTERMed."""
    root = root.absolute()  # the server runs in the run directory; TMPDIR must not be relative
    root.mkdir(parents=True, exist_ok=True)
    sweep_stale(prefix, root=root)
    work = Path(tempfile.mkdtemp(prefix=f"{prefix}{os.getpid()}-", dir=root))
    child_tmp = work / "tmp"
    child_tmp.mkdir()
    previous_tmpdir = os.environ.get("TMPDIR")
    os.environ["TMPDIR"] = str(child_tmp)
    previous_term = signal.signal(signal.SIGTERM, _terminate)
    try:
        yield work
    finally:
        signal.signal(signal.SIGTERM, previous_term)
        if previous_tmpdir is None:
            os.environ.pop("TMPDIR", None)
        else:
            os.environ["TMPDIR"] = previous_tmpdir
        shutil.rmtree(work, ignore_errors=True)
        if work.exists():
            print(f"browser scratch: could not remove {work}", file=sys.stderr, flush=True)
