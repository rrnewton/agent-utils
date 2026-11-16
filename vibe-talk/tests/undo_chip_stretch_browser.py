#!/usr/bin/env python3
"""The Undo chip keeps its size with the keyboard up and Discuss docked (`#1725`).

The owner, 2026-11-16, on an Android phone in the light theme, replying in a thread with
the on-screen keyboard up and the Discuss half-pane docked open: the Undo chip stood as
a tall pill down the right edge of the pane, over the message text, the right of the
reply box and the Send reply button — "It looks like we tried to move it out of the way
and stretched it instead". The keyboard-open relocation of the chips row (`#1680`) said
`top` and `bottom: auto` in one rule; the docked half-pane's own `bottom` (22-discuss.css)
outranked that `auto`, so both offsets stood and the strip — and the lone Undo chip in
it — stretched between them.

So, at a 412x915 phone (device scale 2.6, touch) in the light scheme and the dark, with
the Discuss half-pane docked open and a dismissal's Undo up: reply with the keyboard up
(simulated as Chrome on Android lays it out, the viewport shorter) -> the Undo chip
keeps a chip's height, the chips strip is one line, and neither covers the reply box or
Send reply; the keyboard down -> the chip is still a chip, at the list's foot above the
pane, clear of the reply's row. At a 1280x800 desk with Discuss open the same chip
never moves or stretches when the window shortens.

Every wait is on a page condition; no wall-clock wait decides a result.
Pass --screenshots DIR to keep a PNG of each state.
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING, Literal

import offline_cache_browser as fixture
from inline_reply_browser import ReplyColumnsApi, sign_in
from offline_cache_browser import Json, check, handler_for
from two_column_browser import Desk
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

PHONE = (412, 915)
DESK = (1280, 800)
KEYBOARD_PX = 360
# A chip is one line of text with its padding: a few dozen px at any scheme. The stretched
# chip stood most of the pane tall, so the bound is set far under that and over any chip.
CHIP_MAX = 56.0
STRIP_MAX = 80.0

STATE_JS = """() => {
    const rect = (id) => { const n = document.getElementById(id);
        if (!n || !n.getClientRects().length) return null;
        const r = n.getBoundingClientRect();
        return {top: r.top, bottom: r.bottom, left: r.left, right: r.right,
                width: r.width, height: r.height}; };
    const undo = document.getElementById('undo-dismiss');
    const hit = undo && undo.getClientRects().length ? (() => {
        const r = undo.getBoundingClientRect();
        const found = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        return found === undo || undo.contains(found); })() : false;
    const panel = document.getElementById('discuss-panel');
    return {
        said: document.documentElement.getAttribute('data-keyboard'),
        sheet: panel ? panel.getAttribute('data-sheet') : null,
        panel: rect('discuss-panel'),
        tools: rect('scroll-tools'),
        undo: rect('undo-dismiss'), undoHit: hit,
        replyText: rect('reply-text'), replySend: rect('reply-send'),
    };
}"""


class UndoDiscussApi(ReplyColumnsApi):
    """The reply channel with Discuss available, so the half-pane can dock open."""

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope), "discuss_available": True}


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per state into")
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


def box(found: Json, name: str) -> dict:
    value = found[name]
    assert isinstance(value, dict), f"{name} is {value!r}, not a box"
    return value


def meets(a: dict, b: dict) -> bool:
    return (float(a["left"]) < float(b["right"]) - 0.5 and float(b["left"]) < float(a["right"]) - 0.5
            and float(a["top"]) < float(b["bottom"]) - 0.5 and float(b["top"]) < float(a["bottom"]) - 0.5)


def state(desk: Desk) -> Json:
    found = desk.page.evaluate(STATE_JS)
    assert isinstance(found, dict)
    return found


def keyboard(desk: Desk, up: bool) -> Json:
    """The keyboard up or down, as Chrome on Android resizes the content for it."""
    desk.page.set_viewport_size({"width": desk.width, "height": desk.height - (KEYBOARD_PX if up else 0)})
    desk.wait_for(f"() => (window.visualViewport ? window.visualViewport.height : window.innerHeight) === "
                  f"{desk.height - (KEYBOARD_PX if up else 0)}", "the viewport did not take the keyboard's size")
    desk.settled()
    desk.settled()
    return state(desk)


def open_discuss(desk: Desk) -> None:
    page = desk.page
    if page.locator("#discuss-open").is_visible():
        page.click("#discuss-open")
    else:
        page.click("#action-menu-open")
        page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
        page.click("#discuss-menu-open")
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
    desk.wait_for("() => { const p = document.getElementById('discuss-panel'); "
                  "return Boolean(p) && p.getClientRects().length > 0; }", "the Discuss pane did not open")


def raise_undo(desk: Desk, skip: str | None = None) -> None:
    """Dismiss a message that is not the one being answered, so its Undo chip stands."""
    page = desk.page
    row = page.evaluate("""(skip) => { const area = document.getElementById('scroll-area').getBoundingClientRect();
        const li = [...document.querySelectorAll('#discord-log > li[data-id]')]
            .find((n) => { const r = n.getBoundingClientRect();
                return n.getAttribute('data-id') !== skip && r.bottom > area.top && r.top < area.bottom
                    && n.querySelector('.done-button'); });
        if (li) li.scrollIntoView({block: 'center'});
        return li ? li.getAttribute('data-id') : null; }""", skip)
    check(isinstance(row, str), f"{desk.label}: no message on screen to dismiss")
    assert isinstance(row, str)
    desk.settled()
    done = page.locator(f'#discord-log > li[data-id="{row}"] .done-button')
    if done.is_visible():
        done.click()
    else:
        done.dispatch_event("click")
    desk.wait_for("() => { const n = document.getElementById('undo-dismiss'); "
                  "return Boolean(n) && !n.hidden && n.getClientRects().length > 0; }",
                  "dismissing did not raise the Undo chip")


def check_chip(desk: Desk, found: Json, where: str, *, hittable: bool = True) -> None:
    undo, tools = box(found, "undo"), box(found, "tools")
    desk.check(24.0 <= float(undo["height"]) <= CHIP_MAX,
               f"{where}: the Undo chip stands {undo['height']}px tall, not a chip's height: {found}")
    desk.check(float(tools["height"]) <= STRIP_MAX,
               f"{where}: the chips strip stands {tools['height']}px tall: {found}")
    if hittable:
        desk.check(found["undoHit"] is True, f"{where}: the Undo chip is not hittable at its centre: {found}")
    for part in ("replyText", "replySend"):
        if isinstance(found[part], dict):
            desk.check(not meets(undo, box(found, part)),
                       f"{where}: the Undo chip covers the reply's {part}: {found}")


def phone_walk(desk: Desk) -> None:
    page = desk.page
    open_discuss(desk)
    desk.check(state(desk)["sheet"] == "half", f"the Discuss pane is not the docked half-pane: {state(desk)}")

    # The reply form open mid-list, as the owner had it, then the dismissal that raises Undo.
    page.evaluate("() => document.querySelector(\"#discord-log > li[data-id='203']\").scrollIntoView({block: 'center'})")
    desk.settled()
    raise_undo(desk, skip="203")
    page.evaluate("() => document.querySelector(\"#discord-log > li[data-id='203'] .reply-button\").click()")
    desk.wait_for("() => document.activeElement && document.activeElement.id === 'reply-text'",
                  "Reply did not put the focus in the reply's box")
    desk.settled()

    # Keyboard up: the owner's state. The chip keeps its size at the top, clear of the form.
    found = keyboard(desk, True)
    desk.check(found["said"] == "open", f"replying, keyboard up: the page does not say the keyboard is up: {found}")
    desk.shot("1-keyboard-up-discuss")
    check_chip(desk, found, "replying, keyboard up, Discuss docked")
    desk.notes.append("keyboard up with Discuss docked: the Undo chip keeps a chip's height, clear of the reply form")

    # Keyboard down: the chip is back at the list's foot, above the pane, still a chip.
    found = keyboard(desk, False)
    desk.check(found["said"] == "closed", f"replying, keyboard down: {found}")
    desk.shot("2-keyboard-down-discuss")
    check_chip(desk, found, "replying, keyboard down, Discuss docked")
    desk.check(float(box(found, "tools")["bottom"]) <= float(box(found, "panel")["top"]) + 1,
               f"replying, keyboard down: the chips row stands over the Discuss pane: {found}")
    desk.notes.append("keyboard down: the chip stands at the list's foot above the pane, a chip still")


def desk_walk(desk: Desk) -> None:
    open_discuss(desk)
    raise_undo(desk)
    found = state(desk)
    # No hittability here: on a desk the Discuss sheet overlays the strip's right end as it
    # already did; this check is for the chip's size and anchor, the stretch itself.
    desk.shot("1-desk-discuss")
    check_chip(desk, found, "on a desk, Discuss open", hittable=False)
    # A shorter window is no keyboard on a desk: the row stays at the list's foot.
    found = keyboard(desk, True)
    check_chip(desk, found, "on a desk with a shorter window, Discuss open", hittable=False)
    anchored = desk.page.evaluate("() => { const style = getComputedStyle(document.getElementById('scroll-tools')); "
                                  "return {bottom: style.bottom, top: style.top}; }")
    assert isinstance(anchored, dict)
    desk.check(anchored["bottom"] != "auto", f"on a desk: the chips row left the list's foot: {anchored}")
    desk.shot("2-desk-shorter-window")
    desk.notes.append("on a desk with Discuss open: the Undo chip keeps a chip's height and its foot anchor")
    keyboard(desk, False)


def walk(chromium: "BrowserType", args: argparse.Namespace, label: str, width: int, height: int,
         *, touch: bool, scheme: Literal["light", "dark"]) -> str:
    api = UndoDiscussApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-undo-chip-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme=scheme,
                has_touch=touch, is_mobile=touch,
                device_scale_factor=2.6 if touch else 1)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"undo-chip-{label}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            sign_in(desk)
            if touch:
                phone_walk(desk)
            else:
                desk_walk(desk)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height} ({scheme}): " + "; ".join(desk.notes)
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
        report.append(walk(playwright.chromium, args, "phone-light", *PHONE, touch=True, scheme="light"))
        report.append(walk(playwright.chromium, args, "phone-dark", *PHONE, touch=True, scheme="dark"))
        report.append(walk(playwright.chromium, args, "desk-light", *DESK, touch=False, scheme="light"))
        report.append(walk(playwright.chromium, args, "desk-dark", *DESK, touch=False, scheme="dark"))
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
