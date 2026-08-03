"""Memory-primary admission control for safe-ci-dag-runner boxes.

The runner never lets a caller silently contend for a saturated host. A caller
asks to run a box; admission returns one of three decisions, always with a
human-readable message decided against LIVE host memory plus the runner's own
reservation ledger:

* **GRANT** — the box fits now; a memory reservation is recorded and released on
  teardown (or reclaimed when the owning PID dies).
* **QUEUE** — the box *could* fit on an empty host but not right now; the caller
  is told its position and exactly which resource it is waiting on.
* **REFUSE** — the box can never fit (its request alone exceeds the whole-host
  box budget); the caller is told the number so it can ask for less.

**No Silent Contention.** The one behaviour admission must never exhibit is
admitting work that then fights for memory while the host looks healthy — the
failure mode behind the multi-hour tail, the phantom PMU cap, and the 64%-idle
"saturated" box. If the host cannot take another box, admission SAYS SO.

Memory is the binding constraint (it is what must be reclaimed); CPU
oversubscription is harmless, so cores never *block* admission here — the core
allocator (:mod:`safe_ci_dag_runner.coreallocator`) hands out distinct cores
separately, and admission only reports core pressure as advisory context.

Decisions compare a request against LIVE state, not a picked constant:

* ``budget`` — the ceiling on the SUM of the runner's own reservations, derived
  from ``MemTotal`` (a fraction, so a share is always left for the OS and other
  tenants) unless an explicit budget is supplied.
* ``MemAvailable`` — read live each call, so a host already loaded by other
  tenants (not just our boxes) still gates admission.

Both must hold to GRANT; whichever fails names the ``bound_by`` reason.
"""

from __future__ import annotations

import argparse
import errno
import fcntl
import json
import os
import sys
from contextlib import contextmanager
from dataclasses import dataclass
from enum import Enum
from pathlib import Path
from typing import Callable, Final, IO, Iterator, Mapping, Sequence

from safe_ci_dag_runner.coreallocator import _default_state_dir

__all__ = [
    "Verdict",
    "AdmissionDecision",
    "MemState",
    "read_meminfo",
    "Admitter",
    "DEFAULT_MEM_BUDGET_FRACTION",
    "DEFAULT_SAFETY_MARGIN_BYTES",
    "main",
]

DEFAULT_MEM_BUDGET_FRACTION: Final = 0.85
DEFAULT_SAFETY_MARGIN_BYTES: Final = 8 * 1024**3  # never admit into the last 8 GiB
_GIB: Final = float(1024**3)

MeminfoReader = Callable[[], str]


def _gib(n: int) -> str:
    return f"{n / _GIB:.1f} GiB"


class Verdict(str, Enum):
    GRANT = "grant"
    QUEUE = "queue"
    REFUSE = "refuse"


@dataclass(frozen=True)
class MemState:
    """Live host memory, in bytes."""

    total: int
    available: int


def read_meminfo(reader: MeminfoReader | None = None) -> MemState:
    """Parse ``MemTotal``/``MemAvailable`` (kiB in /proc/meminfo) into bytes.

    A missing field degrades to 0 rather than raising, so a stripped container
    ``/proc`` cannot crash admission — 0 available simply means "queue everything",
    which is the safe direction (never silently admit)."""
    text = reader() if reader is not None else _read_meminfo_live()
    total = _field_kib(text, "MemTotal") * 1024
    avail = _field_kib(text, "MemAvailable") * 1024
    return MemState(total=total, available=avail)


def _read_meminfo_live() -> str:
    try:
        return Path("/proc/meminfo").read_text()
    except OSError:
        return ""


def _field_kib(text: str, name: str) -> int:
    for line in text.splitlines():
        if line.startswith(name + ":"):
            parts = line.split()
            if len(parts) >= 2 and parts[1].isdigit():
                return int(parts[1])
    return 0


@dataclass(frozen=True)
class AdmissionDecision:
    """The runner's answer to one admission request — the message is what the caller sees."""

    verdict: Verdict
    message: str
    bound_by: str  # "" for GRANT; else "over-budget" | "reservation-budget" | "live-memory"
    reservation_id: str | None = None
    position: int | None = None  # 1-based queue position when QUEUEd

    @property
    def admitted(self) -> bool:
        return self.verdict is Verdict.GRANT


def _pid_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
    except OSError as exc:
        return exc.errno == errno.EPERM
    return True


class Admitter:
    """Memory-primary admit/queue/refuse over a flock-guarded reservation ledger.

    All runners sharing one ``state_dir`` see one ledger, so admission accounts for
    every concurrent box on the host, not just this process's. Dead owners' reservations
    are reclaimed lazily (PID liveness), so a crashed box never wedges the queue."""

    def __init__(
        self,
        *,
        state_dir: Path | None = None,
        meminfo_reader: MeminfoReader | None = None,
        mem_budget_bytes: int | None = None,
        budget_fraction: float = DEFAULT_MEM_BUDGET_FRACTION,
        safety_margin_bytes: int = DEFAULT_SAFETY_MARGIN_BYTES,
        pid: int | None = None,
    ) -> None:
        self._dir = state_dir or _default_state_dir()
        self._dir.mkdir(parents=True, exist_ok=True)
        self._state_path = self._dir / "admissions.json"
        self._lock_path = self._dir / "admissions.lock"
        self._reader = meminfo_reader
        self._explicit_budget = mem_budget_bytes
        self._fraction = budget_fraction
        self._margin = safety_margin_bytes
        self._pid = pid if pid is not None else os.getpid()

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

    def _load(self) -> list[dict[str, object]]:
        try:
            raw = json.loads(self._state_path.read_text())
        except (OSError, ValueError):
            return []
        recs = raw.get("reservations") if isinstance(raw, dict) else None
        if not isinstance(recs, list):
            return []
        out: list[dict[str, object]] = []
        for rec in recs:
            if not isinstance(rec, dict):
                continue
            rid, pid, mem = rec.get("reservation_id"), rec.get("pid"), rec.get("mem_bytes")
            if isinstance(rid, str) and isinstance(pid, int) and isinstance(mem, int):
                out.append({"reservation_id": rid, "pid": pid, "mem_bytes": mem,
                            "box": rec.get("box") if isinstance(rec.get("box"), str) else ""})
        return out

    def _store(self, recs: Sequence[Mapping[str, object]]) -> None:
        payload = {"reservations": [dict(r) for r in recs]}
        tmp = self._state_path.with_suffix(".json.tmp")
        tmp.write_text(json.dumps(payload, sort_keys=True))
        os.replace(tmp, self._state_path)

    @staticmethod
    def _rec_mem(rec: Mapping[str, object]) -> int:
        """Reserved bytes of one ledger record; 0 if malformed (``_load`` already validates)."""
        v = rec.get("mem_bytes")
        return v if isinstance(v, int) else 0

    @staticmethod
    def _reclaim_dead(recs: Sequence[dict[str, object]]) -> list[dict[str, object]]:
        alive: list[dict[str, object]] = []
        for rec in recs:
            pid = rec.get("pid")
            if isinstance(pid, int) and _pid_alive(pid):
                alive.append(rec)
        return alive

    def budget(self, mem: MemState) -> int:
        """Ceiling on the SUM of concurrent box reservations."""
        if self._explicit_budget is not None:
            return self._explicit_budget
        return int(mem.total * self._fraction)

    # -- public API ---------------------------------------------------------------- #

    def request(
        self,
        mem_bytes: int,
        *,
        box: str = "box",
        reservation_id: str | None = None,
    ) -> AdmissionDecision:
        """Decide GRANT / QUEUE / REFUSE for a box needing ``mem_bytes`` of memory.

        Live ``MemAvailable`` and the shared reservation ledger both gate the grant;
        the returned message states which term bound and by how much. A GRANT records
        the reservation (release it with :meth:`release`)."""
        if mem_bytes < 0:
            raise ValueError(f"mem_bytes must be >= 0, got {mem_bytes}")
        rid = reservation_id or f"{self._pid}:{os.urandom(6).hex()}"
        mem = read_meminfo(self._reader)
        budget = self.budget(mem)
        with self._locked():
            recs = self._reclaim_dead(self._load())
            reserved = sum(self._rec_mem(r) for r in recs)

            # REFUSE: cannot fit even on an otherwise-empty host.
            if mem_bytes > budget:
                return AdmissionDecision(
                    Verdict.REFUSE,
                    f"REFUSED: {box} needs {_gib(mem_bytes)} but the whole-host box "
                    f"memory budget is only {_gib(budget)} "
                    f"({int(self._fraction * 100)}% of {_gib(mem.total)}). Ask for less.",
                    bound_by="over-budget",
                )

            # QUEUE: our ledger would overcommit the budget.
            if reserved + mem_bytes > budget:
                return AdmissionDecision(
                    Verdict.QUEUE,
                    f"QUEUED (position {len(recs) + 1}): {box} needs {_gib(mem_bytes)}; "
                    f"{_gib(reserved)} already reserved by {len(recs)} box(es) leaves "
                    f"{_gib(max(0, budget - reserved))} of the {_gib(budget)} budget. "
                    "Waiting for a box to finish.",
                    bound_by="reservation-budget",
                    position=len(recs) + 1,
                )

            # QUEUE: the live host is too full right now (other tenants included).
            if mem.available < mem_bytes + self._margin:
                return AdmissionDecision(
                    Verdict.QUEUE,
                    f"QUEUED (position {len(recs) + 1}): {box} needs {_gib(mem_bytes)} "
                    f"+ {_gib(self._margin)} safety margin, but only {_gib(mem.available)} "
                    "is free on the host right now. Waiting for memory to free up.",
                    bound_by="live-memory",
                    position=len(recs) + 1,
                )

            # GRANT.
            recs.append({"reservation_id": rid, "pid": self._pid,
                         "mem_bytes": int(mem_bytes), "box": box})
            self._store(recs)
            headroom = min(budget - (reserved + mem_bytes), mem.available - mem_bytes)
            return AdmissionDecision(
                Verdict.GRANT,
                f"GRANTED: {box} reserved {_gib(mem_bytes)}; {_gib(max(0, headroom))} "
                "headroom remains (min of budget and live-free).",
                bound_by="",
                reservation_id=rid,
            )

    def release(self, reservation_id: str) -> None:
        with self._locked():
            recs = self._load()
            kept = [r for r in recs if r.get("reservation_id") != reservation_id]
            self._store(kept)

    def snapshot(self) -> dict[str, object]:
        """Live admission state for a status line (does not mutate the ledger)."""
        mem = read_meminfo(self._reader)
        with self._locked():
            recs = self._reclaim_dead(self._load())
            reserved = sum(self._rec_mem(r) for r in recs)
        budget = self.budget(mem)
        return {
            "mem_total_bytes": mem.total,
            "mem_available_bytes": mem.available,
            "budget_bytes": budget,
            "reserved_bytes": reserved,
            "active_boxes": len(recs),
            "budget_headroom_bytes": max(0, budget - reserved),
        }


# --------------------------------------------------------------------------------- #
# CLI: `python -m safe_ci_dag_runner.admission {request,status}`                     #
# --------------------------------------------------------------------------------- #

_EXIT_GRANT: Final = 0
_EXIT_QUEUE: Final = 75  # EX_TEMPFAIL — caller should retry
_EXIT_REFUSE: Final = 1


def _cmd_request(ns: argparse.Namespace, out: IO[str], err: IO[str]) -> int:
    adm = Admitter()
    decision = adm.request(int(ns.mem_gib * _GIB), box=ns.box)
    err.write(decision.message + "\n")
    json.dump(
        {"verdict": decision.verdict.value, "bound_by": decision.bound_by,
         "reservation_id": decision.reservation_id, "position": decision.position},
        out, sort_keys=True,
    )
    out.write("\n")
    if decision.verdict is Verdict.GRANT:
        return _EXIT_GRANT
    return _EXIT_QUEUE if decision.verdict is Verdict.QUEUE else _EXIT_REFUSE


def _cmd_status(ns: argparse.Namespace, out: IO[str]) -> int:
    json.dump(Admitter().snapshot(), out, sort_keys=True)
    out.write("\n")
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="admission",
        description="Memory-primary box admission: grant / queue / refuse vs live host state.",
    )
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_req = sub.add_parser("request", help="ask to admit a box needing N GiB of memory")
    p_req.add_argument("--mem-gib", type=float, required=True, help="box peak memory, GiB")
    p_req.add_argument("--box", default="box", help="label for messages (e.g. 'validate')")
    p_req.set_defaults(fn="request")

    p_stat = sub.add_parser("status", help="print live admission state (budget, reserved, free)")
    p_stat.set_defaults(fn="status")

    ns = parser.parse_args(argv)
    if ns.fn == "status":
        return _cmd_status(ns, sys.stdout)
    return _cmd_request(ns, sys.stdout, sys.stderr)


if __name__ == "__main__":  # pragma: no cover
    raise SystemExit(main())
