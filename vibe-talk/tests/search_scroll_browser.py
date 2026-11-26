#!/usr/bin/env python3
"""Entering a search stays on a visible hit, else goes to the newest hits, in real Chromium.

`#1734 vibe-talk-search-view-scroll-default`. The owner, 2026-11-16: entering search should
"stay where I am (if any message on screen is a scroll hit) or scroll to the bottom so I see
the most RECENT search hits first." Forty messages in one day; "needle" only in the last
fifteen, "haystack" in every one. At a desk (1280x800) and on a phone (412x915), in light
and dark, with the keyboard closed and open (a shrunken `visualViewport`, played the way
tests/keyboard_hides_bar_browser.py plays Safari's):

  read from the top, search "needle" -> the reader lands at the foot, newest hits first;
  clear, park mid-list, search "haystack" -> the row under the eye does not move.

Pass --screenshots DIR to keep a PNG of each stage (before/after, per theme, size, keyboard).
"""

from __future__ import annotations

import argparse
import os
import tempfile
import time
from datetime import datetime
from pathlib import Path
from typing import TYPE_CHECKING
from zoneinfo import ZoneInfo

import bookmark_browser as fixture
from bookmark_browser import AGENT, CHANNEL, TOKEN, FakeApi, Json, check, serve
import page_sources
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page, Playwright

ZONE = "America/New_York"
NOW = datetime(2026, 10, 12, 12, 0, tzinfo=ZoneInfo(ZONE))
OWNER = {"author": "owner", "author_id": "1000000000000000007", "author_is_bot": False}
KEYBOARD_PX = 340

# Safari's keyboard, as tests/keyboard_hides_bar_browser.py plays it.
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

STATE_JS = """() => {
  const area = document.getElementById('scroll-area');
  const shown = [...document.querySelectorAll('#discord-log > li[data-id]:not(.search-hidden)')];
  const clip = area.getBoundingClientRect();
  const visible = shown.filter((li) => { const r = li.getBoundingClientRect();
    return r.bottom > r.top && r.bottom > clip.top && r.top < clip.bottom; });
  const head = visible.length ? visible[0].getBoundingClientRect().top - clip.top : null;
  // `#2262`: the search's collapsed server line under the newest row is room past the rows' foot,
  // so the foot a search lands on is the rows' foot, above that line.
  const line = document.getElementById('server-search');
  const detail = document.getElementById('server-detail');
  let room = 0;
  if (line && !line.hidden && line.getClientRects().length && !(detail && !detail.hidden)) {
    let before = line.previousElementSibling;
    while (before && !before.getClientRects().length) before = before.previousElementSibling;
    const box = line.getBoundingClientRect();
    room = box.bottom + (parseFloat(getComputedStyle(line).marginBottom) || 0) - (before ? Math.min(before.getBoundingClientRect().bottom, box.top) : box.top);
  }
  return {top: area.scrollTop, foot: area.scrollHeight - area.clientHeight, footRoom: room,
    firstVisible: visible.length ? visible[0].dataset.id : null, firstTop: head,
    lastVisible: visible.length ? visible[visible.length - 1].dataset.id : null,
    shown: shown.length};
}"""


def number(state: Json, key: str) -> float:
    value = state.get(key)
    return float(value) if isinstance(value, (int, float)) else 0.0


def message_id(index: int) -> str:
    return f"88000000000000000{index:02d}"


class ScrollApi(FakeApi):
    """Forty messages today; needle in the last fifteen, haystack in all."""

    def __post_init__(self) -> None:
        for index in range(40):
            sent = NOW.replace(hour=8, minute=0).timestamp() + index * 300
            from datetime import datetime as _dt
            stamp = _dt.fromtimestamp(sent, tz=ZoneInfo("UTC")).isoformat().replace("+00:00", "Z")
            words = f"haystack note {index}" + (" needle" if index >= 25 else "")
            self.messages.append({
                "id": message_id(index), "channel_id": CHANNEL["id"], "timestamp": stamp,
                "spoken_time": "", "reply_to": None, **(OWNER if index % 2 else AGENT),
                "content": words,
            })

    def client_config(self) -> Json:
        return {**super().client_config(), "owner_author_id": OWNER["author_id"],
                "discuss_available": True}


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

    def until(self, script: str, why: str, seconds: float = 10) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and not self.page.evaluate(script):
            self.page.wait_for_timeout(25)
        self.check(bool(self.page.evaluate(script)), why)

    def state(self) -> Json:
        found = self.page.evaluate(STATE_JS)
        return found if isinstance(found, dict) else {}


def walk_through(walk: Walk, keyboard: bool, discuss: bool = False) -> None:
    page = walk.page
    if keyboard:
        page.evaluate(f"() => window.__iosKeyboard(window.innerHeight - {KEYBOARD_PX})")
        walk.settled()
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function("() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden", timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass  # no first-run Welcome on this path (#1781: a returning reader, a refusal, or a saved choice)
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    walk.until("() => document.querySelectorAll('#discord-log > li[data-id]').length === 40",
               "the channel did not open with forty messages")
    walk.settled()
    if discuss:
        # The Discuss half-pane open, as the standing matrix asks on the phone.
        if page.locator("#discuss-open").is_visible():
            page.click("#discuss-open")
        else:
            page.click("#action-menu-open")
            page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
            page.click("#discuss-menu-open")
        page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
        walk.settled()
    # Read from the top, where no needle stands.
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
    walk.settled()
    walk.shot("before-search-at-top")
    # No needle on screen at the top: entering the search goes to the foot.
    page.click("#search-toggle")
    page.fill("#search-field", "needle")
    walk.until("() => document.querySelectorAll('#discord-log > li[data-id]:not(.search-hidden)').length === 15",
               "the needle search did not filter to fifteen")
    walk.settled()
    state = walk.state()
    # `#2262`: on the newest line: at the list's foot, or at the rows' foot just above the search's
    # line there (that line is room past the rows' foot, as the right column's overscroll room is).
    walk.check(number(state, "foot") - number(state, "footRoom") - 2 <= number(state, "top") <= number(state, "foot") + 2,
               f"no hit on screen: reader at {state['top']}, foot is {state['foot']} (room past the rows' foot {state['footRoom']})")
    walk.check(state["lastVisible"] == message_id(39),
               f"the newest hit is not on screen: last visible is {state['lastVisible']}")
    walk.shot("after-search-no-hit-on-screen-bottom")
    # Clear, park mid-list, search haystack: a hit is on screen, so stay.
    page.click("#search-toggle")
    walk.until("() => document.querySelectorAll('#discord-log > li[data-id]:not(.search-hidden)').length === 40",
               "clearing the search did not show every message again")
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = Math.round((a.scrollHeight - a.clientHeight) / 2); }")
    walk.settled()
    parked = walk.state()
    walk.shot("before-search-hit-on-screen")
    page.click("#search-toggle")
    page.fill("#search-field", "haystack")
    walk.until("() => document.querySelectorAll('#discord-log > li[data-id]:not(.search-hidden)').length === 40",
               "the haystack search did not keep every message")
    walk.settled()
    state = walk.state()
    walk.check(state["firstVisible"] == parked["firstVisible"],
               f"the row at the head of the screen changed: {parked['firstVisible']} -> {state['firstVisible']}")
    walk.check(abs(number(state, "firstTop") - number(parked, "firstTop")) <= 2,
               f"a hit was on screen but the row under the eye moved from {parked['firstTop']} to {state['firstTop']}")
    walk.shot("after-search-hit-on-screen-stayed")
    if keyboard:
        page.evaluate("() => window.__iosClose()")


def launch(playwright: Playwright, args: argparse.Namespace, profile: str, width: int, height: int,
           touch: bool, scheme: str) -> BrowserContext:
    context = playwright.chromium.launch_persistent_context(
        profile, headless=True, executable_path=args.browser_executable, timezone_id=ZONE,
        viewport={"width": width, "height": height}, color_scheme=scheme,  # type: ignore[arg-type]
        device_scale_factor=2.625 if touch else 1, is_mobile=touch, has_touch=touch,
    )
    browser_waits.hot_host_timeouts(context)
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
                    api = ScrollApi()
                    server, thread, url = serve(api, web_root)
                    try:
                        with tempfile.TemporaryDirectory(prefix="vibe-talk-search-scroll-") as profile:
                            context = launch(playwright, args, profile, width, height, touch, scheme)
                            context.clock.set_fixed_time(NOW)
                            page = context.pages[0]
                            errors: list[str] = []
                            page.on("pageerror", lambda error: errors.append(str(error)))
                            page.goto(url, wait_until="load")
                            name = f"{scheme}-{label}-{'keyboard' if keyboard else 'nokeyboard'}"
                            walk = Walk(page, name, args.screenshots)
                            walk_through(walk, keyboard, discuss=touch)
                            check(not errors, f"{name}: the page threw: {errors}")
                            context.close()
                            report.append(f"{name} {width}x{height}: stay-on-hit and go-to-newest both held")
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
