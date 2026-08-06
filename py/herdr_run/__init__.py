"""herdr-run — run an allowlisted command in a Herdr pane outside the agent sandbox.

An agent process is confined (BpfJailer + a per-destination forward-proxy allowlist), so
``with-proxy git ls-remote https://github.com/...`` fails in-jail with ``CONNECT tunnel failed,
response 403``. The Herdr terminal server runs OUTSIDE that confinement, so a shell in a Herdr pane
is not confined either. This package turns that observation into ONE narrow, audited, allowlisted
door rather than an ad-hoc pile of send-keys.

Two design points are load-bearing and easy to undo by accident: the result is read back from FILES
rather than scraped off the terminal (the terminal has no exit code on it), and readiness keys on
the pane's foreground process group rather than on a prompt regex.
"""

from __future__ import annotations

__all__ = ["__version__"]

__version__ = "0.1.0"
