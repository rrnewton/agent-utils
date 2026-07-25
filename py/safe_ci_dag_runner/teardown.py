"""Fast, zombie-free process/scope teardown for the DAG runner (Linux cgroup-v2).

Three distinct teardown surfaces, all ported from DeepScry's validate harness but carrying
zero DeepScry/MTG specifics (concrete names, scope prefixes, and liveness predicates are
supplied by the caller):

* :func:`reap` — tear down ONE step's whole process tree. Ported from
  ``scripts/validate.py`` ``Runner._reap`` (~L1768-1788). Writes the step cgroup's
  ``cgroup.kill`` FIRST (atomic SIGKILL of the entire subtree, including ``setsid`` /
  double-fork escapees that changed session/pgid but not cgroup membership), then falls
  back to ``killpg`` as a belt-and-suspenders for the no-cgroup path.
* :func:`install_scope_teardown` — a SIGINT/SIGTERM handler that tears down the OUTER scope
  (the whole run's cgroup) so an aborted run leaves no orphans. Ported from
  ``scripts/validate.py`` ``_install_scope_teardown`` (~L3134-3191) plus
  ``scripts/validate_cgroup.py`` ``stop_scope`` / ``kill_scope_cgroup`` (~L582-640,
  ~L827-862).
* :func:`reap_external` (and its parts :func:`reap_processes_by_cwd`,
  :func:`stop_leftover_scopes`) — the external "reap my leftover scopes/PIDs by cwd"
  reaper, ported from the whole of ``scripts/kill_zombie_processes.py`` but parameterized
  by ``(cwd, patterns)`` and caller-supplied liveness/protection predicates instead of
  hardcoded ``deepscry`` / ``validate-`` / ``supervisor`` names.

No Silent Failure (generic strengthening of the originals): where DeepScry silently
``pass``-es on a failed ``cgroup.kill`` write and thereby degrades teardown to killpg-only
(which misses ``setsid`` escapees) without a word, this module emits a visible degraded-
enforcement WARNING on stderr and continues. See the per-function docstrings and the
report accompanying this port for the exact spots changed.
"""

from __future__ import annotations

import os
import shutil
import signal
import subprocess
import sys
import time
from collections.abc import Callable, Sequence
from dataclasses import dataclass
from pathlib import Path
from types import FrameType
from typing import Protocol, runtime_checkable

from safe_ci_dag_runner.protocols import CgroupManager

__all__ = [
    "CGROUP_ROOT",
    "ProcessGroupLeader",
    "ReapSummary",
    "reap",
    "install_scope_teardown",
    "kill_cgroup",
    "stop_systemd_scope",
    "current_cgroup_path",
    "outer_scope_cgroup",
    "reap_external",
    "reap_processes_by_cwd",
    "stop_leftover_scopes",
]

#: cgroup-v2 unified hierarchy mount point. Linux-only, as DeepScry targets.
CGROUP_ROOT = Path("/sys/fs/cgroup")


def _warn(message: str) -> None:
    """Emit a visible degraded-enforcement warning (No Silent Failure)."""
    print(f"[teardown] ⚠ {message}", file=sys.stderr)


@runtime_checkable
class ProcessGroupLeader(Protocol):
    """The one thing :func:`reap` needs from a launched step: its process-group id.

    A :class:`subprocess.Popen` satisfies this structurally. The caller MUST have started
    the process with ``start_new_session=True`` so the leader's ``pid`` equals the
    process-group id (``pgid``); that group id stays valid for ``killpg`` while ANY member
    is alive, even after the leader itself has been ``wait()``-reaped (which is why we read
    the stored ``pid`` rather than calling ``os.getpgid`` at reap time — the DeepScry
    ``Runner`` captures the pgid right after ``Popen`` for exactly this reason).
    """

    pid: int


# --------------------------------------------------------------------------------------
# Single-step teardown  (ports validate.py Runner._reap)
# --------------------------------------------------------------------------------------


def reap(
    process: ProcessGroupLeader,
    cgroups: CgroupManager | None,
    tag: str | None,
) -> None:
    """Tear down one step's whole process tree: ``cgroup.kill`` first, then ``killpg``.

    When per-step containment is available, writing the step's child ``cgroup.kill``
    SIGKILLs the ENTIRE subtree atomically, including ``setsid`` / double-fork escapees a
    process-group kill misses (an escapee changes session/pgid but not cgroup membership).
    The ``killpg`` that follows is a belt-and-suspenders for the no-cgroup path and is
    harmless once the cgroup already cleared the group.

    ``process.pid`` is used as the pgid (valid because the caller started the step with
    ``start_new_session=True``); the guard refuses ``pgid <= 1`` and the runner's OWN
    process group so a reap can never turn into suicide (the historical DeepScry exit-144).

    No Silent Failure: DeepScry's ``StepCgroups.kill`` returns ``False`` on a failed
    ``cgroup.kill`` write and the caller ignored it, silently degrading to killpg-only.
    Here, when containment is ENABLED but the kill write fails, we surface a warning so the
    operator knows ``setsid`` escapees may have survived; we still run the killpg backstop.
    """
    if cgroups is not None and tag is not None and cgroups.enabled:
        if not cgroups.kill(tag):
            _warn(
                f"cgroup.kill for step {tag!r} failed; falling back to process-group "
                "kill only — setsid/double-fork escapees may survive."
            )

    pgid = process.pid
    if pgid <= 1 or pgid == os.getpgrp():
        return
    try:
        os.killpg(pgid, signal.SIGKILL)
    except (ProcessLookupError, OSError):
        pass  # whole group already gone — an expected, benign race, not a degraded skip


# --------------------------------------------------------------------------------------
# Outer-scope cgroup helpers  (port validate_cgroup.kill_scope_cgroup / stop_scope)
# --------------------------------------------------------------------------------------


def kill_cgroup(cgroup: Path) -> bool:
    """Atomically SIGKILL a cgroup and every descendant via its ``cgroup.kill`` file.

    Ports ``validate_cgroup.kill_scope_cgroup``. Returns ``True`` when the write landed,
    ``False`` on ``OSError`` (e.g. the file is gone / not writable). Best-effort; never
    raises — the caller decides whether a ``False`` here is worth a warning.
    """
    try:
        (cgroup / "cgroup.kill").write_text("1")
        return True
    except OSError:
        return False


def stop_systemd_scope(unit: str, scope_cgroup: Path | None = None, *, timeout: int = 15) -> bool:
    """Tear down a whole ``systemd --user`` transient scope: ``cgroup.kill`` then stop.

    Ports ``validate_cgroup.stop_scope``. Two steps for speed AND cleanliness:

    1. ``cgroup.kill`` the scope's cgroup directly — an INSTANT atomic SIGKILL of every
       member, including processes (e.g. a browser) that IGNORE the SIGTERM that
       ``systemctl stop`` would send first and would otherwise stall the stop in
       ``stop-sigterm`` for the unit's full ``TimeoutStopSec``.
    2. ``systemctl --user stop`` to deactivate and garbage-collect the now-empty unit.

    Pass ``scope_cgroup`` (resolved ahead of time via :func:`outer_scope_cgroup`) to skip
    the ``systemctl show`` lookup for step 1 — important inside a signal handler, where
    shelling out under load can stall. Returns ``True`` iff the ``systemctl stop`` call
    completed. Best-effort throughout; never raises.
    """
    if not unit:
        return False
    cg = scope_cgroup if scope_cgroup is not None else _systemd_scope_cgroup(unit)
    if cg is not None:
        kill_cgroup(cg)
    try:
        subprocess.run(
            ["systemctl", "--user", "stop", unit],
            capture_output=True,
            timeout=timeout,
        )
        return True
    except (subprocess.TimeoutExpired, OSError):
        return False


def _systemd_scope_cgroup(unit: str) -> Path | None:
    """Filesystem cgroup path of a ``systemd --user`` unit, via ``systemctl show``.

    Ports ``validate_cgroup._scope_cgroup_path`` / ``kill_zombie_processes._scope_cgroup_path``.
    """
    try:
        r = subprocess.run(
            ["systemctl", "--user", "show", unit, "--property=ControlGroup", "--value"],
            capture_output=True,
            text=True,
            timeout=8,
        )
    except (subprocess.TimeoutExpired, OSError):
        return None
    cg = r.stdout.strip()
    return (CGROUP_ROOT / cg.lstrip("/")) if cg else None


def current_cgroup_path() -> Path | None:
    """This process's own cgroup-v2 path from ``/proc/self/cgroup`` (unified line ``0::``).

    Ports the ``/proc/<pid>/cgroup`` parse in ``kill_zombie_processes._process_cgroup_path``
    for ``self``. Reads with no ``systemctl`` shell-out, so it is safe to call before
    installing a signal handler (which must not shell out under load).
    """
    try:
        for line in Path("/proc/self/cgroup").read_text().splitlines():
            hierarchy, controllers, relative = line.split(":", 2)
            if hierarchy == "0" and controllers == "":
                return CGROUP_ROOT / relative.lstrip("/")
    except (OSError, ValueError):
        pass
    return None


def outer_scope_cgroup(*, supervisor_name: str = "supervisor") -> Path | None:
    """Resolve the OUTER run scope's cgroup from this process's own cgroup, no shell-out.

    Generic port of ``validate_cgroup.scope_cgroup_from_self``. When the runner lives in a
    ``<scope>/<supervisor_name>`` child, the scope is the parent; if this process is already
    at a ``*.scope`` leaf, that leaf IS the scope. Returns ``None`` when not inside a scope
    (the caller then knows there is no outer scope to tear down — a visible ``None``, not a
    silent skip).
    """
    mine = current_cgroup_path()
    if mine is None:
        return None
    if mine.name == supervisor_name:
        return mine.parent
    if mine.name.endswith(".scope"):
        return mine
    return None


# --------------------------------------------------------------------------------------
# Signal-handler installer  (ports validate.py _install_scope_teardown)
# --------------------------------------------------------------------------------------


def install_scope_teardown(
    *,
    scope_cgroup: Path | None = None,
    systemd_unit: str | None = None,
    on_teardown: Callable[[], None] | None = None,
    signals: Sequence[int] = (signal.SIGINT, signal.SIGTERM),
) -> bool:
    """Install a SIGINT/SIGTERM handler that tears down the WHOLE outer scope, then exits.

    Ports ``validate.py._install_scope_teardown``. THE GAP THIS CLOSES: killing only the
    runner process leaves ``setsid``-escapee orphans (servers, browsers) alive in the scope
    cgroup — killpg and systemd ``--collect`` both miss them. On signal we instead SIGKILL
    the entire scope subtree atomically.

    Provide EITHER a ``systemd_unit`` (a ``systemd --user`` transient scope, torn down with
    :func:`stop_systemd_scope`) OR a ``scope_cgroup`` path (a directly-delegated cgroup,
    torn down with :func:`kill_cgroup`). ``scope_cgroup`` doubles as the pre-resolved path
    that lets the systemd path skip the in-handler ``systemctl show`` (resolve it once now
    with :func:`outer_scope_cgroup`, not inside the handler). ``on_teardown`` runs first —
    use it to release a lock the normal ``finally`` block will no longer reach once the
    scope teardown SIGKILLs this process.

    The handler prints its reason, tears down, then ``os._exit(128 + signum)`` so an aborted
    run exits with the conventional signal code. This is intentionally the SIGNAL path only;
    the NORMAL-exit backstop belongs elsewhere (see
    :meth:`CgroupManager.kill_all_remaining`, which does NOT stop the scope and so preserves
    a successful run's exit code).

    Returns ``True`` when a handler was installed. Returns ``False`` (No Silent Failure: a
    visible, inspectable signal to the caller) when neither a unit nor a scope cgroup was
    given — there is simply nothing to tear down, e.g. a run with no cgroup delegation.
    """
    if systemd_unit is None and scope_cgroup is None:
        return False

    def _on_signal(signum: int, _frame: FrameType | None) -> None:
        scope_name = systemd_unit or str(scope_cgroup)
        try:
            sys.stderr.write(
                f"\n[teardown] signal {signum} — tearing down scope {scope_name} "
                "(kills all steps + orphans)…\n"
            )
            sys.stderr.flush()
        except OSError:
            pass
        if on_teardown is not None:
            try:
                on_teardown()
            except Exception as exc:  # a teardown callback must never eat the exit path
                _warn(f"on_teardown callback raised during signal {signum}: {exc!r}")
        torn_down = False
        if systemd_unit is not None:
            torn_down = stop_systemd_scope(systemd_unit, scope_cgroup)
        elif scope_cgroup is not None:
            torn_down = kill_cgroup(scope_cgroup)
        if not torn_down:
            _warn(
                f"scope teardown of {scope_name} did not confirm success; orphaned "
                "processes may remain in the scope cgroup."
            )
        # If teardown somehow did not already kill us, exit with the signal code.
        os._exit(128 + signum)

    installed = False
    for sig in signals:
        try:
            signal.signal(sig, _on_signal)
            installed = True
        except (ValueError, OSError) as exc:
            _warn(f"could not install teardown handler for signal {sig}: {exc!r}")
    return installed


# --------------------------------------------------------------------------------------
# External reaper  (ports kill_zombie_processes.py, parameterized by cwd + patterns)
# --------------------------------------------------------------------------------------


@dataclass(frozen=True)
class ReapSummary:
    """Outcome of an external reap pass: which PIDs were killed / failed, scopes stopped."""

    killed: tuple[int, ...] = ()
    failed: tuple[int, ...] = ()
    scopes_stopped: int = 0

    @property
    def ok(self) -> bool:
        """True when nothing that was targeted failed to die."""
        return not self.failed


def _running_processes() -> list[str]:
    """Lines of ``ps aux`` (PID + full command), or ``[]`` with a visible warning on error.

    Ports ``kill_zombie_processes.get_processes``.
    """
    try:
        result = subprocess.run(
            ["ps", "aux"], capture_output=True, text=True, timeout=10
        )
        return result.stdout.splitlines()
    except (subprocess.TimeoutExpired, OSError) as exc:
        _warn(f"could not read process list (ps aux): {exc!r}")
        return []


def _proc_cwd(pid: int) -> str | None:
    """Resolved working directory of a PID via ``/proc/<pid>/cwd``, or ``None``."""
    try:
        return os.readlink(f"/proc/{pid}/cwd")
    except OSError:
        return None


def _cmdline_targets_cwd(cmd: str, pid: int, cwd: str) -> bool:
    """Does a process belong to ``cwd``? By its command line OR its actual ``/proc`` cwd.

    DeepScry matches the working directory as a substring of the command line
    (``current_dir in cmd``); we additionally check the real ``/proc/<pid>/cwd`` so a
    process launched with a relative path (whose cwd never appears in ``argv``) is still
    correctly attributed — the same robustness the scope reaper already relied on.
    """
    if cwd in cmd:
        return True
    resolved = _proc_cwd(pid)
    return resolved is not None and (resolved == cwd or resolved.startswith(cwd + os.sep))


def kill_process(pid: int, description: str, *, term_grace_s: float = 0.5) -> bool:
    """SIGTERM a PID, wait a grace period, then SIGKILL if it is still alive.

    Ports ``kill_zombie_processes.kill_process``. Returns ``True`` when the process existed
    and was signalled, ``False`` when it was already dead, permission was denied, or another
    OS error occurred (each reported visibly rather than swallowed).
    """
    print(f"  Killing PID {pid}: {description}")
    try:
        os.kill(pid, signal.SIGTERM)
        time.sleep(term_grace_s)
        try:
            os.kill(pid, 0)  # signal 0 only probes existence
            os.kill(pid, signal.SIGKILL)
            print("    (used SIGKILL)")
        except ProcessLookupError:
            pass  # exited during the grace period
        return True
    except ProcessLookupError:
        print("    (already dead)")
        return False
    except PermissionError:
        _warn(f"permission denied killing PID {pid} ({description})")
        return False
    except OSError as exc:
        _warn(f"error killing PID {pid} ({description}): {exc!r}")
        return False


def reap_processes_by_cwd(
    cwd: str,
    patterns: Sequence[str],
    *,
    protect: Callable[[int], bool] | None = None,
    dry_run: bool = False,
) -> tuple[list[int], list[int]]:
    """Kill leftover processes that (a) match a command-line ``pattern`` and (b) belong to
    ``cwd``, sparing this process and any PID a caller-supplied ``protect`` predicate keeps.

    Generic port of ``kill_zombie_processes.should_kill_process`` + the kill loop in
    ``main``: the hardcoded ``deepscry`` / ``cargo`` / ``chromium`` / ``validate.py`` rules
    become caller-supplied ``patterns``, and the DeepScry-specific "belongs to a live
    validate cgroup" guard becomes the generic ``protect`` predicate (pass one that returns
    ``True`` for any PID under a live run so an in-progress run is never reaped).

    Returns ``(killed_pids, failed_pids)``. With ``dry_run=True`` nothing is signalled and
    the would-be-killed PIDs are returned in ``killed_pids``.
    """
    own_pid = os.getpid()
    killed: list[int] = []
    failed: list[int] = []
    for line in _running_processes():
        if line.startswith("USER"):  # ps header
            continue
        parts = line.split(None, 10)
        if len(parts) < 11:
            continue
        try:
            pid = int(parts[1])
        except ValueError:
            continue
        cmd = parts[10]
        if pid == own_pid:
            continue
        if not any(pattern in cmd for pattern in patterns):
            continue
        if not _cmdline_targets_cwd(cmd, pid, cwd):
            continue
        if protect is not None and protect(pid):
            continue
        description = cmd[:80]
        if dry_run:
            print(f"  [dry-run] would kill PID {pid}: {description}")
            killed.append(pid)
            continue
        (killed if kill_process(pid, description) else failed).append(pid)
    return killed, failed


def stop_leftover_scopes(
    cwd: str,
    *,
    scope_glob: str,
    is_live: Callable[[Path], bool] | None = None,
) -> int:
    """Stop leftover ``systemd --user`` transient scopes that belong to ``cwd``.

    Generic port of ``kill_zombie_processes.stop_my_validate_scopes``. A ``systemctl stop``
    kills a scope's WHOLE cgroup including ``setsid`` escapees the per-PID scan misses.
    CROSS-CHECKOUT SAFE: a scope is stopped only when one of its processes has a
    ``/proc/<pid>/cwd`` within ``cwd`` (walked RECURSIVELY, since cgroup-v2 ``cgroup.procs``
    is per-cgroup and a live orphan may sit in a per-step child cgroup while the scope's own
    ``cgroup.procs`` is empty); a concurrent scope rooted elsewhere is left untouched.

    ``scope_glob`` selects candidate units (e.g. ``"validate-*.scope"``). ``is_live``, when
    given, spares a scope the caller still considers a live run (e.g. its supervisor PID is
    alive) even if it belongs to ``cwd``. Returns the count of scopes stopped.
    """
    if not shutil.which("systemctl"):
        return 0
    try:
        r = subprocess.run(
            ["systemctl", "--user", "list-units", "--type=scope", "--no-legend",
             "--plain", scope_glob],
            capture_output=True,
            text=True,
            timeout=8,
        )
    except (subprocess.TimeoutExpired, OSError) as exc:
        _warn(f"could not list systemd scopes ({scope_glob!r}): {exc!r}")
        return 0

    stopped = 0
    for line in r.stdout.splitlines():
        parts = line.split()
        if not parts or not parts[0].endswith(".scope"):
            continue
        unit = parts[0]
        cg = _systemd_scope_cgroup(unit)
        if cg is None or not cg.exists():
            continue
        if is_live is not None and is_live(cg):
            print(f"  Preserved live scope: {unit}")
            continue
        if not _scope_belongs_to_cwd(cg, cwd):
            continue  # cross-checkout (or empty) scope — DO NOT touch
        # cgroup.kill FIRST for an instant atomic SIGKILL of the whole subtree (a browser
        # ignores the SIGTERM `systemctl stop` sends first); then stop GCs the empty unit.
        if not kill_cgroup(cg):
            _warn(f"cgroup.kill of leftover scope {unit} failed; relying on systemctl stop only.")
        try:
            subprocess.run(["systemctl", "--user", "stop", unit],
                           capture_output=True, timeout=8)
            print(f"  Stopped leftover scope (this checkout): {unit}")
            stopped += 1
        except (subprocess.TimeoutExpired, OSError) as exc:
            _warn(f"failed to stop leftover scope {unit}: {exc!r}")
    return stopped


def _scope_belongs_to_cwd(cgroup: Path, cwd: str) -> bool:
    """True when any process anywhere in a scope's cgroup subtree has its cwd within ``cwd``.

    Ports the recursive ``cgroup.procs`` / ``/proc/<pid>/cwd`` walk in
    ``kill_zombie_processes.stop_my_validate_scopes``.
    """
    prefix = cwd + os.sep
    try:
        procs_files = list(cgroup.rglob("cgroup.procs"))
    except OSError:
        return False
    for procs in procs_files:
        try:
            pids = procs.read_text().split()
        except OSError:
            continue
        for pid in pids:
            resolved = _proc_cwd(int(pid)) if pid.isdigit() else None
            if resolved is not None and (resolved == cwd or resolved.startswith(prefix)):
                return True
    return False


def reap_external(
    cwd: str,
    patterns: Sequence[str],
    *,
    scope_glob: str | None = None,
    is_live: Callable[[Path], bool] | None = None,
    protect: Callable[[int], bool] | None = None,
    dry_run: bool = False,
) -> ReapSummary:
    """One-call external reap: leftover PIDs by cwd + leftover systemd scopes by cwd.

    Convenience composition of :func:`reap_processes_by_cwd` and (when ``scope_glob`` is
    given) :func:`stop_leftover_scopes` — the generic equivalent of
    ``kill_zombie_processes.main``, minus the DeepScry-specific ``.validate.lock`` handling
    (that lock lifecycle is the caller's concern, not the reaper's). Scope reaping runs
    only when ``scope_glob`` is provided and ``dry_run`` is off.
    """
    killed, failed = reap_processes_by_cwd(cwd, patterns, protect=protect, dry_run=dry_run)
    scopes_stopped = 0
    if scope_glob is not None and not dry_run:
        scopes_stopped = stop_leftover_scopes(cwd, scope_glob=scope_glob, is_live=is_live)
    return ReapSummary(
        killed=tuple(killed),
        failed=tuple(failed),
        scopes_stopped=scopes_stopped,
    )
