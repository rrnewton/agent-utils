#!/usr/bin/env python3
"""Executable entry point.

Runnable three ways, all equivalent:
  * `python -m safe_ci_dag_runner`
  * the installed console script `safe-ci-dag-runner`
  * directly via the `py/bin/safe-ci-dag-runner` symlink (no install needed)

The last case is why we fix up sys.path here: when this file is executed directly, the
package's parent dir (py/) may not be importable yet.
"""

from __future__ import annotations

import os
import sys

_PKG_PARENT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
if _PKG_PARENT not in sys.path:
    sys.path.insert(0, _PKG_PARENT)

from safe_ci_dag_runner.cli import main

if __name__ == "__main__":
    raise SystemExit(main())
