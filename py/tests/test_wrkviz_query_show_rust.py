"""Differential: the Rust `wrkviz query show` against the Python oracle.

The schema-3 corpus is the same deterministic synthetic archive
the list differential builds. A hand-written schema-1 archive
covers relationship expansion, phase details stored plain and
compressed, and rollup markdown. The only intentional divergence
is a `message:`/`tool:` transcript search reference, which the
Rust edition refuses by name until its search slice lands.
"""

from __future__ import annotations

import gzip
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


def _check_error(args: list[str]) -> subprocess.CompletedProcess[str]:
    oracle = _cli(args, rust=False)
    result = _cli(args, rust=True)
    assert oracle.returncode == 2, oracle.stdout
    assert result.returncode == oracle.returncode, (result.stderr, oracle.stderr)
    assert result.stdout == oracle.stdout == ""
    assert result.stderr == oracle.stderr, (args, result.stderr, oracle.stderr)
    return oracle


def _listed(archive: Path, resource: str) -> list[dict[str, object]]:
    payload = json.loads(_cli(
        ["query", "--output", str(archive), "list", resource], rust=False
    ).stdout)
    items = payload["items"]
    assert isinstance(items, list) and items
    return items


@pytest.fixture(scope="module")
def corpus(tmp_path_factory: pytest.TempPathFactory) -> Path:
    sys.path.insert(0, str(AU_ROOT / "py"))
    from wrkviz.synthetic import SyntheticScale, build_corpus, generate_sources

    base = tmp_path_factory.mktemp("wrkviz-query-show-corpus")
    scale = SyntheticScale(teams=1, agents=14, days=3, tool_calls_per_agent=6, seed=7)
    generated = generate_sources(base / "sources", scale)
    archive = base / "archive"
    build_corpus(generated, archive)
    return archive.resolve()


def test_show_schema3_matches_python(corpus: Path) -> None:
    refs = [
        "team:synthetic-team-1",
        str(_listed(corpus, "agents")[0]["ref"]),
        str(_listed(corpus, "phases")[0]["ref"]),
        str(_listed(corpus, "rollups")[0]["ref"]),
    ]
    for reference in refs:
        oracle = _check(["query", "--output", str(corpus), "show", reference])
        payload = json.loads(oracle.stdout)
        assert payload["command"] == f"show {reference}"
        assert payload["count"] == 1
    phase_ref = refs[2]
    _check(["query", "--output", str(corpus), "show", phase_ref, "--transcript"])
    _check(["query", "--output", str(corpus), "--format", "markdown",
            "show", refs[1]])
    _check(["query", "--output", str(corpus), "--format", "text",
            "show", phase_ref])
    _check(["query", "--output", str(corpus), "--format", "jsonl",
            "show", refs[3]])


def test_show_errors_match_python(corpus: Path) -> None:
    for reference in ("team:nope", "agent:nope::x", "phase", "wat:x"):
        oracle = _cli(["query", "--output", str(corpus), "show", reference], rust=False)
        result = _cli(["query", "--output", str(corpus), "show", reference], rust=True)
        assert result.returncode == oracle.returncode == 2
        assert result.stdout == oracle.stdout == ""
        assert result.stderr == oracle.stderr


def test_show_transcript_reference_matches_python(corpus: Path) -> None:
    # Ported in the search slice: an unknown message reference
    # now answers with the oracle's unknown-reference error.
    _check_error(
        ["query", "--output", str(corpus), "show", "message:synthetic-team-1::x"]
    )


def _schema1_archive(root: Path) -> Path:
    (root / "data" / "details" / "alpha").mkdir(parents=True)
    (root / "data" / "details" / "beta").mkdir(parents=True)
    summaries = root / "teams" / "beta" / "summaries" / "daily"
    summaries.mkdir(parents=True)
    (summaries / "technical.md").write_text("# Technical\n\nBeta technical.\n")
    (summaries / "plain.md").write_text("# Plain\n\nBeta plain.\n")
    (root / "data" / "details" / "alpha" / "p1.json").write_text(
        '{"summary": "Alpha phase",'
        ' "transcript": [{"role": "user", "at_ms": 1100, "text": "hello"}],'
        ' "big": 1e16, "small": 1e-05, "huge": 1.5e+300, "tiny": 5e-324,'
        ' "exact": 123456789012345678901234567890, "plain": 0.1}'
    )
    (root / "data" / "details" / "alpha" / ".json").write_text("{}")
    (root / "teams" / "beta" / "summaries" / "daily" / ".md").write_text("# Dot\n")
    (root / "data" / "details" / "alpha" / "bad.json").write_text(json.dumps({
        "summary": "Bad transcript",
        "transcript": ["oops"],
    }))
    with gzip.open(root / "data" / "details" / "beta" / "p2.json.gz", "wt") as handle:
        json.dump({
            "summary": "Beta phase",
            "transcript": [{"role": "agent", "at_ms": 300, "text": "reply"}],
        }, handle)
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
             "detail_path": "data/details/alpha/p1.json",
             "start_ms": 1100, "end_ms": 1500},
            {"id": "p2", "team": "beta", "agent_id": "root", "phrase": "compressed",
             "detail_path": "data/details/beta/p2.json",
             "start_ms": 200, "end_ms": 400},
            {"id": "p3", "team": "alpha", "agent_id": "a1", "phrase": "dotfile",
             "detail_path": "data/details/alpha/.json",
             "start_ms": 1600, "end_ms": 1700},
            {"id": "p4", "team": "alpha", "agent_id": "a1", "phrase": "bad transcript",
             "detail_path": "data/details/alpha/bad.json",
             "start_ms": 1800, "end_ms": 1900},
        ],
        "rollups": [
            {"id": "r1", "team": "beta", "kind": "daily", "label": "Day",
             "technical_path": "teams/beta/summaries/daily/technical.md",
             "plain_language_path": "teams/beta/summaries/daily/plain.md",
             "technical_summary_available": True,
             "plain_language_summary_available": True,
             "start_ms": 0, "end_ms": 86400000},
            {"id": "r2", "team": "beta", "kind": "weekly", "label": "Week",
             "technical_path": "teams/beta/summaries/daily/.md",
             "plain_language_path": "teams/beta/summaries/daily/plain.md",
             "technical_summary_available": True,
             "plain_language_summary_available": False,
             "start_ms": 0, "end_ms": 604800000},
        ],
        "edges": [],
        "events": [],
        "summary_files": [],
    }
    (root / "data" / "timeline.json").write_text(json.dumps(timeline))
    return root


def test_show_schema1_matches_python(tmp_path: Path) -> None:
    archive = _schema1_archive(tmp_path / "legacy")
    for reference in (
        "team:alpha",
        "team:beta",
        "agent:alpha::a1",
        "agent:beta::b1",
        "agent:beta::root",
        "phase:alpha::p1",
        "phase:beta::p2",
        "rollup:beta::daily::0",
    ):
        _check(["query", "--output", str(archive), "show", reference])
    _check(["query", "--output", str(archive), "show", "phase:beta::p2",
            "--transcript"])
    _check(["query", "--output", str(archive), "--format", "markdown",
            "show", "phase:alpha::p1", "--transcript"])
    beta = json.loads(_check(
        ["query", "--output", str(archive), "show", "agent:beta::root"]
    ).stdout)["items"][0]
    assert beta["child_refs"] == ["agent:beta::b1"]
    assert beta["phase_refs"] == ["phase:beta::p2"]


def test_show_reviewer_edges_match_python(tmp_path: Path) -> None:
    archive = _schema1_archive(tmp_path / "edges")
    detail = json.loads(_check(
        ["query", "--output", str(archive), "show", "phase:alpha::p1"]
    ).stdout)["items"][0]["detail"]
    assert detail["big"] == 1e16
    assert detail["exact"] == 123456789012345678901234567890
    oracle = _check_error(
        ["query", "--output", str(archive), "show", "phase:alpha::p3"]
    )
    assert "phase detail path is outside data/details" in oracle.stderr
    oracle = _check_error(
        ["query", "--output", str(archive), "show", "rollup:beta::weekly::0"]
    )
    assert "rollup path is outside teams/beta/summaries" in oracle.stderr
    oracle = _check_error(
        ["query", "--output", str(archive), "--format", "markdown",
         "show", "phase:alpha::p4", "--transcript"]
    )
    assert "detail.transcript[0]: expected an object" in oracle.stderr


def test_show_help_and_errors_match_python() -> None:
    for args in (["query", "show", "--help"],
                 ["query", "show", "--h"],
                 ["query", "--output", "/tmp/x", "show"],
                 ["query", "--output", "/tmp/x", "show", "team:a", "team:b"],
                 ["query", "--output", "/tmp/x", "show", "team:a", "team:b", "team:c"],
                 ["query", "--output", "/tmp/x", "show", "--bogus", "team:a"],
                 ["query", "--output", "/tmp/x", "show", "--bogus", "--also-bad", "team:a"],
                 ["query", "--output", "/tmp/x", "show", "--transcript=x", "team:a"],
                 ["query", "--output", "/tmp/x", "show", "--trans=1", "team:a"],
                 ["query", "--output", "/tmp/x", "show", "--help=foo"],
                 ["query", "--output", "/tmp/x", "show", "--", "team:a"],
                 ["query", "--output", "/tmp/x", "show", "-x"],
                 ["query", "--output", "/tmp/x", "show", "-x", "team:a"]):
        oracle = _cli(args, rust=False)
        result = _cli(args, rust=True)
        assert result.returncode == oracle.returncode, (args, result.stderr)
        assert result.stdout == oracle.stdout, args
        assert result.stderr == oracle.stderr, args
