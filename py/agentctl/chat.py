#!/usr/bin/env python3
"""Durable, single-coordinator chat bridge; transport and harness stay independent.

Run with ``python -m agentctl.chat``. A command adapter exchanges one JSON request
and response on stdin/stdout. The built-in Google Chat adapter uses the public API.
"""

from __future__ import annotations

import argparse
import fcntl
import hashlib
import json
import os
import re
import shlex
import signal
import subprocess
import sys
import time
import uuid
from collections.abc import Sequence
from dataclasses import asdict, dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Protocol
from urllib.parse import urlencode
from urllib.request import Request, urlopen

_PKG_PARENT = str(Path(__file__).resolve().parent.parent)
if _PKG_PARENT not in sys.path:
    sys.path.insert(0, _PKG_PARENT)

from agentctl import __version__
from agentctl.agent import (
    Target, _atomic_json, _open_private_lock, _read_queue_json,
    _validate_private_directory, drain, enqueue, resolve_target,
)
from agentctl.client import AgentPaneInfo, HerdrClient, Pane
from agentctl.errors import HerdrRunError, HerdrUnavailable
from agentctl.jsonx import as_mapping, as_sequence, get_str


class Transport(Protocol):
    """Bounded poll/send/react operations; send deduplicates request_id retries.

    React receives a stable request_id for adapter reconciliation. Adapters must
    ensure the requested reaction is present, never toggle an existing reaction.
    The public REST API has no reaction requestId or exactly-once guarantee.
    """

    def __call__(self, request: dict[str, object]) -> dict[str, object]: ...


def _utc() -> str:
    return datetime.now(timezone.utc).isoformat().replace("+00:00", "Z")


def _timestamp(value: str) -> datetime:
    result = datetime.fromisoformat(value.replace("Z", "+00:00"))
    if result.tzinfo is None:
        raise ValueError("chat timestamps must include a timezone")
    return result


def _private(directory: Path) -> None:
    directory.mkdir(mode=0o700, parents=True, exist_ok=True)
    _validate_private_directory(str(directory), "chat state directory", tighten=True)


def _read(path: Path) -> dict[str, object]:
    return as_mapping(_read_queue_json(str(path), "chat state", require_private=True), "chat state")


def _write(path: Path, document: dict[str, object]) -> None:
    _atomic_json(str(path), document)


def _strings(value: object, what: str) -> tuple[str, ...]:
    values = as_sequence(value, what)
    if not values or any(not isinstance(item, str) or not item for item in values):
        raise ValueError(f"{what} must be a nonempty list of nonempty strings")
    return tuple(str(item) for item in values)


def _reaction_emoji(value: object) -> str | None:
    if value is None or value == "":
        return None
    if (not isinstance(value, str) or not value.strip() or len(value.encode()) > 128
            or any(ord(character) < 32 or ord(character) == 127 for character in value)):
        raise ValueError("ack_reaction must be a Unicode emoji of at most 128 UTF-8 bytes, or null/empty to disable")
    return value


@dataclass(frozen=True)
class Config:
    """The authority boundary is one space, named senders, and one pinned target."""

    space: str
    allowed_senders: tuple[str, ...]
    target: Target
    agent_label: str
    transport_command: tuple[str, ...] = ()
    token_env: str = "HERDR_CHAT_TOKEN"
    token_command: tuple[str, ...] = ()
    agent_name: str | None = None
    ack_reaction: str | None = "🤖"
    reaction_user: str | None = None

    @classmethod
    def parse(cls, document: dict[str, object]) -> Config:
        """Validate explicit sender, target, and transport configuration."""
        if set(document) - set(cls.__dataclass_fields__):
            raise ValueError("unknown chat configuration field")
        space = get_str(document, "space", "chat config")
        if re.fullmatch(r"spaces/[A-Za-z0-9_-]+", space) is None:
            raise ValueError("space must be an exact spaces/ID resource")
        label = get_str(document, "agent_label", "chat config")
        if not label.strip() or "\n" in label or len(label) > 100:
            raise ValueError("agent_label must identify the replying agent on one line")
        values = as_mapping(document.get("target"), "chat target")
        fields: dict[str, str | None] = {}
        for key in Target.__dataclass_fields__:
            value = values.get(key)
            if value is not None and (not isinstance(value, str) or not value):
                raise ValueError(f"target.{key} must be a nonempty string")
            fields[key] = value if isinstance(value, str) else None
        if set(values) - set(fields):
            raise ValueError("unknown target identity assertion")
        target = Target(**fields)
        if not target.pane_id and not target.session_value:
            raise ValueError("chat target needs pane_id or session_value")
        if not target.expected_agent or not target.expected_cwd or not target.expected_workspace:
            raise ValueError("chat target requires expected_agent, expected_cwd, expected_workspace")
        command = document.get("transport_command")
        token_command = document.get("token_command")
        token_env = document.get("token_env", "HERDR_CHAT_TOKEN")
        if not isinstance(token_env, str) or not token_env:
            raise ValueError("token_env must name an environment variable")
        agent_name = document.get("agent_name")
        if agent_name is not None and (not isinstance(agent_name, str) or not agent_name.strip() or "\0" in agent_name):
            raise ValueError("agent_name must name a live Herdr agent")
        reaction_user = document.get("reaction_user")
        if reaction_user is not None and (
            not isinstance(reaction_user, str)
            or re.fullmatch(r"users/[A-Za-z0-9_-]+", reaction_user) is None
            or reaction_user in ("users/me", "users/app")
        ):
            raise ValueError("reaction_user must be the canonical users/ID of the OAuth user")
        return cls(space, _strings(document.get("allowed_senders"), "allowed_senders"), target,
                   label, () if command is None or command == [] else _strings(command, "transport_command"), token_env,
                   () if token_command is None or token_command == [] else _strings(token_command, "token_command"),
                   agent_name, _reaction_emoji(document.get("ack_reaction", "🤖")), reaction_user)


class _NamedClient(HerdrClient):
    """Revalidate a named coordinator at every readiness probe and submission."""

    def __init__(self, client: HerdrClient, name: str) -> None:
        self._delegate, self._name = client, name

    def pane_info(self, pane_id: str) -> AgentPaneInfo:
        if self._delegate.agent_pane(self._name) != pane_id:
            raise HerdrUnavailable(f"named coordinator {self._name!r} no longer owns pane {pane_id!r}")
        return self._delegate.pane_info(pane_id)

    def panes(self, workspace_id: str | None = None) -> tuple[Pane, ...]:
        pane_id = self._delegate.agent_pane(self._name)
        return tuple(pane for pane in self._delegate.panes(workspace_id) if pane.pane_id == pane_id)

    def workspace_label(self, workspace_id: str) -> str:
        return self._delegate.workspace_label(workspace_id)

    def prompt_agent(self, pane_id: str, command: str) -> None:
        self.pane_info(pane_id)
        self._delegate.prompt_agent(pane_id, command)

    def wait_agent_status(self, pane_id: str, status: str, timeout_ms: int) -> None:
        self.pane_info(pane_id)
        self._delegate.wait_agent_status(pane_id, status, timeout_ms)
        self.pane_info(pane_id)

    def read(self, pane_id: str, *, source: str = "recent-unwrapped", lines: int | None = None) -> str:
        self.pane_info(pane_id)
        return self._delegate.read(pane_id, source=source, lines=lines)


class CommandTransport:
    """Operator-selected adapter; arguments are literal, never evaluated by a shell."""

    def __init__(self, command: Sequence[str]) -> None:
        self.command = tuple(command)

    def __call__(self, request: dict[str, object]) -> dict[str, object]:
        result = _run_command(self.command, input_text=json.dumps(request), timeout=60)
        if result.returncode:
            raise ValueError(f"chat adapter exited {result.returncode}: {result.stderr[-2000:]}")
        return as_mapping(json.loads(result.stdout), "chat adapter response")


def _run_command(
    command: Sequence[str], *, timeout: float, input_text: str | None = None,
) -> subprocess.CompletedProcess[str]:
    """Bound adapters and credential helpers, including their child processes."""
    process = subprocess.Popen(
        command, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True, encoding="utf-8", start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(input=input_text, timeout=timeout)
    except subprocess.TimeoutExpired:
        # A helper can inherit captured pipes after its wrapper exits. Killing
        # the entire new group also bounds communicate() while draining them.
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.kill()
        process.communicate()
        raise
    return subprocess.CompletedProcess(command, process.returncode, stdout, stderr)


class GoogleChatTransport:
    """Public Google Chat REST transport with an operator-provided OAuth access token."""

    def __init__(self, token_env: str = "HERDR_CHAT_TOKEN", token_command: Sequence[str] = (),
                 reaction_user: str | None = None) -> None:
        self.token_env = token_env
        self.token_command = tuple(token_command)
        self.reaction_user = reaction_user

    @staticmethod
    def _http(token: str, path: str, params: dict[str, str], data: bytes | None = None) -> dict[str, object]:
        query = "?" + urlencode(params) if params else ""
        http = Request(f"https://chat.googleapis.com/v1/{path}{query}", data=data,
                       headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
        with urlopen(http, timeout=45) as response:
            return as_mapping(json.load(response), "Google Chat response")

    def _react(self, token: str, space: str, request: dict[str, object]) -> dict[str, object]:
        message = get_str(request, "message", "reaction request")
        if re.fullmatch(re.escape(space) + r"/messages/[A-Za-z0-9_.-]+", message) is None:
            raise ValueError("reaction message must belong to the configured space")
        uuid.UUID(get_str(request, "request_id", "reaction request"))
        emoji = _reaction_emoji(request.get("emoji"))
        if emoji is None:
            raise ValueError("reaction request requires a Unicode emoji")
        path = message + "/reactions"

        def identifier(document: dict[str, object]) -> str:
            name = get_str(document, "name", "Google Chat reaction")
            if re.fullmatch(re.escape(path) + r"/[A-Za-z0-9_.-]+", name) is None:
                raise ValueError("Google Chat returned a reaction outside the requested message")
            return name

        if self.reaction_user:
            # Neither users/me nor requestId is part of the documented reaction
            # contract. An explicit OAuth actor lets us reconcile a lost create
            # response without mistaking somebody else's emoji for our ACK.
            params = {"pageSize": "200", "filter":
                      f"emoji.unicode = {json.dumps(emoji, ensure_ascii=False)} AND user.name = {json.dumps(self.reaction_user)}"}
            document = self._http(token, path, params)
            for value in as_sequence(document.get("reactions", []), "Google Chat reactions"):
                reaction = as_mapping(value, "Google Chat reaction")
                user = as_mapping(reaction.get("user", {}), "reaction user")
                existing_emoji = as_mapping(reaction.get("emoji", {}), "reaction emoji")
                if user.get("name") == self.reaction_user and existing_emoji.get("unicode") == emoji:
                    return {"id": identifier(reaction)}
            if document.get("nextPageToken"):
                # A single actor/emoji should yield at most one result. Fail
                # visibly if that contract changes; never perform an unbounded
                # scan before delivering the user's prompt.
                raise ValueError("Google Chat reaction reconciliation unexpectedly requires pagination")
        document = self._http(token, path, {}, json.dumps({"emoji": {"unicode": emoji}}).encode())
        return {"id": identifier(document)}

    def __call__(self, request: dict[str, object]) -> dict[str, object]:
        token = os.environ.get(self.token_env)
        if self.token_command:
            completed = _run_command(self.token_command, timeout=45)
            if completed.returncode:
                raise ValueError(f"token command exited {completed.returncode}")
            token = completed.stdout.strip()
        if not token:
            raise ValueError(f"set {self.token_env} to a Google Chat OAuth access token")
        if any(not 0x21 <= ord(character) <= 0x7e for character in token):
            # The HTTP library's invalid-header exception includes the value.
            # Refuse malformed helper output without echoing credentials to logs.
            raise ValueError("OAuth access token must be ASCII without whitespace")
        space = get_str(request, "space", "chat request")
        if re.fullmatch(r"spaces/[A-Za-z0-9_-]+", space) is None:
            raise ValueError("invalid Google Chat space")
        action = request.get("action")
        if action == "react":
            return self._react(token, space, request)
        data: bytes | None = None
        params: dict[str, str] = {}
        if action == "poll":
            after = get_str(request, "after", "poll request")
            _timestamp(after)
            params.update(pageSize="100", orderBy="createTime asc", filter=f'createTime > "{after}"')
            cursor = request.get("cursor")
            if cursor is not None and not isinstance(cursor, str):
                raise ValueError("transport cursor must be a string or null")
            if cursor:
                params["pageToken"] = cursor
        elif action == "send":
            request_id = get_str(request, "request_id", "send request")
            uuid.UUID(request_id)
            reply_thread = get_str(request, "thread", "send request")
            if re.fullmatch(re.escape(space) + r"/threads/[A-Za-z0-9_.-]+", reply_thread) is None:
                raise ValueError("reply thread must belong to the configured space")
            params.update(requestId=request_id,
                          messageReplyOption="REPLY_MESSAGE_OR_FAIL")
            data = json.dumps({"text": get_str(request, "text", "send request"),
                               "thread": {"name": reply_thread}}).encode()
        else:
            raise ValueError("unknown chat transport action")
        document = self._http(token, space + "/messages", params, data)
        if action == "send":
            identifier = get_str(document, "name", "sent message")
            if not identifier.startswith(space + "/messages/"):
                raise ValueError("Google Chat returned a reply outside the configured space")
            return {"id": identifier}
        messages: list[dict[str, object]] = []
        for value in as_sequence(document.get("messages", []), "Google Chat messages"):
            message = as_mapping(value, "Google Chat message")
            sender = as_mapping(message.get("sender", {}), "Google Chat sender")
            thread = as_mapping(message.get("thread", {}), "Google Chat thread")
            messages.append({"id": message.get("name"), "text": message.get("text", ""),
                             "sender": sender.get("name"), "thread": thread.get("name"),
                             "created_at": message.get("createTime")})
        return {"messages": messages, "cursor": document.get("nextPageToken")}


class Bridge:
    """Restartable inbox, Herdr delivery queue, and idempotent threaded reply outbox."""

    def __init__(self, state: Path, client: HerdrClient | None = None,
                 transport: Transport | None = None) -> None:
        self.state = state.absolute()
        _private(self.state)
        self.config = Config.parse(as_mapping(_read(self.state / "bridge.json")["config"], "saved config"))
        selected_client = client or HerdrClient()
        self.client = _NamedClient(selected_client, self.config.agent_name) if self.config.agent_name else selected_client
        self.transport: Transport = transport or (CommandTransport(self.config.transport_command)
            if self.config.transport_command else GoogleChatTransport(
                self.config.token_env, self.config.token_command, self.config.reaction_user))
        for name in ("requests", "replies", "queue"):
            _private(self.state / name)

    @classmethod
    def initialize(cls, state: Path, config: Config, *, after: str | None = None) -> None:
        """Bind new private state to this configuration and a history cutoff."""
        _private(state)
        descriptor = _open_private_lock(str(state / ".bridge.lock"), "chat bridge lock")
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX)
            path = state / "bridge.json"
            if path.exists():
                raise ValueError("bridge already initialized; use a new state directory to change its authority")
            start = after or _utc()
            _timestamp(start)
            _write(path, {"version": 1, "config": asdict(config), "after": start,
                          "cursor": None, "high_water": start, "started_at": start})
        finally:
            os.close(descriptor)

    def _ingest(self, checkpoint: dict[str, object]) -> None:
        own_replies = {_read(path).get("reply_id")
                       for path in (self.state / "requests").glob("*.json")}
        result = self.transport({"action": "poll", "space": self.config.space,
                                 "after": checkpoint["after"], "cursor": checkpoint.get("cursor")})
        high = _timestamp(get_str(checkpoint, "high_water", "checkpoint"))
        start = _timestamp(get_str(checkpoint, "started_at", "checkpoint"))
        for value in as_sequence(result.get("messages"), "polled messages"):
            message = as_mapping(value, "polled message")
            identifier = get_str(message, "id", "message")
            if identifier in own_replies:
                continue
            thread = get_str(message, "thread", "message")
            if not identifier.startswith(self.config.space + "/messages/") or not thread.startswith(self.config.space + "/threads/"):
                raise ValueError("transport returned a message outside the configured space")
            created = _timestamp(get_str(message, "created_at", "message"))
            high = max(high, created)
            if created < start:
                continue
            if message.get("sender") not in self.config.allowed_senders or not message.get("text"):
                continue
            text = get_str(message, "text", "message")
            if len(text.encode()) > 32000:
                raise ValueError("chat message exceeds 32000 bytes")
            key = hashlib.sha256(identifier.encode()).hexdigest()
            path = self.state / "requests" / f"{key}.json"
            if not path.exists():
                _write(path, {"key": key, "message": message, "phase": "received",
                              "queue_id": f"{int(created.timestamp() * 1_000_000):020d}-{key}",
                              "request_id": str(uuid.uuid5(uuid.NAMESPACE_URL, identifier)),
                              "received_at": _utc(), "ack": self._ack_record(identifier)})
        cursor = result.get("cursor")
        if cursor is not None and not isinstance(cursor, str):
            raise ValueError("transport cursor must be a string or null")
        checkpoint.update(cursor=cursor or None, high_water=high.isoformat().replace("+00:00", "Z"))
        if not cursor:
            # Overlap the high-water timestamp: messages with equal timestamps and brief
            # indexing delays are deduplicated by their immutable resource names.
            checkpoint["after"] = (high - timedelta(seconds=60)).isoformat().replace("+00:00", "Z")
        _write(self.state / "bridge.json", checkpoint)

    def _ack_record(self, message: str, *, completed_legacy: bool = False) -> dict[str, object]:
        return {"state": "disabled" if self.config.ack_reaction is None or completed_legacy else "pending",
                "emoji": self.config.ack_reaction,
                "request_id": str(uuid.uuid5(uuid.NAMESPACE_URL, message + "#agentctl-ack")),
                "attempts": 0, "error": None, "next_retry_at": None,
                "last_attempt_at": None, "reaction_id": None, "acked_at": None}

    def _acknowledge(self) -> None:
        """Attempt due durable ACKs without coupling failure to agent execution."""
        for path in sorted((self.state / "requests").glob("*.json")):
            record = _read(path)
            message = as_mapping(record["message"], "source message")
            identifier = get_str(message, "id", "source message")
            if "ack" not in record:
                # Upgrade unfinished work, but do not decorate historical final
                # answers when an existing bridge first gains ACK support.
                record["ack"] = self._ack_record(identifier, completed_legacy=record.get("phase") == "replied")
                _write(path, record)
            ack = as_mapping(record["ack"], "request acknowledgement")
            if ack.get("state") != "pending":
                continue
            now = datetime.now(timezone.utc)
            retry_at = ack.get("next_retry_at")
            if isinstance(retry_at, str) and _timestamp(retry_at) > now:
                continue
            attempts = ack.get("attempts", 0)
            if not isinstance(attempts, int) or isinstance(attempts, bool) or attempts < 0:
                raise ValueError("ack attempts must be a nonnegative integer")
            attempts += 1
            retry = now + timedelta(seconds=min(60, 3 * 2 ** min(attempts - 1, 5)))
            ack.update(attempts=attempts, last_attempt_at=now.isoformat().replace("+00:00", "Z"),
                       next_retry_at=retry.isoformat().replace("+00:00", "Z"))
            record["ack"] = ack
            # Commit the attempt before contacting Chat. A crash or lost response
            # retries the same identity after the saved deadline.
            _write(path, record)
            try:
                result = self.transport({"action": "react", "space": self.config.space,
                    "message": identifier, "emoji": ack["emoji"], "request_id": ack["request_id"]})
                reaction = get_str(result, "id", "acknowledgement reaction")
                if re.fullmatch(re.escape(identifier) + r"/reactions/[A-Za-z0-9_.-]+", reaction) is None:
                    raise ValueError("transport returned a reaction outside the requested message")
            except (OSError, ValueError, TypeError, HerdrRunError, subprocess.SubprocessError) as exc:
                ack["error"] = str(exc)[:2000]
            else:
                ack.update(state="acked", reaction_id=reaction, acked_at=_utc(), error=None, next_retry_at=None)
            _write(path, record)

    def _prompt(self, record: dict[str, object]) -> str:
        message = as_mapping(record["message"], "message")
        key = get_str(record, "key", "request")
        reply = shlex.join([sys.executable, str(Path(__file__).resolve()), "reply", "--state", str(self.state),
                            "--request", key, "--file", "PATH_TO_YOUR_REPLY"])
        return ("A message from an authorized user arrived through your configured chat bridge.\n"
                f"Source: {message['id']}\nSender: {message['sender']}\n\n"
                f"{message['text']}\n\n"
                "Complete this user's request using your normal instructions and tools. Only you are the "
                "chat coordinator; manage other long-lived agents through agentctl as needed. "
                "To return your final user-facing answer to the originating chat thread, write it to a "
                "UTF-8 file and run this command, replacing PATH_TO_YOUR_REPLY with that file:\n"
                f"{reply}\nDo not send a separate chat message; the bridge delivers this reply durably.")

    def _deliver(self) -> None:
        queue = self.state / "queue"
        paths = sorted((self.state / "requests").glob("*.json"),
                       key=lambda path: get_str(_read(path), "received_at", "request"))
        for path in paths:
            record = _read(path)
            queue_id = get_str(record, "queue_id", "request")
            if record["phase"] == "received":
                if not any((queue / phase / f"{queue_id}.json").exists()
                           for phase in ("inbox", "inflight", "processed", "failed")):
                    enqueue(str(queue), self._prompt(record), message_id=queue_id)
                record["phase"] = "queued"
                _write(path, record)
        if any(record.get("phase") == "queued" for record in map(_read, paths)):
            # Zero readiness wait keeps chat polling responsive while the lead is busy.
            drain(self.client, self.config.target, str(queue), ready_timeout=0)
        for path in paths:
            record = _read(path)
            key = get_str(record, "key", "request")
            queue_id = get_str(record, "queue_id", "request")
            if record["phase"] == "queued":
                if (queue / "processed" / f"{queue_id}.json").exists():
                    record["phase"] = "awaiting_reply"
                elif (queue / "failed" / f"{queue_id}.json").exists():
                    record["phase"] = "delivery_uncertain"
                _write(path, record)
            reply_path = self.state / "replies" / f"{key}.json"
            if record["phase"] in ("awaiting_reply", "reply_pending", "delivery_uncertain") and reply_path.exists():
                if record["phase"] == "delivery_uncertain":
                    record["delivery_confirmed_by"] = "reply_artifact"
                reply = get_str(_read(reply_path), "text", "reply")
                record["phase"] = "reply_pending"
                _write(path, record)
                source = as_mapping(record["message"], "source message")
                prefix = f"[{self.config.agent_label}]"
                result = self.transport({"action": "send", "space": self.config.space,
                    "thread": source["thread"], "request_id": record["request_id"],
                    "text": reply if reply.startswith(prefix) else f"{prefix} {reply}"})
                reply_id = get_str(result, "id", "sent reply")
                if not reply_id.startswith(self.config.space + "/messages/"):
                    raise ValueError("transport returned a reply outside the configured space")
                record.update(phase="replied", reply_id=reply_id)
                _write(path, record)

    def tick(self) -> dict[str, object]:
        """Reconcile replies, poll a page, and attempt ready deliveries once."""
        descriptor = _open_private_lock(str(self.state / ".bridge.lock"), "chat bridge lock")
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            # Identity failures must precede external reads or sends.
            resolve_target(self.client, self.config.target)
            self._acknowledge()
            # Recover an accepted reply's lost acknowledgement before reading its
            # echo. This matters when the adapter posts as an allowlisted user.
            self._deliver()
            self._ingest(_read(self.state / "bridge.json"))
            self._acknowledge()
            self._deliver()
            return self.status()
        finally:
            os.close(descriptor)

    def status(self) -> dict[str, object]:
        """Return durable request phases without accessing Chat or the harness."""
        records = [_read(path) for path in sorted((self.state / "requests").glob("*.json"))]
        return {"space": self.config.space, "target": asdict(self.config.target),
                "agent_name": self.config.agent_name, "ack_reaction": self.config.ack_reaction,
                "requests": records}


def submit_reply(state: Path, key: str, text: str) -> None:
    """Commit one final answer locally; the bridge alone has transport credentials."""
    if re.fullmatch(r"[0-9a-f]{64}", key) is None:
        raise ValueError("invalid request key")
    if not text.strip() or len(text.encode()) > 30000:
        raise ValueError("reply must contain 1-30000 UTF-8 bytes")
    record = _read(state / "requests" / f"{key}.json")
    _private(state / "replies")
    path = state / "replies" / f"{key}.json"
    if path.exists():
        if _read(path).get("text") == text:
            return
        raise ValueError("a different reply already exists for this request")
    if record.get("phase") not in ("queued", "awaiting_reply", "reply_pending", "delivery_uncertain"):
        raise ValueError("request is not awaiting a reply")
    from agentctl.agent import _atomic_json_create
    try:
        _atomic_json_create(str(path), {"text": text})
    except FileExistsError:
        if _read(path).get("text") != text:
            raise ValueError("a different reply already exists for this request") from None


def run_cli(argv: Sequence[str] | None = None, *, prog: str = "agentctl chat",
         default_state: Path = Path(".agentctl/.chat")) -> int:
    """Initialize, inspect, run, or reply through a coordinator bridge."""
    parser = argparse.ArgumentParser(
        prog=prog,
        description=(
            "Message a Codex or Claude coordinator in Herdr from one Google Chat space.\n"
            "Authorized messages enter a durable queue and receive a configurable reaction ACK\n"
            "(default: 🤖). The coordinator explicitly submits each final threaded reply."
        ),
        epilog=f"""Examples:
  {prog} init --config chat.json
  {prog} run --interval 10
  {prog} status
  {prog} reply --request "$REQUEST_KEY" --file answer.txt

Use '{prog} quickstart' for setup, '{prog} userguide' for the full guide,
and '{prog} COMMAND --help' for command-specific options.
""",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        allow_abbrev=False,
    )
    parser.add_argument("--version", action="version", version=f"{prog} {__version__}",
                        help="print the installed version and exit")
    parser.add_argument("--userguide", action="store_true", help="print the complete guide and exit (alias for userguide)")
    parser.add_argument("--state", type=Path, default=default_state, metavar="DIR",
                        help=f"private bridge state directory (default: {default_state}); also accepted after the command")
    commands = parser.add_subparsers(dest="command", title="commands", metavar="COMMAND")
    descriptions = {
        "init": "Create private bridge state from configuration and check the pinned target.",
        "tick": "Poll one page, ACK accepted messages, deliver ready prompts, and post final replies.",
        "run": "Keep polling, retrying pending ACKs, delivering prompts, and posting replies until interrupted.",
        "status": "Print saved target, request phases, ACK attempts, and retry errors as JSON; no network or harness access.",
        "reply": "Durably queue one final answer from a UTF-8 file for the bridge to post in its originating thread.",
        "quickstart": "Print the shortest setup path and an example configuration.",
        "userguide": "Print the complete setup, authentication, adapter, and recovery guide.",
    }
    subparsers: dict[str, argparse.ArgumentParser] = {}
    examples = {"init": "--config chat.json", "tick": "", "run": "--interval 10", "status": "",
                "reply": '--request "$REQUEST_KEY" --file answer.txt', "quickstart": "", "userguide": ""}
    for name, description in descriptions.items():
        command = commands.add_parser(name, help=description, description=description,
            epilog=f"Example: {prog} {name} {examples[name]}".rstrip(), allow_abbrev=False)
        if name not in ("quickstart", "userguide"):
            command.add_argument("--state", type=Path, default=argparse.SUPPRESS, metavar="DIR",
                                 help=f"private bridge state directory (default: {default_state})")
        subparsers[name] = command
    subparsers["init"].add_argument("--config", type=Path, required=True, metavar="JSON_FILE",
        help="JSON configuration with the space, allowed senders, pinned target, and authentication")
    subparsers["init"].add_argument("--after", metavar="RFC3339_TIME",
        help="earliest message time, including timezone (default: current time; no history replay)")
    subparsers["run"].add_argument("--interval", type=float, default=3, metavar="SECONDS",
        help="polling interval in seconds, 0.1–60 (default: 3); failures back off to 60 seconds")
    subparsers["reply"].add_argument("--request", required=True, metavar="KEY",
        help="64-character hexadecimal request key supplied in the coordinator's reply instructions")
    subparsers["reply"].add_argument("--file", type=Path, required=True, metavar="UTF8_FILE",
        help="file containing the final answer, 1–30000 UTF-8 bytes; identical retries are safe")
    args = parser.parse_args(argv)
    if args.userguide or args.command == "userguide":
        from importlib.resources import files
        sys.stdout.write((files("agentctl") / "CHAT_USER_GUIDE.md").read_text(encoding="utf-8"))
        return 0
    if args.command == "quickstart":
        print(f"""Google Chat a coordinator running in Herdr

1. Start a Codex or Claude coordinator in Herdr. Record its pane ID, workspace
   label, working directory, and (when available) native conversation ID.
2. Configure user OAuth access to read/send Google Chat messages and create
   reactions. Set the access token in HERDR_CHAT_TOKEN, or use token_command.
3. Save chat.json, replacing the example IDs and paths with your own:
   {{
     "space": "spaces/SPACE_ID",
     "allowed_senders": ["users/YOUR_USER_ID"],
     "agent_label": "codex-coordinator",
     "target": {{"pane_id": "PANE_ID", "expected_agent": "codex",
                "expected_cwd": "/work/project", "expected_workspace": "project"}},
     "ack_reaction": "🤖"
   }}
4. Run: {prog} init --config chat.json
   Then: {prog} run --interval 3
5. Send a message in the configured space. The reaction acknowledges durable
   intake; the agent's final answer arrives later in the same thread.

Set ack_reaction to a different Unicode emoji, or null/"" to disable ACKs.
Optional reaction_user: "users/OAUTH_USER_ID" lets the public REST adapter
reconcile an existing reaction after a lost create response. This is the
credential's user, which can differ from an allowed sender.

Keep the bridge process running independently of the coordinator's pane.
Inspect delivery and reaction retry errors with '{prog} status'.
See '{prog} userguide' for exact scopes and the command-adapter protocol.""")
        return 0
    if args.command is None:
        parser.print_help()
        return 0
    try:
        if args.command == "init":
            with args.config.open(encoding="utf-8") as handle:
                config = Config.parse(as_mapping(json.load(handle), "chat config"))
            client = HerdrClient()
            resolve_target(_NamedClient(client, config.agent_name) if config.agent_name else client, config.target)
            Bridge.initialize(args.state, config, after=args.after)
            print(json.dumps({"state": str(args.state.absolute()), "space": config.space}))
        elif args.command == "reply":
            submit_reply(args.state, args.request, args.file.read_text(encoding="utf-8"))
            print(json.dumps({"outcome": "reply_queued", "request": args.request}))
        else:
            bridge = Bridge(args.state)
            if args.command == "status":
                print(json.dumps(bridge.status(), indent=2))
            elif args.command == "tick":
                print(json.dumps(bridge.tick(), indent=2))
            else:
                if not 0.1 <= args.interval <= 60:
                    raise ValueError("interval must be between 0.1 and 60 seconds")
                delay = args.interval
                while True:
                    try:
                        bridge.tick()
                    except (OSError, ValueError, TypeError, HerdrRunError, subprocess.SubprocessError) as exc:
                        print(f"{prog}: {exc}", file=sys.stderr, flush=True)
                        delay = min(60.0, delay * 2)
                    else:
                        delay = args.interval
                    time.sleep(delay)
        return 0
    except (OSError, ValueError, TypeError, HerdrRunError, subprocess.SubprocessError) as exc:
        print(f"{prog}: {exc}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130


def main(argv: Sequence[str] | None = None) -> int:
    """Compatibility entry point retaining herdr-chat's state directory."""
    return run_cli(argv, prog="herdr-chat", default_state=Path(".herdr-chat"))


if __name__ == "__main__":
    raise SystemExit(main() if Path(sys.argv[0]).name == "herdr-chat" else run_cli())
