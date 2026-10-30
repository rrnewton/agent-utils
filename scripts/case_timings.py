#!/usr/bin/env python3
"""One normalized per-case timing schema for every validation runner.

Dagrun's profile rows time whole gates. The cases inside a gate are known
only to its runner, and each runner reports them differently: pytest by
nodeid, libtest by test name, Playwright by spec, node:test by TAP line, the
cross-language differential by check label, and the package and example
harnesses by the project or example they are checking. This module is the
single record those shapes are normalized into, so one report can rank cases
across runners and prove a run's cases match its collected inventory exactly.

The schema (``agent-utils-case-timing/v1``) is one JSON object per runner
invocation::

    {"schema": "agent-utils-case-timing/v1", "runner": "pytest",
     "gate": "python.dagrun.shard-0", "nested": false,
     "inventory": ["tests/test_a.py::test_one"],
     "cases": [{"case_id": "tests/test_a.py::test_one",
                "outcome": "pass", "duration_s": 0.25}]}

``gate`` is the owning validation step (``DAGRUN_OUTER_RUN`` inside the
graph). ``outcome`` is one of ``pass``, ``fail``, ``skip``, ``error``.
``duration_s`` is elapsed case time in seconds, or ``null`` when the runner
cannot attribute time to a case; a record never invents a duration.

Who emits, and how:

* **pytest** (component shards, lifecycle shards, separate suites) — native:
  ``scripts/run_pytest_shard.py`` and ``scripts/run_component_tests.py``
  collect nodeids, outcomes, and summed stage durations when
  ``AGENT_UTILS_CASE_TIMING_DIR`` names a directory.
* **cross-language differential** — native: ``cross/differential.py`` times
  each ``Report`` check between successive verdicts and emits at its verdict.
* **examples** — native: ``scripts/check_dagrun_examples.py`` times each
  example (an expected fixed-resource refusal is a ``skip``: it did not run).
* **packages** — native: ``scripts/check_python_packages.py`` and
  ``scripts/check_rust_packages.py`` time each checked project or crate.
* **JavaScript/browser** — Playwright's JSON reporter drops
  ``playwright-<gate>.json`` into the same directory (see
  ``validation/timeline.dag.yaml``) and the loader below normalizes it;
  node:test TAP output normalizes through ``ingest-node-tap``.
* **Rust/libtest and application (vibe-talk) cargo suites** — libtest's
  stable output carries no per-case durations, so their gates run through
  ``run-cargo`` below: it builds with ``cargo test --no-run``, executes each
  test binary's JSON event stream (timestamped on arrival, so durations are
  real per-case intervals), inventories each binary with ``--list``, and runs
  doctests as their own cases. ``ingest-libtest-json`` remains for ad-hoc
  streams.

A record whose cases were produced by a nested alternate-environment rerun
carries ``"nested": true``. The verifier checks every record's cases against
its own inventory exactly, then counts each ``(runner, gate, case_id)`` once
across primary records; nested records are reported, never double-counted.

Bounds: a record file over ``MAX_FILE_BYTES`` and a directory over
``MAX_FILES`` files are problems, not work items, and the rendered report
prints per-runner totals plus at most ``slowest`` case lines, so report size
and verification cost stay bounded however many cases a run produces.

Examples::

    python3 scripts/case_timings.py --self-test
    python3 scripts/case_timings.py report --dir DIR
    python3 scripts/case_timings.py ingest-junit results.xml --runner pytest --gate g --out DIR
    some-test-binary -Z unstable-options --format json | \\
        python3 scripts/case_timings.py ingest-libtest-json --gate rust.dagrun.test --out DIR
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import time
import xml.etree.ElementTree as ElementTree
from collections.abc import Iterable, Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path


SCHEMA = "agent-utils-case-timing/v1"
TIMING_DIR_ENV = "AGENT_UTILS_CASE_TIMING_DIR"
TIMING_NESTED_ENV = "AGENT_UTILS_CASE_TIMING_NESTED"
GATE_ENV = "DAGRUN_OUTER_RUN"
OUTCOMES = frozenset({"pass", "fail", "skip", "error"})

#: Bounds that keep one verification pass cheap and its report small.
MAX_FILE_BYTES = 16 << 20
MAX_FILES = 4096
MAX_CASES_PER_FILE = 200_000

_write_counter = 0


@dataclass(frozen=True)
class CaseRecord:
    """One case in the normalized schema."""

    runner: str
    gate: str
    case_id: str
    outcome: str
    duration_s: float | None


@dataclass(frozen=True)
class TimingReport:
    """The verified aggregate over one directory of timing records."""

    problems: list[str] = field(default_factory=list)
    primary_cases: tuple[CaseRecord, ...] = ()
    nested_count: int = 0
    files: int = 0
    #: Expected gates that left no primary record. Unlike a mismatched
    #: record (a problem), a missing one means coverage could not be
    #: confirmed — the PARTIAL shape cross coverage already uses.
    missing_gates: tuple[str, ...] = ()

    @property
    def primary_count(self) -> int:
        return len(self.primary_cases)


def requested_directory() -> Path | None:
    """The directory emission was asked for, or None when it was not."""

    raw = os.environ.get(TIMING_DIR_ENV)
    return Path(raw) if raw else None


def current_gate() -> str:
    """The owning validation gate for this process, or ``top-level``."""

    return os.environ.get(GATE_ENV) or "top-level"


def is_nested() -> bool:
    """Whether this process is a nested alternate-environment rerun."""

    return os.environ.get(TIMING_NESTED_ENV) == "1"


def _safe_name(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "-", value).strip("-") or "run"


def write_records(
    directory: Path,
    *,
    runner: str,
    gate: str,
    inventory: Sequence[str],
    cases: Sequence[CaseRecord],
    nested: bool = False,
) -> Path:
    """Write one runner invocation's record atomically into ``directory``."""

    global _write_counter
    _write_counter += 1
    directory.mkdir(parents=True, exist_ok=True)
    record: dict[str, object] = {
        "schema": SCHEMA,
        "runner": runner,
        "gate": gate,
        "nested": nested,
        "inventory": list(inventory),
        "cases": [
            {
                "case_id": case.case_id,
                "outcome": case.outcome,
                "duration_s": case.duration_s,
            }
            for case in cases
        ],
    }
    target = directory / f"{_safe_name(runner)}-{_safe_name(gate)}-{os.getpid()}-{_write_counter}.json"
    fd, temporary = tempfile.mkstemp(prefix=".case-timing-", suffix=".tmp", dir=directory)
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as handle:
            json.dump(record, handle, sort_keys=True)
            handle.write("\n")
        os.replace(temporary, target)
    except BaseException:
        try:
            os.unlink(temporary)
        except OSError:
            pass
        raise
    return target


def emit_if_requested(
    *,
    runner: str,
    inventory: Sequence[str],
    cases: Sequence[CaseRecord],
    gate: str | None = None,
    nested: bool | None = None,
) -> Path | None:
    """Emit one record when ``TIMING_DIR_ENV`` asked for it; fail hard if it cannot.

    A caller that summarises these records would otherwise report a complete
    inventory for a runner whose evidence never landed, so a write that was
    asked for and fails raises instead of being swallowed.
    """

    directory = requested_directory()
    if directory is None:
        return None
    try:
        return write_records(
            directory,
            runner=runner,
            gate=gate if gate is not None else current_gate(),
            inventory=inventory,
            cases=cases,
            nested=is_nested() if nested is None else nested,
        )
    except OSError as exc:
        raise SystemExit(
            f"case-timings[{runner}]: cannot write the timing record "
            f"{TIMING_DIR_ENV}={directory} asked for: {exc}"
        ) from exc


# ---------------------------------------------------------------- adapters


def _duration(value: object) -> float | None:
    if isinstance(value, bool):
        return None
    if isinstance(value, (int, float)):
        return float(value) if value >= 0 else None
    if isinstance(value, str):
        try:
            parsed = float(value)
        except ValueError:
            return None
        return parsed if parsed >= 0 else None
    return None


def cases_from_junit(text: str, *, runner: str, gate: str) -> list[CaseRecord]:
    """Normalize JUnit XML (pytest's xunit2 shape and libtest's junit shape).

    Pytest's shard runner records the canonical nodeid as a testcase
    property; when present it wins over ``classname::name`` reconstruction.
    """

    root = ElementTree.fromstring(text)
    cases: list[CaseRecord] = []
    for element in root.iter("testcase"):
        case_id: str | None = None
        for prop in element.iter("property"):
            if prop.get("name") == "nodeid" and prop.get("value"):
                case_id = prop.get("value")
        if case_id is None:
            classname = element.get("classname") or ""
            name = element.get("name") or ""
            case_id = f"{classname}::{name}" if classname else name
        if element.find("failure") is not None:
            outcome = "fail"
        elif element.find("error") is not None:
            outcome = "error"
        elif element.find("skipped") is not None:
            outcome = "skip"
        else:
            outcome = "pass"
        cases.append(
            CaseRecord(runner, gate, case_id, outcome, _duration(element.get("time")))
        )
    return cases


def cases_from_libtest_events(
    events: Iterable[tuple[float, Mapping[str, object]]], *, gate: str
) -> list[CaseRecord]:
    """Normalize libtest JSON events paired with their arrival timestamps.

    Stable libtest output has no per-case duration; the JSON event stream
    (nightly ``-Z unstable-options --format json``) has start/finish events,
    so the ingest pipe timestamps arrival and a case's duration is its
    finish-minus-start interval. Ignored tests emit a start too.
    """

    started: dict[str, float] = {}
    cases: list[CaseRecord] = []
    for arrived, event in events:
        if event.get("type") != "test":
            continue
        name = event.get("name")
        if not isinstance(name, str):
            continue
        kind = event.get("event")
        if kind == "started":
            started[name] = arrived
            continue
        outcome = {"ok": "pass", "failed": "fail", "ignored": "skip"}.get(
            kind if isinstance(kind, str) else ""
        )
        if outcome is None:
            continue
        begin = started.pop(name, None)
        duration = arrived - begin if begin is not None and arrived >= begin else None
        cases.append(CaseRecord("libtest", gate, name, outcome, duration))
    return cases


def _playwright_specs(suite: Mapping[str, object]) -> Iterable[Mapping[str, object]]:
    specs = suite.get("specs")
    if isinstance(specs, list):
        for spec in specs:
            if isinstance(spec, dict):
                yield spec
    nested = suite.get("suites")
    if isinstance(nested, list):
        for child in nested:
            if isinstance(child, dict):
                yield from _playwright_specs(child)


def cases_from_playwright(payload: Mapping[str, object], *, gate: str) -> list[CaseRecord]:
    """Normalize Playwright's JSON reporter output (milliseconds to seconds)."""

    status_map = {
        "passed": "pass",
        "failed": "fail",
        "timedOut": "fail",
        "skipped": "skip",
        "interrupted": "error",
    }
    cases: list[CaseRecord] = []
    suites = payload.get("suites")
    if not isinstance(suites, list):
        return cases
    for suite in suites:
        if not isinstance(suite, dict):
            continue
        for spec in _playwright_specs(suite):
            title = spec.get("title")
            file_name = spec.get("file")
            line = spec.get("line")
            if not isinstance(title, str) or not isinstance(file_name, str):
                continue
            case_id = f"{file_name}:{line} {title}" if isinstance(line, int) else f"{file_name} {title}"
            outcome = "error"
            duration_ms = 0.0
            seen_result = False
            tests = spec.get("tests")
            if isinstance(tests, list):
                for test in tests:
                    if not isinstance(test, dict):
                        continue
                    results = test.get("results")
                    if not isinstance(results, list):
                        continue
                    for result in results:
                        if not isinstance(result, dict):
                            continue
                        seen_result = True
                        status = result.get("status")
                        if isinstance(status, str) and status in status_map:
                            outcome = status_map[status]
                        millis = _duration(result.get("duration"))
                        if millis is not None:
                            duration_ms += millis
            cases.append(
                CaseRecord(
                    "playwright",
                    gate,
                    case_id,
                    outcome if seen_result else "error",
                    duration_ms / 1000.0 if seen_result else None,
                )
            )
    return cases


_TAP_RESULT = re.compile(r"^(not )?ok \d+ - (.+)$")
_TAP_DURATION = re.compile(r"^\s+duration_ms:\s*([0-9.]+)\s*$")


def cases_from_node_tap(text: str, *, gate: str) -> list[CaseRecord]:
    """Normalize node:test TAP output, including its ``duration_ms`` blocks."""

    cases: list[CaseRecord] = []
    for line in text.splitlines():
        result = _TAP_RESULT.match(line)
        if result is not None:
            name = result.group(2).strip()
            if " # SKIP" in name:
                name = name.split(" # SKIP", 1)[0].strip()
                outcome = "skip"
            elif result.group(1):
                outcome = "fail"
            else:
                outcome = "pass"
            cases.append(CaseRecord("node-test", gate, name, outcome, None))
            continue
        duration = _TAP_DURATION.match(line)
        if duration is not None and cases:
            last = cases[-1]
            cases[-1] = CaseRecord(
                last.runner, last.gate, last.case_id, last.outcome, float(duration.group(1)) / 1000.0
            )
    return cases


# ---------------------------------------------------------------- verifier


@dataclass(frozen=True)
class _RunFile:
    source: str
    runner: str
    gate: str
    nested: bool
    inventory: tuple[str, ...]
    cases: tuple[CaseRecord, ...]


def _load_run_file(path: Path, problems: list[str]) -> _RunFile | None:
    """Read one record file, appending a problem instead of raising."""

    source = path.name
    try:
        size = path.stat().st_size
    except OSError as exc:
        problems.append(f"{source}: cannot stat timing record: {exc}")
        return None
    if size > MAX_FILE_BYTES:
        problems.append(f"{source}: timing record is {size} bytes, over the {MAX_FILE_BYTES} bound")
        return None
    try:
        value: object = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, UnicodeError, json.JSONDecodeError) as exc:
        problems.append(f"{source}: unreadable timing record: {exc}")
        return None
    if source.startswith("playwright-") and isinstance(value, dict) and "suites" in value:
        gate = source[len("playwright-") : -len(".json")]
        playwright_cases = cases_from_playwright(value, gate=gate)
        return _RunFile(
            source,
            "playwright",
            gate,
            False,
            tuple(case.case_id for case in playwright_cases),
            tuple(playwright_cases),
        )
    if not isinstance(value, dict):
        problems.append(f"{source}: timing record is not an object: {value!r}")
        return None
    if value.get("schema") != SCHEMA:
        problems.append(f"{source}: timing record schema is {value.get('schema')!r}, expected {SCHEMA!r}")
        return None
    runner = value.get("runner")
    gate_value = value.get("gate")
    nested = value.get("nested", False)
    inventory_raw = value.get("inventory")
    cases_raw = value.get("cases")
    if (
        not isinstance(runner, str)
        or not isinstance(gate_value, str)
        or not isinstance(nested, bool)
        or not isinstance(inventory_raw, list)
        or not all(isinstance(item, str) for item in inventory_raw)
        or not isinstance(cases_raw, list)
    ):
        problems.append(f"{source}: malformed timing record header: {value!r}")
        return None
    if len(cases_raw) > MAX_CASES_PER_FILE:
        problems.append(
            f"{source}: {len(cases_raw)} cases, over the {MAX_CASES_PER_FILE} per-file bound"
        )
        return None
    cases: list[CaseRecord] = []
    for item in cases_raw:
        if not isinstance(item, dict):
            problems.append(f"{source}: malformed case entry: {item!r}")
            return None
        case_id = item.get("case_id")
        outcome = item.get("outcome")
        duration = _duration(item.get("duration_s"))
        if not isinstance(case_id, str) or not isinstance(outcome, str) or outcome not in OUTCOMES:
            problems.append(f"{source}: malformed case entry: {item!r}")
            return None
        if item.get("duration_s") is not None and duration is None:
            problems.append(f"{source}: case {case_id} has an invalid duration: {item!r}")
            return None
        cases.append(CaseRecord(runner, gate_value, case_id, outcome, duration))
    inventory = tuple(item for item in inventory_raw if isinstance(item, str))
    return _RunFile(source, runner, gate_value, nested, inventory, tuple(cases))


def _load_tap_file(path: Path, problems: list[str]) -> _RunFile | None:
    """Normalize one ``node-<gate>.tap`` file, checked against its TAP plan."""

    source = path.name
    try:
        size = path.stat().st_size
        text = path.read_text(encoding="utf-8") if size <= MAX_FILE_BYTES else ""
    except (OSError, UnicodeError) as exc:
        problems.append(f"{source}: unreadable TAP record: {exc}")
        return None
    if size > MAX_FILE_BYTES:
        problems.append(f"{source}: TAP record is {size} bytes, over the {MAX_FILE_BYTES} bound")
        return None
    gate = source[len("node-") : -len(".tap")]
    cases = cases_from_node_tap(text, gate=gate)
    plan: int | None = None
    for line in text.splitlines():
        stripped = line.strip()
        if stripped.startswith("1.."):
            try:
                plan = int(stripped[3:].split()[0])
            except (ValueError, IndexError):
                plan = None
    if plan is not None and plan != len(cases):
        problems.append(
            f"{source}: TAP plan is {plan} case(s) but {len(cases)} result(s) were reported"
        )
    return _RunFile(
        source, "node-test", gate, False, tuple(case.case_id for case in cases), tuple(cases)
    )


def verify_directory(
    directory: Path, *, expected_gates: Sequence[str] = ()
) -> TimingReport:
    """Verify exact per-record inventory equality and build the primary aggregate.

    Every record must account for its own collected inventory exactly once.
    Across primary records a ``(runner, gate, case_id)`` may appear only
    once; nested records are inventory-checked too but their cases are
    counted separately and never enter the primary aggregate. Every gate in
    ``expected_gates`` must have left at least one primary record: an absent
    record is a runner whose evidence silently vanished, not a small report.
    """

    problems: list[str] = []
    paths = sorted(directory.glob("*.json")) if directory.is_dir() else []
    tap_paths = sorted(directory.glob("node-*.tap")) if directory.is_dir() else []
    if len(paths) > MAX_FILES:
        problems.append(f"{len(paths)} timing record files, over the {MAX_FILES} bound")
        paths = paths[:MAX_FILES]
    primary: dict[tuple[str, str, str], CaseRecord] = {}
    nested_count = 0
    loaded = 0
    primary_gates: set[str] = set()
    runs: list[_RunFile] = []
    for path in paths:
        loaded_run = _load_run_file(path, problems)
        if loaded_run is not None:
            runs.append(loaded_run)
    for path in tap_paths:
        loaded_tap = _load_tap_file(path, problems)
        if loaded_tap is not None:
            runs.append(loaded_tap)
    for run in runs:
        loaded += 1
        if len(set(run.inventory)) != len(run.inventory):
            problems.append(f"{run.source}: duplicate inventory entries in {run.gate}")
        seen: set[str] = set()
        for case in run.cases:
            if case.case_id in seen:
                problems.append(f"{run.source}: duplicate case {case.case_id} in {run.gate}")
            seen.add(case.case_id)
        inventory_set = set(run.inventory)
        for missing in sorted(inventory_set - seen):
            problems.append(f"{run.source}: missing case {missing} collected by {run.gate}")
        for extra in sorted(seen - inventory_set):
            problems.append(f"{run.source}: case {extra} is not in its inventory for {run.gate}")
        if run.nested:
            nested_count += len(run.cases)
            continue
        primary_gates.add(run.gate)
        for case in run.cases:
            key = (case.runner, case.gate, case.case_id)
            if key in primary:
                problems.append(
                    f"{run.source}: duplicate primary case {case.case_id} for "
                    f"{case.runner} gate {case.gate}"
                )
                continue
            primary[key] = case
    missing_gates = tuple(sorted(set(expected_gates) - primary_gates))
    ordered = tuple(sorted(primary.values(), key=lambda case: (case.runner, case.gate, case.case_id)))
    return TimingReport(problems, ordered, nested_count, loaded, missing_gates)


def render_report(report: TimingReport, *, slowest: int = 20) -> list[str]:
    """Bounded summary lines: totals per runner, then the slowest few cases."""

    lines = [
        f"case timings: {report.primary_count} primary case(s) in {report.files} record(s); "
        f"{report.nested_count} nested alternate-environment case(s) not double-counted"
    ]
    per_runner: dict[str, list[CaseRecord]] = {}
    for case in report.primary_cases:
        per_runner.setdefault(case.runner, []).append(case)
    for runner in sorted(per_runner):
        cases = per_runner[runner]
        total = sum(case.duration_s or 0.0 for case in cases)
        counts: dict[str, int] = {}
        for case in cases:
            counts[case.outcome] = counts.get(case.outcome, 0) + 1
        outcomes = ", ".join(
            f"{outcome} {counts[outcome]}"
            for outcome in ("pass", "fail", "skip", "error")
            if outcome in counts
        )
        lines.append(f"  {runner}: {len(cases)} case(s), {total:.3f}s summed [{outcomes}]")
    ranked = sorted(
        (case for case in report.primary_cases if case.duration_s is not None),
        key=lambda case: (-(case.duration_s or 0.0), case.runner, case.gate, case.case_id),
    )
    for case in ranked[: max(0, slowest)]:
        lines.append(
            f"  slowest {case.duration_s or 0.0:.3f}s {case.runner} {case.gate} {case.case_id}"
        )
    return lines


# ---------------------------------------------------------------- cargo runner


def _crate_name(package_id: object, fallback: str) -> str:
    if not isinstance(package_id, str):
        return fallback
    if "#" in package_id:
        fragment = package_id.rsplit("#", 1)[1]
        name = fragment.split("@", 1)[0] if "@" in fragment else fallback
        return name or fallback
    return package_id.split(" ", 1)[0] or fallback


def _stream_lines(process: subprocess.Popen[str]) -> Iterable[tuple[float, str]]:
    assert process.stdout is not None
    for line in process.stdout:
        yield time.monotonic(), line.rstrip("\n")


CACHEDIR_TAG = (
    "Signature: 8a477f597d28d172789f06886806bc55\n"
    "# This file is a cache directory tag created by agent-utils scripts/case_timings.py.\n"
    "# For information about cache directory tags see https://bford.info/cachedir/\n"
)


def _tag_created_target_parent(cargo_args: Sequence[str]) -> None:
    """Create a nested ``--target-dir``'s missing parent as a tagged cache directory.

    Cargo writes ``CACHEDIR.TAG`` only into the target directory it creates. Given
    ``--target-dir rs/target/validation-tests`` before ``rs/target`` exists, it creates
    ``rs/target`` untagged, and the tracked launchers' later ``cargo clean --target-dir rs/target``
    refuses it. A parent created here holds nothing but the target directory, so it is a cache.
    """

    target_dir: str | None = None
    for index, arg in enumerate(cargo_args):
        if arg == "--target-dir" and index + 1 < len(cargo_args):
            target_dir = cargo_args[index + 1]
        elif arg.startswith("--target-dir="):
            target_dir = arg.split("=", 1)[1]
    if not target_dir:
        return
    parent = Path(target_dir).parent
    if os.path.lexists(parent):
        return
    parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(
        "w", dir=parent, prefix=".CACHEDIR.TAG.", delete=False, encoding="utf-8"
    ) as handle:
        handle.write(CACHEDIR_TAG)
    os.replace(handle.name, parent / "CACHEDIR.TAG")


def _run_cargo(argv: Sequence[str], *, gate: str | None) -> int:
    """Run ``cargo test`` with per-case timings; see the module docstring.

    ``argv`` is the full cargo command after ``--``. Test-binary filters
    after a second ``--`` are forwarded to every binary (and to ``--list``,
    so the inventory is exactly what was selected to run).
    """

    if not argv or Path(argv[0]).name != "cargo" or list(argv[1:2]) != ["test"]:
        print("case-timings run-cargo: expected `cargo test ...` after --", file=sys.stderr)
        return 2
    rest = list(argv[2:])
    if "--" in rest:
        split = rest.index("--")
        cargo_args, test_args = rest[:split], rest[split + 1 :]
    else:
        cargo_args, test_args = rest, []
    resolved_gate = gate if gate is not None else current_gate()
    _tag_created_target_parent(cargo_args)

    build = subprocess.Popen(
        [argv[0], "test", *cargo_args, "--no-run", "--message-format", "json"],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
    )
    artifacts: list[tuple[str, str, str]] = []
    assert build.stdout is not None
    for line in build.stdout:
        try:
            message: object = json.loads(line)
        except json.JSONDecodeError:
            print(line, end="", file=sys.stderr)
            continue
        if not isinstance(message, dict):
            continue
        compiler = message.get("message")
        if isinstance(compiler, dict) and isinstance(compiler.get("rendered"), str):
            print(compiler["rendered"], end="", file=sys.stderr)
        executable = message.get("executable")
        profile = message.get("profile")
        target = message.get("target")
        if (
            isinstance(executable, str)
            and isinstance(profile, dict)
            and profile.get("test") is True
            and isinstance(target, dict)
            and isinstance(target.get("name"), str)
        ):
            target_name = target["name"]
            assert isinstance(target_name, str)
            artifacts.append(
                (_crate_name(message.get("package_id"), target_name), target_name, executable)
            )
    build.wait()
    if build.returncode != 0:
        return build.returncode

    cases: list[CaseRecord] = []
    inventory: list[str] = []
    failures: list[str] = []
    status = 0
    binary_env = dict(os.environ)
    binary_env["RUSTC_BOOTSTRAP"] = "1"
    for crate, target_name, executable in artifacts:
        prefix = f"{crate}/{target_name}::"
        listed = subprocess.run(
            [executable, "--list", "--test", *test_args],
            env=binary_env,
            capture_output=True,
            text=True,
            check=False,
        )
        listed_names = [
            line[: -len(": test")]
            for line in listed.stdout.splitlines()
            if line.endswith(": test")
        ]
        inventory.extend(f"{prefix}{name}" for name in listed_names)
        process = subprocess.Popen(
            [executable, "-Z", "unstable-options", "--format", "json", "--test", *test_args],
            env=binary_env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
        )
        events: list[tuple[float, Mapping[str, object]]] = []
        for arrived, line in _stream_lines(process):
            stripped = line.strip()
            parsed: object = None
            if stripped.startswith("{"):
                try:
                    parsed = json.loads(stripped)
                except json.JSONDecodeError:
                    parsed = None
            if isinstance(parsed, dict):
                events.append((arrived, parsed))
                if parsed.get("event") == "failed" and isinstance(parsed.get("stdout"), str):
                    failures.append(f"{prefix}{parsed.get('name')}:\n{parsed['stdout']}")
            elif line:
                print(line)
        process.wait()
        if process.returncode != 0:
            status = 1
        if events:
            for case in cases_from_libtest_events(events, gate=resolved_gate):
                cases.append(
                    CaseRecord(
                        "libtest", resolved_gate, f"{prefix}{case.case_id}", case.outcome, case.duration_s
                    )
                )
        elif listed_names:
            # The binary rejected the JSON stream (older libtest): rerun it
            # plainly and keep identities and outcomes without durations.
            rerun = subprocess.run(
                [executable, "--test", *test_args],
                env=binary_env,
                capture_output=True,
                text=True,
                check=False,
            )
            print(rerun.stdout, end="")
            print(rerun.stderr, end="", file=sys.stderr)
            if rerun.returncode != 0:
                status = 1
            for line in rerun.stdout.splitlines():
                match = re.match(r"^test (.+) \.\.\. (ok|FAILED|ignored)$", line.strip())
                if match is None:
                    continue
                outcome = {"ok": "pass", "FAILED": "fail", "ignored": "skip"}[match.group(2)]
                cases.append(
                    CaseRecord("libtest", resolved_gate, f"{prefix}{match.group(1)}", outcome, None)
                )

    doctest_list = subprocess.run(
        [argv[0], "test", *cargo_args, "--doc", "--", "--list", *test_args],
        capture_output=True,
        text=True,
        check=False,
    )
    doc_names = [
        line[: -len(": test")]
        for line in doctest_list.stdout.splitlines()
        if line.endswith(": test")
    ]
    if doc_names:
        inventory.extend(f"doc::{name}" for name in doc_names)
        doctors = subprocess.run(
            [argv[0], "test", *cargo_args, "--doc", "--", *test_args],
            capture_output=True,
            text=True,
            check=False,
        )
        print(doctors.stdout, end="")
        print(doctors.stderr, end="", file=sys.stderr)
        if doctors.returncode != 0:
            status = 1
        for line in doctors.stdout.splitlines():
            match = re.match(r"^test (.+) \.\.\. (ok|FAILED|ignored)$", line.strip())
            if match is None:
                continue
            # The run report suffixes compile-only doctests ("- compile",
            # "- compile fail"); --list does not. Strip to the listed name.
            doc_name = re.sub(r" - compile( fail)?$", "", match.group(1))
            outcome = {"ok": "pass", "FAILED": "fail", "ignored": "skip"}[match.group(2)]
            cases.append(
                CaseRecord("libtest", resolved_gate, f"doc::{doc_name}", outcome, None)
            )
    for failure in failures:
        print(failure, file=sys.stderr)
    passed = sum(1 for case in cases if case.outcome == "pass")
    failed = sum(1 for case in cases if case.outcome in {"fail", "error"})
    print(
        f"case-timings run-cargo: {len(cases)} case(s), {passed} passed, {failed} failed "
        f"across {len(artifacts)} test binar(ies)"
    )
    emit_if_requested(runner="libtest", inventory=inventory, cases=cases, gate=resolved_gate)
    return status


# ---------------------------------------------------------------- CLI


def family_weights(report: TimingReport) -> dict[str, float]:
    """Total recorded seconds per pytest family (node ID before ``[``)."""

    weights: dict[str, float] = {}
    for case in report.primary_cases:
        if case.runner != "pytest":
            continue
        family = case.case_id.split("[", 1)[0]
        weights[family] = weights.get(family, 0.0) + (case.duration_s or 0.0)
    return weights


def _weights(directory: Path, out: Path) -> int:
    """Write the shard weights ``scripts/run_pytest_shard.py`` balances by."""

    verified = verify_directory(directory)
    for problem in verified.problems:
        print(f"case timings: {problem}", file=sys.stderr)
    if verified.problems:
        return 1
    weights = family_weights(verified)
    if not weights:
        print("case-timings: no pytest cases in the timing records", file=sys.stderr)
        return 1
    recorded = sum(
        1 for case in verified.primary_cases if case.runner == "pytest"
    )
    payload = {
        "schema": "pytest-shard-weights/v1",
        "recorded_cases": recorded,
        "total_seconds": round(sum(weights.values()), 3),
        "families": {family: round(weights[family], 3) for family in sorted(weights)},
    }
    out.write_text(json.dumps(payload, indent=1, sort_keys=True) + "\n", encoding="utf-8")
    print(
        f"case-timings: wrote {len(weights)} family weights "
        f"({recorded} cases, {payload['total_seconds']} s) to {out}"
    )
    return 0


def _ingest(cases: list[CaseRecord], *, runner: str, gate: str, out: Path) -> int:
    if not cases:
        print("case-timings: no cases found in the input", file=sys.stderr)
        return 1
    path = write_records(
        out,
        runner=runner,
        gate=gate,
        inventory=[case.case_id for case in cases],
        cases=cases,
    )
    print(f"case-timings: wrote {len(cases)} case(s) to {path}")
    return 0


def _self_test() -> int:
    """Offline controls: every adapter, the verifier's refusals, and the bounds."""

    junit = cases_from_junit(
        '<testsuites><testsuite><testcase classname="m" name="t" time="0.5"/>'
        '<testcase classname="m" name="u" time="0"><skipped/></testcase>'
        "</testsuite></testsuites>",
        runner="pytest",
        gate="g",
    )
    assert [(case.case_id, case.outcome) for case in junit] == [
        ("m::t", "pass"),
        ("m::u", "skip"),
    ]
    libtest = cases_from_libtest_events(
        [
            (0.0, {"type": "test", "event": "started", "name": "a"}),
            (0.25, {"type": "test", "event": "ok", "name": "a"}),
        ],
        gate="g",
    )
    assert libtest[0].duration_s == 0.25
    tap = cases_from_node_tap("ok 1 - x\n  duration_ms: 10\n", gate="g")
    assert tap[0].duration_s == 0.01

    with tempfile.TemporaryDirectory(prefix="case-timings-self-test-") as raw:
        directory = Path(raw)
        write_records(
            directory,
            runner="pytest",
            gate="g",
            inventory=["a"],
            cases=[CaseRecord("pytest", "g", "a", "pass", 0.1)],
        )
        assert verify_directory(directory).problems == []
        write_records(
            directory,
            runner="pytest",
            gate="g",
            inventory=["a"],
            cases=[CaseRecord("pytest", "g", "a", "pass", 0.2)],
            nested=True,
        )
        report = verify_directory(directory)
        assert report.problems == [] and report.primary_count == 1 and report.nested_count == 1
        assert family_weights(report) == {"a": 0.1}
        (directory / "bad.json").write_text(
            json.dumps(
                {
                    "schema": SCHEMA,
                    "runner": "pytest",
                    "gate": "h",
                    "nested": False,
                    "inventory": ["gone"],
                    "cases": [],
                }
            ),
            encoding="utf-8",
        )
        assert any("missing" in problem for problem in verify_directory(directory).problems)
    print("case_timings --self-test: PASSED")
    return 0


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--self-test", action="store_true", help="run the offline controls")
    sub = parser.add_subparsers(dest="command")
    report = sub.add_parser("report", help="verify a timing directory and print the bounded report")
    report.add_argument("--dir", type=Path, required=True)
    report.add_argument("--slowest", type=int, default=20)
    report.add_argument(
        "--expected-gate",
        action="append",
        default=[],
        help="gate that must have left a primary record (repeatable)",
    )
    weights = sub.add_parser(
        "weights", help="write pytest family shard weights from a timing directory"
    )
    weights.add_argument("--dir", type=Path, required=True)
    weights.add_argument("--out", type=Path, required=True)
    cargo = sub.add_parser("run-cargo", help="run cargo test with per-case timings")
    cargo.add_argument("--gate", default=None, help="owning validation gate")
    cargo.add_argument("cargo_argv", nargs=argparse.REMAINDER, help="cargo test ... after --")
    for name, help_text in (
        ("ingest-junit", "normalize a JUnit XML file"),
        ("ingest-playwright", "normalize a Playwright JSON report file"),
        ("ingest-node-tap", "normalize node:test TAP output from a file"),
        ("ingest-libtest-json", "normalize libtest JSON events from stdin, timed on arrival"),
    ):
        ingest = sub.add_parser(name, help=help_text)
        if name != "ingest-libtest-json":
            ingest.add_argument("file", type=Path)
        ingest.add_argument("--runner", default=None, help="runner name (ingest-junit only)")
        ingest.add_argument("--gate", required=True, help="owning validation gate")
        ingest.add_argument("--out", type=Path, required=True, help="timing record directory")
    args = parser.parse_args(argv)
    if args.self_test:
        if args.command is not None:
            parser.error("--self-test does not take a subcommand")
        return _self_test()
    if args.command == "weights":
        return _weights(args.dir, args.out)
    if args.command == "run-cargo":
        cargo_argv = list(args.cargo_argv)
        if cargo_argv[:1] == ["--"]:
            cargo_argv = cargo_argv[1:]
        return _run_cargo(cargo_argv, gate=args.gate)
    if args.command == "report":
        verified = verify_directory(args.dir, expected_gates=args.expected_gate)
        for line in render_report(verified, slowest=args.slowest):
            print(line)
        for gate in verified.missing_gates:
            print(
                f"case timings: expected timing gate {gate} left no primary timing record",
                file=sys.stderr,
            )
        for problem in verified.problems:
            print(f"case timings: {problem}", file=sys.stderr)
        return 1 if verified.problems or verified.missing_gates else 0
    if args.command == "ingest-junit":
        runner = args.runner or "junit"
        return _ingest(
            cases_from_junit(args.file.read_text(encoding="utf-8"), runner=runner, gate=args.gate),
            runner=runner,
            gate=args.gate,
            out=args.out,
        )
    if args.command == "ingest-playwright":
        payload: object = json.loads(args.file.read_text(encoding="utf-8"))
        if not isinstance(payload, dict):
            print("case-timings: Playwright report is not a JSON object", file=sys.stderr)
            return 1
        return _ingest(
            cases_from_playwright(payload, gate=args.gate),
            runner="playwright",
            gate=args.gate,
            out=args.out,
        )
    if args.command == "ingest-node-tap":
        return _ingest(
            cases_from_node_tap(args.file.read_text(encoding="utf-8"), gate=args.gate),
            runner="node-test",
            gate=args.gate,
            out=args.out,
        )
    if args.command == "ingest-libtest-json":
        events: list[tuple[float, Mapping[str, object]]] = []
        for raw_line in sys.stdin:
            arrived = time.monotonic()
            line = raw_line.strip()
            if not line.startswith("{"):
                continue
            try:
                parsed: object = json.loads(line)
            except json.JSONDecodeError:
                continue
            if isinstance(parsed, dict):
                events.append((arrived, parsed))
        return _ingest(
            cases_from_libtest_events(events, gate=args.gate),
            runner="libtest",
            gate=args.gate,
            out=args.out,
        )
    parser.error("a subcommand or --self-test is required")


if __name__ == "__main__":
    raise SystemExit(main())
