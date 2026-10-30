"""Run engine: admit, launch, list, stop validation runs on this machine."""
from __future__ import annotations

import fcntl
import json
import os
import subprocess
import uuid
from datetime import datetime, timezone
from pathlib import Path

from .config import Config
from .ledger import Ledger


def _handles(cfg: Config) -> Path:
    d = cfg.state_dir / "runs"
    d.mkdir(parents=True, exist_ok=True)
    return d


def _commit(root: Path) -> str:
    try:
        return subprocess.run(["git", "rev-parse", "HEAD"], cwd=root, capture_output=True, text=True, timeout=10).stdout.strip()
    except Exception:
        return ""


def start(cfg: Config, *, wait: bool = True, commit: str = "", cell: str = "") -> dict:
    """Admit and run the validate command, recording its ledger result."""
    handles = _handles(cfg)
    lock_fd = (handles / "admission.lock").open("a+")
    fcntl.flock(lock_fd, fcntl.LOCK_EX)  # machine-wide admission is atomic under this lock
    active = []
    for h in handles.glob("*.json"):
        try:
            data = json.loads(h.read_text())
        except Exception:
            continue
        if data.get("state") != "running":
            continue
        pid = data.get("pid")
        alive = False
        if pid:
            try:
                os.kill(int(pid), 0); alive = True
            except (ProcessLookupError, ValueError):
                alive = False
        if alive:
            active.append(h)
        else:
            data["state"] = "stale"  # reaped: a dead pid never holds a slot forever
            h.write_text(json.dumps(data, indent=1))
    if len(active) >= cfg.admission_slots:
        return {"schema": "local-ci-hub/run/v1", "state": "refused", "reason": f"admission full ({cfg.admission_slots} slots)"}
    run_id = f"run-{uuid.uuid4().hex[:12]}"
    commit = commit or _commit(cfg.root) or "unknown"
    log = cfg.state_dir / f"{run_id}.log"
    cfg.state_dir.mkdir(parents=True, exist_ok=True)
    handle = {"schema": "local-ci-hub/run/v1", "run_id": run_id, "commit": commit, "cell": cell,
              "state": "running", "command": cfg.validate_command, "log": str(log),
              "started_at": datetime.now(timezone.utc).isoformat(), "pid": None}
    hp = handles / f"{run_id}.json"
    hp.write_text(json.dumps(handle, indent=1))
    with log.open("wb") as lf:
        proc = subprocess.Popen(cfg.validate_command, cwd=cfg.root, stdout=lf, stderr=subprocess.STDOUT)
    handle["pid"] = proc.pid
    hp.write_text(json.dumps(handle, indent=1))
    lock_fd.close()  # admission decided and pid published; do not serialize the run itself
    if not wait:
        return handle
    rc = proc.wait()
    result = "PASS" if rc == 0 else "FAIL"
    Ledger(cfg).append_result(commit=commit, result=result, run_id=run_id, cell=cell,
                              command=" ".join(cfg.validate_command))
    Ledger(cfg).publish()
    handle.update(state="finished", result=result, exit_code=rc)
    hp.write_text(json.dumps(handle, indent=1))
    return handle


def list_runs(cfg: Config) -> list[dict]:
    """List retained run handles on this machine."""
    out = []
    for h in sorted(_handles(cfg).glob("*.json")):
        try:
            out.append(json.loads(h.read_text()))
        except Exception:
            out.append({"run_id": h.stem, "state": "unreadable"})
    return out


def stop(cfg: Config, run_id: str) -> dict:
    """Stop one running validation by run id."""
    hp = _handles(cfg) / f"{run_id}.json"
    if not hp.exists():
        return {"run_id": run_id, "state": "not-found"}
    handle = json.loads(hp.read_text())
    pid = handle.get("pid")
    if pid:
        try:
            os.kill(int(pid), 15)
            handle["state"] = "stopped"
        except ProcessLookupError:
            handle["state"] = "finished"
    hp.write_text(json.dumps(handle, indent=1))
    return handle
