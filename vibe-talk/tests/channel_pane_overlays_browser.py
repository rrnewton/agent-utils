#!/usr/bin/env python3
"""Floating overlays centre on the channel pane while Discuss is docked (`#1813`).

The owner, 2026-11-17: "See how the undo chip is crossing into the Discuss
region. Both it and another chip I saw pop up were still centered on the
whole screen rather than the channel pane even though the Discuss panel
was docked." The chips row (`#scroll-tools`, holding the Undo chips and the
"Newest (n)" pill), the pull report (`#pull-refresh`), the transient status
pill (`#status-line`, the "Ready." toast) and the read-aloud strip
(`#read-steer`) were all anchored to the whole screen, so with Discuss
docked beside the list on a desk they crossed into its column, and with
the half-pane docked under the list on a phone they stood over the sheet.
One shared helper (`--channel-pane-end` / `--channel-pane-bottom`,
22-discuss.css) now confines them to the channel pane.

The walk, at a 412x915 phone (device scale 2.6, touch) and a 1280x800
desk, in the light scheme and the dark, against the page served with a
fake API: open Discuss docked (the desk's Dock toggle; the phone's
half-height sheet is the docked pane), raise a dismissal's Undo chip,
scroll up so the Newest pill stands, show the status pill and the pull
report, and assert every floating overlay's bounding box lies inside the
channel pane (`#scroll-area`). On the phone, with the keyboard up (the
viewport 360px shorter, as Chrome on Android lays it out), the main
composer and its Send must stay inside the channel pane too, never over
the Discuss sheet. Every wait is on a page condition; no wall-clock wait
decides a result. Pass --screenshots DIR to keep a PNG of each state.
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

STATE_JS = """() => {
    const rect = (id) => { const n = document.getElementById(id);
        if (!n || !n.getClientRects().length) return null;
        const r = n.getBoundingClientRect();
        return {top: r.top, bottom: r.bottom, left: r.left, right: r.right,
                width: r.width, height: r.height}; };
    return {
        pane: rect('scroll-area'), panel: rect('discuss-panel'),
        tools: rect('scroll-tools'), undo: rect('undo-dismiss'),
        newest: rect('jump-newest'), status: rect('status-line'),
        pull: rect('pull-refresh'), steer: rect('read-steer'),
        composer: rect('channel-composer'), send: rect('channel-send'),
        discuss: document.getElementById('screen-main')?.dataset.discuss ?? null,
        sheet: document.getElementById('discuss-panel')?.getAttribute('data-sheet') ?? null,
    };
}"""


class PaneApi(ReplyColumnsApi):
    """The reply channel with Discuss available, so the pane can dock open."""

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


def box(found: Json, name: str) -> dict[str, object]:
    value = found[name]
    assert isinstance(value, dict), f"{name} is {value!r}, not a box"
    return value


def side(rect: dict[str, object], key: str) -> float:
    value = rect[key]
    assert isinstance(value, (int, float)), f"{key} is {value!r}, not a number"
    return float(value)


def meets(a: dict[str, object], b: dict[str, object]) -> bool:
    return (side(a, "left") < side(b, "right") - 0.5 and side(b, "left") < side(a, "right") - 0.5
            and side(a, "top") < side(b, "bottom") - 0.5 and side(b, "top") < side(a, "bottom") - 0.5)


def state(desk: Desk) -> Json:
    found = desk.page.evaluate(STATE_JS)
    assert isinstance(found, dict)
    return found


def open_discuss(desk: Desk, *, docked: bool) -> None:
    page = desk.page
    if page.locator("#discuss-open").is_visible():
        page.click("#discuss-open")
    else:
        page.click("#action-menu-open")
        page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
        page.click("#discuss-menu-open")
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
    if docked:
        page.click("#discuss-dock-toggle")
        desk.wait_for("() => document.getElementById('screen-main').dataset.discuss === 'docked'",
                      "Discuss did not dock beside the list")
    desk.wait_for("() => { const p = document.getElementById('discuss-panel'); "
                  "return Boolean(p) && p.getClientRects().length > 0; }",
                  "the Discuss pane did not open")


def raise_overlays(desk: Desk) -> None:
    """A dismissal's Undo chip up, the list scrolled up so Newest stands, and
    the status pill and pull report shown for their geometry."""
    page = desk.page
    row = page.evaluate("""() => { const area = document.getElementById('scroll-area').getBoundingClientRect();
        const li = [...document.querySelectorAll('#discord-log > li[data-id]')]
            .find((n) => { const r = n.getBoundingClientRect();
                return r.bottom > area.top && r.top < area.bottom && n.querySelector('.done-button'); });
        if (li) li.scrollIntoView({block: 'center'});
        return li ? li.getAttribute('data-id') : null; }""")
    check(isinstance(row, str), f"{desk.label}: no message on screen to dismiss")
    desk.settled()
    done = page.locator(f'#discord-log > li[data-id="{row}"] .done-button')
    if done.is_visible():
        done.click()
    else:
        done.dispatch_event("click")
    desk.wait_for("() => { const n = document.getElementById('undo-dismiss'); "
                  "return Boolean(n) && !n.hidden && n.getClientRects().length > 0; }",
                  "dismissing did not raise the Undo chip")
    page.evaluate("""() => {
        document.getElementById('scroll-area').scrollTop = 0;
        const status = document.getElementById('status-line');
        status.hidden = false;
        document.getElementById('status').textContent = '1 message marked as dealt with here — not in the channel';
        const pull = document.getElementById('pull-refresh');
        pull.hidden = false;
        pull.textContent = 'Updated 10:07';
    }""")
    desk.wait_for("() => { const n = document.getElementById('jump-newest'); "
                  "return Boolean(n) && !n.hidden && n.getClientRects().length > 0; }",
                  "scrolling up did not raise the Newest pill")
    desk.settled()


def check_inside(desk: Desk, found: Json, where: str) -> None:
    pane = box(found, "pane")
    for name in ("tools", "undo", "newest", "status", "pull"):
        rect = box(found, name)
        desk.check(side(rect, "left") >= side(pane, "left") - 1
                   and side(rect, "right") <= side(pane, "right") + 1
                   and side(rect, "top") >= side(pane, "top") - 1
                   and side(rect, "bottom") <= side(pane, "bottom") + 1,
                   f"{where}: #{name} {rect} lies outside the channel pane {pane}: {found}")
    desk.notes.append(f"{where}: the Undo chip, Newest pill, status pill and pull report "
                      "all lie inside the channel pane")


def phone_walk(desk: Desk) -> None:
    open_discuss(desk, docked=False)
    desk.check(state(desk)["sheet"] == "half", f"the Discuss pane is not the half-pane: {state(desk)}")
    raise_overlays(desk)
    found = state(desk)
    desk.shot("1-phone-docked-overlays")
    check_inside(desk, found, "phone, Discuss half-pane docked")
    # The keyboard up over the main composer at the list's end: its own Send
    # rests under the keys, so the one Send standing is the corner Send at
    # the keyboard's top — at the channel pane's foot, never over the sheet.
    desk.page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    desk.settled()
    desk.page.tap("#channel-compose-text")
    desk.wait_for("() => document.activeElement && document.activeElement.id === 'channel-compose-text'",
                  "the main composer did not take the focus")
    desk.page.set_viewport_size({"width": desk.width, "height": desk.height - KEYBOARD_PX})
    desk.wait_for(f"() => (window.visualViewport ? window.visualViewport.height : window.innerHeight) === "
                  f"{desk.height - KEYBOARD_PX}", "the viewport did not take the keyboard's size")
    desk.wait_for("() => document.documentElement.getAttribute('data-keyboard') === 'open'",
                  "the page does not say the keyboard is up")
    desk.wait_for("() => { const n = document.getElementById('keyboard-send'); "
                  "return (Boolean(n) && !n.hidden && n.getClientRects().length > 0) "
                  "|| document.getElementById('channel-send').getClientRects().length > 0; }",
                  "neither Send stood with the keyboard up")
    desk.settled()
    desk.settled()
    found = state(desk)
    desk.shot("3-phone-keyboard-corner-send")
    corner = desk.page.evaluate("""() => { const n = document.getElementById('keyboard-send');
        if (!n || n.hidden || !n.getClientRects().length) return null;
        const r = n.getBoundingClientRect();
        return {top: r.top, bottom: r.bottom, left: r.left, right: r.right,
                width: r.width, height: r.height}; }""")
    if isinstance(corner, dict):
        desk.check(not meets(corner, box(found, "panel")),
                   f"phone, keyboard up: the corner Send lies over the Discuss sheet: {found} {corner}")
    desk.shot("2-phone-keyboard-composer")
    if meets(box(found, "send"), box(found, "panel")):
        hit = desk.page.evaluate("""() => { const s = document.getElementById('channel-send');
            const r = s.getBoundingClientRect();
            const found = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
            return found === s || s.contains(found); }""")
        desk.check(hit is False,
                   f"phone, keyboard up: the main composer's Send is visible over the Discuss sheet: {found}")
    desk.notes.append("phone, keyboard up: the main composer's Send and the corner Send "
                      "stay off the Discuss sheet")
    desk.page.set_viewport_size({"width": desk.width, "height": desk.height})


def desk_walk(desk: Desk) -> None:
    open_discuss(desk, docked=True)
    desk.check(state(desk)["discuss"] == "docked", f"Discuss is not docked: {state(desk)}")
    raise_overlays(desk)
    found = state(desk)
    desk.shot("1-desk-docked-overlays")
    check_inside(desk, found, "desk, Discuss docked")
    panel = box(found, "panel")
    for name in ("undo", "newest", "status", "pull"):
        desk.check(not meets(box(found, name), panel),
                   f"desk, Discuss docked: #{name} crosses into the Discuss pane: {found}")
    # A shorter window (the desk's keyboard state): still inside the pane.
    desk.page.set_viewport_size({"width": desk.width, "height": desk.height - KEYBOARD_PX})
    desk.wait_for(f"() => window.innerHeight === {desk.height - KEYBOARD_PX}",
                  "the desk window did not shorten")
    desk.settled()
    desk.settled()
    found = state(desk)
    desk.shot("2-desk-shorter-window")
    check_inside(desk, found, "desk, Discuss docked, shorter window")
    desk.page.set_viewport_size({"width": desk.width, "height": desk.height})


def walk(chromium: "BrowserType", args: argparse.Namespace, label: str, width: int, height: int,
         *, touch: bool, scheme: Literal["light", "dark"]) -> str:
    api = PaneApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-pane-overlays-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme=scheme,
                has_touch=touch, is_mobile=touch,
                device_scale_factor=2.6 if touch else 1)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"pane-overlays-{label}", width, height)
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
