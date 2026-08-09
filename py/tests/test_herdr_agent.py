"""Focused contract tests for the shared interactive-agent queue."""

from __future__ import annotations

import json
import os
import threading
from pathlib import Path
from typing import cast

import pytest

from herdr_run.agent import Target, drain, enqueue, read, send, status
from herdr_run.agent import QueueResult
import herdr_run.agent_cli as agent_cli
from herdr_run.client import AgentPaneInfo, HerdrClient, Pane
from herdr_run.errors import AgentDeliveryError, AgentPending, AgentPossiblySubmitted, HerdrUnavailable


class FakeAgentHerdr:
    def __init__(self, states: list[str] | None = None) -> None:
        self.states = states or ["idle"]
        self.index = 0
        self.runs: list[str] = []
        self.waits: list[tuple[str, str, int]] = []
        self.workspace = "deepscry"
        self.cwd = "/work/mtg"
        self.session = "session-1"
        self.read_text = "agent transcript\n"
        self.run_entered = threading.Event()
        self.run_release: threading.Event | None = None

    def panes(self, workspace_id: str | None = None) -> tuple[Pane, ...]:
        del workspace_id
        return (Pane("w1:p1", "w1:t1", "w1"),)

    def pane_info(self, pane_id: str) -> AgentPaneInfo:
        assert pane_id == "w1:p1"
        state = self.states[min(self.index, len(self.states) - 1)]
        self.index += 1
        return AgentPaneInfo(pane_id, "w1", self.cwd, "codex", state, "codex", self.session)

    def workspace_label(self, workspace_id: str) -> str:
        assert workspace_id == "w1"
        return self.workspace

    def run(self, pane_id: str, text: str) -> None:
        assert pane_id == "w1:p1"
        self.run_entered.set()
        if self.run_release is not None:
            assert self.run_release.wait(5)
        self.runs.append(text)

    def wait_agent_status(self, pane_id: str, state: str, timeout_ms: int) -> None:
        self.waits.append((pane_id, state, timeout_ms))
        if state != "working":
            raise AssertionError(state)

    def read(self, pane_id: str, *, source: str, lines: int) -> str:
        assert pane_id == "w1:p1" and source in ("recent-unwrapped", "recent") and lines == 17
        return self.read_text


def client(fake: FakeAgentHerdr) -> HerdrClient:
    return cast(HerdrClient, fake)


def target(**changes: str) -> Target:
    values = {
        "pane_id": "w1:p1", "session_agent": "codex", "session_value": "session-1",
        "expected_agent": "codex", "expected_workspace": "deepscry", "expected_cwd": "/work/mtg",
    }
    values.update(changes)
    return Target(**values)


def test_multiline_busy_then_idle_is_atomic_and_confirmed(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["working", "working", "idle"])
    text = "first line\nsecond line\nthird line"
    result = send(client(fake), target(), str(tmp_path), text, ready_timeout=1, sleep=lambda _s: None)
    assert fake.runs == [text]
    assert fake.waits == [("w1:p1", "working", 30000)]
    assert result.message_id in result.delivered


def test_busy_for_more_than_thirty_seconds_still_delivers_with_turn_sized_budget(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["working", "working", "idle"])
    clock = {"now": 0.0}

    def monotonic() -> float:
        return clock["now"]

    def sleep(_seconds: float) -> None:
        clock["now"] += 31.0

    result = send(
        client(fake), target(), str(tmp_path), "after a normal turn",
        ready_timeout=900, sleep=sleep, monotonic=monotonic,
    )
    assert clock["now"] > 30
    assert result.delivered == (result.message_id,)


def test_done_is_submit_safe(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["done"])
    send(client(fake), target(), str(tmp_path), "from done")
    assert fake.runs == ["from done"]


def test_concurrent_senders_are_serialized_and_fifo(tmp_path: object) -> None:
    root = str(tmp_path)
    fake = FakeAgentHerdr(["idle"])
    fake.run_release = threading.Event()
    errors: list[BaseException] = []

    def invoke(text: str) -> None:
        try:
            send(client(fake), target(), root, text)
        except BaseException as exc:
            errors.append(exc)

    first = threading.Thread(target=invoke, args=("first",), daemon=True)
    second = threading.Thread(target=invoke, args=("second",), daemon=True)
    first.start()
    assert fake.run_entered.wait(5)
    second.start()
    second.join(0.05)
    assert second.is_alive()
    fake.run_release.set()
    first.join(5)
    second.join(5)
    assert errors == []
    assert fake.runs == ["first", "second"]


def test_working_confirmation_failure_never_resubmits_and_marks_ambiguous(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["idle"])

    def fail(*_args: object) -> None:
        raise HerdrUnavailable("no working transition")

    fake.wait_agent_status = fail  # type: ignore[assignment]
    with pytest.raises(AgentDeliveryError, match="retained"):
        send(client(fake), target(), str(tmp_path), "preserve me", max_attempts=2)
    failed = list((tmp_path / "failed").glob("*.json"))  # type: ignore[operator]
    assert len(failed) == 1
    document = json.loads(failed[0].read_text())
    assert document["text"] == "preserve me"
    assert document["delivery_attempts"] == 1
    assert document["possibly_submitted"] is True
    assert len(fake.runs) == 1


@pytest.mark.parametrize(
    "change,expected",
    [
        ({"session_value": "wrong"}, "found 0"),
        ({"expected_workspace": "wrong"}, "workspace"),
        ({"expected_cwd": "/wrong"}, "cwd"),
        ({"expected_agent": "claude"}, "agent"),
    ],
)
def test_wrong_identity_workspace_or_cwd_refuses(
    tmp_path: object, change: dict[str, str], expected: str
) -> None:
    fake = FakeAgentHerdr(["idle"])
    with pytest.raises(AgentDeliveryError, match=expected):
        send(client(fake), target(**change), str(tmp_path), "never submit", max_attempts=1)
    assert fake.runs == []
    assert len(list((tmp_path / "inbox").glob("*.json"))) == 1  # type: ignore[operator]
    assert len(list((tmp_path / "failed").glob("*.json"))) == 0  # type: ignore[operator]


def test_timeout_retains_prompt_and_loud_error(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["working"])
    with pytest.raises(AgentDeliveryError, match="remains pending"):
        send(client(fake), target(), str(tmp_path), "poll artifact survives", ready_timeout=0, max_attempts=1)
    pending = next((tmp_path / "inbox").glob("*.json"))  # type: ignore[operator]
    document = json.loads(pending.read_text())
    assert document["text"] == "poll artifact survives"
    assert document["delivery_attempts"] == 0
    assert list((tmp_path / "failed").glob("*.json")) == []  # type: ignore[operator]


def test_fast_idle_working_idle_transition_is_confirmed_without_screen_matching(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["idle", "idle"])

    def fast_transition(pane_id: str, state: str, timeout_ms: int) -> None:
        assert (pane_id, state, timeout_ms) == ("w1:p1", "working", 30000)
        # Native wait observed working even though a later point probe is already idle.
        fake.states = ["idle"]

    fake.wait_agent_status = fast_transition  # type: ignore[method-assign]
    result = send(client(fake), target(), str(tmp_path), "fast turn")
    assert result.delivered == (result.message_id,)
    assert fake.runs == ["fast turn"]
    assert status(client(fake), target(), str(tmp_path))["agent_status"] == "idle"


def test_post_run_transport_crash_is_quarantined_once_as_possibly_submitted(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["idle"])

    def crash_after_possible_accept(_pane_id: str, text: str) -> None:
        fake.runs.append(text)
        raise RuntimeError("connection vanished after write")

    fake.run = crash_after_possible_accept  # type: ignore[assignment]
    with pytest.raises(AgentDeliveryError, match="may have been submitted"):
        send(client(fake), target(), str(tmp_path), "only once", max_attempts=3)
    assert fake.runs == ["only once"]
    document = json.loads(next((tmp_path / "failed").glob("*.json")).read_text())  # type: ignore[operator]
    assert document["possibly_submitted"] is True
    assert document["delivery_attempts"] == 1


def test_queue_binding_refuses_a_different_session_without_moving_prompt(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["working"])
    with pytest.raises(AgentDeliveryError, match="remains pending"):
        send(client(fake), target(), str(tmp_path), "bound prompt", ready_timeout=0)
    with pytest.raises(AgentDeliveryError, match="bound to"):
        drain(client(fake), target(session_value="different"), str(tmp_path))
    pending = next((tmp_path / "inbox").glob("*.json"))  # type: ignore[operator]
    assert "bound prompt" in pending.read_text()


def test_status_and_read_cover_arbitrary_target(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["idle"])
    queued = enqueue(str(tmp_path), "queued")
    snapshot = status(client(fake), target(), str(tmp_path))
    assert snapshot["pending"] == [queued]
    assert snapshot["session_value"] == "session-1"
    assert read(client(fake), target(), lines=17) == "agent transcript\n"


def test_read_falls_back_when_unwrapped_source_is_empty(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["idle"])
    sources: list[str] = []

    def source_read(_pane_id: str, *, source: str, lines: int) -> str:
        assert lines == 17
        sources.append(source)
        return "" if source == "recent-unwrapped" else "fallback transcript\n"

    fake.read = source_read  # type: ignore[assignment]
    assert read(client(fake), target(), lines=17) == "fallback transcript\n"
    assert sources == ["recent-unwrapped", "recent"]


def test_drain_accepts_existing_subagent_message_shape(tmp_path: object) -> None:
    inbox = tmp_path / "inbox"  # type: ignore[operator]
    inbox.mkdir()
    path = inbox / "000000000007.json"
    path.write_text(json.dumps({"seq": 7, "text": "legacy fifo", "tui_delivery_attempts": 0}))
    fake = FakeAgentHerdr(["idle"])
    result = drain(client(fake), target(), str(tmp_path))
    assert result.delivered == ("000000000007",)
    assert fake.runs == ["legacy fifo"]
    assert os.path.isfile(tmp_path / "processed" / path.name)  # type: ignore[operator]


def test_drain_cli_returns_temporary_failure_when_fifo_remains_blocked(
    tmp_path: object, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        agent_cli,
        "drain",
        lambda *_args, **_kwargs: QueueResult("", (), (), ("pending",), "agent still working"),
    )
    assert agent_cli.main(["drain", "--pane", "w1:p1", "--queue", str(tmp_path)]) == 75


@pytest.mark.parametrize("raw", [b"{not json\n", b'{"id":"bad","text":7}\n'])
def test_invalid_fifo_head_preserves_raw_and_does_not_block_valid(
    tmp_path: object, raw: bytes
) -> None:
    inbox = tmp_path / "inbox"  # type: ignore[operator]
    inbox.mkdir()
    bad = inbox / "000000000001.json"
    bad.write_bytes(raw)
    good = inbox / "000000000002.json"
    good.write_text(json.dumps({"id": "good", "text": "deliver me"}) + "\n")
    fake = FakeAgentHerdr(["idle"])
    result = drain(client(fake), target(), str(tmp_path))
    failed = tmp_path / "failed" / bad.name  # type: ignore[operator]
    assert failed.read_bytes() == raw
    metadata = json.loads((tmp_path / "failed" / f"{bad.name}.error").read_text())  # type: ignore[operator]
    assert metadata["outcome"] == "invalid_message"
    assert result.quarantined == ("000000000001",)
    assert result.delivered == ("good",)
    assert fake.runs == ["deliver me"]


def test_crash_after_run_before_wait_is_never_resubmitted_on_restart(tmp_path: object) -> None:
    fake = FakeAgentHerdr(["idle"])

    def crash_after_run(_pane: str, _state: str, _timeout: int) -> None:
        raise KeyboardInterrupt("simulated process death")

    fake.wait_agent_status = crash_after_run  # type: ignore[assignment]
    with pytest.raises(KeyboardInterrupt):
        send(client(fake), target(), str(tmp_path), "at most once")
    assert fake.runs == ["at most once"]
    assert len(list((tmp_path / "inflight").glob("*.json"))) == 1  # type: ignore[operator]

    restarted = FakeAgentHerdr(["idle"])
    result = drain(client(restarted), target(), str(tmp_path))
    assert restarted.runs == []
    assert len(list((tmp_path / "inflight").glob("*.json"))) == 0  # type: ignore[operator]
    assert len(list((tmp_path / "failed").glob("*.json"))) == 1  # type: ignore[operator]
    assert result.outcome == "possibly_submitted"


def test_send_failures_have_distinct_typed_machine_outcomes(tmp_path: Path) -> None:
    busy = FakeAgentHerdr(["working"])
    with pytest.raises(AgentPending) as pending:
        send(client(busy), target(), str(tmp_path / "pending"), "safe", ready_timeout=0)
    assert pending.value.outcome == "pending"
    assert os.path.isfile(pending.value.artifact)

    ambiguous = FakeAgentHerdr(["idle"])
    ambiguous.wait_agent_status = lambda *_args: (_ for _ in ()).throw(HerdrUnavailable("lost"))  # type: ignore[method-assign]
    with pytest.raises(AgentPossiblySubmitted) as submitted:
        send(client(ambiguous), target(), str(tmp_path / "ambiguous"), "unsafe")
    assert submitted.value.outcome == "possibly_submitted"
    assert os.path.isfile(submitted.value.artifact)


def test_send_cli_emits_structured_distinct_failure(monkeypatch: pytest.MonkeyPatch, tmp_path: Path, capsys: pytest.CaptureFixture[str]) -> None:
    artifact = str(tmp_path / "inbox" / "m.json")
    monkeypatch.setattr(
        agent_cli,
        "send",
        lambda *_a, **_k: (_ for _ in ()).throw(
            AgentPending("busy", message_id="m", artifact=artifact)
        ),
    )
    assert agent_cli.main(["send", "--pane", "w1:p1", "hello"]) == 75
    assert json.loads(capsys.readouterr().out) == {
        "artifact": artifact,
        "error": "busy",
        "message_id": "m",
        "outcome": "pending",
        "safe_to_retry": True,
    }

    failed_artifact = str(tmp_path / "failed" / "m2.json")
    monkeypatch.setattr(
        agent_cli,
        "send",
        lambda *_a, **_k: (_ for _ in ()).throw(
            AgentPossiblySubmitted("unknown", message_id="m2", artifact=failed_artifact)
        ),
    )
    assert agent_cli.main(["send", "--pane", "w1:p1", "hello"]) == 76
    assert json.loads(capsys.readouterr().out)["safe_to_retry"] is False


def test_drain_cli_distinguishes_quarantine_from_pending(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        agent_cli,
        "drain",
        lambda *_args, **_kwargs: QueueResult(
            "", (), ("ambiguous",), (), None, "possibly_submitted"
        ),
    )
    assert agent_cli.main(["drain", "--pane", "w1:p1", "--queue", str(tmp_path)]) == 76


def test_enqueue_and_delivery_state_transitions_fsync(monkeypatch: pytest.MonkeyPatch, tmp_path: Path) -> None:
    calls: list[int] = []
    real_fsync = os.fsync

    def recording_fsync(descriptor: int) -> None:
        calls.append(descriptor)
        real_fsync(descriptor)

    monkeypatch.setattr("herdr_run.agent.os.fsync", recording_fsync)
    fake = FakeAgentHerdr(["idle"])
    send(client(fake), target(), str(tmp_path), "durable")
    # Enqueue file+directory, inbox->inflight, inflight update, and
    # inflight->processed each require syncs; keep the assertion structural.
    assert len(calls) >= 8
