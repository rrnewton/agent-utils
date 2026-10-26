#!/usr/bin/env python3
"""`#524 thread-bridge-even-connectors` and `#526 pack-triangle-dim` in real Chromium.

The owner, 2026-10-25: "I don't like the variable length thread bridge connectors. Right
now we've made MY messages less wide and right-indented them. Let's have them
left-justified so the colored line is the SAME length... if we keep the current width for
my messages we'll have a ragged edge on the right, but at least we won't have these
variable length connectors." And, of the pack triangle: "keep them the same hue but DIM
them some, and move them a bit up and left. They should be close to touching the bottom of
the message. When I hover them they appear as a colored rounded rectangle. The
arrow/triangle should be horizontally centered in that rectangle."

Against the fake API of tests/threads_polish_browser.py with two of the runner thread's
replies the owner's, in the Threads view with the runner unpacked, at 393x852, 412x915,
768x1024 and 1280x800, each in dark and light:

* every unpacked row starts at the same left edge and every bridge connector (the row's
  left edge back to its spine) is the same length, the owner's rows included; the owner's
  rows keep their narrower width and their own colour, their right edge short of the
  others' — the ragged right edge the owner accepted;
* the pack triangle rests dimmed in its card's hue and at full strength under the mouse,
  its tip a few px below the bottom of the card above it, horizontally centred in its
  button's rectangle, its target at least 32px, on a phone its ink a few px from the
  screen edge (`#501 thread-bridge-gutter`);
* `pack-triangle-clear-bridge` (2026-10-26, "I want them to move left enough that the rounded
  rectangle button that comes up when I hover is not intersecting the bridge"): at 768 and
  1280 wide the whole button, the rectangle its wash draws, stands at least 3px left of the
  bridge's vertical line; on a phone, whose gutter is narrower than that, how far it meets the
  line is recorded;
* replies gathered under their root in All (Reply on the root, `#215 reply-coalesce`) keep
  one left edge and one connector length too;
* the triangle still packs its thread.

Pass --screenshots DIR to keep a PNG of each state; --web-root DIR for another copy.
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
from inline_reply_browser import place, press_reply, sign_in
from offline_cache_browser import SPEAKERS, check, handler_for
from threads_polish_browser import PolishApi, to_threads, unpack_to
from two_column_browser import RUNNER, Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

SIZES = (("phone-393", 393, 852, True, "dark"), ("phone-393-light", 393, 852, True, "light"),
         ("phone-412", 412, 915, True, "dark"), ("phone-412-light", 412, 915, True, "light"),
         ("tablet-768", 768, 1024, True, "dark"), ("tablet-768-light", 768, 1024, True, "light"),
         ("desk", 1280, 800, False, "dark"), ("desk-light", 1280, 800, False, "light"))

# `pack-triangle-clear-bridge`: the least room, in px, between the pack triangle's whole button — the
# rounded rectangle its hover wash draws — and the bridge's vertical line to its right.
BRIDGE_CLEARANCE_PX = 3

# The runner's replies the owner wrote (message ids, from the columns fixture's plan).
OWN_REPLIES = ("206", "217")

ROWS_JS = """(id) => {
    const box = (node) => { const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height}; };
    const card = document.querySelector(`#side-cards > li[data-card][data-id='${id}']`);
    const rows = [...document.querySelectorAll(`#side-cards > li[data-unpack-thread='${id}']`)];
    const first = rows[0];
    const button = first.querySelector('.unpack-pack');
    const mark = button.querySelector('.unpack-pack-mark');
    const tri = getComputedStyle(mark, '::before');
    const triW = parseFloat(tri.borderLeftWidth) + parseFloat(tri.borderRightWidth);
    const triH = parseFloat(tri.borderBottomWidth);
    const markBox = box(mark);
    const bridge = first.querySelector(':scope > .reply-bridge');
    const bridgeBox = box(bridge);
    const stroke = parseFloat(getComputedStyle(bridge).borderLeftWidth);
    return {card: box(card), spineLeft: bridgeBox.left, spineRight: bridgeBox.left + stroke,
        spineTop: bridgeBox.top, spineBottom: bridgeBox.bottom, buttonRadius: getComputedStyle(button).borderTopLeftRadius, overflow: document.documentElement.scrollWidth - window.innerWidth,
        rows: rows.map((li) => ({id: li.getAttribute('data-id'), who: li.getAttribute('data-who'),
            box: box(li), bg: getComputedStyle(li).backgroundColor,
            bridge: box(li.querySelector(':scope > .reply-bridge'))})),
        button: box(button), mark: markBox, markOpacity: parseFloat(getComputedStyle(mark).opacity),
        buttonBg: getComputedStyle(button).backgroundColor, markColor: getComputedStyle(button).color,
        spine: getComputedStyle(first.querySelector(':scope > .reply-bridge')).borderLeftColor,
        inkLeft: markBox.left + (markBox.width - triW) / 2,
        inkCentre: markBox.left + markBox.width / 2,
        tipY: markBox.top + (markBox.height - triH) / 2};
}"""

STACK_JS = """() => {
    const box = (node) => { const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, width: r.width}; };
    return [...document.querySelectorAll('#discord-log > li[data-stack-under]')].map((li) => ({
        id: li.getAttribute('data-id'), who: li.getAttribute('data-who'), box: box(li),
        bridge: box(li.querySelector(':scope > .reply-bridge'))}));
}"""


class EvenApi(PolishApi):
    """`PolishApi` with two of the runner thread's replies the owner's."""

    def __init__(self) -> None:
        super().__init__()
        for index, row in enumerate(self.messages):
            if str(row.get("id")) in OWN_REPLIES:
                self.messages[index] = {**row, **SPEAKERS["me"]}


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per state into (default: none)")
    parser.add_argument("--port", type=int, default=0, help="loopback port for the fake server (default: 0)")
    parser.add_argument("--size", choices=[size[0] for size in SIZES], action="append",
                        help="walk only at these sizes, repeatable (default: all)")
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
        for label, width, height, touch, scheme in SIZES:
            if args.size and label not in args.size:
                continue
            report.append(walk(playwright.chromium, args, label, width, height, touch, scheme))
    for line in report:
        print(line)
    return 0


def part(found: dict[str, object], name: str) -> dict[str, float]:
    piece = found[name]
    assert isinstance(piece, dict)
    return {str(key): float(value) for key, value in piece.items() if isinstance(value, (int, float))}


def rows_of(found: dict[str, object]) -> list[dict[str, object]]:
    rows = found["rows"]
    assert isinstance(rows, list)
    return [row for row in rows if isinstance(row, dict)]


def box_of(row: dict[str, object], name: str) -> dict[str, float]:
    piece = row[name]
    assert isinstance(piece, dict)
    return {str(key): float(value) for key, value in piece.items() if isinstance(value, (int, float))}


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
         touch: bool, scheme: str) -> str:
    api = EvenApi()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-bridge-even-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme=scheme,  # type: ignore[arg-type]
                has_touch=touch, is_mobile=touch)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"bridge-even-{label}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            sign_in(desk)

            # Replies gathered in All: one left edge, one connector length.
            place(desk, "discord-log", "202", 0.15)
            press_reply(desk, "discord-log", "202")
            desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-stack-under]').length >= 8",
                          "the runner's replies did not gather in All")
            desk.settled()
            stack = page.evaluate(STACK_JS)
            assert isinstance(stack, list)
            own = [row for row in stack if isinstance(row, dict) and row.get("who") == "me"]
            desk.check(len(own) >= 1, f"All: no owner reply gathered: {stack}")
            if stack and own:
                lefts = [box_of(row, "box")["left"] for row in stack if isinstance(row, dict)]
                spans = [box_of(row, "box")["left"] - box_of(row, "bridge")["left"]
                         for row in stack if isinstance(row, dict)]
                desk.check(max(lefts) - min(lefts) <= 1.5, f"All: gathered rows start at {lefts}")
                desk.check(max(spans) - min(spans) <= 1.5, f"All: gathered connectors reach {spans}")
            desk.shot("1-all-gathered")
            page.keyboard.press("Escape")
            desk.wait_for("() => document.getElementById('column-reply').hidden", "Escape did not put the box away")

            to_threads(desk)
            unpack_to(desk, "true")
            desk.wait_for(f"() => document.querySelectorAll(\"#side-cards > li[data-unpack-thread='{RUNNER}']\").length >= 9",
                          "the runner did not unpack under its card")
            desk.settled()
            page.evaluate(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{RUNNER}']\")"
                          ".scrollIntoView({block: 'start'})")
            desk.settled()
            found = page.evaluate(ROWS_JS, RUNNER)
            assert isinstance(found, dict)
            rows = rows_of(found)
            mine = [row for row in rows if row.get("who") == "me"]
            others = [row for row in rows if row.get("who") != "me"]
            desk.check(float(str(found["overflow"])) <= 0, f"the page scrolls sideways: {found['overflow']}")
            desk.check(len(mine) >= 1 and len(others) >= 2, f"the unpacked thread holds no mix: {rows}")
            if rows and mine and others:
                lefts = [box_of(row, "box")["left"] for row in rows]
                spans = [box_of(row, "box")["left"] - box_of(row, "bridge")["left"] for row in rows]
                desk.check(max(lefts) - min(lefts) <= 1.5, f"unpacked rows start at {lefts}")
                desk.check(max(spans) - min(spans) <= 1.5, f"bridge connectors reach {spans}")
                other_width = box_of(others[0], "box")["width"]
                for row in mine:
                    box = box_of(row, "box")
                    desk.check(box["width"] < other_width - 8,
                               f"the owner's row is not the narrower one: {box['width']} against {other_width}")
                    desk.check(box["right"] < box_of(others[0], "box")["right"] - 8,
                               "the owner's row reaches the others' right edge")
                    desk.check(row.get("bg") != others[0].get("bg"), "the owner's row lost its colour")
            button, card = part(found, "button"), part(found, "card")
            desk.check(button["width"] >= 32 and button["height"] >= 32,
                       f"the triangle's target is {button['width']:.0f}px by {button['height']:.0f}px")
            desk.check(abs(float(str(found["inkCentre"])) - (button["left"] + button["width"] / 2)) <= 2,
                       f"the triangle is not centred in its rectangle: ink centre {found['inkCentre']}, "
                       f"button {button['left']}..{button['right']}")
            # `pack-triangle-clear-bridge`. The owner, 2026-10-26: "I want them to move left enough that
            # the rounded rectangle button that comes up when I hover is not intersecting the bridge.
            # Visual overlap between objects like that looks sloppy and bad." The whole button is the
            # wash's rectangle, its 32px phone floor included, and it stands beside the spine for its
            # whole height, so its right edge must stop short of the spine's left edge.
            clearance = float(str(found["spineLeft"])) - button["right"]
            beside = (button["top"] < float(str(found["spineBottom"]))
                      and button["bottom"] > float(str(found["spineTop"])))
            desk.notes.append(f"rectangle {button['left']:.1f}..{button['right']:.1f}, {clearance:.1f}px clear of "
                              f"the line at {float(str(found['spineLeft'])):.1f}")
            if width >= 600:
                desk.check(not beside or clearance >= BRIDGE_CLEARANCE_PX,
                           f"the triangle's rectangle {button['left']:.1f}..{button['right']:.1f} comes within "
                           f"{clearance:.1f}px of the bridge's line at {found['spineLeft']:.1f}")
            else:
                # A phone's gutter left of the line (about 28px) is narrower than the 32px target and the
                # room, so the rectangle — drawn there only under a press, a phone having no hover — still
                # meets the line until the owner chooses between a smaller wash and the bridge moving
                # right. Recorded, not refused, and refused if it ever gets worse.
                desk.check(not beside or clearance >= -5,
                           f"the triangle's rectangle reaches {-clearance:.1f}px over the bridge's line")
                desk.notes.append("phone: the rectangle meets the line, pending the owner's choice")
            desk.check(button["left"] >= 0, f"the triangle's rectangle runs off the screen at {button['left']:.1f}px")
            gap = float(str(found["tipY"])) - card["bottom"]
            desk.check(-2 <= gap <= 10, f"the triangle's tip is {gap:.1f}px below the card above it")
            desk.check(float(str(found["markOpacity"])) <= 0.85,
                       f"the triangle rests at opacity {found['markOpacity']}, not dimmed")
            desk.check(str(found["markColor"]) == str(found["spine"]),
                       f"the triangle {found['markColor']} is not its bridge's hue {found['spine']}")
            if touch and width < 600:
                # On a phone the gutter is the screen's edge (`#501 thread-bridge-gutter`).
                ink_left = float(str(found["inkLeft"]))
                desk.check(0 <= ink_left <= 8, f"the triangle's ink starts {ink_left:.1f}px from the left edge")
                desk.check(button["left"] >= 0, f"the triangle's target runs off the screen at {button['left']:.1f}px")
            desk.shot("2-unpacked")
            if not touch:
                page.mouse.move(button["left"] + button["width"] / 2, button["top"] + button["height"] / 2)
                desk.settled()
                hovered = page.evaluate(ROWS_JS, RUNNER)
                assert isinstance(hovered, dict)
                desk.check(float(str(hovered["markOpacity"])) >= 0.99,
                           f"the triangle under the mouse is at opacity {hovered['markOpacity']}")
                desk.check(str(hovered["buttonBg"]) not in ("rgba(0, 0, 0, 0)", "transparent"),
                           "hover draws no rounded rectangle behind the triangle")
                desk.shot("3-unpacked-hover")
            page.click(f"#side-cards > li[data-unpack-thread='{RUNNER}'] .unpack-pack")
            desk.wait_for(f"() => document.querySelectorAll(\"#side-cards > li[data-unpack-thread='{RUNNER}']\").length === 0",
                          "the triangle did not pack the runner")
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return (f"{version} at {width}x{height} {scheme}: " + "; ".join(desk.notes)
                    if desk.notes else f"{version} at {width}x{height} {scheme}: even connectors, dim centred triangle")
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
