"""Run engine: admit, launch, list, stop validation runs on this machine."""
from __future__ import annotations


import dataclasses
import fcntl
import socket
from collections.abc import Callable, Mapping
import json
import os
import subprocess
import time
import uuid
from datetime import datetime, timezone
from pathlib import Path

from . import host_limits, result_adapter
from .config import Config
from .ledger import Ledger


def _state_root(cfg: Config, environ: Mapping[str, str] | None = None) -> Path:
    eff = host_limits.load_effective(reader="run", environ=environ)
    return eff.state_root


def _handles(cfg: Config, environ: Mapping[str, str] | None = None) -> Path:
    d = host_limits.load_effective(reader="run", environ=environ).state_root / "runs"
    d.mkdir(parents=True, exist_ok=True)
    try:
        d.parent.chmod(0o700)
    except OSError:
        pass
    return d


def _git(root: Path, *args: str) -> str:
    try:
        return subprocess.run(["git", *args], cwd=root, capture_output=True,
                              text=True, timeout=10).stdout.strip()
    except Exception:
        return ""


def _record_fields(cfg: Config, *, commit: str, run_id: str,
                     started_at: str, decision: object, inputs: object) -> dict[str, object]:
    """Terminal-record fields the runner itself computes.

    Anchoring is honest about this runner: it executes in place,
    so commit_anchored is false until the checkout lifecycle
    materializes a slot. Tooling identity is the tool tree HEAD.
    """
    tree = _git(cfg.root, "rev-parse", f"{commit}^{{tree}}") or ""
    base_sha = _git(cfg.root, "rev-parse", f"{commit}^") or commit
    base_tree = _git(cfg.root, "rev-parse", f"{base_sha}^{{tree}}") or tree
    tool_root = Path(__file__).resolve().parents[2]
    tooling = _git(tool_root, "rev-parse", "HEAD") or "unknown"
    dirty = bool(_git(cfg.root, "status", "--porcelain"))
    adapted = result_adapter.from_result_file(cfg.state_dir / f"{run_id}.result.json")
    fields: dict[str, object] = {
        "schema_version": 6, "producer": cfg.producer, "repo": cfg.repo,
        "record_id": f"rec-{run_id}", "tree": tree, "base_sha": base_sha,
        "base_tree": base_tree, "commit_anchored": False, "tree_dirty": dirty,
        "admission": "host-limits-live",
        "admission_floor_evidence": {
            "floor_kind": "current-main", "floor_sha": commit,
            "observed_main_sha": _commit(cfg.root), "floor_tree": tree},
        "tooling_sha": tooling, "cwd": str(cfg.root),
        "host": socket.gethostname().split(".")[0],
        "started_at": started_at,
        "concurrent_validates": int(getattr(inputs, "v", 0)) + 1,
        "concurrency_proof": {"decision": decision.as_dict() if hasattr(decision, "as_dict") else {},
                              "inputs": inputs.as_dict() if hasattr(inputs, "as_dict") else {}},
    }
    fields.update(adapted)
    return fields


def _commit(root: Path) -> str:
    try:
        return subprocess.run(["git", "rev-parse", "HEAD"], cwd=root, capture_output=True, text=True, timeout=10).stdout.strip()
    except Exception:
        return ""


def _lock_held(lock_path: Path) -> bool:
    """Whether a run life lock is still held (never a bare PID proof)."""
    try:
        fd = os.open(lock_path, os.O_RDWR | os.O_CREAT)
    except OSError:
        return False
    try:
        fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
    except OSError:
        return True
    try:
        fcntl.flock(fd, fcntl.LOCK_UN)
    finally:
        os.close(fd)
    return False


def count_live(handles: Path, now_ts: float) -> tuple[int, int, int]:
    """(V live runs, Y young runs, P cleanup-pending) under one root."""
    v = y = p = 0
    for h in sorted(handles.glob("*.json")):
        try:
            data = json.loads(h.read_text())
        except Exception:
            continue
        if not isinstance(data, dict):
            continue
        if data.get("state") == "cleanup-pending" or data.get("cleanup_pending") is True:
            p += 1
            continue
        if data.get("state") != "running":
            continue
        lock_path = handles / f"{h.stem}.lock"
        if lock_path.exists():
            # Liveness is lock ownership, never a bare stored PID.
            live = _lock_held(lock_path)
        else:
            pid = data.get("pid")
            live = False
            if pid:
                try:
                    os.kill(int(str(pid)), 0)
                    live = True
                except (ProcessLookupError, ValueError, TypeError):
                    live = False
        if not live:
            continue
        v += 1
        admitted = data.get("admitted_at_epoch")
        young = False
        if isinstance(admitted, (int, float)) and not isinstance(admitted, bool):
            young = (now_ts - float(admitted)) < host_limits.YOUNG_WINDOW_S
        if young and data.get("first_steady_sample") is not True:
            y += 1
    return v, y, p


def start(cfg: Config, *, wait: bool = True, commit: str = "", cell: str = "",
          admission_wait_s: float = 0.0, environ: Mapping[str, str] | None = None,
          sampler: Callable[[], host_limits.Headroom] | None = None,
          free_fn: Callable[[], tuple[int, dict[str, object]]] | None = None,
          now_fn: Callable[[], float] | None = None,
          event_wait_fn: Callable[[float], None] | None = None,
          box_events: tuple[str | None, str | None] | None = None,
          wall_breached: bool = False, cpu_breached: bool = False) -> dict[str, object]:
    """Admit and run the validate command, recording its ledger result.

    Admission is live: measured headroom, the young-run charge, the
    cleanup-pending bound and the disk floor decide, under the host
    ceiling clamped by the instance request. Refusal is no result
    (exit 75 at the CLI), never a failure row and never a checkout.
    A host-imposed box breach likewise maps to no result here, in
    this tool's result handling only.
    """
    now = now_fn if callable(now_fn) else time.time
    slot_total = host_limits.slot_total_bytes(cfg.root)
    eff = host_limits.load_effective(reader="run", environ=environ, slot_total_bytes=slot_total)
    handles = eff.state_root / "runs"
    handles.mkdir(parents=True, exist_ok=True)
    try:
        eff.state_root.chmod(0o700)
    except OSError:
        pass
    ceiling, bound_by = host_limits.clamp_request(cfg.admission_slots,
                                                  eff.max_concurrent_validation_runs)
    limits = dataclasses.replace(eff, max_concurrent_validation_runs=ceiling)
    gate_g, gate_cap, gate_note = host_limits.count_gate_runs(environ)

    def attempt() -> tuple[host_limits.Decision, host_limits.AdmissionInputs, dict[str, object]]:
        lock_fd = (handles / "admission.lock").open("a+")
        try:
            fcntl.flock(lock_fd, fcntl.LOCK_EX)
            v, y, p = count_live(handles, float(now()))
            if sampler is not None:
                head = sampler()
            else:
                head = host_limits.sample_headroom(environ=environ)
            if free_fn is not None:
                free_bytes, components = free_fn()
            else:
                free_bytes, components = host_limits.measure_free_bytes(cfg.root)
            inputs = host_limits.AdmissionInputs(v=v, y=y, p=p, f=int(free_bytes), headroom=head)
            return host_limits.decide(inputs, limits), inputs, components
        finally:
            lock_fd.close()

    decision, inputs, components = attempt()
    if not decision.admit and admission_wait_s > 0:
        def _decide() -> host_limits.Decision:
            d, i, c = attempt()
            nonlocal_decision[0] = (d, i, c)
            return d
        nonlocal_decision: list[object] = [(decision, inputs, components)]
        decision, admitted = host_limits.wait_for_admission(
            _decide, bound_s=admission_wait_s, now_fn=now,
            wait_event_fn=event_wait_fn)
        _d, inputs, components = nonlocal_decision[0]  # type: ignore[misc]
        if not admitted:
            return {"schema": "local-ci-hub/run/v1", "state": "refused", "outcome": "no_result",
                    "reason": decision.binding, "bound_by": bound_by,
                    "host_limits_path": str(eff.path), "warnings": eff.warnings,
                    "decision": decision.as_dict(), "inputs": inputs.as_dict(),
                    "free_components": components,
                    "gate": {"g": gate_g, "g_cap": gate_cap, "note": gate_note}}
    if not decision.admit:
        return {"schema": "local-ci-hub/run/v1", "state": "refused", "outcome": "no_result",
                "reason": decision.binding, "bound_by": bound_by,
                "host_limits_path": str(eff.path), "warnings": eff.warnings,
                "decision": decision.as_dict(), "inputs": inputs.as_dict(),
                "free_components": components,
                "gate": {"g": gate_g, "g_cap": gate_cap, "note": gate_note}}

    run_id = f"run-{uuid.uuid4().hex[:12]}"
    commit = commit or _commit(cfg.root) or "unknown"
    log = cfg.state_dir / f"{run_id}.log"
    cfg.state_dir.mkdir(parents=True, exist_ok=True)
    admitted_epoch = float(now())
    handle: dict[str, object] = {
        "schema": "local-ci-hub/run/v1", "run_id": run_id, "commit": commit, "cell": cell,
        "state": "running", "command": cfg.validate_command, "log": str(log),
        "started_at": datetime.now(timezone.utc).isoformat(),
        "admitted_at_epoch": admitted_epoch, "first_steady_sample": False,
        "pid": None, "host_limits_path": str(eff.path), "bound_by": bound_by,
        "decision": decision.as_dict(), "inputs": inputs.as_dict(),
        "free_components": components,
        "gate": {"g": gate_g, "g_cap": gate_cap, "note": gate_note},
        "box": {"cpu_quota_cpus": limits.cpu_quota_cpus,
                "memory_max_bytes": limits.memory_max_bytes, "pids_max": limits.pids_max},
        "warnings": eff.warnings,
    }
    hp = handles / f"{run_id}.json"
    hp.write_text(json.dumps(handle, indent=1))
    lock_fd = os.open(handles / f"{run_id}.lock", os.O_RDWR | os.O_CREAT)
    fcntl.flock(lock_fd, fcntl.LOCK_EX)
    with log.open("wb") as lf:
        proc = subprocess.Popen(cfg.validate_command, cwd=cfg.root, stdout=lf, stderr=subprocess.STDOUT)
    handle["pid"] = proc.pid
    hp.write_text(json.dumps(handle, indent=1))
    if not wait:
        # The life lock stays with the process table entry: close our
        # copy only when this call owns the run to completion. For a
        # detached start, keep liveness by pid record as before.
        os.close(lock_fd)
        return handle
    rc = proc.wait()
    mem_events = pids_events = None
    if box_events is not None:
        mem_events, pids_events = box_events
    else:
        mem_path = handles / f"{run_id}.memory.events"
        pids_path = handles / f"{run_id}.pids.events"
        mem_events = mem_path.read_text(errors="replace") if mem_path.exists() else None
        pids_events = pids_path.read_text(errors="replace") if pids_path.exists() else None
    breach = host_limits.box_breach(mem_events, pids_events)
    _cpu_quota, cpu_bound_by = host_limits.clamp_request(
        getattr(cfg, "cpu_quota_cpus", None), limits.cpu_quota_cpus)
    outcome, exit_code = host_limits.classify_outcome(
        validator_exit=rc, breach=breach, wall_breached=wall_breached,
        cpu_breached=cpu_breached, cpu_bound_by=cpu_bound_by)
    ledger_result = {"pass": "PASS", "fail": "FAIL", "timeout": "TIMEOUT",
                     "no_result": "NO_RESULT"}[outcome]
    Ledger(cfg).append_result(commit=commit, result=ledger_result, run_id=run_id, cell=cell,
                              command=" ".join(cfg.validate_command),
                              extra=_record_fields(cfg, commit=commit, run_id=run_id,
                                                   started_at=str(handle.get("started_at", "")),
                                                   decision=decision, inputs=inputs))
    Ledger(cfg).publish()
    handle.update(state="finished", result=ledger_result, outcome=outcome, exit_code=exit_code,
                box_breach=breach)
    hp.write_text(json.dumps(handle, indent=1))
    os.close(lock_fd)
    return dict(handle)


def sweep(cfg: Config, *, environ: Mapping[str, str] | None = None,
          now_ts: float | None = None, quiet_runner: Callable[[str], int] | None = None) -> dict[str, object]:
    """Clear cleanup-pending handles, deferring under holds/quiet.

    Deletions defer while a measurement hold or host quiet is active
    on the generic defer surface: nothing is deleted, the pending
    count is unchanged, and the deferral and its source are reported.
    """
    eff = host_limits.load_effective(reader="sweep", environ=environ)
    handles = eff.state_root / "runs"
    handles.mkdir(parents=True, exist_ok=True)
    now = time.time() if now_ts is None else now_ts
    _v, _y, pending = count_live(handles, now)
    deferred, sources = host_limits.sweep_deferral(now, environ, quiet_runner)
    if deferred:
        return {"schema": "local-ci-hub/sweep/v1", "deferred": True, "deleted": 0,
                "pending": pending, "sources": sources,
                "host_limits_path": str(eff.path)}
    deleted = 0
    for h in sorted(handles.glob("*.json")):
        try:
            data = json.loads(h.read_text())
        except Exception:
            continue
        if isinstance(data, dict) and (data.get("state") == "cleanup-pending"
                                       or data.get("cleanup_pending") is True):
            h.unlink(missing_ok=True)
            (handles / f"{h.stem}.lock").unlink(missing_ok=True)
            deleted += 1
    return {"schema": "local-ci-hub/sweep/v1", "deferred": False, "deleted": deleted,
            "pending": 0, "sources": [], "host_limits_path": str(eff.path)}


def list_runs(cfg: Config, environ: Mapping[str, str] | None = None) -> list[dict[str, object]]:
    """List retained run handles on this machine."""
    out = []
    for h in sorted(_handles(cfg, environ).glob("*.json")):
        try:
            data = json.loads(h.read_text())
            out.append(data if isinstance(data, dict) else {"state": "unreadable"})
        except Exception:
            out.append({"run_id": h.stem, "state": "unreadable"})
    return out


def stop(cfg: Config, run_id: str, environ: Mapping[str, str] | None = None) -> dict[str, object]:
    """Stop one running validation by run id."""
    hp = _handles(cfg, environ) / f"{run_id}.json"
    if not hp.exists():
        return {"run_id": run_id, "state": "not-found"}
    loaded = json.loads(hp.read_text())
    handle: dict[str, object] = loaded if isinstance(loaded, dict) else {}
    pid = handle.get("pid")
    if pid:
        try:
            os.kill(int(str(pid)), 15)
            handle["state"] = "stopped"
        except ProcessLookupError:
            handle["state"] = "finished"
    hp.write_text(json.dumps(handle, indent=1))
    return dict(handle)
