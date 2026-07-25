#!/usr/bin/env python3
"""Python-vs-Rust differential tester.

Assert that the Python and Rust builds of a tool produce identical observable behavior.

Bootstrap scope: check that `--version` stdout is byte-identical and that a small set of
invocations agree on exit code. As the runner is ported, extend INVOCATIONS (and add
input-fixture cases) to cover the load-bearing decisions listed in cross/README.md.
"""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from collections.abc import Sequence
from dataclasses import dataclass

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


@dataclass(frozen=True)
class Invocation:
    name: str
    args: tuple[str, ...]


@dataclass(frozen=True)
class Outcome:
    returncode: int
    stdout: str
    stderr: str


def py_command(tool: str) -> list[str]:
    pkg = tool.replace("-", "_")
    return [sys.executable, "-m", pkg]


def rs_command(tool: str) -> list[str]:
    for candidate in (
        os.path.join(REPO_ROOT, "rs", "target", "release", tool),
        os.path.join(REPO_ROOT, "rs", "bin", tool),
    ):
        if os.path.exists(candidate):
            return [candidate]
    raise FileNotFoundError(
        f"rust binary for {tool!r} not found; run `./setup rs` or `cargo build --release`"
    )


def run(cmd: Sequence[str], inv: Invocation) -> Outcome:
    env = dict(os.environ)
    env["PYTHONPATH"] = (
        os.path.join(REPO_ROOT, "py") + os.pathsep + env.get("PYTHONPATH", "")
    )
    proc = subprocess.run(
        [*cmd, *inv.args],
        capture_output=True,
        text=True,
        env=env,
        check=False,
    )
    return Outcome(proc.returncode, proc.stdout, proc.stderr)


INVOCATIONS: tuple[Invocation, ...] = (
    Invocation("version", ("--version",)),
    Invocation("help", ("--help",)),
    Invocation("noargs", ()),
)


def compare(tool: str) -> int:
    py = py_command(tool)
    rs = rs_command(tool)
    failures = 0
    for inv in INVOCATIONS:
        po = run(py, inv)
        ro = run(rs, inv)
        if po.returncode != ro.returncode:
            print(
                f"DIVERGENCE [{inv.name}] exit: py={po.returncode} rs={ro.returncode}"
            )
            failures += 1
        if inv.name == "version" and po.stdout != ro.stdout:
            print(f"DIVERGENCE [version] stdout: py={po.stdout!r} rs={ro.stdout!r}")
            failures += 1
    if failures:
        print(f"cross[{tool}]: {failures} divergence(s)")
        return 1
    print(f"cross[{tool}]: OK ({len(INVOCATIONS)} invocations agree)")
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="py-vs-rs differential tester")
    parser.add_argument("--tool", default="safe-ci-dag-runner")
    ns = parser.parse_args(list(argv) if argv is not None else None)
    tool = str(ns.tool)
    return compare(tool)


if __name__ == "__main__":
    raise SystemExit(main())
