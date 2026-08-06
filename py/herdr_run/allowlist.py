"""Decide whether a requested command may cross the sandbox boundary, and render it safely.

Two separate jobs, deliberately not conflated:

1. **Admission** — is this program on the project's allowlist? Answered on the SPLIT argv, never on
   the raw string, so quoting tricks cannot change which program the policy thinks it is admitting.
2. **Rendering** — turn the admitted argv back into a shell word list. Every token is passed through
   :func:`shlex.quote`, so the rendered command contains exactly the argv that was admitted. This is
   why there is no metacharacter blocklist here: injection is prevented by CONSTRUCTION rather than
   by trying to enumerate ``;``, ``&&``, backticks, ``$()``, newlines, and so on. A blocklist of
   shell metacharacters is a proxy for "cannot start a second command"; re-quoting is the property
   itself.

The remaining privilege is whatever the allowlisted program can do on its own. That residue is real
and documented in the user guide's threat model; :data:`Config.deny_global` and friends shave off
the best-known self-escapes (``git -c alias.x='!sh'``) as defense in depth, not as the boundary.
"""

from __future__ import annotations

import os
import shlex
from dataclasses import dataclass

from herdr_run.config import Config
from herdr_run.errors import Refused

__all__ = ["Admission", "admit", "render", "expand_tilde"]


@dataclass(frozen=True)
class Admission:
    """A command the policy accepted, with the reasoning kept for the audit record."""

    #: Full argv as split, including any allowed prefix wrappers.
    argv: tuple[str, ...]
    #: The allowed wrapper chain that preceded the program (e.g. ``("with-proxy",)``).
    prefix: tuple[str, ...]
    #: The allowlisted program itself (e.g. ``"git"``).
    program: str
    #: The program's first non-option argument, when it has one (e.g. ``"ls-remote"``).
    subcommand: str | None

    @property
    def rendered(self) -> str:
        """The exact shell text that will be executed, safe to embed in a larger shell command."""
        return render(self.argv)


def expand_tilde(token: str) -> str:
    """Expand a LEADING ``~`` / ``~user`` in one token, the way a shell would.

    Quoting every token is what makes injection impossible, but it also suppresses the one
    expansion callers reasonably expect: ``git -C ~/work/repo`` arrived at git as a literal ``~``
    and failed with ``cannot change to '~/work/repo'``. Doing the expansion HERE keeps the security
    property intact -- Python resolves the path and the result is still quoted, so the shell never
    gets a chance to interpret anything.

    Only a leading tilde is touched. A tilde anywhere else is an ordinary character (``a~b`` stays
    ``a~b``), and a token that ``expanduser`` cannot resolve is returned unchanged.
    """
    if not token.startswith("~"):
        return token
    return os.path.expanduser(token)


def render(argv: tuple[str, ...]) -> str:
    """Quote every token so the resulting shell word list is exactly ``argv``."""
    return " ".join(shlex.quote(expand_tilde(token)) for token in argv)


def _split(command: str) -> tuple[str, ...]:
    try:
        # comments=False: a '#' is a literal argument character here (e.g. `gh issue view '#12'`),
        # not a comment introducer. Letting shlex strip from '#' onward would silently TRUNCATE a
        # command after admission, so the audited text and the executed text would disagree.
        tokens = shlex.split(command, comments=False, posix=True)
    except ValueError as exc:
        raise Refused(f"cannot parse command (unbalanced quoting?): {exc}") from exc
    if not tokens:
        raise Refused("empty command")
    return tuple(tokens)


def admit(command: str, config: Config) -> Admission:
    """Admit ``command`` under ``config``, or raise :class:`Refused` explaining exactly why.

    Raises before any pane is touched, so a refusal never leaves partial state anywhere.
    """
    argv = _split(command)

    # Peel allowed wrapper prefixes. Each wrapper may appear at most once, so `with-proxy with-proxy
    # ... ` cannot be used to pad an argv into a shape the later checks read differently.
    prefix: list[str] = []
    rest = list(argv)
    while rest and rest[0] in config.prefixes:
        wrapper = rest.pop(0)
        if wrapper in prefix:
            raise Refused(f"prefix {wrapper!r} repeated; each wrapper may appear at most once")
        prefix.append(wrapper)

    if not rest:
        raise Refused(
            f"command is only wrapper prefixes ({' '.join(prefix)}) with no program. "
            f"Allowed programs: {', '.join(sorted(config.allow))}"
        )

    program = rest[0]

    # A bare name only. An explicit path (`/bin/sh`, `./git`, `../x/gh`) would let any binary be
    # presented under an allowlisted-looking basename, so the name must be resolved by the pane's
    # PATH rather than chosen by the caller.
    if "/" in program:
        raise Refused(
            f"program must be a bare command name resolved from PATH, not a path: {program!r}"
        )

    if program not in config.allow:
        allowed = ", ".join(sorted(config.allow))
        hint = ""
        if program in config.prefixes:
            hint = f" ({program!r} is a wrapper prefix, not a program)"
        raise Refused(f"program {program!r} is not allowlisted{hint}. Allowed: {allowed}")

    args = rest[1:]

    for token in args:
        # Match on the option NAME so `--upload-pack=/tmp/evil` is caught as well as the
        # space-separated `--upload-pack /tmp/evil`.
        if token.split("=", 1)[0] in config.deny_anywhere:
            raise Refused(f"option {token!r} is denied: it names a program for {program} to execute")

    # Global options are the tokens BEFORE the first non-option token; that is where git puts the
    # config/exec-path switches that turn `git` into an arbitrary-code runner.
    deny_global = config.deny_global.get(program, ())
    value_options = config.value_options.get(program, ())
    subcommand: str | None = None
    expect_value = False
    for token in args:
        if expect_value:
            # This token is the VALUE of the preceding option (`git -C <path>`), not the
            # subcommand. Without this, `git -C /tmp/repo log` would read `/tmp/repo` as the
            # subcommand and every subcommand-level rule would be looking at the wrong token.
            expect_value = False
            continue
        if not token.startswith("-"):
            subcommand = token
            break
        base = token.split("=", 1)[0]
        if base in deny_global:
            raise Refused(
                f"global option {base!r} is denied for {program}: it can make {program} execute "
                "arbitrary code"
            )
        if base in value_options and "=" not in token:
            expect_value = True

    # A per-program subcommand ALLOWLIST, when present, is fail-closed: it is checked before the
    # deny-list and refuses anything not named, including a bare program with no subcommand. This
    # is what makes `cargo` admissible at all -- `cargo fetch` only downloads, while `cargo build`
    # would execute build scripts from third-party crates OUTSIDE the sandbox.
    allowed_subcommands = config.allow_subcommand.get(program)
    if allowed_subcommands is not None:
        if subcommand is None:
            raise Refused(
                f"{program} requires a subcommand, and only these are allowed: "
                f"{', '.join(sorted(allowed_subcommands))}"
            )
        if subcommand not in allowed_subcommands:
            raise Refused(
                f"subcommand '{program} {subcommand}' is not allowlisted. Allowed: "
                f"{', '.join(sorted(allowed_subcommands))}. "
                f"{program} subcommands that compile or execute code are deliberately excluded: "
                "fetch through this tool, then build in-jail with --offline against the warm cache."
            )

    if subcommand is not None and subcommand in config.deny_subcommand.get(program, ()):
        raise Refused(f"subcommand '{program} {subcommand}' is denied: it defines or runs arbitrary code")

    return Admission(argv=argv, prefix=tuple(prefix), program=program, subcommand=subcommand)
