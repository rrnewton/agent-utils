#!/usr/bin/env python3
"""Measure the message box that grows, behind its Grow checkbox, in real Chromium on a desk and a phone.

`#340 composer-grow-box`. The owner, on a desk: pasting a long message left the message box tiny,
while the room left of Send was empty. A Grow checkbox in that room, off by default and kept on the
device, lets the box grow with its text up to about 60% of the window and scroll inside beyond that,
so Send stays on screen. The fake-DOM suite pins the markup, the rules and the choice; this lays the
page out in Chromium and measures the box, Send and the list they are in, with a real paste
(`navigator.clipboard` written, then Ctrl+V through CDP `Input.dispatchKeyEvent`):

  at 1280x800 and 1920x1080, the channel's box -> Grow drawn, unticked, left of Send on Send's row,
  their centres within 4px, both inside the composer, and the box at the height it always had -> a
  long paste with Grow off leaves the box at that height, scrolling inside -> ticked (all three
  checkboxes ticked, kept as "1"), the same paste grows it to within a line of 60% of the window and
  no more, scrolling inside, `field-sizing: content` computed, Send wholly on screen inside the list ->
  three short lines fit the box, under its cap -> Send clears the box and it rests again; then at
  1280x800 a reload keeps it ticked, and with a long draft kept for the channel the reload is back at
  the list's foot with Send on screen, and the call's box, with its recap line (`#331
  resume-on-websocket`) shown above it, keeps that line across the composer and Grow left of Send on
  Send's row; in a short 1280x600 window the long paste still leaves Send on screen; and at 1920x1080
  in two columns, the right column's Grow ticked in step and a long paste into its box growing it with
  that column's Send on screen.

  The owner's own order, too: a paste with Grow off, the list at its foot, and only then Grow ticked,
  which takes the focus from the box to the checkbox. The box grows and the reader stays at the foot
  with Send and Grow on screen and no jump to the newest offered, in the channel's box (ten lines and
  eighty), the call's and the right column's, and in the second desk below by a click and by Space.
  And from 30 or 40px above the foot, past the 24px that counts as the foot but with Send and Grow on
  screen: the checkbox just ticked stays where it was and the reader as far above the foot, at
  1280x800 by a click and by Space, at 1920x1080 by a click, and in the second desk by Space. With
  the checkbox given the focus and then scrolled out of view up the list, Space moves nothing.

  Safari and Firefox on a Mac focus no checkbox a click ticks, and the focus leaves the box for the
  body; Chromium focuses it everywhere. So the channel's box is ticked by such a click too (the press
  kept from moving the focus, the box let go): at the foot and 30 and 40px above it at 1280x800, 30px
  above it at 1920x1080, and 40px above it in the second desk, each with the focus on the body.

  A second desk whose browser does not size a field by its text (`CSS.supports` answering no for
  `field-sizing`, and `field-sizing: fixed` forced): the box grows through the height the page writes,
  capped the same, and a reader at the foot of the list who types line after line stays at the foot
  with Send on screen. The same typing at the foot in the first desk.

  A 393x852 phone with a touch screen and "1" stored before the page loads: no checkbox drawn, and the
  composer after a long paste exactly as tall as with "0" stored.

It serves the checked-out assets and keyboard_browser's fake channel from one loopback origin, with
sends answered. --web-root DIR runs it against another copy of the page; against one from before the
change it fails at the first measurement, which finds no checkbox. --screenshots DIR keeps a PNG of
each state, Grow off and on.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import threading
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, unquote, urlsplit

import keyboard_browser as kb
from keyboard_browser import TOKEN, Json, check

if TYPE_CHECKING:
    from playwright.sync_api import Browser, BrowserContext, CDPSession, Page

FIRST_ROW = "#discord-log > li[data-id]"
GROW_KEY = "vibe-talk.voice.composer-grow"
LONG = "\n".join(f"Line {n + 1}: the release notes as pasted, every target named and every step kept." for n in range(80))
TEN = "\n".join(f"Line {n + 1} of a ten-line message." for n in range(10))
SHORT = "first line\nsecond line\nthird line"

# Each list's jump to its newest message, which a reader left above the foot is offered.
JUMP_CHIPS = {"scroll-area": "jump-newest", "side-scroll": "side-jump-newest"}

# Each composer: the composer, its box, its Send, its Grow checkbox, and the list it is in.
CHANNEL_PARTS = ("channel-composer", "channel-compose-text", "channel-send", "channel-compose-grow", "scroll-area")
SIDE_PARTS = ("side-composer", "side-compose-text", "side-send", "side-compose-grow", "side-scroll")
VOICE_PARTS = ("voice-composer", "compose-text", "send-text", "compose-grow", "scroll-area")

GEOMETRY_JS = """([composerId, boxId, sendId, growId, listId]) => {
  const rect = (node) => {
    const r = node.getBoundingClientRect();
    return {top: r.top, bottom: r.bottom, left: r.left, right: r.right, height: r.height, width: r.width};
  };
  const box = document.getElementById(boxId);
  const grow = document.getElementById(growId);
  const label = grow.closest('label');
  const list = document.getElementById(listId);
  const style = getComputedStyle(box);
  return {
    composer: rect(document.getElementById(composerId)), box: rect(box),
    send: rect(document.getElementById(sendId)), label: rect(label), list: rect(list),
    labelShown: label.getClientRects().length > 0,
    checked: grow.checked,
    all: ['compose-grow', 'channel-compose-grow', 'side-compose-grow'].map((id) => document.getElementById(id).checked),
    scrollHeight: box.scrollHeight, clientHeight: box.clientHeight,
    fieldSizing: style.getPropertyValue('field-sizing'),
    growHeight: box.style.getPropertyValue('--composer-grow-height'),
    line: parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.25,
    // The height `rows="3"` gives the box, which is the height it has always rested at.
    rows: 3 * parseFloat(style.lineHeight) + parseFloat(style.paddingTop) + parseFloat(style.paddingBottom)
      + parseFloat(style.borderTopWidth) + parseFloat(style.borderBottomWidth),
    rem: parseFloat(getComputedStyle(document.documentElement).fontSize),
    innerHeight: window.innerHeight,
    stored: localStorage.getItem('vibe-talk.voice.composer-grow'),
    gap: list.scrollHeight - list.scrollTop - list.clientHeight,
    length: box.value.length,
  };
}"""

# A browser that does not size a field by its text: `CSS.supports` says no, and `field-sizing: fixed`
# wins over the page's rule, so the page's own measurement is what grows the box.
NO_FIELD_SIZING_JS = """(() => {
  const own = CSS.supports.bind(CSS);
  CSS.supports = (...args) => (/field-sizing/.test(args.join(' ')) ? false : own(...args));
  document.addEventListener('DOMContentLoaded', () => {
    const style = document.createElement('style');
    style.textContent = 'textarea { field-sizing: fixed !important; }';
    document.head.append(style);
  });
})();"""

# A click as Safari and Firefox on a Mac make it, which ticks a checkbox without focusing it: the focus
# leaves the box for nothing, which the document reports as the body. Chromium focuses a clicked
# checkbox on every system, so for one click (`on` true, then false to take it away) the press is kept
# from moving the focus, the box is let go, and a focus the label's click hands the checkbox is undone.
MAC_CLICK_JS = """(on) => {
  const held = window.__macClick || [];
  for (const [type, listener] of held) document.removeEventListener(type, listener, true);
  delete window.__macClick;
  if (!on) return;
  const inGrow = (event) => event.target instanceof Element && event.target.closest('.composer-grow') !== null;
  const listeners = [
    ['mousedown', (event) => {
      if (!inGrow(event)) return;
      event.preventDefault();
      if (document.activeElement instanceof HTMLElement) document.activeElement.blur();
    }],
    ['focusin', (event) => {
      if (inGrow(event)) event.target.blur();
    }],
  ];
  for (const [type, listener] of listeners) document.addEventListener(type, listener, true);
  window.__macClick = listeners;
}"""


def handler_for(api: kb.FakeApi, web_root: Path, posts: list[Json]) -> type[BaseHTTPRequestHandler]:
    """keyboard_browser's routes, with a send answered as the chat service would record it."""

    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            parts = urlsplit(self.path)
            path = unquote(parts.path)
            if not path.startswith("/api/"):
                self.asset(path)
            elif self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path == "/api/v1/client-config":
                self.json(200, api.client_config())
            elif path == "/api/v1/conversations":
                self.json(200, {"conversations": []})
            elif path == "/api/v1/transcript":
                self.json(200, {"turns": [], "has_more": False, "next_before": None})
            elif path.endswith("/timeline"):
                self.json(200, api.timeline(kb.channel_of(path), parse_qs(parts.query)))
            elif path.endswith("/pins"):
                self.json(200, api.pins(kb.channel_of(path)))
            elif path.endswith("/stream"):
                self.stream()
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def do_POST(self) -> None:  # noqa: N802
            path = unquote(urlsplit(self.path).path)
            raw: object = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"null")
            if self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path.endswith("/reply") and isinstance(raw, dict):
                body = {str(key): value for key, value in raw.items()}
                with api.lock:
                    posts.append(body)
                    serial = len(posts)
                self.json(200, {"posted": {
                    "id": f"89000000000000{serial:04d}", "channel_id": kb.channel_of(path)["id"], "author": "me",
                    "author_id": "1000000000000000098", "author_is_bot": False,
                    "timestamp": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
                    "spoken_time": "", "reply_to": body.get("reply_to"), "content": body.get("text"),
                }})
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def asset(self, request_path: str) -> None:
            relative = "voice.html" if request_path == "/" else request_path.lstrip("/")
            target = (web_root / relative).resolve()
            if web_root not in target.parents or not target.is_file():
                self.send_error(404)
                return
            body = target.read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", mimetypes.guess_type(target.name)[0] or "application/octet-stream")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def json(self, status: int, payload: Json) -> None:
            body = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
            except OSError:
                pass  # a read the browser gave up on when the page reloaded

        def stream(self) -> None:
            # Held open with keep-alive comments, as the live route is; nothing arrives on it.
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            try:
                while not api.stopping.wait(1):
                    self.wfile.write(b": keep-alive\n\n")
                    self.wfile.flush()
            except OSError:
                pass
            self.close_connection = True

        def log_message(self, _format: str, *_args: object) -> None:
            pass

    return Handler


def number(found: Json, *path: str) -> float:
    value: object = found
    for name in path:
        assert isinstance(value, dict)
        value = value[name]
    assert isinstance(value, (int, float))
    return float(value)


class Desk:
    """One page, its CDP session, and the measurements each state takes."""

    def __init__(self, page: Page, cdp: CDPSession, shots: Path | None, label: str) -> None:
        self.page = page
        self.cdp = cdp
        self.shots = shots
        self.label = label
        self.notes: list[str] = []

    def laid_out(self) -> None:
        self.page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")
        self.page.wait_for_timeout(120)

    def geometry(self, parts: tuple[str, ...] = CHANNEL_PARTS) -> Json:
        self.laid_out()
        found = self.page.evaluate(GEOMETRY_JS, list(parts))
        assert isinstance(found, dict)
        return found

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.label}-{name}.png"))

    def paste(self, box: str, text: str) -> None:
        """`text` on the clipboard, and Ctrl+V in `box`, as a reader pastes."""
        self.page.evaluate("(text) => navigator.clipboard.writeText(text)", text)
        self.page.focus(f"#{box}")
        kb.press(self.cdp, "Control+v")
        self.page.wait_for_function("([id, n]) => document.getElementById(id).value.length >= n",
                                    arg=[box, len(text)], timeout=5000)

    def clear(self, box: str) -> None:
        self.page.fill(f"#{box}", "")

    def tick(self, grow: str, on: bool) -> None:
        if self.page.evaluate("(id) => document.getElementById(id).checked", grow) != on:
            self.page.click(f"label:has(#{grow})")
        self.page.wait_for_function("([id, on]) => document.getElementById(id).checked === on", arg=[grow, on],
                                    timeout=5000)


def send_on_screen(found: Json) -> bool:
    """Send wholly inside the list it is in, and inside the window."""
    return (number(found, "send", "top") >= number(found, "list", "top") - 0.5
            and number(found, "send", "bottom") <= number(found, "list", "bottom") + 0.5
            and number(found, "send", "bottom") <= number(found, "innerHeight"))


def capped(found: Json, where: str) -> None:
    """Grown to within a line of 60% of the window and no more, scrolling inside, Send on screen."""
    cap = 0.6 * number(found, "innerHeight")
    height = number(found, "box", "height")
    check(cap - number(found, "line") - 2 <= height <= cap + 1,
          f"{where}: the box is {height:.0f}px tall, not within a line of 60% of the window ({cap:.0f}px): {found}")
    check(number(found, "scrollHeight") > number(found, "clientHeight") + 1,
          f"{where}: the long paste does not scroll inside the box at its cap: {found}")
    check(send_on_screen(found), f"{where}: Send is not wholly on screen inside the list: {found}")


def resting(found: Json, rest: Json, where: str) -> None:
    check(abs(number(found, "box", "height") - number(rest, "box", "height")) <= 1,
          f"{where}: the box is {number(found, 'box', 'height'):.0f}px, not its resting "
          f"{number(rest, 'box', 'height'):.0f}px")


def sign_in(page: Page, url: str) -> None:
    page.goto(url, wait_until="load")
    if page.locator("#api-token").is_visible():
        page.fill("#api-token", TOKEN)
        page.click("#save-token")
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    if page.evaluate("() => document.getElementById('view-switch').getAttribute('aria-checked')") != "true":
        page.click("#view-switch")
    page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)


def to_the_foot(desk: Desk, list_id: str = "scroll-area") -> None:
    desk.page.evaluate("(id) => { const list = document.getElementById(id); list.scrollTop = list.scrollHeight; }", list_id)
    desk.laid_out()


def channel_box(desk: Desk, posts: list[Json]) -> Json:
    """The channel's box at this window's size: Grow off, on, short lines, and Send. Leaves Grow ticked."""
    box, send, grow = CHANNEL_PARTS[1], CHANNEL_PARTS[2], CHANNEL_PARTS[3]
    where = desk.label
    desk.tick(grow, False)
    desk.clear(box)
    to_the_foot(desk)
    rest = desk.geometry()
    check(rest["labelShown"] is True and rest["checked"] is False, f"{where}: Grow is not drawn, unticked: {rest}")
    check(number(rest, "label", "right") <= number(rest, "send", "left"), f"{where}: Grow is not left of Send: {rest}")
    centre = (number(rest, "label", "top") + number(rest, "label", "bottom")) / 2
    send_centre = (number(rest, "send", "top") + number(rest, "send", "bottom")) / 2
    check(abs(centre - send_centre) <= 4, f"{where}: Grow is not on Send's row ({centre:.1f} vs {send_centre:.1f})")
    for part in ("label", "send"):
        check(number(rest, "composer", "left") <= number(rest, part, "left")
              and number(rest, part, "right") <= number(rest, "composer", "right")
              and number(rest, "composer", "top") <= number(rest, part, "top")
              and number(rest, part, "bottom") <= number(rest, "composer", "bottom"),
              f"{where}: {part} is not inside the composer: {rest}")
    check(abs(number(rest, "box", "height") - number(rest, "rows")) <= 1,
          f"{where}: the empty box is {number(rest, 'box', 'height'):.0f}px, not the three lines "
          f"({number(rest, 'rows'):.0f}px) it always was")
    check(number(rest, "label", "height") >= 24, f"{where}: Grow is a {number(rest, 'label', 'height'):.0f}px target")
    desk.shot("grow-off-empty")

    desk.paste(box, LONG)
    off = desk.geometry()
    resting(off, rest, f"{where}, Grow off, a long paste")
    check(number(off, "scrollHeight") > number(off, "clientHeight"), f"{where}: with Grow off the paste does not scroll inside")
    desk.shot("grow-off-long")

    desk.clear(box)
    desk.tick(grow, True)
    ticked = desk.geometry()
    check(ticked["all"] == [True, True, True], f"{where}: ticking one Grow left the others: {ticked['all']}")
    check(ticked["stored"] == "1", f"{where}: ticking Grow kept {ticked['stored']!r}")
    resting(ticked, rest, f"{where}, Grow on, the box empty")
    desk.paste(box, LONG)
    grown = desk.geometry()
    capped(grown, f"{where}, Grow on, a long paste")
    check(grown["fieldSizing"] == "content", f"{where}: the growing box computes field-sizing {grown['fieldSizing']!r}")
    desk.shot("grow-on-long")

    desk.clear(box)
    desk.page.fill(f"#{box}", SHORT)
    short = desk.geometry()
    check(number(short, "scrollHeight") <= number(short, "clientHeight") + 1,
          f"{where}: three short lines do not fit the growing box: {short}")
    check(number(short, "box", "height") < 0.6 * number(short, "innerHeight"), f"{where}: three lines took the cap")
    sent = len(posts)
    desk.page.click(f"#{send}")
    desk.page.wait_for_function("(id) => document.getElementById(id).value === ''", arg=box, timeout=5000)
    check(len(posts) == sent + 1, f"{where}: Send posted {len(posts) - sent} messages")
    resting(desk.geometry(), rest, f"{where}, the box cleared by Send")
    desk.notes.append(f"{where}: Grow drawn left of Send on its row, the box resting at "
                      f"{number(rest, 'box', 'height'):.0f}px; Grow off kept it there through a long paste; on, the paste "
                      f"grew it to {number(grown, 'box', 'height'):.0f}px of a {number(grown, 'innerHeight'):.0f}px window "
                      "with Send on screen; three lines fitted; Send cleared it back to rest")
    return rest


def typing_at_the_foot(desk: Desk, rest: Json) -> None:
    """A reader at the foot of the list who types line after line stays at the foot, Send on screen."""
    box = CHANNEL_PARTS[1]
    desk.clear(box)
    to_the_foot(desk)
    desk.page.focus(f"#{box}")
    for line in range(14):
        for letter in "words":
            kb.press(desk.cdp, letter)
        kb.press(desk.cdp, "Shift+Enter")
        found = desk.geometry()
        check(number(found, "gap") <= 2, f"{desk.label}: typing line {line + 1} left the reader "
                                         f"{number(found, 'gap'):.0f}px above the foot: {found}")
        check(send_on_screen(found), f"{desk.label}: typing line {line + 1} put Send off screen: {found}")
    grown = desk.geometry()
    check(number(grown, "box", "height") > number(rest, "box", "height") + 2 * number(grown, "line"),
          f"{desk.label}: fourteen typed lines did not grow the box: {grown}")
    desk.clear(box)
    desk.notes.append(f"{desk.label}: a reader at the foot typing fourteen lines stayed at the foot, Send on screen")


def tick_after_paste(desk: Desk, parts: tuple[str, ...], text: str, by: str = "click", up: int = 0) -> None:
    """The owner's order: paste with Grow off, the list at its foot, see the box tiny, then tick Grow.

    The focus leaves the box for the checkbox as it is ticked (`by` "click" on its label, or "space" on
    the checkbox), or for nothing (`by` "mac-click": a click on its label as Safari and Firefox on a Mac
    make it, focusing no checkbox), so nothing that follows the reader's typing can be what keeps them
    at the foot: the box grows and the reader stays at the foot, Send, Grow and the box all on screen,
    no jump to the newest offered; and clicking back into the box moves nothing.

    `up` px above the foot instead, past the 24px that still counts as the foot but with Send and Grow
    on screen (at 1280x800 the label shows up to about 47px up): the checkbox just ticked stays where it
    was, under the pointer, and so does the room under it, Send and Grow on screen.
    """
    _composer, box, _send, grow, list_id = parts
    lines = text.count("\n") + 1
    where = f"{desk.label}, #{box}, Grow ticked by {by} after a {lines}-line paste"
    if up:
        where += f", {up}px above the foot"
    desk.tick(grow, False)
    desk.clear(box)
    desk.paste(box, text)
    to_the_foot(desk, list_id)
    if up:
        desk.page.evaluate("([id, up]) => { const list = document.getElementById(id); "
                           "list.scrollTop = list.scrollHeight - list.clientHeight - up; }", [list_id, up])
    before = desk.geometry(parts)
    check(abs(number(before, "gap") - up) <= 2 and send_on_screen(before)
          and number(before, "label", "bottom") <= number(before, "list", "bottom") + 0.5,
          f"{where}: the premise, {up}px above the foot with Send and Grow on screen before Grow is ticked, does "
          f"not hold: {before}")
    if by == "space":
        desk.page.focus(f"#{grow}")
        kb.press(desk.cdp, " ")
    else:
        desk.page.evaluate(MAC_CLICK_JS, by == "mac-click")
        desk.page.click(f"label:has(#{grow})")
    desk.page.wait_for_function("(id) => document.getElementById(id).checked", arg=grow, timeout=5000)
    focused = desk.page.evaluate("() => document.activeElement === document.body ? 'body' "
                                 ": document.activeElement && document.activeElement.id")
    desk.page.evaluate(MAC_CLICK_JS, False)
    held, named = ("body", "nothing") if by == "mac-click" else (grow, "the checkbox just ticked")
    check(focused == held, f"{where}: the premise, the focus on {named}, does not hold: it is on {focused!r}")
    # Where the reader is to be left: at the foot, or as far above it as they were.
    gap = number(before, "gap") if up else 0
    after = desk.geometry(parts)
    check(number(after, "box", "height") > number(before, "box", "height") + 2 * number(after, "line"),
          f"{where}: the box did not grow: {after}")
    check(abs(number(after, "gap") - gap) <= 2,
          f"{where}: the reader is left {number(after, 'gap'):.0f}px above the foot, not {gap:.0f}px: {after}")
    if up:
        check(abs(number(after, "label", "top") - number(before, "label", "top")) <= 2,
              f"{where}: the Grow checkbox just ticked moved from {number(before, 'label', 'top'):.0f}px to "
              f"{number(after, 'label', 'top'):.0f}px: {after}")
    check(send_on_screen(after), f"{where}: Send is not wholly on screen inside the list: {after}")
    check(number(after, "label", "bottom") <= min(number(after, "list", "bottom"), number(after, "innerHeight")) + 0.5,
          f"{where}: the Grow checkbox just ticked left the screen: {after}")
    check(number(after, "box", "top") >= number(after, "list", "top") - 0.5,
          f"{where}: the top of the box is above the list: {after}")
    chip = JUMP_CHIPS[list_id]
    check(desk.page.evaluate("(id) => document.getElementById(id).hidden", chip) is True,
          f"{where}: the jump to the newest message (#{chip}) is offered {number(after, 'gap'):.0f}px above the foot")
    desk.shot(f"tick-after-paste-{box}-{lines}-{by}" + (f"-{up}up" if up else ""))
    desk.page.click(f"#{box}")
    back = desk.geometry(parts)
    check(abs(number(back, "gap") - gap) <= 2 and send_on_screen(back),
          f"{where}: clicking back into the box left the reader {number(back, 'gap'):.0f}px above the foot: {back}")
    desk.clear(box)
    kept = "with the checkbox kept under the pointer" if up else "with the reader kept at the foot"
    desk.notes.append(f"{where}: grown from {number(before, 'box', 'height'):.0f}px to "
                      f"{number(after, 'box', 'height'):.0f}px {kept}, Send and Grow on screen")


def space_from_up_the_list(desk: Desk) -> None:
    """The checkbox given the focus, then scrolled out of view up the list, and Space pressed: nothing moves.

    What keeps a checkbox just ticked under the pointer is for a checkbox on screen; one the reader has
    scrolled away from moves no list, as a box growing out of sight moves none.
    """
    box, grow, list_id = CHANNEL_PARTS[1], CHANNEL_PARTS[3], CHANNEL_PARTS[4]
    where = f"{desk.label}, Grow ticked by Space with the checkbox scrolled out of view up the list"
    desk.tick(grow, False)
    desk.clear(box)
    desk.paste(box, TEN)
    to_the_foot(desk, list_id)
    desk.page.focus(f"#{grow}")
    desk.page.evaluate("(id) => { document.getElementById(id).scrollTop -= 300; }", list_id)
    before = desk.geometry()
    place = desk.page.evaluate("(id) => document.getElementById(id).scrollTop", list_id)
    check(number(before, "label", "top") >= number(before, "list", "bottom")
          and desk.page.evaluate("() => document.activeElement && document.activeElement.id") == grow,
          f"{where}: the premise, the focus on a checkbox out of view, does not hold: {before}")
    kb.press(desk.cdp, " ")
    desk.page.wait_for_function("(id) => document.getElementById(id).checked", arg=grow, timeout=5000)
    after = desk.geometry()
    check(number(after, "box", "height") > number(before, "box", "height") + 2 * number(after, "line"),
          f"{where}: the box did not grow: {after}")
    moved = desk.page.evaluate("(id) => document.getElementById(id).scrollTop", list_id) - place
    check(abs(moved) <= 1, f"{where}: the list moved {moved:.0f}px")
    desk.clear(box)
    desk.notes.append(f"{where}: the box grew out of sight and the list stayed where the reader had it")


def call_tick_after_paste(desk: Desk) -> None:
    """`tick_after_paste` in the call's box, on the call's view."""
    desk.page.click("#view-switch")
    desk.page.wait_for_selector(f"#{VOICE_PARTS[1]}", state="visible", timeout=10_000)
    tick_after_paste(desk, VOICE_PARTS, TEN)
    desk.page.click("#view-switch")
    desk.page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)


def reload_with_a_long_draft(desk: Desk, url: str) -> None:
    box = CHANNEL_PARTS[1]
    desk.page.reload(wait_until="load")
    desk.page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)
    again = desk.geometry()
    check(again["checked"] is True and again["stored"] == "1", f"{desk.label}: a reload unticked Grow: {again}")
    # A long draft kept for the channel, the reader at the foot: back at the foot, Send on screen.
    desk.page.fill(f"#{box}", LONG)
    to_the_foot(desk)
    desk.page.reload(wait_until="load")
    desk.page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)
    desk.page.wait_for_function("(id) => document.getElementById(id).value.length > 1000", arg=box, timeout=5000)
    desk.page.wait_for_timeout(400)
    back = desk.geometry()
    check(number(back, "gap") <= 2, f"{desk.label}: back with a long draft, the reader is {number(back, 'gap'):.0f}px "
                                    f"above the foot: {back}")
    check(send_on_screen(back), f"{desk.label}: back with a long draft, Send is not on screen: {back}")
    desk.shot("grow-on-reloaded-long-draft")
    desk.clear(box)
    desk.notes.append(f"{desk.label}: a reload kept Grow ticked, and with a long draft came back at the foot with "
                      "Send on screen")


def call_box_with_recap(desk: Desk) -> None:
    """The call's box with its recap line shown above it: the line across the composer, Grow left of Send on its row."""
    page = desk.page
    where = f"{desk.label}, the call's box under its recap line"
    page.click("#view-switch")
    page.wait_for_selector(f"#{VOICE_PARTS[1]}", state="visible", timeout=10_000)
    # What `renderCallRecap` draws once the voice service has taken a recap; no call is needed to lay it out.
    page.evaluate("""() => {
      const line = document.getElementById('call-recap');
      line.textContent = 'Caught the agent up with 1,240 words from earlier';
      line.setAttribute('data-recap', 'accepted');
      line.hidden = false;
    }""")
    found = desk.geometry(VOICE_PARTS)
    recap = page.evaluate("""() => {
      const r = document.getElementById('call-recap').getBoundingClientRect();
      return {top: r.top, bottom: r.bottom, left: r.left, right: r.right, height: r.height, width: r.width};
    }""")
    assert isinstance(recap, dict)
    found["recap"] = recap
    check(number(found, "recap", "height") > 0, f"{where}: the recap line is not drawn: {found}")
    check(abs(number(found, "recap", "left") - number(found, "box", "left")) <= 1
          and abs(number(found, "recap", "right") - number(found, "box", "right")) <= 1,
          f"{where}: the recap line does not span the composer as the box does: {found}")
    check(number(found, "recap", "bottom") <= number(found, "box", "top") + 0.5,
          f"{where}: the recap line is not above the box: {found}")
    check(found["labelShown"] is True, f"{where}: Grow is not drawn: {found}")
    for part in ("label", "send"):
        check(number(found, part, "top") >= number(found, "box", "bottom") - 0.5,
              f"{where}: {part} is not below the box: {found}")
    check(number(found, "label", "right") <= number(found, "send", "left"), f"{where}: Grow is not left of Send: {found}")
    centre = (number(found, "label", "top") + number(found, "label", "bottom")) / 2
    send_centre = (number(found, "send", "top") + number(found, "send", "bottom")) / 2
    check(abs(centre - send_centre) <= 4, f"{where}: Grow is not on Send's row ({centre:.1f} vs {send_centre:.1f})")
    desk.shot("call-recap")
    page.evaluate("""() => {
      const line = document.getElementById('call-recap');
      line.textContent = '';
      line.removeAttribute('data-recap');
      line.hidden = true;
    }""")
    page.click("#view-switch")
    page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)
    desk.notes.append(f"{desk.label}: the call's recap line spanned its composer above the box, Grow left of Send on "
                      "Send's row")


def two_columns(desk: Desk) -> None:
    page = desk.page
    page.click("#column-toggle")
    page.wait_for_selector("#side-cards > li[data-id] .side-card-open", state="visible", timeout=10_000)
    page.click("#side-cards > li[data-id] .side-card-open")
    page.wait_for_selector("#side-compose-text", state="visible", timeout=10_000)
    side = desk.geometry(SIDE_PARTS)
    check(side["checked"] is True and side["labelShown"] is True, f"{desk.label}: the right column's Grow is not "
                                                                  f"drawn ticked: {side}")
    desk.paste(SIDE_PARTS[1], LONG)
    grown = desk.geometry(SIDE_PARTS)
    capped(grown, f"{desk.label}, the right column's box, a long paste")
    desk.shot("grow-on-two-columns")
    desk.clear(SIDE_PARTS[1])
    desk.notes.append(f"{desk.label}: in two columns the right column's Grow was ticked in step, and a long paste grew "
                      "its box with its Send on screen")
    tick_after_paste(desk, SIDE_PARTS, TEN)
    page.click("#column-toggle")
    page.wait_for_function("() => document.getElementById('side-column').getClientRects().length === 0", timeout=5000)


def short_window(desk: Desk) -> None:
    desk.page.set_viewport_size({"width": 1280, "height": 600})
    desk.label = "1280x600"
    desk.clear(CHANNEL_PARTS[1])
    to_the_foot(desk)
    desk.paste(CHANNEL_PARTS[1], LONG)
    grown = desk.geometry()
    check(send_on_screen(grown), f"1280x600: the long paste put Send off screen: {grown}")
    check(number(grown, "box", "height") <= 0.6 * number(grown, "innerHeight") + 1,
          f"1280x600: the box passed 60% of the window: {grown}")
    desk.shot("grow-on-long")
    desk.clear(CHANNEL_PARTS[1])
    desk.notes.append(f"1280x600: a long paste grew the box to {number(grown, 'box', 'height'):.0f}px with Send on screen")


def desk_walk(browser: Browser, url: str, args: argparse.Namespace, posts: list[Json], errors: list[str]) -> list[str]:
    context = browser.new_context(viewport={"width": 1280, "height": 800}, color_scheme="dark")
    context.grant_permissions(["clipboard-read", "clipboard-write"], origin=url.rstrip("/"))
    page = context.new_page()
    page.on("pageerror", lambda error: errors.append(str(error)))
    desk = Desk(page, context.new_cdp_session(page), args.screenshots, "1280x800")
    sign_in(page, url)
    check(page.evaluate("() => CSS.supports('field-sizing', 'content')") is True,
          "this Chromium has no field-sizing: the fallback desk below is the only one that grows")
    rest = channel_box(desk, posts)
    typing_at_the_foot(desk, rest)
    tick_after_paste(desk, CHANNEL_PARTS, TEN)
    tick_after_paste(desk, CHANNEL_PARTS, LONG)
    tick_after_paste(desk, CHANNEL_PARTS, TEN, up=30)
    tick_after_paste(desk, CHANNEL_PARTS, TEN, by="space", up=40)
    tick_after_paste(desk, CHANNEL_PARTS, TEN, by="mac-click")
    tick_after_paste(desk, CHANNEL_PARTS, TEN, by="mac-click", up=30)
    tick_after_paste(desk, CHANNEL_PARTS, TEN, by="mac-click", up=40)
    space_from_up_the_list(desk)
    reload_with_a_long_draft(desk, url)
    call_box_with_recap(desk)
    call_tick_after_paste(desk)
    short_window(desk)
    page.set_viewport_size({"width": 1920, "height": 1080})
    desk.label = "1920x1080"
    channel_box(desk, posts)
    tick_after_paste(desk, CHANNEL_PARTS, TEN, up=30)
    tick_after_paste(desk, CHANNEL_PARTS, TEN, by="mac-click", up=30)
    two_columns(desk)
    context.close()
    return desk.notes


def fallback_walk(browser: Browser, url: str, args: argparse.Namespace, posts: list[Json], errors: list[str]) -> list[str]:
    context = browser.new_context(viewport={"width": 1280, "height": 800}, color_scheme="dark")
    context.grant_permissions(["clipboard-read", "clipboard-write"], origin=url.rstrip("/"))
    context.add_init_script(NO_FIELD_SIZING_JS)
    page = context.new_page()
    page.on("pageerror", lambda error: errors.append(str(error)))
    desk = Desk(page, context.new_cdp_session(page), args.screenshots, "1280x800-no-field-sizing")
    sign_in(page, url)
    box, grow = CHANNEL_PARTS[1], CHANNEL_PARTS[3]
    desk.tick(grow, False)
    desk.clear(box)
    rest = desk.geometry()
    check(rest["fieldSizing"] == "fixed", f"the fallback desk still sizes by content: {rest['fieldSizing']!r}")
    desk.tick(grow, True)
    desk.paste(box, LONG)
    grown = desk.geometry()
    check(str(grown["growHeight"]).endswith("px"), f"the fallback desk wrote no height for the box: {grown}")
    capped(grown, "without field-sizing, a long paste")
    desk.shot("grow-on-long")
    desk.clear(box)
    resting(desk.geometry(), rest, "without field-sizing, the box cleared")
    typing_at_the_foot(desk, rest)
    tick_after_paste(desk, CHANNEL_PARTS, TEN)
    tick_after_paste(desk, CHANNEL_PARTS, LONG, by="space")
    tick_after_paste(desk, CHANNEL_PARTS, TEN, by="space", up=30)
    tick_after_paste(desk, CHANNEL_PARTS, TEN, by="mac-click", up=40)
    desk.tick(grow, False)
    off = desk.geometry()
    check(off["growHeight"] == "", f"without field-sizing, Grow off left a height on the box: {off}")
    context.close()
    desk.notes.append(f"without field-sizing, the page's measured height grew the box to "
                      f"{number(grown, 'box', 'height'):.0f}px, capped the same, and took it away with Grow off")
    return desk.notes


def phone_walk(browser: Browser, url: str, args: argparse.Namespace, errors: list[str]) -> list[str]:
    heights: dict[str, Json] = {}
    for stored in ("1", "0"):
        context: BrowserContext = browser.new_context(viewport={"width": 393, "height": 852}, color_scheme="dark",
                                                      device_scale_factor=2.75, is_mobile=True, has_touch=True)
        context.add_init_script(f"localStorage.setItem('{GROW_KEY}', '{stored}');")
        page = context.new_page()
        page.on("pageerror", lambda error: errors.append(str(error)))
        desk = Desk(page, context.new_cdp_session(page), args.screenshots, f"393x852-stored-{stored}")
        sign_in(page, url)
        page.fill(f"#{CHANNEL_PARTS[1]}", LONG)
        to_the_foot(desk)
        found = desk.geometry()
        check(found["labelShown"] is False, f"393x852 with {stored!r} stored: the phone draws Grow: {found}")
        check(found["checked"] is (stored == "1"), f"393x852: the stored {stored!r} was not read: {found}")
        heights[stored] = found
        desk.shot("long-draft")
        context.close()
    for part in ("box", "composer"):
        check(abs(number(heights["1"], part, "height") - number(heights["0"], part, "height")) <= 0.5,
              f"393x852: the {part} is {number(heights['1'], part, 'height'):.0f}px with Grow stored on and "
              f"{number(heights['0'], part, 'height'):.0f}px with it off")
    return [f"393x852 with touch: no Grow drawn, and the composer {number(heights['1'], 'composer', 'height'):.0f}px "
            "tall with a long draft whether Grow was stored on or off"]


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=Path(__file__).resolve().parents[1] / "web",
                        help="the page assets to serve (default: this checkout's vibe-talk/web)")
    parser.add_argument("--screenshots", type=Path, default=None,
                        help="a directory to keep a PNG of each state in (default: none kept)")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)

    api = kb.FakeApi()
    posts: list[Json] = []
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api, args.web_root.resolve(), posts))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/"
    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(headless=True, executable_path=args.browser_executable)
            errors: list[str] = []
            notes = desk_walk(browser, url, args, posts, errors)
            notes += fallback_walk(browser, url, args, posts, errors)
            notes += phone_walk(browser, url, args, errors)
            check(not errors, f"the page threw: {errors}")
            version = browser.version
            browser.close()
        print(f"{version}: " + "; ".join(notes))
        return 0
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
