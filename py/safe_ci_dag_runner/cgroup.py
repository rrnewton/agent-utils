"""Two-level Linux cgroup-v2 containment for the safe-ci DAG runner.

This is the generic port of DeepScry's ``scripts/validate_cgroup.py`` — the same
observable behavior, but with zero project-specific names baked in. All the
DeepScry specifics (the ``mtg-validate.slice`` name, the ``MTG_VALIDATE_*``
environment sentinels, the ``[validate]`` log prefix) are lifted into a caller-
supplied :class:`ScopeNaming` value; nothing about MTG or DeepScry remains.

WHY THIS EXISTS
---------------
A build/test DAG re-execs itself inside a transient ``systemd-run --user
--scope`` (an OUTER cgroup) so the whole descendant tree is contained. But a
naive per-step ``killpg`` teardown has empirically-verified gaps (probed on a
dev box: systemd 255, cgroup v2, user-delegated cpu/memory/pids):

  1. ``killpg`` misses a ``setsid``/double-forked grandchild (an orphan server /
     ``http.server`` / browser) — the exact zombie / port-collision class this
     model guards against. ``setsid`` changes only the session/pgid, never
     cgroup membership, so a per-step ``cgroup.kill`` catches the escapee.
  2. On NORMAL exit a ``setsid`` orphan left in the OUTER scope keeps the scope
     ``active running``; ``systemd-run --collect`` does NOT garbage-collect a
     scope that still has live processes, so the orphan survives the run.
  3. Nothing STOPS the outer scope on Ctrl-C / ``kill`` of the runner without an
     explicit signal handler.

THE MODEL (two levels, both real cgroups — NOT sibling scopes)
--------------------------------------------------------------
A transient ``systemd-run --user --scope`` lands every unit as a SIBLING under
``app.slice`` — so a naive per-step ``systemd-run --scope`` would NOT be torn
down by stopping the outer scope (stopping a parent scope does not cascade to a
sibling child scope). Instead the OUTER scope is a DELEGATED cgroup
(``-p Delegate=yes``) and genuine CHILD cgroups are managed by hand::

    app.slice/<unit>.scope/          <- outer scope (delegated)
        ├── supervisor/               <- the runner itself lives here (cgroup-v2
        │                                "no internal processes": a cgroup with
        │                                child cgroups may not also hold procs)
        ├── step-build.release/       <- one child cgroup per step; the step's
        └── step-net.gui/                bash leader self-moves here FIRST

Per-step teardown is ``echo 1 > step-<tag>/cgroup.kill`` — an atomic SIGKILL of
the WHOLE subtree, ``setsid`` escapees included. Whole-run teardown is
``systemctl --user stop <unit>.scope``, which flushes every child cgroup because
they are genuinely nested.

GRACEFUL DEGRADATION + NO SILENT FAILURE
----------------------------------------
Everything here is best-effort: if ``Delegate=yes`` was not granted, the
delegated cgroup can't be found, or the host has no cgroup v2 / systemd, the
manager reports ``enabled == False`` and the caller falls back to ``killpg``.
cgroups are an ADDITIONAL, stronger reaper, never a hard dependency.

However — and this is the deliberate strengthening over the DeepScry original —
a best-effort cgroupfs write that would drop a requested cap (``memory.max``,
``memory.swap.max``, controller delegation, ``cgroup.kill``) never fails
SILENTLY. The original swallowed the ``OSError`` and ran the step uncapped with
no trace; here every such degradation emits a visible warning via :func:`_warn`.
"""

from __future__ import annotations

import os
import re
import shutil
import signal
import subprocess
import sys
from collections.abc import Callable, Mapping, Sequence
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from types import FrameType
from typing import TYPE_CHECKING

CGROUP_ROOT = Path("/sys/fs/cgroup")

#: Fraction of WHOLE-SYSTEM CPU the shared aggregate slice may use. 0.90 leaves
#: ~10% headroom for SSH, the coordinator, and the OS so the box stays
#: responsive even when concurrent runs saturate their budget.
DEFAULT_CPU_BUDGET_FRACTION = 0.90

#: Per-step child cgroup directory prefix (also the scan key for the
#: normal-exit backstop). cgroup-v2 directory names may not contain '/'.
_STEP_PREFIX = "step-"


@dataclass(frozen=True)
class ScopeNaming:
    """Caller-supplied names for the outer scope, slice, and sentinels.

    These are the ONLY project-specific strings in this module. A caller keeps
    the defaults or overrides them to brand its own runs; nothing else here
    needs changing to reuse the two-level cgroup machinery.
    """

    #: Shared parent slice for ALL concurrent runs. Every run's transient scope
    #: launches under it, so a single ``CPUQuota`` on the slice bounds the SUM of
    #: CPU across however many runs execute at once (not each one individually).
    slice_name: str = "safe-ci.slice"
    #: Prefix for the per-run transient scope unit (``<prefix>-<pid>``).
    unit_prefix: str = "safe-ci"
    #: Environment sentinel set in the re-exec'd (in-scope) child.
    env_in_scope: str = "SAFE_CI_IN_SCOPE"
    #: Environment var carrying the outer scope unit name to the in-scope child.
    env_scope_unit: str = "SAFE_CI_SCOPE_UNIT"
    #: Environment var carrying the delegated (systemd-free) cgroup path.
    env_direct_cgroup: str = "SAFE_CI_DIRECT_CGROUP"
    #: Prefix for every log/warning line this module prints.
    log_prefix: str = "[safe-ci]"
    #: Child cgroup the runner vacates into (cgroup-v2 no-internal-processes).
    supervisor_name: str = "supervisor"


DEFAULT_NAMING = ScopeNaming()


class CgroupEnforcementKind(str, Enum):
    """A cgroup boundary that can actually constrain a run.

    A ``str``-valued enum (the Python-3.10-compatible equivalent of the
    ``StrEnum`` the DeepScry original used): members compare equal to their
    string value, so ``kind.value`` and direct string comparison both work."""

    USER_SCOPE_QUOTA = "user-scope-quota"
    DELEGATED_CGROUPFS = "delegated-cgroupfs"
    CONTAINER_CPUSET = "container-cpuset"
    CONTAINER_QUOTA = "container-quota"


#: The CPU-boundary enum is the same set of kinds; kept as a named alias so
#: call sites reading a CPU-only enforcement result document their intent.
CpuEnforcementKind = CgroupEnforcementKind


def _warn(naming: ScopeNaming, msg: str) -> None:
    """Emit a visible degraded-enforcement warning (No Silent Failure).

    Best-effort cgroupfs writes that fail must NEVER drop a requested cap
    silently. Rather than crash (containment is additive, not a hard
    dependency), we print a clearly-labelled warning to stderr so the operator
    can see that enforcement degraded and why.
    """
    sys.stderr.write(f"{naming.log_prefix} WARNING: degraded enforcement: {msg}\n")
    sys.stderr.flush()


def _fmt_bytes(n: int) -> str:
    """Human-readable byte count for log lines (e.g. ``12.0 GiB``)."""
    value = float(n)
    for unit in ("B", "KiB", "MiB", "GiB", "TiB"):
        if value < 1024.0 or unit == "TiB":
            return f"{value:.1f} {unit}"
        value /= 1024.0
    return f"{n} B"


# --------------------------------------------------------------------------- #
# Low-level cgroup-v2 filesystem helpers                                       #
# --------------------------------------------------------------------------- #


def _sanitize(tag: str) -> str:
    """A cgroup directory name for a step tag. cgroup-v2 names may not contain
    '/'; keep it readable (``group.job`` -> ``step-group.job``) but strip
    anything odd."""
    safe = re.sub(r"[^A-Za-z0-9._-]", "_", tag)
    return f"{_STEP_PREFIX}{safe}"


def _my_cgroup_path() -> Path | None:
    """Filesystem path of THIS process's cgroup v2, via ``/proc/self/cgroup``
    (``0::<path>`` for the unified hierarchy). None if not cgroup v2."""
    try:
        for line in Path("/proc/self/cgroup").read_text().splitlines():
            if line.startswith("0::"):
                rel = line[3:].lstrip("/")
                return CGROUP_ROOT / rel
    except OSError:
        pass
    return None


def _read_cgroup_value(group: Path | None, name: str) -> str | None:
    if group is None:
        return None
    try:
        return (group / name).read_text().strip()
    except OSError:
        return None


def _bounded(value: str | None) -> bool:
    """True for a numeric cgroup-v2 limit, never for ``max`` or unreadable."""
    try:
        return value is not None and value != "max" and int(value) >= 0
    except ValueError:
        return False


def _quota_is_bounded(value: str | None) -> bool:
    try:
        quota, period = (int(part) for part in (value or "").split())
        return quota > 0 and period > 0
    except (TypeError, ValueError):
        return False


def _cpuset_count(value: str | None) -> int | None:
    """Count Linux cpulist entries such as ``0-3,8,10-11``."""
    if not value:
        return None
    count = 0
    try:
        for item in value.split(","):
            bounds = item.split("-", maxsplit=1)
            start = int(bounds[0])
            end = int(bounds[-1])
            if end < start:
                return None
            count += end - start + 1
    except ValueError:
        return None
    return count


def _is_below_root(group: Path) -> bool:
    return group == CGROUP_ROOT or CGROUP_ROOT in group.parents


def _controller_set(group: Path | None, name: str) -> set[str]:
    """Controller names from a cgroup interface file. Leading ``+``/``-`` are
    stripped so a value seeded via a raw ``+cpu +memory`` write parses
    identically to the kernel's plain rendering."""
    raw = _read_cgroup_value(group, name) or ""
    return {token.lstrip("+-") for token in raw.split() if token.lstrip("+-")}


def _delegated_cgroupfs_available(group: Path | None) -> bool:
    """Probe whether THIS cgroup may create constrained child cgroups.

    Merely reading cgroup accounting is not enough. A delegated hierarchy must
    expose cpu and memory to children, have them enabled in ``subtree_control``,
    and permit a throwaway child directory to be created and removed.
    """
    if group is None:
        return False
    controllers = _controller_set(group, "cgroup.controllers")
    enabled = _controller_set(group, "cgroup.subtree_control")
    if not {"cpu", "memory"}.issubset(controllers | enabled):
        return False
    if not {"cpu", "memory"}.issubset(enabled):
        return False
    probe = group / f"safe-ci-probe-{os.getpid()}"
    try:
        probe.mkdir()
        probe.rmdir()
        return True
    except OSError:
        try:
            probe.rmdir()
        except OSError:
            pass
        return False


def _delegated_cgroup_base(current: Path | None) -> Path | None:
    """The TOPMOST ancestor (current leaf up to the namespace root) that can
    host constrained child cgroups, or None.

    Why walk up: in a systemd-less container every process lives in a POPULATED
    leaf (e.g. ``/init`` after the standard no-internal-processes dance), and a
    populated cgroup can never enable subtree controllers — so probing only the
    current cgroup wrongly reports "unavailable" even when the namespace root
    has cpu+memory+pids delegated and accepts child cgroups. Job cgroups must
    then be created as SIBLINGS under this base (next to the process leaf),
    never as children of the populated leaf itself."""
    if current is None:
        return None
    if not _is_below_root(current):
        return current if _delegated_cgroupfs_available(current) else None
    best: Path | None = None
    group = current
    while True:
        if _delegated_cgroupfs_available(group):
            best = group  # keep walking: prefer the topmost passing ancestor
        if group == CGROUP_ROOT:
            return best
        group = group.parent


# --------------------------------------------------------------------------- #
# systemd user-slice aggregate cap                                            #
# --------------------------------------------------------------------------- #


def cpu_quota_percent(fraction: float = DEFAULT_CPU_BUDGET_FRACTION) -> int:
    """systemd ``CPUQuota`` percentage for ``fraction`` of ALL cores, as an
    integer percent. With 16 cores and fraction 0.90 this is 1440 (``"1440%"``),
    i.e. up to 14.4 cores of aggregate CPU shared across everything in the
    slice. Minimum 100% (one core) so a 1-core box still makes progress."""
    ncpu = os.cpu_count() or 1
    return max(100, int(round(ncpu * fraction * 100)))


def ensure_aggregate_slice(
    fraction: float = DEFAULT_CPU_BUDGET_FRACTION,
    naming: ScopeNaming = DEFAULT_NAMING,
) -> bool:
    """Create/refresh the shared aggregate slice with a ``CPUQuota`` capping the
    AGGREGATE CPU of every scope launched under it. Idempotent:
    ``systemctl --user set-property`` updates a live slice in place and starting
    an already-active slice is a no-op. Best-effort — returns False (caller just
    omits ``--slice`` and runs unconstrained) if systemd/user-cgroup is
    unavailable.

    cgroup-v2 detail: a slice's ``cpu.max`` is only enforced once the ``cpu``
    controller is delegated to it by its parent. ``set-property CPUQuota=...``
    makes systemd enable ``+cpu`` in the parent's ``subtree_control``, so the cap
    actually bites; without going through systemd we'd have to do that by hand."""
    quota = cpu_quota_percent(fraction)
    try:
        r1 = subprocess.run(
            ["systemctl", "--user", "start", naming.slice_name],
            capture_output=True, timeout=10,
        )
        r2 = subprocess.run(
            ["systemctl", "--user", "--runtime", "set-property",
             naming.slice_name, f"CPUQuota={quota}%"],
            capture_output=True, timeout=10,
        )
        return r1.returncode == 0 and r2.returncode == 0
    except (subprocess.TimeoutExpired, OSError):
        return False


_SCOPE_PROBE: bool | None = None


def systemd_scope_available() -> bool:
    """True iff ``systemd-run --user --scope`` actually works here (cached)."""
    global _SCOPE_PROBE
    if _SCOPE_PROBE is None:
        if not shutil.which("systemd-run"):
            _SCOPE_PROBE = False
        else:
            try:
                r = subprocess.run(
                    ["systemd-run", "--user", "--scope", "--quiet",
                     f"--unit=safe-ci-probe-{os.getpid()}", "true"],
                    capture_output=True, timeout=8,
                )
                _SCOPE_PROBE = r.returncode == 0
            except (subprocess.TimeoutExpired, OSError):
                _SCOPE_PROBE = False
    return _SCOPE_PROBE


# --------------------------------------------------------------------------- #
# systemd-free delegated-cgroupfs fallback                                    #
# --------------------------------------------------------------------------- #


def self_heal_namespace_root() -> tuple[bool, str]:
    """One-shot self-heal of a private-namespace cgroup root (the
    "no-internal-processes dance"), for containers whose entrypoint did not
    perform it. Idempotent and additive: it only MOVES processes out of the
    namespace ROOT into ``<root>/init`` and ENABLES controllers; it never moves
    processes out of any other cgroup and never disables anything.

    Preconditions (all checked; returns ``(False, why)`` when any fails):
      * the root delegates cpu+memory in ``cgroup.controllers``,
      * cpu or memory is still MISSING from root ``cgroup.subtree_control``,
      * root ``cgroup.subtree_control`` is writable,
      * this process's own cgroup path is SHALLOW (namespace-root evidence —
        refuses to touch what looks like a shared host hierarchy).

    Returns ``(True, detail)`` only when cpu+memory are verifiably enabled after
    the dance. Callers must log the detail loudly either way (no silent skip)."""
    root = CGROUP_ROOT
    controllers = _controller_set(root, "cgroup.controllers")
    if not {"cpu", "memory"}.issubset(controllers):
        return False, (f"namespace root {root} does not delegate cpu+memory "
                       f"(controllers: {sorted(controllers) or 'unreadable'})")
    enabled = _controller_set(root, "cgroup.subtree_control")
    if {"cpu", "memory"}.issubset(enabled):
        return False, f"namespace root {root} already enables cpu+memory for children"
    if not os.access(root / "cgroup.subtree_control", os.W_OK):
        return False, f"{root}/cgroup.subtree_control is not writable"
    mine = _my_cgroup_path()
    if mine is None:
        return False, "own cgroup path unreadable; refusing to touch the root"
    depth = (0 if mine == root
             else len(mine.relative_to(root).parts) if _is_below_root(mine) else 99)
    if depth > 1:
        return False, (f"own cgroup {mine} is nested {depth} deep — looks like a shared "
                       "host hierarchy, not a private namespace root; refusing the dance")
    # cgroup-v2 "no internal processes": the root cannot enable controllers for
    # children while it directly holds processes. Drain them into `init/`.
    init = root / "init"
    moved = 0
    if (_read_cgroup_value(root, "cgroup.procs") or "").split():
        try:
            init.mkdir(exist_ok=True)
        except OSError as exc:
            return False, f"could not create {init}: {exc}"
        for _ in range(5):  # a moved leader can reveal late-forked children
            pids = (_read_cgroup_value(root, "cgroup.procs") or "").split()
            if not pids:
                break
            for pid in pids:
                try:
                    (init / "cgroup.procs").write_text(pid)
                    moved += 1
                except OSError:
                    pass  # pid may have exited mid-move; racy, legitimately ignored
    # Enable controllers: atomic combined write first, then per-controller
    # fallback (the combined form fails wholesale if any one is unavailable).
    want = [c for c in ("cpu", "memory", "pids") if c in controllers]
    try:
        (root / "cgroup.subtree_control").write_text(" ".join(f"+{c}" for c in want))
    except OSError:
        for c in want:
            try:
                (root / "cgroup.subtree_control").write_text(f"+{c}")
            except OSError:
                pass
    enabled = _controller_set(root, "cgroup.subtree_control")
    if {"cpu", "memory"}.issubset(enabled):
        return True, (f"enabled {'+'.join(want)} on namespace root {root} "
                      f"(moved {moved} root process(es) into {init})")
    return False, (f"dance attempted (moved {moved} process(es)) but subtree_control "
                   f"still lacks cpu+memory: {sorted(enabled) or 'empty'}")


def probe_current_enforcement() -> tuple[CgroupEnforcementKind | None, str]:
    """Return verified container/delegated enforcement, never accounting alone."""
    current = _my_cgroup_path()
    if current is None:
        return None, "current cgroup is unreadable"

    base = _delegated_cgroup_base(current)
    if base is not None:
        where = "current cgroup" if base == current else f"ancestor {base}"
        return CgroupEnforcementKind.DELEGATED_CGROUPFS, (
            f"writable delegated cpu+memory cgroupfs at {where}")

    group = current
    while _is_below_root(group):
        cpu_max = _read_cgroup_value(group, "cpu.max")
        memory_max = _read_cgroup_value(group, "memory.max")
        if _quota_is_bounded(cpu_max) or _bounded(memory_max):
            return CgroupEnforcementKind.CONTAINER_QUOTA, (
                f"bounded container limit at {group}: cpu.max={cpu_max or 'UNREADABLE'}, "
                f"memory.max={memory_max or 'UNREADABLE'}"
            )
        cpuset = _read_cgroup_value(group, "cpuset.cpus.effective")
        if _cpuset_count(cpuset):
            return CgroupEnforcementKind.CONTAINER_CPUSET, (
                f"container cpuset at {group}: {cpuset}"
            )
        if group == CGROUP_ROOT:
            break
        group = group.parent
    return None, "no writable delegated cgroupfs, bounded quota, or effective cpuset"


def enter_delegated_scope(
    memory_max: int | None,
    cpu_count: int | None = None,
    naming: ScopeNaming = DEFAULT_NAMING,
) -> tuple[bool, str]:
    """Move this process into a constrained delegated child cgroup.

    This is the systemd-free fallback. The cgroup probe has already established
    that a delegated ancestor exposes cpu and memory; failures here are fatal to
    callers because continuing would make the advertised limits advisory only —
    which is why the whole body reports its ``OSError`` loudly via the returned
    ``(False, detail)`` rather than swallowing it.

    The job cgroup is created under the topmost delegated ancestor — as a SIBLING
    of the process's own (possibly populated) leaf, not a child of it: a
    populated leaf can never enable subtree controllers, so nesting under it
    would make every limit advisory."""
    base = _delegated_cgroup_base(_my_cgroup_path())
    if base is None:
        return False, "delegated cgroupfs became unavailable"
    child = base / f"{naming.unit_prefix}-{os.getpid()}"
    try:
        child.mkdir()
        # Keep the complete hierarchy swapless. Some hosts' userspace OOM policy
        # may choose swap-heavy cgroups even when the kernel did not record a
        # cgroup OOM event.
        (child / "memory.swap.max").write_text("0")
        if memory_max is not None:
            (child / "memory.max").write_text(str(memory_max))
        # Keep hard caps only. Soft reclaim throttling would mark a legitimately
        # long run as memory-pressure-heavy on some hosts.
        (child / "memory.high").write_text("max")
        quota = cpu_count * 100 if cpu_count is not None else cpu_quota_percent()
        (child / "cpu.max").write_text(f"{quota * 1000} 100000")
        (child / "cgroup.procs").write_text(str(os.getpid()))
        os.environ[naming.env_in_scope] = "1"
        os.environ[naming.env_direct_cgroup] = str(child)
        return True, f"entered delegated cgroup {child}"
    except OSError as exc:
        try:
            child.rmdir()
        except OSError:
            pass
        return False, str(exc)


# --------------------------------------------------------------------------- #
# Outer systemd-run --user --scope (Delegate=yes) re-exec                     #
# --------------------------------------------------------------------------- #


def reexec_in_scope(
    argv: Sequence[str],
    *,
    memory_max: int | None,
    cpu_count: int | None = None,
    naming: ScopeNaming = DEFAULT_NAMING,
    use_aggregate_slice: bool = True,
    skip_in_ci: bool = True,
) -> bool:
    """Re-exec ``argv`` inside a transient ``systemd-run --user --scope`` (a
    delegated cgroup), so EVERY descendant — including ``setsid``/double-forked
    escapees the per-step ``killpg`` reaper can't catch — is contained and reaped
    atomically when the run ends or via ``systemctl --user stop <unit>.scope``.

    On success ``os.execvp`` REPLACES this process and never returns. The bool
    return distinguishes the paths that DON'T re-exec:

      * ``True``  — already in-scope (anti-recursion) or intentionally skipped in
        CI: the caller should proceed to run directly.
      * ``False`` — systemd scope is unavailable or the exec failed: the caller
        must refuse to run advisory-only (No Silent Failure — the reason is
        written to stderr).

    ``Delegate=yes`` makes the scope a DELEGATED cgroup so the in-scope runner
    can carve per-step CHILD cgroups under it (:class:`Cgroups`) for
    ``setsid``-proof teardown via ``cgroup.kill``. ``MemorySwapMax=0`` is applied
    independently of the optional RAM hard cap: a run must never become a
    host-side swap-kill candidate."""
    if os.environ.get(naming.env_in_scope) == "1":
        return True  # already re-exec'd into the scope (anti-recursion)
    if skip_in_ci and (os.environ.get("CI") or os.environ.get("GITHUB_ACTIONS")):
        return True
    if not systemd_scope_available():
        sys.stderr.write(f"{naming.log_prefix} ERROR: systemd --user scope is unavailable; "
                         "refusing advisory-only containment.\n")
        return False

    unit = f"{naming.unit_prefix}-{os.getpid()}"
    # Swaplessness is independent of the optional RAM hard cap (a 40 GB runaway
    # swapping into a smaller RAM still thrashes the host).
    props = ["-p", "Delegate=yes", "-p", "MemorySwapMax=0"]
    if cpu_count is not None:
        props += ["-p", f"CPUQuota={cpu_count * 100}%"]
        print(f"{naming.log_prefix} per-run CPU cap: CPUQuota={cpu_count * 100}% "
              f"({cpu_count} CPU{'s' if cpu_count != 1 else ''}).")

    # Launch under the SHARED aggregate slice so the kernel shares its CPUQuota
    # across however many runs execute concurrently — bounding the AGGREGATE, not
    # each one. Best-effort: without the slice we run unconstrained (as before).
    slice_args: list[str] = []
    if use_aggregate_slice and ensure_aggregate_slice(naming=naming):
        slice_args = [f"--slice={naming.slice_name}"]
        quota = cpu_quota_percent()
        print(f"{naming.log_prefix} CPU cap: shared {naming.slice_name} CPUQuota={quota}% "
              f"(~90% of {os.cpu_count()} cores, AGGREGATE across concurrent runs).")

    if memory_max:
        # MemoryMax is the sole memory limit; no MemoryHigh (reclaim throttling
        # creates spurious pressure signals). MemorySwapMax=0 already OOM-kills a
        # runaway at the cap rather than letting it swap.
        props += ["-p", f"MemoryMax={memory_max}"]
        print(f"{naming.log_prefix} outer scope memory cap: MemoryMax="
              f"{_fmt_bytes(memory_max)} (hard-cap-only, swap=0).")

    cmd = ["systemd-run", "--user", "--scope", "--collect", "--quiet",
           f"--unit={unit}", *slice_args, *props,
           f"--setenv={naming.env_in_scope}=1",
           f"--setenv={naming.env_scope_unit}={unit}.scope",
           "--", *argv]
    print(f"{naming.log_prefix} re-exec inside transient systemd scope {unit}.scope "
          "(two-level cgroup; full-descendant cleanup on exit)…")
    sys.stdout.flush()
    try:
        os.execvp("systemd-run", cmd)  # replaces this process; never returns
    except OSError as exc:
        sys.stderr.write(f"{naming.log_prefix} ERROR: systemd-run exec failed ({exc}); "
                         "refusing to run without cgroup enforcement.\n")
        return False


def install_scope_teardown(
    naming: ScopeNaming = DEFAULT_NAMING,
    on_teardown: Callable[[], None] | None = None,
) -> None:
    """Inside the scope, make Ctrl-C / ``kill`` of the runner tear down the WHOLE
    cgroup — not just this PID. Killing only the scoped runner would leave
    ``setsid``-escapee orphans alive in the scope cgroup (``killpg`` AND
    ``--collect`` both miss them). On SIGINT/SIGTERM this ``systemctl --user
    stop``s our OWN scope (or, for the systemd-free delegated case, writes the
    scope's ``cgroup.kill``), which SIGKILLs every child step cgroup + escapee
    atomically and kills us too (an aborted run exits with the signal code).

    The NORMAL-exit backstop is separate (:meth:`Cgroups.kill_all_remaining`),
    which does NOT stop the scope so a SUCCESSFUL run's exit code is preserved.

    ``on_teardown`` runs BEFORE the scope is stopped (e.g. to release a lock the
    ``finally`` won't reach once the SIGKILL lands). No-op when not in-scope."""
    if os.environ.get(naming.env_in_scope) != "1":
        return
    unit = os.environ.get(naming.env_scope_unit)
    # Resolve the scope cgroup path NOW (not inside the handler) so the handler's
    # cgroup.kill is a single fast file-write with no systemctl shell-out (which
    # can stall under load).
    scope_cg = scope_cgroup_from_self(naming)
    if not unit and scope_cg is None:
        return

    def _on_signal(signum: int, _frame: FrameType | None) -> None:
        try:
            scope_name = unit or str(scope_cg)
            sys.stderr.write(f"\n{naming.log_prefix} signal {signum} — stopping scope "
                             f"{scope_name} (tears down all steps + orphans)…\n")
            sys.stderr.flush()
        except Exception:
            pass
        if on_teardown is not None:
            try:
                on_teardown()
            except Exception:
                pass
        if unit:
            stop_scope(unit, scope_cg, naming=naming)
        elif scope_cg is not None:
            kill_scope_cgroup(scope_cg)
        os._exit(128 + signum)

    for sig in (signal.SIGINT, signal.SIGTERM):
        try:
            signal.signal(sig, _on_signal)
        except (ValueError, OSError):
            pass


# --------------------------------------------------------------------------- #
# Scope discovery + teardown                                                  #
# --------------------------------------------------------------------------- #


def _scope_cgroup_path(unit: str) -> Path | None:
    """Filesystem cgroup path of a --user transient scope (via systemctl)."""
    try:
        r = subprocess.run(
            ["systemctl", "--user", "show", unit, "--property=ControlGroup", "--value"],
            capture_output=True, text=True, timeout=8)
    except (subprocess.TimeoutExpired, OSError):
        return None
    cg = r.stdout.strip()
    return (CGROUP_ROOT / cg.lstrip("/")) if cg else None


def scope_cgroup_from_self(naming: ScopeNaming = DEFAULT_NAMING) -> Path | None:
    """The OUTER scope's cgroup path, derived from THIS process's own cgroup — no
    systemctl shell-out (fast + contention-proof, for the signal handler). Inside
    the scope the runner lives in ``<scope>/supervisor``, so the scope is the
    parent; if for some reason we're at the scope root, return it directly.
    Returns None when not in a scope."""
    direct = os.environ.get(naming.env_direct_cgroup)
    if direct:
        scope = Path(direct)
        if scope.is_dir():
            return scope
    mine = _my_cgroup_path()
    if mine is None:
        return None
    if mine.name == naming.supervisor_name:
        return mine.parent
    if mine.name.endswith(".scope"):
        return mine
    return None


def scope_memory_peak(naming: ScopeNaming = DEFAULT_NAMING) -> int | None:
    """Peak memory (bytes) of the WHOLE scope cgroup — the authoritative peak RSS
    across every step, read from the scope's ``memory.peak`` (no sampling). None
    when not in a scope / file absent."""
    scope = scope_cgroup_from_self(naming)
    if scope is None:
        return None
    try:
        return int((scope / "memory.peak").read_text().strip())
    except (OSError, ValueError):
        return None


def kill_scope_cgroup(scope: Path) -> bool:
    """Atomically SIGKILL a delegated scope cgroup and every descendant."""
    try:
        (scope / "cgroup.kill").write_text("1")
        return True
    except OSError:
        return False


def stop_scope(
    unit: str,
    scope_cg: Path | None = None,
    naming: ScopeNaming = DEFAULT_NAMING,
) -> bool:
    """Tear down the whole outer scope. Two-step for SPEED + cleanliness:

      1. ``cgroup.kill`` the scope's cgroup directly — an INSTANT, atomic SIGKILL
         of every member (including a browser, which IGNORES the SIGTERM that
         ``systemctl stop`` sends first). Without this, ``systemctl stop`` sits
         in ``stop-sigterm`` for the scope's full ``TimeoutStopSec``.
      2. ``systemctl --user stop`` to deactivate + GC the (now-empty) unit.

    Either step alone flushes the descendants; doing both makes teardown both
    immediate and tidy. SIGKILL-proof, setsid-proof. Best-effort throughout.

    ``scope_cg``, if given (from :func:`scope_cgroup_from_self`), skips the
    ``systemctl show`` lookup for step 1 — important in a SIGNAL HANDLER where
    shelling out under load can stall."""
    if not unit:
        return False
    cg = scope_cg or _scope_cgroup_path(unit)
    if cg is not None:
        kill_scope_cgroup(cg)
    try:
        subprocess.run(["systemctl", "--user", "stop", unit],
                       capture_output=True, timeout=15)
        return True
    except (subprocess.TimeoutExpired, OSError):
        return False


# --------------------------------------------------------------------------- #
# Enforcement verification + usage reporting                                  #
# --------------------------------------------------------------------------- #


def verify_scope_limits(
    expected_memory_max: int | None,
    expected_cpu_count: int | None,
    naming: ScopeNaming = DEFAULT_NAMING,
) -> bool:
    """Verify the requested outer limits reached cgroup v2; print actionable
    evidence. Intentionally a hard boolean rather than a best-effort metric: a
    run that claims containment while its requested limits are absent is unsafe."""
    scope = scope_cgroup_from_self(naming)
    if scope is None:
        print(f"{naming.log_prefix} ERROR: outer cgroup limit audit unavailable: "
              "scope not found")
        return False

    memory_max = _read_cgroup_value(scope, "memory.max")
    memory_swap_max = _read_cgroup_value(scope, "memory.swap.max")
    cpu_max = _read_cgroup_value(scope, "cpu.max")
    if expected_memory_max is None:
        memory_ok = memory_max == "max"
    else:
        try:
            actual_memory_max = int(memory_max or "")
            # cgroup v2 rounds a byte limit down to its page boundary. Accept
            # exactly that kernel representation, but never a broader limit.
            page_size = os.sysconf("SC_PAGE_SIZE")
            memory_ok = (actual_memory_max <= expected_memory_max
                         and expected_memory_max - actual_memory_max < page_size)
        except (OSError, ValueError):
            memory_ok = False
    swap_ok = memory_swap_max == "0"
    cpu_ok = True
    if expected_cpu_count is not None:
        try:
            quota, period = (int(part) for part in (cpu_max or "").split())
            cpu_ok = quota == expected_cpu_count * period
        except (TypeError, ValueError):
            cpu_ok = False

    print(f"{naming.log_prefix} outer cgroup audit: memory.max={memory_max or 'UNREADABLE'} "
          f"({'bound' if memory_ok else 'MISMATCH'}), "
          f"memory.swap.max={memory_swap_max or 'UNREADABLE'} "
          f"({'disabled' if swap_ok else 'MISMATCH'}), cpu.max={cpu_max or 'UNREADABLE'} "
          f"({'bound' if cpu_ok else 'MISMATCH'})")
    return memory_ok and swap_ok and cpu_ok


def _quota_matches(cpu_max: str | None, expected_cpu_count: int) -> bool:
    try:
        quota, period = (int(part) for part in (cpu_max or "").split())
        return quota == expected_cpu_count * period
    except (TypeError, ValueError):
        return False


def verify_current_cpu_enforcement(
    expected_cpu_count: int,
    naming: ScopeNaming = DEFAULT_NAMING,
) -> CgroupEnforcementKind | None:
    """Prove the container CPU boundary from quota or effective cpuset state."""
    current = _my_cgroup_path()
    if current is None:
        print(f"{naming.log_prefix} container cgroup audit: current cgroup UNREADABLE")
        return None

    cpu_max = _read_cgroup_value(current, "cpu.max")
    cpuset = _read_cgroup_value(current, "cpuset.cpus.effective")
    cpuset_count = _cpuset_count(cpuset)
    kind: CgroupEnforcementKind | None
    if _quota_matches(cpu_max, expected_cpu_count):
        kind = CgroupEnforcementKind.CONTAINER_QUOTA
    elif cpuset_count == expected_cpu_count:
        kind = CgroupEnforcementKind.CONTAINER_CPUSET
    else:
        kind = None
        parent = current.parent
        while parent == CGROUP_ROOT or CGROUP_ROOT in parent.parents:
            if _quota_matches(_read_cgroup_value(parent, "cpu.max"), expected_cpu_count):
                kind = CgroupEnforcementKind.CONTAINER_QUOTA
                break
            if parent == CGROUP_ROOT:
                break
            parent = parent.parent

    print(f"{naming.log_prefix} container cgroup audit: cpu.max={cpu_max or 'UNREADABLE'}; "
          f"cpuset.cpus.effective={cpuset or 'UNREADABLE'} "
          f"(count={cpuset_count if cpuset_count is not None else 'unknown'}); "
          f"enforcement={kind.value if kind is not None else 'UNVERIFIED'}")
    return kind


def report_scope_usage(naming: ScopeNaming = DEFAULT_NAMING) -> bool:
    """Print outer cgroup peak/OOM/CPU evidence; return False on OOM or unreadable
    stats."""
    scope = scope_cgroup_from_self(naming)
    if scope is None:
        print(f"{naming.log_prefix} ERROR: outer cgroup usage audit unavailable: "
              "scope not found")
        return False
    peak = _read_cgroup_value(scope, "memory.peak")
    events = _read_cgroup_value(scope, "memory.events")
    cpu_stat = _read_cgroup_value(scope, "cpu.stat")
    if peak is None or events is None or cpu_stat is None:
        print(f"{naming.log_prefix} ERROR: outer cgroup usage audit could not read "
              "memory.peak, memory.events, and cpu.stat")
        return False
    event_values = dict(line.split(maxsplit=1) for line in events.splitlines())
    oom_kill = int(event_values.get("oom_kill", "0"))
    print(f"{naming.log_prefix} outer cgroup usage: memory.peak={peak} bytes; "
          f"memory.events oom={event_values.get('oom', '0')} oom_kill={oom_kill}; "
          f"cpu.stat {cpu_stat.replace(chr(10), ' ')}")
    return oom_kill == 0


def report_current_usage(naming: ScopeNaming = DEFAULT_NAMING) -> bool:
    """Print the current (container) cgroup usage evidence; return False on OOM or
    unreadable stats — the systemd-free/container counterpart of
    :func:`report_scope_usage`."""
    current = _my_cgroup_path()
    if current is None:
        print(f"{naming.log_prefix} ERROR: current cgroup unavailable")
        return False
    peak = _read_cgroup_value(current, "memory.peak")
    events = _read_cgroup_value(current, "memory.events")
    cpu_stat = _read_cgroup_value(current, "cpu.stat")
    if peak is None or events is None or cpu_stat is None:
        print(f"{naming.log_prefix} ERROR: container cgroup accounting files unreadable")
        return False
    event_values = dict(line.split(maxsplit=1) for line in events.splitlines())
    oom_kill = int(event_values.get("oom_kill", "0"))
    print(f"{naming.log_prefix} container cgroup usage: memory.peak={peak} bytes; "
          f"oom_kill={oom_kill}; cpu.stat {cpu_stat.replace(chr(10), ' ')}")
    return oom_kill == 0


# --------------------------------------------------------------------------- #
# Per-step child cgroups (the concrete CgroupManager)                         #
# --------------------------------------------------------------------------- #


class Cgroups:
    """Per-step child cgroups under the delegated outer scope — the concrete
    :class:`safe_ci_dag_runner.protocols.CgroupManager` for a real Linux
    cgroup-v2 host.

    Lifecycle:
      * construct once (in the in-scope runner). :attr:`enabled` tells the caller
        whether per-step cgroups are usable; if not, it must use ``killpg``.
      * :meth:`prepare_command` wraps a step's shell command so its bash leader
        self-moves into its child cgroup BEFORE forking any grandchild.
      * :meth:`kill` SIGKILLs the step's whole subtree (setsid-proof).
      * :meth:`cleanup` removes the now-empty child cgroup dir (best-effort).

    Ported from ``validate_cgroup.StepCgroups``. Per No Silent Failure, every
    best-effort cgroupfs write that would drop a requested cap now emits a
    visible warning via :func:`_warn` instead of swallowing the ``OSError``.
    """

    def __init__(self, naming: ScopeNaming = DEFAULT_NAMING) -> None:
        self._naming = naming
        self.enabled: bool = False
        self.root: Path | None = None  # the delegated scope cgroup root
        self._made: set[str] = set()
        # Only meaningful inside the scope (the re-exec sets this sentinel).
        if os.environ.get(naming.env_in_scope) != "1":
            return
        scope_cg = _my_cgroup_path()
        if scope_cg is None or not scope_cg.is_dir():
            return
        try:
            controllers = (scope_cg / "cgroup.controllers").read_text().split()
        except OSError as exc:
            _warn(naming, f"outer scope cgroup.controllers unreadable ({exc}); "
                  "per-step containment disabled — falling back to process-group kill")
            return
        # Move EVERY process out of the scope root into the `supervisor/` child
        # cgroup so the root holds NO processes and may then enable controllers
        # for its children (cgroup-v2 "no internal processes" rule). Draining
        # only os.getpid() is not enough: sibling helpers already running in the
        # scope root would leave it populated and make the subtree_control write
        # below fail with EBUSY (silently no-op'ing every per-step cap). Drain the
        # whole root, repeatedly (moving a leader can reveal late-forked
        # children), best-effort per pid.
        sup = scope_cg / naming.supervisor_name
        try:
            sup.mkdir(exist_ok=True)
        except OSError as exc:
            _warn(naming, f"could not create supervisor cgroup {sup} ({exc}); "
                  "per-step containment disabled — falling back to process-group kill")
            return
        for _ in range(5):
            try:
                pids = (scope_cg / "cgroup.procs").read_text().split()
            except OSError:
                pids = []
            if not pids:
                break
            for pid in pids:
                try:
                    (sup / "cgroup.procs").write_text(pid)
                except OSError:
                    pass  # pid may have exited mid-drain; racy, legitimately ignored
        # Root should now be empty → enable controllers for children. Enable each
        # INDEPENDENTLY so a single unavailable one doesn't block the rest (the
        # atomic multi-controller write fails wholesale on any one error).
        for c in ("memory", "cpu", "pids"):
            if c in controllers:
                try:
                    (scope_cg / "cgroup.subtree_control").write_text(f"+{c}")
                except OSError as exc:
                    # Non-fatal: cgroup.kill on a child still works without this
                    # controller. But `memory` is load-bearing for the inner
                    # caps, so surface the degradation rather than silently
                    # dropping per-step limits/accounting for this controller.
                    _warn(naming, f"could not delegate '{c}' controller to per-step cgroups "
                          f"({exc}); per-step {c} limits/accounting unavailable "
                          "(outer scope cap still applies)")
        self.root = scope_cg
        self.enabled = True

    def prepare_command(
        self,
        tag: str,
        cmd: str,
        mem_max: int | None = None,
        cpu_count: int | None = None,
    ) -> str:
        """Wrap ``cmd`` so its bash leader joins the step's child cgroup FIRST
        (before forking grandchildren). No-op string-wrap when disabled.

        ``mem_max`` (bytes), if given, is the INNER per-step ``memory.max`` cap so
        a single runaway step is OOM-killed at its own characterized limit,
        leaving the rest of the run + the host alive. ``cpu_count``, if given, is
        the inner ``cpu.max`` cap — and a cpu-cap write that cannot be verified
        makes the returned command FAIL loudly (never silently run uncapped).

        No Silent Failure: a ``mem_max`` / swap / soft-cap write that fails (e.g.
        the ``memory`` controller was not delegated) emits a visible warning; the
        step still runs under the outer cap, but the degradation is never
        invisible."""
        if not self.enabled or self.root is None:
            return cmd
        child = self.root / _sanitize(tag)
        try:
            child.mkdir(exist_ok=True)
            self._made.add(tag)
        except OSError as exc:
            _warn(self._naming, f"step {tag}: could not create child cgroup {child} "
                  f"({exc}); step runs under the outer cap only")
            return cmd
        try:
            # Every step is swapless, including uncharacterized ones without an
            # inner memory.max, so host-side swap policy never selects a step
            # cgroup as a kill target. Also clear any inherited soft cap.
            (child / "memory.swap.max").write_text("0")
            (child / "memory.high").write_text("max")
        except OSError as exc:
            _warn(self._naming, f"step {tag}: could not disable swap / clear soft cap "
                  f"({exc}); memory controller may not be delegated — outer cap still applies")
        if mem_max:
            try:
                (child / "memory.max").write_text(str(int(mem_max)))
            except OSError as exc:
                _warn(self._naming, f"step {tag}: could not apply inner memory cap "
                      f"memory.max={mem_max} ({exc}); step runs under the outer cap only")
        if cpu_count:
            period = 100_000
            expected = f"{int(cpu_count) * period} {period}"
            try:
                cpu_max = child / "cpu.max"
                cpu_max.write_text(expected)
                applied = cpu_max.read_text().strip()
                if applied != expected:
                    return (f"echo 'ERROR: step {tag} cpu.max mismatch: expected {expected}, "
                            f"got {applied}' >&2\nexit 1\n")
            except OSError as exc:
                return (f"echo 'ERROR: step {tag} cpu.max could not be applied: {exc}' >&2\n"
                        "exit 1\n")
        procs = child / "cgroup.procs"
        # $$ is the bash leader's own pid. Writing it migrates the leader; every
        # subsequently-forked child/grandchild inherits this cgroup at fork.
        # The self-move is best-effort in the shell (`|| true`): if it fails the
        # step still runs and the outer-scope reaper remains the backstop.
        # Errors are redirected so a delegation hiccup can't corrupt stdout.
        return f'echo $$ > {procs} 2>/dev/null || true\n{cmd}'

    def oom_kills(self, tag: str) -> int:
        """OOM-kill event count inside the step's cgroup (``memory.events``
        ``oom_kill``). ``> 0`` means the step (or a descendant) hit its INNER
        ``memory.max`` — the actionable-OOM signal. 0 if absent/unreadable. Read
        BEFORE :meth:`cleanup`."""
        if not self.enabled or self.root is None or tag not in self._made:
            return 0
        try:
            events = (self.root / _sanitize(tag) / "memory.events").read_text()
            for line in events.splitlines():
                if line.startswith("oom_kill "):
                    return int(line.split()[1])
        except (OSError, ValueError, IndexError):
            pass
        return 0

    def peak_bytes(self, tag: str) -> int | None:
        """Peak RSS (bytes) of the step's cgroup (``memory.peak``), for baseline
        characterization. None if unreadable. Read BEFORE :meth:`cleanup`."""
        if not self.enabled or self.root is None or tag not in self._made:
            return None
        try:
            return int((self.root / _sanitize(tag) / "memory.peak").read_text().strip())
        except (OSError, ValueError):
            return None

    def thread_count(self, tag: str) -> int | None:
        """Current descendant thread count from the step's ``cgroup.threads``."""
        if not self.enabled or self.root is None or tag not in self._made:
            return None
        try:
            threads = (self.root / _sanitize(tag) / "cgroup.threads").read_text()
            return len(threads.splitlines())
        except OSError:
            return None

    def cpu_stats(self, tag: str) -> Mapping[str, int] | None:
        """Current per-step cgroup-v2 CPU counters from ``cpu.stat``."""
        if not self.enabled or self.root is None or tag not in self._made:
            return None
        try:
            lines = (self.root / _sanitize(tag) / "cpu.stat").read_text().splitlines()
            return {
                key: int(value)
                for key, value in (line.split(maxsplit=1) for line in lines)
            }
        except (OSError, ValueError):
            return None

    def cpu_pressure(self, tag: str) -> Mapping[str, float] | None:
        """Per-step CPU pressure-stall averages (``cpu.pressure`` ``some`` line)."""
        if not self.enabled or self.root is None or tag not in self._made:
            return None
        try:
            lines = (self.root / _sanitize(tag) / "cpu.pressure").read_text().splitlines()
            some = next(line for line in lines if line.startswith("some "))
            values = dict(item.split("=", 1) for item in some.split()[1:])
            return {name: float(values[name]) for name in ("avg10", "avg60")}
        except (OSError, StopIteration, ValueError, KeyError):
            return None

    def kill(self, tag: str) -> bool:
        """SIGKILL the step's entire cgroup subtree (setsid escapees included).
        Returns True if the kill file was written. Never raises; a write failure
        emits a warning and returns False so the caller falls back to
        process-group kill."""
        if not self.enabled or self.root is None or tag not in self._made:
            return False
        killf = self.root / _sanitize(tag) / "cgroup.kill"
        try:
            killf.write_text("1")
            return True
        except OSError as exc:
            _warn(self._naming, f"step {tag}: cgroup.kill write failed ({exc}); "
                  "falling back to process-group kill for this step")
            return False

    def cleanup(self, tag: str) -> None:
        """Remove the step's (now-empty) child cgroup directory. Best-effort:
        ``rmdir`` fails with EBUSY if procs remain, which is FINE (expected) — the
        outer-scope stop flushes it at end of run, so this is NOT warned on."""
        if not self.enabled or self.root is None or tag not in self._made:
            return
        try:
            (self.root / _sanitize(tag)).rmdir()
        except OSError:
            pass
        self._made.discard(tag)

    def kill_all_remaining(self) -> int:
        """NORMAL-EXIT backstop: ``cgroup.kill`` + ``rmdir`` EVERY step child
        cgroup we ever created that still exists (catches a setsid orphan a step
        left behind). Does NOT touch the supervisor cgroup, so it never kills the
        runner — the exit code is preserved. Returns the count of step cgroups
        that still existed."""
        if not self.enabled or self.root is None:
            return 0
        n = 0
        # Scan the real directory, not just self._made: a step whose cleanup ran
        # may be gone, while a crashed step's dir lingers.
        try:
            children = [p for p in self.root.iterdir()
                        if p.is_dir() and p.name.startswith(_STEP_PREFIX)]
        except OSError:
            children = []
        for child in children:
            n += 1
            try:
                (child / "cgroup.kill").write_text("1")
            except OSError as exc:
                _warn(self._naming, f"backstop: cgroup.kill on {child.name} failed ({exc}); "
                      "a leftover orphan may survive the run")
            try:
                child.rmdir()
            except OSError:
                pass  # EBUSY: procs still dying; outer-scope stop will flush it
        return n


class NoopCgroups:
    """Advisory-only stand-in for a non-Linux / non-systemd / non-delegated host.

    :attr:`enabled` is always ``False``. Every method is a safe no-op so a caller
    can still use the full DAG/scheduler/logging core — teardown falls back to
    process-group kill and no per-step metrics are available. Structurally
    satisfies :class:`safe_ci_dag_runner.protocols.CgroupManager`."""

    enabled: bool = False

    def prepare_command(
        self,
        tag: str,
        cmd: str,
        mem_max: int | None = None,
        cpu_count: int | None = None,
    ) -> str:
        return cmd

    def kill(self, tag: str) -> bool:
        return False

    def cleanup(self, tag: str) -> None:
        return None

    def oom_kills(self, tag: str) -> int:
        return 0

    def peak_bytes(self, tag: str) -> int | None:
        return None

    def cpu_stats(self, tag: str) -> Mapping[str, int] | None:
        return None

    def cpu_pressure(self, tag: str) -> Mapping[str, float] | None:
        return None

    def thread_count(self, tag: str) -> int | None:
        return None

    def kill_all_remaining(self) -> int:
        return 0


if TYPE_CHECKING:
    # Compile-time guarantee that both concrete managers structurally satisfy the
    # CgroupManager protocol (matched signatures — no adapter shim needed).
    from safe_ci_dag_runner.protocols import CgroupManager

    def _assert_protocol_conformance() -> None:
        _real: CgroupManager = Cgroups()
        _noop: CgroupManager = NoopCgroups()
