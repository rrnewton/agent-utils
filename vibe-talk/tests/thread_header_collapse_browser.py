#!/usr/bin/env python3
"""The card at the top of a thread folded to a thin bar and opened again, in real Chromium at 393x852 and 1280x800.

`#388 thread-header-collapse`. The owner, 2026-10-19, of a phone in the thread view: "On a small mobile
screen, I don't like wasting vertical space on the thread header at the top. I like the consistent
visual language for the colored thread cards. But I don't always want to use the space. The same way
tapping a message expands and contracts it I want tapping that header to collapse it to basically a
thin colored bar. Just enough vertical height that you can see the inside color as well as the border
color and so it's wide enough to tap on." The fake-DOM suite (tests/js/voice_page.test.mjs) pins what a
press does to the card and what is kept; it lays nothing out, so it cannot say how tall the bar is, what
colours it shows, how large its press target is, whether the thread under it moves, or where the line
from the root meets it. This does.

It serves the checked-out page and the fake API of tests/two_column_browser.py — a channel taller than
the window with three threads — from one loopback origin.

On a 393x852 phone by touch, in the dark scheme and the light: the thread list -> the summarised
thread across the window under its card -> the thread scrolled part-way up -> a tap on the card's text:
the card is a bar 14px tall from edge to edge, as wide as the card and where it was, its press target
at least 24px tall over it, its fill and its edge the card's own by the colours Chromium computes and
by the pixels drawn, its button saying aria-expanded false; the heading gave up more than 70px and
every row on screen is where it was, to the pixel -> a tap on the bar: the card again, as tall as before,
every row where it was -> the reader at the foot, a tap on the card's age: the bar, the reader still at
the foot -> a tap on the bar: the card, still at the foot -> a link put into the card for the purpose,
tapped: nothing folds (negative control) -> the X: the thread list, and back into the thread, the card
still open (negative control: the X is not the card) -> folded, then the page reloaded: the card
folded on arrival, before a tap, and its bar 14px tall.

At a 1280x800 desk with a mouse, in both schemes: two columns, the summarised thread's root on screen
on Main -> its card's replies button: its thread on the right under its card, one line from the root
-> a click on the card's age: the bar, every row of the thread where it was, the line's arrowhead
meeting the bar's left edge inside the bar, its lane still left of every row of the thread
(`#385 thread-line-overlap`) and right of the divider -> Enter on the card's button: the card again,
aria-expanded true, the line meeting the card's edge -> Space: the bar again -> a click on the bar:
the card.

No horizontal overflow and no page error at any step. Pass --screenshots DIR to keep a PNG of each state
for review; --web-root DIR to run the same walk against another copy of the page.
"""

from __future__ import annotations

import argparse
import base64
import os
import re
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import Json, check, handler_for
from thread_card_focus_browser import PIXELS_JS, open_two_columns
from thread_card_header_browser import RUNNER_ROOT, at_rest, back_in_list, into_list_thread, open_card, phone_thread_list
from two_column_browser import RUNNER, ColumnsApi, Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

# (label, width, height, colour scheme, walk).
SIZES = (
    ("phone-393-dark", 393, 852, "dark", "phone"),
    ("phone-393-light", 393, 852, "light", "phone"),
    ("desk-1280-dark", 1280, 800, "dark", "desk"),
    ("desk-1280-light", 1280, 800, "light", "desk"),
)

# The bar, from edge to edge (web/voice.css), and the least height of its press target.
BAR_PX = 14
TARGET_PX = 24

# How far a row may move on screen as the card folds or opens, in px: sub-pixel layout.
STILL_PX = 1

# The least room between the line and a row's left edge (tests/thread_line_browser.py).
CLEAR_PX = 4

# The card at the top of a thread, its press target, the list it heads and the rows on screen in it, the
# line between the columns, and the divider, as Chromium lays them out.
STATE_JS = """({card, area, list}) => {
    const box = (node) => { const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height}; };
    const node = document.getElementById(card);
    const button = document.getElementById(card + '-text');
    const scroller = document.getElementById(area);
    const style = getComputedStyle(node);
    const view = box(scroller);
    const rows = [...document.querySelectorAll('#' + list + ' > li[data-id]')]
        .map((li) => ({id: li.getAttribute('data-id'), ...box(li)}))
        .filter((row) => row.bottom > view.top && row.top < view.bottom && row.height > 0);
    const svg = document.getElementById('column-links');
    const corner = svg.getBoundingClientRect();
    const numbers = (path) => ((path && path.getAttribute('d')) || '').match(/-?\\d+(?:\\.\\d+)?/g) || [];
    const paths = [...svg.querySelectorAll('path.column-link')].map((path) => ({d: path.getAttribute('d'),
        clip: path.getAttribute('data-link'), selected: path.hasAttribute('data-selected'),
        marks: numbers(path.nextElementSibling).map((value, at) => Number(value) + (at % 2 === 0 ? corner.left : corner.top))}));
    const side = document.getElementById('side-column');
    let stored = null;
    try { stored = localStorage.getItem('vibe-talk.voice.thread-card-folded'); } catch (_error) { stored = 'refused'; }
    return {
        card: box(node), target: box(button), folded: node.hasAttribute('data-folded'),
        expanded: button.getAttribute('aria-expanded'), stored,
        fill: style.backgroundColor, edge: style.borderTopColor, leftEdge: style.borderLeftColor,
        heading: box(node.parentElement),
        area: {...view, scrollTop: scroller.scrollTop, gap: scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight},
        rows, corner: {left: corner.left, top: corner.top}, paths,
        divider: side.getClientRects().length ? box(side).left : null,
        allRows: [...document.querySelectorAll('#' + list + ' > li[data-id]')].map((li) => box(li).left),
        overflow: document.documentElement.scrollWidth - window.innerWidth,
    };
}"""

# A link in the card, which a press on must not fold it: put in for the purpose, its default kept from
# the browser so the press does nothing else.
LINK_JS = """(card) => {
    const link = document.createElement('a');
    link.id = 'fold-probe-link';
    link.href = '#fold-probe';
    link.textContent = 'a link';
    link.addEventListener('click', (event) => event.preventDefault());
    document.getElementById(card + '-age').append(link);
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
        for label, width, height, scheme, kind in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, scheme, kind))
    for line in report:
        print(line)
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, scheme: str,
         kind: str) -> str:
    """One reader at one size in one scheme, against a fresh fake API and a fresh browser profile."""
    api = ColumnsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-card-fold-") as profile:
            phone = kind == "phone"
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="light" if scheme == "light" else "dark",
                **({"device_scale_factor": 2.625, "is_mobile": True, "has_touch": True} if phone else {}),
            )
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, label, width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            if phone:
                phone_walk(desk)
            else:
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


def state(desk: Desk, card: str, area: str, rows: str) -> Json:
    found: Json = desk.page.evaluate(STATE_JS, {"card": card, "area": area, "list": rows})
    desk.check(float(str(found["overflow"])) <= 0, "the page scrolls sideways")
    return found


def box(found: Json, key: str) -> dict[str, float]:
    value = found[key]
    assert isinstance(value, dict), f"{key} is {value!r}"
    return {str(name): float(number) for name, number in value.items()}


def rows_of(found: Json) -> dict[str, float]:
    """The top of every row on screen, by its id."""
    rows = found["rows"]
    assert isinstance(rows, list)
    return {str(row["id"]): float(row["top"]) for row in rows if isinstance(row, dict)}


def pixels(desk: Desk, points: list[tuple[float, float]]) -> list[list[float]]:
    """The colours drawn at viewport points, from one screenshot: what the reader sees there."""
    png = base64.b64encode(desk.page.screenshot()).decode("ascii")
    found = desk.page.evaluate(PIXELS_JS, {"png": png, "points": [list(point) for point in points]})
    assert isinstance(found, list)
    return [[float(channel) for channel in point] for point in found]


def near(one: list[float], other: list[float], within: float = 3) -> bool:
    return max(abs(a - b) for a, b in zip(one, other)) <= within


def card_colours(desk: Desk, found: Json) -> tuple[list[float], list[float]]:
    """The fill drawn just inside the card's top edge, clear of its text, and its left edge drawn."""
    card = box(found, "card")
    fill_y = card["top"] + (card["height"] / 2 if found["folded"] else 3)
    fill, edge = pixels(desk, [(card["left"] + card["width"] / 2, fill_y), (card["left"] + 1, card["top"] + card["height"] / 2)])
    return fill, edge


def check_still(desk: Desk, before: Json, after: Json, where: str) -> int:
    """Every row on screen before is on screen after, at the same height. How many there were."""
    was, now = rows_of(before), rows_of(after)
    shared = [row for row in was if row in now]
    desk.check(len(shared) >= 2, f"{where}: the rows on screen before ({sorted(was)}) and after ({sorted(now)}) share too few")
    moved = {row: round(now[row] - was[row], 1) for row in shared if abs(now[row] - was[row]) > STILL_PX}
    desk.check(not moved, f"{where}: rows moved on screen by {moved}")
    return len(shared)


def check_bar(desk: Desk, open_state: Json, folded: Json, where: str) -> str:
    """The bar: 14px, as wide as the card and where it was, its press target at least 24px over it, its
    fill and edge the card's own — computed and drawn — and its button saying it is folded."""
    card, bar, target = box(open_state, "card"), box(folded, "card"), box(folded, "target")
    desk.check(bool(folded["folded"]) and folded["expanded"] == "false", f"{where}: the card does not say it is folded: "
               f"{folded['folded']}, aria-expanded {folded['expanded']}")
    desk.check(abs(bar["height"] - BAR_PX) <= 0.5, f"{where}: the bar is {bar['height']}px tall, not {BAR_PX}")
    desk.check(abs(bar["left"] - card["left"]) <= 0.5 and abs(bar["width"] - card["width"]) <= 0.5,
               f"{where}: the bar {bar} is not where the card {card} was")
    desk.check(target["height"] >= TARGET_PX and target["top"] <= bar["top"] and target["bottom"] >= bar["bottom"]
               and target["left"] <= bar["left"] + 3 and target["right"] >= bar["right"] - 1,
               f"{where}: the bar's press target {target} is not at least {TARGET_PX}px over the bar {bar}")
    for key in ("fill", "edge", "leftEdge"):
        desk.check(open_state[key] == folded[key], f"{where}: the bar's {key} is {folded[key]}, the card's {open_state[key]}")
    return f"{bar['height']:.0f}px bar, {target['height']:.0f}px press target"


def check_drawn(desk: Desk, card_drawn: tuple[list[float], list[float]], bar_drawn: tuple[list[float], list[float]],
                where: str) -> None:
    """The bar is drawn in the card's fill and its edge, and the two are told apart: both show."""
    (card_fill, card_edge), (bar_fill, bar_edge) = card_drawn, bar_drawn
    desk.check(near(card_fill, bar_fill), f"{where}: the bar is filled {bar_fill}, the card {card_fill}")
    desk.check(near(card_edge, bar_edge), f"{where}: the bar's edge is {bar_edge}, the card's {card_edge}")
    desk.check(not near(bar_fill, bar_edge, 20), f"{where}: the bar's fill {bar_fill} and edge {bar_edge} cannot be told apart")


def tap_box(desk: Desk, part: dict[str, float], dx: float = 0.5, dy: float = 0.5) -> None:
    desk.page.touchscreen.tap(part["left"] + part["width"] * dx, part["top"] + part["height"] * dy)


def selector_box(desk: Desk, selector: str) -> dict[str, float]:
    found = desk.page.locator(selector).bounding_box()
    desk.check(found is not None, f"{selector} is not drawn")
    assert found is not None
    return {"left": found["x"], "top": found["y"], "width": found["width"], "height": found["height"]}


def folded_to(desk: Desk, card: str, folded: bool, why: str) -> None:
    desk.wait_for(f"() => document.getElementById('{card}').hasAttribute('data-folded') === {'true' if folded else 'false'}", why)
    desk.settled()
    desk.settled()


# --- on a phone ----------------------------------------------------------------------------------------


def phone_state(desk: Desk) -> Json:
    return state(desk, "thread-card", "scroll-area", "discord-log")


def phone_walk(desk: Desk) -> None:
    page = desk.page
    phone_thread_list(desk)
    into_list_thread(desk, RUNNER, tap=True)
    at_rest(desk, "thread-card", "pane-discord")
    opened = phone_state(desk)
    desk.check(not opened["folded"] and opened["expanded"] == "true", "the card arrived folded, or not saying it is open")
    desk.shot("1-open")

    # Part-way up the thread: a tap on the card's text folds it, and nothing on screen moves.
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = Math.max(0, a.scrollHeight - a.clientHeight - 400); }")
    desk.settled()
    desk.settled()
    up = phone_state(desk)
    desk.check(float(str(box(up, "area")["gap"])) > 100, f"the thread is too short to scroll part-way up it: {up['area']}")
    card_drawn = card_colours(desk, up)
    tap_box(desk, box(up, "card"), 0.5, 0.25)
    folded_to(desk, "thread-card", True, "a tap on the card's text did not fold it")
    folded = phone_state(desk)
    bar = check_bar(desk, up, folded, "on the phone")
    check_drawn(desk, card_drawn, card_colours(desk, folded), "on the phone")
    gave = box(up, "heading")["height"] - box(folded, "heading")["height"]
    desk.check(gave > 70, f"folding gave the thread only {gave:.0f}px")
    held = check_still(desk, up, folded, "folding part-way up the thread")
    desk.check(folded["stored"] == "on", f"the fold was not kept on the device: {folded['stored']}")
    desk.shot("2-folded")

    # A tap on the bar opens it, as tall as before, and nothing on screen moves.
    tap_box(desk, box(folded, "card"))
    folded_to(desk, "thread-card", False, "a tap on the bar did not open the card")
    reopened = phone_state(desk)
    desk.check(abs(box(reopened, "card")["height"] - box(up, "card")["height"]) <= 0.5 and reopened["expanded"] == "true",
               f"opened again, the card is {box(reopened, 'card')['height']}px, not {box(up, 'card')['height']}px")
    check_still(desk, folded, reopened, "opening part-way up the thread")

    # At the foot: folded by its age and opened by the bar, the reader stays on the newest line.
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    desk.settled()
    desk.settled()
    tap_box(desk, selector_box(desk, "#thread-card-age"))
    folded_to(desk, "thread-card", True, "a tap on the card's age did not fold it")
    at_foot = phone_state(desk)
    desk.check(float(str(box(at_foot, "area")["gap"])) <= 1, f"folding at the foot left the reader {box(at_foot, 'area')['gap']}px above it")
    tap_box(desk, box(at_foot, "card"))
    folded_to(desk, "thread-card", False, "a tap on the bar did not open the card")
    at_foot = phone_state(desk)
    desk.check(float(str(box(at_foot, "area")["gap"])) <= 1, f"opening at the foot left the reader {box(at_foot, 'area')['gap']}px above it")
    desk.shot("3-open-at-foot")

    # Negative controls: a link in the card and its X are not the card.
    page.evaluate(LINK_JS, "thread-card")
    tap_box(desk, selector_box(desk, "#fold-probe-link"))
    desk.settled()
    desk.settled()
    desk.check(not phone_state(desk)["folded"], "a tap on a link in the card folded it")
    page.evaluate("() => document.getElementById('fold-probe-link').remove()")
    tap_box(desk, selector_box(desk, "#thread-back"))
    back_in_list(desk, "the X")
    into_list_thread(desk, RUNNER, tap=True)
    at_rest(desk, "thread-card", "pane-discord")
    desk.check(not phone_state(desk)["folded"], "the X folded the card")

    # Folded, and the page reloaded: folded on arrival.
    tap_box(desk, box(phone_state(desk), "card"), 0.5, 0.25)
    folded_to(desk, "thread-card", True, "a tap on the card's text did not fold it")
    page.reload(wait_until="load")
    desk.wait_for("() => document.getElementById('thread-card').hasAttribute('data-folded')"
                  " && document.getElementById('thread-card-text').getAttribute('aria-expanded') === 'false'",
                  "the reload forgot the fold")
    desk.wait_for("() => !document.getElementById('search-toggle').hidden", "the reload did not sign in again")
    if page.evaluate("() => document.getElementById('thread-heading').hidden"):
        if page.evaluate("() => document.getElementById('screen-main').getAttribute('data-columns') !== 'list'"):
            page.select_option("#thread-select", "threads")
            desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-card]').length === 3", "no thread list after the reload")
        into_list_thread(desk, RUNNER, tap=True)
    at_rest(desk, "thread-card", "pane-discord")
    after = phone_state(desk)
    desk.check(abs(box(after, "card")["height"] - BAR_PX) <= 0.5, f"after the reload the bar is {box(after, 'card')['height']}px")
    desk.shot("4-folded-after-reload")
    desk.notes.append(f"folded by a tap, a {bar}, the heading {gave:.0f}px shorter, its fill and edge the card's, "
                      f"{held} rows on screen unmoved, folding and opening; at the foot it stayed at the foot; a link "
                      "and the X did not fold it; a reload kept it folded")


# --- at a desk -----------------------------------------------------------------------------------------


def desk_state(desk: Desk) -> Json:
    return state(desk, "side-thread-card", "side-scroll", "side-log")


def line_meets(desk: Desk, found: Json, where: str) -> str:
    """The one line: arriving inside the card, its arrowhead's point on the card's left edge, its lane right
    of the divider and left of every row of the thread (`#385 thread-line-overlap`)."""
    paths = found["paths"]
    assert isinstance(paths, list)
    desk.check(len(paths) == 1, f"{where}: over a thread the lines are {paths}, not its one")
    path = paths[0]
    assert isinstance(path, dict)
    desk.check(path["clip"] == "both", f"{where}: with the root on screen the line is clipped {path['clip']}")
    corner = box(found, "corner")
    d = str(path["d"])
    whole = re.match(r"M (-?[\d.]+) (-?[\d.]+) H (-?[\d.]+) V (-?[\d.]+) H (-?[\d.]+)", d)
    desk.check(whole is not None, f"{where}: the line's path {d!r} is not root, lane, card")
    assert whole is not None
    lane = float(whole.group(3)) + corner["left"]
    end_y = float(whole.group(4)) + corner["top"]
    marks = [float(value) for value in path["marks"]] if isinstance(path["marks"], list) else []
    card = box(found, "card")
    desk.check(len(marks) >= 6 and abs(marks[-6] - card["left"]) <= 1 and card["top"] <= marks[-5] <= card["bottom"],
               f"{where}: the arrowhead's point is at {marks[-6:-4]}, not on the card's edge {card['left']} inside {card['top']}..{card['bottom']}")
    desk.check(card["top"] <= end_y <= card["bottom"], f"{where}: the line meets the card at y {end_y}, outside {card['top']}..{card['bottom']}")
    divider = float(str(found["divider"]))
    lefts = [float(value) for value in found["allRows"]] if isinstance(found["allRows"], list) else []
    desk.check(len(lefts) >= 8 and lane > divider + 2 and min(lefts) - lane >= CLEAR_PX,
               f"{where}: the line's lane at {lane:.1f} is not between the divider at {divider} and the rows from {min(lefts or [0])}")
    return f"the line meets it at y {end_y - card['top']:.0f} of {card['height']:.0f}px, its lane {min(lefts) - lane:.0f}px left of the rows"


def desk_walk(desk: Desk) -> None:
    page = desk.page
    open_two_columns(desk)
    page.evaluate(f"() => document.querySelector(\"#discord-log > li[data-id='{RUNNER_ROOT}']\").scrollIntoView({{block: 'center'}})")
    page.mouse.move(5, desk.height - 5)
    desk.settled()
    open_card(desk, RUNNER)
    page.evaluate("() => { const a = document.getElementById('side-scroll'); a.scrollTop = Math.max(0, a.scrollHeight - a.clientHeight - 200); }")
    page.mouse.move(5, desk.height - 5)
    desk.settled()
    desk.settled()
    opened = desk_state(desk)
    desk.check(not opened["folded"] and opened["expanded"] == "true", "the card arrived folded, or not saying it is open")
    card_line = line_meets(desk, opened, "the card open")
    card_drawn = card_colours(desk, opened)
    desk.shot("1-open")

    # A click on the card's age: the bar, the thread unmoved, the line meeting the bar.
    page.click("#side-thread-card-age")
    page.mouse.move(5, desk.height - 5)
    folded_to(desk, "side-thread-card", True, "a click on the card's age did not fold it")
    folded = desk_state(desk)
    bar = check_bar(desk, opened, folded, "at the desk")
    check_drawn(desk, card_drawn, card_colours(desk, folded), "at the desk")
    held = check_still(desk, opened, folded, "folding the right column's card")
    bar_line = line_meets(desk, folded, "the bar")
    desk.shot("2-folded")

    # The keys: Enter on its button opens it, Space folds it again; a click on the bar opens it.
    page.focus("#side-thread-card-text")
    page.keyboard.press("Enter")
    folded_to(desk, "side-thread-card", False, "Enter on the bar's button did not open the card")
    keyed = desk_state(desk)
    desk.check(keyed["expanded"] == "true", f"opened by Enter, the button says aria-expanded {keyed['expanded']}")
    check_still(desk, folded, keyed, "opening the right column's card by Enter")
    line_meets(desk, keyed, "the card opened by Enter")
    page.keyboard.press(" ")
    folded_to(desk, "side-thread-card", True, "Space on the card's button did not fold it")
    page.click("#side-thread-card", position={"x": 200, "y": BAR_PX / 2})
    page.mouse.move(5, desk.height - 5)
    folded_to(desk, "side-thread-card", False, "a click on the bar did not open the card")
    desk.notes.append(f"folded by a click on its age, a {bar}, its fill and edge the card's, {held} rows of the thread "
                      f"unmoved; open, {card_line}; folded, {bar_line}; Enter and Space on its button opened and folded "
                      "it, and a click on the bar opened it")


if __name__ == "__main__":
    raise SystemExit(main())
