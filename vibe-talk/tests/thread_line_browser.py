#!/usr/bin/env python3
"""The line from a thread's root to its card, beside the thread's rows and never over them, in real Chromium.

`#385 thread-line-overlap`. The owner, 2026-10-19, of two columns with one thread open on the right:
"With only one thread expanded and one colored arrow showing ... why does it CROSS over my
(now-full-width) text boxes?" The line from the root on Main runs down a lane in the gap between the
columns to the card at the top of the thread; the lane was placed by the card's left edge, 2.5rem in
from the divider, while the thread's rows under the card started at the column's own inset, so the
line ran a few pixels inside every agent box it passed, on its left border. The fake-DOM suite lays
nothing out; this measures where the line and the rows are drawn.

It serves the checked-out page and the fake API of tests/two_column_browser.py — a channel taller
than the window with three threads — with two of the summarised thread's replies the owner's own, from
one loopback origin. At 1000x800 (the narrowest window with two columns), 1280x800 and 1920x1080 with
a mouse, in the dark scheme and the light:

  two columns, the summarised thread's root on screen -> its card's replies button: the thread on the
  right under its card, one line from the root -> the line's lane right of the divider and left of
  every row of the thread by at least 4px, so it crosses none; the rows level with the card; the
  agent's rows and a third party's span the column to its right edge, and the owner's own stop at
  that edge too but start further in, partial width (negative control: `#373 two-column-full-width`
  kept) -> Main scrolled to its foot, the root above it: the line clipped at Main's top, its lane left
  of every row again.

On a 393x852 phone by touch, in both schemes: the same thread across the window from the thread list
-> no line is drawn there at all, and the owner's rows are still partial width beside the agent's.

No horizontal overflow and no page error at any step. Pass --screenshots DIR to keep a PNG of each
state for review; --web-root DIR to run the same walk against another copy of the page.
"""

from __future__ import annotations

import argparse
import os
import re
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import OWNER_ID, SPEAKERS, Json, check, handler_for
from thread_card_focus_browser import open_two_columns
from thread_card_header_browser import RUNNER_ROOT, at_rest, into_list_thread, open_card, phone_thread_list
from two_column_browser import RUNNER, ColumnsApi, Desk
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

# (label, width, height, colour scheme, walk).
SIZES = (
    ("desk-1000-dark", 1000, 800, "dark", "desk"),
    ("desk-1280-dark", 1280, 800, "dark", "desk"),
    ("desk-1280-light", 1280, 800, "light", "desk"),
    ("desk-1920-dark", 1920, 1080, "dark", "desk"),
    ("phone-393-dark", 393, 852, "dark", "phone"),
    ("phone-393-light", 393, 852, "light", "phone"),
)

# The summarised thread's replies that are the owner's own: 213 and 219, at the indices
# tests/two_column_browser.py gives a third party.
OWN_REPLIES = (13, 19)

# The least room between the line and a row's left edge, in px.
CLEAR_PX = 4

# The line, the divider, and every row of the thread list `list`, as Chromium lays them out. The line's
# path is `M x y H x V y H x` whole, or starts `M x y V` clipped at Main's edge: its lane is the x of
# its one vertical run.
LINE_JS = """(list) => {
    const box = (node) => { const r = node.getBoundingClientRect(); return {left: r.left, right: r.right, top: r.top, bottom: r.bottom}; };
    const svg = document.getElementById('column-links');
    const corner = svg.getBoundingClientRect();
    const paths = [...svg.querySelectorAll('path.column-link')].map((path) => ({d: path.getAttribute('d'), clip: path.getAttribute('data-link')}));
    const rows = [...document.querySelectorAll('#' + list + ' > li[data-id]')].map((li) => ({id: li.getAttribute('data-id'),
        who: li.getAttribute('data-who'), ...box(li)}));
    const side = document.getElementById('side-column');
    const card = document.getElementById('side-thread-card');
    return {corner: {left: corner.left, top: corner.top}, paths, rows,
            divider: side.getClientRects().length ? box(side).left : null,
            card: card.getClientRects().length ? box(card) : null,
            overflow: document.documentElement.scrollWidth - window.innerWidth};
}"""


class OwnRepliesApi(ColumnsApi):
    """tests/two_column_browser.py's channel, with two of the summarised thread's replies the owner's."""

    def __init__(self) -> None:
        super().__init__()
        for index in OWN_REPLIES:
            self.messages[index].update(SPEAKERS["me"])

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope), "owner_author_id": OWNER_ID}


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
        for label, width, height, scheme, kind in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, scheme, kind))
    for line in report:
        print(line)
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, scheme: str,
         kind: str) -> str:
    """One reader at one size in one scheme, against a fresh fake API and a fresh browser profile."""
    api = OwnRepliesApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-thread-line-") as profile:
            phone = kind == "phone"
            if phone:
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="light" if scheme == "light" else "dark",
                    device_scale_factor=2.625, is_mobile=True, has_touch=True,
                )
            else:
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="light" if scheme == "light" else "dark",
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


def measure(desk: Desk, list_id: str) -> Json:
    found: Json = desk.page.evaluate(LINE_JS, list_id)
    desk.check(float(str(found["overflow"])) <= 0, "the page scrolls sideways")
    return found


def rows_of(found: Json) -> list[dict[str, object]]:
    rows = found["rows"]
    assert isinstance(rows, list)
    return [row for row in rows if isinstance(row, dict)]


def lane_of(desk: Desk, found: Json) -> tuple[float, str]:
    """The x of the one line's vertical run, in the viewport, and how it is clipped."""
    paths = found["paths"]
    assert isinstance(paths, list)
    desk.check(len(paths) == 1, f"over a thread the lines are {paths}, not its one")
    path = paths[0]
    assert isinstance(path, dict)
    corner = found["corner"]
    assert isinstance(corner, dict)
    d = str(path["d"])
    whole = re.match(r"M (-?[\d.]+) (-?[\d.]+) H (-?[\d.]+) V", d)
    clipped = re.match(r"M (-?[\d.]+) (-?[\d.]+) V", d)
    if whole:
        lane = float(whole.group(3))
    elif clipped:
        lane = float(clipped.group(1))
    else:
        raise AssertionError(f"{desk.label}: the line's path {d!r} has no vertical run")
    return lane + float(str(corner["left"])), str(path["clip"])


def check_clear(desk: Desk, found: Json, lane: float, where: str) -> float:
    """The lane right of the divider and left of every row of the thread; the least room it leaves."""
    rows = rows_of(found)
    desk.check(len(rows) >= 8, f"{where}: the thread draws {len(rows)} rows")
    divider = float(str(found["divider"]))
    desk.check(lane > divider + 2, f"{where}: the line's lane at {lane} is on or left of the divider at {divider}")
    room = min(float(str(row["left"])) - lane for row in rows)
    crossed = [str(row["id"]) for row in rows if float(str(row["left"])) - lane < CLEAR_PX]
    desk.check(not crossed, f"{where}: the line's lane at {lane:.1f} crosses rows {crossed}, "
                            f"which start at {[row['left'] for row in rows if str(row['id']) in crossed]}")
    return room


def check_widths(desk: Desk, found: Json, where: str, third_full: bool) -> str:
    """The agent's rows span the list to one right edge, and a third party's too in two columns (on a
    phone theirs are set in at both edges); the owner's start further in, and end at that edge in two
    columns, inside it on a phone."""
    rows = rows_of(found)
    own = [row for row in rows if row["who"] == "me"]
    full = [row for row in rows if row["who"] == "coder" or (third_full and row["who"] == "human")]
    desk.check(len(own) == len(OWN_REPLIES) and len(full) >= 6, f"{where}: the rows are {[(r['id'], r['who']) for r in rows]}")
    left = min(float(str(row["left"])) for row in full)
    right = max(float(str(row["right"])) for row in full)
    desk.check(all(abs(float(str(row["left"])) - left) <= 1 and abs(float(str(row["right"])) - right) <= 1 for row in full),
               f"{where}: the agent's and a third party's rows are not one width: {[(r['id'], r['left'], r['right']) for r in full]}")
    for row in own:
        # In two columns the owner's row ends at the agent's right edge; on a phone it is set in there too.
        ends = abs(float(str(row["right"])) - right) <= 1 if third_full else float(str(row["right"])) <= right
        desk.check(ends and float(str(row["left"])) >= left + 20,
                   f"{where}: the owner's row {row['id']} spans {row['left']}..{row['right']}, "
                   f"not partial width inside {left}..{right}")
    inset = float(str(own[0]["left"])) - left
    return f"the owner's rows {inset:.0f}px further in than the agent's {right - left:.0f}px"


def desk_walk(desk: Desk) -> None:
    page = desk.page
    open_two_columns(desk)
    page.evaluate(f"() => document.querySelector(\"#discord-log > li[data-id='{RUNNER_ROOT}']\").scrollIntoView({{block: 'center'}})")
    page.mouse.move(5, desk.height - 5)
    desk.settled()
    open_card(desk, RUNNER)
    found = measure(desk, "side-log")
    lane, clip = lane_of(desk, found)
    desk.check(clip == "both", f"with the root on screen the line is clipped {clip}")
    room = check_clear(desk, found, lane, "the root on screen")
    card = found["card"]
    assert isinstance(card, dict)
    rows_left = min(float(str(row["left"])) for row in rows_of(found))
    desk.check(abs(rows_left - float(str(card["left"]))) <= 1,
               f"the thread's rows start at {rows_left}, the card at the top at {card['left']}")
    widths = check_widths(desk, found, "two columns", third_full=True)
    desk.shot("1-thread")

    # Main at its foot, the root above it: the line clipped at Main's top, down the same lane.
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    desk.settled()
    desk.settled()
    found = measure(desk, "side-log")
    clipped, clip = lane_of(desk, found)
    desk.check(clip == "up", f"with the root above Main the line is clipped {clip}")
    check_clear(desk, found, clipped, "the root above Main")
    desk.shot("2-root-above")
    desk.notes.append(f"the line's lane at x {lane:.1f}, {room:.0f}px left of every row of the thread, whole "
                      f"and clipped at Main's top; the rows level with the card; {widths}")


def phone_walk(desk: Desk) -> None:
    phone_thread_list(desk)
    into_list_thread(desk, RUNNER, tap=True)
    at_rest(desk, "thread-card", "pane-discord")
    found = measure(desk, "discord-log")
    desk.check(found["paths"] == [], f"a line is drawn on the phone: {found['paths']}")
    widths = check_widths(desk, found, "on the phone", third_full=False)
    desk.shot("1-thread")
    desk.notes.append(f"the thread across the window draws no line; {widths}")


if __name__ == "__main__":
    raise SystemExit(main())
