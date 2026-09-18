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
import secrets
import shlex
import signal
import subprocess
import sys
import threading
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
from agentctl.chat_output import PaneAgentStatus, PaneOutputSnapshot, PaneOutputStream
from agentctl.chat_replies import extract_replies
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


def _reply_instruction(nonce: str) -> str:
    if re.fullmatch(r"[A-Za-z0-9_-]{22}", nonce) is None:
        raise ValueError("invalid saved chat reply nonce")
    return (f"Put your final reply between <GCHAT_REPLY_{nonce}> and </GCHAT_REPLY_{nonce}>, "
            "each on its own line, without a code fence.")


def _closing_pattern(nonces: Sequence[str]) -> str:
    for nonce in nonces:
        _reply_instruction(nonce)
    return r"^\s*(?:[•⏺]\s+)?</GCHAT_REPLY_(?:" + "|".join(map(re.escape, nonces)) + r")>\s*$"


def _request_path(state: Path, key: str) -> Path:
    if re.fullmatch(r"[0-9a-f]{12,64}", key) is None:
        raise ValueError("request must be a hexadecimal key or unique prefix of at least 12 characters")
    matches = list((state / "requests").glob(key + "*.json"))
    if len(matches) != 1:
        raise ValueError(f"request prefix must identify exactly one saved request; found {len(matches)}")
    return matches[0]


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
    reply_mode: str = "tagged"
    event_command: tuple[str, ...] = ()
    transport_socket: str | None = None

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
        event_command = document.get("event_command")
        transport_socket = document.get("transport_socket")
        if transport_socket is not None and (
            not isinstance(transport_socket, str) or not os.path.isabs(transport_socket) or "\0" in transport_socket
        ):
            raise ValueError("transport_socket must be an absolute Unix socket path")
        if transport_socket is not None and command:
            raise ValueError("transport_socket and transport_command are mutually exclusive")
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
        reply_mode = document.get("reply_mode", "tagged")
        if reply_mode not in ("tagged", "file"):
            raise ValueError("reply_mode must be tagged or file")
        return cls(space, _strings(document.get("allowed_senders"), "allowed_senders"), target,
                   label, () if command is None or command == [] else _strings(command, "transport_command"), token_env,
                   () if token_command is None or token_command == [] else _strings(token_command, "token_command"),
                   agent_name, _reaction_emoji(document.get("ack_reaction", "🤖")), reaction_user, str(reply_mode),
                   () if event_command is None or event_command == [] else _strings(event_command, "event_command"),
                   transport_socket)


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

    def event_socket(self) -> str:
        return self._delegate.event_socket()


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
        if action == "context":
            from agentctl.chat_context import google_context_params
            params.update(google_context_params(request))
        elif action == "poll":
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
            normalized: dict[str, object] = {"id": message.get("name"), "text": message.get("text", ""),
                                            "sender": sender.get("name"), "thread": thread.get("name"),
                                            "created_at": message.get("createTime")}
            if "threadReply" in message:
                if not isinstance(message["threadReply"], bool):
                    raise ValueError("Google Chat threadReply must be a boolean")
                normalized["thread_reply"] = message["threadReply"]
            messages.append(normalized)
        return {"messages": messages, "cursor": document.get("nextPageToken") or None}


class Bridge:
    """Restartable inbox, Herdr delivery queue, and idempotent threaded reply outbox."""

    def __init__(self, state: Path, client: HerdrClient | None = None,
                 transport: Transport | None = None) -> None:
        self.state = state.absolute()
        _private(self.state)
        self.config = Config.parse(as_mapping(_read(self.state / "bridge.json")["config"], "saved config"))
        selected_client = client or HerdrClient()
        self.client = _NamedClient(selected_client, self.config.agent_name) if self.config.agent_name else selected_client
        from agentctl.chat_socket import SocketTransport
        self.transport: Transport = transport or (SocketTransport(self.config.transport_socket)
            if self.config.transport_socket else CommandTransport(self.config.transport_command)
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
        result = self.transport({"action": "poll", "space": self.config.space,
                                 "after": checkpoint["after"], "cursor": checkpoint.get("cursor")})
        self._ingest_result(result, checkpoint)

    def _validate_message_source(self, message: dict[str, object]) -> datetime:
        """Validate normalized resource identity and creation time before saving input."""
        identifier = get_str(message, "id", "message")
        thread = get_str(message, "thread", "message")
        if (re.fullmatch(re.escape(self.config.space) + r"/messages/[A-Za-z0-9_.-]+", identifier) is None
                or re.fullmatch(re.escape(self.config.space) + r"/threads/[A-Za-z0-9_.-]+", thread) is None):
            raise ValueError("transport returned a message outside the configured space")
        return _timestamp(get_str(message, "created_at", "message"))

    @staticmethod
    def _validate_message_content(message: dict[str, object]) -> None:
        """Validate authorized nonempty text and optional provider reply metadata."""
        text = get_str(message, "text", "message")
        if "thread_reply" in message and not isinstance(message["thread_reply"], bool):
            raise ValueError("thread_reply must be a boolean")
        if len(text.encode()) > 32000:
            raise ValueError("chat message exceeds 32000 bytes")

    def _ingest_result(self, result: dict[str, object], checkpoint: dict[str, object] | None = None) -> None:
        own_replies = {_read(path).get("reply_id")
                       for path in (self.state / "requests").glob("*.json")}
        saved = checkpoint if checkpoint is not None else _read(self.state / "bridge.json")
        high = _timestamp(get_str(saved, "high_water", "checkpoint"))
        start = _timestamp(get_str(saved, "started_at", "checkpoint"))
        for value in as_sequence(result.get("messages"), "polled messages"):
            message = as_mapping(value, "polled message")
            identifier = get_str(message, "id", "message")
            if identifier in own_replies:
                continue
            created = self._validate_message_source(message)
            high = max(high, created)
            if created < start:
                continue
            if message.get("sender") not in self.config.allowed_senders or not message.get("text"):
                continue
            self._validate_message_content(message)
            key = hashlib.sha256(identifier.encode()).hexdigest()
            path = self.state / "requests" / f"{key}.json"
            if not path.exists():
                record: dict[str, object] = {"key": key, "message": message, "phase": "received",
                                            "queue_id": f"{int(created.timestamp() * 1_000_000):020d}-{key}",
                                            "request_id": str(uuid.uuid5(uuid.NAMESPACE_URL, identifier)),
                                            "received_at": _utc(), "ack": self._ack_record(identifier)}
                if self.config.reply_mode == "tagged":
                    # Persist an unpredictable marker before the prompt can reach the TUI.
                    # Source text cannot know it; retained output from another request cannot match it.
                    record["reply_nonce"] = secrets.token_urlsafe(16)
                _write(path, record)
        if checkpoint is None:
            return
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
        context = ""
        if message.get("thread_reply") is True:
            launcher = Path(__file__).resolve().parent.parent / "bin" / "agentctl"
            invocation = ([str(launcher), "chat"] if launcher.is_file() and os.access(launcher, os.X_OK)
                          else [sys.executable, str(Path(__file__).resolve())])
            command = shlex.join([*invocation, "context", "--state", str(self.state),
                                  "--request", key[:12], "--limit", "10"])
            context = ("Run this for the prior 10 messages in this thread; read further back if needed:\n"
                       f"{command}\n")
        nonce = record.get("reply_nonce")
        if isinstance(nonce, str):
            return ("The user's request arrived through the Google Chat bridge.\n"
                    f"Source: {message['id']}\n"
                    f"{_reply_instruction(nonce)}\n"
                    f"{context}\n{message['text']}")
        reply = shlex.join([sys.executable, str(Path(__file__).resolve()), "reply", "--state", str(self.state),
                            "--request", key, "--file", "PATH_TO_YOUR_REPLY"])
        return ("A message from an authorized user arrived through your configured chat bridge.\n"
                f"Source: {message['id']}\nSender: {message['sender']}\n{context}\n"
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

    def output_requests(self, *, retry_failed: bool = False) -> dict[str, str]:
        """Return unfinished tagged requests; a completed artifact needs no more TUI reads."""
        result: dict[str, str] = {}
        for path in sorted((self.state / "requests").glob("*.json")):
            record = _read(path)
            nonce = record.get("reply_nonce")
            if not isinstance(nonce, str) or record.get("phase") not in (
                "queued", "awaiting_reply", "delivery_uncertain"
            ):
                continue
            _reply_instruction(nonce)
            key = get_str(record, "key", "chat request")
            if (self.state / "replies" / f"{key}.json").exists():
                continue
            if record.get("capture_error") and not retry_failed:
                continue
            if nonce in result:
                raise ValueError("saved chat requests contain a duplicate reply nonce")
            result[nonce] = key
        return result

    def open_output(self, nonces: Sequence[str]) -> PaneOutputStream:
        """Subscribe to unique closing markers after checking the pinned coordinator."""
        info = resolve_target(self.client, self.config.target)
        return PaneOutputStream(self.client.event_socket(), info.pane_id,
                               _closing_pattern(nonces) if nonces else "", watch_settled=True)

    def capture_event(self, event: PaneOutputSnapshot | PaneAgentStatus, *, deliver: bool = True) -> dict[str, object]:
        """Treat settled-state events as a hint to recheck incomplete replies."""
        if isinstance(event, PaneOutputSnapshot):
            return self.capture_output(event, deliver=deliver)
        info = resolve_target(self.client, self.config.target)
        if info.pane_id != event.pane_id:
            raise HerdrUnavailable("chat status event belongs to a different coordinator pane")
        if info.status not in ("idle", "done"):
            return {"captured": [], "errors": []}
        # One read per settled event recovers an early close inside a quoted
        # example, or a missed output edge. Idle alone never proves an answer.
        text = self.client.read(info.pane_id, source="recent-unwrapped", lines=4000)
        if len(text.encode("utf-8")) > 2 * 1024 * 1024:
            raise ValueError("retained chat output exceeds the 2 MiB capture limit")
        return self.capture_output(PaneOutputSnapshot(info.pane_id, text, None), deliver=deliver)

    def capture_output(self, snapshot: PaneOutputSnapshot, *, deliver: bool = True) -> dict[str, object]:
        """Durably capture complete matching blocks and immediately reconcile the outbox."""
        return self._capture_output(snapshot, deliver=deliver, verify_target=True)

    def _capture_output(self, snapshot: PaneOutputSnapshot, *, deliver: bool, verify_target: bool) -> dict[str, object]:
        descriptor = _open_private_lock(str(self.state / ".bridge.lock"), "chat bridge lock")
        try:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
            if verify_target:
                info = resolve_target(self.client, self.config.target)
                if snapshot.pane_id != info.pane_id:
                    raise HerdrUnavailable("chat reply output belongs to a different coordinator pane")
            captured: list[str] = []
            errors: list[dict[str, str]] = []
            for nonce, key in self.output_requests(retry_failed=True).items():
                if re.search(_closing_pattern([nonce]), snapshot.text, flags=re.MULTILINE) is None:
                    continue
                path = self.state / "requests" / f"{key}.json"
                record = _read(path)
                # Remove unrelated older scrollback when the echoed instruction is
                # still retained. Its inline markers cannot constitute a reply.
                text = snapshot.text
                instruction = _reply_instruction(nonce)
                start = text.find(instruction)
                if start >= 0:
                    text = text[start + len(instruction):]
                try:
                    answers = extract_replies(text, (nonce,))
                    if nonce not in answers:
                        raise ValueError("closing marker is visible but no complete reply block is retained; inspect the pane and retry chat tick or submit a reply file")
                    submit_reply(self.state, key, answers[nonce])
                except ValueError as exc:
                    error = str(exc)[:2000]
                    record.update(capture_error=error, capture_failed_at=_utc())
                    errors.append({"request": key, "error": error})
                else:
                    record.pop("capture_error", None)
                    record.pop("capture_failed_at", None)
                    record["reply_capture"] = {"source": "herdr_output", "pane_id": snapshot.pane_id,
                                               "captured_at": _utc(), "snapshot_truncated": snapshot.truncated}
                    captured.append(key)
                _write(path, record)
            # Existing send request IDs retain their idempotency semantics. A
            # provider failure here leaves the captured artifact ready for retry.
            if deliver:
                self._deliver()
            return {"captured": captured, "errors": errors}
        finally:
            os.close(descriptor)

    def capture_once(self) -> dict[str, object]:
        """Inspect retained matching output once, including explicitly retried failures."""
        nonces = tuple(self.output_requests(retry_failed=True))
        if not nonces:
            return {"captured": [], "errors": []}
        stream = self.open_output(nonces)
        try:
            for snapshot in stream.wait(0.25):
                return self.capture_event(snapshot)
            return {"captured": [], "errors": []}
        finally:
            stream.close()

    def status(self) -> dict[str, object]:
        """Return durable request phases without accessing Chat or the harness."""
        records = [_read(path) for path in sorted((self.state / "requests").glob("*.json"))]
        return {"space": self.config.space, "target": asdict(self.config.target),
                "agent_name": self.config.agent_name, "ack_reaction": self.config.ack_reaction,
                "reply_mode": self.config.reply_mode,
                "input_observer": _read(self.state / "input.json") if (self.state / "input.json").exists() else None,
                "output_observer": _read(self.state / "output.json") if (self.state / "output.json").exists() else None,
                "requests": records}


def _run_bridge(bridge: Bridge, interval: float, prog: str, *, reconcile_interval: float = 300) -> None:
    """Run one durable bridge owner with streaming or polling intake."""
    descriptor = _open_private_lock(str(bridge.state / ".run.lock"), "chat runner lock")
    previous = signal.getsignal(signal.SIGTERM)
    handle_signals = threading.current_thread() is threading.main_thread()
    def terminate(signum: int, frame: object) -> None:
        raise KeyboardInterrupt
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        if handle_signals:
            signal.signal(signal.SIGTERM, terminate)
        if bridge.config.event_command:
            from agentctl.chat_runtime import run_streaming
            run_streaming(bridge, reconcile_interval=reconcile_interval, prog=prog)
        else:
            _run_polling(bridge, interval, prog)
    finally:
        if handle_signals:
            signal.signal(signal.SIGTERM, previous)
        os.close(descriptor)


def _run_polling(bridge: Bridge, interval: float, prog: str) -> None:
    """Poll Chat on its schedule while blocking for terminal reply events between polls."""
    stream: PaneOutputStream | None = None
    subscribed: tuple[tuple[str, ...], tuple[str, ...]] | None = None
    next_poll = 0.0
    poll_delay = interval
    reconnect_at = 0.0
    reconnect_delay = 1.0
    errors = (OSError, ValueError, TypeError, HerdrRunError, subprocess.SubprocessError)
    try:
        while True:
            if time.monotonic() >= next_poll:
                try:
                    bridge.tick()
                except errors as exc:
                    print(f"{prog}: {exc}", file=sys.stderr, flush=True)
                    poll_delay = min(60.0, poll_delay * 2)
                else:
                    poll_delay = interval
                next_poll = time.monotonic() + poll_delay
            unfinished = tuple(bridge.output_requests(retry_failed=True))
            nonces = tuple(bridge.output_requests())
            signature = (unfinished, nonces)
            if signature != subscribed:
                if stream is not None:
                    stream.close()
                stream = None
                subscribed = signature
                reconnect_at = 0.0
                reconnect_delay = 1.0
                if not unfinished:
                    _write(bridge.state / "output.json", {"state": "idle", "error": None,
                                                         "updated_at": _utc()})
            if unfinished and stream is None and time.monotonic() >= reconnect_at:
                try:
                    stream = bridge.open_output(nonces)
                except errors as exc:
                    print(f"{prog}: output subscription: {exc}", file=sys.stderr, flush=True)
                    _write(bridge.state / "output.json", {"state": "retrying", "error": str(exc)[:2000],
                                                         "updated_at": _utc()})
                    reconnect_at = time.monotonic() + reconnect_delay
                    reconnect_delay = min(60.0, reconnect_delay * 2)
                else:
                    _write(bridge.state / "output.json", {"state": "connected", "error": None,
                                                         "updated_at": _utc()})
                    reconnect_delay = 1.0
            deadline = next_poll
            if unfinished and stream is None:
                deadline = min(deadline, reconnect_at)
            timeout = max(0.0, deadline - time.monotonic())
            if stream is None:
                time.sleep(timeout)
                continue
            try:
                for snapshot in stream.wait(timeout):
                    outcome = bridge.capture_event(snapshot)
                    for error in as_sequence(outcome["errors"], "capture errors"):
                        print(f"{prog}: output capture: {error}", file=sys.stderr, flush=True)
            except errors as exc:
                print(f"{prog}: output subscription: {exc}", file=sys.stderr, flush=True)
                _write(bridge.state / "output.json", {"state": "retrying", "error": str(exc)[:2000],
                                                     "updated_at": _utc()})
                stream.close()
                stream = None
                reconnect_at = time.monotonic() + reconnect_delay
                reconnect_delay = min(60.0, reconnect_delay * 2)
    finally:
        if stream is not None:
            stream.close()


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
            "(default: 🤖). Bracketed final replies are harvested from terminal output and sent durably."
        ),
        epilog=f"""Examples:
  {prog} init --config chat.json
  {prog} run --interval 10
  {prog} status
  {prog} context --request "$REQUEST_KEY" --limit 10
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
        "tick": "Poll one page, ACK accepted messages, deliver ready prompts, and inspect retained tagged replies once.",
        "run": "Stream Chat events when event_command is configured, otherwise poll; deliver prompts, ACKs, and tagged replies.",
        "status": "Print saved target, request phases, ACK attempts, and retry errors as JSON; no network or harness access.",
        "context": "Read a page of messages before a saved request in its thread; no live harness or state changes required.",
        "reply": "Durably queue one final answer from a UTF-8 file for the bridge to post in its originating thread.",
        "quickstart": "Print the shortest setup path and an example configuration.",
        "userguide": "Print the complete setup, authentication, adapter, and recovery guide.",
    }
    subparsers: dict[str, argparse.ArgumentParser] = {}
    examples = {"init": "--config chat.json", "tick": "", "run": "--interval 10", "status": "",
                "reply": '--request "$REQUEST_KEY" --file answer.txt', "quickstart": "", "userguide": "",
                "context": '--request "$REQUEST_KEY" --limit 10'}
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
        help="delay after each poll cycle in seconds, 0.1–60 (default: 3); used without event_command; failures back off to 60 seconds")
    subparsers["run"].add_argument("--reconcile-interval", type=float, default=300, metavar="SECONDS",
        help="REST recovery scan interval with event_command, 10–86400 seconds (default: 300); push messages never wait for this scan")
    subparsers["context"].add_argument("--request", required=True, metavar="KEY_OR_PREFIX",
        help="saved request's hexadecimal key or unique prefix of at least 12 characters; fixes thread and cutoff")
    subparsers["context"].add_argument("--limit", type=int, default=10, metavar="COUNT",
        help="maximum prior messages per page, 1–200 (default: 10); nearest prior messages, displayed chronologically")
    subparsers["context"].add_argument("--cursor", metavar="TOKEN",
        help="opaque cursor from the preceding context result to read older messages; keep request and limit unchanged")
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
   intake; the agent brackets its final answer with the unique tags in its prompt.
   The daemon captures that block and posts it in the same thread. No reply tool
   call or file write is required from the coordinator.

Set ack_reaction to a different Unicode emoji, or null/"" to disable ACKs.
Optional reaction_user: "users/OAUTH_USER_ID" lets the public REST adapter
reconcile an existing reaction after a lost create response. This is the
credential's user, which can differ from an allowed sender.

The default reply_mode is "tagged". Set it to "file" for explicit reply files.
Herdr output subscriptions wake the bridge independently of Chat's poll interval;
Herdr 0.8 checks its match predicates internally every 100 milliseconds.
Thread replies include a command to read the prior ten messages when needed.

For push intake, configure event_command as an adapter argv array. It receives
one subscribe request and emits newline-delimited message/control events.
In this mode ACKs, prompts, and replies run independently; --reconcile-interval
(default 300 seconds) controls REST recovery checks, not message intake.
Use transport_socket for a persistent private Unix adapter, or transport_command
for a per-request command. The built-in REST transport needs neither.

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
        elif args.command == "context":
            from agentctl.chat_context import read_context
            from agentctl.chat_socket import SocketTransport
            config = Config.parse(as_mapping(_read(args.state / "bridge.json")["config"], "saved config"))
            path = _request_path(args.state, args.request)
            record = _read(path)
            if record.get("key") != path.stem:
                raise ValueError("saved request key does not match its file")
            source = as_mapping(record.get("message"), "saved source message")
            if not get_str(source, "id", "saved source message").startswith(config.space + "/messages/"):
                raise ValueError("saved request belongs to a different configured space")
            transport: Transport = (SocketTransport(config.transport_socket) if config.transport_socket
                                    else CommandTransport(config.transport_command) if config.transport_command
                                    else GoogleChatTransport(config.token_env, config.token_command, config.reaction_user))
            print(json.dumps(read_context(transport, source, limit=args.limit, cursor=args.cursor), indent=2))
        else:
            bridge = Bridge(args.state)
            if args.command == "status":
                print(json.dumps(bridge.status(), indent=2))
            elif args.command == "tick":
                descriptor = _open_private_lock(str(bridge.state / ".run.lock"), "chat runner lock")
                try:
                    fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
                    bridge.tick()
                    bridge.capture_once()
                finally:
                    os.close(descriptor)
                print(json.dumps(bridge.status(), indent=2))
            else:
                if not 0.1 <= args.interval <= 60:
                    raise ValueError("interval must be between 0.1 and 60 seconds")
                if not 10 <= args.reconcile_interval <= 86400:
                    raise ValueError("reconcile interval must be between 10 and 86400 seconds")
                _run_bridge(bridge, args.interval, prog, reconcile_interval=args.reconcile_interval)
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
