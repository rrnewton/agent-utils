"""Differential: Rust `wrkviz github-metadata` vs the oracle.

Twin archives are ingested and built by each edition with a fake `gh`
on the resolver seam (AGENT_UTILS_GH). Pull metadata is fetched,
conditionally re-fetched (ETag/304), and failed over, with stdout,
stderr, exit code, the pull cache, and both trees compared.
Fixtures only; no network.
"""

from __future__ import annotations

import json
import os
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
    "run_id", "started_at", "completed_at", "created_at", "last_run_at",
    "last_run_id", "updated_at", "generated_at", "pid", "fetched_at",
}
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
                         "text": "Implemented per https://github.com/example/widget/pull/7 "
                                 "and example/widget#9; the broken example/widget#13 fell over"}]}),
        rec(1050, "event_msg", {
            "type": "task_complete", "turn_id": "root-turn",
            "started_at": 1010, "completed_at": 1050,
            "time_to_first_token_ms": 12, "last_agent_message": "Root done"}),
    ]
    path = root / "rollout-2026-01-01T00-00-00-root-thread.jsonl"
    path.write_bytes(b"".join(
        json.dumps(item, sort_keys=True).encode() + b"\n" for item in records))
    return root


FAKE_GH = r'''#!/bin/sh
# Canned `gh api --include` responses keyed by the endpoint argument.
last=""
inm=""
for arg in "$@"; do
  case "$arg" in
    If-None-Match:*) inm="$arg" ;;
    *) last="$arg" ;;
  esac
done
case "$last" in
  repos/example/widget/pulls/7)
    if [ -n "$inm" ]; then
      printf 'HTTP/2 304 \r\netag: "v7"\r\n\r\n'
    else
      printf 'HTTP/2 200 \r\netag: "v7"\r\ncontent-type: application/json\r\n\r\n{"title":"Fix the widget parser","state":"closed","draft":false,"merged_at":"2026-01-02T00:00:00Z","body":"Body text","base":{"ref":"main"},"head":{"label":"example:fix-widget"},"user":{"login":"octocat"},"updated_at":"2026-01-02T00:00:00Z"}'
    fi
    ;;
  repos/example/widget/pulls/9)
    printf 'HTTP/2 200 \r\ncontent-type: application/json\r\n\r\n{"title":"Parser tests","state":"open","draft":true,"merged_at":null,"body":null,"base":{"ref":"main"},"head":{"label":"example:parser-tests"},"user":null,"updated_at":"2026-01-03T00:00:00Z"}'
    ;;
  *)
    printf 'HTTP/2 404 \r\ncontent-type: application/json\r\n\r\n{"message":"Not Found"}'
    ;;
esac
'''


def _fake_gh(base: Path) -> Path:
    script = base / "fake-gh"
    script.write_text(FAKE_GH)
    script.chmod(0o755)
    return script


def _run(command: list[str], args: list[str],
         env: dict[str, str] | None = None) -> subprocess.CompletedProcess[str]:
    full_env = dict(os.environ)
    if env:
        full_env.update(env)
    return subprocess.run(
        command + args, cwd=AU_ROOT, capture_output=True, text=True,
        check=False, timeout=300, env=full_env,
    )


def _normalize(text: str, archive: Path) -> str:
    text = text.replace(str(archive), "<ARCHIVE>")
    text = text.replace(str(archive.parent), "<BASE>")
    text = re.sub(r"runs/[^\s]+\.json", "runs/<RUN>.json", text)
    return re.sub(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z", "<TS>", text)


def _scrub(value: object) -> object:
    if isinstance(value, dict):
        return {
            key: ("<GEN>" if key in GEN_KEYS else
                  "<TS>" if key in TS_KEYS else _scrub(item))
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [_scrub(item) for item in value]
    if isinstance(value, str):
        return re.sub(
            r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z", "<TS>", value)
    return value


def _decoded(path: Path, raw: bytes) -> object:
    import gzip
    if path.suffix == ".gz":
        raw = gzip.decompress(raw)
    try:
        return _scrub(json.loads(raw))
    except (ValueError, UnicodeDecodeError):
        return re.sub(r"[0-9a-f]{64}", "<HEX>", raw.decode("utf-8", "replace"))


def _tree(root: Path) -> dict[str, object]:
    out: dict[str, object] = {}
    for path in sorted(root.rglob("*")):
        if path.is_dir() or path.name == ".wrkviz.lock":
            continue
        rel = f"{root.name}/{path.relative_to(root)}"
        out[rel] = _decoded(path, path.read_bytes())
    return out


def _built(tmp_path: Path, rust_wrkviz: Path) -> dict[str, Path]:
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
        outcome = _run(command, [
            "build", "--output", str(archive), "--team", "widget",
        ])
        assert outcome.returncode == 0, outcome.stderr
        # Built details are gzip-only on disk; discovery reads the plain
        # spelling, so seed the plain generated detail both editions read.
        detail = archive / "data" / "details" / "phase-seed.json"
        detail.write_text(json.dumps({
            "work_summary": [],
            "transcript": [{
                "text": "Implemented per https://github.com/example/widget/pull/7 "
                        "and example/widget#9; the broken example/widget#13 fell over",
            }],
        }, indent=1) + "\n")
        archives[label] = archive
    return archives


def _both(archives: dict[str, Path], gh: Path, expect: int = 0,
          ) -> dict[str, subprocess.CompletedProcess[str]]:
    env = {"AGENT_UTILS_GH": str(gh)}
    outcomes = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(RUST_WRKVIZ)])):
        outcomes[label] = _run(command, [
            "github-metadata", "--output", str(archives[label]),
            "--team", "widget",
        ], env=env)
    oracle, result = outcomes["py"], outcomes["rs"]
    assert oracle.returncode == expect, oracle.stderr
    assert result.returncode == oracle.returncode
    assert _normalize(result.stdout, archives["rs"]) == _normalize(
        oracle.stdout, archives["py"])
    assert _normalize(result.stderr, archives["rs"]) == _normalize(
        oracle.stderr, archives["py"])
    assert _tree(archives["rs"].parent) == _tree(archives["py"].parent)
    return outcomes


def test_github_metadata_fetch_and_conditional_refetch(
        tmp_path: Path, rust_wrkviz: Path) -> None:
    archives = _built(tmp_path, rust_wrkviz)
    gh = _fake_gh(tmp_path)
    # Pull 13 404s in both editions: the run fails after retaining records.
    outcomes = _both(archives, gh, expect=2)
    assert "2 fetched, 0 unchanged, 1 failed" in outcomes["py"].stdout
    assert "GitHub metadata refresh had failures" in outcomes["py"].stderr
    cache = json.loads((
        archives["py"] / "teams/widget/summary_data/github/pulls.json"
    ).read_text())
    assert sorted(cache["records"]) == [
        "example/widget#7", "example/widget#9"]
    # Second run: 7 is a 304, 9 refetches (no ETag), 13 still fails.
    outcomes = _both(archives, gh, expect=2)
    assert "1 fetched, 1 unchanged, 1 failed" in outcomes["py"].stdout


def test_github_metadata_help(rust_wrkviz: Path) -> None:
    oracle = _run([sys.executable, str(PYTHON_WRKVIZ)],
                  ["github-metadata", "--help"])
    result = _run([str(rust_wrkviz)], ["github-metadata", "--help"])
    assert result.returncode == oracle.returncode == 0
    assert result.stdout == oracle.stdout
    assert result.stderr == oracle.stderr
