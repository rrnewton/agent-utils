"""Runner-enforced resource-exclusivity admission: the ``solo_validate`` capability.

A ``validate`` invocation demands SOLO possession of the box: it is REFUSED ADMISSION while another
validate OR a benchmark harness holds the box. Possession is advertised as a small holder file under
a shared holders directory; liveness is proven PER-HOLDER (``/proc/<pid>`` exists), so a stale holder
left by a crashed process is ignored and best-effort unlinked. The predicate therefore binds refusal
to a *live* competing holder rather than to the mere presence of a file (Proxy-Binding: carry the
condition — the live pid — with the value).

The holder file format is two lines (``role=<role>`` / ``pid=<pid>``) so the Python and Rust engines
read each other's holders. MUST stay behaviorally identical to
``rs/safe-ci-dag-runner/src/admission.rs``.
"""

from __future__ import annotations

import os
from dataclasses import dataclass
from pathlib import Path

#: Exclusivity roles a runner invocation may declare. ``validate`` is the solo role.
VALIDATE = "validate"
BENCHMARK = "benchmark"
ROLES: tuple[str, ...] = (VALIDATE, BENCHMARK)


@dataclass(frozen=True)
class Holder:
    """A live process currently possessing the box."""

    role: str
    pid: int
    path: Path


def _pid_alive(pid: int) -> bool:
    """True iff ``pid`` names a live process (``/proc/<pid>`` exists), matching the Rust engine."""
    return pid > 0 and Path(f"/proc/{pid}").exists()


def scan_live_holders(holders_dir: Path, *, exclude_pid: int | None = None) -> list[Holder]:
    """Live holders in ``holders_dir``, sorted by ``(role, pid)``.

    A holder file whose pid is dead is SKIPPED (crash-safe) and best-effort unlinked. ``exclude_pid``
    drops the caller's own holder so a runner never refuses itself.
    """
    holders: list[Holder] = []
    try:
        entries = sorted(holders_dir.iterdir())
    except OSError:
        return holders
    for entry in entries:
        if entry.suffix != ".holder":
            continue
        role: str | None = None
        pid: int | None = None
        try:
            for line in entry.read_text().splitlines():
                if line.startswith("role="):
                    role = line[len("role="):].strip()
                elif line.startswith("pid="):
                    pid = int(line[len("pid="):].strip())
        except (OSError, ValueError):
            continue
        if role is None or pid is None:
            continue
        if not _pid_alive(pid):
            try:
                entry.unlink()
            except OSError:
                pass
            continue
        if exclude_pid is not None and pid == exclude_pid:
            continue
        holders.append(Holder(role=role, pid=pid, path=entry))
    holders.sort(key=lambda h: (h.role, h.pid))
    return holders


def solo_validate_refusal(role: str, holders: list[Holder]) -> str | None:
    """The exclusivity predicate. A refusal reason string when ``role`` must be REFUSED given the
    live ``holders``, else ``None`` (admit).

    * a ``validate`` node is SOLO: refused while ANY live foreign holder (validate or benchmark) holds
      the box;
    * a ``benchmark`` node is refused while a live ``validate`` holder holds the box (validate's solo
      claim wins);
    * any other role is unconstrained (admit).
    """
    if role == VALIDATE:
        blockers = list(holders)
    elif role == BENCHMARK:
        blockers = [h for h in holders if h.role == VALIDATE]
    else:
        return None
    if not blockers:
        return None
    who = ", ".join(f"{h.role}(pid {h.pid})" for h in blockers)
    return f"{role} refused admission: box held by {who}"


def acquire(holders_dir: Path, role: str, pid: int | None = None) -> Path:
    """Write this process's holder file and return its path. The caller MUST :func:`release` it."""
    pid = os.getpid() if pid is None else pid
    holders_dir.mkdir(parents=True, exist_ok=True)
    path = holders_dir / f"{role}.{pid}.holder"
    path.write_text(f"role={role}\npid={pid}\n")
    return path


def release(path: Path) -> None:
    """Best-effort remove a holder file created by :func:`acquire`."""
    try:
        path.unlink()
    except OSError:
        pass
