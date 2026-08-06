"""Best-effort append-only records of attempts to cross the sandbox boundary.

A tool whose whole purpose is to bypass a confinement boundary must leave a trail that is complete
rather than convenient: REFUSED attempts are logged as prominently as successful ones, because a
run of refusals is exactly the signal worth noticing, and a log that only recorded successes would
make the allowlist's behaviour unobservable after the fact.

JSONL, one object per line, appended with one ``write`` of a line that ends in ``\\n``. Nothing here
locks. Linux ``O_APPEND`` assigns each write its end-of-file offset atomically, but this remains an
operational aid rather than durable or tamper-proof audit storage: writes are not fsynced and a
same-UID process can modify the file.
"""

from __future__ import annotations

import json
import os
import time
from collections.abc import Callable

from herdr_run.client import _bounded_control_command

__all__ = ["audit_path", "record", "spool_is_ignored", "warn_if_spool_is_tracked"]


def audit_path(project_root: str, spool_dir: str) -> str:
    """Absolute path of the append-only audit log for a project's spool directory."""
    root = (
        spool_dir if os.path.isabs(spool_dir) else os.path.join(project_root, spool_dir)
    )
    return os.path.join(root, "audit.jsonl")


def spool_is_ignored(project_root: str, spool_dir: str) -> bool | None:
    """Is the spool path git-ignored? ``None`` when the question does not apply or cannot be answered.

    Asks git itself rather than parsing ``.gitignore``: the effective ignore state is the product of
    repo, global, and info/exclude rules, and only ``git check-ignore`` resolves all three.
    """
    import subprocess

    root = (
        spool_dir if os.path.isabs(spool_dir) else os.path.join(project_root, spool_dir)
    )
    try:
        completed = _bounded_control_command(
            [
                "git",
                "-C",
                project_root,
                "check-ignore",
                "-q",
                "--",
                os.path.join(root, "probe"),
            ],
            timeout=10,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    if completed.returncode == 0:
        return True
    if completed.returncode == 1:
        return False
    # rc 128: not a git work tree, or git unusable here. Not a finding, so do not claim one.
    return None


def warn_if_spool_is_tracked(
    project_root: str, spool_dir: str, *, stream: object = None
) -> bool:
    """Warn when command output would be written into a tracked part of a source tree.

    The spool holds real stdout/stderr from commands that crossed the sandbox boundary. Writing that
    where ``git add`` can pick it up is how command output ends up committed. A warning rather than a
    refusal: an un-ignored spool is a hygiene defect, not a reason to block the run.
    """
    import sys

    if spool_is_ignored(project_root, spool_dir) is not False:
        return False
    target = stream if stream is not None else sys.stderr
    print(
        f"herdr-run: WARNING: spool directory {spool_dir!r} is NOT git-ignored in {project_root}. "
        "Command output and the audit log will be written into a tracked tree. "
        f"Add '{spool_dir.rstrip('/')}/' to .gitignore.",
        file=target,  # type: ignore[arg-type]
    )
    return True


def record(
    path: str,
    *,
    agent: str,
    command: str,
    verdict: str,
    detail: str,
    fields: dict[str, object] | None = None,
    now: Callable[[], float] = time.time,
) -> bool:
    """Append one audit entry, returning success without ever masking the command result."""
    entry: dict[str, object] = {
        "time": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(now())),
        "agent": agent,
        "pid": os.getpid(),
        "command": command,
        "verdict": verdict,
        "detail": detail,
    }
    if fields:
        entry.update(fields)
    try:
        parent = os.path.dirname(path) or "."
        os.makedirs(parent, mode=0o700, exist_ok=True)
        encoded = (json.dumps(entry, sort_keys=True, ensure_ascii=False) + "\n").encode(
            "utf-8"
        )
        flags = (
            os.O_APPEND
            | os.O_CREAT
            | os.O_WRONLY
            | getattr(os, "O_CLOEXEC", 0)
            | getattr(os, "O_NOFOLLOW", 0)
        )
        descriptor = os.open(path, flags, 0o600)
        try:
            os.fchmod(descriptor, 0o600)
            # One O_APPEND write gives concurrent callers a whole-line placement boundary.
            return os.write(descriptor, encoded) == len(encoded)
        finally:
            os.close(descriptor)
    except (OSError, UnicodeError, ValueError):
        return False
