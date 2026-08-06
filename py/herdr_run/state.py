"""Account-global coordination state for Herdr session and pane ownership.

Herdr workspace, tab, and pane identifiers belong to the operating-system account, not to one
project checkout. Locks protecting those objects therefore live below the account home recorded
by the OS account database. Caller-controlled ``HOME`` and ``XDG_*`` variables are deliberately
ignored.
"""

from __future__ import annotations

import os
import pwd
from typing import BinaryIO

from herdr_run.errors import HerdrUnavailable

__all__ = ["account_state_root", "open_lock_file", "pane_lock_path", "session_lock_path"]


def _account_home() -> str:
    """Return the real account's absolute home path from the passwd/NSS database."""
    try:
        home = pwd.getpwuid(os.getuid()).pw_dir
    except (KeyError, OSError) as exc:
        raise HerdrUnavailable(
            f"cannot resolve the current account's home directory for lock state: {exc}"
        ) from exc
    if not home:
        raise HerdrUnavailable("the current account has no home directory for lock state")
    if not os.path.isabs(home):
        raise HerdrUnavailable(f"the current account home is not absolute: {home!r}")
    if "\x00" in home:
        raise HerdrUnavailable("the current account home contains a NUL byte")
    return home


def account_state_root() -> str:
    """Return the deterministic per-account root; this does not consult the environment."""
    return os.path.join(_account_home(), ".local", "state", "herdr-run")


def _ensure_private_directory(path: str) -> None:
    try:
        os.makedirs(path, mode=0o700, exist_ok=True)
        # Tighten a pre-existing directory as well as accounting for a permissive process umask.
        os.chmod(path, 0o700)
    except (OSError, ValueError) as exc:
        raise HerdrUnavailable(f"cannot prepare private herdr-run state directory {path}: {exc}") from exc


def _lock_path(*parts: str) -> str:
    root = account_state_root()
    _ensure_private_directory(root)
    directory = root
    for part in ("locks", *parts[:-1]):
        directory = os.path.join(directory, part)
        _ensure_private_directory(directory)
    return os.path.join(directory, parts[-1])


def session_lock_path() -> str:
    """Return the one account-global session-resolution lock path."""
    return _lock_path("session-resolve.lock")


def pane_lock_path(pane_id: str) -> str:
    """Return the account-global lock path for one opaque Herdr pane identifier."""
    import hashlib

    digest = hashlib.sha256(pane_id.encode("utf-8")).hexdigest()
    return _lock_path("panes", f"{digest}.lock")


def open_lock_file(path: str) -> BinaryIO:
    """Open one advisory-lock inode with private mode and without following a final symlink."""
    flags = os.O_CREAT | os.O_RDWR
    flags |= getattr(os, "O_CLOEXEC", 0)
    flags |= getattr(os, "O_NOFOLLOW", 0)
    descriptor = -1
    try:
        descriptor = os.open(path, flags, 0o600)
        os.fchmod(descriptor, 0o600)
        return os.fdopen(descriptor, "r+b")
    except (OSError, ValueError) as exc:
        if descriptor >= 0:
            os.close(descriptor)
        raise HerdrUnavailable(f"cannot open private herdr-run lock {path}: {exc}") from exc
