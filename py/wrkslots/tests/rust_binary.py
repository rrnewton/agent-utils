"""Build the wrkslots differential's executable once per test process.

Every case compares the same checked-out Rust implementation with the Python
oracle. Invoking Cargo per case repeats build-directory locking and dependency
resolution; sharing the launcher's target can also replace its stamped binary.
This target belongs only to these tests. Cargo still checks source freshness
once in every new test process, including standalone pytest runs.

Run as a script, it builds the executable and prints its path. The validation
DAG does that in its own step, ``python.wrkslots.build-rust``, before any
wrkslots shard starts (#2318, the shape of #2306): the Rust differential cases
are spread over every shard, so whichever shard reached one first used to run
the release compile inside its own memory cap. Now each shard's freshness
check finds the binary current, and its cap measures the Python tests alone.
"""

from __future__ import annotations

import functools
import json
import os
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]


@functools.cache
def rust_wrkslotsd() -> Path:
    """Return this session's freshly built release binary, or fail the build."""
    target = REPO_ROOT / "rs" / "target" / "validation-adapters" / "wrkslotsd"
    # Bound Cargo's job parallelism (#883): a cold release build at the
    # host's default width peaked past a test shard's memory cap, while
    # two jobs keep the same binary with a much smaller build tree.
    env = dict(os.environ)
    env["CARGO_BUILD_JOBS"] = "2"
    result = subprocess.run(
        [
            "cargo", "build", "--quiet", "--release", "--locked",
            "--manifest-path", str(REPO_ROOT / "rs" / "Cargo.toml"),
            "-p", "wrkslotsd", "--bin", "wrkslotsd", "--target-dir", str(target),
            "--message-format", "json",
        ],
        cwd=REPO_ROOT, check=True, capture_output=True, text=True, timeout=900, env=env,
    )
    for line in result.stdout.splitlines():
        artifact: object = json.loads(line)
        if not isinstance(artifact, dict):
            continue
        built = artifact.get("target")
        executable = artifact.get("executable")
        if (
            artifact.get("reason") == "compiler-artifact"
            and isinstance(built, dict)
            and built.get("name") == "wrkslotsd"
            and built.get("kind") == ["bin"]
            and isinstance(executable, str)
            and Path(executable).is_file()
        ):
            return Path(executable)
    raise RuntimeError("Cargo did not report a wrkslotsd executable")


def main() -> int:
    """Build the adapter executable above and print where it is."""
    try:
        executable = rust_wrkslotsd()
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
    print(executable, flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
