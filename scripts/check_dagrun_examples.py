#!/usr/bin/env python3
"""Validate every shipped DAG example under one selected implementation.

Two examples state a fixed resource contract that a smaller runner cannot
honestly satisfy. Example 05 fixes its guest width at eight jobs; example 08
models a clean-build worker under a 16 GiB hard memory cap. On a runner below
either contract this harness does not skip the example, shrink its contract,
or report capacity it does not have. It runs the example and requires the
engine's documented pre-spawn refusal instead: a refusal that names the
contract and the runner's real budget, with evidence the guest never started.
The low-memory path for example 08 therefore *proves an expected pre-spawn
refusal*; it does not execute the example. At or above the contract both
examples execute at full strength, unchanged.
"""

from __future__ import annotations

import argparse
import csv
import json
import os
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import case_timings


REPO_ROOT = Path(__file__).resolve().parent.parent
VALIDATION_JOBS_ENV = "AGENT_UTILS_VALIDATION_JOBS"
DEFAULT_VALIDATION_JOBS = 8
FIXED_WIDTH_EXAMPLE = "05-inner-jobs.json"
FIXED_WIDTH_JOBS = 8
#: Example 08's documented contract: a clean-build worker under a 16 GiB hard
#: cap. The same value is the example step's ``hard_mem_max_bytes`` and the
#: outer validation gate's delegated envelope; the graph-contract tests pin
#: all three together so the small-runner path cannot quietly shrink any one.
FIXED_MEMORY_EXAMPLE = "08-dagrun-clean-build-sweep.yaml"
FIXED_MEMORY_BYTES = 17179869184


def _effective_validation_jobs() -> int:
    """Return this process's effective whole-core affinity/quota budget."""

    python_root = str(REPO_ROOT / "py")
    inserted = python_root not in sys.path
    if inserted:
        sys.path.insert(0, python_root)
    try:
        from dagrun.profile_enrich import container_core_budget

        return container_core_budget()
    finally:
        if inserted:
            sys.path.remove(python_root)


def _validation_jobs(
    environment: dict[str, str] | None = None, *, effective_jobs: int | None = None
) -> int:
    """Read the nested CPU budget delivered through the outer DAG's jobs_env channel."""

    environ = os.environ if environment is None else environment
    effective = _effective_validation_jobs() if effective_jobs is None else effective_jobs
    if effective < 1:
        raise ValueError(f"effective CPU budget must be positive, got {effective}")
    raw = environ.get(VALIDATION_JOBS_ENV)
    if raw is None:
        return min(effective, DEFAULT_VALIDATION_JOBS)
    if not raw or not raw.isascii() or not raw.isdecimal():
        raise ValueError(
            f"{VALIDATION_JOBS_ENV} must be a positive integer, got {raw!r}"
        )
    jobs = int(raw)
    if jobs < 1:
        raise ValueError(
            f"{VALIDATION_JOBS_ENV} must be a positive integer, got {raw!r}"
        )
    return min(jobs, effective, DEFAULT_VALIDATION_JOBS)


def _memory_budget_bytes(*, cgroup_cap: int | None, mem_total: int | None) -> int | None:
    """The honest memory budget: the smaller of the signals that could be read.

    ``MemAvailable`` is deliberately not a signal here. It moves with whatever
    else the host is doing, so a busy large runner would flap between executing
    example 08 and expecting its refusal. Total memory and a finite cgroup cap
    are properties of the runner, which is what the contract is stated against.
    """

    signals = [value for value in (cgroup_cap, mem_total) if value is not None and value > 0]
    return min(signals) if signals else None


def _mem_total_bytes() -> int | None:
    try:
        for line in Path("/proc/meminfo").read_text(encoding="utf-8").splitlines():
            if line.startswith("MemTotal:"):
                return int(line.split()[1]) * 1024
    except (OSError, ValueError, IndexError):
        return None
    return None


def _effective_memory_budget_bytes() -> int | None:
    """This process's cgroup cap and host total, combined by the honest minimum."""

    python_root = str(REPO_ROOT / "py")
    inserted = python_root not in sys.path
    if inserted:
        sys.path.insert(0, python_root)
    try:
        from dagrun.sizing import cgroup_mem_max_bytes

        cgroup_cap = cgroup_mem_max_bytes()
    finally:
        if inserted:
            sys.path.remove(python_root)
    return _memory_budget_bytes(cgroup_cap=cgroup_cap, mem_total=_mem_total_bytes())


def _expected_memory_refusal(example_name: str, budget: int | None) -> bool:
    """Whether this runner is provably below example 08's fixed memory contract.

    An unreadable budget is not a small budget: with no signal the example
    runs, because a refusal the harness cannot predict cannot be required.
    """

    return (
        example_name == FIXED_MEMORY_EXAMPLE
        and budget is not None
        and budget < FIXED_MEMORY_BYTES
    )


def _is_documented_memory_refusal(
    returncode: int, combined: str, *, budget: int
) -> bool:
    """Whether output is example 08's pre-spawn hard-cap refusal at ``budget``.

    The footprint in the message must be the example's full 16 GiB hard cap:
    a smaller number would mean admission had been weakened to fit. The guest
    step's ``[build]`` echo must be absent, so a failure after spawn cannot
    masquerade as the documented refusal.
    """

    return (
        returncode == 2
        and "REFUSED" in combined
        and f"minimum runnable footprint {FIXED_MEMORY_BYTES} bytes" in combined
        and f"budget {budget} bytes" in combined
        and "[build]" not in combined
    )


def _examples() -> list[Path]:
    paths = sorted((REPO_ROOT / "examples").glob("*.json"))
    paths.extend(sorted((REPO_ROOT / "examples").glob("*.yaml")))
    if not paths:
        raise RuntimeError("no examples/*.json or examples/*.yaml files were found")
    return paths


def _engine(name: str) -> tuple[list[str], dict[str, str]]:
    environment = os.environ.copy()
    # This is a harness control, not input to the nested dagrun or its example guests.
    environment.pop(VALIDATION_JOBS_ENV, None)
    if name == "python":
        python_path = str(REPO_ROOT / "py")
        inherited = environment.get("PYTHONPATH")
        environment["PYTHONPATH"] = (
            os.pathsep.join((python_path, inherited)) if inherited else python_path
        )
        return [sys.executable, "-m", "dagrun"], environment
    return [str(REPO_ROOT / "rs/bin/dagrun")], environment


def _profile_smoke(
    command: list[str], environment: dict[str, str], *, validation_jobs: int
) -> None:
    """Exercise the real profile table and default store for one engine."""

    with tempfile.TemporaryDirectory(prefix="dagrun-example-profile-") as raw:
        root = Path(raw)
        dag = root / "profile-smoke.json"
        dag.write_text(
            json.dumps(
                {
                    "mem_cap_factor": 1.0,
                    "mem_cap_floor_bytes": 0,
                    "steps": [
                        {
                            "group": "profile",
                            "job": "smoke",
                            "cmd": "true",
                            "hint": {"rss_baseline_bytes": 1048576},
                        }
                    ],
                }
            ),
            encoding="utf-8",
        )
        profile_environment = environment.copy()
        profile_environment.pop("DAGRUN_PROFILE_DIR", None)
        completed = subprocess.run(
            [
                *command,
                "run",
                "--dag",
                str(dag),
                "--max-cpus",
                str(validation_jobs),
                "--allow-cgroup-failure",
                "--profile",
                "--no-profile-feedback",
            ],
            cwd=root,
            env=profile_environment,
            text=True,
            capture_output=True,
            check=False,
        )
        if completed.returncode != 0:
            raise RuntimeError(
                f"profile smoke failed with exit {completed.returncode}:\n"
                f"{completed.stdout}\n{completed.stderr}"
            )
        if "per-step profile:" not in completed.stdout or "profile.smoke" not in completed.stdout:
            raise RuntimeError("--profile output omitted its table or real step row")
        stores = sorted((root / ".dagrun" / "profiles").glob("step_profiles_*.csv"))
        if len(stores) != 1:
            raise RuntimeError(f"default profile store has {len(stores)} step CSVs, expected one")
        with stores[0].open(newline="", encoding="utf-8") as handle:
            rows = list(csv.DictReader(handle))
        if len(rows) != 1 or rows[0].get("step") != "profile.smoke":
            raise RuntimeError("default profile store omitted the successful profile.smoke row")


def _emit_timings(
    cases: list[case_timings.CaseRecord], inventory: list[str]
) -> None:
    case_timings.emit_if_requested(
        runner="examples", inventory=inventory, cases=cases
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", required=True, choices=("python", "rust"))
    arguments = parser.parse_args(argv)
    try:
        validation_jobs = _validation_jobs()
    except ValueError as error:
        parser.error(str(error))
    command, environment = _engine(arguments.engine)
    memory_budget = _effective_memory_budget_bytes()
    examples = _examples()
    gate = case_timings.current_gate()
    inventory = [str(path.relative_to(REPO_ROOT)) for path in examples]
    inventory.append("profile-smoke")
    timing_cases: list[case_timings.CaseRecord] = []
    for path in examples:
        relative = path.relative_to(REPO_ROOT)
        print(f"examples[{arguments.engine}]: {relative}", flush=True)
        started = time.monotonic()
        expected_width_refusal = (
            path.name == FIXED_WIDTH_EXAMPLE and validation_jobs < FIXED_WIDTH_JOBS
        )
        expected_memory_refusal = _expected_memory_refusal(path.name, memory_budget)
        expected_refusal = expected_width_refusal or expected_memory_refusal
        completed = subprocess.run(
            [
                *command,
                "run",
                "--dag",
                str(path),
                "--allow-cgroup-failure",
                "--no-profile",
                "--max-cpus",
                str(validation_jobs),
                *(
                    ["--max-mem", str(memory_budget)]
                    if expected_memory_refusal and memory_budget is not None
                    else []
                ),
            ],
            cwd=REPO_ROOT,
            env=environment,
            text=expected_refusal,
            capture_output=expected_refusal,
            check=False,
        )
        elapsed = time.monotonic() - started
        if expected_refusal:
            combined = completed.stdout + completed.stderr
            if expected_width_refusal and (
                completed.returncode == 2
                and "cannot lower guest parallelism" in combined
                and "build.app (preferred_inner_jobs=8)" in combined
                and f"--max-cpus {validation_jobs}" in combined
                and "[build] make -j8 build" not in combined
            ):
                print(
                    f"examples[{arguments.engine}]: {relative} correctly refused at "
                    f"{validation_jobs} CPU(s) (fixed width {FIXED_WIDTH_JOBS})",
                    flush=True,
                )
                timing_cases.append(
                    case_timings.CaseRecord("examples", gate, str(relative), "skip", elapsed)
                )
                continue
            if (
                expected_memory_refusal
                and memory_budget is not None
                and _is_documented_memory_refusal(
                    completed.returncode, combined, budget=memory_budget
                )
            ):
                print(
                    f"examples[{arguments.engine}]: {relative} correctly refused at "
                    f"{memory_budget} byte(s) (fixed memory {FIXED_MEMORY_BYTES})",
                    flush=True,
                )
                timing_cases.append(
                    case_timings.CaseRecord("examples", gate, str(relative), "skip", elapsed)
                )
                continue
            print(completed.stdout, end="", file=sys.stdout)
            print(completed.stderr, end="", file=sys.stderr)
            expected = (
                f"documented fixed-width refusal at {validation_jobs} CPU(s)"
                if expected_width_refusal
                else f"documented fixed-memory refusal at {memory_budget} byte(s)"
            )
            print(
                f"examples[{arguments.engine}]: expected {expected}",
                file=sys.stderr,
            )
            timing_cases.append(
                case_timings.CaseRecord("examples", gate, str(relative), "fail", elapsed)
            )
            _emit_timings(timing_cases, inventory)
            return 1
        if completed.returncode != 0:
            timing_cases.append(
                case_timings.CaseRecord("examples", gate, str(relative), "fail", elapsed)
            )
            _emit_timings(timing_cases, inventory)
            return completed.returncode
        timing_cases.append(
            case_timings.CaseRecord("examples", gate, str(relative), "pass", elapsed)
        )
    smoke_started = time.monotonic()
    try:
        _profile_smoke(command, environment, validation_jobs=validation_jobs)
    except RuntimeError as error:
        timing_cases.append(
            case_timings.CaseRecord(
                "examples", gate, "profile-smoke", "fail", time.monotonic() - smoke_started
            )
        )
        _emit_timings(timing_cases, inventory)
        print(f"examples[{arguments.engine}]: {error}", file=sys.stderr)
        return 1
    timing_cases.append(
        case_timings.CaseRecord(
            "examples", gate, "profile-smoke", "pass", time.monotonic() - smoke_started
        )
    )
    _emit_timings(timing_cases, inventory)
    print(f"examples[{arguments.engine}]: {len(examples)} validated")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
