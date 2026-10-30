"""Run engine: admit, launch, list, stop validation runs on this machine."""
from __future__ import annotations

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
    handles = _handles(cfg)
    active = [h for h in handles.glob("*.json") if json.loads(h.read_text()).get("state") == "running"]
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
    out = []
    for h in sorted(_handles(cfg).glob("*.json")):
        try:
            out.append(json.loads(h.read_text()))
        except Exception:
            out.append({"run_id": h.stem, "state": "unreadable"})
    return out


def stop(cfg: Config, run_id: str) -> dict:
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
