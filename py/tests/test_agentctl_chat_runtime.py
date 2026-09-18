"""Push intake stays responsive while independent transport and harness work waits."""

from __future__ import annotations

import hashlib
import queue
import threading
from collections.abc import Callable, Iterator, Sequence
from concurrent.futures import Future
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest

import agentctl.chat as chat_module
import agentctl.chat_runtime as runtime_module
from agentctl.agent import Target
from agentctl.chat import Bridge, Config, _read, _write, submit_reply
from agentctl.chat_input import InputStreamError
from agentctl.chat_output import PaneOutputSnapshot
from agentctl.chat_runtime import _Notice, _Runtime
from agentctl.client import AgentPaneInfo
from agentctl.jsonx import as_mapping
from tests.test_herdr_chat import Harness


_SPACE = "spaces/test"
_THREAD = _SPACE + "/threads/conversation"


def _message(name: str = "one", **fields: object) -> dict[str, object]:
    message: dict[str, object] = {
        "id": _SPACE + "/messages/" + name, "sender": "users/owner", "thread": _THREAD,
        "text": name, "created_at": "2026-01-02T00:00:00Z", "thread_reply": True,
    }
    message.update(fields)
    return message


def _key(name: str = "one") -> str:
    return hashlib.sha256((_SPACE + "/messages/" + name).encode()).hexdigest()


class Gate:
    def __init__(self) -> None:
        self.entered = threading.Event()
        self.release = threading.Event()
        self.expired = threading.Event()

    def wait(self) -> None:
        self.entered.set()
        if not self.release.wait(10):
            self.expired.set()
            raise AssertionError("fixture gate was not released")


class GatedHarness(Harness):
    def __init__(self) -> None:
        super().__init__()
        self.prompted = threading.Event()
        self.lookup: Gate | None = None

    def pane_info(self, pane_id: str) -> AgentPaneInfo:
        if self.lookup is not None:
            self.lookup.wait()
        return super().pane_info(pane_id)

    def prompt_agent(self, pane_id: str, text: str) -> None:
        super().prompt_agent(pane_id, text)
        self.prompted.set()


class Transport:
    def __init__(self) -> None:
        self.gates: dict[str, Gate] = {}
        self.lock = threading.Lock()
        self.requests: list[dict[str, object]] = []
        self.sent: dict[str, str] = {}
        self.lose_send_once = False
        self.poll_messages: list[dict[str, object]] = []

    def gate(self, action: str) -> Gate:
        gate = Gate()
        self.gates[action] = gate
        return gate

    def calls(self, action: str) -> list[dict[str, object]]:
        with self.lock:
            return [dict(row) for row in self.requests if row["action"] == action]

    def __call__(self, request: dict[str, object]) -> dict[str, object]:
        action = str(request["action"])
        with self.lock:
            self.requests.append(dict(request))
            lose = action == "send" and self.lose_send_once
            if action == "send":
                self.lose_send_once = False
                request_id = str(request["request_id"])
                reply_id = self.sent.setdefault(request_id, _SPACE + "/messages/reply-" + request_id)
        gate = self.gates.get(action)
        if gate is not None:
            gate.wait()
        if action == "poll":
            return {"messages": list(self.poll_messages), "cursor": None}
        if action == "react":
            return {"id": str(request["message"]) + "/reactions/robot"}
        if action == "send":
            if lose:
                raise OSError("reply accepted but acknowledgement lost")
            return {"id": reply_id}
        raise AssertionError("unexpected transport action")


class Rig:
    def __init__(self, state: Path) -> None:
        self.state = state
        self.harness = GatedHarness()
        self.transport = Transport()
        pane_id = "fixture:" + hashlib.sha256(str(state).encode()).hexdigest()[:16]
        target = Target(pane_id=pane_id, expected_agent="codex",
                        expected_cwd="/work/project", expected_workspace="project")
        Bridge.initialize(state, Config(_SPACE, ("users/owner",), target, "fixture-agent",
                                       event_command=("fixture-events",)), after="2026-01-01T00:00:00Z")
        self.bridge = Bridge(state, self.harness, self.transport)
        self.instances: list[_Runtime] = []
        self.runtime = self.restart()

    def restart(self) -> _Runtime:
        self.bridge = Bridge(self.state, self.harness, self.transport)
        instance = _Runtime(self.bridge, 300, "test-chat", threading.Event())
        instance.reconcile_requested = False
        instance.next_poll = float("inf")
        self.instances.append(instance)
        self.runtime = instance
        return instance

    def record(self, name: str = "one") -> dict[str, object]:
        return _read(self.state / "requests" / (_key(name) + ".json"))

    def phase(self, name: str = "one") -> object:
        return self.record(name)["phase"]

    def ack(self, name: str = "one") -> object:
        return as_mapping(self.record(name)["ack"], "ack")["state"]

    def accept(self, name: str = "one", cursor: str = "cursor-one", **fields: object) -> None:
        self.runtime._input_event({"type": "message", "message": _message(name, **fields), "cursor": cursor})

    def until(self, predicate: Callable[[], bool]) -> None:
        while not predicate():
            try:
                notice = self.runtime.events.get(timeout=5)
            except queue.Empty:
                pytest.fail(f"runtime produced no completion; remaining jobs: {self.runtime.jobs}")
            self.runtime._handle(notice)
            self.runtime._stage()

    def finish(self) -> None:
        for gate in self.transport.gates.values():
            gate.release.set()
        if self.harness.lookup is not None:
            self.harness.lookup.release.set()
        for instance in self.instances:
            instance.stop.set()
            if instance.input.thread.ident is not None:
                instance.input.close()
            if instance.output.thread.ident is not None:
                instance.output.close()
            for pool in instance.pools.values():
                pool.shutdown(wait=True, cancel_futures=True)
        gates = list(self.transport.gates.values())
        if self.harness.lookup is not None:
            gates.append(self.harness.lookup)
        assert not any(gate.expired.is_set() for gate in gates), "a blocked lane only progressed after its test gate expired"


@pytest.fixture
def rig(tmp_path: Path) -> Iterator[Rig]:
    result = Rig(tmp_path / "bridge")
    try:
        yield result
    finally:
        result.finish()


def test_hung_ack_does_not_delay_prompt_or_overwrite_later_reply_phase(rig: Rig) -> None:
    ack = rig.transport.gate("react")
    rig.accept()
    rig.runtime._stage()
    assert ack.entered.wait(5)
    assert rig.harness.prompted.wait(5)
    assert not ack.release.is_set()
    rig.until(lambda: rig.phase() == "awaiting_reply")
    assert rig.ack() == "pending"
    submit_reply(rig.state, _key(), "Finished while the reaction is pending")
    rig.runtime._stage()
    rig.until(lambda: rig.phase() == "replied")
    replied = rig.record()
    assert not ack.release.is_set()
    ack.release.set()
    rig.until(lambda: rig.ack() == "acked")
    current = rig.record()
    assert current["phase"] == "replied"
    assert current["reply_id"] == replied["reply_id"]
    assert current["reply_nonce"] == replied["reply_nonce"]
    assert len(rig.harness.prompts) == 1


def test_blocked_cursor_commit_does_not_delay_ack_or_native_delivery(
    rig: Rig, monkeypatch: pytest.MonkeyPatch,
) -> None:
    rig.runtime._input_event({"type": "checkpoint", "cursor": "previous"})
    checkpoint = Gate()
    ack = rig.transport.gate("react")
    failures: list[BaseException] = []
    finished = threading.Event()

    def blocked_cursor(path: Path, document: dict[str, object]) -> None:
        if path == rig.state / "input.json":
            checkpoint.wait()
        _write(path, document)

    def accept() -> None:
        try:
            rig.accept()
        except BaseException as exc:
            failures.append(exc)
        finally:
            finished.set()

    monkeypatch.setattr(runtime_module, "_write", blocked_cursor)
    # This intake must not run unrelated polling or housekeeping while making
    # the newly durable request eligible for the two independent workers.
    rig.runtime.reconcile_requested = True
    owner = threading.Thread(target=accept)
    owner.start()
    try:
        assert checkpoint.entered.wait(5)
        assert ack.entered.wait(5)
        assert rig.harness.prompted.wait(5)
        assert not finished.is_set()
        assert not checkpoint.release.is_set()
        assert _read(rig.state / "input.json")["cursor"] == "previous"
        saved = rig.record()
        attempted = as_mapping(saved["ack"], "ack")
        assert saved["phase"] == "received"
        assert attempted["attempts"] == 1
        assert attempted["last_attempt_at"] is not None
        assert attempted["next_retry_at"] is not None
        assert rig.transport.calls("react")[0]["request_id"] == attempted["request_id"]
        assert len(rig.harness.prompts) == 1
        assert rig.transport.calls("poll") == []
        assert not (rig.state / "output.json").exists()
    finally:
        checkpoint.release.set()
        owner.join(timeout=5)
        ack.release.set()
    assert not owner.is_alive()
    assert not checkpoint.expired.is_set()
    assert failures == []
    assert _read(rig.state / "input.json")["cursor"] == "cursor-one"
    rig.runtime.reconcile_requested = False
    rig.until(lambda: rig.phase() == "awaiting_reply" and rig.ack() == "acked")


@pytest.mark.parametrize("failed_commit", ["request", "ack-attempt"])
def test_failed_request_or_attempt_commit_cannot_start_external_effects(
    rig: Rig, monkeypatch: pytest.MonkeyPatch, failed_commit: str,
) -> None:
    def failed_write(path: Path, document: dict[str, object]) -> None:
        if path.parent == rig.state / "requests":
            raise OSError("fixture persistence failure")
        _write(path, document)

    module = chat_module if failed_commit == "request" else runtime_module
    monkeypatch.setattr(module, "_write", failed_write)
    with pytest.raises(OSError, match="persistence failure"):
        rig.accept()
    assert rig.transport.requests == []
    assert rig.harness.prompts == []
    assert not (rig.state / "input.json").exists()
    for phase in ("inbox", "inflight", "processed", "failed"):
        assert list((rig.state / "queue" / phase).glob("*.json")) == []
    if failed_commit == "request":
        assert list((rig.state / "requests").glob("*.json")) == []
    else:
        assert as_mapping(rig.record()["ack"], "ack")["attempts"] == 0


def test_fast_completion_with_full_mailbox_cannot_block_owner_start(
    rig: Rig, monkeypatch: pytest.MonkeyPatch,
) -> None:
    events = rig.runtime.events
    for _ in range(events.maxsize):
        events.put_nowait(_Notice("fixture"))
    worker_ready = threading.Event()
    owner_returned = threading.Event()
    completion_posted = threading.Event()
    original_post = runtime_module._post
    original_submit = rig.runtime.pools["ack"].submit

    def post(mailbox: queue.Queue[_Notice], notice: _Notice, stop: threading.Event) -> None:
        worker_ready.set()
        original_post(mailbox, notice, stop)
        completion_posted.set()

    def submit(operation: Callable[[], object]) -> Future[object]:
        future = original_submit(operation)
        future.add_done_callback(lambda completed: worker_ready.set())
        # Return only once the fast operation has completed or its worker has
        # reached the full mailbox. This forces the completion-registration race.
        assert worker_ready.wait(5)
        return future

    monkeypatch.setattr(runtime_module, "_post", post)
    monkeypatch.setattr(rig.runtime.pools["ack"], "submit", submit)

    def start() -> None:
        rig.runtime._start("ack", "fixture", lambda: {"id": "fixture"})
        owner_returned.set()

    owner = threading.Thread(target=start)
    owner.start()
    try:
        assert owner_returned.wait(5), "the event-loop owner waited for space in its own mailbox"
        assert events.full()
        assert not completion_posted.is_set()
    finally:
        events.get_nowait()
        owner.join(timeout=5)
    assert not owner.is_alive()
    assert completion_posted.wait(5)
    notices = [events.get_nowait() for _ in range(events.maxsize)]
    assert sum(notice.kind == "complete" for notice in notices) == 1


@pytest.mark.parametrize("state", ["working", "blocked"])
def test_busy_or_blocked_harness_does_not_delay_ack(rig: Rig, state: str) -> None:
    rig.harness.state = state
    rig.accept()
    rig.runtime._stage()
    rig.until(lambda: rig.ack() == "acked")
    assert rig.phase() == "queued"
    assert rig.harness.prompts == []
    rig.harness.state = "idle"
    rig.runtime.next_drain = 0
    rig.runtime._stage()
    rig.until(lambda: rig.phase() == "awaiting_reply")
    assert len(rig.harness.prompts) == 1


def test_slow_harness_lookup_cannot_hold_intake_or_ack_workers(rig: Rig) -> None:
    lookup = rig.harness.lookup = Gate()
    rig.accept()
    rig.runtime._stage()
    assert lookup.entered.wait(5)
    rig.accept("two", "cursor-two")
    rig.runtime._stage()
    rig.until(lambda: rig.ack("one") == "acked" and rig.ack("two") == "acked")
    assert not lookup.release.is_set()
    assert not lookup.expired.is_set()
    assert _read(rig.state / "input.json")["cursor"] == "cursor-two"
    assert rig.harness.prompts == []
    lookup.release.set()
    rig.until(lambda: rig.phase("one") == "awaiting_reply" and rig.phase("two") == "awaiting_reply")


def test_delayed_poll_and_send_do_not_stall_new_push_acceptance(rig: Rig) -> None:
    poll = rig.transport.gate("poll")
    rig.runtime.reconcile_requested = True
    rig.runtime._stage()
    assert poll.entered.wait(5)
    rig.accept()
    rig.runtime._stage()
    rig.until(lambda: rig.phase() == "awaiting_reply" and rig.ack() == "acked")
    send = rig.transport.gate("send")
    submit_reply(rig.state, _key(), "First answer")
    rig.runtime._stage()
    assert send.entered.wait(5)
    rig.accept("two", "cursor-two")
    rig.runtime._stage()
    rig.until(lambda: rig.phase("two") == "awaiting_reply" and rig.ack("two") == "acked")
    assert not poll.release.is_set() and not send.release.is_set()
    assert _read(rig.state / "input.json")["cursor"] == "cursor-two"
    assert len(rig.harness.prompts) == 2
    assert len(rig.transport.calls("poll")) == 1
    assert len(rig.transport.calls("send")) == 1


def test_cursor_commit_follows_durable_intake_and_restart_replays_only_once(
    rig: Rig, monkeypatch: pytest.MonkeyPatch,
) -> None:
    original = _write
    observed: list[object] = []
    ack = rig.transport.gate("react")

    def crash_cursor(path: Path, document: dict[str, object]) -> None:
        if path == rig.state / "input.json":
            saved = rig.record()
            assert saved["phase"] == "received"
            assert as_mapping(saved["message"], "source")["id"] == _SPACE + "/messages/one"
            assert as_mapping(saved["ack"], "ack")["attempts"] == 1
            assert ack.entered.wait(5)
            assert rig.harness.prompted.wait(5)
            observed.append(saved["reply_nonce"])
            raise OSError("simulated crash before cursor commit")
        original(path, document)

    monkeypatch.setattr(runtime_module, "_write", crash_cursor)
    with pytest.raises(OSError, match="before cursor"):
        rig.accept()
    assert len(observed) == 1
    assert not (rig.state / "input.json").exists()
    # Lose the old owner's completions after the effects reached their workers.
    # The restarted owner must use the durable queue and ACK retry identities.
    rig.runtime.stop.set()
    ack.release.set()
    for pool in rig.runtime.pools.values():
        pool.shutdown(wait=True, cancel_futures=True)
    retry_at = as_mapping(rig.record()["ack"], "ack")["next_retry_at"]
    assert isinstance(retry_at, str)
    retry_due = datetime.fromisoformat(retry_at.replace("Z", "+00:00")) + timedelta(seconds=1)

    class RetryClock:
        @staticmethod
        def now(zone: timezone) -> datetime:
            assert zone is timezone.utc
            return retry_due

    monkeypatch.setattr(runtime_module, "datetime", RetryClock)
    monkeypatch.setattr(runtime_module, "_write", original)
    rig.restart()
    rig.accept()
    rig.accept()
    assert rig.record()["reply_nonce"] == observed[0]
    assert _read(rig.state / "input.json")["cursor"] == "cursor-one"
    rig.runtime._stage()
    rig.until(lambda: rig.phase() == "awaiting_reply" and rig.ack() == "acked")
    assert len(list((rig.state / "requests").glob("*.json"))) == 1
    assert len(list((rig.state / "queue" / "processed").glob("*.json"))) == 1
    assert len(rig.harness.prompts) == 1
    reactions = rig.transport.calls("react")
    assert len(reactions) == 2
    assert reactions[0]["request_id"] == reactions[1]["request_id"]
    assert [reaction["attempt"] for reaction in reactions] == [1, 2]


def test_echo_before_lost_send_confirmation_is_durable_then_deduplicated(rig: Rig) -> None:
    rig.accept()
    rig.runtime._stage()
    rig.until(lambda: rig.phase() == "awaiting_reply" and rig.ack() == "acked")
    send = rig.transport.gate("send")
    rig.transport.lose_send_once = True
    submit_reply(rig.state, _key(), "Completed")
    rig.runtime._stage()
    assert send.entered.wait(5)
    request_id = str(rig.record()["request_id"])
    with rig.transport.lock:
        reply_id = rig.transport.sent[request_id]
    echo = _message("echo", id=reply_id, text="[fixture-agent] Completed")
    rig.runtime._input_event({"type": "message", "message": echo, "cursor": "cursor-echo"})
    assert len(list(rig.runtime.deferred.glob("*.json"))) == 1
    assert _read(rig.state / "input.json")["cursor"] == "cursor-echo"
    assert len(list((rig.state / "requests").glob("*.json"))) == 1
    send.release.set()
    rig.until(lambda: "reply_error" in rig.record())
    assert rig.phase() == "reply_pending"
    assert len(list(rig.runtime.deferred.glob("*.json"))) == 1
    rig.runtime.retry[("send", _key())] = (0, 1)
    rig.runtime._stage()
    rig.until(lambda: rig.phase() == "replied")
    rig.runtime._stage()
    assert list(rig.runtime.deferred.glob("*.json")) == []
    assert len(list((rig.state / "requests").glob("*.json"))) == 1
    assert len(rig.harness.prompts) == 1
    sends = rig.transport.calls("send")
    assert len(sends) == 2
    assert sends[0]["request_id"] == sends[1]["request_id"] == request_id


@pytest.mark.parametrize("change", ["space", "thread", "cursor", "type"])
def test_invalid_stream_authority_never_advances_cursor_or_queues_input(rig: Rig, change: str) -> None:
    event: dict[str, object] = {"type": "message", "message": _message(), "cursor": "bad-cursor"}
    if change == "space":
        event["message"] = _message(id="spaces/other/messages/one")
    elif change == "thread":
        event["message"] = _message(thread="spaces/other/threads/one")
    elif change == "cursor":
        event["cursor"] = 123
    else:
        event["type"] = "unexpected"
    with pytest.raises((ValueError, TypeError)):
        rig.runtime._input_event(event)
    assert not (rig.state / "input.json").exists()
    assert list((rig.state / "requests").glob("*.json")) == []
    assert rig.transport.requests == []
    assert rig.harness.prompts == []


def test_excluded_sender_advances_safe_checkpoint_without_task_authority(rig: Rig) -> None:
    rig.accept(sender="users/unapproved")
    assert _read(rig.state / "input.json")["cursor"] == "cursor-one"
    assert list((rig.state / "requests").glob("*.json")) == []
    assert rig.transport.requests == []
    assert rig.harness.prompts == []


def test_source_supplied_nonce_and_unknown_output_cannot_authorize_a_reply(rig: Rig) -> None:
    supplied = "X" * 22
    rig.accept(reply_nonce=supplied)
    rig.runtime._stage()
    rig.until(lambda: rig.phase() == "awaiting_reply" and rig.ack() == "acked")
    assert rig.record()["reply_nonce"] != supplied
    target = rig.bridge.config.target.pane_id
    assert target is not None
    text = f"<GCHAT_REPLY_{supplied}>\nUnrelated output\n</GCHAT_REPLY_{supplied}>"
    rig.runtime._handle(_Notice("output", PaneOutputSnapshot(target, text, False)))
    rig.runtime._stage()
    rig.until(lambda: not rig.runtime.jobs and not rig.runtime.output_pending)
    assert list((rig.state / "replies").glob("*.json")) == []
    assert rig.transport.calls("send") == []
    assert rig.phase() == "awaiting_reply"


def test_output_identity_lookup_cannot_delay_next_message_intake_or_ack(rig: Rig) -> None:
    rig.accept()
    rig.runtime._stage()
    rig.until(lambda: rig.phase() == "awaiting_reply" and rig.ack() == "acked")
    nonce = rig.record()["reply_nonce"]
    target = rig.bridge.config.target.pane_id
    assert target is not None
    snapshot = PaneOutputSnapshot(target, f"<GCHAT_REPLY_{nonce}>\nFirst answer\n</GCHAT_REPLY_{nonce}>", False)
    lookup = rig.harness.lookup = Gate()
    rig.runtime._handle(_Notice("output", snapshot))
    rig.runtime._stage()
    assert lookup.entered.wait(5)
    rig.accept("two", "cursor-two")
    rig.runtime._stage()
    rig.until(lambda: rig.ack("two") == "acked")
    assert not lookup.release.is_set()
    assert not lookup.expired.is_set()
    assert _read(rig.state / "input.json")["cursor"] == "cursor-two"
    assert rig.phase() == "awaiting_reply"
    lookup.release.set()
    rig.until(lambda: rig.phase() == "replied" and rig.phase("two") == "awaiting_reply")
    assert len(rig.harness.prompts) == 2


def test_input_source_reconnects_with_last_durable_cursor(
    rig: Rig, monkeypatch: pytest.MonkeyPatch,
) -> None:
    subscriptions: list[dict[str, object]] = []
    fail_source = threading.Event()
    second_connected = threading.Event()
    sleeping = threading.Event()

    class Stream:
        def __init__(self, command: Sequence[str], request: dict[str, object]) -> None:
            assert tuple(command) == ("fixture-events",)
            self.number = len(subscriptions)
            self.delivered = False
            subscriptions.append(dict(request))
            if self.number == 1:
                second_connected.set()

        def wait(self, timeout: float) -> list[dict[str, object]]:
            if self.number == 0:
                if not self.delivered:
                    self.delivered = True
                    return [{"type": "message", "message": _message(), "cursor": "durable-cursor"}]
                assert fail_source.wait(5)
                raise InputStreamError("stream_eof", "fixture source disconnected")
            sleeping.wait(5)
            return []

        def wake(self) -> None:
            sleeping.set()
            fail_source.set()

        def close(self) -> None:
            pass

    monkeypatch.setattr(runtime_module, "EventCommandStream", Stream)
    monkeypatch.setattr(rig.runtime.stop, "wait", lambda timeout=None: rig.runtime.stop.is_set())
    rig.runtime.input.thread.start()
    connected = rig.runtime.events.get(timeout=5)
    assert connected.kind == "input_connected"
    rig.runtime._handle(connected)
    incoming = rig.runtime.events.get(timeout=5)
    assert incoming.kind == "input"
    rig.runtime._handle(incoming)
    assert _read(rig.state / "input.json")["cursor"] == "durable-cursor"
    fail_source.set()
    error = rig.runtime.events.get(timeout=5)
    while error.kind == "complete":
        rig.runtime._handle(error)
        error = rig.runtime.events.get(timeout=5)
    assert error.kind == "input_error"
    rig.runtime._handle(error)
    assert _read(rig.state / "input.json")["cursor"] == "durable-cursor"
    assert second_connected.wait(5)
    assert subscriptions == [
        {"action": "subscribe", "space": _SPACE, "cursor": None},
        {"action": "subscribe", "space": _SPACE, "cursor": "durable-cursor"},
    ]
    assert len(list((rig.state / "requests").glob("*.json"))) == 1
