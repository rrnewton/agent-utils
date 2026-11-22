#!/usr/bin/env python3
"""The X on the right of the search bar closes it and puts the view back, in real Chromium.

`#1968 vibe-talk-search-close-x`. The owner, 2026-11-20: closing search by tapping the
magnifier again is not obvious, so the open bar grows a prominent X at its right-hand end.
This drives the real page (fixture API only, served on a kernel-picked loopback port):

  at 412x915 and 1280x800, in light and dark, keyboard closed (and open on the phone,
  played as a shrunken `visualViewport` the way tests/search_scroll_browser.py plays it):
  sign in, park mid-list, open search from the magnifier, type a query that filters,
  then click `#search-close` -> the bar folds, the query is cleared, every row is back,
  and the reader is on the same row at the same scroll offset as before the search.
  The X itself: named "Close search", at least 44px square, the rightmost control in
  `#search-float`, overlapping neither the field nor any chip on the bar.

Pass --screenshots DIR to keep a PNG of the open bar and the restored view per profile.
"""

from __future__ import annotations

import argparse
import os
import tempfile
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
    return r.bottom > clip.top && r.top < clip.bottom; });
  const box = (el) => { const r = el.getBoundingClientRect();
    return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height}; };
  const close = document.getElementById('search-close');
  const bar = document.getElementById('search-float');
  const others = [...bar.children].filter((el) => el !== close && !el.hidden);
  return {top: area.scrollTop,
    firstVisible: visible.length ? visible[0].dataset.id : null,
    shown: shown.length,
    closeHidden: close.hidden, closeLabel: close.getAttribute('aria-label'),
    closeBox: box(close), barBox: box(bar),
    fieldHidden: document.getElementById('search-field').hidden,
    query: document.getElementById('search-field').value,
    searching: document.getElementById('screen-main').hasAttribute('data-searching'),
    overlap: others.some((el) => { const r = el.getBoundingClientRect(); const c = close.getBoundingClientRect();
      return r.left < c.right && c.left < r.right && r.top < c.bottom && c.top < r.bottom; })};
}"""


def message_id(index: int) -> str:
    return f"88000000000000000{index:02d}"


class CloseApi(FakeApi):
    """Forty messages today; needle in the last fifteen, haystack in all."""

    def __post_init__(self) -> None:
        for index in range(40):
            sent = NOW.replace(hour=8, minute=0).timestamp() + index * 300
            stamp = datetime.fromtimestamp(sent, tz=ZoneInfo("UTC")).isoformat().replace("+00:00", "Z")
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


def number(state: Json, key: str) -> float:
    value = state.get(key)
    return float(value) if isinstance(value, (int, float)) else 0.0


def box_of(state: Json, key: str) -> dict[str, float]:
    value = state.get(key)
    if not isinstance(value, dict):
        return {}
    return {str(k): float(v) if isinstance(v, (int, float)) else 0.0 for k, v in value.items()}


def walk(page: Page, label: str, shots: Path | None, keyboard: bool) -> None:
    def state() -> Json:
        found = page.evaluate(STATE_JS)
        return found if isinstance(found, dict) else {}

    def shot(name: str) -> None:
        if shots is not None:
            page.screenshot(path=str(shots / f"{label}-{name}.png"))

    if keyboard:
        page.evaluate(f"() => window.__iosKeyboard(window.innerHeight - {KEYBOARD_PX})")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function("() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden", timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass  # no first-run Welcome on this path (#1781)
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length === 40",
                           timeout=10_000)
    check(state().get("closeHidden") is True, f"{label}: the X stands with the search closed")

    # Park mid-list, then search from there: the place the X must put back.
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = Math.round((a.scrollHeight - a.clientHeight) / 2); }")
    page.wait_for_function("() => document.getElementById('scroll-area').scrollTop > 100", timeout=10_000)
    parked = state()
    page.click("#search-toggle")
    page.wait_for_selector("#search-close", state="visible", timeout=10_000)
    open_state = state()
    box = box_of(open_state, "closeBox")
    check(open_state.get("closeLabel") == "Close search", f"{label}: the X is named {open_state.get('closeLabel')!r}")
    check(box.get("width", 0.0) >= 44 and box.get("height", 0.0) >= 44,
          f"{label}: the X target is {box.get('width')}x{box.get('height')}, under 44px")
    bar = box_of(open_state, "barBox")
    check(abs(box.get("right", 0.0) - bar.get("right", 0.0)) <= 6,
          f"{label}: the X is not at the right of the bar ({box.get('right')} vs bar {bar.get('right')})")
    check(open_state.get("overlap") is not True, f"{label}: the X overlaps another control on the bar")
    shot("search-open")

    page.fill("#search-field", "needle")
    page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]:not(.search-hidden)').length === 15",
                           timeout=10_000)
    shot("search-typed")
    page.click("#search-close")
    page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]:not(.search-hidden)').length === 40",
                           timeout=10_000)
    page.wait_for_function("() => document.getElementById('search-field').hidden && document.getElementById('search-close').hidden",
                           timeout=10_000)
    after = state()
    check(after.get("query") == "", f"{label}: the X left the query {after.get('query')!r} in the field")
    check(after.get("searching") is not True, f"{label}: the X left the bar's room at the head of the list")
    check(after.get("firstVisible") == parked.get("firstVisible"),
          f"{label}: the row at the head changed {parked.get('firstVisible')} -> {after.get('firstVisible')}")
    check(abs(number(after, "top") - number(parked, "top")) <= 2,
          f"{label}: closing with the X moved the scroll from {number(parked, 'top')} to {number(after, 'top')}")
    shot("search-closed-by-x")
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
    profiles = (("phone", 412, 915, True, (False, True)), ("desk", 1280, 800, False, (False,)))
    with sync_playwright() as playwright:
        for scheme in ("light", "dark"):
            for label, width, height, touch, keyboards in profiles:
                for keyboard in keyboards:
                    api = CloseApi()
                    server, thread, url = serve(api, web_root)
                    try:
                        with tempfile.TemporaryDirectory(prefix="vibe-talk-search-close-") as profile:
                            context = launch(playwright, args, profile, width, height, touch, scheme)
                            context.clock.set_fixed_time(NOW)
                            page = context.pages[0]
                            errors: list[str] = []
                            page.on("pageerror", lambda error: errors.append(str(error)))
                            page.goto(url, wait_until="load")
                            name = f"{scheme}-{label}-{'keyboard' if keyboard else 'nokeyboard'}"
                            walk(page, name, args.screenshots, keyboard)
                            check(not errors, f"{name}: the page threw: {errors}")
                            context.close()
                            report.append(f"{name} {width}x{height}: the X closed search and restored the parked view")
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
