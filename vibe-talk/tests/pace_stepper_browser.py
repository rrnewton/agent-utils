#!/usr/bin/env python3
"""The Pace popover's [-] [100%] [+] stepper, in real Chromium on a phone with touch and on a desk.

`#567 pace-stepper`. The owner, 2026-10-26: "When I pop open Pace the slider is actually SUPER annoying.
In fact let's completely get rid of it. Let's have a text box in the center for the percent, initially
storing "100%". Then buttons flanking it for (-) and (+). Popping it open and clicking plus a few times is
what I want."

It serves the checked-out page and the fake API of tests/read_aloud_views_browser.py (the agent's voice
streamed at speaking pace), and at 393x852 with touch, then at 1280x800 with a mouse: Read on, Pace
opened, no slider in it, the box saying 100%, the buttons at least 44 px each way on the phone; + three
times to 130%; the arrow keys in the box; a typed value with and without "%", one out of range clamped,
one that is not a number put back; the pace kept where it is stored; and, with a message being read, a
press of + reading the block again on a fresh request.

Pass --screenshots DIR to keep a PNG of each step, and --web-root DIR to run against another copy of the
page (the one before #567 has no box, and fails at once).
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

import offline_cache_browser as fixture
from inline_reply_browser import PHONE, sign_in
from offline_cache_browser import check
from read_aloud_views_browser import VoicesApi, handler
from two_column_browser import Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

DESK = (1280, 800)
# The smallest a touch target may be on a phone, in CSS px.
TOUCH_PX = 44
# How long the page lets the pace rest before a read in progress takes it up (READ_SPEED_SETTLE_MS),
# with room for a loaded host.
SETTLE_S = 3.0


def box(page: Page) -> str:
    return str(page.evaluate("() => document.getElementById('read-speed-box').value"))


def press(page: Page, selector: str, touch: bool) -> None:
    if touch:
        found = page.locator(selector).bounding_box()
        check(found is not None, f"{selector} is not drawn")
        assert found is not None
        page.touchscreen.tap(found["x"] + found["width"] / 2, found["y"] + found["height"] / 2)
    else:
        page.click(selector)


def walk(desk: Desk, api: VoicesApi, touch: bool) -> None:
    page = desk.page
    sign_in(desk)
    press(page, "#read-aloud", touch)
    desk.settled()
    press(page, "#read-speed", touch)
    desk.wait_for("() => !document.getElementById('speed-popover').hidden", "Pace did not open its popover")
    desk.check(page.evaluate("() => !document.querySelector('#speed-popover input[type=range]')"),
               "the pace slider is still in the popover")
    desk.check(box(page) == "100%", f"the box starts at {box(page)!r}, not 100%")
    if touch:
        for selector in ("#read-speed-down", "#read-speed-up", "#read-speed-box"):
            found = page.locator(selector).bounding_box()
            assert found is not None
            desk.check(found["width"] >= TOUCH_PX and found["height"] >= TOUCH_PX,
                       f"{selector} is {found['width']:.0f}x{found['height']:.0f}px, under {TOUCH_PX}px")
    desk.shot("1-opened")

    for _ in range(3):
        press(page, "#read-speed-up", touch)
    desk.settled()
    desk.check(box(page) == "130%", f"+ three times gave {box(page)!r}, not 130%")
    stored = page.evaluate("() => localStorage.getItem('vibe-talk.voice.read-speed')")
    desk.check(stored == "130", f"the pace was stored as {stored!r}")
    label = page.evaluate("() => document.getElementById('read-speed-label').textContent")
    desk.check(label == "130%", f"the Pace button says {label!r}")
    desk.shot("2-plus-three-times")

    page.focus("#read-speed-box")
    page.keyboard.press("ArrowDown")
    desk.check(box(page) == "120%", f"ArrowDown in the box gave {box(page)!r}")
    page.keyboard.press("ArrowUp")
    page.keyboard.press("ArrowUp")
    desk.check(box(page) == "140%", f"ArrowUp twice gave {box(page)!r}")
    for typed, shown in (("85", "85%"), ("115%", "115%"), ("300", "200%"), ("quick", "200%"), ("45 %", "50%")):
        page.fill("#read-speed-box", typed)
        page.keyboard.press("Enter")
        desk.check(box(page) == shown, f"typing {typed!r} and Enter gave {box(page)!r}, not {shown!r}")
    page.fill("#read-speed-box", "90")
    page.locator("#read-speed-box").blur()
    desk.check(box(page) == "90%", f"typing 90 and leaving the box gave {box(page)!r}")
    desk.shot("3-typed")

    # With a message being read, + reads the block again at the new pace.
    press(page, "#read-speed", touch)
    desk.wait_for("() => document.getElementById('speed-popover').hidden", "Pace did not close its popover")
    row = "#discord-log > li[data-id='205'] .body"
    page.evaluate("(selector) => document.querySelector(selector).scrollIntoView({block: 'center'})", row)
    desk.settled()
    before = len(api.tickets("205"))
    press(page, row, touch)
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline and len(api.tickets("205")) <= before:
        page.wait_for_timeout(25)
    desk.check(len(api.tickets("205")) > before, "the message was never read")
    asked = len(api.tickets("205"))
    press(page, "#read-speed", touch)
    desk.wait_for("() => !document.getElementById('speed-popover').hidden", "Pace did not open again")
    press(page, "#read-speed-up", touch)
    desk.shot("4-mid-read")
    deadline = time.monotonic() + SETTLE_S + 5
    while time.monotonic() < deadline and len(api.tickets("205")) <= asked:
        page.wait_for_timeout(25)
    desk.check(len(api.tickets("205")) > asked, "a pace changed mid-read did not read the block again")
    desk.check(box(page) == "100%", f"+ from 90 gave {box(page)!r}")
    desk.notes.append("+ three times to 130%, arrows and typed values, clamped, and a mid-read + read the block "
                      "again" + (", every target at least 44 px" if touch else ""))


def run(chromium: BrowserType, args: argparse.Namespace, touch: bool) -> str:
    api = VoicesApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    width, height = PHONE if touch else DESK
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-pace-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark", has_touch=touch, is_mobile=touch)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"pace-{width}x{height}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            walk(desk, api, touch)
            check(not errors, f"{width}x{height}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write the walk's PNGs into (default: none)")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required: python3 -m pip install --user playwright && "
                         "python3 -m playwright install chromium") from error
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = args.web_root.resolve()
    report = []
    with sync_playwright() as playwright:
        for touch in (True, False):
            report.append(run(playwright.chromium, args, touch))
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
