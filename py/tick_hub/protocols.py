"""Pluggable side-effect boundaries, so the tick engine core stays pure and testable.

The two things a tick does that touch the outside world — running a reminder's shell **gate** and
measuring a health check's **file age** — are expressed as small :class:`typing.Protocol` interfaces
here. The engine takes them as parameters; production uses the default implementations in
:mod:`tick_hub.probes`, and tests pass deterministic fakes. This mirrors how the sibling
safe-ci-dag-runner tool makes containment and metrics pluggable.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Optional, Protocol


@dataclass(frozen=True)
class GateResult:
    """The outcome of running a reminder's gate command."""

    #: Exit code of the command (``-1`` when it could not be launched at all).
    returncode: int
    #: Captured standard output.
    stdout: str
    #: True iff the command actually ran (False when it could not be launched).
    ok: bool
    #: Human-readable reason when ``ok`` is False; otherwise ``None``.
    error: Optional[str] = None


class GateRunner(Protocol):
    """Runs a reminder's gate command and reports its exit code + stdout."""

    def run(self, cmd: str) -> GateResult: ...


class FileAgeProbe(Protocol):
    """Reports the age (seconds) of the newest file matching a glob, relative to ``now``.

    Returns ``None`` when nothing matches (so the caller can report ``missing``)."""

    def newest_age_secs(self, pattern: str, now: int) -> Optional[int]: ...
