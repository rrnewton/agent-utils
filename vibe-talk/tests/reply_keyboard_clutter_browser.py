#!/usr/bin/env python3
"""One Send, no floating chips, one compact row, while the keyboard is up (`#1680`).

The owner, 2026-11-15, on an Android phone with Gboard, of the inline reply form: "Do you see
how the send button is overlapping the underneath send button? That's pretty bad... It seems
when we pop the keyboard for this screen we need to simplify. We've got these additional chips
including the undo chip getting in the way."

Three things stand here, beside `tests/keyboard_hides_bar_browser.py`'s bars and fit:

  * ONE SEND ONLY. The Send at the keyboard's top (`#keyboard-send`, `ios-keyboard-send`,
    #1504) stands only while no part of the focused box's own Send is in the visual viewport.
    The reply in the stream is never fitted, so its row straddles the keyboard's top while it
    is written in; a fully-inside test stood the second Send over the visible half. At
    393x852 by touch, Reply mid-list with the keyboard up: exactly one Send stands in the
    viewport, the reply's own, and the two never overlap. In the main composer, which rests
    whole on the keys with its row in sight (`#2277`): exactly one Send stands, the row's own.
  * NO FLOATING CHIPS WHILE THE KEYBOARD IS UP. The jump chips stand down and the chips row
    moves to the top of the list, where Undo stays; the keyboard going down brings the whole
    row back at the list's foot as it was. The chips' drawn state here is the real one: the
    list scrolled up brings the jump to the newest up by itself.
  * THE REPLY'S ROW COMPACT WHILE TYPING. Helper text away, Cancel and Send reply on one
    line, Post to main a small toggle at the line's start where it is offered.

THE KEYBOARD IS SIMULATED by resizing the viewport, as Chrome on Android does with the page's
`interactive-widget=resizes-content` (the same stand-in as `keyboard_hides_bar_browser.py`).
At 1280x800 with a mouse nothing of this applies: the corner Send never stands, the chips
keep the list's foot, and the reply's row keeps its rest layout.

Pass --screenshots DIR to keep a PNG of each state.
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
import page_sources
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

PHONE = (393, 852)
DESK = (1280, 800)
KEYBOARD_PX = 336

STATE_JS = """() => {
    const drawn = (id) => { const n = document.getElementById(id);
        return Boolean(n) && !n.hidden && n.getClientRects().length > 0; };
    const rect = (id) => { const n = document.getElementById(id); if (!n || !n.getClientRects().length) return null;
        const r = n.getBoundingClientRect(); return {top: r.top, bottom: r.bottom, left: r.left, right: r.right, height: r.height}; };
    const shown = (id) => { const n = document.getElementById(id); if (!n) return null;
        const style = getComputedStyle(n); return {display: style.display, hidden: n.hidden}; };
    const tools = document.getElementById('scroll-tools');
    const toolsRect = tools && tools.getClientRects().length ? tools.getBoundingClientRect() : null;
    return {
        said: document.documentElement.getAttribute('data-keyboard'),
        viewport: window.visualViewport ? window.visualViewport.height : window.innerHeight,
        active: document.activeElement ? document.activeElement.id : null,
        corner: rect('keyboard-send'), cornerDrawn: drawn('keyboard-send'),
        replySend: rect('reply-send'), replyCancel: rect('reply-cancel'),
        mainSend: rect('channel-send'), box: rect('channel-compose-text'),
        tools: toolsRect ? {top: toolsRect.top, bottom: toolsRect.bottom} : null,
        jumpNewest: shown('jump-newest'), jumpBookmark: shown('jump-bookmark'),
        undo: shown('undo-dismiss'),
        dest: shown('reply-destination'), meta: shown('reply-target-meta'),
        toMain: rect('reply-to-main-row'), replyBox: rect('column-reply'),
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


def number(found: Json, *path: str) -> float:
    value: object = found
    for key in path:
        check(isinstance(value, dict), f"nothing at {path} in {found}")
        assert isinstance(value, dict)
        value = value[key]
    check(isinstance(value, (int, float)), f"{path} is not a number in {found}")
    assert isinstance(value, (int, float))
    return float(value)


def state(desk: Desk) -> Json:
    found = desk.page.evaluate(STATE_JS)
    assert isinstance(found, dict)
    desk.check(number(found, "overflow") <= 0, f"the page scrolls sideways: {found}")
    return found


def keyboard(desk: Desk, up: bool) -> Json:
    """The keyboard up or down, as Chrome on Android resizes the content for it."""
    desk.page.set_viewport_size({"width": desk.width, "height": desk.height - (KEYBOARD_PX if up else 0)})
    desk.wait_for(f"() => (window.visualViewport ? window.visualViewport.height : window.innerHeight) === "
                  f"{desk.height - (KEYBOARD_PX if up else 0)}", "the viewport did not take the keyboard's size")
    desk.settled()
    desk.settled()
    return state(desk)


def coord(rect: Json, key: str) -> float:
    assert isinstance(rect, dict)
    value = rect[key]
    assert isinstance(value, (int, float))
    return float(value)


def in_viewport(found: Json, part: str) -> bool:
    """Any part of `part`'s rect stands in the visible viewport (its top edge at 0 here)."""
    rect = found[part]
    if not isinstance(rect, dict):
        return False
    return number(found, "viewport") > 0 and coord(rect, "bottom") > 0 and coord(rect, "top") < number(found, "viewport")


def overlap(a: Json, b: Json) -> bool:
    assert isinstance(a, dict) and isinstance(b, dict)
    return (coord(a, "left") < coord(b, "right") and coord(b, "left") < coord(a, "right")
            and coord(a, "top") < coord(b, "bottom") and coord(b, "top") < coord(a, "bottom"))


def phone_walk(desk: Desk) -> None:
    page = desk.page
    # The list scrolled up: the way back to the newest stands by itself.
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = 0; }")
    desk.settled()
    desk.wait_for("() => { const n = document.getElementById('jump-newest'); "
                  "return Boolean(n) && !n.hidden && n.getClientRects().length > 0; }",
                  "scrolling up did not bring the jump to the newest up")
    found = state(desk)
    desk.check(isinstance(found["jumpNewest"], dict) and found["jumpNewest"]["display"] != "none",
               f"at rest: the jump chip is not drawn: {found}")

    # Reply on an old message mid-list, then the keyboard up over it.
    page.evaluate("() => document.querySelector(\"#discord-log > li[data-id='203']\").scrollIntoView({block: 'center'})")
    desk.settled()
    page.evaluate("() => document.querySelector(\"#discord-log > li[data-id='203'] .reply-button\").click()")
    desk.wait_for("() => document.activeElement && document.activeElement.id === 'reply-text'",
                  "Reply did not put the focus in the reply's box")
    desk.settled()
    found = keyboard(desk, True)
    desk.check(found["said"] == "open" and found["active"] == "reply-text", f"replying, keyboard up: {found}")
    # One Send only: the reply's own stands in the viewport; the corner Send does not stand at all.
    desk.check(in_viewport(found, "replySend"), f"replying, keyboard up: the reply's Send is not on screen: {found}")
    desk.check(found["cornerDrawn"] is False, f"replying, keyboard up: a second Send stands: {found}")
    if isinstance(found["corner"], dict) and isinstance(found["replySend"], dict):
        desk.check(not overlap(found["corner"], found["replySend"]),
                   f"replying, keyboard up: the two Sends overlap: {found}")
    # No floating chips over the form; the row compact on one line.
    desk.check(isinstance(found["jumpNewest"], dict) and found["jumpNewest"]["display"] == "none",
               f"replying, keyboard up: the jump chip still floats: {found}")
    desk.check(isinstance(found["dest"], dict) and found["dest"]["display"] == "none",
               f"replying, keyboard up: the helper text still stands: {found}")
    desk.check(isinstance(found["replyCancel"], dict) and isinstance(found["replySend"], dict)
               and abs(float(found["replyCancel"]["top"]) - float(found["replySend"]["top"])) <= 2,
               f"replying, keyboard up: Cancel and Send reply are not on one line: {found}")
    desk.shot("1-reply-keyboard-up")
    desk.notes.append("replying with the keyboard up: one Send, no jump chips, one compact row")

    # The keyboard down: the chips row and the helper text are back as they were.
    found = keyboard(desk, False)
    desk.check(found["said"] == "closed", f"replying, keyboard down: {found}")
    desk.check(isinstance(found["jumpNewest"], dict) and found["jumpNewest"]["display"] != "none",
               f"replying, keyboard down: the jump chip did not come back: {found}")
    desk.check(found["cornerDrawn"] is False, f"replying, keyboard down: the corner Send outlived the keyboard: {found}")
    desk.shot("2-reply-keyboard-down")
    page.click("#reply-cancel")
    if page.is_visible("#reply-discard"):
        page.keyboard.press("Escape")
    desk.wait_for("() => document.getElementById('column-reply').hidden", "Cancel did not put the reply away")

    # The main composer: the whole composer rests on the keys, its row in sight (`#2277`; #1135 had
    # parked the row under the keys), so the one Send standing is the row's own, and the corner Send
    # stands down: nothing floats over the composer's corner (`#2274`).
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    desk.settled()
    page.tap("#channel-compose-text")
    found = keyboard(desk, True)
    desk.check(found["said"] == "open" and found["active"] == "channel-compose-text", f"composing, keyboard up: {found}")
    desk.check(in_viewport(found, "mainSend"), f"composing, keyboard up: no Send stands above the keyboard: {found}")
    desk.check(found["cornerDrawn"] is False,
               f"composing, keyboard up: the corner Send also stands beside the row's own, two Sends: {found}")
    desk.shot("3-composer-keyboard-up")
    found = keyboard(desk, False)
    desk.check(found["cornerDrawn"] is False, f"composing, keyboard down: the corner Send outlived the keyboard: {found}")
    desk.notes.append("in the main composer with the keyboard up: the row's own Send alone stands")


def desk_walk(desk: Desk) -> None:
    page = desk.page
    page.click("#channel-compose-text")
    found = keyboard(desk, True)
    # The page may say the keyboard is up on a desk too; the stylesheet's coarse-pointer
    # regime is what must not reach it: no corner Send, its own Send standing, and the chips
    # row still anchored to the list's foot (its computed `bottom`, not `top`).
    desk.check(found["cornerDrawn"] is False, f"on a desk: the corner Send stands: {found}")
    desk.check(in_viewport(found, "mainSend"), f"on a desk: the composer's own Send does not stand: {found}")
    anchored = page.evaluate("() => { const style = getComputedStyle(document.getElementById('scroll-tools')); "
                             "return {bottom: style.bottom, top: style.top, coarse: matchMedia('(pointer: coarse)').matches}; }")
    assert isinstance(anchored, dict)
    desk.check(anchored["coarse"] is False and anchored["bottom"] != "auto",
               f"on a desk: the chips row left the list's foot: {anchored} in {found}")
    desk.shot("1-desk-shorter-window")
    desk.notes.append("a desk's shorter window: no corner Send, the chips at the list's foot")
    keyboard(desk, False)


def walk(chromium: "BrowserType", args: argparse.Namespace, label: str, width: int, height: int,
         *, touch: bool) -> str:
    api = ReplyColumnsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-reply-clutter-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark", has_touch=touch, is_mobile=touch)
            browser_waits.hot_host_timeouts(context)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"reply-clutter-{label}", width, height)
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
    report: list[str] = []
    with sync_playwright() as playwright:
        report.append(walk(playwright.chromium, args, "phone", *PHONE, touch=True))
        report.append(walk(playwright.chromium, args, "desk", *DESK, touch=False))
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
