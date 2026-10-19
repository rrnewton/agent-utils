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
  stable output carries no per-case durations, so these emit through
  ``ingest-libtest-json``, which timestamps the test binary's JSON event
  stream as it arrives (serial runs give true per-case intervals). Their
  validation gates remain gate-timed by dagrun's profile until they are
  rewired through that wrapper; this module does not pretend otherwise.

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


def verify_directory(directory: Path) -> TimingReport:
    """Verify exact per-record inventory equality and build the primary aggregate.

    Every record must account for its own collected inventory exactly once.
    Across primary records a ``(runner, gate, case_id)`` may appear only
    once; nested records are inventory-checked too but their cases are
    counted separately and never enter the primary aggregate.
    """

    problems: list[str] = []
    paths = sorted(directory.glob("*.json")) if directory.is_dir() else []
    if len(paths) > MAX_FILES:
        problems.append(f"{len(paths)} timing record files, over the {MAX_FILES} bound")
        paths = paths[:MAX_FILES]
    primary: dict[tuple[str, str, str], CaseRecord] = {}
    nested_count = 0
    loaded = 0
    for path in paths:
        run = _load_run_file(path, problems)
        if run is None:
            continue
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
        for case in run.cases:
            key = (case.runner, case.gate, case.case_id)
            if key in primary:
                problems.append(
                    f"{run.source}: duplicate primary case {case.case_id} for "
                    f"{case.runner} gate {case.gate}"
                )
                continue
            primary[key] = case
    ordered = tuple(sorted(primary.values(), key=lambda case: (case.runner, case.gate, case.case_id)))
    return TimingReport(problems, ordered, nested_count, loaded)


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
        outcomes = ", ".join(
            f"{outcome} {sum(1 for case in cases if case.outcome == outcome)}"
            for outcome in ("pass", "fail", "skip", "error")
            if any(case.outcome == outcome for case in cases)
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


# ---------------------------------------------------------------- CLI


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
    if args.command == "report":
        verified = verify_directory(args.dir)
        for line in render_report(verified, slowest=args.slowest):
            print(line)
        for problem in verified.problems:
            print(f"case timings: {problem}", file=sys.stderr)
        return 1 if verified.problems else 0
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
