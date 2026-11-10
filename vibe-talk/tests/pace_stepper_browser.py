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

The gating verdict is that behaviour and nothing else (`#1250 vibe-talk-timing-robust`, the pattern of
`#1248 fold-latency-behaviour-verdict`): every step waits on the drawn state or the server-side ticket
it expects, so a hot host cannot fail it while the stepper behaves. How long the first ticket and the
mid-read retake took is printed on the report line but decides nothing; `--benchmark` enforces the old
wait bounds as an opt-in speed measurement outside the gate. The mid-read read is given blocks far
longer than the walk takes (`MID_READ_BLOCK_S`), so a host stall cannot end the read before the walk
presses + in it — the retake is then judged by the fresh ticket, not by winning a race.

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
import read_aloud_views_browser
from read_aloud_views_browser import VoicesApi, handler
from two_column_browser import Desk
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

DESK = (1280, 800)
# The smallest a touch target may be on a phone, in CSS px.
TOUCH_PX = 44
# The most a walk waits for a state it expects the page to reach (the precedent of
# read_aloud_views_browser.py's STATE_S, #998). Each such wait polls for the state, so a pass takes
# as long as the page does and the deadline only bounds a failure; the old 8 s ticket windows and
# the wait_for default of 10 s ran out under shared-gate load and passed solo.
STATE_S = 30.0
# The old wait bounds, kept only as the --benchmark budgets: the first ticket inside 8 s, and the
# mid-read retake inside the page's pace settle (READ_SPEED_SETTLE_MS, 600 ms in the page) with room
# for a loaded host — together 3 + 5 = 8 s, as this check waited before #1250.
BENCHMARK_S = 8.0
# Each block of the fake voice in this walk, far longer than the two presses made inside it: the
# read is still in progress whatever a loaded host does to this process between the tap that starts
# it and the + that must retake it. Set on the fake API's module in main(); its handler reads the
# module's BLOCK_SECONDS when each ticket is asked for.
MID_READ_BLOCK_S = 30.0


def box(page: Page) -> str:
    return str(page.evaluate("() => document.getElementById('read-speed-box').value"))


def shown(desk: Desk, value: str, why: str) -> None:
    """Wait until the box shows `value`: a key or a blur is judged by what it draws, whenever a
    loaded host lets the handler run, never by reading the box before it has."""
    desk.wait_for(f"() => document.getElementById('read-speed-box').value === '{value}'", why, STATE_S)


def wait_tickets(desk: Desk, api: VoicesApi, more_than: int, why: str) -> float:
    """Wait until the fake voice has been asked for message 205 more than `more_than` times, and
    say how long that took. The count is the behaviour; the seconds are printed on the report
    line and decide only a --benchmark run."""
    started = time.monotonic()
    deadline = started + STATE_S
    while time.monotonic() < deadline and len(api.tickets("205")) <= more_than:
        desk.page.wait_for_timeout(25)
    desk.check(len(api.tickets("205")) > more_than, why)
    return time.monotonic() - started


def wait_retake(desk: Desk, api: VoicesApi, snapshot: int, block: int | None, why: str) -> float:
    """Wait until a ticket asked for since the snapshot of `snapshot` tickets is for `block`, the
    block the read started on, and say how long that took. A count alone would pass on the page
    asking for a later block ahead of time; a pace change mid-read reads the block it is on again,
    so a fresh ticket for that same block is the behaviour."""
    started = time.monotonic()
    deadline = started + STATE_S

    def retaken() -> bool:
        return any(asked == block for asked, _ in api.tickets("205")[snapshot:])

    while time.monotonic() < deadline and not retaken():
        desk.page.wait_for_timeout(25)
    desk.check(retaken(), why)
    return time.monotonic() - started


def press(page: Page, selector: str, touch: bool) -> None:
    if touch:
        found = page.locator(selector).bounding_box()
        check(found is not None, f"{selector} is not drawn")
        assert found is not None
        page.touchscreen.tap(found["x"] + found["width"] / 2, found["y"] + found["height"] / 2)
    else:
        page.click(selector)


def walk(desk: Desk, api: VoicesApi, touch: bool, benchmark: bool) -> None:
    page = desk.page
    sign_in(desk)
    press(page, "#read-aloud", touch)
    desk.settled()
    press(page, "#read-speed", touch)
    desk.wait_for("() => !document.getElementById('speed-popover').hidden", "Pace did not open its popover",
                  STATE_S)
    desk.check(page.evaluate("() => !document.querySelector('#speed-popover input[type=range]')"),
               "the pace slider is still in the popover")
    shown(desk, "100%", "the box did not start at 100%")
    if touch:
        desk.wait_for("() => ['read-speed-down', 'read-speed-up', 'read-speed-box'].every((id) => { "
                      "const r = document.getElementById(id).getBoundingClientRect(); "
                      "return r.width > 0 && r.height > 0; })",
                      "the pace controls are not drawn", STATE_S)
        for selector in ("#read-speed-down", "#read-speed-up", "#read-speed-box"):
            found = page.locator(selector).bounding_box()
            assert found is not None
            desk.check(found["width"] >= TOUCH_PX and found["height"] >= TOUCH_PX,
                       f"{selector} is {found['width']:.0f}x{found['height']:.0f}px, under {TOUCH_PX}px")
    desk.shot("1-opened")

    for _ in range(3):
        press(page, "#read-speed-up", touch)
    desk.wait_for("() => document.getElementById('read-speed-box').value === '130%' "
                  "&& document.getElementById('read-speed-label').textContent === '130%' "
                  "&& localStorage.getItem('vibe-talk.voice.read-speed') === '130'",
                  "+ three times did not step the pace to 130%", STATE_S)
    desk.check(box(page) == "130%", f"+ three times gave {box(page)!r}, not 130%")
    stored = page.evaluate("() => localStorage.getItem('vibe-talk.voice.read-speed')")
    desk.check(stored == "130", f"the pace was stored as {stored!r}")
    label = page.evaluate("() => document.getElementById('read-speed-label').textContent")
    desk.check(label == "130%", f"the Pace button says {label!r}")
    desk.shot("2-plus-three-times")

    page.focus("#read-speed-box")
    page.keyboard.press("ArrowDown")
    shown(desk, "120%", "ArrowDown in the box did not step the pace down to 120%")
    page.keyboard.press("ArrowUp")
    page.keyboard.press("ArrowUp")
    shown(desk, "140%", "ArrowUp twice in the box did not step the pace up to 140%")
    for typed, expected in (("85", "85%"), ("115%", "115%"), ("300", "200%"), ("quick", "200%"), ("45 %", "50%")):
        page.fill("#read-speed-box", typed)
        page.keyboard.press("Enter")
        shown(desk, expected, f"typing {typed!r} and Enter did not leave the box at {expected!r}")
    page.fill("#read-speed-box", "90")
    page.locator("#read-speed-box").blur()
    shown(desk, "90%", "typing 90 and leaving the box did not leave it at 90%")
    desk.shot("3-typed")

    # With a message being read, + reads the block again at the new pace.
    press(page, "#read-speed", touch)
    desk.wait_for("() => document.getElementById('speed-popover').hidden", "Pace did not close its popover",
                  STATE_S)
    row = "#discord-log > li[data-id='205'] .body"
    page.evaluate("(selector) => document.querySelector(selector).scrollIntoView({block: 'center'})", row)
    desk.settled()
    before = len(api.tickets("205"))
    press(page, row, touch)
    first_s = wait_tickets(desk, api, before, "the message was never read")
    first_block = api.tickets("205")[before][0]
    desk.wait_for("() => { const row = document.querySelector(\"#discord-log > li[data-id='205']\"); "
                  "return row && row.getAttribute('data-reading') === 'true'; }",
                  "the message being read is not drawn as being read", STATE_S)
    asked = len(api.tickets("205"))
    press(page, "#read-speed", touch)
    desk.wait_for("() => !document.getElementById('speed-popover').hidden", "Pace did not open again", STATE_S)
    press(page, "#read-speed-up", touch)
    desk.shot("4-mid-read")
    retake_s = wait_retake(desk, api, asked, first_block,
                           "a pace changed mid-read did not read the block again")
    shown(desk, "100%", "+ from 90 did not leave the box at 100%")
    if benchmark:
        desk.check(first_s <= BENCHMARK_S,
                   f"the first ticket took {first_s:.2f} s, over the {BENCHMARK_S:.0f} s benchmark budget")
        desk.check(retake_s <= BENCHMARK_S,
                   f"the mid-read retake took {retake_s:.2f} s, over the {BENCHMARK_S:.0f} s benchmark budget")
    desk.notes.append("+ three times to 130%, arrows and typed values, clamped, and a mid-read + read the block "
                      f"again (first ticket in {first_s:.2f} s, mid-read retake in {retake_s:.2f} s)"
                      + (", every target at least 44 px" if touch else ""))


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
            walk(desk, api, touch, bool(args.benchmark))
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
    parser.add_argument("--benchmark", action="store_true",
                        help="also fail when the first ticket or the mid-read retake takes longer than the "
                             "old wait bounds: an opt-in speed measurement, never part of the gating "
                             "verdict (default: behaviour only)")
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
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    read_aloud_views_browser.BLOCK_SECONDS = MID_READ_BLOCK_S
    report = []
    with sync_playwright() as playwright:
        for touch in (True, False):
            report.append(run(playwright.chromium, args, touch))
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
