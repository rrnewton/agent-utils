"""Pluggable PR priority sources for landing order (lower number = more urgent).

Ordering among "land-now" PRs is priority -> size -> age. The priority itself is pluggable:

* ``none``   — every PR is priority 0 (order falls back to size then age); the default.
* ``labels`` — parse a priority integer from a PR label matching a caller pattern (e.g. ``p0`` or
  ``priority-2``). This is pure (reads :attr:`RawPr.labels`).
* a command hook (``beads``) — run a caller-supplied shell command per PR that prints an integer
  (e.g. a beads-priority lookup). This is the ONE impure priority source and lives behind the same
  protocol so the pure core never depends on it.
"""

from __future__ import annotations

import re
import subprocess
from collections.abc import Sequence
from typing import Protocol

DEFAULT_PRIORITY = 100
#: Default label pattern: ``p0``..``p9`` or ``priority-N`` / ``priority:N`` (case-insensitive).
DEFAULT_LABEL_PATTERN = r"^(?:p|priority[:-])(\d+)$"


class PriorityProvider(Protocol):
    """Resolve a landing priority for one PR (lower = more urgent)."""

    def priority(self, pr_number: int, labels: Sequence[str]) -> int: ...


class NonePriority:
    """Every PR is equal priority; ordering falls back to size then age."""

    def priority(self, pr_number: int, labels: Sequence[str]) -> int:
        return 0


class LabelPriority:
    """Derive priority from a label matching ``pattern`` (first capture group = the integer)."""

    def __init__(self, pattern: str = DEFAULT_LABEL_PATTERN, default: int = DEFAULT_PRIORITY) -> None:
        self._regex = re.compile(pattern, re.IGNORECASE)
        self._default = default

    def priority(self, pr_number: int, labels: Sequence[str]) -> int:
        best: int | None = None
        for label in labels:
            match = self._regex.match(label)
            if match is not None:
                value = int(match.group(1))
                best = value if best is None else min(best, value)
        return self._default if best is None else best


class CommandPriority:
    """Run ``cmd`` (with ``{pr}`` substituted) per PR; its stdout must be an integer priority.

    The impure priority source (a beads-priority hook). On any failure it falls back to ``default``
    and does NOT crash the plan — but the fallback is visible via :attr:`last_error` so a caller can
    surface it (No Silent Failure)."""

    def __init__(
        self,
        cmd: str,
        wrapper: Sequence[str] = (),
        default: int = DEFAULT_PRIORITY,
        timeout: int = 20,
    ) -> None:
        self._cmd = cmd
        self._wrapper = tuple(wrapper)
        self._default = default
        self._timeout = timeout
        self.last_error: str | None = None

    def priority(self, pr_number: int, labels: Sequence[str]) -> int:
        rendered = self._cmd.replace("{pr}", str(pr_number))
        argv = [*self._wrapper, "bash", "-c", rendered]
        try:
            proc = subprocess.run(
                argv, capture_output=True, text=True, timeout=self._timeout
            )
        except (OSError, subprocess.TimeoutExpired) as exc:
            self.last_error = f"priority command failed for #{pr_number}: {exc}"
            return self._default
        text = proc.stdout.strip()
        if proc.returncode != 0 or not text:
            self.last_error = (
                f"priority command for #{pr_number} exited {proc.returncode} / empty output"
            )
            return self._default
        try:
            return int(text.split()[0])
        except ValueError:
            self.last_error = f"priority command for #{pr_number} printed non-integer {text!r}"
            return self._default


def make_priority_provider(
    source: str,
    *,
    label_pattern: str = DEFAULT_LABEL_PATTERN,
    command: str = "",
    wrapper: Sequence[str] = (),
) -> PriorityProvider:
    """Build a :class:`PriorityProvider` from a ``--priority-source`` selection."""
    if source == "none":
        return NonePriority()
    if source == "labels":
        return LabelPriority(label_pattern)
    if source == "beads":
        if not command:
            # A beads source with no command hook degrades to label-derived priority, loudly.
            return LabelPriority(label_pattern)
        return CommandPriority(command, wrapper)
    raise ValueError(f"unknown priority source {source!r} (want none|labels|beads)")


__all__ = [
    "PriorityProvider",
    "NonePriority",
    "LabelPriority",
    "CommandPriority",
    "make_priority_provider",
    "DEFAULT_PRIORITY",
    "DEFAULT_LABEL_PATTERN",
]
