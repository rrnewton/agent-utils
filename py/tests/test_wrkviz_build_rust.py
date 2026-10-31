"""Differential: Rust `wrkviz build` against the Python oracle.

Both editions ingest the same tiny Codex rollout into twin archives,
optionally summarize under the deterministic heuristic backend, then
build. stdout, stderr, exit code, and the archive trees must agree
with timestamps, run ids, and gzip-generation bytes normalized:
compressed members are compared by decompressed content, and the
seekable index/bootstrap compressed-size and compressed-digest fields
(which depend on the zlib generation) are scrubbed.

Fixtures only; no live sources or real model calls.
"""

from __future__ import annotations

import gzip
import hashlib
import json
import re
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
# Compressed-size/digest fields depend on the zlib generation that
# produced a member; the uncompressed digests are compared as-is.
GEN_KEYS = {"c_bytes", "c_size", "c_sha256", "c_off", "c_len",
            "shard_object_bytes", "shard_transfer_bytes"}


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
    text = re.sub(r"runs/[0-9A-Za-z-]+\.json", "runs/RUN.json", text)
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
            elif key == "command" and isinstance(item, list):
                out[key] = [
                    re.sub(r"(py|rs)-(out|multi)", "OUT", str(arg)) for arg in item
                ]
            else:
                out[key] = _scrub(item)
        return out
    if isinstance(value, list):
        return [_scrub(item) for item in value]
    if isinstance(value, str):
        value = re.sub(r"(py|rs)-(out|multi)", "OUT", value)
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
    if path.name.endswith(".jsonl") or ".jsonl." in path.name:
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
        if rel.startswith("runs/"):
            rel = "runs/RUN.json"
        raw = path.read_bytes()
        value = _decoded(path, raw)
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


def _ingest(tmp_path: Path, rust_wrkviz: Path) -> dict[str, Path]:
    sessions = _sessions(tmp_path)
    archives = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(rust_wrkviz)])):
        archive = tmp_path / f"{label}-out"
        outcome = _run(command, [
            "ingest", "--sessions-root", str(sessions),
            "--root-session", CODEX_ROOT, "--team", "widget",
            "--output", str(archive),
        ])
        assert outcome.returncode == 0, outcome.stderr
        archives[label] = archive
    return archives


def _both(
    archives: dict[str, Path],
    args: list[str],
    *,
    expect: int = 0,
) -> dict[str, subprocess.CompletedProcess[str]]:
    outcomes = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(RUST_WRKVIZ)])):
        outcomes[label] = _run(command, [
            *args[0:1], "--output", str(archives[label]),
            "--team", "widget", *args[1:],
        ])
    oracle, result = outcomes["py"], outcomes["rs"]
    assert oracle.returncode == expect, oracle.stderr
    assert result.returncode == oracle.returncode
    assert _normalize(result.stdout, archives["rs"]) == _normalize(
        oracle.stdout, archives["py"])
    assert _normalize(result.stderr, archives["rs"]) == _normalize(
        oracle.stderr, archives["py"])
    assert _tree(archives["rs"]) == _tree(archives["py"])
    return outcomes


def test_build_without_summaries(tmp_path: Path, rust_wrkviz: Path) -> None:
    archives = _ingest(tmp_path, rust_wrkviz)
    _both(archives, ["build"])


def test_build_after_summarize_and_rebuild(
    tmp_path: Path, rust_wrkviz: Path
) -> None:
    archives = _ingest(tmp_path, rust_wrkviz)
    _both(archives, ["summarize", "--backend", "heuristic",
                     "--model", "offline"])
    _both(archives, ["build"])
    outcomes = _both(archives, ["build"])
    assert "0 presentation files changed" in outcomes["rs"].stdout


def test_build_phase_minutes(tmp_path: Path, rust_wrkviz: Path) -> None:
    archives = _ingest(tmp_path, rust_wrkviz)
    _both(archives, ["build", "--phase-minutes", "5"])


def test_build_help(rust_wrkviz: Path) -> None:
    oracle = _run([sys.executable, str(PYTHON_WRKVIZ)], ["build", "--help"])
    result = _run([str(rust_wrkviz)], ["build", "--help"])
    assert oracle.returncode == 0
    assert result.returncode == 0
    assert result.stdout == oracle.stdout
    assert result.stderr == oracle.stderr


CHILD = "child-thread"


def _sessions_multi(base: Path) -> Path:
    root = _sessions(base / "multi-src")
    sessions = base / "multi-sessions"
    sessions.mkdir(parents=True, exist_ok=True)
    for path in root.iterdir():
        (sessions / path.name).write_bytes(path.read_bytes())

    def rec(ts: int, kind: str, payload: dict[str, object]) -> dict[str, object]:
        return {"timestamp": _iso(ts), "type": kind, "payload": payload}

    child_meta: dict[str, object] = {
        "id": CHILD, "session_id": CODEX_ROOT,
        "parent_thread_id": CODEX_ROOT, "timestamp": _iso(1020),
        "cwd": "/work/project", "thread_source": "subagent",
        "agent_path": "/root/worker", "agent_nickname": "Ada",
        "source": {"subagent": {"thread_spawn": {
            "parent_thread_id": CODEX_ROOT, "depth": 1,
            "agent_path": "/root/worker", "agent_nickname": "Ada",
            "agent_role": "reviewer"}}},
    }
    records = [
        rec(1020, "session_meta", child_meta),
        rec(1030, "event_msg", {
            "type": "task_started", "turn_id": "child-turn",
            "started_at": 1030}),
        rec(1031, "response_item", {
            "type": "message", "id": "msg-child-user", "role": "user",
            "content": [{"type": "input_text",
                         "text": "Review the widget parser"}]}),
        rec(1031, "event_msg", {
            "type": "user_message", "message": "Review the widget parser"}),
        rec(1040, "response_item", {
            "type": "message", "id": "msg-child-asst", "role": "assistant",
            "phase": "commentary",
            "content": [{"type": "output_text",
                         "text": "Reviewed the widget parser and verified "
                                 "the edge cases were tested"}]}),
        rec(1050, "event_msg", {
            "type": "task_complete", "turn_id": "child-turn",
            "started_at": 1030, "completed_at": 1050}),
    ]
    path = sessions / "rollout-2026-01-01T00-00-00-child-thread.jsonl"
    path.write_bytes(b"".join(
        json.dumps(item, sort_keys=True).encode() + b"\n"
        for item in records))
    return sessions


def test_build_multi_agent(tmp_path: Path, rust_wrkviz: Path) -> None:
    sessions = _sessions_multi(tmp_path)
    archives = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(rust_wrkviz)])):
        archive = tmp_path / f"{label}-multi"
        outcome = _run(command, [
            "ingest", "--sessions-root", str(sessions),
            "--root-session", CODEX_ROOT, "--team", "widget",
            "--output", str(archive),
        ])
        assert outcome.returncode == 0, outcome.stderr
        archives[label] = archive
    _both(archives, ["summarize", "--backend", "heuristic",
                     "--model", "offline"])
    _both(archives, ["build"])


def _tamper_archives(
    tmp_path: Path,
    rust_wrkviz: Path,
    mutate: object,
) -> dict[str, Path]:
    archives = _ingest(tmp_path, rust_wrkviz)
    _both(archives, ["summarize", "--backend", "heuristic",
                     "--model", "offline"])
    for archive in archives.values():
        mutate(archive)  # type: ignore[operator]
    return archives


def test_build_legacy_overview_tampered(
    tmp_path: Path, rust_wrkviz: Path
) -> None:
    def mutate(archive: Path) -> None:
        path = (archive / "teams" / "widget" / "summary_data"
                / "project_overview.json")
        current = json.loads(path.read_text())
        legacy = {
            "schema_version": 2,
            "style": current["style"],
            "source": {
                "event_ids": [],
                "start_ms": 0,
                "end_ms": 1,
                "context_sha256": "0" * 64,
                "transcript": "tampered transcript",
            },
            "summary": current["summary"],
            "knowledge_epoch": {
                "epoch_id": "knowledge-1",
                "cutoff_ms": 1,
                "cutoff_reason": "archive-window-end",
            },
        }
        path.write_text(json.dumps(legacy, sort_keys=True))

    archives = _tamper_archives(tmp_path, rust_wrkviz, mutate)
    _both(archives, ["build"], expect=2)


def test_build_invalid_pull_cache(tmp_path: Path, rust_wrkviz: Path) -> None:
    def mutate(archive: Path) -> None:
        path = (archive / "teams" / "widget" / "summary_data"
                / "github" / "pulls.json")
        path.parent.mkdir(parents=True, exist_ok=True)
        record = {
            "repository": "bad", "number": 0, "title": "",
            "state": "merged", "draft": False, "merged_at": None,
            "body_excerpt": "", "base_ref": "", "head_label": "",
            "author": None, "updated_at": "", "etag": 123,
            "fetched_at": "",
        }
        path.write_text(json.dumps(
            {"schema_version": 1, "records": {"bad#0": record}},
            sort_keys=True))

    archives = _tamper_archives(tmp_path, rust_wrkviz, mutate)
    _both(archives, ["build"], expect=2)


def test_build_parent_symlink_refused(
    tmp_path: Path, rust_wrkviz: Path
) -> None:
    def mutate(archive: Path) -> None:
        summaries = archive / "teams" / "widget" / "summaries"
        import shutil
        shutil.rmtree(summaries, ignore_errors=True)
        summaries.symlink_to(
            archive / "teams" / "widget" / "summary_data",
            target_is_directory=True,
        )

    archives = _tamper_archives(tmp_path, rust_wrkviz, mutate)
    _both(archives, ["build"], expect=2)


def test_build_missing_team(tmp_path: Path, rust_wrkviz: Path) -> None:
    archives = {
        label: tmp_path / f"{label}-out" for label in ("py", "rs")
    }
    outcomes = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(rust_wrkviz)])):
        outcomes[label] = _run(command, [
            "build", "--output", str(archives[label]), "--team", "widget",
        ])
    oracle, result = outcomes["py"], outcomes["rs"]
    assert oracle.returncode == 2, oracle.stdout + oracle.stderr
    assert result.returncode == oracle.returncode
    assert _normalize(result.stdout, archives["rs"]) == _normalize(
        oracle.stdout, archives["py"])
    assert _normalize(result.stderr, archives["rs"]) == _normalize(
        oracle.stderr, archives["py"])
    assert _tree(archives["rs"]) == _tree(archives["py"])
