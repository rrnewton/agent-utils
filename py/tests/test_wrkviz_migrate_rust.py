"""Differential: Rust `wrkviz migrate-build-state` vs the oracle.

Twin archives are ingested by each edition; build state is then
reported (dry run), moved to the sibling store, and built again
from the external store. stdout, stderr, exit code, and both
archive and store trees must agree, with timestamps, run ids, and
gzip-generation bytes normalized. Fixtures only.
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
RUST_WRKVIZ = AU_ROOT / "rs" / "target" / "release" / "wrkvizd"
CODEX_ROOT = "root-thread"

TS_KEYS = {
    "edition", "run_id", "started_at", "completed_at", "created_at", "last_run_at",
    "last_run_id", "updated_at", "generated_at", "pid",
}
ID_KEYS = {"usage_receipt_id", "receipt_id", "batch_receipt_ids",
           "artifact_receipt_ids"}
GEN_KEYS = {"c_bytes", "c_size", "c_sha256", "c_off", "c_len",
            "shard_object_bytes", "shard_transfer_bytes",
            "bootstrap_bytes", "bootstrap_transfer_bytes"}


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


def _normalize(text: str, base: Path) -> str:
    text = text.replace(str(base), "BASE")
    text = text.replace(str(base.resolve()), "BASE")
    return re.sub(r"(py|rs)-[a-z]+", "OUT", text)


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
                    re.sub(r"(py|rs)-[a-z]+", "OUT", str(arg)) for arg in item
                ]
            else:
                out[key] = _scrub(item)
        return out
    if isinstance(value, list):
        return [_scrub(item) for item in value]
    if isinstance(value, str):
        value = re.sub(r"(py|rs)-[a-z]+", "OUT", value)
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


def _tree(root: Path) -> dict[str, object]:
    result: dict[str, object] = {}
    for path in sorted(root.rglob("*")):
        if not path.is_file() or path.is_symlink():
            continue
        rel = path.relative_to(root).as_posix()
        if "/runs/" in f"/{rel}" or rel.startswith("runs/"):
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
        1 for path in root.rglob("runs/*.json") if path.is_file()
    )
    return result


def _ingested(tmp_path: Path, rust_wrkviz: Path) -> dict[str, Path]:
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
    base: Path,
    archives: dict[str, Path],
    verb: str,
    args: list[str],
    *,
    expect: int = 0,
) -> None:
    outcomes = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(RUST_WRKVIZ)])):
        outcomes[label] = _run(command, [
            verb, "--output", str(archives[label]), *args,
        ])
    oracle, result = outcomes["py"], outcomes["rs"]
    assert oracle.returncode == expect, oracle.stderr
    assert result.returncode == oracle.returncode
    assert _normalize(result.stdout, archives["rs"].parent) == _normalize(
        oracle.stdout, archives["py"].parent)
    assert _normalize(result.stderr, archives["rs"].parent) == _normalize(
        oracle.stderr, archives["py"].parent)
    assert _tree(archives["rs"].parent) == _tree(archives["py"].parent)
    _ = base


def test_migrate_build_state_round_trip(
    tmp_path: Path, rust_wrkviz: Path
) -> None:
    archives = _ingested(tmp_path, rust_wrkviz)
    _both(tmp_path, archives, "migrate-build-state", [])
    _both(tmp_path, archives, "migrate-build-state", ["--move"])
    # Second dry run: nothing left inside.
    _both(tmp_path, archives, "migrate-build-state", [])
    # Building still works against the external store.
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(rust_wrkviz)])):
        outcome = _run(command, [
            "build", "--output", str(archives[label]), "--team", "widget",
        ])
        assert outcome.returncode == 0, outcome.stderr
    assert _tree(archives["rs"].parent) == _tree(archives["py"].parent)


def test_migrate_build_state_empty_archive(
    tmp_path: Path, rust_wrkviz: Path
) -> None:
    archives = {"py": tmp_path / "py-empty", "rs": tmp_path / "rs-empty"}
    for archive in archives.values():
        archive.mkdir(parents=True)
    _both(tmp_path, archives, "migrate-build-state", [])


def test_migrate_build_state_help(rust_wrkviz: Path) -> None:
    oracle = _run([sys.executable, str(PYTHON_WRKVIZ)],
                  ["migrate-build-state", "--help"])
    result = _run([str(rust_wrkviz)], ["migrate-build-state", "--help"])
    assert oracle.returncode == 0
    assert result.returncode == 0
    assert result.stdout == oracle.stdout
    assert result.stderr == oracle.stderr
