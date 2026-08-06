"""Boxing-independent CPU-time accounting for a step's process group, read from procfs.

WHY THIS EXISTS
---------------
The per-step ``cpu_timeout`` budget is normally enforced from the step's cgroup
``cpu.stat`` ``usage_usec``, which is exact and kernel-accounted. But
``CgroupManager.cpu_stats`` returns ``None`` whenever boxing is not established, and
the scheduler's guard is then simply skipped — the budget is declared and enforces
nothing.

That is not an exotic corner. A caller that passes ``--allow-cgroup-failure`` (which
hermit's ``ci/run-node.sh`` does unconditionally under ``GITHUB_ACTIONS``/``CI``) runs
UNBOXED by construction, so on that lane every ``cpu_timeout`` was inert: measured, a
step with ``cpu_timeout: 3`` burned 60 CPU-seconds and exited green.

The 2026-08-03 decision that chose cgroup polling over ``RLIMIT_CPU`` discounted
rlimit's "works unboxed" advantage with *"boxing is default-on, unboxed = opted out of
enforcement."* That premise does not hold for a lane where unboxed is the norm rather
than an opt-out, so the fallback below restores a bound there — WITHOUT displacing the
cgroup reading, which remains the primary and is strictly better where available.

WHAT IT MEASURES, AND WHAT IT MISSES
------------------------------------
The scheduler starts each step with ``start_new_session=True``, so the step and its
descendants share one process group whose pgid equals the step leader's pid. This sums,
over every live member of that group:

    utime + stime            CPU burned by that process itself
  + cutime + cstime          CPU of descendants it has already REAPED

``cutime``/``cstime`` roll up recursively on ``wait``, and a reaped process is by
definition no longer in the live set, so the two terms do not double-count. That makes
this an AGGREGATE measure — the property that made cgroup polling win over per-process
``RLIMIT_CPU`` in the first place, and the one that matters for ``make -jN`` / ``cargo
test`` style fan-out.

Known gaps, all strictly narrower than "no enforcement at all":

* A descendant that calls ``setsid`` leaves the process group and stops being counted.
  The unboxed teardown path (``killpg``) already has exactly this blind spot, so the
  fallback is no weaker than the reaping it triggers.
* CPU burned by a descendant that exited but has NOT yet been waited for by its parent
  is invisible until the parent reaps it.
* Sampling is at the monitor interval, so overshoot up to one tick is expected — the
  same granularity caveat the cgroup path already carries.

Accuracy is therefore "no worse than the cgroup reading, and unboundedly better than
the ``None`` it replaces". Callers must keep reporting WHICH source produced a breach
so a reader is never left guessing which of the two accountings killed a step.
"""

from __future__ import annotations

import os
from pathlib import Path

__all__ = ["CPU_SOURCE_CGROUP", "CPU_SOURCE_PROCFS", "subtree_cpu_seconds"]

# Stable identifiers for the accounting that produced a CPU reading. These are reported
# alongside a breach so the evidence carries its own condition rather than leaving the
# reader to infer which mechanism was live.
CPU_SOURCE_CGROUP = "cgroup"
CPU_SOURCE_PROCFS = "procfs-subtree"

_PROC = Path("/proc")

# /proc/<pid>/stat field indices, 0-based, AFTER the comm field has been split off.
# The raw layout is `pid (comm) state ppid pgrp ...`; comm can contain spaces and
# parentheses, so it must be removed by taking the text after the LAST ')' before any
# field splitting. Relative to that remainder, field 0 is `state`:
#   state ppid pgrp session tty_nr tpgid flags minflt cminflt majflt cmajflt
#     0     1    2      3      4      5     6     7       8       9      10
#   utime stime cutime cstime
#    11    12     13     14
_F_PGRP = 2
_F_UTIME = 11
_F_STIME = 12
_F_CUTIME = 13
_F_CSTIME = 14

try:
    _CLK_TCK = os.sysconf("SC_CLK_TCK") or 100
except (ValueError, OSError):  # pragma: no cover - every Linux defines SC_CLK_TCK
    _CLK_TCK = 100


def _read_stat_fields(pid_dir: Path) -> list[str] | None:
    """Return the whitespace-split tail of ``/proc/<pid>/stat`` after the comm field.

    Returns ``None`` for any process that disappeared mid-scan or whose stat line is
    malformed. A racing exit is the normal case, not an error: the caller simply does
    not count a process it cannot read.
    """
    try:
        raw = (pid_dir / "stat").read_text()
    except (OSError, UnicodeDecodeError):
        return None
    close = raw.rfind(")")
    if close == -1:
        return None
    fields = raw[close + 1 :].split()
    if len(fields) <= _F_CSTIME:
        return None
    return fields


def subtree_cpu_seconds(pgid: int, *, proc_root: Path | None = None) -> float | None:
    """Total CPU-seconds consumed by process group ``pgid`` and its reaped descendants.

    Returns ``None`` when procfs is unreadable (so a caller can distinguish "cannot
    measure" from a genuine 0.0), and ``0.0`` when the group exists but has burned no
    measurable CPU yet. ``pgid <= 1`` is refused outright: pgid 0 would mean "the
    caller's own group" and pgid 1 is init, and mistaking either for a step's group
    would attribute unrelated CPU to the step and reap it spuriously.
    """
    if pgid <= 1:
        return None
    root = proc_root if proc_root is not None else _PROC
    try:
        entries = list(root.iterdir())
    except OSError:
        return None

    ticks = 0
    seen = False
    for entry in entries:
        name = entry.name
        if not name.isdigit():
            continue
        fields = _read_stat_fields(entry)
        if fields is None:
            continue
        try:
            if int(fields[_F_PGRP]) != pgid:
                continue
            # Own CPU, plus the CPU of descendants this process has already waited for.
            # cutime/cstime accumulate recursively and only for REAPED children, which
            # are absent from the live scan, so the two terms are disjoint.
            ticks += (
                int(fields[_F_UTIME])
                + int(fields[_F_STIME])
                + int(fields[_F_CUTIME])
                + int(fields[_F_CSTIME])
            )
        except (ValueError, IndexError):
            continue
        seen = True

    if not seen:
        # The group has fully exited (or never existed). Report 0.0 rather than None:
        # "nothing left running" is a measurement, not a measurement failure, and
        # returning None here would look like procfs was unavailable.
        return 0.0
    return ticks / _CLK_TCK
