#!/usr/bin/env python3
"""The card at the top of a thread on the right, and its line to the thread's root, in real Chromium at 1280x800.

`#289 thread-card-header`. The owner, 2026-10-14: "When I drill into a given thread card to see the
messages inside it, I would like to KEEP that same style ... I would like to see the SAME colorful card
move up to the top of the screen and KEEP its connector linkage over to the main channel message on the
left column. So it just moves up and disgorges its internal messages underneath while keeping the visual
object of the thread card. We can still sneak in an X button somewhere for a mouse-only exit back to
all-threads view." The fake-DOM suite (tests/js/voice_page.test.mjs) pins what the page does; it lays
nothing out and draws nothing, so it cannot say where the card stands, how tall it is whatever it says,
what colour its edge is, or whether opening it moves the list. This does.

It serves the checked-out page and the fake API of tests/two_column_browser.py — a channel taller than
the window with three threads: one summarised, one opened by a long question, one named — from one
loopback origin, at a 1280x800 desk with a mouse, in the dark scheme and the light one:

  two columns over the cards, Main scrolled until the summarised thread's root is on screen -> that
  card's replies button: its thread on the right under its card at the top of the column, saying what
  the card said, at the card's left edge and the card's height, above the list and not in it, "Thread
  list" gone and the X on the card; its edge and its ring in the thread's colour, by the colour Chromium
  computes and by the pixels drawn, and tinted -> one line between the columns, the selection's, leaving
  the root's right edge level with its Thread(N) and arriving with its arrowhead's point on the card's
  left edge, inside the card, in the card's colour -> the thread scrolled: the card does not move ->
  Main scrolled to its foot, the root above it: the line clipped at Main's top, pointing up -> the mouse
  crossing the card to the dock: nothing moves for 2.5 s -> the mouse resting on the card: after 1.5 s
  Main glides, through several scroll positions, until the root is on screen and the line whole -> the X:
  the cards -> the long question's card's replies button: its text cut to three lines, the card exactly
  as tall as the summarised one's -> a click on the text: all of it, over the list, which neither moves
  nor shrinks by a pixel, the heading its own height -> Esc: three lines again, the thread still open ->
  Esc: the cards.

No horizontal overflow and no page error at any step. Pass --screenshots DIR to keep a PNG of each state
for review; --web-root DIR to run the same walk against another copy of the page.
"""

from __future__ import annotations

import argparse
import base64
import os
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import Json, check, handler_for
from thread_card_focus_browser import COLOURED, COLOURED_PIXEL, PIXELS_JS, chroma, open_two_columns
from two_column_browser import REPLICA, RUNNER, THREAD_ROWS, ColumnsApi, Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

# (label, width, height, colour scheme): one desk, both schemes.
SIZES = (("desk-1280-dark", 1280, 800, "dark"), ("desk-1280-light", 1280, 800, "light"))

# The root of the summarised thread's card on Main.
RUNNER_ROOT = "202"

# How long the page waits on the mouse resting on what the glide follows (web/voice.js `COLUMN_GLIDE_MS`).
GLIDE_MS = 1500

# The card at the top of the right column's thread, the line between the columns, and the boxes they
# should meet, as Chromium lays them out and draws them.
HEADER_JS = """(root) => {
    const rgb = (text) => (String(text).match(/[0-9.]+/g) || []).slice(0, 3).map(Number);
    const box = (node) => { const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height}; };
    const card = document.getElementById('side-thread-card');
    const text = card.querySelector('.side-card-text');
    const style = getComputedStyle(card);
    const area = document.getElementById('side-scroll');
    const main = document.getElementById('scroll-area');
    const row = [...document.querySelectorAll('#discord-log > li[data-id]')].find((li) => li.getAttribute('data-id') === root);
    const badge = row ? row.querySelector('.thread-badge') : null;
    const close = document.getElementById('side-close');
    const svg = document.getElementById('column-links');
    const corner = svg.getBoundingClientRect();
    const numbers = (path) => (path.getAttribute('d').match(/-?\\d+(?:\\.\\d+)?/g) || []).map(Number);
    const links = [...svg.querySelectorAll('path.column-link')].map((path) => ({
        thread: path.getAttribute('data-thread'), clip: path.getAttribute('data-link'),
        selected: path.hasAttribute('data-selected'), dimmed: path.hasAttribute('data-dimmed'),
        line: numbers(path).map((value, at) => value + (at % 2 === 0 ? corner.left : corner.top)),
        marks: numbers(path.nextElementSibling).map((value, at) => value + (at % 2 === 0 ? corner.left : corner.top)),
        stroke: rgb(getComputedStyle(path).stroke)}));
    return {
        shown: card.getClientRects().length > 0, thread: card.getAttribute('data-thread'),
        text: text ? text.textContent : '', tip: document.getElementById('side-thread-card-text').getAttribute('title'),
        clamped: Boolean(text) && text.scrollHeight > text.clientHeight + 2,
        lines: text ? Math.round(text.clientHeight / parseFloat(getComputedStyle(text).lineHeight)) : 0,
        expanded: card.hasAttribute('data-expanded'),
        box: box(card), heading: box(document.getElementById('side-heading')),
        list: {...box(area), scrollTop: area.scrollTop, clientHeight: area.clientHeight},
        titleShown: document.getElementById('side-title').getClientRects().length > 0,
        close: {...box(close), shown: close.getClientRects().length > 0},
        edge: rgb(style.borderLeftColor), ring: rgb(style.borderTopColor), tint: rgb(style.backgroundColor),
        panel: rgb(getComputedStyle(document.querySelector('#discord-log > li[data-id]')).backgroundColor),
        root: row ? box(row) : null, badge: badge ? box(badge) : null,
        main: {top: main.getBoundingClientRect().top, bottom: main.getBoundingClientRect().top + main.clientHeight,
               scrollTop: main.scrollTop},
        links, overflow: document.documentElement.scrollWidth - window.innerWidth,
    };
}"""


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--browser-executable",
        default=os.environ.get("VIBE_TALK_CHROMIUM"),
        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)",
    )
    parser.add_argument(
        "--web-root",
        type=Path,
        default=fixture.WEB_ROOT,
        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})",
    )
    parser.add_argument(
        "--screenshots",
        type=Path,
        help="directory to write one PNG per state into (default: none are kept)",
    )
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
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = args.web_root.resolve()
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, scheme in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, scheme))
    for line in report:
        print(line)
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, scheme: str) -> str:
    """One reader at one desk in one scheme, against a fresh fake API and a fresh browser profile."""
    api = ColumnsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-card-header-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height},
                color_scheme="light" if scheme == "light" else "dark",
            )
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, label, width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/voice", wait_until="load")
            desk_walk(desk)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}, {scheme}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


# --- reading what was measured -----------------------------------------------------------------------


def measure(desk: Desk) -> Json:
    found: Json = desk.page.evaluate(HEADER_JS, RUNNER_ROOT)
    return found


def part(found: Json, key: str) -> Json:
    value = found[key]
    assert isinstance(value, dict), f"{key} is {value!r}"
    return value


def number(item: Json, key: str) -> float:
    value = item[key]
    assert isinstance(value, (int, float)), f"{key} is {value!r}"
    return float(value)


def colour(item: Json, key: str) -> list[float]:
    value = item[key]
    assert isinstance(value, list)
    channels = [float(channel) for channel in value if isinstance(channel, (int, float))]
    assert len(channels) == 3, f"{key} is {value!r}"
    return channels


def links_of(found: Json) -> list[Json]:
    value = found["links"]
    assert isinstance(value, list)
    return [link for link in value if isinstance(link, dict)]


def numbers(link: Json, key: str) -> list[float]:
    value = link[key]
    assert isinstance(value, list)
    return [float(each) for each in value]


def pixel(desk: Desk, x: float, y: float) -> list[float]:
    """The colour drawn at one viewport point, from a screenshot: what the reader sees there."""
    png = base64.b64encode(desk.page.screenshot()).decode("ascii")
    found = desk.page.evaluate(PIXELS_JS, {"png": png, "points": [[x, y]]})
    assert isinstance(found, list) and isinstance(found[0], list)
    return [float(channel) for channel in found[0]]


def near(one: list[float], other: list[float], within: float = 2) -> bool:
    return max(abs(a - b) for a, b in zip(one, other)) <= within


def card_box(desk: Desk, thread_id: str) -> dict[str, float]:
    box = desk.page.locator(f"#side-cards > li[data-id='{thread_id}']").bounding_box()
    desk.check(box is not None, f"the card of {thread_id} is not drawn")
    assert box is not None
    return {"x": float(box["x"]), "y": float(box["y"]), "width": float(box["width"]), "height": float(box["height"])}


def main_scroll_samples(desk: Desk, seconds: float) -> list[float]:
    """Main's scroll position at every frame for `seconds`."""
    found = desk.page.evaluate(
        """(ms) => new Promise((done) => {
            const area = document.getElementById('scroll-area');
            const seen = [];
            const end = performance.now() + ms;
            const step = () => { seen.push(area.scrollTop); if (performance.now() < end) requestAnimationFrame(step); else done(seen); };
            requestAnimationFrame(step);
        })""",
        int(seconds * 1000),
    )
    assert isinstance(found, list)
    return [float(value) for value in found]


def open_card(desk: Desk, thread_id: str) -> Json:
    """The thread of card `thread_id` opened by its replies button, and the card at the top once it has settled."""
    desk.page.click(f"#side-cards > li[data-id='{thread_id}'] .side-card-open")
    desk.wait_for(f"() => !document.getElementById('side-pane').hidden"
                  f" && document.querySelectorAll('#side-log > li[data-id]').length === {THREAD_ROWS[thread_id]}"
                  f" && document.getElementById('side-thread-card').getAttribute('data-thread') === '{thread_id}'",
                  f"the replies button of {thread_id} did not open its thread under its card")
    desk.settled()
    desk.settled()
    return measure(desk)


# --- the walk ------------------------------------------------------------------------------------------


def desk_walk(desk: Desk) -> None:
    page = desk.page
    open_two_columns(desk)
    page.evaluate(f"() => document.querySelector(\"#discord-log > li[data-id='{RUNNER_ROOT}']\").scrollIntoView({{block: 'center'}})")
    page.mouse.move(5, desk.height - 5)
    desk.settled()
    desk.settled()
    card = card_box(desk, RUNNER)
    said = str(page.text_content(f"#side-cards > li[data-id='{RUNNER}'] .side-card-text"))
    desk.shot("0-cards")

    # Its replies button: the thread, under its card at the top of the column.
    found = open_card(desk, RUNNER)
    box, heading, listed = part(found, "box"), part(found, "heading"), part(found, "list")
    desk.check(bool(found["shown"]), "no card at the top of the thread")
    desk.check(found["text"] == said and found["tip"] == said, f"the card at the top says {found['text']!r}, its card said {said!r}")
    desk.check(abs(number(box, "left") - card["x"]) <= 1, f"the card at the top stands at {box['left']}, its card at {card['x']}")
    desk.check(abs(number(box, "height") - card["height"]) <= 1,
               f"the card at the top is {box['height']}px tall, its card {card['height']}px")
    desk.check(number(box, "bottom") <= number(listed, "top") + 0.5, f"the card at the top is in the list: {box} over {listed}")
    desk.check(not found["titleShown"], "\"Thread list\" is still drawn over the thread")
    close = part(found, "close")
    desk.check(bool(close["shown"]) and number(close, "left") >= number(box, "left") and number(close, "right") <= number(box, "right")
               and number(close, "top") >= number(box, "top") and number(close, "bottom") <= number(box, "bottom"),
               f"the X is not on the card: {close} in {box}")
    edge, ring, tint, panel = colour(found, "edge"), colour(found, "ring"), colour(found, "tint"), colour(found, "panel")
    desk.check(chroma(edge) >= COLOURED and chroma(ring) >= COLOURED and near(edge, ring),
               f"the card at the top is edged {edge} and ringed {ring}, not in its thread's colour")
    desk.check(not near(tint, panel), f"the card at the top is not tinted: {tint}, a row's surface {panel}")
    drawn = pixel(desk, number(box, "left") + 1.5, (number(box, "top") + number(box, "bottom")) / 2)
    desk.check(chroma(drawn) >= COLOURED_PIXEL, f"the card's edge is drawn {drawn}, not in its colour")
    desk.check(number(found, "overflow") <= 0, "the page scrolls sideways")

    # Its one line: the selection's, from the root's right edge level with its tag into the card's left edge.
    links = links_of(found)
    desk.check(len(links) == 1, f"over a thread the lines are {[link['thread'] for link in links]}, not its one")
    link = links[0]
    root, badge = part(found, "root"), part(found, "badge")
    line, marks = numbers(link, "line"), numbers(link, "marks")
    desk.check(link["thread"] == RUNNER and link["clip"] == "both" and bool(link["selected"]) and not link["dimmed"],
               f"the line is {link['thread']} {link['clip']}, selected {link['selected']}, dimmed {link['dimmed']}")
    desk.check(abs(line[0] - number(root, "right")) <= 1, f"the line leaves at x {line[0]}, not the root's right edge {root['right']}")
    desk.check(number(badge, "top") <= line[1] <= number(badge, "bottom"), f"the line leaves at y {line[1]}, not level with Thread(N) {badge}")
    desk.check(abs(marks[-6] - number(box, "left")) <= 1, f"the arrowhead's point is at x {marks[-6]}, not the card's edge {box['left']}")
    desk.check(number(box, "top") <= line[3] <= number(box, "bottom"), f"the line arrives at y {line[3]}, outside the card {box}")
    desk.check(near(colour(link, "stroke"), edge), f"the line is {colour(link, 'stroke')}, the card's edge {edge}")
    desk.shot("1-thread")
    desk.notes.append(f"the replies button opened the thread under its card, at the card's place and its "
                      f"{number(box, 'height'):.0f}px height, edged {edge}, joined to its root by one line")

    # The thread scrolled: the card does not move.
    page.evaluate("() => { document.getElementById('side-scroll').scrollTop += 150; }")
    desk.settled()
    moved = part(measure(desk), "box")
    desk.check(near([number(moved, key) for key in ("top", "left", "height")], [number(box, key) for key in ("top", "left", "height")], 0.5),
               f"the card moved with the thread: {moved} from {box}")

    # Main at its foot, the root above it: the line clipped at Main's top, pointing up.
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    desk.settled()
    desk.settled()
    above = measure(desk)
    links = links_of(above)
    main = part(above, "main")
    desk.check(len(links) == 1 and links[0]["clip"] == "up" and bool(links[0]["selected"]),
               f"with the root above Main the line is {[(one['thread'], one['clip']) for one in links]}")
    head = numbers(links[0], "marks")
    desk.check(abs(head[1] - (number(main, "top") + 2)) <= 1, f"the arrowhead points at y {head[1]}, not Main's top {main['top']}")
    desk.shot("2-root-above")

    # The mouse crossing the card to the dock: nothing moves.
    middle = ((number(box, "left") + number(box, "right")) / 2, (number(box, "top") + number(box, "bottom")) / 2)
    page.mouse.move(*middle)
    page.mouse.move(middle[0] - 400, desk.height - 20, steps=4)
    still = main_scroll_samples(desk, 2.5)
    desk.check(max(still) - min(still) < 1, f"the mouse crossing the card moved Main: {min(still)}..{max(still)}")

    # The mouse resting on it: Main glides until the root is on screen, and the line is whole.
    page.mouse.move(*middle, steps=3)
    started = time.monotonic()
    seen = main_scroll_samples(desk, (GLIDE_MS + 1700) / 1000)
    distinct = sorted({round(value) for value in seen})
    desk.check(len(distinct) >= 4, f"Main did not glide through several places: {distinct}")
    desk.wait_for("() => document.querySelector('#column-links path.column-link') !== null"
                  " && document.querySelector('#column-links path.column-link').getAttribute('data-link') === 'both'",
                  "the glide did not bring the root on screen with its line whole")
    glided = measure(desk)
    desk.check(number(part(glided, "main"), "top") <= number(part(glided, "root"), "top"),
               "the root is not on screen after the glide")
    desk.shot("3-glided")
    desk.notes.append(f"with the root above Main the line pointed up at its top; the mouse crossing the card moved "
                      f"nothing for 2.5 s, and resting on it glided Main through {len(distinct)} places in "
                      f"{time.monotonic() - started:.1f}s until the line was whole")

    # The X: the cards. The long question's card: three lines, and the card at the top as tall as the other.
    page.click("#side-close")
    desk.wait_for("() => !document.getElementById('side-list').hidden && document.getElementById('side-pane').hidden"
                  " && document.getElementById('side-thread-card').hidden", "the X did not go back to the cards")
    long = open_card(desk, REPLICA)
    long_box = part(long, "box")
    desk.check(bool(long["clamped"]) and number(long, "lines") == 3, f"the long question is not cut to three lines: {long['lines']}")
    desk.check(abs(number(long_box, "height") - number(box, "height")) <= 0.5,
               f"the card at the top is {long_box['height']}px over the long question and {box['height']}px over the other")
    desk.shot("4-long")

    # A click on its text: all of it, over the list, which does not move by a pixel.
    before = part(long, "list")
    page.click("#side-thread-card-text")
    desk.wait_for("() => document.getElementById('side-thread-card').hasAttribute('data-expanded')",
                  "a click on the clamped text did not open the card")
    desk.settled()
    opened = measure(desk)
    after, opened_box = part(opened, "list"), part(opened, "box")
    desk.check(not opened["clamped"] and number(opened, "lines") > 3, f"the opened card does not show its whole text: {opened['lines']} lines")
    for key in ("top", "height", "scrollTop", "clientHeight"):
        desk.check(number(after, key) == number(before, key), f"opening the card moved the list: {key} {before[key]} -> {after[key]}")
    desk.check(abs(number(part(opened, "heading"), "height") - number(part(long, "heading"), "height")) <= 0.5,
               "the heading changed height as the card opened")
    desk.check(number(opened_box, "bottom") > number(after, "top") + 20, f"the opened card is not over the list: {opened_box} over {after}")
    desk.shot("5-expanded")
    page.keyboard.press("Escape")
    desk.wait_for("() => !document.getElementById('side-thread-card').hasAttribute('data-expanded')", "Esc did not close the card")
    desk.settled()
    closed = measure(desk)
    desk.check(bool(closed["clamped"]) and number(closed, "lines") == 3 and page.is_visible("#side-pane"),
               "Esc did more than close the card, or left its text whole")
    page.keyboard.press("Escape")
    desk.wait_for("() => !document.getElementById('side-list').hidden", "the next Esc did not close the thread")
    desk.notes.append(f"the long question's card was three lines and the same height; opened, {number(opened, 'lines'):.0f} "
                      "lines over the list, which moved by no pixel; Esc closed it, and Esc again the thread")


if __name__ == "__main__":
    raise SystemExit(main())
