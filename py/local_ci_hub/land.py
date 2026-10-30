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
    """Acquire the machine-scoped landing lock, or report its holder."""
    lock = _lock(cfg)
    try:
        fd = lock.open("x")
    except FileExistsError:
        return {"status": "held", "holder": lock.read_text().strip()}
    fd.write(holder)
    fd.close()
    return {"status": "acquired", "holder": holder}


def release(cfg: Config, holder: str) -> dict:
    """Release the landing lock if held by the named holder."""
    lock = _lock(cfg)
    if not lock.exists():
        return {"status": "free"}
    if lock.read_text().strip() != holder:
        return {"status": "held-by-other", "holder": lock.read_text().strip()}  # an empty lock is a live acquisition, never removable by others
    lock.unlink()
    return {"status": "released"}


def run_locked(cfg: Config, holder: str, command: list[str]) -> dict:
    """Run one project command while holding the landing lock; always releases."""
    got = acquire(cfg, holder)
    if got.get("status") != "acquired":
        return got
    try:
        import subprocess as _sp
        r = _sp.run(command, cwd=cfg.root, timeout=3600)
        return {"schema": "local-ci-hub/land/v1", "status": "ran", "exit_code": r.returncode}
    finally:
        release(cfg, holder)


def verify(cfg: Config, commit: str, target: str = "HEAD") -> dict:
    """Return whether a commit is an ancestor of the target."""
    try:
        r = subprocess.run(["git", "merge-base", "--is-ancestor", commit, target], cwd=cfg.root, capture_output=True, timeout=15)
        return {"schema": "local-ci-hub/land/v1", "commit": commit, "target": target, "ancestor": r.returncode == 0}
    except Exception as exc:
        return {"schema": "local-ci-hub/land/v1", "commit": commit, "ancestor": False, "error": str(exc)}
