"""Claude and Muse CLI backend contracts, using local fakes and no model calls.

The Muse half covers #839 wrkviz-muse-backend: the default backend, its cache shape, its time
limit and its zero-spend plan.
"""

from __future__ import annotations

from dataclasses import replace
import json
from pathlib import Path
import time

import pytest

from wrkviz import cli as timeline_cli
from wrkviz.backend_process import TIMED_OUT_EXIT, BackendProcesses
from wrkviz.budget import (
    PHASE_KIND,
    TECHNICAL_ROLLUP_KIND,
    Prices,
    StageCount,
    SummaryPlan,
    calibrate,
)
from wrkviz.muse_backend import MuseBackendError, parse_muse_events, run_muse_json
from wrkviz.naming import AgentNameError, AgentNameJob, name_agents
from wrkviz.summarize import SummaryError, SummaryJob, build_summary_prompt, summarize_jobs
from wrkviz.token_usage import TokenUsage, parse_claude_json_usage


def _summary_job() -> SummaryJob:
    return SummaryJob(
        key="phase-one",
        team_slug="test-team",
        agent_label="transcript indexer",
        start_ms=1_800_000_000_000,
        end_ms=1_800_000_060_000,
        prior_context="The user asked for a durable transcript index.",
        transcript="Implemented the transcript index and passed 12 parser tests.",
        glossary="transcript index: chronological owner-intent records",
        stats={"messages": 2, "tool_calls": 1},
    )


def _name_job() -> AgentNameJob:
    return AgentNameJob(
        key="name-one",
        team_slug="test-team",
        thread_id="thread-one",
        start_ms=1_800_000_000_000,
        end_ms=1_800_000_060_000,
        official_path="/root/transcript_index_audit",
        coordinator_nickname="Noether",
        role="reviewer",
        depth=1,
        parent_official_path="/root",
        prior_context="The user called this the transcript index audit.",
        work_summary="The agent audited the transcript index and its parser tests.",
    )


def _write_fake_claude(path: Path) -> None:
    path.write_text(
        """#!/usr/bin/env python3
import json
import os
import sys
from pathlib import Path

args = sys.argv[1:]
assert args[0] == "--print"
assert args[args.index("--output-format") + 1] == "json"
schema = json.loads(args[args.index("--json-schema") + 1])
assert "$schema" not in schema
assert args[args.index("--model") + 1] == "claude-test"
assert args[args.index("--effort") + 1] == "medium"
assert "--safe-mode" in args
assert "--no-session-persistence" in args
assert args[args.index("--permission-mode") + 1] == "dontAsk"
assert args[args.index("--tools") + 1] == ""
prompt = sys.stdin.read()
if "BEGIN_AGENT_NAME_JOBS_JSON\\n" in prompt:
    payload = prompt.split("BEGIN_AGENT_NAME_JOBS_JSON\\n", 1)[1].split(
        "END_AGENT_NAME_JOBS_JSON", 1
    )[0]
    jobs = json.loads(payload)
    assert schema["properties"]["names"]["type"] == "array"
    structured = {"names": [{
        "key": job["key"],
        "thread_id": job["thread_id"],
        "short_name": "Transcript index audit",
        "rationale": "The completed work audited the transcript index.",
        "lifetime_summary": "The agent audited the transcript index and verified its parser tests.",
    } for job in jobs]}
else:
    payload = prompt.split("BEGIN_JOBS_JSON\\n", 1)[1].split(
        "END_JOBS_JSON", 1
    )[0]
    jobs = json.loads(payload)
    assert schema["properties"]["summaries"]["type"] == "array"
    structured = {"summaries": [{
        "key": job["key"],
        "phrase": "Built transcript index",
        "paragraph": "Implemented a durable transcript index and passed its parser tests.",
        "work_summary": [{
            "at_ms": job["start_ms"],
            "text": "Implemented the transcript index and passed 12 parser tests.",
        }],
    } for job in jobs]}
result = {
    "type": "result",
    "subtype": "success",
    "is_error": False,
    "result": json.dumps(structured),
    "usage": {
        "input_tokens": 11,
        "cache_creation_input_tokens": 5,
        "cache_read_input_tokens": 7,
        "output_tokens": 13,
    },
}
mode = os.environ.get("FAKE_CLAUDE_MODE", "success")
if mode == "path-name":
    structured["names"][-1]["short_name"] = jobs[-1]["official_path"]
if mode in {"success", "path-name"}:
    result["structured_output"] = structured
elif mode == "failure":
    result["subtype"] = "error"
    result["is_error"] = True
    result["result"] = "provider deployment unavailable"
with Path(os.environ["FAKE_CLAUDE_LOG"]).open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({"args": args, "cwd": str(Path.cwd())}) + "\\n")
print(json.dumps(result))
if mode == "failure":
    raise SystemExit(9)
""",
        encoding="utf-8",
    )
    path.chmod(0o755)


def test_claude_usage_normalizes_native_cache_counters() -> None:
    usage = parse_claude_json_usage(
        json.dumps(
            {
                "usage": {
                    "input_tokens": 11,
                    "cache_creation_input_tokens": 5,
                    "cache_read_input_tokens": 7,
                    "output_tokens": 13,
                }
            }
        )
    )
    assert usage == TokenUsage(
        input_tokens=23,
        cached_input_tokens=7,
        cache_write_input_tokens=5,
        output_tokens=13,
    )
    assert usage.total_tokens == 36


def test_claude_summary_uses_safe_structured_mode_and_exact_receipts(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = tmp_path / "fake-claude"
    log = tmp_path / "claude-calls.jsonl"
    _write_fake_claude(fake)
    monkeypatch.setenv("FAKE_CLAUDE_LOG", str(log))
    cache = tmp_path / "cache"

    results, stats = summarize_jobs(
        [_summary_job()],
        cache,
        backend="claude",
        model="claude-test",
        reasoning_effort="medium",
        max_workers=1,
        claude_command=(str(fake),),
    )

    assert results["phase-one"].phrase == "Built transcript index"
    assert stats.newly_spent_usage == TokenUsage(
        input_tokens=23,
        cached_input_tokens=7,
        cache_write_input_tokens=5,
        output_tokens=13,
    )
    assert stats.newly_spent_unknown_receipts == 0
    assert stats.usage_run_path is not None
    run = json.loads(stats.usage_run_path.read_text(encoding="utf-8"))
    assert run["backend"] == "claude"
    assert run["service_tier"] is None
    assert run["newly_spent_usage"]["total_tokens"] == 36
    call = json.loads(log.read_text(encoding="utf-8"))
    assert Path(call["cwd"]).name.startswith("wrkviz-summary-")
    args = call["args"]
    assert args[:3] == ["--print", "--output-format", "json"]
    serialized_schema = json.loads(args[args.index("--json-schema") + 1])
    assert "$schema" not in serialized_schema
    assert serialized_schema["properties"]["summaries"]["type"] == "array"
    assert args[args.index("--model") + 1] == "claude-test"
    assert args[args.index("--effort") + 1] == "medium"

    _, cached = summarize_jobs(
        [_summary_job()],
        cache,
        backend="claude",
        model="claude-test",
        reasoning_effort="medium",
        max_workers=1,
        claude_command=(str(fake),),
    )
    assert cached.hits == 1
    assert cached.newly_spent_usage == TokenUsage()
    assert len(log.read_text(encoding="utf-8").splitlines()) == 1


def test_claude_does_not_fall_back_to_result_string_and_keeps_usage(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = tmp_path / "fake-claude"
    log = tmp_path / "claude-calls.jsonl"
    _write_fake_claude(fake)
    monkeypatch.setenv("FAKE_CLAUDE_LOG", str(log))
    monkeypatch.setenv("FAKE_CLAUDE_MODE", "result-only")
    cache = tmp_path / "cache"

    with pytest.raises(SummaryError, match="malformed structured output"):
        summarize_jobs(
            [_summary_job()],
            cache,
            backend="claude",
            model="claude-test",
            reasoning_effort="medium",
            max_workers=1,
            claude_command=(str(fake),),
        )

    receipts = list((cache / "_usage" / "receipts").glob("*.json"))
    assert len(receipts) == 1
    receipt = json.loads(receipts[0].read_text(encoding="utf-8"))
    assert receipt["status"] == "failed"
    assert receipt["usage"]["total_tokens"] == 36
    assert not list(cache.glob("*.json"))
    assert not list((cache / "_usage" / "backend_outputs").glob("*.json"))


def test_claude_provider_failure_and_naming_are_accounted(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = tmp_path / "fake-claude"
    log = tmp_path / "claude-calls.jsonl"
    _write_fake_claude(fake)
    monkeypatch.setenv("FAKE_CLAUDE_LOG", str(log))
    cache = tmp_path / "failed-cache"
    monkeypatch.setenv("FAKE_CLAUDE_MODE", "failure")
    with pytest.raises(SummaryError, match="deployment unavailable"):
        summarize_jobs(
            [_summary_job()],
            cache,
            backend="claude",
            model="claude-test",
            reasoning_effort="medium",
            max_workers=1,
            claude_command=(str(fake),),
        )
    failed_receipt = json.loads(
        next((cache / "_usage" / "receipts").glob("*.json")).read_text(
            encoding="utf-8"
        )
    )
    assert failed_receipt["usage"]["total_tokens"] == 36

    monkeypatch.setenv("FAKE_CLAUDE_MODE", "success")
    named, stats = name_agents(
        [_name_job()],
        tmp_path / "name-cache",
        backend="claude",
        model="claude-test",
        reasoning_effort="medium",
        max_workers=1,
        claude_command=(str(fake),),
    )
    assert named["thread-one"].short_name == "Transcript index audit"
    assert named["thread-one"].lifetime_summary is not None
    assert stats.newly_spent_usage.total_tokens == 36


def test_claude_path_name_repair_keeps_the_whole_batch_and_audit(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = tmp_path / "fake-claude"
    log = tmp_path / "claude-calls.jsonl"
    _write_fake_claude(fake)
    monkeypatch.setenv("FAKE_CLAUDE_LOG", str(log))
    monkeypatch.setenv("FAKE_CLAUDE_MODE", "path-name")
    cache = tmp_path / "name-cache"
    first = _name_job()
    second = replace(
        first,
        key="name-two",
        thread_id="thread-two",
        official_path="/root/release_receipt_audit",
        coordinator_nickname="Curie",
    )

    named, stats = name_agents(
        [first, second],
        cache,
        backend="claude",
        model="claude-test",
        reasoning_effort="medium",
        max_workers=1,
        batch_size=12,
        claude_command=(str(fake),),
    )

    assert named[first.thread_id].short_name == "Transcript index audit"
    assert named[second.thread_id].short_name == "Release receipt audit"
    assert stats.misses == 2 and stats.batches == 1
    assert stats.newly_spent_usage.total_tokens == 36
    assert len(list(cache.glob("*.json"))) == 2
    receipt = json.loads(
        next((cache / "_usage" / "receipts").glob("*.json")).read_text(
            encoding="utf-8"
        )
    )
    assert receipt["status"] == "completed"
    audit = json.loads(
        (
            cache
            / "_usage"
            / "backend_outputs"
            / f"{receipt['receipt_id']}.json"
        ).read_text(encoding="utf-8")
    )
    assert audit["status"] == "repaired"
    assert audit["repairs"] == [
        {
            "key": second.key,
            "index": 1,
            "field": "short_name",
            "action": "normalized-path-or-slug",
            "rejected": second.official_path,
            "replacement": "Release receipt audit",
        }
    ]
    assert "BEGIN_AGENT_NAME_JOBS_JSON" not in audit["raw_output"]

    reused, cached = name_agents(
        [first, second],
        cache,
        backend="claude",
        model="claude-test",
        reasoning_effort="medium",
        max_workers=1,
        batch_size=12,
        claude_command=(str(fake),),
    )
    assert reused == named
    assert cached.hits == 2 and cached.misses == 0 and cached.batches == 0
    assert cached.newly_spent_usage == TokenUsage()
    assert len(log.read_text(encoding="utf-8").splitlines()) == 1


def test_claude_rejects_codex_only_service_tier(tmp_path: Path) -> None:
    with pytest.raises(SummaryError, match="only supported by the codex backend"):
        summarize_jobs(
            [_summary_job()],
            tmp_path / "cache",
            backend="claude",
            model="claude-test",
            service_tier="priority",
        )

    with pytest.raises(AgentNameError, match="only supported by the codex backend"):
        name_agents(
            [_name_job()],
            tmp_path / "name-cache",
            backend="claude",
            model="claude-test",
            service_tier="priority",
        )


def test_cli_exposes_and_forwards_claude_command(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    namespace = timeline_cli._parser().parse_args(
        [
            "summarize",
            "--output",
            str(tmp_path),
            "--team",
            "test-team",
            "--backend",
            "claude",
            "--model",
            "sonnet",
            "--reasoning-effort",
            "medium",
            "--claude-command",
            "/opt/test/claude-wrapper",
        ]
    )
    captured_args: list[tuple[object, ...]] = []
    captured_kwargs: list[dict[str, object]] = []

    class ExpectedCall(RuntimeError):
        pass

    def fake_summarize_archive(*args: object, **kwargs: object) -> None:
        captured_args.append(args)
        captured_kwargs.append(kwargs)
        raise ExpectedCall

    monkeypatch.setattr(timeline_cli, "summarize_archive", fake_summarize_archive)
    with pytest.raises(ExpectedCall):
        timeline_cli._summary_call(namespace)
    assert captured_args[0][2:4] == ("claude", "sonnet")
    assert captured_kwargs[0]["claude_command"] == ("/opt/test/claude-wrapper",)
    assert captured_kwargs[0]["reasoning_effort"] == "medium"


def test_cli_requires_explicit_claude_model(tmp_path: Path) -> None:
    namespace = timeline_cli._parser().parse_args(
        [
            "summarize",
            "--output",
            str(tmp_path),
            "--team",
            "test-team",
            "--backend",
            "claude",
        ]
    )

    with pytest.raises(ValueError, match="--model is required"):
        timeline_cli._summary_call(namespace)

def _event(kind: str, **payload: object) -> str:
    return json.dumps({"seq": 1, "payload": {"kind": kind, **payload}})


def _write_fake_muse(path: Path) -> None:
    path.write_text(
        """#!/usr/bin/env python3
import json
import os
import sys
import time
from pathlib import Path

args = sys.argv[1:]
assert args[:2] == ["exec", "--json"], args
prompt = Path(args[args.index("--prompt-file") + 1]).read_text(encoding="utf-8")
schema = json.loads(Path(args[args.index("--output-schema") + 1]).read_text(encoding="utf-8"))
assert "$schema" not in schema
for flag in ("--disable-shell", "--disable-write", "--disable-web-tools", "--no-session-log"):
    assert flag in args, flag
assert args[args.index("--preset") + 1] == "empty"
with Path(os.environ["FAKE_MUSE_LOG"]).open("a", encoding="utf-8") as handle:
    handle.write(json.dumps({"args": args, "cwd": str(Path.cwd())}) + "\\n")
mode = os.environ.get("FAKE_MUSE_MODE", "success")
if mode == "hang":
    time.sleep(60)
if "BEGIN_AGENT_NAME_JOBS_JSON\\n" in prompt:
    jobs = json.loads(prompt.split("BEGIN_AGENT_NAME_JOBS_JSON\\n", 1)[1].split(
        "END_AGENT_NAME_JOBS_JSON", 1)[0])
    answers = schema["properties"]["names"]
    assert answers["minItems"] == answers["maxItems"] == len(jobs)
    assert answers["items"]["properties"]["key"]["enum"] == [job["key"] for job in jobs]
    structured = {"names": [{
        "key": job["key"],
        "thread_id": job["thread_id"],
        "short_name": "Transcript index audit",
        "rationale": "The completed work audited the transcript index.",
        "lifetime_summary": "The agent audited the transcript index and verified its parser tests.",
    } for job in jobs]}
else:
    jobs = json.loads(prompt.split("BEGIN_JOBS_JSON\\n", 1)[1].split("END_JOBS_JSON", 1)[0])
    answers = schema["properties"]["summaries"]
    assert answers["minItems"] == answers["maxItems"] == len(jobs)
    assert answers["items"]["properties"]["key"]["enum"] == [job["key"] for job in jobs]
    structured = {"summaries": [{
        "key": job["key"],
        "phrase": "Built transcript index",
        "paragraph": "Implemented a durable transcript index and passed its parser tests.",
        "work_summary": [{
            "at_ms": job["start_ms"],
            "text": "Implemented the transcript index and passed 12 parser tests.",
        }],
    } for job in jobs]}
print("muse banner, not JSON")
print(json.dumps({"payload": {"kind": "assistant_delta", "text": "..."}}))
usage = {"kind": "run_usage", "models": [{
    "model": args[args.index("--model") + 1],
    "input_tokens": 30, "cached_tokens": 10, "output_tokens": 9, "reasoning_tokens": 4,
}]}
terminal = {"kind": "run_terminal", "terminal": "completed", "reason": "done",
            "text": json.dumps(structured)}
if mode == "failed-run":
    terminal = {"kind": "run_terminal", "terminal": "failed", "reason": "model unavailable"}
print(json.dumps({"payload": usage}))
print(json.dumps({"payload": terminal}))
""",
        encoding="utf-8",
    )
    path.chmod(0o755)


@pytest.fixture
def fake_muse(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> Path:
    fake = tmp_path / "fake-muse"
    _write_fake_muse(fake)
    monkeypatch.setenv("FAKE_MUSE_LOG", str(tmp_path / "muse-calls.jsonl"))
    return fake


def test_muse_events_report_terminal_text_and_summed_usage() -> None:
    stdout = "\n".join(
        [
            "not json",
            _event("assistant_delta", text="partial"),
            _event(
                "run_usage",
                models=[
                    {"input_tokens": 100, "cached_tokens": 40, "output_tokens": 7,
                     "reasoning_tokens": 5},
                    {"input_tokens": 20, "cached_tokens": 0, "output_tokens": 3},
                ],
            ),
            _event("run_terminal", terminal="completed", reason="done", text='{"a": 1}'),
        ]
    )
    events = parse_muse_events(stdout)
    assert events.terminal == "completed"
    assert events.text == '{"a": 1}'
    assert events.usage == TokenUsage(
        input_tokens=120, cached_input_tokens=40, output_tokens=10, reasoning_output_tokens=5
    )


def test_muse_events_reject_malformed_usage() -> None:
    with pytest.raises(ValueError, match="cached_tokens"):
        parse_muse_events(_event("run_usage", models=[{"cached_tokens": -1}]))


def test_muse_summary_fills_the_same_cache_shape_as_claude(
    tmp_path: Path, fake_muse: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    muse_cache = tmp_path / "muse-cache"
    results, stats = summarize_jobs(
        [_summary_job()],
        muse_cache,
        backend="muse",
        model="muse-spark-test",
        reasoning_effort="medium",
        max_workers=1,
        muse_command=(str(fake_muse),),
    )
    assert results["phase-one"].phrase == "Built transcript index"
    assert stats.newly_spent_usage == TokenUsage(
        input_tokens=30, cached_input_tokens=10, output_tokens=9, reasoning_output_tokens=4
    )
    call = json.loads((tmp_path / "muse-calls.jsonl").read_text(encoding="utf-8"))
    assert Path(call["cwd"]).name.startswith("wrkviz-summary-")
    assert call["args"][call["args"].index("--model") + 1] == "muse-spark-test"
    assert call["args"][call["args"].index("--reasoning-effort") + 1] == "medium"

    fake_claude = tmp_path / "fake-claude"
    _write_fake_claude(fake_claude)
    monkeypatch.setenv("FAKE_CLAUDE_LOG", str(tmp_path / "claude-calls.jsonl"))
    claude_cache = tmp_path / "claude-cache"
    summarize_jobs(
        [_summary_job()],
        claude_cache,
        backend="claude",
        model="claude-test",
        reasoning_effort="medium",
        max_workers=1,
        claude_command=(str(fake_claude),),
    )

    def entries(cache: Path) -> list[dict[str, object]]:
        return [
            json.loads(path.read_text(encoding="utf-8"))
            for path in sorted(cache.glob("*.json"))
        ]

    muse_entries, claude_entries = entries(muse_cache), entries(claude_cache)
    assert len(muse_entries) == len(claude_entries) == 1
    muse_entry, claude_entry = muse_entries[0], claude_entries[0]
    assert sorted(muse_entry) == sorted(claude_entry)
    muse_result = muse_entry["result"]
    claude_result = claude_entry["result"]
    assert isinstance(muse_result, dict) and isinstance(claude_result, dict)
    assert sorted(muse_result) == sorted(claude_result)
    assert muse_result["phrase"] == claude_result["phrase"]
    assert muse_result["work_summary"] == claude_result["work_summary"]
    muse_artifact, claude_artifact = muse_entry["artifact"], claude_entry["artifact"]
    assert isinstance(muse_artifact, dict) and isinstance(claude_artifact, dict)
    assert sorted(muse_artifact) == sorted(claude_artifact)
    assert (muse_artifact["backend"], muse_artifact["model"]) == ("muse", "muse-spark-test")

    _, cached = summarize_jobs(
        [_summary_job()],
        muse_cache,
        backend="muse",
        model="muse-spark-test",
        reasoning_effort="medium",
        max_workers=1,
        muse_command=(str(fake_muse),),
    )
    assert cached.cache_hits == 1
    assert cached.backend_batches == 0


def test_muse_agent_names_use_the_muse_runner(tmp_path: Path, fake_muse: Path) -> None:
    named, stats = name_agents(
        [_name_job()],
        tmp_path / "names",
        backend="muse",
        model="muse-spark-test",
        reasoning_effort="medium",
        max_workers=1,
        muse_command=(str(fake_muse),),
    )
    assert named["thread-one"].short_name == "Transcript index audit"
    assert stats.newly_spent_usage.input_tokens == 30


def test_muse_failed_run_aborts_without_cache_entries(
    tmp_path: Path, fake_muse: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setenv("FAKE_MUSE_MODE", "failed-run")
    cache = tmp_path / "cache"
    with pytest.raises(SummaryError, match="model unavailable"):
        summarize_jobs(
            [_summary_job()],
            cache,
            backend="muse",
            model="muse-spark-test",
            max_workers=1,
            muse_command=(str(fake_muse),),
        )
    assert not list(cache.glob("*.json"))


def test_backend_timeout_stops_a_stalled_call(tmp_path: Path, fake_muse: Path,
                                              monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("FAKE_MUSE_MODE", "hang")
    work = tmp_path / "work"
    work.mkdir()
    started = time.monotonic()
    with pytest.raises(MuseBackendError, match="stopped at its time limit"):
        run_muse_json(
            (str(fake_muse),),
            prompt="BEGIN_JOBS_JSON\n[]\nEND_JOBS_JSON",
            schema={"type": "object"},
            model="m",
            reasoning_effort=None,
            cwd=work,
            processes=BackendProcesses(timeout_seconds=1.0),
        )
    assert time.monotonic() - started < 30


def test_backend_processes_report_timeout_exit(tmp_path: Path) -> None:
    completed = BackendProcesses(timeout_seconds=0.5).run(
        ["sleep", "30"], input_text="", cwd=tmp_path
    )
    assert completed.returncode == TIMED_OUT_EXIT
    assert "--backend-timeout" in completed.stderr
    with pytest.raises(ValueError):
        BackendProcesses(timeout_seconds=0)


def test_phase_prompt_states_each_job_time_window() -> None:
    job = _summary_job()
    prompt = build_summary_prompt([job])
    assert "Time windows:" in prompt
    assert (
        f"- {job.key}: 2027-01-15 08:00:00 UTC to 2027-01-15 08:01:00 UTC "
        f"(start_ms {job.start_ms}, end_ms {job.end_ms})"
    ) in prompt


def _plan(backend: str, model: str, rollup_model: str, cache: Path) -> SummaryPlan:
    stages = [
        StageCount(name="phase summaries", kind=PHASE_KIND, total=13, to_generate=13),
        StageCount(name="daily rollups (technical)", kind=TECHNICAL_ROLLUP_KIND, total=2,
                   to_generate=2),
    ]
    return SummaryPlan(
        team_slug="t",
        stages=stages,
        costs=calibrate(cache, cache / "names", backend, model, "medium", rollup_model),
        prices=Prices.parse("input=1.25,output=4.25"),
        budget_usd=0.0,
        batch_size=6,
        workers=3,
    )


def test_muse_plan_reports_zero_spend_and_wall_time(tmp_path: Path) -> None:
    plan = _plan("muse", "muse-spark-1.3-internal", "watermelon-full-preview", tmp_path)
    assert plan.estimated_usd == 0.0
    assert not plan.over_budget
    # 13 phases -> 3 batches on 3 workers: one 50 s round; 2 rollups at 190 s each, serially.
    assert plan.estimated_seconds == 50.0 + 380.0
    total = plan.lines()[-1]
    assert "$0.00 model spend (zero-rated backend)" in total
    assert "expected wall time ~8 min" in total
    assert plan.to_json()["estimated_seconds"] == 430.0

    paid = _plan("codex", "gpt-x", "gpt-x", tmp_path)
    assert paid.estimated_usd is not None and paid.estimated_usd > 0
    assert paid.over_budget
    assert "expected wall time" not in paid.lines()[-1]
    assert "estimated_seconds" not in paid.to_json()


def test_only_keyed_schemas_fix_the_answer_count() -> None:
    from wrkviz import naming, summarize

    for module, field in ((summarize, "summaries"), (naming, "names")):
        open_schema = module._output_schema()["properties"]
        assert isinstance(open_schema, dict)
        assert "minItems" not in open_schema[field]
        keyed = module._output_schema(["a", "b"])["properties"]
        assert isinstance(keyed, dict)
        answers = keyed[field]
        assert isinstance(answers, dict)
        assert (answers["minItems"], answers["maxItems"]) == (2, 2)
        item = answers["items"]
        assert isinstance(item, dict) and isinstance(item["properties"], dict)
        assert item["properties"]["key"] == {"type": "string", "enum": ["a", "b"]}
