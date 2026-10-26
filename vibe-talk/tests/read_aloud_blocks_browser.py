#!/usr/bin/env python3
"""Walk a long message block by block in real Chromium, against a voice that speaks at speaking pace.

`#376 read-aloud-blocks`. A message of several blocks (paragraphs, list items, a quote, a code block)
is read one block at a time: the block being read is lit, and inside it the sentence being said
(`#379 read-aloud-sentences`); the row says "reading k of n", a tap on
another block of it jumps there, Previous and Next and the Media Session's previoustrack, nexttrack,
pause and play move through it, and the boundaries between blocks are heard without a gap.

`#433 read-strip-low` and `#434 read-aloud-pause`: the strip sits directly on the chips row with
no gap (measured, at 412x915, 393x852 and on a desk), and the Read button and the media keys are one
state — pausing by either holds the position, resuming by the other carries on from the held block.

`#442 read-strip-order` and `#537 scrub-bar-no-play-pause`: the strip reads Previous, k of n, Next,
and no Pause or Stop of its own, in one line, in the same order on screen as in the markup and for
Tab, at 393x852, 375x812 and on a desk (where the grip that moves it comes first). While a read plays
the bar's Read is its Pause with Stop on its left, also in the keyboard-up slim row (`#429`). The
bar's Stop ends the read and leaves reading mode. On a desk the strip is dragged by its grip and a
double-click on the grip docks it.

`#451 read-strip-followups`: on a desk a one-block read started with s outside reading mode — which
draws no strip — is paused and stopped from the bar, the mode never entered. (The moved strip held in
the window as the chips row grows, and the grip's keys in Help's list, are pinned in the fake-DOM
suite, which can grow a row and read the list.)

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
from typing import TYPE_CHECKING, TypedDict, TypeVar
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
_TEXTS = FIXTURE["texts"]
LONG_TEXTS = [str(text) for text in _TEXTS] if isinstance(_TEXTS, list) else []
# `#379 read-aloud-sentences`. Each block's audio is as long as what it says at this rate, the rate
# the page assumes until it has measured the voice, so the sentence it lights can be checked.
SECONDS_PER_CHAR = 0.068
FIRST_SENTENCES = ["Where the overnight run got to.", "Three things finished.", "One is still going."]
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
# A pause between two pieces of one block, as the voice sent them, long enough to say the host held the
# voice up: four pieces' worth. An underrun inside a block needs its piece half a second late (the
# page's cushion), so a host that caused one shows a pause like this on the voice's side.
STALL_S = 0.2
# At speaking pace a block cannot be generated before its turn, so a boundary is the next block's own
# first-audio wait (`voice_wait_ms`) and no longer: the page spends none of its cushion on it, and adds
# nothing.
# A voice that runs ahead has the next block waiting before the current one ends: no gap.
GAP_AHEAD_MOST_MS = SLACK_MS
# The control's boundaries must show its slow first audio, or the harness cannot see a gap.
CONTROL_GAP_AT_LEAST_MS = round(CONTROL_FIRST_AUDIO_SECONDS * 1000) - 300

# Records every Web Audio source the page starts: when it was due, how long it lasts, the audio clock
# when it was started (when its piece had arrived and been decoded), and its sample rate, which says
# which block it is from (`rate_of`).
SOURCE_PROBE = """(() => {
  const proto = window.AudioBufferSourceNode && AudioBufferSourceNode.prototype;
  if (!proto) return;
  const sources = [];
  window.readAloudProbe = { sources };
  const start = proto.start;
  proto.start = function (when = 0, ...rest) {
    sources.push({ due: when, until: when + (this.buffer ? this.buffer.duration : 0), stopped: false,
                   at: this.context.currentTime, rate: this.buffer ? this.buffer.sampleRate : 0 });
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

# `#442 read-strip-order`. The strip's drawn controls (not its timeline line), in markup order and
# in the order they stand left to right, and how far their middles stray from one line.
STRIP_LAYOUT = """() => {
  const strip = document.getElementById('read-steer');
  const drawn = [...strip.children].filter((e) => e.id !== 'read-timeline' && e.getClientRects().length > 0);
  const boxes = drawn.map((e) => [e.id, e.getBoundingClientRect()]);
  const mids = boxes.map(([, b]) => (b.top + b.bottom) / 2);
  return {
    dom: drawn.map((e) => e.id),
    seen: [...boxes].sort((a, b) => a[1].left - b[1].left).map(([id]) => id),
    spread: Math.max(...mids) - Math.min(...mids),
  };
}"""

# `#537 scrub-bar-no-play-pause`: no Pause or Stop on the strip; the bar under the list has both.
STRIP_ORDER = ["read-prev", "read-steer-count", "read-next"]

# The bar's Read (Pause while a read plays) and the Stop on its left: drawn, side by side, in one row,
# Read keeping the edge under the thumb.
BAR_LAYOUT = """() => {
  const read = document.getElementById('read-aloud').getBoundingClientRect();
  const stop = document.getElementById('read-aloud-stop');
  const box = stop.getBoundingClientRect();
  return {
    label: document.getElementById('read-aloud-label').textContent,
    drawn: stop.getClientRects().length > 0,
    height: box.height,
    beside: box.right <= read.left + 1,
    row: Math.abs((box.top + box.bottom) / 2 - (read.top + read.bottom) / 2),
    readWidth: read.width,
    stopWidth: box.width,
  };
}"""


class BarLayout(TypedDict):
    """What `BAR_LAYOUT` measures."""

    label: str
    drawn: bool
    height: float
    beside: bool
    row: float
    readWidth: float  # noqa: N815 - the page's own camelCase keys
    stopWidth: float  # noqa: N815


# The gaps on the audio clock, in milliseconds, between consecutive sources still meant to play: each
# whether it is a block boundary (the two are from different blocks, by their sample rates), the block
# the later one is from, and, inside a block, whether it is an underrun -- the piece after it was
# scheduled only once the audio before it had run out (`#472 gate-flake-hardening`).
GAPS = """() => {
  const sources = window.readAloudProbe.sources.filter((s) => !s.stopped).sort((a, b) => a.due - b.due);
  const gaps = [];
  for (let at = 1; at < sources.length; at += 1) {
    const before = sources[at - 1], next = sources[at];
    const boundary = before.rate !== next.rate;
    gaps.push({ ms: Math.round((next.due - before.until) * 1000), boundary, rate: next.rate,
                underrun: !boundary && next.at >= before.until - 0.006 });
  }
  return gaps;
}"""


@dataclass(frozen=True)
class Gap:
    """One gap `GAPS` measured."""

    ms: int
    boundary: bool
    rate: int
    underrun: bool

    @property
    def block(self) -> int:
        return (self.rate - RATE) // 8


def rate_of(block: int | None) -> int:
    """The sample rate block `block` is said at: each its own, so the page's sources say which they are from."""
    return RATE + 8 * (block or 0)


def wav_header(rate: int) -> bytes:
    # Unknown length, as the server streams it.
    return (b"RIFF" + struct.pack("<I", 0xFFFFFFFF) + b"WAVEfmt "
            + struct.pack("<IHHIIHH", 16, 1, 1, rate, rate * 2, 2, 16)
            + b"data" + struct.pack("<I", 0xFFFFFFFF))


def tone(start_sample: int, samples: int, pitch: float, rate: int) -> bytes:
    return struct.pack(f"<{samples}h", *(
        int(6000 * math.sin(2 * math.pi * pitch * (start_sample + i) / rate)) for i in range(samples)))


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
    # When its first and its latest piece of audio were sent, and how often the host held it up between
    # two pieces (`STALL_S`).
    first_sent: float | None = None
    last_sent: float | None = None
    stalls: int = 0


def voice_wait_ms(before: Read, after: Read) -> int:
    """The voice's own first-audio wait at the boundary between two reads, as it really took it: from when
    it could have begun the second -- the first sent whole and the second asked for -- to its first audio.
    On a calm host this is `first_audio`; on a loaded one the voice's sleep and its hand-over run long, and
    the boundary the page hears with them. Nothing the page does late is counted in it: a page that asked
    late moves `opened`, and one that schedules late moves only the gap."""
    assert before.last_sent is not None and after.first_sent is not None
    return round((after.first_sent - max(before.last_sent, after.opened)) * 1000)


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
                            "blocks": [{"kind": kind, "spoken": True, "chars": len(text)}
                                       for kind, text in zip(LONG_BLOCKS, LONG_TEXTS)]})
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
            seconds = (max(0.6, len(LONG_TEXTS[block or 0]) * SECONDS_PER_CHAR) if message_id == LONG
                       else SHORT_SECONDS)
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
                rate = rate_of(block)
                chunk(wav_header(rate))
                if not api.take_turn(read):
                    raise OSError("cut off before its turn")
                time.sleep(api.first_audio)
                began = time.monotonic()
                total = int(seconds * rate)
                per = rate * CHUNK_MS // 1000
                pitch = 300 + 40 * (block or 0)
                sent = 0
                # AT SPEAKING PACE (or `pace` times it): each piece no sooner than that.
                while sent < total and not api.stopping.is_set():
                    if api.cut_off(read):
                        raise OSError("cut off by a newer selection")
                    count = min(per, total - sent)
                    chunk(tone(sent, count, pitch, rate))
                    now = time.monotonic()
                    if read.last_sent is not None and now - read.last_sent > STALL_S:
                        read.stalls += 1
                    read.last_sent = now
                    if read.first_sent is None:
                        read.first_sent = now
                    sent += count
                    delay = began + sent / rate / api.pace - time.monotonic()
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


def gaps_of(page: Page) -> list[Gap]:
    found: object = page.evaluate(GAPS)
    if not isinstance(found, list) or not all(isinstance(gap, dict) for gap in found):
        raise AssertionError(f"the page returned no gaps: {found!r}")
    return [Gap(int(gap["ms"]), bool(gap["boundary"]), int(gap["rate"]), bool(gap["underrun"])) for gap in found]


def boundaries(gaps: list[Gap]) -> list[int]:
    """The gaps at the block boundaries: what the walks are about."""
    return [gap.ms for gap in gaps if gap.boundary]


def check_within_blocks(gaps: list[Gap], reads: list[Read], where: str) -> int:
    """Inside a block the audio is continuous, but for an underrun the host caused: the page scheduled a
    piece only after the audio before it had run out, AND the voice was held up sending that block at
    least as often (`Read.stalls`). A page that holds pieces back, or schedules them late, with the voice
    on time, still fails. Says how many there were."""
    late = [gap for gap in gaps if not gap.boundary and gap.ms > SLACK_MS]
    check(all(gap.underrun for gap in late),
          f"{where}: a block's audio was given a gap though its next piece was there in time: {late}")
    for block in sorted({gap.block for gap in late}):
        stalls = max((r.stalls for r in reads if r.message_id == LONG and r.block == block), default=0)
        underruns = sum(1 for gap in late if gap.block == block)
        check(underruns <= stalls,
              f"{where}: block {block + 1} ran out {underruns} time(s) inside it, but the voice was held up"
              f" sending it only {stalls} time(s), so the page was late with audio it had: {late}")
    return len(late)


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

            def open_page(profile: str, width: int, height: int, mobile: bool,
                          scheme: str = "light") -> tuple[BrowserContext, Page]:
                context = playwright.chromium.launch_persistent_context(
                    profile,
                    headless=True,
                    executable_path=args.browser_executable,
                    viewport={"width": width, "height": height},
                    color_scheme=scheme,  # type: ignore[arg-type]
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

            def strip_gap(page: Page) -> float:
                # `#433 read-strip-low`: the strip's bottom edge against the chips row's top
                # edge, in CSS pixels. Nothing but the two borders between them reads as 0.
                return float(page.evaluate(
                    "() => { const s = document.getElementById('read-steer').getBoundingClientRect();"
                    " const t = document.getElementById('scroll-tools').getBoundingClientRect();"
                    " return t.top - s.bottom; }"))

            def check_strip_order(page: Page, where: str, desk: bool = False) -> None:
                # `#442 read-strip-order`, `#537`: Previous, k of n, Next and nothing else, in one line, the
                # markup's order on screen. The grip that moves it is drawn on a desk only.
                layout = page.evaluate(STRIP_LAYOUT)
                want = (["read-steer-grip"] if desk else []) + STRIP_ORDER
                check(layout["dom"] == want, f"the strip's markup at {where} is {layout['dom']}")
                check(layout["seen"] == want, f"the strip at {where} reads {layout['seen']} left to right")
                check(layout["spread"] <= 2, f"the strip wraps at {where}: its controls stray {layout['spread']:.1f}px")

            def check_bar_stop(page: Page, where: str, tallest: float = 44) -> BarLayout:
                # A phone's target is 44px at least; a desk's row is 2.25rem (36px) throughout.
                bar: BarLayout = page.evaluate(BAR_LAYOUT)
                check(bar["label"] in ("Pause", "Resume"), f"the bar's Read says {bar['label']!r} at {where}")
                check(bar["drawn"] and bar["stopWidth"] >= 44 and bar["height"] >= tallest,
                      f"no Stop target beside Read at {where}: {bar}")
                check(bar["beside"] and bar["row"] <= 2, f"the bar's Stop is not beside its Pause at {where}: {bar}")
                return bar

            def check_mode_left(page: Page, where: str) -> None:
                wait_for(lambda: page.text_content("#read-aloud-label") == "Read", 3,
                         f"Stop at {where} left the Read button saying {page.text_content('#read-aloud-label')!r}")
                check(page.get_attribute("#read-aloud", "data-active") == "false", f"Stop at {where} left reading mode on")
                check(position(page) == "", f"Stop at {where} left the strip up")
                check(page.is_hidden("#read-aloud-stop"), f"Stop at {where} left the bar's Stop up")

            def wait_position(page: Page, wanted: str, seconds: float = 10) -> None:
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
            control_worst = max(boundaries(control_gaps), default=0)
            check(control_worst >= CONTROL_GAP_AT_LEAST_MS,
                  f"with first audio {CONTROL_FIRST_AUDIO_SECONDS} s the worst boundary was {control_worst} ms"
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
                check(lit_element(page).startswith("p:Where the overnight run got to."),
                      f"the first block is not the one lit: {lit_element(page)!r}")
                check(author_line(page) == "reading 1 of 7", f"the row does not count the read: {author_line(page)!r}")
                gap412 = strip_gap(page)
                check(abs(gap412) <= 2, f"the strip floats {gap412:.1f}px off the chips row at 412x915 (`#433`)")
                shot("1-first-block")

                # 1b. Inside it, the sentence being said is lit, each in turn. `#379`.
                sentences: list[str] = []
                deadline = time.monotonic() + 8
                while time.monotonic() < deadline and position(page) == "1 of 7":
                    said = str(page.evaluate(
                        "() => { const lit = CSS.highlights.get('read-aloud-sentence');"
                        " return lit ? [...lit][0].toString() : ''; }"))
                    if said and (not sentences or sentences[-1] != said):
                        sentences.append(said)
                        if len(sentences) == 2:
                            shot("1b-second-sentence")
                    time.sleep(0.05)
                check(sentences == FIRST_SENTENCES,
                      f"the sentences lit in the first block were {sentences}, not {FIRST_SENTENCES}")

                # 2. It carries on through the blocks by itself, the light moving with the voice, and
                # the boundaries are heard without a gap.
                wait_position(page, "2 of 7")
                check(lit_element(page) == "li:the retry budget landed", f"block 2 lit: {lit_element(page)!r}")
                wait_position(page, "4 of 7", 15)
                check(lit_element(page) == "li:waiting on review",
                      f"the nested item is not the one lit: {lit_element(page)!r}")
                shot("2-nested-item")
                natural_gaps = gaps_of(page)
                worst = max(boundaries(natural_gaps), default=0)
                check(len(boundaries(natural_gaps)) >= 3, f"not three block boundaries were scheduled: {natural_gaps}")
                boundary_reads = [r for r in api.reads_of(LONG) if r.block in (0, 1, 2, 3)]
                check([r.block for r in boundary_reads] == [0, 1, 2, 3],
                      f"the blocks were not each asked for once, in order: {[r.block for r in boundary_reads]}")
                # Each boundary against the voice's own wait there: `FIRST_AUDIO_SECONDS` on a calm host,
                # longer where the host held the voice up (`#472 gate-flake-hardening`).
                waits = [max(round(FIRST_AUDIO_SECONDS * 1000), voice_wait_ms(before, after))
                         for before, after in zip(boundary_reads, boundary_reads[1:])]
                for gap, wait in zip(boundaries(natural_gaps), waits):
                    check(gap <= wait + SLACK_MS,
                          f"a block boundary ran {gap} ms with nothing to play, more than the voice's own"
                          f" {wait} ms first-audio wait there: {natural_gaps}")
                underruns = check_within_blocks(natural_gaps, api.reads_of(LONG), "at speaking pace")
                pace_boundaries = sorted(gap for gap in boundaries(natural_gaps) if gap > 0)
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

                # 3b. The timeline: a tap on its fifth block reads from the quote. `#380`.
                count = page.evaluate("() => document.getElementById('read-timeline').children.length")
                check(count == len(LONG_BLOCKS), f"the timeline shows {count} blocks, not {len(LONG_BLOCKS)}")
                press(page, "#read-timeline > button:nth-child(5)", True)
                wait_position(page, "5 of 7")
                check(lit_element(page).startswith("blockquote:"), f"the timeline's tap lit {lit_element(page)!r}")
                current = page.evaluate(
                    "() => [...document.querySelectorAll('#read-timeline > button')]"
                    ".findIndex((b) => b.getAttribute('aria-current') === 'true')")
                check(current == 4, f"the timeline marks block {current}, not the fifth")
                press(page, "#read-timeline > button:nth-child(6)", True)
                wait_position(page, "6 of 7")
                shot("3b-timeline")

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
                # `#434 read-aloud-pause`: the one state, said by every control. The media pause
                # is the Read button's Resume, and the Read button resumes it.
                check(page.text_content("#read-aloud-label") == "Resume",
                      f"a media pause left the Read button saying {page.text_content('#read-aloud-label')!r}")
                shot("5b-paused")
                press(page, "#read-aloud", True)
                wait_for(lambda: page.evaluate("() => readAloudContext.state") == "running", 2,
                         "the Read button did not resume a media pause")
                check(position(page) == "6 of 7", f"resuming moved the read to {position(page)}")
                # And the Read button holds and resumes the same read, from the same block.
                press(page, "#read-aloud", True)
                wait_for(lambda: page.evaluate("() => readAloudContext.state") == "suspended", 2,
                         "the Read button's Pause did not stop the audio clock")
                check(page.text_content("#read-aloud-label") == "Resume",
                      "the Read button's pause left it saying Pause")
                press(page, "#read-aloud", True)
                wait_for(lambda: page.evaluate("() => readAloudContext.state") == "running", 2,
                         "the Read button's Resume did not start the audio clock again")
                page.evaluate("() => window.mediaProbe.handlers.play({action: 'play'})")
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
            ahead_from = len(api.reads_of(LONG))
            with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-") as profile:
                context, page = open_page(profile, 1280, 800, False)
                press(page, f"{row} .body", False)
                wait_position(page, "1 of 7")
                box = page.locator("#read-steer").bounding_box()
                check(box is not None and box["width"] > 0, "the steering strip is not drawn on a desk")
                gap_desk = strip_gap(page)
                check(abs(gap_desk) <= 2, f"the strip floats {gap_desk:.1f}px off the chips row on a desk (`#433`)")
                wait_position(page, "4 of 7", 15)
                ahead_gaps = gaps_of(page)
                ahead_worst = max(boundaries(ahead_gaps), default=0)
                check(len(boundaries(ahead_gaps)) >= 3, f"not three block boundaries were scheduled: {ahead_gaps}")
                check(ahead_worst <= GAP_AHEAD_MOST_MS,
                      f"with the voice running ahead a block boundary still ran {ahead_worst} ms: {ahead_gaps}")
                underruns += check_within_blocks(ahead_gaps, api.reads_of(LONG)[ahead_from:], "running ahead")
                press(page, "#read-next", False)
                wait_position(page, "5 of 7")
                check_strip_order(page, "a desk", desk=True)
                bar_desk = check_bar_stop(page, "a desk", 36)
                # The narrowest desk, where the dock is tightest: still Pause with Stop beside it.
                page.set_viewport_size({"width": 900, "height": 800})
                time.sleep(0.3)
                check_strip_order(page, "a 900px desk", desk=True)
                check_bar_stop(page, "a 900px desk", 36)
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / "8-desk-900.png"))
                page.set_viewport_size({"width": 1280, "height": 800})
                time.sleep(0.3)
                # Tab meets them as they are seen (the count is not a stop).
                page.focus("#read-prev")
                for wanted in ("read-next",):
                    page.keyboard.press("Tab")
                    focused = page.evaluate("() => document.activeElement.id")
                    check(focused == wanted, f"Tab from the strip reached {focused!r}, not {wanted!r}")
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / "8-desk.png"))
                # Dragged by its grip, it goes where it is put and is remembered; a double-click
                # on the grip docks it on the chips row again.
                strip_before = page.locator("#read-steer").bounding_box()
                grip_box = page.locator("#read-steer-grip").bounding_box()
                check(strip_before is not None and grip_box is not None, "the strip or its grip is not drawn on a desk")
                assert strip_before is not None and grip_box is not None
                page.mouse.move(grip_box["x"] + grip_box["width"] / 2, grip_box["y"] + grip_box["height"] / 2)
                page.mouse.down()
                page.mouse.move(grip_box["x"] + grip_box["width"] / 2 - 300, grip_box["y"] + grip_box["height"] / 2 - 250, steps=6)
                page.mouse.up()
                strip_after = page.locator("#read-steer").bounding_box()
                assert strip_after is not None
                moved = (strip_after["x"] - strip_before["x"], strip_after["y"] - strip_before["y"])
                check(abs(moved[0] + 300) <= 2 and abs(moved[1] + 250) <= 2, f"the dragged strip moved {moved}")
                stored = page.evaluate("() => localStorage.getItem('vibe-talk.voice.read-steer-offset')")
                check(bool(stored), "the dragged strip's place is not remembered")
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / "8-desk-moved.png"))
                page.mouse.move(grip_box["x"] - 300 + grip_box["width"] / 2, grip_box["y"] - 250 + grip_box["height"] / 2)
                page.dblclick("#read-steer-grip")
                gap_docked = strip_gap(page)
                check(abs(gap_docked) <= 2, f"a double-click left the strip {gap_docked:.1f}px off the chips row")
                # Stop, in the bar: the read ends and reading mode is left, in one click.
                press(page, "#read-aloud-stop", False)
                check_mode_left(page, "the desk's bar")
                context.close()

            # 9. At 393x852: the strip still hugs the chips row (`#433`), and the bar's Read holds
            # and resumes the read where it was held (`#434`), the strip having no Pause (`#537`).
            with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-") as profile:
                context, page = open_page(profile, 393, 852, True)
                press(page, f"{row} .body", True)
                wait_position(page, "1 of 7")
                gap393 = strip_gap(page)
                check(abs(gap393) <= 2, f"the strip floats {gap393:.1f}px off the chips row at 393x852 (`#433`)")
                check_strip_order(page, "393x852")
                bar393 = check_bar_stop(page, "393x852")
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / "9-phone-393.png"))
                press(page, "#read-next", True)
                wait_position(page, "2 of 7")
                press(page, "#read-aloud", True)
                wait_for(lambda: page.evaluate("() => readAloudContext.state") == "suspended", 2,
                         "the Read button's Pause did not stop the audio clock at 393x852")
                check(position(page) == "2 of 7", f"the pause moved the read to {position(page)}")
                check(page.text_content("#read-aloud-label") == "Resume", "the held read does not offer Resume")
                # The status pill stacks over the strip rather than under its top edge (`#433`).
                page.evaluate("() => setStatus('held at the second block')")
                pill_gap = float(page.evaluate(
                    "() => { const p = document.getElementById('status-line').getBoundingClientRect();"
                    " const s = document.getElementById('read-steer').getBoundingClientRect();"
                    " return s.top - p.bottom; }"))
                check(pill_gap >= 4, f"the status pill stands {pill_gap:.1f}px into the strip at 393x852")
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / "9-phone-393-paused.png"))
                press(page, "#read-aloud", True)
                wait_for(lambda: page.evaluate("() => readAloudContext.state") == "running", 2,
                         "the Read button's Resume did not resume the read at 393x852")
                check(position(page) == "2 of 7", f"the resume moved the read to {position(page)}")
                # At the foot of the list the last message scrolls clear of the strip: the room
                # the composer keeps above itself grew by the strip's height (`#433`).
                page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
                time.sleep(0.3)
                cover = float(page.evaluate(
                    f"() => {{ const last = document.querySelector('#discord-log > li[data-id=\"{SHORT}\"]').getBoundingClientRect();"
                    " const s = document.getElementById('read-steer').getBoundingClientRect();"
                    " return last.bottom - s.top; }"))
                check(cover <= 2, f"at the foot the strip covers {cover:.1f}px of the last message")
                # The bar's Stop: the read ends and reading mode is left, in one tap (`#442`).
                press(page, "#read-aloud-stop", True)
                check_mode_left(page, "393x852's bar")
                context.close()

            # 10. At 375x812, the narrow phone: the strip's targets still stand in one line on the
            # chips row, the bar has Pause and Stop side by side, the keyboard-up slim row keeps both
            # (`#429`), and the bar's Stop leaves reading mode (`#442`).
            with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-") as profile:
                context, page = open_page(profile, 375, 812, True)
                press(page, f"{row} .body", True)
                wait_position(page, "1 of 7")
                gap375 = strip_gap(page)
                check(abs(gap375) <= 2, f"the strip floats {gap375:.1f}px off the chips row at 375x812")
                check_strip_order(page, "375x812")
                # A long message's count is two digits a side; the line still holds it.
                wide = float(page.evaluate(
                    "() => { const c = document.getElementById('read-steer-count'); const was = c.textContent;"
                    " c.textContent = '12 of 12'; const f = (" + STRIP_LAYOUT + ")(); c.textContent = was;"
                    " return f.spread; }"))
                check(wide <= 2, f"a two-digit count wraps the strip at 375x812: its controls stray {wide:.1f}px")
                bar375 = check_bar_stop(page, "375x812")
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / "10-phone-375.png"))
                page.evaluate("() => { document.documentElement.dataset.keyboard = 'open'; }")
                slim = check_bar_stop(page, "375x812 with the keyboard up")
                check(page.is_hidden("#read-speed"), "the slim row kept Pace beside Pause and Stop")
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / "10-phone-375-keyboard-row.png"))
                page.evaluate("() => { delete document.documentElement.dataset.keyboard; }")
                press(page, "#read-aloud-stop", True)
                check_mode_left(page, "375x812's bar")
                context.close()

            # 11. `#451`: on a desk, a one-block read started with s outside reading mode. It draws
            # no strip, so the bar is its only transport: Read pauses it and the Stop beside Read
            # ends it, with the mode never entered (aria-pressed, the mode's own say, stays off).
            with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-") as profile:
                context, page = open_page(profile, 1280, 800, False)
                press(page, "#read-aloud", False)
                wait_for(lambda: page.text_content("#read-aloud-label") == "Read", 3,
                         "the premise: leaving the mode the walk opened in")
                check(page.get_attribute("#read-aloud", "aria-pressed") == "false",
                      "the premise: reading mode is on")
                check(page.is_hidden("#read-aloud-stop"), "the premise: a Stop stands before anything plays")
                short_row = f'#discord-log > li[data-id="{SHORT}"]'
                page.locator(short_row).hover()
                page.wait_for_selector(f"{short_row}[data-selected='true']", timeout=5_000)
                page.keyboard.press("s")
                wait_for(lambda: page.text_content("#read-aloud-label") == "Pause" and page.is_visible("#read-aloud-stop"),
                         5, "the read started with s has no Pause and Stop in the bar")
                check(position(page) == "", "the premise: a one-block read drew a strip")
                check(page.get_attribute("#read-aloud", "aria-pressed") == "false",
                      "the read started with s entered reading mode")
                press(page, "#read-aloud", False)
                wait_for(lambda: page.evaluate("() => readAloudContext.state") == "suspended", 2,
                         "Read did not hold the read started with s")
                check(page.text_content("#read-aloud-label") == "Resume",
                      "the held one-off read does not offer Resume")
                check(page.is_visible("#read-aloud-stop"), "the held one-off read lost the bar's Stop")
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / "11-desk-s-held.png"))
                press(page, "#read-aloud-stop", False)
                check_mode_left(page, "the desk's bar on a read started with s")
                check(page.get_attribute("#read-aloud", "aria-pressed") == "false",
                      "stopping the one-off read left reading mode on")
                context.close()

        print(f"{browser_version}: 7 blocks walked at 412x915, 393x852, 375x812 and on a desk. Block boundaries, voice"
              f" first audio {round(FIRST_AUDIO_SECONDS * 1000)} ms: at speaking pace {pace_boundaries} ms"
              f" (worst {worst}, the voice's waits {waits} ms); generating {AHEAD_PACE:g}x faster than it speaks, worst {ahead_worst} ms;"
              f" control with first audio {round(CONTROL_FIRST_AUDIO_SECONDS * 1000)} ms, worst {control_worst} ms;"
              f" {underruns} underrun(s) inside a block, the host late with the audio."
              f" The strip's gap over the chips row: {gap412:.1f}px at 412x915, {gap393:.1f}px at 393x852,"
              f" {gap_desk:.1f}px on a desk, {gap375:.1f}px at 375x812. Strip order Previous, k of n, Next, no"
              f" Pause or Stop, in one line at 393, 375 and on a desk; the bar's Pause and Stop {bar375['readWidth']:.0f}px and"
              f" {bar375['stopWidth']:.0f}px wide at 375 ({slim['readWidth']:.0f}/{slim['stopWidth']:.0f}px in the"
              f" keyboard-up row), {bar393['readWidth']:.0f}/{bar393['stopWidth']:.0f}px at 393,"
              f" {bar_desk['readWidth']:.0f}/{bar_desk['stopWidth']:.0f}px on a desk; dragged 300,250px and docked"
              " again on a desk; a one-block read started"
              " with s paused and stopped from a desk's bar. The first block's three sentences were lit in turn; highlight,"
              " k of n, tap-to-jump, the timeline, Previous/Next, the media keys and Pause/Resume by the"
              " Read button and the media keys all move or hold the one read.")
        return 0
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
