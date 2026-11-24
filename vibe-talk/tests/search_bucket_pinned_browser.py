#!/usr/bin/env python3
"""Bucket chips stay under the search bar, plainly pressed or not (`#1899`).

Follow-up to `#1894 vibe-talk-search-extra-hits-off-by-default`. The bucket
row (`#search-bucket-row`) stood at the top of the scrolling list and left
the screen with the first scroll, and pressed and unpressed chips read the
same. This walk serves the real page and a fake chat upstream (only the
upstream is stubbed; the server search answers through the real route) over
a channel whose server copy holds one "lantern" hit in main and two in
threads, among enough loaded messages to scroll at both sizes:

  search "lantern" -> "1 in main" pressed, "2 in threads" unpressed, the
  two told apart by filled ground, check and `aria-pressed` in light and
  dark; scroll the results to the bottom -> both chips still under the bar,
  on phone (keyboard closed and really open) and desktop; a new search
  ("old lantern") -> the extra chip unpressed again, home chip pressed.

Condition waits only; no wall-clock budgets beyond the page's own loads.
Pass --screenshots DIR to keep a PNG of each stage.
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
from offline_cache_browser import CHANNEL, TOKEN, FakeApi, Json, check, handler_for, message
import page_sources
import search_unmatched_browser as unmatched
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import Page, Playwright

IOS_VIEWPORT_JS = unmatched.__dict__.get("IOS_VIEWPORT_JS") or """(() => {
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

KEYBOARD_PX = 336

CHIP_JS = """(id) => {
    const chip = document.getElementById(id);
    if (!chip) return null;
    const r = chip.getBoundingClientRect();
    const cs = getComputedStyle(chip);
    const before = getComputedStyle(chip, '::before');
    return {text: (chip.textContent || '').trim(), pressed: chip.getAttribute('aria-pressed'),
            hidden: chip.hidden, bg: cs.backgroundColor, color: cs.color,
            check: before.visibility, left: r.left, top: r.top, right: r.right, bottom: r.bottom,
            width: r.width, height: r.height};
}"""

PIN_JS = """() => {
    const row = document.getElementById('search-bucket-row');
    const area = document.getElementById('scroll-area');
    const anchor = document.getElementById('channel-freshness-anchor');
    if (!row || !area || !anchor) return null;
    const r = row.getBoundingClientRect();
    return {inScroller: area.contains(row), inPanel: anchor.contains(row),
            top: r.top, bottom: r.bottom, scrollTop: area.scrollTop,
            scrollHeight: area.scrollHeight, innerHeight: window.innerHeight};
}"""


class PinnedApi(unmatched.GapApi):
    """The lantern channel, lengthened so both sizes must scroll."""

    def __init__(self) -> None:
        super().__init__()
        base = len(self.messages)
        for extra in range(44):
            self.messages.append(message(base + extra, f"Routine lane note {extra}, filed and done."))


def serve(api: PinnedApi, web_root: Path) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
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
                with api.lock:
                    api.history_asked.append(path)
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

    def settled(self) -> None:
        self.page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")

    def until(self, script: str, why: str, seconds: float = 15) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and not self.page.evaluate(script):
            self.page.wait_for_timeout(25)
        self.check(bool(self.page.evaluate(script)), why)


def walk_through(walk: Walk, keyboard: bool) -> None:
    page = walk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function(
            "() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden",
            timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    walk.until("() => document.querySelectorAll('#discord-log > li[data-id]').length > 20",
               "the channel did not open with its loaded messages")
    if keyboard:
        shrunk = page.evaluate("() => window.innerHeight") - KEYBOARD_PX
        page.evaluate(f"() => window.__iosKeyboard({shrunk})")
        walk.until(f"() => Math.abs(window.visualViewport.height - {shrunk}) < 1",
                   "the keyboard-open viewport never shrank")
        walk.settled()
    page.click("#search-toggle")
    page.fill("#search-field", "lantern")
    walk.until("() => { const r = document.getElementById('search-bucket-row');"
               " return r && !r.hidden && !document.getElementById('search-bucket-main').hidden"
               " && !document.getElementById('search-bucket-threads').hidden; }",
               "the bucket chips never appeared under the bar")
    main = page.evaluate(CHIP_JS, "search-bucket-main")
    threads = page.evaluate(CHIP_JS, "search-bucket-threads")
    walk.check(main is not None and main["text"] == "1 in main" and main["pressed"] == "true",
               f"the main chip says {main}")
    walk.check(threads is not None and threads["text"] == "2 in threads" and threads["pressed"] == "false",
               f"the threads chip says {threads}")
    walk.check(main["bg"] != threads["bg"] and main["color"] != threads["color"],
               f"pressed and unpressed read the same: main={main} threads={threads}")
    walk.check(main["check"] == "visible" and threads["check"] == "hidden",
               f"the check does not tell the states apart: main={main} threads={threads}")
    pin = page.evaluate(PIN_JS)
    walk.check(pin is not None and pin["inPanel"] and not pin["inScroller"],
               f"the bucket row is not pinned in the search panel: {pin}")
    # The shot starts from the head of the list, so the first row stands
    # whole under the chips rather than wherever the search landing left it.
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
    walk.settled()
    # `#2200`: the first result starts below the chip panel, never under it.
    first_below = page.evaluate("""() => {
      const row = document.getElementById('search-bucket-row');
      const first = [...document.querySelectorAll('#discord-log > li[data-id]')]
        .find((li) => li.getClientRects().length);
      return Boolean(first) && first.getBoundingClientRect().top >= row.getBoundingClientRect().bottom - 1;
    }""")
    walk.check(first_below, "the chip panel covers the first result")
    walk.shot("chips-under-bar")
    # Scroll the results to the bottom: the chips stay under the bar.
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    walk.until("() => { const a = document.getElementById('scroll-area');"
               " return a.scrollTop > 0 && a.scrollTop + a.clientHeight >= a.scrollHeight - 2; }",
               "the results never scrolled to the bottom")
    walk.settled()
    pin = page.evaluate(PIN_JS)
    main = page.evaluate(CHIP_JS, "search-bucket-main")
    walk.check(pin is not None and pin["top"] >= -1 and pin["bottom"] <= pin["innerHeight"] + 1,
               f"after scrolling to the bottom the chips left the screen: {pin}")
    walk.check(main is not None and main["height"] > 0 and main["top"] >= -1,
               f"after scrolling to the bottom the main chip is not visible: {main}")
    walk.shot("chips-after-scroll-bottom")
    # Turn the extra bucket on, so the new search below has something to reset.
    page.click("#search-bucket-threads")
    walk.until("() => document.getElementById('search-bucket-threads').getAttribute('aria-pressed') === 'true'",
               "tapping the threads chip did not press it")
    # A new search: extra buckets off again, home bucket on (`#1894`).
    page.fill("#search-field", "old lantern")
    walk.until("() => document.getElementById('search-bucket-threads')"
               " && document.getElementById('search-bucket-threads').getAttribute('aria-pressed') === 'false'",
               "the new search did not stand the threads chip back down")
    walk.settled()
    threads = page.evaluate(CHIP_JS, "search-bucket-threads")
    main = page.evaluate(CHIP_JS, "search-bucket-main")
    walk.check(threads is not None and threads["pressed"] == "false" and main is not None and main["pressed"] == "true",
               f"the new search states are main={main} threads={threads}")
    walk.shot("new-search-unpressed")
    # A search with no bucket hits: the pinned row does not render at all
    # (no blank panel over the head of the list, no space taken).
    page.fill("#search-field", "routine")
    walk.until("() => { const r = document.getElementById('search-bucket-row');"
               " return r && r.hidden && getComputedStyle(r).display === 'none'; }",
               "the empty bucket row still renders with no chips to show")
    walk.settled()
    walk.shot("no-bucket-hits-no-panel")
    if keyboard:
        page.evaluate("() => window.__iosClose()")


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required") from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    web_root = page_sources.served(args.web_root)
    runs = (
        ("dark-phone", 412, 915, True, "dark", False),
        ("dark-phone-keyboard", 412, 915, True, "dark", True),
        ("light-phone", 412, 915, True, "light", False),
        ("dark-desk", 1280, 800, False, "dark", False),
        ("light-desk", 1280, 800, False, "light", False),
    )
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch, scheme, keyboard in runs:
            api = PinnedApi()
            server, thread, url = serve(api, web_root)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-bucket-pinned-") as profile:
                    context = playwright.chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable,
                        timezone_id="America/New_York",
                        viewport={"width": width, "height": height}, color_scheme=scheme,  # type: ignore[arg-type]
                        device_scale_factor=2.625 if touch else 1, is_mobile=touch, has_touch=touch)
                    browser_waits.hot_host_timeouts(context)
                    context.add_init_script(IOS_VIEWPORT_JS)
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(url, wait_until="load")
                    walk = Walk(page, label, args.screenshots)
                    walk_through(walk, keyboard)
                    check(not errors, f"{label}: the page threw: {errors}")
                    context.close()
                    report.append(f"{label} {width}x{height}: chips pinned under the bar, pressed told apart")
            finally:
                server.shutdown()
                server.server_close()
                thread.join()
    for line in report:
        print(line)
    print("search bucket pinned browser walk: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
