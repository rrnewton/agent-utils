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


def test_real_agentctl_writer_records_reanchor() -> None:
    """The real #744 writer appends the agreed entry on anchor --replace.

    Runs agentctl's own Rust test, which drives ManagedAgents::anchor with
    --replace against its fake Herdr client and asserts the agent.json
    `reanchors` entry (reason restart-same-pane, current pid, null
    slot_generation) that the reader above consumes.
    """
    import subprocess

    repo = Path(__file__).resolve().parents[3]
    # Bound the in-test Cargo build (#883): with default job parallelism
    # its test-binary link peaked at 3.7 GiB inside a shard whose cap is
    # about 1.9 GiB, OOM-killing whichever shard this test lands in.
    # Two jobs and line-tables-only debug info keep the writer's real
    # test while shrinking that build's peak.
    env = dict(os.environ)
    env["CARGO_BUILD_JOBS"] = "2"
    env.update(
        {
            "CARGO_PROFILE_DEV_DEBUG": "line-tables-only",
            "CARGO_PROFILE_TEST_DEBUG": "line-tables-only",
            "CARGO_INCREMENTAL": "0",
        }
    )
    result = subprocess.run(
        [
            "cargo", "test", "--quiet", "--locked",
            "--manifest-path", str(repo / "rs" / "Cargo.toml"),
            "-p", "agentctl", "--lib",
            "anchor_refuses_a_changed_harness_without_replace",
        ],
        cwd=repo, capture_output=True, text=True, check=False, timeout=900, env=env,
    )
    assert result.returncode == 0, result.stdout[-2000:] + result.stderr[-2000:]


def test_writer_shaped_entry_is_alive(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    """The exact entry record_reanchor emits (null generation) reads alive."""
    record = _record(tmp_path, monkeypatch, generation=None)
    document = json.loads(
        (tmp_path / "agents" / "agent-a" / "agent.json").read_text()
    )
    entry = document["reanchors"][0]
    entry["reason"] = "restart-same-pane"
    (tmp_path / "agents" / "agent-a" / "agent.json").write_text(json.dumps(document))
    answer = wrkslots_cli._reanchor_liveness(record)  # type: ignore[arg-type]
    assert answer is not None
    assert answer.state == "alive"
    assert "re-registered" in answer.detail
