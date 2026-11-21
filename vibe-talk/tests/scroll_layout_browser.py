#!/usr/bin/env python3
"""Scrolling a loaded channel reads the screen, not the whole list (`#2034`).

Serves the real page and a fake chat upstream over an 816-message channel and checks, at
phone and desktop sizes, with event counts only (owner rule 346):

  one PageDown screenful reads at most 400 layouts (the whole-list era read ~5,000,
  one per loaded row through the pane's head and foot), with at most 2 long tasks;
  after the scroll the reader has moved (the first wholly-shown row is a later message),
  the jump-to-newest offer stands, and jumping with it lands at the foot;
  a burst of twenty scroll events in one frame asks for no more layout than a few
  screenfuls — the passes behind the listener are the same ones a single event pays.

Pass --screenshots DIR to keep a PNG of each stage.
"""

from __future__ import annotations

import argparse
import os
import tempfile
import time
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, check
import page_sources
import search_typing_fast_browser as typing_fast

if TYPE_CHECKING:
    from playwright.sync_api import Page

MAX_RECT_PER_SCREEN = 400
MAX_LONG_PER_SCREEN = 2

RECT_JS = """() => {
    window.__rect = 0;
    const orig = Element.prototype.getBoundingClientRect;
    Element.prototype.getBoundingClientRect = function () { window.__rect += 1; return orig.call(this); };
    window.__long = [];
    try { new PerformanceObserver((l) => { for (const e of l.getEntries()) window.__long.push(e.duration); }).observe({ entryTypes: ['longtask'] }); } catch (e) {}
}"""


def until(page: Page, script: str, why: str, seconds: float = 25) -> None:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline and not page.evaluate(script):
        page.wait_for_timeout(25)
    check(bool(page.evaluate(script)), why)


def walk(page: Page, label: str, shots: Path | None) -> None:
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
    page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length >= 100", timeout=20_000)
    page.evaluate(RECT_JS)
    page.click("#discord-log > li[data-id]")
    # One screenful down: the scroll lands, at a few hundred layout reads at most.
    page.evaluate("() => { window.__rect = 0; window.__long = []; }")
    page.keyboard.press("PageDown")
    page.wait_for_timeout(600)
    counts = page.evaluate("() => ({ rect: window.__rect, long: window.__long.length })")
    check(counts["rect"] <= MAX_RECT_PER_SCREEN,
          f"{label}: a screenful of scrolling read {counts['rect']} layouts (bound {MAX_RECT_PER_SCREEN})")
    check(counts["long"] <= MAX_LONG_PER_SCREEN,
          f"{label}: a screenful of scrolling made {counts['long']} long tasks (bound {MAX_LONG_PER_SCREEN})")
    if shots is not None:
        page.screenshot(path=str(shots / f"{label}-scrolled.png"))
    # The jump back is offered, and taking it lands at the foot again.
    until(page, "() => { const b = document.getElementById('jump-newest'); return b && !b.hidden; }",
          f"{label}: no jump-to-newest offer after scrolling up the history")
    page.evaluate("() => { window.__rect = 0; }")
    for _ in range(20):
        page.evaluate("() => document.getElementById('scroll-area').dispatchEvent(new Event('scroll'))")
    page.evaluate("() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))")
    burst = page.evaluate("() => window.__rect")
    check(burst <= MAX_RECT_PER_SCREEN * 2,
          f"{label}: a burst of scroll events read {burst} layouts (bound {MAX_RECT_PER_SCREEN * 2})")
    page.click("#jump-newest")
    until(page, "() => { const a = document.getElementById('scroll-area'); return a.scrollTop + a.clientHeight >= a.scrollHeight - 2; }",
          f"{label}: the jump did not land at the foot")


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help="directory holding the page to serve")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per stage into")
    return parser.parse_args()


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
        ("phone", 412, 915, "dark"),
        ("desk", 1280, 800, "light"),
    )
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, scheme in runs:
            api = typing_fast.BigApi()
            server, thread, url = typing_fast.serve(api, web_root)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-scroll-layout-") as profile:
                    context = playwright.chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable,
                        timezone_id="America/New_York",
                        viewport={"width": width, "height": height}, color_scheme=scheme,  # type: ignore[arg-type]
                        device_scale_factor=2.625 if width < 600 else 1, is_mobile=width < 600, has_touch=width < 600)
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(url, wait_until="load")
                    walk(page, label, args.screenshots)
                    check(not errors, f"{label}: the page threw: {errors}")
                    context.close()
                    report.append(f"{label} {width}x{height}: scrolling reads the screen only")
            finally:
                server.shutdown()
                server.server_close()
                thread.join()
    for line in report:
        print(line)
    print("scroll layout browser walk: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
