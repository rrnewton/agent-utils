#!/usr/bin/env python3
"""An opt-in live check of channel messages relayed into a call, against any `vibe-talk-v1` bridge.

`#354 live-relay-check`. The rule under test (`#337 voice-relay-silent`, `#330 voice-long-turn-drop`,
`#352 voice-reconnect-on-bridge-error`): with Settings > Voice > "Arriving messages during a call"
set to "Read them out in full", a new message in ANY watched channel is relayed into the live call
exactly once and read out, or the page says why not; it is never dropped in silence, never read out
twice, and never cut into a long answer. A message in a channel that is not watched is never relayed.
The page suite and the loopback walks prove the page against fakes; what a real bridge does with a
relayed prompt — and with an interrupt followed by a relay, where a bridge can lose the turn and ask
the page to reconnect — is only seen on a real call. That is what this runs.

What runs, all on this machine:

  * a fake chat API speaking the Discord REST protocol, which vibe-talk reads its channels from. It
    refuses every post. It delivers a new message on demand: stores it for reads and pushes it to
    vibe-talk's adapter route `POST /api/v1/live/events` with the ingest token, as a push adapter does;
  * the vibe-talk binary under test, on a free loopback port in a fresh run directory, with a
    generated config whose `[conversation]` backend is the websocket bridge at the page's own `/ws`;
  * a loopback front that sends `/ws` to the bridge named by --bridge (rewriting the request to the
    bridge's own path and host) and everything else to that vibe-talk;
  * the harness's own read-token subscriber to the server's live streams, which says whether the
    SERVER published a message when the page did not relay it;
  * headless Chromium on its fake microphone, signed in, watching channels A (open in view) and B
    (watched, not in view); C is configured and then taken off the list (unwatched).

The calls, each under two minutes, one at a time:

  1. (a) a message in A while idle; (b) one in B while idle; (d) one in C, the negative control.
     Then the bridge's own voice is asked to say a story request, and that audio is kept.
  2. The browser is relaunched with that audio as its microphone, and asks for a long answer OUT
     LOUD. Into the answer go (c), one in B 8 s in, and (e), one in A 12 s in.
  3. (f) INTERRUPT THEN RELAY. The microphone says the request, then, a few seconds into the answer,
     the first second of it again: the bridge's barge-in cuts the answer. A second after the
     interrupted turn, a message arrives in B. A bridge may lose the next turn here and send an error
     asking the client to reconnect. PASS is the message read out exactly once: directly, or on the new
     session the page carried the call to — with the call still live, the transcript saying it
     reconnected, and the server's record of the replaced socket. A bridge that loses the turn again
     within the minute may end the call, if the page says the message was not read out.

Every case is judged after all the calls, over the whole run's frames. Verdicts are PASS, FAIL,
ERROR (the harness could not test it) or SKIP, each with where the evidence points.

SAFETY. A bridge address that asks for agent tools (`tools=`) is refused: tools act on a real chat,
and a test call must never be able to read or post in one. Run it only against a bridge nobody else
is in a call on; it starts at most --max-calls calls, one at a time, each hung up by --call-budget.
It stops only the processes it started. Nothing it starts listens anywhere but loopback.

`--bridge fake` runs the whole check against an in-process stand-in, with no network, and
`--fake-fault` makes the stand-in misbehave, to check the verdicts. `--self-test` checks the verdict
logic alone on synthetic frame logs. Neither needs a bridge; the live run is never part of an
automated suite.
"""

from __future__ import annotations

import argparse
import array
import asyncio
import base64
import hashlib
import json
import math
import os
import random
import re
import secrets
import shutil
import signal
import socket
import ssl
import subprocess
import sys
import threading
import time
import traceback
import urllib.error
import urllib.request
import wave
from collections.abc import Callable
from dataclasses import asdict, dataclass, field, is_dataclass
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, unquote, urlsplit

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Error, Page, Playwright, WebSocket
    from websockets.asyncio.server import ServerConnection

Json = dict[str, object]

HARNESS_VERSION = "1"
VIBE_TALK = Path(__file__).resolve().parent.parent
DEFAULT_BINARY = VIBE_TALK / "target" / "release" / "vibe-talk"
# The bridge's audio rate both ways (vibe-talk's websocket provider says 24 kHz).
PCM_RATE = 24_000

EXIT_OK = 0
EXIT_FAIL = 1
EXIT_ERROR = 2
EXIT_SELF_TEST = 12

CHANNELS: dict[str, dict[str, str]] = {
    "A": {"id": "1430000000000000101", "label": "qa-alpha"},
    "B": {"id": "1430000000000000102", "label": "qa-bravo"},
    "C": {"id": "1430000000000000103", "label": "qa-charlie"},
}
BOT_ID = "1430000000000000009"
OWNER_ID = "1430000000000000002"
SENDER = {"id": "1430000000000000077", "name": "Dana Q"}
# Words a story about a lighthouse keeper is unlikely to use on its own, so the long answer cannot
# "say" a case's marker without the relay.
MARKER_WORDS = ["kumquat", "zeppelin", "tamarind", "marmalade", "saffron", "nutmeg", "tangerine",
                "cardamom", "accordion", "pistachio", "xylophone", "rutabaga", "kohlrabi", "trombone"]
LONG_PROMPT = ("Please tell me one long story, out loud, about a lighthouse keeper on a stormy night. "
               "Make it about one hundred and fifty words, as one continuous answer, and do not stop "
               "to ask me anything.")
# The long answer is asked for OUT LOUD, as the owner's was: a typed prompt is a different hold path in
# the page (a relay waits behind a typed turn with no bound), so typed is only the labelled fallback.
# Call 1 has the bridge's own voice say the request, and later calls play that as the microphone. One
# clause only: a request of two lets the endpointer finalise the first, and the tail barges in.
SAY_PROMPT = ("Please say the following sentence out loud, word for word, and nothing else: "
              "Tell me a story about a lighthouse keeper on a stormy night.")
SPOKEN_REQUEST_RE = re.compile(r"lighthouse|story|keeper", re.I)

# Wording that says a MESSAGE was not read out, or that the page cannot follow every channel. Bare
# "not read" or "fell behind" match unrelated status lines and could excuse a silent drop.
NOTICE_RE = re.compile(r"\bnot (?:have been |been )?read out\b|\bnever read (?:it |them )?out\b|"
                       r"only the channel on screen|cannot stream every channel", re.I)
# The page saying it carried the call on a new session (`#352`).
RECONNECT_NOTICE_RE = re.compile(r"reconnected after the voice service lost a turn|^reconnected:", re.I)
# A bridge's error that asks the client to reconnect.
RECONNECT_ASKED_RE = re.compile(r";\s*reconnect\s*\.?\s*$", re.I)
ANSI = re.compile(r"\x1b\[[0-9;]*m")

LONG_CASES = ("c", "e")
INTERRUPT_CASES = ("f",)
FAKE_FAULTS = ("none", "ignore-relay", "story-says-markers", "never-start-after-interrupt", "never-start-twice")


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


def log(text: str) -> None:
    print(f"[{datetime.now(timezone.utc).strftime('%H:%M:%S')}] {text}", flush=True)


def as_json(value: object) -> Json:
    return {str(key): item for key, item in value.items()} if isinstance(value, dict) else {}


def as_str(value: object) -> str:
    return value if isinstance(value, str) else ""


def as_strs(value: object) -> list[str]:
    return [str(item) for item in value] if isinstance(value, list) else []


def as_int(value: object) -> int | None:
    return value if isinstance(value, int) and not isinstance(value, bool) else None


# A loopback check must not go through whatever proxy the environment names.
OPENER = urllib.request.build_opener(urllib.request.ProxyHandler({}))


def http(method: str, url: str, token: str | None = None, body: Json | None = None,
         timeout: float = 15.0) -> tuple[int, object]:
    data = None if body is None else json.dumps(body).encode()
    headers = {"Accept": "application/json"}
    if data is not None:
        headers["Content-Type"] = "application/json"
    if token:
        headers["Authorization"] = f"Bearer {token}"
    request = urllib.request.Request(url, data=data, headers=headers, method=method)
    try:
        with OPENER.open(request, timeout=timeout) as response:
            raw: bytes = response.read()
            status = int(response.status)
    except urllib.error.HTTPError as error:
        raw = error.read()
        status = int(error.code)
    try:
        parsed: object = json.loads(raw) if raw else None
    except ValueError:
        parsed = raw.decode(errors="replace")[:500]
    return status, parsed


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


def snowflake(ms: int, counter: int) -> str:
    return str(((ms - 1_420_070_400_000) << 22) + (counter & 0x3FFFFF))


@dataclass(frozen=True)
class Bridge:
    """Where the front sends the page's `/ws`: a bridge's address, or the in-process stand-in."""

    fake: bool
    host: str = "127.0.0.1"
    port: int = 0
    target: str = "/"
    tls: bool = False

    def describe(self) -> str:
        """The address without its query, which may hold a credential."""
        if self.fake:
            return "fake (in-process)"
        return f"{'wss' if self.tls else 'ws'}://{self.host}:{self.port}{self.target.split('?', 1)[0]}"


def asks_for_tools(value: str) -> bool:
    """Whether an address asks the bridge for agent tools, however the key is cased or percent-encoded.

    Every query key is decoded and case-folded, and the whole address is also searched with its escapes
    undone until none are left, so `Tools=`, `%74ools=` and a key hidden in a fragment are refused alike.
    """
    decoded = value
    while (undone := unquote(decoded)) != decoded:
        decoded = undone
    if "tools=" in decoded.casefold():
        return True
    return any(key.casefold() == "tools" for key in parse_qs(urlsplit(value).query, keep_blank_values=True))


def parse_bridge(value: str) -> Bridge:
    """`fake`, or a `ws://` or `wss://` address. Raises ValueError with the reason it is refused."""
    if value == "fake":
        return Bridge(fake=True)
    if asks_for_tools(value):
        raise ValueError("refusing a bridge address with tools=: agent tools act on a real chat, so a test "
                         "call could read or post in one")
    parts = urlsplit(value)
    if parts.scheme not in ("ws", "wss") or not parts.hostname:
        raise ValueError("the bridge must be `fake` or a ws:// or wss:// address")
    tls = parts.scheme == "wss"
    target = (parts.path or "/") + (f"?{parts.query}" if parts.query else "")
    return Bridge(fake=False, host=parts.hostname, port=parts.port or (443 if tls else 80), target=target, tls=tls)


# ------------------------------------------------------------------------------------------------
# The fake chat API, Discord protocol.


@dataclass
class FakeChatApi:
    lock: threading.Lock = field(default_factory=threading.Lock)
    messages: dict[str, list[Json]] = field(default_factory=dict)
    requests: list[Json] = field(default_factory=list)
    posts: list[Json] = field(default_factory=list)
    deliveries: list[Json] = field(default_factory=list)
    counter: int = 0
    ingest_url: str = ""
    ingest_token: str = ""

    def __post_init__(self) -> None:
        for channel in CHANNELS.values():
            self.messages[channel["id"]] = []

    def new_message(self, channel_id: str, author: Json, content: str, at_ms: int | None = None) -> Json:
        with self.lock:
            self.counter += 1
            ms = at_ms if at_ms is not None else int(time.time() * 1000)
            stamp = datetime.fromtimestamp(ms / 1000, timezone.utc).isoformat(timespec="milliseconds")
            message: Json = {
                "id": snowflake(ms, self.counter), "channel_id": channel_id, "type": 0,
                "author": author, "timestamp": stamp.replace("+00:00", "Z"), "content": content,
                "edited_timestamp": None, "mentions": [], "attachments": [], "embeds": [],
            }
            self.messages[channel_id].append(message)
            return message

    def human(self) -> Json:
        return {"id": SENDER["id"], "username": "dana.q", "global_name": SENDER["name"], "bot": False}

    def seed(self) -> None:
        start = int(time.time() * 1000) - 3_600_000
        for index, channel in enumerate(CHANNELS.values()):
            for line in range(3):
                self.new_message(channel["id"], self.human(),
                                 f"Earlier note {line + 1} in {channel['label']} for the relay check.",
                                 at_ms=start + index * 60_000 + line * 5_000)

    def newest_first(self, channel_id: str, query: dict[str, list[str]]) -> list[Json]:
        with self.lock:
            rows = list(self.messages.get(channel_id, []))
        limit = max(1, min(100, int((query.get("limit") or ["50"])[0])))
        if query.get("after"):
            after = int(query["after"][0])
            later = [row for row in rows if int(as_str(row["id"])) > after]
            return list(reversed(later[:limit]))
        if query.get("before"):
            before = int(query["before"][0])
            rows = [row for row in rows if int(as_str(row["id"])) < before]
        return list(reversed(rows))[:limit]

    def deliver(self, channel_id: str, content: str) -> Json:
        """A new message from a person: stored for reads, then pushed as an adapter pushes one."""
        message = self.new_message(channel_id, self.human(), content)
        normalized: Json = {
            "id": message["id"], "channel_id": channel_id, "author": SENDER["name"],
            "author_id": SENDER["id"], "author_is_bot": False, "timestamp": message["timestamp"],
            "content": content, "reply_to": None,
        }
        event: Json = {"event_id": f"check-{message['id']}", "historical": False, "kind": "create",
                       "message": normalized}
        started = time.monotonic()
        utc = utc_now()
        status, answer = http("POST", self.ingest_url, self.ingest_token, event)
        record: Json = {"channel_id": channel_id, "message_id": message["id"], "t": started, "utc": utc,
                        "push_status": status, "push_answer": answer,
                        "push_ms": round((time.monotonic() - started) * 1000)}
        with self.lock:
            self.deliveries.append(record)
        return record


def chat_handler(api: FakeChatApi) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def answer(self, status: int, payload: object) -> None:
            body = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
            except OSError:
                pass

        def note(self, status: int) -> None:
            with api.lock:
                api.requests.append({"utc": utc_now(), "method": self.command,
                                     "path": urlsplit(self.path).path, "status": status})

        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            url = urlsplit(self.path)
            path = re.sub(r"^/api/v\d+", "", unquote(url.path))
            query = parse_qs(url.query)
            parts = [part for part in path.split("/") if part]
            known = {channel["id"]: channel for channel in CHANNELS.values()}
            if path == "/users/@me":
                self.note(200)
                self.answer(200, {"id": BOT_ID, "username": "vibe-talk-check", "bot": True})
            elif len(parts) == 2 and parts[0] == "channels" and parts[1] in known:
                self.note(200)
                self.answer(200, {"id": parts[1], "name": known[parts[1]]["label"], "type": 0})
            elif len(parts) == 3 and parts[0] == "channels" and parts[1] in known and parts[2] == "messages":
                self.note(200)
                self.answer(200, api.newest_first(parts[1], query))
            elif len(parts) == 3 and parts[0] == "channels" and parts[1] in known and parts[2] == "pins":
                self.note(200)
                self.answer(200, [])
            else:
                self.note(404)
                self.answer(404, {"message": f"not served by the fake chat API: GET {path}", "code": 0})

        def do_POST(self) -> None:  # noqa: N802
            path = re.sub(r"^/api/v\d+", "", unquote(urlsplit(self.path).path))
            length = int(self.headers.get("Content-Length") or 0)
            raw = self.rfile.read(length) if length else b""
            with api.lock:
                api.posts.append({"utc": utc_now(), "path": path, "bytes": len(raw)})
            # Nothing in this check may post: refused, so nothing could look as if it had landed.
            self.note(403)
            self.answer(403, {"message": "the fake chat API accepts no posts", "code": 50013})

        def do_PUT(self) -> None:  # noqa: N802
            self.note(204)
            self.send_response(204)
            self.send_header("Content-Length", "0")
            self.end_headers()

        def do_PATCH(self) -> None:  # noqa: N802
            self.do_POST()

        def do_DELETE(self) -> None:  # noqa: N802
            self.do_PUT()

        def log_message(self, format: str, *args: object) -> None:  # noqa: A002 - the base class's name
            pass

    return Handler


# ------------------------------------------------------------------------------------------------
# The harness's own subscriber to the server's live streams.


class StreamObserver:
    """Did the SERVER publish a case's message, on which stream, and when?

    It follows each channel's stream and the stream of every channel with the READ token, so its
    attaches are told apart from the page's in the access log. A subscription has no side effect on
    the server. A missing relay the server carried is the page's; one it never carried is the server's
    (or the fake's push)."""

    def __init__(self, port: int, token: str, markers: list[str]) -> None:
        self.port = port
        self.token = token
        self.markers = [marker.lower() for marker in markers]
        self.lock = threading.Lock()
        self.sightings: list[Json] = []
        self.streams: dict[str, Json] = {}

    def start(self, name: str, path: str) -> None:
        threading.Thread(target=self.follow, args=(name, path), daemon=True).start()

    def follow(self, name: str, path: str) -> None:
        import http.client

        info: Json = {"path": path, "status": None, "data_events": 0, "error": None}
        with self.lock:
            self.streams[name] = info
        events = 0
        try:
            connection = http.client.HTTPConnection("127.0.0.1", self.port, timeout=600)
            connection.request("GET", path, headers={"Authorization": f"Bearer {self.token}",
                                                     "Accept": "text/event-stream"})
            response = connection.getresponse()
            info["status"] = int(response.status)
            if response.status != 200:
                response.read()
                connection.close()
                return
            kind: str | None = None
            while True:
                line = response.readline()
                if not line:
                    info["ended"] = utc_now()
                    break
                text = line.decode(errors="replace").rstrip("\r\n")
                if text.startswith("event:"):
                    kind = text[6:].strip()
                elif text.startswith("data:"):
                    data = text[5:].lower()
                    events += 1
                    info["data_events"] = events
                    for marker in self.markers:
                        if marker in data:
                            with self.lock:
                                self.sightings.append({"marker": marker, "stream": name, "event": kind,
                                                       "t": time.monotonic(), "utc": utc_now()})
                elif not text:
                    kind = None
        except Exception as error:  # noqa: BLE001 - recorded; the server is stopped at the end
            info["error"] = f"{type(error).__name__}: {error}"[:200]

    def seen(self, marker: str) -> list[Json]:
        with self.lock:
            return [dict(sighting) for sighting in self.sightings if sighting["marker"] == marker.lower()]

    def summary(self) -> Json:
        with self.lock:
            return {"streams": {key: dict(value) for key, value in self.streams.items()},
                    "sightings": [{k: v for k, v in s.items() if k != "t"} for s in self.sightings]}


# ------------------------------------------------------------------------------------------------
# Loopback front: the page's /ws to the bridge, the rest to vibe-talk.


class Front:
    """A byte relay on loopback. A request for `/ws` (or `/ws?…`) goes to the bridge, its request
    target rewritten to the bridge's own path and its Host header to the bridge's; anything else goes
    to the vibe-talk under test unchanged."""

    def __init__(self, listen_port: int, app: tuple[str, int], bridge: Bridge) -> None:
        self.listen_port = listen_port
        self.app = app
        self.bridge = bridge
        self.loop = asyncio.new_event_loop()
        self.ready = threading.Event()
        self.voice_sockets = 0
        self.thread = threading.Thread(target=self.run, daemon=True)

    async def relay(self, reader: asyncio.StreamReader, writer: asyncio.StreamWriter) -> None:
        try:
            while data := await reader.read(65536):
                writer.write(data)
                await writer.drain()
        except (OSError, asyncio.CancelledError):
            pass
        finally:
            try:
                writer.close()
            except OSError:
                pass

    def rewritten(self, head: bytes) -> bytes:
        """The upgrade request as the bridge expects it: its own path, its own host."""
        lines = head.split(b"\r\n")
        method, _, version = lines[0].split(b" ", 2)
        host = self.bridge.host if self.bridge.port in (80, 443) else f"{self.bridge.host}:{self.bridge.port}"
        out = [b" ".join([method, self.bridge.target.encode(), version])]
        for line in lines[1:]:
            if line.lower().startswith(b"host:"):
                out.append(b"Host: " + host.encode())
            else:
                out.append(line)
        return b"\r\n".join(out)

    async def handle(self, client_reader: asyncio.StreamReader, client_writer: asyncio.StreamWriter) -> None:
        try:
            head = await asyncio.wait_for(client_reader.readuntil(b"\r\n\r\n"), 10)
            parts = head.split(b"\r\n", 1)[0].split(b" ")
            is_voice = len(parts) == 3 and (parts[1] == b"/ws" or parts[1].startswith(b"/ws?"))
            if is_voice:
                self.voice_sockets += 1
                if self.bridge.tls:
                    context = ssl.create_default_context()
                    up_reader, up_writer = await asyncio.open_connection(
                        self.bridge.host, self.bridge.port, ssl=context, server_hostname=self.bridge.host)
                else:
                    up_reader, up_writer = await asyncio.open_connection(self.bridge.host, self.bridge.port)
                up_writer.write(self.rewritten(head))
            else:
                up_reader, up_writer = await asyncio.open_connection(*self.app)
                up_writer.write(head)
            await up_writer.drain()
        except (OSError, ValueError, asyncio.IncompleteReadError, asyncio.LimitOverrunError, asyncio.TimeoutError):
            client_writer.close()
            return
        await asyncio.gather(self.relay(client_reader, up_writer), self.relay(up_reader, client_writer),
                             return_exceptions=True)

    def run(self) -> None:
        asyncio.set_event_loop(self.loop)

        async def main() -> None:
            server = await asyncio.start_server(self.handle, "127.0.0.1", self.listen_port, reuse_address=True)
            self.ready.set()
            async with server:
                await server.serve_forever()

        try:
            self.loop.run_until_complete(main())
        except (RuntimeError, asyncio.CancelledError):
            pass

    def start(self) -> None:
        self.thread.start()
        if not self.ready.wait(10):
            raise RuntimeError("the loopback front did not start")

    def stop(self) -> None:
        def cancel_all() -> None:
            for task in asyncio.all_tasks(self.loop):
                task.cancel()

        self.loop.call_soon_threadsafe(cancel_all)
        self.thread.join(timeout=5)


# ------------------------------------------------------------------------------------------------
# A fake vibe-talk-v1 bridge, for checking this harness without a real call.


def tone(seconds: float, amplitude: int = 3000) -> bytes:
    """A warble (220/330 Hz, switching every 0.1 s), so a browser's noise suppression does not take
    it for stationary noise when it is replayed as the microphone."""
    samples = array.array("h", (int(amplitude * math.sin(2 * math.pi * (220 if (i // 2400) % 2 else 330) * i / PCM_RATE))
                                for i in range(int(PCM_RATE * seconds))))
    return samples.tobytes()


def rms(pcm: bytes) -> float:
    samples = array.array("h")
    samples.frombytes(pcm[: len(pcm) - len(pcm) % 2])
    return math.sqrt(sum(x * x for x in samples) / len(samples)) if samples else 0.0


STORY = " ".join(["The keeper climbed the stair and lit the lamp."] * 24)


@dataclass
class FakeSessionState:
    """One fake session's turn bookkeeping."""

    turn: int = 0
    greeted: bool = False
    greeting_done: bool = False
    loud_since: float | None = None
    last_loud: float = 0.0
    story: bool = False
    interrupted: bool = False
    prompts: int = 0
    answering: asyncio.Task[None] | None = None


class FakeBridge:
    """Speaks a bridge's frames: a greeting on `audio_start`; loud microphone audio after it, then a
    quiet moment, taken as a spoken request for a story; loud audio during an answer as a barge-in that
    cuts it; `audio_end` or `interrupt` cuts an answer too; a prompt is echoed as the user's transcript
    and answered.

    Faults, to check the verdicts: `ignore-relay` answers a relayed message without reading it;
    `story-says-markers` puts every marker word in the story; `never-start-after-interrupt` echoes the
    first prompt after a cut answer and never answers it, then sends the lost-turn error asking the
    client to reconnect after --fake-watchdog-s (later sessions serve normally);
    `never-start-twice` does that and also loses the next session's first prompt."""

    def __init__(self, port: int, long_answer_s: float, fault: str, watchdog_s: float, replay: bool) -> None:
        self.port = port
        self.long_answer_s = long_answer_s
        self.fault = fault
        self.watchdog_s = watchdog_s
        self.replay = replay
        self.sessions = 0
        self.lost_turns = 0
        self.lose_next_session_first = False
        self.spoken_requests = 0
        self.barge_ins = 0
        self.loop = asyncio.new_event_loop()
        self.ready = threading.Event()
        self.thread = threading.Thread(target=self.run, daemon=True)

    def loses(self, state: FakeSessionState, first_of_session: bool) -> bool:
        """Does this prompt go the way of the lost turn?"""
        if self.fault not in ("never-start-after-interrupt", "never-start-twice"):
            return False
        if first_of_session and self.lose_next_session_first:
            self.lose_next_session_first = False
            return True
        if state.interrupted and self.lost_turns == 0:
            if self.fault == "never-start-twice":
                self.lose_next_session_first = True
            return True
        return False

    async def session(self, ws: ServerConnection) -> None:
        self.sessions += 1
        state = FakeSessionState()
        started: Json = {"type": "session_started", "session_id": f"fake-{self.sessions}", "greeting": True}
        if self.replay:
            started["capabilities"] = ["replay"]
        await ws.send(json.dumps(started))

        async def speak(number: int, text: str, seconds: float) -> None:
            words = text.split()
            steps = max(1, int(seconds / 0.5))
            per = max(1, math.ceil(len(words) / steps))
            said: list[str] = []
            try:
                for index in range(steps):
                    chunk = words[index * per:(index + 1) * per]
                    said.extend(chunk)
                    await ws.send(tone(0.5))
                    if chunk:
                        await ws.send(json.dumps({"type": "transcript", "role": "assistant", "turn": number,
                                                  "text": " ".join(said), "final": False}))
                    await asyncio.sleep(0.45)
                await ws.send(json.dumps({"type": "transcript", "role": "assistant", "turn": number,
                                          "text": text, "final": True}))
                await ws.send(json.dumps({"type": "turn_complete", "turn": number}))
                if number == 1:
                    state.greeting_done = True
            except asyncio.CancelledError:
                await ws.send(json.dumps({"type": "turn_complete", "turn": number, "interrupted": True}))
                raise

        async def cut() -> bool:
            """Cut the answer in progress, if any; whether there was one."""
            running = state.answering
            if running is None or running.done():
                return False
            running.cancel()
            try:
                await running
            except asyncio.CancelledError:
                pass
            state.interrupted = True
            return True

        async def watchdog(number: int) -> None:
            await asyncio.sleep(self.watchdog_s)
            await ws.send(json.dumps({"type": "error",
                                      "message": f"turn {number} never started after an interrupt; reconnect"}))

        async def heard(raw: bytes) -> None:
            if not state.greeting_done:
                return
            now = time.monotonic()
            loud = rms(raw) > 300
            answering = state.answering is not None and not state.answering.done()
            if answering:
                # A barge-in: the owner's voice over an answer cuts it, as a bridge's own barge-in does.
                if loud:
                    state.loud_since = state.loud_since or now
                    if now - state.loud_since >= 0.3:
                        self.barge_ins += 1
                        state.loud_since = None
                        await cut()
                else:
                    state.loud_since = None
                return
            if state.story:
                return
            if loud:
                state.last_loud = now
                state.loud_since = state.loud_since or now
            elif state.loud_since is not None and now - state.last_loud > 0.8:
                state.story = True
                state.loud_since = None
                self.spoken_requests += 1
                state.turn += 1
                await ws.send(json.dumps({"type": "transcript", "role": "user", "turn": state.turn, "final": True,
                                          "text": "Tell me one long story about a lighthouse keeper."}))
                story = STORY + (" " + " ".join(MARKER_WORDS) if self.fault == "story-says-markers" else "")
                state.answering = asyncio.create_task(speak(state.turn, story, self.long_answer_s))

        try:
            async for raw in ws:
                if isinstance(raw, bytes):
                    await heard(raw)
                    continue
                frame = as_json(json.loads(raw))
                kind = frame.get("type")
                if kind == "audio_start" and not state.greeted:
                    state.greeted = True
                    state.turn += 1
                    state.answering = asyncio.create_task(speak(state.turn, "Hello from the fake bridge.", 1.5))
                elif kind == "replay":
                    await ws.send(json.dumps({"type": "replay_ack", "accepted": True}))
                elif kind in ("audio_end", "interrupt"):
                    if not await cut() and kind == "audio_end":
                        state.turn += 1
                        await ws.send(json.dumps({"type": "turn_complete", "turn": state.turn}))
                elif kind == "prompt":
                    state.turn += 1
                    state.prompts += 1
                    text = as_str(frame.get("text"))
                    await ws.send(json.dumps({"type": "transcript", "role": "user", "turn": state.turn,
                                              "text": text, "final": True}))
                    if self.loses(state, state.prompts == 1):
                        self.lost_turns += 1
                        state.answering = asyncio.create_task(watchdog(state.turn))
                        continue
                    if "BEGIN QUOTED MESSAGES" in text and self.fault == "ignore-relay":
                        answer, seconds = "Okay, noted.", 1.5
                    elif "BEGIN QUOTED MESSAGES" in text:
                        quoted = text.split("BEGIN QUOTED MESSAGES", 1)[1].split("END QUOTED MESSAGES", 1)[0]
                        answer, seconds = "New message. " + " ".join(quoted.split()), 3.0
                    elif "word for word, and nothing else:" in text:
                        answer, seconds = text.split("nothing else:", 1)[1].strip(), 6.0
                    elif "story" in text.lower():
                        answer = STORY + (" " + " ".join(MARKER_WORDS) if self.fault == "story-says-markers" else "")
                        seconds = self.long_answer_s
                    else:
                        answer, seconds = "Okay.", 1.0
                    state.answering = asyncio.create_task(speak(state.turn, answer, seconds))
                elif kind == "quit":
                    break
        except Exception:  # noqa: BLE001 - a test double; a dropped socket ends the session
            pass
        finally:
            if state.answering is not None and not state.answering.done():
                state.answering.cancel()

    def run(self) -> None:
        asyncio.set_event_loop(self.loop)
        from websockets.asyncio.server import serve

        async def main() -> None:
            async with serve(self.session, "127.0.0.1", self.port, max_size=None):
                self.ready.set()
                await asyncio.Future()

        try:
            self.loop.run_until_complete(main())
        except (RuntimeError, asyncio.CancelledError):
            pass

    def start(self) -> None:
        self.thread.start()
        if not self.ready.wait(10):
            raise RuntimeError("the fake bridge did not start")

    def stop(self) -> None:
        def cancel_all() -> None:
            for task in asyncio.all_tasks(self.loop):
                task.cancel()

        self.loop.call_soon_threadsafe(cancel_all)
        self.thread.join(timeout=5)


# ------------------------------------------------------------------------------------------------
# The page's websocket frames.


@dataclass
class Frame:
    """One page<->bridge websocket event, by the monotonic clock `t`."""

    t: float
    utc: str
    dir: str  # open, close, sent, recv
    socket: int
    binary: int | None = None
    rms: int | None = None
    raw: str | None = None
    type: str | None = None
    role: str | None = None
    turn: int | None = None
    final: bool | None = None
    interrupted: bool | None = None
    text: str | None = None
    message: str | None = None
    url: str | None = None

    def public(self, base: float) -> Json:
        out: Json = {key: value for key, value in asdict(self).items() if value is not None}
        out["t"] = round(self.t - base, 3)
        return out


def frame_from(direction: str, index: int, payload: str | bytes, at: float, utc: str,
               capture: bytearray | None = None) -> Frame:
    """What one frame on socket `index` says, kept whole when the page sent it."""
    frame = Frame(t=at, utc=utc, dir=direction, socket=index)
    if isinstance(payload, (bytes, bytearray)):
        frame.binary = len(payload)
        if capture is not None:
            capture.extend(payload)
        if direction == "sent":
            frame.rms = round(rms(bytes(payload)))  # the microphone's level, for the spoken turn
        return frame
    text = str(payload)
    if direction == "sent":
        # The WHOLE frame the page sent, so a relay is found whatever field carries it.
        frame.raw = text[:65536]
    try:
        parsed: object = json.loads(text)
    except ValueError:
        parsed = None
    data = as_json(parsed)
    if data:
        frame.type = as_str(data.get("type")) or None
        frame.role = as_str(data.get("role")) or None
        frame.turn = as_int(data.get("turn"))
        final, interrupted = data.get("final"), data.get("interrupted")
        frame.final = final if isinstance(final, bool) else None
        frame.interrupted = interrupted if isinstance(interrupted, bool) else None
        if "text" in data:
            frame.text = str(data.get("text"))
        if "message" in data:
            frame.message = str(data.get("message"))[:300]
    elif frame.raw is None:
        frame.raw = text[:300]
    return frame


@dataclass
class FrameLog:
    events: list[Frame] = field(default_factory=list)
    sockets: int = 0
    # The agent's audio of one turn, kept to replay as the microphone (the spoken long turn).
    capture: bytearray | None = None
    capture_socket: int = 0

    def attach(self, ws: WebSocket) -> None:
        url = str(ws.url)
        if not urlsplit(url).path.startswith("/ws"):
            return
        self.sockets += 1
        index = self.sockets
        self.events.append(Frame(t=time.monotonic(), utc=utc_now(), dir="open", socket=index,
                                 url=re.sub(r"\?.*$", "", url)))
        ws.on("framesent", lambda payload: self.add("sent", index, payload))
        ws.on("framereceived", lambda payload: self.add("recv", index, payload))
        ws.on("close", lambda _ws: self.events.append(Frame(t=time.monotonic(), utc=utc_now(), dir="close",
                                                            socket=index)))

    def add(self, direction: str, index: int, payload: str | bytes) -> None:
        keep = self.capture if direction == "recv" and index == self.capture_socket else None
        self.events.append(frame_from(direction, index, payload, time.monotonic(), utc_now(), keep))

    # Queries ------------------------------------------------------------------------------------
    def sent_with(self, marker: str, since: float = 0.0) -> list[Frame]:
        needle = marker.lower()
        return [e for e in self.events if e.dir == "sent" and e.t >= since and e.binary is None
                and needle in (e.raw or "").lower()]

    def heard_after_relay(self, marker: str, relay: Frame) -> list[Frame]:
        """Assistant transcripts saying `marker` on the relay's socket, after the relay and in a turn that
        had not begun before it: a story already running that happens to use the word is not the agent
        reading the message out."""
        needle = marker.lower()
        before = {e.turn for e in self.events if e.t < relay.t and e.dir == "recv"
                  and e.socket == relay.socket and e.turn is not None}
        return [e for e in self.events if e.dir == "recv" and e.t >= relay.t and e.socket == relay.socket
                and e.type == "transcript" and e.role == "assistant" and needle in (e.text or "").lower()
                and (e.turn is None or e.turn not in before)]

    def said_turns(self, marker: str, relays: list[Frame]) -> list[tuple[int, int]]:
        """The distinct turns, across sockets, in which the agent said `marker` after a relay on that
        socket: one is reading it out once, two is reading it out twice."""
        turns: list[tuple[int, int]] = []
        for relay in relays:
            for heard in self.heard_after_relay(marker, relay):
                key = (heard.socket, heard.turn if heard.turn is not None else -int(heard.t * 1000))
                if key not in turns:
                    turns.append(key)
        return turns

    def assistant_with(self, marker: str, since: float = 0.0) -> list[Frame]:
        needle = marker.lower()
        return [e for e in self.events if e.dir == "recv" and e.t >= since and e.type == "transcript"
                and e.role == "assistant" and needle in (e.text or "").lower()]

    def user_echo_with(self, marker: str, since: float = 0.0) -> list[Frame]:
        needle = marker.lower()
        return [e for e in self.events if e.dir == "recv" and e.t >= since and e.type == "transcript"
                and e.role == "user" and needle in (e.text or "").lower()]

    def first(self, predicate: Callable[[Frame], bool], since: float = 0.0) -> Frame | None:
        return next((e for e in self.events if e.t >= since and predicate(e)), None)

    def last_output_at(self) -> float:
        out = [e.t for e in self.events if e.dir == "recv" and
               (e.binary is not None or (e.type == "transcript" and e.role == "assistant"))]
        return out[-1] if out else 0.0

    def count(self, predicate: Callable[[Frame], bool], since: float = 0.0, until: float = float("inf")) -> int:
        return sum(1 for e in self.events if since <= e.t <= until and predicate(e))

    def pcm_seconds(self, since: float, until: float = float("inf"), socket_index: int | None = None) -> float:
        total = sum(e.binary or 0 for e in self.events if e.dir == "recv" and e.binary is not None
                    and since <= e.t <= until and (socket_index is None or e.socket == socket_index))
        return total / (2 * PCM_RATE)

    def sockets_opened(self, since: float, until: float = float("inf")) -> list[int]:
        return [e.socket for e in self.events if e.dir == "open" and since <= e.t <= until]


# ------------------------------------------------------------------------------------------------
# What a run records.


@dataclass
class LongAnswer:
    long_turn: str | None = None
    long_turn_note: str | None = None
    mic_loud_first_s_after_talk: float | None = None
    spoken_request_heard_s_after_talk: float | None = None
    spoken_request_text: str | None = None
    prompt_sent: bool | None = None
    answer_turn: int | None = None
    answer_started_s_after_prompt: float | None = None
    answer_completed: bool = False
    answer_interrupted: bool = False
    answer_s: float = 0.0
    answer_audio_s: float = 0.0
    answer_words: int = 0
    page_frames_sent_into_answer: list[Json] = field(default_factory=list)
    unattributed_cuts: list[str] = field(default_factory=list)
    playback_end_estimate_s_after_complete: float | None = None
    relay_window_truncated_by_budget: bool = False


@dataclass
class SpeechCapture:
    error: str | None = None
    audio_s: float = 0.0
    rms: int = 0
    agent_said: str | None = None
    wav: str | None = None


@dataclass
class CallInfo:
    index: int
    skipped: str | None = None
    error: str | None = None
    socket: int | None = None
    sockets: list[int] = field(default_factory=list)
    t_start: float | None = None
    t_end: float | None = None
    started_utc: str | None = None
    ended_utc: str | None = None
    duration_s: float | None = None
    socket_opened_utc: str | None = None
    session_started: bool = False
    greeting_completed: bool = False
    greeting_s: float | None = None
    view_during_cases: str = ""
    page_state: Json = field(default_factory=dict)
    page_error_after_greeting: str | None = None
    notices_after_hang_up: list[str] = field(default_factory=list)
    status_after_hang_up: str | None = None
    error_after_hang_up: str | None = None
    hang_up_error: str | None = None
    page_prompts_sent: int = 0
    budget_exceeded: bool = False
    microphone: str = "silence"
    speech_capture: SpeechCapture | None = None
    long_answer: LongAnswer | None = None
    ended_by: list[str] = field(default_factory=list)
    socket_closed_s_after_quit: float | None = None


@dataclass
class CaseResult:
    skipped: str | None = None
    call_error: str | None = None
    setup_error: str | None = None
    t_delivered: float | None = None
    delivered_utc: str | None = None
    socket: int | None = None
    push_status: int | None = None
    push_answer: object = None
    notices_before: list[str] = field(default_factory=list)
    long_answer_note: str | None = None
    delivered_s_into_answer: float | None = None
    bridge_error_frames: list[str] = field(default_factory=list)
    call_socket_closed_during_case: bool = False
    relay_frames_at_case_end: int | None = None
    notices_during_case: list[str] = field(default_factory=list)
    status_at_case_end: str | None = None
    relay_frames: int = 0
    relay_frames_later_calls: int = 0
    relay_frame_types: list[str] = field(default_factory=list)
    relay_latency_ms: int | None = None
    relay_frame_utc: list[str] = field(default_factory=list)
    relay_frame_text: str | None = None
    agent_said_marker: bool = False
    agent_said_turns: int = 0
    agent_said_latency_ms: int | None = None
    agent_text: str | None = None
    marker_in_assistant_text_outside_relay_turn: bool = False
    bridge_echoed_prompt: bool = False
    server_published_on: list[str] | None = None
    server_published_latency_ms: int | None = None
    window_truncated: str | None = None
    relay_before_answer_complete: bool = False
    cut_frames_for_this_case: list[str] = field(default_factory=list)
    notices_after_call: list[str] = field(default_factory=list)
    long_answer: LongAnswer | None = None
    # Case f, interrupt then relay.
    interrupt: str | None = None
    interrupt_s_before_delivery: float | None = None
    call_sockets: list[int] = field(default_factory=list)
    relay_frames_per_socket: dict[str, int] = field(default_factory=dict)
    lost_turn_errors: list[str] = field(default_factory=list)
    bridge_lost_turn: bool = False
    reconnected: bool = False
    reconnect_notice: bool = False
    replay_frames_after_reconnect: int = 0
    call_live_after: bool | None = None
    ended_by: list[str] = field(default_factory=list)
    # The verdict.
    verdict: str | None = None
    reason: str | None = None
    attribution: str | None = None
    notice_shown: bool = False
    notices: list[str] = field(default_factory=list)
    negative_control_scope: str | None = None
    long_turn_coverage: str | None = None


@dataclass
class Case:
    key: str
    title: str
    channel: str  # A, B or C
    expect_relay: bool
    marker: str
    call: int
    text: str = ""
    result: CaseResult = field(default_factory=CaseResult)


def public(value: object) -> object:
    """A record as JSON: dataclasses as objects, monotonic times (`t_…`) and absent values dropped."""
    if is_dataclass(value) and not isinstance(value, type):
        value = asdict(value)
    if isinstance(value, dict):
        return {str(key): public(item) for key, item in value.items()
                if item is not None and not str(key).startswith("t_")}
    if isinstance(value, list):
        return [public(item) for item in value]
    return value


# ------------------------------------------------------------------------------------------------
# What the page shows.


NOTICES_JS = """() => {
  const text = (id) => { const e = document.getElementById(id); return e ? (e.textContent || '') : ''; };
  const seams = [...document.querySelectorAll('#transcript li.seam')].map((li) => {
    const label = li.querySelector('.seam-label');
    const detail = li.querySelector('.seam-detail');
    return `${label ? label.textContent : ''}: ${detail ? detail.textContent : ''}`;
  });
  const err = document.getElementById('error');
  const hangUp = document.getElementById('hang-up');
  return {
    status: text('status'),
    connection_detail: text('connection-detail'),
    seams,
    error: err && !err.hidden ? (err.textContent || '') : '',
    hang_up_visible: !!(hangUp && !hangUp.hidden && hangUp.offsetParent),
  };
}"""

STATE_JS = """() => ({
  relay_mode: localStorage.getItem('vibe-talk.voice.relay'),
  read_new: (document.getElementById('read-new') || {}).getAttribute ? document.getElementById('read-new').getAttribute('data-mode') : null,
  relay_select: (document.getElementById('relay-to-agent') || {}).value || null,
  selected_channel: (document.getElementById('discord-channel') || {}).value || null,
  channel_options: [...document.querySelectorAll('#discord-channel option')].map((o) => o.value),
  voice_pane_visible: !!(document.getElementById('pane-voice') && document.getElementById('pane-voice').offsetParent),
  discord_pane_visible: !!(document.getElementById('pane-discord') && document.getElementById('pane-discord').offsetParent),
})"""


SET_RELAY_FULL_JS = """() => {
  const select = document.getElementById('relay-to-agent');
  select.value = 'full';
  select.dispatchEvent(new Event('change'));
}"""


def relay_setting_problem(state: Json) -> str | None:
    """The setting is READ BACK and required: the page keeps it in localStorage `vibe-talk.voice.relay`
    and draws it on the bar's Read new button and the Settings select; a stored value it does not know
    falls back to `gist`, so each is checked."""
    seen = {key: state.get(key) for key in ("relay_mode", "read_new", "relay_select")}
    if seen["relay_mode"] != "full" or seen["read_new"] != "full" or seen["relay_select"] not in ("full", None):
        return f"the relay setting is not 'full': {seen}"
    return None


def notice_lines(snapshot: Json) -> list[str]:
    lines: list[str] = []
    for seam in as_strs(snapshot.get("seams")):
        if NOTICE_RE.search(seam):
            lines.append(f"transcript: {seam.strip()}")
    for part in as_str(snapshot.get("connection_detail")).split(" · "):
        if NOTICE_RE.search(part):
            lines.append(f"connection details: {part.strip()}")
    status = as_str(snapshot.get("status"))
    if NOTICE_RE.search(status):
        lines.append(f"status line: {status.strip()}")
    error = as_str(snapshot.get("error"))
    if NOTICE_RE.search(error):
        lines.append(f"error: {error.strip()}")
    return lines


def reconnect_said(snapshot: Json) -> bool:
    """Does the page say it carried the call on a new session (`#352`)?"""
    parts = as_strs(snapshot.get("seams")) + as_str(snapshot.get("connection_detail")).split(" · ")
    return any(RECONNECT_NOTICE_RE.search(part.strip()) for part in parts)


def lines_for_channel(lines: list[str], label: str) -> list[str]:
    """Notice lines that can excuse a message in `label`: ones naming it, or naming no channel."""
    return [line for line in lines if label in line or not any(
        channel["label"] in line for channel in CHANNELS.values())]


def call_end_records(server_log: str, since_utc: str, until_utc: str) -> list[str]:
    """The `ended_by` of each call-end record the server logged between two UTC instants."""
    found: list[str] = []
    for raw in server_log.splitlines():
        line = ANSI.sub("", raw)
        stamp = re.match(r"(\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d(?:\.\d+)?)Z", line)
        ended = re.search(r"voice_call_ended .*?ended_by=(\w+)", line)
        if stamp and ended and since_utc[:23] <= stamp.group(1)[:23] <= until_utc[:23]:
            found.append(ended.group(1))
    return found


# ------------------------------------------------------------------------------------------------
# The walk.


class CallBudgetExceeded(Exception):
    pass


class Walk:
    def __init__(self, args: argparse.Namespace, run_dir: Path, origin: str, tokens: dict[str, str],
                 api: FakeChatApi, observer: StreamObserver | None = None) -> None:
        self.args = args
        self.run_dir = run_dir
        self.origin = origin
        self.tokens = tokens
        self.api = api
        self.observer = observer
        self.frames = FrameLog()
        self.page: Page | None = None
        self.calls: list[CallInfo] = []
        self.console: list[str] = []
        self.call_deadline = float("inf")
        self.calls_started = 0
        self.current_socket = 0
        self.current_call: CallInfo | None = None
        # The bridge's own voice saying the story request, at 48 kHz, once call 1 has captured it.
        self.request_samples: array.array[int] | None = None

    @property
    def tab(self) -> Page:
        if self.page is None:
            raise RuntimeError("no browser page")
        return self.page

    # Waiting --------------------------------------------------------------------------------------
    def tick(self, ms: int = 50) -> None:
        self.tab.wait_for_timeout(ms)
        if time.monotonic() > self.call_deadline:
            raise CallBudgetExceeded()

    def wait_until(self, predicate: Callable[[], bool], seconds: float) -> bool:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            if predicate():
                return True
            self.tick()
        return bool(predicate())

    def wait_quiet(self, quiet_s: float = 2.5, max_s: float = 25.0, since: float = 0.0) -> bool:
        """No assistant output for `quiet_s`, and every prompt sent since `since` answered."""
        def done() -> bool:
            prompts = self.frames.count(lambda e: e.dir == "sent" and e.type == "prompt", since)
            completes = self.frames.count(lambda e: e.dir == "recv" and e.type == "turn_complete", since)
            last = self.frames.last_output_at()
            return completes >= prompts and time.monotonic() - max(last, since) >= quiet_s
        return self.wait_until(done, max_s)

    def budget_left(self, reserve: float = 3.0) -> float:
        return max(0.0, self.call_deadline - time.monotonic() - reserve)

    def snapshot(self) -> Json:
        try:
            return as_json(self.tab.evaluate(NOTICES_JS))
        except Exception as error:  # noqa: BLE001 - a page mid-navigation
            return {"error": f"snapshot failed: {error}", "seams": []}

    def state(self) -> Json:
        return as_json(self.tab.evaluate(STATE_JS))

    def shot(self, name: str) -> None:
        try:
            self.tab.screenshot(path=str(self.run_dir / f"{name}.png"))
        except Exception:  # noqa: BLE001 - a screenshot is evidence, not the check
            pass

    def ensure_view(self, name: str) -> None:
        """`channel` or `voice`, through the bar's Voice/Channel switch, as the owner does."""
        state = self.state()
        visible = state.get("discord_pane_visible") if name == "channel" else state.get("voice_pane_visible")
        if not visible:
            self.tab.click("#view-switch", timeout=10_000)
            self.tab.wait_for_timeout(400)

    # Setup ----------------------------------------------------------------------------------------
    def sign_in_and_configure(self) -> Json:
        page = self.tab
        page.goto(f"{self.origin}/", wait_until="load")
        page.wait_for_selector("#api-token", state="visible", timeout=20_000)
        if self.args.replay:
            # The server offers replay; this device's Resuming switch is on, so a reconnected session
            # can be handed the call's record.
            page.evaluate("() => localStorage.setItem('vibe-talk.voice.resume', 'on')")
        page.fill("#api-token", self.tokens["write"])
        page.click("#save-token")
        page.wait_for_selector("#talk", state="visible", timeout=20_000)
        page.wait_for_function(
            "(id) => [...document.querySelectorAll('#discord-channel option')].some((o) => o.value === id)",
            arg=CHANNELS["A"]["id"], timeout=20_000)
        self.ensure_view("channel")
        page.select_option("#discord-channel", CHANNELS["A"]["id"])
        page.wait_for_timeout(500)
        # Settings > Voice > Arriving messages during a call: Read them out in full. Settings folds its
        # sections, so the select is set as its own change handler reads it, with Settings open.
        page.click("#open-settings")
        page.wait_for_selector("#relay-to-agent", state="attached", timeout=10_000)
        page.evaluate(SET_RELAY_FULL_JS)
        page.wait_for_timeout(300)
        in_settings = self.state()
        self.shot("00-settings")
        page.click("#close-settings")
        page.wait_for_timeout(500)
        self.ensure_view("channel")
        page.wait_for_timeout(1500)
        state = self.state()
        state["relay_select_in_settings"] = in_settings.get("relay_select")
        self.shot("01-channel-a-open")
        problem = relay_setting_problem({**state, "relay_select": in_settings.get("relay_select")})
        if problem:
            raise RuntimeError(f"setup: {problem}")
        if state.get("selected_channel") != CHANNELS["A"]["id"]:
            raise RuntimeError(f"setup: channel A is not the channel in view: {state.get('selected_channel')}")
        if CHANNELS["C"]["id"] in as_strs(state.get("channel_options")):
            raise RuntimeError("setup: the page still lists channel C, which was taken off the list")
        return state

    def resume_page(self) -> Json:
        """After the browser was relaunched with another microphone file (same profile): the page
        signed in, channel A in view, and the relay setting still 'full'."""
        page = self.tab
        page.goto(f"{self.origin}/", wait_until="load")
        page.wait_for_timeout(1500)
        if page.locator("#api-token").is_visible():
            page.fill("#api-token", self.tokens["write"])
            page.click("#save-token")
        # Signed in: the bar's Voice/Channel switch. Not Talk, which the channel view, restored from
        # the last call, replaces with Read.
        page.wait_for_selector("#view-switch", state="visible", timeout=20_000)
        page.wait_for_function(
            "(id) => [...document.querySelectorAll('#discord-channel option')].some((o) => o.value === id)",
            arg=CHANNELS["A"]["id"], timeout=20_000)
        self.ensure_view("channel")
        if self.state().get("selected_channel") != CHANNELS["A"]["id"]:
            page.select_option("#discord-channel", CHANNELS["A"]["id"])
        page.wait_for_timeout(1500)
        state = self.state()
        problem = relay_setting_problem({**state, "relay_select": None})
        if problem:
            raise RuntimeError(f"after the relaunch, {problem}")
        if state.get("selected_channel") != CHANNELS["A"]["id"]:
            raise RuntimeError("after the relaunch, channel A is not in view")
        return state

    # Calls ----------------------------------------------------------------------------------------
    def start_call(self, index: int, microphone: str) -> CallInfo:
        call = CallInfo(index=index, microphone=microphone)
        self.calls.append(call)
        self.current_call = call
        if self.calls_started >= self.args.max_calls:
            call.skipped = f"the run's budget of {self.args.max_calls} calls was spent; no call made"
            return call
        self.calls_started += 1
        sockets_before = self.frames.sockets
        self.current_socket = sockets_before + 1
        call.socket = self.current_socket
        call.t_start = time.monotonic()
        call.started_utc = utc_now()
        self.call_deadline = call.t_start + self.args.call_budget
        log(f"call {index}: Talk")
        self.ensure_view("voice")
        self.tab.click("#talk")
        if not self.wait_until(lambda: self.frames.sockets > sockets_before, 20):
            call.error = "the page opened no socket to the bridge"
            return call
        opened = self.frames.first(lambda e: e.dir == "open" and e.socket == sockets_before + 1)
        t_open = opened.t if opened else time.monotonic()
        call.socket_opened_utc = opened.utc if opened else None
        call.session_started = self.wait_until(lambda: self.frames.first(
            lambda e: e.dir == "recv" and e.type == "session_started", t_open) is not None, 30)
        call.greeting_completed = self.wait_until(lambda: self.frames.first(
            lambda e: e.dir == "recv" and e.type == "turn_complete", t_open) is not None, 40)
        first_complete = self.frames.first(lambda e: e.dir == "recv" and e.type == "turn_complete", t_open)
        if first_complete:
            call.greeting_s = round(first_complete.t - call.t_start, 1)
        # The page's capture hand-off after a turn_complete, and the greeting's audio to play out.
        self.wait_quiet(quiet_s=2.0, max_s=15, since=t_open)
        self.tab.wait_for_timeout(1500)
        if self.args.view_during_call == "channel":
            self.ensure_view("channel")
        call.view_during_cases = self.args.view_during_call
        state = self.state()
        call.page_state = state
        error = as_str(self.snapshot().get("error"))
        if error:
            call.page_error_after_greeting = error
        problem = relay_setting_problem({**state, "relay_select": None})
        if problem:
            call.error = f"at call start, {problem}"
        if not call.session_started:
            call.error = "the bridge never sent session_started"
        return call

    def end_call(self, call: CallInfo) -> None:
        if call.t_start is None or call.t_end is not None:
            return
        self.call_deadline = float("inf")
        try:
            if not self.tab.locator("#hang-up").is_visible():
                self.ensure_view("voice")
            if self.tab.locator("#hang-up").is_visible():
                self.tab.click("#hang-up", timeout=5_000)
            else:
                call.hang_up_error = "no visible Hang up button: the call had already ended"
        except Exception as error:  # noqa: BLE001
            call.hang_up_error = str(error)
        call.sockets = self.frames.sockets_opened(call.t_start)
        last = call.sockets[-1] if call.sockets else self.current_socket
        self.wait_until(lambda: any(e.dir == "close" and e.socket == last for e in self.frames.events), 8)
        call.t_end = time.monotonic()
        call.ended_utc = utc_now()
        call.duration_s = round(call.t_end - call.t_start, 1)
        self.tab.wait_for_timeout(2500)
        after = self.snapshot()
        call.notices_after_hang_up = notice_lines(after)
        call.status_after_hang_up = as_str(after.get("status"))
        call.error_after_hang_up = as_str(after.get("error"))
        call.page_prompts_sent = self.frames.count(
            lambda e: e.dir == "sent" and e.type == "prompt" and e.socket in call.sockets)
        log(f"call {call.index}: hung up after {call.duration_s} s on socket(s) {call.sockets}")

    def capture_speech(self, call: CallInfo) -> bool:
        """In call 1: have the bridge's voice say the story request, and keep its audio to play as the
        microphone in later calls. False if that did not work."""
        info = SpeechCapture()
        call.speech_capture = info
        sock = self.current_socket
        typed_at = time.monotonic()
        try:
            self.ensure_view("voice")
            self.tab.fill("#compose-text", SAY_PROMPT)
            self.tab.click("#send-text")
        except Exception as error:  # noqa: BLE001
            info.error = f"could not type the request: {error}"
            return False

        def is_say(e: Frame) -> bool:
            return (e.dir == "sent" and e.type == "prompt" and e.socket == sock
                    and "word for word, and nothing else" in (e.raw or ""))

        self.frames.capture = bytearray()
        self.frames.capture_socket = sock
        try:
            if not self.wait_until(lambda: self.frames.first(is_say, typed_at) is not None, 15):
                info.error = "the request never reached the bridge"
                return False
            said_frame = self.frames.first(is_say, typed_at)
            said_at = said_frame.t if said_frame else typed_at
            # Audio that arrived before the prompt went is not this turn's.
            del self.frames.capture[:]
            self.wait_until(lambda: self.frames.first(
                lambda e: e.dir == "recv" and e.socket == sock and e.type == "turn_complete", said_at + 0.05)
                is not None, min(20.0, self.budget_left(4)))
            pcm = bytes(self.frames.capture)
        finally:
            self.frames.capture = None
        said = [e.text or "" for e in self.frames.events if e.t >= said_at and e.socket == sock
                and e.type == "transcript" and e.role == "assistant"]
        info.audio_s = round(len(pcm) / (2 * PCM_RATE), 1)
        info.rms = round(rms(pcm))
        info.agent_said = said[-1][:300] if said else None
        if len(pcm) < 2 * PCM_RATE * 2 or rms(pcm) < 200:
            info.error = "too little audio to replay"
            return False
        if not said or not re.search(r"tell me", said[-1], re.I) or not re.search(r"story", said[-1], re.I):
            info.error = "the agent did not say the request"
            return False
        if len(pcm) > 2 * PCM_RATE * 20:
            info.error = "the agent said more than the request (over 20 s of audio)"
            return False
        # 24 kHz to 48 kHz by repeating each sample.
        samples = array.array("h")
        samples.frombytes(pcm[: len(pcm) - len(pcm) % 2])
        self.request_samples = array.array("h", (x for x in samples for _ in (0, 1)))
        info.wav = "captured"
        self.wait_quiet(quiet_s=1.5, max_s=min(8.0, self.budget_left(4)), since=said_at)
        return True

    def microphone_file(self, name: str, barge_in: bool) -> Path:
        """The captured request as the fake microphone: lead-in silence so it comes after the greeting,
        then the request, and for case f the first --barge-in-s of it again --barge-in-gap-s after it
        ends, inside the answer; then long silence, in case the browser loops the file anyway."""
        samples = self.request_samples
        if samples is None:
            raise RuntimeError("no spoken request was captured")
        path = self.run_dir / name
        with wave.open(str(path), "wb") as out:
            out.setnchannels(1)
            out.setsampwidth(2)
            out.setframerate(48_000)
            out.writeframes(b"\0\0" * int(48_000 * self.args.speech_lead_s))
            out.writeframes(samples.tobytes())
            if barge_in:
                out.writeframes(b"\0\0" * int(48_000 * self.args.barge_in_gap_s))
                out.writeframes(samples[: int(48_000 * self.args.barge_in_s)].tobytes())
            out.writeframes(b"\0\0" * 48_000 * 150)
        return path

    # Cases ----------------------------------------------------------------------------------------
    def message_text(self, case: Case) -> str:
        return (f"Heads up from the relay check: the {case.marker} build finished and is ready "
                f"for review in {CHANNELS[case.channel]['label']}.")

    def deliver(self, case: Case) -> None:
        case.text = self.message_text(case)
        before = notice_lines(self.snapshot())
        record = self.api.deliver(CHANNELS[case.channel]["id"], case.text)
        log(f"case {case.key}: delivered to {CHANNELS[case.channel]['label']} "
            f"(push HTTP {record['push_status']}), marker {case.marker!r}")
        moment = record.get("t")
        case.result.t_delivered = moment if isinstance(moment, float) else time.monotonic()
        case.result.delivered_utc = as_str(record.get("utc"))
        case.result.socket = self.current_socket
        case.result.push_status = as_int(record.get("push_status"))
        case.result.push_answer = record.get("push_answer")
        case.result.notices_before = before

    def observe(self, case: Case, window_s: float) -> None:
        """After delivery: the relay frame, then the agent saying the marker, then quiet."""
        t0 = case.result.t_delivered or 0.0
        if case.expect_relay:
            self.wait_until(lambda: bool(self.frames.sent_with(case.marker, t0)), window_s)
            sent = self.frames.sent_with(case.marker, t0)
            if sent:
                self.wait_until(lambda: bool(self.frames.heard_after_relay(case.marker, sent[0])),
                                self.args.spoken_timeout)
                self.wait_quiet(quiet_s=2.5, max_s=20, since=t0)
        else:
            self.wait_until(lambda: False, window_s)

    def record(self, case: Case) -> None:
        """What the page shows when the case's window ends. `finalize` recounts over the whole run, so a
        duplicate or a late relay in a later window, or in the next call, still counts against it."""
        t0 = case.result.t_delivered or 0.0
        sent = self.frames.sent_with(case.marker, t0)
        after = self.snapshot()
        new = [line for line in notice_lines(after) if line not in case.result.notices_before]
        call = self.current_call
        sockets = self.frames.sockets_opened(call.t_start) if call and call.t_start is not None else [self.current_socket]
        case.result.bridge_error_frames = [e.message or "" for e in self.frames.events
                                           if e.t >= t0 and e.dir == "recv" and e.type == "error" and e.socket in sockets]
        case.result.call_socket_closed_during_case = any(
            e.t >= t0 and e.dir == "close" and e.socket in sockets for e in self.frames.events)
        case.result.relay_frames_at_case_end = len(sent)
        case.result.notices_during_case = new
        case.result.status_at_case_end = as_str(after.get("status"))
        if case.key in INTERRUPT_CASES:
            case.result.reconnect_notice = reconnect_said(after)
            case.result.call_live_after = bool(after.get("hang_up_visible"))

    def finalize(self, case: Case, call: CallInfo | None) -> None:
        """The case's numbers over the WHOLE run: exactly once means once in its call — on whichever of
        the call's sockets — and never again, including in a later call; read out means an assistant
        turn after the relay, and read out once means one such turn."""
        t0 = case.result.t_delivered
        if t0 is None:
            return
        result = case.result
        sockets = call.sockets if call and call.sockets else ([result.socket] if result.socket else [])
        sent = self.frames.sent_with(case.marker, t0)
        in_call = [e for e in sent if e.socket in sockets]
        later = [e for e in sent if e.socket not in sockets]
        first_each = [next(e for e in in_call if e.socket == sock) for sock in sockets
                      if any(e.socket == sock for e in in_call)]
        turns = self.frames.said_turns(case.marker, first_each)
        heard = [h for relay in first_each for h in self.frames.heard_after_relay(case.marker, relay)]
        heard_any = self.frames.assistant_with(case.marker, t0)
        first = in_call[0] if in_call else None
        result.relay_frames = len(in_call)
        result.relay_frames_later_calls = len(later)
        result.relay_frame_types = [e.type or "" for e in in_call]
        result.relay_latency_ms = round((first.t - t0) * 1000) if first else None
        result.relay_frame_utc = [e.utc for e in in_call]
        result.relay_frame_text = (first.text or (first.raw or "")[:2000]) if first else None
        result.agent_said_marker = bool(heard)
        result.agent_said_turns = len(turns)
        result.agent_said_latency_ms = round((heard[0].t - t0) * 1000) if heard else None
        result.agent_text = heard[-1].text if heard else None
        result.marker_in_assistant_text_outside_relay_turn = bool(heard_any) and not heard
        result.bridge_echoed_prompt = bool(self.frames.user_echo_with(case.marker, t0))
        if self.observer is not None:
            seen = self.observer.seen(case.marker)
            result.server_published_on = sorted({as_str(s.get("stream")) for s in seen})
            moments = [stamp for stamp in (s.get("t") for s in seen) if isinstance(stamp, float)]
            result.server_published_latency_ms = round((min(moments) - t0) * 1000) if moments else None
        if case.key in INTERRUPT_CASES:
            result.call_sockets = sockets
            result.relay_frames_per_socket = {str(sock): sum(1 for e in in_call if e.socket == sock) for sock in sockets}
            result.lost_turn_errors = [e.message or "" for e in self.frames.events
                                       if e.t >= t0 and e.dir == "recv" and e.type == "error" and e.socket in sockets
                                       and RECONNECT_ASKED_RE.search(e.message or "")]
            result.bridge_lost_turn = bool(result.lost_turn_errors)
            after = [sock for sock in sockets if any(e.dir == "open" and e.socket == sock and e.t >= t0
                                                     for e in self.frames.events)]
            result.reconnected = bool(after)
            result.replay_frames_after_reconnect = sum(1 for e in self.frames.events
                                                       if e.dir == "sent" and e.type == "replay" and e.socket in after)
            result.ended_by = call.ended_by if call else []

    def run_case_idle(self, case: Case, window_s: float) -> None:
        self.deliver(case)
        self.observe(case, window_s)
        self.record(case)
        self.shot(f"case-{case.key}")

    def spoken_request(self, call: CallInfo, extra: LongAnswer) -> Frame | None:
        """Wait for the spoken request to be heard: the bridge's user transcript of it, or an answer that
        began after the microphone got loud with no prompt from the page in between."""
        sock = self.current_socket
        opened = self.frames.first(lambda e: e.dir == "open" and e.socket == sock)
        t_open = opened.t if opened else (call.t_start or 0.0)

        def is_spoken(e: Frame) -> bool:
            return (e.dir == "recv" and e.socket == sock and e.type == "transcript" and e.role == "user"
                    and SPOKEN_REQUEST_RE.search(e.text or "") is not None)

        greeting = self.frames.first(lambda e: e.dir == "recv" and e.socket == sock and e.type == "turn_complete", t_open)

        def loud() -> Frame | None:
            return self.frames.first(lambda e: e.dir == "sent" and e.socket == sock and (e.rms or 0) > 300, t_open)

        def spoken_answer() -> Frame | None:
            started = loud()
            if started is None:
                return None
            if self.frames.first(lambda e: e.dir == "sent" and e.socket == sock and e.type == "prompt", started.t):
                return None
            floor = greeting.turn if greeting and greeting.turn is not None else 0
            return self.frames.first(lambda e: e.dir == "recv" and e.socket == sock and e.type == "transcript"
                                     and e.role == "assistant" and (e.turn is None or e.turn > floor), started.t)

        wait = max(1.0, (call.t_start or 0.0) + self.args.speech_lead_s + 25 - time.monotonic())
        self.wait_until(lambda: self.frames.first(is_spoken, t_open) is not None or spoken_answer() is not None,
                        min(wait, self.budget_left(40)))
        if self.frames.first(is_spoken, t_open) is None:
            self.wait_until(lambda: self.frames.first(is_spoken, t_open) is not None, 2.0)
        echo = self.frames.first(is_spoken, t_open) or spoken_answer()
        first_loud = loud()
        extra.mic_loud_first_s_after_talk = round(first_loud.t - (call.t_start or 0.0), 1) if first_loud else None
        if echo is not None:
            extra.long_turn = "spoken"
            extra.spoken_request_heard_s_after_talk = round(echo.t - (call.t_start or 0.0), 1)
            extra.spoken_request_text = (echo.text or "")[:200]
            if echo.role == "assistant":
                extra.long_turn_note = "no user transcript of the spoken request; its answer began after it"
        return echo

    def run_long_answer(self, cases: list[Case], call: CallInfo) -> None:
        """Ask for a long answer, deliver each case into it (the first --mid-answer-delay in, each next
        --second-delivery-gap later), and watch every case through the answer's end, its playback, and
        the relay window."""
        page = self.tab
        sock = self.current_socket
        extra = LongAnswer()
        call.long_answer = extra
        for case in cases:
            case.result.long_answer = extra
        echo: Frame | None = None
        if call.microphone == "spoken request":
            echo = self.spoken_request(call, extra)
            if echo is None:
                extra.long_turn_note = "no spoken request was heard; fell back to a typed prompt"
        if echo is None:
            extra.long_turn = "typed"
            typed_at = time.monotonic()
            self.ensure_view("voice")
            page.fill("#compose-text", self.args.long_prompt)
            page.click("#send-text")
            if self.args.view_during_call == "channel":
                self.ensure_view("channel")

            def is_prompt(e: Frame) -> bool:
                return e.dir == "sent" and e.type == "prompt" and "lighthouse" in (e.raw or "").lower()

            extra.prompt_sent = self.wait_until(lambda: self.frames.first(is_prompt, typed_at) is not None, 20)
            prompt_frame = self.frames.first(is_prompt, typed_at)
            if not prompt_frame:
                for case in cases:
                    case.result.setup_error = "the typed prompt never reached the bridge"
                return
            prompt_at = prompt_frame.t
            sock = prompt_frame.socket

            def is_echo(e: Frame) -> bool:
                return (e.dir == "recv" and e.socket == sock and e.type == "transcript" and e.role == "user"
                        and "lighthouse" in (e.text or "").lower())

            self.wait_until(lambda: self.frames.first(is_echo, prompt_at) is not None, 5)
            echo = self.frames.first(is_echo, prompt_at)
        else:
            prompt_at = echo.t
        answer_turn = echo.turn if echo else None
        extra.answer_turn = answer_turn

        def is_output(e: Frame) -> bool:
            return e.dir == "recv" and e.socket == sock and (
                e.binary is not None or (e.type == "transcript" and e.role == "assistant"))

        self.wait_until(lambda: self.frames.first(is_output, prompt_at) is not None, 20)
        start = self.frames.first(is_output, prompt_at)
        if not start:
            for case in cases:
                case.result.setup_error = "the agent never began answering the long prompt"
            return
        answer_at = start.t
        extra.answer_started_s_after_prompt = round(answer_at - prompt_at, 2)

        def is_complete(e: Frame) -> bool:
            return (e.dir == "recv" and e.socket == sock and e.type == "turn_complete"
                    and (answer_turn is None or e.turn is None or e.turn >= answer_turn))

        for index, case in enumerate(cases):
            target = answer_at + self.args.mid_answer_delay + index * self.args.second_delivery_gap
            self.wait_until(lambda: self.frames.first(is_complete, prompt_at) is not None
                            or time.monotonic() >= target, max(0.0, target - time.monotonic()) + 0.2)
            if self.frames.first(is_complete, prompt_at) is not None:
                case.result.long_answer_note = "the answer completed before this case's delivery point"
            case.result.delivered_s_into_answer = round(time.monotonic() - answer_at, 1)
            self.deliver(case)
        self.wait_until(lambda: self.frames.first(is_complete, prompt_at) is not None, max(1.0, self.budget_left(2)))
        complete = self.frames.first(is_complete, prompt_at)
        completed_at = complete.t if complete else time.monotonic()
        extra.answer_completed = complete is not None
        extra.answer_interrupted = bool(complete and complete.interrupted)
        extra.answer_s = round(completed_at - answer_at, 1)
        extra.answer_audio_s = round(self.frames.pcm_seconds(answer_at, completed_at, sock), 1)
        texts = [e.text or "" for e in self.frames.events if answer_at <= e.t <= completed_at
                 and e.type == "transcript" and e.role == "assistant" and e.socket == sock]
        extra.answer_words = len(texts[-1].split()) if texts else 0
        first_delivery = min(case.result.t_delivered or answer_at for case in cases)
        # Every frame the page sent into the answer after the first delivery, with which case's marker
        # the prompt after it carried, so a cut is put down to the case that caused it.
        into: list[Json] = []
        for e in [e for e in self.frames.events if e.dir == "sent" and e.socket == sock
                  and first_delivery <= e.t < completed_at and e.type in ("audio_end", "interrupt", "prompt")]:
            following = next((p for p in self.frames.events if p.dir == "sent" and p.socket == sock
                              and p.type == "prompt" and e.t <= p.t <= e.t + 3.0), None)
            carried = [case.key for case in cases if following and case.marker in (following.raw or "").lower()]
            into.append({"type": e.type, "s_into_answer": round(e.t - answer_at, 1), "cases": carried})
        extra.page_frames_sent_into_answer = into
        extra.unattributed_cuts = [str(f["type"]) for f in into
                                   if f["type"] in ("audio_end", "interrupt") and not f["cases"]]
        # The page plays the answer as it arrives and may still be playing it after its turn_complete;
        # a relay that waits for the answer to be heard out goes after that.
        play_end = max(completed_at, answer_at + extra.answer_audio_s)
        extra.playback_end_estimate_s_after_complete = round(play_end - completed_at, 1)
        until = min(play_end + self.args.relay_timeout, self.call_deadline - 3)
        extra.relay_window_truncated_by_budget = play_end + self.args.relay_timeout > self.call_deadline - 3

        def all_sent() -> bool:
            return all(self.frames.sent_with(case.marker, case.result.t_delivered or 0.0) for case in cases)

        self.wait_until(all_sent, max(0.5, until - time.monotonic()))

        def all_heard() -> bool:
            for case in cases:
                relays = self.frames.sent_with(case.marker, case.result.t_delivered or 0.0)
                if relays and not self.frames.heard_after_relay(case.marker, relays[0]):
                    return False
            return True

        self.wait_until(all_heard, min(self.budget_left(3), self.args.spoken_timeout))
        # Settle, so a second relay of the same message before the hang-up is seen.
        self.wait_quiet(quiet_s=3.0, max_s=min(self.budget_left(3), 12.0), since=first_delivery)
        for case in cases:
            self.record(case)
            relays = self.frames.sent_with(case.marker, case.result.t_delivered or 0.0)
            if not relays and extra.relay_window_truncated_by_budget:
                case.result.window_truncated = "the call budget ended the relay window early"
            case.result.relay_before_answer_complete = bool(relays and complete and relays[0].t < complete.t)
            case.result.cut_frames_for_this_case = [str(f["type"]) for f in into
                                                    if case.key in as_strs(f["cases"])
                                                    and f["type"] in ("audio_end", "interrupt")]
        self.shot("case-long-answer")

    def run_interrupt_relay(self, case: Case, call: CallInfo) -> None:
        """Case f: the spoken request, the bridge's barge-in on its answer, and a message a moment after
        the interrupted turn — then what came of it on every socket the call used."""
        sock = self.current_socket
        extra = LongAnswer()
        call.long_answer = extra
        case.result.long_answer = extra
        echo = self.spoken_request(call, extra)
        if echo is None:
            case.result.setup_error = "no spoken request was heard, so there was no answer to interrupt"
            return
        answer = self.frames.first(lambda e: e.dir == "recv" and e.socket == sock and (
            e.binary is not None or (e.type == "transcript" and e.role == "assistant")), echo.t)
        answer_at = answer.t if answer else echo.t

        def cut() -> Frame | None:
            """An interrupted turn_complete after the answer began, or one that ended a turn with under a
            second of audio and no text: the bridge cut the answer, flagged or not."""
            def is_cut(e: Frame) -> bool:
                if e.dir != "recv" or e.socket != sock or e.type != "turn_complete" or e.t < answer_at:
                    return False
                if e.interrupted:
                    return True
                said = any(f.dir == "recv" and f.socket == sock and f.type == "transcript" and f.role == "assistant"
                           and answer_at <= f.t <= e.t and (f.text or "").strip() for f in self.frames.events)
                return not said and self.frames.pcm_seconds(answer_at, e.t, sock) < 1.0
            return self.frames.first(is_cut, answer_at)

        self.wait_until(lambda: cut() is not None,
                        min(self.args.barge_in_gap_s + self.args.barge_in_s + 20.0, self.budget_left(30)))
        interrupted = cut()
        if interrupted is None:
            case.result.setup_error = ("the bridge did not cut the answer: no interrupted turn followed the "
                                       "barge-in, so there was no interrupt to relay after")
            return
        case.result.interrupt = "interrupted" if interrupted.interrupted else "short turn with no text"
        log(f"case f: the answer was cut ({case.result.interrupt}); a message in "
            f"{self.args.interrupt_relay_delay} s")
        self.wait_until(lambda: False, self.args.interrupt_relay_delay)
        self.deliver(case)
        case.result.interrupt_s_before_delivery = round((case.result.t_delivered or 0.0) - interrupted.t, 2)
        t0 = case.result.t_delivered or 0.0
        self.wait_until(lambda: bool(self.frames.sent_with(case.marker, t0)), self.args.relay_timeout)

        def settled() -> bool:
            relays = self.frames.sent_with(case.marker, t0)
            heard = any(self.frames.heard_after_relay(case.marker, relay) for relay in relays)
            return heard or not bool(self.snapshot().get("hang_up_visible"))

        # A lost turn takes the bridge's watchdog (10 s on the bridge it was seen on), a reconnect, and
        # maybe a greeting before the message goes again.
        self.wait_until(settled, min(self.budget_left(4), self.args.spoken_timeout + 30.0))
        self.wait_quiet(quiet_s=3.0, max_s=min(self.budget_left(4), 12.0), since=t0)
        self.record(case)
        self.shot("case-interrupt-relay")


# ------------------------------------------------------------------------------------------------
# Verdicts.


def judge_unrunnable(case: Case) -> bool:
    """The verdicts that come before anything the page did: SKIP, or ERROR when the harness could not
    test the case. Answers whether one was given."""
    result = case.result
    if result.skipped:
        result.verdict, result.reason = "SKIP", result.skipped
        return True
    if result.call_error:
        result.verdict, result.reason, result.attribution = "ERROR", f"call: {result.call_error}", "harness"
        return True
    if result.setup_error and result.t_delivered is None:
        result.verdict, result.reason = "ERROR", f"setup: {result.setup_error}"
        result.attribution = "harness/bridge"
        return True
    if result.t_delivered is None:
        result.verdict = "ERROR"
        result.reason = "never delivered (the call ended or failed before this case)"
        result.attribution = "harness"
        return True
    if case.key in LONG_CASES and result.long_answer_note:
        result.verdict = "ERROR"
        result.reason = f"setup: {result.long_answer_note}; this case did not test a long answer"
        result.attribution = "harness/agent"
        return True
    push = result.push_status
    if case.expect_relay and push is not None and not 200 <= push < 300:
        result.verdict = "ERROR"
        result.reason = f"the fake's push was refused by the server (HTTP {push}); the page was never tested"
        result.attribution = "fake/harness"
        return True
    return False


def silent_drop(result: CaseResult, reasons: list[str], attribution: list[str]) -> None:
    """Not relayed and nothing said: where the evidence points."""
    reasons.append("not relayed and no notice on the page: silently dropped")
    published = result.server_published_on
    if published is None:
        attribution.append("page?")
    elif published:
        reasons.append(f"(the server published it on stream(s) {published} "
                       f"{result.server_published_latency_ms} ms after delivery; the page sent nothing)")
        attribution.append("page")
    else:
        reasons.append("(the server accepted the push but the harness saw it on no server stream)")
        attribution.append("server")
    if result.bridge_error_frames or result.call_socket_closed_during_case:
        reasons.append(f"(the call ended under this case: bridge errors {result.bridge_error_frames})")
        attribution.append("bridge")


def judge(case: Case, call_notices: list[str]) -> None:
    """PASS / FAIL / SKIP / ERROR, with `attribution`: which side the evidence points at."""
    result = case.result
    label = CHANNELS[case.channel]["label"]
    result.attribution = None
    if judge_unrunnable(case):
        return
    if case.key in INTERRUPT_CASES:
        judge_interrupt(case, call_notices)
        return
    push = result.push_status
    count = result.relay_frames
    later = result.relay_frames_later_calls
    notices = list(dict.fromkeys(lines_for_channel(
        result.notices_during_case + result.notices_after_call + call_notices, label)))
    result.notice_shown = bool(notices)
    result.notices = notices
    if not case.expect_relay:
        if count == 0 and later == 0 and not result.marker_in_assistant_text_outside_relay_turn:
            result.verdict = "PASS"
            if push == 404:
                result.reason = ("not relayed. Scope: the server's ingest refused it (HTTP 404 unknown_channel), "
                                 "so it never reached the page; this proves the server's filter, not the page's")
                result.negative_control_scope = "server-ingest"
            else:
                result.reason = f"not relayed; the server accepted the push (HTTP {push}) and the page did not relay it"
                result.negative_control_scope = "page"
            if result.server_published_on and "all" in result.server_published_on:
                result.verdict = "FAIL"
                result.reason = "the server carried an unwatched channel's message on the stream of every channel"
                result.attribution = "server"
        else:
            result.verdict = "FAIL"
            result.reason = f"an unwatched channel's message was relayed {count + later} time(s)"
            result.attribution = "page"
        return
    reasons: list[str] = []
    attribution: list[str] = []
    passed_by_notice = ""
    if count == 1:
        if not result.agent_said_marker:
            reasons.append("relayed once, but no assistant turn after the relay said the marker")
            attribution.append("bridge/agent")
        elif result.agent_said_turns > 1:
            reasons.append(f"relayed once, but read out in {result.agent_said_turns} turns")
            attribution.append("bridge/agent")
    elif count > 1:
        reasons.append(f"relayed {count} times in its call (must be exactly once)")
        attribution.append("page")
    elif notices:
        passed_by_notice = "not relayed, and the page said why: " + notices[0][:160]
    elif result.window_truncated:
        result.verdict = "ERROR"
        result.reason = f"not relayed, but {result.window_truncated}: no verdict"
        result.attribution = "harness"
        return
    else:
        silent_drop(result, reasons, attribution)
    if later:
        reasons.append(f"relayed again in a later call ({later} frame(s)): stale news, not exactly once")
        attribution.append("page")
    long = result.long_answer
    if case.key in LONG_CASES and long is not None:
        if result.relay_before_answer_complete:
            reasons.append("the relay went before the long answer's turn_complete (cut into the answer)")
            attribution.append("page")
        if result.cut_frames_for_this_case:
            reasons.append(f"the page sent {result.cut_frames_for_this_case} into the long answer for this message")
            attribution.append("page")
        if long.unattributed_cuts:
            reasons.append(f"the page sent {long.unattributed_cuts} into the long answer, carrying no case's "
                           "message (not attributable to one case)")
            attribution.append("page")
    result.attribution = ", ".join(dict.fromkeys(attribution)) or None
    if reasons:
        result.verdict, result.reason = "FAIL", "; ".join(reasons)
    elif passed_by_notice:
        result.verdict, result.reason = "PASS", passed_by_notice
    else:
        result.verdict, result.reason = "PASS", "relayed exactly once and the agent read it out"
    if case.key in LONG_CASES and long is not None and result.verdict == "PASS":
        if long.answer_s < 30:
            result.reason = f"{result.reason} (note: the answer lasted only {long.answer_s} s, under the 30 s aimed for)"
        if long.long_turn != "spoken":
            result.reason = (f"{result.reason} (note: the long answer was asked for by a TYPED prompt, which the "
                             "page holds a relay behind with no time cap; the spoken-turn hold was not exercised)")
            result.long_turn_coverage = "typed only"


def judge_interrupt(case: Case, call_notices: list[str]) -> None:
    """Case f, interrupt then relay. A bridge may lose the turn after an interrupt and send an error that
    asks to reconnect; either way the message must be read out exactly once, or the page must say why
    not. Whether the bridge lost a turn is recorded on every verdict: it is the bridge's defect even
    when the page recovers."""
    result = case.result
    label = CHANNELS[case.channel]["label"]
    notices = list(dict.fromkeys(lines_for_channel(
        result.notices_during_case + result.notices_after_call + call_notices, label)))
    result.notice_shown = bool(notices)
    result.notices = notices
    total = result.relay_frames
    said = result.agent_said_turns
    reasons: list[str] = []
    blame: list[str] = []
    passed = ""
    if not result.bridge_lost_turn:
        if total == 1 and said == 1:
            passed = "relayed once and read out once; the bridge lost no turn this time"
        elif total == 0 and notices:
            passed = "not relayed, and the page said why: " + notices[0][:160]
        elif total == 0:
            silent_drop(result, reasons, blame)
        elif total > 1:
            reasons.append(f"relayed {total} times in its call (must be exactly once)")
            blame.append("page")
        elif said == 0:
            reasons.append("relayed once, but no assistant turn after the relay said the marker")
            blame.append("bridge/agent")
        else:
            reasons.append(f"relayed once, but read out in {said} turns")
            blame.append("page")
    elif not result.reconnected:
        reasons.append("the bridge lost the turn and asked to reconnect, and the call ended there: the page "
                       "opened no new session")
        blame.append("page")
    else:
        lost_socket = result.call_sockets[0] if result.call_sockets else 0
        again = sum(n for sock, n in result.relay_frames_per_socket.items() if sock != str(lost_socket))
        ended_early = any(word in ("reconnect_limit", "reconnect_failed") for word in result.ended_by)
        if said > 1:
            reasons.append(f"read out in {said} turns across the call's sessions (must be once)")
            blame.append("page")
        elif again > 1:
            reasons.append(f"sent {again} times to the reconnected session (must be once)")
            blame.append("page")
        elif said == 1 and again == 1:
            if not result.reconnect_notice:
                reasons.append("read out once after the reconnect, but the page never said it reconnected")
                blame.append("page")
            if result.call_live_after is False and not ended_early:
                reasons.append("the call was no longer live after the reconnect")
                blame.append("page")
            if result.ended_by and "reconnected" not in result.ended_by:
                reasons.append(f"the server has no record of the replaced socket: {result.ended_by}")
                blame.append("page")
            passed = "the bridge lost the turn; the page reconnected, and the message was read out once on the new session"
        elif said == 0 and ended_early:
            if notices:
                passed = ("the bridge lost the turn again after the reconnect; the call ended saying why, and the "
                          f"page said the message was not read out: {notices[0][:120]}")
            else:
                reasons.append("the call ended after a second lost turn without saying the message was not read out")
                blame.append("page")
        elif again == 0 and notices:
            passed = "not sent again after the reconnect, and the page said why: " + notices[0][:160]
        elif again == 0:
            reasons.append("the lost relay was neither sent again nor said not to have been read out")
            blame.append("page")
        else:
            reasons.append("sent again once after the reconnect, but no assistant turn said the marker")
            blame.append("bridge/agent")
    if result.relay_frames_later_calls:
        reasons.append(f"relayed again in a later call ({result.relay_frames_later_calls} frame(s))")
        blame.append("page")
    if result.bridge_lost_turn:
        blame.append("bridge (lost a turn)")
    result.attribution = ", ".join(dict.fromkeys(blame)) or None
    if reasons:
        result.verdict, result.reason = "FAIL", "; ".join(reasons)
    else:
        result.verdict, result.reason = "PASS", passed


# ------------------------------------------------------------------------------------------------
# The server under test, and the processes this check starts.


def binary_identity(path: Path) -> Json:
    resolved = path.resolve()
    digest = hashlib.sha256(resolved.read_bytes()).hexdigest()
    return {"path": str(path), "resolved": str(resolved), "sha256": digest, "short": digest[:12]}


def write_private(path: Path, text: str) -> None:
    fd = os.open(path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
    with os.fdopen(fd, "w") as handle:
        handle.write(text)


def silence_wav(path: Path) -> None:
    with wave.open(str(path), "wb") as out:
        out.setnchannels(1)
        out.setsampwidth(2)
        out.setframerate(48_000)
        out.writeframes(b"\0\0" * 48_000 * 5)


def toml_string(value: str) -> str:
    return json.dumps(value)


def write_config(run_dir: Path, vt_port: int, api_port: int, tokens: dict[str, str], probe_url: str | None) -> Path:
    """vibe-talk's config for the run: the fake chat API, the bridge at the page's own `/ws`, push
    ingest, and storage in the run directory. With `probe_url`, replay is on and the server asks the
    bridge there whether it takes a recap."""
    channels = "\n".join(
        f'[[channels]]\nid = "{c["id"]}"\nlabel = "{c["label"]}"\nwritable = false\n' for c in CHANNELS.values())
    replay = f"\n[replay]\nenabled = true\n" if probe_url else ""
    probe = f"websocket_url = {toml_string(probe_url)}\n" if probe_url else ""
    config = run_dir / "vibe-talk.toml"
    write_private(config, f"""# A loopback instance for the live relay check: a fake chat API, the bridge under test.
[server]
bind = "127.0.0.1:{vt_port}"
timezone = "UTC"

[discord]
provider_name = "Relay check chat"
bot_token = {toml_string(tokens['bot'])}
api_base = "http://127.0.0.1:{api_port}"
owner_user_id = "{OWNER_ID}"
thread_api = "off"
live_poll_seconds = 0
request_timeout_seconds = 10

[auth]
read_token = {toml_string(tokens['read'])}
write_token = {toml_string(tokens['write'])}

[ingest]
token = {toml_string(tokens['ingest'])}

{channels}
[conversation]
ask_before_send = true
backend = "websocket"
websocket_url = "/ws"
label = "Bridge under test"

[read_aloud]
backend = "browser"
{probe}{replay}
[storage]
path = {toml_string(str(run_dir / "state" / "vibe-talk.sqlite3"))}
""")
    return config


def descendants(pid: int) -> list[int]:
    children: dict[int, list[int]] = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdigit():
            continue
        try:
            stat = (entry / "stat").read_text()
            ppid = int(stat.rsplit(")", 1)[1].split()[1])
        except (OSError, ValueError, IndexError):
            continue
        children.setdefault(ppid, []).append(int(entry.name))
    found: list[int] = []
    stack = [pid]
    while stack:
        for child in children.get(stack.pop(), []):
            found.append(child)
            stack.append(child)
    return found


def is_zombie(pid: int) -> bool:
    try:
        return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[0] == "Z"
    except (OSError, IndexError):
        return True  # gone


def process_name(pid: int) -> str:
    try:
        return Path(f"/proc/{pid}/comm").read_text().strip()
    except OSError:
        return "?"


# ------------------------------------------------------------------------------------------------
# The run.


def parser() -> argparse.ArgumentParser:
    result = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    result.add_argument("--bridge", help="the vibe-talk-v1 bridge: a ws:// or wss:// address (tools= is refused), "
                                         "or `fake` for the in-process stand-in. Required for a run")
    result.add_argument("--binary", type=Path, default=DEFAULT_BINARY,
                        help="the vibe-talk binary to test (default: this checkout's target/release/vibe-talk; "
                             "build it first, since the page is compiled into it)")
    result.add_argument("--cases", default="a,b,c,d,e,f", help="which cases to run (default a,b,c,d,e,f)")
    result.add_argument("--max-calls", type=int, default=3, help="most calls this run may start (default 3)")
    result.add_argument("--call-budget", type=float, default=110.0, help="seconds a call may last (default 110)")
    result.add_argument("--relay-timeout", type=float, default=20.0, help="seconds to wait for a relay frame")
    result.add_argument("--spoken-timeout", type=float, default=25.0, help="seconds to wait for the agent to say it")
    result.add_argument("--negative-window", type=float, default=15.0, help="seconds to watch the unwatched case")
    result.add_argument("--mid-answer-delay", type=float, default=8.0,
                        help="seconds into the long answer to deliver case c (default 8)")
    result.add_argument("--second-delivery-gap", type=float, default=4.0,
                        help="seconds after case c to deliver case e into the same long answer (default 4)")
    result.add_argument("--interrupt-relay-delay", type=float, default=1.0,
                        help="case f: seconds after the interrupted turn to deliver the message (default 1; "
                             "a few seconds measures whether a settle after an interrupt avoids a lost turn)")
    result.add_argument("--barge-in-gap-s", type=float, default=5.0,
                        help="case f: seconds after the spoken request ends that the barge-in starts (default 5)")
    result.add_argument("--barge-in-s", type=float, default=1.0,
                        help="case f: seconds of the request replayed as the barge-in (default 1)")
    result.add_argument("--long-prompt", default=LONG_PROMPT, help="typed prompt that makes the agent talk >30 s")
    result.add_argument("--long-turn", choices=["spoken", "typed"], default="spoken",
                        help="how call 2 asks for the long answer: out loud, replaying the bridge's own voice "
                             "captured in call 1 (default; falls back to typed), or typed. Case f needs spoken")
    result.add_argument("--speech-lead-s", type=float, default=9.0,
                        help="seconds of silence before the spoken request in the microphone file")
    result.add_argument("--replay", action="store_true",
                        help="turn on the server's replay and this page's Resuming switch, so a reconnected "
                             "session that lists replay is handed the call's record; the server asks the bridge "
                             "at startup and the run waits for its yes")
    result.add_argument("--fake-long-answer", type=float, default=40.0, help="fake bridge: long answer seconds")
    result.add_argument("--fake-fault", choices=FAKE_FAULTS, default="none",
                        help="fake bridge: misbehave, to check the verdicts (default none)")
    result.add_argument("--fake-watchdog-s", type=float, default=2.0,
                        help="fake bridge: seconds before a lost turn's error (default 2)")
    result.add_argument("--view-during-call", choices=["channel", "voice"], default="channel",
                        help="what is on screen while the cases run: channel A's view (default) or the call's")
    result.add_argument("--out", type=Path, default=None,
                        help="the run directory (default: debug/live-relay-check/<UTC>-<binary>)")
    result.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"))
    result.add_argument("--self-test", action="store_true",
                        help="check the verdict logic on synthetic frame logs; no browser, server or bridge")
    return result


def build_cases(wanted: set[str]) -> list[Case]:
    markers = random.sample(MARKER_WORDS, 6)
    cases = [
        Case("a", "open channel A, idle", "A", True, markers[0], 1),
        Case("b", "watched channel B (not in view), idle", "B", True, markers[1], 1),
        Case("d", "unwatched channel C (negative control)", "C", False, markers[3], 1),
        Case("c", "watched channel B during a long answer", "B", True, markers[2], 2),
        Case("e", "open channel A during a long answer", "A", True, markers[4], 2),
        Case("f", "watched channel B a moment after the bridge cut an answer", "B", True, markers[5], 3),
    ]
    return [case for case in cases if case.key in wanted]


def wait_for_server(server: subprocess.Popen[bytes], port: int, run_dir: Path) -> None:
    deadline = time.monotonic() + 60
    while True:
        if server.poll() is not None:
            raise RuntimeError(f"vibe-talk exited with {server.returncode}; see {run_dir / 'server.log'}")
        try:
            status, _ = http("GET", f"http://127.0.0.1:{port}/healthz", timeout=2)
            if status == 200:
                return
        except OSError:
            pass
        if time.monotonic() > deadline:
            raise RuntimeError("vibe-talk never answered /healthz")
        time.sleep(0.3)


def run_calls(args: argparse.Namespace, walk: Walk, cases: list[Case], launch: Callable[[Path | None], BrowserContext],
              silence: Path, results: Json) -> None:
    """Every call the cases need, one at a time, each hung up."""
    context = launch(silence)
    try:
        results["page_state"] = walk.sign_in_and_configure()
        spoken_needed = args.long_turn == "spoken" and any(case.call > 1 for case in cases)
        captured = False
        for index in sorted({case.call for case in cases} | ({1} if spoken_needed else set())):
            in_call = [case for case in cases if case.call == index]
            microphone = "silence"
            if index == 3 and not captured:
                for case in in_call:
                    case.result.setup_error = "no spoken request was captured in call 1, so nothing can barge in"
                continue
            if index > 1 and captured:
                context.close()
                file = walk.microphone_file("barge-in-request.wav" if index == 3 else "spoken-request.wav",
                                            barge_in=index == 3)
                context = launch(file)
                walk.resume_page()
                microphone = "barge-in request" if index == 3 else "spoken request"
                log(f"browser relaunched with the {microphone} as the microphone")
            call = walk.start_call(index, microphone)
            if call.skipped:
                for case in in_call:
                    case.result.skipped = call.skipped
                continue
            if call.error:
                for case in in_call:
                    case.result.call_error = call.error
                walk.end_call(call)
                continue
            try:
                for case in [c for c in in_call if c.key not in LONG_CASES + INTERRUPT_CASES]:
                    walk.run_case_idle(case, args.relay_timeout if case.expect_relay else args.negative_window)
                long_cases = [c for c in in_call if c.key in LONG_CASES]
                if long_cases:
                    walk.run_long_answer(long_cases, call)
                for case in [c for c in in_call if c.key in INTERRUPT_CASES]:
                    walk.run_interrupt_relay(case, call)
                if index == 1 and spoken_needed:
                    captured = walk.capture_speech(call)
                    log(f"spoken request captured: {public(call.speech_capture)}")
            except CallBudgetExceeded:
                log(f"call {index}: budget of {args.call_budget} s reached; hanging up")
                call.budget_exceeded = True
                walk.call_deadline = float("inf")
                walk.frames.capture = None
                for case in in_call:
                    if case.result.t_delivered is not None and case.result.relay_frames_at_case_end is None:
                        walk.record(case)
                        case.result.window_truncated = "the call budget ran out during this case"
            finally:
                walk.end_call(call)
            for case in in_call:
                if case.result.t_delivered is not None:
                    late = notice_lines(walk.snapshot())
                    case.result.notices_after_call = [line for line in late if line not in case.result.notices_before]
        walk.shot("99-end")
    finally:
        context.close()


def main(argv: list[str] | None = None) -> int:
    options = parser()
    args = options.parse_args(argv)
    if args.self_test:
        return self_test()
    if not args.bridge:
        options.error("--bridge is required: a ws:// or wss:// address, or `fake`")
    try:
        bridge = parse_bridge(args.bridge)
    except ValueError as error:
        options.error(str(error))
    wanted = {key.strip() for key in args.cases.split(",") if key.strip()}
    if not wanted <= {"a", "b", "c", "d", "e", "f"}:
        options.error(f"unknown cases: {sorted(wanted - set('abcdef'))}")
    if "f" in wanted and args.long_turn != "spoken":
        options.error("case f needs --long-turn spoken: its barge-in is the bridge's own voice")
    if not args.binary.is_file():
        options.error(f"no vibe-talk binary at {args.binary}: build it (cargo build --release) or pass --binary")
    from playwright.sync_api import sync_playwright

    binary = binary_identity(args.binary)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    run_dir: Path = args.out or VIBE_TALK / "debug" / "live-relay-check" / f"{stamp}-{binary['short']}"
    run_dir.mkdir(parents=True, mode=0o700)
    (run_dir / "state").mkdir(mode=0o700)
    tokens = {
        "read": secrets.token_urlsafe(32), "write": secrets.token_urlsafe(32), "ingest": secrets.token_urlsafe(32),
        "bot": base64.urlsafe_b64encode(BOT_ID.encode()).decode().rstrip("=") + "." + secrets.token_urlsafe(24),
    }
    write_private(run_dir / "tokens.json", json.dumps(tokens))
    log(f"binary {binary['resolved']} ({binary['short']}), bridge {bridge.describe()}, run dir {run_dir}")
    cases = build_cases(wanted)

    api = FakeChatApi()
    api.seed()
    api_port, vt_port, front_port = free_port(), free_port(), free_port()
    api.ingest_url = f"http://127.0.0.1:{vt_port}/api/v1/live/events"
    api.ingest_token = tokens["ingest"]
    chat_server = ThreadingHTTPServer(("127.0.0.1", api_port), chat_handler(api))
    chat_server.daemon_threads = True
    chat_thread = threading.Thread(target=chat_server.serve_forever, daemon=True)
    chat_thread.start()

    fake: FakeBridge | None = None
    probe_url: str | None = args.bridge if args.replay and not bridge.fake else None
    if bridge.fake:
        fake_port = free_port()
        fake = FakeBridge(fake_port, args.fake_long_answer, args.fake_fault, args.fake_watchdog_s, args.replay)
        fake.start()
        bridge = Bridge(fake=True, host="127.0.0.1", port=fake_port, target="/ws")
        if args.replay:
            probe_url = f"ws://127.0.0.1:{fake_port}/ws"
    front = Front(front_port, ("127.0.0.1", vt_port), bridge)
    front.start()
    origin = f"http://127.0.0.1:{front_port}"

    config = write_config(run_dir, vt_port, api_port, tokens, probe_url)
    env = {key: value for key, value in os.environ.items()
           if not key.startswith("VIBE_TALK_") and key.lower() not in ("http_proxy", "https_proxy", "all_proxy")}
    env["RUST_LOG"] = "info"
    server_log = (run_dir / "server.log").open("wb")
    server = subprocess.Popen([str(binary["resolved"]), "--config", str(config), "--skip-startup-probe"],
                              cwd=run_dir, stdout=server_log, stderr=subprocess.STDOUT, env=env,
                              start_new_session=True)
    pids_started: list[Json] = [{"pid": server.pid, "what": "vibe-talk server under test"}]
    log(f"vibe-talk pid {server.pid} on 127.0.0.1:{vt_port}; fake chat API :{api_port}; front :{front_port}")

    results: Json = {
        "harness": {"version": HARNESS_VERSION, "script": "vibe-talk/scripts/live-relay-check.py"},
        "binary": binary, "started_utc": utc_now(), "run_dir": str(run_dir),
        "bridge": bridge.describe() if not bridge.fake else f"fake (fault {args.fake_fault})",
        "channels": {key: {**value, "watched": key != "C", "in_view": key == "A"} for key, value in CHANNELS.items()},
        "settings": {"relay": "full", "call_budget_s": args.call_budget, "long_turn": args.long_turn,
                     "speech_lead_s": args.speech_lead_s, "replay": args.replay,
                     "interrupt_relay_delay_s": args.interrupt_relay_delay},
    }
    observer: StreamObserver | None = None
    walk: Walk | None = None
    harness_error = False
    try:
        wait_for_server(server, vt_port, run_dir)
        # C is configured, then taken off the list, as the owner removes a channel he does not watch.
        status, answer = http("DELETE", f"http://127.0.0.1:{vt_port}/api/v1/channels/{CHANNELS['C']['id']}",
                              tokens["write"])
        _, listed = http("GET", f"http://127.0.0.1:{vt_port}/api/v1/channels", tokens["read"])
        rows = as_json(listed).get("channels", listed)
        listed_ids = sorted(as_str(as_json(row).get("id")) for row in (rows if isinstance(rows, list) else []))
        results["unwatch_c"] = {"delete_status": status, "listed": listed_ids}
        if CHANNELS["C"]["id"] in listed_ids:
            raise RuntimeError(f"could not take channel C off the list (DELETE {status}: {answer})")
        if args.replay:
            offered = False
            for _ in range(60):
                config_now = as_json(http("GET", f"http://127.0.0.1:{vt_port}/api/v1/client-config", tokens["write"])[1])
                if config_now.get("replay_enabled") is True:
                    offered = True
                    break
                time.sleep(0.5)
            if not offered:
                raise RuntimeError("--replay: the server never offered replay; the bridge did not list `replay` "
                                   "when the server asked it at startup")
        observer = StreamObserver(vt_port, tokens["read"], [case.marker for case in cases])
        for key, channel in CHANNELS.items():
            observer.start(key, f"/api/v1/channels/{channel['id']}/stream")
        observer.start("all", "/api/v1/live/stream")

        silence = run_dir / "silence.wav"
        silence_wav(silence)
        walk = Walk(args, run_dir, origin, tokens, api, observer)
        known_pids = {server.pid}

        with sync_playwright() as playwright:
            def launch(mic_file: Path | None, playwright: Playwright = playwright) -> BrowserContext:
                """The browser on the fake microphone playing `mic_file`, on the run's own profile."""
                assert walk is not None
                mic_args = ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream",
                            "--autoplay-policy=no-user-gesture-required"]
                if mic_file is not None:
                    once = "" if mic_file == silence else "%noloop"
                    mic_args.append(f"--use-file-for-fake-audio-capture={mic_file}{once}")
                context = playwright.chromium.launch_persistent_context(
                    str(run_dir / "chrome-profile"), headless=True, executable_path=args.browser_executable,
                    args=mic_args,
                    user_agent=("Mozilla/5.0 (Linux; Android 15; Pixel 7) AppleWebKit/537.36 "
                                "(KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36"),
                    viewport={"width": 412, "height": 915}, device_scale_factor=2.625, is_mobile=True, has_touch=True)
                for pid in descendants(os.getpid()):
                    if pid not in known_pids:
                        known_pids.add(pid)
                        pids_started.append({"pid": pid, "what": f"playwright/browser ({process_name(pid)})"})
                context.grant_permissions(["microphone"], origin=origin)
                page = context.pages[0] if context.pages else context.new_page()
                walk.page = page
                page.on("websocket", walk.frames.attach)
                console = walk.console

                def page_error(error: Error) -> None:
                    console.append(f"pageerror: {error}")

                page.on("pageerror", page_error)
                return context

            run_calls(args, walk, cases, launch, silence, results)
    except Exception as error:  # noqa: BLE001 - reported as a harness error, never a page verdict
        harness_error = True
        results["harness_error"] = f"{type(error).__name__}: {error}"
        results["harness_traceback"] = traceback.format_exc()[-3000:]
        log(f"HARNESS ERROR: {results['harness_error']}")
    finally:
        if server.poll() is None:
            os.kill(server.pid, signal.SIGTERM)
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                os.kill(server.pid, signal.SIGKILL)
                server.wait(timeout=5)
        server_log.close()
        front.stop()
        if fake is not None:
            fake.stop()
        chat_server.shutdown()
        chat_server.server_close()
        for pid in descendants(os.getpid()):
            try:
                os.kill(pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
        for _ in range(50):
            if not [pid for pid in descendants(os.getpid()) if not is_zombie(pid)]:
                break
            time.sleep(0.1)
        shutil.rmtree(run_dir / "chrome-profile", ignore_errors=True)
        results["pids_started"] = pids_started
        results["pids_left_after_cleanup"] = [pid for pid in descendants(os.getpid()) if not is_zombie(pid)]

    return report(args, results, cases, walk, observer, api, front, run_dir, harness_error, fake)


def report(args: argparse.Namespace, results: Json, cases: list[Case], walk: Walk | None,
           observer: StreamObserver | None, api: FakeChatApi, front: Front, run_dir: Path,
           harness_error: bool, fake: FakeBridge | None) -> int:
    """Judge every case over the whole run, write the results, print the table, and answer the exit code."""
    server_log = (run_dir / "server.log").read_text(errors="replace") if (run_dir / "server.log").exists() else ""
    calls = walk.calls if walk else []
    for call in calls:
        if call.started_utc and call.ended_utc:
            call.ended_by = call_end_records(server_log, call.started_utc, call.ended_utc)
    if walk is not None and not harness_error:
        call_of = {call.index: call for call in calls}
        for case in cases:
            its_call = call_of.get(case.call)
            walk.finalize(case, its_call)
            judge(case, its_call.notices_after_hang_up if its_call else [])
    for case in cases:
        if case.result.verdict is None:
            case.result.verdict = "ERROR"
            case.result.reason = str(results.get("harness_error", "not run"))
            case.result.attribution = "harness"
    if walk is not None:
        for call in calls:
            sockets = call.sockets
            quit_frame = next((e for e in walk.frames.events
                               if e.dir == "sent" and e.type == "quit" and e.socket in sockets), None)
            close = next((e for e in walk.frames.events
                          if e.dir == "close" and bool(sockets) and e.socket == sockets[-1]), None)
            call.socket_closed_s_after_quit = round(close.t - quit_frame.t, 1) if quit_frame and close else None
        base = walk.frames.events[0].t if walk.frames.events else 0.0
        write_private(run_dir / "frames.jsonl",
                      "\n".join(json.dumps(event.public(base), ensure_ascii=False) for event in walk.frames.events) + "\n")
        results["page_console"] = walk.console[-60:]
        results["calls_started"] = walk.calls_started
    results["finished_utc"] = utc_now()
    results["calls"] = public(calls)
    results["front_voice_sockets"] = front.voice_sockets
    if fake is not None:
        results["fake_bridge"] = {"sessions": fake.sessions, "lost_turns": fake.lost_turns,
                                  "spoken_requests": fake.spoken_requests, "barge_ins": fake.barge_ins}
    results["cases"] = [{"case": case.key, "title": case.title, "channel": CHANNELS[case.channel]["label"],
                         "watched": case.channel != "C", "in_view": case.channel == "A",
                         "expect_relay": case.expect_relay, "marker": case.marker, "call": case.call,
                         "message_text": case.text, **as_json(public(case.result))}
                        for case in sorted(cases, key=lambda c: c.key)]
    results["fake_chat_api"] = {"deliveries": [{k: v for k, v in d.items() if k != "t"} for d in api.deliveries],
                                "posts_attempted": api.posts,
                                "requests_404": [r for r in api.requests if r.get("status") == 404][:50],
                                "requests_total": len(api.requests)}
    if observer is not None:
        results["server_stream_observer"] = observer.summary()
    verdicts = [case.result.verdict for case in cases]
    overall = "PASS" if verdicts and all(v == "PASS" for v in verdicts) and not harness_error else "FAIL"
    results["overall"] = overall
    out_path = run_dir / "results.json"
    write_private(out_path, json.dumps(results, indent=2, ensure_ascii=False, default=str))

    print()
    print(f"Relays during a call (read in full) - binary {as_json(results.get('binary')).get('short')} - "
          f"bridge {results.get('bridge')}")
    header = (f"{'case':<4} {'channel':<11} {'verdict':<7} {'relayed':>7} {'latency':>9} {'read out':>8} "
              f"{'notice':>6} {'blame':<20} reason")
    print(header)
    print("-" * len(header))
    for case in sorted(cases, key=lambda c: c.key):
        r = case.result
        latency = f"{r.relay_latency_ms} ms" if r.relay_latency_ms is not None else "-"
        print(f"{case.key:<4} {CHANNELS[case.channel]['label']:<11} {r.verdict or '?':<7} {r.relay_frames:>7} "
              f"{latency:>9} {r.agent_said_turns:>8} {('yes' if r.notice_shown else 'no'):>6} "
              f"{str(r.attribution or '-')[:20]:<20} {(r.reason or '').splitlines()[0][:150] if r.reason else ''}")
    print(f"\noverall: {overall}   results: {out_path}")
    if harness_error or any(v == "ERROR" for v in verdicts):
        return EXIT_ERROR
    return EXIT_OK if overall == "PASS" else EXIT_FAIL


# ------------------------------------------------------------------------------------------------
# The verdict logic, offline.


def self_test() -> int:
    """Synthetic frame logs through `finalize` and `judge`: each scenario is one way a check like this
    could PASS vacuously or FAIL for the wrong reason, and case f's ways through a lost turn."""
    checks: list[tuple[str, bool, object]] = []

    def check(name: str, condition: bool, detail: object) -> None:
        checks.append((name, condition, detail))

    def walk_with(events: list[tuple[float, str, int, str | bytes]], seen: list[Json] | None = None) -> Walk:
        walk = Walk.__new__(Walk)
        walk.frames = FrameLog()
        walk.observer = None
        if seen is not None:
            observer = StreamObserver(0, "", [])
            observer.sightings = seen
            walk.observer = observer
        for at, direction, sock, payload in events:
            if direction in ("open", "close"):
                walk.frames.events.append(Frame(t=at, utc=f"2026-10-19T00:00:{at:06.3f}Z", dir=direction, socket=sock))
            else:
                walk.frames.events.append(frame_from(direction, sock, payload, at, f"2026-10-19T00:00:{at:06.3f}Z"))
        return walk

    def prompt(text: str) -> str:
        return json.dumps({"type": "prompt", "text": text})

    def said(role: str, turn: int, text: str, final: bool = False) -> str:
        return json.dumps({"type": "transcript", "role": role, "turn": turn, "text": text, "final": final})

    def done(turn: int) -> str:
        return json.dumps({"type": "turn_complete", "turn": turn})

    def relay(*markers: str) -> str:
        body = "\n".join(f"in qa-x, Dana Q said: the {m} build finished" for m in markers)
        return prompt(f"Read this new chat message out loud.\nBEGIN QUOTED MESSAGES\n{body}\nEND QUOTED MESSAGES")

    def lost(turn: int) -> str:
        return json.dumps({"type": "error", "message": f"turn {turn} never started after an interrupt; reconnect"})

    def case(key: str, channel: str, marker: str, expect: bool = True, call: int = 1, **fields: object) -> Case:
        made = Case(key, key, channel, expect, marker, call)
        made.result.t_delivered = 10.0
        made.result.socket = 1
        made.result.push_status = 202
        for name, value in fields.items():
            setattr(made.result, name, value)
        return made

    def on(*sockets: int, ended_by: list[str] | None = None) -> CallInfo:
        return CallInfo(index=1, sockets=list(sockets), ended_by=ended_by or [])

    # 1. A duplicate relay in the NEXT call is stale news.
    w = walk_with([(11.2, "sent", 1, relay("kumquat")), (12.0, "recv", 1, said("user", 3, "kumquat")),
                   (13.0, "recv", 1, said("assistant", 3, "Dana said the kumquat build finished")), (14.0, "recv", 1, done(3)),
                   (70.0, "sent", 2, relay("kumquat")), (72.0, "recv", 2, said("assistant", 3, "kumquat again"))])
    c = case("b", "B", "kumquat")
    w.finalize(c, on(1))
    judge(c, [])
    check("a duplicate in a later call is a FAIL", c.result.verdict == "FAIL" and "later call" in (c.result.reason or ""),
          (c.result.verdict, c.result.reason))

    # 2. A late relay (25 s, in the next case's window) is counted once, with its latency.
    w = walk_with([(35.0, "sent", 1, relay("zeppelin")), (37.0, "recv", 1, said("assistant", 5, "the zeppelin build"))])
    c = case("b", "B", "zeppelin", relay_frames_at_case_end=0)
    w.finalize(c, on(1))
    judge(c, [])
    check("a late relay is counted once, with its latency",
          c.result.verdict == "PASS" and c.result.relay_latency_ms == 25000, (c.result.verdict, c.result.relay_latency_ms))

    # 3. The story itself says the marker; the relay after it is not read out.
    w = walk_with([(5.0, "recv", 2, said("assistant", 3, "the keeper fed a tamarind to the gull")),
                   (40.0, "recv", 2, said("assistant", 3, "the keeper fed a tamarind to the gull and slept")),
                   (41.0, "recv", 2, done(3)), (43.0, "sent", 2, relay("tamarind")),
                   (44.0, "recv", 2, said("assistant", 4, "Okay, noted.")), (45.0, "recv", 2, done(4))])
    c = case("c", "B", "tamarind", call=2, socket=2, t_delivered=8.0, long_answer=LongAnswer(answer_s=36))
    w.finalize(c, on(2))
    judge(c, [])
    check("a story saying the marker is not reading it out",
          c.result.verdict == "FAIL" and not c.result.agent_said_marker, (c.result.verdict, c.result.reason))

    # 4. A notice about ANOTHER channel does not excuse a drop.
    w = walk_with([])
    c = case("b", "B", "saffron", notices_during_case=[
        "transcript: not read out: 1 message, in qa-alpha, was still waiting when the call ended"])
    w.finalize(c, on(1))
    judge(c, [])
    check("a notice naming another channel does not excuse a drop", c.result.verdict == "FAIL", (c.result.verdict, c.result.reason))

    # 5. Status lines that are not notices, and the page's notices that are.
    for line in ["could not read that message aloud: synthesis failed", "the live feed fell behind — re-reading the channel.",
                 "Could not read All — try again."]:
        check(f"status {line[:30]!r} is not a notice",
              not notice_lines({"status": line, "seams": [], "connection_detail": "", "error": ""}), line)
    for line in ["not read out: 1 new message, in qa-bravo, was still waiting when the call ended, so the agent never read it out.",
                 "not read out: 1 new message, in qa-bravo, went to the agent, but the call ended before it answered, so it was never read out.",
                 "not read out: 1 new message, in qa-bravo, went to the agent, but the voice service lost the turn, so it was never read out.",
                 "this server cannot stream every channel at once, so a call reads out new messages only from the channel on screen"]:
        check(f"notice {line[13:50]!r} is a notice",
              bool(notice_lines({"status": "", "seams": [line], "connection_detail": "", "error": ""})), line)
    check("the reconnected line says the page reconnected",
          reconnect_said({"seams": ["reconnected: The voice service lost a turn, so the call moved to a new session"],
                          "connection_detail": ""}), "seam")
    check("the connection details say the page reconnected",
          reconnect_said({"seams": [], "connection_detail": "voice session started · reconnected after the voice service "
                                                            "lost a turn: turn 4 never started after an interrupt; reconnect"}),
          "details")
    check("an ordinary line is not a reconnect", not reconnect_said({"seams": ["new conversation: x"], "connection_detail": "a"}),
          "plain")

    # 6. A relay carried in a field the check did not expect is still found.
    w = walk_with([(11.0, "sent", 1, json.dumps({"type": "context", "items": [{"content": "the nutmeg build finished"}]})),
                   (12.0, "recv", 1, said("assistant", 4, "the nutmeg build finished"))])
    c = case("b", "B", "nutmeg")
    w.finalize(c, on(1))
    judge(c, [])
    check("a relay in any field is found", c.result.relay_frames == 1, (c.result.relay_frames, c.result.verdict))

    # 7. The fake's push refused: the page was never tested.
    w = walk_with([])
    c = case("b", "B", "cardamom", push_status=401)
    w.finalize(c, on(1))
    judge(c, [])
    check("a refused push is ERROR (fake/harness), not a page FAIL",
          c.result.verdict == "ERROR" and c.result.attribution == "fake/harness", (c.result.verdict, c.result.attribution))

    # 8. A silent drop is put down to the side the server's stream points at.
    w = walk_with([], [{"marker": "pistachio", "stream": "B", "t": 10.01}])
    c = case("b", "B", "pistachio")
    w.finalize(c, on(1))
    judge(c, [])
    check("published by the server, the page silent: page", c.result.attribution == "page", (c.result.attribution, c.result.reason))
    w = walk_with([], [])
    c = case("b", "B", "pistachio")
    w.finalize(c, on(1))
    judge(c, [])
    check("never published by the server: server", c.result.attribution == "server", (c.result.attribution, c.result.reason))

    # 9. Read out twice, in two turns, is not exactly once.
    w = walk_with([(11.0, "sent", 1, relay("accordion")), (12.0, "recv", 1, said("assistant", 4, "the accordion build", True)),
                   (13.0, "recv", 1, done(4)), (15.0, "recv", 1, said("assistant", 5, "again, the accordion build", True))])
    c = case("a", "A", "accordion")
    w.finalize(c, on(1))
    judge(c, [])
    check("read out in two turns is a FAIL", c.result.verdict == "FAIL" and c.result.agent_said_turns == 2,
          (c.result.verdict, c.result.reason))

    # 10. Two cases in one long answer; e's cut is not blamed on c.
    long = LongAnswer(answer_s=43)
    c_case = case("c", "B", "rutabaga", call=2, socket=2, t_delivered=8.0, long_answer=long)
    e_case = case("e", "A", "kohlrabi", call=2, socket=2, t_delivered=12.0, cut_frames_for_this_case=["audio_end"],
                  relay_before_answer_complete=True, long_answer=long)
    w = walk_with([(43.0, "sent", 2, json.dumps({"type": "audio_end"})), (43.0, "sent", 2, relay("kohlrabi")),
                   (45.0, "recv", 2, said("assistant", 4, "the kohlrabi build"))], [{"marker": "rutabaga", "stream": "B", "t": 8.01}])
    for each in (c_case, e_case):
        w.finalize(each, on(2))
        judge(each, [])
    check("c is a silent drop without e's cut",
          c_case.result.verdict == "FAIL" and "into the long answer" not in (c_case.result.reason or ""), c_case.result.reason)
    check("e is a cut into the answer",
          e_case.result.verdict == "FAIL" and "cut into the answer" in (e_case.result.reason or ""), e_case.result.reason)

    # 11. d refused at ingest: a PASS, scoped to the server.
    w = walk_with([], [])
    d = case("d", "C", "trombone", expect=False, push_status=404)
    w.finalize(d, on(1))
    judge(d, [])
    check("d refused at ingest is a server-scoped PASS",
          d.result.verdict == "PASS" and d.result.negative_control_scope == "server-ingest", d.result.reason)

    # 12. A case never delivered is ERROR, not a silent drop.
    undelivered = Case("e", "e", "A", True, "kumquat", 2)
    judge(undelivered, [])
    check("an undelivered case is ERROR", undelivered.result.verdict == "ERROR", undelivered.result.reason)

    # Case f, interrupt then relay.
    def interrupted(*extra: tuple[float, str, int, str | bytes], seen: list[Json] | None = None) -> Walk:
        """Socket 1: the answer cut, the message relayed at 11 s and echoed as turn 4."""
        base: list[tuple[float, str, int, str | bytes]] = [
            (0.0, "open", 1, ""), (8.0, "recv", 1, json.dumps({"type": "turn_complete", "turn": 2, "interrupted": True})),
            (11.0, "sent", 1, json.dumps({"type": "audio_end"})), (11.1, "recv", 1, done(3)),
            (11.2, "sent", 1, relay("marmalade")), (11.3, "recv", 1, said("user", 4, "marmalade", True))]
        return walk_with(base + list(extra), seen)

    def f_case(**fields: object) -> Case:
        made = case("f", "B", "marmalade", call=3, interrupt="interrupted", reconnect_notice=True, call_live_after=True)
        for name, value in fields.items():
            setattr(made.result, name, value)
        return made

    w = interrupted((12.0, "recv", 1, said("assistant", 5, "Dana said the marmalade build finished", True)), (13.0, "recv", 1, done(5)))
    f = f_case(reconnect_notice=False)
    w.finalize(f, on(1, ended_by=["hang_up"]))
    judge(f, [])
    check("f: no lost turn, read out once: PASS", f.result.verdict == "PASS" and not f.result.bridge_lost_turn, f.result.reason)

    reconnected: list[tuple[float, str, int, str | bytes]] = [
        (21.3, "recv", 1, lost(4)), (21.4, "sent", 1, json.dumps({"type": "quit"})), (21.5, "close", 1, ""),
        (22.0, "open", 2, ""), (22.1, "recv", 2, json.dumps({"type": "session_started", "greeting": True})),
        (26.0, "sent", 2, relay("marmalade")), (26.1, "recv", 2, said("user", 3, "marmalade", True))]
    w = interrupted(*reconnected, (27.0, "recv", 2, said("assistant", 3, "Dana said the marmalade build finished", True)),
                    (28.0, "recv", 2, done(3)))
    f = f_case()
    w.finalize(f, on(1, 2, ended_by=["reconnected", "hang_up"]))
    judge(f, [])
    check("f: lost turn, reconnected, read out once on the new session: PASS, the bridge blamed",
          f.result.verdict == "PASS" and f.result.bridge_lost_turn and "bridge (lost a turn)" in (f.result.attribution or ""),
          (f.result.verdict, f.result.reason, f.result.attribution))

    w = interrupted((21.3, "recv", 1, lost(4)), (21.4, "sent", 1, json.dumps({"type": "quit"})), (21.5, "close", 1, ""))
    f = f_case(reconnect_notice=False, call_live_after=False)
    w.finalize(f, on(1, ended_by=["error_frame"]))
    judge(f, [])
    check("f: the call ended at the lost turn's error: FAIL, the page",
          f.result.verdict == "FAIL" and "page" in (f.result.attribution or "") and "no new session" in (f.result.reason or ""),
          (f.result.verdict, f.result.reason))

    w = interrupted(*reconnected, (27.0, "recv", 2, said("assistant", 3, "the marmalade build finished", True)),
                    (28.0, "recv", 2, done(3)), (30.0, "recv", 2, said("assistant", 4, "and again, marmalade", True)))
    f = f_case()
    w.finalize(f, on(1, 2, ended_by=["reconnected", "hang_up"]))
    judge(f, [])
    check("f: read out twice across the reconnect: FAIL", f.result.verdict == "FAIL" and "2 turns" in (f.result.reason or ""),
          f.result.reason)

    w = interrupted((21.3, "recv", 1, lost(4)), (22.0, "open", 2, ""),
                    (22.1, "recv", 2, json.dumps({"type": "session_started", "greeting": False})))
    f = f_case()
    w.finalize(f, on(1, 2, ended_by=["reconnected", "hang_up"]))
    judge(f, [])
    check("f: reconnected, but the lost relay neither sent again nor said not read out: FAIL",
          f.result.verdict == "FAIL" and "neither sent again" in (f.result.reason or ""), f.result.reason)

    w = interrupted(*reconnected, (28.0, "recv", 2, lost(3)))
    f = f_case(call_live_after=False, notices_after_call=[
        "transcript: not read out: 1 new message, in qa-bravo, went to the agent, but the call ended before it answered, "
        "so it was never read out."])
    w.finalize(f, on(1, 2, ended_by=["reconnected", "reconnect_limit"]))
    judge(f, [])
    check("f: lost again within the minute, ended saying the message was not read out: PASS",
          f.result.verdict == "PASS" and "lost the turn again" in (f.result.reason or ""), (f.result.verdict, f.result.reason))

    w = interrupted(*reconnected, (27.0, "recv", 2, said("assistant", 3, "the marmalade build finished", True)),
                    (28.0, "recv", 2, done(3)))
    f = f_case(reconnect_notice=False)
    w.finalize(f, on(1, 2, ended_by=["reconnected", "hang_up"]))
    judge(f, [])
    check("f: reconnected and read out, but never said it reconnected: FAIL",
          f.result.verdict == "FAIL" and "never said it reconnected" in (f.result.reason or ""), f.result.reason)

    w = interrupted(*reconnected, (27.0, "recv", 2, said("assistant", 3, "the marmalade build finished", True)),
                    (28.0, "recv", 2, done(3)))
    f = f_case()
    w.finalize(f, on(1, 2, ended_by=["error_frame", "hang_up"]))
    judge(f, [])
    check("f: no server record of the replaced socket: FAIL",
          f.result.verdict == "FAIL" and "no record of the replaced socket" in (f.result.reason or ""), f.result.reason)

    never = Case("f", "f", "B", True, "marmalade", 3)
    never.result.setup_error = "the bridge did not cut the answer"
    judge(never, [])
    check("f: no interrupt, so nothing delivered: ERROR", never.result.verdict == "ERROR"
          and never.result.attribution == "harness/bridge", (never.result.verdict, never.result.attribution))

    # The safety rules and the plumbing.
    for refused in ["ws://127.0.0.1:9100/ws?mode=voice&tools=chat", "ws://example.invalid/x?Tools=chat",
                    "ws://example.invalid/x?mode=a&%74ools=x", "ws://example.invalid/x?%2574OOLS=x",
                    "ws://example.invalid/x?mode=a&tools", "http://127.0.0.1:9100/ws", "127.0.0.1:9100"]:
        try:
            parse_bridge(refused)
            check(f"{refused!r} is refused", False, "accepted")
        except ValueError as error:
            check(f"{refused!r} is refused", True, str(error))
    bridge = parse_bridge("wss://voice.example.test/session?mode=a")
    check("a wss address is taken whole", (bridge.host, bridge.port, bridge.target, bridge.tls)
          == ("voice.example.test", 443, "/session?mode=a", True), bridge)
    check("fake is the stand-in", parse_bridge("fake").fake, "fake")
    front = Front(0, ("127.0.0.1", 1), Bridge(fake=False, host="127.0.0.1", port=9100, target="/voice?mode=b"))
    rewritten = front.rewritten(b"GET /ws HTTP/1.1\r\nHost: 127.0.0.1:41000\r\nUpgrade: websocket\r\n\r\n")
    check("the front sends /ws to the bridge's own path and host",
          rewritten == b"GET /voice?mode=b HTTP/1.1\r\nHost: 127.0.0.1:9100\r\nUpgrade: websocket\r\n\r\n", rewritten)
    front.loop.close()
    sample = ("\x1b[2m2026-10-19T00:00:05.100000Z\x1b[0m WARN vibe_talk::http::api: voice_call_ended protocol=vibe-talk-v1 "
              "chat=false ended_by=reconnected close_code=- close_reason=\"turn 4 never started; reconnect\"\n"
              "2026-10-19T00:01:05.100000Z INFO vibe_talk::http::api: voice_call_ended protocol=vibe-talk-v1 ended_by=hang_up\n"
              "2026-10-19T00:09:05.100000Z INFO vibe_talk::http::api: voice_call_ended protocol=vibe-talk-v1 ended_by=error_frame\n")
    check("the server's call-end records are read per call",
          call_end_records(sample, "2026-10-19T00:00:00.000Z", "2026-10-19T00:02:00.000Z") == ["reconnected", "hang_up"],
          call_end_records(sample, "2026-10-19T00:00:00.000Z", "2026-10-19T00:02:00.000Z"))

    failed = [name for name, ok, _detail in checks if not ok]
    for name, ok, detail in checks:
        print(f"{'ok  ' if ok else 'FAIL'} {name}" + ("" if ok else f"   {detail}"))
    print(f"\nlive relay check self-test: {len(checks) - len(failed)}/{len(checks)} checks passed")
    return EXIT_SELF_TEST if failed else EXIT_OK


if __name__ == "__main__":
    sys.exit(main())
