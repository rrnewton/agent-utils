"""Typed failure modes, one per distinguishable outcome.

Each maps to its own process exit code so a caller can branch on WHY it failed without parsing
prose. In particular ``Refused`` (policy said no) must never be confused with ``PaneBusy``
(policy said yes, the terminal was not in a state where we could safely type) or with a non-zero
exit code from the command itself, which is not a herdr-run failure at all.
"""

from __future__ import annotations

__all__ = [
    "HerdrRunError",
    "ConfigError",
    "Refused",
    "HerdrUnavailable",
    "PaneBusy",
    "RunTimeout",
    "EXIT_CONFIG",
    "EXIT_REFUSED",
    "EXIT_UNAVAILABLE",
    "EXIT_BUSY",
    "EXIT_TIMEOUT",
]

#: Exit codes. Kept out of the 0-127 range a wrapped command commonly returns where possible; 0 and
#: any code the wrapped command itself produced are passed through untouched by the CLI, so these
#: are deliberately distinctive values documented in the user guide.
EXIT_CONFIG = 78  # EX_CONFIG: the project config is malformed or unreadable.
EXIT_REFUSED = 77  # EX_NOPERM: the allowlist rejected the command. Nothing was executed.
EXIT_UNAVAILABLE = 69  # EX_UNAVAILABLE: Herdr server / workspace / pane could not be brought up.
EXIT_BUSY = 75  # EX_TEMPFAIL: the pane was not idle. Nothing was executed; retry is meaningful.
EXIT_TIMEOUT = 76  # EX_PROTOCOL: the command was launched but did not finish in time.


class HerdrRunError(Exception):
    """Base class so a caller can catch every herdr-run failure with one except clause."""

    exit_code = 1


class ConfigError(HerdrRunError):
    """The per-project YAML config is missing a required shape, or PyYAML is absent."""

    exit_code = EXIT_CONFIG


class Refused(HerdrRunError):
    """The allowlist rejected the command. NOTHING was sent to any pane.

    This is the security-relevant outcome: it must be raised before any pane interaction, so a
    refusal can never be confused with a command that ran and failed.
    """

    exit_code = EXIT_REFUSED


class HerdrUnavailable(HerdrRunError):
    """The Herdr server, workspace, tab, or pane could not be established."""

    exit_code = EXIT_UNAVAILABLE


class PaneBusy(HerdrRunError):
    """The target pane was not observably idle, so typing into it was unsafe.

    Conservative by construction: we refuse rather than risk interleaving with whatever the pane is
    already doing (or appending to a half-typed command line a human left there).
    """

    exit_code = EXIT_BUSY


class RunTimeout(HerdrRunError):
    """The command was launched in the pane but produced no exit-code file in time.

    The command is NOT killed by default: it is running in a terminal this process does not own.
    """

    exit_code = EXIT_TIMEOUT
