"""Remote CI (GitHub): read and dispatch. Reports; never gates by itself."""
from __future__ import annotations


import json
import subprocess

from .config import Config


def _gh(cfg: Config, args: list[str]) -> dict[str, object]:
    if cfg.provider == "none":
        return {"status": "disabled", "reason": "remote.provider is none"}
    try:
        r = subprocess.run(["gh", *args], cwd=cfg.root, capture_output=True, text=True, timeout=60)
    except FileNotFoundError:
        return {"status": "unknown", "reason": "gh not found"}
    if r.returncode != 0:
        return {"status": "unknown", "reason": (r.stderr or "gh failed").strip()[:300]}
    try:
        return {"status": "ok", "data": json.loads(r.stdout) if r.stdout.strip() else {}}
    except json.JSONDecodeError:
        return {"status": "ok", "data": r.stdout.strip()[:500]}


def status(cfg: Config) -> dict[str, object]:
    """Return recent remote workflow runs, or disabled."""
    return {"schema": "local-ci-hub/remote/v1", **_gh(cfg, ["run", "list", "--limit", "20", "--json", "databaseId,headSha,status,conclusion,workflowName"])}


def dispatch(cfg: Config, workflow: str, ref: str = "") -> dict[str, object]:
    """Dispatch one remote workflow explicitly; reports, never gates."""
    args = ["workflow", "run", workflow] + (["--ref", ref] if ref else [])
    return {"schema": "local-ci-hub/remote/v1", **_gh(cfg, args)}
