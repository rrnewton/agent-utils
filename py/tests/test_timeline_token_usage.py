"""Tests for strict model-token accounting and immutable receipts."""

from __future__ import annotations

import json
from pathlib import Path

import pytest

from agent_team_timeline.token_usage import (
    BatchUsageReceipt,
    TokenUsage,
    load_batch_receipt,
    parse_codex_jsonl_usage,
    write_batch_receipt,
)


def test_parse_codex_jsonl_usage_ignores_banner_and_uses_completed_turn() -> None:
    output = "\n".join(
        (
            "Codex CLI banner",
            json.dumps({"type": "turn.started"}),
            json.dumps(
                {
                    "type": "turn.completed",
                    "usage": {
                        "input_tokens": 10_631,
                        "cached_input_tokens": 7_424,
                        "cache_write_input_tokens": 0,
                        "output_tokens": 5,
                        "reasoning_output_tokens": 0,
                    },
                }
            ),
        )
    )
    usage = parse_codex_jsonl_usage(output)
    assert usage == TokenUsage(
        input_tokens=10_631,
        cached_input_tokens=7_424,
        output_tokens=5,
    )
    assert usage.total_tokens == 10_636


def test_parse_installed_codex_usage_with_only_guaranteed_fields() -> None:
    output = json.dumps(
        {
            "type": "turn.completed",
            "usage": {
                "input_tokens": 10_631,
                "cached_input_tokens": 7_424,
                "output_tokens": 5,
            },
        }
    )
    assert parse_codex_jsonl_usage(output) == TokenUsage(
        input_tokens=10_631,
        cached_input_tokens=7_424,
        output_tokens=5,
    )


def test_absent_completed_turn_is_explicitly_unknown() -> None:
    assert parse_codex_jsonl_usage('{"type":"turn.started"}\n') is None


@pytest.mark.parametrize(
    "missing",
    ("input_tokens", "cached_input_tokens", "output_tokens"),
)
def test_installed_codex_required_usage_fields_remain_strict(missing: str) -> None:
    usage = {
        "input_tokens": 10,
        "cached_input_tokens": 4,
        "output_tokens": 2,
    }
    del usage[missing]
    output = json.dumps({"type": "turn.completed", "usage": usage})
    with pytest.raises(ValueError, match=missing):
        parse_codex_jsonl_usage(output)


def test_invalid_completed_usage_is_rejected() -> None:
    output = json.dumps(
        {
            "type": "turn.completed",
            "usage": {
                "input_tokens": 3,
                "cached_input_tokens": 4,
                "cache_write_input_tokens": 0,
                "output_tokens": 1,
                "reasoning_output_tokens": 0,
            },
        }
    )
    with pytest.raises(ValueError, match="cached input"):
        parse_codex_jsonl_usage(output)


def test_batch_receipt_round_trips_and_detects_tampering(tmp_path: Path) -> None:
    receipt = BatchUsageReceipt.create(
        backend="codex",
        model="gpt-5.6-sol",
        reasoning_effort="xhigh",
        status="completed",
        started_at="2026-08-05T20:00:00+00:00",
        completed_at="2026-08-05T20:00:01+00:00",
        input_hashes=("a", "b"),
        usage=TokenUsage(input_tokens=20, cached_input_tokens=5, output_tokens=3),
        error=None,
    )
    root = tmp_path / "usage"
    path = write_batch_receipt(root, receipt)
    assert load_batch_receipt(root, receipt.receipt_id) == receipt
    raw = json.loads(path.read_text(encoding="utf-8"))
    raw["usage"]["output_tokens"] = 4
    path.write_text(json.dumps(raw), encoding="utf-8")
    assert load_batch_receipt(root, receipt.receipt_id) is None


def test_unknown_usage_is_not_serialized_as_zero(tmp_path: Path) -> None:
    receipt = BatchUsageReceipt.create(
        backend="codex",
        model="legacy-test",
        reasoning_effort=None,
        status="completed",
        started_at="2026-08-05T20:00:00+00:00",
        completed_at="2026-08-05T20:00:01+00:00",
        input_hashes=("a",),
        usage=None,
        error=None,
    )
    path = write_batch_receipt(tmp_path, receipt)
    raw = json.loads(path.read_text(encoding="utf-8"))
    assert raw["usage"] is None
