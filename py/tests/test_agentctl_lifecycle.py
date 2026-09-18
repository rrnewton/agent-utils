"""Regression cases for queue transitions and generation-owned terminal lifecycle."""
from __future__ import annotations

import fcntl
import json
import os
import threading
from dataclasses import replace
from pathlib import Path
from typing import cast

import pytest

from agentctl import agent
from agentctl.client import AgentPaneInfo, HerdrClient, Pane
from agentctl.errors import AgentDeliveryError, HerdrUnavailable
from agentctl.subagents import ManagedAgents
from .test_herdr_subagents import FakeManagedClient, setup


def test_start_holds_its_generation_lock_through_brief_and_returned_status(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    original_info = fake.pane_info

    def info(pane: str) -> AgentPaneInfo:
        descriptor = os.open(tmp_path / "registry/.worker.lock", os.O_RDONLY)
        try:
            with pytest.raises(BlockingIOError):
                fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        finally:
            os.close(descriptor)
        return original_info(pane)

    monkeypatch.setattr(fake, "pane_info", info)
    result = manager.start("worker", cwd=str(tmp_path), brief="this generation's task")
    assert result["lifecycle"] == "running"
    assert fake.submitted == ["this generation's task"]


def test_allocation_captures_pane_before_later_launch_failure(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    fake.workspace = "w1"
    fake.fail_start = True
    with pytest.raises(AgentDeliveryError, match="failed"):
        manager.start("worker", cwd=str(tmp_path))
    record = manager.get("worker")
    assert record.tab_id == "w1:t1" and record.pane_id == "w1:p1"
    assert manager.stop("worker")["pane_closed"] is True


@pytest.mark.parametrize("claimed", [False, True])
def test_old_partial_allocation_is_recovered_only_for_the_original_unclaimed_shell(claimed: bool, tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    fake.fail_start = True
    with pytest.raises(AgentDeliveryError):
        manager.start("worker", cwd=str(tmp_path))
    manager._save(replace(manager.get("worker"), pane_id=None))
    if claimed:
        fake.infos["w1:p1"] = replace(fake.infos["w1:p1"], agent="claude")
        with pytest.raises(AgentDeliveryError, match="ownership changed"):
            manager.stop("worker")
        assert fake.closed == []
    else:
        result = manager.stop("worker")
        assert result["pane_closed"] is True
        archived = json.loads((Path(str(result["archive"])) / "agent.json").read_text())
        assert archived["pane_id"] == "w1:p1"


def test_stop_preserves_a_sibling_pane_added_during_output_capture(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    original_read = fake.read
    inserted = False

    def read(pane_id: str, *, source: str, lines: int) -> str:
        nonlocal inserted
        if not inserted:
            inserted = True
            fake.presentations.append(Pane("w1:human", "w1:t1", "w1"))
            fake.infos["w1:human"] = replace(fake.infos[pane_id], pane_id="w1:human", agent=None,
                session_agent=None, session_value=None)
        return original_read(pane_id, source=source, lines=lines)

    monkeypatch.setattr(fake, "read", read)
    result = manager.stop("worker")
    assert result["pane_closed"] is True and result["tab_closed"] is False
    assert fake.presentations == [Pane("w1:human", "w1:t1", "w1")]


def test_postclose_probe_failure_does_not_prevent_archival(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    original_close = fake.close_pane

    def close(pane: str) -> None:
        original_close(pane)
        fake.offline = True

    monkeypatch.setattr(fake, "close_pane", close)
    result = manager.stop("worker")
    assert result["pane_closed"] is True and result["tab_closed"] is None
    assert Path(str(result["archive"])).is_dir()


def test_explicit_send_serializes_id_reservation_against_delivery_transitions(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    fake = FakeManagedClient()
    manager = ManagedAgents(cast(HerdrClient, fake), tmp_path / "registry")
    monkeypatch.delenv("HERDR_WORKSPACE_ID", raising=False)
    manager.start("worker", cwd=str(tmp_path))
    queue = str(tmp_path / "queue")
    target = manager.get("worker").target()
    agent._bind_queue(queue, target)
    lock = agent._open_private_lock(str(Path(queue) / ".delivery.lock"), "test transaction")
    entered, wrote = threading.Event(), threading.Event()
    original_enqueue, original_create = agent._enqueue, agent._atomic_json_create

    def enqueue(*args: object, **kwargs: object) -> str:
        entered.set()
        return original_enqueue(*args, **kwargs)  # type: ignore[arg-type]

    def create(path: str, document: dict[str, object]) -> None:
        original_create(path, document)
        if path.endswith("inbox/reused-id.json"):
            wrote.set()

    monkeypatch.setattr(agent, "_enqueue", enqueue)
    monkeypatch.setattr(agent, "_atomic_json_create", create)
    errors: list[BaseException] = []

    def sender() -> None:
        try:
            agent.send(cast(HerdrClient, fake), target, queue, "new task", message_id="reused-id")
        except BaseException as exc:
            errors.append(exc)

    fcntl.flock(lock, fcntl.LOCK_EX)
    thread = threading.Thread(target=sender)
    thread.start()
    try:
        assert entered.wait(2)
        assert not wrote.wait(0.1), "explicit ID escaped the queue transition lock"
        agent._atomic_json(str(Path(queue) / "processed/reused-id.json"),
            {"id": "reused-id", "text": "previously submitted", "queued_at": 0, "delivery_attempts": 1})
    finally:
        os.close(lock)
        thread.join(2)
    assert not thread.is_alive()
    assert len(errors) == 1 and "already exists" in str(errors[0])
    assert fake.submitted == []


def test_unknown_metadata_survives_ownership_and_session_updates_without_a_wrapper(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, _ = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    path = tmp_path / "registry/worker/agent.json"
    record = json.loads(path.read_text())
    record["future_metadata"] = {"nested": [1, "preserve"]}
    agent._atomic_json(str(path), record)
    manager.pause("worker")
    manager.bind_session("worker", "session-1")
    saved = json.loads(path.read_text())
    assert saved["future_metadata"] == record["future_metadata"]
    assert "_unknown" not in saved
    assert manager.status("worker")["future_metadata"] == record["future_metadata"]
    invalid = manager.get("worker")
    invalid._unknown["paused"] = False
    with pytest.raises(AgentDeliveryError, match="conflicts"):
        manager._save(invalid)


def test_stop_refuses_native_identity_replacement_during_output_capture(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    manager, fake = setup(tmp_path, monkeypatch)
    manager.start("worker", cwd=str(tmp_path))
    original_read = fake.read

    def read(pane_id: str, *, source: str, lines: int) -> str:
        monkeypatch.setattr(fake, "agent_pane", lambda _: "w1:replacement")
        return original_read(pane_id, source=source, lines=lines)

    monkeypatch.setattr(fake, "read", read)
    with pytest.raises(HerdrUnavailable, match="no longer owns"):
        manager.stop("worker")
    assert not fake.closed
    assert manager.get("worker").lifecycle == "running"
