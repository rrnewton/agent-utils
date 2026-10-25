#!/usr/bin/env python3
"""`#501 thread-bridge-gutter` in real Chromium at 393x852, 412x915 and 1280x800.

The owner, 2026-10-25, of an unpacked thread on his phone: "In mobile we waste too much space
on this connector bridge. I would like to push it left so that the triangle is almost touching
the left screen edge. Also, if I am scrolled way down and I tap on the bridge itself, I would
like to scroll back up to the thread card at the top of the thread."

Against the fake API of tests/threads_polish_browser.py, in the Threads view with every thread
unpacked under its card:

* on each phone, the runner's first unpacked row: the pack triangle's ink a few pixels from the
  left screen edge on a target 32px at least each way; the bridge's spine left of the row by a
  short branch, in the card's colour (drawn, as the polish walk checks); the row itself a slim
  indent in from its card — clearly the card's, and materially wider than the full gutter left
  it; and no sideways scroll;
* on the desk, the same row keeps the wider indent it always had (negative control);
* scrolled way down the runner's rows until its card is off the top, a tap on the bridge's
  spine itself — not the triangle — glides back to the card: the card stands wholly at the
  head of the list, below its heading, and the thread is still unpacked;
* the triangle still packs its thread with the card where it was (`#440`, `#459`);
* under reduced motion the same tap still lands on the card (the fake-DOM suite pins the glide
  itself turning instant: tests/js/voice_page.test.mjs, `#501 thread-bridge-gutter`, which also
  covers the bridge of replies gathered in All going back to their parent).

Pass --screenshots DIR to keep a PNG of each state; --web-root DIR to run against another copy
of the page (an older one fails it at the gutter).
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from inline_reply_browser import sign_in
from offline_cache_browser import check, handler_for
from threads_polish_browser import PolishApi, to_threads, unpack_to
from two_column_browser import REPLICA, RUNNER, Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

SIZES = (("phone-393", 393, 852, True), ("phone-412", 412, 915, True), ("desk", 1280, 800, False))

# A thread's card, its first unpacked row, that row's bridge and triangle, and its scroller.
GUTTER_JS = """(id) => {
    const card = document.querySelector(`#side-cards > li[data-card][data-id='${id}']`);
    const rows = [...document.querySelectorAll(`#side-cards > li[data-unpack-thread='${id}']`)];
    const first = rows[0];
    const bridge = first.querySelector(':scope > .reply-bridge');
    const button = first.querySelector('.unpack-pack');
    const mark = button.querySelector('.unpack-pack-mark');
    const tri = getComputedStyle(mark, '::before');
    const triWidth = parseFloat(tri.borderLeftWidth) + parseFloat(tri.borderRightWidth);
    const box = (node) => { const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height}; };
    const markBox = box(mark);
    return {card: box(card), first: box(first), bridge: box(bridge), button: box(button),
            scroller: box(document.getElementById('side-scroll')),
            rows: rows.length, inkLeft: markBox.left + (markBox.width - triWidth) / 2,
            overflow: document.documentElement.scrollWidth - window.innerWidth};
}"""


def gutter(desk: Desk, thread_id: str) -> dict[str, object]:
    found = desk.page.evaluate(GUTTER_JS, thread_id)
    assert isinstance(found, dict)
    return found


def part(found: dict[str, object], name: str) -> dict[str, float]:
    piece = found[name]
    assert isinstance(piece, dict)
    return {str(key): float(value) for key, value in piece.items()}


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per state into (default: none)")
    parser.add_argument("--port", type=int, default=0,
                        help="loopback port for the fake server (default: 0, any free port)")
    parser.add_argument("--size", choices=[size[0] for size in SIZES], action="append",
                        help="walk only at these sizes, repeatable (default: all of them)")
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
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch in SIZES:
            if args.size and label not in args.size:
                continue
            report.append(walk(playwright.chromium, args, label, width, height, touch, reduced=False))
        if not args.size or "phone-393" in args.size:
            report.append(walk(playwright.chromium, args, "phone-393-reduced", 393, 852, True, reduced=True))
    for line in report:
        print(line)
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
         touch: bool, reduced: bool) -> str:
    api = PolishApi()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-bridge-gutter-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark",
                has_touch=touch, is_mobile=touch,
                reduced_motion="reduce" if reduced else "no-preference")
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"bridge-gutter-{label}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            sign_in(desk)
            to_threads(desk)
            unpack_to(desk, "true")
            desk.wait_for(f"() => document.querySelectorAll(\"#side-cards > li[data-unpack-thread='{RUNNER}']\").length >= 9",
                          "the runner did not unpack under its card")
            desk.settled()
            page.evaluate(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{RUNNER}']\")"
                          ".scrollIntoView({block: 'start'})")
            desk.settled()
            found = gutter(desk, RUNNER)
            card, first, bridge, button = (part(found, name) for name in ("card", "first", "bridge", "button"))
            ink_left = float(str(found["inkLeft"]))
            desk.check(float(str(found["overflow"])) <= 0, f"the page scrolls sideways: {found}")
            desk.check(button["width"] >= 32 and button["height"] >= 32,
                       f"the triangle's target is {button['width']:.0f}px by {button['height']:.0f}px")
            desk.check(bridge["left"] < first["left"], f"the spine is not left of its row: bridge {bridge}, row {first}")
            indent = first["left"] - card["left"]
            if touch:
                desk.check(0 <= ink_left <= 8, f"the triangle's ink starts {ink_left:.1f}px from the left edge")
                desk.check(button["left"] >= 0, f"the triangle's target runs off the screen at {button['left']:.1f}px")
                desk.check(6 <= first["left"] - bridge["left"] <= 13,
                           f"the bridge reaches {first['left'] - bridge['left']:.1f}px into its row")
                desk.check(18 <= indent <= 34, f"the unpacked rows hang {indent:.1f}px in from their card")
                desk.check(first["left"] <= 46, f"the unpacked rows start {first['left']:.1f}px from the left edge")
            else:
                desk.check(indent >= 60, f"the desk's indent shrank with the phone's: {indent:.1f}px")
            desk.shot("1-unpacked")
            desk.notes.append(f"triangle ink at {ink_left:.0f}px on a {button['width']:.0f}px target, rows "
                              f"{indent:.0f}px in from their card starting at {first['left']:.0f}px")

            # Way down the thread, a tap on the spine itself goes back to the card.
            page.evaluate(f"() => {{ const rows = document.querySelectorAll("
                          f"\"#side-cards > li[data-unpack-thread='{RUNNER}']\");"
                          " rows[rows.length - 1].scrollIntoView({block: 'end'}); }")
            desk.settled()
            before = gutter(desk, RUNNER)
            scroller = part(before, "scroller")
            desk.check(part(before, "card")["top"] < scroller["top"],
                       f"the premise: the card is still on screen at {part(before, 'card')['top']:.0f}px")
            desk.shot("2-scrolled-down")
            spot = page.evaluate(f"""() => {{ const rows = [...document.querySelectorAll(
                "#side-cards > li[data-unpack-thread='{RUNNER}']")];
                const area = document.getElementById('side-scroll').getBoundingClientRect();
                for (const row of rows) {{ const b = row.querySelector(':scope > .reply-bridge').getBoundingClientRect();
                    const y = Math.min(b.bottom - 12, area.bottom - 90);
                    if (y > Math.max(b.top + 12, area.top + 12)) return {{x: b.left + 1, y}}; }}
                return null; }}""")
            desk.check(isinstance(spot, dict), "no part of the bridge is on screen to tap")
            if touch:
                page.touchscreen.tap(float(spot["x"]), float(spot["y"]))
            else:
                page.mouse.click(float(spot["x"]), float(spot["y"]))
            desk.wait_for(f"""() => {{ const area = document.getElementById('side-scroll').getBoundingClientRect();
                const card = document.querySelector("#side-cards > li[data-card][data-id='{RUNNER}']")
                    .getBoundingClientRect();
                return card.top >= area.top - 1 && card.top <= area.top + 20; }}""",
                          "a tap on the bridge did not go back to the thread's card")
            desk.settled()
            landed = gutter(desk, RUNNER)
            card_now, scroller_now = part(landed, "card"), part(landed, "scroller")
            desk.check(card_now["bottom"] <= scroller_now["bottom"] + 1,
                       f"the card is not wholly below the heading: bottom {card_now['bottom']:.0f}px, "
                       f"the list ends at {scroller_now['bottom']:.0f}px")
            desk.check(int(str(landed["rows"])) == int(str(before["rows"])),
                       f"the bridge packed the thread: {landed['rows']} rows against {before['rows']}")
            desk.shot("3-back-at-the-card")
            desk.notes.append(f"from the thread's foot, a tap at ({float(spot['x']):.0f}px, {float(spot['y']):.0f}px) "
                              f"on the spine landed with the card at {card_now['top']:.0f}px, still unpacked")

            # And the triangle is still the triangle: the replica packs, its card unmoved.
            page.evaluate(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{REPLICA}']\")"
                          ".scrollIntoView({block: 'start'})")
            desk.settled()
            top = part(gutter(desk, REPLICA), "card")["top"]
            page.click(f"#side-cards > li[data-unpack-thread='{REPLICA}'] .unpack-pack")
            desk.wait_for(f"() => document.querySelectorAll(\"#side-cards > li[data-unpack-thread='{REPLICA}']\").length === 0",
                          "the triangle did not pack the replica")
            desk.settled()
            moved = page.evaluate(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{REPLICA}']\")"
                                  ".getBoundingClientRect().top") - top
            desk.check(abs(float(moved)) <= 2, f"packing moved the replica's card by {float(moved):.1f}px")
            desk.notes.append(f"the triangle packed the replica, its card moved {float(moved):.1f}px")
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            mode = " under reduced motion" if reduced else ""
            return f"{version} at {width}x{height}{mode}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
