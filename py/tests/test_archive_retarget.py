"""Focused tests for the Python archive set-aside mirror (#2028 v2)."""
import json
import time
from pathlib import Path

from agentctl.subagents import _set_aside_archived_queue


def _queue(root: Path) -> Path:
    for lane in ("inbox", "inbox.v2", "inflight", "processed", "failed", "cancelled"):
        (root / lane).mkdir(parents=True, exist_ok=True)
    return root


def _put(queue: Path, lane: str, mid: str, text: str, not_after=None, **extra) -> None:
    doc = {"id": mid, "text": text, "queued_at": time.time(), "delivery_attempts": 0, **extra}
    if not_after is not None:
        doc["not_after"] = not_after
    (queue / lane / f"{mid}.json").write_text(json.dumps(doc))


def test_pending_set_aside_raised_never_processed(tmp_path: Path) -> None:
    queue = _queue(tmp_path / "queue")
    _put(queue, "inbox.v2", "keep-1", "hello world", not_after=4_102_444_800_000)
    items = _set_aside_archived_queue(queue)
    assert [item["id"] for item in items] == ["keep-1"]
    aside = json.loads((queue / "cancelled/keep-1.json").read_text())
    assert aside["cancel_reason"] == "stranded-archived"
    assert aside["delivery_state"] == "cancelled"
    assert not (queue / "processed/keep-1.json").exists()
    assert not (queue / "inbox.v2/keep-1.json").exists()


def test_expired_is_stranded_expired(tmp_path: Path) -> None:
    queue = _queue(tmp_path / "queue")
    _put(queue, "inbox", "old-1", "stale", not_after=1)
    items = _set_aside_archived_queue(queue)
    assert items[0]["outcome"] == "stranded-expired"
    aside = json.loads((queue / "cancelled/old-1.json").read_text())
    assert aside["cancel_reason"] == "stranded-expired"


def test_inflight_maybe_typed_is_possibly_submitted(tmp_path: Path) -> None:
    queue = _queue(tmp_path / "queue")
    _put(queue, "inflight", "half-1", "half typed", paste_started_at=1.0)
    items = _set_aside_archived_queue(queue)
    assert items[0]["outcome"] == "possibly_submitted"
    failed = json.loads((queue / "failed/half-1.json").read_text())
    assert failed["possibly_submitted"] is True
    assert not (queue / "processed/half-1.json").exists()
