"""herdr-run — run an allowlisted command in a Herdr pane outside the agent sandbox.

An agent process may be confined by a sandbox whose network policy blocks a destination needed to
publish work. The Herdr terminal server runs outside that confinement, so a shell in a Herdr pane
is not confined either. This package turns that observation into one narrow, allowlisted,
cooperatively audited interface rather than an ad-hoc pile of send-keys. It is not a same-user
containment boundary; the full trust model is included in the installed guide.

Two design points are load-bearing and easy to undo by accident: the result is read back from FILES
rather than scraped off the terminal (the terminal has no exit code on it), and readiness keys on
the pane's foreground process group rather than on a prompt regex.
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.1.0"
