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
"""
from __future__ import annotations

import argparse
import fcntl
import json
import os
import subprocess
import sys
import time
import uuid
from pathlib import Path

DEFAULT_DIR = Path("/tmp/dev-agent-utils-gate-admission")


def state_dir() -> Path:
    return Path(os.environ.get("GATE_ADMISSION_DIR") or DEFAULT_DIR)


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
    try:
        rc = subprocess.run(command).returncode
    finally:
        adm.finish(run_id)
        adm.record({"id": run_id, "class": args.cls, "command": command,
                    "queued_at": queued_at, "started_at": started,
                    "finished_at": time.time(), "wait_s": round(waited, 2),
                    "run_s": round(time.time() - started, 2), "exit_code": rc})
    return rc


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


def main(argv: list[str] | None = None) -> int:
    argv = list(sys.argv[1:] if argv is None else argv)
    parser = argparse.ArgumentParser(prog="gate", description="Gate admission: counting slots plus an exclusive class.")
    parser.add_argument("--slots", type=int, default=int(os.environ.get("GATE_SLOTS", "2")),
                        help="concurrent shared gates (default 2, or $GATE_SLOTS)")
    parser.add_argument("--exclusive", dest="cls", action="store_const", const="exclusive", default="shared",
                        help="run alone: wait for an empty box and block new admissions")
    parser.add_argument("--wait-timeout", type=float, default=0.0, help="give up queueing after N seconds (0 = wait forever)")
    if argv[:1] == ["status"]:
        return show_status(parser.parse_args([]))
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
