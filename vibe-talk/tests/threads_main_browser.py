#!/usr/bin/env python3
"""The Threads view with the main channel's messages and box, in real Chromium at 1280x800 and 393x852.

`#419 threads-view-main-messages`. The owner, 2026-10-21: in the Threads view (Cmd+3) the main channel's
messages that are not threads were missing and nothing there posted to the main channel; its Unread only
and Unpack msgs stood below the search glass when they would fit beside it. So the heading sits on the
glass's row, and a third chip, Main channel, adds the main channel's messages among the cards and makes
the main channel's box the last card; Reply on one of those messages opens the one inline reply under it
(`#410 inline-reply-editor`), the box at the foot giving way to it. The fake-DOM suite
(tests/js/voice_page.test.mjs) pins what is drawn and posted; this measures where it is drawn.

It serves the checked-out page and the fake API of tests/two_column_browser.py (24 messages, three
threads), from one loopback origin. At each size: sign in, the thread list from the picker -> the heading
(title, Unread only, Unpack msgs, Main channel) on the glass's row: its middle level with the glass's
within a few pixels, every chip ending before the glass begins, nothing past the window's edge -> Main
messages pressed: the main channel's messages among the cards in time order, the main channel's box the
last thing in the list -> Reply on an old main-channel message mid-list: the reply's box directly under
it, the rows after it below, the box at the foot hidden, one message box on screen -> Cancel: the box at
the foot back. No page error and no sideways scroll at any step.

Pass --screenshots DIR to keep a PNG of each state; --web-root DIR to run against another copy of the
page (an older one fails it at the heading).
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
from inline_reply_browser import ReplyColumnsApi, sign_in
from offline_cache_browser import Json, check, handler_for
from two_column_browser import Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

SIZES = (("desk", 1280, 800, False), ("phone", 393, 852, True))

# The heading, its chips and the glass, measured in the page.
HEADING_JS = """() => {
    const box = (id) => { const n = document.getElementById(id);
        if (!n || n.hidden || !n.getClientRects().length) return null;
        const r = n.getBoundingClientRect(); return {left: r.left, right: r.right, top: r.top, bottom: r.bottom}; };
    return {heading: box('side-heading'), glass: box('search-toggle'),
            chips: ['side-unread-only', 'side-unpack', 'side-main'].map(box),
            title: box('side-title'), overflow: document.documentElement.scrollWidth - window.innerWidth};
}"""

# The cards' list as drawn: what each child is, and the box at its foot.
LIST_JS = """() => {
    const kind = (li) => li.hasAttribute('data-card') ? 'card' : li.hasAttribute('data-main-row') ? 'main' : 'row';
    const items = [...document.querySelectorAll('#side-cards > li')].map((li) => ({kind: kind(li), id: li.getAttribute('data-id')}));
    const composer = document.getElementById('channel-composer');
    const drawn = (id) => { const n = document.getElementById(id); return Boolean(n) && !n.hidden && n.getClientRects().length > 0; };
    return {items, boxIn: composer.parentNode.id, boxLast: document.getElementById('side-list').lastElementChild === composer,
            boxes: ['channel-compose-text', 'side-compose-text', 'reply-text'].filter(drawn)};
}"""


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per state into (default: none)")
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
            report.append(walk(playwright.chromium, args, label, width, height, touch))
    for line in report:
        print(line)
    return 0


def box(found: Json, name: str) -> dict[str, float]:
    part = found.get(name)
    check(isinstance(part, dict), f"nothing to measure for {name}: {found}")
    assert isinstance(part, dict)
    return {str(key): float(value) for key, value in part.items()}


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, touch: bool) -> str:
    api = ReplyColumnsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-threads-main-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark", has_touch=touch, is_mobile=touch)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"threads-main-{label}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            threads_walk(desk)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def threads_walk(desk: Desk) -> None:
    page = desk.page
    sign_in(desk)
    page.select_option("#thread-select", "threads")
    desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
                  " && document.querySelectorAll('#side-cards > li[data-card]').length === 3",
                  "the view picker did not show the thread list")
    desk.settled()

    # The heading on the glass's row.
    found = page.evaluate(HEADING_JS)
    assert isinstance(found, dict)
    heading, glass = box(found, "heading"), box(found, "glass")
    middle = (heading["top"] + heading["bottom"]) / 2
    desk.check(abs(middle - (glass["top"] + glass["bottom"]) / 2) <= 4,
               f"the heading is not on the glass's row: heading {heading}, glass {glass}")
    chips = found.get("chips")
    assert isinstance(chips, list)
    for chip in chips:
        desk.check(isinstance(chip, dict), f"a chip of the heading is not drawn: {chips}")
        assert isinstance(chip, dict)
        desk.check(float(chip["right"]) <= glass["left"] - 2 and float(chip["left"]) >= 0,
                   f"a chip runs into the glass or off the window: {chip} beside {glass}")
        desk.check(abs((float(chip["top"]) + float(chip["bottom"])) / 2 - middle) <= 4, f"a chip is off the heading's row: {chip}")
    desk.check(float(str(found["overflow"])) <= 0, "the page scrolls sideways")
    desk.shot("1-heading")
    desk.notes.append(f"the heading's middle {middle:.1f}px, the glass's {(glass['top'] + glass['bottom']) / 2:.1f}px, three chips before it")

    # Main channel: the main channel's messages among the cards, and its box last.
    page.click("#side-main")
    desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-main-row]').length > 0",
                  "Main channel put no main-channel message among the cards")
    desk.settled()
    listed = page.evaluate(LIST_JS)
    assert isinstance(listed, dict)
    items = listed.get("items")
    assert isinstance(items, list)
    kinds = [str(item["kind"]) for item in items if isinstance(item, dict)]
    desk.check("main" in kinds and "card" in kinds and kinds.index("main") < kinds.index("card"),
               f"the main channel's messages are not among the cards in time order: {kinds}")
    desk.check(listed["boxIn"] == "side-list" and listed["boxLast"] is True, f"the main channel's box is not the last card: {listed}")
    page.evaluate("() => { const a = document.getElementById('side-scroll'); a.scrollTop = a.scrollHeight; }")
    desk.settled()
    desk.shot("2-main-messages-foot")

    # Reply on an old main-channel message: the one box under it, the box at the foot hidden.
    page.evaluate("() => document.querySelector(\"#side-cards > li[data-main-row][data-id='203']\").scrollIntoView({block: 'center'})")
    desk.settled()
    page.evaluate("() => document.querySelector(\"#side-cards > li[data-id='203'] .reply-button\").click()")
    desk.wait_for("() => !document.getElementById('side-reply').hidden", "Reply on a main-channel message did not open the reply")
    desk.settled()
    placed = page.evaluate("""() => {
        const row = document.querySelector("#side-cards > li[data-id='203']").getBoundingClientRect();
        const mount = document.getElementById('side-reply').getBoundingClientRect();
        const rows = [...document.querySelectorAll('#side-cards > li')];
        const at = rows.findIndex((li) => li.getAttribute('data-id') === '203');
        const next = rows.slice(at + 1).find((li) => li.getClientRects().length > 0);
        return {gap: mount.top - row.bottom, below: next ? next.getBoundingClientRect().top - mount.bottom : null};
    }""")
    assert isinstance(placed, dict)
    desk.check(-1 <= float(placed["gap"]) <= 9, f"the reply's box is not directly under the message: {placed}")
    below = placed["below"]
    desk.check(below is None or float(below) >= -0.5, f"the row after the message is under the box: {placed}")
    listed = page.evaluate(LIST_JS)
    assert isinstance(listed, dict)
    desk.check(listed["boxes"] == ["reply-text"], f"more than one message box on screen: {listed['boxes']}")
    desk.shot("3-reply-under-main-message")
    page.click("#reply-cancel")
    desk.wait_for("() => document.getElementById('side-reply').hidden", "Cancel did not put the reply away")
    listed = page.evaluate(LIST_JS)
    assert isinstance(listed, dict)
    desk.check("channel-compose-text" in listed["boxes"], f"the box at the foot did not come back: {listed['boxes']}")
    desk.notes.append("Main channel among the cards and the box last; a reply under a main-channel message, the box "
                      "at the foot giving way and coming back")


if __name__ == "__main__":
    raise SystemExit(main())
