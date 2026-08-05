"""Tests for the content-addressed timeline summarization layer."""

from __future__ import annotations

import json
import sys
from collections.abc import Mapping
from dataclasses import replace
from pathlib import Path

import pytest

from agent_team_timeline.summarize import (
    PROMPT_VERSION,
    SummaryError,
    SummaryJob,
    _PendingJob,
    _input_hash,
    _parse_backend_output,
    build_summary_prompt,
    summarize_jobs,
)
from agent_team_timeline.token_usage import TokenUsage


def _job(
    key: str,
    transcript: str = "Implemented transcript indexing. All 12 parser tests passed.",
    stats: Mapping[str, int] | None = None,
) -> SummaryJob:
    return SummaryJob(
        key=key,
        team_slug="codex-hermit",
        agent_label="coordinator" if key == "root" else key,
        start_ms=1_800_000_000_000,
        end_ms=1_800_000_060_000,
        prior_context="The user named this workstream transcript-DAG and then spawned the agent.",
        transcript=transcript,
        glossary="transcript-DAG: the durable multi-agent history and summary archive",
        stats=dict(stats or {"messages": 4, "tool_calls": 2}),
    )


def test_fingerprint_is_canonical_and_second_run_is_a_no_churn_hit(tmp_path: Path) -> None:
    cache = tmp_path / "cache"
    first_job = _job("root", stats={"messages": 4, "tool_calls": 2})
    reordered_stats = _job("root", stats={"tool_calls": 2, "messages": 4})

    first, first_stats = summarize_jobs(
        [first_job], cache, backend="heuristic", model="offline"
    )
    assert first_stats.hits == 0
    assert first_stats.misses == 1
    assert first_stats.batches == 1
    assert "Implemented transcript indexing" in first["root"].paragraph
    assert len(first["root"].phrase) <= 80

    cache_file = next(cache.glob("*.json"))
    original_bytes = cache_file.read_bytes()
    original_mtime = cache_file.stat().st_mtime_ns

    second, second_stats = summarize_jobs(
        [reordered_stats], cache, backend="heuristic", model="offline"
    )
    assert second == first
    assert second_stats.hits == 1
    assert second_stats.misses == 0
    assert second_stats.batches == 0
    assert second_stats.cache_hits == 1
    assert second_stats.backend_batches == 0
    assert cache_file.read_bytes() == original_bytes
    assert cache_file.stat().st_mtime_ns == original_mtime


def test_clean_runs_have_deterministic_fingerprints(tmp_path: Path) -> None:
    job = _job("agent-a")
    first, _ = summarize_jobs(
        [job], tmp_path / "one", backend="heuristic", model="offline"
    )
    second, _ = summarize_jobs(
        [job], tmp_path / "two", backend="heuristic", model="offline"
    )
    assert first[job.key].input_hash == second[job.key].input_hash
    assert first[job.key].phrase == second[job.key].phrase
    assert first[job.key].paragraph == second[job.key].paragraph
    assert first[job.key].work_summary == second[job.key].work_summary
    assert _input_hash(job, "codex", "same-model", "high") != _input_hash(
        job, "codex", "same-model", "xhigh"
    )


def test_changing_one_job_invalidates_only_that_job(tmp_path: Path) -> None:
    cache = tmp_path / "cache"
    jobs = [_job("agent-a"), _job("agent-b")]
    first, _ = summarize_jobs(jobs, cache, backend="heuristic", model="offline")

    changed = replace(
        jobs[1], transcript="Fixed the scheduler race. The 37 stress tests passed."
    )
    second, stats = summarize_jobs(
        [jobs[0], changed], cache, backend="heuristic", model="offline"
    )
    assert stats.hits == 1
    assert stats.misses == 1
    assert stats.batches == 1
    assert second["agent-a"] == first["agent-a"]
    assert second["agent-b"].input_hash != first["agent-b"].input_hash
    assert "scheduler race" in second["agent-b"].paragraph
    assert len(list(cache.glob("*.json"))) == 3


def test_prompt_contains_full_context_glossary_and_terminology_rules() -> None:
    job = _job("agent-a")
    prompt = build_summary_prompt([job])
    assert job.prior_context in prompt
    assert job.transcript in prompt
    assert job.glossary in prompt
    assert '"messages": 4' in prompt
    assert "cross the subagent spawn edge" in prompt
    assert "phase 2" in prompt and "descriptive workstream" in prompt
    assert "Do not call tools" in prompt
    assert PROMPT_VERSION in prompt


def _write_fake_codex(path: Path) -> None:
    path.write_text(
        """#!/usr/bin/env python3
import json
import os
import sys
from pathlib import Path

args = sys.argv[1:]
assert args[0] == "exec"
for required in (
    "--ephemeral", "--skip-git-repo-check", "--sandbox", "read-only",
    "--model", "--json", "--output-schema", "--output-last-message", "-",
):
    assert required in args
assert Path.cwd().name.startswith("agent-team-timeline-summary-")
schema_path = Path(args[args.index("--output-schema") + 1])
schema = json.loads(schema_path.read_text(encoding="utf-8"))
assert schema["properties"]["summaries"]["type"] == "array"
prompt = sys.stdin.read()
payload_text = prompt.split("BEGIN_JOBS_JSON\\n", 1)[1].split("END_JOBS_JSON", 1)[0]
jobs = json.loads(payload_text)
summaries = []
for job in jobs:
    summaries.append({
        "key": job["key"],
        "phrase": "Built " + job["key"],
        "paragraph": "Implemented durable summary output for " + job["agent_label"] + ".",
        "work_summary": [{
            "at_ms": job["start_ms"],
            "text": "Implemented durable summary output.",
        }],
    })
output_path = Path(args[args.index("--output-last-message") + 1])
output_path.write_text(json.dumps({"summaries": summaries}), encoding="utf-8")
with Path(os.environ["FAKE_CODEX_LOG"]).open("a", encoding="utf-8") as handle:
    handle.write("CALL\\n")
    handle.write(prompt)
    handle.write("PROMPT_END\\n")
print(json.dumps({
    "type": "turn.completed",
    "usage": {
        "input_tokens": 100,
        "cached_input_tokens": 40,
        "cache_write_input_tokens": 5,
        "output_tokens": 20,
        "reasoning_output_tokens": 7,
    },
}))
""",
        encoding="utf-8",
    )


def test_codex_backend_batches_with_schema_stdin_and_temp_workdir(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    fake = tmp_path / "fake_codex.py"
    log = tmp_path / "calls.log"
    _write_fake_codex(fake)
    monkeypatch.setenv("FAKE_CODEX_LOG", str(log))
    jobs = [_job("agent-a"), _job("agent-b"), _job("agent-c")]

    results, stats = summarize_jobs(
        jobs,
        tmp_path / "cache",
        backend="codex",
        model="gpt-test",
        max_workers=1,
        batch_size=2,
        codex_command=(sys.executable, str(fake)),
    )
    assert stats.hits == 0
    assert stats.misses == 3
    assert stats.batches == 2
    assert stats.newly_spent_usage == TokenUsage(
        input_tokens=200,
        cached_input_tokens=80,
        cache_write_input_tokens=10,
        output_tokens=40,
        reasoning_output_tokens=14,
    )
    assert stats.newly_spent_usage.total_tokens == 240
    assert stats.newly_spent_unknown_receipts == 0
    assert stats.artifact_generation_usage == stats.newly_spent_usage
    assert stats.artifact_generation_unknown_receipts == 0
    assert stats.unknown_legacy_artifacts == 0
    assert stats.usage_run_path is not None
    run = json.loads(stats.usage_run_path.read_text(encoding="utf-8"))
    assert run["model"] == "gpt-test"
    assert run["newly_spent_usage"]["total_tokens"] == 240
    assert len(run["batch_receipt_ids"]) == 2
    assert list(results) == [job.key for job in jobs]
    assert results["agent-a"].phrase == "Built agent-a"
    assert results["agent-a"].work_summary[0].at_ms == jobs[0].start_ms
    log_text = log.read_text(encoding="utf-8")
    assert log_text.count("CALL\n") == 2
    assert jobs[0].prior_context in log_text
    assert jobs[0].glossary in log_text

    _, cached_stats = summarize_jobs(
        jobs,
        tmp_path / "cache",
        backend="codex",
        model="gpt-test",
        max_workers=1,
        batch_size=2,
        codex_command=(sys.executable, str(fake)),
    )
    assert cached_stats.hits == 3
    assert cached_stats.batches == 0
    assert cached_stats.newly_spent_usage == TokenUsage()
    assert cached_stats.newly_spent_unknown_receipts == 0
    assert cached_stats.artifact_generation_usage == stats.artifact_generation_usage
    assert cached_stats.artifact_generation_unknown_receipts == 0
    assert log.read_text(encoding="utf-8") == log_text


def test_corrupt_cache_is_ignored_and_regenerated(tmp_path: Path) -> None:
    cache = tmp_path / "cache"
    job = _job("agent-a")
    summarize_jobs([job], cache, backend="heuristic", model="offline")
    cache_file = next(cache.glob("*.json"))
    cache_file.write_text("{ definitely not JSON", encoding="utf-8")

    results, stats = summarize_jobs(
        [job], cache, backend="heuristic", model="offline"
    )
    assert stats.hits == 0
    assert stats.misses == 1
    assert "transcript indexing" in results[job.key].paragraph
    assert cache_file.read_text(encoding="utf-8").startswith("{")
    json.loads(cache_file.read_text(encoding="utf-8"))


def test_legacy_cache_is_a_hit_with_explicitly_unknown_historical_cost(
    tmp_path: Path,
) -> None:
    cache = tmp_path / "cache"
    job = _job("agent-a")
    original, _ = summarize_jobs([job], cache, backend="heuristic", model="offline")
    cache_file = next(cache.glob("*.json"))
    raw = json.loads(cache_file.read_text(encoding="utf-8"))
    raw["cache_version"] = 1
    del raw["usage_receipt_id"]
    cache_file.write_text(json.dumps(raw), encoding="utf-8")

    reused, stats = summarize_jobs(
        [job], cache, backend="heuristic", model="offline"
    )
    assert reused == original
    assert stats.hits == 1
    assert stats.misses == 0
    assert stats.newly_spent_usage == TokenUsage()
    assert stats.unknown_legacy_artifacts == 1


def test_backend_failure_is_concise_and_preserves_existing_cache(tmp_path: Path) -> None:
    cache = tmp_path / "cache"
    cache.mkdir()
    job = _job("agent-a")
    cache_file = cache / f"{_input_hash(job, 'codex', 'gpt-test')}.json"
    original = b"corrupt but pre-existing\n"
    cache_file.write_bytes(original)

    with pytest.raises(SummaryError, match="exit 7: boom") as caught:
        summarize_jobs(
            [job],
            cache,
            backend="codex",
            model="gpt-test",
            codex_command=(
                sys.executable,
                "-c",
                "import sys; sys.stderr.write('boom'); sys.exit(7)",
            ),
        )
    assert len(str(caught.value)) < 300
    assert cache_file.read_bytes() == original
    receipt_paths = list((cache / "_usage" / "receipts").glob("*.json"))
    assert len(receipt_paths) == 1
    receipt = json.loads(receipt_paths[0].read_text(encoding="utf-8"))
    assert receipt["status"] == "failed"
    assert receipt["model"] == "gpt-test"
    assert receipt["usage"] is None


def test_failed_invocation_keeps_independently_validated_batch(tmp_path: Path) -> None:
    cache = tmp_path / "cache"
    first = _job("agent-a")
    failing = _job("agent-b")
    backend = """
import json
import sys
from pathlib import Path

prompt = sys.stdin.read()
if '"key":"agent-b"' in prompt or '"key": "agent-b"' in prompt:
    sys.stderr.write("deliberate second-batch failure")
    raise SystemExit(7)
payload_text = prompt.split("BEGIN_JOBS_JSON\\n", 1)[1].split("END_JOBS_JSON", 1)[0]
job = json.loads(payload_text)[0]
result = {"summaries": [{
    "key": job["key"],
    "phrase": "Preserved successful batch",
    "paragraph": "Validated work remains reusable after another batch fails.",
    "work_summary": [],
}]}
args = sys.argv[1:]
Path(args[args.index("--output-last-message") + 1]).write_text(json.dumps(result))
"""
    with pytest.raises(SummaryError, match="deliberate second-batch failure"):
        summarize_jobs(
            [first, failing],
            cache,
            backend="codex",
            model="gpt-test",
            max_workers=1,
            batch_size=1,
            codex_command=(sys.executable, "-c", backend),
        )

    first_cache = cache / f"{_input_hash(first, 'codex', 'gpt-test')}.json"
    failing_cache = cache / f"{_input_hash(failing, 'codex', 'gpt-test')}.json"
    assert first_cache.is_file()
    assert not failing_cache.exists()


def _pending(job: SummaryJob, tmp_path: Path) -> _PendingJob:
    input_hash = _input_hash(job, "codex", "gpt-test")
    return _PendingJob(job, input_hash, tmp_path / f"{input_hash}.json")


@pytest.mark.parametrize(
    "payload",
    [
        {"summaries": [], "extra": True},
        {
            "summaries": [
                {"key": "agent-a", "phrase": "short", "work_summary": []}
            ]
        },
        {
            "summaries": [
                {
                    "key": "unexpected",
                    "phrase": "short",
                    "paragraph": "paragraph",
                    "work_summary": [],
                }
            ]
        },
        {
            "summaries": [
                {
                    "key": "agent-a",
                    "phrase": "x" * 81,
                    "paragraph": "paragraph",
                    "work_summary": [],
                }
            ]
        },
        {
            "summaries": [
                {
                    "key": "agent-a",
                    "phrase": "short",
                    "paragraph": "paragraph",
                    "work_summary": [{"at_ms": "not-an-int", "text": "work"}],
                }
            ]
        },
    ],
)
def test_backend_output_is_strictly_narrowed(
    payload: object, tmp_path: Path
) -> None:
    job = _job("agent-a")
    with pytest.raises(SummaryError):
        _parse_backend_output(
            json.dumps(payload), [_pending(job, tmp_path)], "gpt-test", "generated"
        )


def test_valid_backend_output_is_narrowed_to_frozen_types(tmp_path: Path) -> None:
    job = _job("agent-a")
    payload = {
        "summaries": [
            {
                "key": job.key,
                "phrase": "Built timeline cache",
                "paragraph": "Implemented and verified the timeline summary cache.",
                "work_summary": [
                    {"at_ms": job.start_ms, "text": "Implemented the cache."},
                    {"at_ms": job.end_ms, "text": "Verified cache hits."},
                ],
            }
        ]
    }
    result = _parse_backend_output(
        json.dumps(payload), [_pending(job, tmp_path)], "gpt-test", "generated"
    )[job.key]
    assert result.model == "gpt-test"
    assert result.prompt_version == PROMPT_VERSION
    assert isinstance(result.work_summary, tuple)
    assert result.work_summary[-1].at_ms == job.end_ms
