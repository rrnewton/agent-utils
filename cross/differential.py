#!/usr/bin/env python3
"""Python-vs-Rust differential tester for safe-ci-dag-runner.

Prove the Python and Rust builds produce identical OBSERVABLE behavior. For a set of
representative and randomized DAG fixtures, this runs BOTH the Python CLI
(``python3 -m safe_ci_dag_runner``) and the Rust binary (``rs/target/release/…`` or
``rs/bin/…``) and asserts:

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
* ``run`` agrees on exit code, and on the passed/failed/aborted/skipped counts. Counts are
  compared under ``--keep-going`` so they are deterministic (the default eager-exit path
  races on which in-flight step is cancelled first, so only its exit code is compared).
* ``--only`` selection (Feature A) agrees: running EXACTLY one named step matches on exit code
  and counts, and an unknown ``--only`` tag exits 2 on both builds. (The ``sweep`` timing table
  and the ``--profile`` table are NOT byte-compared — runtimes legitimately differ across the two
  languages.)
* The memory-aware ``-j`` decision and modeled footprint from ``--max-mem`` match.
* The auto-logging profile STORE (Feature D) has an identical on-disk schema across builds: an
  unboxed run under each build (into separate ``--perf-dir`` dirs) writes the SAME set of CSV
  filenames — so ``machine_id`` + ``container_class`` (and hence ``nproc``) agree — with
  byte-identical HEADER rows and the SAME line-ending style. Data rows (timestamps, elapsed, git
  SHA) legitimately differ and are not compared. The dynamic cgroup ``cpu.*`` columns only appear
  under boxing (out of scope for the unboxed differential); their alphabetical ordering is pinned
  by each build's own perflog tests.
* The ``sweep`` ``--jobs`` error text (malformed range / not-an-integer) matches across builds.
* The profile-store FEEDBACK loop + ``--planner`` agree (``compare_plan_feedback``): against a FIXED
  synthetic store, ``plan`` output is byte-identical across builds for BOTH planners and BOTH formats
  (so the contention-discounted median durations, high-percentile rss estimates, and dispatch order
  all match); the ``critical-path`` order differs from ``greedy-lpt`` (the planner really reorders);
  the hint-only ``--no-profile-feedback`` plan is also identical; and the ``--max-mem`` sizing fed by
  the store's rss estimates matches across builds and throttles below the CPU count.
* The remaining ``run`` comparisons pass ``--no-profile`` (no store WRITE into the harness CWD) and
  ``--no-profile-feedback`` (no store READ / hint refinement), so the base scheduling behavior under
  test stays hermetic and hint-only; feedback parity is asserted separately (above).

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
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field

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


def _env(extra: Mapping[str, str] | None = None) -> dict[str, str]:
    env = dict(os.environ)
    env["PYTHONPATH"] = os.path.join(REPO_ROOT, "py") + os.pathsep + env.get("PYTHONPATH", "")
    # Deterministic, color-free output regardless of the runner's TTY state.
    env["NO_COLOR"] = "1"
    if extra:
        env.update(extra)
    return env


def run(
    cmd: Sequence[str], args: Sequence[str], extra_env: Mapping[str, str] | None = None
) -> Outcome:
    proc = subprocess.run(
        [*cmd, *args],
        capture_output=True,
        text=True,
        env=_env(extra_env),
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

    # jobs_flag: the inner-parallelism flag appended to a step's command when it declares
    # preferred_inner_jobs. Each fixture's command PASSES iff exactly the expected appended token(s)
    # arrive as "$*", so a serial run (-j1) yields 1-passed in BOTH builds only when Python and
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
        po = run(py, ("run", "--dag", dag_path, "-q", "-j", "4", NOPROF, NOFB, ACF))
        ro = run(rs, ("run", "--dag", dag_path, "-q", "-j", "4", NOPROF, NOFB, ACF))
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
        po = run(py, ("run", "--dag", dag_path, "-q", "-j", "1", NOPROF, NOFB, ACF))
        ro = run(rs, ("run", "--dag", dag_path, "-q", "-j", "1", NOPROF, NOFB, ACF))
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
            if ps is None or rss is None:
                rep.bad(label, f"missing sizing line py={po.stderr!r} rs={ro.stderr!r}")
            elif ps != rss:
                rep.bad(label, f"(-j, footprint, budget) py={ps} rs={rss}")
            else:
                rep.ok(label)

        # 6) --only selection parity (Feature A): running EXACTLY the named step(s) must agree on
        # exit code AND the passed/failed/aborted/skipped counts across both builds. Selecting a
        # single step at -j1 is deterministic (its deps outside the selection are dropped), so the
        # counts are reproducible even though full-DAG timing is not.
        tag = _first_tag(fx)
        if tag is not None:
            po = run(py, ("run", "--dag", dag_path, "-q", "-j", "1", "--only", tag, NOPROF, NOFB, ACF))
            ro = run(rs, ("run", "--dag", dag_path, "-q", "-j", "1", "--only", tag, NOPROF, NOFB, ACF))
            label = f"{fx.name}/only({tag})"
            pc, rc = _counts(po.stderr), _counts(ro.stderr)
            if po.returncode != ro.returncode:
                rep.bad(label, f"exit py={po.returncode} rs={ro.returncode}")
            elif pc is None or rc is None:
                rep.bad(label, f"missing summary counts py={po.stderr!r} rs={ro.stderr!r}")
            elif pc != rc:
                rep.bad(label, f"counts py={pc} rs={rc}")
            elif pc != (1, 0, 0, 0) and pc != (0, 1, 0, 0):
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


def compare_profile_store(py: list[str], rs: list[str], rep: Report) -> None:
    """Assert the auto-logging profile STORE (Feature D) has an identical on-disk schema in both
    builds. Runs the SAME tiny DAG under each build with ``--perf-dir`` into a fresh temp dir
    (unboxed via ``--allow-cgroup-failure``, so it is environment-independent), then asserts the two
    stores agree on: (a) the SET of CSV filenames (proving ``machine_id`` + ``container_class``, and
    hence ``nproc``, agree), (b) each file's HEADER row byte-for-byte, and (c) the line-ending
    style. Data rows are NOT compared (their timestamps/elapsed/git-SHA legitimately differ)."""
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
        po = run(py, ("run", "--dag", dag_path, "-q", "-j", "1", "--perf-dir", py_dir, NOFB, ACF))
        ro = run(rs, ("run", "--dag", dag_path, "-q", "-j", "1", "--perf-dir", rs_dir, NOFB, ACF))
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
#: -j1). Written with the pinned SYNTH identity so the file name matches what the reader loads.
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
#:   h.ws elapsed [' 8.0 '->8, 4.0, '10.0' under ' 50.0 '% -> 5] => robust median 5.000; peaks
#:        [1000,2000,3000] => p90 3000.
#:   h.us elapsed ['1_0.0' rejected, 4.0, 6.0] => robust median 5.000; peaks ['1_000' rejected,
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
        #   both hostile steps must derive est_duration 5.000 FROM THE STORE (not the 99.0 hint).
        if '"est_source": "store"' in po.stdout and po.stdout.count('"est_duration_s": "5.000"') == 2:
            rep.ok("hostile-cells:trim-applied")
        else:
            rep.bad(
                "hostile-cells:trim-applied",
                "expected both steps to learn est_duration 5.000 from the trimmed store cells; "
                f"got\n{po.stdout}",
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

        # Memory-aware sizing fed by the store's rss estimates: both builds must pick the same -j
        # and throttle below the CPU count (the 6 GiB heavy+solo pair overflows an 8 GiB budget).
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
            rep.bad("plan-feedback:sizing", f"(-j, footprint, budget) py={ps} rs={rss}")
        elif ps[0] != 1:
            rep.bad(
                "plan-feedback:sizing",
                f"expected the store's rss estimates to throttle to -j1; got -j{ps[0]}",
            )
        else:
            rep.ok("plan-feedback:sizing")


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
    examples = example_fixtures()
    for fx in examples:
        compare_example_static(py, rs, fx, rep)
    compare_yaml_isomorphism(py, rs, rep)
    compare_scalar_parity(py, rs, rep)
    compare_only_errors(py, rs, rep)
    compare_profile_store(py, rs, rep)
    compare_plan_feedback(py, rs, rep)
    compare_hostile_numeric_cells(py, rs, rep)
    compare_sweep_errors(py, rs, rep)
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
