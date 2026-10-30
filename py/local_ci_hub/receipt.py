"""Receipt verdict: what is proven about one exact commit."""
from __future__ import annotations

import json
from pathlib import Path

from .config import Config
from .ledger import Ledger


def remote_for_commit(cfg: Config, commit: str) -> dict:
    """Return cached remote evidence for one exact commit, or unknown."""
    """Remote evidence: an optional JSON file written by `remote` ingest, else unknown."""
    p = cfg.state_dir / "remote.json"
    if cfg.provider == "none" or not p.exists():
        return {"source": "remote", "status": "unknown"}
    try:
        data = json.loads(p.read_text())
    except Exception:
        return {"source": "remote", "status": "unknown"}
    for row in data.get("runs", []):
        if str(row.get("commit", "")) == commit:
            return {"source": "remote", "status": str(row.get("status", "unknown")).lower(), "row": row}
    return {"source": "remote", "status": "unknown"}


def show(cfg: Config, commit: str, remote: dict | None = None) -> dict:
    """Return the receipt verdict for one exact commit."""
    if commit in ("HEAD", "@"):
        import subprocess
        try:
            commit = subprocess.run(["git", "rev-parse", "HEAD"], cwd=cfg.root, capture_output=True, text=True, timeout=10).stdout.strip() or commit
        except Exception:
            pass
    rows = Ledger(cfg).for_commit(commit)
    def _strict_pass(r):
        if r.get("result") != "PASS" or r.get("_digest_ok") is False or r.get("_evidence_ok") is False:
            return False
        if r.get("commit_anchored") is False:
            return False
        if cfg.require_full_evidence and not (r.get("commit_anchored") is True and isinstance(r.get("executed_tests"), (int, float)) and not isinstance(r.get("executed_tests"), bool) and r.get("executed_tests", 0) > 0):
            return False  # full-evidence projects: anchoring and executed tests are mandatory, not optional
        tests = r.get("executed_tests")
        if tests is not None and not (isinstance(tests, (int, float)) and not isinstance(tests, bool) and tests > 0):
            return False
        return True
    local_pass = any(_strict_pass(r) for r in rows)
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
