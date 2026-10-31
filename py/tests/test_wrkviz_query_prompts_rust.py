"""Differential: Rust `wrkviz query prompts`/`messages` vs the Python oracle.

The schema-3 corpus is the deterministic synthetic archive with
`extract-transcripts` applied. A hand-written transcript export
covers ordinal ranges, authorship selection, tail/limit paging,
response linkage, and `--verify`.
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

    base = tmp_path_factory.mktemp("wrkviz-query-prompts-corpus")
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


def test_prompts_messages_schema3_match_python(corpus: Path) -> None:
    for action in ("prompts", "messages"):
        base = ["query", "--output", str(corpus), action]
        payload = json.loads(_check(base).stdout)
        assert payload["command"] == action
        _check([*base, "--which", "all"])
        _check([*base, "--which", "bot"])
        _check([*base, "--limit", "2"])
        _check([*base, "--tail", "2"])
        _check([*base, "--verify"])
        _check([*base, "--team", "synthetic-team-1"])
        _check([*base, "--start-time", "2026-01-02T00:00:00Z"])
        for fmt in ("jsonl", "markdown", "text"):
            _check(["query", "--output", str(corpus), "--format", fmt, action,
                    "--limit", "3"])
    prompts = json.loads(_check(
        ["query", "--output", str(corpus), "prompts", "--which", "all"]
    ).stdout)
    assert prompts["count"] > 0
    first = prompts["items"][0]
    _check(["query", "--output", str(corpus), "prompts", "--which", "all",
            "--range", str(int(first["ordinal"]))])
    _check(["query", "--output", str(corpus), "messages", "--which", "all",
            "--range", "1-2"])


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
    prompt_records: list[dict[str, object]] = [
        {"ordinal": 1, "record_id": "p1", "team_slug": "alpha",
         "timestamp_ms": 1100, "text": "first human prompt",
         "author_kind": "owner_human"},
        {"ordinal": 2, "record_id": "p2", "team_slug": "alpha",
         "timestamp_ms": 1200, "text": "bot prompt",
         "author_kind": "agent"},
        {"ordinal": 3, "record_id": "p3", "team_slug": "beta",
         "timestamp_ms": 1300, "text": "second human prompt",
         "author_kind": "other_human"},
    ]
    message_records: list[dict[str, object]] = [
        {"record_type": "prompt", "record_id": "p1", "team_slug": "alpha",
         "timestamp_ms": 1100, "text": "first human prompt"},
        {"record_type": "response", "record_id": "r1", "team_slug": "alpha",
         "timestamp_ms": 1150, "text": "reply to first",
         "in_reply_to_prompt_id": "p1"},
        {"record_type": "prompt", "record_id": "p2", "team_slug": "alpha",
         "timestamp_ms": 1200, "text": "bot prompt"},
        {"record_type": "prompt", "record_id": "p3", "team_slug": "beta",
         "timestamp_ms": 1300, "text": "second human prompt"},
        {"record_type": "response", "record_id": "r2", "team_slug": "beta",
         "timestamp_ms": 1400, "text": "reply to second",
         "in_reply_to_prompt_id": "p3"},
        {"record_type": "response", "record_id": "r3", "team_slug": "beta",
         "timestamp_ms": 1500, "text": "reply to nobody",
         "in_reply_to_prompt_id": "nope"},
    ]
    files = {
        "prompts.jsonl": _write_projection(transcripts, "prompts.jsonl", prompt_records),
        "messages.jsonl": _write_projection(transcripts, "messages.jsonl", message_records),
        "system-inputs.jsonl": _write_projection(transcripts, "system-inputs.jsonl", []),
    }
    (transcripts / "manifest.json").write_text(json.dumps(
        {"schema_version": 1, "files": files}
    ))
    (root / "data").mkdir(parents=True)
    (root / "data" / "timeline.json").write_text(json.dumps({
        "schema_version": 1,
        "teams": [{"slug": "alpha", "label": "Alpha"}, {"slug": "beta", "label": "Beta"}],
        "agents": [], "phases": [], "rollups": [],
        "edges": [], "events": [], "summary_files": [],
    }))
    return root


def test_prompts_messages_hand_archive_match_python(tmp_path: Path) -> None:
    archive = _hand_archive(tmp_path / "hand")
    for action in ("prompts", "messages"):
        base = ["query", "--output", str(archive), action]
        _check(base)
        _check([*base, "--which", "all"])
        _check([*base, "--which", "bot"])
        _check([*base, "--limit", "1"])
        _check([*base, "--tail", "1"])
        _check([*base, "--verify"])
        _check([*base, "--range", "1-2"])
        _check([*base, "--range", "3"])
        _check([*base, "--team", "beta"])
        _check([*base, "--start-time", "1970-01-01T00:00:01Z",
                "--end-time", "1970-01-01T00:00:02Z"])
        for fmt in ("jsonl", "markdown", "text"):
            _check(["query", "--output", str(archive), "--format", fmt, action,
                    "--which", "all"])
    messages = json.loads(_check(
        ["query", "--output", str(archive), "messages", "--which", "all"]
    ).stdout)
    responses = [item for item in messages["items"]
                 if item.get("record_type") == "response"]
    assert [item["prompt_ordinal"] for item in responses] == [1, 3]


def test_prompts_messages_errors_match_python(tmp_path: Path) -> None:
    archive = _hand_archive(tmp_path / "errs")
    for action in ("prompts", "messages"):
        base = ["query", "--output", str(archive), action]
        _check_error([*base, "--range", "0"])
        _check_error([*base, "--range", "3-1"])
        _check_error([*base, "--range", "x"])
        _check_error([*base, "--limit", "0"])
        _check_error([*base, "--limit", "abc"])
        _check_error([*base, "--limit", "1", "--tail", "1"])
        _check_error([*base, "--which", "other"])
        _check_error([*base, "--verify=x"])
    for args in (["query", "prompts", "--help"],
                 ["query", "messages", "--help"],
                 ["query", "--output", "/tmp/x", "prompts", "--bogus"],
                 ["query", "--output", "/tmp/x", "messages", "--limit"],
                 ["query", "prompts", "--which", "other"]):
        oracle = _cli(args, rust=False)
        result = _cli(args, rust=True)
        assert result.returncode == oracle.returncode, (args, result.stderr, oracle.stderr)
        assert result.stdout == oracle.stdout, args
        assert result.stderr == oracle.stderr, args
