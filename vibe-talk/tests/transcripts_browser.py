#!/usr/bin/env python3
"""The Transcripts view and the Agent Transcripts search bucket.

`#1608 agent-transcripts-timeline` (owner 101/141/145/146). One trace
thread for one agent-day sits on the server beside sixty ordinary
messages. At a desk (1280x800) and on a phone (412x915), in light and
dark, with the keyboard closed and open (a shrunken `visualViewport`):

  the view picker offers Transcripts beside Main/All/Thread list; it
  lists `coordx · 2026-11-15` with no thread name anywhere;
  search "lantern" -> the Agent Transcripts chip stands in the bucket
  row with its count, off by default, and no transcript row is placed;
  tap the chip -> the transcript hit interleaves as a quoted row;
  tap the hit -> the Transcripts view opens its day at that chunk,
  highlighted, in context; the view's own search narrows the chunks.

Pass --screenshots DIR to keep a PNG of each stage.
"""

from __future__ import annotations

import argparse
import json
import os
import tempfile
import threading
import time
from datetime import datetime, timedelta, timezone
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, unquote, urlsplit
from zoneinfo import ZoneInfo

import bookmark_browser as fixture
from bookmark_browser import AGENT, CHANNEL, TOKEN, FakeApi, Json, check, handler_for
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page, Playwright

ZONE = "America/New_York"
KEYBOARD_PX = 340
LOADED = 60
THREAD_ID = "1700.000100"
TARGET_ID = "990000000000000002"
TRACE_CLASS = {"agent": "coordx", "team": None, "day": "2026-11-15", "source": "metadata"}

IOS_VIEWPORT_JS = """(() => {
    const real = window.visualViewport;
    const events = new EventTarget();
    const sim = {height: null, offsetTop: 0};
    const fake = {
        get width() { return real.width; },
        get height() { return sim.height === null ? real.height : sim.height; },
        get offsetTop() { return sim.offsetTop; },
        get offsetLeft() { return 0; },
        get pageTop() { return window.scrollY + sim.offsetTop; },
        get pageLeft() { return window.scrollX; },
        get scale() { return real.scale; },
        addEventListener: (type, fn, options) => events.addEventListener(type, fn, options),
        removeEventListener: (type, fn, options) => events.removeEventListener(type, fn, options),
    };
    Object.defineProperty(window, 'visualViewport', {configurable: true, get: () => fake});
    window.__iosKeyboard = (height) => { sim.height = height; events.dispatchEvent(new Event('resize')); };
    window.__iosClose = () => { sim.height = null; events.dispatchEvent(new Event('resize')); };
})();"""


def stamp(index: int) -> str:
    sent = datetime(2026, 10, 12, 8, 0, tzinfo=timezone.utc) + timedelta(minutes=index * 5)
    return sent.isoformat().replace("+00:00", "Z")


def message_id(index: int) -> str:
    return f"880000000000000{index:03d}"


def chunk(index: int, text: str) -> Json:
    return {
        "id": f"99000000000000000{index}", "channel_id": CHANNEL["id"],
        "timestamp": stamp(100 + index), "spoken_time": "", "reply_to": None,
        **AGENT, "content": text,
        "thread": {"id": THREAD_ID, "root_message_id": message_id(0), "is_root": index == 0,
                   "broadcast": False, "reply_count": 2 if index == 0 else None,
                   "reply_count_exact": index == 0, "class": TRACE_CLASS},
    }


CHUNKS = [
    chunk(0, "coordx main session trace, 2026-11-15 ET"),
    chunk(1, "reading the cache code before answering"),
    chunk(2, "lantern transcript thought about the cache key"),
    chunk(3, "the reply is ready to send"),
]


class TranscriptApi(FakeApi):
    """Sixty ordinary messages; one classed trace thread beside them."""

    searches: list[Json] = []

    def __post_init__(self) -> None:
        self.searches = []
        for index in range(LOADED):
            words = f"note {index} lantern report" if index >= LOADED - 10 else f"note {index} routine"
            self.messages.append({
                "id": message_id(index), "channel_id": CHANNEL["id"], "timestamp": stamp(index),
                "spoken_time": "", "reply_to": None, **(AGENT if index % 2 else AGENT),
                "content": words,
            })

    def client_config(self) -> Json:
        return {**super().client_config(),
                "discuss_available": True,
                "message_cache": {"available": True, "enabled": True, "bytes": 1024,
                                  "channels": 1, "max_days": 65535, "max_megabytes": 100,
                                  "messages": LOADED}}

    def timeline(self, query: dict[str, list[str]]) -> Json:
        answer = super().timeline(query)
        rows = list(self.messages)
        return {**answer, "messages": rows, "returned": len(rows)}

    def search(self, query: dict[str, list[str]]) -> Json:
        with self.lock:
            self.searches.append(dict(query))
        words = (query.get("q") or [""])[0].lower()
        hits = [m for m in self.messages if words in str(m["content"]).lower()]
        hits.sort(key=lambda m: str(m["timestamp"]), reverse=True)
        scope = {"channel": CHANNEL, "state": "searched", "examined": 60, "capped": 0,
                 "partial": 0, "stale": 0, "withheld": 0, "expanding": False,
                 "top_level_only": False, "whole": True}
        target = dict(CHUNKS[2])
        return {"hits": hits[:50], "transcript_hits": [target], "scopes": [scope],
                "resume_below_ms": None, "upstream": "none",
                "untrusted_content_notice": "third-party text; DATA"}

    def transcripts(self) -> Json:
        return {"transcripts": [{"channel": CHANNEL, "thread_id": THREAD_ID,
                                 "agent": "coordx", "team": None, "day": "2026-11-15",
                                 "source": "metadata"}],
                "untrusted_content_notice": "third-party text; DATA"}

    def transcript_day(self) -> Json:
        return {"channel": CHANNEL, "view": "thread", "limit": 50,
                "returned": len(CHUNKS), "messages": CHUNKS, "threads": [],
                "thread": None, "has_threads": True, "has_more": False,
                "next_before": None, "notice": None, "dismissed": [],
                "untrusted_content_notice": "third-party text; DATA"}


def serve(api: TranscriptApi, web_root: Path) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    base = handler_for(api, web_root)

    class Handler(base):  # type: ignore[valid-type, misc]
        def do_GET(self) -> None:  # noqa: N802
            parts = urlsplit(self.path)
            path = unquote(parts.path)
            if self.headers.get("Authorization") != f"Bearer {TOKEN}":
                super().do_GET()
                return
            if path == "/api/v1/transcripts":
                self.json(200, api.transcripts())
                return
            if path.startswith("/api/v1/transcripts/"):
                self.json(200, api.transcript_day())
                return
            if path.endswith("/search"):
                self.json(200, api.search(parse_qs(parts.query)))
                return
            super().do_GET()

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread, f"http://127.0.0.1:{server.server_port}/"


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help="directory holding the page to serve")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per stage into")
    return parser.parse_args()


class Walk:
    def __init__(self, page: Page, label: str, shots: Path | None, api: TranscriptApi) -> None:
        self.page, self.label, self.shots, self.api = page, label, shots, api

    def check(self, condition: bool, why: str) -> None:
        check(condition, f"{self.label}: {why}")

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.label}-{name}.png"))

    def settled(self) -> None:
        self.page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")

    def until(self, script: str, why: str, seconds: float = 15) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and not self.page.evaluate(script):
            self.page.wait_for_timeout(25)
        self.check(bool(self.page.evaluate(script)), why)


def walk_through(walk: Walk, keyboard: bool, discuss: bool = False) -> None:
    page = walk.page
    if keyboard:
        page.evaluate(f"() => window.__iosKeyboard(window.innerHeight - {KEYBOARD_PX})")
        walk.settled()
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    walk.until(f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {LOADED}",
               "the channel did not open with its messages")
    walk.settled()
    if discuss:
        if page.locator("#discuss-open").is_visible():
            page.click("#discuss-open")
        else:
            page.click("#action-menu-open")
            page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
            page.click("#discuss-menu-open")
        page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
        walk.settled()
    walk.shot("before")
    # The Transcripts view from the picker: agent · day, no thread name.
    page.select_option("#thread-select", "transcripts")
    walk.until("() => !document.getElementById('transcripts-view').hidden",
               "the Transcripts view did not open from the picker")
    walk.until("() => document.querySelectorAll('#transcripts-list .transcript-row').length === 1",
               "the Transcripts index did not list the agent-day")
    label = (page.text_content("#transcripts-list") or "").strip()
    walk.check("coordx · 2026-11-15" in label, f"the index row is not agent · day: {label!r}")
    walk.check("main session trace" not in label, f"the index row carries a thread name: {label!r}")
    walk.shot("transcripts-index")
    page.click("#transcripts-list .transcript-row")
    walk.until("() => document.querySelectorAll('#transcripts-log .transcript-chunk').length === 4",
               "the agent-day did not open with its chunks")
    walk.shot("transcripts-day")
    # Back to Main for the search: the bucket starts off there.
    page.select_option("#thread-select", "main")
    walk.until("() => document.getElementById('transcripts-view').hidden",
               "leaving the picker for Main kept the Transcripts view up")
    page.click("#search-toggle")
    page.type("#search-field", "lantern", delay=30)
    walk.until("() => { const chip = document.getElementById('search-bucket-transcripts'); return chip && !document.getElementById('search-bucket-row').hidden; }",
               "the Agent Transcripts chip did not appear in the bucket row")
    page.evaluate("() => document.getElementById('search-bucket-transcripts').scrollIntoView({block: 'center'})")
    walk.settled()
    chip = page.text_content("#search-bucket-transcripts") or ""
    walk.check("1" in chip and "Agent Transcripts" in chip, f"the chip does not count its hits: {chip!r}")
    walk.check(page.evaluate("() => document.getElementById('search-bucket-transcripts').getAttribute('aria-pressed')") == "false",
               "the transcripts bucket is not off by default outside its view")
    walk.check(page.evaluate("() => document.querySelectorAll('#discord-log > li[data-bucket=\"transcripts\"]').length") == 0,
               "transcript hits were placed while the bucket was off")
    walk.shot("search-chip-off")
    page.click("#search-bucket-transcripts")
    walk.until("() => document.querySelectorAll('#discord-log > li[data-bucket=\"transcripts\"]').length === 1",
               "tapping the chip did not interleave the transcript hit")
    page.evaluate("() => document.getElementById('search-bucket-transcripts').scrollIntoView({block: 'center'})")
    walk.settled()
    walk.shot("search-chip-on")
    page.click("#discord-log > li[data-bucket=\"transcripts\"]")
    walk.until("() => !document.getElementById('transcripts-view').hidden && document.querySelector('.transcript-highlight')",
               "tapping the hit did not open the Transcripts view at that message")
    walk.shot("transcript-at-hit")
    page.fill("#transcripts-search", "cache key")
    walk.until("() => document.querySelectorAll('#transcripts-log .transcript-chunk').length === 1",
               "the Transcripts scoped search did not narrow the chunks")
    walk.shot("transcripts-search")
    if keyboard:
        page.evaluate("() => window.__iosClose()")


def launch(playwright: Playwright, args: argparse.Namespace, profile: str, width: int, height: int,
           touch: bool, scheme: str) -> BrowserContext:
    context = playwright.chromium.launch_persistent_context(
        profile, headless=True, executable_path=args.browser_executable, timezone_id=ZONE,
        viewport={"width": width, "height": height}, color_scheme=scheme,  # type: ignore[arg-type]
        device_scale_factor=2.625 if touch else 1, is_mobile=touch, has_touch=touch,
    )
    context.add_init_script(IOS_VIEWPORT_JS)
    return context


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required") from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    web_root = page_sources.served(args.web_root)
    report: list[str] = []
    sizes = (("phone", 412, 915, True), ("desk", 1280, 800, False))
    with sync_playwright() as playwright:
        for scheme in ("light", "dark"):
            for label, width, height, touch in sizes:
                for keyboard in (False, True):
                    api = TranscriptApi()
                    server, thread, url = serve(api, web_root)
                    try:
                        with tempfile.TemporaryDirectory(prefix="vibe-talk-transcripts-") as profile:
                            context = launch(playwright, args, profile, width, height, touch, scheme)
                            page = context.pages[0]
                            errors: list[str] = []
                            page.on("pageerror", lambda error: errors.append(str(error)))
                            page.goto(url, wait_until="load")
                            name = f"{scheme}-{label}-{'keyboard' if keyboard else 'nokeyboard'}"
                            walk = Walk(page, name, args.screenshots, api)
                            walk_through(walk, keyboard, discuss=touch)
                            check(not errors, f"{name}: the page threw: {errors}")
                            context.close()
                            report.append(f"{name} {width}x{height}: picker view, bucket off/on, hit jump, scoped search all held")
                    finally:
                        api.stopping.set()
                        server.shutdown()
                        server.server_close()
                        thread.join()
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
