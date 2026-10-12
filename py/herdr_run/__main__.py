#!/usr/bin/env python3
"""Executable entry point.

Runnable three ways, all equivalent:
  * `python -m herdr_run`
  * the installed console script `herdr-run`
  * directly, as a script, via a symlink to this file (no install needed)

The last case is why we fix up sys.path here: when this file is executed directly, the directory
holding the package may not be importable yet.
"""

from __future__ import annotations

import os
import sys

_PKG_PARENT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _PKG_PARENT not in sys.path:
    sys.path.insert(0, _PKG_PARENT)

from herdr_run.cli import main  # noqa: E402  (import after the sys.path fixup, by design)

if __name__ == "__main__":
    # When the process must restart in UTF-8 mode it restarts from /proc/self/cmdline. Without
    # /proc it rebuilds the command instead, the same way it was started: as the module, or as
    # this script (through whatever symlink named it).
    _REEXEC = ["-m", "herdr_run"] if __spec__ is not None else [os.path.abspath(sys.argv[0])]
    raise SystemExit(main(reexec=_REEXEC))
