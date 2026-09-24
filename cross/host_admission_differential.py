#!/usr/bin/env python3
"""Require byte-identical Python/Rust decisions for the canonical corpus."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PY_ROOT = ROOT / "py"
FIXTURES = ROOT / "common" / "host-admission" / "fixtures-v1.json"
sys.path.insert(0, str(PY_ROOT))

from host_admission import (  # noqa: E402
    AdmissionPolicy,
    HostAdmissionLedger,
    HostSnapshot,
    LeaseView,
    OwnerState,
    ProcessOwner,
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


def python_bytes() -> tuple[bytes, int]:
    """Evaluate every fixture through the Python core."""
    payload = mapping(json.loads(FIXTURES.read_text(encoding="utf-8")))
    if payload.get("schema") != "host-admission-fixtures/v1":
        raise ValueError("unsupported fixture schema")
    chunks: list[bytes] = []
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
        chunks.append(
            text(case["name"]).encode("utf-8")
            + b"\t"
            + canonical_decision_json(decision).encode("utf-8")
        )
    return b"".join(chunks), len(chunks)


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
        capture_output=True,
        check=False,
        timeout=180,
    )
    if result.returncode != 0:
        print(result.stderr.decode("utf-8", errors="replace"), file=sys.stderr)
        return 1
    python, count = python_bytes()
    rust = result.stdout
    if python != rust:
        mismatch = next(
            (
                index
                for index, (left, right) in enumerate(zip(python, rust))
                if left != right
            ),
            min(len(python), len(rust)),
        )
        print(
            f"byte stream differs at offset {mismatch}: "
            f"python_len={len(python)} rust_len={len(rust)}",
            file=sys.stderr,
        )
        return 1
    with tempfile.TemporaryDirectory(prefix="host-admission-cross-") as temporary:
        root = Path(temporary)
        root.chmod(0o700)
        python_path = root / "python-ledger.json"
        rust_path = root / "rust-ledger.json"
        owner = ProcessOwner("host-a", "boot-a", 10, 100)
        request = ResourceRequest(
            "cross-edition", "parity-test", owner, 123, (("build", 1),), 7, "fixture"
        )
        snapshot = HostSnapshot(1000, "host-a", "boot-a", 1000, 1000, 0, 0, 0, 0)
        policy = AdmissionPolicy(
            memory_budget_bytes=700,
            memory_reserve_bytes=0,
            token_capacities=(("build", 1),),
        )
        decision, lease = HostAdmissionLedger(python_path).request(
            request,
            snapshot,
            policy,
            now_unix_ms=1010,
            owner_probe=lambda _owner, _snapshot: OwnerState.ALIVE,
        )
        if decision.verdict.value != "grant" or lease is None:
            raise AssertionError("Python cross-edition fixture was not granted")
        roundtrip = subprocess.run(
            (
                "cargo", "run", "--quiet", "--manifest-path", str(ROOT / "rs" / "Cargo.toml"),
                "-p", "host-admission", "--example", "ledger-roundtrip", "--",
                str(python_path), str(rust_path),
            ),
            cwd=ROOT,
            env=environment,
            capture_output=True,
            check=False,
            timeout=180,
        )
        if roundtrip.returncode != 0:
            print(roundtrip.stderr.decode("utf-8", errors="replace"), file=sys.stderr)
            return 1
        HostAdmissionLedger(rust_path).validate()
        if python_path.read_bytes() != rust_path.read_bytes():
            print("Python/Rust canonical ledger bytes differ", file=sys.stderr)
            return 1
    print(
        f"host-admission differential: {count} canonical decision byte streams "
        "and bidirectional ledger bytes match"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
