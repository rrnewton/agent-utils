"""Build the wrkslots differential's executable once per test process.

Every case compares the same checked-out Rust implementation with the Python
oracle. Invoking Cargo per case repeats build-directory locking and dependency
resolution; sharing the launcher's target can also replace its stamped binary.
This target belongs only to these tests. Cargo still checks source freshness
once in every new test process, including standalone pytest runs.
"""

from __future__ import annotations

import functools
import json
import subprocess
from pathlib import Path


REPO_ROOT = Path(__file__).resolve().parents[3]


@functools.cache
def rust_wrkslotsd() -> Path:
    """Return this session's freshly built release binary, or fail the build."""
    target = REPO_ROOT / "rs" / "target" / "validation-adapters" / "wrkslotsd"
    result = subprocess.run(
        [
            "cargo", "build", "--quiet", "--release", "--locked",
            "--manifest-path", str(REPO_ROOT / "rs" / "Cargo.toml"),
            "-p", "wrkslotsd", "--bin", "wrkslotsd", "--target-dir", str(target),
            "--message-format", "json",
        ],
        cwd=REPO_ROOT, check=True, capture_output=True, text=True, timeout=900,
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
