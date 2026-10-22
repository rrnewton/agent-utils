#!/usr/bin/env python3
"""Both bars away while the on-screen keyboard is up, in real Chromium at 393x852 by touch, and at 1280x800.

`#429 keyboard-hides-bar`. The owner, 2026-10-22: writing a message in the channel view on a phone, the
open keyboard plus the big blue Read and Pace tile "used too much screen real estate for me to read the
thing I'm responding to", and then: "hide the entire bar. That is both rows of it until the keyboard is
dismissed again." The fake-DOM suite (tests/js/voice_page.test.mjs) pins when the page says the keyboard
is up, from a focused field and a visual viewport a keyboard shorter; it lays nothing out, so it cannot
say that the bars are gone from the screen, that the list has their room, or that nothing moved. This
does, in the engine.

THE KEYBOARD IS SIMULATED by resizing the viewport, as Chrome on Android does with the page's
`interactive-widget=resizes-content`: the layout and the visual viewport both lose the keyboard's 336px.
Safari's way, the visual viewport alone, is the fake-DOM suite's; no real phone is driven here.

It serves the checked-out page and the fake API of tests/two_column_browser.py (24 messages, three
threads), from one loopback origin. At 393x852 by touch, in All at the foot of the list:

  the bars up -> a tap in the channel's box with no keyboard (a keyboard plugged in): both bars still
  drawn -> the keyboard up: the control bar and the dock gone, the list reaching the foot of the window,
  the box with Send and the message above it on screen, the reader still at the foot -> the keyboard
  down with the box still focused: both bars back, the reader at the foot -> up again, then the box
  losing the focus: both bars back at once, with the keyboard still up -> Reply on an old message mid-list,
  the keyboard up: the reply's box whole above the keys and the message on screen, moved up only as far as
  that takes; the keyboard down: the message not moved by a pixel -> Read on, the keyboard up: the
  dock one slim row holding Stop and nothing else, the control bar gone, the reading still on -> Stop
  tapped there: the reading off (the tap's click landing on Stop, not on what moved under it), and both
  bars back, since the tap took the focus off the box. Then: the reader scrolled up to the message being
  answered with the keyboard up, the keyboard growing 44px (an emoji strip): the reader not moved; the
  phone turned to landscape and back with the keyboard up: the bars away throughout, back once it goes;
  and the Threads view with Main messages (#419): the main channel's box among the cards, its Send above
  the keys.

At 1280x800 with a mouse, the box focused and the window 336px shorter: both bars still drawn, since the
stylesheet takes them away only where the pointer is coarse. No page error and no sideways scroll at any
step.

Pass --screenshots DIR to keep a PNG of each state; --web-root DIR to run against another copy of the
page (an older one fails it at the first keyboard).
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

PHONE = (393, 852)
DESK = (1280, 800)
# An on-screen keyboard's height on a phone this size, with its suggestion strip.
KEYBOARD_PX = 336
# How near the foot of the list a reader at its foot is kept: the page's own slack (`BOTTOM_SLACK_PX`).
FOOT_SLACK_PX = 24
# The tallest the dock may be while it holds only Stop: a 2.75rem row and its padding, and a little over.
SLIM_DOCK_PX = 72

# What is drawn, and where: the bars, the list, the box and its Send, and the reader's place in the list.
STATE_JS = """() => {
    const drawn = (id) => { const n = document.getElementById(id);
        return Boolean(n) && !n.hidden && n.getClientRects().length > 0; };
    const rect = (id) => { const n = document.getElementById(id); if (!n || !n.getClientRects().length) return null;
        const r = n.getBoundingClientRect(); return {top: r.top, bottom: r.bottom, left: r.left, right: r.right, height: r.height}; };
    const area = document.getElementById('scroll-area');
    const pane = document.getElementById('control-pane');
    const shown = pane && pane.getClientRects().length
        ? [...pane.children].filter((kid) => kid.getClientRects().length > 0).map((kid) => kid.id) : [];
    return {
        said: document.documentElement.getAttribute('data-keyboard'),
        bar: drawn('control-bar'), dock: drawn('dock'), dockBox: rect('dock'), paneShown: shown,
        list: rect('scroll-area'), box: rect('channel-compose-text'), send: rect('channel-send'),
        gap: area.scrollHeight - area.scrollTop - area.clientHeight,
        viewport: window.visualViewport ? window.visualViewport.height : window.innerHeight,
        active: document.activeElement ? document.activeElement.id : null,
        reading: document.getElementById('read-aloud').getAttribute('data-active'),
        overflow: document.documentElement.scrollWidth - window.innerWidth,
    };
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
        report.append(walk(playwright.chromium, args, "phone", *PHONE, touch=True))
        report.append(walk(playwright.chromium, args, "desk", *DESK, touch=False))
    for line in report:
        print(line)
    return 0


def number(found: Json, *path: str) -> float:
    value: object = found
    for key in path:
        check(isinstance(value, dict), f"nothing at {path} in {found}")
        assert isinstance(value, dict)
        value = value[key]
    check(isinstance(value, (int, float)), f"{path} is not a number in {found}")
    assert isinstance(value, (int, float))
    return float(value)


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, *, touch: bool) -> str:
    api = ReplyColumnsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-keyboard-bar-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark", has_touch=touch, is_mobile=touch)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"keyboard-bar-{label}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            sign_in(desk)
            if touch:
                phone_walk(desk)
            else:
                desk_walk(desk)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def state(desk: Desk) -> Json:
    found = desk.page.evaluate(STATE_JS)
    assert isinstance(found, dict)
    desk.check(number(found, "overflow") <= 0, f"the page scrolls sideways: {found}")
    return found


def keyboard(desk: Desk, up: bool) -> Json:
    """The keyboard up or down, as Chrome on Android resizes the content for it, and the page once settled."""
    desk.page.set_viewport_size({"width": desk.width, "height": desk.height - (KEYBOARD_PX if up else 0)})
    desk.wait_for(f"() => (window.visualViewport ? window.visualViewport.height : window.innerHeight) === "
                  f"{desk.height - (KEYBOARD_PX if up else 0)}", "the viewport did not take the keyboard's size")
    desk.settled()
    desk.settled()
    return state(desk)


def to_foot(desk: Desk) -> None:
    desk.page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    desk.settled()


def bars_up(desk: Desk, found: Json, when: str) -> None:
    desk.check(found["bar"] is True and found["dock"] is True, f"{when}: the bars are not both drawn: {found}")


def bars_gone(desk: Desk, found: Json, when: str) -> None:
    desk.check(found["said"] == "open", f"{when}: the page did not hear the keyboard: {found}")
    desk.check(found["bar"] is False, f"{when}: the control bar is still drawn over the keyboard: {found}")
    desk.check(found["dock"] is False, f"{when}: the dock is still drawn over the keyboard: {found}")
    desk.check(abs(number(found, "list", "bottom") - number(found, "viewport")) <= 1,
               f"{when}: the list does not reach the foot of the window: {found}")


def phone_walk(desk: Desk) -> None:
    page = desk.page
    to_foot(desk)
    found = state(desk)
    bars_up(desk, found, "at rest")
    resting_list = number(found, "list", "height")
    resting_dock = number(found, "dockBox", "height")
    desk.shot("1-bars-up")

    # A tap in the box with no keyboard coming up (one plugged in): nothing goes.
    page.tap("#channel-compose-text")
    desk.settled()
    found = state(desk)
    desk.check(found["active"] == "channel-compose-text", f"the tap did not focus the box: {found}")
    desk.check(found["said"] != "open", f"a focused box alone was taken for a keyboard: {found}")
    bars_up(desk, found, "focused, no keyboard")

    # The keyboard up: both bars gone, the list with their room, the box and Send on screen, at the foot.
    found = keyboard(desk, True)
    bars_gone(desk, found, "keyboard up")
    desk.check(number(found, "gap") <= FOOT_SLACK_PX, f"keyboard up: the reader was moved off the foot: {found}")
    for part in ("box", "send"):
        desk.check(0 <= number(found, part, "top") and number(found, part, "bottom") <= number(found, "viewport") + 0.5,
                   f"keyboard up: the {part} is not on screen: {found}")
    above = page.evaluate("""() => { const rows = [...document.querySelectorAll('#discord-log > li[data-id]')];
        const r = rows[rows.length - 1].getBoundingClientRect(); return {top: r.top, bottom: r.bottom}; }""")
    assert isinstance(above, dict)
    desk.check(number(above, "bottom") > 0, f"keyboard up: the last message, the one being answered, is off screen: {above}")
    keyboard_list = number(found, "list", "height")
    desk.shot("2-keyboard-up")
    desk.notes.append(f"the list {resting_list:.0f}px at rest with a {resting_dock:.0f}px dock, {keyboard_list:.0f}px with "
                      f"the keyboard up and both bars gone (the keyboard {KEYBOARD_PX}px)")

    # The keyboard down with the box still focused (Android's Back): both bars back, the reader at the foot.
    found = keyboard(desk, False)
    desk.check(found["said"] == "closed" and found["active"] == "channel-compose-text", f"keyboard down: {found}")
    bars_up(desk, found, "keyboard down")
    desk.check(number(found, "gap") <= FOOT_SLACK_PX, f"keyboard down: the reader was left above the foot: {found}")
    desk.shot("3-keyboard-down")

    # Up again, then the box losing the focus: the bars back at once, the keyboard not yet gone.
    keyboard(desk, True)
    page.evaluate("() => document.activeElement.blur()")
    desk.settled()
    found = state(desk)
    desk.check(found["said"] == "closed", f"blurred: the page still says the keyboard is up: {found}")
    bars_up(desk, found, "blurred with the keyboard still up")
    desk.shot("4-blurred")
    keyboard(desk, False)

    # Reply on an old message mid-list. The keyboard up: the reply's box whole on screen with the message
    # above it, which goes up only as far as that takes; the keyboard down: nothing moves at all.
    page.evaluate("() => document.querySelector(\"#discord-log > li[data-id='203']\").scrollIntoView({block: 'center'})")
    desk.settled()
    page.evaluate("() => document.querySelector(\"#discord-log > li[data-id='203'] .reply-button\").click()")
    desk.wait_for("() => document.activeElement && document.activeElement.id === 'reply-text'",
                  "Reply did not put the focus in the reply's box")
    desk.settled()
    where = """() => { const row = document.querySelector("#discord-log > li[data-id='203']").getBoundingClientRect();
        const box = document.getElementById('column-reply').getBoundingClientRect();
        return {top: row.top, bottom: row.bottom, boxTop: box.top, boxBottom: box.bottom}; }"""
    before = page.evaluate(where)
    assert isinstance(before, dict)
    found = keyboard(desk, True)
    bars_gone(desk, found, "replying, keyboard up")
    up = page.evaluate(where)
    assert isinstance(up, dict)
    viewport = number(found, "viewport")
    desk.check(number(up, "boxBottom") <= viewport + 0.5 and number(up, "boxTop") >= 0,
               f"replying, keyboard up: the reply's box is not whole on screen: {up} in {viewport}px")
    desk.check(number(up, "bottom") > 0, f"replying, keyboard up: the message being answered is off screen: {up}")
    moved = number(before, "top") - number(up, "top")
    desk.check(moved >= -1 and (moved <= 1 or abs(number(up, "boxBottom") - number(found, "list", "bottom")) <= 1),
               f"replying, keyboard up: the message moved further than showing the box takes: {before} -> {up}")
    desk.shot("5-reply-keyboard-up")
    found = keyboard(desk, False)
    bars_up(desk, found, "replying, keyboard down")
    down = page.evaluate(where)
    assert isinstance(down, dict)
    desk.check(abs(number(down, "top") - number(up, "top")) <= 1,
               f"replying, keyboard down: the message being answered moved: {up} -> {down}")
    desk.shot("6-reply-keyboard-down")
    desk.notes.append(f"the message being answered at {number(before, 'top'):.1f}px, {number(up, 'top'):.1f}px with the "
                      f"keyboard up (its reply's box whole above the keys), {number(down, 'top'):.1f}px after")
    page.click("#reply-cancel")
    desk.wait_for("() => document.getElementById('column-reply').hidden", "Cancel did not put the reply away")

    # Read on, then the keyboard: one slim row holding Stop, and nothing else of either bar.
    to_foot(desk)
    page.tap("#read-aloud")
    desk.wait_for("() => document.getElementById('read-aloud').getAttribute('data-active') === 'true'", "Read did not start reading")
    page.tap("#channel-compose-text")
    found = keyboard(desk, True)
    desk.check(found["said"] == "open" and found["reading"] == "true", f"reading, keyboard up: {found}")
    desk.check(found["bar"] is False, f"reading, keyboard up: the control bar is still drawn: {found}")
    desk.check(found["dock"] is True and found["paneShown"] == ["read-aloud"],
               f"reading, keyboard up: the dock is not Stop alone: {found}")
    slim = number(found, "dockBox", "height")
    desk.check(slim <= SLIM_DOCK_PX, f"reading, keyboard up: the dock holding Stop is {slim:.0f}px tall: {found}")
    desk.shot("7-reading-keyboard-up")
    desk.notes.append(f"reading with the keyboard up, the dock is Stop alone, {slim:.0f}px")
    page.tap("#read-aloud")
    desk.wait_for("() => document.getElementById('read-aloud').getAttribute('data-active') === 'false'",
                  "Stop did not stop the reading with the keyboard up")
    desk.settled()
    found = state(desk)
    # The tap on Stop took the focus off the box, so the bars are back, as with any tap off the box.
    desk.check(found["said"] == "closed" and found["active"] != "channel-compose-text", f"stopped: {found}")
    bars_up(desk, found, "stopped, the focus on Stop")
    desk.shot("8-stopped")
    keyboard(desk, False)
    scrolled_up_walk(desk)
    turned_walk(desk)
    threads_walk(desk)


def scrolled_up_walk(desk: Desk) -> None:
    """The keyboard up, the reader scrolled up to the message being answered, the keyboard grows (an emoji
    or clipboard strip): the reader is left where they are, not taken back to the box."""
    page = desk.page
    to_foot(desk)
    page.tap("#channel-compose-text")
    keyboard(desk, True)
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop -= 600; }")
    desk.settled()
    top = float(page.evaluate("() => document.getElementById('scroll-area').scrollTop"))
    page.set_viewport_size({"width": desk.width, "height": desk.height - KEYBOARD_PX - 44})
    desk.wait_for(f"() => window.visualViewport.height === {desk.height - KEYBOARD_PX - 44}", "the keyboard did not grow")
    desk.settled()
    desk.settled()
    after = float(page.evaluate("() => document.getElementById('scroll-area').scrollTop"))
    desk.check(abs(after - top) <= 1, f"a reader scrolled up was taken back to the box as the keyboard grew: {top} -> {after}")
    found = keyboard(desk, False)
    bars_up(desk, found, "after the scrolled-up reader's keyboard went down")
    desk.notes.append(f"a reader scrolled up stays at {top:.0f}px as the keyboard grows 44px")


def turned_walk(desk: Desk) -> None:
    """The phone turned with the keyboard up, and back: the bars stay away until the keyboard goes."""
    page = desk.page
    to_foot(desk)
    page.tap("#channel-compose-text")
    keyboard(desk, True)
    for width, height, what in ((desk.height, desk.width - 200, "turned"), (desk.width, desk.height - KEYBOARD_PX, "turned back")):
        page.set_viewport_size({"width": width, "height": height})
        desk.wait_for(f"() => window.visualViewport.height === {height}", f"{what}: the viewport did not turn")
        desk.settled()
        desk.settled()
        found = state(desk)
        bars_gone(desk, found, f"{what} with the keyboard up")
        if what == "turned":
            desk.shot("9-turned-keyboard-up")
    found = keyboard(desk, False)
    bars_up(desk, found, "turned back, the keyboard down")
    desk.notes.append("turned to landscape and back with the keyboard up, the bars stay away until it goes")


def threads_walk(desk: Desk) -> None:
    """The Threads view with Main messages (`#419`): the main channel's box among the cards, in the right
    column's list, kept above the keys with its Send as the keyboard comes up."""
    page = desk.page
    page.select_option("#thread-select", "threads")
    desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-card]').length === 3", "the Threads view did not open")
    page.click("#side-main")
    desk.wait_for("() => document.getElementById('side-list').lastElementChild === document.getElementById('channel-composer')",
                  "Main messages did not put the main channel's box last among the cards")
    page.evaluate("() => { const a = document.getElementById('side-scroll'); a.scrollTop = a.scrollHeight; }")
    desk.settled()
    page.tap("#channel-compose-text")
    found = keyboard(desk, True)
    bars_gone(desk, found, "Threads view, keyboard up")
    viewport = number(found, "viewport")
    desk.check(0 <= number(found, "send", "top") and number(found, "send", "bottom") <= viewport + 0.5,
               f"Threads view, keyboard up: Send is not on screen: {found}")
    desk.shot("10-threads-keyboard-up")
    keyboard(desk, False)
    desk.notes.append("in the Threads view the main channel's box and Send stay above the keys")


def desk_walk(desk: Desk) -> None:
    page = desk.page
    to_foot(desk)
    page.click("#channel-compose-text")
    found = keyboard(desk, True)
    bars_up(desk, found, "a desk's window made shorter with the box focused")
    desk.shot("1-shorter-window")
    desk.notes.append("a mouse-driven window 336px shorter with the box focused keeps both bars")
    keyboard(desk, False)


if __name__ == "__main__":
    raise SystemExit(main())
