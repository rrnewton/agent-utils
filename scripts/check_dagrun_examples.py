#!/usr/bin/env python3
"""Run every shipped DAG example under one selected implementation."""

from __future__ import annotations

import argparse
import csv
import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parent.parent


def _examples() -> list[Path]:
    paths = sorted((REPO_ROOT / "examples").glob("*.json"))
    paths.extend(sorted((REPO_ROOT / "examples").glob("*.yaml")))
    if not paths:
        raise RuntimeError("no examples/*.json or examples/*.yaml files were found")
    return paths


def _engine(name: str) -> tuple[list[str], dict[str, str]]:
    environment = os.environ.copy()
    if name == "python":
        python_path = str(REPO_ROOT / "py")
        inherited = environment.get("PYTHONPATH")
        environment["PYTHONPATH"] = (
            os.pathsep.join((python_path, inherited)) if inherited else python_path
        )
        return [sys.executable, "-m", "dagrun"], environment
    return [str(REPO_ROOT / "rs/bin/dagrun")], environment


def _profile_smoke(command: list[str], environment: dict[str, str]) -> None:
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


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--engine", required=True, choices=("python", "rust"))
    arguments = parser.parse_args(argv)
    command, environment = _engine(arguments.engine)
    examples = _examples()
    for path in examples:
        extra = ["--max-cpus", "8"] if path.name == "05-inner-jobs.json" else []
        relative = path.relative_to(REPO_ROOT)
        print(f"examples[{arguments.engine}]: {relative}", flush=True)
        completed = subprocess.run(
            [
                *command,
                "run",
                "--dag",
                str(path),
                "--allow-cgroup-failure",
                "--no-profile",
                *extra,
            ],
            cwd=REPO_ROOT,
            env=environment,
            check=False,
        )
        if completed.returncode != 0:
            return completed.returncode
    try:
        _profile_smoke(command, environment)
    except RuntimeError as error:
        print(f"examples[{arguments.engine}]: {error}", file=sys.stderr)
        return 1
    print(f"examples[{arguments.engine}]: {len(examples)} passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
