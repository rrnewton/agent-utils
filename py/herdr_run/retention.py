"""Bounded retention for the run spool: delete run directories older than the retention window.

Two deliberate choices.

**Pruned ON WRITE, not on a timer.** A scheduled job that silently stops running is indistinguishable
from one that runs and finds nothing to do, and that inert-guard failure is exactly the class this
repository keeps rediscovering. Pruning as a side effect of creating a new run means retention can
only lapse if the tool itself stops being used -- in which case nothing is accumulating either.

**Scoped by construction, not by care.** This module DELETES DIRECTORIES, so the scope is enforced
rather than trusted: only entries whose real path's parent is exactly the resolved runs root are
considered, symlinks are skipped outright rather than followed, and the runs root itself is never
removed. Nothing above the capture root is reachable from here even if a caller passes a hostile
spool path.

The audit log is deliberately NOT pruned. It is the durable record of every attempt to cross the
sandbox boundary, it is small (one JSON line per invocation), and an evidence trail that quietly
deletes itself is worse than no policy at all.
"""

from __future__ import annotations

import os
import shutil
import time
from collections.abc import Callable
from dataclasses import dataclass

__all__ = ["RETENTION_DAYS", "PruneResult", "prune_runs", "runs_root"]

#: How long a run's captured output is kept. Four days spans a long weekend, so a failure on Friday
#: is still inspectable on Monday.
RETENTION_DAYS = 4

_SECONDS_PER_DAY = 86_400


@dataclass(frozen=True)
class PruneResult:
    """What a prune pass did, so the caller can log or assert on it rather than infer."""

    removed: tuple[str, ...]
    kept: int
    skipped: tuple[str, ...]

    @property
    def removed_count(self) -> int:
        """How many run directories this pass removed."""
        return len(self.removed)


def runs_root(spool_dir: str, project_root: str) -> str:
    """Absolute path of the directory holding per-run spool directories."""
    root = spool_dir if os.path.isabs(spool_dir) else os.path.join(project_root, spool_dir)
    return os.path.join(root, "runs")


def prune_runs(
    root: str,
    *,
    retention_days: int = RETENTION_DAYS,
    now: Callable[[], float] = time.time,
) -> PruneResult:
    """Remove run directories under ``root`` whose mtime is older than the retention window.

    Never raises: retention is housekeeping, and a failure to tidy must not fail the run that
    triggered it. Returns what happened so a caller can assert on both halves -- what was removed
    AND what was kept, because a prune that removes everything also passes a removal test.
    """
    removed: list[str] = []
    skipped: list[str] = []
    kept = 0

    try:
        real_root = os.path.realpath(root)
    except OSError:
        return PruneResult((), 0, ())
    if not os.path.isdir(real_root):
        return PruneResult((), 0, ())

    cutoff = now() - retention_days * _SECONDS_PER_DAY

    try:
        entries = sorted(os.listdir(real_root))
    except OSError:
        return PruneResult((), 0, ())

    for name in entries:
        path = os.path.join(real_root, name)

        # A symlink is never followed and never removed: following one would let a link planted in
        # the spool reach arbitrary paths, which is the whole risk this module has to not have.
        if os.path.islink(path):
            skipped.append(path)
            continue
        if not os.path.isdir(path):
            skipped.append(path)
            continue

        # Containment, checked rather than assumed: the entry's real parent must be the real root.
        try:
            if os.path.dirname(os.path.realpath(path)) != real_root:
                skipped.append(path)
                continue
            mtime = os.stat(path).st_mtime
        except OSError:
            skipped.append(path)
            continue

        if mtime >= cutoff:
            kept += 1
            continue

        try:
            shutil.rmtree(path)
        except OSError:
            skipped.append(path)
            continue
        removed.append(path)

    return PruneResult(tuple(removed), kept, tuple(skipped))
