#!/usr/bin/env python3
"""Prove the checked-in agent-queue JSON Schemas match their Rust types.

The format types in `rs/agent-queue/src/schema.rs` are the single source
of truth for the queue item, cancellation tombstone and quarantine
sidecar formats (#723 rust-primary-schema-collapse). The schemas under
`validation/schemas/` are generated from those types by the crate's
`generate_schemas` example; this checker regenerates them into a
temporary directory and fails closed on any byte difference.

Usage:
    python3 scripts/check_agent_queue_schemas.py
    python3 scripts/check_agent_queue_schemas.py --self-test
    python3 scripts/check_agent_queue_schemas.py --write
"""

from __future__ import annotations

import argparse
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
SCHEMA_DIR = REPO_ROOT / "validation" / "schemas"
STEMS = ("queue-item", "tombstone", "quarantine-sidecar")
# Schemas owned by sibling checkers (#723 slices); not unexpected here.
SIBLING_STEMS = ("wrkslots-active-state", "wrkslots-archived-state", "wrkslots-handoff-sidecar", "wrkslots-handoff-sidecar-v1", "wrkviz-timeline-v3-bootstrap")


def generate(target: Path) -> None:
    target.mkdir(parents=True, exist_ok=True)
    subprocess.run(
        [
            "cargo",
            "run",
            "--quiet",
            "--manifest-path",
            str(REPO_ROOT / "rs" / "Cargo.toml"),
            "-p",
            "agent-queue",
            "--example",
            "generate_schemas",
            "--",
            str(target),
        ],
        check=True,
        cwd=REPO_ROOT,
    )


def differences(generated: Path, checked_in: Path) -> list[str]:
    failures: list[str] = []
    if checked_in.is_dir():
        expected = {f"{stem}.schema.json" for stem in STEMS + SIBLING_STEMS}
        for extra in sorted(checked_in.glob("*.schema.json")):
            if extra.name not in expected:
                failures.append(f"agent-queue schema unexpected: validation/schemas/{extra.name}")
    for stem in STEMS:
        name = f"{stem}.schema.json"
        fresh = generated / name
        current = checked_in / name
        if not current.is_file():
            failures.append(f"agent-queue schema missing: validation/schemas/{name}")
        elif not fresh.is_file() or fresh.read_bytes() != current.read_bytes():
            failures.append(
                f"agent-queue schema stale: validation/schemas/{name}; "
                "regenerate with scripts/check_agent_queue_schemas.py --write"
            )
    return failures


def self_test() -> int:
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        generated = root / "generated"
        # This tests the byte comparator, not the Rust generator (the freshness
        # node runs that separately). A synthetic output makes its negative
        # controls cheap and independent of Cargo and build-directory locking.
        generated.mkdir()
        for stem in STEMS:
            (generated / f"{stem}.schema.json").write_bytes(b'{"type":"object"}\n')
        if differences(generated, generated):
            print("self-test FAILED: identical schemas were reported stale")
            return 1
        stale = root / "stale"
        stale.mkdir()
        for stem in STEMS:
            (stale / f"{stem}.schema.json").write_bytes(
                (generated / f"{stem}.schema.json").read_bytes()
            )
        for stem in STEMS:
            name = f"{stem}.schema.json"
            current = stale / name
            fresh = (generated / name).read_bytes()
            current.write_bytes(fresh + b" ")
            if not any(name in failure for failure in differences(generated, stale)):
                print(f"self-test FAILED: tampered {name} was not detected")
                return 1
            current.unlink()
            if not any(name in failure for failure in differences(generated, stale)):
                print(f"self-test FAILED: missing {name} was not detected")
                return 1
            current.write_bytes(fresh)
    print("check_agent_queue_schemas self-test: ok")
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--write", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    if args.write:
        generate(SCHEMA_DIR)
        print(f"wrote {len(STEMS)} schemas to validation/schemas/")
        return 0
    with tempfile.TemporaryDirectory() as tmp:
        generated = Path(tmp)
        generate(generated)
        failures = differences(generated, SCHEMA_DIR)
    for failure in failures:
        print(failure)
    if failures:
        return 1
    print("agent-queue schemas: current")
    return 0


if __name__ == "__main__":
    sys.exit(main())
