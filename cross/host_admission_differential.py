#!/usr/bin/env python3
"""Require byte-identical Python/Rust decisions for the canonical corpus."""

from __future__ import annotations

import json
import os
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PY_ROOT = ROOT / "py"
FIXTURES = ROOT / "common" / "host-admission" / "fixtures-v1.json"
sys.path.insert(0, str(PY_ROOT))

from host_admission import (  # noqa: E402
    AdmissionPolicy,
    HostSnapshot,
    LeaseView,
    OwnerState,
    QueueView,
    ResourceRequest,
    canonical_decision_json,
    decide,
)


def mapping(raw: object) -> dict[str, object]:
    """Narrow one JSON object."""
    if not isinstance(raw, dict) or not all(isinstance(key, str) for key in raw):
        raise ValueError("expected object")
    return raw


def sequence(raw: object) -> list[object]:
    """Narrow one JSON array."""
    if not isinstance(raw, list):
        raise ValueError("expected array")
    return raw


def text(raw: object) -> str:
    """Narrow one JSON string."""
    if not isinstance(raw, str):
        raise ValueError("expected string")
    return raw


def integer(raw: object) -> int:
    """Narrow one JSON integer."""
    if isinstance(raw, bool) or not isinstance(raw, int):
        raise ValueError("expected integer")
    return raw


def python_lines() -> list[str]:
    """Evaluate every fixture through the Python core."""
    payload = mapping(json.loads(FIXTURES.read_text(encoding="utf-8")))
    if payload.get("schema") != "host-admission-fixtures/v1":
        raise ValueError("unsupported fixture schema")
    lines: list[str] = []
    for raw_case in sequence(payload["cases"]):
        case = mapping(raw_case)
        request = ResourceRequest.from_json(case["request"])
        leases = tuple(
            LeaseView(text(mapping(raw)["lease_id"]), ResourceRequest.from_json(mapping(raw)["request"]), 1)
            for raw in sequence(case["leases"])
        )
        queue = tuple(
            QueueView(ResourceRequest.from_json(mapping(raw)["request"]), integer(mapping(raw)["sequence"]))
            for raw in sequence(case["queue"])
        )
        states = {key: OwnerState(text(value)) for key, value in mapping(case["owner_states"]).items()}
        decision = decide(
            request,
            HostSnapshot.from_json(case["snapshot"]),
            AdmissionPolicy.from_json(case["policy"]),
            leases,
            queue,
            now_unix_ms=integer(case["now_unix_ms"]),
            owner_states=states,
        )
        expected = mapping(case["expect"])
        if decision.verdict.value != text(expected["verdict"]) or decision.code != text(expected["code"]):
            raise AssertionError(f"{text(case['name'])}: expected {expected}, got {decision.to_json()}")
        lines.append(f"{text(case['name'])}\t{canonical_decision_json(decision).rstrip()}")
    return lines


def main() -> int:
    """Run the paired implementation and compare complete decision bytes."""
    environment = dict(os.environ)
    environment["PATH"] = f"{Path.home() / '.cargo' / 'bin'}:{environment.get('PATH', '')}"
    result = subprocess.run(
        (
            "cargo", "run", "--quiet", "--manifest-path", str(ROOT / "rs" / "Cargo.toml"),
            "-p", "host-admission", "--example", "fixture-decisions", "--", str(FIXTURES),
        ),
        cwd=ROOT,
        env=environment,
        text=True,
        capture_output=True,
        check=False,
        timeout=180,
    )
    if result.returncode != 0:
        print(result.stderr, file=sys.stderr)
        return 1
    python = python_lines()
    rust = result.stdout.splitlines()
    if python != rust:
        for index, (left, right) in enumerate(zip(python, rust)):
            if left != right:
                print(f"case {index} differs\npython: {left}\nrust:   {right}", file=sys.stderr)
                break
        if len(python) != len(rust):
            print(f"line count differs: python={len(python)} rust={len(rust)}", file=sys.stderr)
        return 1
    print(f"host-admission differential: {len(python)} canonical decisions match")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
