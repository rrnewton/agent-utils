#!/usr/bin/env python3
"""Durable, single-coordinator chat bridge; transport and harness stay independent.

Run with ``python -m herdr_run.chat``. A command adapter exchanges one JSON request
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

from herdr_run import __version__
from herdr_run.agent import (
    Target, _atomic_json, _open_private_lock, _read_queue_json,
    _validate_private_directory, drain, enqueue, resolve_target,
)
from herdr_run.client import AgentPaneInfo, HerdrClient, Pane
from herdr_run.errors import HerdrRunError, HerdrUnavailable
from herdr_run.jsonx import as_mapping, as_sequence, get_str


class Transport(Protocol):
    """One bounded operation; send MUST deduplicate repeated request_id values."""

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
        return cls(space, _strings(document.get("allowed_senders"), "allowed_senders"), target,
                   label, () if command is None or command == [] else _strings(command, "transport_command"), token_env,
                   () if token_command is None or token_command == [] else _strings(token_command, "token_command"),
                   agent_name)


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

    def run(self, pane_id: str, command: str) -> None:
        self.pane_info(pane_id)
        self._delegate.run(pane_id, command)

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

    def __init__(self, token_env: str = "HERDR_CHAT_TOKEN", token_command: Sequence[str] = ()) -> None:
        self.token_env = token_env
        self.token_command = tuple(token_command)

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
        url = f"https://chat.googleapis.com/v1/{space}/messages?{urlencode(params)}"
        http = Request(url, data=data, headers={"Authorization": f"Bearer {token}",
                                               "Content-Type": "application/json"})
        with urlopen(http, timeout=45) as response:
            document = as_mapping(json.load(response), "Google Chat response")
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
            if self.config.transport_command else GoogleChatTransport(self.config.token_env, self.config.token_command))
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
                              "received_at": _utc()})
        cursor = result.get("cursor")
        if cursor is not None and not isinstance(cursor, str):
            raise ValueError("transport cursor must be a string or null")
        checkpoint.update(cursor=cursor or None, high_water=high.isoformat().replace("+00:00", "Z"))
        if not cursor:
            # Overlap the high-water timestamp: messages with equal timestamps and brief
            # indexing delays are deduplicated by their immutable resource names.
            checkpoint["after"] = (high - timedelta(seconds=60)).isoformat().replace("+00:00", "Z")
        _write(self.state / "bridge.json", checkpoint)

    def _prompt(self, record: dict[str, object]) -> str:
        message = as_mapping(record["message"], "message")
        key = get_str(record, "key", "request")
        reply = shlex.join([sys.executable, str(Path(__file__).resolve()), "reply", "--state", str(self.state),
                            "--request", key, "--file", "PATH_TO_YOUR_REPLY"])
        return ("A message from an authorized user arrived through your configured chat bridge.\n"
                f"Source: {message['id']}\nSender: {message['sender']}\n\n"
                f"{message['text']}\n\n"
                "Complete this user's request using your normal instructions and tools. Only you are the "
                "chat coordinator; manage other long-lived agents through herdr-agent as needed. "
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
            # Recover an accepted reply's lost acknowledgement before reading its
            # echo. This matters when the adapter posts as an allowlisted user.
            self._deliver()
            self._ingest(_read(self.state / "bridge.json"))
            self._deliver()
            return self.status()
        finally:
            os.close(descriptor)

    def status(self) -> dict[str, object]:
        """Return durable request phases without accessing Chat or the harness."""
        records = [_read(path) for path in sorted((self.state / "requests").glob("*.json"))]
        return {"space": self.config.space, "target": asdict(self.config.target),
                "agent_name": self.config.agent_name, "requests": records}


def submit_reply(state: Path, key: str, text: str) -> None:
    """Commit one final answer locally; the bridge alone has transport credentials."""
    if re.fullmatch(r"[0-9a-f]{64}", key) is None:
        raise ValueError("invalid request key")
    if not text.strip() or len(text.encode()) > 30000:
        raise ValueError("reply must contain 1-30000 UTF-8 bytes")
    record = _read(state / "requests" / f"{key}.json")
    if record.get("phase") not in ("queued", "awaiting_reply", "reply_pending", "delivery_uncertain"):
        raise ValueError("request is not awaiting a reply")
    _private(state / "replies")
    path = state / "replies" / f"{key}.json"
    if path.exists():
        if _read(path).get("text") == text:
            return
        raise ValueError("a different reply already exists for this request")
    from herdr_run.agent import _atomic_json_create
    _atomic_json_create(str(path), {"text": text})


def main(argv: Sequence[str] | None = None) -> int:
    """Initialize, inspect, run, or reply through a coordinator bridge."""
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("--version", action="version", version=f"herdr-chat {__version__}")
    parser.add_argument("--userguide", action="store_true")
    parser.add_argument("command", nargs="?", choices=("init", "tick", "run", "status", "reply"))
    parser.add_argument("--state", type=Path, default=Path(".herdr-chat"))
    parser.add_argument("--config", type=Path)
    parser.add_argument("--request")
    parser.add_argument("--file", type=Path)
    parser.add_argument("--after")
    parser.add_argument("--interval", type=float, default=3)
    args = parser.parse_args(argv)
    if args.userguide:
        from importlib.resources import files
        sys.stdout.write((files("herdr_run") / "CHAT_USER_GUIDE.md").read_text(encoding="utf-8"))
        return 0
    if args.command is None:
        parser.print_help()
        return 0
    try:
        if args.command == "init":
            if args.config is None:
                raise ValueError("init requires --config")
            with args.config.open(encoding="utf-8") as handle:
                config = Config.parse(as_mapping(json.load(handle), "chat config"))
            client = HerdrClient()
            resolve_target(_NamedClient(client, config.agent_name) if config.agent_name else client, config.target)
            Bridge.initialize(args.state, config, after=args.after)
            print(json.dumps({"state": str(args.state.absolute()), "space": config.space}))
        elif args.command == "reply":
            if args.file is None or args.request is None:
                raise ValueError("reply requires --request and --file")
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
                        print(f"herdr-chat: {exc}", file=sys.stderr, flush=True)
                        delay = min(60.0, delay * 2)
                    else:
                        delay = args.interval
                    time.sleep(delay)
        return 0
    except (OSError, ValueError, TypeError, HerdrRunError, subprocess.SubprocessError) as exc:
        print(f"herdr-chat: {exc}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        return 130


if __name__ == "__main__":
    raise SystemExit(main())
