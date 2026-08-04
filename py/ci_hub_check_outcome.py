#!/usr/bin/env python3
"""Load the content-pinned ci-hub check-outcome authority without copying it."""

from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
from types import ModuleType
from typing import Callable, Sequence, cast
from urllib.request import urlopen


AUTHORITY_COMMIT = "f9e61247e83bb07c11297541b591606de24a89a8"
AUTHORITY_SHA256 = "d37794a8a67b1e565fe68e6435ef2af672b31bfdf2a34ab01ba1037b598bf113"
AUTHORITY_RELATIVE_PATH = Path("ci-hub/check_outcome.py")


def _candidate_authorities() -> list[Path]:
    candidates: list[Path] = []
    if parent := os.environ.get("DEV_HERMIT_PARENT"):
        candidates.append(Path(parent) / AUTHORITY_RELATIVE_PATH)
    for start in (Path(__file__).resolve(), Path.cwd().resolve()):
        candidates.extend(parent / AUTHORITY_RELATIVE_PATH for parent in start.parents)
    return candidates


def _verified_source() -> bytes:
    for candidate in _candidate_authorities():
        if candidate.is_file():
            source = candidate.read_bytes()
            if hashlib.sha256(source).hexdigest() == AUTHORITY_SHA256:
                return source

    url = (
        "https://raw.githubusercontent.com/rrnewton/dev-hermit/"
        f"{AUTHORITY_COMMIT}/{AUTHORITY_RELATIVE_PATH}"
    )
    with urlopen(url, timeout=30) as response:  # exact commit plus digest below
        source = cast(bytes, response.read())
    digest = hashlib.sha256(source).hexdigest()
    if digest != AUTHORITY_SHA256:
        raise RuntimeError(f"canonical check-status authority digest mismatch: {digest}")
    return source


def _load_authority() -> ModuleType:
    with tempfile.NamedTemporaryFile(suffix=".py") as source_file:
        source_file.write(_verified_source())
        source_file.flush()
        spec = importlib.util.spec_from_file_location(
            "ci_hub_check_outcome_authority", source_file.name
        )
        if spec is None or spec.loader is None:
            raise RuntimeError("cannot load canonical check-status authority")
        module = importlib.util.module_from_spec(spec)
        sys.modules[spec.name] = module
        spec.loader.exec_module(module)
        return module


AUTHORITY = _load_authority()
_classify = cast(Callable[[object, object], object], AUTHORITY.classify_check)
FAIL_CONCLUSIONS = cast(frozenset[str], AUTHORITY.FAIL_CONCLUSIONS)


def classify_check(status: object, conclusion: object) -> str:
    """Return the canonical authority's PASSED/FAILED/NO_RESULT value."""
    result = _classify(status, conclusion)
    value = getattr(result, "value", None)
    if value not in ("PASSED", "FAILED", "NO_RESULT"):
        raise RuntimeError(f"canonical check-status authority returned {value!r}")
    return cast(str, value)


def annotate_rollups(value: object) -> object:
    """Attach the canonical result to every check-like object in JSON."""
    if isinstance(value, list):
        return [annotate_rollups(item) for item in value]
    if not isinstance(value, dict):
        return value
    result = {key: annotate_rollups(item) for key, item in value.items()}
    if "status" in value or "conclusion" in value or "state" in value:
        result["_checkOutcome"] = classify_check(
            value.get("status"), value.get("conclusion", value.get("state"))
        )
    return result


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--status", default="")
    parser.add_argument("--conclusion", default="")
    parser.add_argument("--annotate-rollups", action="store_true")
    args = parser.parse_args(argv)
    if args.annotate_rollups:
        json.dump(annotate_rollups(json.load(sys.stdin)), sys.stdout, separators=(",", ":"))
        print()
    else:
        print(classify_check(args.status, args.conclusion))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
