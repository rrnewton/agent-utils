"""Host-limits file: parsing, live admission sampling and decisions.

One module owns the host-limits file. The parser, sampler and
effective-value logic live here; nothing under ``scripts/`` parses
this file. No code in this module writes the host file: a host admin
installs one by hand, and readers only ever read it.

The file names host-wide ceilings and floors. A missing file is limp
mode, never a refusal: conservative built-ins apply, every run warns,
and only ``doctor`` reports the missing file as an error. Malformed
values fall back per key; the slot-filesystem floor falls back to the
stricter of its two bounds. Live admission is decided from measured
headroom at admit time, never by summing burst caps.
"""
from __future__ import annotations

import json
import math
import os
import random
import stat as statmod
import subprocess
import sys
import time
from dataclasses import dataclass, field
from pathlib import Path

if sys.version_info >= (3, 11):
    import tomllib
else:  # Python 3.10: the tomli package provides the same API
    import tomli as tomllib

GIB = 1024 ** 3
DEFAULT_FLOOR_BYTES = 50 * GIB
DEFAULT_HOST_LIMITS_PATH = Path("~/.local/etc/agent-utils/host-limits.toml")
ENV_HOST_LIMITS_CONFIG = "AGENT_UTILS_HOST_LIMITS_CONFIG"
DEFAULT_STATE_ROOT = Path("~/.local/state/agent-utils/local-ci-hub")

# Code constants, not file keys: retuning any of these is a reviewed
# code change informed by recorded samples, not a config tweak.
SAMPLE_WINDOW_S = 0.5
YOUNG_WINDOW_S = 120.0
YOUNG_THREADS = 20
CPU_FREE_FRACTION = 0.50

# Built-in limp-mode values. The memory and pids values are unmeasured
# built-ins pending first-install measurement; no measured dataset
# backs them, and a host-imposed breach of either maps to no result,
# never a failure.
BUILTIN_MAX_CONCURRENT = 1
BUILTIN_CLEANUP_PENDING_MAX = 1
BUILTIN_CPU_QUOTA_CPUS = 16
BUILTIN_PIDS_MAX = 16384

_KNOWN_KEYS = {
    "version",
    "admission.max_concurrent_validation_runs",
    "admission.cleanup_pending_max",
    "admission.slot_fs_min_free_bytes",
    "run_scope.cpu_quota_cpus",
    "run_scope.memory_max_bytes",
    "run_scope.pids_max",
    "paths.state_root",
}
# Which dotted keys each reader validates. A malformed key outside a
# reader's set does not change that reader's effective values.
_READER_KEYS = {
    "run": _KNOWN_KEYS,
    "sweep": {"version", "admission.cleanup_pending_max", "paths.state_root"},
    "doctor": _KNOWN_KEYS,
}


def _is_int(value: object) -> bool:
    return isinstance(value, int) and not isinstance(value, bool)


def host_limits_path(environ: dict[str, str] | None = None) -> Path:
    """The consulted host-limits path; the env hook selects a path only."""
    env = os.environ if environ is None else environ
    raw = env.get(ENV_HOST_LIMITS_CONFIG)
    return Path(raw).expanduser() if raw else DEFAULT_HOST_LIMITS_PATH.expanduser()


def mem_total_bytes(meminfo_text: str | None = None) -> int:
    """MemTotal in bytes from /proc/meminfo text (or the live file)."""
    if meminfo_text is None:
        try:
            meminfo_text = Path("/proc/meminfo").read_text(errors="replace")
        except OSError:
            return 0
    for line in meminfo_text.splitlines():
        if line.startswith("MemTotal:"):
            parts = line.split()
            try:
                return int(parts[1]) * 1024
            except (IndexError, ValueError):
                return 0
    return 0


def mem_available_bytes(meminfo_text: str | None = None) -> int:
    """MemAvailable in bytes, falling back to MemFree when absent."""
    if meminfo_text is None:
        try:
            meminfo_text = Path("/proc/meminfo").read_text(errors="replace")
        except OSError:
            return 0
    found: dict[str, int] = {}
    for line in meminfo_text.splitlines():
        parts = line.split()
        if len(parts) >= 2 and parts[0] in ("MemAvailable:", "MemFree:"):
            try:
                found[parts[0]] = int(parts[1]) * 1024
            except ValueError:
                pass
    return found.get("MemAvailable:", found.get("MemFree:", 0))


@dataclass
class EffectiveLimits:
    """Effective host limits after precedence and fallback rules."""

    path: Path
    file_present: bool
    version: int = 1
    max_concurrent_validation_runs: int = BUILTIN_MAX_CONCURRENT
    cleanup_pending_max: int = BUILTIN_CLEANUP_PENDING_MAX
    slot_fs_min_free_bytes: int = DEFAULT_FLOOR_BYTES
    cpu_quota_cpus: int = BUILTIN_CPU_QUOTA_CPUS
    memory_max_bytes: int = 0
    pids_max: int = BUILTIN_PIDS_MAX
    state_root: Path = field(default_factory=lambda: DEFAULT_STATE_ROOT.expanduser())
    sources: dict[str, str] = field(default_factory=dict)
    warnings: list[str] = field(default_factory=list)
    errors: list[str] = field(default_factory=list)
    unknown_keys: list[str] = field(default_factory=list)

    def as_dict(self) -> dict[str, object]:
        return {
            "path": str(self.path), "file_present": self.file_present,
            "version": self.version,
            "admission.max_concurrent_validation_runs": self.max_concurrent_validation_runs,
            "admission.cleanup_pending_max": self.cleanup_pending_max,
            "admission.slot_fs_min_free_bytes": self.slot_fs_min_free_bytes,
            "run_scope.cpu_quota_cpus": self.cpu_quota_cpus,
            "run_scope.memory_max_bytes": self.memory_max_bytes,
            "run_scope.pids_max": self.pids_max,
            "paths.state_root": str(self.state_root),
            "sources": dict(self.sources), "warnings": list(self.warnings),
            "errors": list(self.errors), "unknown_keys": list(self.unknown_keys),
        }


def _builtins(mem_total: int) -> dict[str, object]:
    return {
        "version": 1,
        "admission.max_concurrent_validation_runs": BUILTIN_MAX_CONCURRENT,
        "admission.cleanup_pending_max": BUILTIN_CLEANUP_PENDING_MAX,
        "admission.slot_fs_min_free_bytes": DEFAULT_FLOOR_BYTES,
        "run_scope.cpu_quota_cpus": BUILTIN_CPU_QUOTA_CPUS,
        "run_scope.memory_max_bytes": mem_total // 8 if mem_total > 0 else 0,
        "run_scope.pids_max": BUILTIN_PIDS_MAX,
        "paths.state_root": str(DEFAULT_STATE_ROOT.expanduser()),
    }


def _flatten(data: dict[str, object]) -> dict[str, object]:
    flat: dict[str, object] = {}
    if "version" in data:
        flat["version"] = data["version"]
    for table in ("admission", "run_scope", "paths"):
        sub = data.get(table)
        if isinstance(sub, dict):
            for key, value in sub.items():
                flat[f"{table}.{key}"] = value
        elif table in data:
            flat[table] = sub
    for key in data:
        if key not in ("version", "admission", "run_scope", "paths"):
            value = data[key]
            if isinstance(value, dict):
                for sub_key, sub_value in value.items():
                    flat[f"{key}.{sub_key}"] = sub_value
            else:
                flat[key] = value
    return flat


def _stricter_floor(slot_total_bytes: int | None) -> int:
    """Malformed-floor fallback: the stricter (larger) of the two bounds."""
    if slot_total_bytes is None or slot_total_bytes <= 0:
        return DEFAULT_FLOOR_BYTES
    return max(DEFAULT_FLOOR_BYTES, slot_total_bytes // 10)


def check_state_root(path: Path, uid: int | None = None) -> list[str]:
    """Problems with a state root that exists; empty means acceptable.

    A root that does not exist yet reports no problem: the runner
    creates it owned by the running user with mode 0700. An existing
    root must be owned by the running user, mode 0700, not a symlink,
    and on a filesystem where flock works (probed by the caller when
    it matters; the ownership, mode and symlink facts live here).
    """
    uid = os.getuid() if uid is None else uid
    try:
        info = path.lstat()
    except FileNotFoundError:
        return []
    except OSError as exc:
        return [f"state root {path} cannot be inspected: {exc}"]
    problems: list[str] = []
    if statmod.S_ISLNK(info.st_mode):
        problems.append(f"state root {path} is a symlink")
    if info.st_uid != uid:
        problems.append(f"state root {path} is not owned by the running user")
    if statmod.S_IMODE(info.st_mode) & 0o777 != 0o700:
        problems.append(f"state root {path} mode is not 0700")
    return problems


def load_effective(
    path: Path | None = None,
    *,
    reader: str = "run",
    environ: dict[str, str] | None = None,
    mem_total: int | None = None,
    slot_total_bytes: int | None = None,
) -> EffectiveLimits:
    """Load effective limits for one reader (run, sweep or doctor).

    Missing file: built-ins, a warning, never a refusal. Malformed
    values fall back alone to their built-ins, except the
    slot-filesystem floor, which falls back to the stricter of
    50 GiB and 10% of the slot filesystem. Unparseable TOML, an
    unusable file, or a bad version discards the whole file.
    """
    consulted = Path(path) if path is not None else host_limits_path(environ)
    total = mem_total_bytes() if mem_total is None else mem_total
    builtins = _builtins(total)
    eff = EffectiveLimits(path=consulted, file_present=False)
    eff.memory_max_bytes = int(builtins["run_scope.memory_max_bytes"])
    for key in _KNOWN_KEYS:
        eff.sources[key] = "built-in"
    wanted = _READER_KEYS.get(reader, _KNOWN_KEYS)

    if not consulted.exists():
        eff.warnings.append(
            f"missing host-limits file {consulted}; running on built-in limp limits")
        eff.errors.append(f"missing host-limits file: {consulted}")
        return eff
    eff.file_present = True
    try:
        info = consulted.lstat()
    except OSError as exc:
        eff.warnings.append(f"host-limits file {consulted} unusable ({exc}); built-ins apply")
        eff.errors.append(f"host-limits file {consulted} unusable: {exc}")
        return eff
    if (statmod.S_ISLNK(info.st_mode) or not statmod.S_ISREG(info.st_mode)
            or (statmod.S_IMODE(info.st_mode) & 0o022)):
        eff.warnings.append(
            f"host-limits file {consulted} is symlinked, not regular, or writable "
            "beyond its owner; built-ins apply")
        eff.errors.append(f"host-limits file {consulted} fails file checks")
        return eff
    try:
        data = tomllib.loads(consulted.read_text(encoding="utf-8"))
    except Exception as exc:
        eff.warnings.append(f"host-limits file {consulted} unparseable ({exc}); built-ins apply")
        eff.errors.append(f"host-limits file {consulted} unparseable: {exc}")
        return eff
    if not isinstance(data, dict):
        eff.warnings.append(f"host-limits file {consulted} is not a table; built-ins apply")
        eff.errors.append(f"host-limits file {consulted} is not a table")
        return eff
    flat = _flatten(data)
    eff.unknown_keys = sorted(k for k in flat if k not in _KNOWN_KEYS)
    for key in eff.unknown_keys:
        eff.warnings.append(f"unknown host-limits key {key} in {consulted}")
    version = flat.get("version")
    if not _is_int(version) or version != 1:
        eff.warnings.append(
            f"host-limits file {consulted} has absent or unsupported version "
            f"{version!r}; built-ins apply")
        eff.errors.append(f"host-limits file {consulted}: bad version {version!r}")
        return eff

    def take_int(key: str, minimum: int) -> None:
        default = int(builtins[key])  # type: ignore[arg-type]
        if key not in wanted:
            return
        value = flat.get(key)
        if value is None and key not in flat:
            setattr(eff, _attr(key), default)
            return
        if not _is_int(value) or int(value) < minimum:  # type: ignore[arg-type]
            eff.warnings.append(
                f"host-limits key {key} malformed ({value!r}) in {consulted}; "
                f"built-in {default} applies")
            eff.errors.append(f"host-limits {consulted}: key {key} malformed: {value!r}")
            setattr(eff, _attr(key), default)
            eff.sources[key] = "built-in (malformed fallback)"
            return
        setattr(eff, _attr(key), int(value))  # type: ignore[arg-type]
        eff.sources[key] = "file"

    take_int("admission.max_concurrent_validation_runs", 1)
    take_int("admission.cleanup_pending_max", 1)
    take_int("run_scope.cpu_quota_cpus", 1)
    take_int("run_scope.memory_max_bytes", 1)
    take_int("run_scope.pids_max", 1)

    if "admission.slot_fs_min_free_bytes" in wanted:
        value = flat.get("admission.slot_fs_min_free_bytes")
        if value is not None or "admission.slot_fs_min_free_bytes" in flat:
            if not _is_int(value) or int(value) < 0:  # type: ignore[arg-type]
                fallback = _stricter_floor(slot_total_bytes)
                eff.warnings.append(
                    f"host-limits key admission.slot_fs_min_free_bytes malformed "
                    f"({value!r}) in {consulted}; stricter fallback {fallback} applies")
                eff.errors.append(
                    f"host-limits {consulted}: key admission.slot_fs_min_free_bytes "
                    f"malformed: {value!r}")
                eff.slot_fs_min_free_bytes = fallback
                eff.sources["admission.slot_fs_min_free_bytes"] = "built-in (stricter fallback)"
            else:
                eff.slot_fs_min_free_bytes = int(value)  # type: ignore[arg-type]
                eff.sources["admission.slot_fs_min_free_bytes"] = "file"

    if "paths.state_root" in wanted:
        value = flat.get("paths.state_root")
        if value is not None or "paths.state_root" in flat:
            expanded = Path(str(value)).expanduser() if isinstance(value, str) else None
            if expanded is None or not expanded.is_absolute():
                eff.warnings.append(
                    f"host-limits key paths.state_root malformed ({value!r}) in "
                    f"{consulted}; built-in applies")
                eff.errors.append(
                    f"host-limits {consulted}: key paths.state_root malformed: {value!r}")
                eff.sources["paths.state_root"] = "built-in (malformed fallback)"
            else:
                problems = check_state_root(expanded)
                if problems:
                    eff.warnings.append(
                        f"host-limits paths.state_root {expanded} unusable "
                        f"({'; '.join(problems)}); built-in applies")
                    eff.errors.extend(
                        f"host-limits {consulted}: {p}" for p in problems)
                    eff.sources["paths.state_root"] = "built-in (malformed fallback)"
                else:
                    eff.state_root = expanded
                    eff.sources["paths.state_root"] = "file"
    return eff


_ATTR = {
    "admission.max_concurrent_validation_runs": "max_concurrent_validation_runs",
    "admission.cleanup_pending_max": "cleanup_pending_max",
    "run_scope.cpu_quota_cpus": "cpu_quota_cpus",
    "run_scope.memory_max_bytes": "memory_max_bytes",
    "run_scope.pids_max": "pids_max",
}


def _attr(key: str) -> str:
    return _ATTR[key]


# ---------------------------------------------------------------- sampler

def affinity_set() -> set[int]:
    """CPUs this process may run on; the set that gives N."""
    try:
        cpus = os.sched_getaffinity(0)  # type: ignore[attr-defined]
        if cpus:
            return set(cpus)
    except (AttributeError, OSError):
        pass
    count = os.cpu_count() or 1
    return set(range(count))


def parse_proc_stat(text: str) -> dict[int, tuple[int, ...]]:
    """Per-CPU jiffies by CPU index from /proc/stat text (cpuN lines only)."""
    out: dict[int, tuple[int, ...]] = {}
    for line in text.splitlines():
        parts = line.split()
        if not parts or not parts[0].startswith("cpu") or parts[0] == "cpu":
            continue
        try:
            index = int(parts[0][3:])
            out[index] = tuple(int(v) for v in parts[1:])
        except ValueError:
            continue
    return out


def idle_fraction_between(
    first: dict[int, tuple[int, ...]], second: dict[int, tuple[int, ...]],
    cpus: set[int],
) -> float | None:
    """Idle fraction over exactly `cpus`; None when unusable.

    Uses delta(idle + iowait) over delta(total) summed over the
    affinity set. CPUs outside the set never move the result, and
    the aggregate line is never consulted.
    """
    idle_delta = 0
    total_delta = 0
    for cpu in cpus:
        a = first.get(cpu)
        b = second.get(cpu)
        if a is None or b is None or len(a) < 5 or len(b) < 5:
            continue
        total_a, total_b = sum(a), sum(b)
        idle_a, idle_b = a[3] + a[4], b[3] + b[4]
        if total_b < total_a or idle_b < idle_a:
            continue
        total_delta += total_b - total_a
        idle_delta += idle_b - idle_a
    if total_delta <= 0:
        return None
    return min(1.0, max(0.0, idle_delta / total_delta))


@dataclass
class Headroom:
    """One sampled headroom reading with its provenance."""

    n: int
    idle_fraction: float
    h: int
    signal: str
    mem_available_bytes: int

    def as_dict(self) -> dict[str, object]:
        return {"n": self.n, "idle_fraction": round(self.idle_fraction, 6),
                "h": self.h, "signal": self.signal,
                "mem_available_bytes": self.mem_available_bytes}


def _read_text(path: str) -> str | None:
    try:
        return Path(path).read_text(errors="replace")
    except OSError:
        return None


ENV_STAT_SNAPSHOT_A = "AGENT_UTILS_HOST_LIMITS_STAT_A"
ENV_STAT_SNAPSHOT_B = "AGENT_UTILS_HOST_LIMITS_STAT_B"


def sample_headroom(
    *,
    cpus: set[int] | None = None,
    snapshots: tuple[str, str] | None = None,
    meminfo_text: str | None = None,
    loadavg_1min: float | None = None,
    sleep_fn: object = None,
    stat_texts: tuple[str | None, str | None] | None = None,
    environ: dict[str, str] | None = None,
) -> Headroom:
    """Sample headroom. Injected snapshots never sleep.

    Production reads /proc/stat twice over SAMPLE_WINDOW_S. Tests
    pass `snapshots` (two /proc/stat texts) or `stat_texts` with an
    injected load average, so no test waits on the real clock. The
    AGENT_UTILS_HOST_LIMITS_STAT_A/B hooks name snapshot files with
    the same injected texts for subprocess callers; they select
    paths only, never values, and are unset in production.
    """
    env = os.environ if environ is None else environ
    if snapshots is None and env.get(ENV_STAT_SNAPSHOT_A) and env.get(ENV_STAT_SNAPSHOT_B):
        try:
            snapshots = (Path(env[ENV_STAT_SNAPSHOT_A]).read_text(errors="replace"),
                         Path(env[ENV_STAT_SNAPSHOT_B]).read_text(errors="replace"))
        except OSError:
            snapshots = None
    cpus = affinity_set() if cpus is None else cpus
    n = max(1, len(cpus))
    mem_avail = mem_available_bytes(meminfo_text) if meminfo_text is not None \
        else mem_available_bytes()
    fraction: float | None = None
    signal = "proc-stat"
    if snapshots is not None:
        fraction = idle_fraction_between(
            parse_proc_stat(snapshots[0]), parse_proc_stat(snapshots[1]), cpus)
    else:
        texts = stat_texts
        if texts is None:
            first = _read_text("/proc/stat")
            sleeper = sleep_fn if callable(sleep_fn) else time.sleep
            sleeper(SAMPLE_WINDOW_S)  # type: ignore[operator]
            second = _read_text("/proc/stat")
            texts = (first, second)
        if texts[0] is not None and texts[1] is not None:
            fraction = idle_fraction_between(
                parse_proc_stat(texts[0]), parse_proc_stat(texts[1]), cpus)
    if fraction is None:
        load = loadavg_1min
        if load is None:
            try:
                load = os.getloadavg()[0]
            except (AttributeError, OSError):
                load = None
        if load is None:
            return Headroom(n=n, idle_fraction=0.0, h=0,
                            signal="none", mem_available_bytes=mem_avail)
        h = max(0, n - math.ceil(load))
        return Headroom(n=n, idle_fraction=h / n, h=h,
                        signal="loadavg-fallback", mem_available_bytes=mem_avail)
    return Headroom(n=n, idle_fraction=fraction, h=int(n * fraction),
                    signal=signal, mem_available_bytes=mem_avail)


# ------------------------------------------------------------------- disk

def statvfs_free(path: Path) -> tuple[int, int] | None:
    """(free bytes for unprivileged users, total bytes) or None."""
    try:
        vfs = os.statvfs(path)
    except OSError:
        return None
    return vfs.f_bavail * vfs.f_frsize, vfs.f_blocks * vfs.f_frsize


def measure_free_bytes(
    slot_root: Path,
    *,
    wrkslots_free: int | None = None,
    backing_free: int | None = None,
    qgroup_room: int | None = None,
    statvfs_fn: object = None,
) -> tuple[int, dict[str, object]]:
    """Free bytes F on the pre-slot path, with each component.

    The measured path is the existing directory under which a slot
    will be created, never the not-yet-created slot itself. A
    wrkslots-reported value is preferred for the slot filesystem.
    On image-backed storage a single statvfs reading can overstate
    room, so F is the minimum of every known component: the slot
    filesystem, the backing filesystem, and qgroup room.
    """
    components: dict[str, object] = {"measured_path": str(slot_root)}
    values: list[int] = []
    if wrkslots_free is not None:
        components["wrkslots_free_bytes"] = wrkslots_free
        values.append(wrkslots_free)
    else:
        measured = None
        if callable(statvfs_fn):
            measured = statvfs_fn(slot_root)  # type: ignore[operator]
        elif statvfs_free(slot_root) is not None:
            measured = statvfs_free(slot_root)
        if measured is not None:
            components["slot_fs_free_bytes"] = measured[0]
            components["slot_fs_total_bytes"] = measured[1]
            values.append(int(measured[0]))
    if backing_free is not None:
        components["backing_fs_free_bytes"] = backing_free
        values.append(backing_free)
    if qgroup_room is not None:
        components["qgroup_room_bytes"] = qgroup_room
        values.append(qgroup_room)
    return (min(values) if values else 0), components


def slot_total_bytes(slot_root: Path) -> int | None:
    measured = statvfs_free(slot_root)
    return measured[1] if measured is not None else None


# ------------------------------------------------------------- gate count

def gate_slots_path(environ: dict[str, str] | None = None) -> Path:
    env = os.environ if environ is None else environ
    raw = env.get("GATE_SLOTS_CONFIG")
    return Path(raw).expanduser() if raw else Path(
        "~/.local/etc/agent-utils/gate-slots.toml").expanduser()


def count_gate_runs(
    environ: dict[str, str] | None = None,
) -> tuple[int | str, int | None, str]:
    """Live gate runs and cap, read-only. Returns (G, G_cap, note).

    With the admission directory unset, G is the string "unknown"
    and informational only: it never blocks, charges or refuses.
    Counting confers no authority; nothing here writes gate state.
    """
    env = os.environ if environ is None else environ
    cap: int | None = None
    try:
        raw = tomllib.loads(gate_slots_path(env).read_text(encoding="utf-8"))
        value = raw.get("slots")
        if _is_int(value) and int(value) >= 1:  # type: ignore[arg-type]
            cap = int(value)  # type: ignore[arg-type]
    except Exception:
        cap = None
    raw_dir = env.get("GATE_ADMISSION_DIR")
    if not raw_dir:
        return "unknown", cap, "GATE_ADMISSION_DIR unset; informational only"
    runs_dir = Path(raw_dir).expanduser() / "runs"
    try:
        count = sum(1 for _ in runs_dir.glob("*.json"))
    except OSError:
        return "unknown", cap, "gate admission directory unreadable"
    return count, cap, f"counted read-only under {runs_dir}"


# -------------------------------------------------------------- admission

@dataclass
class AdmissionInputs:
    """Live counts and measurements for one admission decision."""

    v: int
    y: int
    p: int
    f: int
    headroom: Headroom

    def as_dict(self) -> dict[str, object]:
        return {"v": self.v, "y": self.y, "p": self.p, "f": self.f,
                "headroom": self.headroom.as_dict()}


@dataclass
class Decision:
    """One admission decision: pure in its sampled inputs."""

    admit: bool
    binding: str
    h_adj: int
    mem_adj: int

    def as_dict(self) -> dict[str, object]:
        return {"admit": self.admit, "binding": self.binding,
                "h_adj": self.h_adj, "mem_adj": self.mem_adj}


def decide(inputs: AdmissionInputs, limits: EffectiveLimits) -> Decision:
    """The exact admit rule. Live admission may only admit fewer
    than the hard ceiling, never more."""
    head = inputs.headroom
    h_adj = head.h - YOUNG_THREADS * inputs.y
    mem_adj = head.mem_available_bytes - limits.memory_max_bytes * inputs.y
    if inputs.v >= limits.max_concurrent_validation_runs:
        return Decision(False, "max_concurrent_validation_runs", h_adj, mem_adj)
    if inputs.p >= limits.cleanup_pending_max:
        return Decision(False, "cleanup_pending_max", h_adj, mem_adj)
    if inputs.f < limits.slot_fs_min_free_bytes:
        return Decision(False, "slot_fs_min_free_bytes", h_adj, mem_adj)
    if head.n <= 0 or (h_adj / head.n) < CPU_FREE_FRACTION:
        return Decision(False, "cpu_headroom", h_adj, mem_adj)
    if mem_adj < limits.memory_max_bytes:
        return Decision(False, "memory_headroom", h_adj, mem_adj)
    return Decision(True, "admitted", h_adj, mem_adj)


def clamp_request(requested: int | None, ceiling: int) -> tuple[int, str]:
    """Clamp a project or CLI request under a host ceiling."""
    if requested is None:
        return ceiling, "host"
    if requested > ceiling:
        return ceiling, "host"
    return requested, "instance"


# --------------------------------------------------------- holds / quiet
# Sweep deletions defer through the generic defer surface only: the
# gate-defer host file and whatever holds directories and quiet
# command it names. No consumer path or tool name appears here; a
# deployment points the generic keys at its own surface.

def defer_config_path(environ: dict[str, str] | None = None) -> Path:
    env = os.environ if environ is None else environ
    raw = env.get("GATE_DEFER_CONFIG")
    return Path(raw).expanduser() if raw else Path(
        "~/.local/etc/agent-utils/gate-defer.toml").expanduser()


def defer_settings(environ: dict[str, str] | None = None) -> dict[str, str]:
    """Holds directories and quiet command from the defer file.

    An absent file means no deferral signals. The same keys in the
    gate-slots file are a fallback, matching the gate's own rule.
    Environment hooks win per key, including empty (disabled).
    """
    env = os.environ if environ is None else environ
    settings = {"holds_dir": "", "shared_holds_dir": "~/.local/state/measurement-holds",
                "holds_root": "", "quiet_command": ""}
    parsed: dict[str, object] = {}
    try:
        parsed = tomllib.loads(defer_config_path(env).read_text(encoding="utf-8"))
    except FileNotFoundError:
        try:
            parsed = tomllib.loads(gate_slots_path(env).read_text(encoding="utf-8"))
        except Exception:
            parsed = {}
    except Exception:
        parsed = {}
    for key, name in (("defer_holds_dir", "holds_dir"),
                      ("defer_shared_holds_dir", "shared_holds_dir"),
                      ("defer_holds_root", "holds_root"),
                      ("defer_quiet_command", "quiet_command")):
        value = parsed.get(key)
        if isinstance(value, str):
            settings[name] = value.strip()
    for env_key, name in (("GATE_DEFER_HOLDS_DIR", "holds_dir"),
                          ("GATE_DEFER_SHARED_HOLDS_DIR", "shared_holds_dir"),
                          ("GATE_DEFER_HOLDS_ROOT", "holds_root"),
                          ("GATE_DEFER_QUIET_COMMAND", "quiet_command")):
        if env_key in env:
            settings[name] = env[env_key].strip()
    return settings


def _hold_active(declaration: Path, now_ts: float, shared: bool,
                 root: Path) -> str | None:
    """Blocker label when one hold declaration is in force.

    Fail closed: a malformed declaration, or a missing marker with a
    future end, defers. An expired hold never defers.
    """
    import datetime as _dt

    now = _dt.datetime.fromtimestamp(now_ts, _dt.timezone.utc)
    try:
        raw = json.loads(declaration.read_text(encoding="utf-8"))
        if not isinstance(raw, dict):
            raise ValueError("declaration is not an object")
        if shared and raw.get("schema_version") != 1:
            raise ValueError("schema_version must be 1")
        marker_raw = raw.get("marker_file", raw.get("status_file"))
        if not isinstance(marker_raw, str) or not marker_raw:
            raise ValueError("marker_file must name a file")
        cap_raw = raw.get("max_hold_seconds")
        if not _is_int(cap_raw) and not isinstance(cap_raw, float):
            raise ValueError("max_hold_seconds must be positive")
        cap = float(cap_raw)  # type: ignore[arg-type]
        if isinstance(cap_raw, bool) or cap <= 0:
            raise ValueError("max_hold_seconds must be positive")
        started: _dt.datetime | None = None
        started_raw = raw.get("started_at")
        if started_raw is not None:
            started = _dt.datetime.fromisoformat(
                str(started_raw).replace("Z", "+00:00"))
        elif shared:
            raise ValueError("started_at required")
    except (OSError, ValueError) as exc:
        return f"hold declaration {declaration}: {exc}"
    marker = Path(marker_raw).expanduser()
    if not marker.is_absolute():
        marker = root / marker
    window_start: _dt.datetime | None = None
    in_window = False
    ended = False
    try:
        lines = marker.read_text(encoding="utf-8", errors="replace").splitlines()
        lines_ok = True
    except OSError:
        lines, lines_ok = [], False
    if lines_ok:
        for line in lines:
            text = line.strip()
            if text.startswith("WINDOW-START"):
                try:
                    window_start = _dt.datetime.fromisoformat(
                        text[len("WINDOW-START"):].strip().replace("Z", "+00:00"))
                    in_window, ended = True, False
                except ValueError:
                    return f"hold {declaration.stem}: bad WINDOW-START line"
            elif text.startswith("WINDOW-END"):
                in_window = False
                ended = window_start is not None or started is not None
    else:
        expiry = (started + _dt.timedelta(seconds=cap)) if started else None
        if expiry is not None and now >= expiry:
            return None
        return f"hold {declaration.stem}: marker {marker} unreadable"
    if window_start is not None:
        started = window_start
    if ended and not in_window:
        return None
    if started is None:
        return None
    expiry = started + _dt.timedelta(seconds=cap)
    if now >= expiry or started > now:
        return None
    if not shared and window_start is not None and not in_window:
        return None
    if not shared and started_raw is None and window_start is None:
        return None
    return f"hold {declaration.stem} until {expiry:%Y-%m-%dT%H:%M:%SZ}"


def hold_blockers(now_ts: float, environ: dict[str, str] | None = None,
                  settings: dict[str, str] | None = None) -> list[str]:
    """Active measurement-hold blockers on the generic defer surface."""
    cfg = settings if settings is not None else defer_settings(environ)
    blockers: list[str] = []
    for key, shared in (("holds_dir", False), ("shared_holds_dir", True)):
        raw = cfg.get(key, "")
        if not raw:
            continue
        root = Path(raw).expanduser()
        base = Path(cfg["holds_root"]).expanduser() if cfg.get("holds_root") else root
        if not root.is_dir():
            continue
        for declaration in sorted(root.glob("*.json")):
            label = _hold_active(declaration, now_ts, shared, base)
            if label is not None:
                blockers.append(label)
    return blockers


def quiet_blocker(command: str, runner_fn: object = None) -> str | None:
    """Quiet-command blocker, or None when the host is quiet.

    Exit 0 means quiet. Any other exit, or a command that cannot
    run, defers: the check fails closed.
    """
    if not command:
        return None
    if callable(runner_fn):
        code = runner_fn(command)  # type: ignore[operator]
        return None if code == 0 else f"quiet command exited {code}"
    try:
        done = subprocess.run(["/bin/sh", "-c", command],
                              capture_output=True, text=True, timeout=30, check=False)
    except (OSError, subprocess.TimeoutExpired) as exc:
        return f"quiet command could not run: {exc}"
    return None if done.returncode == 0 else f"quiet command exited {done.returncode}"


def sweep_deferral(now_ts: float, environ: dict[str, str] | None = None,
                   quiet_runner: object = None) -> tuple[bool, list[str]]:
    """Whether sweep deletions must defer, and the sources saying so."""
    cfg = defer_settings(environ)
    sources = hold_blockers(now_ts, environ, cfg)
    quiet = quiet_blocker(cfg.get("quiet_command", ""), quiet_runner)
    if quiet is not None:
        sources.append(quiet)
    return bool(sources), sources


# ------------------------------------------------------------ box breach

def parse_events_oom(text: str) -> int:
    """oom_kill count from cgroup memory.events text (0 when absent)."""
    for line in text.splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[0] == "oom_kill":
            try:
                return int(parts[1])
            except ValueError:
                return 0
    return 0


def parse_events_max(text: str) -> int:
    """max count from cgroup pids.events text (0 when absent)."""
    for line in text.splitlines():
        parts = line.split()
        if len(parts) == 2 and parts[0] == "max":
            try:
                return int(parts[1])
            except ValueError:
                return 0
    return 0


def box_breach(memory_events: str | None = None,
               pids_events: str | None = None) -> str | None:
    """Host-imposed box breach kind, or None.

    A breach of the run's own memory or pids ceiling is the box
    doing the killing. Callers in result handling map it to no
    result; nothing here mints a failure.
    """
    if memory_events is not None and parse_events_oom(memory_events) > 0:
        return "memory"
    if pids_events is not None and parse_events_max(pids_events) > 0:
        return "pids"
    return None


def classify_outcome(*, validator_exit: int, breach: str | None,
                     wall_breached: bool, cpu_breached: bool) -> tuple[str, int]:
    """Map one finished run to (ledger outcome, component exit).

    Real test failure is exit 1. A CPU-budget breach is a timeout,
    also exit 1 with its own outcome. A wall-backstop breach and a
    host-imposed box breach are no result, exit 75, never a failure.
    This mapping lives only in this tool's result handling.
    """
    if breach is not None or wall_breached:
        return "no_result", 75
    if cpu_breached:
        return "timeout", 1
    if validator_exit == 0:
        return "pass", 0
    return "fail", 1


# ------------------------------------------------------------ event wait

def wait_for_admission(
    decide_fn: object,
    *,
    bound_s: float,
    now_fn: object = None,
    wait_event_fn: object = None,
    jitter_fn: object = None,
) -> tuple[object, bool]:
    """Wait on events to a caller-set bound; never sleep-poll.

    `decide_fn` re-samples and re-decides from scratch on each wake;
    a wake confers no right to admit. `wait_event_fn(timeout)` blocks
    until an event (a run finishing) or the timeout. With a zero
    bound this is a single attempt. Returns (decision, admitted).
    All clocks and waits are injectable so tests use fake time.
    """
    now = now_fn if callable(now_fn) else time.monotonic
    jitter = jitter_fn if callable(jitter_fn) else (
        lambda: random.uniform(0.0, 0.25))
    decision = decide_fn()  # type: ignore[operator]
    if decision.admit or bound_s <= 0:  # type: ignore[attr-defined]
        return decision, bool(decision.admit)  # type: ignore[attr-defined]
    deadline = now() + bound_s  # type: ignore[operator]
    while True:
        remaining = deadline - now()  # type: ignore[operator]
        if remaining <= 0:
            return decision, False
        if callable(wait_event_fn):
            wait_event_fn(remaining)  # type: ignore[operator]
        else:
            time.sleep(min(remaining, 0.05))
        pause = jitter()  # type: ignore[operator]
        if pause > 0:
            if callable(wait_event_fn):
                wait_event_fn(pause)  # type: ignore[operator]
            else:
                time.sleep(pause)
        decision = decide_fn()  # type: ignore[operator]
        if decision.admit:  # type: ignore[attr-defined]
            return decision, True
        if now() >= deadline:  # type: ignore[operator]
            return decision, False

