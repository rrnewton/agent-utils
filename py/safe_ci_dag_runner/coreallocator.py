"""Stateful, IRQ-aware core allocator: hand out K distinct *clean* cores per box.

This exists because a cgroup ``cpu.max`` QUOTA and a CPU-core PIN are different things
and only one produces what a scheduling-contention experiment needs. A quota throttles
*total* CPU consumption but leaves the task movable across every core; two tasks under
tight quotas usually get spread onto different cores and never contend. To reproduce
single-core scheduling contention the workload and its competing load must be *resident
on the same core* — that is PINNING (``sched_setaffinity``), not quota. hermit-ci's
1-core hang experiment needs the second; the DAG runner previously offered only the
first (see the task report: no ``cpuset``/affinity write existed in either engine).

Three properties the caller cannot get from quota, provided here:

* **Stateful, cross-process leasing.** The allocator is the central bookkeeper of which
  of the host's cores are handed out *right now*, across independent runner invocations,
  via a flock-guarded JSON registry. Two boxes never silently share a core — a shared
  core would confound every measurement taken in either, and neither would know.
* **Confound avoidance.** A core that services device IRQs (network especially), or that
  hosts a pinned non-per-cpu kernel thread, or CPU0 (the conventional timer/RCU sink),
  carries pre-existing interference: a measurement taken there is contaminated before it
  starts. The allocator PREFERS clean cores and, when only confounded cores remain,
  hands them out ANNOTATED with the reason — never silently.
* **Release on teardown, including abnormal exit.** Abandonment is the *common* exit
  path here (agent recycle, the 120s tool cap, detached runs outliving their launcher),
  so leases self-heal without a daemon: every ``acquire`` first drops any lease whose
  owner PID is dead (lazy PID-liveness reclamation). This is lease-row bookkeeping — a
  distinct, real need — NOT the process-reaper whose premise was refuted (there is no
  stranded-worker class to reap; there IS a leaked-lease-row class that degrades the
  registry to "no cores available").

The caller asks for K; the allocator picks WHICH. Callers hand-picking core numbers is
exactly how you land on a busy or IRQ-loaded core.

No Silent Failure note: the ``/proc`` confound readers are pure best-effort and degrade
to an explicit empty/annotated result a caller can see, never a hidden skip. The one
place that *must* be loud is exhaustion — :class:`CoreExhausted` is raised rather than
quietly returning fewer cores than requested or reusing a leased one.
"""

from __future__ import annotations

import argparse
import errno
import fcntl
import json
import os
import re
import subprocess
import sys
from collections.abc import Callable, Iterable, Mapping, Sequence
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path
from typing import IO, Final, Iterator

__all__ = [
    "CoreConfound",
    "CoreLease",
    "CoreExhausted",
    "CoreAllocator",
    "parse_interrupts",
    "scan_confounds",
    "pin_current_process",
    "main",
]

#: Device-IRQ description substrings that mark a genuine *network interface*. The owner
#: called these out ("network IRQs especially"): a NIC takes frequent, bursty interrupts
#: that directly perturb any latency/scheduling measurement pinned to its target core.
#: Deliberately NARROW — storage (nvme/ahci) and USB (xhci) are high-rate too but are
#: classified as plain ``dev-irq`` (lower weight), so a true NIC core always ranks worst.
_NET_IRQ_RE: Final = re.compile(
    r"\b(eth\d|eno\d|ens\d|enp\d+s|em\d|mlx\d|mlx5|mlx4|ena\b|bnxt|ixgbe|i40e|ice-|igb|"
    r"virtio\d*-net|vmxnet|bnx2|tg3|be2net|qede|nfp)",
    re.IGNORECASE,
)

#: A core with a device-IRQ count strictly above this is treated as an IRQ target. 0 = any
#: activity confounds (the conservative default a clean-core guarantee wants); the CLI can
#: raise it for hosts where irqbalance sprays tiny counts across every core.
DEFAULT_IRQ_COUNT_THRESHOLD: Final = 0

#: CPU0 is, by strong convention, the default sink for the timer tick, RCU callbacks and
#: unpinned IRQs on most Linux hosts. Always treated as confounded.
_CPU0_REASON: Final = "cpu0-timer-sink"


@dataclass(frozen=True)
class CoreConfound:
    """Why one core is a poor measurement target, or empty ``reasons`` if it is clean."""

    core: int
    reasons: tuple[str, ...]

    @property
    def is_clean(self) -> bool:
        return not self.reasons

    @property
    def is_network(self) -> bool:
        return any(r.startswith("net-irq:") for r in self.reasons)

    @property
    def severity(self) -> int:
        """Lower is better. Clean=0; a plain device IRQ or kthread=1 each; a network IRQ=3
        each (weighted worst). Used only to order the *fallback* pick when no clean core is
        free, so the least-contaminated confounded core goes first."""
        s = 0
        for r in self.reasons:
            if r.startswith("net-irq:"):
                s += 3
            else:
                s += 1
        return s


class CoreExhausted(RuntimeError):
    """Raised when fewer than K cores can be leased — loud, never a silent short return."""


def parse_interrupts(text: str) -> dict[int, tuple[str, ...]]:
    """Map core index -> device-IRQ reason tags active on it, from ``/proc/interrupts`` text.

    Only NUMBERED IRQ rows (real device interrupts) are considered. Architectural per-cpu
    summary rows (``LOC``, ``RES``, ``CAL``, ``TLB``, ``NMI``, ...) are skipped: those fire
    on every core by construction and would wrongly confound the whole machine.
    """
    lines = text.splitlines()
    if not lines:
        return {}
    ncpu = len(lines[0].split())  # header is "CPU0 CPU1 ... CPUn"
    if ncpu == 0:
        return {}
    acc: dict[int, list[str]] = {}
    for line in lines[1:]:
        parts = line.split()
        if not parts or not parts[0].endswith(":"):
            continue
        name = parts[0][:-1]
        if not name.isdigit():  # skip LOC/RES/NMI/... arch rows
            continue
        counts = parts[1 : 1 + ncpu]
        desc = " ".join(parts[1 + ncpu :])
        kind = "net-irq" if _NET_IRQ_RE.search(desc) else "dev-irq"
        for core, raw in enumerate(counts):
            try:
                n = int(raw)
            except ValueError:
                continue
            if n > DEFAULT_IRQ_COUNT_THRESHOLD:
                acc.setdefault(core, []).append(f"{kind}:{name}")
    return {core: tuple(tags) for core, tags in acc.items()}


def _scan_pinned_kthreads(proc_root: Path) -> dict[int, tuple[str, ...]]:
    """Best-effort: core -> reasons for pinned NON-per-cpu kernel threads resident on it.

    A kernel thread is a PID with an empty ``cmdline``. Per-cpu helpers (``ksoftirqd/7``,
    ``migration/7``) are pinned to their own core by design — their ``comm`` ends in the
    core index — and are NOT counted (they live on every core; counting them would exclude
    the whole machine). Only a kthread pinned to a single core that is NOT its natural
    per-cpu home is a genuine, avoidable confound.
    """
    acc: dict[int, list[str]] = {}
    try:
        pids = [p.name for p in proc_root.iterdir() if p.name.isdigit()]
    except OSError:
        return {}
    for pid in pids:
        base = proc_root / pid
        try:
            if (base / "cmdline").read_bytes().strip(b"\x00"):
                continue  # has argv => userspace process, not a kernel thread
            comm = (base / "comm").read_text().strip()
            allowed = _parse_cpu_list(_status_field(base / "status", "Cpus_allowed_list"))
        except OSError:
            continue
        if allowed is None or len(allowed) != 1:
            continue  # unpinned or spread kthread — not a single-core confound
        (core,) = tuple(allowed)
        if _percpu_home(comm) == core:
            continue  # natural per-cpu home: ksoftirqd/N, migration/N, kworker/N:M[-suffix]
        acc.setdefault(core, []).append(f"kthread:{comm}")
    return {core: tuple(tags) for core, tags in acc.items()}


def _percpu_home(comm: str) -> int | None:
    """Core index a per-cpu kernel thread names itself after, or ``None``.

    Per-cpu helpers encode their home core in the segment after the last ``/``:
    ``ksoftirqd/7`` and ``migration/7`` (bare index), but also ``kworker/7:1`` and
    ``kworker/7:1-events`` (index followed by ``:worker`` and an optional ``-suffix``).
    The bare-``endswith`` test missed the kworker forms — the most common per-cpu
    threads — and wrongly flagged their cores as confounds. Unbound rescuer kworkers
    (``kworker/R-kblockd``) have no leading integer and return ``None`` (they are also
    spread across cores, so the single-core pin filter excludes them anyway)."""
    seg = comm.rsplit("/", 1)[-1]
    m = re.match(r"(\d+)", seg)
    return int(m.group(1)) if m is not None else None


def _status_field(status_path: Path, field: str) -> str | None:
    try:
        for line in status_path.read_text().splitlines():
            if line.startswith(field + ":"):
                return line.split(":", 1)[1].strip()
    except OSError:
        return None
    return None


def _parse_cpu_list(spec: str | None) -> frozenset[int] | None:
    """Parse a Linux cpu-list like ``0-3,7,9-10`` into a set, or ``None`` if unparseable."""
    if not spec:
        return None
    out: set[int] = set()
    for chunk in spec.split(","):
        chunk = chunk.strip()
        if not chunk:
            continue
        if "-" in chunk:
            lo_s, hi_s = chunk.split("-", 1)
            try:
                lo, hi = int(lo_s), int(hi_s)
            except ValueError:
                return None
            out.update(range(lo, hi + 1))
        else:
            try:
                out.add(int(chunk))
            except ValueError:
                return None
    return frozenset(out)


def scan_confounds(
    cores: Iterable[int],
    *,
    interrupts_text: str | None = None,
    proc_root: Path | None = None,
    include_kthreads: bool = True,
) -> dict[int, CoreConfound]:
    """Build the confound verdict for every core in ``cores``.

    Readers are injectable so tests feed synthetic ``/proc`` content; when a reader is not
    supplied the live host is read best-effort (a missing/unreadable source contributes no
    reasons rather than raising).
    """
    if interrupts_text is None:
        interrupts_text = _read_text_or_empty(Path("/proc/interrupts"))
    irq = parse_interrupts(interrupts_text)
    kthreads: dict[int, tuple[str, ...]] = {}
    if include_kthreads:
        kthreads = _scan_pinned_kthreads(proc_root or Path("/proc"))
    verdict: dict[int, CoreConfound] = {}
    for core in cores:
        reasons: list[str] = []
        if core == 0:
            reasons.append(_CPU0_REASON)
        reasons.extend(irq.get(core, ()))
        reasons.extend(kthreads.get(core, ()))
        verdict[core] = CoreConfound(core=core, reasons=tuple(reasons))
    return verdict


def _read_text_or_empty(path: Path) -> str:
    try:
        return path.read_text()
    except OSError:
        return ""


@dataclass(frozen=True)
class CoreLease:
    """A granted lease: the specific cores, and which of them are (regrettably) confounded."""

    lease_id: str
    cores: tuple[int, ...]
    confounded: tuple[int, ...]

    @property
    def all_clean(self) -> bool:
        return not self.confounded


def pin_current_process(cores: Iterable[int]) -> None:
    """Pin this process (and, by inheritance, its future children) to ``cores``."""
    os.sched_setaffinity(0, set(cores))


def _pid_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except OSError as exc:
        return exc.errno == errno.EPERM  # exists but not ours => alive
    return True


class CoreAllocator:
    """Central, cross-process core leaser backed by a flock-guarded JSON registry.

    All hosts' runners sharing one ``state_dir`` see one registry, so concurrent boxes get
    disjoint cores. The universe of leasable cores defaults to this process's own affinity
    mask (``sched_getaffinity``) — you can never pin outside it anyway.
    """

    def __init__(
        self,
        *,
        cores: Sequence[int] | None = None,
        state_dir: Path | None = None,
        confounds: Mapping[int, CoreConfound] | None = None,
        pid: int | None = None,
    ) -> None:
        self._cores: tuple[int, ...] = tuple(
            sorted(cores) if cores is not None else sorted(os.sched_getaffinity(0))
        )
        self._dir = state_dir or _default_state_dir()
        self._dir.mkdir(parents=True, exist_ok=True)
        self._state_path = self._dir / "leases.json"
        self._lock_path = self._dir / "leases.lock"
        self._pid = pid if pid is not None else os.getpid()
        self._confounds = (
            dict(confounds) if confounds is not None else scan_confounds(self._cores)
        )

    # -- registry I/O (always under the flock) ------------------------------------- #

    @contextmanager
    def _locked(self) -> Iterator[None]:
        fd = os.open(self._lock_path, os.O_CREAT | os.O_RDWR, 0o600)
        try:
            fcntl.flock(fd, fcntl.LOCK_EX)
            yield
        finally:
            fcntl.flock(fd, fcntl.LOCK_UN)
            os.close(fd)

    def _load(self) -> dict[int, dict[str, object]]:
        try:
            raw = json.loads(self._state_path.read_text())
        except (OSError, ValueError):
            return {}
        held = raw.get("cores") if isinstance(raw, dict) else None
        if not isinstance(held, dict):
            return {}
        out: dict[int, dict[str, object]] = {}
        for core_s, rec in held.items():
            if not isinstance(rec, dict):
                continue
            try:
                core = int(core_s)
            except ValueError:
                continue
            pid = rec.get("pid")
            lease_id = rec.get("lease_id")
            if isinstance(pid, int) and isinstance(lease_id, str):
                out[core] = {"pid": pid, "lease_id": lease_id}
        return out

    def _store(self, held: Mapping[int, dict[str, object]]) -> None:
        payload = {"cores": {str(c): rec for c, rec in held.items()}}
        tmp = self._state_path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(payload, sort_keys=True))
        os.replace(tmp, self._state_path)

    @staticmethod
    def _reclaim_dead(held: dict[int, dict[str, object]]) -> dict[int, dict[str, object]]:
        alive: dict[int, dict[str, object]] = {}
        for core, rec in held.items():
            pid = rec.get("pid")
            if isinstance(pid, int) and _pid_alive(pid):
                alive[core] = rec
        return alive

    # -- public API ---------------------------------------------------------------- #

    def acquire(self, k: int, *, lease_id: str | None = None) -> CoreLease:
        """Lease K distinct cores, preferring clean ones. Raises :class:`CoreExhausted`."""
        if k < 1:
            raise ValueError(f"k must be >= 1, got {k}")
        lid = lease_id or f"{self._pid}:{os.urandom(6).hex()}"
        with self._locked():
            held = self._reclaim_dead(self._load())
            free = [c for c in self._cores if c not in held]
            ranked = sorted(
                free, key=lambda c: (self._confounds[c].severity, c)
            )  # clean (severity 0) first, then least-confounded, then by index
            if len(ranked) < k:
                raise CoreExhausted(
                    f"requested {k} cores; only {len(ranked)} free of "
                    f"{len(self._cores)} (held={len(held)})"
                )
            picked = ranked[:k]
            for c in picked:
                held[c] = {"pid": self._pid, "lease_id": lid}
            self._store(held)
        confounded = tuple(c for c in picked if not self._confounds[c].is_clean)
        return CoreLease(lease_id=lid, cores=tuple(picked), confounded=confounded)

    def release(self, lease: CoreLease) -> None:
        self.release_lease_id(lease.lease_id)

    def release_lease_id(self, lease_id: str) -> None:
        with self._locked():
            held = self._load()
            kept = {c: rec for c, rec in held.items() if rec.get("lease_id") != lease_id}
            self._store(kept)

    def confound_report(self) -> list[CoreConfound]:
        return [self._confounds[c] for c in self._cores]


def _default_state_dir() -> Path:
    """Host-local, per-uid registry directory shared by every runner on the host."""
    xdg = os.environ.get("XDG_RUNTIME_DIR")
    if xdg and os.path.isdir(xdg):
        return Path(xdg) / "safe-ci-core-leases"
    return Path("/tmp") / f"safe-ci-core-leases-{os.getuid()}"


# --------------------------------------------------------------------------------- #
# CLI: `python -m safe_ci_dag_runner.coreallocator {scan,run}`                       #
# --------------------------------------------------------------------------------- #


def _cmd_scan(ns: argparse.Namespace, out: IO[str]) -> int:
    alloc = CoreAllocator()
    report = alloc.confound_report()
    clean = [c.core for c in report if c.is_clean]
    obj = {
        "total": len(report),
        "clean_count": len(clean),
        "clean": clean,
        "confounded": {
            str(c.core): list(c.reasons) for c in report if not c.is_clean
        },
    }
    json.dump(obj, out, sort_keys=True)
    out.write("\n")
    return 0


def _cmd_run(ns: argparse.Namespace, out: IO[str], err: IO[str]) -> int:
    if not ns.command:
        err.write("coreallocator run: a command after `--` is required\n")
        return 2
    alloc = CoreAllocator()
    try:
        lease = alloc.acquire(ns.k)
    except CoreExhausted as exc:
        err.write(f"coreallocator run: {exc}\n")
        return 3
    try:
        note = "" if lease.all_clean else f" (CONFOUNDED: {list(lease.confounded)})"
        err.write(f"coreallocator: leased cores {list(lease.cores)}{note}; pinning + exec\n")
        proc = subprocess.Popen(  # noqa: S603 - argv passed through verbatim, no shell
            list(ns.command),
            preexec_fn=lambda: os.sched_setaffinity(0, set(lease.cores)),
        )
        return proc.wait()
    finally:
        alloc.release(lease)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="coreallocator",
        description="Stateful IRQ-aware core allocator: lease K distinct clean cores and pin.",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_scan = sub.add_parser("scan", help="print the host confound map (which cores are clean)")
    p_scan.set_defaults(fn="scan")

    p_run = sub.add_parser(
        "run", help="lease K clean cores, pin, and exec a command pinned to them"
    )
    p_run.add_argument("--k", type=int, default=1, help="how many cores to lease (default: 1)")
    p_run.add_argument("command", nargs=argparse.REMAINDER, help="the argv after `--`")
    p_run.set_defaults(fn="run")

    ns = parser.parse_args(argv)
    if ns.fn == "scan":
        return _cmd_scan(ns, sys.stdout)
    # argparse.REMAINDER keeps a leading "--"; drop it so it is not treated as argv[0].
    if ns.command and ns.command[0] == "--":
        ns.command = ns.command[1:]
    return _cmd_run(ns, sys.stdout, sys.stderr)


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
