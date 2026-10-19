"""Choose the command a tool runs as the GitHub CLI.

The order, first match wins:

1. ``AGENT_UTILS_GH``: an explicit command, split like a shell word list, for
   example ``gh`` (plain gh, no pacing) or ``/opt/tools/my-gh-wrapper``.
2. The gh-paced pacing wrapper: first a build in the source checkout this
   module runs from (``target/release/gh-paced`` or
   ``target/<host>/release/gh-paced`` in its build tree), then a ``gh-paced``
   on ``PATH``. It wraps the real ``gh``: ``GH_PACED_REAL_GH`` when set, else
   the first ``gh`` on ``PATH`` that is not itself a gh-paced wrapper (a
   script naming gh-paced, which would pace every call twice), else
   gh-paced's own default. Its budgets need the GitHub login it paces:
   ``GH_PACED_ACCOUNT``, else the real gh's active login (``gh config get
   user -h github.com``, which reads local configuration only). Without one
   the resolver refuses rather than run GitHub unpaced.
3. ``gh`` on ``PATH``.

No home-directory path is assumed and nothing is built on demand.
"""

# Maintenance: the canonical source is common/gh_resolve/gh_resolve.py. Each
# tool package that calls GitHub carries an exact copy, because every tool is
# published as an independent distribution that may not import a sibling, and
# py/tests/test_gh_resolve_copies.py fails when a copy differs.

from __future__ import annotations

from collections.abc import Mapping
import os
from pathlib import Path
import shlex
import shutil
import subprocess

OVERRIDE_VARIABLE = "AGENT_UTILS_GH"
ACCOUNT_VARIABLE = "GH_PACED_ACCOUNT"
REAL_GH_VARIABLE = "GH_PACED_REAL_GH"


class GhResolveError(RuntimeError):
    """No usable GitHub CLI command could be chosen."""


def source_root() -> Path | None:
    """The source checkout this module runs from, or None when installed."""

    root = Path(__file__).resolve().parents[2]
    if (root / "rs" / "gh-paced" / "Cargo.toml").is_file():
        return root
    return None


def _executable(path: Path) -> bool:
    return path.is_file() and os.access(path, os.X_OK)


def _built_gh_paced(root: Path) -> Path | None:
    target = root / "rs" / "target"
    candidates = [target / "release" / "gh-paced"]
    if target.is_dir():
        candidates += sorted(target.glob("*/release/gh-paced"))
    for candidate in candidates:
        if _executable(candidate):
            return candidate
    return None


def _wraps_gh_paced(path: Path) -> bool:
    """True for a script that runs gh-paced: wrapping it again would pace twice."""

    try:
        with path.open("rb") as handle:
            head = handle.read(64 * 1024)
    except OSError:
        return True
    return head.startswith(b"#!") and b"gh-paced" in head


def _real_gh(env: Mapping[str, str], paced: Path) -> str | None:
    configured = env.get(REAL_GH_VARIABLE, "").strip()
    if configured:
        return configured
    for directory in env.get("PATH", "").split(os.pathsep):
        if not directory:
            continue
        candidate = Path(directory) / "gh"
        if not _executable(candidate):
            continue
        resolved = candidate.resolve()
        if resolved == paced.resolve() or _wraps_gh_paced(resolved):
            continue
        return str(resolved)
    return None


def _active_login(real: str) -> str | None:
    """The login the real gh is signed in as, from its local configuration."""

    try:
        completed = subprocess.run(
            [real, "config", "get", "user", "-h", "github.com"],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired):
        return None
    login = completed.stdout.strip()
    return login if completed.returncode == 0 and login else None


def resolve_gh(
    env: Mapping[str, str] | None = None,
    *,
    root: Path | None = None,
    use_source_root: bool = True,
) -> list[str]:
    """Return the argv prefix that runs the GitHub CLI; append gh's own arguments.

    ``root`` names the source checkout to look for a gh-paced build in; by
    default it is the checkout this module runs from, if any. Tests pass
    ``use_source_root=False`` to ignore that checkout.
    """

    environment = os.environ if env is None else env
    override = environment.get(OVERRIDE_VARIABLE, "").strip()
    if override:
        command = shlex.split(override)
        if not command:
            raise GhResolveError(f"{OVERRIDE_VARIABLE} names no command")
        return command

    checkout = root if root is not None else (source_root() if use_source_root else None)
    paced = _built_gh_paced(checkout) if checkout is not None else None
    if paced is None:
        installed = shutil.which("gh-paced", path=environment.get("PATH"))
        paced = Path(installed) if installed else None
    if paced is not None:
        real = _real_gh(environment, paced)
        account = environment.get(ACCOUNT_VARIABLE, "").strip() or (
            _active_login(real) if real else None
        )
        if not account:
            raise GhResolveError(
                f"gh-paced is available ({paced}) but no GitHub login to pace was found; set "
                f"{ACCOUNT_VARIABLE}, sign the real gh in, or set {OVERRIDE_VARIABLE}=gh to run "
                "GitHub unpaced"
            )
        return [
            str(paced),
            "--account",
            account,
            *(["--real-gh", real] if real else []),
            "--",
        ]

    return ["gh"]


__all__ = [
    "ACCOUNT_VARIABLE",
    "GhResolveError",
    "OVERRIDE_VARIABLE",
    "REAL_GH_VARIABLE",
    "resolve_gh",
    "source_root",
]
