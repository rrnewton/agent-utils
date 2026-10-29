#!/usr/bin/env python3
"""The card at the top of a thread — on the right, and across the window — in real Chromium at 1280x800 and 412x915.

`#298 thread-card-header`. The owner, 2026-10-14: "When I drill into a given thread card to see the
messages inside it, I would like to KEEP that same style ... I would like to see the SAME colorful card
move up to the top of the screen and KEEP its connector linkage over to the main channel message on the
left column. So it just moves up and disgorges its internal messages underneath while keeping the visual
object of the thread card. We can still sneak in an X button somewhere for a mouse-only exit back to
all-threads view." The fake-DOM suite (tests/js/voice_page.test.mjs) pins what the page does; it lays
nothing out and draws nothing, so it cannot say where the card stands, how tall its words make it,
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
  the cards -> the long question's card's replies button: its text cut to three lines, the card taller
  than the shorter summarised one's by its lines (`#482 thread-card-fit`) -> Esc: the cards. (A press on the card folds it to a bar since
  `#388 thread-header-collapse`, which tests/thread_header_collapse_browser.py walks.)

The card moves up (part 2 of #298): "I would like to see the SAME colorful card move up to the top of the
screen ... So it just moves up and disgorges its internal messages underneath". Watched frame by frame
from before the press: the card at the top first drawn where the card in the list was, then moving
only upward through at least three places between, to its place, the thread fading in under it and
the line from the root following it; the thread's scroll position the same in every frame once its
rows are drawn, so nothing jumps under the reader (`#280 scroll-stability`).

And the same card across the window, the owner: "When we go to single column thread view Cmd-3 an
drill in I'd like the same thing. One unified thread list view and one unified intra-thread view."
At the desk, after the walk above: real Ctrl+3, the thread list across the window -> the summarised
thread's replies button: its card moves up from the list to the top of the window, across layouts,
and heads the thread there, saying what the card said in its fitted lines, its X on it, no line drawn ->
the thread scrolled: the card does not move (the owner: "the card should NOT scroll away when I
scroll down, but should be the fixed header object") -> the X, Esc and <- each go back to the list.

With reduced motion asked for: the card at the top is at its place in every frame from the first, and
nothing fades, in two columns and from the thread list.

A redraw of Main leaves the keys where the reader is (`#284 redraw-keeps-pane`), at the desk with the
page's clock installed, so the channel's own re-read can be brought forward: two columns over the cards
-> the mouse over Main's newest message, which selects it and takes the focus -> the summarised card's
replies button: its thread on the right -> a new message in the channel and the re-read brought
forward: Main drawn again, the selected message's row replaced -> real Esc: the right column's thread
closes. Then the same with the focus lost first, by a click on the card at the top where it takes no
focus, its age (which folds the card, `#388`): the focus on the body -> the re-read: Main drawn again, the focus still on the body and
not given to Main -> real Esc: the right column's thread closes.

On a 412x915 phone by touch, in the dark scheme and the light: the thread list from the view picker
-> a card's replies button: the card moves up to the top of the screen and heads the thread, at
most two lines of it and fitted below that, its X a thumb's size -> the thread scrolled: the card does not move -> the X: the list.

With the larger type a phone reader's own settings give, stood in for by the root font as
tests/offline_cache_browser.py does (130% and 150% on a 360x800 phone, 150% on 412x915): the card
heading the thread, saying what it says and then what a busier thread would ("3d3h", "123 replies ·
99+ unread") -> its age whole and ending before the count starts, the count before the X, the X a
thumb's size inside the card and the screen, the page no wider than the screen, the card no taller
for the busier count -> the X: the list.

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
from offline_cache_browser import Json, check, handler_for, larger_root_font, said
from offline_cache_browser import TOKEN
from thread_card_focus_browser import COLOURED, COLOURED_PIXEL, PIXELS_JS, chroma, open_two_columns
from two_column_browser import FILLER, REPLICA, RUNNER, THREAD_ROWS, ColumnsApi, Desk
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

# (label, width, height, colour scheme, walk, root font percent): the desk in both schemes, the desk
# asking for reduced motion, the phone in both schemes, and the phones with larger type — where the
# facts line's age, count and X stop fitting on one line: 360px at 130% and 150%, and 412px at 150%,
# the sizes tests/offline_cache_browser.py walks the other phone controls at.
SIZES = (
    ("desk-1280-dark", 1280, 800, "dark", "desk", 100),
    ("desk-1280-light", 1280, 800, "light", "desk", 100),
    ("desk-1280-reduced", 1280, 800, "dark", "reduced", 100),
    ("desk-1280-redraw", 1280, 800, "dark", "redraw", 100),
    ("phone-412-dark", 412, 915, "dark", "phone", 100),
    ("phone-412-light", 412, 915, "light", "phone", 100),
    ("phone-360-font-130", 360, 800, "dark", "type", 130),
    ("phone-360-font-150", 360, 800, "dark", "type", 150),
    ("phone-412-font-150", 412, 915, "dark", "type", 150),
)

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


# `#298 thread-card-header`, part 2. Every frame from now: where the card at the top `card` is drawn,
# whether it is moving, the thread's scroll position in `area` and how many of its rows `list` draws, the
# line between the columns, and how opaque the thread's pane `pane` is. Kept in `window.__flight`, so
# the press can come after this has started. It is `done` once the card has been drawn at rest — not
# moving, the thread no longer fading in — for `REST_FRAMES` frames in a row, counted from when it was
# drawn, not from when the watch began: a press that waits for its button to be ready, on a loaded
# machine, opens the thread later than a fixed window starting now would allow for. `ms` only bounds a
# thread that never opens.
REST_FRAMES = 20

# `#472 gate-flake-hardening`. The card's move, ten times as long as the page's own 220 ms, through the
# stylesheet variable the page reads its length from (`--card-flight`, web/voice.js `cardFlightMs`), so
# its fallback timer stretches with it. At 220 ms a renderer starved for a frame or two on a loaded host
# drew the move in two or three frames, and "only upward through at least three places between" failed
# on a move that was really there ("through only [276]"). Slowed, a stall of 200 ms still leaves most of
# the move to be drawn, while a card that jumps -- no move at all -- is drawn at its place from the first
# frame whatever the length. Not under reduced motion, where the length is 0 and nothing must move.
SLOW_FLIGHT_CSS = "@media (prefers-reduced-motion: no-preference) { #screen-main { --card-flight: 2200ms; } }"
WATCH_JS = """({card, area, list, pane, ms, rest}) => {
    const node = document.getElementById(card), scroller = document.getElementById(area);
    const fading = document.getElementById(pane);
    const seen = [];
    window.__flight = {seen, done: false};
    const end = performance.now() + ms;
    let calm = 0;
    const step = () => {
        const r = node.getBoundingClientRect();
        const path = document.querySelector('#column-links path.column-link');
        const shown = node.getClientRects().length > 0 && !node.closest('[hidden]');
        const flying = node.hasAttribute('data-flying'), arriving = fading.hasAttribute('data-arriving');
        seen.push({shown, top: r.top, left: r.left, flying, scrollTop: scroller.scrollTop,
                   rows: document.querySelectorAll(`#${list} > li[data-id]`).length,
                   link: path ? path.getAttribute('d') : '', opacity: parseFloat(getComputedStyle(fading).opacity)});
        calm = shown && !flying && !arriving ? calm + 1 : 0;
        if (calm < rest && performance.now() < end) requestAnimationFrame(step); else window.__flight.done = true;
    };
    requestAnimationFrame(step);
}"""

# The card at the top of a thread across the window, as Chromium lays it out.
WIDE_JS = """() => {
    const box = (node) => { const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height}; };
    const card = document.getElementById('thread-card');
    const text = card.querySelector('.side-card-text');
    const back = document.getElementById('thread-back');
    const area = document.getElementById('scroll-area');
    return {
        shown: card.getClientRects().length > 0 && !card.closest('[hidden]'), thread: card.getAttribute('data-thread'),
        text: text ? text.textContent : '', clamped: Boolean(text) && text.scrollHeight > text.clientHeight + 2,
        lines: text ? Math.round(text.clientHeight / parseFloat(getComputedStyle(text).lineHeight)) : 0,
        box: box(card), back: {...box(back), shown: back.getClientRects().length > 0, title: back.getAttribute('title')},
        list: {...box(area), scrollTop: area.scrollTop, clientHeight: area.clientHeight, scrollHeight: area.scrollHeight},
        edge: (getComputedStyle(card).borderLeftColor.match(/[0-9.]+/g) || []).slice(0, 3).map(Number),
        paths: document.querySelectorAll('#column-links path').length,
        overflow: document.documentElement.scrollWidth - window.innerWidth,
    };
}"""


# The facts line of the card heading a thread across the window — its age, its count and its X — as
# Chromium lays it out: the box each has and the box its words are drawn in, line by line for the
# count. With `busier`, first saying what a busier thread's card would ("3d3h", "123 replies · 99+
# unread"), in the parts the page draws them in (web/voice.js `cardCountParts`).
FACTS_JS = """(busier) => {
    const box = (r) => ({left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height});
    const card = document.getElementById('thread-card');
    const age = document.getElementById('thread-card-age');
    const count = document.getElementById('thread-card-count');
    if (busier) {
        const part = (name, words) => { const span = document.createElement('span'); span.className = name; span.textContent = words; return span; };
        age.textContent = '3d3h';
        count.replaceChildren(part('side-card-replies', '123 replies'), document.createTextNode(' · '), part('side-card-unread', '99+ unread'));
        count.hidden = false;
    }
    const words = (node) => { const range = document.createRange(); range.selectNodeContents(node);
        return [...range.getClientRects()].filter((r) => r.width > 0).map(box); };
    const ageWords = words(age), countWords = words(count);
    const lineHeight = parseFloat(getComputedStyle(count).lineHeight);
    return {
        age: box(age.getBoundingClientRect()), ageWords: box(ageWords.reduce((a, r) => ({left: Math.min(a.left, r.left),
            right: Math.max(a.right, r.right), top: Math.min(a.top, r.top), bottom: Math.max(a.bottom, r.bottom)}),
            {left: Infinity, right: -Infinity, top: Infinity, bottom: -Infinity})),
        countWords, countLines: countWords.length ? Math.round((Math.max(...countWords.map((r) => r.bottom))
            - Math.min(...countWords.map((r) => r.top))) / lineHeight) : 0,
        said: [age.textContent, count.textContent],
        close: box(document.getElementById('thread-back').getBoundingClientRect()),
        card: box(card.getBoundingClientRect()),
        overflow: document.documentElement.scrollWidth,
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
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, scheme, kind, root_percent in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, scheme, kind, root_percent))
    for line in report:
        print(line)
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, scheme: str,
         kind: str, root_percent: int) -> str:
    """One reader at one size in one scheme, against a fresh fake API and a fresh browser profile."""
    api = ColumnsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-card-header-") as profile:
            if kind in ("phone", "type"):
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="light" if scheme == "light" else "dark",
                    device_scale_factor=2.625, is_mobile=True, has_touch=True,
                )
            else:
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height},
                    color_scheme="light" if scheme == "light" else "dark",
                    reduced_motion="reduce" if kind == "reduced" else "no-preference",
                )
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, label, width, height)
            larger_root_font(page, root_percent)
            if kind == "redraw":
                # The channel re-reads itself every 45 s; the walk brings that forward on this clock.
                page.clock.install()
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            # The page's own length first, so the slowed one cannot stand in for a move the page lost.
            flight = page.evaluate("() => getComputedStyle(document.getElementById('screen-main'))"
                                   ".getPropertyValue('--card-flight').trim()")
            check(flight == ("0ms" if kind == "reduced" else "220ms"),
                  f"{label}: the page says its card moves for {flight!r}")
            page.add_style_tag(content=SLOW_FLIGHT_CSS)
            if kind == "redraw":
                redraw_walk(desk, api)
            elif kind == "desk":
                desk_walk(desk)
                list_walk(desk)
            elif kind == "reduced":
                reduced_walk(desk)
            elif kind == "type":
                type_walk(desk, root_percent)
            else:
                phone_walk(desk)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            said = f"{', reduced motion' if kind == 'reduced' else ''}{f', root font {root_percent}%' if root_percent != 100 else ''}"
            return f"{version} at {width}x{height}, {scheme}{said}: " + "; ".join(desk.notes)
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
    """The thread of card `thread_id` opened by its replies button, and the card at the top once it has
    come to rest at the top of the column."""
    desk.page.click(f"#side-cards > li[data-id='{thread_id}'] .side-card-open")
    desk.wait_for(f"() => !document.getElementById('side-pane').hidden"
                  f" && document.querySelectorAll('#side-log > li[data-id]').length === {THREAD_ROWS[thread_id]}"
                  f" && document.getElementById('side-thread-card').getAttribute('data-thread') === '{thread_id}'",
                  f"the replies button of {thread_id} did not open its thread under its card")
    at_rest(desk, "side-thread-card", "side-pane")
    return measure(desk)


def at_rest(desk: Desk, card: str, pane: str) -> None:
    """Wait for the card at the top to have come to rest and the thread under it to have faded in."""
    desk.wait_for(f"() => !document.getElementById('{card}').hasAttribute('data-flying')"
                  f" && !document.getElementById('{pane}').hasAttribute('data-arriving')",
                  f"#{card} did not come to rest")
    desk.settled()
    desk.settled()


def watch(desk: Desk, card: str, area: str, rows: str, pane: str) -> None:
    """Start watching every frame, before the press that opens the thread (`WATCH_JS`), until the card at
    the top has been at rest for `REST_FRAMES` frames, or for at most 8 s."""
    desk.page.evaluate(WATCH_JS, {"card": card, "area": area, "list": rows, "pane": pane, "ms": 8000, "rest": REST_FRAMES})


def watched(desk: Desk) -> list[Json]:
    """Every frame `watch` saw, once the card at the top has been at rest for long enough."""
    desk.wait_for("() => window.__flight && window.__flight.done", "the frames were not all watched", seconds=12)
    found = desk.page.evaluate("() => window.__flight.seen")
    assert isinstance(found, list)
    return [frame for frame in found if isinstance(frame, dict)]


def check_flight(desk: Desk, frames: list[Json], start: float, rest: float, rows: int, where: str, linked: bool) -> str:
    """The card at the top moved up from `start`, where its card in the list was, to `rest`, its place:
    first drawn within 2px of its card, then only upward through at least three places between, ending at
    its place; the thread fading in under it; the line following it where there is one; and the thread's
    scroll position the same in every frame once its `rows` are drawn. Says what it saw."""
    shown = [frame for frame in frames if frame["shown"]]
    desk.check(len(shown) >= 8, f"{where}: the card at the top was drawn in only {len(shown)} frames")
    tops = [number(frame, "top") for frame in shown]
    desk.check(abs(tops[0] - start) <= 2, f"{where}: the card at the top was first drawn at {tops[0]:.1f}, its card at {start:.1f}")
    desk.check(abs(tops[-1] - rest) <= 0.5, f"{where}: the card at the top ended at {tops[-1]:.1f}, not its place {rest:.1f}")
    desk.check(all(after <= before + 0.5 for before, after in zip(tops, tops[1:])),
               f"{where}: the card at the top did not only move up: {[round(top) for top in tops]}")
    between = sorted({round(top) for top in tops if rest + 1 < top < start - 1})
    desk.check(len(between) >= 3, f"{where}: the card at the top jumped, through only {between}")
    desk.check(any(frame["flying"] for frame in shown), f"{where}: the card was never said to be moving")
    fades = [number(frame, "opacity") for frame in shown]
    desk.check(min(fades[:3]) < 0.9 and fades[-1] == 1, f"{where}: the thread did not fade in under the card: {fades[:6]}…{fades[-1]}")
    if linked:
        lines = {str(frame["link"]) for frame in shown if frame["flying"] and frame["link"]}
        desk.check(len(lines) >= 2, f"{where}: the line did not follow the card up: {len(lines)} lines drawn while it moved")
    places = [number(frame, "scrollTop") for frame in shown if number(frame, "rows") == rows]
    desk.check(len(places) >= 3 and max(places) - min(places) < 1,
               f"{where}: the thread moved under the reader as its card came up: {min(places, default=0)}..{max(places, default=0)}")
    return (f"{where}, the card at the top moved up {start - rest:.0f}px from its card through {len(between)} places in "
            f"{sum(1 for frame in shown if frame['flying'])} frames, the thread fading in and holding its place")


def check_still(desk: Desk, frames: list[Json], rest: float, where: str) -> None:
    """Under reduced motion: the card at the top at its place in every frame it is drawn, nothing fading."""
    shown = [frame for frame in frames if frame["shown"]]
    desk.check(len(shown) >= 8, f"{where}: the card at the top was drawn in only {len(shown)} frames")
    tops = {round(number(frame, "top"), 1) for frame in shown}
    desk.check(tops == {round(rest, 1)}, f"{where}: the card at the top moved under reduced motion: {sorted(tops)}")
    desk.check(not any(frame["flying"] for frame in shown), f"{where}: the card was said to be moving under reduced motion")
    desk.check(all(number(frame, "opacity") == 1 for frame in shown), f"{where}: the thread faded in under reduced motion")


def wide(desk: Desk) -> Json:
    found: Json = desk.page.evaluate(WIDE_JS)
    return found


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

    # Its replies button: the thread, under its card at the top of the column, moving up from the card.
    watch(desk, "side-thread-card", "side-scroll", "side-log", "side-pane")
    found = open_card(desk, RUNNER)
    box, heading, listed = part(found, "box"), part(found, "heading"), part(found, "list")
    desk.notes.append(check_flight(desk, watched(desk), card["y"], number(box, "top"), THREAD_ROWS[RUNNER],
                                   "two columns", linked=True))
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

    # The X: the cards. The long question's card: three lines, and taller than the shorter one's
    # card by the lines it has over it (`#482 thread-card-fit`: fitted, no longer one fixed height).
    page.click("#side-close")
    desk.wait_for("() => !document.getElementById('side-list').hidden && document.getElementById('side-pane').hidden"
                  " && document.getElementById('side-thread-card').hidden", "the X did not go back to the cards")
    long = open_card(desk, REPLICA)
    long_box = part(long, "box")
    # Cut by the clamp, or split for the card (`#522 thread-card-max-lines`), its words ending "…".
    desk.check((bool(long["clamped"]) or str(long["text"]).endswith("…")) and number(long, "lines") == 3,
               f"the long question is not cut to three lines: {long['lines']}")
    desk.check(number(found, "lines") < 3 and number(long_box, "height") > number(box, "height") + 10,
               f"the card at the top is {long_box['height']}px over the long question and {box['height']}px "
               f"over the {found['lines']}-line one: fitted cards should differ by their lines")
    desk.shot("4-long")

    page.keyboard.press("Escape")
    desk.wait_for("() => !document.getElementById('side-list').hidden", "Esc did not close the thread")
    desk.notes.append("the long question's card was three lines and taller than the shorter one's by its lines; Esc closed the thread")

    # Thread(N) on the root in Main, the thread's card on screen on the right: the card moves up from
    # there too, as it does from its replies button.
    page.evaluate(f"() => document.querySelector(\"#discord-log > li[data-id='{RUNNER_ROOT}']\").scrollIntoView({{block: 'center'}})")
    page.evaluate(f"() => document.querySelector(\"#side-cards > li[data-id='{RUNNER}']\").scrollIntoView({{block: 'center'}})")
    page.mouse.move(5, desk.height - 5)
    desk.settled()
    desk.settled()
    card = card_box(desk, RUNNER)
    watch(desk, "side-thread-card", "side-scroll", "side-log", "side-pane")
    page.click(f"#discord-log > li[data-id='{RUNNER_ROOT}'] .thread-badge")
    desk.wait_for(f"() => !document.getElementById('side-pane').hidden"
                  f" && document.querySelectorAll('#side-log > li[data-id]').length === {THREAD_ROWS[RUNNER]}"
                  f" && document.getElementById('side-thread-card').getAttribute('data-thread') === '{RUNNER}'",
                  "Thread(N) on the root did not open its thread on the right under its card")
    at_rest(desk, "side-thread-card", "side-pane")
    tagged = part(measure(desk), "box")
    desk.notes.append(check_flight(desk, watched(desk), card["y"], number(tagged, "top"), THREAD_ROWS[RUNNER],
                                   "Thread(N) on Main", linked=True))



# --- the same card across the window: the thread list, at a desk -----------------------------------------


def list_card(desk: Desk, thread_id: str) -> tuple[dict[str, float], str]:
    """Card `thread_id` of the thread list, its box and what it says.

    The cards are drawn first from the channel's roots, saying each root's
    own words; the threads read then folds in the summaries and repaints a
    summarised card with its name and summary (`sideCardText`).  Read the
    card only once that read has landed for this thread, or on a loaded
    machine the words read here are the root's while the heading card,
    painted after the fold, says the summary.  `#389 gate-flakes`.
    """
    desk.wait_for(
        "() => { const held = channelCanon.threads.find((entry) => String(entry.id) === '"
        + thread_id
        + "'); if (!held) return false;"
        + " const card = document.querySelector(\"#side-cards > li[data-id='"
        + thread_id
        + "'] .side-card-text\"); if (!card) return false;"
        + " return !held.display_name || !held.summary"
        + " || card.textContent.trim() === `${held.display_name}: ${held.summary}`.trim(); }",
        f"the threads read never brought {thread_id}'s summary to its card",
        seconds=60,
    )
    desk.settled()
    box = card_box(desk, thread_id)
    said = str(desk.page.text_content(f"#side-cards > li[data-id='{thread_id}'] .side-card-text"))
    return box, said


def into_list_thread(desk: Desk, thread_id: str, tap: bool = False) -> None:
    """Card `thread_id`'s replies button, in the thread list: its thread across the window."""
    selector = f"#side-cards > li[data-id='{thread_id}'] .side-card-open"
    if tap:
        box = desk.page.locator(selector).bounding_box()
        desk.check(box is not None, f"the replies button of {thread_id} is not drawn")
        assert box is not None
        desk.page.touchscreen.tap(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
    else:
        desk.page.click(selector)
    desk.wait_for("() => !document.getElementById('thread-heading').hidden"
                  " && !document.getElementById('screen-main').hasAttribute('data-columns')"
                  f" && document.querySelectorAll('#discord-log > li[data-id]').length === {THREAD_ROWS[thread_id]}"
                  f" && document.getElementById('thread-card').getAttribute('data-thread') === '{thread_id}'",
                  f"the replies button of {thread_id} did not open its thread across the window under its card")


def back_in_list(desk: Desk, how: str) -> None:
    desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
                  " && !document.getElementById('side-list').hidden && document.getElementById('thread-heading').hidden",
                  f"{how} did not go back to the thread list")
    desk.settled()


def check_wide(desk: Desk, found: Json, said: str, lines: int, where: str) -> None:
    """The card heading a thread across the window: the list card's words in at most `lines` of them
    (fitted below that most, `#482 thread-card-fit`), its X on it, above the thread and not in it,
    in the thread's colour, no line drawn, and the page no wider."""
    box, back, listed = part(found, "box"), part(found, "back"), part(found, "list")
    desk.check(bool(found["shown"]), f"{where}: no card heads the thread")
    desk.check(found["text"] == said, f"{where}: the card at the top says {found['text']!r}, its card said {said!r}")
    desk.check(1 <= number(found, "lines") <= lines, f"{where}: the card at the top is {found['lines']} lines, not within {lines}")
    desk.check(not found["clamped"], f"{where}: the summarised card at the top is cut, not fitted whole")
    desk.check(bool(back["shown"]) and number(back, "left") >= number(box, "left") and number(back, "right") <= number(box, "right")
               and number(back, "top") >= number(box, "top") and number(back, "bottom") <= number(box, "bottom"),
               f"{where}: the X is not on the card: {back} in {box}")
    desk.check(back["title"] == "Back to the thread list", f"{where}: the X says {back['title']!r}")
    desk.check(number(box, "bottom") <= number(listed, "top") + 0.5, f"{where}: the card is in the list: {box} over {listed}")
    desk.check(chroma(colour(found, "edge")) >= COLOURED, f"{where}: the card's edge is {colour(found, 'edge')}, not its colour")
    desk.check(number(found, "paths") == 0, f"{where}: a line is drawn across the window, with no Main to draw it to")
    desk.check(number(found, "overflow") <= 0, f"{where}: the page scrolls sideways")


def check_fixed(desk: Desk, found: Json, where: str) -> None:
    """The owner: "the card should NOT scroll away when I scroll down, but should be the fixed header
    object." The thread scrolled both ways: the card stays where it stands."""
    box = part(found, "box")
    for by in (-200, 200):
        desk.page.evaluate(f"() => {{ document.getElementById('scroll-area').scrollTop += {by}; }}")
        desk.settled()
        now = wide(desk)
        moved = part(now, "box")
        desk.check(abs(number(part(now, "list"), "scrollTop") - number(part(found, "list"), "scrollTop")) > 20 or by > 0,
                   f"{where}: the thread did not scroll, so this proves nothing")
        desk.check(all(abs(number(moved, key) - number(box, key)) <= 0.5 for key in ("top", "left", "height")),
                   f"{where}: the card moved with the thread: {moved} from {box}")


def list_walk(desk: Desk) -> None:
    page = desk.page
    # Real Ctrl+3: the thread list, across the window.
    page.focus("#side-scroll")
    page.keyboard.press("Control+3")
    desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
                  " && document.querySelectorAll('#side-cards > li[data-card]').length === 3",
                  "Ctrl+3 did not show the thread list")
    page.mouse.move(5, desk.height - 5)
    desk.settled()
    desk.settled()
    card, said = list_card(desk, RUNNER)
    desk.shot("6-thread-list")

    # Its replies button: the card moves up to the top of the window and heads the thread there.
    watch(desk, "thread-card", "scroll-area", "discord-log", "pane-discord")
    into_list_thread(desk, RUNNER)
    at_rest(desk, "thread-card", "pane-discord")
    found = wide(desk)
    box = part(found, "box")
    desk.notes.append(check_flight(desk, watched(desk), card["y"], number(box, "top"), THREAD_ROWS[RUNNER],
                                   "from the thread list", linked=False))
    check_wide(desk, found, said, 3, "a thread from the thread list")
    desk.check(abs(number(box, "left") - card["x"]) <= 20, f"the card at the top stands at {box['left']}, its card at {card['x']}")
    desk.shot("7-list-thread")
    check_fixed(desk, found, "a thread from the thread list")

    # The X, Esc and <- each go back to the list.
    page.click("#thread-back")
    back_in_list(desk, "the X")
    into_list_thread(desk, RUNNER)
    at_rest(desk, "thread-card", "pane-discord")
    page.keyboard.press("Escape")
    back_in_list(desk, "Esc")
    into_list_thread(desk, RUNNER)
    at_rest(desk, "thread-card", "pane-discord")
    page.keyboard.press("ArrowLeft")
    back_in_list(desk, "<-")
    desk.notes.append(f"Ctrl+3 then a replies button: the thread across the window under its card, {number(box, 'height'):.0f}px "
                      f"and {number(found, 'lines'):.0f} fitted lines, fixed while the thread scrolled; the X, Esc and <- each went back to the list")


# --- Esc after two columns went, the card at the top left open ----------------------------------------------


# --- a redraw of Main while the reader is in the right column (#284) -------------------------------------

# How long the page waits between its re-reads of the channel (web/voice.js `DISCORD_POLL_MS`), and a little.
POLL_PAST_MS = 46_000

# What holds the focus: "body", a row by its list and id, or the element by its id.
FOCUS_JS = """() => {
    const held = document.activeElement;
    if (!held || held === document.body) return 'body';
    const row = held.closest('li[data-id]');
    return row ? `${row.parentElement.id}:${row.getAttribute('data-id')}` : `#${held.id || held.tagName}`;
}"""


def redraw_walk(desk: Desk, api: ColumnsApi) -> None:
    """`#284 redraw-keeps-pane`: the keys stay in the right column through a redraw of Main, with the
    focus where the replies button left it and with the focus lost to the body."""
    page = desk.page
    open_two_columns(desk)
    for index, lose in enumerate((False, True)):
        where = "with the focus lost to the body" if lose else "after the replies button"
        newest = str(223 + index)
        row = f"#discord-log > li[data-id='{newest}']"
        page.hover(row)
        desk.wait_for(f'() => document.activeElement === document.querySelector("{row}")',
                      f"{where}: the mouse over Main's newest message did not select it")
        open_card(desk, RUNNER)
        if lose:
            # A press on the card folds it to a bar (`#388 thread-header-collapse`) and takes no focus.
            page.click("#side-thread-card-age")
            desk.settled()
            desk.check(page.evaluate(FOCUS_JS) == "body",
                       f"{where}: a click on the card's age left the focus on {page.evaluate(FOCUS_JS)}, not the body")
        before = str(page.evaluate(FOCUS_JS))
        page.evaluate(f'() => {{ document.querySelector("{row}").dataset.beforeRedraw = "yes"; }}')
        with api.lock:
            api.messages.append(said(24 + index, f"Message {224 + index}, new in the channel. {FILLER}", "human"))
        page.clock.fast_forward(POLL_PAST_MS)
        desk.wait_for(f"() => Boolean(document.querySelector(\"#discord-log > li[data-id='{224 + index}']\"))",
                      f"{where}: the channel's re-read did not draw the new message in Main")
        desk.settled()
        desk.check(bool(page.evaluate(f'() => document.querySelector("{row}").dataset.beforeRedraw !== "yes"')),
                   f"{where}: Main's selected row was not drawn again, so this proves nothing")
        after = str(page.evaluate(FOCUS_JS))
        desk.check(not after.startswith("discord-log:"), f"{where}: a redraw of Main gave the focus to {after}, from {before}")
        page.keyboard.press("Escape")
        desk.wait_for("() => !document.getElementById('side-list').hidden && document.getElementById('side-pane').hidden",
                      f"{where}: after a redraw of Main, Esc did not close the right column's thread (the focus on {after})",
                      seconds=3)
        desk.notes.append(f"{where}, a redraw of Main left the focus on {after} and Esc closed the right column's thread")


# --- reduced motion ----------------------------------------------------------------------------------------


def reduced_walk(desk: Desk) -> None:
    page = desk.page
    open_two_columns(desk)
    page.mouse.move(5, desk.height - 5)
    desk.settled()
    watch(desk, "side-thread-card", "side-scroll", "side-log", "side-pane")
    found = open_card(desk, RUNNER)
    check_still(desk, watched(desk), number(part(found, "box"), "top"), "two columns")
    page.click("#side-close")
    desk.wait_for("() => !document.getElementById('side-list').hidden", "the X did not go back to the cards")
    page.focus("#side-scroll")
    page.keyboard.press("Control+3")
    desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
                  " && document.querySelectorAll('#side-cards > li[data-card]').length === 3",
                  "Ctrl+3 did not show the thread list")
    desk.settled()
    watch(desk, "thread-card", "scroll-area", "discord-log", "pane-discord")
    into_list_thread(desk, RUNNER)
    at_rest(desk, "thread-card", "pane-discord")
    check_still(desk, watched(desk), number(part(wide(desk), "box"), "top"), "from the thread list")
    desk.shot("8-reduced-list-thread")
    desk.notes.append("asked for reduced motion, the card at the top was at its place in every frame from the first, "
                      "and nothing faded, in two columns and from the thread list")


# --- on a phone --------------------------------------------------------------------------------------------


def phone_thread_list(desk: Desk) -> None:
    """Signed in on a phone, the channel in All, then the thread list from the view picker."""
    page = desk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24", "the channel did not open in All")
    page.select_option("#thread-select", "threads")
    desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
                  " && document.querySelectorAll('#side-cards > li[data-card]').length === 3",
                  "the view picker did not show the thread list")
    desk.settled()
    desk.settled()


def phone_walk(desk: Desk) -> None:
    page = desk.page
    phone_thread_list(desk)
    card, said = list_card(desk, RUNNER)
    desk.shot("1-thread-list")

    # A tap on its replies button: the card moves up to the top of the screen.
    watch(desk, "thread-card", "scroll-area", "discord-log", "pane-discord")
    into_list_thread(desk, RUNNER, tap=True)
    at_rest(desk, "thread-card", "pane-discord")
    found = wide(desk)
    box, back = part(found, "box"), part(found, "back")
    desk.notes.append(check_flight(desk, watched(desk), card["y"], number(box, "top"), THREAD_ROWS[RUNNER],
                                   "on the phone", linked=False))
    check_wide(desk, found, said, 2, "on the phone")
    desk.check(number(back, "width") >= 44 and number(back, "height") >= 44, f"the X is not a thumb's size: {back}")
    desk.shot("2-thread")
    check_fixed(desk, found, "on the phone")
    desk.shot("3-thread-scrolled")

    # The X: the list.
    page.touchscreen.tap(number(back, "left") + number(back, "width") / 2, number(back, "top") + number(back, "height") / 2)
    back_in_list(desk, "the X")
    desk.notes.append(f"a tap on a replies button in the thread list moved its card up {card['y'] - number(box, 'top'):.0f}px "
                      f"to head the thread, {number(found, 'lines'):.0f} fitted lines and {number(box, 'height'):.0f}px, its X {number(back, 'width'):.0f}px, "
                      "fixed while the thread scrolled; the X went back to the list")



# --- on a phone, with larger type -----------------------------------------------------------------------


def facts(desk: Desk, busier: bool = False) -> Json:
    found: Json = desk.page.evaluate(FACTS_JS, busier)
    return found


def check_facts(desk: Desk, found: Json, where: str) -> None:
    """The facts line legible: the age whole, inside its own box, and ending before the count starts;
    the count's words ending before the X, on at most two lines; the X a thumb's size, inside the card
    and the screen; the card inside the screen, and the page no wider than it."""
    age, words, close, card = part(found, "age"), part(found, "ageWords"), part(found, "close"), part(found, "card")
    lines, said = found["countWords"], found["said"]
    assert isinstance(lines, list) and isinstance(said, list)
    age_said, count_said = str(said[0]), str(said[1])
    counted = [line for line in lines if isinstance(line, dict)]
    desk.check(bool(counted), f"{where}: the count says nothing: {said}")
    desk.check(number(words, "right") <= number(age, "right") + 0.5,
               f"{where}: the age {age_said!r} is drawn past its own box, {words['right']:.1f} > {age['right']:.1f}")
    start = min(number(line, "left") for line in counted)
    end = max(number(line, "right") for line in counted)
    desk.check(number(words, "right") + 4 <= start,
               f"{where}: the age {age_said!r} ends at {words['right']:.1f}, and the count {count_said!r} starts at {start:.1f}")
    desk.check(end <= number(close, "left"), f"{where}: the count ends at {end:.1f}, over the X at {close['left']:.1f}")
    desk.check(number(found, "countLines") <= 2, f"{where}: the count {count_said!r} takes {found['countLines']} lines")
    desk.check(number(close, "width") >= 44 and number(close, "height") >= 44, f"{where}: the X is not a thumb's size: {close}")
    desk.check(number(close, "left") >= number(card, "left") and number(close, "right") <= number(card, "right")
               and number(close, "top") >= number(card, "top") and number(close, "bottom") <= number(card, "bottom"),
               f"{where}: the X is not inside the card: {close} in {card}")
    desk.check(number(card, "left") >= 0 and number(card, "right") <= desk.width and number(close, "right") <= desk.width,
               f"{where}: the card or its X is past the edge of the {desk.width}px screen: {card}, the X {close}")
    desk.check(number(found, "overflow") <= desk.width, f"{where}: the page is {found['overflow']}px wide on a {desk.width}px screen")


def type_walk(desk: Desk, root_percent: int) -> None:
    """`#298 thread-card-header` with larger type: the card heading a thread across the window, from the
    thread list, saying what it says and then what a busier thread would, each legible and the card
    no taller for the busier count; then its X, by touch, back to the list."""
    phone_thread_list(desk)
    into_list_thread(desk, RUNNER, tap=True)
    at_rest(desk, "thread-card", "pane-discord")
    plain = facts(desk)
    check_facts(desk, plain, f"at {root_percent}% type, its own words")
    desk.shot("1-thread")
    busy = facts(desk, busier=True)
    check_facts(desk, busy, f"at {root_percent}% type, a busier thread's words")
    plain_card, busy_card = part(plain, "card"), part(busy, "card")
    desk.check(abs(number(busy_card, "height") - number(plain_card, "height")) <= 0.5,
               f"at {root_percent}% type the card is {plain_card['height']}px tall saying {plain['said']} and "
               f"{busy_card['height']}px saying {busy['said']}")
    desk.shot("2-busier")
    close = part(busy, "close")
    desk.page.touchscreen.tap(number(close, "left") + number(close, "width") / 2, number(close, "top") + number(close, "height") / 2)
    back_in_list(desk, "the X")
    desk.notes.append(f"at {root_percent}% type the card heading the thread said {plain['said']}, then {busy['said']} on "
                      f"{number(busy, 'countLines'):.0f} lines of count, its age whole and clear of the count, its "
                      f"{number(close, 'width'):.0f}px X inside the card and the screen, {number(plain_card, 'height'):.0f}px "
                      "tall either way; the X went back to the list")


if __name__ == "__main__":
    raise SystemExit(main())
