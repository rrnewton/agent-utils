#!/usr/bin/env python3
"""Prove the checked-in run-handle JSON Schema matches its Rust type.

The type in `rs/local-ci-hub/src/run_handle.rs` is the single source
of truth for the `local-ci-hub/run/v1` handle format. The schema
under `validation/schemas/` is generated from that type by the
crate's `generate_run_schemas` example; this checker regenerates it
into a temporary directory and fails closed on any byte difference.

Usage:
    python3 scripts/check_local_ci_hub_schemas.py
    python3 scripts/check_local_ci_hub_schemas.py --write
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
SCHEMA_DIR = REPO_ROOT / "validation" / "schemas"
STEM = "local-ci-hub-run-v1"


def generate(target: Path) -> None:
    """Generate the schema into `target` with the crate example."""
    target.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        ["cargo", "run", "--quiet", "--manifest-path",
         str(REPO_ROOT / "rs" / "Cargo.toml"), "-p", "local-ci-hub",
         "--example", "generate_run_schemas", "--", str(target)],
        check=True, cwd=REPO_ROOT)


def main(argv: list[str] | None = None) -> int:
    """Run the schema check; returns the process exit code."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true",
                        help="overwrite the checked-in schema instead of diffing")
    args = parser.parse_args(argv)
    checked = SCHEMA_DIR / f"{STEM}.schema.json"
    if args.write:
        generate(SCHEMA_DIR)
        print(f"wrote {checked}")
        return 0
    with tempfile.TemporaryDirectory() as tmp:
        generate(Path(tmp))
        fresh = Path(tmp) / f"{STEM}.schema.json"
        if not checked.exists():
            print(f"missing checked-in schema: {checked}", file=sys.stderr)
            return 1
        if fresh.read_bytes() != checked.read_bytes():
            print(f"schema drift: {checked} differs from its Rust type; "
                  "regenerate with scripts/check_local_ci_hub_schemas.py --write",
                  file=sys.stderr)
            return 1
    print("local-ci-hub run-handle schema matches its Rust type")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
