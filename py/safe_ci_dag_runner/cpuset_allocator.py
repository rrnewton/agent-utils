"""``cpuset-alloc`` — a STATEFUL cpuset allocator for benchmarks.

Pins a whole process tree (a benchmark command AND all its children — including a
NON-hermit native run, or a hermit ptrace tracer+tracee) to a set of RESERVED CPU
cores, so two concurrent benchmarks never land on the same core and contaminate
each other's timing. This is the apples-to-apples-comparison primitive: pinning a
native run and a hermit run to the SAME single-core box separates INSTRUMENTATION
cost from SEQUENTIALIZATION cost (hermit sequentializes threads; native does not).

Two layers, deliberately separated:

  * STATE — :mod:`safe_ci_dag_runner.reservation` is the durable, ``flock``-serialized
    core→holder ledger. ``acquire(K)`` hands out K DISJOINT cores under concurrency,
    reclaims cores whose holder crashed (dead-holder sweep), and releases on exit.
    The caller NEVER picks WHICH core — the allocator picks (the standing rule).

  * ENFORCEMENT — the pin is a HARD, inescapable, tree-wide cgroup ``cpuset.cpus``
    bound, applied by launching the command inside a transient
    ``systemd-run --user --scope -p AllowedCPUs=<reserved-set>``. This is the ONLY
    mechanism mutation-verified to hold in the 3pai agent sandbox: the agent's own
    scope delegates only ``io memory pids`` (no ``cpuset``), but a ``systemd --user``
    transient scope lands under ``app.slice`` where ``cpuset`` IS delegated, so
    systemd writes ``cpuset.cpus`` there. ``sched_setaffinity``/``taskset`` is NOT
    used: it is escapable — a child can widen its own affinity back onto an excluded
    core (mutation-verified 2026-08-04, experiments/cpuset-pin-mechanism-mutation-
    verified_20260804/). ``selftest`` re-runs that escape mutation and refuses to
    bless a soft bound.

Every pin records ``{cores: [...], count: K}`` — an assignment without WHICH cores
and HOW MANY is unqualified.

CLI::

    cpuset-alloc run --cores K [--tag T] -- CMD [ARGS...]   # pin+run, release on exit
    cpuset-alloc status                                      # held cores + reservations
    cpuset-alloc reclaim                                     # sweep dead holders
    cpuset-alloc selftest [--cores K]                        # mutation self-test (HARD?)
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from collections.abc import Sequence
from pathlib import Path

# Support the `py/bin/cpuset-alloc` symlink (run directly, no install): the package
# parent (py/) may not be on sys.path yet, so fix it up BEFORE the package import
# below. A normal `import`/console-script entry has __package__ set and skips this.
if __name__ == "__main__" and __package__ in (None, ""):  # pragma: no cover
    sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from safe_ci_dag_runner import reservation

PROG = "cpuset-alloc"


def _cpulist(cores: Sequence[int]) -> str:
    """Render cores as a Linux cpulist (comma form). ``AllowedCPUs=`` and
    ``cpuset.cpus`` both accept this; the kernel may re-render with ranges."""
    return ",".join(str(c) for c in sorted(cores))


def _systemd_run_available() -> bool:
    """True iff ``systemd-run --user --scope`` actually works here (a transient
    user scope can be created). A read of a flag is not evidence — this creates a
    throwaway scope and checks the exit code."""
    if not shutil.which("systemd-run"):
        return False
    try:
        r = subprocess.run(
            ["systemd-run", "--user", "--scope", "--quiet", "--collect", "true"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            timeout=15,
        )
    except (OSError, subprocess.TimeoutExpired):
        return False
    return r.returncode == 0


def _scope_argv(cores: Sequence[int], cmd: Sequence[str], *, tag: str = "") -> list[str]:
    """Build the ``systemd-run`` argv that boxes ``cmd``'s whole tree onto ``cores``.

    ``--scope`` runs the command SYNCHRONOUSLY in this session and propagates its
    exit code; ``-p AllowedCPUs=`` sets ``cpuset.cpus`` on the scope cgroup so the
    command and every descendant inherit the hard bound; ``--collect`` reaps the
    unit even if the command fails so a leaked scope does not linger."""
    argv = [
        "systemd-run",
        "--user",
        "--scope",
        "--collect",
        "--quiet",
        "-p",
        f"AllowedCPUs={_cpulist(cores)}",
    ]
    if tag:
        # A human-readable unit name aids `systemctl --user list-units` debugging.
        safe = "".join(c if c.isalnum() else "-" for c in tag)[:48]
        argv += ["--unit", f"cpuset-alloc-{safe}-{os.getpid()}"]
    argv.append("--")
    argv.extend(cmd)
    return argv


def cmd_run(args: argparse.Namespace) -> int:
    """``run --cores K -- CMD``: reserve K disjoint cores, then run CMD's whole
    tree HARD-pinned to them, releasing the reservation on exit (success or
    failure). Exit code == CMD's."""
    cmd: list[str] = list(args.argv_rest)
    if cmd and cmd[0] == "--":
        cmd = cmd[1:]
    if not cmd:
        print(f"{PROG}: run: no command given (use `-- CMD ARGS...`)", file=sys.stderr)
        return 2
    if not _systemd_run_available():
        print(
            f"{PROG}: run: `systemd-run --user --scope` is unavailable here, so a HARD "
            "cpuset pin cannot be applied. Refusing to run un-pinned (a soft "
            "sched_setaffinity bound is escapable and would silently contaminate the "
            "benchmark). Run on a host with a systemd user session.",
            file=sys.stderr,
        )
        return 3

    with reservation.reserve_cores(
        args.cores, tag=args.tag, sample_s=args.sample_s
    ) as cores:
        probe = _probe_hard_pin(cores)
        if probe.get("verdict") != "HARD":
            print(
                f"{PROG}: run: reserved cores could not be mutation-verified as a HARD "
                f"tree-wide pin; refusing to run: {json.dumps(probe, sort_keys=True)}",
                file=sys.stderr,
            )
            return 3
        # CONDITION CARRIED WITH THE VALUE: which cores, and how many.
        assignment = {"cores": cores, "count": len(cores)}
        print(
            f"{PROG}: reserved {json.dumps(assignment)}; "
            f"running whole tree pinned via AllowedCPUs={_cpulist(cores)}",
            file=sys.stderr,
        )
        argv = _scope_argv(cores, cmd, tag=args.tag)
        try:
            completed = subprocess.run(argv)
        except OSError as exc:
            print(f"{PROG}: run: failed to launch scope ({exc})", file=sys.stderr)
            return 3
    return completed.returncode


def cmd_status(args: argparse.Namespace) -> int:
    """``status``: print the live held cores and every current reservation
    (dead holders swept first, so the answer reflects reality not leaks)."""
    ledger = Path(args.ledger) if args.ledger else None
    held = reservation.held_cores(ledger=ledger)
    out = {"held_cores": held, "held_count": len(held)}
    print(json.dumps(out, indent=2))
    return 0


def cmd_reclaim(args: argparse.Namespace) -> int:
    """``reclaim``: sweep the ledger and drop records whose holder crashed;
    print the reclaimed records (the leaked-reservation recovery path)."""
    ledger = Path(args.ledger) if args.ledger else None
    dead = reservation.reclaim_dead(ledger=ledger)
    print(json.dumps({"reclaimed": dead, "reclaimed_count": len(dead)}, indent=2))
    return 0


# --------------------------------------------------------------------------- #
# selftest — VERIFY BY MUTATION, not by reading back what we wrote.            #
# --------------------------------------------------------------------------- #

# Inner script run INSIDE the pinned scope. It spawns a child, records the child's
# allowed CPUs, attempts to ESCAPE the child onto an excluded core, re-reads, then
# runs `count` busy spinners and reports which cores they actually executed on.
# Everything is emitted as one JSON line on stdout for the parent to parse.
_SELFTEST_INNER = r"""
import json, os, subprocess, sys, time

excluded = int(sys.argv[1])
count = int(sys.argv[2])

def allowed_list(pid):
    with open(f"/proc/{pid}/status") as fh:
        for line in fh:
            if line.startswith("Cpus_allowed_list:"):
                return line.split(":", 1)[1].strip()
    return ""

# --- NEGATIVE: a child cannot be moved to an excluded core ---
child = subprocess.Popen(["sleep", "5"])
time.sleep(0.2)
before = allowed_list(child.pid)
try:
    subprocess.run(["taskset", "-pc", str(excluded), str(child.pid)],
                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
except OSError:
    pass
after = allowed_list(child.pid)
child.terminate()

# --- POSITIVE: spinners actually use the assigned cores (not inertly one) ---
def spin_and_report():
    end = time.time() + 0.4
    x = 0
    while time.time() < end:
        x += 1
    # field 39 (index 38) of /proc/self/stat is the last CPU this task ran on.
    data = open(f"/proc/{os.getpid()}/stat").read()
    rest = data[data.rfind(")") + 2:].split()
    return int(rest[36])  # processor = field 39 -> index 36 after comm split

procs = []
for _ in range(count):
    procs.append(subprocess.Popen(
        [sys.executable, "-c",
         "import time\nend=time.time()+0.4\nx=0\nwhile time.time()<end:\n x+=1"]))
# read each spinner's processor while it runs
used = set()
time.sleep(0.15)
for p in procs:
    try:
        data = open(f"/proc/{p.pid}/stat").read()
        rest = data[data.rfind(")") + 2:].split()
        used.add(int(rest[36]))
    except OSError:
        pass
for p in procs:
    p.wait()

print(json.dumps({
    "child_allowed_before": before,
    "child_allowed_after": after,
    "escape_masked": before == after,
    "positive_cores_used": sorted(used),
}))
"""


def _probe_hard_pin(cores: Sequence[int]) -> dict[str, object]:
    """Mutation-probe an ``AllowedCPUs`` scope before trusting it.

    Some hosts accept the systemd property yet leave the child able to escape
    onto an excluded core. That is a soft/inert mechanism, not containment.
    """
    allowed = sorted(os.sched_getaffinity(0))
    excluded = next((core for core in allowed if core not in set(cores)), None)
    if excluded is None:
        return {"verdict": "UNTESTABLE", "reason": "no core outside the reserved set"}

    argv = _scope_argv(
        cores,
        [sys.executable, "-c", _SELFTEST_INNER, str(excluded), str(len(cores))],
    )
    try:
        completed = subprocess.run(argv, capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {"verdict": "ERROR", "reason": str(exc)}

    inner: dict[str, object] = {}
    for line in completed.stdout.splitlines():
        line = line.strip()
        if line.startswith("{"):
            try:
                inner = json.loads(line)
            except json.JSONDecodeError:
                pass
    if not inner:
        return {
            "verdict": "ERROR",
            "reason": "scope probe produced no result",
            "returncode": completed.returncode,
            "stderr": completed.stderr.strip(),
        }

    escape_masked = bool(inner.get("escape_masked", False))
    used_raw = inner.get("positive_cores_used", [])
    used = [int(core) for core in used_raw] if isinstance(used_raw, list) else []
    positive_ok = bool(used) and set(used).issubset(set(cores))
    verdict = "HARD" if (escape_masked and positive_ok) else "SOFT_OR_INERT"
    return {
        "verdict": verdict,
        "cores": list(cores),
        "count": len(cores),
        "excluded_core": excluded,
        "negative_escape_masked": escape_masked,
        "positive_cores_used": used,
        "positive_stayed_in_set": positive_ok,
        "inner": inner,
    }


def cmd_selftest(args: argparse.Namespace) -> int:
    """``selftest``: reserve K cores, hard-pin a scope onto them, and MUTATE —
    try to escape a pinned child onto a K+1th (excluded) core. A HARD bound masks
    the escape; a soft bound lets the child move. Also confirm (POSITIVE) the
    spinners actually run on the assigned cores, not inertly stuck on one. Prints
    a verdict carrying ``{cores, count}`` and exits 0 iff the bound is HARD."""
    if not _systemd_run_available():
        print(
            json.dumps(
                {
                    "verdict": "UNTESTABLE",
                    "reason": "systemd-run --user --scope unavailable",
                }
            )
        )
        return 3

    with reservation.reserve_cores(args.cores, tag="selftest", sample_s=args.sample_s) as cores:
        result = _probe_hard_pin(cores)
        print(json.dumps(result, indent=2))
        verdict = result.get("verdict")
        if verdict == "HARD":
            return 0
        return 1 if verdict == "SOFT_OR_INERT" else 3


def _build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(
        prog=PROG,
        description="Stateful cpuset allocator: reserve DISJOINT cores and HARD-pin "
        "a process tree to them (systemd AllowedCPUs). For benchmark isolation.",
    )
    sub = p.add_subparsers(dest="command", metavar="<command>", required=True)

    run_p = sub.add_parser("run", help="reserve K cores and run CMD's whole tree pinned to them")
    run_p.add_argument("--cores", type=int, required=True, metavar="K", help="how many cores to reserve (allocator picks WHICH)")
    run_p.add_argument("--tag", default="", help="label for this reservation (debugging)")
    run_p.add_argument("--sample-s", type=float, default=0.3, help="/proc/stat idle-sampling window (s)")
    run_p.add_argument("argv_rest", nargs=argparse.REMAINDER, metavar="-- CMD [ARGS...]", help="the command to run pinned")
    run_p.set_defaults(func=cmd_run)

    st_p = sub.add_parser("status", help="print live held cores + reservations")
    st_p.add_argument("--ledger", default="", help="override ledger path (default: $XDG_RUNTIME_DIR)")
    st_p.set_defaults(func=cmd_status)

    rc_p = sub.add_parser("reclaim", help="sweep dead holders; print reclaimed records")
    rc_p.add_argument("--ledger", default="", help="override ledger path")
    rc_p.set_defaults(func=cmd_reclaim)

    stf_p = sub.add_parser("selftest", help="mutation self-test: is the pin HARD (inescapable)?")
    stf_p.add_argument("--cores", type=int, default=2, metavar="K", help="cores to reserve for the test (>=2 exercises the positive multi-core check)")
    stf_p.add_argument("--sample-s", type=float, default=0.3, help="/proc/stat idle-sampling window (s)")
    stf_p.set_defaults(func=cmd_selftest)

    return p


def main(argv: Sequence[str] | None = None) -> int:
    parser = _build_parser()
    args = parser.parse_args(argv)
    func = args.func
    result = func(args)
    return int(result)


if __name__ == "__main__":
    # Support the `py/bin/cpuset-alloc` symlink (run directly, no install): the
    # package parent (py/) may not be on sys.path yet. Mirrors __main__.py.
    _PKG_PARENT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    if _PKG_PARENT not in sys.path:
        sys.path.insert(0, _PKG_PARENT)
    raise SystemExit(main())
