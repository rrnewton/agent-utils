"""Reap-on-teardown: clean up abandoned prior runs' scopes before starting a new one.

Abandonment is the COMMON exit path here, not the exception — an agent recycle, the 120s tool
cap killing a run mid-flight, or a detached run outliving its launcher all SIGKILL the launcher
before any ``finally`` block or SIGINT/SIGTERM handler can run. When that happens the transient
``parallel-experiment-<pid>.scope`` stays ``active running`` with its descendants still inside it
(e.g. ``tracing-appender`` workers parked in ``__futex_wait``, each pinning a PID namespace so its
zombies are never reaped). Those descendants consume ZERO CPU and ZERO MEMORY, so a box that
validated only the CPU and memory axes would report PERFECT HEALTH while the leak accumulated —
measured at 415 leaked threads across 8 abandoned runs. ``systemd-run --collect`` does not GC a
scope that still has live members, so nothing upstream cleans this up either.

The fix is next-run-cleans-previous: on every fresh (outer) launch, before this process establishes
its own scope, enumerate sibling ``parallel-experiment-*.scope`` units, and for each whose launcher
pid is dead, ``cgroup.kill`` + stop the scope — naming the unit and how many tasks it reaped, so an
abandoned leak is never silent. Reaping is deliberately conservative: a scope whose launcher pid is
still alive is left untouched (a live run owns it), so the only failure mode is a missed reap under
pid reuse, never a false kill of a live run. The planner is pure over sampled inputs; the one impure
helper (:func:`reap_orphaned_runs`) is isolated and reuses the shared ``safe_ci_dag_runner.cgroup``
teardown primitives rather than reimplementing the kill.
"""

from __future__ import annotations

import os
import re
from collections.abc import Callable
from dataclasses import dataclass
from pathlib import Path

from safe_ci_dag_runner.cgroup import ScopeNaming

Emit = Callable[[str], None]


@dataclass(frozen=True)
class ScopeInfo:
    """A sampled sibling scope: its unit, the launcher pid encoded in its name, and how many
    tasks (threads) still live in its cgroup subtree. ``launcher_pid``/``task_count`` are ``None``
    when they could not be parsed/read (treated as unknown, not zero)."""

    unit: str
    launcher_pid: int | None
    task_count: int | None


@dataclass(frozen=True)
class ReapPlan:
    """A decision to reap one abandoned scope, carrying enough to name what and how much."""

    unit: str
    launcher_pid: int | None
    task_count: int | None
    reason: str


def _unit_pid_re(naming: ScopeNaming) -> re.Pattern[str]:
    """``parallel-experiment-<pid>.scope`` -> capture ``<pid>`` (prefix is naming-specific)."""
    return re.compile(rf"^{re.escape(naming.unit_prefix)}-(\d+)\.scope$")


def parse_launcher_pid(unit: str, naming: ScopeNaming) -> int | None:
    """Extract the launcher pid the scope unit name was minted from, or ``None`` if it does not
    match this naming's ``<prefix>-<pid>.scope`` shape."""
    m = _unit_pid_re(naming).match(unit.strip())
    return int(m.group(1)) if m else None


def plan_reaps(
    scopes: list[ScopeInfo],
    *,
    self_pid: int,
    pid_alive: Callable[[int], bool],
) -> list[ReapPlan]:
    """PURE: decide which sampled scopes are abandoned and must be reaped.

    A scope is abandoned when its launcher pid is known, is not *this* process, and is no longer
    alive. Everything else is left alone: a live launcher owns its scope, and a scope whose pid we
    could not parse is not ours to touch. This never kills a live run — the worst case under pid
    reuse is a missed reap, which the next launch retries.
    """
    plans: list[ReapPlan] = []
    for s in scopes:
        pid = s.launcher_pid
        if pid is None or pid == self_pid:
            continue
        if pid_alive(pid):
            continue
        count = "unknown number of" if s.task_count is None else str(s.task_count)
        plans.append(
            ReapPlan(
                unit=s.unit,
                launcher_pid=pid,
                task_count=s.task_count,
                reason=(
                    f"launcher pid {pid} is dead but the scope is still active with {count} "
                    "leaked task(s)/thread(s)"
                ),
            )
        )
    return plans


def _pid_alive(pid: int) -> bool:
    return Path(f"/proc/{pid}").exists()


def _subtree_task_count(scope_cg: Path) -> int | None:
    """Sum ``pids.current`` over the scope cgroup and every descendant cgroup.

    ``pids.current`` is per-node (not recursive) in cgroup v2, and leaked threads may sit in the
    scope root or in leftover per-worker child cgroups, so we walk the subtree. Returns ``None`` if
    nothing could be read (unknown, reported honestly rather than as a misleading 0)."""
    total = 0
    read_any = False
    try:
        nodes = [scope_cg, *(p.parent for p in scope_cg.rglob("pids.current"))]
    except OSError:
        return None
    for node in {scope_cg, *nodes}:
        try:
            total += int((node / "pids.current").read_text().strip())
            read_any = True
        except (OSError, ValueError):
            continue
    return total if read_any else None


def _list_scope_units(naming: ScopeNaming) -> list[str]:
    """Enumerate transient ``<prefix>-*.scope`` --user units via systemctl (outer process only)."""
    import subprocess

    try:
        proc = subprocess.run(
            [
                "systemctl", "--user", "list-units", "--all", "--plain",
                "--no-legend", "--type=scope", f"{naming.unit_prefix}-*.scope",
            ],
            capture_output=True,
            text=True,
            timeout=15,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return []
    units: list[str] = []
    for line in proc.stdout.splitlines():
        line = line.strip()
        if not line:
            continue
        unit = line.split(None, 1)[0]
        if unit.endswith(".scope"):
            units.append(unit)
    return units


def sample_scopes(naming: ScopeNaming) -> list[ScopeInfo]:
    """IMPURE: enumerate sibling scopes and, for each, sample the launcher pid + subtree task
    count. Isolated so :func:`plan_reaps` stays pure and directly unit-testable."""
    from safe_ci_dag_runner.cgroup import _scope_cgroup_path

    infos: list[ScopeInfo] = []
    for unit in _list_scope_units(naming):
        pid = parse_launcher_pid(unit, naming)
        scope_cg = _scope_cgroup_path(unit)
        count = _subtree_task_count(scope_cg) if scope_cg is not None else None
        infos.append(ScopeInfo(unit=unit, launcher_pid=pid, task_count=count))
    return infos


def reap_orphaned_runs(naming: ScopeNaming, *, emit: Emit) -> list[ReapPlan]:
    """IMPURE: reap every abandoned prior-run scope, naming each reap. Returns the plans acted on.

    Called on the OUTER (pre-re-exec) launch, before this process mints its own scope, so a fresh
    run always cleans up the wreckage of abandoned predecessors — the mechanism that stops 8
    abandoned runs from accumulating 415 leaked threads. Best-effort: a scope we fail to stop is
    reported, not swallowed, and the next launch retries it.
    """
    from safe_ci_dag_runner.cgroup import _scope_cgroup_path, stop_scope

    scopes = sample_scopes(naming)
    plans = plan_reaps(scopes, self_pid=os.getpid(), pid_alive=_pid_alive)
    for plan in plans:
        scope_cg = _scope_cgroup_path(plan.unit)
        ok = stop_scope(plan.unit, scope_cg, naming=naming)
        count = "an unknown number of" if plan.task_count is None else str(plan.task_count)
        verb = "REAPED" if ok else "FAILED to reap"
        emit(
            f"{naming.log_prefix} {verb} abandoned run {plan.unit} "
            f"(launcher pid {plan.launcher_pid} dead): killed {count} leaked task(s)/thread(s) "
            "that were pinning the scope"
        )
    return plans
