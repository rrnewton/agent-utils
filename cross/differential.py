#!/usr/bin/env python3
"""Python-vs-Rust differential tester for safe-ci-dag-runner.

Prove the Python and Rust builds produce identical OBSERVABLE behavior. For a set of
representative and randomized DAG fixtures, this runs BOTH the Python CLI
(``python3 -m safe_ci_dag_runner``) and a private copy of the executable resolved by the tracked
``rs/bin`` Cargo launcher and asserts:

* ``list``, ``ascii``, ``dot`` stdout are BYTE-IDENTICAL.
* ``json`` stdout is BYTE-IDENTICAL (both builds emit ``ensure_ascii=False`` canonical JSON, so
  the bytes match for every input — including multi-line / quote / backslash / control-char /
  unicode ``description`` fields, and floats in scientific notation).
* Every ``.yaml`` fixture is ISOMORPHIC to JSON: loaded in BOTH builds and re-emitted as
  canonical JSON, the bytes match; and each ``examples/NAME.{json,yaml}`` pair loads to the same
  DAG.
* Scalar-resolution PARITY: a battery of adversarial scalars (the YAML "Norway problem",
  octal/underscore/sexagesimal/timestamp tokens, non-finite and overflow floats, and
  out-of-i64-range integers) is loaded in BOTH builds and must yield the SAME accept/reject exit
  code — and byte-identical JSON whenever both accept. This catches PyYAML-1.1-vs-serde_norway-1.2
  and Python-json-vs-serde_json divergences that the isomorphism check (which only covers
  documents both builds accept) cannot.
* ``run`` agrees on exit code, and on the passed/failed/aborted/intentionally-skipped/
  dependency-skipped counts. Counts are
  compared under ``--keep-going`` so they are deterministic (the default eager-exit path
  races on which in-flight step is cancelled first, so only its exit code is compared).
* ``--only`` selection (Feature A) agrees: running EXACTLY one named step matches on exit code
  and counts, and an unknown ``--only`` tag exits 2 on both builds. A successful ``sweep`` must
  produce the same width rows and table schema; measured timing cells and the ``--profile`` table
  are not byte-compared because runtimes legitimately differ.
* The memory-aware ``--max-steps`` decision and modeled footprint from ``--max-mem`` match,
  including effective-width scaling, hard/default/engine-only runnable steps, prompt refusal when
  even one step cannot fit, CPA infeasibility driven by learned RSS, and signed-64 saturation.
* ``run`` keeps active-step fan-out (``-s``) independent from its total CPU budget
  (``-j`` / ``--max-cpus``): the same fork-based guest workload records normalized step/worker
  overlap under both engines, including two full-width steps whose aggregate live workers exceed
  the budget while each individual width remains bounded. A declared over-budget self-managed
  width is refused before spawn, and profile-derived recommendations shown by every run planner
  remain within the same per-step ceiling. A capability-gated boxed case additionally proves that
  this allowed worker oversubscription remains inside the live outer ``cpu.max`` and long-window
  ``cpu.stat`` bandwidth envelope.
* The auto-logging profile STORE (Feature D) has an identical on-disk schema across builds: an
  unboxed run under each build (into separate ``--perf-dir`` dirs) writes the SAME set of CSV
  filenames — so ``machine_id`` + ``container_class`` (and hence ``nproc``) agree — with
  byte-identical HEADER rows and the SAME line-ending style. Data rows (timestamps, elapsed, git
  SHA) legitimately differ and are not compared. The dynamic cgroup ``cpu.*`` columns only appear
  under boxing (out of scope for the unboxed differential); their alphabetical ordering is pinned
  by each build's own perflog tests.
* The ``sweep`` ``--jobs`` error text (malformed range / not-an-integer) matches across builds, and
  a step with an empty effective ``jobs_flag`` is refused because its guest width cannot vary.
* The profile-store FEEDBACK loop + ``--planner`` agree (``compare_plan_feedback``): against a FIXED
  synthetic store, ``plan`` output is byte-identical across builds for BOTH planners and BOTH formats
  (so the contention-discounted median durations, high-percentile rss estimates, and dispatch order
  all match); the ``critical-path`` order differs from ``greedy-lpt`` (the planner really reorders);
  the hint-only ``--no-profile-feedback`` plan is also identical; and the ``--max-mem`` sizing fed by
  the store's rss estimates matches across builds and throttles below the CPU count.
* The remaining ``run`` comparisons pass ``--no-profile`` (no store WRITE into the harness CWD) and
  ``--no-profile-feedback`` (no store READ / hint refinement), so the base scheduling behavior under
  test stays hermetic and hint-only; feedback parity is asserted separately (above).

It also keeps the bootstrap ``--version`` / ``--help`` / no-args exit-code checks, and asserts
``--userguide`` stdout is BYTE-IDENTICAL across builds (both embed the same single-source guide).

Exit status is nonzero on any divergence. The module is kept mypy-strict clean.
"""

from __future__ import annotations

import argparse
import csv
import fcntl
import hashlib
import json
import os
import signal
import random
import re
import shlex
import shutil
import stat
import subprocess
import sys
import tempfile
import time
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path

from cpu_footprint_analysis import (
    FootprintStats,
    analyze as analyze_cpu_footprint,
    check_cgroup_bandwidth,
    check_limits as check_cpu_limits,
    load_events as load_cpu_events,
)
from herdr_agent_differential import compare_herdr_agent
from herdr_differential import compare_herdr_run

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))

#: Passed to every `run` comparison. Cgroup boxing is ON by default in both builds; this flag
#: downgrades to the deterministic, environment-independent UNBOXED scheduling core so the two
#: implementations are compared on identical observable behavior (boxing is proven separately).
ACF = "--allow-cgroup-failure"

#: Passed to every `run` comparison so the default auto-logging profile store (Feature D) does not
#: write CSVs into the harness CWD during the differential; the scheduling behavior under test is
#: independent of profile logging, which is proven by each build's own tests.
NOPROF = "--no-profile"

#: Passed to the existing `run` comparisons so plan-time profile-store FEEDBACK (the learned-estimate
#: reader) does NOT read the repo-local default store and silently refine hints mid-differential —
#: the base scheduling behavior under test must stay hint-only and hermetic. Feedback parity is
#: proven separately by :func:`compare_plan_feedback` against a fixed SYNTHETIC store.
NOFB = "--no-profile-feedback"

#: Feedback identity envs (mirrors safe_ci_dag_runner.estimates): pin the machine id + container
#: class so the feedback reader loads a fixed synthetic ``step_profiles_<mid>_<cc>.csv`` regardless
#: of the host, making the plan/sizing feedback checks deterministic everywhere.
SYNTH_MACHINE = "cross_synth_machine"
SYNTH_CONTAINER = "cross_synth_container"

_COUNTS_RE = re.compile(
    r"(\d+) passed, (\d+) failed, (\d+) aborted, "
    r"(\d+) intentionally skipped, (\d+) dependency-skipped"
)
_SIZING_RE = re.compile(
    r"-> modeled memory ceiling (\d+) active steps "
    r"\(worst-case (\d+) bytes fits budget (\d+) bytes\); "
    r"base active-step ceiling (\d+); final --max-steps (\d+)"
)
_SIZING_REFUSAL_RE = re.compile(
    r"REFUSED — minimum runnable footprint (\d+) bytes cannot fit safely within budget "
    r"(\d+) bytes"
)

CPU_FOOTPRINT_GUEST = os.path.join(REPO_ROOT, "cross", "cpu_footprint_guest.py")


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
    elapsed_s: float = 0.0


@dataclass(frozen=True)
class CpuFootprintFacts:
    completed_steps: int
    workers_per_step: tuple[int, ...]
    max_live_steps: int
    max_live_workers: int


@dataclass
class Report:
    checks: int = 0
    failures: list[str] = field(default_factory=list)
    json_byte_identical: int = 0
    yaml_isomorphic: int = 0
    scalar_parity: int = 0

    def ok(self, _label: str) -> None:
        self.checks += 1

    def bad(self, label: str, detail: str) -> None:
        self.checks += 1
        self.failures.append(f"{label}: {detail}")


# --------------------------------------------------------------------------- command wiring


def py_command() -> list[str]:
    return [sys.executable, "-m", "safe_ci_dag_runner"]


_RUST_SNAPSHOTS: list[tempfile.TemporaryDirectory[str]] = []


def _sha256_file(path: str) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def _read_provenance(path: str) -> tuple[str, str] | None:
    try:
        lines = Path(path).read_text(encoding="utf-8").splitlines()
    except (OSError, UnicodeError):
        return None
    source_values = [line.removeprefix("source_fp=") for line in lines if line.startswith("source_fp=")]
    binary_values = [
        line.removeprefix("binary_sha256=")
        for line in lines
        if line.startswith("binary_sha256=")
    ]
    if len(source_values) != 1 or len(binary_values) != 1:
        return None
    if not re.fullmatch(r"[0-9a-f]{64}", source_values[0]) or not re.fullmatch(
        r"[0-9a-f]{64}", binary_values[0]
    ):
        return None
    return source_values[0], binary_values[0]


def rs_command(tool: str) -> list[str]:
    launcher = os.path.join(REPO_ROOT, "rs", "bin", tool)
    if not os.path.exists(launcher):
        raise FileNotFoundError(f"tracked Rust launcher for {tool!r} is missing: {launcher}")
    target_root = os.path.realpath(os.path.join(REPO_ROOT, "rs", "target"))
    lock_root = os.path.join(REPO_ROOT, "rs", ".agent-utils-locks")
    os.makedirs(lock_root, exist_ok=True)
    fingerprint = os.path.join(REPO_ROOT, "common", "bin", "rs-source-fingerprint")
    last_error = "launcher did not return an artifact"

    for _attempt in range(3):
        env = dict(os.environ)
        env["AGENT_UTILS_RS_ENSURE_ONLY"] = "1"
        ensured = subprocess.run(
            [launcher],
            cwd=REPO_ROOT,
            env=env,
            capture_output=True,
            text=True,
            check=False,
            timeout=300,
        )
        if ensured.returncode != 0:
            raise FileNotFoundError(
                f"Rust launcher for {tool!r} could not verify/build its cache: {ensured.stderr}"
            )
        candidate = ensured.stdout.strip()
        candidate_real = os.path.realpath(candidate)
        try:
            inside_target_root = os.path.commonpath([target_root, candidate_real]) == target_root
        except ValueError:
            inside_target_root = False
        expected_suffix = os.path.join("release", tool)
        if not inside_target_root or not candidate_real.endswith(os.sep + expected_suffix):
            raise FileNotFoundError(
                f"Rust launcher for {tool!r} returned an invalid executable path: {candidate!r}"
            )

        # The launcher releases its lock before returning. Reacquire the same outside-target lock,
        # then revalidate source + provenance around a private copy. A concurrent clean or raw Cargo
        # write either precedes this critical section or produces a hash mismatch and a retry.
        with open(os.path.join(lock_root, "cache.lock"), "a+b") as lock_file:
            fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX)
            provenance = _read_provenance(candidate_real + ".provenance")
            source_before = subprocess.run(
                [fingerprint],
                cwd=REPO_ROOT,
                capture_output=True,
                text=True,
                check=False,
                timeout=30,
            )
            if provenance is None or source_before.returncode != 0:
                last_error = "artifact or provenance disappeared before it could be copied"
                continue
            stamped_source, stamped_binary = provenance
            snapshot = tempfile.TemporaryDirectory(prefix=f"agent-utils-{tool}-")
            snapshot_path = os.path.join(snapshot.name, tool)
            try:
                shutil.copy2(candidate_real, snapshot_path)
                copied_binary = _sha256_file(snapshot_path)
            except OSError as exc:
                snapshot.cleanup()
                last_error = f"artifact copy failed: {exc}"
                continue
            source_after = subprocess.run(
                [fingerprint],
                cwd=REPO_ROOT,
                capture_output=True,
                text=True,
                check=False,
                timeout=30,
            )
            if (
                source_after.returncode == 0
                and source_before.stdout.strip() == stamped_source
                and source_after.stdout.strip() == stamped_source
                and copied_binary == stamped_binary
                and os.access(snapshot_path, os.X_OK)
            ):
                _RUST_SNAPSHOTS.append(snapshot)
                # Hundreds of comparisons now reuse immutable private bytes, not Cargo's mutable
                # top-level target path, without repeatedly invoking the build frontend.
                return [snapshot_path]
            snapshot.cleanup()
            last_error = "source, artifact, or provenance changed while making the private copy"

    raise FileNotFoundError(f"Rust artifact for {tool!r} would not stabilize: {last_error}")


def _env(extra: Mapping[str, str] | None = None) -> dict[str, str]:
    env = dict(os.environ)
    env["PYTHONPATH"] = os.path.join(REPO_ROOT, "py") + os.pathsep + env.get("PYTHONPATH", "")
    # Deterministic, color-free output regardless of the runner's TTY state.
    env["NO_COLOR"] = "1"
    # Rust and Python both support explicit evidence, but an ambient caller setting must not make
    # unrelated differential cases write into one shared directory or gain extra banners.
    env.pop("SAFE_CI_DAG_RUNNER_LOG_DIR", None)
    env.pop("SAFE_CI_DAG_RUNNER_NO_STEP_LOGS", None)
    if extra:
        env.update(extra)
    return env


def run(
    cmd: Sequence[str],
    args: Sequence[str],
    extra_env: Mapping[str, str] | None = None,
    *,
    timeout_s: float = 120.0,
) -> Outcome:
    started = time.monotonic()
    try:
        proc = subprocess.run(
            [*cmd, *args],
            capture_output=True,
            text=True,
            env=_env(extra_env),
            start_new_session=True,
            check=False,
            timeout=timeout_s,
        )
    except subprocess.TimeoutExpired as exc:
        stdout = exc.stdout if isinstance(exc.stdout, str) else ""
        stderr = exc.stderr if isinstance(exc.stderr, str) else ""
        return Outcome(
            124,
            stdout,
            stderr + f"\nTIMEOUT after {timeout_s:g} seconds",
            time.monotonic() - started,
        )
    return Outcome(proc.returncode, proc.stdout, proc.stderr, time.monotonic() - started)


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
                        # cpu_timeout is a per-step enforcement field; carrying it on a fixture step
                        # makes the json/yaml byte-identity round-trip cross-check its serialization
                        # across both engines (environment-independent), complementing the boxed
                        # behavioral smoke tests that only run where cgroups exist.
                        "cpu_timeout": 45,
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

    # Presence-sensitive per-node write domains: the no-protected-write node must retain an explicit
    # empty list, the writer must retain its structural guarantee, and both engines must execute
    # the valid graph without turning the domain into a scheduler mutex.
    fixtures.append(
        Fixture(
            "write_domains",
            {
                "write_domain_policy": {
                    "require_explicit": True,
                    "allowed_domains": ["shared-cargo-target", "isolated-target"],
                },
                "steps": [
                    {
                        "group": "g",
                        "job": "reader",
                        "cmd": "true",
                        "write_domains": [],
                    },
                    {
                        "group": "g",
                        "job": "barrier",
                        "cmd": "true",
                        "write_domains": ["shared-cargo-target"],
                        "write_domain_guarantee": "immutable-artifact-barrier",
                    },
                    {
                        "group": "g",
                        "job": "shielded",
                        "cmd": "true",
                        "deps": ["g.barrier"],
                        "write_domains": ["shared-cargo-target"],
                        "write_domain_guarantee": "artifact-barrier-dependent",
                    },
                    {
                        "group": "g",
                        "job": "isolated",
                        "cmd": "true",
                        "write_domains": ["isolated-target"],
                        "write_domain_guarantee": "explicitly-isolated",
                    },
                ],
            },
        )
    )

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
                "description": 'top-level: "quotes", \\slash\\, unicode é☃, and a\nnewline',
                "steps": [
                    {
                        "group": "g",
                        "job": "quote",
                        "desc": 'has "quotes" and \\backslash\\ and\ttab',
                        # A multi-line description with quotes, backslashes, a tab, a control
                        # char, and unicode — proves the JSON string escaping is byte-identical
                        # across the two builds (FEATURE 1).
                        # Adversarial description covering hostile classes across the full escape
                        # surface: every C0 control (0x00 NUL .. 0x1f), DEL (0x7f), double
                        # backslashes, leading/trailing spaces, and the sneaky Unicode line/
                        # paragraph separators (U+2028/U+2029/U+0085) that some serializers escape
                        # and others pass raw. Both builds must agree byte-for-byte.
                        "description": (
                            "  leading+trailing spaces  \n"
                            'multi-line\nwith "quotes", \\backslash\\, double\\\\backslash, \ttab, '
                            "all-ctrl "
                            + "".join(chr(c) for c in range(0x00, 0x20))
                            + " del\x7f, unicode é☃\U0001F600, "
                            "line-seps \u2028\u2029\u0085 end"
                        ),
                        "cmd": "true",
                        "env": {"K2": "v2", "K1": "v1"},
                    }
                ],
            },
        )
    )

    # A memory-modeled DAG sized against a tight budget: the chosen --max-steps and footprint are
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

    # jobs_flag: the inner-parallelism flag appended to a step's command when it declares
    # preferred_inner_jobs. Each fixture's command PASSES iff exactly the expected appended token(s)
    # arrive as "$*", so a serial run (-s1 with an ample -j budget) yields 1-passed in BOTH builds
    # only when Python and
    # Rust render + append the flag identically. The `json` check additionally pins schema parity
    # for the jobs_flag / default_jobs_flag fields.
    fixtures.append(
        Fixture(
            "jobs_flag_percent",  # "-j%d" -> "-j4" (no space)
            {
                "steps": [
                    {
                        "group": "g",
                        "job": "j",
                        "cmd": 'c() { [ "$*" = "-j4" ]; }; c',
                        "jobs_flag": "-j%d",
                        "hint": {"preferred_inner_jobs": 4},
                    }
                ]
            },
        )
    )
    fixtures.append(
        Fixture(
            "jobs_flag_equals",  # "--jobs=" -> "--jobs=8" (concatenated)
            {
                "steps": [
                    {
                        "group": "g",
                        "job": "j",
                        "cmd": 'c() { [ "$*" = "--jobs=8" ]; }; c',
                        "jobs_flag": "--jobs=",
                        "hint": {"preferred_inner_jobs": 8},
                    }
                ]
            },
        )
    )
    fixtures.append(
        Fixture(
            "jobs_flag_default_inherit",  # step inherits DAG default_jobs_flag "--num-threads" -> "--num-threads 3"
            {
                "default_jobs_flag": "--num-threads",
                "steps": [
                    {
                        "group": "g",
                        "job": "j",
                        "cmd": 'c() { [ "$*" = "--num-threads 3" ]; }; c',
                        "hint": {"preferred_inner_jobs": 3},
                    }
                ],
            },
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
            # Only demand resources that exist in caps. An undeclared demand is refused before
            # any node starts (both engines), which is a correct outcome but not the one this
            # generator is here to compare.
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


def example_fixtures() -> list[Fixture]:
    """Every DAG shipped under ``examples/`` becomes a fixture, so the runnable newcomer examples
    are proven to render identically under both builds.

    These are checked STATIC-ONLY (``list``/``ascii``/``dot``/``json`` parity via
    :func:`compare_example_static`): the examples use real multi-second ``sleep`` commands to make
    parallelism visible, so folding them into the full run/timing battery would slow the harness for
    no added coverage — run/exit/counts/sizing parity is already exercised by the fast representative
    and randomized fixtures.
    """
    examples_dir = os.path.join(REPO_ROOT, "examples")
    if not os.path.isdir(examples_dir):
        return []
    fixtures: list[Fixture] = []
    for name in sorted(os.listdir(examples_dir)):
        if not name.endswith(".json"):
            continue
        with open(os.path.join(examples_dir, name), encoding="utf-8") as fh:
            loaded: object = json.load(fh)
        if not isinstance(loaded, dict):
            raise TypeError(f"examples/{name}: expected a JSON object at the top level")
        dag: dict[str, object] = {str(key): val for key, val in loaded.items()}
        fixtures.append(Fixture(f"example:{name[:-len('.json')]}", dag))
    return fixtures


def compare_example_static(py: list[str], rs: list[str], fx: Fixture, rep: Report) -> None:
    """Static-command parity for a shipped example DAG (see :func:`example_fixtures`)."""
    with tempfile.TemporaryDirectory() as tmp:
        dag_path = os.path.join(tmp, "dag.json")
        with open(dag_path, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(fx.dag))
        _static_parity(py, rs, dag_path, fx.name, rep)


# --------------------------------------------------------------------------- YAML isomorphism


def yaml_fixture_paths() -> list[str]:
    """Every ``.yaml``/``.yml`` fixture: the shipped ``examples/`` plus the dedicated adversarial
    torture set under ``cross/yaml_fixtures/`` (block scalars, quoted Norway tokens, unicode)."""
    paths: list[str] = []
    for directory in (
        os.path.join(REPO_ROOT, "examples"),
        os.path.join(REPO_ROOT, "cross", "yaml_fixtures"),
    ):
        if not os.path.isdir(directory):
            continue
        for name in sorted(os.listdir(directory)):
            if name.endswith((".yaml", ".yml")):
                paths.append(os.path.join(directory, name))
    return paths


def compare_yaml_isomorphism(py: list[str], rs: list[str], rep: Report) -> None:
    """Assert YAML is ISOMORPHIC to JSON.

    For each YAML fixture: load it in BOTH builds, emit CANONICAL JSON, and assert the two are
    BYTE-IDENTICAL (so the two parsers agree — including on block scalars, the quoted Norway-problem
    tokens, and unicode). Additionally, when an ``examples/NAME.json`` pair exists, assert the
    ``.json`` and ``.yaml`` load to the SAME DAG in each language.
    """
    for path in yaml_fixture_paths():
        rel = os.path.relpath(path, REPO_ROOT)
        po = run(py, ("json", "--dag", path))
        ro = run(rs, ("json", "--dag", path))
        label = f"yaml-iso:{rel}"
        if po.returncode != 0 or ro.returncode != 0:
            rep.bad(
                label,
                f"YAML load failed: py rc={po.returncode} ({po.stderr!r}); "
                f"rs rc={ro.returncode} ({ro.stderr!r})",
            )
            continue
        if po.stdout != ro.stdout:
            rep.bad(
                label,
                f"YAML not isomorphic (py vs rs canonical JSON differ)\n"
                f"--- py ---\n{po.stdout}\n--- rs ---\n{ro.stdout}",
            )
            continue
        rep.ok(label)
        rep.yaml_isomorphic += 1

        json_pair = os.path.splitext(path)[0] + ".json"
        if not os.path.exists(json_pair):
            continue
        pj = run(py, ("json", "--dag", json_pair))
        rj = run(rs, ("json", "--dag", json_pair))
        plabel = f"yaml-pair:{rel}"
        if pj.returncode != 0 or rj.returncode != 0:
            rep.bad(plabel, f"JSON pair load failed: py rc={pj.returncode}; rs rc={rj.returncode}")
        elif pj.stdout != po.stdout:
            rep.bad(
                plabel,
                f"py: json/yaml pair loads differ\n--- json ---\n{pj.stdout}\n--- yaml ---\n{po.stdout}",
            )
        elif rj.stdout != ro.stdout:
            rep.bad(
                plabel,
                f"rs: json/yaml pair loads differ\n--- json ---\n{rj.stdout}\n--- yaml ---\n{ro.stdout}",
            )
        else:
            rep.ok(plabel)


# --------------------------------------------------------------------------- scalar parity


def scalar_parity_cases() -> list[tuple[str, str, str]]:
    """``(name, extension, document)`` triples pinning cross-language AGREEMENT on adversarial
    scalar resolution, non-finite / overflow floats, and out-of-range integers.

    Unlike :func:`compare_yaml_isomorphism` (which only covers documents BOTH builds accept), each
    case here asserts the two builds return the SAME exit code — so "both correctly REJECT" is a
    pass — and, when both accept, byte-identical canonical JSON. This is the regression net for the
    PyYAML-1.1-vs-serde_norway-1.2 divergences (the "Norway problem", octal/underscore/sexagesimal/
    timestamp resolution), Python-vs-serde_json non-finite handling, and i64-range integer bounds.
    """
    # A single valid step, appended after whichever adversarial top-level/step field is under test.
    step = "steps:\n  - {group: g, job: j, cmd: make}\n"
    cases: list[tuple[str, str, str]] = []

    # Norway problem in a STRING field (top-level `description`): plain yes/no/on/off/y/n must stay
    # strings (accept); true/false resolve to bools (reject as non-string) — the SAME on both sides.
    for tok in ("no", "yes", "on", "off", "y", "n", "Yes", "No"):
        cases.append((f"norway_str_{tok}", "yaml", f"description: {tok}\n{step}"))
    for tok in ("true", "false", "True", "FALSE"):
        cases.append((f"bool_in_str_{tok}", "yaml", f"description: {tok}\n{step}"))

    # Norway problem in a BOOL field (`networkonly`): only true/false are bools; yes/no/on/off are
    # strings and must be rejected by the boolean field — identically in both builds.
    for tok in ("true", "false"):
        cases.append((f"bool_field_ok_{tok}", "yaml", f"steps:\n  - {{group: g, job: j, cmd: make, networkonly: {tok}}}\n"))
    for tok in ("no", "yes", "on", "off"):
        cases.append((f"bool_field_reject_{tok}", "yaml", f"steps:\n  - {{group: g, job: j, cmd: make, networkonly: {tok}}}\n"))

    # Integer resolution in an INT field (`default_step_timeout`).
    for name, tok in (
        ("dec", "42"), ("plus", "+5"), ("minus", "-5"), ("zero", "0"),
        ("octal_prefixed", "0o17"), ("hex", "0x10"), ("binary", "0b101"),
        ("leading_zero", "0755"), ("leading_zero2", "010"),
        ("underscore", "1_000"), ("sexagesimal", "1:20"), ("floatish", "1e3"),
        ("bignum", "99999999999999999999999999"),
        ("just_over_i64", "9223372036854775808"),
    ):
        cases.append((f"int_{name}", "yaml", f"default_step_timeout: {tok}\n{step}"))

    # Float resolution in a FLOAT field (`mem_cap_factor`).
    for name, tok in (
        ("sci", "1e3"), ("dot_frac", ".5"), ("trailing_dot", "1."),
        ("overflow", "1e400"), ("neg_overflow", "-1e400"),
        ("inf", ".inf"), ("nan", ".nan"), ("leading_zero", "0755"),
    ):
        cases.append((f"float_{name}", "yaml", f"mem_cap_factor: {tok}\n{step}"))

    # Non-finite in a NULLABLE float field: serde maps `.inf` to null, so both read null (accept).
    cases.append(("nullable_inf", "yaml", "steps:\n  - {group: g, job: j, cmd: make, hint: {measured_effective_cores: .inf}}\n"))

    # String field fed YAML null tokens: `description` has default "" but an explicit null value is
    # not a string and must be rejected by BOTH builds.
    for name, tok in (("null_word", "null"), ("tilde", "~")):
        cases.append((f"str_{name}", "yaml", f"description: {tok}\n{step}"))

    # Timestamp / time-like tokens must stay plain strings in a string field (accept on both).
    for name, tok in (("date", "2024-01-01"), ("time", "12:34:56")):
        cases.append((f"str_{name}", "yaml", f"description: {tok}\n{step}"))

    # Malformed YAML must be a load error (exit 2), not an uncaught exception, on both builds.
    cases.append(("malformed_yaml", "yaml", "steps: [ this is : broken : yaml"))

    # JSON non-finite literals: Python's json accepts them by default, serde_json rejects — both
    # must reject here.
    for name, tok in (("infinity", "Infinity"), ("neg_infinity", "-Infinity"), ("nan", "NaN")):
        cases.append((f"json_{name}", "json", f'{{"mem_cap_factor": {tok}, "steps": []}}'))
    # JSON overflow-to-infinity and out-of-i64-range integer: both reject.
    cases.append(("json_overflow_float", "json", '{"mem_cap_factor": 1e400, "steps": []}'))
    cases.append(("json_bignum", "json", '{"mem_cap_floor_bytes": 99999999999999999999999999, "steps": []}'))

    # Write-domain policy must refuse identically before execution: omitted is not explicit
    # no-protected-write declaration, and neither unknown nor duplicate domains can silently enter
    # the vocabulary.
    cases.append((
        "write_domain_missing",
        "json",
        '{"steps":[{"group":"g","job":"j","cmd":"true"}],'
        '"write_domain_policy":{"require_explicit":true,"allowed_domains":[]}}',
    ))
    cases.append((
        "write_domain_unknown",
        "json",
        '{"steps":[{"group":"g","job":"j","cmd":"true",'
        '"write_domains":["typo"],"write_domain_guarantee":"artifact-producer"}],'
        '"write_domain_policy":{"require_explicit":true,'
        '"allowed_domains":["shared-cargo-target"]}}',
    ))
    cases.append((
        "write_domain_duplicate",
        "json",
        '{"steps":[{"group":"g","job":"j","cmd":"true",'
        '"write_domains":["shared-cargo-target","shared-cargo-target"]}],'
        '"write_domain_policy":{"require_explicit":true,'
        '"allowed_domains":["shared-cargo-target"]}}',
    ))

    # Scientific-notation float VALUES must serialize byte-identically (the json_float parity fix).
    for name, tok in (
        ("e20", "1e20"), ("e_minus7", "1e-7"), ("e16", "1e16"), ("e100", "1e100"),
        ("big", "1.2345678901234568e17"), ("e_minus5", "1e-5"), ("fixed_e15", "1e15"),
    ):
        cases.append((f"sci_{name}", "json", f'{{"mem_cap_factor": {tok}, "steps": []}}'))

    # Duplicate mapping keys: both builds accept last-wins, on both YAML and JSON, byte-identically.
    cases.append(("dup_key_yaml", "yaml", "steps:\n  - {group: g, job: j, cmd: first, cmd: second}\n"))
    cases.append(("dup_key_json", "json", '{"steps": [{"group": "g", "job": "j", "cmd": "first", "cmd": "second"}]}'))

    return cases


def compare_scalar_parity(py: list[str], rs: list[str], rep: Report) -> None:
    """Run every :func:`scalar_parity_cases` document through both builds and assert exit-code parity
    (so "both reject" passes) plus byte-identical canonical JSON whenever both accept."""
    with tempfile.TemporaryDirectory() as tmp:
        for i, (name, ext, doc) in enumerate(scalar_parity_cases()):
            path = os.path.join(tmp, f"case{i}.{ext}")
            with open(path, "w", encoding="utf-8") as fh:
                fh.write(doc)
            po = run(py, ("json", "--dag", path))
            ro = run(rs, ("json", "--dag", path))
            label = f"scalar-parity:{name}"
            if po.returncode != ro.returncode:
                rep.bad(
                    label,
                    f"exit py={po.returncode} rs={ro.returncode} "
                    f"(py stderr={po.stderr!r}; rs stderr={ro.stderr!r})",
                )
            elif po.returncode == 0 and po.stdout != ro.stdout:
                rep.bad(
                    label,
                    f"both accepted but json differs\n--- py ---\n{po.stdout}\n--- rs ---\n{ro.stdout}",
                )
            else:
                rep.ok(label)
                rep.scalar_parity += 1


# --------------------------------------------------------------------------- comparisons


def _counts(stderr: str) -> tuple[int, int, int, int, int] | None:
    m = _COUNTS_RE.search(stderr)
    if not m:
        return None
    return (
        int(m.group(1)),
        int(m.group(2)),
        int(m.group(3)),
        int(m.group(4)),
        int(m.group(5)),
    )


def _sizing_details(stderr: str) -> tuple[int, int, int, int, int] | None:
    """Return ``(memory_ceiling, footprint, budget, base_ceiling, final_ceiling)``."""
    m = _SIZING_RE.search(stderr)
    if not m:
        return None
    memory_steps = int(m.group(1))
    footprint = int(m.group(2))
    budget = int(m.group(3))
    base = int(m.group(4))
    selected = int(m.group(5))
    if selected != min(base, memory_steps):
        return None
    return memory_steps, footprint, budget, base, selected


def _sizing(stderr: str) -> tuple[int, int, int] | None:
    details = _sizing_details(stderr)
    if details is None:
        return None
    _memory_steps, footprint, budget, _base, selected = details
    return selected, footprint, budget


def _sizing_refusal(stderr: str) -> tuple[int, int] | None:
    """Return ``(minimum_footprint, budget)`` from a fail-closed max-memory refusal."""
    match = _SIZING_REFUSAL_RE.search(stderr)
    if match is None:
        return None
    return int(match.group(1)), int(match.group(2))


def _static_parity(py: list[str], rs: list[str], dag_path: str, name: str, rep: Report) -> None:
    """Assert the static (non-running) commands agree for one DAG file: ``list``/``ascii``/``dot``
    AND ``json`` stdout are all BYTE-IDENTICAL.

    Both builds now emit ``ensure_ascii=False`` canonical JSON, so the ``json`` output must match
    byte-for-byte for every input, including hairy ``description`` fields (multi-line, quotes,
    backslashes, control chars, unicode). A parse-only difference is still a failure — the note in
    the failure message localizes whether it is a formatting-only or a semantic divergence.

    Shared by the full-battery fixtures (:func:`compare_fixture`) and the shipped-example fixtures
    (:func:`compare_example_static`), so the two paths cannot drift apart.
    """
    # list / ascii / dot: byte-identical stdout.
    for mode in ("list", "ascii", "dot"):
        po = run(py, (mode, "--dag", dag_path))
        ro = run(rs, (mode, "--dag", dag_path))
        label = f"{name}/{mode}"
        if po.returncode != ro.returncode:
            rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}")
        elif po.stdout != ro.stdout:
            rep.bad(label, f"stdout differs\n--- py ---\n{po.stdout}\n--- rs ---\n{ro.stdout}")
        else:
            rep.ok(label)

    # json: BYTE-identical stdout.
    po = run(py, ("json", "--dag", dag_path))
    ro = run(rs, ("json", "--dag", dag_path))
    label = f"{name}/json"
    if po.returncode != ro.returncode:
        rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}")
        return
    if po.stdout != ro.stdout:
        try:
            note = "parse-equal" if json.loads(po.stdout) == json.loads(ro.stdout) else "parse-DIFFER"
        except json.JSONDecodeError as exc:
            note = f"unparseable: {exc}"
        rep.bad(
            label,
            f"json stdout not byte-identical ({note})\n--- py ---\n{po.stdout}\n--- rs ---\n{ro.stdout}",
        )
        return
    rep.ok(label)
    rep.json_byte_identical += 1


def _first_tag(fx: Fixture) -> str | None:
    """The ``group.job`` tag of the fixture's first step, or ``None`` for an empty/odd DAG.

    Used to exercise ``--only`` selection parity (Feature A) on a concrete, present tag."""
    steps = fx.dag.get("steps")
    if not isinstance(steps, list) or not steps:
        return None
    first = steps[0]
    if not isinstance(first, dict):
        return None
    group, job = first.get("group"), first.get("job")
    if isinstance(group, str) and isinstance(job, str):
        return f"{group}.{job}"
    return None


def compare_only_errors(py: list[str], rs: list[str], rep: Report) -> None:
    """``--only`` with an unknown tag must exit 2 on BOTH builds (Feature A error parity)."""
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "dag.json")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write('{"steps": [{"group": "g", "job": "j", "cmd": "true"}]}')
        po = run(py, ("run", "--dag", path, "-q", "--only", "no.pe", NOPROF, NOFB, ACF))
        ro = run(rs, ("run", "--dag", path, "-q", "--only", "no.pe", NOPROF, NOFB, ACF))
        label = "only-unknown-tag"
        if po.returncode != ro.returncode:
            rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}")
        elif po.returncode != 2:
            rep.bad(label, f"expected exit 2 for an unknown --only tag; got {po.returncode}")
        else:
            rep.ok(label)


#: How long the planted escapee lives. It MUST exceed `run`'s own expiry, or the case cannot
#: distinguish a build that finishes from one that blocks: a self-limiting escapee releases the
#: pipes on its own, the blocked build returns just before the harness gives up, and the two builds
#: agree on an exit code for the wrong reason. Measured against a build with the defect still in
#: place: at a 90s lifetime it returned at 90s and the check PASSED; the case only fails when the
#: survivor outlives the harness. The fixture kills the pid it recorded once the run is over, so a
#: long lifetime does not mean a long-lived leak.
_ESCAPEE_LIFETIME_S = 300


def compare_run_timeout(py: list[str], rs: list[str], rep: Report) -> None:
    """Both builds must bound the WHOLE run identically, and refuse a mis-ordered budget alike.

    THE BOUND THAT MATTERS IS THE ONE THAT STILL REPORTS. Per-step budgets cannot bound a run:
    any number of individually-legal steps can sum past any ceiling, and the only thing that
    stopped such a run was an external job kill, which discards the evidence it was supposed to
    explain. So the outer budget is checked in the scheduler's own loop, and this asserts both
    engines do it: the run must finish EARLY (well before the DAG's natural length), must not
    report success, and must not hit the harness timeout.

    The second half is the ordering contract made executable. A step allowed to run as long as
    the whole run can only ever be killed by the outer bound, which attributes the overrun to the
    run instead of to the node — so both builds must REFUSE to start rather than run with a
    budget whose breach cannot be attributed.
    """
    with tempfile.TemporaryDirectory() as tmp:
        # Natural length ~12s (three 4s steps, -s1); budget 6s; every step's own wall budget 5s,
        # strictly under the run budget, so the ordering is legal.
        bounded = os.path.join(tmp, "bounded.json")
        with open(bounded, "w", encoding="utf-8") as fh:
            fh.write(
                json.dumps(
                    {
                        "steps": [
                            {
                                "group": "a",
                                "job": name,
                                "cmd": "sleep 4",
                                "timeout": 5,
                                "cpu_timeout": 600,
                            }
                            for name in ("one", "two", "three")
                        ]
                    }
                )
            )
        # Same DAG with a step allowed to outlive the run: a mis-ordered budget.
        misordered = os.path.join(tmp, "misordered.json")
        with open(misordered, "w", encoding="utf-8") as fh:
            fh.write(
                json.dumps(
                    {
                        "steps": [
                            {
                                "group": "a",
                                "job": "wide",
                                "cmd": "sleep 1",
                                "timeout": 600,
                                "cpu_timeout": 600,
                            }
                        ]
                    }
                )
            )

        def _run(cmd: list[str], dag: str) -> Outcome:
            return run(
                cmd,
                (
                    "run", "--dag", dag, "-q", "-s", "1", "-j", "1", NOPROF, NOFB,
                    ACF, "--run-timeout", "6",
                ),
            )

        po, ro = _run(py, bounded), _run(rs, bounded)
        label = "run-timeout"
        blocked = [n for n, o in (("py", po), ("rs", ro)) if o.returncode == 124]
        if blocked:
            rep.bad(
                label,
                f"{'/'.join(blocked)} never returned under an outer run budget "
                f"(exit py={po.returncode} rs={ro.returncode}); a bound that cannot report is "
                "the external job kill it exists to replace",
            )
        elif po.returncode != ro.returncode:
            rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}")
        elif po.returncode == 0:
            rep.bad(label, "a run cut short by its outer budget must not report success")
        else:
            rep.ok(label)

        pm, rm = _run(py, misordered), _run(rs, misordered)
        label = "run-timeout-misordered-refusal"
        if pm.returncode != rm.returncode:
            rep.bad(label, f"exit py={pm.returncode} rs={rm.returncode}")
        elif pm.returncode == 0:
            rep.bad(
                label,
                "a step allowed to run as long as the whole run must be REFUSED, not accepted: "
                "the outer bound would fire first and the overrun could not be attributed",
            )
        elif not all("REFUSING" in o.stdout + o.stderr for o in (pm, rm)):
            rep.bad(
                label,
                "both builds must SAY they refused and name the offending steps; "
                f"py={pm.stderr[-200:]!r} rs={rm.stderr[-200:]!r}",
            )
        else:
            rep.ok(label)


def compare_escapee_teardown(py: list[str], rs: list[str], rep: Report) -> None:
    """Both builds must FINISH and REAP a step whose child escapes its process group.

    THE CASE THIS COVERS, AND WHY ITS ABSENCE MATTERED. Every other `run` comparison here uses a
    step that dies when its process group is signalled, so all of them exercise the easy teardown
    path. A step can instead leave a `setsid` child behind: the child changes session and process
    group, the intermediate parent exits, and the survivor keeps the step's stdout/stderr write ends
    open. A build that waits unconditionally for those pipes to reach EOF never returns -- it does
    not fail, it hangs, and a run that hangs writes no results at all. Two builds can therefore
    disagree completely on this path while every other check agrees, which is exactly what happened:
    the suite reported full agreement across every fixture while the two runners behaved differently
    on the one behaviour that decides whether the system can report on its own failures.

    WHAT IS ASSERTED. Both builds must return the SAME exit status, neither may hit the harness
    timeout, AND the exact recorded escapee pid must be gone before fixture cleanup runs. The last
    assertion is load-bearing: merely closing the runner's pipe and returning would still pass an
    exit-code comparison while leaking work onto the host.

    The escapee bounds itself with a short sleep and is also killed by recorded pid afterwards, so
    the fixture cannot leak a process onto a shared machine even if a build leaves it running.
    """
    with tempfile.TemporaryDirectory() as tmp:
        # ONE PID FILE PER BUILD. Both runs plant an escapee, so a single shared file records only
        # the second one and the first is leaked onto the machine on every invocation. Found by
        # counting survivors after a full suite run, not by reading the code.
        def _case(tag: str, cmd: list[str]) -> tuple[str, Outcome]:
            pid_file = os.path.join(tmp, f"escapee-{tag}.pid")
            script = os.path.join(tmp, f"escapee-{tag}.sh")
            with open(script, "w", encoding="utf-8") as fh:
                fh.write(
                    "#!/usr/bin/env bash\n"
                    # setsid leaves the step's session AND process group, so a process-group kill
                    # cannot reach it; it inherits the pipes, so the readers never see EOF.
                    f"setsid bash -c 'echo $$ > {pid_file}; exec sleep {_ESCAPEE_LIFETIME_S}' &\n"
                    f"sleep {_ESCAPEE_LIFETIME_S}\n"
                )
            os.chmod(script, 0o755)
            dag_path = os.path.join(tmp, f"dag-{tag}.json")
            with open(dag_path, "w", encoding="utf-8") as fh:
                fh.write(
                    json.dumps(
                        {
                            "steps": [
                                {
                                    "group": "escapee",
                                    "job": "setsid_child",
                                    "cmd": f"bash {script}",
                                    "timeout": 5,
                                    "cpu_timeout": 600,
                                }
                            ]
                        }
                    )
                )
            return pid_file, run(
                cmd,
                ("run", "--dag", dag_path, "-q", "-s", "1", "-j", "1", NOPROF, NOFB, ACF),
            )

        pid_files: list[str] = []
        survivors: dict[str, bool | None] = {}
        try:
            pf, po = _case("py", py)
            pid_files.append(pf)
            survivors["py"] = _recorded_pid_alive(pf)
            _kill_recorded_pid(pf)
            pf, ro = _case("rs", rs)
            pid_files.append(pf)
            survivors["rs"] = _recorded_pid_alive(pf)
        finally:
            for pf in pid_files:
                _kill_recorded_pid(pf)

        label = "escapee-teardown"
        blocked = [n for n, o in (("py", po), ("rs", ro)) if o.returncode == 124]
        if blocked:
            rep.bad(
                label,
                f"{'/'.join(blocked)} never returned after a step left a setsid child behind "
                f"(exit py={po.returncode} rs={ro.returncode}); a run that cannot finish cannot "
                "report what went wrong",
            )
        elif po.returncode != ro.returncode:
            rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}")
        elif po.returncode == 0:
            rep.bad(label, "a step killed at its wall budget must not report success")
        elif any(state is None for state in survivors.values()):
            rep.bad(label, f"fixture did not record both escapee pids: {survivors}")
        elif any(state for state in survivors.values()):
            leaked = "/".join(name for name, alive in survivors.items() if alive)
            rep.bad(label, f"{leaked} returned while its recorded setsid escapee was still alive")
        else:
            rep.ok(label)


def compare_term_attribution(py: list[str], rs: list[str], rep: Report) -> None:
    """A timed-out step gets a bounded SIGTERM opportunity to identify its in-flight work.

    This cross-check intentionally asserts the observable marker and bounded command completion,
    not an absolute sub-four-second CLI duration. Overall latency also includes repeated ``/proc``
    ownership sweeps and host scheduling, so that wall-clock heuristic falsely accused correct
    teardown on loaded builders. Paired unit tests pin the underlying early-exit rule directly:
    an unreaped zombie is not a live process-group member and therefore cannot consume the grace.
    """
    with tempfile.TemporaryDirectory() as tmp:
        dag_path = os.path.join(tmp, "dag.json")
        marker = "TERM_ATTRIBUTION_MARKER"
        with open(dag_path, "w", encoding="utf-8") as fh:
            fh.write(
                json.dumps(
                    {
                        "steps": [
                            {
                                "group": "timeout",
                                "job": "reports_term",
                                "cmd": (
                                    f"trap 'printf {marker}\\n >&2; exit 0' TERM; "
                                    "while :; do sleep 1; done"
                                ),
                                "timeout": 2,
                                "cpu_timeout": 600,
                            }
                        ]
                    }
                )
            )
        args = (
            "run",
            "--dag",
            dag_path,
            "-q",
            "-s",
            "1",
            "-j",
            "1",
            NOPROF,
            NOFB,
            "--unsafe-no-cgroups",
        )
        po, ro = run(py, args), run(rs, args)
        combined = {"py": po.stdout + po.stderr, "rs": ro.stdout + ro.stderr}
        label = "term-attribution"
        blocked = [name for name, out in (("py", po), ("rs", ro)) if out.returncode == 124]
        missing = [name for name, output in combined.items() if marker not in output]
        if blocked:
            rep.bad(label, f"{'/'.join(blocked)} did not return after its wall timeout")
        elif po.returncode != ro.returncode:
            rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}")
        elif po.returncode == 0:
            rep.bad(label, "a step that exceeded its wall budget reported success")
        elif missing:
            rep.bad(label, f"{'/'.join(missing)} suppressed the timed-out step's SIGTERM marker")
        else:
            rep.ok(label)


def compare_test_attribution_evidence(py: list[str], rs: list[str], rep: Report) -> None:
    """Explicit evidence and test-level culprit attribution are paired public behavior."""
    with tempfile.TemporaryDirectory() as tmp:
        dag_path = os.path.join(tmp, "attribution.json")
        Path(dag_path).write_text(
            json.dumps(
                {
                    "steps": [
                        {
                            "group": "tests",
                            "job": "suite",
                            "cmd": (
                                "printf '##TEST-START suite::alpha\\n'; "
                                "printf '##TEST-END suite::alpha ok\\n'; "
                                "printf '##TEST-START suite::gamma_the_hang\\n'; "
                                "trap 'exit 0' TERM; while :; do sleep 1; done"
                            ),
                            "timeout": 2,
                            "cpu_timeout": 600,
                        }
                    ]
                }
            ),
            encoding="utf-8",
        )
        dirs = {name: os.path.join(tmp, name) for name in ("py", "rs")}
        args = (
            "run",
            "--dag",
            dag_path,
            "-q",
            "-s",
            "1",
            "-j",
            "1",
            NOPROF,
            NOFB,
            "--unsafe-no-cgroups",
        )
        outcomes = {
            "py": run(py, args, {"SAFE_CI_DAG_RUNNER_LOG_DIR": dirs["py"]}),
            "rs": run(rs, args, {"SAFE_CI_DAG_RUNNER_LOG_DIR": dirs["rs"]}),
        }
        phrase = "culprit test suite::gamma_the_hang"
        if any(out.returncode == 0 or phrase not in out.stdout + out.stderr for out in outcomes.values()):
            rep.bad("attribution:culprit", f"py={outcomes['py']}\nrs={outcomes['rs']}")
        else:
            rep.ok("attribution:culprit")

        def normalized(directory: str) -> list[tuple[str, str, str, str, str, str]]:
            rows = [
                json.loads(line)
                for line in Path(directory, "journal.jsonl").read_text(encoding="utf-8").splitlines()
            ]
            return [
                (
                    str(row.get("event", "")),
                    str(row.get("step", "")),
                    str(row.get("test", "")),
                    str(row.get("verdict", "")),
                    str(row.get("tests_started", "")),
                    str(row.get("tests_completed", "")),
                )
                for row in rows
                if row.get("event") in {"test_start", "test_end", "step_end"}
            ]

        try:
            py_rows, rs_rows = normalized(dirs["py"]), normalized(dirs["rs"])
            logs = {
                name: Path(directory, "tests.suite.log").read_text(encoding="utf-8")
                for name, directory in dirs.items()
            }
            private = all(
                stat.S_IMODE(Path(directory, filename).stat().st_mode) == 0o600
                for directory in dirs.values()
                for filename in ("journal.jsonl", "tests.suite.log")
            )
        except (OSError, ValueError, json.JSONDecodeError) as exc:
            rep.bad("attribution:evidence", str(exc))
        else:
            if py_rows != rs_rows:
                rep.bad("attribution:evidence", f"journal py={py_rows!r} rs={rs_rows!r}")
            elif not private or any("suite::gamma_the_hang" not in log for log in logs.values()):
                rep.bad("attribution:evidence", f"private={private} logs={logs!r}")
            else:
                rep.ok("attribution:evidence")


def compare_batch_teardown_grace(py: list[str], rs: list[str], rep: Report) -> None:
    """Eager cancellation grants one diagnostic window, not one window per sibling.

    Use an outer run timeout as a behavioral tripwire instead of treating shared-host wall time as
    a performance SLO. Eight serial five-second TERM graces cannot finish before that tripwire,
    while one shared grace has ample room even under scheduler and `/proc`-scan contention.
    """
    with tempfile.TemporaryDirectory() as tmp:
        dag_path = os.path.join(tmp, "batch-grace.json")
        steps: list[dict[str, object]] = [
            {
                "group": "batch",
                "job": "fails",
                "cmd": "sleep 0.5; false",
                "timeout": 20,
                "cpu_timeout": 600,
            }
        ]
        steps.extend(
            {
                "group": "batch",
                "job": f"resists_{index}",
                "cmd": "trap '' TERM; sleep 30",
                "timeout": 20,
                "cpu_timeout": 600,
            }
            for index in range(8)
        )
        Path(dag_path).write_text(json.dumps({"steps": steps}), encoding="utf-8")
        args = (
            "run",
            "--dag",
            dag_path,
            "-q",
            "-s",
            "9",
            "-j",
            "9",
            "--run-timeout",
            "25",
            NOPROF,
            NOFB,
            "--unsafe-no-cgroups",
        )
        po, ro = run(py, args), run(rs, args)
        evidence = (po.stdout + po.stderr, ro.stdout + ro.stderr)
        if (
            po.returncode == ro.returncode != 0
            and all("[scheduler] RUN TIMEOUT" not in text for text in evidence)
            and max(po.elapsed_s, ro.elapsed_s) < 32.0
        ):
            rep.ok("teardown:shared-batch-grace")
        else:
            rep.bad(
                "teardown:shared-batch-grace",
                f"exit py={po.returncode} rs={ro.returncode}; "
                f"elapsed py={po.elapsed_s:.3f}s rs={ro.elapsed_s:.3f}s",
            )


def _recorded_pid_alive(pid_file: str) -> bool | None:
    """Whether the exact fixture pid still exists as a non-zombie; None means no pid was recorded."""
    try:
        with open(pid_file, encoding="utf-8") as fh:
            pid = int(fh.read().strip())
    except (OSError, ValueError):
        return None
    try:
        stat = Path(f"/proc/{pid}/stat").read_text(encoding="utf-8")
    except OSError:
        return False
    close = stat.rfind(")")
    return close >= 0 and stat[close + 2 :].split()[0] != "Z"


def _kill_recorded_pid(pid_file: str) -> None:
    """SIGKILL the pid this fixture recorded, if it is still alive.

    Kills one EXPLICIT pid that the fixture itself spawned -- never a name or command-line pattern,
    which on a shared machine would reach other tenants' work.
    """
    try:
        with open(pid_file, encoding="utf-8") as fh:
            pid = int(fh.read().strip())
    except (OSError, ValueError):
        return  # never started, or already cleaned up
    try:
        os.kill(pid, signal.SIGKILL)
    except (ProcessLookupError, PermissionError, OSError):
        pass  # already gone, which is the expected outcome once a build reaps correctly


def compare_fixture(py: list[str], rs: list[str], fx: Fixture, rep: Report) -> None:
    with tempfile.TemporaryDirectory() as tmp:
        dag_path = os.path.join(tmp, "dag.json")
        with open(dag_path, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(fx.dag))

        # 1-2) static commands: list/ascii/dot byte-identical, json parsed-equal.
        _static_parity(py, rs, dag_path, fx.name, rep)

        # 3) run: default concurrent exit-code parity. The default eager-exit path is
        # deterministic in its EXIT CODE (1 iff any reachable step fails) but NOT in the
        # passed/aborted split, which depends on which in-flight step is cancelled first, so
        # only the exit code is compared here.
        #
        # All `run` comparisons pass --allow-cgroup-failure: cgroup-v2 boxing is ON by default in
        # BOTH builds, but boxing is environment-dependent and cannot be asserted byte-identically
        # here (see cross/README.md). The flag makes both builds run the SAME observable UNBOXED
        # scheduling core deterministically, regardless of whether the host can box. Boxing itself
        # is proven by each build's own tests (Python pytest + the Rust boxing smoke test).
        concurrent_args = (
            "run", "--dag", dag_path, "-q", "-s", "4", "-j", "64", NOPROF, NOFB, ACF,
        )
        po = run(py, concurrent_args)
        ro = run(rs, concurrent_args)
        label = f"{fx.name}/run(default-exit)"
        if po.returncode != ro.returncode:
            rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}")
        else:
            rep.ok(label)

        # 4) run serial (-s1): deterministic counts + exit code. Keep an ample -j64 aggregate CPU
        # budget so serial scheduling does not rewrite an authored inner width or its jobs_flag.
        # With one step at a time the
        # ready-set loop dispatches in a single deterministic LPT sequence, so the
        # passed/failed/aborted/intentionally-skipped/dependency-skipped counts are fully
        # reproducible between the two builds.
        # (Note: --keep-going only suppresses the eager-abort of in-flight steps; on any
        # failure BOTH builds set stop and launch no new steps, so counts still race at -s>1.)
        serial_args = (
            "run", "--dag", dag_path, "-q", "-s", "1", "-j", "64", NOPROF, NOFB, ACF,
        )
        po = run(py, serial_args)
        ro = run(rs, serial_args)
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
            po = run(py, ("run", "--dag", dag_path, "-q", "-k", "--max-mem", fx.max_mem, NOPROF, NOFB, ACF))
            ro = run(rs, ("run", "--dag", dag_path, "-q", "-k", "--max-mem", fx.max_mem, NOPROF, NOFB, ACF))
            label = f"{fx.name}/sizing"
            ps, rss = _sizing(po.stderr), _sizing(ro.stderr)
            prefusal, rrefusal = _sizing_refusal(po.stderr), _sizing_refusal(ro.stderr)
            if (
                po.returncode == ro.returncode == 2
                and prefusal is not None
                and prefusal == rrefusal
            ):
                rep.ok(label)
            elif ps is None or rss is None:
                rep.bad(label, f"missing sizing line py={po.stderr!r} rs={ro.stderr!r}")
            elif ps != rss:
                rep.bad(label, f"(--max-steps, footprint, budget) py={ps} rs={rss}")
            else:
                rep.ok(label)

        # 6) --only selection parity (Feature A): running EXACTLY the named step(s) must agree on
        # exit code AND the passed/failed/aborted/intentionally-skipped/dependency-skipped counts
        # across both builds. Selecting a
        # single step at -s1 is deterministic (its deps outside the selection are dropped), so the
        # counts are reproducible even though full-DAG timing is not.
        tag = _first_tag(fx)
        if tag is not None:
            only_args = (
                "run", "--dag", dag_path, "-q", "-s", "1", "-j", "64", "--only", tag,
                NOPROF, NOFB, ACF,
            )
            po = run(py, only_args)
            ro = run(rs, only_args)
            label = f"{fx.name}/only({tag})"
            pc, rc = _counts(po.stderr), _counts(ro.stderr)
            if po.returncode != ro.returncode:
                rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}")
            elif pc is None or rc is None:
                rep.bad(label, f"missing summary counts py={po.stderr!r} rs={ro.stderr!r}")
            elif pc != rc:
                rep.bad(label, f"counts py={pc} rs={rc}")
            elif pc != (1, 0, 0, 0, 0) and pc != (0, 1, 0, 0, 0):
                # --only <one tag> runs exactly one step: it either passes or fails, nothing else.
                rep.bad(label, f"--only one step should run exactly one step; got counts {pc}")
            else:
                rep.ok(label)


def _store_csv_names(directory: str) -> list[str]:
    """Sorted basenames of the ``*.csv`` files in a profile-store directory (``[]`` if absent)."""
    if not os.path.isdir(directory):
        return []
    return sorted(name for name in os.listdir(directory) if name.endswith(".csv"))


def _header_and_eol(path: str) -> tuple[str, str]:
    """The first line (header, with any trailing ``\\r`` stripped) and the line-ending STYLE
    (``"CRLF"`` / ``"LF"`` / ``"none"``) of a CSV file, read as raw bytes so the terminator is
    observed exactly (never newline-translated)."""
    with open(path, "rb") as fh:
        data = fh.read()
    eol = "CRLF" if b"\r\n" in data else ("LF" if b"\n" in data else "none")
    header = data.split(b"\n", 1)[0].rstrip(b"\r").decode("utf-8")
    return header, eol


def _profile_width_records(directory: str) -> list[tuple[str, str]]:
    """Read ``(inner_jobs, container_class)`` from the one per-step profile CSV."""

    names = [name for name in _store_csv_names(directory) if name.startswith("step_profiles_")]
    if len(names) != 1:
        return []
    with open(os.path.join(directory, names[0]), newline="", encoding="utf-8") as handle:
        return [
            (row.get("inner_jobs", ""), row.get("container_class", ""))
            for row in csv.DictReader(handle)
        ]


def _ambient_width_from_container_class(container_class: str) -> int | None:
    """Mirror the conservative whole-core budget encoded in a profile identity."""

    matched = re.fullmatch(
        r"affinity([0-9]+)_cpu-max-(max|unknown|([0-9]+)_([0-9]+))",
        container_class,
    )
    if matched is None:
        return None
    affinity = int(matched.group(1))
    quota_text = matched.group(2)
    if quota_text in {"max", "unknown"}:
        return max(1, affinity)
    quota = int(matched.group(3))
    period = int(matched.group(4))
    if period <= 0:
        return None
    return max(1, min(affinity, quota // period))


def compare_profile_store(py: list[str], rs: list[str], rep: Report) -> None:
    """Assert the auto-logging profile STORE (Feature D) has an identical on-disk schema in both
    builds. Runs the SAME tiny DAG under each build with ``--perf-dir`` into a fresh temp dir
    (explicitly unboxed, so it is environment-independent), then asserts the two
    stores agree on: (a) the SET of CSV filenames (proving ``machine_id`` + ``container_class``, and
    hence ``nproc``, agree), (b) each file's HEADER row byte-for-byte, and (c) the line-ending
    style. It also proves that explicitly unboxed undeclared steps retain the same positive ambient
    width in both engines instead of claiming a cgroup default that was not enforced; boxed
    default-width behavior is pinned by each engine's scheduler tests. Other data cells are not
    compared because timestamps/elapsed/git-SHA differ."""
    dag = (
        '{"steps": [{"group": "g", "job": "a", "cmd": "true"}, '
        '{"group": "g", "job": "b", "cmd": "true", "deps": ["g.a"]}]}'
    )
    with tempfile.TemporaryDirectory() as tmp:
        dag_path = os.path.join(tmp, "dag.json")
        with open(dag_path, "w", encoding="utf-8") as fh:
            fh.write(dag)
        py_dir = os.path.join(tmp, "py_store")
        rs_dir = os.path.join(tmp, "rs_store")
        po = run(
            py,
            ("run", "--dag", dag_path, "-q", "-s", "1", "-j", "4", "--perf-dir", py_dir,
             NOFB, "--unsafe-no-cgroups"),
        )
        ro = run(
            rs,
            ("run", "--dag", dag_path, "-q", "-s", "1", "-j", "4", "--perf-dir", rs_dir,
             NOFB, "--unsafe-no-cgroups"),
        )
        label = "profile-store"
        if po.returncode != ro.returncode:
            rep.bad(label, f"run exit py={po.returncode} rs={ro.returncode}")
            return
        py_names = _store_csv_names(py_dir)
        rs_names = _store_csv_names(rs_dir)
        if py_names != rs_names:
            rep.bad(
                label,
                f"profile-store filename sets differ (machine_id/container_class/nproc mismatch)\n"
                f"--- py ---\n{py_names}\n--- rs ---\n{rs_names}",
            )
            return
        if not py_names:
            rep.bad(label, "no profile CSVs were written by either build")
            return
        py_records = _profile_width_records(py_dir)
        rs_records = _profile_width_records(rs_dir)
        expected = (
            _ambient_width_from_container_class(py_records[0][1])
            if py_records
            else None
        )
        valid_ambient = (
            len(py_records) == 2
            and expected is not None
            and all(width == str(expected) for width, _container in py_records)
        )
        if py_records != rs_records or not valid_ambient:
            rep.bad(
                f"{label}:unboxed-ambient-inner-jobs",
                f"unboxed undeclared steps must record their identity-derived ambient width "
                f"{expected!r}; py={py_records!r} rs={rs_records!r}",
            )
        else:
            rep.ok(f"{label}:unboxed-ambient-inner-jobs")
        for name in py_names:
            py_hdr, py_eol = _header_and_eol(os.path.join(py_dir, name))
            rs_hdr, rs_eol = _header_and_eol(os.path.join(rs_dir, name))
            sub = f"{label}:{name}"
            if py_eol != rs_eol:
                rep.bad(sub, f"line endings differ py={py_eol} rs={rs_eol}")
            elif py_hdr != rs_hdr:
                rep.bad(sub, f"header differs\n--- py ---\n{py_hdr}\n--- rs ---\n{rs_hdr}")
            else:
                rep.ok(sub)


# --------------------------------------------------------------------------- plan feedback

#: A DAG whose steps carry only est_duration HINTS (no rss baselines) so the SYNTHETIC store below
#: is the sole source of learned durations + memory estimates. Its shape makes the two planners
#: disagree: g.prep is cheap on its own but heads a long chain (bottom-level 1+8), so critical-path
#: dispatches it first while greedy-lpt dispatches the individually-largest g.heavy/g.solo first.
_FEEDBACK_DAG = {
    "mem_cap_factor": 1.0,
    "mem_cap_floor_bytes": 0,
    "outer_mem_safety_factor": 1.0,
    "steps": [
        {"group": "g", "job": "prep", "desc": "prep", "cmd": "true",
         "hint": {"est_duration_s": 1.0}},
        {"group": "g", "job": "heavy", "desc": "heavy", "cmd": "true", "deps": ["g.prep"],
         "hint": {"est_duration_s": 10.0}},
        {"group": "g", "job": "solo", "desc": "solo", "cmd": "true",
         "hint": {"est_duration_s": 5.0}},
    ],
}

#: A fixed synthetic per-step profile store for :data:`_FEEDBACK_DAG`. g.heavy's 20s sample was
#: taken under 60% other-work contention, so the reader must DISCOUNT it back to ~8s (matching the
#: uncontended 8s sample) — proving contention-discounted median duration recovery. peak_bytes give
#: the memory model its rss estimates (6 GiB for heavy/solo => a tight --max-mem budget throttles to
#: --max-steps 1). Written with the pinned SYNTH identity so the file name matches what the reader
#: loads.
_FEEDBACK_STORE_CSV = (
    "timestamp,machine_id,container_class,git_sha,outer_jobs,profile_base_sha,enforcement_kind,"
    "runner_name,step,classification,inner_jobs,elapsed_s,returncode,ok,timed_out,oom_kills,"
    "peak_bytes,thread_peak,pct_other\n"
    f"2026-07-26T10:00:00,{SYNTH_MACHINE},{SYNTH_CONTAINER},abc,1,abc,unverified,local,"
    "g.prep,light,1,1.000,0,True,False,0,1073741824,,0.0\n"
    f"2026-07-26T10:00:01,{SYNTH_MACHINE},{SYNTH_CONTAINER},abc,1,abc,unverified,local,"
    "g.prep,light,1,1.100,0,True,False,0,1073741824,,0.0\n"
    f"2026-07-26T10:00:02,{SYNTH_MACHINE},{SYNTH_CONTAINER},abc,1,abc,unverified,local,"
    "g.heavy,cpu-bound,1,8.000,0,True,False,0,6442450944,,0.0\n"
    f"2026-07-26T10:00:03,{SYNTH_MACHINE},{SYNTH_CONTAINER},abc,1,abc,unverified,local,"
    "g.heavy,cpu-bound,1,20.000,0,True,False,0,6442450944,,60.0\n"
    f"2026-07-26T10:00:04,{SYNTH_MACHINE},{SYNTH_CONTAINER},abc,1,abc,unverified,local,"
    "g.solo,light,1,5.000,0,True,False,0,6442450944,,0.0\n"
    f"2026-07-26T10:00:05,{SYNTH_MACHINE},{SYNTH_CONTAINER},abc,1,abc,unverified,local,"
    "g.solo,light,1,5.200,0,True,False,0,6442450944,,0.0\n"
)


#: A DAG whose two steps carry only large est_duration HINTS, so the hostile store below (whose
#: numeric cells the reader must parse identically in both builds) is the sole source of the learned
#: durations/rss. If either build failed to parse a hostile cell the way the other does, the store
#: estimate — and thus the plan JSON — would diverge.
_HOSTILE_DAG = {
    "mem_cap_factor": 1.0,
    "mem_cap_floor_bytes": 0,
    "outer_mem_safety_factor": 1.0,
    "steps": [
        {"group": "h", "job": "ws", "desc": "ws", "cmd": "true",
         "hint": {"est_duration_s": 99.0}},
        {"group": "h", "job": "us", "desc": "us", "cmd": "true",
         "hint": {"est_duration_s": 99.0, "rss_baseline_bytes": 4242}},
    ],
}

#: A per-step profile store full of HOSTILE numeric cells that Python's ``float()``/``int()`` accept
#: but Rust's ``str::parse`` rejects (or vice versa) unless both builds normalize identically:
#: whitespace-padded ``elapsed_s`` and ``pct_other`` (must TRIM and then apply the discount in both),
#: PEP-515 underscore separators (must be REJECTED in both), and an out-of-i64 ``peak_bytes`` (must
#: be REJECTED in both — Python's arbitrary-precision int would otherwise keep it). This is the
#: adversarial data the earlier clean-only fixture could not exercise; it guards parse-helper
#: equivalence across the two builds.
#:
#: Expected (identical) derivation in BOTH builds:
#:   h.ws elapsed [' 8.0 '->8, 4.0, '10.0' under ' 50.0 '% -> 5] => robust median 5.000 (n=3); peaks
#:        [1000,2000,3000] => p90 3000.
#:   h.us elapsed ['1_0.0' rejected, 4.0, 6.0] => two surviving samples cannot MAD-reject an outlier,
#:        so the robust estimate is the MINIMUM => 4.000 (n<3); peaks ['1_000' rejected,
#:        9999999999999999999999 rejected, 5000] => p90 5000.
_HOSTILE_STORE_CSV = (
    "timestamp,machine_id,container_class,git_sha,outer_jobs,profile_base_sha,enforcement_kind,"
    "runner_name,step,classification,inner_jobs,elapsed_s,returncode,ok,timed_out,oom_kills,"
    "peak_bytes,thread_peak,pct_other\n"
    f"2026-07-26T10:00:00,{SYNTH_MACHINE},{SYNTH_CONTAINER},abc,1,abc,unverified,local,"
    "h.ws,light,1, 8.0 ,0,True,False,0,1000,,0.0\n"
    f"2026-07-26T10:00:01,{SYNTH_MACHINE},{SYNTH_CONTAINER},abc,1,abc,unverified,local,"
    "h.ws,light,1,4.0,0,True,False,0,2000,,0.0\n"
    f"2026-07-26T10:00:02,{SYNTH_MACHINE},{SYNTH_CONTAINER},abc,1,abc,unverified,local,"
    "h.ws,light,1,10.0,0,True,False,0,3000,, 50.0 \n"
    f"2026-07-26T10:00:03,{SYNTH_MACHINE},{SYNTH_CONTAINER},abc,1,abc,unverified,local,"
    "h.us,light,1,1_0.0,0,True,False,0,1_000,,0.0\n"
    f"2026-07-26T10:00:04,{SYNTH_MACHINE},{SYNTH_CONTAINER},abc,1,abc,unverified,local,"
    "h.us,light,1,4.0,0,True,False,0,9999999999999999999999,,0.0\n"
    f"2026-07-26T10:00:05,{SYNTH_MACHINE},{SYNTH_CONTAINER},abc,1,abc,unverified,local,"
    "h.us,light,1,6.0,0,True,False,0,5000,,0.0\n"
)


def compare_hostile_numeric_cells(py: list[str], rs: list[str], rep: Report) -> None:
    """Prove the profile-store numeric-cell PARSING is equivalent across builds on ADVERSARIAL data.

    Feeds a fixed store whose cells stress the Python-``float()``/``int()`` vs Rust-``str::parse``
    difference (leading/trailing whitespace, PEP-515 ``_`` separators, out-of-i64 magnitudes) and
    asserts ``plan --format json`` is BYTE-IDENTICAL across the two builds — plus that the derived
    values actually reflect the trim/reject rules (so the check proves the normalization happened,
    not merely that both builds fell back to the hint). This is the regression guard the earlier
    clean-only feedback fixture could not provide."""
    extra = {
        "SAFE_CI_DAG_RUNNER_MACHINE_ID": SYNTH_MACHINE,
        "SAFE_CI_DAG_RUNNER_CONTAINER_CLASS": SYNTH_CONTAINER,
    }
    with tempfile.TemporaryDirectory() as tmp:
        store = os.path.join(tmp, "store")
        os.makedirs(store, exist_ok=True)
        csv_name = f"step_profiles_{SYNTH_MACHINE}_{SYNTH_CONTAINER}.csv"
        with open(os.path.join(store, csv_name), "w", encoding="utf-8") as fh:
            fh.write(_HOSTILE_STORE_CSV)
        dag_path = os.path.join(tmp, "dag.json")
        with open(dag_path, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(_HOSTILE_DAG))

        args = ("plan", "--dag", dag_path, "--perf-dir", store, "--format", "json")
        po = run(py, args, extra)
        ro = run(rs, args, extra)
        if po.returncode != 0 or ro.returncode != 0:
            rep.bad(
                "hostile-cells",
                f"exit py={po.returncode} rs={ro.returncode} "
                f"(py stderr={po.stderr!r}; rs stderr={ro.stderr!r})",
            )
            return
        if po.stdout != ro.stdout:
            rep.bad(
                "hostile-cells",
                f"plan output not byte-identical on hostile numeric cells\n--- py ---\n{po.stdout}\n"
                f"--- rs ---\n{ro.stdout}",
            )
            return
        rep.ok("hostile-cells")
        # Positive proof the whitespace-trim + reject rules actually fired (both agree, checked py):
        #   both hostile steps must derive their est_duration FROM THE STORE (not the 99.0 hint).
        #   h.ws has 3 surviving samples -> robust median 5.000; h.us has 2 -> the small-sample
        #   estimator returns the minimum 4.000 (see estimates._robust_median). Distinct values also
        #   prove BOTH steps were read (not one value coincidentally matched twice).
        if (
            '"est_source": "store"' in po.stdout
            and po.stdout.count('"est_duration_s": "5.000"') == 1
            and po.stdout.count('"est_duration_s": "4.000"') == 1
        ):
            rep.ok("hostile-cells:trim-applied")
        else:
            rep.bad(
                "hostile-cells:trim-applied",
                "expected h.ws to learn est_duration 5.000 and h.us 4.000 from the trimmed store "
                f"cells; got\n{po.stdout}",
            )


def _order_from_plan_json(stdout: str) -> list[str] | None:
    """Extract the scheduled ``order`` list from ``plan --format json`` output (``None`` if
    unparseable)."""
    try:
        obj = json.loads(stdout)
    except json.JSONDecodeError:
        return None
    if not isinstance(obj, dict):
        return None
    order = obj.get("order")
    if not isinstance(order, list) or not all(isinstance(x, str) for x in order):
        return None
    return [str(x) for x in order]


def compare_plan_feedback(py: list[str], rs: list[str], rep: Report) -> None:
    """Prove the profile-store FEEDBACK loop + the ``--planner`` choices agree across builds.

    Against a FIXED synthetic store (pinned to a host-independent machine/container identity):

    * ``plan`` output is BYTE-IDENTICAL across the two builds for BOTH planners and BOTH formats —
      so the derived estimates (contention-discounted median duration, high-percentile rss) and the
      dispatch order match exactly.
    * the ``critical-path`` scheduled order DIFFERS from ``greedy-lpt`` (the planner actually
      reorders), while both remain identical across builds.
    * ``plan`` with feedback OFF (``--no-profile-feedback``) is also byte-identical (hint-only path).
    * the memory-aware ``--max-mem`` sizing decision, now fed the store's rss estimates, matches
      across builds AND throttles below the CPU count (proving the store feeds the memory model).
    """
    extra = {
        "SAFE_CI_DAG_RUNNER_MACHINE_ID": SYNTH_MACHINE,
        "SAFE_CI_DAG_RUNNER_CONTAINER_CLASS": SYNTH_CONTAINER,
    }
    with tempfile.TemporaryDirectory() as tmp:
        store = os.path.join(tmp, "store")
        os.makedirs(store, exist_ok=True)
        csv_name = f"step_profiles_{SYNTH_MACHINE}_{SYNTH_CONTAINER}.csv"
        with open(os.path.join(store, csv_name), "w", encoding="utf-8") as fh:
            fh.write(_FEEDBACK_STORE_CSV)
        dag_path = os.path.join(tmp, "dag.json")
        with open(dag_path, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(_FEEDBACK_DAG))

        orders: dict[str, list[str]] = {}
        for planner in ("greedy-lpt", "critical-path"):
            for fmt in ("text", "json"):
                po = run(
                    py,
                    ("plan", "--dag", dag_path, "--perf-dir", store, "--planner", planner,
                     "--format", fmt),
                    extra,
                )
                ro = run(
                    rs,
                    ("plan", "--dag", dag_path, "--perf-dir", store, "--planner", planner,
                     "--format", fmt),
                    extra,
                )
                label = f"plan-feedback:{planner}/{fmt}"
                if po.returncode != 0 or ro.returncode != 0:
                    rep.bad(label, f"exit py={po.returncode} rs={ro.returncode} "
                                   f"(py stderr={po.stderr!r}; rs stderr={ro.stderr!r})")
                    continue
                if po.stdout != ro.stdout:
                    rep.bad(
                        label,
                        f"plan output not byte-identical\n--- py ---\n{po.stdout}\n"
                        f"--- rs ---\n{ro.stdout}",
                    )
                    continue
                rep.ok(label)
                if fmt == "json":
                    order = _order_from_plan_json(po.stdout)
                    if order is None:
                        rep.bad(f"plan-feedback:{planner}/order", "unparseable order")
                    else:
                        orders[planner] = order

        # The two planners must actually produce a DIFFERENT scheduled order (else --planner is a
        # no-op on this fixture). Both builds already agree per-planner (asserted above).
        if "greedy-lpt" in orders and "critical-path" in orders:
            if orders["greedy-lpt"] == orders["critical-path"]:
                rep.bad(
                    "plan-feedback:planner-differs",
                    f"greedy-lpt and critical-path produced the SAME order {orders['greedy-lpt']}",
                )
            else:
                rep.ok("plan-feedback:planner-differs")

        # Feedback OFF: hint-only plan must still be byte-identical across builds.
        po = run(py, ("plan", "--dag", dag_path, "--no-profile-feedback", "--format", "json"), extra)
        ro = run(rs, ("plan", "--dag", dag_path, "--no-profile-feedback", "--format", "json"), extra)
        if po.stdout != ro.stdout:
            rep.bad(
                "plan-feedback:no-feedback",
                f"hint-only plan differs\n--- py ---\n{po.stdout}\n--- rs ---\n{ro.stdout}",
            )
        else:
            rep.ok("plan-feedback:no-feedback")

        # Memory-aware sizing fed by the store's rss estimates: both builds must pick the same
        # --max-steps and throttle below the CPU count (the 6 GiB heavy+solo pair overflows an
        # 8 GiB budget).
        po = run(
            py,
            ("run", "--dag", dag_path, "-q", "-k", "--max-mem", "8G", "--perf-dir", store,
             NOPROF, ACF),
            extra,
        )
        ro = run(
            rs,
            ("run", "--dag", dag_path, "-q", "-k", "--max-mem", "8G", "--perf-dir", store,
             NOPROF, ACF),
            extra,
        )
        ps, rss = _sizing(po.stderr), _sizing(ro.stderr)
        if ps is None or rss is None:
            rep.bad(
                "plan-feedback:sizing",
                f"missing sizing line py={po.stderr!r} rs={ro.stderr!r}",
            )
        elif ps != rss:
            rep.bad(
                "plan-feedback:sizing",
                f"(--max-steps, footprint, budget) py={ps} rs={rss}",
            )
        elif ps[0] != 1:
            rep.bad(
                "plan-feedback:sizing",
                "expected the store's rss estimates to throttle to --max-steps 1; "
                f"got --max-steps {ps[0]}",
            )
        else:
            rep.ok("plan-feedback:sizing")


# --------------------------------------------------------------------------- speedup model

#: Pinned identity for the speedup fixtures. The container class is an ``affinityN`` form so the
#: reader parses a core BUDGET (8 cores) from it — exercising the budget cap in the recommendation.
_SPEEDUP_MACHINE = "cross_speedup_machine"
_SPEEDUP_CONTAINER = "affinity8_cpu-max-max"

#: A DAG whose steps carry only est_duration HINTS; the synthetic multi-inner_jobs store below is the
#: sole source of the learned speedup curves. Four steps cover the shapes the model must distinguish.
_SPEEDUP_DAG = {
    "mem_cap_factor": 1.0,
    "mem_cap_floor_bytes": 0,
    "outer_mem_safety_factor": 1.0,
    "steps": [
        {"group": "p", "job": "lin", "desc": "linear", "cmd": "true",
         "hint": {"est_duration_s": 9.0}},
        {"group": "p", "job": "knee", "desc": "sub-linear knee", "cmd": "true",
         "hint": {"est_duration_s": 9.0}},
        {"group": "p", "job": "plat", "desc": "plateau", "cmd": "true",
         "hint": {"est_duration_s": 9.0}},
        {"group": "p", "job": "bud", "desc": "budget-capped", "cmd": "true",
         "hint": {"est_duration_s": 9.0}},
    ],
}

_SPEEDUP_HEADER = (
    "timestamp,machine_id,container_class,git_sha,outer_jobs,profile_base_sha,enforcement_kind,"
    "runner_name,step,classification,inner_jobs,elapsed_s,returncode,ok,timed_out,oom_kills,"
    "peak_bytes,thread_peak,effective_cores,user_s,sys_s,throttled_s\n"
)


def _speedup_row(
    step: str, inner_jobs: int, elapsed: str, eff: str, user_s: str, sys_s: str, throttled: str
) -> str:
    return (
        f"2026-07-26T10:00:00,{_SPEEDUP_MACHINE},{_SPEEDUP_CONTAINER},abc,1,abc,unverified,local,"
        f"{step},cpu-bound,{inner_jobs},{elapsed},0,True,False,0,1000,,{eff},{user_s},{sys_s},"
        f"{throttled}\n"
    )


def _speedup_store_csv() -> str:
    """A fixed synthetic per-step store spanning LINEAR, SUB-LINEAR (knee), PLATEAU, and
    BUDGET-CAPPED speedup shapes — the data the py and rs speedup readers must fold IDENTICALLY.

    Expected recommendations (identical in both builds):
      * p.lin   : wall 8->4->2 with flat CPU-s -> near-linear, recommend the widest measured (-j4).
      * p.knee  : wall 10->5->4.5 with CPU-s rising 10.2->18 -> sub-linear, recommend -j2 (marginal
                  gain 2->4 is <1.15 AND total CPU-s more than doubles: the knee is at 2).
      * p.plat  : wall 6->5.8 -> the -j2 gain (1.03x) is below the threshold, recommend -j1.
      * p.bud   : wall halves cleanly to -j16, but the affinity-8 core budget caps it at -j8.
    """
    rows: list[str] = []
    # p.lin — near-linear, flat total CPU-seconds (two samples per width).
    for j, walls in ((1, ("8.0", "8.0")), (2, ("4.0", "4.0")), (4, ("2.0", "2.0"))):
        for w in walls:
            rows.append(_speedup_row("p.lin", j, w, "", "8.0", "0.0", "0.0"))
    # p.knee — halves 1->2 but flattens 2->4 while total CPU-seconds blow up (work-conservation).
    rows.append(_speedup_row("p.knee", 1, "10.0", "1.0", "10.0", "0.2", "0.0"))
    rows.append(_speedup_row("p.knee", 1, "10.1", "1.0", "10.1", "0.2", "0.0"))
    rows.append(_speedup_row("p.knee", 2, "5.0", "1.98", "10.0", "0.4", "0.0"))
    rows.append(_speedup_row("p.knee", 2, "5.05", "1.98", "10.1", "0.4", "0.0"))
    rows.append(_speedup_row("p.knee", 4, "4.5", "3.2", "17.5", "0.5", "1.2"))
    rows.append(_speedup_row("p.knee", 4, "4.6", "3.2", "17.6", "0.5", "1.3"))
    # p.plat — the second width barely helps.
    rows.append(_speedup_row("p.plat", 1, "6.0", "", "6.0", "0.0", "0.0"))
    rows.append(_speedup_row("p.plat", 2, "5.8", "", "6.5", "0.0", "0.0"))
    # p.bud — perfect halving to -j16, but the core budget (8) caps the recommendation.
    for j, wall in ((1, "16.0"), (2, "8.0"), (4, "4.0"), (8, "2.0"), (16, "1.0")):
        rows.append(_speedup_row("p.bud", j, wall, "", "16.0", "0.1", "0.0"))
    return _SPEEDUP_HEADER + "".join(rows)


def compare_speedup_model(py: list[str], rs: list[str], rep: Report) -> None:
    """Prove the per-step PARALLEL-SPEEDUP model is byte-identical across builds on a fixed store.

    Against a synthetic store spanning linear / sub-linear / plateau / budget-capped shapes, assert
    ``plan`` output (BOTH ``text`` and ``json``) is BYTE-IDENTICAL between the Python and Rust
    builds — so the derived speedup curves AND the recommended inner_jobs match exactly — and that
    the recommendations are the expected 4 / 2 / 1 / 8 (positive proof the model actually fired,
    not merely that both builds agree on ``null``)."""
    extra = {
        "SAFE_CI_DAG_RUNNER_MACHINE_ID": _SPEEDUP_MACHINE,
        "SAFE_CI_DAG_RUNNER_CONTAINER_CLASS": _SPEEDUP_CONTAINER,
    }
    with tempfile.TemporaryDirectory() as tmp:
        store = os.path.join(tmp, "store")
        os.makedirs(store, exist_ok=True)
        csv_name = f"step_profiles_{_SPEEDUP_MACHINE}_{_SPEEDUP_CONTAINER}.csv"
        with open(os.path.join(store, csv_name), "w", encoding="utf-8") as fh:
            fh.write(_speedup_store_csv())
        dag_path = os.path.join(tmp, "dag.json")
        with open(dag_path, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(_SPEEDUP_DAG))

        json_out = ""
        for fmt in ("text", "json"):
            args = ("plan", "--dag", dag_path, "--perf-dir", store, "--format", fmt)
            po = run(py, args, extra)
            ro = run(rs, args, extra)
            label = f"speedup-model:{fmt}"
            if po.returncode != 0 or ro.returncode != 0:
                rep.bad(label, f"exit py={po.returncode} rs={ro.returncode} "
                               f"(py stderr={po.stderr!r}; rs stderr={ro.stderr!r})")
                continue
            if po.stdout != ro.stdout:
                rep.bad(
                    label,
                    f"speedup plan not byte-identical\n--- py ---\n{po.stdout}\n--- rs ---\n{ro.stdout}",
                )
                continue
            rep.ok(label)
            if fmt == "json":
                json_out = po.stdout

        # Positive proof: the four shapes yield the expected recommended widths in BOTH builds
        # (already asserted byte-identical above; checked once on the shared output).
        expected = {"p.lin": 4, "p.knee": 2, "p.plat": 1, "p.bud": 8}
        try:
            obj = json.loads(json_out)
            recs = {
                step["tag"]: step["speedup"]["recommended_inner_jobs"]
                for step in obj.get("steps", [])
                if isinstance(step.get("speedup"), dict)
            }
        except (json.JSONDecodeError, KeyError, TypeError):
            recs = {}
        if recs == expected:
            rep.ok("speedup-model:recommendations")
        else:
            rep.bad(
                "speedup-model:recommendations",
                f"recommended inner_jobs mismatch: expected {expected}, got {recs}",
            )

        # A run's explicit total CPU budget must also bound profile-derived recommendations shown
        # by --show-plan under EVERY planner, not only CPA. Measurements above P remain visible in
        # the curve, but no actionable recommendation may exceed the outer quota.
        for planner in ("greedy-lpt", "critical-path", "cpa"):
            run_args: tuple[str, ...] = (
                "run", "--dag", dag_path, "--perf-dir", store, "--planner", planner,
                "--show-plan", "--max-steps", "1", "--max-cpus", "2", NOPROF,
                "--unsafe-no-cgroups", "-q",
            )
            po = run(py, run_args, extra)
            ro = run(rs, run_args, extra)
            label = f"speedup-model:run-budget/{planner}"
            if po.returncode != ro.returncode or po.returncode != 0:
                rep.bad(label, f"py={po}\nrs={ro}")
                continue
            if po.stdout != ro.stdout:
                rep.bad(
                    label,
                    f"budgeted --show-plan differs\n--- py ---\n{po.stdout}\n--- rs ---\n{ro.stdout}",
                )
                continue
            table = po.stdout.partition("parallel-speedup model")[2]
            recommendations: dict[str, int] = {}
            for step in expected:
                match = re.search(rf"(?m)^{re.escape(step)}\s+(\d+)\s+", table)
                if match is not None:
                    recommendations[step] = int(match.group(1))
            expected_capped = {"p.lin": 2, "p.knee": 2, "p.plat": 1, "p.bud": 2}
            if recommendations == expected_capped:
                rep.ok(label)
            else:
                rep.bad(
                    label,
                    f"recommendations exceed/miss P=2: expected {expected_capped}, "
                    f"got {recommendations}\n{po.stdout}",
                )

        # Positive execution proof for the profile-derived CPA width: the guest accepts exactly
        # the flag CPA should apply under P=2. This catches a plan/application disconnect even if
        # both implementations render the same recommendation table.
        observed_dag = os.path.join(tmp, "observed-width.json")
        with open(observed_dag, "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "steps": [
                        {
                            "group": "p",
                            "job": "lin",
                            "cmd": 'check() { [ "$*" = "--workers=2" ]; }; check',
                            "jobs_flag": "--workers=",
                            "hint": {
                                "est_duration_s": 9.0,
                                "preferred_inner_jobs": 1,
                            },
                        }
                    ]
                },
                handle,
            )
        applied_args = (
            "run", "--dag", observed_dag, "--perf-dir", store, "--planner", "cpa",
            "--max-steps", "1", "--max-cpus", "2", NOPROF, "--unsafe-no-cgroups", "-q",
        )
        po = run(py, applied_args, extra)
        ro = run(rs, applied_args, extra)
        if po.returncode == ro.returncode == 0:
            rep.ok("speedup-model:cpa-applied-width")
        else:
            rep.bad(
                "speedup-model:cpa-applied-width",
                f"profile-derived CPA width did not reach the guest exactly: py={po}\nrs={ro}",
            )


# --------------------------------------------------------------------------- CPA planner

#: Pinned identity for the CPA fixtures. An ``affinityN`` container so the speedup reader parses a
#: 16-core knee budget (well above the widths used), keeping the recommended widths curve-limited
#: rather than budget-limited.
_CPA_MACHINE = "cross_cpa_machine"
_CPA_CONTAINER = "affinity16_cpu-max-max"

#: A DAG with a long dependency chain (prep -> build -> test) plus an independent plateau step
#: (side). All durations come from the synthetic store below. Its shape makes CPA earn its keep: at
#: inner-jobs=1 the chain critical path is prep(2)+build(40)+test(16) = 58s; the allocator widens the
#: two SCALING chain steps (build, test) to shrink that path while the plateau step (side) and the
#: curveless prep stay narrow.
_CPA_DAG = {
    "mem_cap_factor": 1.0,
    "mem_cap_floor_bytes": 0,
    "outer_mem_safety_factor": 1.0,
    "steps": [
        {"group": "c", "job": "prep", "desc": "prep", "cmd": "true",
         "hint": {"est_duration_s": 2.0}},
        {"group": "c", "job": "build", "desc": "build", "cmd": "true", "deps": ["c.prep"],
         "hint": {"est_duration_s": 40.0}},
        {"group": "c", "job": "test", "desc": "test", "cmd": "true", "deps": ["c.build"],
         "hint": {"est_duration_s": 16.0}},
        {"group": "c", "job": "side", "desc": "side", "cmd": "true",
         "hint": {"est_duration_s": 9.0}},
    ],
}

#: The width-1 critical-path length of :data:`_CPA_DAG` (prep 2 + build 40 + test 16). This is the
#: makespan of the "critical-path with fixed inner-jobs = 1" baseline (side runs in parallel), which
#: the CPA allocation must BEAT once it can widen the scaling chain steps (any core budget >= 2).
_CPA_WIDTH1_CRITICAL_PATH_S = 58.0

_CPA_HEADER = (
    "timestamp,machine_id,container_class,git_sha,outer_jobs,profile_base_sha,enforcement_kind,"
    "runner_name,step,classification,inner_jobs,elapsed_s,returncode,ok,timed_out,oom_kills,"
    "peak_bytes,thread_peak,effective_cores,user_s,sys_s,throttled_s\n"
)


def _cpa_row(
    machine: str, container: str, step: str, inner_jobs: int, elapsed: str, user_s: str,
    peak_bytes: str = "1000",
) -> str:
    return (
        f"t,{machine},{container},a,1,a,unverified,local,{step},cpu-bound,{inner_jobs},{elapsed},"
        f"0,True,False,0,{peak_bytes},,,{user_s},0.0,0.0\n"
    )


def _cpa_store_csv() -> str:
    """A fixed synthetic store for :data:`_CPA_DAG`: build and test scale near-linearly (flat total
    CPU-seconds, so the work-conservation knee does not truncate them) while side plateaus after
    inner-jobs=1. So the allocator's admissible widths are build up to 8, test up to 4, side rigid at
    1 — and CPA widens build+test to collapse the 58s chain."""
    rows: list[str] = []
    for j, w in ((1, "40.0"), (2, "20.0"), (4, "10.0"), (8, "5.0")):
        rows.append(_cpa_row(_CPA_MACHINE, _CPA_CONTAINER, "c.build", j, w, "40.0"))
    for j, w in ((1, "16.0"), (2, "8.0"), (4, "4.0")):
        rows.append(_cpa_row(_CPA_MACHINE, _CPA_CONTAINER, "c.test", j, w, "16.0"))
    for j, w in ((1, "9.0"), (2, "8.7")):
        rows.append(_cpa_row(_CPA_MACHINE, _CPA_CONTAINER, "c.side", j, w, "9.0"))
    return _CPA_HEADER + "".join(rows)


#: A DAG whose one scaling CPU-bound step carries a memory baseline (3 GiB, factor 1.0) so its cap
#: SCALES with width above inner-jobs=4 (``step_mem_cap_for_inner_jobs``: cap*p/4). Widening it
#: 4 -> 8 doubles the modeled footprint from 3 GiB to 6 GiB, so a 5G RAM budget must BLOCK that
#: widening (mem-capped) even though cores are free.
_CPA_MEM_DAG = {
    "mem_cap_factor": 1.0,
    "mem_cap_floor_bytes": 0,
    "outer_mem_safety_factor": 1.0,
    "steps": [
        {"group": "m", "job": "prep", "desc": "prep", "cmd": "true",
         "hint": {"est_duration_s": 2.0}},
        {"group": "m", "job": "heavy", "desc": "heavy", "cmd": "true", "deps": ["m.prep"],
         "hint": {"est_duration_s": 40.0, "rss_baseline_bytes": 3221225472,
                  "classification": "cpu-bound"}},
    ],
}


def _cpa_mem_store_csv() -> str:
    """Store for :data:`_CPA_MEM_DAG`: m.heavy scales near-linearly to inner-jobs=8 (flat CPU-s).

    Every row also carries the same measured 3-GiB RSS as the DAG hint. CPA correctly plans from
    the selected store estimate, so the fixture must not depend on the pre-fix bug where allocation
    saw the authored 3-GiB hint but execution later installed a tiny learned RSS value.
    """
    rows = [
        _cpa_row(
            _CPA_MACHINE,
            _CPA_CONTAINER,
            "m.heavy",
            j,
            w,
            "40.0",
            peak_bytes="3221225472",
        )
        for j, w in ((1, "40.0"), (2, "20.0"), (4, "10.0"), (8, "5.0"))
    ]
    return _CPA_HEADER + "".join(rows)


def _cpa_widths_from_json(stdout: str) -> dict[str, int] | None:
    """Extract ``{tag: alloc_inner_jobs}`` from ``plan --planner cpa --format json`` output."""
    try:
        obj = json.loads(stdout)
    except json.JSONDecodeError:
        return None
    if not isinstance(obj, dict):
        return None
    steps = obj.get("steps")
    if not isinstance(steps, list):
        return None
    out: dict[str, int] = {}
    for s in steps:
        if not isinstance(s, dict):
            return None
        tag, w = s.get("tag"), s.get("alloc_inner_jobs")
        if isinstance(tag, str) and isinstance(w, int):
            out[tag] = w
    return out


def compare_cpa_planner(py: list[str], rs: list[str], rep: Report) -> None:
    """Prove the ``--planner cpa`` moldable allocator agrees across builds AND actually helps.

    Against a fixed synthetic store (host-independent identity), for both output formats:

    * ``plan --planner cpa`` output is BYTE-IDENTICAL py vs rs (so the chosen widths, the dispatch
      order at the allocated weights, the allocator summary, the makespan lower bound, and the
      modeled makespan all match exactly).
    * the allocator FIRED — it widened the scaling chain step (``c.build`` gets inner-jobs > 1) —
      and the modeled makespan never dips below the ``max(T_CP, area/P)`` lower bound.
    * CPA BEATS the fixed-inner-jobs=1 baseline: its modeled makespan is below the 58s width-1
      critical path (whenever the core budget allows any widening, i.e. P >= 2).
    * the MEMORY cap binds: with a tight ``--max-mem`` the memory-heavy step is widened LESS than
      with no budget (and the run reports ``mem-capped``) — provided cores were not the limit.
    """
    extra = {
        "SAFE_CI_DAG_RUNNER_MACHINE_ID": _CPA_MACHINE,
        "SAFE_CI_DAG_RUNNER_CONTAINER_CLASS": _CPA_CONTAINER,
    }
    with tempfile.TemporaryDirectory() as tmp:
        store = os.path.join(tmp, "store")
        os.makedirs(store, exist_ok=True)
        csv_name = f"step_profiles_{_CPA_MACHINE}_{_CPA_CONTAINER}.csv"
        with open(os.path.join(store, csv_name), "w", encoding="utf-8") as fh:
            fh.write(_cpa_store_csv())
        dag_path = os.path.join(tmp, "dag.json")
        with open(dag_path, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(_CPA_DAG))

        json_out = ""
        for fmt in ("text", "json"):
            args = ("plan", "--dag", dag_path, "--perf-dir", store, "--planner", "cpa",
                    "--format", fmt)
            po = run(py, args, extra)
            ro = run(rs, args, extra)
            label = f"cpa:{fmt}"
            if po.returncode != 0 or ro.returncode != 0:
                rep.bad(label, f"exit py={po.returncode} rs={ro.returncode} "
                               f"(py stderr={po.stderr!r}; rs stderr={ro.stderr!r})")
                continue
            if po.stdout != ro.stdout:
                rep.bad(label, f"cpa plan not byte-identical\n--- py ---\n{po.stdout}\n"
                               f"--- rs ---\n{ro.stdout}")
                continue
            rep.ok(label)
            if fmt == "json":
                json_out = po.stdout

        # Positive proofs on the shared (byte-identical) JSON.
        try:
            obj = json.loads(json_out)
            alloc = obj["allocation"]
            widths = _cpa_widths_from_json(json_out) or {}
            core_budget = int(alloc["core_budget"])
            modeled = float(alloc["modeled_makespan_s"])
            lower_bound = float(alloc["lower_bound_s"])
        except (json.JSONDecodeError, KeyError, TypeError, ValueError):
            rep.bad("cpa:allocation", f"unparseable allocation in\n{json_out}")
            return

        # The allocator actually allocated: the scaling chain head widened past 1 (needs P >= 2).
        if core_budget >= 2 and widths.get("c.build", 0) > 1:
            rep.ok("cpa:widened")
        elif core_budget < 2:
            rep.ok("cpa:widened")  # a 1-core box cannot widen; not a divergence
        else:
            rep.bad("cpa:widened",
                    f"expected c.build to be widened past 1 (P={core_budget}); widths={widths}")

        # The modeled makespan must never dip below the max(T_CP, area/P) lower bound.
        if modeled + 1e-9 >= lower_bound:
            rep.ok("cpa:modeled-ge-lower-bound")
        else:
            rep.bad("cpa:modeled-ge-lower-bound",
                    f"modeled {modeled} < lower_bound {lower_bound}")

        # CPA beats the fixed-inner-jobs=1 baseline (58s width-1 critical path) once it can widen.
        if core_budget < 2 or modeled < _CPA_WIDTH1_CRITICAL_PATH_S:
            rep.ok("cpa:beats-fixed")
        else:
            rep.bad("cpa:beats-fixed",
                    f"CPA modeled makespan {modeled}s did not beat the width-1 critical path "
                    f"{_CPA_WIDTH1_CRITICAL_PATH_S}s (P={core_budget})")

        # Memory cap: a tight --max-mem must widen the memory-heavy step LESS than an unbounded run.
        mstore = os.path.join(tmp, "mstore")
        os.makedirs(mstore, exist_ok=True)
        with open(os.path.join(mstore, csv_name), "w", encoding="utf-8") as fh:
            fh.write(_cpa_mem_store_csv())
        mdag = os.path.join(tmp, "mdag.json")
        with open(mdag, "w", encoding="utf-8") as fh:
            fh.write(json.dumps(_CPA_MEM_DAG))
        free_args = ("plan", "--dag", mdag, "--perf-dir", mstore, "--planner", "cpa",
                     "--format", "json")
        capped_args = free_args + ("--max-mem", "5G")
        pf, rf = run(py, free_args, extra), run(rs, free_args, extra)
        pc, rc = run(py, capped_args, extra), run(rs, capped_args, extra)
        if pf.stdout != rf.stdout or pc.stdout != rc.stdout:
            rep.bad("cpa:mem-byte-identical",
                    "cpa --max-mem plan not byte-identical py vs rs")
        else:
            rep.ok("cpa:mem-byte-identical")
            free_w = (_cpa_widths_from_json(pf.stdout) or {}).get("m.heavy", 0)
            capped_w = (_cpa_widths_from_json(pc.stdout) or {}).get("m.heavy", 0)
            try:
                capped_reason = json.loads(pc.stdout)["allocation"]["stop_reason"]
                capped_budget = int(json.loads(pc.stdout)["allocation"]["core_budget"])
            except (json.JSONDecodeError, KeyError, TypeError, ValueError):
                capped_reason, capped_budget = "", 0
            # Only assert the memory throttle when cores were not the binding constraint (P >= 8, so
            # the unbounded run could actually reach width 8 where the footprint doubles).
            if capped_budget < 8:
                rep.ok("cpa:mem-capped")  # core-limited box; memory never got to bind
            elif capped_w < free_w and capped_reason == "mem-capped":
                rep.ok("cpa:mem-capped")
            else:
                rep.bad("cpa:mem-capped",
                        f"expected --max-mem to throttle m.heavy below the unbounded width "
                        f"{free_w} with stop_reason 'mem-capped'; got width {capped_w} "
                        f"reason {capped_reason!r} (P={capped_budget})")

        # A self-managed command wider than any realistic ambient P is not moldable. Standalone
        # CPA must preserve that declared width as infeasible, not invent P as a guest width; the
        # two renderers must agree on the null allocation and infinite modeled makespan.
        fixed_dag = os.path.join(tmp, "fixed-width.json")
        with open(fixed_dag, "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "steps": [
                        {
                            "group": "f",
                            "job": "fixed",
                            "cmd": "true",
                            "jobs_flag": "",
                            "hint": {
                                "est_duration_s": 1.0,
                                "preferred_inner_jobs": 1_000_000_000,
                            },
                        }
                    ]
                },
                handle,
            )
        fixed_args = (
            "plan", "--dag", fixed_dag, "--planner", "cpa", "--format", "json",
            "--no-profile-feedback",
        )
        pfixed = run(py, fixed_args)
        rfixed = run(rs, fixed_args)
        fixed_ok = False
        if pfixed.returncode == rfixed.returncode == 0 and pfixed.stdout == rfixed.stdout:
            try:
                fixed_obj = json.loads(pfixed.stdout)
                fixed_alloc = fixed_obj["allocation"]
                fixed_step = fixed_obj["steps"][0]
                fixed_ok = (
                    fixed_alloc["stop_reason"] == "infeasible-fixed-width"
                    and fixed_alloc["modeled_makespan_s"] == "inf"
                    and fixed_step["alloc_inner_jobs"] is None
                )
            except (json.JSONDecodeError, KeyError, IndexError, TypeError):
                fixed_ok = False
        if fixed_ok:
            rep.ok("cpa:infeasible-fixed-width")
        else:
            rep.bad(
                "cpa:infeasible-fixed-width",
                "fixed self-managed width was not rendered as the same infeasible CPA plan\n"
                f"py={pfixed}\nrs={rfixed}",
            )

        # Intentional skips have zero executable demand. Adding a huge skipped node must not
        # suppress the live step's width, and both plans must expose the skip as zero/skip/null.
        control_dag = os.path.join(tmp, "skip-control.json")
        skipped_dag = os.path.join(tmp, "skip-present.json")
        live_step = {
            "group": "c",
            "job": "build",
            "cmd": "true",
            "jobs_flag": "-j%d",
            "hint": {"est_duration_s": 40.0, "preferred_inner_jobs": 1},
        }
        with open(control_dag, "w", encoding="utf-8") as handle:
            json.dump({"steps": [live_step]}, handle)
        with open(skipped_dag, "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "steps": [
                        {
                            "group": "c",
                            "job": "skipped",
                            "cmd": "false",
                            "jobs_flag": "",
                            "skip_reason": "empty-manifest-bucket",
                            "hint": {
                                "est_duration_s": 100.0,
                                "rss_baseline_bytes": 1_000_000_000_000,
                                "preferred_inner_jobs": 1_000_000_000,
                            },
                        },
                        live_step,
                    ]
                },
                handle,
            )
        control_args = (
            "plan", "--dag", control_dag, "--perf-dir", store, "--planner", "cpa",
            "--format", "json",
        )
        skipped_args = (
            "plan", "--dag", skipped_dag, "--perf-dir", store, "--planner", "cpa",
            "--format", "json",
        )
        pcontrol, rcontrol = run(py, control_args, extra), run(rs, control_args, extra)
        pskipped, rskipped = run(py, skipped_args, extra), run(rs, skipped_args, extra)
        skip_ok = False
        if pcontrol.stdout == rcontrol.stdout and pskipped.stdout == rskipped.stdout:
            try:
                control_obj = json.loads(pcontrol.stdout)
                skipped_obj = json.loads(pskipped.stdout)
                control_live = next(
                    step for step in control_obj["steps"] if step["tag"] == "c.build"
                )
                skipped_by_tag = {step["tag"]: step for step in skipped_obj["steps"]}
                skip_entry = skipped_by_tag["c.skipped"]
                skip_ok = (
                    skipped_by_tag["c.build"]["alloc_inner_jobs"]
                    == control_live["alloc_inner_jobs"]
                    and skip_entry["est_duration_s"] == "0.000"
                    and skip_entry["est_source"] == "skip"
                    and skip_entry["alloc_inner_jobs"] is None
                )
            except (json.JSONDecodeError, KeyError, StopIteration, TypeError):
                skip_ok = False
        if skip_ok:
            rep.ok("cpa:intentional-skip-zero-demand")
        else:
            rep.bad(
                "cpa:intentional-skip-zero-demand",
                "intentional skip changed live allocation or was not zero/skip/null\n"
                f"control py={pcontrol}\ncontrol rs={rcontrol}\n"
                f"skip py={pskipped}\nskip rs={rskipped}",
            )

        # A self-managed fixed width uses an exact measured curve level when one exists. With no
        # exact level it falls back to the independently resolved scalar estimate (store-derived in
        # this CLI fixture); it must not substitute a neighboring curve width.
        fixed_curve_dag = os.path.join(tmp, "fixed-curve-source.json")
        with open(fixed_curve_dag, "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "steps": [
                        {
                            "group": "c",
                            "job": "build",
                            "cmd": "true",
                            "jobs_flag": "",
                            "hint": {"est_duration_s": 13.0, "preferred_inner_jobs": 2},
                        },
                        {
                            "group": "c",
                            "job": "test",
                            "cmd": "true",
                            "jobs_flag": "",
                            "hint": {"est_duration_s": 13.0, "preferred_inner_jobs": 3},
                        },
                    ]
                },
                handle,
            )
        source_args = (
            "plan", "--dag", fixed_curve_dag, "--perf-dir", store, "--planner", "cpa",
            "--format", "json",
        )
        psource, rsource = run(py, source_args, extra), run(rs, source_args, extra)
        source_ok = False
        if psource.returncode == rsource.returncode == 0 and psource.stdout == rsource.stdout:
            try:
                source_obj = json.loads(psource.stdout)
                source_by_tag = {step["tag"]: step for step in source_obj["steps"]}
                source_ok = (
                    source_by_tag["c.build"]["est_duration_s"] == "20.000"
                    and source_by_tag["c.build"]["est_source"] == "store"
                    and source_by_tag["c.test"]["est_duration_s"] == "8.000"
                    and source_by_tag["c.test"]["est_source"] == "store"
                    and source_by_tag["c.build"]["alloc_inner_jobs"] is None
                    and source_by_tag["c.test"]["alloc_inner_jobs"] is None
                )
            except (json.JSONDecodeError, KeyError, TypeError):
                source_ok = False
        if source_ok:
            rep.ok("cpa:self-managed-curve-source")
        else:
            rep.bad(
                "cpa:self-managed-curve-source",
                "self-managed exact/scalar curve provenance diverged or was mislabeled\n"
                f"py={psource}\nrs={rsource}",
            )


def compare_memory_hardening(py: list[str], rs: list[str], rep: Report) -> None:
    """Cross-check the memory semantics that must agree before scheduling begins.

    These cases are intentionally CLI-level rather than duplicate unit tests. They prove that the
    two independently packaged implementations make the same externally visible decision after
    config loading, profile enrichment, CPA allocation, and ``--max-mem`` sizing:

    * CPU-bound caps use each step's effective width;
    * hard-cap-only, default-capped, and selected ``engine_only`` steps remain real memory demand;
    * an infeasible one-step budget refuses before the guest can spawn;
    * learned RSS can make a CPA plan explicitly ``infeasible-memory``; and
    * large concurrent sums saturate in the shared signed-64 domain rather than wrapping or using
      Python's unbounded integer as a different model.
    """

    gib = 1024**3
    i64_max = 2**63 - 1

    def sizing_case(
        directory: str,
        label: str,
        dag: Mapping[str, object],
        budget: str,
        max_cpus: int,
        expected: tuple[int, int, int],
    ) -> None:
        dag_path = os.path.join(directory, f"{label}.json")
        with open(dag_path, "w", encoding="utf-8") as handle:
            json.dump(dag, handle)
        args = (
            "run", "--dag", dag_path, "--max-mem", budget, "--max-cpus", str(max_cpus),
            "--unsafe-no-cgroups", "-q", NOPROF, NOFB,
        )
        po, ro = run(py, args), run(rs, args)
        py_sizing, rs_sizing = _sizing(po.stderr), _sizing(ro.stderr)
        if po.returncode != ro.returncode or po.returncode != 0:
            rep.bad(f"memory:{label}", f"py={po}\nrs={ro}")
        elif py_sizing is None or rs_sizing is None:
            rep.bad(
                f"memory:{label}",
                f"missing sizing evidence py={po.stderr!r} rs={ro.stderr!r}",
            )
        elif py_sizing != rs_sizing:
            rep.bad(
                f"memory:{label}",
                f"sizing differs py={py_sizing} rs={rs_sizing}",
            )
        elif py_sizing != expected:
            rep.bad(
                f"memory:{label}",
                f"expected sizing {expected}, got {py_sizing}",
            )
        else:
            rep.ok(f"memory:{label}")

    def refusal_case(
        directory: str,
        label: str,
        step: Mapping[str, object],
        *,
        budget: str,
        budget_bytes: int,
        footprint: int,
    ) -> None:
        marker = {
            engine: os.path.join(directory, f"{label}-{engine}-spawned")
            for engine in ("py", "rs")
        }
        value: dict[str, object] = {
            "mem_cap_factor": 1.0,
            "mem_cap_floor_bytes": 0,
            "outer_mem_safety_factor": 1.0,
            "steps": [step],
        }
        dag_path = os.path.join(directory, f"{label}.json")
        with open(dag_path, "w", encoding="utf-8") as handle:
            json.dump(value, handle)
        args = (
            "run", "--dag", dag_path, "--max-mem", budget, "--max-cpus", "2",
            "--unsafe-no-cgroups", "-q", NOPROF, NOFB,
        )
        outcomes = {
            engine: run(command, args, {"MEMORY_MARKER": marker[engine]})
            for engine, command in (("py", py), ("rs", rs))
        }
        phrase = (
            f"minimum runnable footprint {footprint} bytes cannot fit safely within budget "
            f"{budget_bytes} bytes"
        )
        if (
            all(outcome.returncode == 2 for outcome in outcomes.values())
            and all(phrase in outcome.stderr for outcome in outcomes.values())
            and not any(os.path.exists(path) for path in marker.values())
        ):
            rep.ok(f"memory:{label}")
        else:
            rep.bad(
                f"memory:{label}",
                f"expected pre-spawn infeasible refusal containing {phrase!r}; "
                f"outcomes={outcomes} markers={marker}",
            )

    with tempfile.TemporaryDirectory(prefix="safe-ci-cross-memory-") as tmp:
        width_scaled = {
            "mem_cap_factor": 1.0,
            "mem_cap_floor_bytes": 0,
            "outer_mem_safety_factor": 1.0,
            "steps": [
                {
                    "group": "width",
                    "job": "preferred",
                    "cmd": "sleep 0.05",
                    "jobs_flag": "",
                    "hint": {
                        "rss_baseline_bytes": gib,
                        "classification": "cpu-bound",
                        "preferred_inner_jobs": 8,
                    },
                },
                {
                    "group": "width",
                    "job": "sibling",
                    "cmd": "sleep 0.05",
                    "jobs_flag": "",
                    "hint": {
                        "rss_baseline_bytes": gib,
                        "classification": "cpu-bound",
                        "preferred_inner_jobs": 8,
                    },
                },
            ],
        }
        sizing_case(
            tmp,
            "effective-j8",
            width_scaled,
            "3G",
            8,
            (1, 2 * gib, 3 * gib),
        )

        # A nonbinding memory model must never loosen the CPU-derived active-step base. Four
        # independent 1-GiB steps fit the 16-GiB memory budget, so the modeled ceiling reaches the
        # host CPU count; explicit -j1 still makes the final active-step ceiling exactly one. A
        # genuinely one-CPU host cannot demonstrate a strictly looser memory ceiling and is
        # reported as a capability skip after all remaining invariants agree.
        nonbinding = {
            "mem_cap_factor": 1.0,
            "mem_cap_floor_bytes": 0,
            "outer_mem_safety_factor": 1.0,
            "steps": [
                {
                    "group": "nonbinding",
                    "job": f"s{index}",
                    "cmd": "sleep 0.05",
                    "jobs_flag": "",
                    "hint": {"hard_mem_max_bytes": gib},
                }
                for index in range(4)
            ],
        }
        nonbinding_path = os.path.join(tmp, "nonbinding-max-mem.json")
        with open(nonbinding_path, "w", encoding="utf-8") as handle:
            json.dump(nonbinding, handle)
        nonbinding_args = (
            "run", "--dag", nonbinding_path, "--max-mem", "16G", "--max-cpus", "1",
            "--unsafe-no-cgroups", "-q", NOPROF, NOFB,
        )
        pnon, rnon = run(py, nonbinding_args), run(rs, nonbinding_args)
        pdetails, rdetails = _sizing_details(pnon.stderr), _sizing_details(rnon.stderr)
        common_nonbinding = (
            pnon.returncode == rnon.returncode == 0
            and pdetails is not None
            and pdetails == rdetails
            and pdetails[1] == min(pdetails[0], 4) * gib
            and pdetails[2:] == (16 * gib, 1, 1)
        )
        if common_nonbinding and pdetails is not None and pdetails[0] > 1:
            rep.ok("memory:nonbinding-max-mem-keeps-cpu-base")
        elif common_nonbinding and pdetails is not None and pdetails[0] == 1:
            print(
                "cross[safe-ci-dag-runner]: SKIP strict nonbinding max-mem proof: "
                "host exposes only one online CPU"
            )
            rep.ok("memory:nonbinding-max-mem-one-cpu-capability")
        else:
            rep.bad(
                "memory:nonbinding-max-mem-keeps-cpu-base",
                f"expected memory ceiling >1 (or one-CPU capability) but final S=1; "
                f"py={pnon} details={pdetails} rs={rnon} details={rdetails}",
            )

        # Six-exabyte caps overflow an i64 when two siblings co-run. The modeled peak must
        # saturate to the i64::MAX unbounded sentinel, which fits no finite budget (including MAX),
        # so sizing safely falls back to one finite-width step. Wrapping could admit both.
        huge = 6_000_000_000_000_000_000
        saturated = {
            "mem_cap_factor": 1.0,
            "mem_cap_floor_bytes": 0,
            "outer_mem_safety_factor": 1.0,
            "steps": [
                {
                    "group": "huge",
                    "job": f"s{index}",
                    "cmd": "true",
                    "jobs_flag": "",
                    "hint": {"hard_mem_max_bytes": huge},
                }
                for index in range(2)
            ],
        }
        sizing_case(
            tmp,
            "saturating-sum",
            saturated,
            str(i64_max),
            2,
            (1, huge, i64_max),
        )

        marker_step = {
            "group": "memory",
            "job": "probe",
            "cmd": 'printf spawned > "$MEMORY_MARKER"',
            "jobs_flag": "",
        }
        refusal_case(
            tmp,
            "hard-cap-only-infeasible",
            {
                **marker_step,
                "hint": {"hard_mem_max_bytes": 3 * gib},
            },
            budget="2G",
            budget_bytes=2 * gib,
            footprint=3 * gib,
        )
        refusal_case(
            tmp,
            "default-cap-infeasible",
            marker_step,
            budget="512M",
            budget_bytes=gib // 2,
            footprint=gib,
        )
        refusal_case(
            tmp,
            "engine-only-infeasible",
            {
                **marker_step,
                "engine_only": True,
                "hint": {"hard_mem_max_bytes": 3 * gib},
            },
            budget="2G",
            budget_bytes=2 * gib,
            footprint=3 * gib,
        )
        refusal_case(
            tmp,
            "unbounded-sentinel-infeasible",
            {
                **marker_step,
                "hint": {"hard_mem_max_bytes": i64_max},
            },
            budget=str(i64_max),
            budget_bytes=i64_max,
            footprint=i64_max,
        )

        store = os.path.join(tmp, "store")
        os.makedirs(store, exist_ok=True)
        csv_name = f"step_profiles_{_CPA_MACHINE}_{_CPA_CONTAINER}.csv"
        with open(os.path.join(store, csv_name), "w", encoding="utf-8") as handle:
            handle.write(
                _CPA_HEADER
                + _cpa_row(
                    _CPA_MACHINE,
                    _CPA_CONTAINER,
                    "learn.heavy",
                    1,
                    "10.0",
                    "10.0",
                    str(5 * gib),
                )
            )
        learned_dag = os.path.join(tmp, "learned-rss.json")
        learned_marker = {
            engine: os.path.join(tmp, f"learned-{engine}-spawned")
            for engine in ("py", "rs")
        }
        with open(learned_dag, "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "mem_cap_factor": 1.0,
                    "mem_cap_floor_bytes": 0,
                    "outer_mem_safety_factor": 1.0,
                    "steps": [
                        {
                            "group": "learn",
                            "job": "heavy",
                            "cmd": 'printf spawned > "$MEMORY_MARKER"',
                            "hint": {
                                "est_duration_s": 10.0,
                                "classification": "cpu-bound",
                                "preferred_inner_jobs": 1,
                            },
                        }
                    ],
                },
                handle,
            )
        extra = {
            "SAFE_CI_DAG_RUNNER_MACHINE_ID": _CPA_MACHINE,
            "SAFE_CI_DAG_RUNNER_CONTAINER_CLASS": _CPA_CONTAINER,
        }
        plan_args = (
            "plan", "--dag", learned_dag, "--perf-dir", store, "--planner", "cpa",
            "--max-mem", "4G", "--format", "json",
        )
        pplan, rplan = run(py, plan_args, extra), run(rs, plan_args, extra)
        learned_ok = False
        if pplan.returncode == rplan.returncode == 0 and pplan.stdout == rplan.stdout:
            try:
                obj = json.loads(pplan.stdout)
                step = obj["steps"][0]
                allocation = obj["allocation"]
                learned_ok = (
                    step["rss_estimate_bytes"] == 5 * gib
                    and step["rss_source"] == "store"
                    and step["alloc_inner_jobs"] is None
                    and allocation["stop_reason"] == "infeasible-memory"
                    and allocation["modeled_makespan_s"] == "inf"
                )
            except (json.JSONDecodeError, KeyError, IndexError, TypeError):
                learned_ok = False
        if learned_ok:
            rep.ok("memory:cpa-learned-rss-infeasible-plan")
        else:
            rep.bad(
                "memory:cpa-learned-rss-infeasible-plan",
                f"learned RSS did not produce one byte-identical infeasible plan\n"
                f"py={pplan}\nrs={rplan}",
            )

        run_args = (
            "run", "--dag", learned_dag, "--perf-dir", store, "--planner", "cpa",
            "--max-mem", "4G", "--max-cpus", "8", "--unsafe-no-cgroups", "-q", NOPROF,
        )
        outcomes = {
            engine: run(
                command,
                run_args,
                {**extra, "MEMORY_MARKER": learned_marker[engine]},
            )
            for engine, command in (("py", py), ("rs", rs))
        }
        phrase = "CPA allocation is infeasible under --max-mem 4G"
        if (
            all(outcome.returncode == 2 for outcome in outcomes.values())
            and all(phrase in outcome.stderr for outcome in outcomes.values())
            and not any(os.path.exists(path) for path in learned_marker.values())
        ):
            rep.ok("memory:cpa-learned-rss-run-refusal")
        else:
            rep.bad(
                "memory:cpa-learned-rss-run-refusal",
                f"expected learned-RSS CPA refusal before spawn; "
                f"outcomes={outcomes} markers={learned_marker}",
            )

        # Stress performs an authored preflight before tag expansion, then applies learned RSS to
        # the expanded tags and runs one final no-spawn footprint guard over that planned graph.
        # The final guard must neither miss the `#N` profile rows nor multiply the expanded graph
        # by N a second time.
        stress_store = os.path.join(tmp, "stress-store")
        os.makedirs(stress_store, exist_ok=True)
        stress_csv = f"step_profiles_{_CPA_MACHINE}_{_CPA_CONTAINER}.csv"
        learned_rss = 4_000_000_000_000_000_000
        with open(os.path.join(stress_store, stress_csv), "w", encoding="utf-8") as handle:
            handle.write(
                _CPA_HEADER
                + "".join(
                    _cpa_row(
                        _CPA_MACHINE,
                        _CPA_CONTAINER,
                        f"stress.seeded#{copy}",
                        1,
                        "1.0",
                        "0.1",
                        str(learned_rss),
                    )
                    for copy in (1, 2)
                )
            )
        stress_dag = os.path.join(tmp, "seeded-profile-stress.json")
        stress_marker = {
            engine: os.path.join(tmp, f"stress-profile-{engine}-spawned")
            for engine in ("py", "rs")
        }
        with open(stress_dag, "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "mem_cap_factor": 1.0,
                    "mem_cap_floor_bytes": 0,
                    "outer_mem_safety_factor": 1.0,
                    "steps": [
                        {
                            "group": "stress",
                            "job": "seeded",
                            "cmd": 'printf spawned > "$STRESS_MEMORY_MARKER"',
                            "jobs_flag": "",
                            "hint": {
                                "est_duration_s": 1.0,
                                "rss_baseline_bytes": 1,
                                "classification": "light",
                                "preferred_inner_jobs": 1,
                            },
                        }
                    ],
                },
                handle,
            )
        stress_args = (
            "run", "--dag", stress_dag, "--stress", "2", "--max-cpus", "2",
            "--perf-dir", stress_store, "--unsafe-no-cgroups", "-q", NOPROF,
        )
        stress_outcomes = {
            engine: run(
                command,
                stress_args,
                {**extra, "STRESS_MEMORY_MARKER": stress_marker[engine]},
            )
            for engine, command in (("py", py), ("rs", rs))
        }
        if (
            all(outcome.returncode == 2 for outcome in stress_outcomes.values())
            and all("--stress 2: OK" in outcome.stderr for outcome in stress_outcomes.values())
            and all(
                "final planned expanded graph needs" in outcome.stderr
                and "exceeding the box memory budget" in outcome.stderr
                and "unbounded or overflowed" not in outcome.stderr
                for outcome in stress_outcomes.values()
            )
            and not any(os.path.exists(path) for path in stress_marker.values())
        ):
            rep.ok("memory:stress-profile-expanded-final-refusal")
        else:
            rep.bad(
                "memory:stress-profile-expanded-final-refusal",
                f"expected authored preflight success then finite planned-graph refusal; "
                f"outcomes={stress_outcomes} markers={stress_marker}",
            )


def compare_sweep_errors(py: list[str], rs: list[str], rep: Report) -> None:
    """``sweep --jobs`` malformed inputs must exit 2 with byte-identical stderr in both builds
    (guards the not-an-integer / malformed-range error-text parity)."""
    with tempfile.TemporaryDirectory() as tmp:
        path = os.path.join(tmp, "dag.json")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write('{"steps": [{"group": "g", "job": "j", "cmd": "true"}]}')
        bad_jobs = ("abc", "3..1", "0..2", "1..x")
        for spec in bad_jobs:
            po = run(py, ("sweep", "--dag", path, "--step", "g.j", "--jobs", spec, ACF))
            ro = run(rs, ("sweep", "--dag", path, "--step", "g.j", "--jobs", spec, ACF))
            label = f"sweep-jobs-error:{spec}"
            if po.returncode != ro.returncode:
                rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}")
            elif po.returncode != 2:
                rep.bad(label, f"expected exit 2 for --jobs {spec!r}; got {po.returncode}")
            elif po.stderr != ro.stderr:
                rep.bad(
                    label,
                    f"stderr differs\n--- py ---\n{po.stderr}\n--- rs ---\n{ro.stderr}",
                )
            else:
                rep.ok(label)

        fixed = os.path.join(tmp, "self-managed.json")
        marker = {name: os.path.join(tmp, f"sweep-{name}-spawned") for name in ("py", "rs")}
        with open(fixed, "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "steps": [
                        {
                            "group": "g",
                            "job": "fixed",
                            "cmd": 'printf spawned > "$SELF_MANAGED_MARKER"',
                            "jobs_flag": "",
                        }
                    ]
                },
                handle,
            )
        args = (
            "sweep", "--dag", fixed, "--step", "g.fixed", "--jobs", "1..2", NOPROF,
        )
        po = run(py, args, {"SELF_MANAGED_MARKER": marker["py"]})
        ro = run(rs, args, {"SELF_MANAGED_MARKER": marker["rs"]})
        phrase = "empty effective jobs_flag"
        if (
            po.returncode == ro.returncode == 2
            and phrase in po.stderr
            and phrase in ro.stderr
            and not any(os.path.exists(path) for path in marker.values())
        ):
            rep.ok("sweep-jobs-error:self-managed-width")
        else:
            rep.bad(
                "sweep-jobs-error:self-managed-width",
                f"expected prompt refusal without spawn; py={po}\nrs={ro}\nmarkers={marker}",
            )


def _sweep_widths(text: str) -> set[int]:
    """Return widths from well-formed six-column sweep result rows."""

    widths: set[int] = set()
    for line in text.splitlines():
        fields = line.split()
        if len(fields) != 6 or not fields[0].isdigit():
            continue
        try:
            float(fields[1])
            float(fields[2])
            float(fields[3])
            if fields[4] != "-":
                int(fields[4])
            float(fields[5].removesuffix("x"))
        except ValueError:
            continue
        widths.add(int(fields[0]))
    return widths


def compare_sweep_success(py: list[str], rs: list[str], rep: Report) -> None:
    """Exercise a sweep and prove each table width is the width the guest actually received."""

    with tempfile.TemporaryDirectory(prefix="sweep-success-cross-") as tmp:
        observed = os.path.join(tmp, "observed-widths")
        guest = os.path.join(tmp, "record-width.sh")
        with open(guest, "w", encoding="utf-8") as handle:
            handle.write('#!/bin/sh\nprintf "%s\\n" "$1" >> "$OBSERVED_WIDTHS"\n')
        os.chmod(guest, 0o700)
        path = os.path.join(tmp, "dag.json")
        with open(path, "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "steps": [
                        {
                            "group": "g",
                            "job": "j",
                            "cmd": shlex.quote(guest),
                            "jobs_flag": "--workers=",
                            "env": {"OBSERVED_WIDTHS": observed},
                        }
                    ]
                },
                handle,
            )
        args = ("sweep", "--dag", path, "--step", "g.j", "--jobs", "1..2", NOPROF, ACF)
        po = run(py, args)
        try:
            with open(observed, encoding="utf-8") as handle:
                py_observed = handle.read().splitlines()
        except OSError:
            py_observed = []
        try:
            os.unlink(observed)
        except FileNotFoundError:
            pass
        ro = run(rs, args)
        try:
            with open(observed, encoding="utf-8") as handle:
                rs_observed = handle.read().splitlines()
        except OSError:
            rs_observed = []
        header = "jobs  wall_s  user_s  sys_s  rss_hwm  speedup(vs j1)"
        if (
            po.returncode == ro.returncode == 0
            and header in po.stdout
            and header in ro.stdout
            and _sweep_widths(po.stdout) == _sweep_widths(ro.stdout) == {1, 2}
            and py_observed == rs_observed == ["--workers=1", "--workers=2"]
        ):
            rep.ok("sweep:successful-table-and-guest-width")
        else:
            rep.bad(
                "sweep:successful-table-and-guest-width",
                f"py={po.returncode}:{po.stdout!r}:{po.stderr!r}\n"
                f"rs={ro.returncode}:{ro.stdout!r}:{ro.stderr!r}\n"
                f"observed py={py_observed!r} rs={rs_observed!r}",
            )


def compare_spawn_failure(py: list[str], rs: list[str], rep: Report) -> None:
    """Invalid spawn input must fail promptly and eager-cancel an in-flight sibling."""

    with tempfile.TemporaryDirectory(prefix="spawn-failure-cross-") as tmp:
        path = os.path.join(tmp, "dag.json")
        with open(path, "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "steps": [
                        {
                            "group": "g",
                            "job": "slow",
                            "cmd": "sleep 10",
                            "hint": {"est_duration_s": 100.0},
                        },
                        {
                            "group": "g",
                            "job": "gate",
                            "cmd": "sleep 0.3",
                            "hint": {"est_duration_s": 50.0},
                        },
                        {
                            "group": "g",
                            "job": "spawn",
                            "cmd": "true",
                            "deps": ["g.gate"],
                            "env": {"BAD": "embedded\0nul"},
                        }
                    ]
                },
                handle,
            )
        args = (
            "run",
            "--dag",
            path,
            "-q",
            "-s2",
            "-j2",
            NOPROF,
            NOFB,
            "--unsafe-no-cgroups",
        )
        po = run(py, args, timeout_s=8.0)
        ro = run(rs, args, timeout_s=8.0)
        py_text = po.stdout + po.stderr
        rs_text = ro.stdout + ro.stderr
        if (
            po.returncode == ro.returncode == 1
            and "spawn failed" in py_text
            and "spawn failed" in rs_text
            and "ABORT" in py_text
            and "ABORT" in rs_text
            and "Traceback" not in py_text
            and "Exception in thread" not in py_text
        ):
            rep.ok("run:spawn-failure-returns")
        else:
            rep.bad(
                "run:spawn-failure-returns",
                f"py={po.returncode}:{po.stdout!r}:{po.stderr!r}\n"
                f"rs={ro.returncode}:{ro.stdout!r}:{ro.stderr!r}",
            )


def _summary_sync_case(
    py: list[str],
    rs: list[str],
    rep: Report,
    *,
    label: str,
    machine: str,
    container: str,
    csv: str,
    dag: dict[str, object],
) -> None:
    """One profile-SUMMARY parity case against a fixed synthetic store + DAG. Asserts, across the
    Python and Rust builds:

    * ``summary build`` (store CSV -> summary JSON) is BYTE-IDENTICAL — the serialization correctness
      core (float-as-fixed-string, canonical bucket + reservoir order, FNV-1a subsample order).
    * ``summary plan`` FROM that summary equals ``plan --perf-dir`` from the raw store in EACH build
      (the summary recomputes the same estimates as the direct reader) AND is byte-identical across
      builds.
    * ``summary merge`` is byte-identical AND COMMUTATIVE (merge(a,b) == merge(b,a)), across builds —
      the mergeable-summary property that makes concurrent contributions order-independent."""
    extra = {
        "SAFE_CI_DAG_RUNNER_MACHINE_ID": machine,
        "SAFE_CI_DAG_RUNNER_CONTAINER_CLASS": container,
    }
    with tempfile.TemporaryDirectory() as tmp:
        store = os.path.join(tmp, "store")
        os.makedirs(store, exist_ok=True)
        with open(os.path.join(store, f"step_profiles_{machine}_{container}.csv"), "w") as fh:
            fh.write(csv)
        dag_path = os.path.join(tmp, "dag.json")
        with open(dag_path, "w") as fh:
            fh.write(json.dumps(dag))

        # 1) summary build: byte-identical.
        build = ("summary", "build", "--perf-dir", store)
        pb, rb = run(py, build, extra), run(rs, build, extra)
        if pb.returncode != 0 or rb.returncode != 0:
            rep.bad(f"summary-build:{label}", f"exit py={pb.returncode} rs={rb.returncode} "
                                               f"(py {pb.stderr!r}; rs {rb.stderr!r})")
            return
        if pb.stdout != rb.stdout:
            rep.bad(f"summary-build:{label}",
                    f"summary JSON not byte-identical\n--- py ---\n{pb.stdout}\n--- rs ---\n{rb.stdout}")
            return
        rep.ok(f"summary-build:{label}")

        fallback_extra = dict(extra)
        fallback_extra["SAFE_CI_DAG_RUNNER_PROFILE_DIR"] = store
        empty_assignments = ("summary", "build", "--perf-dir=", "--out=")
        empty_py = run(py, empty_assignments, fallback_extra)
        empty_rs = run(rs, empty_assignments, fallback_extra)
        if (
            empty_py.returncode == empty_rs.returncode == 0
            and empty_py.stdout == empty_rs.stdout == pb.stdout
            and not empty_py.stderr
            and not empty_rs.stderr
        ):
            rep.ok(f"summary-build-empty-assignment:{label}")
        else:
            rep.bad(
                f"summary-build-empty-assignment:{label}",
                f"py={empty_py.returncode}:{empty_py.stdout!r}:{empty_py.stderr!r}\n"
                f"rs={empty_rs.returncode}:{empty_rs.stdout!r}:{empty_rs.stderr!r}",
            )

        s_path = os.path.join(tmp, "summary.json")
        with open(s_path, "w") as fh:
            fh.write(pb.stdout)

        # A single input is a valid identity merge in both editions. Keep this aligned with the
        # `FILE [FILE ...]` action schema and its "one or more" help text.
        single_args = ("summary", "merge", "--", s_path)
        single_py, single_rs = run(py, single_args), run(rs, single_args)
        if (
            single_py.returncode == single_rs.returncode == 0
            and single_py.stdout == single_rs.stdout == pb.stdout
        ):
            rep.ok(f"summary-merge-single-delimited:{label}")
        else:
            rep.bad(
                f"summary-merge-single-delimited:{label}",
                f"py={single_py.returncode}:{single_py.stdout!r}:{single_py.stderr!r}\n"
                f"rs={single_rs.returncode}:{single_rs.stdout!r}:{single_rs.stderr!r}",
            )

        # 2) summary plan (from the summary) == plan --perf-dir (from the store), per build + cross.
        for fmt in ("text", "json"):
            sp = ("summary", "plan", "--dag", dag_path, "--summary", s_path, "--format", fmt)
            pp = ("plan", "--dag", dag_path, "--perf-dir", store, "--format", fmt)
            psp, rsp = run(py, sp, extra), run(rs, sp, extra)
            ppp = run(py, pp, extra)
            if psp.stdout != ppp.stdout:
                rep.bad(f"summary-plan-eq-store:{label}/{fmt}",
                        f"summary plan != store plan (py)\n--- summary ---\n{psp.stdout}\n"
                        f"--- store ---\n{ppp.stdout}")
            else:
                rep.ok(f"summary-plan-eq-store:{label}/{fmt}")
            if psp.stdout != rsp.stdout:
                rep.bad(f"summary-plan:{label}/{fmt}",
                        f"summary plan not byte-identical\n--- py ---\n{psp.stdout}\n--- rs ---\n{rsp.stdout}")
            else:
                rep.ok(f"summary-plan:{label}/{fmt}")

        # 3) summary merge: byte-identical + commutative, across builds. Split the store into two
        # halves so the merge actually combines distinct contributions.
        lines = csv.splitlines(keepends=True)
        header, body = lines[0], lines[1:]
        mid = max(1, len(body) // 2)
        a_csv, b_csv = header + "".join(body[:mid]), header + "".join(body[mid:])
        a_path, b_path = os.path.join(tmp, "a.json"), os.path.join(tmp, "b.json")
        for half, path in ((a_csv, a_path), (b_csv, b_path)):
            half_store = os.path.join(tmp, "half")
            os.makedirs(half_store, exist_ok=True)
            with open(os.path.join(half_store, f"step_profiles_{machine}_{container}.csv"), "w") as fh:
                fh.write(half)
            out = run(py, ("summary", "build", "--perf-dir", half_store), extra)
            with open(path, "w") as fh:
                fh.write(out.stdout)
        for order, name in (((a_path, b_path), "ab"), ((b_path, a_path), "ba")):
            args = ("summary", "merge", *order)
            pm, rm = run(py, args), run(rs, args)
            if pm.stdout != rm.stdout:
                rep.bad(f"summary-merge:{label}/{name}",
                        f"merge not byte-identical\n--- py ---\n{pm.stdout}\n--- rs ---\n{rm.stdout}")
            else:
                rep.ok(f"summary-merge:{label}/{name}")
        # Commutativity: merge(a,b) == merge(b,a) in the python build (already cross-checked identical).
        m_ab = run(py, ("summary", "merge", a_path, b_path)).stdout
        m_ba = run(py, ("summary", "merge", b_path, a_path)).stdout
        if m_ab == m_ba:
            rep.ok(f"summary-merge-commutative:{label}")
        else:
            rep.bad(f"summary-merge-commutative:{label}",
                    f"merge(a,b) != merge(b,a)\n--- ab ---\n{m_ab}\n--- ba ---\n{m_ba}")


def compare_summary_sync(py: list[str], rs: list[str], rep: Report) -> None:
    """Prove the mergeable profile SUMMARY (the profile-artifact sync feature's correctness core) is
    byte-identical py<->rs for serialization, merge, and recomputed plan — on BOTH the feedback store
    (contention discount) and the multi-width speedup store (speedup curves)."""
    _summary_sync_case(
        py, rs, rep, label="feedback", machine=SYNTH_MACHINE, container=SYNTH_CONTAINER,
        csv=_FEEDBACK_STORE_CSV, dag=_FEEDBACK_DAG,
    )
    _summary_sync_case(
        py, rs, rep, label="speedup", machine=_SPEEDUP_MACHINE, container=_SPEEDUP_CONTAINER,
        csv=_speedup_store_csv(), dag=_SPEEDUP_DAG,
    )


INVOCATIONS: tuple[Invocation, ...] = (
    Invocation("version", ("--version",)),
    Invocation("help", ("--help",)),
    Invocation("noargs", ()),
    # Each installed edition carries a standalone generated guide. Content isolation is checked
    # separately; language-specific install/API fragments intentionally differ.
    Invocation("userguide", ("--userguide",)),
    # `capabilities` prints each engine's machine-readable enforcement manifest (which guards it
    # actually implements: cpu_affinity, cpu_bandwidth, cpu_timeout, memory_max, oom_detection,
    # pids_guard, run_timeout, wall_timeout, and write_domains). The
    # WHOLE POINT is that the two engines must enforce the SAME set, so the manifests are asserted
    # byte-identical. This is the recurrence guard for the historical gap where the Rust runner
    # silently did NOT enforce `cpu_timeout` while the Python runner did: with this check, that
    # divergence is a `cross` failure in ANY environment (it needs no cgroup boxing, so it fires on
    # ubuntu-latest CI where the behavioral boxed tests can only loud-skip). The boxed smoke tests
    # in each build (boxing_smoke / cpu_timeout_smoke, and the Python pytest equivalents) anchor
    # these declarations to real behavior wherever a cgroup-v2 + systemd --user scope exists.
    Invocation("capabilities", ("capabilities",)),
)

#: Invocations whose stdout must be BYTE-IDENTICAL across builds (not just exit-code equal).
_BYTE_IDENTICAL_INVOCATIONS = frozenset({"version", "capabilities"})


def compare_invocations(py: list[str], rs: list[str], rep: Report) -> None:
    for inv in INVOCATIONS:
        po = run(py, inv.args)
        ro = run(rs, inv.args)
        if po.returncode != ro.returncode:
            rep.bad(f"invocation/{inv.name}", f"exit py={po.returncode} rs={ro.returncode}")
        elif inv.name in _BYTE_IDENTICAL_INVOCATIONS and po.stdout != ro.stdout:
            rep.bad(
                f"invocation/{inv.name}",
                f"stdout not byte-identical\n--- py ---\n{po.stdout}\n--- rs ---\n{ro.stdout}",
            )
        else:
            rep.ok(f"invocation/{inv.name}")


def compare_cli_schema(py: list[str], rs: list[str], rep: Report) -> None:
    """Require both CLIs to expose the complete supported command/flag inventory.

    Exit-code-only help checks historically allowed one implementation to omit whole features.
    This semantic schema check is deliberately explicit: a one-sided command or flag is a parity
    failure even when human-facing wrapping/wording differs.
    """
    command_flags: dict[str, tuple[str, ...]] = {
        "run": (
            "--dag", "--max-steps", "--max-cpus", "--cores", "--cpuset", "--pin",
            "--max-mem",
            "--only",
            "--args", "--stress", "--perf-dir", "--no-profile", "--profile", "--planner",
            "--show-plan", "--no-profile-feedback", "--profile-sync",
            "--profile-sync-direction", "--keep-going", "--run-timeout",
            "--allow-cgroup-failure",
            "--unsafe-no-cgroups", "--small-default-cap", "--quiet",
        ),
        "sweep": (
            "--dag", "--step", "--jobs", "--repeat", "--perf-dir", "--no-profile",
            "--allow-cgroup-failure", "--unsafe-no-cgroups",
        ),
        "plan": ("--dag", "--planner", "--max-mem", "--format", "--perf-dir", "--no-profile-feedback"),
        "pin-run": ("--cores", "--tag"),
    }
    command_forbidden_flags: dict[str, tuple[str, ...]] = {
        # Retained as a hidden 0.13 compatibility alias, not as public run vocabulary.
        "run": ("--jobs",),
    }
    summary_action_contracts: dict[str, tuple[tuple[str, ...], tuple[str, ...]]] = {
        "build": (
            ("--perf-dir", "--out", "--reservoir-cap"),
            ("--summary", "--dag", "--planner", "--max-mem", "--format"),
        ),
        "merge": (
            ("FILE", "--out", "--reservoir-cap"),
            ("--perf-dir", "--summary", "--dag", "--planner", "--max-mem", "--format"),
        ),
        "plan": (
            ("--summary", "--dag", "--planner", "--max-mem", "--format"),
            ("--perf-dir", "--out", "--reservoir-cap"),
        ),
        "stats": (
            ("FILE",),
            ("--perf-dir", "--out", "--reservoir-cap", "--summary", "--dag", "--planner"),
        ),
    }
    summary_top_phrases = (
        "build a plan from a summary json and dag",
        "merge one or more summary json files",
    )
    top_commands = (
        "run", "sweep", "plan", "list", "ascii", "dot", "json", "yaml", "summary",
        "pin-run", "quickstart", "capabilities",
    )
    for engine, command in (("py", py), ("rs", rs)):
        top = run(command, ("--help",))
        missing_commands = [name for name in top_commands if name not in top.stdout]
        if top.returncode != 0 or missing_commands:
            rep.bad(
                f"cli-schema:{engine}/commands",
                f"exit={top.returncode}; missing commands={missing_commands}\n{top.stdout}",
            )
        else:
            rep.ok(f"cli-schema:{engine}/commands")
        summary_top = run(command, ("summary", "--help"))
        normalized_summary_top = " ".join(summary_top.stdout.lower().split())
        missing_summary_phrases = [
            phrase for phrase in summary_top_phrases if phrase not in normalized_summary_top
        ]
        stale_backend_claim = "plan a summary sync from a backend spec" in normalized_summary_top
        if summary_top.returncode != 0 or missing_summary_phrases or stale_backend_claim:
            rep.bad(
                f"cli-schema:{engine}/summary-top",
                f"exit={summary_top.returncode}; missing={missing_summary_phrases}; "
                f"stale backend claim={stale_backend_claim}\n{summary_top.stdout}",
            )
        else:
            rep.ok(f"cli-schema:{engine}/summary-top")
        for subcommand, flags in command_flags.items():
            outcome = run(command, (subcommand, "--help"))
            missing = [flag for flag in flags if flag not in outcome.stdout]
            unexpected = [
                flag for flag in command_forbidden_flags.get(subcommand, ())
                if flag in outcome.stdout
            ]
            if outcome.returncode != 0 or missing or unexpected:
                rep.bad(
                    f"cli-schema:{engine}/{subcommand}",
                    f"exit={outcome.returncode}; missing flags={missing}; "
                    f"unexpected flags={unexpected}\n{outcome.stdout}",
                )
            else:
                rep.ok(f"cli-schema:{engine}/{subcommand}")
        for action, (required, forbidden) in summary_action_contracts.items():
            outcome = run(command, ("summary", action, "--help"))
            missing = [flag for flag in required if flag not in outcome.stdout]
            unexpected = [flag for flag in forbidden if flag in outcome.stdout]
            action_usage = f"summary {action}"
            generic = "summary <action>" in outcome.stdout
            if (
                outcome.returncode != 0
                or missing
                or unexpected
                or action_usage not in outcome.stdout
                or generic
            ):
                rep.bad(
                    f"cli-schema:{engine}/summary-{action}",
                    f"exit={outcome.returncode}; missing flags={missing}; "
                    f"unexpected flags={unexpected}; "
                    f"action usage present={action_usage in outcome.stdout}; "
                    f"generic={generic}\n{outcome.stdout}",
                )
            else:
                rep.ok(f"cli-schema:{engine}/summary-{action}")
    summary_invalid_invocations: tuple[tuple[str, tuple[str, ...]], ...] = (
        ("build-unexpected-positional", ("summary", "build", "unexpected")),
        (
            "plan-unexpected-positional",
            (
                "summary",
                "plan",
                "unexpected",
                "--summary",
                "missing-summary.json",
                "--dag",
                "missing-dag.json",
            ),
        ),
        ("stats-extra-file", ("summary", "stats", "one.json", "two.json")),
        ("merge-missing-file", ("summary", "merge")),
        ("build-cap-malformed", ("summary", "build", "--reservoir-cap", "nope")),
        ("build-cap-underscore", ("summary", "build", "--reservoir-cap", "1_0")),
        ("build-cap-leading-space", ("summary", "build", "--reservoir-cap", " 10")),
        ("build-cap-trailing-space", ("summary", "build", "--reservoir-cap", "10 ")),
        ("build-cap-unicode-digits", ("summary", "build", "--reservoir-cap", "١٠")),
        ("build-cap-zero", ("summary", "build", "--reservoir-cap", "0")),
        (
            "build-cap-out-of-i64-range",
            ("summary", "build", "--reservoir-cap", "9223372036854775808"),
        ),
        (
            "merge-cap-negative",
            ("summary", "merge", "missing.json", "--reservoir-cap", "-1"),
        ),
        (
            "plan-bad-planner",
            (
                "summary",
                "plan",
                "--summary",
                "missing-summary.json",
                "--dag",
                "missing-dag.json",
                "--planner",
                "unknown",
            ),
        ),
        (
            "plan-bad-format",
            (
                "summary",
                "plan",
                "--summary",
                "missing-summary.json",
                "--dag",
                "missing-dag.json",
                "--format",
                "xml",
            ),
        ),
        ("build-missing-value", ("summary", "build", "--perf-dir")),
        (
            "build-delimited-positional",
            ("summary", "build", "--", "--looks-like-an-option"),
        ),
        (
            "plan-missing-value-before-flag",
            ("summary", "plan", "--summary", "--dag", "missing-dag.json"),
        ),
    )
    for label, args in summary_invalid_invocations:
        po, ro = run(py, args), run(rs, args)
        if (
            po.returncode == ro.returncode == 2
            and not po.stdout
            and not ro.stdout
            and po.stderr
            and ro.stderr
        ):
            rep.ok(f"cli-schema:summary-invalid/{label}")
        else:
            rep.bad(f"cli-schema:summary-invalid/{label}", f"py={po}\nrs={ro}")
    po = run(py, ("run", "--da", "missing.json"))
    ro = run(rs, ("run", "--da", "missing.json"))
    if po.returncode == ro.returncode == 2:
        rep.ok("cli-schema:abbreviation-refused")
    else:
        rep.bad("cli-schema:abbreviation-refused", f"py={po.returncode}; rs={ro.returncode}")


def compare_args_stress(py: list[str], rs: list[str], rep: Report) -> None:
    """Cross-check the previously Python-only passthrough and stress surfaces."""
    with tempfile.TemporaryDirectory(prefix="safe-ci-cross-args-stress-") as td:
        passthrough_path = os.path.join(td, "passthrough.json")
        with open(passthrough_path, "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "steps": [
                        {
                            "group": "test",
                            "job": "unit",
                            "cmd": 'test "{args}" = "--case xyz"',
                            "hint": {"hard_mem_max_bytes": 1},
                        }
                    ]
                },
                handle,
            )
        passthrough = (
            "run", "--dag", passthrough_path, "--args=--case xyz", "--unsafe-no-cgroups",
            "--no-profile", "-q",
        )
        po, ro = run(py, passthrough), run(rs, passthrough)
        if po.returncode == ro.returncode == 0:
            rep.ok("args:substitution")
        else:
            rep.bad(
                "args:substitution",
                f"expected both 0; py={po.returncode} rs={ro.returncode}\npy:{po.stderr}\nrs:{ro.stderr}",
            )

        invalid_path = os.path.join(td, "no-placeholder.json")
        with open(invalid_path, "w", encoding="utf-8") as handle:
            json.dump(
                {
                    "steps": [
                        {
                            "group": "test",
                            "job": "unit",
                            "cmd": "true",
                            "hint": {"hard_mem_max_bytes": 1},
                        }
                    ]
                },
                handle,
            )
        invalid = (
            "run", "--dag", invalid_path, "--args=x", "--unsafe-no-cgroups", "--no-profile", "-q",
        )
        po, ro = run(py, invalid), run(rs, invalid)
        if po.returncode == ro.returncode == 2 and "no selected step declares" in po.stderr and "no selected step declares" in ro.stderr:
            rep.ok("args:undeclared-refused")
        else:
            rep.bad("args:undeclared-refused", f"py={po}\nrs={ro}")

        stress = (
            "run", "--dag", invalid_path, "--stress", "3", "--unsafe-no-cgroups",
            "--no-profile", "-q",
        )
        po, ro = run(py, stress), run(rs, stress)
        # Parallel completion lines and the observed peak overlap are intentionally
        # nondeterministic for three near-instant `true` commands. Compare the stable pass ratio
        # and configured ceilings while only requiring each observed peak to be in range. Exact
        # overlap is exercised by the barrier-backed scheduler cases elsewhere in this harness.
        def stress_report(stdout: str) -> tuple[str, str, int] | None:
            lines = stdout.splitlines()
            start = next(
                (index for index, line in enumerate(lines) if line.startswith("stress results (")),
                len(lines),
            )
            report = lines[start:]
            if len(report) != 3:
                return None
            match = re.fullmatch(
                r"  maximum concurrent steps: ([0-9]+) (\(--max-steps .+\))",
                report[2],
            )
            if match is None:
                return None
            return report[0] + "\n" + report[1], match.group(2), int(match.group(1))

        py_report, rs_report = stress_report(po.stdout), stress_report(ro.stdout)
        if (
            po.returncode == ro.returncode == 0
            and py_report is not None
            and rs_report is not None
            and py_report[:2] == rs_report[:2]
            and "3/3 passed" in py_report[0]
            and 1 <= py_report[2] <= 3
            and 1 <= rs_report[2] <= 3
        ):
            rep.ok("stress:three-pass-ratio")
        else:
            rep.bad(
                "stress:three-pass-ratio",
                f"py={po.returncode}:{py_report!r}\nrs={ro.returncode}:{rs_report!r}",
            )

        invalid_stress = (
            "run", "--dag", invalid_path, "--stress", "0", "--unsafe-no-cgroups", "--no-profile",
        )
        po, ro = run(py, invalid_stress), run(rs, invalid_stress)
        if po.returncode == ro.returncode == 2 and "must be >= 1" in po.stderr and "must be >= 1" in ro.stderr:
            rep.ok("stress:nonpositive-refused")
        else:
            rep.bad("stress:nonpositive-refused", f"py={po}\nrs={ro}")

        oversized_stress = (
            "run",
            "--dag",
            invalid_path,
            "--stress",
            "1000000",
            "--unsafe-no-cgroups",
            "--no-profile",
        )
        po, ro = run(py, oversized_stress), run(rs, oversized_stress)
        expansion_phrase = (
            "expansion would create 1000000 generated DAG nodes/control units, "
            "exceeding safety limit 100000"
        )
        if (
            po.returncode == ro.returncode == 2
            and expansion_phrase in po.stderr
            and expansion_phrase in ro.stderr
        ):
            rep.ok("stress:generated-node-cap-refuses-oversized-fanout")
        else:
            rep.bad("stress:generated-node-cap-refuses-oversized-fanout", f"py={po}\nrs={ro}")

        hard_cores = (
            "run",
            "--dag",
            invalid_path,
            "--cores",
            "1",
            "--unsafe-no-cgroups",
            "--no-profile",
            "-q",
        )
        ledger = {"SAFE_CI_CORE_LEDGER": os.path.join(td, "hard-refusal-ledger.json")}
        po, ro = run(py, hard_cores, ledger), run(rs, hard_cores, ledger)
        if (
            po.returncode == ro.returncode == 3
            and "hard cgroup cpuset unavailable; refusing to run" in po.stderr
            and "hard cgroup cpuset unavailable; refusing to run" in ro.stderr
        ):
            rep.ok("cores:unboxed-soft-affinity-refused")
        else:
            rep.bad("cores:unboxed-soft-affinity-refused", f"py={po}\nrs={ro}")


def _cpu_guest_dag(
    widths: Sequence[int],
    *,
    duration_s: float,
    hardcoded_workers: int | None = None,
    cgroup_parent_levels: int | None = None,
    barrier_participants: int | None = None,
) -> dict[str, object]:
    """One shared DAG whose output path is supplied per engine through the environment."""

    steps: list[object] = []
    for index, width in enumerate(widths):
        tag = f"footprint.s{index}"
        command = (
            f"{shlex.quote(sys.executable)} {shlex.quote(CPU_FOOTPRINT_GUEST)} "
            f"--output \"$CPU_FOOTPRINT_OUTPUT\" --step {shlex.quote(tag)} "
            f"--duration-s {duration_s:g} --sample-ms 10 --start-delay-ms 500"
        )
        jobs_flag: str | None = "--workers="
        if hardcoded_workers is not None:
            command += f" --workers={hardcoded_workers}"
            jobs_flag = ""
        if cgroup_parent_levels is not None:
            command += (
                f" --cgroup-parent-levels {cgroup_parent_levels} --cgroup-sample-ms 25"
            )
        if barrier_participants is not None:
            command += (
                " --barrier-file \"$CPU_FOOTPRINT_BARRIER\" "
                f"--barrier-participants {barrier_participants} --barrier-timeout-s 10"
            )
        steps.append(
            {
                "group": "footprint",
                "job": f"s{index}",
                "desc": f"CPU footprint width {width}",
                "cmd": command,
                "jobs_flag": jobs_flag,
                "timeout": 15,
                "cpu_timeout": 120,
                "hint": {
                    "preferred_inner_jobs": width,
                    "hard_mem_max_bytes": 536_870_912,
                    "est_duration_s": duration_s,
                },
            }
        )
    return {"steps": steps}


def _cpu_facts(log_path: str) -> tuple[FootprintStats, CpuFootprintFacts]:
    events = load_cpu_events([Path(log_path)])
    stats = analyze_cpu_footprint(events, bucket_ns=25_000_000)
    workers: dict[str, set[tuple[int, int]]] = {}
    for record in events:
        if record.get("event") != "worker_start":
            continue
        step, worker, pid = record.get("step"), record.get("worker"), record.get("pid")
        if not isinstance(step, str):
            continue
        if not isinstance(worker, int) or isinstance(worker, bool):
            continue
        if not isinstance(pid, int) or isinstance(pid, bool):
            continue
        workers.setdefault(step, set()).add((worker, pid))
    facts = CpuFootprintFacts(
        completed_steps=len(stats.step_intervals),
        workers_per_step=tuple(sorted(len(entries) for entries in workers.values())),
        max_live_steps=stats.max_live_steps,
        max_live_workers=stats.max_live_workers,
    )
    return stats, facts


def compare_run_parallel_limits(py: list[str], rs: list[str], rep: Report) -> None:
    """Cross-check active-step overlap, per-step width caps, and outer CPU bandwidth."""

    with tempfile.TemporaryDirectory(prefix="safe-ci-cross-cpu-limits-") as td:
        invalid_dag = os.path.join(td, "valid.json")
        Path(invalid_dag).write_text(
            '{"steps":[{"group":"g","job":"ok","cmd":"true"}]}', encoding="utf-8"
        )
        invalid_cases: tuple[tuple[str, tuple[str, ...]], ...] = (
            ("max-steps-spaced-zero", ("-s", "0")),
            ("max-steps-bare-zero", ("-s0",)),
            ("max-steps-invalid", ("--max-steps", "nope")),
            ("max-steps-i64-overflow", ("--max-steps", "9223372036854775808")),
            ("max-cpus-spaced-zero", ("-j", "0")),
            ("max-cpus-bare-zero", ("-j0",)),
            ("max-cpus-invalid", ("--max-cpus", "nope")),
            ("max-cpus-i64-overflow", ("--max-cpus", "9223372036854775808")),
        )
        for label, flags in invalid_cases:
            args = (
                "run", "--dag", invalid_dag, *flags, NOPROF, NOFB, "--unsafe-no-cgroups",
            )
            po, ro = run(py, args), run(rs, args)
            if po.returncode == ro.returncode == 2 and po.stderr and ro.stderr:
                rep.ok(f"run-limits:invalid/{label}")
            else:
                rep.bad(f"run-limits:invalid/{label}", f"py={po}\nrs={ro}")

        conflict_args = (
            "run", "--dag", invalid_dag, "--max-cpus", "2", "--jobs", "3",
            NOPROF, NOFB, "--unsafe-no-cgroups",
        )
        po, ro = run(py, conflict_args), run(rs, conflict_args)
        conflict_message = "--max-cpus and legacy --jobs disagree"
        if (
            po.returncode == ro.returncode == 2
            and conflict_message in po.stderr
            and conflict_message in ro.stderr
        ):
            rep.ok("run-limits:invalid/max-cpus-legacy-jobs-conflict")
        else:
            rep.bad(
                "run-limits:invalid/max-cpus-legacy-jobs-conflict",
                f"expected exit 2 with {conflict_message!r}; py={po}\nrs={ro}",
            )

        self_managed_dag = os.path.join(td, "self-managed.json")
        marker = {name: os.path.join(td, f"run-{name}-spawned") for name in ("py", "rs")}
        Path(self_managed_dag).write_text(
            json.dumps(
                {
                    "steps": [
                        {
                            "group": "g",
                            "job": "fixed",
                            "cmd": 'printf spawned > "$SELF_MANAGED_MARKER"',
                            "jobs_flag": "",
                            "hint": {"preferred_inner_jobs": 5},
                        }
                    ]
                }
            ),
            encoding="utf-8",
        )
        args = (
            "run", "--dag", self_managed_dag, "--max-cpus", "2", NOPROF, NOFB,
            "--unsafe-no-cgroups", "-q",
        )
        po = run(py, args, {"SELF_MANAGED_MARKER": marker["py"]})
        ro = run(rs, args, {"SELF_MANAGED_MARKER": marker["rs"]})
        phrase = "cannot lower guest parallelism"
        if (
            po.returncode == ro.returncode == 2
            and phrase in po.stderr
            and phrase in ro.stderr
            and not any(os.path.exists(path) for path in marker.values())
        ):
            rep.ok("run-limits:self-managed-over-budget-refused")
        else:
            rep.bad(
                "run-limits:self-managed-over-budget-refused",
                f"expected prompt refusal without spawn; py={po}\nrs={ro}\nmarkers={marker}",
            )

        whitespace_dag = os.path.join(td, "whitespace-jobs-flag.json")
        Path(whitespace_dag).write_text(
            json.dumps(
                {
                    "steps": [
                        {
                            "group": "g",
                            "job": "fixed",
                            "cmd": "sh -c 'test \"$#\" -eq 0' _",
                            "jobs_flag": "   ",
                            "hint": {"preferred_inner_jobs": 2},
                        }
                    ]
                }
            ),
            encoding="utf-8",
        )
        args = (
            "run", "--dag", whitespace_dag, "--max-cpus", "2", NOPROF, NOFB,
            "--unsafe-no-cgroups", "-q",
        )
        po, ro = run(py, args), run(rs, args)
        if po.returncode == ro.returncode == 0:
            rep.ok("run-limits:whitespace-jobs-flag-does-not-append-width")
        else:
            rep.bad(
                "run-limits:whitespace-jobs-flag-does-not-append-width",
                f"whitespace-only jobs_flag appended an argument; py={po}\nrs={ro}",
            )

        cases: tuple[
            tuple[str, tuple[int, ...], tuple[str, ...], CpuFootprintFacts], ...
        ] = (
            (
                "step-cap-two-cpu-budget-four",
                (2, 2, 2, 2),
                ("-s", "2", "-j", "4"),
                CpuFootprintFacts(4, (2, 2, 2, 2), 2, 4),
            ),
            (
                "bare-step-four-cpu-budget-two",
                (1, 1, 1, 1),
                ("-s4", "-j2"),
                CpuFootprintFacts(4, (1, 1, 1, 1), 4, 4),
            ),
            (
                "two-width-eight-steps-share-eight-core-budget",
                (8, 8),
                ("-s2", "-j8"),
                CpuFootprintFacts(2, (8, 8), 2, 16),
            ),
            (
                "authored-width-capped-to-j",
                (5,),
                ("--max-steps=4", "--max-cpus=2"),
                CpuFootprintFacts(1, (2,), 1, 2),
            ),
            (
                "legacy-hidden-jobs-alias",
                (1, 1, 1),
                ("--max-steps=3", "--jobs=2"),
                CpuFootprintFacts(3, (1, 1, 1), 3, 3),
            ),
            (
                "canonical-and-legacy-equal",
                (1, 1, 1),
                ("--max-steps=3", "--max-cpus=2", "--jobs=2"),
                CpuFootprintFacts(3, (1, 1, 1), 3, 3),
            ),
        )
        for label, widths, flags, expected in cases:
            dag_path = os.path.join(td, f"{label}.json")
            barrier_participants = expected.max_live_steps if expected.max_live_steps > 1 else None
            Path(dag_path).write_text(
                json.dumps(
                    _cpu_guest_dag(
                        widths,
                        duration_s=1.5 if expected.max_live_steps > 1 else 0.4,
                        barrier_participants=barrier_participants,
                    )
                ),
                encoding="utf-8",
            )
            logs = {name: os.path.join(td, f"{label}-{name}.jsonl") for name in ("py", "rs")}
            args = (
                "run", "--dag", dag_path, *flags, "-q", NOPROF, NOFB,
                "--unsafe-no-cgroups",
            )
            extra = {
                name: {
                    "CPU_FOOTPRINT_OUTPUT": path,
                    "SAFE_CI_DAG_RUNNER_NO_STEP_LOGS": "1",
                    "CPU_FOOTPRINT_BARRIER": os.path.join(td, f"{label}-{name}.barrier"),
                }
                for name, path in logs.items()
            }
            outcomes = {"py": run(py, args, extra["py"]), "rs": run(rs, args, extra["rs"])}
            if any(outcome.returncode != 0 for outcome in outcomes.values()):
                rep.bad(f"run-limits:{label}", f"py={outcomes['py']}\nrs={outcomes['rs']}")
                continue
            try:
                analyzed = {name: _cpu_facts(path) for name, path in logs.items()}
            except (OSError, ValueError) as exc:
                rep.bad(f"run-limits:{label}", f"could not analyze guest evidence: {exc}")
                continue
            facts = {name: pair[1] for name, pair in analyzed.items()}
            verdicts = {
                name: check_cpu_limits(
                    pair[0],
                    max_steps=expected.max_live_steps,
                    max_workers=expected.max_live_workers,
                )
                for name, pair in analyzed.items()
            }
            if facts["py"] != facts["rs"]:
                rep.bad(
                    f"run-limits:{label}",
                    f"normalized facts py={facts['py']} rs={facts['rs']}",
                )
            elif facts["py"] != expected:
                rep.bad(
                    f"run-limits:{label}",
                    f"expected {expected}, observed {facts['py']} "
                    "(CPU-ID union intentionally ignored)",
                )
            elif not all(verdict.ok for verdict in verdicts.values()):
                rep.bad(f"run-limits:{label}", f"limit verdicts={verdicts}")
            else:
                rep.ok(f"run-limits:{label}")


def _boxing_capability_unavailable(outcome: Outcome) -> bool:
    combined = outcome.stdout + outcome.stderr
    return outcome.returncode == 3 and (
        "systemd --user scope is unavailable" in combined
        or "scope setup was attempted and failed" in combined
        or ("cgroup boxing could not be established" in combined and "unavailable" in combined)
    )


def compare_boxed_cpu_bandwidth(py: list[str], rs: list[str], rep: Report) -> None:
    """Anchor ``-j`` / ``--max-cpus`` to live quota and aggregate CPU counters."""

    with tempfile.TemporaryDirectory(prefix="safe-ci-cross-boxed-cpu-") as td:
        dag_path = os.path.join(td, "dag.json")
        # Each runner-controlled step receives the full per-step ceiling J=8. Both steps must be
        # live together under -s2, so their sixteen requested workers exceed J in aggregate while
        # the parent cgroup's long-window CPU bandwidth remains bounded to eight core-equivalents.
        Path(dag_path).write_text(
            json.dumps(
                _cpu_guest_dag(
                    (8, 8),
                    duration_s=1.75,
                    cgroup_parent_levels=1,
                    barrier_participants=2,
                )
            ),
            encoding="utf-8",
        )
        logs = {name: os.path.join(td, f"boxed-{name}.jsonl") for name in ("py", "rs")}
        args = (
            "run", "--dag", dag_path, "-s2", "-j8", "-q", NOPROF, NOFB,
        )
        extra = {
            name: {
                "CPU_FOOTPRINT_OUTPUT": path,
                "CPU_FOOTPRINT_BARRIER": os.path.join(td, f"boxed-{name}.barrier"),
                "SAFE_CI_DAG_RUNNER_NO_STEP_LOGS": "1",
                "SAFE_CI_FORCE_SCOPE_ATTEMPT": "1",
            }
            for name, path in logs.items()
        }
        outcomes = {"py": run(py, args, extra["py"]), "rs": run(rs, args, extra["rs"])}
        unavailable = {name: _boxing_capability_unavailable(out) for name, out in outcomes.items()}
        if all(unavailable.values()):
            print(
                "cross[safe-ci-dag-runner]: SKIP boxed CPU-bandwidth differential: "
                "cgroup-v2 + a working systemd --user scope are unavailable"
            )
            rep.ok("boxed-cpu-bandwidth:capability-unavailable")
            return
        if any(unavailable.values()) or any(out.returncode != 0 for out in outcomes.values()):
            rep.bad("boxed-cpu-bandwidth", f"py={outcomes['py']}\nrs={outcomes['rs']}")
            return

        normalized: dict[str, tuple[bool, ...]] = {}
        details: dict[str, object] = {}
        try:
            for name, path in logs.items():
                events = load_cpu_events([Path(path)])
                stats = analyze_cpu_footprint(events, bucket_ns=100_000_000)
                facts = _cpu_facts(path)[1]
                limits = check_cpu_limits(stats, max_steps=2)
                bandwidth = check_cgroup_bandwidth(
                    events, min_window_periods=10, scheduler_slack_usec=100_000
                )
                normalized[name] = (
                    facts.completed_steps == 2,
                    facts.workers_per_step == (8, 8),
                    facts.max_live_steps == 2,
                    facts.max_live_workers == 16,
                    limits.ok,
                    bandwidth.checkable,
                    bandwidth.ok,
                    bandwidth.quota_cores == (8.0,),
                )
                details[name] = {
                    "facts": facts,
                    "quota_cores": bandwidth.quota_cores,
                    "checked_windows": bandwidth.checked_windows,
                    "bandwidth_violations": bandwidth.violations,
                    # sampled_cpu_union is intentionally absent: migration/CPU identity is not
                    # an aggregate CPU-bandwidth or simultaneous-worker invariant.
                }
        except (OSError, ValueError) as exc:
            rep.bad("boxed-cpu-bandwidth", f"could not analyze boxed guest evidence: {exc}")
            return
        expected = (True,) * 8
        if normalized.get("py") != normalized.get("rs"):
            rep.bad("boxed-cpu-bandwidth", f"normalized={normalized}; details={details}")
        elif normalized.get("py") != expected:
            rep.bad("boxed-cpu-bandwidth", f"invariants={normalized}; details={details}")
        else:
            rep.ok("boxed-cpu-bandwidth")


def compare_pin_run(py: list[str], rs: list[str], rep: Report) -> None:
    """Exercise shared reservation semantics through the paired safe-runner wrapper."""
    with tempfile.TemporaryDirectory(prefix="safe-ci-cross-pin-") as td:
        extra = {"SAFE_CI_CORE_LEDGER": os.path.join(td, "ledger.json")}
        missing = ("pin-run", "--cores", "1")
        po, ro = run(py, missing, extra), run(rs, missing, extra)
        if po.returncode == ro.returncode == 2:
            rep.ok("pin-run:missing-command")
        else:
            rep.bad("pin-run:missing-command", f"py={po}\nrs={ro}")
        invalid = ("pin-run", "--cores", "0", "--", "true")
        po, ro = run(py, invalid, extra), run(rs, invalid, extra)
        if po.returncode == ro.returncode == 2 and "must be >= 1" in po.stderr and "must be >= 1" in ro.stderr:
            rep.ok("pin-run:nonpositive-refused")
        else:
            rep.bad("pin-run:nonpositive-refused", f"py={po}\nrs={ro}")
        valid = ("pin-run", "--cores", "1", "--", "true")
        po, ro = run(py, valid, extra), run(rs, valid, extra)
        if po.returncode == ro.returncode == 0:
            rep.ok("pin-run:reserve-apply-release")
        elif po.returncode == ro.returncode == 3 and "HARD" in po.stderr and "HARD" in ro.stderr:
            rep.ok("pin-run:hard-capability-unavailable-refused")
        else:
            rep.bad(
                "pin-run:reserve-apply-release",
                f"expected both 0; py={po.returncode} rs={ro.returncode}\npy:{po.stderr}\nrs:{ro.stderr}",
            )

        missing_exec = ("pin-run", "--cores", "1", "--", "/definitely/missing/command")
        po, ro = run(py, missing_exec, extra), run(rs, missing_exec, extra)
        if po.returncode == ro.returncode == 3 and "Traceback" not in po.stderr and "panicked" not in ro.stderr:
            rep.ok("pin-run:missing-executable-clean")
        else:
            rep.bad("pin-run:missing-executable-clean", f"py={po}\nrs={ro}")

        signaled = (
            "pin-run",
            "--cores",
            "1",
            "--",
            sys.executable,
            "-c",
            "import os; os.kill(os.getpid(), 15)",
        )
        po, ro = run(py, signaled, extra), run(rs, signaled, extra)
        if po.returncode == ro.returncode == 143:
            rep.ok("pin-run:signal-status")
        elif po.returncode == ro.returncode == 3 and "HARD" in po.stderr and "HARD" in ro.stderr:
            rep.ok("pin-run:signal-status-hard-capability-unavailable")
        else:
            rep.bad("pin-run:signal-status", f"py={po}\nrs={ro}")


def compare_safe_ci_dag_runner(rand_count: int, seed: int) -> int:
    tool = "safe-ci-dag-runner"
    py = py_command()
    rs = rs_command(tool)
    rep = Report()

    compare_invocations(py, rs, rep)
    compare_package_guides(tool, py, rs, rep)
    compare_cli_schema(py, rs, rep)
    fixtures = representative_fixtures() + randomized_fixtures(rand_count, seed)
    for fx in fixtures:
        compare_fixture(py, rs, fx, rep)
    examples = example_fixtures()
    for fx in examples:
        compare_example_static(py, rs, fx, rep)
    compare_yaml_isomorphism(py, rs, rep)
    compare_scalar_parity(py, rs, rep)
    compare_only_errors(py, rs, rep)
    compare_escapee_teardown(py, rs, rep)
    compare_term_attribution(py, rs, rep)
    compare_test_attribution_evidence(py, rs, rep)
    compare_batch_teardown_grace(py, rs, rep)
    compare_run_timeout(py, rs, rep)
    compare_spawn_failure(py, rs, rep)
    compare_profile_store(py, rs, rep)
    compare_plan_feedback(py, rs, rep)
    compare_hostile_numeric_cells(py, rs, rep)
    compare_speedup_model(py, rs, rep)
    compare_cpa_planner(py, rs, rep)
    compare_memory_hardening(py, rs, rep)
    compare_summary_sync(py, rs, rep)
    compare_sweep_success(py, rs, rep)
    compare_sweep_errors(py, rs, rep)
    compare_args_stress(py, rs, rep)
    compare_run_parallel_limits(py, rs, rep)
    compare_boxed_cpu_bandwidth(py, rs, rep)
    compare_pin_run(py, rs, rep)
    yaml_paths = yaml_fixture_paths()
    n_fixtures = len(fixtures) + len(examples) + len(yaml_paths)

    if rep.failures:
        for failure in rep.failures:
            print(f"DIVERGENCE [{failure}")
        print(
            f"cross[{tool}]: {len(rep.failures)} divergence(s) out of {rep.checks} checks "
            f"across {n_fixtures} fixtures ({len(examples)} shipped JSON examples static-only; "
            f"{len(yaml_paths)} YAML fixtures for isomorphism)"
        )
        return 1

    print(
        f"cross[{tool}]: OK - {rep.checks} checks across {n_fixtures} fixtures agree "
        f"({len(examples)} shipped JSON examples static-only; "
        f"{len(yaml_paths)} YAML fixtures isomorphic to JSON; "
        f"json byte-identical: {rep.json_byte_identical}, yaml isomorphic: {rep.yaml_isomorphic}, "
        f"scalar-resolution parity cases: {rep.scalar_parity})"
    )
    return 0


def py_command_for(tool: str) -> list[str]:
    modules = {
        "safe-ci-dag-runner": "safe_ci_dag_runner",
        "cpuset-alloc": "safe_ci_dag_runner.cpuset_allocator",
        "tick-hub": "tick_hub",
        "pr-landing-planner": "pr_landing_planner",
        "herdr-run": "herdr_run",
        "herdr-agent": "herdr_run.agent_cli",
    }
    module = modules.get(tool)
    if module is None:
        raise ValueError(f"unknown Python tool {tool!r}")
    return [sys.executable, "-m", module]


def _record_exact(
    rep: Report,
    label: str,
    py: Sequence[str],
    rs: Sequence[str],
    args: Sequence[str],
    extra_env: Mapping[str, str] | None = None,
    *,
    expected: int | None = None,
    compare_stderr: bool = False,
) -> tuple[Outcome, Outcome]:
    po = run(py, args, extra_env)
    ro = run(rs, args, extra_env)
    if po.returncode != ro.returncode:
        rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}\npy:{po.stderr}\nrs:{ro.stderr}")
    elif expected is not None and po.returncode != expected:
        rep.bad(label, f"expected exit {expected}; both returned {po.returncode}")
    elif po.stdout != ro.stdout:
        rep.bad(label, f"stdout differs\n--- py ---\n{po.stdout}\n--- rs ---\n{ro.stdout}")
    elif compare_stderr and po.stderr != ro.stderr:
        rep.bad(label, f"stderr differs\n--- py ---\n{po.stderr}\n--- rs ---\n{ro.stderr}")
    else:
        rep.ok(label)
    return po, ro


def _record_same_exit(
    rep: Report,
    label: str,
    py: Sequence[str],
    rs: Sequence[str],
    args: Sequence[str],
    expected: int | None = None,
) -> tuple[Outcome, Outcome]:
    po = run(py, args)
    ro = run(rs, args)
    if po.returncode != ro.returncode:
        rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}\npy:{po.stderr}\nrs:{ro.stderr}")
    elif expected is not None and po.returncode != expected:
        rep.bad(label, f"expected exit {expected}; both returned {po.returncode}")
    else:
        rep.ok(label)
    return po, ro


def _parsed_json(text_value: str) -> tuple[bool, object]:
    try:
        value: object = json.loads(text_value)
    except json.JSONDecodeError as exc:
        return False, str(exc)
    return True, value


def compare_package_guides(
    tool: str, py: Sequence[str], rs: Sequence[str], rep: Report
) -> None:
    """Installed guides may contain language-specific install/API sections, but never leak the
    other distribution's package manager or language."""
    pairs = (
        ("py", py, ("cargo", "crates.io", "rust api", "rs/")),
        ("rs", rs, ("pip install", "pypi", "python api", "py/")),
    )
    for language, command, forbidden in pairs:
        outcome = run(command, ("--userguide",))
        lowered = outcome.stdout.lower()
        leaks = [token for token in forbidden if token in lowered]
        if outcome.returncode != 0:
            rep.bad(f"guide:{language}", f"exit={outcome.returncode}: {outcome.stderr}")
        elif tool not in lowered or len(outcome.stdout) < 500:
            rep.bad(f"guide:{language}", "guide is missing, truncated, or for the wrong tool")
        elif leaks:
            rep.bad(f"guide:{language}", f"cross-language/package-manager references: {leaks}")
        else:
            rep.ok(f"guide:{language}")


def compare_cpuset_alloc() -> int:
    tool = "cpuset-alloc"
    py = py_command_for(tool)
    rs = rs_command(tool)
    rep = Report()

    _record_exact(rep, "version", py, rs, ("--version",))
    for args, required in (
        ((), ("run", "status", "reclaim", "selftest")),
        (("--help",), ("run", "status", "reclaim", "selftest")),
        (("run", "--help"), ("--cores", "--tag", "--sample-s", "--max-irq-rate")),
        (("status", "--help"), ("--ledger",)),
        (("reclaim", "--help"), ("--ledger",)),
        (("selftest", "--help"), ("--cores", "--sample-s", "--max-irq-rate")),
    ):
        for engine, command in (("py", py), ("rs", rs)):
            outcome = run(command, args)
            missing = [token for token in required if token not in outcome.stdout]
            if outcome.returncode != 0 or missing:
                rep.bad(
                    f"schema:{engine}/{' '.join(args) or 'no-args'}",
                    f"exit={outcome.returncode}; missing={missing}\n{outcome.stdout}\n{outcome.stderr}",
                )
            else:
                rep.ok(f"schema:{engine}/{' '.join(args) or 'no-args'}")

    invalid_budget_cases: tuple[tuple[str, ...], ...] = (
        (
            "run",
            "--cores",
            "1",
            "--sample-s",
            "0",
            "--max-irq-rate",
            "1",
            "--",
            "true",
        ),
        ("selftest", "--sample-s", "0", "--max-irq-rate", "1"),
    )
    for budget_args in invalid_budget_cases:
        po, ro = run(py, budget_args), run(rs, budget_args)
        detail = "--sample-s must be > 0 when --max-irq-rate is set"
        if po.returncode == ro.returncode == 2 and detail in po.stderr and detail in ro.stderr:
            rep.ok(f"irq-budget:nonzero-sample:{budget_args[0]}")
        else:
            rep.bad(f"irq-budget:nonzero-sample:{budget_args[0]}", f"py={po}\nrs={ro}")

    selftest_args = ("selftest", "--cores", "1", "--sample-s", "0")
    py_selftest = run(py, selftest_args)
    rs_selftest = run(rs, selftest_args)
    py_ok, py_value = _parsed_json(py_selftest.stdout)
    rs_ok, rs_value = _parsed_json(rs_selftest.stdout)
    py_verdict = py_value.get("verdict") if py_ok and isinstance(py_value, dict) else None
    rs_verdict = rs_value.get("verdict") if rs_ok and isinstance(rs_value, dict) else None
    if (
        py_selftest.returncode == rs_selftest.returncode
        and py_selftest.returncode in (0, 1, 3)
        and py_verdict == rs_verdict
        and isinstance(py_verdict, str)
    ):
        rep.ok("selftest:mutation-verdict")
    else:
        rep.bad(
            "selftest:mutation-verdict",
            f"py={py_selftest.returncode}:{py_value!r}; "
            f"rs={rs_selftest.returncode}:{rs_value!r}",
        )

    with tempfile.TemporaryDirectory(prefix="cpuset-cross-") as tmp:
        py_ledger = os.path.join(tmp, "py.json")
        rs_ledger = os.path.join(tmp, "rs.json")
        for subcommand in ("status", "reclaim"):
            po = run(py, (subcommand, "--ledger", py_ledger))
            ro = run(rs, (subcommand, "--ledger", rs_ledger))
            p_ok, p_value = _parsed_json(po.stdout)
            r_ok, r_value = _parsed_json(ro.stdout)
            if po.returncode == ro.returncode == 0 and p_ok and r_ok and p_value == r_value:
                rep.ok(f"{subcommand}:empty-ledger")
            else:
                rep.bad(
                    f"{subcommand}:empty-ledger",
                    f"py={po.returncode}:{p_value!r}; rs={ro.returncode}:{r_value!r}",
                )

        Path(py_ledger).write_text("{", encoding="utf-8")
        Path(rs_ledger).write_text("{", encoding="utf-8")
        po = run(py, ("status", "--ledger", py_ledger))
        ro = run(rs, ("status", "--ledger", rs_ledger))
        if (
            po.returncode == ro.returncode == 3
            and "corrupt" in po.stderr
            and "corrupt" in ro.stderr
        ):
            rep.ok("status:corrupt-ledger-fails-closed")
        else:
            rep.bad("status:corrupt-ledger-fails-closed", f"py={po}\nrs={ro}")

        fifo_outcomes: dict[str, Outcome] = {}
        for engine, command in (("py", py), ("rs", rs)):
            fifo_ledger = os.path.join(tmp, f"{engine}-fifo.json")
            os.mkfifo(fifo_ledger, mode=0o600)
            fifo_outcomes[engine] = run(command, ("status", "--ledger", fifo_ledger))
        if all(out.returncode == 3 and out.elapsed_s < 2.0 for out in fifo_outcomes.values()):
            rep.ok("status:fifo-ledger-nonblocking-refusal")
        else:
            rep.bad("status:fifo-ledger-nonblocking-refusal", repr(fifo_outcomes))

        base_record: dict[str, object] = {
            "pid": 1,
            "starttime": 1,
            "cores": [0],
            "tag": "holder",
            "ts": 1.0,
        }

        def changed(**fields: object) -> dict[str, object]:
            return {**base_record, **fields}

        def missing_field(field: str) -> dict[str, object]:
            record = dict(base_record)
            del record[field]
            return record

        invalid_records = {
            "mixed-core": changed(cores=[0, "bad"]),
            "missing-cores": missing_field("cores"),
            "empty-cores": changed(cores=[]),
            "negative-core": changed(cores=[-1]),
            "overflow-core": changed(cores=[1 << 32]),
            "boolean-core": changed(cores=[True]),
            "fractional-core": changed(cores=[1.0]),
            "duplicate-core": changed(cores=[1, 1]),
            "zero-pid": changed(pid=0),
            "overflow-pid": changed(pid=1 << 32),
            "boolean-pid": changed(pid=True),
            "missing-pid": missing_field("pid"),
            "missing-starttime": missing_field("starttime"),
            "zero-starttime": changed(starttime=0),
            "string-starttime": changed(starttime="1"),
            "overflow-starttime": changed(starttime=1 << 64),
            "non-string-tag": changed(tag=7),
            "missing-tag": missing_field("tag"),
            "missing-ts": missing_field("ts"),
            "nonfinite-ts": changed(ts=float("inf")),
            "string-ts": changed(ts="1.0"),
            "boolean-ts": changed(ts=True),
            "negative-ts": changed(ts=-1),
        }
        for case, record in invalid_records.items():
            original = json.dumps({"reservations": [record]})
            outcomes: dict[str, Outcome] = {}
            preserved = True
            for engine, command in (("py", py), ("rs", rs)):
                path = os.path.join(tmp, f"invalid-{case}-{engine}.json")
                Path(path).write_text(original, encoding="utf-8")
                os.chmod(path, 0o600)
                outcomes[engine] = run(command, ("status", "--ledger", path))
                preserved = preserved and Path(path).read_text(encoding="utf-8") == original
            if all(outcome.returncode == 3 for outcome in outcomes.values()) and preserved:
                rep.ok(f"schema:{case}-fails-closed")
            else:
                rep.bad(
                    f"schema:{case}-fails-closed",
                    f"preserved={preserved}; outcomes={outcomes!r}",
                )

        # A parent-owned record is live from both child processes' perspective.
        stat_text = open(f"/proc/{os.getpid()}/stat", encoding="utf-8").read()
        starttime = int(stat_text[stat_text.rfind(")") + 2 :].split()[19])
        live_payload = {
            "reservations": [
                {
                    "pid": os.getpid(),
                    "starttime": starttime,
                    "cores": [7, 3],
                    "tag": "cross-live",
                    "ts": 1.25,
                }
            ]
        }
        for path in (py_ledger, rs_ledger):
            with open(path, "w", encoding="utf-8") as handle:
                json.dump(live_payload, handle)
        po = run(py, ("status", "--ledger", py_ledger))
        ro = run(rs, ("status", "--ledger", rs_ledger))
        p_ok, p_value = _parsed_json(po.stdout)
        r_ok, r_value = _parsed_json(ro.stdout)
        if po.returncode == ro.returncode == 0 and p_ok and r_ok and p_value == r_value:
            rep.ok("status:live-shared-schema")
        else:
            rep.bad("status:live-shared-schema", f"py={p_value!r}; rs={r_value!r}")

        dead_payload = {
            "reservations": [
                {"pid": 2147483647, "starttime": 1, "cores": [5], "tag": "dead", "ts": 2.5}
            ]
        }
        for path in (py_ledger, rs_ledger):
            with open(path, "w", encoding="utf-8") as handle:
                json.dump(dead_payload, handle)
        po = run(py, ("reclaim", "--ledger", py_ledger))
        ro = run(rs, ("reclaim", "--ledger", rs_ledger))
        p_ok, p_value = _parsed_json(po.stdout)
        r_ok, r_value = _parsed_json(ro.stdout)
        if po.returncode == ro.returncode == 0 and p_ok and r_ok and p_value == r_value:
            rep.ok("reclaim:dead-shared-schema")
        else:
            rep.bad("reclaim:dead-shared-schema", f"py={p_value!r}; rs={r_value!r}")

        # Each implementation must be able to observe the other's LIVE reservation, choose a
        # disjoint core, and leave the shared ledger empty after both wrapped commands exit.
        if len(os.sched_getaffinity(0)) >= 2:
            assignment_re = re.compile(r'reserved (\{"cores":\[[^]]*\],"count":\d+\})')

            def reserved_cores(stderr: str) -> set[int] | None:
                match = assignment_re.search(stderr)
                if match is None:
                    return None
                value: object = json.loads(match.group(1))
                if not isinstance(value, dict):
                    return None
                cores = value.get("cores")
                if not isinstance(cores, list) or not all(
                    isinstance(core, int) and not isinstance(core, bool) for core in cores
                ):
                    return None
                return set(cores)

            for label, first, second in (
                ("py-then-rs", py, rs),
                ("rs-then-py", rs, py),
            ):
                shared_ledger = os.path.join(tmp, f"interop-{label}.json")
                extra = {"SAFE_CI_CORE_LEDGER": shared_ledger}
                hold = (
                    "run",
                    "--cores",
                    "1",
                    "--tag",
                    label,
                    "--",
                    sys.executable,
                    "-c",
                    "import time; time.sleep(2)",
                )
                first_proc = subprocess.Popen(
                    [*first, *hold],
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                    env=_env(extra),
                    start_new_session=True,
                )
                deadline = time.monotonic() + 10
                while first_proc.poll() is None and time.monotonic() < deadline:
                    try:
                        payload = json.loads(Path(shared_ledger).read_text(encoding="utf-8"))
                    except (OSError, json.JSONDecodeError):
                        payload = {}
                    if isinstance(payload, dict) and payload.get("reservations"):
                        break
                    time.sleep(0.02)
                second_outcome = run(second, hold, extra)
                try:
                    first_stdout, first_stderr = first_proc.communicate(timeout=30)
                except subprocess.TimeoutExpired:
                    first_proc.kill()
                    first_stdout, first_stderr = first_proc.communicate()
                first_outcome = Outcome(
                    first_proc.returncode or 0,
                    first_stdout,
                    first_stderr,
                )
                if first_outcome.returncode == second_outcome.returncode == 0:
                    try:
                        first_cores = reserved_cores(first_outcome.stderr)
                        second_cores = reserved_cores(second_outcome.stderr)
                        final_payload: object = json.loads(
                            Path(shared_ledger).read_text(encoding="utf-8")
                        )
                    except (OSError, TypeError, json.JSONDecodeError):
                        rep.bad(
                            f"interop:{label}",
                            f"could not parse assignments/ledger\nfirst={first_outcome}\n"
                            f"second={second_outcome}",
                        )
                    else:
                        if (
                            first_cores is not None
                            and second_cores is not None
                            and first_cores
                            and second_cores
                            and first_cores.isdisjoint(second_cores)
                            and final_payload == {"reservations": []}
                        ):
                            rep.ok(f"interop:{label}")
                        else:
                            rep.bad(
                                f"interop:{label}",
                                f"first={first_cores}; second={second_cores}; "
                                f"final={final_payload!r}",
                            )
                elif (
                    first_outcome.returncode == second_outcome.returncode == 3
                    and "HARD" in first_outcome.stderr
                    and "HARD" in second_outcome.stderr
                ):
                    rep.ok(f"interop:{label}:hard-capability-unavailable")
                else:
                    rep.bad(
                        f"interop:{label}",
                        f"first={first_outcome}\nsecond={second_outcome}",
                    )

    _record_same_exit(rep, "run:missing-command", py, rs, ("run", "--cores", "1"), 2)
    _record_same_exit(
        rep,
        "run:separator-required",
        py,
        rs,
        ("run", "--cores", "1", "true"),
        2,
    )
    invalid_invocations: tuple[tuple[str, tuple[str, ...]], ...] = (
        ("run:nonpositive-cores", ("run", "--cores", "0", "--", "true")),
        (
            "run:negative-sample",
            ("run", "--cores", "1", "--sample-s", "-1", "--", "true"),
        ),
        (
            "run:nonfinite-sample",
            ("run", "--cores", "1", "--sample-s", "nan", "--", "true"),
        ),
        ("selftest:tag-rejected", ("selftest", "--tag", "x")),
    )
    for label, invalid_args in invalid_invocations:
        _record_same_exit(rep, label, py, rs, invalid_args, 2)

    # A wrapped command's own help flag belongs to that command, not the allocator.
    help_args = ("run", "--cores", "1", "--", "printf", "%s\\n", "--help")
    po, ro = run(py, help_args), run(rs, help_args)
    if (
        po.returncode == ro.returncode == 0
        and po.stdout.endswith("--help\n")
        and ro.stdout.endswith("--help\n")
    ):
        rep.ok("run:wrapped-help-passthrough")
    elif (
        po.returncode == ro.returncode == 3
        and "HARD" in po.stderr
        and "HARD" in ro.stderr
    ):
        rep.ok("run:wrapped-help-hard-capability-unavailable")
    else:
        rep.bad("run:wrapped-help-passthrough", f"py={po}\nrs={ro}")

    missing_exec = ("run", "--cores", "1", "--", "/definitely/missing/command")
    po, ro = run(py, missing_exec), run(rs, missing_exec)
    if (
        po.returncode == ro.returncode == 3
        and "Traceback" not in po.stderr
        and "panicked" not in ro.stderr
    ):
        rep.ok("run:missing-executable-clean")
    else:
        rep.bad("run:missing-executable-clean", f"py={po}\nrs={ro}")

    signaled = (
        "run",
        "--cores",
        "1",
        "--",
        sys.executable,
        "-c",
        "import os; os.kill(os.getpid(), 15)",
    )
    po, ro = run(py, signaled), run(rs, signaled)
    if po.returncode == ro.returncode == 143:
        rep.ok("run:signal-status")
    elif po.returncode == ro.returncode == 3 and "HARD" in po.stderr and "HARD" in ro.stderr:
        rep.ok("run:signal-status-hard-capability-unavailable")
    else:
        rep.bad("run:signal-status", f"py={po}\nrs={ro}")
    _record_same_exit(rep, "cli:abbreviation-refused", py, rs, ("status", "--ledg", "x"), 2)
    _record_same_exit(rep, "unknown-command", py, rs, ("not-a-command",), 2)

    if rep.failures:
        for failure in rep.failures:
            print(f"DIVERGENCE [{failure}]")
        print(f"cross[{tool}]: {len(rep.failures)} divergence(s) out of {rep.checks} checks")
        return 1
    print(f"cross[{tool}]: OK - {rep.checks} behavioral and ledger-schema checks agree")
    return 0


def compare_tick_hub(rand_count: int, seed: int) -> int:
    tool = "tick-hub"
    py = py_command_for(tool)
    rs = rs_command(tool)
    rep = Report()

    _record_exact(rep, "version", py, rs, ("--version",))
    compare_package_guides(tool, py, rs, rep)
    for engine, command in (("py", py), ("rs", rs)):
        top = run(command, ("--help",))
        required = ("tick", "state", "list", "json", "yaml", "quickstart", "--userguide")
        missing = [token for token in required if token not in top.stdout]
        if top.returncode != 0 or missing:
            rep.bad(f"schema:{engine}/commands", f"exit={top.returncode}; missing={missing}")
        else:
            rep.ok(f"schema:{engine}/commands")
        command_flags = {
            "tick": (
                "--config",
                "--state",
                "--fired-state",
                "--now",
                "--current-tick-min",
                "--flush",
                "--no-header",
            ),
            "state": ("--state", "--current-tick-min"),
            "list": ("--config",),
            "json": ("--config",),
            "yaml": ("--config",),
        }
        for subcommand, flags in command_flags.items():
            outcome = run(command, (subcommand, "--help"))
            absent = [flag for flag in flags if flag not in outcome.stdout]
            if outcome.returncode != 0 or absent:
                rep.bad(
                    f"schema:{engine}/{subcommand}",
                    f"exit={outcome.returncode}; missing={absent}\n{outcome.stdout}",
                )
            else:
                rep.ok(f"schema:{engine}/{subcommand}")

    with tempfile.TemporaryDirectory(prefix="tick-hub-cross-") as tmp:
        watched = os.path.join(tmp, "snapshot.dat")
        with open(watched, "w", encoding="utf-8") as handle:
            handle.write("snapshot\n")
        os.utime(watched, (800, 800))
        config: dict[str, object] = {
            "description": "cross-check unicode \u2603",
            "reminders": [
                {
                    "name": "always",
                    "cadence_secs": 0,
                    "emit": {
                        "kind": "action",
                        "skill": "refresh-cache",
                        "fields": {"z": "last", "a": "first"},
                        "title": "refresh {a} {z}",
                    },
                },
                {
                    "name": "gated",
                    "cadence_secs": 60,
                    "gate": {
                        "cmd": "printf 'count=3\\n'",
                        "when": "nonempty",
                        "capture": True,
                    },
                    "emit": {
                        "kind": "action",
                        "skill": "triage",
                        "title": "triage {count}",
                    },
                },
                {
                    "name": "guarded",
                    "requires_flags": ["ready"],
                    "emit": {"kind": "note", "title": "guard is enabled"},
                },
                {
                    "name": "not_due",
                    "cadence_secs": 500,
                    "emit": {"kind": "note", "title": "must stay quiet"},
                },
            ],
            "health_checks": [
                {
                    "name": "snapshot",
                    "glob": watched,
                    "threshold_secs": 100,
                    "detail": "fixture age",
                },
                {
                    "name": "absent",
                    "glob": os.path.join(tmp, "missing-*"),
                    "threshold_secs": 1,
                },
            ],
        }
        json_path = os.path.join(tmp, "config.json")
        with open(json_path, "w", encoding="utf-8") as handle:
            json.dump(config, handle, ensure_ascii=False)
        state_path = os.path.join(tmp, "state.yaml")
        with open(state_path, "w", encoding="utf-8") as handle:
            handle.write(
                "enabled: true\n"
                "tick_frequency_min: 30\n"
                "label: cross-host\n"
                "flags: {ready: true, count: 2}\n"
                "_annotation: ignored\n"
            )
        fired = "gated=900\nnot_due=900\n"
        py_fired = os.path.join(tmp, "py-fired")
        rs_fired = os.path.join(tmp, "rs-fired")
        for path in (py_fired, rs_fired):
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(fired)

        for subcommand in ("json", "list"):
            _record_exact(rep, f"representative:{subcommand}", py, rs, (subcommand, "--config", json_path))
        _record_exact(
            rep,
            "representative:state",
            py,
            rs,
            ("state", "--state", state_path, "--current-tick-min", "15"),
        )
        common_tick = (
            "tick",
            "--config",
            json_path,
            "--state",
            state_path,
            "--now",
            "1000",
            "--no-header",
        )
        po = run(py, (*common_tick, "--fired-state", py_fired))
        ro = run(rs, (*common_tick, "--fired-state", rs_fired))
        if po.returncode == ro.returncode == 0 and po.stdout == ro.stdout:
            rep.ok("representative:tick")
        else:
            rep.bad(
                "representative:tick",
                f"py={po.returncode}:{po.stdout!r}\nrs={ro.returncode}:{ro.stdout!r}",
            )

        dependency_config = {
            "reminders": [
                {
                    "name": "foundation",
                    "gate": {
                        "cmd": "printf 'summary=foundation unavailable\\n'; exit 75",
                        "when": "failure",
                        "capture": True,
                    },
                    "emit": {"skill": "warn", "title": "foundation problem"},
                },
                {
                    "name": "dependent",
                    "depends_on": ["foundation"],
                    "gate": {"cmd": "exit 0", "when": "failure"},
                    "emit": {"skill": "warn", "title": "dependent problem"},
                },
                {
                    "name": "independent",
                    "gate": {"cmd": "exit 0", "when": "failure"},
                    "emit": {"skill": "warn", "title": "independent problem"},
                },
            ]
        }
        dependency_path = os.path.join(tmp, "dependencies.json")
        with open(dependency_path, "w", encoding="utf-8") as handle:
            json.dump(dependency_config, handle)
        py_dependency_fired = os.path.join(tmp, "py-dependency-fired")
        rs_dependency_fired = os.path.join(tmp, "rs-dependency-fired")
        dependency_args = (
            "tick",
            "--config",
            dependency_path,
            "--state",
            state_path,
            "--now",
            "1000",
            "--no-header",
        )
        po = run(py, (*dependency_args, "--fired-state", py_dependency_fired, "--flush"))
        ro = run(rs, (*dependency_args, "--fired-state", rs_dependency_fired, "--flush"))
        with open(py_dependency_fired, encoding="utf-8") as handle:
            py_dependency_state = handle.read()
        with open(rs_dependency_fired, encoding="utf-8") as handle:
            rs_dependency_state = handle.read()
        if (
            po.returncode == ro.returncode == 0
            and po.stdout == ro.stdout
            and py_dependency_state == rs_dependency_state
            and "NO_RESULT: dependent is unevaluable" in po.stdout
            and not any(
                line.startswith("dependent=") for line in py_dependency_state.splitlines()
            )
            and any(
                line == "independent=1000" for line in py_dependency_state.splitlines()
            )
        ):
            rep.ok("dependency-no-result:quiet-dependent-unevaluable")
        else:
            rep.bad(
                "dependency-no-result:quiet-dependent-unevaluable",
                f"py={po.returncode}:{po.stdout!r}:{py_dependency_state!r}\n"
                f"rs={ro.returncode}:{ro.stdout!r}:{rs_dependency_state!r}",
            )

        unresolved_config: dict[str, object] = {
            "reminders": [
                {
                    "name": "unresolved",
                    "gate": {
                        "cmd": "exit 1",
                        "when": "failure",
                        "capture": True,
                    },
                    "emit": {"skill": "warn", "title": "problem: {summary}"},
                }
            ]
        }
        unresolved_path = os.path.join(tmp, "unresolved.json")
        with open(unresolved_path, "w", encoding="utf-8") as handle:
            json.dump(unresolved_config, handle)
        py_unresolved_fired = os.path.join(tmp, "py-unresolved-fired")
        rs_unresolved_fired = os.path.join(tmp, "rs-unresolved-fired")
        repeated_ok = True
        repeated_detail = []
        for consecutive, now in enumerate((1000, 1001, 1002), start=1):
            unresolved_args = (
                "tick",
                "--config",
                unresolved_path,
                "--state",
                state_path,
                "--now",
                str(now),
                "--no-header",
                "--flush",
            )
            po = run(py, (*unresolved_args, "--fired-state", py_unresolved_fired))
            ro = run(rs, (*unresolved_args, "--fired-state", rs_unresolved_fired))
            with open(py_unresolved_fired, encoding="utf-8") as handle:
                py_unresolved_state = handle.read()
            with open(rs_unresolved_fired, encoding="utf-8") as handle:
                rs_unresolved_state = handle.read()
            repeated = "consecutive_failures=" in po.stdout
            expected_repeated = consecutive >= 3
            step_ok = (
                po.returncode == ro.returncode == 0
                and po.stdout == ro.stdout
                and py_unresolved_state == rs_unresolved_state
                and repeated == expected_repeated
                and "reason=unresolved-placeholder" in po.stdout
                and f".count={consecutive}\n" in py_unresolved_state
                and ".first_failure_epoch=1000\n" in py_unresolved_state
                and not any(
                    line.startswith("unresolved=")
                    for line in py_unresolved_state.splitlines()
                )
            )
            repeated_ok = repeated_ok and step_ok
            repeated_detail.append(
                f"step={consecutive} ok={step_ok} py={po.stdout!r} state={py_unresolved_state!r}"
            )
        if repeated_ok:
            rep.ok("unresolved-render:third-consecutive-escalates")
        else:
            rep.bad(
                "unresolved-render:third-consecutive-escalates",
                "\n".join(repeated_detail),
            )

        recovered_config = {
            "reminders": [
                {
                    "name": "unresolved",
                    "gate": {
                        "cmd": "printf 'summary=recovered\\n'; exit 1",
                        "when": "failure",
                        "capture": True,
                    },
                    "emit": {"skill": "warn", "title": "problem: {summary}"},
                }
            ]
        }
        with open(unresolved_path, "w", encoding="utf-8") as handle:
            json.dump(recovered_config, handle)
        recovery_args = (
            "tick",
            "--config",
            unresolved_path,
            "--state",
            state_path,
            "--now",
            "1003",
            "--no-header",
            "--flush",
        )
        po = run(py, (*recovery_args, "--fired-state", py_unresolved_fired))
        ro = run(rs, (*recovery_args, "--fired-state", rs_unresolved_fired))
        with open(py_unresolved_fired, encoding="utf-8") as handle:
            py_recovered_state = handle.read()
        with open(rs_unresolved_fired, encoding="utf-8") as handle:
            rs_recovered_state = handle.read()
        if (
            po.returncode == ro.returncode == 0
            and po.stdout == ro.stdout
            and py_recovered_state == rs_recovered_state
            and "problem: recovered" in po.stdout
            and "unresolved=1003\n" in py_recovered_state
            and "__tick_hub_internal__.unresolved_render.unresolved" not in py_recovered_state
        ):
            rep.ok("unresolved-render:successful-render-clears-streak")
        else:
            rep.bad(
                "unresolved-render:successful-render-clears-streak",
                f"py={po.returncode}:{po.stdout!r}:{py_recovered_state!r}\n"
                f"rs={ro.returncode}:{ro.stdout!r}:{rs_recovered_state!r}",
            )

        po = run(py, (*common_tick, "--fired-state", py_fired, "--flush"))
        ro = run(rs, (*common_tick, "--fired-state", rs_fired, "--flush"))
        with open(py_fired, encoding="utf-8") as handle:
            py_state = handle.read()
        with open(rs_fired, encoding="utf-8") as handle:
            rs_state = handle.read()
        if po.returncode == ro.returncode == 0 and po.stdout == ro.stdout and py_state == rs_state:
            rep.ok("representative:flush-bytes")
        else:
            rep.bad(
                "representative:flush-bytes",
                f"exit py={po.returncode} rs={ro.returncode}; stdout_equal={po.stdout == ro.stdout}; "
                f"state py={py_state!r} rs={rs_state!r}",
            )

        py_yaml = run(py, ("yaml", "--config", json_path))
        rs_yaml = run(rs, ("yaml", "--config", json_path))
        py_yaml_path = os.path.join(tmp, "py.yaml")
        rs_yaml_path = os.path.join(tmp, "rs.yaml")
        with open(py_yaml_path, "w", encoding="utf-8") as handle:
            handle.write(py_yaml.stdout)
        with open(rs_yaml_path, "w", encoding="utf-8") as handle:
            handle.write(rs_yaml.stdout)
        py_roundtrip = run(py, ("json", "--config", py_yaml_path))
        rs_from_py = run(rs, ("json", "--config", py_yaml_path))
        py_from_rs = run(py, ("json", "--config", rs_yaml_path))
        rs_roundtrip = run(rs, ("json", "--config", rs_yaml_path))
        canonical = run(py, ("json", "--config", json_path)).stdout
        outputs = (
            py_roundtrip.stdout,
            rs_from_py.stdout,
            py_from_rs.stdout,
            rs_roundtrip.stdout,
        )
        if (
            py_yaml.returncode == rs_yaml.returncode == 0
            and all(outcome == canonical for outcome in outputs)
        ):
            rep.ok("yaml:bidirectional-isomorphism")
        else:
            rep.bad("yaml:bidirectional-isomorphism", "YAML emit/load did not preserve canonical JSON")

        invalid_docs: tuple[tuple[str, str], ...] = (
            ("unknown.json", '{"unknown": true}'),
            ("null-list.json", '{"reminders": null}'),
            (
                "duplicate-name.json",
                '{"reminders":[{"name":"r","emit":{"skill":"s"}},{"name":"r","emit":{"skill":"s"}}]}',
            ),
            (
                "duplicate-key.json",
                '{"reminders":[],"reminders":[]}',
            ),
            (
                "reserved.json",
                '{"reminders":[{"name":"r","emit":{"skill":"s","fields":{"<<":"x"}}}]}',
            ),
            (
                "negative.json",
                '{"reminders":[{"name":"r","cadence_secs":-1,"emit":{"skill":"s"}}]}',
            ),
            (
                "overflow.json",
                '{"reminders":[{"name":"r","cadence_secs":9223372036854775808,"emit":{"skill":"s"}}]}',
            ),
            (
                "unknown-dependency.json",
                '{"reminders":[{"name":"r","depends_on":["missing"],"emit":{"skill":"s"}}]}',
            ),
            (
                "dependency-cycle.json",
                '{"reminders":[{"name":"a","depends_on":["b"],"emit":{"skill":"s"}},'
                '{"name":"b","depends_on":["a"],"emit":{"skill":"s"}}]}',
            ),
            ("duplicate.yaml", "reminders: []\nreminders: []\n"),
            ("non-string.yaml", "1: value\nreminders: []\n"),
            ("merge.yaml", "base: &b {description: x}\n<<: *b\nreminders: []\n"),
            (
                "nonfinite.yaml",
                "reminders:\n  - {name: r, gate: .nan, emit: {skill: s}}\n",
            ),
        )
        for name, body in invalid_docs:
            path = os.path.join(tmp, name)
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(body)
            _record_same_exit(rep, f"reject:{name}", py, rs, ("json", "--config", path), 2)

        for value in ("9223372036854775808", "-9223372036854775809"):
            _record_same_exit(
                rep,
                f"cli-i64:{value}",
                py,
                rs,
                ("tick", "--config", json_path, "--now", value),
                2,
            )
        _record_same_exit(
            rep,
            "cli-domain:negative-now",
            py,
            rs,
            ("tick", "--config", json_path, "--now", "-1"),
            2,
        )
        _record_same_exit(
            rep,
            "cli-domain:zero-current-cadence",
            py,
            rs,
            ("tick", "--config", json_path, "--current-tick-min", "0"),
            2,
        )
        _record_same_exit(
            rep,
            "cli:abbreviation-refused",
            py,
            rs,
            ("tick", "--conf", json_path, "--now", "0"),
            2,
        )
        for hostile_integer in ("1_0", " 10", "١٠"):
            _record_same_exit(
                rep,
                f"cli:integer-syntax:{hostile_integer!r}",
                py,
                rs,
                ("tick", "--config", json_path, f"--now={hostile_integer}"),
                2,
            )

        rng = random.Random(seed)
        for index in range(rand_count):
            reminders: list[object] = []
            for reminder_index in range(rng.randint(0, 6)):
                kind = "action" if rng.random() < 0.7 else "note"
                emit: dict[str, object] = {
                    "kind": kind,
                    "title": f"random {index}:{reminder_index} \u2603",
                }
                if kind == "action":
                    emit["skill"] = f"handler-{reminder_index}"
                    emit["fields"] = {"iteration": str(index)}
                reminders.append(
                    {
                        "name": f"r{reminder_index}",
                        "cadence_secs": rng.choice((0, 1, 30, 3600)),
                        "emit": emit,
                    }
                )
            random_config: dict[str, object] = {
                "description": f"random fixture {index}",
                "reminders": reminders,
                "health_checks": [],
            }
            path = os.path.join(tmp, f"random-{index}.json")
            with open(path, "w", encoding="utf-8") as handle:
                json.dump(random_config, handle, ensure_ascii=False)
            _record_exact(rep, f"random:{index}/json", py, rs, ("json", "--config", path))
            _record_exact(
                rep,
                f"random:{index}/tick",
                py,
                rs,
                ("tick", "--config", path, "--now", str(index), "--no-header"),
            )

    if rep.failures:
        for failure in rep.failures:
            print(f"DIVERGENCE [{failure}]")
        print(f"cross[{tool}]: {len(rep.failures)} divergence(s) out of {rep.checks} checks")
        return 1
    print(f"cross[{tool}]: OK - {rep.checks} deterministic, randomized, and adversarial checks agree")
    return 0


def compare_pr_landing_planner(rand_count: int, seed: int) -> int:
    tool = "pr-landing-planner"
    py = py_command_for(tool)
    rs = rs_command(tool)
    rep = Report()

    _record_exact(rep, "version", py, rs, ("--version",))
    compare_package_guides(tool, py, rs, rep)
    command_flags = (
        "--repo",
        "--base",
        "--fixture",
        "--format",
        "--git-dir",
        "--prs",
        "--conflict-detector",
        "--gate-check",
        "--landing-context",
        "--flaky-signatures",
        "--outage-min-prs",
        "--freshness-max-behind",
        "--priority-source",
        "--batch",
    )
    for engine, command in (("py", py), ("rs", rs)):
        outcome = run(command, ("--help",))
        commands = ("plan", "graph", "clusters", "status", "quickstart")
        missing = [token for token in commands if token not in outcome.stdout]
        if outcome.returncode != 0 or missing:
            rep.bad(
                f"schema:{engine}/commands",
                f"exit={outcome.returncode}; missing={missing}\n{outcome.stdout}",
            )
        else:
            rep.ok(f"schema:{engine}/commands")
        plan_help = run(command, ("plan", "--help"))
        missing_flags = [token for token in command_flags if token not in plan_help.stdout]
        neutral_priority_help = (
            "command" in plan_help.stdout and "beads" not in plan_help.stdout.lower()
        )
        if plan_help.returncode != 0 or missing_flags or not neutral_priority_help:
            rep.bad(
                f"schema:{engine}/plan",
                f"exit={plan_help.returncode}; missing={missing_flags}; "
                f"neutral_priority_help={neutral_priority_help}\n{plan_help.stdout}",
            )
        else:
            rep.ok(f"schema:{engine}/plan")

    with tempfile.TemporaryDirectory(prefix="planner-cross-") as tmp:
        fixture: dict[str, object] = {
            "repo": "OWNER/NAME",
            "base": "main",
            "prs": [
                {
                    "number": 1,
                    "title": "green",
                    "head_ref": "green",
                    "additions": 4,
                    "changed_files": ["src/a.rs"],
                    "checks": [
                        {"name": "CI", "status": "COMPLETED", "conclusion": "SUCCESS"},
                        {"name": "merge-gate", "status": "COMPLETED", "conclusion": "SUCCESS"},
                    ],
                },
                {
                    "number": 2,
                    "title": "behind",
                    "head_ref": "behind",
                    "commits_behind": 3,
                    "changed_files": ["src/b.rs"],
                    "checks": [
                        {"name": "CI", "status": "COMPLETED", "conclusion": "SUCCESS"},
                        {"name": "merge-gate", "status": "COMPLETED", "conclusion": "SUCCESS"},
                    ],
                },
                {
                    "number": 3,
                    "title": "stale gate",
                    "head_ref": "stale",
                    "changed_files": ["src/c.rs"],
                    "checks": [
                        {"name": "CI", "status": "COMPLETED", "conclusion": "SUCCESS"},
                        {
                            "name": "merge-gate",
                            "status": "COMPLETED",
                            "conclusion": "FAILURE",
                            "text": "stale",
                        },
                    ],
                },
                {
                    "number": 4,
                    "title": "gate race",
                    "head_ref": "race",
                    "changed_files": ["src/d.rs"],
                    "checks": [
                        {"name": "CI", "status": "IN_PROGRESS", "conclusion": ""},
                        {
                            "name": "merge-gate",
                            "status": "COMPLETED",
                            "conclusion": "FAILURE",
                            "text": "Full CI still queued; rerun after CI completes",
                        },
                    ],
                },
                {
                    "number": 5,
                    "title": "real failure",
                    "head_ref": "red",
                    "changed_files": ["src/e.rs"],
                    "checks": [
                        {
                            "name": "unit",
                            "status": "COMPLETED",
                            "conclusion": "FAILURE",
                            "text": "assertion",
                        },
                        {"name": "merge-gate", "status": "COMPLETED", "conclusion": "SUCCESS"},
                    ],
                },
                {
                    "number": 6,
                    "title": "missing required gate",
                    "head_ref": "missing-gate",
                    "changed_files": ["src/f.rs"],
                    "checks": [
                        {"name": "CI", "status": "COMPLETED", "conclusion": "SUCCESS"}
                    ],
                },
                {
                    "number": 7,
                    "title": "neutral required gate",
                    "head_ref": "neutral-gate",
                    "changed_files": ["src/g.rs"],
                    "checks": [
                        {"name": "CI", "status": "COMPLETED", "conclusion": "SUCCESS"},
                        {"name": "merge-gate", "status": "COMPLETED", "conclusion": "NEUTRAL"},
                    ],
                },
                {
                    "number": 8,
                    "title": "non-gate queued words",
                    "head_ref": "real-queued",
                    "changed_files": ["src/h.rs"],
                    "checks": [
                        {
                            "name": "unit",
                            "status": "COMPLETED",
                            "conclusion": "FAILURE",
                            "text": "dependency still queued",
                        },
                        {"name": "merge-gate", "status": "COMPLETED", "conclusion": "SUCCESS"},
                    ],
                },
                {
                    "number": 9,
                    "title": "draft policy change",
                    "head_ref": "policy",
                    "is_draft": True,
                    "changed_files": ["src/i.rs"],
                    "checks": [
                        {"name": "CI", "status": "COMPLETED", "conclusion": "SUCCESS"},
                        {"name": "merge-gate", "status": "COMPLETED", "conclusion": "SUCCESS"},
                    ],
                },
                {
                    "number": 10,
                    "title": "approval required",
                    "head_ref": "review-required",
                    "review_decision": "REVIEW_REQUIRED",
                    "changed_files": ["src/j.rs"],
                    "checks": [
                        {"name": "CI", "status": "COMPLETED", "conclusion": "SUCCESS"},
                        {"name": "merge-gate", "status": "COMPLETED", "conclusion": "SUCCESS"},
                    ],
                },
                {
                    "number": 11,
                    "title": "changes requested",
                    "head_ref": "changes-requested",
                    "review_decision": "CHANGES_REQUESTED",
                    "changed_files": ["src/k.rs"],
                    "checks": [
                        {"name": "CI", "status": "COMPLETED", "conclusion": "SUCCESS"},
                        {"name": "merge-gate", "status": "COMPLETED", "conclusion": "SUCCESS"},
                    ],
                },
                {
                    "number": 12,
                    "title": "green child of broken predecessor",
                    "head_ref": "child-of-red",
                    "base_ref": "red",
                    "changed_files": ["src/l.rs"],
                    "checks": [
                        {"name": "CI", "status": "COMPLETED", "conclusion": "SUCCESS"},
                        {"name": "merge-gate", "status": "COMPLETED", "conclusion": "SUCCESS"},
                    ],
                },
                {
                    "number": 13,
                    "title": "cycle root",
                    "head_ref": "cycle-root",
                    "changed_files": ["src/m.rs"],
                    "checks": [
                        {"name": "CI", "status": "COMPLETED", "conclusion": "SUCCESS"},
                        {"name": "merge-gate", "status": "COMPLETED", "conclusion": "SUCCESS"},
                    ],
                },
                {
                    "number": 14,
                    "title": "cycle child",
                    "head_ref": "cycle-child",
                    "base_ref": "cycle-root",
                    "changed_files": ["src/n.rs"],
                    "checks": [
                        {"name": "CI", "status": "COMPLETED", "conclusion": "SUCCESS"},
                        {"name": "merge-gate", "status": "COMPLETED", "conclusion": "SUCCESS"},
                    ],
                },
            ],
            "conflicts": [{"a": 1, "b": 2, "paths": ["src/shared.rs"]}],
            "ancestry": [
                {"before": 1, "after": 3},
                {"before": 14, "after": 13},
            ],
        }
        fixture_path = os.path.join(tmp, "fixture.json")
        with open(fixture_path, "w", encoding="utf-8") as handle:
            json.dump(fixture, handle)
        context = {
            "prs": [
                {
                    "pr": 9,
                    "head_sha": "sha-9",
                    "base_sha": "basesha-main",
                    "validation_evidence": "clean-validate-record",
                    "policy_class": "gate-policy",
                    "assigned_agent": "reviewer",
                }
            ]
        }
        context_path = os.path.join(tmp, "context.json")
        with open(context_path, "w", encoding="utf-8") as handle:
            json.dump(context, handle)

        base_args = ("--fixture", fixture_path, "--landing-context", context_path)
        for planner_command, formats in (
            ("plan", ("human", "json", "actions")),
            ("graph", ("human", "json")),
            ("clusters", ("human", "json")),
            ("status", ("human", "json")),
        ):
            for output_format in formats:
                command_extra = ("--batch",) if planner_command == "plan" else ()
                _record_exact(
                    rep,
                    f"representative:{planner_command}/{output_format}",
                    py,
                    rs,
                    (planner_command, *base_args, "--format", output_format, *command_extra),
                )

        _record_exact(
            rep,
            "representative:command-priority",
            py,
            rs,
            (
                "plan",
                *base_args,
                "--format",
                "json",
                "--priority-source",
                "command",
                "--priority-cmd",
                "printf '%s' '{pr}'",
            ),
            compare_stderr=True,
        )

        for label, priority_args in (
            ("missing-command", ("--priority-source", "command")),
            (
                "missing-label-capture",
                ("--priority-source", "labels", "--priority-label-pattern", "^p[0-9]+$"),
            ),
            (
                "overflow-command-output",
                (
                    "--priority-source",
                    "command",
                    "--priority-cmd",
                    "printf 9223372036854775808",
                ),
            ),
            (
                "failed-command",
                ("--priority-source", "command", "--priority-cmd", "exit 9"),
            ),
            (
                "hook-launch-failure",
                (
                    "--priority-source",
                    "command",
                    "--priority-cmd",
                    "printf 1",
                    "--net-wrapper",
                    "/definitely/missing-priority-wrapper",
                ),
            ),
        ):
            _record_exact(
                rep,
                f"reject:priority/{label}",
                py,
                rs,
                ("plan", *base_args, *priority_args),
                expected=2,
                compare_stderr=True,
            )

        plan = run(py, ("plan", *base_args, "--format", "json", "--batch"))
        ok, parsed = _parsed_json(plan.stdout)
        actions: dict[int, str] = {}
        if ok and isinstance(parsed, dict):
            plan_obj = parsed.get("plan")
            if isinstance(plan_obj, dict):
                decisions = plan_obj.get("per_pr_actions")
                if isinstance(decisions, list):
                    for decision in decisions:
                        if isinstance(decision, dict):
                            number = decision.get("pr")
                            action = decision.get("action")
                            if isinstance(number, int) and isinstance(action, str):
                                actions[number] = action
        expected_actions = {
            1: "land-now",
            2: "rebase-then-land",
            3: "refire-stale-gate",
            4: "wait",
            5: "hold-fix",
            6: "refire-ci",
            7: "refire-ci",
            8: "hold-fix",
            9: "escalate-gate-policy",
            10: "wait",
            11: "wait",
            12: "wait",
            13: "wait",
            14: "wait",
        }
        if plan.returncode == 0 and all(actions.get(number) == action for number, action in expected_actions.items()):
            rep.ok("representative:safety-actions")
        else:
            rep.bad("representative:safety-actions", f"expected={expected_actions}; got={actions}")

        if ok and isinstance(parsed, dict):
            nodes = parsed.get("nodes")
            held = parsed.get("held_prs")
            plan_obj = parsed.get("plan")
            node_schema_ok = isinstance(nodes, list) and all(
                isinstance(node, dict)
                and "base_sha" in node
                and "review_decision" in node
                for node in nodes
            )
            held_reasons: dict[int, list[str]] = {}
            if isinstance(held, list):
                for item in held:
                    if not isinstance(item, dict):
                        continue
                    number = item.get("pr")
                    reasons = item.get("reasons")
                    if (
                        isinstance(number, int)
                        and isinstance(reasons, list)
                        and all(isinstance(reason, str) for reason in reasons)
                    ):
                        held_reasons[number] = reasons
            batch_values = plan_obj.get("batch") if isinstance(plan_obj, dict) else None
            safety_schema_ok = (
                node_schema_ok
                and "review-required" in held_reasons.get(10, [])
                and "changes-requested" in held_reasons.get(11, [])
                and "ordering-cycle" in held_reasons.get(13, [])
                and "ordering-cycle" in held_reasons.get(14, [])
                and isinstance(batch_values, list)
                and not any(number in batch_values for number in (12, 13, 14))
            )
            if safety_schema_ok:
                rep.ok("representative:safety-schema")
            else:
                rep.bad(
                    "representative:safety-schema",
                    f"node_schema={node_schema_ok}; held={held_reasons}; batch={batch_values}",
                )
        else:
            rep.bad("representative:safety-schema", "representative plan was not JSON")

        _record_same_exit(rep, "live:repo-required", py, rs, ("plan",), 2)
        _record_same_exit(
            rep,
            "cli:abbreviation-refused",
            py,
            rs,
            ("plan", "--fixt", fixture_path),
            2,
        )
        _record_same_exit(
            rep,
            "fixture:unknown-detector",
            py,
            rs,
            ("plan", "--fixture", fixture_path, "--conflict-detector", "unknown"),
            2,
        )
        invalid_fixtures: tuple[tuple[str, str], ...] = (
            ("root-array.json", "[]"),
            ("missing-prs.json", '{"repo":"R","base":"main"}'),
            ("prs-not-list.json", '{"repo":"R","base":"main","prs":{}}'),
            ("number-not-int.json", '{"repo":"R","base":"main","prs":[{"number":"one"}]}'),
            ("malformed.json", "{"),
            ("malformed.yaml", "prs:\n  - number: [\n"),
            ("duplicate-key.json", '{"prs":[],"prs":[]}'),
            ("duplicate-key.yaml", "prs: []\nprs: []\n"),
            ("merge-key.yaml", "base: &b {repo: R}\n<<: *b\nprs: []\n"),
            ("duplicate-pr.json", '{"prs":[{"number":1},{"number":1}]}'),
            ("zero-pr.json", '{"prs":[{"number":0}]}'),
            ("overflow-pr.json", '{"prs":[{"number":9223372036854775808}]}'),
            ("negative-behind.json", '{"prs":[{"number":1,"commits_behind":-1}]}'),
            (
                "unknown-conflict.json",
                '{"prs":[{"number":1}],"conflicts":[{"a":1,"b":2}]}',
            ),
            (
                "self-ancestry.json",
                '{"prs":[{"number":1}],"ancestry":[{"before":1,"after":1}]}',
            ),
            ("relations-shape.json", '{"prs":[{"number":1}],"conflicts":{}}'),
        )
        for name, body in invalid_fixtures:
            path = os.path.join(tmp, name)
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(body)
            _record_same_exit(rep, f"reject:{name}", py, rs, ("plan", "--fixture", path), 2)

        bad_contexts = (
            (
                "head-only-context.json",
                '{"prs":[{"pr":1,"head_sha":"sha-1","validation_evidence":"clean-validate-record"}]}',
            ),
            (
                "stale-base-context.json",
                '{"prs":[{"pr":1,"head_sha":"sha-1","base_sha":"old-base","validation_evidence":"clean-validate-record"}]}',
            ),
        )
        for name, body in bad_contexts:
            path = os.path.join(tmp, name)
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(body)
            _record_same_exit(
                rep,
                f"reject:{name}",
                py,
                rs,
                ("plan", "--fixture", fixture_path, "--landing-context", path),
                2,
            )

        for flag, value in (
            ("--outage-min-prs", "-1"),
            ("--freshness-max-behind", "-1"),
            ("--freshness-max-behind", "9223372036854775808"),
            ("--prs", "0"),
            ("--prs", "9223372036854775808"),
        ):
            _record_same_exit(
                rep,
                f"reject:cli-domain/{flag}={value}",
                py,
                rs,
                ("plan", "--fixture", fixture_path, flag, value),
                2,
            )

        rng = random.Random(seed)
        conclusions = (
            ("COMPLETED", "SUCCESS"),
            ("IN_PROGRESS", ""),
            ("COMPLETED", "FAILURE"),
            ("COMPLETED", "CANCELLED"),
        )
        for index in range(rand_count):
            prs: list[object] = []
            conflicts: list[object] = []
            count = rng.randint(1, 8)
            for number in range(1, count + 1):
                status, conclusion = rng.choice(conclusions)
                gate_status, gate_conclusion = rng.choice(conclusions)
                checks: list[object] = [
                    {
                        "name": "CI",
                        "status": status,
                        "conclusion": conclusion,
                        "text": "assertion" if conclusion == "FAILURE" else "",
                    },
                    {
                        "name": "merge-gate",
                        "status": gate_status,
                        "conclusion": gate_conclusion,
                        "text": "stale" if gate_conclusion == "FAILURE" else "",
                    },
                ]
                prs.append(
                    {
                        "number": number,
                        "head_ref": f"feature-{number}",
                        "base_ref": (
                            f"feature-{number - 1}"
                            if number > 1 and rng.random() < 0.2
                            else "main"
                        ),
                        "title": f"random {index}/{number}",
                        "is_draft": rng.random() < 0.1,
                        "review_decision": rng.choice(("", "APPROVED", "REVIEW_REQUIRED")),
                        "additions": rng.randint(0, 100),
                        "deletions": rng.randint(0, 100),
                        "commits_behind": rng.randint(0, 4),
                        "labels": [f"priority:{rng.randint(0, 3)}"],
                        "changed_files": [f"src/{rng.randint(0, 4)}.rs"],
                        "checks": checks,
                    }
                )
                if number > 1 and rng.random() < 0.25:
                    conflicts.append(
                        {"a": rng.randint(1, number - 1), "b": number, "paths": ["src/shared.rs"]}
                    )
            random_fixture = {
                "repo": "R",
                "base": "main",
                "prs": prs,
                "conflicts": conflicts,
            }
            path = os.path.join(tmp, f"random-{index}.json")
            with open(path, "w", encoding="utf-8") as handle:
                json.dump(random_fixture, handle)
            extra: tuple[str, ...] = (
                "--batch",
                "--freshness-max-behind",
                str(rng.randint(0, 3)),
                "--conflict-detector",
                rng.choice(("merge-tree", "file-overlap")),
            )
            _record_exact(
                rep,
                f"random:{index}/plan-json",
                py,
                rs,
                ("plan", "--fixture", path, "--format", "json", *extra),
            )

    if rep.failures:
        for failure in rep.failures:
            print(f"DIVERGENCE [{failure}]")
        print(f"cross[{tool}]: {len(rep.failures)} divergence(s) out of {rep.checks} checks")
        return 1
    print(f"cross[{tool}]: OK - {rep.checks} graph, CI, randomized, and adversarial checks agree")
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="py-vs-rs differential tester")
    parser.add_argument(
        "--tool",
        default="safe-ci-dag-runner",
        choices=(
            "safe-ci-dag-runner",
            "cpuset-alloc",
            "tick-hub",
            "pr-landing-planner",
            "herdr-run",
            "herdr-agent",
            "all",
        ),
    )
    parser.add_argument("--random", type=int, default=24, help="number of randomized fixtures")
    parser.add_argument("--seed", type=int, default=1234, help="RNG seed for randomized fixtures")
    ns = parser.parse_args(list(argv) if argv is not None else None)
    tool = str(ns.tool)
    rand_count = int(ns.random)
    seed = int(ns.seed)
    if tool == "safe-ci-dag-runner":
        return compare_safe_ci_dag_runner(rand_count, seed)
    if tool == "cpuset-alloc":
        return compare_cpuset_alloc()
    if tool == "tick-hub":
        return compare_tick_hub(rand_count, seed)
    if tool == "pr-landing-planner":
        return compare_pr_landing_planner(rand_count, seed)
    if tool == "herdr-run":
        return compare_herdr_run(py_command_for(tool), rs_command(tool))
    if tool == "herdr-agent":
        return compare_herdr_agent(py_command_for(tool), rs_command(tool))
    results = (
        compare_safe_ci_dag_runner(rand_count, seed),
        compare_cpuset_alloc(),
        compare_tick_hub(rand_count, seed),
        compare_pr_landing_planner(rand_count, seed),
        compare_herdr_run(py_command_for("herdr-run"), rs_command("herdr-run")),
        compare_herdr_agent(py_command_for("herdr-agent"), rs_command("herdr-agent")),
    )
    return 1 if any(results) else 0


if __name__ == "__main__":
    raise SystemExit(main())
