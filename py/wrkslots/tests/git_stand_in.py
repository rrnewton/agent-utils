"""A git stand-in first on PATH for the wrkslots tests (#2065, #2418).

Some hosts wrap git in a launcher that leaves a detached helper behind after each call. The
helper runs git with the caller's working directory, so in a slot, and it outlives the call by
tens of milliseconds, longer on a busy host. A removal or audit census right after wrkslots'
own git call can see it as a live process using the slot, and refuse or count an agent. The
product fix is for wrkslots to wait for its own commands' descendants before a census (#2418).
Until then the suite puts this stand-in first on PATH.

The stand-in (``git_stand_in.c``) is compiled once per session, with the real git's path and
its call log built in. It goes into a private directory under the session's ``TMPDIR`` (a
gate's private run root). If ``TMPDIR`` is unset, or is the host temp dir itself, it goes under
the checkout's ignored ``.pytest_cache`` instead. The directory is removed when the session
ends. The stand-in runs the real git in its own unprivileged user and PID namespace under a
small init. When git exits, the init exits, and the kernel kills whatever git left behind
before the call returns. Exit codes, signal deaths and signals sent to the call pass through.
It needs no privilege beyond unprivileged user namespaces: it maps only the caller's own user
and group, mounts nothing, and git runs with the caller's identity.

The stand-in is off when no C compiler is on PATH, or when a probe call at session start finds
unprivileged user namespaces refused. Then cases marked ``git_stand_in`` are quarantined for
the session and named in the summary, so the helper race cannot redden them. A later call that
finds namespaces refused runs plain git and is counted as a fallback in the summary.
"""

from __future__ import annotations

import collections
import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path

SOURCE = Path(__file__).with_name("git_stand_in.c")
#: Set while a session's stand-in is first on PATH, so a nested session leaves it alone.
ACTIVE_ENV = "WRKSLOTS_TEST_GIT_STAND_IN"
PY_ROOT = Path(__file__).resolve().parents[2]


def _private_parent() -> Path:
    """The session's ``TMPDIR``, unless it is unset or the host temp dir itself."""
    configured = os.environ.get("TMPDIR", "").strip()
    if configured and Path(configured).resolve() != Path("/tmp").resolve():
        return Path(configured)
    cache = PY_ROOT / ".pytest_cache"  # ignored by the repository
    cache.mkdir(exist_ok=True)
    return cache


def _compiler() -> str | None:
    for name in ("cc", "gcc", "clang"):
        found = shutil.which(name)
        if found is not None:
            return found
    return None


def _calls(log: Path) -> collections.Counter[str]:
    counts: collections.Counter[str] = collections.Counter()
    try:
        with open(log, encoding="utf-8") as handle:
            counts.update(line.split()[0] for line in handle if line.strip())
    except FileNotFoundError:
        pass
    return counts


class GitStandIn:
    """One session's stand-in: its directory while it is on, or why it is off."""

    def __init__(
        self, directory: Path | None, off_reason: str = "", *, outer: bool = False
    ) -> None:
        self.directory = directory
        self.off_reason = off_reason
        self.outer = outer

    @property
    def on(self) -> bool:
        return self.directory is not None or self.outer

    @classmethod
    def start(cls) -> GitStandIn:
        if os.environ.get(ACTIVE_ENV):
            return cls(None, outer=True)  # an outer session's stand-in is first on PATH
        real = shutil.which("git")
        if real is None:
            return cls(None, "no git on PATH")
        compiler = _compiler()
        if compiler is None:
            return cls(None, "no C compiler (cc, gcc, or clang) is on PATH")
        directory = Path(tempfile.mkdtemp(prefix="wrkslots-git-stand-in-", dir=_private_parent()))
        binary = directory / "git"
        log = directory / "calls.log"
        built = subprocess.run(
            [
                compiler, "-O2", "-Wall", "-Wextra", "-Werror",
                "-DREAL_GIT=" + json.dumps(real), "-DCALL_LOG=" + json.dumps(str(log)),
                "-o", str(binary), str(SOURCE),
            ],
            capture_output=True,
            text=True,
            check=False,
            timeout=120,
        )
        if built.returncode != 0:
            # A compiler that exists but cannot build the stand-in is a failure, not a skip.
            shutil.rmtree(directory, ignore_errors=True)
            raise RuntimeError(
                f"{compiler} failed to build the git stand-in: {built.stderr.strip()}"
            )
        probe = subprocess.run(
            [str(binary), "--version"], capture_output=True, text=True, check=False, timeout=120
        )
        first = log.read_text(encoding="utf-8").split() if log.exists() else []
        if probe.returncode != 0 or first[:1] != ["ns"]:
            shutil.rmtree(directory, ignore_errors=True)
            if first[:1] == ["plain"]:
                detail = f"errno {first[1] if len(first) > 1 else '?'}"
            else:
                detail = probe.stderr.strip() or f"exit {probe.returncode}"
            return cls(None, f"unprivileged user namespaces are refused here ({detail})")
        log.unlink()
        os.environ[ACTIVE_ENV] = str(directory)
        os.environ["PATH"] = str(directory) + os.pathsep + os.environ.get("PATH", "")
        return cls(directory)

    def summary(self) -> str:
        if self.outer:
            return "an outer session's stand-in is first on PATH; its session counts the calls"
        if self.directory is None:
            return f"off: {self.off_reason}; cases marked git_stand_in are quarantined"
        counts = _calls(self.directory / "calls.log")
        return (
            f"{counts['ns']} git calls ran in their own namespace, "
            f"{counts['plain']} fell back to plain git"
        )

    def cleanup(self) -> None:
        if self.directory is not None:
            shutil.rmtree(self.directory, ignore_errors=True)
