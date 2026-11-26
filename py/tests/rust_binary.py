"""Build the agentctl differential's executables once per test process.

Every case compares the same checked-out Rust implementation with the Python
oracle. Invoking the tracked launcher per case repeats its source-freshness
work under the shared Cargo lock, and one launch case rebuilt the release
binaries per test. This target belongs only to these tests. Cargo still
checks source freshness once in every new test process, including standalone
pytest runs.

Run as a script, it builds every executable these tests use and prints their
paths. The validation DAG does that in its own step, ``python.agentctl.build-rust``,
before either agentctl shard starts (#2306): the release compile of the
agentctl crate peaks near 3 GiB in one rustc, which a pytest shard's memory cap
must not have to cover, so each shard's freshness check finds the binaries
current and its cap measures the Python tests alone.
"""

from __future__ import annotations

import functools
import json
import os
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
TARGET_DIR = REPO_ROOT / "rs" / "target" / "validation-adapters" / "agentctl"


def _build(
    packages: list[str], binaries: list[str], env: dict[str, str] | None = None
) -> dict[str, Path]:
    # Bound Cargo's job parallelism (#883), as in the wrkslots helper.
    build_env = dict(os.environ if env is None else env)
    build_env["CARGO_BUILD_JOBS"] = "2"
    result = subprocess.run(
        [
            "cargo", "build", "--quiet", "--release", "--locked",
            "--manifest-path", str(REPO_ROOT / "rs" / "Cargo.toml"),
            *[item for package in packages for item in ("-p", package)],
            "--target-dir", str(TARGET_DIR),
            "--message-format", "json",
        ],
        cwd=REPO_ROOT, check=True, capture_output=True, text=True, timeout=1200,
        env=build_env,
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


def main() -> int:
    """Build every adapter executable above and print where each one is."""
    try:
        agentctl = rust_agentctl()
        launch_agentctl, fake_plugin = rust_agentctl_with_fake()
    except subprocess.CalledProcessError as error:
        # The build captures Cargo's output; a DAG step must still show why it failed.
        # With --message-format json the compiler's own diagnostics arrive on stdout.
        for line in (error.stdout or "").splitlines():
            try:
                message: object = json.loads(line)
            except ValueError:
                continue
            if isinstance(message, dict) and message.get("reason") == "compiler-message":
                body = message.get("message")
                if isinstance(body, dict) and isinstance(body.get("rendered"), str):
                    sys.stderr.write(body["rendered"])
        sys.stderr.write(error.stderr or "")
        print(f"rust_binary: Cargo exited {error.returncode}", file=sys.stderr)
        return 1
    for executable in (agentctl, launch_agentctl, fake_plugin):
        print(executable, flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
