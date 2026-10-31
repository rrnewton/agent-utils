"""Differential: Rust `wrkviz gc` against the Python oracle.

Twin archives are ingested, summarized (heuristic), and built by
each edition, then gc'd in every mode: dry-run text/JSON,
`--delete` (sweep into a timestamped trash generation with a
receipt), a second dry run over the trash, and `--empty-trash`.
stdout, stderr, exit code, and archive trees must agree, with
timestamps, run ids, trash generation stamps, and gzip-generation
bytes normalized. Fixtures only; no live sources.
"""

from __future__ import annotations

import gzip
import hashlib
import json
import re
import shutil
import subprocess
import sys
from pathlib import Path

import pytest

AU_ROOT = Path(__file__).resolve().parents[2]
PYTHON_WRKVIZ = AU_ROOT / "py" / "bin" / "wrkviz"
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkviz"
CODEX_ROOT = "root-thread"

TS_KEYS = {
    "run_id", "started_at", "completed_at", "created_at", "last_run_at",
    "last_run_id", "updated_at", "generated_at", "pid",
}
ID_KEYS = {"usage_receipt_id", "receipt_id", "batch_receipt_ids",
           "artifact_receipt_ids"}
GEN_KEYS = {"c_bytes", "c_size", "c_sha256", "c_off", "c_len",
            "shard_object_bytes", "shard_transfer_bytes",
            "bootstrap_bytes", "bootstrap_transfer_bytes"}
STAMP = re.compile(r"\.wrkviz-trash/\d{8}T\d{6}Z(?:-\d+)?")


@pytest.fixture(scope="module")
def rust_wrkviz() -> Path:
    subprocess.run(
        ["cargo", "build", "--release", "--manifest-path", "rs/Cargo.toml",
         "-p", "wrkviz"],
        cwd=AU_ROOT, check=True, capture_output=True, text=True, timeout=900,
    )
    return RUST_WRKVIZ


def _iso(seconds: float) -> str:
    hours, rem = divmod(int(seconds), 3600)
    minutes, secs = divmod(rem, 60)
    return f"2026-01-01T{hours:02d}:{minutes:02d}:{secs:02d}.000Z"


def _sessions(base: Path) -> Path:
    root = base / "codex-sessions"
    root.mkdir(parents=True, exist_ok=True)

    def rec(ts: int, kind: str, payload: dict[str, object]) -> dict[str, object]:
        return {"timestamp": _iso(ts), "type": kind, "payload": payload}

    records = [
        rec(1000, "session_meta", {
            "id": CODEX_ROOT, "session_id": CODEX_ROOT,
            "timestamp": _iso(1000), "cwd": "/work/project",
            "source": "cli", "thread_source": "user",
            "git": {"repository_url": "https://github.com/example/widget.git"}}),
        rec(1010, "event_msg", {
            "type": "task_started", "turn_id": "root-turn",
            "started_at": 1010}),
        rec(1011, "response_item", {
            "type": "message", "id": "msg-user", "role": "user",
            "content": [{"type": "input_text",
                         "text": "Build the widget parser and test it"}]}),
        rec(1011, "event_msg", {
            "type": "user_message",
            "message": "Build the widget parser and test it"}),
        rec(1015, "response_item", {
            "type": "message", "id": "msg-asst", "role": "assistant",
            "phase": "commentary",
            "content": [{"type": "output_text",
                         "text": "Implemented the widget parser and verified "
                                 "the fix with tests that passed"}]}),
        rec(1050, "event_msg", {
            "type": "task_complete", "turn_id": "root-turn",
            "started_at": 1010, "completed_at": 1050,
            "time_to_first_token_ms": 12, "last_agent_message": "Root done"}),
    ]
    path = root / "rollout-2026-01-01T00-00-00-root-thread.jsonl"
    path.write_bytes(b"".join(
        json.dumps(item, sort_keys=True).encode() + b"\n" for item in records))
    return root


def _run(command: list[str], args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command + args, cwd=AU_ROOT, capture_output=True, text=True,
        check=False, timeout=300,
    )


def _normalize(text: str, archive: Path) -> str:
    text = text.replace(str(archive), "ARCHIVE")
    text = text.replace(str(archive.resolve()), "ARCHIVE")
    text = STAMP.sub(".wrkviz-trash/GEN", text)
    # The twin archives differ in gzip member bytes, so the archive
    # total (and only the total) is generation-dependent.
    text = re.sub(r'"archive_bytes": \d+', '"archive_bytes": N', text)
    return text


def _scrub(value: object) -> object:
    if isinstance(value, dict):
        out: dict[str, object] = {}
        for key, item in value.items():
            if key in TS_KEYS:
                out[key] = "<ts>"
            elif key in ID_KEYS:
                out[key] = "<id>"
            elif key in GEN_KEYS:
                out[key] = "<gen>"
            elif key == "generation" and isinstance(item, str):
                out[key] = "GEN"
            elif key == "trash_generation":
                out[key] = "GEN" if item is not None else None
            elif key == "command" and isinstance(item, list):
                out[key] = [
                    re.sub(r"(py|rs)-[a-z]+", "OUT", str(arg)) for arg in item
                ]
            else:
                out[key] = _scrub(item)
        return out
    if isinstance(value, list):
        return [_scrub(item) for item in value]
    if isinstance(value, str):
        value = re.sub(r"(py|rs)-[a-z]+", "OUT", value)
        value = STAMP.sub(".wrkviz-trash/GEN", value)
        return re.sub(r"[0-9a-f]{64}", "HASH", value)
    return value


def _decoded(path: Path, raw: bytes) -> object:
    name = path.name
    if name.endswith(".gz"):
        try:
            raw = gzip.decompress(raw)
        except OSError:
            return raw.decode("utf-8", "replace")
        name = name[:-3]
    text = raw.decode("utf-8", "replace")
    if name.endswith(".json"):
        try:
            return _scrub(json.loads(text))
        except ValueError:
            return text
    if name.endswith(".jsonl") or ".jsonl." in name:
        lines = []
        for line in text.splitlines():
            if not line.strip():
                continue
            try:
                lines.append(_scrub(json.loads(line)))
            except ValueError:
                lines.append(line)
        return lines
    return text


def _tree(archive: Path) -> dict[str, object]:
    result: dict[str, object] = {}
    for path in sorted(archive.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(archive).as_posix()
        rel = STAMP.sub(".wrkviz-trash/GEN", rel)
        if rel.startswith("runs/"):
            rel = "runs/RUN.json"
        raw = path.read_bytes()
        value = _decoded(path, raw)
        if isinstance(value, str):
            value = STAMP.sub(".wrkviz-trash/GEN", value)
        if re.fullmatch(r"[0-9a-f]{64}\.json", path.name):
            digest = hashlib.sha256(json.dumps(
                value, sort_keys=True).encode()).hexdigest()[:16]
            directory = rel.rsplit("/", 1)[0]
            rel = f"{directory}/HASH-{digest}.json"
        result[rel] = value
    result["__run_file_count__"] = sum(
        1 for path in archive.rglob("runs/*.json") if path.is_file()
    )
    return result


def _built_archives(tmp_path: Path, rust_wrkviz: Path) -> dict[str, Path]:
    sessions = _sessions(tmp_path)
    archives = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(rust_wrkviz)])):
        archive = tmp_path / f"{label}-out"
        for args in (
            ["ingest", "--sessions-root", str(sessions),
             "--root-session", CODEX_ROOT, "--team", "widget",
             "--output", str(archive)],
            ["summarize", "--output", str(archive), "--team", "widget",
             "--backend", "heuristic", "--model", "offline"],
            ["build", "--output", str(archive), "--team", "widget"],
        ):
            outcome = _run(command, args)
            assert outcome.returncode == 0, outcome.stderr
        archives[label] = archive
    return archives


def _gc_both(
    archives: dict[str, Path],
    args: list[str],
    *,
    expect: int = 0,
) -> None:
    outcomes = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(RUST_WRKVIZ)])):
        outcomes[label] = _run(command, ["gc", "--output",
                                         str(archives[label]), *args])
    oracle, result = outcomes["py"], outcomes["rs"]
    assert oracle.returncode == expect, oracle.stderr
    assert result.returncode == oracle.returncode
    assert _normalize(result.stdout, archives["rs"]) == _normalize(
        oracle.stdout, archives["py"])
    assert _normalize(result.stderr, archives["rs"]) == _normalize(
        oracle.stderr, archives["py"])
    assert _tree(archives["rs"]) == _tree(archives["py"])


def test_gc_dry_run_reports(tmp_path: Path, rust_wrkviz: Path) -> None:
    archives = _built_archives(tmp_path, rust_wrkviz)
    _gc_both(archives, [])
    _gc_both(archives, ["--format", "json"])


def test_gc_sweep_and_empty_trash(tmp_path: Path, rust_wrkviz: Path) -> None:
    archives = _built_archives(tmp_path, rust_wrkviz)
    for archive in archives.values():
        legacy = archive / "data" / "timeline.json"
        legacy.write_text('{"schema_version": 1, "retired": true}\n')
        gzip_legacy = archive / "data" / "timeline.json.gz"
        gzip_legacy.write_bytes(gzip.compress(b'{"schema_version": 1}\n'))
    _gc_both(archives, [])
    _gc_both(archives, ["--delete"])
    _gc_both(archives, [])
    _gc_both(archives, ["--empty-trash"])
    _gc_both(archives, [])


def test_gc_both_passes_refused(tmp_path: Path, rust_wrkviz: Path) -> None:
    archives = _built_archives(tmp_path, rust_wrkviz)
    _gc_both(archives, ["--delete", "--empty-trash"], expect=2)


def test_gc_not_an_archive(tmp_path: Path, rust_wrkviz: Path) -> None:
    archives = {"py": tmp_path / "py-x", "rs": tmp_path / "rs-x"}
    for archive in archives.values():
        archive.mkdir(parents=True)
    _gc_both(archives, [], expect=2)


def test_gc_help(rust_wrkviz: Path) -> None:
    oracle = _run([sys.executable, str(PYTHON_WRKVIZ)], ["gc", "--help"])
    result = _run([str(rust_wrkviz)], ["gc", "--help"])
    assert oracle.returncode == 0
    assert result.returncode == 0
    assert result.stdout == oracle.stdout
    assert result.stderr == oracle.stderr


def test_gc_legacy_tree_untouched(tmp_path: Path, rust_wrkviz: Path) -> None:
    # A pre-retirement schema-2 tree with no complete schema 3 must be
    # reported held wholesale; deleting must move nothing.
    archives = _built_archives(tmp_path, rust_wrkviz)
    for archive in archives.values():
        shutil.rmtree(archive / "data" / "timeline-v3", ignore_errors=True)
        (archive / "data" / "timeline-v3.json").unlink(missing_ok=True)
        objects = archive / "data" / "timeline-v2" / "objects"
        objects.mkdir(parents=True, exist_ok=True)
        digest = "a" * 64
        (objects / f"{digest}.json").write_text('{"x": 1}\n')
        (archive / "data" / "timeline-v2.json").write_text(
            '{"schema_version": 2, "kind": "timeline-bootstrap"}\n')
    _gc_both(archives, [])
    _gc_both(archives, ["--delete"])
