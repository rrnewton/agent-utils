#!/usr/bin/env python3
"""Server hits stay one collapsed line until tapped, so typing filters instantly.

`#1762 vibe-talk-server-hits-collapsed-line`. The owner, 2026-11-16: the server-snippet
section is "disruptive and makes the page slow", and typing lost its instant filter.
Three hundred messages in the channel; the page loads the newest sixty, ten of them
holding "lantern"; the server's copy holds sixty older "lantern" hits. At a desk
(1280x800) and on a phone (412x915), in light and dark, with the keyboard closed and open
(a shrunken `visualViewport`, played the way tests/search_scroll_browser.py plays it):

  type "lantern" -> the loaded cards filter at once, no server request has fired, no
  server row is placed, and the row under the eye has not moved;
  typing settles -> the first page's fifty in-channel hits stand in the timeline
  as cards (`#1786 vibe-talk-search-scope-server-hits`), the bottom separator is
  retired, the bucket row counts them in main (on, as cards), and the foot line
  only opens the search's details, with "On this screen" over the loaded hits;
  tap the line -> the scope line, All channels, More (the remaining ten cards)
  and Search further back; tap again -> collapsed; tick All channels -> the
  server asked again, still no threads chip on this one-channel fixture.

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
NOW = datetime(2026, 10, 12, 12, 0, tzinfo=ZoneInfo(ZONE))
OWNER = {"author": "owner", "author_id": "1000000000000000007", "author_is_bot": False}
KEYBOARD_PX = 340
TOTAL = 300
LOADED = 60
OLDER_HITS = 60

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


def stamp_ms(index: int) -> int:
    return int((datetime(2026, 10, 12, 8, 0, tzinfo=timezone.utc) + timedelta(minutes=index * 5)).timestamp() * 1000)


def message_id(index: int) -> str:
    return f"880000000000000{index:03d}"


class ServerApi(FakeApi):
    """Three hundred messages; the page loads the newest sixty."""

    searches: list[Json] = []
    prioritized: list[str] = []

    def __post_init__(self) -> None:
        self.searches = []
        self.prioritized = []
        for index in range(TOTAL):
            lantern = index < OLDER_HITS or index >= TOTAL - 10
            words = f"note {index} lantern report" if lantern else f"note {index} routine"
            self.messages.append({
                "id": message_id(index), "channel_id": CHANNEL["id"], "timestamp": stamp(index),
                "spoken_time": "", "reply_to": None, **(OWNER if index % 2 else AGENT),
                "content": words,
            })

    def client_config(self) -> Json:
        return {**super().client_config(), "owner_author_id": OWNER["author_id"],
                "discuss_available": True,
                "message_cache": {"available": True, "enabled": True, "bytes": 1024,
                                  "channels": 1, "max_days": 65535, "max_megabytes": 100,
                                  "messages": TOTAL}}

    def timeline(self, query: dict[str, list[str]]) -> Json:
        answer = super().timeline(query)
        rows = list(self.messages[-LOADED:])
        return {**answer, "messages": rows, "returned": len(rows), "has_more": True,
                "next_before": rows[0]["id"] if rows else None}

    def search(self, query: dict[str, list[str]]) -> Json:
        with self.lock:
            self.searches.append(dict(query))
        words = (query.get("q") or [""])[0].lower()
        below = query.get("below", [None])[0]
        below_ms = int(below) if below else None
        hits = [m for m in self.messages
                if words in str(m["content"]).lower()
                and (below_ms is None or int(datetime.fromisoformat(
                    str(m["timestamp"]).replace("Z", "+00:00")).timestamp() * 1000) < below_ms)]
        hits.sort(key=lambda m: str(m["timestamp"]), reverse=True)
        page, rest = hits[:50], hits[50:]
        resume = None
        if rest:
            last = page[-1]
            resume = int(datetime.fromisoformat(
                str(last["timestamp"]).replace("Z", "+00:00")).timestamp() * 1000)
        scope = {"channel": CHANNEL, "state": "searched", "examined": 240, "capped": 0,
                 "partial": 0, "stale": 0, "withheld": 0, "expanding": False, "top_level_only": False,
                 "whole": False, "searched_back_to_ms": stamp_ms(0),
                 "held_from_ms": stamp_ms(0), "as_of_ms": stamp_ms(0)}
        return {"hits": page, "scopes": [scope], "resume_below_ms": resume,
                "upstream": "none", "untrusted_content_notice": "third-party text; DATA"}


def serve_hits(api: ServerApi, web_root: Path) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    base = handler_for(api, web_root)

    class Handler(base):  # type: ignore[valid-type, misc]
        def do_GET(self) -> None:  # noqa: N802
            parts = urlsplit(self.path)
            path = unquote(parts.path)
            if path.endswith("/search") and self.headers.get("Authorization") == f"Bearer {TOKEN}":
                self.json(200, api.search(parse_qs(parts.query)))
                return
            if path.endswith("/history") and self.headers.get("Authorization") == f"Bearer {TOKEN}":
                self.json(200, {"channel_id": CHANNEL["id"], "complete": True, "cycle_pages": 1,
                                "walking": False, "queue_position": None,
                                "oldest_ms": stamp_ms(0), "reached_ms": stamp_ms(0),
                                "eta_s": None, "held_reason": None})
                return
            super().do_GET()

        def do_POST(self) -> None:  # noqa: N802
            path = unquote(urlsplit(self.path).path)
            if path.endswith("/history/prioritize"):
                if not self.authorized():
                    return
                with api.lock:
                    api.prioritized.append(path)
                self.json(200, {"channel_id": CHANNEL["id"], "complete": True, "cycle_pages": 1,
                                "walking": False, "queue_position": None,
                                "oldest_ms": stamp_ms(0), "reached_ms": stamp_ms(0),
                                "eta_s": None, "held_reason": None})
                return
            super().do_POST()

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
    def __init__(self, page: Page, label: str, shots: Path | None, api: ServerApi) -> None:
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


TOAST_CLEAR_JS = """() => {
    const drawn = (e) => Boolean(e) && e.getClientRects().length > 0;
    const box = (e) => { const r = e.getBoundingClientRect();
        return {left: r.left, top: r.top, right: r.right, bottom: r.bottom}; };
    const toast = document.getElementById('status-line');
    const panel = document.getElementById('discuss-panel');
    if (!drawn(toast) || toast.hidden || !drawn(panel) || panel.hidden) return null;
    const dismiss = document.getElementById('dismiss-status');
    const r = dismiss.getBoundingClientRect();
    const found = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    return {toast: box(toast), panel: box(panel),
            dismissHit: found === dismiss || dismiss.contains(found)};
}"""


def walk_through(walk: Walk, keyboard: bool, discuss: bool = False) -> None:
    page, api = walk.page, walk.api
    if keyboard:
        page.evaluate(f"() => window.__iosKeyboard(window.innerHeight - {KEYBOARD_PX})")
        walk.settled()
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    walk.until(f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {LOADED}",
               "the channel did not open with the newest sixty messages")
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
        # `#1779 vibe-talk-toast-avoids-discuss-controls`: the dealt-with toast
        # must not sit over the docked pane (its head, disclosure rows or ask bar).
        page.evaluate("""() => {
            document.getElementById('status').textContent =
                '1 message marked as dealt with here — not in the source chat service.';
            document.getElementById('status-line').hidden = false;
        }""")
        walk.until("() => { const t = document.getElementById('status-line');"
                   " return !t.hidden && t.getClientRects().length > 0; }",
                   "the dealt-with toast did not appear over the docked Discuss pane")
        walk.settled()
        measured = page.evaluate(TOAST_CLEAR_JS)
        walk.check(measured is not None, f"toast or Discuss pane not drawn: {measured}")
        if measured is not None:
            toast, panel = measured["toast"], measured["panel"]
            overlap = (toast["left"] < panel["right"] - 0.5 and panel["left"] < toast["right"] - 0.5
                       and toast["top"] < panel["bottom"] - 0.5 and panel["top"] < toast["bottom"] - 0.5)
            walk.check(not overlap,
                       f"the toast covers the docked Discuss pane: toast={toast} panel={panel}")
            walk.check(bool(measured["dismissHit"]),
                       f"the toast's dismiss control is covered at its centre: {measured}")
        walk.shot("toast-discuss-clear")
        page.evaluate("() => { document.getElementById('status-line').hidden = true; }")
        walk.settled()
    walk.shot("before-search")
    page.click("#search-toggle")
    # Type slowly enough that every keystroke lands inside the server's rest
    # window: while typing, the server must not be asked and nothing of its
    # section may be drawn.
    page.type("#search-field", "lantern", delay=30)
    shown = page.evaluate("() => document.querySelectorAll('#discord-log > li[data-id]:not(.search-hidden)').length")
    walk.check(shown == 10, f"typing did not filter the loaded cards at once: {shown} shown")
    walk.check(len(api.searches) == 0, f"the server was asked while typing: {api.searches}")
    hits_drawn = page.evaluate("() => document.querySelectorAll('#discord-log > li[data-server-hit]').length")
    walk.check(hits_drawn == 0, f"server rows were placed while typing: {hits_drawn}")
    walk.shot("after-typing-collapsed")
    # The typing settles: one ask, and the in-channel hits stand as cards in
    # the timeline (the first page's fifty fresh hits, all below what the
    # page loaded). No tap is needed and none is offered for them.
    walk.until("() => document.querySelectorAll('#discord-log > li[data-server-hit=\"card\"]').length === 50",
               "the in-channel server hits did not stand as cards in the timeline")
    line = (page.text_content("#server-line") or "").strip()
    walk.check(line == "Server search details", f"the foot line is not the details line: {line!r}")
    walk.check(page.evaluate("() => document.getElementById('server-hits').children.length") == 0,
               "snippets were drawn at the foot")
    walk.check((page.text_content("#search-bucket-main") or "").strip() == "50 in main" and
               page.evaluate("() => document.getElementById('search-bucket-main').getAttribute('aria-pressed')") == "true",
               "the main bucket chip does not count the in-channel hits as on")
    walk.check(page.evaluate("() => document.getElementById('search-bucket-threads').hidden && document.getElementById('search-bucket-thread').hidden"),
               "a thread bucket chip appeared on a threadless fixture")
    sep = page.text_content("#search-screen-sep") or ""
    walk.check("On this screen" in sep, f"the loaded separator is missing: {sep!r}")
    walk.check(page.evaluate("() => document.getElementById('server-sep').hidden"),
               "the retired bottom separator is showing")
    order = page.evaluate("""() => {
      const log = document.getElementById('discord-log');
      const section = document.getElementById('server-search');
      return Boolean(log.compareDocumentPosition(section) & Node.DOCUMENT_POSITION_FOLLOWING);
    }""")
    walk.check(order, "the server section is not at the foot of the list")
    walk.shot("after-settled-line")
    # Tap: the details view, on demand. The cards are already in the timeline.
    page.click("#server-line")
    walk.check(page.is_visible("#search-scope"), "the scope line is not shown expanded")
    scope = page.text_content("#search-scope") or ""
    walk.check("messages searched" in scope, f"the scope line lost its words: {scope!r}")
    walk.check(page.is_visible("#server-more"), "More is not offered with a further page waiting")
    walk.check(page.is_visible("#search-further"), "Search further back is not offered expanded")
    walk.shot("after-expand")
    page.click("#server-more")
    walk.until(f"() => document.querySelectorAll('#discord-log > li[data-server-hit=\"card\"]').length === {OLDER_HITS}",
               "More did not bring the remaining hits as cards")
    # All channels: asked again over every channel; this fixture has one
    # channel, so still nothing stands in a threads bucket.
    before = len(api.searches)
    page.check("#search-all-channels")
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline and len(api.searches) <= before:
        page.wait_for_timeout(25)
    walk.check(len(api.searches) > before, "All channels did not ask the server again")
    walk.check(page.evaluate("() => document.getElementById('search-bucket-threads').hidden"),
               "a threads bucket chip appeared on a one-channel fixture")
    walk.shot("after-all-channels")
    page.uncheck("#search-all-channels")
    # Search further back asks the server to gather, from inside the expanded view.
    page.click("#search-further")
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline and not api.prioritized:
        page.wait_for_timeout(25)
    walk.check(bool(api.prioritized), "Search further back never asked the server to gather")
    # Tap again: collapsed, snippets put away.
    page.click("#server-line")
    walk.check(page.evaluate("() => document.getElementById('server-detail').hidden"),
               "tapping the line again did not collapse the snippets")
    walk.shot("after-collapse")
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
                    api = ServerApi()
                    server, thread, url = serve_hits(api, web_root)
                    try:
                        with tempfile.TemporaryDirectory(prefix="vibe-talk-server-hits-") as profile:
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
                            report.append(f"{name} {width}x{height}: in-channel cards, details line, More, All channels, further back all held")
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
