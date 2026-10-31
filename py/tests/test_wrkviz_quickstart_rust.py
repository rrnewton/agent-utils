"""Differential: Rust `wrkviz quickstart` against the Python oracle."""

from __future__ import annotations

import os
import subprocess
import sys
from pathlib import Path

import pytest

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkviz"


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


def _cli(args: list[str], *, rust: bool, rust_wrkviz: Path,
         model: str | None) -> subprocess.CompletedProcess[str]:
    env = dict(os.environ)
    if model is None:
        env.pop("WRKVIZ_MODEL", None)
    else:
        env["WRKVIZ_MODEL"] = model
    command = [str(rust_wrkviz), *args] if rust else [sys.executable, str(PYTHON_WRKVIZ), *args]
    return subprocess.run(
        command, cwd=AU_ROOT, capture_output=True, text=True, check=False,
        timeout=60, env=env,
    )


def test_quickstart_matches_python(rust_wrkviz: Path) -> None:
    for model in (None, "test-model-9", ""):
        oracle = _cli(["quickstart"], rust=False, rust_wrkviz=rust_wrkviz, model=model)
        result = _cli(["quickstart"], rust=True, rust_wrkviz=rust_wrkviz, model=model)
        assert result.returncode == oracle.returncode == 0
        assert result.stdout == oracle.stdout
        assert result.stderr == oracle.stderr == ""


def test_quickstart_errors_match_python(rust_wrkviz: Path) -> None:
    for args in (
        ["quickstart", "--help"],
        ["quickstart", "-h"],
        ["quickstart", "--he"],
        ["quickstart", "--help=foo"],
        ["quickstart", "extra"],
        ["quickstart", "--bogus"],
        ["quickstart", "extra", "--help"],
    ):
        oracle = _cli(args, rust=False, rust_wrkviz=rust_wrkviz, model=None)
        result = _cli(args, rust=True, rust_wrkviz=rust_wrkviz, model=None)
        assert result.returncode == oracle.returncode, (args, result.stderr)
        assert result.stdout == oracle.stdout, args
        assert result.stderr == oracle.stderr, (args, result.stderr, oracle.stderr)
