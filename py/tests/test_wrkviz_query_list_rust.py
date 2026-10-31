"""Differential: the Rust `wrkviz query list` against the Python oracle.

The corpus is the same deterministic synthetic schema-3 archive
the inspect differential builds. A hand-written schema-1 archive
covers the fallback generation and multi-team filters. Outputs
compared are the exact stdout bytes, the exit code, and (where
the text is part of the contract) stderr.
"""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

import pytest

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"


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


def _check(args: list[str]) -> subprocess.CompletedProcess[str]:
    oracle = _cli(args, rust=False)
    result = _cli(args, rust=True)
    assert result.returncode == oracle.returncode, (result.stderr, oracle.stderr)
    assert result.stdout == oracle.stdout
    return oracle


@pytest.fixture(scope="module")
def corpus(tmp_path_factory: pytest.TempPathFactory) -> Path:
    sys.path.insert(0, str(AU_ROOT / "py"))
    from wrkviz.synthetic import SyntheticScale, build_corpus, generate_sources

    base = tmp_path_factory.mktemp("wrkviz-query-corpus")
    scale = SyntheticScale(teams=1, agents=14, days=3, tool_calls_per_agent=6, seed=7)
    generated = generate_sources(base / "sources", scale)
    archive = base / "archive"
    build_corpus(generated, archive)
    return archive.resolve()


@pytest.mark.parametrize("resource", ["teams", "agents", "phases", "rollups"])
def test_list_schema3_matches_python(corpus: Path, resource: str) -> None:
    oracle = _check(["query", "--output", str(corpus), "list", resource])
    assert oracle.returncode == 0
    payload = json.loads(oracle.stdout)
    assert payload["command"] == f"list {resource}"
    assert payload["count"] == len(payload["items"])


@pytest.mark.parametrize("fmt", ["jsonl", "markdown", "text"])
def test_list_formats_match_python(corpus: Path, fmt: str) -> None:
    _check(["query", "--output", str(corpus), "--format", fmt, "list", "agents"])
    _check(["query", "--output", str(corpus), "--format", fmt, "list", "teams"])


def test_list_filters_match_python(corpus: Path) -> None:
    agents = json.loads(_cli(
        ["query", "--output", str(corpus), "list", "agents"], rust=False
    ).stdout)["items"]
    rollups = json.loads(_cli(
        ["query", "--output", str(corpus), "list", "rollups"], rust=False
    ).stdout)["items"]
    assert agents and rollups
    agent_ref = agents[0]["ref"]
    kind = rollups[0]["kind"]
    for args in (
        ["list", "teams", "--team", "synthetic-team-1"],
        ["list", "teams", "--team", "no-such-team"],
        ["list", "agents", "--agent", agent_ref],
        ["list", "phases", "--agent", agent_ref],
        ["list", "rollups", "--kind", kind],
        ["list", "agents", "--start-time", "2000-01-01T00:00:00Z"],
        ["list", "agents", "--end-time", "2000-01-01T00:00:00Z"],
        ["list", "agents", "--start-time", "2000-01-01T00:00:00Z",
         "--end-time", "2100-01-01T00:00:00Z"],
    ):
        _check(["query", "--output", str(corpus), *args])


def test_list_filter_errors_match_python(corpus: Path) -> None:
    for args in (
        ["list", "agents", "--agent", "agent:nope::x"],
        ["list", "agents", "--agent", "team:synthetic-team-1"],
        ["list", "agents", "--start-time", "yesterday"],
        ["list", "agents", "--start-time", "2100-01-01T00:00:00Z",
         "--end-time", "2000-01-01T00:00:00Z"],
    ):
        oracle = _cli(["query", "--output", str(corpus), *args], rust=False)
        result = _cli(["query", "--output", str(corpus), *args], rust=True)
        assert result.returncode == oracle.returncode == 2
        assert result.stderr == oracle.stderr


def test_list_missing_archive_matches_python(tmp_path: Path) -> None:
    missing = tmp_path / "nope"
    oracle = _cli(["query", "--output", str(missing), "list", "teams"], rust=False)
    result = _cli(["query", "--output", str(missing), "list", "teams"], rust=True)
    assert result.returncode == oracle.returncode == 2
    assert result.stderr == oracle.stderr
    assert "no built timeline" in result.stderr


def _schema1_archive(root: Path) -> Path:
    (root / "data").mkdir(parents=True)
    timeline = {
        "schema_version": 1,
        "teams": [
            {"slug": "alpha", "label": "Alpha"},
            {"slug": "beta", "label": "Beta"},
        ],
        "agents": [
            {"id": "a1", "team": "alpha", "short_name": "Ann",
             "start_ms": 1000, "end_ms": 2000, "parent_id": None},
            {"id": "b1", "team": "beta", "short_name": "Bob",
             "start_ms": 500, "end_ms": 900, "parent_id": "beta::root"},
            {"id": "root", "team": "beta", "short_name": "Root",
             "start_ms": 100, "end_ms": 950, "parent_id": None},
        ],
        "phases": [
            {"id": "p1", "team": "alpha", "agent_id": "a1", "phrase": "work",
             "start_ms": 1100, "end_ms": 1500},
        ],
        "rollups": [
            {"id": "r1", "team": "beta", "kind": "daily", "label": "Day",
             "start_ms": 0, "end_ms": 86400000},
        ],
        "edges": [],
        "events": [],
        "summary_files": [],
    }
    (root / "data" / "timeline.json").write_text(json.dumps(timeline))
    return root


def test_list_schema1_matches_python(tmp_path: Path) -> None:
    archive = _schema1_archive(tmp_path / "legacy")
    for resource in ("teams", "agents", "phases", "rollups"):
        _check(["query", "--output", str(archive), "list", resource])
    _check(["query", "--output", str(archive), "list", "agents",
            "--team", "beta"])
    _check(["query", "--output", str(archive), "--format", "markdown",
            "list", "agents"])
    _check(["query", "--output", str(archive), "--format", "text",
            "list", "rollups"])
    oracle = _check(["query", "--output", str(archive), "list", "teams",
                     "--agent", "agent:beta::b1"])
    assert json.loads(oracle.stdout)["count"] == 2


def test_query_help_and_errors_match_python() -> None:
    for args in (["query", "--help"], ["query", "list", "--help"], ["query"],
                 ["query", "list", "teams"],
                 ["query", "--output", "/tmp/x", "list", "bogus"],
                 ["query", "--output", "/tmp/x", "bogus"]):
        oracle = _cli(args, rust=False)
        result = _cli(args, rust=True)
        assert result.returncode == oracle.returncode, (args, result.stderr)
        assert result.stdout == oracle.stdout, args
        assert result.stderr == oracle.stderr, args


def test_query_search_now_matches_python(corpus: Path) -> None:
    # `query search` was the last unported query action; it is
    # covered by test_wrkviz_query_search_rust.py. Here the
    # compatibility scope simply agrees with the oracle.
    oracle = _cli(["query", "--output", str(corpus), "search", "needle"],
                  rust=False)
    result = _cli(["query", "--output", str(corpus), "search", "needle"],
                  rust=True)
    assert result.returncode == oracle.returncode == 0
    assert result.stdout == oracle.stdout
    assert result.stderr == oracle.stderr
