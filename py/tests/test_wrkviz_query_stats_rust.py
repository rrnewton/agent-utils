"""Differential: the Rust `wrkviz query stats` against the Python oracle.

The schema-3 corpus is the same deterministic synthetic archive
the list differential builds. A hand-written schema-1 archive
with a hand-written transcript export covers prompt authorship
classes, response linkage, summary availability, and the
manifest/sidecar integrity refusals. Message/tool/search
references remain for a later slice.
"""

from __future__ import annotations

import gzip
import hashlib
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
    assert oracle.returncode == 0, oracle.stderr
    assert result.returncode == oracle.returncode, (result.stderr, oracle.stderr)
    assert result.stdout == oracle.stdout, args
    assert result.stderr == oracle.stderr, args
    return oracle


def _check_error(args: list[str]) -> subprocess.CompletedProcess[str]:
    oracle = _cli(args, rust=False)
    result = _cli(args, rust=True)
    assert oracle.returncode == 2, oracle.stdout
    assert result.returncode == oracle.returncode, (result.stderr, oracle.stderr)
    assert result.stdout == oracle.stdout == ""
    assert result.stderr == oracle.stderr, (args, result.stderr, oracle.stderr)
    return oracle


@pytest.fixture(scope="module")
def corpus(tmp_path_factory: pytest.TempPathFactory) -> Path:
    sys.path.insert(0, str(AU_ROOT / "py"))
    from wrkviz.synthetic import SyntheticScale, build_corpus, generate_sources

    base = tmp_path_factory.mktemp("wrkviz-query-stats-corpus")
    scale = SyntheticScale(teams=1, agents=14, days=3, tool_calls_per_agent=6, seed=7)
    generated = generate_sources(base / "sources", scale)
    archive = base / "archive"
    build_corpus(generated, archive)
    extracted = subprocess.run(
        [sys.executable, str(PYTHON_WRKVIZ), "extract-transcripts",
         "--output", str(archive)],
        capture_output=True, text=True, check=False, timeout=900,
    )
    assert extracted.returncode == 0, extracted.stderr
    return archive.resolve()


def test_stats_schema3_matches_python(corpus: Path) -> None:
    base = ["query", "--output", str(corpus), "stats"]
    payload = json.loads(_check(base).stdout)
    assert payload["command"] == "stats"
    assert payload["content"]["all_prompts"]["records"] > 0
    assert payload["content"]["total_responses"]["records"] > 0
    for fmt in ("jsonl", "markdown", "text"):
        _check(["query", "--output", str(corpus), "--format", fmt, "stats"])
    _check([*base, "--team", "synthetic-team-1"])
    _check([*base, "--team", "no-such-team"])
    _check([*base, "--kind", "daily"])
    _check([*base, "--kind", "daily", "--kind", "weekly"])
    _check([*base, "--start-time", "2026-01-02T00:00:00Z"])
    _check([*base, "--end-time", "2026-01-02T00:00:00Z"])
    _check([*base, "--start-time", "2026-01-01T12:00:00Z",
            "--end-time", "2026-01-03T12:00:00Z", "--team", "synthetic-team-1"])


def _write_projection(root: Path, name: str, records: list[dict[str, object]]) -> dict[str, object]:
    raw = "".join(json.dumps(record) + "\n" for record in records).encode()
    compressed = gzip.compress(raw, mtime=0)
    container = f"{name}.gz"
    (root / container).write_bytes(compressed)
    timestamps: list[int] = []
    for record in records:
        stamp = record["timestamp_ms"]
        assert isinstance(stamp, int)
        timestamps.append(stamp)
    header = {
        "format": "wrkviz/seekable-jsonl-index",
        "version": 1,
        "codec": "gzip",
        "timestamp_key": "timestamp_ms",
        "timestamps_sorted": True,
        "record_count": len(records),
        "member_count": 1,
        "c_size": len(compressed),
        "u_size": len(raw),
        "c_sha256": hashlib.sha256(compressed).hexdigest(),
        "u_sha256": hashlib.sha256(raw).hexdigest(),
        "data_file": container,
    }
    member = {
        "c_off": 0, "c_len": len(compressed), "u_off": 0, "u_len": len(raw),
        "l0": 0, "n": len(records),
        "t0": min(timestamps) if timestamps else None,
        "t1": max(timestamps) if timestamps else None,
    }
    (root / f"{container}.index.jsonl").write_text(
        json.dumps(header) + "\n" + json.dumps(member) + "\n"
    )
    return {
        "container": "multi-member-gzip",
        "path": container,
        "index_path": f"{container}.index.jsonl",
        "sha256": hashlib.sha256(raw).hexdigest(),
        "bytes": len(raw),
        "records": len(records),
        "c_sha256": hashlib.sha256(compressed).hexdigest(),
        "c_bytes": len(compressed),
    }


def _hand_archive(root: Path) -> Path:
    transcripts = root / "extracted" / "transcripts"
    transcripts.mkdir(parents=True)
    files = {
        "prompts.jsonl": _write_projection(transcripts, "prompts.jsonl", [
            {"team_slug": "alpha", "timestamp_ms": 1100, "text": "hello wide world",
             "author_kind": "owner_human"},
            {"team_slug": "alpha", "timestamp_ms": 1200, "text": "automated prompt",
             "author_kind": "agent"},
            {"team_slug": "beta", "timestamp_ms": 1300, "text": "mystery words here",
             "author_kind": "carrier-pigeon"},
        ]),
        "messages.jsonl": _write_projection(transcripts, "messages.jsonl", [
            {"record_type": "prompt", "team_slug": "alpha", "timestamp_ms": 1150,
             "text": "not a response"},
            {"record_type": "response", "team_slug": "alpha", "timestamp_ms": 1400,
             "text": "linked reply text", "in_reply_to_prompt_id": "p1"},
            {"record_type": "response", "team_slug": "beta", "timestamp_ms": 1500,
             "text": "loose", "in_reply_to_prompt_id": None},
        ]),
        "system-inputs.jsonl": _write_projection(transcripts, "system-inputs.jsonl", []),
    }
    (transcripts / "manifest.json").write_text(json.dumps(
        {"schema_version": 1, "files": files}
    ))
    summaries = root / "teams" / "beta" / "summaries" / "daily"
    summaries.mkdir(parents=True)
    (summaries / "technical.md").write_text("# Technical\n\nBeta technical words.\n")
    (summaries / "plain.md").write_text("# Plain\n\nBeta plain.\n")
    (root / "data").mkdir(parents=True)
    timeline = {
        "schema_version": 1,
        "teams": [{"slug": "alpha", "label": "Alpha"}, {"slug": "beta", "label": "Beta"}],
        "project_overviews": [
            {"team": "alpha", "text": "Alpha overview words", "summary_available": True},
            {"team": "beta", "text": "unused", "summary_available": False},
        ],
        "agents": [
            {"id": "a1", "team": "alpha", "short_name": "Ann",
             "lifetime_summary": "Ann did things", "summary_available": True,
             "start_ms": 1000, "end_ms": 2000, "parent_id": None},
            {"id": "b1", "team": "beta", "short_name": "Bob",
             "lifetime_summary": "", "summary_available": False,
             "start_ms": 500, "end_ms": 900, "parent_id": None},
        ],
        "phases": [
            {"id": "p1", "team": "alpha", "agent_id": "a1", "phrase": "Working",
             "paragraph": "Did the work well", "summary_available": True,
             "start_ms": 1100, "end_ms": 1500},
            {"id": "p2", "team": "beta", "agent_id": "b1", "phrase": "Idle",
             "paragraph": "", "summary_available": False,
             "start_ms": 600, "end_ms": 700},
        ],
        "rollups": [
            {"id": "r1", "team": "beta", "kind": "daily", "label": "Day",
             "technical_path": "teams/beta/summaries/daily/technical.md",
             "plain_language_path": "teams/beta/summaries/daily/plain.md",
             "technical_summary_available": True,
             "plain_language_summary_available": False,
             "start_ms": 0, "end_ms": 86400000},
        ],
        "edges": [], "events": [], "summary_files": [],
    }
    (root / "data" / "timeline.json").write_text(json.dumps(timeline))
    return root


def test_stats_schema1_hand_archive_matches_python(tmp_path: Path) -> None:
    archive = _hand_archive(tmp_path / "hand")
    base = ["query", "--output", str(archive), "stats"]
    payload = json.loads(_check(base).stdout)
    content = payload["content"]
    assert content["human_prompts"] == {"records": 1, "words": 3, "utf8_bytes": 16}
    assert content["bot_prompts"]["records"] == 1
    assert content["unattributed_prompts"]["records"] == 1
    assert content["mechanically_linked_responses"]["records"] == 1
    assert content["unlinked_responses"] == {"records": 1, "words": 1, "utf8_bytes": 5}
    kinds = payload["summary_availability"]["by_kind"]
    assert kinds["project_overviews"] == {
        "available": 1, "unavailable": 1, "total": 2,
        "content": {"records": 1, "words": 3, "utf8_bytes": 20},
    }
    assert kinds["agent_lifetimes"]["available"] == 1
    assert kinds["agent_lifetimes"]["unavailable"] == 1
    assert kinds["work_phases"]["content"] == {"records": 1, "words": 5, "utf8_bytes": 25}
    for fmt in ("jsonl", "markdown", "text"):
        _check(["query", "--output", str(archive), "--format", fmt, "stats"])
    _check([*base, "--team", "alpha"])
    _check([*base, "--team", "beta", "--kind", "daily"])
    _check([*base, "--kind", "weekly"])
    _check([*base, "--start-time", "1970-01-01T00:00:01Z",
            "--end-time", "1970-01-01T00:00:02Z"])


def test_stats_integrity_refusals_match_python(tmp_path: Path) -> None:
    archive = _hand_archive(tmp_path / "tampered")
    manifest_path = archive / "extracted" / "transcripts" / "manifest.json"
    manifest = json.loads(manifest_path.read_text())
    manifest["files"]["prompts.jsonl"]["sha256"] = "0" * 64
    manifest_path.write_text(json.dumps(manifest))
    oracle = _check_error(["query", "--output", str(archive), "stats"])
    assert "indexes a stream the manifest does not describe" in oracle.stderr

    archive = _hand_archive(tmp_path / "missing")
    (archive / "extracted" / "transcripts" / "manifest.json").unlink()
    oracle = _check_error(["query", "--output", str(archive), "stats"])
    assert "no extracted transcripts at" in oracle.stderr


def test_stats_reviewer_edges_match_python(tmp_path: Path) -> None:
    archive = _hand_archive(tmp_path / "edges")
    base = ["query", "--output", str(archive), "stats"]
    _check([*base, "--te=alpha"])
    _check([*base, "--ki=daily"])
    _check(["query", "--out=" + str(archive), "stats"])
    _check_error([*base, "--kind", "yearly", "--help"])
    _check_error(["query", "stats", "--kind", "yearly"])
    _check_error([*base, "--start-time", "2026-01-02T00:00:60Z"])
    _check([*base, "--start-time", "2026-01-02T00:00:00+00:60"])
    # A timestamp beyond i64 is a value to filter, not a refusal.
    transcripts = archive / "extracted" / "transcripts"
    big = _write_projection(transcripts, "prompts.jsonl", [
        {"team_slug": "alpha", "timestamp_ms": 99999999999999999999999999,
         "text": "far future", "author_kind": "owner_human"},
    ])
    manifest_path = transcripts / "manifest.json"
    manifest = json.loads(manifest_path.read_text())
    manifest["files"]["prompts.jsonl"] = big
    manifest_path.write_text(json.dumps(manifest))
    payload = json.loads(_check(base).stdout)
    assert payload["content"]["human_prompts"]["records"] == 1
    payload = json.loads(_check(
        [*base, "--start-time", "1970-01-01T00:00:01Z",
         "--end-time", "1970-01-01T00:00:02Z"]
    ).stdout)
    assert payload["content"]["human_prompts"]["records"] == 0


def test_stats_help_and_errors_match_python(corpus: Path) -> None:
    for args in (["query", "stats", "--help"],
                 ["query", "stats", "--h"],
                 ["query", "--output", "/tmp/x", "stats", "--kind", "yearly"],
                 ["query", "--output", "/tmp/x", "stats", "--bogus"],
                 ["query", "--output", "/tmp/x", "stats", "extra"],
                 ["query", "--output", "/tmp/x", "stats", "--help=foo"],
                 ["query", "stats", "--help=foo"],
                 ["query", "--output", "/tmp/x", "stats", "-hx"],
                 ["query", "--output", "/tmp/x", "stats", "-h=x"],
                 ["query", "--output", "/tmp/x", "stats", "--team"],
                 ["query", "--output", "/tmp/x", "stats", "--team", "--kind"],
                 ["query", "--output", "/tmp/x", "stats", "--", "--team"],
                 ["query", "--output", "/tmp/x", "stats", "--start-time", "bogus"],
                 ["query", "--output", "/tmp/x", "stats",
                  "--start-time", "2026-01-02T00:00:00Z",
                  "--end-time", "2026-01-01T00:00:00Z"]):
        oracle = _cli(args, rust=False)
        result = _cli(args, rust=True)
        assert result.returncode == oracle.returncode, (args, result.stderr, oracle.stderr)
        assert result.stdout == oracle.stdout, args
        assert result.stderr == oracle.stderr, args
