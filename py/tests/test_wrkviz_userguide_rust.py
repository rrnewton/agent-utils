"""Differential: Rust `wrkviz --userguide` vs the oracle.

The guide text lives once, in `common/docs/wrkviz/USER_GUIDE.md`;
the Python edition ships it as the package file `USER_GUIDE.md`
and prints it verbatim for `--userguide`. The Rust edition
embeds the same file, so stdout must agree byte-for-byte, along
with the flag's argparse neighbourhood: abbreviations, explicit
arguments, the help and version actions that still fire after
it, unrecognized options, and invalid subcommand choices.

Known divergence, unpinned: after `--userguide`, a subcommand
with invalid arguments makes the oracle report the subcommand's
own parse error; the Rust edition prints the guide. A subcommand
with well-formed arguments is only parsed, never run, by either
edition, and both print the guide.
"""

from __future__ import annotations

import subprocess
from pathlib import Path

import pytest

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkviz"
GUIDE = AU_ROOT / "common" / "docs" / "wrkviz" / "USER_GUIDE.md"


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


def _run(binary: Path, args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [str(binary), *args],
        cwd=AU_ROOT, capture_output=True, text=True, timeout=300,
    )


def _both(
    rust_wrkviz: Path, args: list[str]
) -> tuple[subprocess.CompletedProcess[str], subprocess.CompletedProcess[str]]:
    oracle = _run(PYTHON_WRKVIZ, args)
    rust = _run(rust_wrkviz, args)
    assert (rust.returncode, rust.stdout, rust.stderr) == (
        oracle.returncode, oracle.stdout, oracle.stderr
    ), (args, oracle, rust)
    return oracle, rust


def test_userguide_prints_the_guide(rust_wrkviz: Path) -> None:
    oracle, _ = _both(rust_wrkviz, ["--userguide"])
    assert oracle.stdout == GUIDE.read_text(encoding="utf-8")
    assert oracle.returncode == 0


def test_userguide_abbreviations_match(rust_wrkviz: Path) -> None:
    for args in (["--userg"], ["--u"], ["--userguide", "--userguide"]):
        _both(rust_wrkviz, args)


def test_userguide_explicit_argument_matches(rust_wrkviz: Path) -> None:
    for args in (
        ["--userguide=x"],
        ["--help=x"],
        ["--version=x"],
        ["--userguide", "--help=x"],
    ):
        _both(rust_wrkviz, args)


def test_userguide_neighbour_actions_match(rust_wrkviz: Path) -> None:
    for args in (
        ["--userguide", "-h"],
        ["-h", "--userguide"],
        ["--userguide", "--version"],
        ["--version", "--userguide"],
        ["--vers"],
        ["--hel"],
    ):
        _both(rust_wrkviz, args)


def test_userguide_following_tokens_match(rust_wrkviz: Path) -> None:
    for args in (
        ["--userguide", "quickstart"],
        ["--userguide", "--bogus"],
        ["--userguide", "frobnicate"],
    ):
        _both(rust_wrkviz, args)
