"""Tagged replies through durable Chat intake, output capture, and daemon scheduling."""

from __future__ import annotations

import re
from collections.abc import Callable, Sequence
from pathlib import Path

import pytest

import agentctl.chat as chat_module
from agentctl.chat import Bridge, _read
from agentctl.chat_output import OutputStreamError, PaneAgentStatus, PaneOutputSnapshot, PaneOutputStream
from agentctl.errors import HerdrUnavailable
from agentctl.jsonx import as_mapping, as_sequence
from tests.test_herdr_chat import Harness, setup


def _records(state: Path) -> dict[str, dict[str, object]]:
    records = [_read(path) for path in (state / "requests").glob("*.json")]
    return {str(as_mapping(record["message"], "message")["id"]).rsplit("/", 1)[-1]: record
            for record in records}


def _block(record: dict[str, object], text: str = "Finished") -> str:
    nonce = record["reply_nonce"]
    return f"<CHAT_REPLY_{nonce}>\n{text}\n</CHAT_REPLY_{nonce}>"


def _snapshot(text: str, *, pane: str = "w1:p1", truncated: bool = False) -> PaneOutputSnapshot:
    return PaneOutputSnapshot(pane, text, truncated, 0)


def test_default_nonce_prompt_is_short_durable_and_not_itself_a_reply(tmp_path: Path) -> None:
    bridge, harness, chat = setup(tmp_path)
    chat.message()
    bridge.tick()
    record = _records(tmp_path)["one"]
    nonce = str(record["reply_nonce"])
    assert re.fullmatch(r"[A-Za-z0-9_-]{22}", nonce)
    prompt = harness.prompts[0]
    assert f"<CHAT_REPLY_{nonce}>" in prompt and f"</CHAT_REPLY_{nonce}>" in prompt
    assert "one or multiple replies" in prompt and "separate chat message" in prompt
    assert "Google Chat" not in prompt and "GCHAT_REPLY" not in prompt
    assert "PATH_TO_YOUR_REPLY" not in prompt and " --file " not in prompt
    assert "Source: spaces/test/messages/one" in prompt
    assert len(prompt) < 700
    assert bridge.capture_output(_snapshot(prompt)) == {"captured": [], "errors": []}
    assert not chat.sent
    restarted = Bridge(tmp_path, harness, chat)
    restarted.tick()
    assert _records(tmp_path)["one"]["reply_nonce"] == nonce
    assert len(harness.prompts) == 1


def test_capture_is_durable_threaded_and_deduplicated_after_restart(tmp_path: Path) -> None:
    bridge, harness, chat = setup(tmp_path)
    chat.message()
    bridge.tick()
    record = _records(tmp_path)["one"]
    snapshot = _snapshot(harness.prompts[0] + "\n\n" + _block(record, "The answer 🤖"))
    outcome = bridge.capture_output(snapshot)
    key = str(record["key"])
    assert outcome == {"captured": [key], "errors": []}
    assert _read(tmp_path / "replies" / f"{key}.json")["text"] == "The answer 🤖"
    saved = _records(tmp_path)["one"]
    assert saved["phase"] == "replied"
    assert as_mapping(saved["reply_capture"], "capture")["source"] == "herdr_output"
    sent = next(iter(chat.sent.values()))
    assert sent["thread"] == "spaces/test/threads/one"
    assert sent["text"] == "[test-agent] The answer 🤖"
    restarted = Bridge(tmp_path, harness, chat)
    assert restarted.capture_output(snapshot) == {"captured": [], "errors": []}
    restarted.tick()
    assert len(harness.prompts) == 1
    assert len([call for call in chat.calls if call["action"] == "send"]) == 1
    assert restarted.output_requests() == {str(record["reply_nonce"]): str(record["key"])}


def test_two_identical_blocks_send_two_replies_and_repeated_snapshot_sends_none(tmp_path: Path) -> None:
    bridge, _, chat = setup(tmp_path)
    chat.message()
    bridge.tick()
    block = _block(_records(tmp_path)["one"])
    snapshot = _snapshot(block + "\n" + block)
    assert not bridge.capture_output(snapshot)["errors"]
    assert len(chat.sent) == 2
    assert bridge.capture_output(snapshot) == {"captured": [], "errors": []}
    assert len(chat.sent) == 2
    assert len([call for call in chat.calls if call["action"] == "send"]) == 2


def test_snapshot_from_wrong_pane_is_refused_before_transport(tmp_path: Path) -> None:
    bridge, _, chat = setup(tmp_path)
    chat.message()
    bridge.tick()
    before = len(chat.calls)
    with pytest.raises(HerdrUnavailable, match="different coordinator pane"):
        bridge.capture_output(_snapshot(_block(_records(tmp_path)["one"]), pane="w9:p1"))
    assert len(chat.calls) == before
    assert not list((tmp_path / "replies").glob("*.json"))


def test_unknown_request_marker_does_not_complete_current_request(tmp_path: Path) -> None:
    bridge, _, chat = setup(tmp_path)
    chat.message()
    bridge.tick()
    text = "<GCHAT_REPLY_XXXXXXXXXXXXXXXXXXXXXX>\nWrong request\n</GCHAT_REPLY_XXXXXXXXXXXXXXXXXXXXXX>"
    assert bridge.capture_output(_snapshot(text)) == {"captured": [], "errors": []}
    assert not chat.sent
    assert _records(tmp_path)["one"]["phase"] == "awaiting_reply"


def test_lost_transport_ack_retries_artifact_without_reprompt_or_recapture(tmp_path: Path) -> None:
    bridge, harness, chat = setup(tmp_path)
    chat.message()
    bridge.tick()
    record = _records(tmp_path)["one"]
    chat.lose_ack = True
    with pytest.raises(OSError, match="acknowledgement lost"):
        bridge.capture_output(_snapshot(_block(record)))
    assert _records(tmp_path)["one"]["phase"] == "reply_pending"
    assert bridge.output_requests() == {str(record["reply_nonce"]): str(record["key"])}
    restarted = Bridge(tmp_path, harness, chat)
    restarted.tick()
    sends = [call for call in chat.calls if call["action"] == "send"]
    assert len(sends) == 2 and sends[0]["request_id"] == sends[1]["request_id"]
    assert len(chat.sent) == 1 and len(harness.prompts) == 1
    assert _records(tmp_path)["one"]["phase"] == "replied"


def test_crash_after_reply_artifact_before_capture_metadata_recovers(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    bridge, harness, chat = setup(tmp_path)
    chat.message()
    bridge.tick()
    record = _records(tmp_path)["one"]
    original = chat_module._write

    def interrupted_write(path: Path, document: dict[str, object]) -> None:
        if "reply_capture" in document:
            raise OSError("simulated crash after reply fsync")
        original(path, document)

    monkeypatch.setattr(chat_module, "_write", interrupted_write)
    with pytest.raises(OSError, match="simulated crash"):
        bridge.capture_output(_snapshot(_block(record)))
    monkeypatch.setattr(chat_module, "_write", original)
    assert not chat.sent
    assert _read(tmp_path / "replies" / f"{record['key']}.json")["text"] == "Finished"
    restarted = Bridge(tmp_path, harness, chat)
    assert restarted.output_requests() == {str(record["reply_nonce"]): str(record["key"])}
    restarted.tick()
    assert len(chat.sent) == 1 and len(harness.prompts) == 1


def test_tagged_capture_confirms_uncertain_prompt_delivery(tmp_path: Path) -> None:
    bridge, harness, chat = setup(tmp_path)
    harness.fail_confirmation = True
    chat.message()
    bridge.tick()
    record = _records(tmp_path)["one"]
    assert record["phase"] == "delivery_uncertain"
    bridge.capture_output(_snapshot(_block(record)))
    saved = _records(tmp_path)["one"]
    assert saved["phase"] == "replied"
    assert saved["delivery_confirmed_by"] == "reply_artifact"
    assert len(harness.prompts) == 1


@pytest.mark.parametrize("kind", ["clipped", "fenced", "empty", "mismatched", "oversize"])
def test_bad_capture_is_visible_and_does_not_starve_another_request(tmp_path: Path, kind: str) -> None:
    bridge, _, chat = setup(tmp_path)
    chat.message("broken")
    chat.message("valid")
    bridge.tick()
    records = _records(tmp_path)
    broken, valid = records["broken"], records["valid"]
    nonce = broken["reply_nonce"]
    variants = {
        "clipped": f"Reply beginning disappeared\n</GCHAT_REPLY_{nonce}>",
        "fenced": "```text\n" + _block(broken) + "\n```",
        "empty": _block(broken, "  "),
        "mismatched": f"<GCHAT_REPLY_{nonce}>\nWrong close\n</GCHAT_REPLY_XXXXXXXXXXXXXXXXXXXXXX>\n</GCHAT_REPLY_{nonce}>",
        "oversize": _block(broken, "x" * 30001),
    }
    result = bridge.capture_output(_snapshot(variants[kind] + "\n" + _block(valid, "Valid answer"), truncated=True))
    assert result["captured"] == [valid["key"]]
    assert len(as_sequence(result["errors"], "capture errors")) == 1
    saved = _records(tmp_path)
    assert saved["broken"]["capture_error"]
    assert saved["broken"]["phase"] == "awaiting_reply"
    assert saved["valid"]["phase"] == "replied"
    assert str(broken["reply_nonce"]) not in bridge.output_requests()
    assert str(broken["reply_nonce"]) in bridge.output_requests(retry_failed=True)
    assert next(iter(chat.sent.values()))["thread"] == "spaces/test/threads/valid"
    status = bridge.status()
    assert any(as_mapping(item, "request").get("capture_error")
               for item in as_sequence(status["requests"], "requests"))


def test_truncated_scrollback_with_a_complete_reply_is_safe(tmp_path: Path) -> None:
    bridge, _, chat = setup(tmp_path)
    chat.message()
    bridge.tick()
    record = _records(tmp_path)["one"]
    result = bridge.capture_output(_snapshot(_block(record), truncated=True))
    assert result["captured"] == [record["key"]]
    assert as_mapping(_records(tmp_path)["one"]["reply_capture"], "capture")["snapshot_truncated"] is True


class _FakeStream(PaneOutputStream):
    def __init__(self, wait: Callable[[float], tuple[PaneOutputSnapshot | PaneAgentStatus, ...]]) -> None:
        self.wait_impl = wait
        self.waits: list[float] = []
        self.closes = 0

    def wait(self, timeout: float) -> tuple[PaneOutputSnapshot | PaneAgentStatus, ...]:
        self.waits.append(timeout)
        return self.wait_impl(timeout)

    def close(self) -> None:
        self.closes += 1


def test_capture_once_explicitly_retries_failed_request_and_closes_stream(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    bridge, _, chat = setup(tmp_path)
    chat.message()
    bridge.tick()
    record = _records(tmp_path)["one"]
    bridge.capture_output(_snapshot(f"</GCHAT_REPLY_{record['reply_nonce']}>"))
    assert not bridge.output_requests()
    stream = _FakeStream(lambda timeout: (_snapshot(_block(record, "Recovered")),))
    requested: list[tuple[str, ...]] = []

    def open_output(nonces: Sequence[str]) -> PaneOutputStream:
        requested.append(tuple(nonces))
        return stream

    monkeypatch.setattr(bridge, "open_output", open_output)
    result = bridge.capture_once()
    assert result["captured"] == [record["key"]]
    assert requested == [(str(record["reply_nonce"]),)]
    assert stream.waits == [0.25] and stream.closes == 1
    assert "capture_error" not in _records(tmp_path)["one"]


def test_capture_once_closes_stream_when_observer_fails(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    bridge, _, chat = setup(tmp_path)
    chat.message()
    bridge.tick()

    def fail(timeout: float) -> tuple[PaneOutputSnapshot, ...]:
        raise OutputStreamError("stream_eof", "gone")

    stream = _FakeStream(fail)
    monkeypatch.setattr(bridge, "open_output", lambda nonces: stream)
    with pytest.raises(OutputStreamError, match="stream_eof"):
        bridge.capture_once()
    assert stream.closes == 1


class _StopLoop(BaseException):
    pass


class _Clock:
    def __init__(self, *, allowed_sleeps: int = 0) -> None:
        self.now = 0.0
        self.sleeps: list[float] = []
        self.allowed_sleeps = allowed_sleeps

    def monotonic(self) -> float:
        return self.now

    def sleep(self, seconds: float) -> None:
        self.sleeps.append(seconds)
        if len(self.sleeps) > self.allowed_sleeps:
            raise _StopLoop()
        self.now += seconds


def _instrument(bridge: Bridge, harness: Harness, clock: _Clock,
                monkeypatch: pytest.MonkeyPatch) -> tuple[list[float], list[str]]:
    monkeypatch.setattr(chat_module, "time", clock)
    ticks: list[float] = []
    reads: list[str] = []
    original = bridge.tick

    def tick() -> dict[str, object]:
        ticks.append(clock.now)
        return original()

    def read(pane_id: str, *, source: str = "recent-unwrapped", lines: int | None = None) -> str:
        reads.append(pane_id)
        raise AssertionError("the event loop must not poll pane reads")

    monkeypatch.setattr(bridge, "tick", tick)
    monkeypatch.setattr(harness, "read", read)
    return ticks, reads


def test_daemon_rearms_output_without_polling_chat_or_reading_panes_each_second(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    bridge, harness, chat = setup(tmp_path)
    chat.message()
    clock = _Clock()
    ticks, reads = _instrument(bridge, harness, clock, monkeypatch)
    opens: list[float] = []
    waits = 0

    def wait(timeout: float) -> tuple[PaneOutputSnapshot, ...]:
        nonlocal waits
        waits += 1
        if waits == 3:
            raise _StopLoop()
        clock.now += timeout
        return ()

    stream = _FakeStream(wait)

    def open_output(nonces: Sequence[str]) -> PaneOutputStream:
        opens.append(clock.now)
        return stream

    monkeypatch.setattr(bridge, "open_output", open_output)
    with pytest.raises(_StopLoop):
        chat_module._run_bridge(bridge, 60, "test-chat")
    assert ticks == [0]
    assert opens == [0, 1, 2] and stream.waits == [1, 1, 1]
    assert not reads and len(harness.prompts) == 1
    assert stream.closes == 3


def test_daemon_sends_output_before_long_provider_poll_deadline(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    bridge, harness, chat = setup(tmp_path)
    chat.message()
    clock = _Clock()
    ticks, reads = _instrument(bridge, harness, clock, monkeypatch)
    waits = 0

    def wait(timeout: float) -> tuple[PaneOutputSnapshot, ...]:
        nonlocal waits
        waits += 1
        if waits == 2:
            assert timeout == 0.75
            raise _StopLoop()
        assert timeout == 1
        clock.now += 0.25
        return (_snapshot(_block(_records(tmp_path)["one"])),)

    stream = _FakeStream(wait)
    monkeypatch.setattr(bridge, "open_output", lambda nonces: stream)
    with pytest.raises(_StopLoop):
        chat_module._run_bridge(bridge, 60, "test-chat")
    assert ticks == [0] and clock.now == 0.25
    assert len(chat.sent) == 1 and not reads
    assert stream.closes == 1
    assert not clock.sleeps
    assert _read(tmp_path / "output.json")["state"] == "connected"
    assert bridge.output_requests()


def test_daemon_delivers_later_blocks_for_same_id_before_next_provider_poll(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    bridge, harness, chat = setup(tmp_path)
    chat.message()
    clock = _Clock()
    ticks, reads = _instrument(bridge, harness, clock, monkeypatch)
    opens: list[float] = []
    snapshots_at: list[float] = []
    streams: list[_FakeStream] = []
    bodies = ["Started", "Milestone reached", "Finished"]

    def open_output(nonces: Sequence[str]) -> PaneOutputStream:
        record = _records(tmp_path)["one"]
        assert tuple(nonces) == (record["reply_nonce"],)
        opens.append(clock.now)
        retained = "\n".join(_block(record, body) for body in bodies[:len(opens)])
        delivered = False

        def wait(timeout: float) -> tuple[PaneOutputSnapshot, ...]:
            nonlocal delivered
            if len(chat.sent) == len(bodies):
                raise _StopLoop()
            if not delivered:
                delivered = True
                clock.now += 0.1
                snapshots_at.append(clock.now)
                return (_snapshot(retained),)
            # A retained closing line can keep the output predicate true. The
            # next subscription must capture the expanded output despite that.
            clock.now += timeout
            return ()

        stream = _FakeStream(wait)
        streams.append(stream)
        return stream

    monkeypatch.setattr(bridge, "open_output", open_output)
    with pytest.raises(_StopLoop):
        chat_module._run_bridge(bridge, 60, "test-chat")
    assert ticks == [0] and opens == [0, 1, 2]
    assert snapshots_at == pytest.approx([0.1, 1.1, 2.1])
    assert [call["text"] for call in chat.calls if call["action"] == "send"] == [
        "[test-agent] " + body for body in bodies
    ]
    assert len(chat.sent) == 3 and len(harness.prompts) == 1 and not reads
    assert [stream.closes for stream in streams] == [1, 1, 1]


def test_daemon_eof_backs_off_then_reconnects_without_reprompt(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    bridge, harness, chat = setup(tmp_path)
    chat.message()
    clock = _Clock(allowed_sleeps=1)
    ticks, reads = _instrument(bridge, harness, clock, monkeypatch)
    opens: list[float] = []
    recovered_waits = 0

    def disconnected(timeout: float) -> tuple[PaneOutputSnapshot, ...]:
        clock.now += 0.25
        raise OutputStreamError("stream_eof", "server restarted")

    def recovered(timeout: float) -> tuple[PaneOutputSnapshot, ...]:
        nonlocal recovered_waits
        recovered_waits += 1
        if recovered_waits == 2:
            raise _StopLoop()
        clock.now += 0.25
        return (_snapshot(_block(_records(tmp_path)["one"])),)

    streams = [_FakeStream(disconnected), _FakeStream(recovered)]

    def open_output(nonces: Sequence[str]) -> PaneOutputStream:
        opens.append(clock.now)
        return streams[len(opens) - 1]

    monkeypatch.setattr(bridge, "open_output", open_output)
    with pytest.raises(_StopLoop):
        chat_module._run_bridge(bridge, 60, "test-chat")
    assert opens == [0, 1.25] and ticks == [0]
    assert clock.now == 1.5 and clock.sleeps == [1]
    assert [stream.closes for stream in streams] == [1, 1]
    assert not reads and len(harness.prompts) == 1 and len(chat.sent) == 1


def test_daemon_failed_connections_use_bounded_exponential_backoff(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    bridge, harness, chat = setup(tmp_path)
    chat.message()
    clock = _Clock(allowed_sleeps=2)
    _instrument(bridge, harness, clock, monkeypatch)
    opens: list[float] = []

    def stopped(timeout: float) -> tuple[PaneOutputSnapshot, ...]:
        raise _StopLoop()

    stream = _FakeStream(stopped)

    def open_output(nonces: Sequence[str]) -> PaneOutputStream:
        opens.append(clock.now)
        if len(opens) < 3:
            raise OSError("socket unavailable")
        return stream

    monkeypatch.setattr(bridge, "open_output", open_output)
    with pytest.raises(_StopLoop):
        chat_module._run_bridge(bridge, 60, "test-chat")
    assert opens == [0, 1, 3] and clock.sleeps == [1, 2]
    assert len(harness.prompts) == 1 and stream.closes == 1


def test_daemon_rebuilds_subscription_after_bad_capture_to_avoid_starving_pending_reply(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    bridge, harness, chat = setup(tmp_path)
    chat.message("broken")
    chat.message("valid")
    clock = _Clock()
    ticks, reads = _instrument(bridge, harness, clock, monkeypatch)
    subscriptions: list[tuple[str, ...]] = []
    complete_waits = 0

    def clipped(timeout: float) -> tuple[PaneOutputSnapshot, ...]:
        clock.now += 0.1
        nonce = _records(tmp_path)["broken"]["reply_nonce"]
        return (_snapshot(f"Only the end remains\n</GCHAT_REPLY_{nonce}>", truncated=True),)

    def complete(timeout: float) -> tuple[PaneOutputSnapshot, ...]:
        nonlocal complete_waits
        complete_waits += 1
        if complete_waits > 1:
            clock.now += timeout
            return ()
        clock.now += 0.1
        return (_snapshot(_block(_records(tmp_path)["valid"])),)

    def status_only(timeout: float) -> tuple[PaneOutputSnapshot | PaneAgentStatus, ...]:
        raise _StopLoop()

    streams = [_FakeStream(clipped), _FakeStream(complete), _FakeStream(status_only)]

    def open_output(nonces: Sequence[str]) -> PaneOutputStream:
        subscriptions.append(tuple(nonces))
        return streams[len(subscriptions) - 1]

    monkeypatch.setattr(bridge, "open_output", open_output)
    with pytest.raises(_StopLoop):
        chat_module._run_bridge(bridge, 60, "test-chat")
    records = _records(tmp_path)
    assert set(subscriptions[0]) == {records["broken"]["reply_nonce"], records["valid"]["reply_nonce"]}
    assert subscriptions[1] == (records["valid"]["reply_nonce"],)
    assert subscriptions[2] == subscriptions[1]
    assert [stream.closes for stream in streams] == [1, 1, 1]
    assert records["broken"]["capture_error"] and records["valid"]["phase"] == "replied"
    assert ticks == [0] and not reads
    assert len(chat.sent) == 1 and len(harness.prompts) == 2


def test_early_fenced_close_recovers_on_idle_and_continues_watching_later_replies(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    bridge, harness, chat = setup(tmp_path)
    chat.message()
    clock = _Clock()
    ticks, _ = _instrument(bridge, harness, clock, monkeypatch)
    subscriptions: list[tuple[str, ...]] = []
    reads: list[float] = []
    statuses = 0

    def quoted_example(timeout: float) -> tuple[PaneOutputSnapshot | PaneAgentStatus, ...]:
        clock.now += 0.1
        harness.state = "working"
        return (_snapshot("```text\n" + _block(_records(tmp_path)["one"], "Example only") + "\n```"),)

    def settled(timeout: float) -> tuple[PaneOutputSnapshot | PaneAgentStatus, ...]:
        nonlocal statuses
        statuses += 1
        clock.now += 0.1
        # A queued stale idle hint must not read a still-working target.
        if statuses == 2:
            harness.state = "idle"
        return (PaneAgentStatus("w1:p1", "idle"),)

    def read(pane_id: str, *, source: str = "recent-unwrapped", lines: int | None = None) -> str:
        assert pane_id == "w1:p1" and source == "recent-unwrapped" and lines == 4000
        reads.append(clock.now)
        record = _records(tmp_path)["one"]
        return "```text\n" + _block(record, "Example only") + "\n```\n" + _block(record, "Actual final answer")

    def stopped(timeout: float) -> tuple[PaneOutputSnapshot | PaneAgentStatus, ...]:
        raise _StopLoop()

    streams = [_FakeStream(quoted_example), _FakeStream(settled), _FakeStream(stopped)]

    def open_output(nonces: Sequence[str]) -> PaneOutputStream:
        subscriptions.append(tuple(nonces))
        return streams[len(subscriptions) - 1]

    monkeypatch.setattr(bridge, "open_output", open_output)
    monkeypatch.setattr(harness, "read", read)
    with pytest.raises(_StopLoop):
        chat_module._run_bridge(bridge, 60, "test-chat")
    assert len(subscriptions[0]) == 1 and subscriptions[1] == ()
    assert subscriptions[2] == subscriptions[0]
    assert [stream.closes for stream in streams] == [1, 1, 1]
    assert len(reads) == 1 and reads[0] == pytest.approx(0.3)
    assert ticks == [0] and len(harness.prompts) == 1
    assert next(iter(chat.sent.values()))["text"] == "[test-agent] Actual final answer"
    saved = _records(tmp_path)["one"]
    assert "capture_error" not in saved and saved["phase"] == "replied"
    assert as_mapping(saved["reply_capture"], "capture")["snapshot_truncated"] is None


def test_already_idle_bad_capture_rearms_watch_without_automatic_pane_rereads(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    bridge, harness, chat = setup(tmp_path)
    chat.message()
    clock = _Clock()
    ticks, _ = _instrument(bridge, harness, clock, monkeypatch)
    subscriptions: list[tuple[str, ...]] = []
    reads: list[float] = []
    statuses = 0

    def clipped_text() -> str:
        return f"Missing opening\n</GCHAT_REPLY_{_records(tmp_path)['one']['reply_nonce']}>"

    def clipped(timeout: float) -> tuple[PaneOutputSnapshot | PaneAgentStatus, ...]:
        return (_snapshot(clipped_text(), truncated=True),)

    def idle_then_wait(timeout: float) -> tuple[PaneOutputSnapshot | PaneAgentStatus, ...]:
        nonlocal statuses
        statuses += 1
        if statuses == 1:
            return (PaneAgentStatus("w1:p1", "idle"),)
        if statuses == 2:
            clock.now += timeout
            return ()
        raise _StopLoop()

    def read(pane_id: str, *, source: str = "recent-unwrapped", lines: int | None = None) -> str:
        reads.append(clock.now)
        return clipped_text()

    def stopped(timeout: float) -> tuple[PaneOutputSnapshot | PaneAgentStatus, ...]:
        raise _StopLoop()

    streams = [_FakeStream(clipped), _FakeStream(idle_then_wait), _FakeStream(stopped)]

    def open_output(nonces: Sequence[str]) -> PaneOutputStream:
        subscriptions.append(tuple(nonces))
        return streams[len(subscriptions) - 1]

    monkeypatch.setattr(bridge, "open_output", open_output)
    monkeypatch.setattr(harness, "read", read)
    with pytest.raises(_StopLoop):
        chat_module._run_bridge(bridge, 60, "test-chat")
    assert len(subscriptions) == 3 and subscriptions[1:] == [(), ()]
    assert reads == [0] and ticks == [0]
    assert [stream.closes for stream in streams] == [1, 1, 1]
    assert len(harness.prompts) == 1 and not chat.sent
    assert _records(tmp_path)["one"]["capture_error"]


def test_settled_hint_from_wrong_pane_does_not_read_or_send(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch,
) -> None:
    bridge, harness, chat = setup(tmp_path)
    chat.message()
    bridge.tick()
    reads: list[str] = []

    def read(pane_id: str, *, source: str = "recent-unwrapped", lines: int | None = None) -> str:
        reads.append(pane_id)
        return _block(_records(tmp_path)["one"])

    monkeypatch.setattr(harness, "read", read)
    before = len(chat.calls)
    with pytest.raises(HerdrUnavailable, match="different coordinator pane"):
        bridge.capture_event(PaneAgentStatus("w9:p1", "idle"))
    assert not reads and len(chat.calls) == before
