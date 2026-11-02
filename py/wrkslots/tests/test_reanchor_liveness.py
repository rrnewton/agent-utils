"""Re-registered pane classification from agentctl reanchors (#777)."""

from __future__ import annotations

import json
import os
from pathlib import Path

import pytest
from wrkslots import cli as wrkslots_cli


def _record(tmp_path: Path, monkeypatch: pytest.MonkeyPatch, *, generation: object) -> object:
    agents = tmp_path / "agents"
    record_dir = agents / "agent-a"
    record_dir.mkdir(parents=True)
    (record_dir / "agent.json").write_text(json.dumps({
        "reanchors": [{
            "previous": {"pane_id": "p1", "terminal_id": "t1", "pid": 999999},
            "current": {"pane_id": "p2", "terminal_id": "t2", "pid": os.getpid()},
            "slot": "slot-a",
            "slot_generation": generation,
            "at": 1800000000,
            "reason": "reuse",
        }]
    }))
    monkeypatch.setenv("WRKSLOTS_AGENTCTL_AGENTS_DIR", str(agents))
    identity = wrkslots_cli._read_process_identity(os.getpid())
    return wrkslots_cli.ActiveRecord(
        slot="slot-a", agent="agent-a", task="t", purpose="p",
        slot_type="agent", machine="node-a", generation=1,
        created_at="2026-10-26T00:00:00Z", heartbeat_at="2026-10-26T01:00:00Z",
        heartbeat_ttl_seconds=60, owner=identity, coordinator_lease=identity,
        coordinator_recovery_note=None, handoff=None, checkouts=(),
    )


def test_reanchor_matching_generation_is_alive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    record = _record(tmp_path, monkeypatch, generation=1)
    answer = wrkslots_cli._reanchor_liveness(record)  # type: ignore[arg-type]
    assert answer is not None
    assert answer.state == "alive"
    assert "re-registered" in answer.detail


def test_reanchor_null_generation_newer_than_creation(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    record = _record(tmp_path, monkeypatch, generation=None)
    answer = wrkslots_cli._reanchor_liveness(record)  # type: ignore[arg-type]
    assert answer is not None
    assert answer.state == "alive"


def test_reanchor_generation_mismatch_is_none(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    record = _record(tmp_path, monkeypatch, generation=2)
    assert wrkslots_cli._reanchor_liveness(record) is None  # type: ignore[arg-type]
