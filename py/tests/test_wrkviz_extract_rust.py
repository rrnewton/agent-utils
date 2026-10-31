"""Differential: Rust `wrkviz extract-transcripts` against the Python oracle.

Both editions extract the same fixture archive; stdout, stderr,
exit code, and the resulting file tree must agree. Compressed
container bytes are the one named exception: they depend on the
zlib generation (the oracle's interpreter carries zlib 1.2.8,
the Rust edition links the system zlib), so containers compare
by inflated content and by every index field except the
compressed digests and lengths. Run records and manifests
compare structurally with timestamps and run ids normalized.
CPython JSON syntax-error wording inside skip summaries is
normalized (the cross-slice serde-vs-CPython gap).
"""

from __future__ import annotations

import gzip
import json
import re
import shutil
from typing import cast
import subprocess
import sys
from pathlib import Path

import pytest

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkviz"
BASE = 1_760_000_000_000

sys.path.insert(0, str(AU_ROOT / "py"))

from wrkviz.archive import JsonValue, canonical_json  # noqa: E402
from wrkviz.model import Agent, Event, SourceSnapshot, TaskNote, TeamData  # noqa: E402


def _event(eid: str, turn: str | None, ts: int, kind: str, text: str,
           line: int, native: str, ingress: str | None, author: str | None,
           thread: str = "root-1") -> Event:
    return Event(
        event_id=eid, thread_id=thread, turn_id=turn, timestamp_ms=ts,
        kind=kind, role=None, phase=None, text=text,
        content_availability="verbatim", encrypted_content=None, author=None,
        recipient=None, source_line=line, ingress_kind=ingress,
        author_kind=author, source_native_id=native,
        classification_version="classifier-1" if ingress else None,
    )


def _team() -> TeamData:
    return TeamData(
        team_slug="alpha-team", provider="codex", root_thread_id="root-1",
        display_timezone="America/New_York",
        sources=(SourceSnapshot(
            path="/src/a.jsonl", thread_id="root-1", size_bytes=100,
            mtime_ns=1, sha256="a" * 64, complete_bytes=100, line_count=10),),
        agents=(
            Agent(thread_id="root-1", parent_thread_id=None, agent_path="/root",
                  nickname="root", role="coordinator", depth=0,
                  started_at_ms=BASE, ended_at_ms=None, status="active",
                  source_path="/src/a.jsonl"),
            Agent(thread_id="child-1", parent_thread_id="root-1",
                  agent_path="/root/kid", nickname="kid", role="worker", depth=1,
                  started_at_ms=BASE, ended_at_ms=None, status="active",
                  source_path="/src/b.jsonl"),
        ),
        turns=(), tool_calls=(), edges=(),
        events=(
            _event("e1", "t1", BASE + 1000, "user_prompt",
                   "Please fix the parser.", 1, "n1", "chat", "external_or_unknown"),
            _event("e2", "t1", BASE + 2000, "assistant_message",
                   "Fixing it now.", 2, "n2", None, None),
            _event("e3", "t2", BASE + 3000, "user_prompt",
                   "Now add tests.", 3, "n3", "chat", "owner_human"),
            _event("e4", "t2", BASE + 4000, "assistant_message",
                   "Tests added.", 4, "n4", None, None),
            _event("e5", None, BASE + 5000, "system_input",
                   "System note for the coordinator.", 5, "n5", "system", "system"),
            _event("e6", "t9", BASE + 1500, "user_prompt",
                   "Subagent prompt, not coordinator.", 6, "n6", "chat", "agent",
                   thread="child-1"),
            _event("e7", "t3", BASE + 6000, "user_prompt",
                   "   ", 7, "n7", "chat", "external_or_unknown"),
        ),
        task_notes=(TaskNote(
            note_id=1, source_path="/tasks.md", task_source_ordinal=1,
            task_id="T1", title="Task", content="Body",
            created_at="2026-10-26T00:00:00Z", server_author=None,
            task_owner=None, upstream_present=True, projection_policy=None,
            projection_sha256=None),),
        window_start_ms=None, window_end_ms=None,
    )


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


def _build_archive(out: Path, *, corrupt_beta: bool = False,
                   rules: bool = False) -> Path:
    raw = out / "teams" / "alpha-team" / "raw"
    raw.mkdir(parents=True)
    marker: JsonValue = {"schema_version": 1, "tool": "wrkviz"}
    (out / ".wrkviz.json").write_text(canonical_json(marker))
    team_document = cast(JsonValue, _team().to_json_obj())
    (raw / "team.json").write_text(canonical_json(team_document))
    if corrupt_beta:
        raw2 = out / "teams" / "beta-team" / "raw"
        raw2.mkdir(parents=True)
        (raw2 / "team.json").write_text("{broken json")
    if rules:
        tdir = out / "extracted" / "transcripts"
        tdir.mkdir(parents=True)
        document: JsonValue = {"schema_version": 1, "rules": [
            {"rule_id": "r-owner", "team_slug": "alpha-team",
             "ingress_kind": "chat", "author_kind": "owner_human",
             "reason": "verified owner", "start_ms": None, "end_ms": None,
             "source_native_ids": ["n1"]},
            {"rule_id": "r-gone", "team_slug": "ghost-team",
             "ingress_kind": "chat", "author_kind": "agent",
             "reason": "no team", "start_ms": None, "end_ms": None,
             "source_native_ids": []},
            {"rule_id": "r-late", "team_slug": "alpha-team",
             "ingress_kind": "chat", "author_kind": "other_human",
             "reason": "bounded window", "start_ms": None,
             "end_ms": 9223372036854775807, "source_native_ids": ["n3"]},
        ]}
        (tdir / "authorship-rules.json").write_text(canonical_json(document))
    return out


def _run(command: list[str], archive: Path,
         args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [*command, "extract-transcripts", "--output", str(archive), *args],
        cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=120,
    )


def _syntax_normalized(text: str) -> str:
    text = re.sub(r"Expecting .*?\(char \d+\)", "<syntax>", text)
    return re.sub(
        r"key must be a string at line \d+ column \d+", "<syntax>", text)


def _normalize(text: str, archive: Path) -> str:
    text = text.replace(str(archive), "ARCH")
    text = re.sub(r"runs/[0-9A-Za-z-]+\.json", "runs/RUN.json", text)
    text = re.sub(
        r"JSONDecodeError: .*? \(char \d+\)", "JSONDecodeError: <syntax>", text)
    text = re.sub(
        r"JSONDecodeError: key must be a string at line \d+ column \d+",
        "JSONDecodeError: <syntax>", text)
    return text


def _tree(root: Path) -> dict[str, object]:
    result: dict[str, object] = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(root).as_posix()
        if rel.startswith("runs/"):
            record = json.loads(path.read_text())
            for key in ("run_id", "started_at", "completed_at"):
                record.pop(key, None)
            if isinstance(record.get("error"), str):
                record["error"] = _syntax_normalized(
                    str(record["error"]).replace(str(root), "ARCH"))
            record["command"] = [
                "ARCH" if str(arg) == str(root) else arg
                for arg in record["command"]
            ]
            mechanical = record.get("mechanical")
            if isinstance(mechanical, dict):
                extraction = mechanical.get("transcript_extraction")
                if isinstance(extraction, dict):
                    for skip in extraction.get("skipped_teams", []):
                        if isinstance(skip, dict) and isinstance(
                                skip.get("error"), str):
                            skip["error"] = _syntax_normalized(
                                str(skip["error"]))
            result["runs/RUN.json"] = record
        elif rel == "manifest.json":
            manifest = json.loads(path.read_text())
            for key in ("created_at", "last_run_at", "last_run_id"):
                manifest.pop(key, None)
            result[rel] = manifest
        elif rel.endswith(".jsonl.gz"):
            result[rel] = gzip.decompress(path.read_bytes())
        elif rel.endswith(".jsonl.gz.index.jsonl"):
            lines = [json.loads(line) for line in path.read_text().splitlines()]
            lines[0].pop("c_sha256", None)
            lines[0].pop("c_size", None)
            for member in lines[1:]:
                member.pop("c_len", None)
                member.pop("c_off", None)
            result[rel] = lines
        elif rel == "extracted/transcripts/manifest.json":
            manifest = json.loads(path.read_text())
            for entry in manifest["files"].values():
                entry.pop("c_sha256", None)
                entry.pop("c_bytes", None)
            for skip in manifest.get("skipped_teams", []):
                skip["error"] = _syntax_normalized(skip["error"])
            result[rel] = manifest
        else:
            result[rel] = path.read_bytes()
    return result


def _pair(tmp_path: Path, rust_wrkviz: Path, args: list[str], *,
          corrupt_beta: bool = False, rules: bool = False,
          second: bool = False) -> tuple[subprocess.CompletedProcess[str],
                                          subprocess.CompletedProcess[str]]:
    py_archive = _build_archive(tmp_path / "py-archive",
                                corrupt_beta=corrupt_beta, rules=rules)
    rs_archive = _build_archive(tmp_path / "rs-archive",
                                corrupt_beta=corrupt_beta, rules=rules)
    py_cmd = [sys.executable, str(PYTHON_WRKVIZ)]
    rs_cmd = [str(rust_wrkviz)]
    oracle = _run(py_cmd, py_archive, args)
    result = _run(rs_cmd, rs_archive, args)
    if second:
        oracle = _run(py_cmd, py_archive, args)
        result = _run(rs_cmd, rs_archive, args)
    assert result.returncode == oracle.returncode, (args, result.stderr)
    assert _normalize(result.stdout, rs_archive) == _normalize(oracle.stdout, py_archive), args
    assert _normalize(result.stderr, rs_archive) == _normalize(oracle.stderr, py_archive), args
    assert _tree(rs_archive) == _tree(py_archive), args
    return oracle, result


def test_extract_fresh_and_second_run(tmp_path: Path, rust_wrkviz: Path) -> None:
    _pair(tmp_path / "fresh", rust_wrkviz, [])
    _pair(tmp_path / "second", rust_wrkviz, [], second=True)


def test_extract_rules_and_skipped_team(tmp_path: Path, rust_wrkviz: Path) -> None:
    oracle, result = _pair(
        tmp_path, rust_wrkviz, [], corrupt_beta=True, rules=True)
    assert oracle.returncode == result.returncode == 2
    prompts = [
        json.loads(line)
        for line in gzip.open(
            tmp_path / "rs-archive" / "extracted" / "transcripts"
            / "prompts.jsonl.gz", "rt")
    ]
    by_text = {row["text"]: row for row in prompts}
    assert by_text["Please fix the parser."]["author_kind"] == "owner_human"
    assert by_text["Please fix the parser."]["authorship_rule_id"] == "r-owner"


def test_extract_team_selection(tmp_path: Path, rust_wrkviz: Path) -> None:
    _pair(tmp_path / "a", rust_wrkviz, ["--team", "alpha-team"])
    _pair(tmp_path / "b", rust_wrkviz, ["--team", "beta-team"], corrupt_beta=True)
    _pair(tmp_path / "c", rust_wrkviz,
          ["--team", "alpha-team", "--team", "alpha-team"])
    _pair(tmp_path / "d", rust_wrkviz, ["--team", "Bad_Slug"])
    _pair(tmp_path / "e", rust_wrkviz, ["--team", "ghost-team"])


def test_extract_cross_edition(tmp_path: Path, rust_wrkviz: Path) -> None:
    archive = _build_archive(tmp_path / "shared-archive")
    oracle = _run([sys.executable, str(PYTHON_WRKVIZ)], archive, [])
    assert oracle.returncode == 0
    result = _run([str(rust_wrkviz)], archive, [])
    assert result.returncode == 0, result.stderr
    again = _run([sys.executable, str(PYTHON_WRKVIZ)], archive, [])
    assert again.returncode == 0, again.stderr


def test_extract_cli_errors(rust_wrkviz: Path, tmp_path: Path) -> None:
    archive = _build_archive(tmp_path / "archive")
    for args in (
        ["extract-transcripts", "--help"],
        ["extract-transcripts"],
        ["extract-transcripts", "--output"],
        ["extract-transcripts", "--output", str(archive), "--bogus"],
        ["extract-transcripts", "--output", str(archive), "extra"],
        ["extract-transcripts", "--output", str(tmp_path / "missing")],
    ):
        oracle = subprocess.run(
            [sys.executable, str(PYTHON_WRKVIZ), *args],
            cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=60)
        result = subprocess.run(
            [str(rust_wrkviz), *args],
            cwd=AU_ROOT, capture_output=True, text=True, check=False, timeout=60)
        assert result.returncode == oracle.returncode, (args, result.stderr)
        assert result.stdout == oracle.stdout, args
        expected = oracle.stderr.replace(str(archive), "ARCH").replace(
            str(tmp_path / "missing"), "ARCH")
        actual = result.stderr.replace(str(archive), "ARCH").replace(
            str(tmp_path / "missing"), "ARCH")
        assert actual == expected, (args, actual, expected)


def test_extract_review_edge_cases(tmp_path: Path, rust_wrkviz: Path) -> None:
    py_archive = _build_archive(tmp_path / "py-archive", rules=True)
    rs_archive = _build_archive(tmp_path / "rs-archive", rules=True)
    # A symlink at the plain projection path is left alone.
    for archive in (py_archive, rs_archive):
        tdir = archive / "extracted" / "transcripts"
        (tdir / "prompts.jsonl").symlink_to("authorship-rules.json")
        cache = archive / "__pycache__"
        cache.mkdir(exist_ok=True)
        (cache / "query.pyc").write_bytes(b"keep")
        (cache / "query.cpython-312.pyc").write_bytes(b"drop")
    py_cmd = [sys.executable, str(PYTHON_WRKVIZ)]
    rs_cmd = [str(rust_wrkviz)]
    oracle = _run(py_cmd, py_archive, [])
    result = _run(rs_cmd, rs_archive, [])
    assert result.returncode == oracle.returncode == 2
    assert _normalize(result.stdout, rs_archive) == _normalize(oracle.stdout, py_archive)
    assert _normalize(result.stderr, rs_archive) == _normalize(oracle.stderr, py_archive)
    assert _tree(rs_archive) == _tree(py_archive)
    for archive in (py_archive, rs_archive):
        assert (archive / "extracted" / "transcripts" / "prompts.jsonl").is_symlink()
        # Without the legacy marker name, the bytecode cache is
        # not this exporter's to clean: both editions keep it.
        assert (archive / "__pycache__" / "query.pyc").is_file()
        assert (archive / "__pycache__" / "query.cpython-312.pyc").is_file()
        for path in (
            archive / "extracted" / "transcripts" / "manifest.json",
            archive / "extracted" / "transcripts" / "prompts.jsonl.gz",
        ):
            assert path.stat().st_mode & 0o777 == 0o600


def test_extract_corrupt_previous_manifest(tmp_path: Path,
                                           rust_wrkviz: Path) -> None:
    py_archive = _build_archive(tmp_path / "py-archive")
    rs_archive = _build_archive(tmp_path / "rs-archive")
    py_cmd = [sys.executable, str(PYTHON_WRKVIZ)]
    rs_cmd = [str(rust_wrkviz)]
    assert _run(py_cmd, py_archive, []).returncode == 0
    assert _run(rs_cmd, rs_archive, []).returncode == 0
    for archive in (py_archive, rs_archive):
        manifest = archive / "extracted" / "transcripts" / "manifest.json"
        document = json.loads(manifest.read_text())
        document["teams"] = {}
        manifest.write_text(json.dumps(document))
    oracle = _run(py_cmd, py_archive, [])
    result = _run(rs_cmd, rs_archive, [])
    assert result.returncode == oracle.returncode == 2
    assert _normalize(result.stderr, rs_archive) == _normalize(oracle.stderr, py_archive)


def test_extract_relative_output(tmp_path: Path, rust_wrkviz: Path) -> None:
    _build_archive(tmp_path / "py-archive")
    _build_archive(tmp_path / "rs-archive")
    for name, command in (("py-archive", [sys.executable, str(PYTHON_WRKVIZ)]),
                          ("rs-archive", [str(rust_wrkviz)])):
        done = subprocess.run(
            [*command, "extract-transcripts", "--output", f"./{name}"],
            cwd=tmp_path, capture_output=True, text=True, check=False, timeout=60)
        assert done.returncode == 0, done.stderr
        assert f"JSONL: {name}/extracted/transcripts/prompts.jsonl.gz" in done.stdout


def test_extract_copies_are_independent(tmp_path: Path) -> None:
    one = _build_archive(tmp_path / "one")
    two = tmp_path / "two"
    shutil.copytree(one, two)
    assert (two / "teams" / "alpha-team" / "raw" / "team.json").is_file()
