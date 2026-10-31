"""Differential: Rust `wrkviz query search` against the Python oracle.

Covers search-v2 (`--in`) over the synthetic schema-3 corpus
with its search streams, the compatibility scopes, and
`query show` for `message:`/`tool:` references.
"""

from __future__ import annotations

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
    assert oracle.returncode == 0, oracle.stderr
    assert result.returncode == oracle.returncode, (result.stderr, oracle.stderr)
    assert result.stdout == oracle.stdout, args
    assert result.stderr == oracle.stderr, args
    return oracle


def _check_error(args: list[str]) -> None:
    oracle = _cli(args, rust=False)
    result = _cli(args, rust=True)
    assert oracle.returncode == 2, oracle.stdout
    assert result.returncode == oracle.returncode, (result.stderr, oracle.stderr)
    assert result.stdout == oracle.stdout == ""
    assert result.stderr == oracle.stderr, (args, result.stderr, oracle.stderr)


@pytest.fixture(scope="module")
def corpus(tmp_path_factory: pytest.TempPathFactory) -> Path:
    sys.path.insert(0, str(AU_ROOT / "py"))
    from wrkviz.synthetic import SyntheticScale, build_corpus, generate_sources

    base = tmp_path_factory.mktemp("wrkviz-query-search-corpus")
    scale = SyntheticScale(teams=1, agents=14, days=3, tool_calls_per_agent=6, seed=7)
    generated = generate_sources(base / "sources", scale)
    archive = base / "archive"
    build_corpus(generated, archive)
    return archive.resolve()


def _term(corpus: Path) -> str:
    payload = _check(
        ["query", "--output", str(corpus), "search", "--in", "all-transcript",
         "the", "--limit", "1"]
    ).stdout
    assert '"total_matches"' in payload
    import json

    data = json.loads(payload)
    assert data["total_matches"] > 0
    return "the"


def test_search_v2_matches_python(corpus: Path) -> None:
    term = _term(corpus)
    for extra in (
        [],
        ["--in", "owner-prompts"],
        ["--in", "agent-responses"],
        ["--in", "all-transcript", "--sort", "newest"],
        ["--in", "all-transcript", "--sort", "oldest"],
        ["--in", "all-transcript", "--match", "phrase"],
        ["--in", "all-transcript", "--match", "literal"],
        ["--in", "all-transcript", "--case-sensitive"],
        ["--in", "all-transcript", "--limit", "2", "--offset", "1"],
        ["--in", "all-transcript", "--prompt-author", "owner"],
        ["--in", "all-transcript", "--linkage", "linked"],
        ["--in", "all-transcript", "--role", "user"],
        ["--in", "all-transcript", "--team", "synthetic-team-1"],
    ):
        _check(["query", "--output", str(corpus), "search", *extra, term])
    for fmt in ("jsonl", "markdown", "text"):
        _check(["query", "--output", str(corpus), "--format", fmt,
                "search", "--in", "all-transcript", term, "--limit", "2"])
    _check(["query", "--output", str(corpus), "search", "--in",
            "all-transcript", "zzzznotfoundterm"])


def test_search_compat_and_show_match_python(corpus: Path) -> None:
    term = _term(corpus)
    _check(["query", "--output", str(corpus), "search", term])
    _check(["query", "--output", str(corpus), "search", "--scope",
            "transcripts", term, "--limit", "3"])
    _check(["query", "--output", str(corpus), "search", "--scope",
            "all", term, "--limit", "3"])
    import json

    payload = json.loads(_check(
        ["query", "--output", str(corpus), "search", "--in",
         "all-transcript", term, "--limit", "1"]
    ).stdout)
    reference = payload["items"][0]["ref"]
    _check(["query", "--output", str(corpus), "show", reference])


def test_search_errors_match_python(corpus: Path) -> None:
    base = ["query", "--output", str(corpus), "search"]
    _check_error([*base, "--in", "bogus", "the"])
    _check_error([*base, "--in", "all-transcript", "--scope", "all", "the"])
    _check_error([*base, "--match", "smart", "the"])
    _check_error([*base, "--in", "all-transcript", "--offset", "-1", "the"])
    _check_error([*base, "--in", "all-transcript", "--limit", "0", "the"])
    _check_error([*base, "--in", "all-transcript", "--limit", "abc", "the"])
    _check_error([*base, "--in", "all-transcript"])
    _check_error([*base, "--in", "all-transcript", "the", "extra"])
    for args in (["query", "search", "--help"],
                 ["query", "search", "--in", "bogus", "the"],
                 ["query", "--output", "/tmp/x", "search", "--bogus", "the"]):
        oracle = _cli(args, rust=False)
        result = _cli(args, rust=True)
        assert result.returncode == oracle.returncode, (args, result.stderr, oracle.stderr)
        assert result.stdout == oracle.stdout, args
        assert result.stderr == oracle.stderr, args
