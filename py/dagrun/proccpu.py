"""Boxing-independent CPU-time accounting for a step's process group, read from procfs.

WHY THIS EXISTS
---------------
The per-step ``cpu_timeout`` budget is normally enforced from the step's cgroup
``cpu.stat`` ``usage_usec``, which is exact and kernel-accounted. But
``CgroupManager.cpu_stats`` returns ``None`` whenever boxing is not established, and
the scheduler's guard is then simply skipped — the budget is declared and enforces
nothing.

That is not an exotic corner. A caller that passes ``--allow-cgroup-failure`` (which
an originating CI wrapper does unconditionally under ``GITHUB_ACTIONS``/``CI``) runs
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
``RLIMIT_CPU`` in the first place, and the one that matters for parallel command fan-out.

Known gaps, all strictly narrower than "no enforcement at all":

* A descendant that calls ``setpgid`` or ``setsid`` leaves the process group and stops
  being counted. Nonce-aware teardown may still find and kill it, so accounting is weaker
  than teardown on this edge.
* CPU burned by an exited descendant is invisible before its parent reaps it.
* Sampling is at the monitor interval and live snapshots are cached for up to half a
  second, so overshoot up to one tick plus that cache window is expected.

Accuracy is therefore a lower bound on true CPU use: weaker than the cgroup reading,
but materially more informative than no measurement. The scheduler names the source in its
operator-facing diagnostic whenever this fallback triggers.
"""

from __future__ import annotations

import os
import threading
import time
from pathlib import Path

__all__ = ["CPU_SOURCE_CGROUP", "CPU_SOURCE_PROCFS", "subtree_cpu_seconds"]

# Stable identifiers for the accounting that produced a CPU reading. The scheduler uses
# these in its operator-facing timeout diagnostic so the active mechanism is explicit.
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


def _scan_group_ticks(root: Path) -> dict[int, int] | None:
    """Read one procfs snapshot and aggregate CPU ticks by process group."""
    try:
        entries = list(root.iterdir())
    except OSError:
        return None

    groups: dict[int, int] = {}
    for entry in entries:
        name = entry.name
        if not name.isdigit():
            continue
        fields = _read_stat_fields(entry)
        if fields is None:
            continue
        try:
            pgrp = int(fields[_F_PGRP])
            ticks = (
                int(fields[_F_UTIME])
                + int(fields[_F_STIME])
                + int(fields[_F_CUTIME])
                + int(fields[_F_CSTIME])
            )
        except (ValueError, IndexError):
            continue
        groups[pgrp] = groups.get(pgrp, 0) + ticks
    return groups


# All active uncontained steps share one short-lived procfs snapshot. Without this cache, N
# concurrent steps each scan every process once per second: O(N * host processes) monitor work.
# Half a second is below the scheduler's one-second polling interval, so peers waking in the same
# tick share a scan without extending the documented sampling granularity.
_SNAPSHOT_TTL_S = 0.5
_snapshot_lock = threading.Lock()
_snapshot_at = float("-inf")
_snapshot_groups: dict[int, int] | None = None


def _default_snapshot() -> dict[int, int] | None:
    global _snapshot_at, _snapshot_groups
    now = time.monotonic()
    with _snapshot_lock:
        if _snapshot_groups is not None and now - _snapshot_at <= _SNAPSHOT_TTL_S:
            return _snapshot_groups
        groups = _scan_group_ticks(_PROC)
        _snapshot_at = time.monotonic()
        _snapshot_groups = groups
        return groups


def subtree_cpu_seconds(pgid: int, *, proc_root: Path | None = None) -> float | None:
    """Lower bound of CPU-seconds observed for process group ``pgid``.

    ``None`` means the procfs snapshot was unreadable or contained no readable member of the
    target group. ``0.0`` therefore means a real readable member was observed with zero ticks;
    missing evidence is never fabricated as a zero reading. Custom roots bypass the shared live
    cache so tests and callers receive an exact snapshot of the supplied tree.
    """
    if pgid <= 1:
        return None
    groups = _default_snapshot() if proc_root is None else _scan_group_ticks(proc_root)
    if groups is None:
        return None
    ticks = groups.get(pgid)
    if ticks is None:
        return None
    return ticks / _CLK_TCK
