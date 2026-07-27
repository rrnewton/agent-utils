"""Default, production implementations of the :mod:`tick_hub.protocols` boundaries.

:class:`SubprocessGateRunner` runs a reminder's gate via ``bash -c`` with a timeout;
:class:`GlobFileAgeProbe` measures the newest matching file's mtime against the tick's clock. Both
are deliberately side-effectful (subprocess / filesystem) and therefore live outside the pure engine
so tests can swap in deterministic fakes.
"""

from __future__ import annotations

import glob
import os
import subprocess
import time
from typing import Optional

from tick_hub.protocols import GateResult

DEFAULT_GATE_TIMEOUT_SECS = 30


class SubprocessGateRunner:
    """Run a gate command with ``bash -c`` under a timeout, capturing stdout.

    A command that cannot be launched, times out, or otherwise fails to execute returns
    ``ok=False`` with a reason (No Silent Failure): the engine surfaces that as an ``ERROR:`` line
    rather than pretending the gate passed or failed."""

    def __init__(self, timeout: int = DEFAULT_GATE_TIMEOUT_SECS) -> None:
        self.timeout = timeout

    def run(self, cmd: str) -> GateResult:
        try:
            proc = subprocess.run(
                ["bash", "-c", cmd],
                capture_output=True,
                text=True,
                timeout=self.timeout,
            )
        except subprocess.TimeoutExpired:
            return GateResult(
                returncode=-1, stdout="", ok=False, error=f"timed out after {self.timeout}s"
            )
        except OSError as exc:
            return GateResult(returncode=-1, stdout="", ok=False, error=str(exc))
        return GateResult(returncode=proc.returncode, stdout=proc.stdout, ok=True)


class GlobFileAgeProbe:
    """Report the age of the newest file matching a glob, relative to ``now``."""

    def newest_age_secs(self, pattern: str, now: int) -> Optional[int]:
        matches = glob.glob(pattern)
        if not matches:
            return None
        try:
            newest_mtime = max(os.path.getmtime(p) for p in matches)
        except OSError:
            return None
        return max(0, int(now - newest_mtime))


def wall_clock_now() -> int:
    """The real wall-clock epoch (seconds), the tick's default ``now``."""
    return int(time.time())
