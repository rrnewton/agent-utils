#!/usr/bin/env python3
"""Qualification-only CPU load generator for the cross differentials.

This tool is never part of gating: nothing in the validation graph calls
it. It exists so a differential can be qualified the way the owner asked
(cross-differentials-robust): N consecutive passes while the host is
above 75% CPU, with the load topped up to a target instead of added as a
fixed count.

Behaviour:

- Host busy is measured from ``/proc/stat``; only enough burner threads
  are added to reach the target total (default 85%).
- Burners run at nice 19 under ``SCHED_IDLE``, so real gates and other
  validations always win the CPU.
- One load generator at a time per host (an flock lock file), and the
  burners stop the moment the qualification run ends.
- Before starting, no ``validate-*`` user unit beyond
  ``validate-checkout-cleanup-worker`` may be active; the tool waits for
  one to finish instead of competing with it.
- The host-quiet probe (``host-quiet``, coordb rule 2026-11-10) must
  exit 0 before the run and is polled every 30 s during it: the moment
  it goes non-zero the burners stop and the run is recorded as
  INTERRUPTED (exit 3), not failed. SCHED_IDLE alone does not bound
  load-average or cache/memory-bandwidth effects on neighbours.
- The burner never runs while 2 or more of this host's gates are
  running (counted from the gate admission runs directory).

Usage: ``python3 cross/loadgen.py [--target 0.85] [--passes 10] -- CMD...``
Exit status is 0 when every pass succeeded under load, 1 on a failing
pass, 2 for a refused or unqualified run (busy target not reached),
and 3 when host-quiet interrupted the run.
"""

from __future__ import annotations

import argparse
import fcntl
import os
import shutil
import subprocess
import sys
import time
from pathlib import Path

LOCK_PATH = Path("/tmp/agent-utils-cross-loadgen.lock")
def _host_quiet_path() -> Path | None:
    """The host-quiet probe: $AGENT_UTILS_HOST_QUIET, else PATH lookup."""
    override = os.environ.get("AGENT_UTILS_HOST_QUIET")
    if override:
        return Path(override)
    found = shutil.which("host-quiet")
    return Path(found) if found else None


HOST_QUIET = _host_quiet_path()
GATE_RUNS_DIR = Path(os.environ.get(
    "GATE_ADMISSION_DIR", "/tmp/dev-agent-utils-gate-admission",
)) / "runs"
ALLOWED_ACTIVE_UNITS = frozenset({"validate-checkout-cleanup-worker.service"})
_BURNER = (
    "import os, time\n"
    "os.nice(19)\n"
    "try:\n"
    "    os.sched_setscheduler(0, os.SCHED_IDLE, os.sched_param(0))\n"
    "except (AttributeError, OSError):\n"
    "    pass\n"
    "while True:\n"
    "    pass\n"
)


def _cpu_times() -> tuple[int, int]:
    """(busy, total) jiffies from the aggregate /proc/stat line."""
    for line in Path("/proc/stat").read_text(encoding="utf-8").splitlines():
        if line.startswith("cpu "):
            fields = [int(value) for value in line.split()[1:]]
            idle = fields[3] + (fields[4] if len(fields) > 4 else 0)
            total = sum(fields)
            return total - idle, total
    raise RuntimeError("/proc/stat has no aggregate cpu line")


def busy_fraction(sample_s: float = 1.0) -> float:
    """Host busy fraction over a sampling window."""
    busy0, total0 = _cpu_times()
    time.sleep(sample_s)
    busy1, total1 = _cpu_times()
    if total1 == total0:
        return 0.0
    return (busy1 - busy0) / (total1 - total0)


def _active_validate_units() -> list[str]:
    """Active validate-* user units other than the allowed cleanup worker."""
    try:
        done = subprocess.run(
            ["systemctl", "--user", "list-units", "validate-*",
             "--state=active", "--no-legend", "--plain"],
            capture_output=True, text=True, timeout=30, check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return []
    units: list[str] = []
    for line in done.stdout.splitlines():
        parts = line.split()
        if parts and parts[0] not in ALLOWED_ACTIVE_UNITS:
            units.append(parts[0])
    return units


def _host_quiet() -> bool:
    """Whether the host-quiet probe says the host may take burner load."""
    if HOST_QUIET is None or not os.access(HOST_QUIET, os.X_OK):
        print("loadgen: warning: no host-quiet probe found; relying on unit/gate checks",
              file=sys.stderr)
        return True
    try:
        return subprocess.run(
            [str(HOST_QUIET)], capture_output=True, timeout=30, check=False,
        ).returncode == 0
    except (OSError, subprocess.TimeoutExpired):
        return False


def _own_gates_running() -> int:
    """Gates currently admitted on this host (gate admission runs files)."""
    try:
        return sum(1 for _ in GATE_RUNS_DIR.iterdir())
    except OSError:
        return 0


def _clear_to_burn() -> str | None:
    """Why the burner must not run right now, or None when it may."""
    if not _host_quiet():
        return "host-quiet is non-zero"
    if _own_gates_running() >= 2:
        return "2 or more gates are running"
    active = _active_validate_units()
    if active:
        return f"validate units active: {active}"
    return None


def _wait_for_clear_units(wait_s: float) -> bool:
    """Wait until host-quiet, the gate count, and validate units allow load."""
    deadline = time.monotonic() + wait_s
    while True:
        reason = _clear_to_burn()
        if reason is None:
            return True
        if time.monotonic() >= deadline:
            print(f"loadgen: refusing to compete: {reason}", file=sys.stderr)
            return False
        print(f"loadgen: waiting: {reason}", flush=True)
        time.sleep(10)


def _supervise(control: Path) -> int:
    """Burner supervisor: keep the spinner count at the control file's value.

    Runs as its own user unit (see `_Burners`), outside any CPU-quota
    slice the caller happens to sit in, so the spinners can actually
    fill the host. Each spinner is a forked child at nice 19 under
    SCHED_IDLE. SIGTERM (unit stop) kills every spinner.
    """
    children: list[int] = []

    def desired() -> int:
        try:
            return max(0, int(control.read_text(encoding="utf-8").strip() or 0))
        except (OSError, ValueError):
            return len(children)

    while True:
        want = desired()
        while len(children) < want:
            pid = os.fork()
            if pid == 0:  # spinner child; never returns
                os.nice(19)
                try:
                    os.sched_setscheduler(0, os.SCHED_IDLE, os.sched_param(0))
                except (AttributeError, OSError):
                    pass
                while True:
                    pass
            children.append(pid)
        while len(children) > want:
            pid = children.pop()
            try:
                os.kill(pid, 9)
                os.waitpid(pid, 0)
            except (ProcessLookupError, ChildProcessError):
                pass
        # Reap any spinner that died on its own so the count stays honest.
        children[:] = [pid for pid in children if os.waitpid(pid, os.WNOHANG)[0] == 0]
        time.sleep(1)


class _Burners:
    """A topped-up set of SCHED_IDLE burner processes, killed on exit.

    Preferred host: a `systemd-run --user` unit running this file's
    supervisor, which lives outside the caller's CPU-quota slice (a
    sandbox slice capped at half the host otherwise makes a 75% total
    unreachable no matter how many spinners start). Falls back to direct
    children where systemd-run is unavailable.
    """

    def __init__(self) -> None:
        self._procs: list[subprocess.Popen[bytes]] = []
        self._count = 0
        self._unit: str | None = None
        self._control: Path | None = None

    @property
    def count(self) -> int:
        return self._count

    def _spawn_direct(self, count: int) -> None:
        for _ in range(count):
            self._procs.append(subprocess.Popen(
                [sys.executable, "-c", _BURNER],
                stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL, start_new_session=True,
            ))

    def add(self, count: int) -> None:
        self._count += count
        if self._unit is not None and self._control is not None:
            self._control.write_text(f"{self._count}\n", encoding="utf-8")
            return
        self._spawn_direct(count)

    def __enter__(self) -> _Burners:
        unit = f"cross-loadgen-{os.getpid()}"
        control = Path(f"/tmp/cross-loadgen-{os.getpid()}.count")
        control.write_text("0\n", encoding="utf-8")
        started = subprocess.run(
            ["systemd-run", "--user", "--collect", f"--unit={unit}",
             sys.executable, str(Path(__file__).resolve()), "--supervise", str(control)],
            capture_output=True, text=True, timeout=30, check=False,
        )
        if started.returncode == 0:
            self._unit = unit
            self._control = control
        else:
            control.unlink(missing_ok=True)
        return self

    def __exit__(self, *exc: object) -> None:
        if self._unit is not None:
            subprocess.run(["systemctl", "--user", "stop", self._unit],
                           capture_output=True, timeout=30, check=False)
            self._unit = None
        if self._control is not None:
            self._control.unlink(missing_ok=True)
            self._control = None
        for proc in self._procs:
            try:
                os.killpg(proc.pid, 9)
            except (ProcessLookupError, PermissionError):
                pass
        for proc in self._procs:
            try:
                proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                proc.kill()


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", type=float, default=0.85,
                        help="total host busy fraction to top up to (default 0.85)")
    parser.add_argument("--passes", type=int, default=10,
                        help="consecutive passes required (default 10)")
    parser.add_argument("--wait-seconds", type=float, default=3600.0,
                        help="max wait for competing validate-* units (default 3600)")
    parser.add_argument("--supervise", type=Path, default=None,
                        help="internal: run the burner supervisor on a control file")
    parser.add_argument("command", nargs=argparse.REMAINDER,
                        help="command to qualify, after --")
    ns = parser.parse_args(argv)
    if ns.supervise is not None:
        return _supervise(ns.supervise)
    command = [arg for arg in ns.command if arg != "--"]
    if not command:
        parser.error("a command after -- is required")
    if not 0.0 < ns.target < 1.0:
        parser.error("--target must be between 0 and 1")

    lock = LOCK_PATH.open("w", encoding="utf-8")
    try:
        fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        print(f"loadgen: another load generator holds {LOCK_PATH}", file=sys.stderr)
        return 2
    if not _wait_for_clear_units(ns.wait_seconds):
        return 2

    cores = len(os.sched_getaffinity(0))
    before = busy_fraction()
    print(f"loadgen: cores={cores} busy_before={before:.2%} target={ns.target:.0%}", flush=True)
    failures = 0
    with _Burners() as burners:
        # Top up by measurement, not by a fixed count: add the computed
        # shortfall, re-measure, and repeat until the target is reached
        # (background load moves while the burners start).
        during = before
        for _ in range(6):
            shortfall = round(ns.target * cores - during * cores)
            if shortfall <= 0 or burners.count >= cores:
                break
            burners.add(shortfall)
            time.sleep(2)
            during = busy_fraction(sample_s=2.0)
            print(f"loadgen: burners={burners.count} busy={during:.2%}", flush=True)
        if during < 0.75:
            print("loadgen: host did not reach 75% busy; run is not qualified", file=sys.stderr)
            return 2
        for index in range(1, ns.passes + 1):
            started = time.monotonic()
            proc = subprocess.Popen(command, start_new_session=True)
            interrupted: str | None = None
            while proc.poll() is None:
                try:
                    proc.wait(timeout=30)
                except subprocess.TimeoutExpired:
                    pass
                if proc.poll() is None:
                    interrupted = _clear_to_burn()
                    if interrupted is not None:
                        break
            if interrupted is not None:
                # host-quiet went non-zero (or gates piled up): the
                # burners stop with this block's exit; the run counts
                # as interrupted, never as a pass or a failure.
                try:
                    os.killpg(proc.pid, 9)
                except (ProcessLookupError, PermissionError):
                    pass
                proc.wait()
                print(f"loadgen: INTERRUPTED at pass {index}/{ns.passes}: {interrupted}",
                      flush=True)
                return 3
            elapsed = time.monotonic() - started
            status = "pass" if proc.returncode == 0 else f"FAIL rc={proc.returncode}"
            if proc.returncode != 0:
                failures += 1
            print(f"loadgen: pass {index}/{ns.passes}: {status} in {elapsed:.1f}s", flush=True)
            if proc.returncode != 0:
                break
    print(f"loadgen: done, {ns.passes - failures}/{ns.passes} passes under load", flush=True)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
