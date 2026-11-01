"""Differential: Rust combined (multi-team) `wrkviz export`.

Twin archives each hold three tiny teams ingested from the same
Codex rollout and summarized under the deterministic heuristic
backend. Combined exports of two and three teams, a windowed
slice, an idempotent rerun, a team-set change into the same
target (which must retire the dropped team's files), and the
duplicate/missing-team errors must agree on stdout, stderr,
exit code, and both the export and archive trees, reusing the
build differential's scrubbing (timestamps, run ids, and
gzip-generation byte counts are normalized; compressed members
compare by decompressed content).

Fixtures only; no live sources or real model calls.
"""

from __future__ import annotations

import importlib.util
import subprocess
import sys
from pathlib import Path

import pytest

AU_ROOT = Path(__file__).resolve().parents[2]
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkviz"

_spec = importlib.util.spec_from_file_location(
    "wrkviz_build_diff", AU_ROOT / "py" / "tests" / "test_wrkviz_build_rust.py"
)
assert _spec is not None and _spec.loader is not None
B = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(B)

PYTHON_WRKVIZ = B.PYTHON_WRKVIZ
TEAMS = ("alpha", "beta", "gamma")


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


def _archives(tmp_path: Path, rust_wrkviz: Path) -> dict[str, Path]:
    sessions = B._sessions(tmp_path)
    archives = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(rust_wrkviz)])):
        archive = tmp_path / f"{label}-out"
        for team in TEAMS:
            outcome = B._run(command, [
                "ingest", "--sessions-root", str(sessions),
                "--root-session", B.CODEX_ROOT, "--team", team,
                "--output", str(archive),
            ])
            assert outcome.returncode == 0, outcome.stderr
        for team in TEAMS:
            outcome = B._run(command, [
                "summarize", "--output", str(archive), "--team", team,
                "--backend", "heuristic", "--model", "offline",
            ])
            assert outcome.returncode == 0, outcome.stderr
        archives[label] = archive
    return archives


def _export(
    archives: dict[str, Path],
    targets: dict[str, Path],
    args: list[str],
    *,
    expect: int = 0,
) -> None:
    outcomes = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(RUST_WRKVIZ)])):
        outcomes[label] = B._run(command, [
            "export", "--archive", str(archives[label]),
            "--output", str(targets[label]), *args,
        ])
    oracle, result = outcomes["py"], outcomes["rs"]
    assert oracle.returncode == expect, (oracle.stdout, oracle.stderr)
    assert result.returncode == oracle.returncode
    assert B._normalize(result.stdout, targets["rs"]) == B._normalize(
        oracle.stdout, targets["py"])
    assert B._normalize(result.stderr, targets["rs"]) == B._normalize(
        oracle.stderr, targets["py"])
    assert B._tree(targets["rs"]) == B._tree(targets["py"])
    assert B._tree(archives["rs"]) == B._tree(archives["py"])


def test_combined_export_matches(
    tmp_path: Path, rust_wrkviz: Path
) -> None:
    archives = _archives(tmp_path, rust_wrkviz)
    targets = {label: tmp_path / f"{label}-combined" for label in ("py", "rs")}
    _export(archives, targets, ["--team", "alpha", "--team", "beta"])
    # An idempotent rerun over the same manifest changes nothing.
    _export(archives, targets, ["--team", "alpha", "--team", "beta"])
    # A windowed slice of the same pair.
    windowed = {label: tmp_path / f"{label}-windowed" for label in ("py", "rs")}
    _export(archives, windowed, [
        "--team", "alpha", "--team", "beta",
        "--start-date", "2026-01-01", "--end-date", "2026-01-02",
        "--rollup-kind", "daily",
    ])


def test_combined_export_set_change_retires(
    tmp_path: Path, rust_wrkviz: Path
) -> None:
    archives = _archives(tmp_path, rust_wrkviz)
    targets = {label: tmp_path / f"{label}-combined" for label in ("py", "rs")}
    _export(archives, targets, ["--team", "alpha", "--team", "beta"])
    _export(archives, targets, ["--team", "beta", "--team", "gamma"])
    manifest = B.json.loads(
        (targets["py"] / "data" / "export.json").read_text(encoding="utf-8")
    )
    assert manifest["teams"] == ["beta", "gamma"]
    # Stale-file removal deletes the dropped team's files (the
    # empty directories stay; both editions agree through the
    # tree comparison above).
    alpha = targets["py"] / "teams" / "alpha"
    assert [p for p in alpha.rglob("*") if p.is_file()] == []


def test_combined_export_prior_manifest_variants_match(
    tmp_path: Path, rust_wrkviz: Path
) -> None:
    archives = _archives(tmp_path, rust_wrkviz)
    targets = {label: tmp_path / f"{label}-combined" for label in ("py", "rs")}
    _export(archives, targets, ["--team", "alpha", "--team", "beta"])
    for label in ("py", "rs"):
        path = targets[label] / "data" / "export.json"
        manifest = B.json.loads(path.read_text(encoding="utf-8"))
        # CPython equality accepts 1.0 for the schema version,
        # and PurePosixPath normalization accepts a trailing
        # slash on a recorded generated file.
        manifest["schema_version"] = 1.0
        manifest["generated_files"].append("teams/alpha/summaries/ghost.md/")
        path.write_text(
            B.json.dumps(manifest, indent=2) + "\n", encoding="utf-8"
        )
    _export(archives, targets, ["--team", "alpha", "--team", "beta"])


def test_combined_export_three_teams_matches(
    tmp_path: Path, rust_wrkviz: Path
) -> None:
    archives = _archives(tmp_path, rust_wrkviz)
    targets = {label: tmp_path / f"{label}-combined" for label in ("py", "rs")}
    _export(archives, targets, [
        "--team", "gamma", "--team", "alpha", "--team", "beta",
    ])


def test_combined_export_errors_match(
    tmp_path: Path, rust_wrkviz: Path
) -> None:
    archives = _archives(tmp_path, rust_wrkviz)
    targets = {label: tmp_path / f"{label}-dup" for label in ("py", "rs")}
    _export(
        archives, targets, ["--team", "alpha", "--team", "alpha"], expect=2
    )
    missing = {label: tmp_path / f"{label}-missing" for label in ("py", "rs")}
    _export(
        archives, missing, ["--team", "alpha", "--team", "nope"], expect=2
    )
