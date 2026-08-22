"""herdr-run — run an allowlisted command in a terminal pane and get its real result back.

The pane belongs to a separate terminal server and is not a child of the calling process, so
whatever constrains that process does not constrain the command: not its network policy, not its
environment, not its lifetime. This package turns that into one narrow, allowlisted, cooperatively
audited interface rather than an ad-hoc pile of send-keys. It is not a same-user containment
boundary; the full trust model is included in the installed guide.

An agent whose sandbox blocks a destination it legitimately needs is ONE use of that, and the one
``herdr-run net-doctor`` checks. It is an example, not the definition.

Two design points are load-bearing and easy to undo by accident: the result is read back from FILES
rather than scraped off the terminal (the terminal has no exit code on it), and readiness keys on
the pane's foreground process group rather than on a prompt regex.
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.2.0"
