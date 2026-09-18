"""Push intake with independent acknowledgement, harness, and reply workers.

Only the event-loop owner mutates request records. Blocking network operations
return results to that owner; the harness worker owns only the existing durable
delivery queue. A saved stream cursor follows durable intake, never a pipe read.
"""

from __future__ import annotations

import hashlib
import queue
import re
import sys
import threading
import time
from collections.abc import Callable
from concurrent.futures import ThreadPoolExecutor
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path

from agentctl.agent import _fsync_dir, drain, enqueue, resolve_target
from agentctl.chat import Bridge, _private, _read, _timestamp, _utc, _write
from agentctl.chat_input import EventCommandStream
from agentctl.chat_output import PaneAgentStatus, PaneOutputSnapshot, PaneOutputStream
from agentctl.errors import HerdrUnavailable
from agentctl.jsonx import as_mapping, get_str


@dataclass(frozen=True)
class _Notice:
    kind: str
    value: object = None


@dataclass(frozen=True)
class _Completion:
    lane: str
    key: str
    result: object
    error: Exception | None
    finished_at: str


def _post(events: queue.Queue[_Notice], notice: _Notice, stop: threading.Event) -> None:
    while not stop.is_set():
        try:
            events.put(notice, timeout=0.1)
            return
        except queue.Full:
            continue


class _InputPump:
    def __init__(self, bridge: Bridge, events: queue.Queue[_Notice], stop: threading.Event) -> None:
        self.bridge, self.events, self.stop = bridge, events, stop
        self.stream: EventCommandStream | None = None
        self.thread = threading.Thread(target=self._run, name="chat-input", daemon=True)

    def _run(self) -> None:
        delay = 1.0
        while not self.stop.is_set():
            try:
                path = self.bridge.state / "input.json"
                cursor = _read(path).get("cursor") if path.exists() else None
                if cursor is not None and (not isinstance(cursor, str) or not cursor):
                    raise ValueError("saved stream cursor must be a nonempty string or null")
                self.stream = EventCommandStream(self.bridge.config.event_command,
                    {"action": "subscribe", "space": self.bridge.config.space, "cursor": cursor})
                _post(self.events, _Notice("input_connected"), self.stop)
                connected = time.monotonic()
                while not self.stop.is_set():
                    for event in self.stream.wait(30):
                        _post(self.events, _Notice("input", event), self.stop)
                    if time.monotonic() - connected >= 30:
                        delay = 1.0
            except Exception as exc:
                _post(self.events, _Notice("input_error", str(exc)), self.stop)
            finally:
                if self.stream is not None:
                    self.stream.close()
                    self.stream = None
            self.stop.wait(delay)
            delay = min(60, delay * 2)

    def close(self) -> None:
        stream = self.stream
        if stream is not None:
            stream.wake()
        self.thread.join(timeout=10)


class _OutputPump:
    def __init__(self, bridge: Bridge, events: queue.Queue[_Notice], stop: threading.Event) -> None:
        self.bridge, self.events, self.stop = bridge, events, stop
        self.desired: tuple[tuple[str, ...], tuple[str, ...]] = ((), ())
        self.changed = threading.Event()
        self.stream: PaneOutputStream | None = None
        self.thread = threading.Thread(target=self._run, name="chat-output", daemon=True)

    def update(self, desired: tuple[tuple[str, ...], tuple[str, ...]]) -> None:
        if desired != self.desired:
            self.desired = desired
            self.changed.set()
            stream = self.stream
            if stream is not None:
                stream.wake()

    def _run(self) -> None:
        subscribed: tuple[tuple[str, ...], tuple[str, ...]] | None = None
        delay = 1.0
        try:
            while not self.stop.is_set():
                self.changed.clear()
                desired = self.desired
                if desired != subscribed:
                    if self.stream is not None:
                        self.stream.close()
                    self.stream = None
                    subscribed = desired
                    delay = 1.0
                if not desired[0]:
                    self.changed.wait(30)
                    continue
                try:
                    if self.stream is None:
                        self.stream = self.bridge.open_output(desired[1])
                        _post(self.events, _Notice("output_connected"), self.stop)
                    if self.stop.is_set() or self.changed.is_set():
                        continue
                    for event in self.stream.wait(30):
                        _post(self.events, _Notice("output", event), self.stop)
                    delay = 1.0
                except Exception as exc:
                    _post(self.events, _Notice("output_error", str(exc)), self.stop)
                    if self.stream is not None:
                        self.stream.close()
                    self.stream = None
                    self.changed.wait(delay)
                    delay = min(60, delay * 2)
        finally:
            if self.stream is not None:
                self.stream.close()
                self.stream = None

    def close(self) -> None:
        self.changed.set()
        stream = self.stream
        if stream is not None:
            stream.wake()
        self.thread.join(timeout=10)


class _Runtime:
    def __init__(self, bridge: Bridge, reconcile_interval: float, prog: str,
                 stop: threading.Event) -> None:
        self.bridge, self.prog, self.stop = bridge, prog, stop
        self.reconcile_interval = reconcile_interval
        self.events: queue.Queue[_Notice] = queue.Queue(maxsize=64)
        self.input = _InputPump(bridge, self.events, stop)
        self.output = _OutputPump(bridge, self.events, stop)
        self.pools = {"ack": ThreadPoolExecutor(4, "chat-ack"),
                      "send": ThreadPoolExecutor(2, "chat-send"),
                      "drain": ThreadPoolExecutor(1, "chat-delivery"),
                      "output": ThreadPoolExecutor(2, "chat-inspect"),
                      "poll": ThreadPoolExecutor(1, "chat-reconcile")}
        self.jobs: set[tuple[str, str]] = set()
        self.retry: dict[tuple[str, str], tuple[float, float]] = {}
        self.next_poll = 0.0
        self.next_drain = 0.0
        self.poll_checkpoint: dict[str, object] | None = None
        self.reconcile_requested = True
        self.output_pending: dict[str, PaneOutputSnapshot | PaneAgentStatus] = {}
        self.output_inflight: dict[str, PaneOutputSnapshot | PaneAgentStatus] = {}
        self.input_path = bridge.state / "input.json"
        self.input_state: dict[str, object] = _read(self.input_path) if self.input_path.exists() else {"cursor": None}
        self.deferred = bridge.state / "deferred"
        _private(self.deferred)

    def _start(self, lane: str, key: str, operation: Callable[[], object]) -> None:
        self.jobs.add((lane, key))
        def execute() -> None:
            result: object = None
            error: Exception | None = None
            try:
                result = operation()
            except Exception as exc:
                error = exc
            _post(self.events, _Notice("complete", _Completion(lane, key, result, error, _utc())), self.stop)
        # Always post from the worker. add_done_callback may run immediately on
        # this owner thread, deadlocking it when the bounded mailbox is full.
        self.pools[lane].submit(execute)

    def _transport(self, lane: str, key: str, request: dict[str, object]) -> None:
        def operation() -> object:
            if lane == "send":
                resolve_target(self.bridge.client, self.bridge.config.target)
            return self.bridge.transport(request)
        self._start(lane, key, operation)

    def _input_status(self, **fields: object) -> None:
        self.input_state.update(fields, updated_at=_utc())
        _write(self.input_path, self.input_state)

    def _log(self, message: str) -> None:
        print(f"{self.prog}: {message}", file=sys.stderr, flush=True)

    def _records(self) -> list[tuple[Path, dict[str, object]]]:
        records = [(p, _read(p)) for p in (self.bridge.state / "requests").glob("*.json")]
        return sorted(records, key=lambda item: get_str(item[1], "received_at", "request"))

    def _outbound_text(self, key: str) -> str:
        text = get_str(_read(self.bridge.state / "replies" / f"{key}.json"), "text", "reply")
        prefix = f"[{self.bridge.config.agent_label}]"
        return text if text.startswith(prefix) else f"{prefix} {text}"

    def _possible_echo(self, message: dict[str, object]) -> bool:
        if message.get("sender") not in self.bridge.config.allowed_senders:
            return False
        for _, record in self._records():
            if record.get("phase") == "reply_pending":
                key = get_str(record, "key", "request")
                if message.get("text") == self._outbound_text(key):
                    return True
        return False

    def _accept_message(self, message: dict[str, object]) -> Path | None:
        identifier = get_str(message, "id", "stream message")
        if re.fullmatch(re.escape(self.bridge.config.space) + r"/messages/[A-Za-z0-9_.-]+", identifier) is None:
            raise ValueError("stream message belongs to another space")
        key = hashlib.sha256(identifier.encode()).hexdigest()
        accepted: Path | None = None
        if self._possible_echo(message):
            # Deferral also commits input before its stream cursor advances.
            # Require the same normalized schema as ordinary accepted messages.
            self.bridge._validate_message_source(message)
            self.bridge._validate_message_content(message)
            _write(self.deferred / f"{key}.json", message)
        else:
            path = self.bridge.state / "requests" / f"{key}.json"
            existed = path.exists()
            self.bridge._ingest_result({"messages": [message]})
            if not existed and path.exists():
                accepted = path
        self.next_drain = 0.0
        return accepted

    def _input_event(self, event: dict[str, object]) -> None:
        kind = event.get("type")
        cursor = event.get("cursor")
        if cursor is not None and (not isinstance(cursor, str) or not cursor or len(cursor) > 8192):
            raise ValueError("stream cursor must be a nonempty string of at most 8192 characters")
        if kind == "message":
            path = self._accept_message(as_mapping(event.get("message"), "stream message"))
            if path is not None:
                # The request is durable. Start its independent workers before
                # cursor persistence can stall; replay retains this request and
                # the delivery queue's existing identities after a crash.
                now = time.monotonic()
                pending, prompt = self._stage_request(path, _read(path), now)
                self._start_delivery(pending, [] if prompt is None else [prompt], now)
        elif kind == "gap":
            self.reconcile_requested = True
        elif kind not in ("heartbeat", "checkpoint"):
            raise ValueError("stream event type must be message, heartbeat, gap, or checkpoint")
        # Any message is durable (or excluded by configured authority) before its cursor advances.
        fields: dict[str, object] = {"state": "connected", "error": None, "last_event_at": _utc()}
        if cursor is not None:
            fields["cursor"] = cursor
        if kind == "message":
            fields["last_message_at"] = _utc()
        if kind == "gap":
            fields["last_gap_at"] = _utc()
        self._input_status(**fields)

    def _stage_request(self, path: Path, record: dict[str, object],
                       now: float) -> tuple[bool, tuple[str, str] | None]:
        queue_root = self.bridge.state / "queue"
        pending = False
        prompt: tuple[str, str] | None = None
        key = get_str(record, "key", "request")
        queue_id = get_str(record, "queue_id", "request")
        changed = False
        if record["phase"] == "received":
            if any((queue_root / phase / f"{queue_id}.json").exists()
                   for phase in ("inbox", "inflight", "processed", "failed")):
                record.update(phase="queued", queued_at=_utc())
                changed = True
            else:
                prompt = (queue_id, self.bridge._prompt(record))
                pending = True
        if record["phase"] == "queued":
            if (queue_root / "processed" / f"{queue_id}.json").exists():
                record.update(phase="awaiting_reply", delivery_confirmed_at=_utc())
                changed = True
            elif (queue_root / "failed" / f"{queue_id}.json").exists():
                record["phase"] = "delivery_uncertain"
                changed = True
            else:
                pending = True
        if "ack" not in record:
            source = as_mapping(record["message"], "source")
            record["ack"] = self.bridge._ack_record(get_str(source, "id", "source"),
                                                    completed_legacy=record["phase"] == "replied")
            changed = True
        if changed:
            _write(path, record)
        ack = as_mapping(record["ack"], "ack")
        retry = ack.get("next_retry_at")
        due = not isinstance(retry, str) or _timestamp(retry) <= datetime.now(timezone.utc)
        if (ack["state"] == "pending" and due and ("ack", key) not in self.jobs
                and sum(lane == "ack" for lane, _ in self.jobs) < 4):
            self._ack_start(path, record)
        if (record["phase"] in ("awaiting_reply", "delivery_uncertain", "reply_pending")
                and (self.bridge.state / "replies" / f"{key}.json").exists()
                and ("send", key) not in self.jobs
                and now >= self.retry.get(("send", key), (0, 1))[0]
                and sum(lane == "send" for lane, _ in self.jobs) < 2):
            # Reload after the ACK attempt's record update.
            self._send_start(path, _read(path))
        return pending, prompt

    def _start_delivery(self, pending: bool, to_enqueue: list[tuple[str, str]], now: float) -> None:
        if pending and ("drain", "queue") not in self.jobs and now >= self.next_drain:
            self.next_drain = now + 30
            # enqueue shares the delivery lock with drain. Both belong in the
            # harness lane: a slow prompt must never hold up durable intake/ACK.
            def deliver() -> object:
                queue_root = self.bridge.state / "queue"
                for identifier, prompt in to_enqueue:
                    enqueue(str(queue_root), prompt, message_id=identifier)
                return drain(self.bridge.client, self.bridge.config.target,
                             str(queue_root), ready_timeout=0)
            self._start("drain", "queue", deliver)

    def _stage(self) -> None:
        now = time.monotonic()
        pending = False
        to_enqueue: list[tuple[str, str]] = []
        for path, record in self._records():
            waiting, prompt = self._stage_request(path, record, now)
            pending = pending or waiting
            if prompt is not None:
                to_enqueue.append(prompt)
        self._start_delivery(pending, to_enqueue, now)
        if ((self.reconcile_requested or now >= self.next_poll) and ("poll", "page") not in self.jobs
                and now >= self.retry.get(("poll", "page"), (0, 1))[0]):
            self.poll_checkpoint = _read(self.bridge.state / "bridge.json")
            self.reconcile_requested = False
            self._transport("poll", "page", {"action": "poll", "space": self.bridge.config.space,
                "after": self.poll_checkpoint["after"], "cursor": self.poll_checkpoint.get("cursor")})
        desired = (tuple(self.bridge.output_requests(retry_failed=True)), tuple(self.bridge.output_requests()))
        self.output.update(desired)
        for key, event in tuple(self.output_pending.items()):
            if ("output", key) not in self.jobs and now >= self.retry.get(("output", key), (0, 1))[0]:
                del self.output_pending[key]
                self.output_inflight[key] = event
                def inspect(event: PaneOutputSnapshot | PaneAgentStatus = event) -> object:
                    return self._inspect_output(event)
                self._start("output", key, inspect)
        if not desired[0]:
            path = self.bridge.state / "output.json"
            if not path.exists() or _read(path).get("state") != "idle":
                _write(path, {"state": "idle", "error": None, "updated_at": _utc()})
        for path in sorted(self.deferred.glob("*.json")):
            message = _read(path)
            if not self._possible_echo(message):
                self.bridge._ingest_result({"messages": [message]})
                path.unlink()
                _fsync_dir(str(self.deferred))
                self.next_drain = 0.0

    def _inspect_output(self, event: PaneOutputSnapshot | PaneAgentStatus) -> PaneOutputSnapshot | None:
        info = resolve_target(self.bridge.client, self.bridge.config.target)
        if info.pane_id != event.pane_id:
            raise HerdrUnavailable("chat output event belongs to a different coordinator pane")
        if isinstance(event, PaneOutputSnapshot):
            return event
        if info.status not in ("idle", "done"):
            return None
        text = self.bridge.client.read(info.pane_id, source="recent-unwrapped", lines=4000)
        if len(text.encode("utf-8")) > 2 * 1024 * 1024:
            raise ValueError("retained chat output exceeds the 2 MiB capture limit")
        return PaneOutputSnapshot(info.pane_id, text, None)

    def _ack_start(self, path: Path, record: dict[str, object]) -> None:
        ack = as_mapping(record["ack"], "ack")
        attempts = ack.get("attempts", 0)
        if type(attempts) is not int or attempts < 0:
            raise ValueError("ack attempts must be a nonnegative integer")
        attempts += 1
        now = datetime.now(timezone.utc)
        ack.update(attempts=attempts, last_attempt_at=now.isoformat().replace("+00:00", "Z"),
                   next_retry_at=(now + timedelta(seconds=min(60, 3 * 2 ** min(attempts - 1, 5))))
                   .isoformat().replace("+00:00", "Z"))
        record["ack"] = ack
        _write(path, record)
        source = as_mapping(record["message"], "source")
        self._transport("ack", get_str(record, "key", "request"), {"action": "react",
            "space": self.bridge.config.space, "message": source["id"], "emoji": ack["emoji"],
            "request_id": ack["request_id"], "attempt": attempts})

    def _send_start(self, path: Path, record: dict[str, object]) -> None:
        if record["phase"] == "delivery_uncertain":
            record["delivery_confirmed_by"] = "reply_artifact"
        record["phase"] = "reply_pending"
        _write(path, record)
        source = as_mapping(record["message"], "source")
        key = get_str(record, "key", "request")
        self._transport("send", key, {"action": "send", "space": self.bridge.config.space,
            "thread": source["thread"], "request_id": record["request_id"], "text": self._outbound_text(key)})

    def _complete(self, item: _Completion) -> None:
        job = (item.lane, item.key)
        self.jobs.discard(job)
        error = item.error
        if item.lane in ("ack", "send"):
            path = self.bridge.state / "requests" / f"{item.key}.json"
            record = _read(path)
            identifier: str | None = None
            if error is None:
                try:
                    identifier = get_str(as_mapping(item.result, "transport result"), "id", "transport result")
                    source = as_mapping(record["message"], "source")
                    prefix = str(source["id"]) + "/reactions/" if item.lane == "ack" else self.bridge.config.space + "/messages/"
                    if re.fullmatch(re.escape(prefix) + r"[A-Za-z0-9_.-]+", identifier) is None:
                        raise ValueError("transport result belongs outside the requested resource")
                except (ValueError, TypeError) as exc:
                    error = exc
            if item.lane == "ack":
                ack = as_mapping(record["ack"], "ack")
                if error is None:
                    ack.update(state="acked", reaction_id=identifier, acked_at=item.finished_at,
                               error=None, next_retry_at=None)
                else:
                    ack["error"] = str(error)[:2000]
                record["ack"] = ack
            elif error is None:
                record.update(phase="replied", reply_id=identifier, replied_at=item.finished_at)
                record.pop("reply_error", None)
            else:
                record["reply_error"] = str(error)[:2000]
            _write(path, record)
        elif item.lane == "output":
            event = self.output_inflight.pop(item.key)
            if error is None:
                if item.result is not None:
                    if not isinstance(item.result, PaneOutputSnapshot):
                        raise ValueError("invalid inspected output")
                    result = self.bridge._capture_output(item.result, deliver=False, verify_target=False)
                    if result["errors"]:
                        self._log(f"output capture: {result['errors']}")
                if isinstance(event, PaneAgentStatus):
                    self.next_drain = 0.0
            else:
                self.output_pending.setdefault(item.key, event)
        elif item.lane == "poll" and error is None:
            try:
                result = as_mapping(item.result, "poll result")
                messages = result.get("messages")
                if not isinstance(messages, list):
                    raise ValueError("poll result messages must be a list")
                accepted: list[object] = []
                for raw in messages:
                    message = as_mapping(raw, "polled message")
                    if self._possible_echo(message):
                        self._accept_message(message)
                    else:
                        accepted.append(raw)
                result["messages"] = accepted
                self.bridge._ingest_result(result, self.poll_checkpoint)
                self.next_poll = time.monotonic() + (0 if result.get("cursor") else self.reconcile_interval)
                self.next_drain = 0.0
            except Exception as exc:
                error = exc
        if error is not None:
            self._log(f"{item.lane}: {error}")
            delay = min(60, self.retry.get(job, (0, 0.5))[1] * 2)
            self.retry[job] = (time.monotonic() + delay, delay)
            if item.lane == "poll":
                self.reconcile_requested = True
                self._input_status(reconcile_error=str(error)[:2000])
        else:
            self.retry.pop(job, None)
            if item.lane == "poll":
                self._input_status(reconcile_error=None, reconciled_at=item.finished_at)

    def _handle(self, notice: _Notice) -> None:
        if notice.kind == "input":
            self._input_event(as_mapping(notice.value, "stream event"))
        elif notice.kind == "input_connected":
            self._input_status(state="connected", error=None, connected_at=_utc())
            self.reconcile_requested = True
        elif notice.kind == "input_error":
            self._input_status(state="retrying", error=str(notice.value)[:2000])
            self._log(f"input subscription: {notice.value}")
        elif notice.kind == "output":
            if not isinstance(notice.value, (PaneOutputSnapshot, PaneAgentStatus)):
                raise ValueError("invalid output event")
            key = "snapshot" if isinstance(notice.value, PaneOutputSnapshot) else "settled"
            self.output_pending[key] = notice.value
        elif notice.kind in ("output_connected", "output_error"):
            _write(self.bridge.state / "output.json", {"state": "connected" if notice.kind == "output_connected" else "retrying",
                "error": notice.value, "updated_at": _utc()})
        elif notice.kind == "complete":
            if not isinstance(notice.value, _Completion):
                raise ValueError("invalid completion event")
            self._complete(notice.value)

    def run(self) -> None:
        resolve_target(self.bridge.client, self.bridge.config.target)
        self.input.thread.start()
        self.output.thread.start()
        try:
            while not self.stop.is_set():
                # Local state maintenance sees explicit file replies and retry deadlines.
                # Network intake and output events wake this wait immediately.
                self._stage()
                try:
                    notice = self.events.get(timeout=1)
                except queue.Empty:
                    continue
                self._handle(notice)
        finally:
            self.stop.set()
            self.input.close()
            self.output.close()
            for pool in self.pools.values():
                pool.shutdown(wait=True, cancel_futures=True)


def run_streaming(bridge: Bridge, *, reconcile_interval: float = 300,
                  prog: str = "agentctl chat", stop: threading.Event | None = None) -> None:
    """Consume push events while ACK, prompt, and reply operations run independently."""
    _Runtime(bridge, reconcile_interval, prog, stop or threading.Event()).run()
