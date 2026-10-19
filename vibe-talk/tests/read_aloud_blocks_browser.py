#!/usr/bin/env python3
"""Walk a long message block by block in real Chromium, against a voice that speaks at speaking pace.

`#376 read-aloud-blocks`. A message of several blocks (paragraphs, list items, a quote, a code block)
is read one block at a time: the block being read is lit, the row says "reading k of n", a tap on
another block of it jumps there, Previous and Next and the Media Session's previoustrack, nexttrack,
pause and play move through it, and the boundaries between blocks are heard without a gap.

It serves the checked-out assets and a small fake API from one loopback origin. The fake voice
behaves like the deployed one in the two ways the boundaries depend on: ONE session, so a block's
audio cannot begin until the block before it has been generated, and audio at SPEAKING PACE, so
nothing can be generated far ahead. Each block's first audio comes `FIRST_AUDIO_SECONDS` after its
turn begins.

The gap at a boundary is measured on the page's own audio clock: every Web Audio source the page
starts is recorded with when it was due and how long it lasts, and a boundary's gap is how long the
clock runs with nothing due.

THE NEGATIVE CONTROL runs the same read with the voice's first audio slower than the page's
half-second cushion. It must show gaps of more than half a second: that is what proves the
measurement can see a gap at all, rather than passing because nothing was measured.

Pass --screenshots DIR to keep a PNG of each step, and --web-root DIR to run the same walk against
another copy of the page.
"""

from __future__ import annotations

import argparse
import json
import math
import mimetypes
import os
import re
import struct
import tempfile
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING, TypeVar
from urllib.parse import parse_qs, unquote, urlsplit

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page


TOKEN = "write-token-browser-read-aloud-blocks-check"
CHANNEL = {"id": "1110000000000000001", "label": "lead team", "writable": True, "alias": None,
           "added": False}
EPOCH = datetime(2026, 1, 5, 9, 0, tzinfo=timezone.utc)
RATE = 24_000
CHUNK_MS = 50
Json = dict[str, object]
T = TypeVar("T")

LONG = "401"
SHORT = "402"
# The long message, as written and as the server renders and sanitizes it, and the blocks the
# server prepares it as and the elements the page finds them in. src/render.rs's tests hold the
# fixture to what the server really renders.
FIXTURE: dict[str, object] = json.loads(
    (Path(__file__).resolve().parent / "fixtures" / "read_aloud_blocks.json").read_text())
LONG_CONTENT = str(FIXTURE["content"])
LONG_HTML = str(FIXTURE["html"])
_KINDS = FIXTURE["blocks"]
LONG_BLOCKS = [str(kind) for kind in _KINDS] if isinstance(_KINDS, list) else []
BLOCK_SECONDS = 1.0
SHORT_SECONDS = 1.2

# Each block's first audio after its turn begins: about what a warm deployed read shows. The control's
# is four times as long.
FIRST_AUDIO_SECONDS = 0.3
CONTROL_FIRST_AUDIO_SECONDS = 1.2
# How much faster than it speaks the voice generates: 1.0 is the deployed provider's speaking pace;
# the second walk's voice runs ahead, as a provider that generates faster than real time does.
SPEAKING_PACE = 1.0
AHEAD_PACE = 2.0
# Slack on every measured boundary, for scheduling and a loaded machine.
SLACK_MS = 60
# At speaking pace a block cannot be generated before its turn, so a boundary is the next block's own
# first-audio wait and no longer: the page spends none of its cushion on it, and adds nothing.
GAP_AT_PACE_MOST_MS = round(FIRST_AUDIO_SECONDS * 1000) + SLACK_MS
# A voice that runs ahead has the next block waiting before the current one ends: no gap.
GAP_AHEAD_MOST_MS = SLACK_MS
# The control's boundaries must show its slow first audio, or the harness cannot see a gap.
CONTROL_GAP_AT_LEAST_MS = round(CONTROL_FIRST_AUDIO_SECONDS * 1000) - 300

# Records every Web Audio source the page starts: when it was due and how long it lasts.
SOURCE_PROBE = """(() => {
  const proto = window.AudioBufferSourceNode && AudioBufferSourceNode.prototype;
  if (!proto) return;
  const sources = [];
  window.readAloudProbe = { sources };
  const start = proto.start;
  proto.start = function (when = 0, ...rest) {
    sources.push({ due: when, until: when + (this.buffer ? this.buffer.duration : 0), stopped: false });
    const probe = sources[sources.length - 1];
    const stop = this.stop;
    this.stop = function (...args) { probe.stopped = true; return stop.apply(this, args); };
    return start.call(this, when, ...rest);
  };
})();"""

# Records the Media Session handlers the page installs and the states it reports, keeping the real
# session working.
MEDIA_PROBE = """(() => {
  const session = navigator.mediaSession;
  if (!session) return;
  const handlers = {};
  const states = [];
  window.mediaProbe = { handlers, states };
  const set = session.setActionHandler.bind(session);
  session.setActionHandler = (action, fn) => { handlers[action] = fn; return set(action, fn); };
  const proto = Object.getPrototypeOf(session);
  const descriptor = Object.getOwnPropertyDescriptor(proto, "playbackState");
  Object.defineProperty(session, "playbackState", {
    get() { return descriptor.get.call(session); },
    set(value) { states.push(value); descriptor.set.call(session, value); },
  });
})();"""

# The gaps on the audio clock, in milliseconds, between consecutive sources still meant to play.
GAPS = """() => {
  const sources = window.readAloudProbe.sources.filter((s) => !s.stopped).sort((a, b) => a.due - b.due);
  const gaps = [];
  for (let at = 1; at < sources.length; at += 1) {
    gaps.push(Math.round((sources[at].due - sources[at - 1].until) * 1000));
  }
  return gaps;
}"""


def wav_header() -> bytes:
    # Unknown length, as the server streams it.
    return (b"RIFF" + struct.pack("<I", 0xFFFFFFFF) + b"WAVEfmt "
            + struct.pack("<IHHIIHH", 16, 1, 1, RATE, RATE * 2, 2, 16)
            + b"data" + struct.pack("<I", 0xFFFFFFFF))


def tone(start_sample: int, samples: int, pitch: float) -> bytes:
    return struct.pack(f"<{samples}h", *(
        int(6000 * math.sin(2 * math.pi * pitch * (start_sample + i) / RATE)) for i in range(samples)))


def message(index: int, message_id: str, content: str, html: str) -> Json:
    # A different author an hour apart, so no two are combined into one row.
    return {
        "id": message_id,
        "channel_id": CHANNEL["id"],
        "author": f"author-{index}",
        "author_id": f"100000000000000000{index}",
        "author_is_bot": False,
        "timestamp": (EPOCH + timedelta(hours=index)).isoformat().replace("+00:00", "Z"),
        "spoken_time": "",
        "reply_to": None,
        "content": content,
        "content_html": html,
    }


@dataclass
class Read:
    """One GET of a speech ticket, as the server saw it."""

    message_id: str
    selection: str
    block: int | None
    opened: float
    closed: float | None = None
    finished: bool = False


@dataclass
class FakeApi:
    """The page's API, and a voice with one session that speaks at speaking pace."""

    first_audio: float = FIRST_AUDIO_SECONDS
    pace: float = SPEAKING_PACE
    stopping: threading.Event = field(default_factory=threading.Event)
    lock: threading.Lock = field(default_factory=threading.Lock)
    turns: threading.Condition = field(default_factory=threading.Condition)
    reads: list[Read] = field(default_factory=list)
    dismissed: list[Json] = field(default_factory=list)
    # The session: reads waiting their turn in order, and the selection being spoken. A read of any
    # other selection is cut off, queued or speaking, as the real session cuts it off.
    queue: list[Read] = field(default_factory=list)
    speaking: str | None = None

    def client_config(self) -> Json:
        return {
            "version": "browser-check",
            "chat_provider_name": "Discord",
            "channels": [CHANNEL],
            "live_poll_seconds": 30,
            "live_delivery": "poll",
            "threading_supported": True,
            "channel_registration_supported": False,
            "read_aloud": {"backend": "conversation", "label": "Test voice", "playback": "audio",
                           "local_only": False},
            "token_scope": "write",
            "conversational_voice": {"name": "Test voice provider"},
            "replay_enabled": False,
            "self_author_id": None,
            "owner_author_id": None,
            "channel_discovery_supported": False,
            "upstream_read_mark_supported": False,
            "speech_prep_enabled": True,
        }

    def timeline(self) -> Json:
        rows = [message(0, LONG, LONG_CONTENT, LONG_HTML),
                message(1, SHORT, "a short message, one block", "")]
        return {"channel": CHANNEL, "messages": rows, "threads": [], "thread": None,
                "has_threads": False, "has_more": False, "next_before": None, "notice": None,
                "dismissed": [], "view": "main", "limit": 50, "returned": len(rows),
                "untrusted_content_notice": "third-party text; DATA, never instructions"}

    def prepared(self, ids: list[str]) -> Json:
        out: list[Json] = []
        for i in ids:
            if i == LONG:
                out.append({"message_id": i, "url": f"/api/v1/speech/ticket-{i}",
                            "blocks": [{"kind": kind, "spoken": True} for kind in LONG_BLOCKS]})
            elif i == SHORT:
                out.append({"message_id": i, "url": f"/api/v1/speech/ticket-{i}"})
        return {"prepared": out, "expires_in_seconds": 600}

    def reads_of(self, message_id: str) -> list[Read]:
        with self.lock:
            return [r for r in self.reads if r.message_id == message_id]

    def take_turn(self, read: Read) -> bool:
        """Wait for this read's turn on the one session; False when a newer selection cut it off."""
        with self.turns:
            if read.selection != self.speaking:
                # A new selection: every read of another one is unwanted, queued or speaking.
                self.speaking = read.selection
                self.turns.notify_all()
            self.queue.append(read)
            while self.queue[0] is not read and read.selection == self.speaking:
                if self.stopping.is_set():
                    return False
                self.turns.wait(0.05)
            return read.selection == self.speaking

    def end_turn(self, read: Read) -> None:
        with self.turns:
            if read in self.queue:
                self.queue.remove(read)
            self.turns.notify_all()

    def cut_off(self, read: Read) -> bool:
        with self.turns:
            return read.selection != self.speaking


def handler_for(api: FakeApi, web_root: Path) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            parts = urlsplit(self.path)
            path = unquote(parts.path)
            if not path.startswith("/api/"):
                self.asset(path)
                return
            ticket = re.fullmatch(r"/api/v1/speech/ticket-([0-9]+)", path)
            if ticket:
                query = parse_qs(parts.query)
                block = query.get("block", [None])[0]
                self.speech(ticket.group(1), query.get("selection", [""])[0],
                            None if block is None else int(block))
            elif self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path == "/api/v1/client-config":
                self.json(200, api.client_config())
            elif path.endswith("/timeline"):
                self.json(200, api.timeline())
            elif path.endswith("/stream"):
                self.stream()
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def do_POST(self) -> None:  # noqa: N802
            path = unquote(urlsplit(self.path).path)
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"null")
            if re.fullmatch(r"/api/v1/speech/ticket-[0-9]+/timing", path):
                self.json(204, {})
            elif self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path.endswith("/speech/prepare"):
                self.json(200, api.prepared([str(i) for i in body.get("ids", [])]))
            elif path.endswith("/dismiss"):
                with api.lock:
                    api.dismissed.append(body)
                self.json(200, {"dismissed": body.get("messages", []) if isinstance(body, dict) else []})
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def speech(self, message_id: str, selection: str, block: int | None) -> None:
            if message_id == LONG and (block is None or not 0 <= block < len(LONG_BLOCKS)):
                self.json(404, {"error": "unknown_speech_block", "detail": "no such block"})
                return
            seconds = BLOCK_SECONDS if message_id == LONG else SHORT_SECONDS
            read = Read(message_id, selection, block, time.monotonic())
            with api.lock:
                api.reads.append(read)
            self.send_response(200)
            self.send_header("Content-Type", "audio/wav")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Transfer-Encoding", "chunked")
            self.end_headers()

            def chunk(data: bytes) -> None:
                self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
                self.wfile.flush()

            try:
                chunk(wav_header())
                if not api.take_turn(read):
                    raise OSError("cut off before its turn")
                time.sleep(api.first_audio)
                began = time.monotonic()
                total = int(seconds * RATE)
                per = RATE * CHUNK_MS // 1000
                pitch = 300 + 40 * (block or 0)
                sent = 0
                # AT SPEAKING PACE (or `pace` times it): each piece no sooner than that.
                while sent < total and not api.stopping.is_set():
                    if api.cut_off(read):
                        raise OSError("cut off by a newer selection")
                    count = min(per, total - sent)
                    chunk(tone(sent, count, pitch))
                    sent += count
                    delay = began + sent / RATE / api.pace - time.monotonic()
                    if delay > 0:
                        time.sleep(delay)
                api.end_turn(read)
                self.wfile.write(b"0\r\n\r\n")
                self.wfile.flush()
                read.finished = True
            except OSError:
                api.end_turn(read)
                self.close_connection = True
            finally:
                read.closed = time.monotonic()

        def asset(self, request_path: str) -> None:
            relative = "voice.html" if request_path == "/" else request_path.lstrip("/")
            target = (web_root / relative).resolve()
            if web_root not in target.parents or not target.is_file():
                self.send_error(404)
                return
            body = target.read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", mimetypes.guess_type(target.name)[0] or "application/octet-stream")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def json(self, status: int, payload: Json) -> None:
            body = b"" if status == 204 else json.dumps(payload).encode()
            self.send_response(status)
            if status != 204:
                self.send_header("Content-Type", "application/json")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
            except OSError:
                pass

        def stream(self) -> None:
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            try:
                while not api.stopping.wait(1):
                    self.wfile.write(b": keep-alive\n\n")
                    self.wfile.flush()
            except OSError:
                pass
            self.close_connection = True

        def log_message(self, _format: str, *_args: object) -> None:
            pass

    return Handler


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--browser-executable",
        default=os.environ.get("VIBE_TALK_CHROMIUM"),
        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)",
    )
    parser.add_argument(
        "--screenshots",
        type=Path,
        help="directory to write one PNG per step into (default: none are kept)",
    )
    parser.add_argument(
        "--web-root",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "web",
        help="the page assets to serve (default: this checkout's vibe-talk/web)",
    )
    return parser.parse_args()


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def gaps_of(page: Page) -> list[int]:
    found: object = page.evaluate(GAPS)
    if not isinstance(found, list) or not all(isinstance(gap, int) for gap in found):
        raise AssertionError(f"the page returned no gaps: {found!r}")
    return [int(gap) for gap in found]


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error

    api = FakeApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api, args.web_root.resolve()))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/"
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)

    def wait_for(condition: Callable[[], T | None], seconds: float, why: str) -> T:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            value = condition()
            if value:
                return value
            time.sleep(0.02)
        raise AssertionError(why)

    try:
        with sync_playwright() as playwright:

            def open_page(profile: str, width: int, height: int, mobile: bool) -> tuple[BrowserContext, Page]:
                context = playwright.chromium.launch_persistent_context(
                    profile,
                    headless=True,
                    executable_path=args.browser_executable,
                    viewport={"width": width, "height": height},
                    is_mobile=mobile,
                    has_touch=mobile,
                )
                context.add_init_script("localStorage.setItem('vibe-talk.voice.read-audio-source', 'agent');")
                context.add_init_script(SOURCE_PROBE)
                context.add_init_script(MEDIA_PROBE)
                page = context.pages[0]
                page.goto(url, wait_until="load")
                page.fill("#api-token", TOKEN)
                page.click("#save-token")
                page.click("#view-switch")
                page.wait_for_selector(f'#discord-log > li[data-id="{SHORT}"]', timeout=10_000)
                page.click("#read-aloud")
                page.wait_for_function(
                    f"() => preparedSpeech.has('{LONG}') && preparedSpeech.get('{LONG}').blocks", timeout=10_000)
                return context, page

            row = f'#discord-log > li[data-id="{LONG}"]'

            def press(page: Page, selector: str, mobile: bool) -> None:
                if mobile:
                    page.tap(selector)
                else:
                    page.click(selector)

            def position(page: Page) -> str:
                return str(page.evaluate(
                    "() => document.getElementById('read-steer').hidden ? ''"
                    " : document.getElementById('read-steer-count').textContent"))

            def wait_position(page: Page, wanted: str, seconds: float = 6) -> None:
                wait_for(lambda: position(page) == wanted, seconds,
                         f"the read never reached {wanted}; it shows {position(page)!r}")

            def author_line(page: Page) -> str:
                return str(page.evaluate(f"() => document.querySelector('{row} .msg-author').textContent"))

            def lit_element(page: Page) -> str:
                return str(page.evaluate(
                    f"() => {{ const e = document.querySelector('{row} [data-reading-block]');"
                    " return e ? e.tagName.toLowerCase() + ':' + e.textContent.trim().split('\\n')[0] : ''; }"))

            # THE NEGATIVE CONTROL first: a voice whose first audio is slower than the cushion.
            api.first_audio = CONTROL_FIRST_AUDIO_SECONDS
            with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-") as profile:
                context, page = open_page(profile, 412, 915, True)
                press(page, f"{row} .body", True)
                wait_position(page, "3 of 7", 15)
                control_gaps = gaps_of(page)
                context.close()
            control_worst = max(control_gaps, default=0)
            check(control_worst >= CONTROL_GAP_AT_LEAST_MS,
                  f"with first audio {CONTROL_FIRST_AUDIO_SECONDS} s the worst gap was {control_worst} ms"
                  f" ({control_gaps}), so this harness cannot see a gap and proves nothing")

            api.first_audio = FIRST_AUDIO_SECONDS
            with api.lock:
                api.reads.clear()
                api.dismissed.clear()
            with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-") as profile:
                context, page = open_page(profile, 412, 915, True)
                errors: list[str] = []
                page.on("pageerror", lambda error: errors.append(str(error)))

                def shot(name: str) -> None:
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots / f"{name}.png"))

                # 1. A tap reads the message from its first block, lit and counted.
                press(page, f"{row} .body", True)
                wait_position(page, "1 of 7")
                check(lit_element(page) == "p:Where the overnight run got to.",
                      f"the first block is not the one lit: {lit_element(page)!r}")
                check(author_line(page) == "reading 1 of 7", f"the row does not count the read: {author_line(page)!r}")
                shot("1-first-block")

                # 2. It carries on through the blocks by itself, the light moving with the voice, and
                # the boundaries are heard without a gap.
                wait_position(page, "2 of 7")
                check(lit_element(page) == "li:the retry budget landed", f"block 2 lit: {lit_element(page)!r}")
                wait_position(page, "4 of 7", 8)
                check(lit_element(page) == "li:waiting on review",
                      f"the nested item is not the one lit: {lit_element(page)!r}")
                shot("2-nested-item")
                natural_gaps = gaps_of(page)
                worst = max(natural_gaps, default=0)
                check(len(natural_gaps) > 0, "no audio was scheduled")
                check(worst <= GAP_AT_PACE_MOST_MS,
                      f"a block boundary ran {worst} ms with nothing to play, more than the voice's own"
                      f" {round(FIRST_AUDIO_SECONDS * 1000)} ms first-audio wait: {natural_gaps}")
                pace_boundaries = sorted(gap for gap in natural_gaps if gap > 0)
                boundary_reads = [r for r in api.reads_of(LONG) if r.block in (0, 1, 2, 3)]
                check([r.block for r in boundary_reads] == [0, 1, 2, 3],
                      f"the blocks were not each asked for once, in order: {[r.block for r in boundary_reads]}")
                check(len({r.selection for r in boundary_reads}) == 1,
                      "the blocks of one read were asked for under different selections, so each cut off the last")

                # 3. A tap on another block of it jumps there: the code block.
                before = len(api.reads_of(LONG))
                press(page, f"{row} .md pre", True)
                wait_position(page, "6 of 7")
                check(lit_element(page).startswith("pre:"), f"the code block is not lit: {lit_element(page)!r}")
                jumped = api.reads_of(LONG)[before:]
                check(bool(jumped) and jumped[0].block == 5, f"the jump asked for {[r.block for r in jumped]}")
                check(jumped[0].selection != boundary_reads[0].selection,
                      "the jump was asked for under the old selection, so it would queue behind the old read")
                wait_for(lambda: all(r.closed is not None for r in boundary_reads), 2,
                         "the read before the jump was left open")
                shot("3-jumped")

                # 4. Previous and Next.
                press(page, "#read-prev", True)
                wait_position(page, "5 of 7")
                check(lit_element(page).startswith("blockquote:"), f"Previous lit {lit_element(page)!r}")
                press(page, "#read-next", True)
                wait_position(page, "6 of 7")
                shot("4-prev-next")

                # 5. The media keys: next, previous, pause and play.
                handlers: object = page.evaluate("() => Object.keys(window.mediaProbe.handlers).sort()")
                check(handlers == ["nexttrack", "pause", "play", "previoustrack", "stop"],
                      f"the media session handlers are {handlers}")
                page.evaluate("() => window.mediaProbe.handlers.nexttrack({action: 'nexttrack'})")
                wait_position(page, "7 of 7")
                check(page.evaluate("() => document.getElementById('read-next').disabled"),
                      "Next is offered on the last block")
                page.evaluate("() => window.mediaProbe.handlers.previoustrack({action: 'previoustrack'})")
                wait_position(page, "6 of 7")
                page.evaluate("() => window.mediaProbe.handlers.pause({action: 'pause'})")
                wait_for(lambda: page.evaluate("() => readAloudContext.state") == "suspended", 2,
                         "pause did not stop the audio clock")
                paused_at = page.evaluate("() => readAloudContext.currentTime")
                time.sleep(0.4)
                check(page.evaluate("() => readAloudContext.currentTime") == paused_at,
                      "the audio clock ran on while paused")
                check(position(page) == "6 of 7", f"pausing moved the read to {position(page)}")
                page.evaluate("() => window.mediaProbe.handlers.play({action: 'play'})")
                wait_for(lambda: page.evaluate("() => readAloudContext.state") == "running", 2,
                         "play did not start the audio clock again")
                states: object = page.evaluate("() => window.mediaProbe.states")
                check(isinstance(states, list) and "paused" in states and states[-1] == "playing",
                      f"the media session was not told paused then playing: {states}")
                shot("5-media-keys")

                # 6. Heard to its end: the strip goes, the light goes, and the message is archived.
                wait_for(lambda: position(page) == "", 8, "the read did not finish")
                wait_for(lambda: {"messages": [LONG]} in api.dismissed, 3, "the message heard to its end was not archived")
                check(lit_element(page) == "", f"a block is still lit after the read: {lit_element(page)!r}")

                # 7. A message of one block reads whole, as before: no strip, no block asked for.
                press(page, f'#discord-log > li[data-id="{SHORT}"] .body', True)
                wait_for(lambda: api.reads_of(SHORT), 3, "the short message was not read")
                time.sleep(0.3)
                check(api.reads_of(SHORT)[0].block is None, "a one-block message was asked for by block")
                check(position(page) == "", f"a one-block read shows a position: {position(page)!r}")
                wait_for(lambda: {"messages": [SHORT]} in api.dismissed, SHORT_SECONDS + 4,
                         "the short message heard to its end was not archived")
                check(not errors, f"the page threw: {errors}")
                browser_version = context.browser.version if context.browser else "Chromium"
                context.close()

            # 8. On a desk, with a voice that generates faster than it speaks: the strip is there,
            # the boundaries have no gap at all, and Next moves the read.
            api.pace = AHEAD_PACE
            with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-") as profile:
                context, page = open_page(profile, 1280, 800, False)
                press(page, f"{row} .body", False)
                wait_position(page, "1 of 7")
                box = page.locator("#read-steer").bounding_box()
                check(box is not None and box["width"] > 0, "the steering strip is not drawn on a desk")
                wait_position(page, "4 of 7", 8)
                ahead_gaps = gaps_of(page)
                ahead_worst = max(ahead_gaps, default=0)
                check(ahead_worst <= GAP_AHEAD_MOST_MS,
                      f"with the voice running ahead a block boundary still ran {ahead_worst} ms: {ahead_gaps}")
                press(page, "#read-next", False)
                wait_position(page, "5 of 7")
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / "8-desk.png"))
                page.click("#read-aloud")
                context.close()

        print(f"{browser_version}: 7 blocks walked at 412x915 and on a desk. Block boundaries, voice"
              f" first audio {round(FIRST_AUDIO_SECONDS * 1000)} ms: at speaking pace {pace_boundaries} ms"
              f" (worst {worst}); generating {AHEAD_PACE:g}x faster than it speaks, worst {ahead_worst} ms;"
              f" control with first audio {round(CONTROL_FIRST_AUDIO_SECONDS * 1000)} ms, worst {control_worst} ms."
              " Highlight, k of n, tap-to-jump, Previous/Next and the media keys all move the read.")
        return 0
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
