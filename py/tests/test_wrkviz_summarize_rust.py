"""Differential: Rust `wrkviz summarize` against the Python oracle.

Both editions ingest the same tiny Codex rollout into twin archives,
then summarize under the deterministic heuristic backend, under
`--plan`, under a budget refusal, and under fake Codex/Claude wrapper
commands (the wrappers answer from the prompt's jobs JSON exactly the
way the Python test-suite fakes do). stdout, stderr, exit code, and
the archive trees must agree with timestamps, receipt ids, run ids,
and process ids normalized.

Fixtures only; no live sources or real model calls.
"""

from __future__ import annotations

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


FAKE_CODEX = """#!/usr/bin/env python3
import json, sys
args = sys.argv[1:]
prompt = sys.stdin.read()
if "--output-last-message" in args:
    out_path = args[args.index("--output-last-message") + 1]
else:
    out_path = None
if "BEGIN_AGENT_NAME_JOBS_JSON" in prompt:
    body = prompt.split("BEGIN_AGENT_NAME_JOBS_JSON\\n", 1)[1].split(
        "END_AGENT_NAME_JOBS_JSON", 1)[0]
    jobs = json.loads(body)
    result = {"names": [{
        "key": job["key"], "thread_id": job["thread_id"],
        "short_name": "Coordinator" if job["official_path"] == "/root"
        else "Widget Worker",
        "rationale": "Fake wrapper name.",
        "lifetime_summary": "Built the widget parser and verified it.",
    } for job in jobs]}
else:
    body = prompt.split("BEGIN_JOBS_JSON\\n", 1)[1].split(
        "END_JOBS_JSON", 1)[0]
    jobs = json.loads(body)
    summaries = []
    for job in jobs:
        if job.get("summary_style") == "project-overview":
            summaries.append({
                "key": job["key"], "phrase": "Project overview supported",
                "paragraph": "The widget project builds a parser for "
                             "widget files.",
                "work_summary": []})
        else:
            summaries.append({
                "key": job["key"], "phrase": "Built the widget parser",
                "paragraph": "The widget parser was implemented and "
                             "verified with tests.",
                "work_summary": [{"at_ms": job["start_ms"],
                                  "text": "Implemented the widget parser"}]})
    result = {"summaries": summaries}
if out_path is not None:
    with open(out_path, "w") as handle:
        json.dump(result, handle)
print(json.dumps({"type": "turn.completed", "usage": {
    "input_tokens": 100, "cached_input_tokens": 10,
    "output_tokens": 40, "reasoning_output_tokens": 5}}))
"""

FAKE_CLAUDE = """#!/usr/bin/env python3
import json, sys
prompt = sys.stdin.read()
if "BEGIN_AGENT_NAME_JOBS_JSON" in prompt:
    body = prompt.split("BEGIN_AGENT_NAME_JOBS_JSON\\n", 1)[1].split(
        "END_AGENT_NAME_JOBS_JSON", 1)[0]
    jobs = json.loads(body)
    structured = {"names": [{
        "key": job["key"], "thread_id": job["thread_id"],
        "short_name": "Coordinator" if job["official_path"] == "/root"
        else "Widget Worker",
        "rationale": "Fake wrapper name.",
        "lifetime_summary": "Built the widget parser and verified it.",
    } for job in jobs]}
else:
    body = prompt.split("BEGIN_JOBS_JSON\\n", 1)[1].split(
        "END_JOBS_JSON", 1)[0]
    jobs = json.loads(body)
    summaries = []
    for job in jobs:
        if job.get("summary_style") == "project-overview":
            summaries.append({
                "key": job["key"], "phrase": "Project overview supported",
                "paragraph": "The widget project builds a parser for "
                             "widget files.",
                "work_summary": []})
        else:
            summaries.append({
                "key": job["key"], "phrase": "Built the widget parser",
                "paragraph": "The widget parser was implemented and "
                             "verified with tests.",
                "work_summary": [{"at_ms": job["start_ms"],
                                  "text": "Implemented the widget parser"}]})
    structured = {"summaries": summaries}
print(json.dumps({
    "type": "result", "subtype": "success", "is_error": False,
    "structured_output": structured,
    "usage": {"input_tokens": 90, "cache_creation_input_tokens": 5,
              "cache_read_input_tokens": 10, "output_tokens": 30}}))
"""

FAILING_CODEX = """#!/usr/bin/env python3
import sys
print("provider unavailable", file=sys.stderr)
sys.exit(7)
"""


def _write_executable(path: Path, text: str) -> Path:
    path.write_text(text)
    path.chmod(0o755)
    return path


def _run(command: list[str], args: list[str]) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        command + args, cwd=AU_ROOT, capture_output=True, text=True,
        check=False, timeout=300,
    )


def _normalize(text: str, archive: Path) -> str:
    text = text.replace(str(archive), "ARCHIVE")
    text = re.sub(r"runs/[0-9A-Za-z-]+\.json", "runs/RUN.json", text)
    text = re.sub(r"receipts/[0-9a-f]{64}\.json", "receipts/HASH.json", text)
    text = re.sub(r"_usage/runs/[0-9a-f]{64}\.json", "_usage/runs/HASH.json",
                  text)
    return text


def _scrub(value: object) -> object:
    if isinstance(value, dict):
        out: dict[str, object] = {}
        for key, item in value.items():
            if key in TS_KEYS:
                out[key] = "<ts>"
            elif key in ID_KEYS:
                out[key] = "<id>"
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


def _tree(archive: Path) -> dict[str, object]:
    result: dict[str, object] = {}
    for path in sorted(archive.rglob("*")):
        if not path.is_file():
            continue
        rel = path.relative_to(archive).as_posix()
        if rel.startswith("runs/"):
            rel = "runs/RUN.json"
        raw = path.read_bytes()
        if path.suffix == ".json":
            try:
                value = json.loads(raw.decode("utf-8"))
            except (ValueError, UnicodeDecodeError):
                result[rel] = raw.decode("utf-8", "replace")
                continue
            scrubbed = _scrub(value)
            if re.fullmatch(r"[0-9a-f]{64}\.json", path.name):
                digest = hashlib.sha256(json.dumps(
                    scrubbed, sort_keys=True).encode()).hexdigest()[:16]
                directory = rel.rsplit("/", 1)[0]
                rel = f"{directory}/HASH-{digest}.json"
            result[rel] = scrubbed
        else:
            result[rel] = raw.decode("utf-8", "replace")
    # The two runs/ records collapse into one key; keep counts comparable
    # by retaining the record count alongside.
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


def _compare(
    tmp_path: Path,
    rust_wrkviz: Path,
    args: list[str],
    *,
    expect: int = 0,
) -> dict[str, subprocess.CompletedProcess[str]]:
    archives = _ingest(tmp_path, rust_wrkviz)
    outcomes = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(rust_wrkviz)])):
        outcomes[label] = _run(command, [
            "summarize", "--output", str(archives[label]),
            "--team", "widget", *args,
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


def test_summarize_heuristic_and_second_run(
    tmp_path: Path, rust_wrkviz: Path
) -> None:
    archives = _ingest(tmp_path, rust_wrkviz)
    for round_ in (1, 2):
        outcomes = {}
        for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                               ("rs", [str(rust_wrkviz)])):
            outcomes[label] = _run(command, [
                "summarize", "--output", str(archives[label]),
                "--team", "widget", "--backend", "heuristic",
                "--model", "offline",
            ])
        oracle, result = outcomes["py"], outcomes["rs"]
        assert oracle.returncode == 0, oracle.stderr
        assert result.returncode == 0
        assert _normalize(result.stdout, archives["rs"]) == _normalize(
            oracle.stdout, archives["py"])
        assert _normalize(result.stderr, archives["rs"]) == _normalize(
            oracle.stderr, archives["py"])
        assert _tree(archives["rs"]) == _tree(archives["py"])
        if round_ == 2:
            assert "cache 11 hit / 0 miss" in result.stdout


def test_summarize_plan(tmp_path: Path, rust_wrkviz: Path) -> None:
    _compare(tmp_path, rust_wrkviz, [
        "--backend", "heuristic", "--model", "offline", "--plan",
    ])


def test_summarize_budget_refusal(tmp_path: Path, rust_wrkviz: Path) -> None:
    _compare(tmp_path, rust_wrkviz, [
        "--backend", "codex", "--model", "fake",
        "--price", "input=2,output=10", "--budget-usd", "0.01",
    ], expect=3)


def test_summarize_codex_fake(tmp_path: Path, rust_wrkviz: Path) -> None:
    fake = _write_executable(tmp_path / "fake-codex", FAKE_CODEX)
    _compare(tmp_path, rust_wrkviz, [
        "--backend", "codex", "--model", "fake-model",
        "--codex-command", str(fake),
    ])


def test_summarize_claude_fake(tmp_path: Path, rust_wrkviz: Path) -> None:
    fake = _write_executable(tmp_path / "fake-claude", FAKE_CLAUDE)
    _compare(tmp_path, rust_wrkviz, [
        "--backend", "claude", "--model", "fake-claude",
        "--claude-command", str(fake),
    ])


def test_summarize_failing_backend(tmp_path: Path, rust_wrkviz: Path) -> None:
    fake = _write_executable(tmp_path / "failing-codex", FAILING_CODEX)
    _compare(tmp_path, rust_wrkviz, [
        "--backend", "codex", "--model", "fake-model",
        "--codex-command", str(fake),
    ], expect=2)


def test_summarize_rollup_selection(tmp_path: Path, rust_wrkviz: Path) -> None:
    _compare(tmp_path, rust_wrkviz, [
        "--backend", "heuristic", "--model", "offline",
        "--rollup-kind", "daily",
    ])


def test_summarize_missing_team(tmp_path: Path, rust_wrkviz: Path) -> None:
    archives = _ingest(tmp_path, rust_wrkviz)
    outcomes = {}
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(rust_wrkviz)])):
        outcomes[label] = _run(command, [
            "summarize", "--output", str(archives[label]),
            "--team", "ghost", "--backend", "heuristic",
            "--model", "offline",
        ])
    oracle, result = outcomes["py"], outcomes["rs"]
    assert oracle.returncode == 2
    assert result.returncode == 2
    assert _normalize(result.stderr, archives["rs"]) == _normalize(
        oracle.stderr, archives["py"])


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


def test_summarize_multi_agent(tmp_path: Path, rust_wrkviz: Path) -> None:
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
    for label, command in (("py", [sys.executable, str(PYTHON_WRKVIZ)]),
                           ("rs", [str(rust_wrkviz)])):
        outcome = _run(command, [
            "summarize", "--output", str(archives[label]),
            "--team", "widget", "--backend", "heuristic",
            "--model", "offline",
        ])
        assert outcome.returncode == 0, outcome.stderr
        if label == "rs":
            assert "agent names" in outcome.stdout
    assert _tree(archives["rs"]) == _tree(archives["py"])


STATEFUL_CODEX = """#!/usr/bin/env python3
import json, os, sys
args = sys.argv[1:]
out_path = args[args.index("--output-last-message") + 1]
prompt = sys.stdin.read()
state_path = os.path.join(
    os.path.dirname(os.path.abspath(__file__)),
    f"state-{os.getppid()}.json")
try:
    state = json.load(open(state_path))
except Exception:
    state = {"singles": {}}
if "BEGIN_AGENT_NAME_JOBS_JSON" in prompt:
    body = prompt.split("BEGIN_AGENT_NAME_JOBS_JSON" + chr(10), 1)[1].split(
        "END_AGENT_NAME_JOBS_JSON", 1)[0]
    jobs = json.loads(body)
    result = {"names": [{
        "key": job["key"], "thread_id": job["thread_id"],
        "short_name": "Coordinator" if job["official_path"] == "/root"
        else "Widget Worker",
        "rationale": "Fake wrapper name.",
        "lifetime_summary": "Built the widget parser and verified it.",
    } for job in jobs]}
else:
    body = prompt.split("BEGIN_JOBS_JSON" + chr(10), 1)[1].split(
        "END_JOBS_JSON", 1)[0]
    jobs = json.loads(body)
    if len(jobs) > 1:
        jobs = jobs[1:]
    elif jobs:
        key = jobs[0]["key"]
        state["singles"][key] = state["singles"].get(key, 0) + 1
        if state["singles"][key] < 2:
            jobs = []
    summaries = []
    for job in jobs:
        if job.get("summary_style") == "project-overview":
            summaries.append({
                "key": job["key"], "phrase": "Project overview supported",
                "paragraph": "The widget project builds a parser for "
                             "widget files.",
                "work_summary": []})
        else:
            summaries.append({
                "key": job["key"], "phrase": "Built the widget parser",
                "paragraph": "The widget parser was implemented and "
                             "verified with tests.",
                "work_summary": [{"at_ms": job["start_ms"],
                                  "text": "Implemented the widget parser"}]})
    result = {"summaries": summaries}
with open(state_path, "w") as handle:
    json.dump(state, handle)
with open(out_path, "w") as handle:
    json.dump(result, handle)
print(json.dumps({"type": "turn.completed", "usage": {
    "input_tokens": 100, "cached_input_tokens": 10,
    "output_tokens": 40, "reasoning_output_tokens": 5}}))
"""


def test_summarize_retryable_split_is_fatal(
    tmp_path: Path, rust_wrkviz: Path
) -> None:
    fake = _write_executable(tmp_path / "stateful-codex", STATEFUL_CODEX)
    _compare(tmp_path, rust_wrkviz, [
        "--backend", "codex", "--model", "fake-model",
        "--codex-command", str(fake), "--summary-batch-size", "2",
    ], expect=2)
