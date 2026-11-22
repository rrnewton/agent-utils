#!/usr/bin/env python3
"""Switching Main -> thread (and back) stays off the layout path (`#1920`).

`#1920 vibe-talk-main-thread-switch-and-scroll-lag`. The owner (thread 225, 2026-11-19):
"lag on Main to thread and scrolling". This walk serves the real page and a fake chat
upstream (only the upstream is stubbed; the timeline answers through the real route) over
a channel of 2000 messages holding one thread of 120 replies, and checks, at phone and
desktop sizes:

  parking mid-list in Main and picking the thread in the bar's real picker, then picking
  Main again, each does a bounded, O(1) number of synchronous layout reads
  (`getBoundingClientRect` calls) — the bound does not grow with the 2000 rows on screen,
  where the old anchor scan read one rectangle per row between the foot and the screen
  (about 1000 per switch on this fixture, about 2000 back); the thread view shows the
  thread's own rows (its root and replies, nothing else), and coming back to Main puts
  the reader on the same message at the same height they left;
  a burst of scrolling steps likewise reads a bounded number of layouts, where the old
  scan paid per row on every capture a held redraw made.

The gate is a count of synchronous work per action, never a wall-clock budget (owner
rule 346); every wait is a condition wait. Pass --screenshots DIR to keep a PNG of each
stage.
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, unquote, urlsplit

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, check, handler_for, message
import page_sources
import search_unmatched_browser as unmatched
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import Page

THREAD_ID = "spaces/A/threads/big"
MAIN_ROWS = 2000
THREAD_REPLIES = 120

# Synchronous layout reads allowed inside one view switch, however many rows stand on
# screen. The bounded switch measures only the screen's neighbourhood (binary searches
# and the rows on screen): about 100-170 on this fixture. Generous on purpose (rule
# 346) — a per-row scan reads 1000+ here and fails by an order of magnitude.
MAX_RECT_READS_PER_SWITCH = 500
# The same bound for a burst of five scroll steps over the long list.
MAX_RECT_READS_PER_SCROLL_BURST = 500

RECT_INSTALL_JS = """() => {
    window.__rectReads = 0;
    const orig = Element.prototype.getBoundingClientRect;
    Element.prototype.getBoundingClientRect = function () {
        window.__rectReads += 1;
        return orig.call(this);
    };
}"""

VISIBLE_JS = """() => {
    const box = document.getElementById('scroll-area').getBoundingClientRect();
    return [...document.querySelectorAll('#discord-log > li[data-id]')]
        .map((row) => ({id: row.getAttribute('data-id'), r: row.getBoundingClientRect()}))
        .filter(({r}) => r.bottom > box.top && r.top < box.bottom)
        .map(({id, r}) => ({id, top: r.top - box.top}));
}"""


class BigApi(unmatched.GapApi):
    """2000 routine messages; one thread root mid-list holding 120 replies."""

    def __init__(self) -> None:
        super().__init__()
        root_thread = {"id": THREAD_ID, "root_message_id": str(200 + 1000), "is_root": True,
                       "reply_count": THREAD_REPLIES, "reply_count_exact": True}
        rows = []
        for index in range(MAIN_ROWS):
            if index == 1000:
                rows.append(message(index, "Thread root: planning the release lane, "
                                    "long discussion below. " * 3, root_thread))
            else:
                rows.append(message(index, f"Routine lane note {index}, filed and done. "
                                    + "Detail line. " * (index % 4)))
        for reply in range(THREAD_REPLIES):
            rows.append(message(MAIN_ROWS + reply,
                                f"Thread reply {reply}: follow-up detail. " + "More text. " * (reply % 3),
                                {**root_thread, "is_root": False}))
        self.messages = rows
        self.threads = [{"id": THREAD_ID, "root": rows[1000], "title": "Thread from 1200",
                         "reply_count": THREAD_REPLIES, "reply_count_exact": True,
                         "updated_at": rows[-1]["timestamp"]}]


def serve(api: BigApi) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    base = handler_for(api)

    class Handler(base):  # type: ignore[valid-type, misc]
        def do_GET(self) -> None:  # noqa: N802
            parts = urlsplit(self.path)
            path = unquote(parts.path)
            if path.endswith("/search") or path.endswith("/history"):
                with api.lock:
                    api.requests.append(f"GET {self.path}")
                if path.endswith("/search"):
                    self.json(200, api.search(path, parse_qs(parts.query)))
                    return
                self.json(200, api.history_state())
                return
            super().do_GET()

        def do_POST(self) -> None:  # noqa: N802
            path = unquote(urlsplit(self.path).path)
            if path.endswith("/history/prioritize"):
                if not self.authorized():
                    return
                self.json(202, api.history_state())
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
    def __init__(self, page: Page, label: str, shots: Path | None) -> None:
        self.page, self.label, self.shots = page, label, shots

    def check(self, condition: bool, why: str) -> None:
        check(condition, f"{self.label}: {why}")

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.label}-{name}.png"))

    def until(self, script: str, why: str, seconds: float = 20) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and not self.page.evaluate(script):
            self.page.wait_for_timeout(25)
        self.check(bool(self.page.evaluate(script)), why)

    def settled(self) -> None:
        self.page.evaluate("() => new Promise((done) => "
                           "requestAnimationFrame(() => requestAnimationFrame(done)))")

    def reads(self) -> int:
        return int(self.page.evaluate("() => window.__rectReads"))

    def reset_reads(self) -> None:
        self.page.evaluate("() => { window.__rectReads = 0; }")


def walk_through(walk: Walk) -> None:
    page = walk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function(
            "() => !document.getElementById('screen-welcome').hidden "
            "|| !document.getElementById('screen-main').hidden", timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass
    page.wait_for_selector("#search-toggle", state="visible", timeout=15_000)
    page.select_option("#thread-select", "main")
    walk.until(f"() => document.querySelectorAll('#discord-log > li[data-id]').length >= {MAIN_ROWS - 20}",
               "the big channel did not open in Main with its messages")
    page.evaluate(RECT_INSTALL_JS)
    # Park mid-list: the ordinary reader's place on a long channel, and the case the old
    # from-the-foot anchor scan paid a thousand reads for.
    page.evaluate("() => { const area = document.getElementById('scroll-area'); "
                  "area.scrollTop = area.scrollHeight / 2; }")
    page.wait_for_timeout(100)
    walk.settled()
    before = page.evaluate(VISIBLE_JS)
    walk.check(len(before) > 0, "no message stands on screen before the switch")

    # Main -> thread: bounded layout reads, and the thread's own rows are what is shown.
    walk.reset_reads()
    page.select_option("#thread-select", f"thread:{THREAD_ID}")
    walk.until(f"() => document.querySelectorAll('#discord-log > li[data-id]').length "
               f"=== {THREAD_REPLIES + 1}", "the thread view did not draw the thread's rows")
    walk.settled()
    to_thread = walk.reads()
    walk.check(to_thread <= MAX_RECT_READS_PER_SWITCH,
               f"switching Main -> thread read layout {to_thread} times over {MAIN_ROWS} rows "
               f"(bound {MAX_RECT_READS_PER_SWITCH} per switch, independent of the row count)")
    walk.shot("thread-view")

    # Thread -> Main: bounded too, and the reader is back on the message they left, unmoved.
    walk.reset_reads()
    page.select_option("#thread-select", "main")
    walk.until(f"() => document.querySelectorAll('#discord-log > li[data-id]').length >= {MAIN_ROWS - 20}",
               "coming back did not draw Main's rows again")
    walk.settled()
    to_main = walk.reads()
    walk.check(to_main <= MAX_RECT_READS_PER_SWITCH,
               f"switching thread -> Main read layout {to_main} times over {MAIN_ROWS} rows "
               f"(bound {MAX_RECT_READS_PER_SWITCH} per switch, independent of the row count)")
    now = {str(row["id"]): float(row["top"]) for row in page.evaluate(VISIBLE_JS)}
    shared = next(((str(row["id"]), float(row["top"])) for row in before if str(row["id"]) in now), None)
    walk.check(shared is not None, f"none of {before} is on screen after coming back: {now}")
    if shared is not None:
        walk.check(abs(now[shared[0]] - shared[1]) <= 1.5,
                   f"the reader's place moved: {shared[0]} from {shared[1]:.1f}px "
                   f"to {now[shared[0]]:.1f}px across Main -> thread -> Main")
    walk.shot("back-in-main")

    # Scrolling the long list: a burst of steps reads bounded layouts, not per-row work.
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
    page.wait_for_timeout(100)
    walk.reset_reads()
    for _ in range(5):
        page.evaluate("(step) => { document.getElementById('scroll-area').scrollTop += step; }", 600)
        page.wait_for_timeout(60)
    page.wait_for_timeout(200)
    burst = walk.reads()
    walk.check(burst <= MAX_RECT_READS_PER_SCROLL_BURST,
               f"five scroll steps read layout {burst} times over {MAIN_ROWS} rows "
               f"(bound {MAX_RECT_READS_PER_SCROLL_BURST} per burst, independent of the row count)")
    walk.shot("scrolled")


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required") from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    runs = (
        ("dark-phone", 412, 915, True, "dark"),
        ("light-desk", 1280, 800, False, "light"),
    )
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch, scheme in runs:
            api = BigApi()
            server, thread, url = serve(api)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-thread-switch-") as profile:
                    context = playwright.chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable,
                        timezone_id="America/New_York",
                        viewport={"width": width, "height": height}, color_scheme=scheme,  # type: ignore[arg-type]
                        device_scale_factor=2.625 if touch else 1, is_mobile=touch, has_touch=touch)
                    browser_waits.hot_host_timeouts(context)
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(url, wait_until="load")
                    walk = Walk(page, label, args.screenshots)
                    walk_through(walk)
                    check(not errors, f"{label}: the page threw: {errors}")
                    context.close()
                    report.append(f"{label} {width}x{height}: switch and scroll bounded per action, "
                                  "thread rows exact, reader's place kept")
            finally:
                server.shutdown()
                server.server_close()
                thread.join()
    for line in report:
        print(line)
    print("thread switch fast browser walk: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
