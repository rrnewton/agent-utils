"""Build the wrkviz differential's executable once per test process.

Every ``test_wrkviz_*_rust.py`` case compares the same checked-out Rust edition
(``wrkvizd``) with the Python oracle. Each file used to run its own
``cargo build --release -p wrkviz`` into the shared ``rs/target``, some through
``cargo run`` on every CLI call, with no ``--locked`` and no cap on Cargo's job
count, so how wide the compile ran and the state of the shared target depended
on the host and on whatever else used that target. This helper builds into a
target that belongs only to these tests, with ``--locked`` and two Cargo jobs
(the cap #883 set for test-shard builds), and reads the executable's path from
Cargo's own report. Cargo still checks source freshness once in every new test
process, including standalone pytest runs.

Run as a script, it builds the executable and prints its path. The validation
DAG does that in its own step, ``python.wrkviz.build-rust``, before any wrkviz
shard starts (#2318, the shape of #2306), so the release compile is not charged
to a shard's memory cap and each shard's freshness check finds the binary
current.
"""

from __future__ import annotations

import functools
import json
import os
import subprocess
import sys
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[2]
TARGET_DIR = REPO_ROOT / "rs" / "target" / "validation-adapters" / "wrkviz"


@functools.cache
def rust_wrkvizd() -> Path:
    """Return this process's freshly built release ``wrkvizd``, or fail the build."""
    env = dict(os.environ)
    env["CARGO_BUILD_JOBS"] = "2"
    result = subprocess.run(
        [
            "cargo", "build", "--quiet", "--release", "--locked",
            "--manifest-path", str(REPO_ROOT / "rs" / "Cargo.toml"),
            "-p", "wrkviz", "--bin", "wrkvizd", "--target-dir", str(TARGET_DIR),
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
            and built.get("name") == "wrkvizd"
            and built.get("kind") == ["bin"]
            and isinstance(executable, str)
            and Path(executable).is_file()
        ):
            return Path(executable)
    raise RuntimeError("Cargo did not report a wrkvizd executable")


def main() -> int:
    """Build the executable above and print where it is."""
    try:
        executable = rust_wrkvizd()
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
        print(f"wrkviz_rust_binary: Cargo exited {error.returncode}", file=sys.stderr)
        return 1
    print(executable, flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
