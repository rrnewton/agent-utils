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
  the box flush on the keys with the message above it, the reader still at the foot -> the keyboard
  down with the box still focused: both bars back, the reader at the foot -> up again, then the box
  losing the focus: both bars back at once, with the keyboard still up -> Reply on an old message mid-list,
  the keyboard up: the reply's box whole above the keys and the message on screen, moved up only as far as
  that takes; the keyboard down: the message not moved by a pixel -> Read on, the keyboard up: the
  dock one slim row holding Stop and nothing else, the control bar gone, the reading still on -> Stop
  tapped there: the reading off (the tap's click landing on Stop, not on what moved under it), and both
  bars back, since the tap took the focus off the box. Then: the reader scrolled up to the message being
  answered with the keyboard up, the keyboard growing 44px (an emoji strip): the reader not moved; the
  phone turned to landscape and back with the keyboard up: the bars away throughout, back once it goes;
  and the Threads view with Main messages (#419): the main channel's box among the cards, its foot on
  the keys.

`#575 mobile-keyboard-compose`, at 393x852 after all that and again at 412x915 (the owner's Android phone),
in a thread and in Main (`compose_walk`): the keyboard up leaves the box's foot on the keyboard's top, Send
gone (the keyboard's own key sends) and a new-line key in the box's corner, whose tap writes a new line and
keeps the focus, after which the keyboard's Enter sends both lines once; with Enter to send off, Send stays
as the last thing on the keys' top and Enter is a new line.

`#610 mobile-keyboard-compose`, Safari's way at 390x664 by touch (`ios_walk`): the layout keeps its full height
and only the visual viewport shrinks and pans, as WebKit does, played to the page by a stand-in
`window.visualViewport` (and `innerHeight`) installed before the page loads. Chromium has no iOS keyboard,
so the stand-in is the keyboard: its 330px visual viewport is what iOS leaves over the keys, the
suggestion strip and the floating Done control. The keyboard up: the box's foot on the visual viewport's
foot, and on it again after Safari's pan, as the list's end; the reader's wheel then scrolls the list up and
back down to it, and a later pan moves nothing; with `innerHeight` already leaving out the keyboard (WebKit,
for this page's `interactive-widget=resizes-content`) the box still lands on the keys, where #575 left it
under them at the end of a list that would not scroll. Playwright's WebKit is no better a stand-in here:
it draws no keyboard and pans no visual viewport either.

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
from typing import TYPE_CHECKING, Callable

import offline_cache_browser as fixture
from inline_reply_browser import ReplyColumnsApi, sign_in
from offline_cache_browser import Json, check, handler_for
from two_column_browser import Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

PHONE = (393, 852)
# `#575 mobile-keyboard-compose`: the owner's Android phone, whose screenshot this walk was written from.
ANDROID = (412, 915)
DESK = (1280, 800)
# `#610 mobile-keyboard-compose`: an iPhone's Safari with its toolbars, and the visual viewport its keyboard leaves
# (the keys, the suggestion strip and the floating Done control taken off the 664px).
IPHONE = (390, 664)
IOS_VISUAL_PX = 330
# The most the list keeps under the composer at its very end (its own padding), as on Android: a reader sent
# to the foot has the box that far over the keys.
END_PADDING_PX = 12
# How far Safari pans the visual viewport down over the focused field after its keyboard comes up.
IOS_PAN_PX = 200
# How long the page goes on making its jump as the viewport moves (`KEYBOARD_SETTLE_MS` in web/voice.js),
# and a little over.
SETTLE_WAIT_MS = 1500

# Safari's keyboard, played to the page: a visual viewport that shrinks and pans over a layout that keeps
# its height. `window.__iosKeyboard(height, offsetTop, innerHeight)` brings it up (innerHeight left as
# the browser has it unless given), `window.__iosPan(offsetTop)` pans, `window.__iosClose()` takes it away;
# each tells the page as Safari does, by `resize` or `scroll` on the visual viewport.
IOS_VIEWPORT_JS = """(() => {
    const real = window.visualViewport;
    const own = Object.getOwnPropertyDescriptor(window, 'innerHeight')
        || Object.getOwnPropertyDescriptor(Window.prototype, 'innerHeight');
    const events = new EventTarget();
    const sim = {height: null, offsetTop: 0, inner: null};
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
    Object.defineProperty(window, 'innerHeight', {configurable: true,
        get: () => (sim.inner === null ? own.get.call(window) : sim.inner)});
    window.__iosKeyboard = (height, offsetTop = 0, inner = null) => {
        Object.assign(sim, {height, offsetTop, inner});
        events.dispatchEvent(new Event('resize'));
    };
    window.__iosPan = (offsetTop) => { sim.offsetTop = offsetTop; events.dispatchEvent(new Event('scroll')); };
    window.__iosClose = () => {
        Object.assign(sim, {height: null, offsetTop: 0, inner: null});
        events.dispatchEvent(new Event('resize'));
    };
})();"""
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
        newline: rect('channel-newline'), state: rect('channel-compose-state'),
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
        report.append(walk(playwright.chromium, args, "android", *ANDROID, touch=True, walker=compose_walk))
        report.append(walk(playwright.chromium, args, "ios", *IPHONE, touch=True, walker=ios_walk,
                           init_script=IOS_VIEWPORT_JS))
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


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, *, touch: bool,
         walker: Callable[[Desk, ReplyColumnsApi], None] | None = None, init_script: str | None = None) -> str:
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
            if init_script is not None:
                page.add_init_script(init_script)
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"keyboard-bar-{label}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            sign_in(desk)
            if walker is not None:
                walker(desk, api)
            elif touch:
                phone_walk(desk)
                compose_walk(desk, api)
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


def flush(desk: Desk, found: Json, when: str) -> None:
    """`#575 mobile-keyboard-compose`: the box whole on screen with its foot on the keyboard's top, Send gone (the
    keyboard's own key sends) and the new-line key drawn in the box's corner."""
    viewport = number(found, "viewport")
    desk.check(number(found, "box", "top") >= 0, f"{when}: the box's top is off screen: {found}")
    desk.check(abs(number(found, "box", "bottom") - viewport) <= 1,
               f"{when}: the box's foot is not on the keyboard's top, {viewport - number(found, 'box', 'bottom'):.0f}px "
               f"wasted between them: {found}")
    desk.check(found["send"] is None, f"{when}: Send is still drawn, though the keyboard's key sends: {found}")
    desk.check(found["newline"] is not None, f"{when}: no new-line key: {found}")
    key, box = found["newline"], found["box"]
    assert isinstance(key, dict) and isinstance(box, dict)
    desk.check(box["top"] <= key["top"] and key["bottom"] <= box["bottom"] and box["left"] <= key["left"]
               and key["right"] <= box["right"], f"{when}: the new-line key is not in the box's corner: {found}")


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
    flush(desk, found, "keyboard up")
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
    # By the reader's own hand (`#610`: a scroll no touch, press or wheel began is no reader's, and in the
    # keyboard's first moment the page takes it for the browser's own and makes its jump again).
    before = float(page.evaluate("() => document.getElementById('scroll-area').scrollTop"))
    wheel(desk, -600)
    top = float(page.evaluate("() => document.getElementById('scroll-area').scrollTop"))
    desk.check(top < before - 300, f"the reader's wheel did not scroll the list up: {before} -> {top}")
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
    column's list, its foot brought onto the keys as the keyboard comes up."""
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
    flush(desk, found, "Threads view, keyboard up")
    desk.shot("10-threads-keyboard-up")
    keyboard(desk, False)
    desk.notes.append("in the Threads view the main channel's box stays flush on the keys")


def posted(api: ReplyColumnsApi) -> list[str]:
    with api.lock:
        return [str(reply.get("text", "")) for reply in api.replies]


def compose_walk(desk: Desk, api: ReplyColumnsApi) -> None:
    """`#575 mobile-keyboard-compose`. The owner, 2026-10-27, on Android and iOS: a tap in the box left Send and
    "some useless white space" between it and the keyboard, whose own send key makes Send needless; and
    no way to write a new line, since the keyboard's Shift does not turn its send key into a return.

    In a thread (the box saying "Reply in this thread…", as in the owner's screenshot) and in Main: the
    keyboard up leaves the box's foot on the keyboard's top, Send gone, and a new-line key in the box's
    corner. A tap on that key writes a new line where the caret is, keeps the focus and the keyboard,
    sends nothing; the keyboard's Enter then sends both lines once. The keyboard down: Send back, the key
    gone. With Enter to send off: the keyboard's key a return (`enterkeyhint`), Send kept as the last
    thing above the keys, its foot on their top, no new-line key, and Enter a new line that sends nothing."""
    page = desk.page
    before = len(posted(api))
    thread = page.evaluate("""() => [...document.getElementById('thread-select').options]
        .map((o) => o.value).find((v) => !['main', 'flat', 'threads', 'columns', 'all'].includes(v) && v)""")
    desk.check(isinstance(thread, str), "the picker offers no thread")
    assert isinstance(thread, str)
    page.select_option("#thread-select", thread)
    desk.wait_for("() => document.getElementById('channel-compose-text').placeholder.startsWith('Reply in this thread')",
                  "choosing a thread did not make the box a reply in it")
    to_foot(desk)
    page.tap("#channel-compose-text")
    found = keyboard(desk, True)
    bars_gone(desk, found, "in a thread, keyboard up")
    flush(desk, found, "in a thread, keyboard up")
    desk.shot("11-thread-keyboard-up")
    wasted = number(found, "viewport") - number(found, "box", "bottom")

    page.keyboard.type("first line")
    page.tap("#channel-newline")
    desk.settled()
    found = state(desk)
    value = page.evaluate("() => document.getElementById('channel-compose-text').value")
    desk.check(value == "first line\n", f"the new-line key did not write a new line at the caret: {value!r}")
    desk.check(found["active"] == "channel-compose-text" and found["said"] == "open",
               f"the new-line key took the focus, and the keyboard with it: {found}")
    desk.check(len(posted(api)) == before, f"the new-line key sent: {posted(api)}")
    page.keyboard.type("second line")
    desk.shot("12-thread-two-lines")
    page.keyboard.press("Enter")
    for _ in range(50):
        if len(posted(api)) > before:
            break
        page.wait_for_timeout(100)
    desk.check(posted(api)[before:] == ["first line\nsecond line"],
               f"the keyboard's Enter did not send both lines once: {posted(api)[before:]}")
    found = keyboard(desk, False)
    desk.check(found["send"] is not None and found["newline"] is None,
               f"keyboard down: Send not back, or the new-line key still drawn: {found}")

    # Main, with Enter to send off: the keyboard's key is a return, so Send stays, at the keys' top.
    page.select_option("#thread-select", "main")
    desk.wait_for("() => document.getElementById('channel-compose-text').placeholder.startsWith('Message the main')",
                  "choosing Main did not make the box Main's")
    page.evaluate("() => document.getElementById('enter-sends').click()")
    desk.check(page.evaluate("() => document.getElementById('channel-compose-text').getAttribute('enterkeyhint')") == "enter",
               "with Enter to send off the keyboard is still told to draw a send key")
    page.evaluate("() => document.activeElement && document.activeElement.blur()")
    to_foot(desk)
    page.tap("#channel-compose-text")
    found = keyboard(desk, True)
    bars_gone(desk, found, "Enter to send off, keyboard up")
    viewport = number(found, "viewport")
    desk.check(found["send"] is not None and abs(number(found, "send", "bottom") - viewport) <= 1,
               f"Enter to send off, keyboard up: Send is not the last thing on the keys' top: {found}")
    desk.check(number(found, "box", "top") >= 0, f"Enter to send off, keyboard up: the box's top is off screen: {found}")
    desk.check(found["newline"] is None, f"Enter to send off, keyboard up: a new-line key beside a return key: {found}")
    desk.shot("13-enter-off-keyboard-up")
    sent = len(posted(api))
    page.keyboard.type("one")
    page.keyboard.press("Enter")
    page.keyboard.type("two")
    desk.settled()
    value = page.evaluate("() => document.getElementById('channel-compose-text').value")
    desk.check(value == "one\ntwo" and len(posted(api)) == sent,
               f"Enter to send off: Enter did not write a new line, or sent: {value!r} {posted(api)[sent:]}")
    page.evaluate("() => { const box = document.getElementById('channel-compose-text'); box.value = ''; "
                  "box.dispatchEvent(new InputEvent('input', {bubbles: true})); }")
    page.evaluate("() => document.getElementById('enter-sends').click()")
    keyboard(desk, False)
    desk.notes.append(f"in a thread with the keyboard up the box's foot is {abs(wasted):.0f}px from the keys, Send gone, "
                      "the corner key's new line and Enter sending both lines once; with Enter to send off Send stays "
                      "on the keys' top")


def ios_place(desk: Desk) -> Json:
    """Where the box, its list and the visual viewport are, and how far the list is scrolled."""
    found = desk.page.evaluate("""() => {
        const v = window.visualViewport, area = document.getElementById('scroll-area');
        const box = document.getElementById('channel-compose-text').getBoundingClientRect();
        const list = area.getBoundingClientRect();
        return {said: document.documentElement.getAttribute('data-keyboard'), top: v.offsetTop,
            keys: v.offsetTop + v.height, boxTop: box.top, boxBottom: box.bottom, listBottom: list.bottom,
            scroll: area.scrollTop, furthest: area.scrollHeight - area.clientHeight,
            room: getComputedStyle(document.documentElement).getPropertyValue('--keyboard-inset').trim(),
            bar: document.getElementById('control-bar').getClientRects().length > 0,
            active: document.activeElement ? document.activeElement.id : null};
    }""")
    assert isinstance(found, dict)
    return found


def ios_flush(desk: Desk, found: Json, when: str, *, above: float = 0) -> None:
    """The box whole in the visual viewport, its foot on the viewport's foot: the keyboard's top, with the
    suggestion strip and the floating control above the keys. `above`: how far over the keys it may stop,
    for the list's end, which keeps the list's own padding under the composer (as on Android)."""
    desk.check(found["said"] == "open", f"{when}: the page did not hear the keyboard: {found}")
    gap = number(found, "keys") - number(found, "boxBottom")
    desk.check(-1 <= gap <= above + 1,
               f"{when}: the box's foot is {number(found, 'boxBottom') - number(found, 'keys'):.0f}px from the "
               f"keyboard's top: {found}")
    desk.check(number(found, "boxTop") >= number(found, "top"), f"{when}: the box's top is above the screen: {found}")


def wheel(desk: Desk, dy: float) -> None:
    """The reader's own scroll of the list, by a wheel over it (Chromium has no touch scroll to drive)."""
    desk.page.mouse.move(desk.width / 2, 120)
    desk.page.mouse.wheel(0, dy)
    desk.page.wait_for_timeout(300)
    desk.settled()


def ios_walk(desk: Desk, api: ReplyColumnsApi) -> None:
    """`#610 mobile-keyboard-compose`. The owner, 2026-10-27, on an iPhone: the jump to the keyboard's top "scrolls
    so that the ENTIRE message box is hidden", and "if I try to scroll down to correct the bug or expose
    the send button it won't scroll further at all. We made it a hard maximum." Safari's keyboard as
    WebKit has it: the layout keeps its height, the visual viewport shrinks, then pans over the field."""
    page = desk.page
    to_foot(desk)
    page.tap("#channel-compose-text")
    desk.settled()
    found = ios_place(desk)
    desk.check(found["said"] != "open" and found["bar"] is True, f"focused, no keyboard yet: {found}")

    # The keyboard up, `innerHeight` left at the layout's height: the box on the keys.
    page.evaluate(f"() => window.__iosKeyboard({IOS_VISUAL_PX})")
    desk.settled()
    found = ios_place(desk)
    ios_flush(desk, found, "keyboard up")
    desk.check(found["bar"] is False, f"keyboard up: the control bar is still drawn: {found}")
    desk.check(number(found, "listBottom") > number(found, "keys") + 100,
               f"keyboard up: the layout shrank, so this is not Safari's way: {found}")
    desk.shot("1-ios-keyboard-up")
    # Safari's pan over the field, straight after: the box follows to the new top.
    page.evaluate(f"() => window.__iosPan({IOS_PAN_PX})")
    desk.settled()
    found = ios_place(desk)
    ios_flush(desk, found, "panned")
    flush_scroll = number(found, "scroll")
    room = found["room"]
    desk.shot("2-ios-panned")

    # The reader scrolls up to read, and a pan of theirs after it moves nothing.
    wheel(desk, -300)
    up = ios_place(desk)
    desk.check(number(up, "scroll") < flush_scroll - 100, f"the reader could not scroll up from the box: {up}")
    page.evaluate("() => window.__iosPan(80)")
    desk.settled()
    page.wait_for_timeout(SETTLE_WAIT_MS)
    after = ios_place(desk)
    desk.check(abs(number(after, "scroll") - number(up, "scroll")) <= 1,
               f"a pan after the reader scrolled took them back to the box: {up} -> {after}")
    desk.check(after["room"] == room, f"the room under the box changed as the viewport panned: {room} -> {after}")
    # Back down: the list's end is the box on the keys, as wherever the page goes to the foot (a send).
    page.evaluate(f"() => window.__iosPan({IOS_PAN_PX})")
    wheel(desk, 2000)
    down = ios_place(desk)
    desk.check(number(down, "scroll") >= number(down, "furthest") - 1, f"the reader's wheel did not reach the end: {down}")
    ios_flush(desk, down, "scrolled back to the end", above=END_PADDING_PX)
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop -= 400; a.scrollTop = a.scrollHeight; }")
    desk.settled()
    ios_flush(desk, ios_place(desk), "sent to the foot by the page", above=END_PADDING_PX)
    desk.shot("3-ios-back-at-the-end")
    # A message sent with the keyboard's own key: the page goes to the newest, and the box stays on the keys.
    before = len(posted(api))
    page.keyboard.type("from the phone")
    page.keyboard.press("Enter")
    for _ in range(50):
        if len(posted(api)) > before:
            break
        page.wait_for_timeout(100)
    desk.check(posted(api)[before:] == ["from the phone"], f"the keyboard's Enter did not send: {posted(api)[before:]}")
    page.wait_for_timeout(500)
    desk.settled()
    ios_flush(desk, ios_place(desk), "after a send", above=END_PADDING_PX)
    desk.shot("4-ios-after-send")
    page.evaluate("() => window.__iosClose()")
    page.evaluate("() => document.activeElement && document.activeElement.blur()")
    desk.settled()
    desk.check(ios_place(desk)["said"] == "closed", "the keyboard closed and the page still lays itself out for it")

    # `innerHeight` already leaving out the keyboard, as WebKit reports it for this page's
    # `interactive-widget=resizes-content`, with the page still laid out at full height.
    to_foot(desk)
    page.tap("#channel-compose-text")
    desk.settled()
    page.evaluate(f"() => window.__iosKeyboard({IOS_VISUAL_PX}, 0, {IOS_VISUAL_PX})")
    desk.settled()
    found = ios_place(desk)
    ios_flush(desk, found, "innerHeight without the keyboard")
    desk.shot("5-ios-short-innerheight")
    # A pan once the keyboard has settled is the reader's: the box is not moved.
    page.wait_for_timeout(SETTLE_WAIT_MS)
    before = number(ios_place(desk), "scroll")
    page.evaluate("() => window.__iosPan(150)")
    desk.settled()
    desk.check(abs(number(ios_place(desk), "scroll") - before) <= 1, "a pan after the keyboard settled moved the list")
    page.evaluate("() => window.__iosClose()")
    page.evaluate("() => document.activeElement && document.activeElement.blur()")
    desk.settled()
    desk.notes.append(f"Safari's way: the box's foot on the visual viewport's foot as the keyboard comes up and after a "
                      f"{IOS_PAN_PX}px pan, with {room} of room, as the list's end; the reader's wheel goes up and back down "
                      "to it, and a pan after that moves nothing; with innerHeight leaving out the keyboard it lands on the keys too")


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
