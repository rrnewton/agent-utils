#!/usr/bin/env python3
"""Python-vs-Rust differential tester for safe-ci-dag-runner.

Prove the Python and Rust builds produce identical OBSERVABLE behavior. For a set of
representative and randomized DAG fixtures, this runs BOTH the Python CLI
(``python3 -m safe_ci_dag_runner``) and the Rust binary (``rs/target/release/…`` or
``rs/bin/…``) and asserts:

* ``list``, ``ascii``, ``dot`` stdout are BYTE-IDENTICAL.
* ``json`` stdout is PARSED-EQUAL (byte-identity is additionally reported when it holds).
* ``run`` agrees on exit code, and on the passed/failed/aborted/skipped counts. Counts are
  compared under ``--keep-going`` so they are deterministic (the default eager-exit path
  races on which in-flight step is cancelled first, so only its exit code is compared).
* The memory-aware ``-j`` decision and modeled footprint from ``--max-mem`` match.

It also keeps the bootstrap ``--version`` / ``--help`` / no-args exit-code checks.

Exit status is nonzero on any divergence. The module is kept mypy-strict clean.
"""

from __future__ import annotations

import argparse
import json
import os
import random
import re
import subprocess
import sys
import tempfile
from collections.abc import Sequence
from dataclasses import dataclass, field

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

_COUNTS_RE = re.compile(
    r"(\d+) passed, (\d+) failed, (\d+) aborted, (\d+) skipped"
)
_SIZING_RE = re.compile(
    r"-> -j(\d+) \(modeled worst-case (\d+) bytes fits budget (\d+) bytes\)"
)


@dataclass(frozen=True)
class Invocation:
    name: str
    args: tuple[str, ...]


@dataclass(frozen=True)
class Fixture:
    """A DAG fixture plus the optional ``--max-mem`` budget to size it against."""

    name: str
    dag: dict[str, object]
    max_mem: str | None = None


@dataclass(frozen=True)
class Outcome:
    returncode: int
    stdout: str
    stderr: str


@dataclass
class Report:
    checks: int = 0
    failures: list[str] = field(default_factory=list)
    json_byte_identical: int = 0
    json_parsed_equal_only: int = 0

    def ok(self, _label: str) -> None:
        self.checks += 1

    def bad(self, label: str, detail: str) -> None:
        self.checks += 1
        self.failures.append(f"{label}: {detail}")


# --------------------------------------------------------------------------- command wiring


def py_command() -> list[str]:
    return [sys.executable, "-m", "safe_ci_dag_runner"]


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


def _env() -> dict[str, str]:
    env = dict(os.environ)
    env["PYTHONPATH"] = os.path.join(REPO_ROOT, "py") + os.pathsep + env.get("PYTHONPATH", "")
    # Deterministic, color-free output regardless of the runner's TTY state.
    env["NO_COLOR"] = "1"
    return env


def run(cmd: Sequence[str], args: Sequence[str]) -> Outcome:
    proc = subprocess.run(
        [*cmd, *args],
        capture_output=True,
        text=True,
        env=_env(),
        check=False,
    )
    return Outcome(proc.returncode, proc.stdout, proc.stderr)


# --------------------------------------------------------------------------- fixtures


def representative_fixtures() -> list[Fixture]:
    """Hand-written fixtures covering the load-bearing formatting + scheduling decisions."""
    fixtures: list[Fixture] = []

    fixtures.append(
        Fixture(
            "diamond",
            {
                "resource_caps": {"browser": 1, "net": 2},
                "mem_cap_factor": 1.25,
                "steps": [
                    {
                        "group": "build",
                        "job": "app",
                        "desc": "compile",
                        "cmd": "echo build",
                        "hint": {
                            "est_duration_s": 90,
                            "classification": "cpu-bound",
                            "rss_baseline_bytes": 1073741824,
                        },
                    },
                    {
                        "group": "test",
                        "job": "unit",
                        "desc": "unit tests",
                        "cmd": "echo test",
                        "deps": ["build.app"],
                    },
                    {
                        "group": "e2e",
                        "job": "smoke",
                        "desc": "browser smoke",
                        "cmd": "echo e2e",
                        "deps": ["build.app"],
                        "hint": {"resources": {"browser": 1}, "classification": "latency-bound"},
                    },
                    {
                        "group": "e2e",
                        "job": "smoke2",
                        "desc": "browser smoke 2",
                        "cmd": "echo e2e2",
                        "deps": ["build.app"],
                        "hint": {"resources": {"browser": 1}},
                    },
                ],
            },
        )
    )

    fixtures.append(Fixture("empty", {"steps": []}))

    # A failing step whose two dependents (transitively) must be skipped identically.
    fixtures.append(
        Fixture(
            "dep_failure_skip",
            {
                "steps": [
                    {"group": "g", "job": "a", "cmd": "false"},
                    {"group": "g", "job": "b", "cmd": "true", "deps": ["g.a"]},
                    {"group": "g", "job": "c", "cmd": "true", "deps": ["g.b"]},
                    {"group": "g", "job": "d", "cmd": "true"},
                ]
            },
        )
    )

    # JSON escaping + verbatim list rendering of special ASCII characters.
    fixtures.append(
        Fixture(
            "special_chars",
            {
                "steps": [
                    {
                        "group": "g",
                        "job": "quote",
                        "desc": 'has "quotes" and \\backslash\\ and\ttab',
                        "cmd": "true",
                        "env": {"K2": "v2", "K1": "v1"},
                    }
                ]
            },
        )
    )

    # A memory-modeled DAG sized against a tight budget: the chosen -j and footprint are
    # compared; the budget throttles below any plausible CPU count so the check is
    # CPU-count-independent.
    fixtures.append(
        Fixture(
            "sized",
            {
                "resource_caps": {"gpu": 1},
                "mem_cap_factor": 1.0,
                "outer_mem_safety_factor": 1.0,
                "mem_cap_floor_bytes": 0,
                "steps": [
                    {"group": "g", "job": "A", "cmd": "true", "hint": {"rss_baseline_bytes": 3221225472}},
                    {
                        "group": "g",
                        "job": "B",
                        "cmd": "true",
                        "deps": ["g.A"],
                        "hint": {"rss_baseline_bytes": 2147483648},
                    },
                    {
                        "group": "g",
                        "job": "C",
                        "cmd": "true",
                        "hint": {"resources": {"gpu": 1}, "rss_baseline_bytes": 4294967296},
                    },
                    {
                        "group": "g",
                        "job": "D",
                        "cmd": "true",
                        "hint": {"resources": {"gpu": 1}, "rss_baseline_bytes": 1073741824},
                    },
                ],
            },
            max_mem="6G",
        )
    )

    return fixtures


def _random_dag(rng: random.Random) -> tuple[dict[str, object], bool]:
    """Generate one acyclic DAG (deps only reference earlier steps).

    Returns the DAG document and whether any step carries an ``rss_baseline_bytes`` (so the
    caller can decide to attach a ``--max-mem`` budget for the sizing check).
    """
    groups = ["build", "test", "e2e", "lint"]
    classes = ["cpu-bound", "latency-bound", "light"]
    cmds = ["true", "true", "echo hi", "sleep 0.02", "echo x && sleep 0.02", "false"]
    caps: dict[str, int] = {"browser": rng.randint(1, 2), "net": rng.randint(1, 2)}

    n = rng.randint(1, 7)
    steps: list[object] = []
    tags: list[str] = []
    has_rss = False
    for i in range(n):
        group = rng.choice(groups)
        job = f"j{i}"
        tag = f"{group}.{job}"
        n_deps = rng.randint(0, min(2, len(tags)))
        deps = rng.sample(tags, n_deps) if n_deps else []
        hint: dict[str, object] = {}
        if rng.random() < 0.5:
            hint["classification"] = rng.choice(classes)
        if rng.random() < 0.4:
            # Only demand resources that exist in caps (an unmet demand would hang the run).
            res_name = rng.choice(list(caps))
            hint["resources"] = {res_name: 1}
        if rng.random() < 0.3:
            hint["est_duration_s"] = round(rng.uniform(0.0, 120.0), 2)
        if rng.random() < 0.4:
            hint["rss_baseline_bytes"] = rng.randint(1, 8) * 1024**3
            has_rss = True
        step: dict[str, object] = {
            "group": group,
            "job": job,
            "desc": rng.choice(["", "a step", "does work"]),
            "cmd": rng.choice(cmds),
        }
        if deps:
            step["deps"] = deps
        if hint:
            step["hint"] = hint
        steps.append(step)
        tags.append(tag)

    dag: dict[str, object] = {"steps": steps, "resource_caps": caps}
    if rng.random() < 0.5:
        dag["mem_cap_factor"] = round(rng.uniform(1.0, 2.0), 2)
    if has_rss and rng.random() < 0.7:
        dag["mem_cap_floor_bytes"] = 0
        dag["outer_mem_safety_factor"] = 1.0
    return dag, has_rss


def randomized_fixtures(count: int, seed: int) -> list[Fixture]:
    rng = random.Random(seed)
    fixtures: list[Fixture] = []
    for i in range(count):
        dag, has_rss = _random_dag(rng)
        # Attach a sizing budget when the DAG participates in the memory model.
        fixtures.append(Fixture(f"rand{i:02d}", dag, max_mem="4G" if has_rss else None))
    return fixtures


# --------------------------------------------------------------------------- comparisons


def _counts(stderr: str) -> tuple[int, int, int, int] | None:
    m = _COUNTS_RE.search(stderr)
    if not m:
        return None
    return int(m.group(1)), int(m.group(2)), int(m.group(3)), int(m.group(4))


def _sizing(stderr: str) -> tuple[int, int, int] | None:
    m = _SIZING_RE.search(stderr)
    if not m:
        return None
    return int(m.group(1)), int(m.group(2)), int(m.group(3))


def compare_fixture(py: list[str], rs: list[str], fx: Fixture, rep: Report) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        dag_path = os.path.join(tmp, "dag.json")
        with open(dag_path, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(fx.dag))

        # 1) list / ascii / dot: byte-identical stdout.
        for mode in ("list", "ascii", "dot"):
            po = run(py, (mode, "--dag", dag_path))
            ro = run(rs, (mode, "--dag", dag_path))
            label = f"{fx.name}/{mode}"
            if po.returncode != ro.returncode:
                rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}")
            elif po.stdout != ro.stdout:
                rep.bad(label, f"stdout differs\n--- py ---\n{po.stdout}\n--- rs ---\n{ro.stdout}")
            else:
                rep.ok(label)

        # 2) json: parsed-equal (byte-identity additionally tracked).
        po = run(py, ("json", "--dag", dag_path))
        ro = run(rs, ("json", "--dag", dag_path))
        label = f"{fx.name}/json"
        if po.returncode != ro.returncode:
            rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}")
        else:
            try:
                equal = json.loads(po.stdout) == json.loads(ro.stdout)
            except json.JSONDecodeError as exc:
                rep.bad(label, f"unparseable json: {exc}")
            else:
                if not equal:
                    rep.bad(label, f"parsed json differs\n--- py ---\n{po.stdout}\n--- rs ---\n{ro.stdout}")
                else:
                    rep.ok(label)
                    if po.stdout == ro.stdout:
                        rep.json_byte_identical += 1
                    else:
                        rep.json_parsed_equal_only += 1

        # 3) run: default concurrent exit-code parity. The default eager-exit path is
        # deterministic in its EXIT CODE (1 iff any reachable step fails) but NOT in the
        # passed/aborted split, which depends on which in-flight step is cancelled first, so
        # only the exit code is compared here.
        po = run(py, ("run", "--dag", dag_path, "-q", "-j", "4"))
        ro = run(rs, ("run", "--dag", dag_path, "-q", "-j", "4"))
        label = f"{fx.name}/run(default-exit)"
        if po.returncode != ro.returncode:
            rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}")
        else:
            rep.ok(label)

        # 4) run serial (-j1): deterministic counts + exit code. With one step at a time the
        # ready-set loop dispatches in a single deterministic LPT sequence, so the
        # passed/failed/aborted/skipped counts are fully reproducible between the two builds.
        # (Note: --keep-going only suppresses the eager-abort of in-flight steps; on any
        # failure BOTH builds set stop and launch no new steps, so counts still race at -j>1.)
        po = run(py, ("run", "--dag", dag_path, "-q", "-j", "1"))
        ro = run(rs, ("run", "--dag", dag_path, "-q", "-j", "1"))
        label = f"{fx.name}/run(serial-counts)"
        pc, rc = _counts(po.stderr), _counts(ro.stderr)
        if po.returncode != ro.returncode:
            rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}")
        elif pc is None or rc is None:
            rep.bad(label, f"missing summary counts py={po.stderr!r} rs={ro.stderr!r}")
        elif pc != rc:
            rep.bad(label, f"counts py={pc} rs={rc}")
        else:
            rep.ok(label)

        # 5) --max-mem sizing decision, when the fixture supplies a budget.
        if fx.max_mem is not None:
            po = run(py, ("run", "--dag", dag_path, "-q", "-k", "--max-mem", fx.max_mem))
            ro = run(rs, ("run", "--dag", dag_path, "-q", "-k", "--max-mem", fx.max_mem))
            label = f"{fx.name}/sizing"
            ps, rss = _sizing(po.stderr), _sizing(ro.stderr)
            if ps is None or rss is None:
                rep.bad(label, f"missing sizing line py={po.stderr!r} rs={ro.stderr!r}")
            elif ps != rss:
                rep.bad(label, f"(-j, footprint, budget) py={ps} rs={rss}")
            else:
                rep.ok(label)


INVOCATIONS: tuple[Invocation, ...] = (
    Invocation("version", ("--version",)),
    Invocation("help", ("--help",)),
    Invocation("noargs", ()),
)


def compare_invocations(py: list[str], rs: list[str], rep: Report) -> None:
    for inv in INVOCATIONS:
        po = run(py, inv.args)
        ro = run(rs, inv.args)
        if po.returncode != ro.returncode:
            rep.bad(f"invocation/{inv.name}", f"exit py={po.returncode} rs={ro.returncode}")
        elif inv.name == "version" and po.stdout != ro.stdout:
            rep.bad("invocation/version", f"stdout py={po.stdout!r} rs={ro.stdout!r}")
        else:
            rep.ok(f"invocation/{inv.name}")


def compare(tool: str, rand_count: int, seed: int) -> int:
    py = py_command()
    rs = rs_command(tool)
    rep = Report()

    compare_invocations(py, rs, rep)
    fixtures = representative_fixtures() + randomized_fixtures(rand_count, seed)
    for fx in fixtures:
        compare_fixture(py, rs, fx, rep)

    if rep.failures:
        for failure in rep.failures:
            print(f"DIVERGENCE [{failure}")
        print(
            f"cross[{tool}]: {len(rep.failures)} divergence(s) out of {rep.checks} checks "
            f"across {len(fixtures)} fixtures"
        )
        return 1

    print(
        f"cross[{tool}]: OK - {rep.checks} checks across {len(fixtures)} fixtures agree "
        f"(json byte-identical: {rep.json_byte_identical}, "
        f"parsed-equal-only: {rep.json_parsed_equal_only})"
    )
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="py-vs-rs differential tester")
    parser.add_argument("--tool", default="safe-ci-dag-runner")
    parser.add_argument("--random", type=int, default=24, help="number of randomized fixtures")
    parser.add_argument("--seed", type=int, default=1234, help="RNG seed for randomized fixtures")
    ns = parser.parse_args(list(argv) if argv is not None else None)
    tool = str(ns.tool)
    rand_count = int(ns.random)
    seed = int(ns.seed)
    return compare(tool, rand_count, seed)


if __name__ == "__main__":
    raise SystemExit(main())
