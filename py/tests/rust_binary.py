"""Build the agentctl differential's executables once per test process.

Every case compares the same checked-out Rust implementation with the Python
oracle. Invoking the tracked launcher per case repeats its source-freshness
work under the shared Cargo lock, and one launch case rebuilt the release
binaries per test. This target belongs only to these tests. Cargo still
checks source freshness once in every new test process, including standalone
pytest runs.
"""

from __future__ import annotations

import functools
import json
import os
import subprocess
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
TARGET_DIR = REPO_ROOT / "rs" / "target" / "validation-adapters" / "agentctl"


def _build(
    packages: list[str], binaries: list[str], env: dict[str, str] | None = None
) -> dict[str, Path]:
    result = subprocess.run(
        [
            "cargo", "build", "--quiet", "--release", "--locked",
            "--manifest-path", str(REPO_ROOT / "rs" / "Cargo.toml"),
            *[item for package in packages for item in ("-p", package)],
            "--target-dir", str(TARGET_DIR),
            "--message-format", "json",
        ],
        cwd=REPO_ROOT, check=True, capture_output=True, text=True, timeout=1200,
        env=env,
    )
    built: dict[str, Path] = {}
    for line in result.stdout.splitlines():
        artifact: object = json.loads(line)
        if not isinstance(artifact, dict):
            continue
        target = artifact.get("target")
        executable = artifact.get("executable")
        if (
            artifact.get("reason") == "compiler-artifact"
            and isinstance(target, dict)
            and target.get("kind") == ["bin"]
            and isinstance(target.get("name"), str)
            and isinstance(executable, str)
            and Path(executable).is_file()
        ):
            built[str(target["name"])] = Path(executable)
    missing = [binary for binary in binaries if binary not in built]
    if missing:
        raise RuntimeError(f"Cargo did not report executables for {missing}")
    return built


@functools.cache
def rust_agentctl() -> Path:
    """Return this session's freshly built release binary, or fail the build."""
    return _build(["agentctl"], ["agentctl"])["agentctl"]


@functools.cache
def rust_agentctl_with_fake() -> tuple[Path, Path]:
    """The launch differential's agentctl and fake-plugin binaries, built once."""
    env = dict(os.environ)
    env.update(
        {
            "CARGO_PROFILE_DEV_DEBUG": "line-tables-only",
            "CARGO_PROFILE_TEST_DEBUG": "line-tables-only",
            "CARGO_INCREMENTAL": "0",
        }
    )
    built = _build(
        ["agentctl", "chat-subscription-fake"],
        ["agentctl", "chat-subscription-fake-plugin"],
        env=env,
    )
    return built["agentctl"], built["chat-subscription-fake-plugin"]
