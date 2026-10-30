"""Receipt verdict: what is proven about one exact commit."""
from __future__ import annotations

import json
from pathlib import Path

from .config import Config
from .ledger import Ledger


def remote_for_commit(cfg: Config, commit: str) -> dict:
    """Remote evidence: an optional JSON file written by `remote` ingest, else unknown."""
    p = cfg.state_dir / "remote.json"
    if cfg.provider == "none" or not p.exists():
        return {"source": "remote", "status": "unknown"}
    try:
        data = json.loads(p.read_text())
    except Exception:
        return {"source": "remote", "status": "unknown"}
    for row in data.get("runs", []):
        if str(row.get("commit", "")).startswith(commit[:12]):
            return {"source": "remote", "status": str(row.get("status", "unknown")).lower(), "row": row}
    return {"source": "remote", "status": "unknown"}


def show(cfg: Config, commit: str, remote: dict | None = None) -> dict:
    if commit in ("HEAD", "@"):
        import subprocess
        try:
            commit = subprocess.run(["git", "rev-parse", "HEAD"], cwd=cfg.root, capture_output=True, text=True, timeout=10).stdout.strip() or commit
        except Exception:
            pass
    rows = Ledger(cfg).for_commit(commit)
    local_pass = any(r.get("result") == "PASS" for r in rows)
    local_fail = any(r.get("result") == "FAIL" for r in rows)
    remote = remote if remote is not None else remote_for_commit(cfg, commit)
    checks = {
        "local-ledger-pass": local_pass and not local_fail,
        "remote-pass": remote.get("status") in ("pass", "success", "passed"),
    }
    qualified = all(checks.get(rule, False) for rule in cfg.qualifying) if cfg.qualifying else False
    if not rows and remote.get("status") == "unknown":
        verdict = "no-result"
    elif qualified:
        verdict = "qualified"
    else:
        verdict = "not-qualified"
    return {
        "schema": "local-ci-hub/receipt/v1", "commit": commit, "verdict": verdict,
        "qualified": qualified, "qualifying": cfg.qualifying, "checks": checks,
        "local_rows": rows, "remote": remote,
        "note": "no-result means no readable ledger row names this commit; it cannot distinguish never-run from failed-before-write",
    }
