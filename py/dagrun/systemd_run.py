"""Keeping ``systemd-run`` from rewriting the command it starts (#2316).

``systemd-run`` expands ``${NAME}`` anywhere in an argument, a whole-word ``$NAME``, and ``$$``
(to ``$``) in the command line it is given. With ``--scope`` it does so itself, from the CALLER's
environment (the default in newer systemd); without ``--scope`` the service manager does it from
the unit's environment. An unknown or invalid name becomes the empty string. A launcher that hands
over text it did not write (a shell script, a user's command, its own argv) must turn that off with
``--expand-environment=no``, or the command that runs is not the one it was given.

systemd before 254 refuses the option as unknown, and on those versions a scope never expanded
anything, so there the launch goes ahead without it after one note on stderr.
"""

from __future__ import annotations

import errno
import subprocess
import sys
import time

#: The option that passes the command's arguments through byte for byte.
NO_EXPAND_ENVIRONMENT = "--expand-environment=no"

#: Longest the option probe may take. It needs no bus and starts no unit, so this only bounds a
#: broken binary.
PROBE_TIMEOUT_S = 10.0

_SUPPORTED: bool | None = None


def probe_no_expand(program: str) -> bool | None:
    """Whether ``program`` accepts :data:`NO_EXPAND_ENVIRONMENT`: True when it does, False when
    it refuses the option, None when it could not be run at all. ``--version`` after the option
    makes a supporting ``systemd-run`` print its version and exit without touching the bus."""

    for _attempt in range(50):
        try:
            completed = subprocess.run(
                [program, NO_EXPAND_ENVIRONMENT, "--version"],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                timeout=PROBE_TIMEOUT_S,
                check=False,
            )
        except subprocess.TimeoutExpired:
            return False
        except OSError as exc:
            # ETXTBSY: a just-written executable can still be open in a sibling's forked child
            # for a moment; that is not a refusal, so try again briefly.
            if exc.errno == errno.ETXTBSY:
                time.sleep(0.02)
                continue
            return None
        return completed.returncode == 0
    return None


def unsupported_note(prefix: str) -> str:
    """The one line said when ``systemd-run`` refuses the option."""

    return (
        f"{prefix} note: this systemd-run refuses {NO_EXPAND_ENVIRONMENT} (systemd older than "
        "254), so it is left out; scopes there do not expand ${...} in arguments (#2316)."
    )


def no_expand_args(prefix: str) -> list[str]:
    """The arguments to put before ``--`` on every ``systemd-run`` call whose command carries
    text this process did not write: ``[NO_EXPAND_ENVIRONMENT]`` when the ``systemd-run`` on
    ``PATH`` accepts it, otherwise nothing. Asked once per process; a refusal is reported once on
    stderr under ``prefix``."""

    global _SUPPORTED
    if _SUPPORTED is None:
        answer = probe_no_expand("systemd-run")
        if answer is False:
            print(unsupported_note(prefix), file=sys.stderr, flush=True)
        # Not runnable at all (None): the launch itself fails and says so.
        _SUPPORTED = bool(answer)
    return [NO_EXPAND_ENVIRONMENT] if _SUPPORTED else []


def reset_cache() -> None:
    """Forget the cached answer (tests that change ``PATH``)."""

    global _SUPPORTED
    _SUPPORTED = None
