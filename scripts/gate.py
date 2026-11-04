#!/usr/bin/env python3
"""Admission control for validation gates: counting slots plus an exclusive class.

Drop-in replacement for the one-gate-at-a-time flock line:

    scripts/gate --base origin/main --keep-going
    scripts/gate --exclusive -- python3 vibe-talk/tests/read_aloud_blocks_browser.py

Shared gates run up to --slots (default 2) concurrently; each gate is already
boxed by the validation scheduler's own resource caps, so concurrency here is
admission, not scheduling. An --exclusive run waits until nothing else is
running and blocks new admissions until it finishes, so load-sensitive steps
(timing-sensitive browser walks, solo shard reruns) never share the box.

Waiting callers print their queue position and elapsed wait. Every finished
run appends one JSON line to the state history for gates/hour and wait-time
measurement (`scripts/gate status`). Admission state lives outside the
repository (default /tmp/dev-agent-utils-gate-admission, override with
$GATE_ADMISSION_DIR); tickets and runs are files, reaped when their pid dies,
so a killed lane never holds a slot forever.

An admitted run's children see $GATE_ADMISSION_RUN_ID. A gate invoked from
inside an admitted run (validate.py's gate-exclusive phase) releases the
parent's slot before queueing: the parent is blocked on that call, so holding
its slot would deadlock, and the nested call must still wait for — and block —
every other gate as a fresh exclusive ticket.
"""
from __future__ import annotations

import argparse
import datetime as dt
import fcntl
import json
import os
import re
import subprocess
import sys
import time
import uuid
from collections import Counter
from pathlib import Path

DEFAULT_DIR = Path("/tmp/dev-agent-utils-gate-admission")

#: Durable per-run ledger (#827): /tmp history dies with every cleanup, taking the
#: full-gate pass rate with it, so finished runs also append here, outside /tmp.
#: Override with $GATE_LEDGER or --ledger.
DEFAULT_LEDGER = Path("~/.local/state/agent-utils/gate-ledger.jsonl")

_FAILED_STEP = re.compile(r"\[([^\]]+)\] ✗ FAIL")
_PASSED_STEP = re.compile(r"\[([^\]]+)\] ✓ PASS")
_VERDICT = re.compile(r"validate: (OK|PARTIAL|FAILED)")


def state_dir() -> Path:
    return Path(os.environ.get("GATE_ADMISSION_DIR") or DEFAULT_DIR)


def ledger_path(explicit: str | None = None) -> Path:
    return Path(explicit or os.environ.get("GATE_LEDGER") or DEFAULT_LEDGER).expanduser()


def _utc(unix: float) -> str:
    return dt.datetime.fromtimestamp(unix, dt.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")


def _git_rev_parse(ref: str) -> str | None:
    try:
        done = subprocess.run(
            ["git", "rev-parse", ref], capture_output=True, text=True, timeout=10
        )
        return done.stdout.strip() if done.returncode == 0 else None
    except Exception:
        return None


def _alive(pid: object) -> bool:
    try:
        os.kill(int(str(pid)), 0)
        return True
    except (ProcessLookupError, ValueError, TypeError):
        return False


class Admission:
    def __init__(self, root: Path, slots: int) -> None:
        self.root = root
        self.slots = slots
        (root / "queue").mkdir(parents=True, exist_ok=True)
        (root / "runs").mkdir(parents=True, exist_ok=True)
        self._lock = (root / "admission.lock").open("a+")

    def _locked(self) -> None:
        fcntl.flock(self._lock, fcntl.LOCK_EX)

    def _unlocked(self) -> None:
        fcntl.flock(self._lock, fcntl.LOCK_UN)

    @staticmethod
    def _read(path: Path) -> dict[str, object] | None:
        try:
            data = json.loads(path.read_text())
            return data if isinstance(data, dict) else None
        except Exception:
            return None

    def _reap(self) -> None:
        for folder in ("queue", "runs"):
            for path in (self.root / folder).glob("*.json"):
                data = self._read(path)
                if data is None or not _alive(data.get("pid")):
                    path.unlink(missing_ok=True)

    def tickets(self) -> list[tuple[Path, dict[str, object]]]:
        out = []
        for path in sorted((self.root / "queue").glob("*.json")):
            data = self._read(path)
            if data is not None:
                out.append((path, data))
        return out

    def runs(self) -> list[dict[str, object]]:
        out = []
        for path in sorted((self.root / "runs").glob("*.json")):
            data = self._read(path)
            if data is not None:
                out.append(data)
        return out

    def enqueue(self, cls: str, command: list[str]) -> tuple[str, Path]:
        run_id = f"{time.time_ns()}-{uuid.uuid4().hex[:8]}"
        ticket: dict[str, object] = {
            "id": run_id, "pid": os.getpid(), "class": cls,
            "queued_at": time.time(), "command": command,
        }
        self._locked()
        try:
            self._reap()
            path = self.root / "queue" / f"{run_id}.json"
            path.write_text(json.dumps(ticket))
        finally:
            self._unlocked()
        return run_id, path

    def try_grant(self, run_id: str, cls: str) -> bool:
        """Grant admission if FIFO order and the class rules allow it."""
        self._locked()
        try:
            self._reap()
            tickets = self.tickets()
            ids = [str(data["id"]) for _, data in tickets]
            if run_id not in ids:
                return False
            ahead = tickets[: ids.index(run_id)]
            runs = self.runs()
            if cls == "exclusive":
                if ahead or runs:
                    return False
            else:
                if any(data.get("class") == "exclusive" for _, data in ahead):
                    return False
                if any(run.get("class") == "exclusive" for run in runs):
                    return False
                if len(runs) >= self.slots:
                    return False
            for path, data in tickets:
                if str(data["id"]) == run_id:
                    data["started_at"] = time.time()
                    (self.root / "runs" / path.name).write_text(json.dumps(data))
                    path.unlink()
                    return True
            return False
        finally:
            self._unlocked()

    def position(self, run_id: str) -> tuple[int, int]:
        self._locked()
        try:
            self._reap()
            tickets = self.tickets()
            ids = [str(data["id"]) for _, data in tickets]
            pos = ids.index(run_id) + 1 if run_id in ids else 0
            return pos, len(self.runs())
        finally:
            self._unlocked()

    def finish(self, run_id: str) -> None:
        self._locked()
        try:
            (self.root / "runs" / f"{run_id}.json").unlink(missing_ok=True)
        finally:
            self._unlocked()

    def record(self, event: dict[str, object]) -> None:
        with (self.root / "history.jsonl").open("a") as fh:
            fh.write(json.dumps(event, sort_keys=True) + "\n")

    def history(self) -> list[dict[str, object]]:
        path = self.root / "history.jsonl"
        if not path.exists():
            return []
        out: list[dict[str, object]] = []
        for line in path.read_text(errors="replace").splitlines():
            try:
                row = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(row, dict):
                out.append(row)
        return out


def run_admitted(args: argparse.Namespace, command: list[str]) -> int:
    adm = Admission(state_dir(), args.slots)
    # A gate invoked from inside an admitted gate (validate.py running its
    # gate-exclusive steps) must not queue behind its own parent: the parent
    # process is blocked waiting for this call and will never finish. Release
    # the parent's slot first; this call then queues as a fresh top-level
    # ticket, so an exclusive phase still waits for every other gate and
    # blocks new ones, with no deadlock.
    parent_run = os.environ.get("GATE_ADMISSION_RUN_ID")
    if parent_run:
        adm.finish(parent_run)
    run_id, _ = adm.enqueue(args.cls, command)
    queued_at = time.time()
    last_pos = -1
    while not adm.try_grant(run_id, args.cls):
        if args.wait_timeout and time.time() - queued_at > args.wait_timeout:
            adm.finish(run_id)
            (state_dir() / "queue" / f"{run_id}.json").unlink(missing_ok=True)
            print(f"gate: gave up after {time.time() - queued_at:.0f}s in queue", flush=True)
            return 75
        pos, running = adm.position(run_id)
        if pos != last_pos:
            print(f"gate: queued at position {pos} ({running} running, class={args.cls})", flush=True)
            last_pos = pos
        time.sleep(1.0)
    waited = time.time() - queued_at
    print(f"gate: admitted after {waited:.0f}s (class={args.cls})", flush=True)
    started = time.time()
    rc = 1
    passed_steps: set[str] = set()
    failed_steps: set[str] = set()
    verdict_word: str | None = None
    try:
        proc = subprocess.Popen(
            command,
            env={**os.environ, "GATE_ADMISSION_RUN_ID": run_id},
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            errors="replace",
            bufsize=1,
        )
        assert proc.stdout is not None
        for line in proc.stdout:
            sys.stdout.write(line)
            match = _FAILED_STEP.search(line)
            if match:
                failed_steps.add(match.group(1))
            match = _PASSED_STEP.search(line)
            if match:
                passed_steps.add(match.group(1))
            match = _VERDICT.search(line)
            if match:
                verdict_word = match.group(1)
        rc = proc.wait()
    finally:
        finished = time.time()
        adm.finish(run_id)
        adm.record({"id": run_id, "class": args.cls, "command": command,
                    "queued_at": queued_at, "started_at": started,
                    "finished_at": finished, "wait_s": round(waited, 2),
                    "run_s": round(finished - started, 2), "exit_code": rc})
        _record_ledger(args, command, started, finished, rc,
                       passed_steps, failed_steps, verdict_word)
    return rc


def _record_ledger(
    args: argparse.Namespace,
    command: list[str],
    started: float,
    finished: float,
    rc: int,
    passed_steps: set[str],
    failed_steps: set[str],
    verdict_word: str | None,
) -> None:
    """Append one durable ledger line for the finished run (#827). Best effort:
    a ledger failure must never change the gate's own result."""

    try:
        base: str | None = None
        if "--base" in command:
            base = command[command.index("--base") + 1]
        verdict = {"OK": "pass", "PARTIAL": "partial", "FAILED": "fail"}.get(
            verdict_word or "", "pass" if rc == 0 else "fail"
        )
        kind = "exclusive-rerun" if args.cls == "exclusive" else "gate"
        line = {
            "started_utc": _utc(started),
            "finished_utc": _utc(finished),
            "base": base,
            "base_sha": _git_rev_parse(base) if base else None,
            "head_sha": _git_rev_parse("HEAD"),
            "selection": "selected" if "--selected" in " ".join(command) else "full",
            "kind": kind,
            "steps": len(passed_steps) + len(failed_steps),
            "failed_steps": sorted(failed_steps),
            "rerun_passed": (rc == 0) if kind == "exclusive-rerun" else None,
            "verdict": verdict,
            "exit_code": rc,
            "log_path": None,
        }
        path = ledger_path(getattr(args, "ledger", None))
        path.parent.mkdir(parents=True, exist_ok=True)
        with path.open("a") as fh:
            fh.write(json.dumps(line, sort_keys=True) + "\n")
    except Exception:
        pass


def show_status(args: argparse.Namespace) -> int:
    adm = Admission(state_dir(), args.slots)
    adm._locked()
    try:
        adm._reap()
        queue = [(str(d["id"]), str(d["class"])) for _, d in adm.tickets()]
        runs = [(str(d["id"]), str(d.get("class"))) for d in adm.runs()]
    finally:
        adm._unlocked()
    rows = adm.history()
    cutoff = time.time() - 24 * 3600
    recent = [r for r in rows if float(str(r.get("finished_at", 0))) >= cutoff]
    waits = sorted(float(str(r["wait_s"])) for r in recent if "wait_s" in r)
    print(f"gate status: {len(runs)} running {runs}, {len(queue)} queued {queue}")
    print(f"gate status: {len(recent)} finished in 24h "
          f"({len(recent) / 24.0:.1f}/hour), "
          f"median wait {(waits[len(waits) // 2] if waits else 0):.0f}s, "
          f"max wait {(waits[-1] if waits else 0):.0f}s")
    return 0


def _parse_since(text: str) -> float:
    match = re.fullmatch(r"(\d+)([hd])", text)
    if match:
        hours = int(match.group(1)) * (24 if match.group(2) == "d" else 1)
        return time.time() - hours * 3600
    return dt.datetime.fromisoformat(text.replace("Z", "+00:00")).timestamp()


def show_stats(args: argparse.Namespace) -> int:
    path = ledger_path(getattr(args, "ledger", None))
    rows: list[dict[str, object]] = []
    if path.exists():
        for raw in path.read_text(errors="replace").splitlines():
            try:
                row = json.loads(raw)
            except json.JSONDecodeError:
                continue
            if isinstance(row, dict):
                rows.append(row)
    since = _parse_since(args.since) if args.since else 0.0
    rows = [
        row for row in rows
        if dt.datetime.fromisoformat(
            str(row.get("finished_utc", "1970-01-01T00:00:00Z")).replace("Z", "+00:00")
        ).timestamp() >= since
    ]
    gates = [row for row in rows if row.get("kind") == "gate"]
    reruns = [row for row in rows if row.get("kind") == "exclusive-rerun"]
    passed = sum(1 for row in gates if row.get("verdict") == "pass")
    partial = sum(1 for row in gates if row.get("verdict") == "partial")
    rate = f"{100.0 * (passed + partial) / len(gates):.1f}%" if gates else "n/a"
    failures: Counter[str] = Counter()
    for row in gates:
        steps = row.get("failed_steps")
        if isinstance(steps, list):
            failures.update(str(step) for step in steps)
    print(f"gate stats: {len(gates)} first-run gate(s), {passed} pass, {partial} partial, "
          f"{len(gates) - passed - partial} fail; first-run pass rate {rate} "
          f"({args.since or 'all time'}, ledger {path})")
    print(f"gate stats: {sum(1 for r in reruns if r.get('rerun_passed'))}/{len(reruns)} "
          "exclusive rerun(s) passed")
    for step, count in failures.most_common(10):
        print(f"gate stats: failing step {step}: {count}")
    return 0


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    parser = argparse.ArgumentParser(prog="gate", description="Gate admission: counting slots plus an exclusive class.")
    parser.add_argument("--slots", type=int, default=int(os.environ.get("GATE_SLOTS", "2")),
                        help="concurrent shared gates (default 2, or $GATE_SLOTS)")
    parser.add_argument("--exclusive", dest="cls", action="store_const", const="exclusive", default="shared",
                        help="run alone: wait for an empty box and block new admissions")
    parser.add_argument("--wait-timeout", type=float, default=0.0, help="give up queueing after N seconds (0 = wait forever)")
    parser.add_argument("--ledger", default=None,
                        help="durable ledger path (default ~/.local/state/agent-utils/gate-ledger.jsonl, or $GATE_LEDGER)")
    if argv[:1] == ["status"]:
        return show_status(parser.parse_args([]))
    if argv[:1] == ["stats"]:
        stats_parser = argparse.ArgumentParser(prog="gate stats")
        stats_parser.add_argument("--since", default=None,
                                  help="window: Nh, Nd, or an ISO date/time (default all time)")
        stats_parser.add_argument("--ledger", default=None, help="ledger path (see --ledger)")
        return show_stats(stats_parser.parse_args(argv[1:]))
    if "--" in argv:
        cut = argv.index("--")
        args = parser.parse_args(argv[:cut])
        command = argv[cut + 1:]
    else:
        # No explicit command: gate flags are consumed here, the rest belong
        # to validate.py (the drop-in replacement for the flock line).
        args, rest = parser.parse_known_args(argv)
        command = [sys.executable, str(Path(__file__).with_name("validate.py")), *rest]
    if not command:
        parser.error("no command to run")
    return run_admitted(args, command)


if __name__ == "__main__":
    raise SystemExit(main())
