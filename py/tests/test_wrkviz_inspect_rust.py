"""Differential: the Rust `wrkviz inspect` against the Python oracle.

The corpus is a real (small) synthetic archive built by the
ordinary ingest/summarize/build path, so both editions answer
over the same schema-3 generation a user would have. Mutations
of that archive exercise the decline rules and fallbacks; the
outputs compared are the exact stdout bytes, the exit code, and
(where the text is part of the contract) stderr.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"


def _python(archive: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(PYTHON_WRKVIZ), "inspect", "--output", str(archive)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def _rust(archive: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        ["cargo", "run", "--quiet", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz", "--", "inspect", "--output", str(archive)],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=900,
    )


def _check(archive: Path) -> subprocess.CompletedProcess[str]:
    oracle = _python(archive)
    result = _rust(archive)
    assert result.returncode == oracle.returncode, (result.stderr, oracle.stderr)
    assert result.stdout == oracle.stdout
    return oracle


@pytest.fixture(scope="module")
def corpus(tmp_path_factory: pytest.TempPathFactory) -> Path:
    sys.path.insert(0, str(AU_ROOT / "py"))
    from wrkviz.synthetic import SyntheticScale, build_corpus, generate_sources

    base = tmp_path_factory.mktemp("wrkviz-corpus")
    scale = SyntheticScale(teams=1, agents=14, days=3, tool_calls_per_agent=6, seed=7)
    generated = generate_sources(base / "sources", scale)
    archive = base / "archive"
    build_corpus(generated, archive)
    return archive.resolve()


def test_inspect_schema3_synthetic_matches_python(corpus: Path) -> None:
    oracle = _check(corpus)
    assert oracle.returncode == 0
    payload = json.loads(oracle.stdout)
    assert payload["agents"] == 15
    assert payload["events"] > 0


def test_inspect_missing_archive_matches_python(tmp_path: Path) -> None:
    missing = tmp_path / "nope"
    oracle = _python(missing)
    result = _rust(missing)
    assert result.returncode == oracle.returncode == 2
    assert result.stdout == oracle.stdout == ""
    assert result.stderr == oracle.stderr
    assert "no built timeline" in result.stderr


def test_inspect_empty_archive_matches_python(tmp_path: Path) -> None:
    empty = tmp_path / "empty"
    empty.mkdir()
    oracle = _python(empty)
    result = _rust(empty)
    assert result.returncode == oracle.returncode == 2
    assert result.stderr == oracle.stderr
    assert "no schema-3 bootstrap" in result.stderr


def test_inspect_timeline_fallback_matches_python(tmp_path: Path) -> None:
    archive = tmp_path / "legacy"
    (archive / "data").mkdir(parents=True)
    timeline = {
        "agents": [{"id": "a"}, {"id": "b"}],
        "phases": [{"id": "p"}],
        "edges": [],
        "events": [{"id": "e"}] * 5,
        "rollups": [],
        "summary_files": [{"id": "s"}] * 3,
    }
    (archive / "data" / "timeline.json").write_text(json.dumps(timeline))
    (archive / "manifest.json").write_text(json.dumps({"team": "legacy", "n": 2}))
    oracle = _check(archive)
    assert oracle.returncode == 0
    payload = json.loads(oracle.stdout)
    assert payload["agents"] == 2
    assert payload["events"] == 5
    assert payload["manifest"] == {"team": "legacy", "n": 2}


def test_inspect_declined_schema3_matches_python(
    corpus: Path, tmp_path: Path
) -> None:
    archive = tmp_path / "broken"
    shutil.copytree(corpus, archive)
    bootstrap = json.loads((archive / "data" / "timeline-v3.json").read_text())
    shard = bootstrap["streams"]["spine"]["shards"][0]["path"]
    with (archive / shard).open("ab") as handle:
        handle.write(b"x")
    oracle = _python(archive)
    result = _rust(archive)
    assert result.returncode == oracle.returncode == 2
    assert result.stderr == oracle.stderr
    assert "partly published" in result.stderr


def test_inspect_unnamed_shard_declines_matches_python(
    corpus: Path, tmp_path: Path
) -> None:
    archive = tmp_path / "stray"
    shutil.copytree(corpus, archive)
    (archive / "data" / "timeline-v3" / "stray.jsonl.gz").write_bytes(b"stray")
    oracle = _python(archive)
    result = _rust(archive)
    assert result.returncode == oracle.returncode == 2
    assert result.stderr == oracle.stderr
    assert "named by no entry" in result.stderr


def test_inspect_legacy_snapshots_match_python(
    corpus: Path, tmp_path: Path
) -> None:
    archive = tmp_path / "legacy-snaps"
    shutil.copytree(corpus, archive)
    snapshots = archive / "teams" / "synthetic-team-1" / "source_snapshots"
    (snapshots / "deep").mkdir(parents=True)
    (snapshots / "one.bin").write_bytes(b"1234")
    (snapshots / "deep" / "two.bin").write_bytes(b"123456")
    oracle = _check(archive)
    assert oracle.returncode == 0
    payload = json.loads(oracle.stdout)
    summary = payload["source_snapshots"]
    assert summary["in_archive_teams"] == ["synthetic-team-1"]
    assert summary["in_archive_files"] == 2
    assert summary["in_archive_bytes"] == 10


def _cli(args: list[str], *, rust: bool) -> subprocess.CompletedProcess[str]:
    if rust:
        command = ["cargo", "run", "--quiet", "--release", "--manifest-path",
                   "rs/Cargo.toml", "-p", "wrkviz", "--", *args]
    else:
        command = [sys.executable, str(PYTHON_WRKVIZ), *args]
    return subprocess.run(
        command, cwd=AU_ROOT, capture_output=True, text=True, check=False,
        timeout=900,
    )


def test_cli_help_and_version_match_python() -> None:
    for args in ([], ["--help"], ["inspect", "--help"], ["--version"]):
        oracle = _cli(args, rust=False)
        result = _cli(args, rust=True)
        assert result.returncode == oracle.returncode == 0
        assert result.stdout == oracle.stdout


def test_inspect_legacy_symlinked_dir_not_counted(
    corpus: Path, tmp_path: Path
) -> None:
    archive = tmp_path / "link-snaps"
    shutil.copytree(corpus, archive)
    snapshots = archive / "teams" / "synthetic-team-1" / "source_snapshots"
    snapshots.mkdir(parents=True)
    (snapshots / "real.bin").write_bytes(b"12")
    elsewhere = archive / "elsewhere"
    elsewhere.mkdir()
    (elsewhere / "deep.bin").write_bytes(b"12345")
    (snapshots / "linked").symlink_to(elsewhere, target_is_directory=True)
    oracle = _check(archive)
    assert oracle.returncode == 0
    summary = json.loads(oracle.stdout)["source_snapshots"]
    assert summary["in_archive_files"] == 1
    assert summary["in_archive_bytes"] == 2


def test_inspect_bad_manifest_matches_python(tmp_path: Path) -> None:
    archive = tmp_path / "bad-manifest"
    (archive / "data").mkdir(parents=True)
    (archive / "data" / "timeline.json").write_text(
        json.dumps(
            {
                "agents": [], "phases": [], "edges": [],
                "events": [], "rollups": [], "summary_files": [],
            }
        )
    )
    (archive / "manifest.json").write_text("{not json")
    oracle = _python(archive)
    result = _rust(archive)
    assert result.returncode == oracle.returncode == 2
    assert result.stderr.startswith("wrkviz: ")
    assert oracle.stderr.startswith("wrkviz: ")
