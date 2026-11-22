"""Keep test sessions from reading or writing the real global git config.

Issue #1474 (tests-rewrite-global-gitconfig): a test that runs ``git config``
with global scope -- explicitly, or implicitly from a working directory where
git resolves the global file -- rewrites the operator's real ``~/.gitconfig``.
The file then carries test state (a throwaway identity was observed in the
wild) into every later git invocation on the host.

The guard installed by the pytest conftests in ``py/tests`` and
``py/wrkslots/tests`` (and mirrored in the environment that
``scripts/validate.py`` and ``scripts/run_pytest_shard.py`` hand to their
workers, which is also how the Rust test harnesses receive it) does two
things:

* At session start it points ``GIT_CONFIG_GLOBAL`` at a fresh per-session
  temporary file seeded with a test identity and sets
  ``GIT_CONFIG_NOSYSTEM=1``, so any
  global-scope git write lands in the throwaway file and the operator's
  system and global configuration cannot leak into a test verdict.
* It fingerprints the real ``~/.gitconfig`` (existence and content hash, plus
  inode and mtime) at session start and compares at session finish. A
  content change fails the session. An inode/mtime change with identical
  content only warns: a dotfile sync may legitimately copy the file into
  place mid-run, and identical bytes are not test damage.
"""

from __future__ import annotations

import atexit
import hashlib
import os
import shutil
import signal
import sys
import tempfile
import threading
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path


def real_gitconfig_path(environ: Mapping[str, str] | None = None) -> Path:
    """The operator's real global git config, ignoring ``GIT_CONFIG_GLOBAL``."""

    home = (os.environ if environ is None else environ).get("HOME")
    if home:
        return Path(home) / ".gitconfig"
    return Path.home() / ".gitconfig"


@dataclass(frozen=True)
class Fingerprint:
    """What identifies the real file's state at one moment."""

    exists: bool
    sha256: str | None
    inode: int | None
    mtime_ns: int | None


def fingerprint(path: Path) -> Fingerprint:
    """Fingerprint ``path``; a missing or unreadable file is ``exists=False``."""

    try:
        data = path.read_bytes()
        info = path.stat()
    except OSError:
        return Fingerprint(False, None, None, None)
    return Fingerprint(True, hashlib.sha256(data).hexdigest(), info.st_ino, info.st_mtime_ns)


#: The minimal keys the suites need from a global config: an identity that
#: reads of ``user.name``/``user.email`` can find, and ``uploadpack``
#: permission for filtered fetches (hosts commonly grant it in the system
#: config, which ``GIT_CONFIG_NOSYSTEM`` hides). The identity is a test
#: identity, so a commit that forgets repo-local identity is visibly a test
#: commit rather than silently the operator's.
_SEED = (
    b"[user]\n\tname = Agent Utils Tests\n"
    b"\temail = agent-utils-tests@example.invalid\n"
    b"[uploadpack]\n\tallowfilter = true\n"
)
_SEED_SHA256 = hashlib.sha256(_SEED).hexdigest()


#: Guards not yet cleaned up in this process, for the signal handlers.
_live_guards: list[GlobalGitConfigGuard] = []
_signal_cleanup_installed = False


def _install_signal_cleanup() -> None:
    """Clean live guards up on SIGTERM/SIGINT, then re-raise the signal.

    Installed once, main thread only (``signal.signal`` refuses other
    threads; the conftest session guard starts on the main thread).
    Mirrors the SIGTERM handling in ``scripts/validate.py``.
    """

    global _signal_cleanup_installed
    if _signal_cleanup_installed:
        return
    if threading.current_thread() is not threading.main_thread():
        return

    def _on_signal(signum: int, _frame: object) -> None:
        for guard in list(_live_guards):
            guard.cleanup()
        signal.signal(signum, signal.SIG_DFL)
        os.kill(os.getpid(), signum)

    try:
        signal.signal(signal.SIGTERM, _on_signal)
        signal.signal(signal.SIGINT, _on_signal)
    except (ValueError, OSError):
        return
    _signal_cleanup_installed = True


@dataclass
class GlobalGitConfigGuard:
    """One session's redirect of global git config plus its damage check."""

    real_path: Path
    before: Fingerprint
    sentinel: Path
    _cleaned: bool = field(default=False, repr=False)

    @classmethod
    def start(
        cls, environ: dict[str, str] | None = None
    ) -> GlobalGitConfigGuard:
        """Fingerprint the real file and redirect global config to a sentinel.

        The sentinel is a fresh seeded file in a per-session temporary
        directory. At session end :meth:`cleanup` removes the directory
        when the sentinel still holds exactly the seed; when a write was
        caught there it keeps the directory and prints its path, so the
        writer can be found. The pytest conftests call it at session
        finish, ``atexit`` covers interpreter exits (failures included),
        and SIGTERM/SIGINT handlers cover a worker killed mid-session.
        Only SIGKILL leaves an entry, for the stale-entry sweep.
        """

        env = os.environ if environ is None else environ
        real = real_gitconfig_path(env)
        before = fingerprint(real)
        directory = Path(tempfile.mkdtemp(prefix="agent-utils-gitconfig-global-"))
        sentinel = directory / "gitconfig"
        sentinel.write_bytes(_SEED)
        sentinel.chmod(0o600)
        env["GIT_CONFIG_GLOBAL"] = str(sentinel)
        env["GIT_CONFIG_NOSYSTEM"] = "1"
        guard = cls(real_path=real, before=before, sentinel=sentinel)
        _live_guards.append(guard)
        atexit.register(guard.cleanup)
        _install_signal_cleanup()
        return guard

    def cleanup(self) -> None:
        """Remove the sentinel directory unless a write was caught in it.

        Safe to call twice. When the sentinel no longer holds exactly the
        seed, a global write landed there: keep the directory for
        inspection and print its path instead of deleting the evidence.
        """

        if self._cleaned:
            return
        self._cleaned = True
        if self in _live_guards:
            _live_guards.remove(self)
        current = fingerprint(self.sentinel)
        if current.exists and current.sha256 not in (None, _SEED_SHA256):
            print(
                f"global git config guard: kept session sentinel at "
                f"{self.sentinel.parent} (a global write landed there; "
                f"inspect {self.sentinel})",
                file=sys.stderr,
            )
            return
        shutil.rmtree(self.sentinel.parent, ignore_errors=True)

    def __enter__(self) -> GlobalGitConfigGuard:
        return self

    def __exit__(self, *exc: object) -> None:
        self.cleanup()

    def check(self) -> tuple[str, str]:
        """Compare the real file against the session-start fingerprint.

        Returns ``(status, message)``: ``fail`` when the content changed
        (created, deleted, or different bytes), ``warn`` when only the
        inode or mtime moved under identical content, else ``ok``.
        """

        after = fingerprint(self.real_path)
        if after.exists != self.before.exists or after.sha256 != self.before.sha256:
            return (
                "fail",
                f"the real global git config {self.real_path} changed content during "
                "the test session (sha256 "
                f"{self.before.sha256} -> {after.sha256}); a test wrote global git "
                "config -- global writes belong in repo-local config or in the "
                f"session sentinel {self.sentinel}",
            )
        if (after.inode, after.mtime_ns) != (self.before.inode, self.before.mtime_ns):
            return (
                "warn",
                f"the real global git config {self.real_path} was replaced during "
                "the test session with identical content (inode/mtime changed); "
                "treating as an external sync, not test damage",
            )
        return ("ok", "")

    def sentinel_note(self) -> str:
        """Note any global-scope write that the redirect caught, else empty."""

        current = fingerprint(self.sentinel)
        if current.exists and current.sha256 not in (None, _SEED_SHA256):
            return (
                f"global git config writes were redirected to the session sentinel "
                f"{self.sentinel}; inspect it to find the writer"
            )
        return ""
