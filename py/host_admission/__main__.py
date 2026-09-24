"""Documentation-only module entry point for the host-admission library."""

from __future__ import annotations

import argparse
from importlib.metadata import version
from importlib.resources import files
from typing import Sequence


def _run(argv: Sequence[str] | None = None) -> int:
    """Print library metadata or the packaged user guide."""
    parser = argparse.ArgumentParser(
        prog="python -m host_admission",
        description="Shared-host memory and named-token admission primitives.",
    )
    parser.add_argument("--version", action="version", version=version("host-admission"))
    parser.add_argument("--userguide", action="store_true", help="print the packaged user guide")
    arguments = parser.parse_args(list(argv) if argv is not None else None)
    if arguments.userguide:
        print(files("host_admission").joinpath("USER_GUIDE.md").read_text(), end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(_run())
