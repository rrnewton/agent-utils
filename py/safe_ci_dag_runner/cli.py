"""Command-line interface for safe-ci-dag-runner."""

from __future__ import annotations

import argparse
import sys
from collections.abc import Sequence

from safe_ci_dag_runner import __version__

PROG = "safe-ci-dag-runner"


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog=PROG,
        description="Run a DAG of CI/build steps under nested cgroup CPU/memory boxing.",
    )
    parser.add_argument(
        "--version", action="version", version=f"{PROG} {__version__}"
    )
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    parser = build_parser()
    parser.parse_args(list(argv) if argv is not None else None)
    # No subcommand yet: the runner is still being ported.
    print(
        f"{PROG} {__version__}: no command given (runner port in progress).",
        file=sys.stderr,
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
