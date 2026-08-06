"""Focused contract tests for hindsight-based agent naming."""

from __future__ import annotations

from dataclasses import replace
import json
from pathlib import Path
import sys

import pytest

from agent_team_timeline.naming import (
    PROMPT_VERSION,
    AgentNameError,
    AgentNameJob,
    _PendingName,
    _input_hash,
    _parse_backend_output,
    build_agent_name_prompt,
    name_agents,
)
from agent_team_timeline.token_usage import TokenUsage


def _job(
    key: str = "budget-agent",
    *,
    thread_id: str = "thread-budget",
    official_path: str = (
        "/root/transcript_auditor/owner_turn_miner/plugin_layout_audit/"
        "budget_overlap_audit"
    ),
    depth: int = 4,
    parent_official_path: str | None = (
        "/root/transcript_auditor/owner_turn_miner/plugin_layout_audit"
    ),
    prior_context: str = (
        "The coordinator called this the CPU budget overlap audit before spawning the child."
    ),
    work_summary: str = (
        "The completed audit separated Hermit CPU-second budgets from Reverie wall-time "
        "throughput ratchets and verified the exact PR heads."
    ),
) -> AgentNameJob:
    return AgentNameJob(
        key=key,
        thread_id=thread_id,
        official_path=official_path,
        coordinator_nickname="Beauvoir" if depth > 0 else None,
        role="reviewer" if depth > 0 else "coordinator",
        depth=depth,
        parent_official_path=parent_official_path,
        prior_context=prior_context,
        work_summary=work_summary,
    )


def _pending(
    job: AgentNameJob,
    *,
    backend: str = "codex",
    model: str = "gpt-test",
) -> _PendingName:
    input_hash = _input_hash(job, backend, model)
    return _PendingName(job, input_hash, Path("unused") / f"{input_hash}.json")


def _entry(
    job: AgentNameJob,
    short_name: str = "CPU budget audit",
    *,
    rationale: str = "The completed work was an audit of CPU budget semantics.",
    lifetime_summary: str = (
        "The agent separated CPU-time limits from wall-clock throughput checks and "
        "verified that each release receipt referred to the exact tested revision."
    ),
) -> dict[str, object]:
    return {
        "key": job.key,
        "thread_id": job.thread_id,
        "short_name": short_name,
        "rationale": rationale,
        "lifetime_summary": lifetime_summary,
    }


def _parse(
    jobs: list[AgentNameJob], names: list[dict[str, object]]
) -> dict[str, object]:
    results = _parse_backend_output(
        json.dumps({"names": names}),
        [_pending(job) for job in jobs],
        "gpt-test",
        "2026-08-05T12:00:00Z",
    )
    return {key: value.short_name for key, value in results.items()}


def test_prompt_contains_hindsight_summary_ancestor_context_and_official_paths() -> None:
    job = _job()
    prompt = build_agent_name_prompt([job])

    assert job.work_summary in prompt
    assert job.prior_context in prompt
    assert job.official_path in prompt
    assert job.parent_official_path is not None
    assert job.parent_official_path in prompt
    assert '"depth": 4' in prompt
    assert '"coordinator_nickname": "Beauvoir"' in prompt
    assert "with hindsight" in prompt
    assert "cross one or more spawn edges" in prompt
    assert "one to three sentences" in prompt
    assert "substantive work and outcomes" in prompt
    assert PROMPT_VERSION.endswith("-v2")


def test_root_is_always_exact_coordinator(tmp_path: Path) -> None:
    root = _job(
        "root-name",
        thread_id="root-thread",
        official_path="/root",
        depth=0,
        parent_official_path=None,
        work_summary="Coordinated the complete release and validation program.",
    )
    results, stats = name_agents(
        [root], tmp_path / "cache", backend="heuristic", model="offline"
    )

    assert results[root.thread_id].short_name == "Coordinator"
    assert stats.misses == 1
    with pytest.raises(AgentNameError, match="must be named Coordinator"):
        _parse([root], [_entry(root, "Root Coordinator")])


def test_valid_model_output_is_strictly_decoded() -> None:
    job = _job()
    entry = _entry(job)
    results = _parse_backend_output(
        json.dumps({"names": [entry]}),
        [_pending(job)],
        "gpt-test",
        "2026-08-05T12:00:00Z",
    )
    assert results[job.key].short_name == "CPU budget audit"
    assert results[job.key].lifetime_summary == entry["lifetime_summary"]


@pytest.mark.parametrize(
    "lifetime_summary, message",
    [
        ("", "must not be empty"),
        ("x" * 801, "exceeds 800 characters"),
    ],
)
def test_model_lifetime_summary_validation(
    lifetime_summary: str, message: str
) -> None:
    job = _job()
    with pytest.raises(AgentNameError, match=message):
        _parse([job], [_entry(job, lifetime_summary=lifetime_summary)])


def test_model_lifetime_summary_whitespace_is_canonicalized_without_retry() -> None:
    job = _job()
    results = _parse_backend_output(
        json.dumps(
            {
                "names": [
                    _entry(
                        job,
                        lifetime_summary=(
                            "The agent repaired the parser.\n\n"
                            "  Focused tests   verified the fix."
                        ),
                    )
                ]
            }
        ),
        [_pending(job)],
        "gpt-test",
        "2026-08-05T12:00:00Z",
    )

    assert results[job.key].lifetime_summary == (
        "The agent repaired the parser. Focused tests verified the fix."
    )


@pytest.mark.parametrize(
    "short_name, message",
    [
        ("Audit", "2 to 5 words"),
        ("budget_overlap_audit", "not a path or slug"),
        ("Budget  overlap audit", "single spaces"),
        ("Budget overlap audit review findings now", "2 to 5 words"),
        ("A" * 24 + " " + "B" * 24, "exceeds 48 characters"),
    ],
)
def test_model_short_name_validation(short_name: str, message: str) -> None:
    job = _job()
    with pytest.raises(AgentNameError, match=message):
        _parse([job], [_entry(job, short_name)])


def test_cache_hit_is_idempotent_without_rewriting(tmp_path: Path) -> None:
    job = _job()
    cache = tmp_path / "cache"
    first, first_stats = name_agents(
        [job], cache, backend="heuristic", model="offline"
    )
    cache_file = next(cache.glob("*.json"))
    original_bytes = cache_file.read_bytes()
    original_mtime = cache_file.stat().st_mtime_ns

    second, second_stats = name_agents(
        [job], cache, backend="heuristic", model="offline"
    )

    assert second == first
    assert first_stats.hits == 0 and first_stats.misses == 1
    assert second_stats.hits == 1 and second_stats.misses == 0
    assert second_stats.batches == 0
    assert cache_file.read_bytes() == original_bytes
    assert cache_file.stat().st_mtime_ns == original_mtime


def test_changed_hindsight_work_summary_invalidates_only_that_name(
    tmp_path: Path,
) -> None:
    job = _job()
    other = _job(
        "other-agent",
        thread_id="thread-other",
        official_path="/root/release_receipt_audit",
        depth=1,
        parent_official_path="/root",
        work_summary="Verified the release receipt against the exact landed SHA.",
    )
    cache = tmp_path / "cache"
    first, _ = name_agents(
        [job, other], cache, backend="heuristic", model="offline"
    )
    changed = replace(
        job,
        work_summary=(
            "The agent ultimately implemented scheduler admission recovery rather than "
            "performing the originally named budget audit."
        ),
    )

    second, stats = name_agents(
        [changed, other], cache, backend="heuristic", model="offline"
    )

    assert second[job.thread_id].input_hash != first[job.thread_id].input_hash
    assert second[other.thread_id] == first[other.thread_id]
    assert stats.hits == 1 and stats.misses == 1
    assert len(list(cache.glob("*.json"))) == 3


def test_heuristic_uses_only_nested_official_leaf(tmp_path: Path) -> None:
    job = _job()
    results, _ = name_agents(
        [job], tmp_path / "cache", backend="heuristic", model="offline"
    )

    result = results[job.thread_id]
    assert result.short_name == "Budget overlap audit"
    assert "budget_overlap_audit" in result.rationale
    assert "transcript auditor" not in result.short_name.lower()
    assert "separated Hermit CPU-second budgets" in result.lifetime_summary


def test_pre_lifetime_cache_version_is_regenerated_once(tmp_path: Path) -> None:
    job = _job()
    cache = tmp_path / "cache"
    input_hash = _input_hash(job, "heuristic", "offline")
    pending = _PendingName(job, input_hash, cache / f"{input_hash}.json")
    pending.cache_path.parent.mkdir(parents=True)
    pending.cache_path.write_text(
        json.dumps(
            {
                "cache_version": 2,
                "backend": "heuristic",
                "usage_receipt_id": "legacy-receipt",
                "result": {
                    "thread_id": job.thread_id,
                    "short_name": "CPU budget audit",
                    "rationale": "Legacy name without a lifetime summary.",
                    "model": "offline",
                    "prompt_version": PROMPT_VERSION,
                    "input_hash": pending.input_hash,
                    "generated_at": "2026-08-05T12:00:00Z",
                },
            }
        ),
        encoding="utf-8",
    )

    first, first_stats = name_agents(
        [job], cache, backend="heuristic", model="offline"
    )
    second, second_stats = name_agents(
        [job], cache, backend="heuristic", model="offline"
    )

    assert first_stats.hits == 0 and first_stats.misses == 1
    assert second_stats.hits == 1 and second_stats.misses == 0
    assert first[job.thread_id].lifetime_summary == second[job.thread_id].lifetime_summary
    migrated = json.loads(pending.cache_path.read_text(encoding="utf-8"))
    assert migrated["cache_version"] == 3
    assert migrated["result"]["lifetime_summary"]


def test_codex_naming_records_tokens_model_and_reasoning_effort(tmp_path: Path) -> None:
    fake = tmp_path / "fake_naming_codex.py"
    fake.write_text(
        """#!/usr/bin/env python3
import json
import shutil
import subprocess
import sys
from pathlib import Path

args = sys.argv[1:]
assert args[0] == "exec"
for required in (
    "--ephemeral", "--skip-git-repo-check", "--sandbox", "read-only",
    "--model", "--json", "--output-schema", "--output-last-message", "-",
):
    assert required in args
assert 'model_reasoning_effort="high"' in args
assert Path.cwd().name.startswith("agent-team-timeline-name-")
repository_check = subprocess.run(
    ["git", "rev-parse", "--is-inside-work-tree"],
    text=True,
    capture_output=True,
    check=True,
)
assert repository_check.stdout.strip() == "true"
compatibility_command = shutil.which("hg") or shutil.which("sl")
if compatibility_command is not None:
    compatibility_check = subprocess.run(
        [compatibility_command, "root"],
        text=True,
        capture_output=True,
        check=True,
    )
    assert Path(compatibility_check.stdout.strip()) == Path.cwd()
prompt = sys.stdin.read()
payload = prompt.split("BEGIN_AGENT_NAME_JOBS_JSON\\n", 1)[1].split(
    "END_AGENT_NAME_JOBS_JSON", 1
)[0]
jobs = json.loads(payload)
names = [{
    "key": job["key"],
    "thread_id": job["thread_id"],
    "short_name": "CPU budget audit",
    "rationale": "The completed work audited CPU budget semantics.",
    "lifetime_summary": "The agent audited CPU budget semantics and verified exact receipts.",
} for job in jobs]
output = Path(args[args.index("--output-last-message") + 1])
output.write_text(json.dumps({"names": names}), encoding="utf-8")
print(json.dumps({
    "type": "turn.completed",
    "usage": {
        "input_tokens": 90,
        "cached_input_tokens": 30,
        "cache_write_input_tokens": 4,
        "output_tokens": 18,
        "reasoning_output_tokens": 6,
    },
}))
""",
        encoding="utf-8",
    )
    cache = tmp_path / "cache"
    jobs = [_job()]
    _, first = name_agents(
        jobs,
        cache,
        backend="codex",
        model="gpt-test",
        max_workers=1,
        codex_command=(sys.executable, str(fake)),
        reasoning_effort="high",
    )
    assert first.newly_spent_usage == TokenUsage(
        input_tokens=90,
        cached_input_tokens=30,
        cache_write_input_tokens=4,
        output_tokens=18,
        reasoning_output_tokens=6,
    )
    assert first.usage_run_path is not None
    run = json.loads(first.usage_run_path.read_text(encoding="utf-8"))
    assert run["model"] == "gpt-test"
    assert run["reasoning_effort"] == "high"

    _, second = name_agents(
        jobs,
        cache,
        backend="codex",
        model="gpt-test",
        max_workers=1,
        codex_command=(sys.executable, str(fake)),
        reasoning_effort="high",
    )
    assert second.newly_spent_usage == TokenUsage()
    assert second.artifact_generation_usage == first.artifact_generation_usage


def test_failed_codex_naming_preserves_terminal_usage_receipt(tmp_path: Path) -> None:
    cache = tmp_path / "cache"
    diagnostic = "HTTP 404 Luna deployment unavailable"
    backend = (
        "import json,sys; "
        "print(json.dumps({'type':'turn.completed','usage':{"
        "'input_tokens':41,'cached_input_tokens':13,'output_tokens':8}})); "
        f"print(json.dumps({{'type':'error','message':{diagnostic!r}}})); "
        "sys.stderr.write('Codex CLI at Meta ' + ('banner ' * 100)); sys.exit(9)"
    )
    with pytest.raises(AgentNameError, match="deployment unavailable") as caught:
        name_agents(
            [_job()],
            cache,
            backend="codex",
            model="gpt-test",
            codex_command=(sys.executable, "-c", backend),
        )

    receipt_paths = list((cache / "_usage" / "receipts").glob("*.json"))
    assert len(receipt_paths) == 1
    receipt_path = receipt_paths[0]
    assert f"failed usage receipt: {receipt_path}" in str(caught.value)
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    assert receipt["status"] == "failed"
    assert receipt["usage"]["input_tokens"] == 41
    assert receipt["usage"]["output_tokens"] == 8
    assert receipt["usage"]["total_tokens"] == 49
    assert diagnostic in receipt["error"]


@pytest.mark.parametrize(
    "raw",
    [
        "not JSON",
        "[]",
        "{}",
        '{"names": {}}',
        '{"names": [], "unexpected": true}',
    ],
)
def test_malformed_model_output_is_rejected(raw: str) -> None:
    job = _job()
    with pytest.raises(AgentNameError):
        _parse_backend_output(
            raw,
            [_pending(job)],
            "gpt-test",
            "2026-08-05T12:00:00Z",
        )


def test_duplicate_model_output_is_rejected() -> None:
    job = _job()
    with pytest.raises(AgentNameError, match="duplicate key"):
        _parse([job], [_entry(job), _entry(job, "Budget semantics review")])


def test_missing_model_output_is_rejected() -> None:
    first = _job()
    second = _job(
        "receipt-agent",
        thread_id="thread-receipt",
        official_path="/root/release_receipt_audit",
        depth=1,
        parent_official_path="/root",
    )
    with pytest.raises(AgentNameError, match="missing names for receipt-agent"):
        _parse([first, second], [_entry(first)])


def test_extra_fields_and_wrong_thread_are_rejected() -> None:
    job = _job()
    extra = _entry(job)
    extra["invented"] = True
    with pytest.raises(AgentNameError, match="extra invented"):
        _parse([job], [extra])

    wrong_thread = _entry(job)
    wrong_thread["thread_id"] = "some-other-thread"
    with pytest.raises(AgentNameError, match="does not match job"):
        _parse([job], [wrong_thread])
