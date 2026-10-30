"""Land primitives: machine-scoped lock + ancestry verify. Project owns merging."""
from __future__ import annotations

import subprocess
from pathlib import Path

from .config import Config


def _lock(cfg: Config) -> Path:
    p = cfg.state_dir / "land.lock"
    p.parent.mkdir(parents=True, exist_ok=True)
    return p


def acquire(cfg: Config, holder: str) -> dict:
    lock = _lock(cfg)
    try:
        fd = lock.open("x")
    except FileExistsError:
        return {"status": "held", "holder": lock.read_text().strip()}
    fd.write(holder)
    fd.close()
    return {"status": "acquired", "holder": holder}


def release(cfg: Config, holder: str) -> dict:
    lock = _lock(cfg)
    if not lock.exists():
        return {"status": "free"}
    if lock.read_text().strip() not in (holder, ""):
        return {"status": "held-by-other", "holder": lock.read_text().strip()}
    lock.unlink()
    return {"status": "released"}


def verify(cfg: Config, commit: str, target: str = "HEAD") -> dict:
    try:
        r = subprocess.run(["git", "merge-base", "--is-ancestor", commit, target], cwd=cfg.root, capture_output=True, timeout=15)
        return {"schema": "local-ci-hub/land/v1", "commit": commit, "target": target, "ancestor": r.returncode == 0}
    except Exception as exc:
        return {"schema": "local-ci-hub/land/v1", "commit": commit, "ancestor": False, "error": str(exc)}
