#!/usr/bin/env python3
"""Example project-owned running check for ``wrkslots init``.

Replace the fixture file lookup with the coordinator's durable agent registry. The exit status, not
the printed text, is authoritative: 0 dead, 1 alive, 2 unverifiable.
"""

from __future__ import annotations

import os
import sys
from pathlib import Path


def main() -> int:
    """Read one fixture result and return the registered exit status."""

    if len(sys.argv) != 2:
        print("usage: liveness_probe.py AGENT", file=sys.stderr)
        return 2
    project = Path(os.environ["WRKSLOTS_PROJECT_ROOT"])
    result = project / ".agent-liveness" / sys.argv[1]
    try:
        state = result.read_text(encoding="utf-8").strip()
    except OSError as exc:
        print(f"unverifiable: {exc}")
        return 2
    codes = {"dead": 0, "alive": 1, "unverifiable": 2}
    print(state)
    return codes.get(state, 2)


if __name__ == "__main__":
    raise SystemExit(main())
