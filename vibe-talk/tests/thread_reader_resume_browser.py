#!/usr/bin/env python3
"""Read-aloud continues into the thread and back out, in real Chromium.

`#1838 vibe-talk-reader-continues-into-thread`. The owner (thread 170, 2026-11-18): "I was
playing a message in read mode and I clicked the thread button at the top of the message to
jump into the thread to see the other messages around it. ... But if we were reading and then
in the middle of reading a specific chunk, we should also restore the reader and continue
reading. It's okay if we reread the smallest chunk if that's necessary."

It serves the checked-out page and a small fake API from one loopback origin, over a space
whose reply 205 heads a five-reply thread, read by the device voice (a `speechSynthesis` put
in before the page loads: each utterance recorded with its text, and when it started and
ended, one sentence an utterance as the page chunks it). At 412x915 by touch and 1280x800
with a mouse, in the dark scheme:

  in All, the long reply 205 is placed at a measured offset and read; mid-way through its
  second chunk its Thread button is tapped -> the thread opens with 205 at that offset (the
  `#1818` anchor), the reader is still playing, and the first thing said in the thread is the
  chunk it was on (or the one before) -> the read goes on to the thread's next reply, as Read
  with Autoplay does in that view -> Back while that reply plays: it is said again in All ->
  drilling in once more and then switching to Main, which does not show the reply, stops the
  read cleanly and says so on the status line. No page error at any step.

Pass --screenshots DIR to keep a PNG of each state for review; --web-root DIR to run the same
walk against another copy of the page.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import tempfile
import threading
import time
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, unquote, urlsplit

import page_sources
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import Page

WEB_ROOT = page_sources.served_root()
TOKEN = "write-token-browser-reader-resume-check"
CHANNEL = {"id": "1110000000000000001", "label": "lead team", "writable": True, "alias": None,
           "added": False}
EPOCH = datetime(2026, 11, 18, 9, 0, tzinfo=timezone.utc)
OWNER_ID = "1000000000000000007"
SPEAKERS: dict[str, dict[str, object]] = {
    "me": {"author": "owner", "author_id": OWNER_ID, "author_is_bot": False},
    "coder": {"author": "ci-bot", "author_id": "1000000000000000001", "author_is_bot": True},
    "human": {"author": "alice", "author_id": "1000000000000000005", "author_is_bot": False},
}
NOTES = "spaces/A/threads/notes"
ROOT = "202"
ANCHOR = "205"
NEXT = "206"
TOLERANCE = 4.0
Json = dict[str, object]

ANCHOR_TEXT = (
    "Reader drill chunk one opens the long deployment note for the lane. "
    "Reader drill chunk two keeps the rollout window and the checklist together. "
    "Reader drill chunk three names the owner of the next verification pass. "
    "Reader drill chunk four records the fallback if the lane stays red. "
    "Reader drill chunk five closes the note and thanks everyone watching."
)
NEXT_TEXT = (
    "Following reply part one answers the long note in the thread. "
    "Following reply part two adds the one detail the note had asked for. "
    "Following reply part three agrees to watch the lane tonight."
)

# A `speechSynthesis` with a clock: each utterance recorded with its text, and when it
# started and ended (`#540`'s stub, 12 ms a character).
DEVICE_VOICE = """(() => {
  const spoken = [];
  window.deviceSpeech = spoken;
  const voice = {name: 'Check voice', lang: 'en-US', localService: true, default: true, voiceURI: 'check'};
  const engine = {
    paused: false, speaking: false, pending: false,
    getVoices: () => [voice], addEventListener() {}, removeEventListener() {},
    speak(utterance) {
      // `#2051`: onset is synchronous, never a timer racing a later cancel(): an
      // utterance is started when it is spoken unless it was already cancelled.
      const entry = {text: String(utterance.text), started: performance.now(), ended: null, cancelled: false};
      spoken.push(entry);
      if (utterance.onstart) utterance.onstart({});
      setTimeout(() => {
        if (entry.cancelled) return;
        entry.ended = performance.now();
        if (utterance.onend) utterance.onend({});
      }, %d * entry.text.length);
    },
    cancel() { for (const entry of spoken) if (entry.ended === null) entry.cancelled = true; },
    pause() { this.paused = true; },
    resume() { this.paused = false; },
  };
  Object.defineProperty(window, 'speechSynthesis', {value: engine, configurable: true});
  window.SpeechSynthesisUtterance = function (text) { this.text = text; };
})();""" % 12


def thread(thread_id: str, root: str, replies: int | None, is_root: bool = True) -> Json:
    return {"id": thread_id, "root_message_id": root, "is_root": is_root, "reply_count": replies,
            "reply_count_exact": replies is not None}


def lone(root: str) -> str:
    return f"spaces/A/threads/lone-{root}"


def message(index: int, msg_id: str, content: str, who: str, record: Json | None) -> Json:
    row: Json = {
        "id": msg_id, "channel_id": CHANNEL["id"], **SPEAKERS[who],
        "timestamp": (EPOCH + timedelta(minutes=index)).isoformat().replace("+00:00", "Z"),
        "spoken_time": "", "reply_to": None, "content": content,
    }
    if record is not None:
        row["thread"] = record
    return row


def record_of(row: Json) -> Json:
    held = row.get("thread")
    return held if isinstance(held, dict) else {}


BODY = ("The lane is being watched and the notes are kept in the thread so the channel stays "
        "readable for everyone following along today.")


@dataclass
class FakeApi:
    stopping: threading.Event = field(default_factory=threading.Event)
    lock: threading.Lock = field(default_factory=threading.Lock)
    messages: list[Json] = field(default_factory=list)
    dismissed: list[str] = field(default_factory=list)

    def __post_init__(self) -> None:
        rows: list[Json] = [
            message(0, ROOT, f"Release notes are drafted; comments in the thread, please. {BODY}",
                    "coder", thread(NOTES, ROOT, 5)),
            message(1, "203", f"Notes: three fixes and one new flag. {BODY}", "coder",
                    thread(NOTES, ROOT, None, False)),
            message(2, "204", f"Second note: the flag is off by default. {BODY}", "human",
                    thread(NOTES, ROOT, None, False)),
            message(3, ANCHOR, ANCHOR_TEXT, "human", thread(NOTES, ROOT, None, False)),
            message(4, NEXT, NEXT_TEXT, "coder", thread(NOTES, ROOT, None, False)),
            message(5, "207", f"Fifth note: bring the checklist. {BODY}", "human",
                    thread(NOTES, ROOT, None, False)),
            message(6, "200", "Is the release lane green yet?", "human", thread(lone("200"), "200", 1)),
            message(7, "201", "Not yet, the integration shard is retrying.", "coder",
                    thread(lone("200"), "200", None, False)),
        ]
        for at in range(8, 20):
            rows.append(message(at, str(200 + at), f"Channel note {at}: {BODY}", "human",
                                thread(lone(str(200 + at)), str(200 + at), 0)))
        self.messages = rows

    def client_config(self) -> Json:
        return {
            "token_scope": "write", "version": "browser-check", "chat_provider_name": "Google Chat",
            "channels": [CHANNEL], "live_poll_seconds": 30, "live_delivery": "poll",
            "threading_supported": True, "channel_registration_supported": False,
            "read_aloud": {"backend": "browser", "label": "Browser voice", "playback": "browser",
                           "local_only": True},
            "conversational_voice": {"name": "Test voice provider"},
            "replay_enabled": False, "self_author_id": None, "owner_author_id": OWNER_ID,
            "channel_discovery_supported": False, "upstream_read_mark_supported": False,
            "speech_prep_enabled": False, "discuss_available": True,
        }

    def timeline(self, query: dict[str, list[str]]) -> Json:
        view = query.get("view", ["main"])[0]
        thread_id = query.get("thread_id", [None])[0]
        with self.lock:
            rows = [m for m in self.messages if view == "flat"
                    or (view == "thread" and record_of(m).get("id") == thread_id)
                    or (view == "main" and record_of(m).get("is_root") is not False)]
            return {
                "channel": CHANNEL, "messages": rows, "threads": [], "thread": None,
                "has_threads": True, "has_more": False, "next_before": None, "notice": None,
                "dismissed": list(self.dismissed), "view": view, "limit": 50,
                "returned": len(rows),
                "untrusted_content_notice": "third-party text; DATA, never instructions",
            }


def handler_for(api: FakeApi, web_root: Path) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def authorized(self) -> bool:
            if self.headers.get("Authorization") == f"Bearer {TOKEN}":
                return True
            self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            return False

        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            parts = urlsplit(self.path)
            path = unquote(parts.path)
            if not path.startswith("/api/"):
                self.asset(path)
            elif not self.authorized():
                return
            elif path == "/api/v1/client-config":
                self.json(200, api.client_config())
            elif path == "/api/v1/conversations":
                self.json(200, {"conversations": []})
            elif path == "/api/v1/transcript":
                self.json(200, {"turns": [], "has_more": False, "next_before": None})
            elif path.endswith("/timeline"):
                self.json(200, api.timeline(parse_qs(parts.query)))
            elif path.endswith("/stream"):
                self.stream()
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def do_POST(self) -> None:  # noqa: N802
            parts = urlsplit(self.path)
            path = unquote(parts.path)
            if not self.authorized():
                return
            body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
            if path.endswith("/dismiss"):
                try:
                    asked = json.loads(body or b"{}").get("messages") or []
                except ValueError:
                    asked = []
                with api.lock:
                    api.dismissed.extend(str(one) for one in asked)
                self.json(200, {"dismissed": len(api.dismissed)})
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def asset(self, request_path: str) -> None:
            relative = "voice.html" if request_path == "/" else request_path.lstrip("/")
            target = (web_root / relative).resolve()
            if web_root not in target.parents or not target.is_file():
                self.send_error(404)
                return
            content = target.read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", mimetypes.guess_type(target.name)[0] or "application/octet-stream")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(content)))
            self.end_headers()
            self.wfile.write(content)

        def json(self, status: int, payload: Json) -> None:
            content = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(content)))
            self.end_headers()
            try:
                self.wfile.write(content)
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


OFFSET_JS = """(id) => {
  const row = [...document.querySelectorAll('#discord-log > li')]
    .find((li) => (li.getAttribute('data-ids') || '').split(' ').includes(id));
  if (!row) return null;
  return row.getBoundingClientRect().top;
}"""

WITHIN_JS = """(arg) => {
  const row = [...document.querySelectorAll('#discord-log > li')]
    .find((li) => (li.getAttribute('data-ids') || '').split(' ').includes(arg.id));
  if (!row) return false;
  return Math.abs(row.getBoundingClientRect().top - arg.before) <= arg.tolerance;
}"""

PLACE_JS = """(arg) => {
  const area = document.getElementById('scroll-area');
  const row = [...document.querySelectorAll('#discord-log > li')]
    .find((li) => (li.getAttribute('data-ids') || '').split(' ').includes(arg.id));
  if (!row) return false;
  area.scrollTop += row.getBoundingClientRect().top - arg.target;
  return true;
}"""

SPOKEN_JS = "() => window.deviceSpeech"
STARTED_JS = """(arg) => window.deviceSpeech.slice(arg.from).some(
  (one) => one.started !== null && arg.texts.includes(one.text))"""


@dataclass
class Walk:
    page: Page
    touch: bool
    size: str
    shots: Path | None
    notes: list[str] = field(default_factory=list)

    def check(self, condition: bool, why: str) -> None:
        if not condition:
            raise AssertionError(f"{self.size}: {why}")

    def wait_for(self, condition: str, why: str, arg: object = None,
                 seconds: float = browser_waits.HANG_BACKSTOP_MS / 1000) -> None:
        # The bound is the hang backstop (#2077), as in the other walks' shared waits.
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and not self.page.evaluate(condition, arg):
            self.page.wait_for_timeout(25)
        self.check(bool(self.page.evaluate(condition, arg)), why)

    def settled(self) -> None:
        self.page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")
        self.page.wait_for_timeout(120)

    def offset(self, msg_id: str) -> float:
        found = self.page.evaluate(OFFSET_JS, msg_id)
        self.check(isinstance(found, (int, float)), f"message {msg_id} has no row on screen")
        return float(found)

    def spoken(self) -> list[dict[str, object]]:
        found = self.page.evaluate(SPOKEN_JS)
        assert isinstance(found, list)
        return found

    def tap_badge(self, msg_id: str) -> None:
        locator = self.page.locator(
            f"#discord-log > li[data-ids~=\"{msg_id}\"] .thread-badge").first
        box = locator.bounding_box()
        self.check(box is not None, f"message {msg_id}'s Thread badge has no box on screen")
        assert box is not None
        x, y = box["x"] + box["width"] / 2, box["y"] + box["height"] / 2
        if self.touch:
            self.page.touchscreen.tap(x, y)
        else:
            self.page.mouse.click(x, y)

    def tap_row(self, msg_id: str) -> None:
        locator = self.page.locator(f"#discord-log > li[data-id=\"{msg_id}\"] .body").first
        box = locator.bounding_box()
        self.check(box is not None, f"message {msg_id}'s body has no box on screen")
        assert box is not None
        x, y = box["x"] + box["width"] / 2, box["y"] + box["height"] / 2
        if self.touch:
            self.page.touchscreen.tap(x, y)
        else:
            self.page.mouse.click(x, y)

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.size}-{name}.png"), full_page=False)


def reader_resume_walk(walk: Walk) -> None:
    page = walk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    walk.wait_for(f"() => !!document.querySelector('#discord-log > li[data-ids~=\"{ANCHOR}\"]')",
                  "the channel did not open")
    page.select_option("#thread-select", "flat")
    walk.wait_for(f"() => !!document.querySelector('#discord-log > li[data-ids~=\"{ANCHOR}\"] .thread-badge')",
                  "All did not open with the anchor reply")

    # The long reply at a measured place, read by the device voice with Autoplay on.
    walk.check(bool(page.evaluate(PLACE_JS, {"id": ANCHOR, "target": 300})),
               "the anchor reply could not be placed at 300px in All")
    walk.settled()
    before = walk.offset(ANCHOR)
    page.click("#read-aloud")
    walk.wait_for("() => document.getElementById('read-aloud').getAttribute('data-active') === 'true'",
                  "Read did not turn on")
    walk.tap_row(ANCHOR)
    walk.wait_for("() => window.deviceSpeech.length >= 1 && window.deviceSpeech[0].started !== null",
                  "the device voice did not start the long reply")
    page.click("#read-autoplay")
    walk.wait_for("() => document.getElementById('read-autoplay').getAttribute('aria-pressed') === 'true'",
                  "Autoplay did not turn on")
    walk.shot("1-all-reading")
    # Mid-message: the second chunk is being said.
    walk.wait_for("() => window.deviceSpeech.length >= 2 && window.deviceSpeech[1].started !== null"
                  " && window.deviceSpeech[1].ended === null",
                  "the read never reached its second chunk")
    chunks = [str(one["text"]) for one in walk.spoken()]
    walk.check(len(chunks) >= 2, "the long reply was not chunked")
    said_at_tap = len(walk.spoken())

    # Tap Thread mid-chunk: the thread opens on the anchor, still playing that chunk.
    walk.tap_badge(ANCHOR)
    walk.wait_for("() => !document.getElementById('thread-heading').hidden", "the thread did not open")
    walk.wait_for(WITHIN_JS, "the thread did not open with the clicked reply at its All offset",
                  {"id": ANCHOR, "before": before, "tolerance": TOLERANCE})
    walk.wait_for(STARTED_JS, "the reader did not resume in the thread",
                  {"from": said_at_tap, "texts": chunks[:2]})
    walk.settled()
    resumed = walk.spoken()[said_at_tap:]
    first_resumed = next((str(one["text"]) for one in resumed if one["started"] is not None), "")
    walk.check(first_resumed in chunks[:2],
               f"the thread resumed on {first_resumed!r}, not the chunk being read {chunks[1]!r} or the one before")
    walk.check(page.evaluate("() => document.getElementById('read-aloud').getAttribute('data-active')") == "true",
               "Read mode was left by the drill-in")
    walk.check(page.evaluate(f"() => document.querySelector('#discord-log > li[data-id=\"{ANCHOR}\"]')"
                             "?.getAttribute('data-reading')") == "true",
               "the anchor row in the thread is not marked as being read")
    walk.shot("2-thread-resumed")
    walk.notes.append(f"mid-chunk Thread tap: resumed in the thread on {first_resumed[:40]!r}... "
                      f"at the anchor's {before:.0f}px place")

    # The read goes on to the thread's next reply, as Read with Autoplay does in this view.
    next_first = NEXT_TEXT.split(". ")[0] + "."
    walk.wait_for(STARTED_JS, "the read did not go on to the thread's next reply",
                  {"from": said_at_tap, "texts": [next_first]})
    walk.wait_for("() => { const last = window.deviceSpeech[window.deviceSpeech.length - 1];"
                  " return last && last.started !== null && last.ended === null"
                  f" && last.text.startsWith({json.dumps('Following reply')}); }}",
                  "the next reply is not playing")
    walk.shot("3-thread-next")

    # Back out while it plays: the same reply is said again in All.
    said_at_back = len(walk.spoken())
    page.click("#thread-back")
    walk.wait_for("() => document.getElementById('thread-heading').hidden", "Back did not leave the thread")
    walk.wait_for(STARTED_JS, "the reader did not resume on Back",
                  {"from": said_at_back, "texts": [next_first]})
    walk.notes.append("Back while the next reply played: it was said again in All")
    walk.shot("4-all-resumed")

    # Drill in once more (the read resumes), then switch to Main, which does not show the
    # reply: the read stops cleanly and says so in place.
    walk.tap_badge(NEXT)
    walk.wait_for("() => !document.getElementById('thread-heading').hidden", "the thread did not reopen")
    said_at_picker = len(walk.spoken())
    page.select_option("#thread-select", "main")
    walk.wait_for("() => (document.getElementById('status')?.textContent || '')"
                  ".includes('not in this view')",
                  "the fail-safe did not say the message is not in this view")
    walk.settled()
    walk.check(len(walk.spoken()) <= said_at_picker + 1,
               "a read started on a message Main does not show")
    walk.check(page.evaluate("() => document.getElementById('read-aloud').getAttribute('data-active')") != "true",
               "Read mode stayed on after the fail-safe stop")
    walk.shot("5-main-stopped")
    walk.notes.append("switching to Main without the reply stopped the read and said so in place")


def serve(api: FakeApi, web_root: Path) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api, web_root))
    server.daemon_threads = True
    thread_ = threading.Thread(target=server.serve_forever, daemon=True)
    thread_.start()
    return server, thread_, f"http://127.0.0.1:{server.server_port}/"


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path,
                        help="directory to write one PNG per state and size into (default: none are kept)")
    return parser.parse_args()


def check(condition: bool, message_: str) -> None:
    if not condition:
        raise AssertionError(message_)


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    web_root = page_sources.served(args.web_root)
    sizes = (("412x915", 412, 915, True), ("1280x800", 1280, 800, False))
    report: list[str] = []
    with sync_playwright() as playwright:
        for size, width, height, phone in sizes:
            api = FakeApi()
            server, thread_, url = serve(api, web_root)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-reader-resume-") as profile:
                    if phone:
                        context = playwright.chromium.launch_persistent_context(
                            profile, headless=True, executable_path=args.browser_executable,
                            viewport={"width": width, "height": height}, color_scheme="dark",
                            user_agent=("Mozilla/5.0 (Linux; Android 15; Pixel 7) AppleWebKit/537.36 "
                                        "(KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36"),
                            device_scale_factor=2.625, is_mobile=True, has_touch=True,
                        )
                        browser_waits.hot_host_timeouts(context)
                    else:
                        context = playwright.chromium.launch_persistent_context(
                            profile, headless=True, executable_path=args.browser_executable,
                            viewport={"width": width, "height": height}, color_scheme="dark",
                        )
                        browser_waits.hot_host_timeouts(context)
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.add_init_script(DEVICE_VOICE)
                    page.add_init_script(
                        "try { localStorage.setItem('vibe-talk.welcome.dismissed', '1'); "
                        "} catch (e) {}")
                    page.goto(url, wait_until="load")
                    walk = Walk(page, phone, size, args.screenshots)
                    reader_resume_walk(walk)
                    check(not errors, f"{size}: the page threw: {errors}")
                    version = context.browser.version if context.browser else "Chromium"
                    context.close()
                    report.append(f"{version} at {size}: " + "; ".join(walk.notes))
            finally:
                api.stopping.set()
                server.shutdown()
                server.server_close()
                thread_.join()
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
