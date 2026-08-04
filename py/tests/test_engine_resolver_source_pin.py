# Copyright (c) Meta Platforms, Inc. and affiliates.
# All rights reserved.
#
# This source code is licensed under the BSD-style license found in the
# LICENSE file in the root directory of this source tree.
#
# Mutation controls for the engine resolver's Rust source-pin guard.
#
# The resolver (common/bin/engine-resolver, reached via the safe-ci-dag-runner symlink) runs the
# UNTRACKED rs/bin Rust binary only when SAFE_CI_DAG_RUNNER_ENGINE=rust AND the binary content-
# verifies against currently-pinned source, using provenance stamped by `setup rs`. These tests
# bracket BOTH sides: a POSITIVE control proving a correctly-stamped binary runs, and NEGATIVE
# controls proving a stale-source binary, a planted/modified binary, and a missing-provenance binary
# are each REFUSED rather than silently run. Fully hermetic: it copies the real resolver + real
# fingerprint helper into a throwaway git tree and plants a fake runner, so no cargo build is needed.

import hashlib
import os
import shutil
import stat
import subprocess
from pathlib import Path

AU_ROOT = Path(__file__).resolve().parents[2]
RESOLVER = AU_ROOT / "common" / "bin" / "engine-resolver"
FINGERPRINT = AU_ROOT / "common" / "bin" / "rs-source-fingerprint"

FAKE_RUNNER = "#!/bin/sh\necho FAKE_RUNNER_RAN \"$@\"\n"


def _chmod_x(p: Path) -> None:
    p.chmod(p.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def _git(root: Path, *args: str) -> None:
    subprocess.run(
        ["git", *args],
        cwd=root,
        check=True,
        capture_output=True,
        env={**os.environ, "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_CONFIG_SYSTEM": "/dev/null"},
    )


def _sha256(p: Path) -> str:
    return hashlib.sha256(p.read_bytes()).hexdigest()


def _run_resolver(root: Path, engine: str = "rust") -> "subprocess.CompletedProcess[str]":
    """Invoke the resolver via its tool symlink; return CompletedProcess."""
    tool = root / "common" / "bin" / "safe-ci-dag-runner"
    return subprocess.run(
        [str(tool), "run", "--dag", "/dev/null"],
        cwd=root,
        capture_output=True,
        text=True,
        env={**os.environ, "SAFE_CI_DAG_RUNNER_ENGINE": engine},
    )


def _build_sandbox(tmp_path: Path) -> Path:
    """Assemble a minimal agent-utils-shaped tree with the REAL resolver + helper and a fake runner,
    correctly provenance-stamped (the state a clean `setup rs` produces)."""
    root = tmp_path / "au"
    (root / "common" / "bin").mkdir(parents=True)
    (root / "rs" / "safe-ci-dag-runner" / "src").mkdir(parents=True)
    (root / "rs" / "bin").mkdir(parents=True)
    (root / "py" / "bin").mkdir(parents=True)

    # Real scripts under test.
    shutil.copy2(RESOLVER, root / "common" / "bin" / "engine-resolver")
    shutil.copy2(FINGERPRINT, root / "common" / "bin" / "rs-source-fingerprint")
    _chmod_x(root / "common" / "bin" / "engine-resolver")
    _chmod_x(root / "common" / "bin" / "rs-source-fingerprint")
    (root / "common" / "bin" / "safe-ci-dag-runner").symlink_to("engine-resolver")

    # Tracked Rust source (what the fingerprint covers).
    (root / "rs" / "Cargo.toml").write_text('[workspace]\nmembers = ["safe-ci-dag-runner"]\n')
    (root / "rs" / "Cargo.lock").write_text('version = 3\n')
    (root / "rs" / "safe-ci-dag-runner" / "Cargo.toml").write_text(
        '[package]\nname = "safe-ci-dag-runner"\nversion = "0.1.0"\n'
    )
    (root / "rs" / "safe-ci-dag-runner" / "src" / "main.rs").write_text(
        'fn main() { println!("v1"); }\n'
    )

    # The UNTRACKED build artifact (a fake stand-in for the real Rust binary).
    binp = root / "rs" / "bin" / "safe-ci-dag-runner"
    binp.write_text(FAKE_RUNNER)
    _chmod_x(binp)

    # Tracked py entrypoint (so the python-engine path is well-formed too).
    (root / "py" / "bin" / "safe-ci-dag-runner").write_text("#!/usr/bin/env python3\nprint('py')\n")

    # gitignore rs/bin so it matches the real repo (helper excludes it anyway).
    (root / ".gitignore").write_text("/rs/bin/*\n!/rs/bin/.gitkeep\n/rs/target/\n")

    _git(root, "init", "-q")
    _git(root, "add", "-A")

    # Stamp provenance exactly as `setup rs` does: current source fp + the binary's sha256.
    src_fp = subprocess.run(
        [str(root / "common" / "bin" / "rs-source-fingerprint")],
        cwd=root, capture_output=True, text=True, check=True,
    ).stdout.strip()
    assert len(src_fp) == 64, f"unexpected fingerprint: {src_fp!r}"
    (root / "rs" / "bin" / "safe-ci-dag-runner.provenance").write_text(
        f"source_fp={src_fp}\nbinary_sha256={_sha256(binp)}\n"
    )
    return root


def test_positive_correctly_stamped_binary_runs(tmp_path: Path) -> None:
    """POSITIVE control: a binary whose provenance matches source AND its own bytes is executed."""
    root = _build_sandbox(tmp_path)
    r = _run_resolver(root)
    assert r.returncode == 0, f"expected run, got rc={r.returncode}\n{r.stderr}"
    assert "source-verified" in r.stderr
    assert "FAKE_RUNNER_RAN" in r.stdout


def test_negative_stale_source_is_refused(tmp_path: Path) -> None:
    """NEGATIVE control: source moves past what the binary was built from -> STALE -> refused."""
    root = _build_sandbox(tmp_path)
    # Advance the pinned source WITHOUT rebuilding (no re-stamp).
    (root / "rs" / "safe-ci-dag-runner" / "src" / "main.rs").write_text(
        'fn main() { println!("v2 — newer than the binary"); }\n'
    )
    r = _run_resolver(root)
    assert r.returncode != 0, f"stale binary was NOT refused (rc={r.returncode})\n{r.stderr}"
    assert "STALE" in r.stderr
    assert "FAKE_RUNNER_RAN" not in r.stdout


def test_negative_planted_binary_is_refused(tmp_path: Path) -> None:
    """NEGATIVE control: binary bytes swapped without a rebuild -> sha mismatch -> refused."""
    root = _build_sandbox(tmp_path)
    binp = root / "rs" / "bin" / "safe-ci-dag-runner"
    binp.write_text("#!/bin/sh\necho PLANTED_RUNNER \"$@\"\n")  # different bytes, still executable
    _chmod_x(binp)
    r = _run_resolver(root)
    assert r.returncode != 0, f"planted binary was NOT refused (rc={r.returncode})\n{r.stderr}"
    assert "does not match its provenance" in r.stderr
    assert "PLANTED_RUNNER" not in r.stdout


def test_negative_missing_provenance_is_refused(tmp_path: Path) -> None:
    """NEGATIVE control: no provenance at all -> the binary is unverifiable -> refused."""
    root = _build_sandbox(tmp_path)
    (root / "rs" / "bin" / "safe-ci-dag-runner.provenance").unlink()
    r = _run_resolver(root)
    assert r.returncode != 0, f"unverifiable binary was NOT refused (rc={r.returncode})\n{r.stderr}"
    assert "no build provenance" in r.stderr
    assert "FAKE_RUNNER_RAN" not in r.stdout
