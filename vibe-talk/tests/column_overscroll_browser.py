#!/usr/bin/env python3
"""The right column scrolls on past its foot into blank room at a desk, in real Chromium at 1280x800 and 1920x1080.

`#338 column-overscroll`. The owner, 2026-10-15: on a desktop monitor the newest thread in the right column
sits at the very bottom of the screen, where the reader's attention is but which is the least comfortable
place to read. The foot stays where the column opens; the wheel and the page keys go on past it into
blank room, until the last card's top is at half the column's height; scrolling back returns to the foot;
the room counts as the foot for the jump and for following; and nothing moves under a reader in it who
did not move (`#280 scroll-stability`). The fake-DOM suite (tests/js/voice_page.test.mjs) holds the
stylesheet to drawing the room at a desk alone; this measures the scrolling itself, which needs the
engine: the wheel, the page keys, the browser's clamp at the end of a scroller, and frames.

It serves the checked-out page and the fake API of tests/scroll_stability_browser.py, with one more
thread: the channel's newest, of fourteen short replies, all Done, so its card is the last of the cards and
it opens at its foot with its last reply short enough to leave room worth having. At each size, in two
columns with a mouse:

  CARDS. The thread list on the right opens at its foot with no room past it (LANDING) -> the wheel rested
  and turned down once goes on past the foot (WHEEL), and turned until it goes no further leaves the last
  card's top at half the column's height, within 2px -> the mouse resting on that card, which draws it
  whole, moves nothing for 2.5 s sampled at every frame (STILL), and the card stays at the middle -> the
  wheel turned back up past the foot closes the room behind the reader (BACK) -> a fling down, turns of
  the wheel with no rest between them, stops on the foot and opens no room (DETENT).

  THREAD. The short thread, opened by its card's replies button, opens at its foot with no room past it and
  no jump (LANDING) -> a small turn of the wheel takes the reader a little way into the room, and the jump
  stays down (JUMP) -> a reply arriving then is followed as at the foot: the reader at or past the new
  foot, the reply on screen, nothing offered as new (FOLLOW) -> PgDn goes on to the end of the room, the
  last reply's top at the middle (PAGING) -> a short reply arriving there moves nothing for 1.5 s sampled
  at every frame and is drawn on screen in the room (STILL), and PgDn again brings its top to the middle:
  the room was fitted to it (FIT) -> End lands on the foot and closes the room (END).

THE NEGATIVE CONTROLS run the walk at 1280x800 against copies of the page with one part of it taken out,
and each must fail at the check that part answers for: the stylesheet drawing no room (WHEEL); the room not
counted as the foot (JUMP or FOLLOW); a follower in the room taken back to the foot (STILL); and no rest
asked of the wheel before it opens the room (DETENT). That is what proves the walk can see each of them.

No horizontal overflow and no page error at any step. Pass --screenshots DIR to keep a PNG of each stage;
--web-root DIR to run the walk against another copy of the page; --no-controls to skip the controls.
"""

from __future__ import annotations

import argparse
import os
import shutil
import tempfile
import threading
import time
from dataclasses import dataclass
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, check, said
from scroll_stability_browser import SUMMARY, Reader, StabilityApi, handler, thread_record

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

# (label, width, height, whether the short thread has room past its foot): the two desks the issue names. At
# 1280x800 the thread's own message box under its last reply already holds that reply's top above the middle
# of the column at the foot, so there it has none; at 1920x1080 it has.
SIZES = (("desk-1280", 1280, 800, False), ("desk-1920", 1920, 1080, True))
# The desk the negative controls are walked at: the one where the thread has room past its foot.
CONTROL_SIZE = SIZES[1]
# The thread of short replies, the channel's newest.
ROOM = "spaces/A/threads/room"
ROOM_REPLIES = 14
# How far from the middle of the column the last card's top may stop: sub-pixel layout and rounding.
MIDDLE_PX = 2.0
# How long the wheel rests before a turn meant to open the room: past the page's OVERSCROLL_REST_MS.
REST_MS = 350
# How long a turn of the wheel or a page key takes to scroll, smoothly, and come to rest.
SCROLL_MS = 600
# A fling of the wheel: this many turns of 120px, this far apart, well inside the page's rest and long enough
# to go on turning at the foot once the list has come to it from 200px above.
FLING_TURNS = 14
FLING_GAP_MS = 40

# What the right column shows of the room, of `list`'s last drawn row and of its jump.
ROOM_JS = """(list) => {
    const area = document.getElementById('side-scroll');
    const room = document.getElementById('side-overscroll');
    const drawn = room.getClientRects().length > 0;
    const px = drawn ? room.getBoundingClientRect().height : 0;
    const head = area.getBoundingClientRect().top + area.clientTop;
    const rows = [...document.getElementById(list).children]
        .filter((row) => row.getClientRects().length > 0 && row.getBoundingClientRect().height > 0);
    const last = rows.length ? rows[rows.length - 1] : null;
    const jump = document.getElementById('side-jump-newest');
    return {
        top: area.scrollTop, height: area.clientHeight, end: area.scrollHeight - area.clientHeight,
        room: px, drawn, foot: Math.max(0, area.scrollHeight - px - area.clientHeight),
        lastTop: last ? last.getBoundingClientRect().top - head : -1,
        lastBottom: last ? last.getBoundingClientRect().bottom - head : -1,
        lastId: last ? String(last.getAttribute('data-id')) : '',
        jump: !jump.hidden, arrivals: jump.hasAttribute('data-arrivals'),
    };
}"""

# The middle of the empty strip down the right column's left edge, inside its scroller and on no card or row.
STRIP_JS = """() => {
    const box = document.getElementById('side-scroll').getBoundingClientRect();
    return {x: box.left + 5, y: box.top + box.height / 2};
}"""

# The middle of a card of the thread list.
CARD_JS = """(id) => {
    const card = document.querySelector(`#side-cards > li[data-id='${id}'] .side-card`);
    const box = card.getBoundingClientRect();
    return {x: (box.left + box.right) / 2, y: box.top + Math.min(20, box.height / 2)};
}"""


@dataclass
class Room:
    """What `ROOM_JS` measured."""

    top: float
    height: float
    end: float
    room: float
    drawn: bool
    foot: float
    last_top: float
    last_bottom: float
    last_id: str
    jump: bool
    arrivals: bool

    @property
    def middle(self) -> float:
        return self.height / 2

    def __str__(self) -> str:
        return (f"scrolled {self.top:.0f} of {self.end:.0f}, foot at {self.foot:.0f}, room {self.room:.0f}px, "
                f"last row {self.last_id} at {self.last_top:.0f}px of a {self.height:.0f}px column")


class OverscrollApi(StabilityApi):
    """The scroll-stability channel, and one thread more: the newest, of short replies, every one Done."""

    def __init__(self) -> None:
        super().__init__()
        root_index = self.next_index
        root = said(root_index, f"Message {200 + root_index}. A thread of short replies starts here.")
        root["thread"] = thread_record(ROOM, root_index, ROOM_REPLIES, True)
        self.messages.append(root)
        self.dismissed.add(str(root["id"]))
        last = root
        for turn in range(ROOM_REPLIES):
            index = root_index + 1 + turn
            last = said(index, f"Short reply {turn + 1}.", "human" if index % 2 else "coder")
            last["thread"] = thread_record(ROOM, root_index, ROOM_REPLIES, False)
            self.messages.append(last)
            self.dismissed.add(str(last["id"]))
        self.next_index = root_index + 1 + ROOM_REPLIES
        self.threads.append({
            "id": ROOM, "root": root, "title": "Thread", "reply_count": ROOM_REPLIES, "reply_count_exact": True,
            "updated_at": last["timestamp"], "display_name": "short-thread", "summary": SUMMARY,
        })


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
        help="directory to write one PNG per stage into (default: none are kept)",
    )
    parser.add_argument(
        "--no-controls",
        action="store_true",
        help="walk the page at both sizes and skip the negative controls, for the edit-run loop (default: run them)",
    )
    return parser.parse_args()


# Each negative control: its name, the file of the page it changes, the text it takes out and what it puts
# in its place, and the checks (by their tags) one of which must be the one that fails.
CONTROLS: tuple[tuple[str, str, str, str, tuple[str, ...]], ...] = (
    ("no room drawn", "voice.css",
     "  #side-scroll > #side-overscroll {\n    display: block;\n  }\n", "", ("WHEEL",)),
    ("the room not the foot", "voice.js",
     "  return area.scrollHeight - overscrollRoom(area) - area.scrollTop - (area.clientHeight || 0);\n",
     "  return area.scrollHeight - area.scrollTop - (area.clientHeight || 0);\n", ("JUMP", "FOLLOW")),
    ("a follower in the room taken to the foot", "voice.js",
     "    if (overscroll !== null && area.scrollTop >= footTop(area) - 0.5) return;\n", "", ("STILL",)),
    ("no rest asked of the wheel", "voice.js",
     "const OVERSCROLL_REST_MS = 150;\n", "const OVERSCROLL_REST_MS = 0;\n", ("DETENT",)),
)


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
    root = args.web_root.resolve()
    report: list[str] = []
    with sync_playwright() as playwright, tempfile.TemporaryDirectory(prefix="vibe-talk-overscroll-") as copies:
        fixture.WEB_ROOT = root
        for label, width, height, thread_room in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, thread_room))
        for at, (name, file, part, instead, tags) in enumerate(() if args.no_controls else CONTROLS):
            broken = Path(copies) / f"control-{at}"
            shutil.copytree(root, broken)
            text = (broken / file).read_text()
            check(text.count(part) == 1, f"the control '{name}' patches {part!r}, which {root / file} no longer "
                                         "holds exactly once")
            (broken / file).write_text(text.replace(part, instead, 1))
            fixture.WEB_ROOT = broken.resolve()
            failed = ""
            try:
                walk(playwright.chromium, args, f"control-{at}", *CONTROL_SIZE[1:])
            except AssertionError as error:
                failed = str(error)
            finally:
                fixture.WEB_ROOT = root
            check(failed != "", f"the page with {name} passed every check, so this walk cannot see it")
            check(any(f"{tag}:" in failed for tag in tags),
                  f"the page with {name} failed somewhere other than {' or '.join(tags)}: {failed}")
            report.append(f"with {name} the walk failed, as it must: {failed.split(': ', 1)[-1][:160]}")
    for line in report:
        print(line)
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
         thread_room: bool) -> str:
    """One reader at one desk, against a fresh fake API and a fresh browser profile."""
    api = OverscrollApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-overscroll-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark",
            )
            try:
                version = context.browser.version if context.browser else "Chromium"
                page = context.pages[0]
                errors: list[str] = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                reader = Reader(page, api, args.screenshots, label, False)
                page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
                open_columns(reader)
                cards_walk(reader)
                if thread_room:
                    thread_walk(reader)
                else:
                    thread_without_room(reader)
                reader.overflow()
                reader.check(not errors, f"the page threw: {errors}")
            finally:
                context.close()
            return f"{version} at {width}x{height}: " + "; ".join(reader.notes)
    finally:
        api.stopping.set()
        api.timeline_gate.set()
        server.shutdown()
        server.server_close()
        thread.join()


def measure(reader: Reader, list_id: str) -> Room:
    found = reader.page.evaluate(ROOM_JS, list_id)
    assert isinstance(found, dict)
    return Room(float(found["top"]), float(found["height"]), float(found["end"]), float(found["room"]),
                bool(found["drawn"]), float(found["foot"]), float(found["lastTop"]), float(found["lastBottom"]),
                str(found["lastId"]), bool(found["jump"]), bool(found["arrivals"]))


def to_strip(reader: Reader) -> None:
    """The mouse to the empty strip down the right column's left edge: over its scroller, on nothing."""
    point = reader.page.evaluate(STRIP_JS)
    assert isinstance(point, dict)
    reader.page.mouse.move(float(point["x"]), float(point["y"]))


def turn(reader: Reader, delta: float, rest: bool = True) -> None:
    """One turn of the wheel over the right column by `delta` px, after it has rested there, and the scroll it
    started come to rest."""
    if rest:
        reader.page.wait_for_timeout(REST_MS)
    reader.page.mouse.wheel(0, delta)
    reader.quiet(SCROLL_MS)


def key(reader: Reader, name: str) -> None:
    """`name` pressed afresh with the focus on the right column's list, and the scroll it started at rest."""
    reader.page.evaluate("() => document.getElementById('side-scroll').focus({preventScroll: true})")
    reader.page.wait_for_timeout(100)
    reader.page.keyboard.press(name)
    reader.quiet(SCROLL_MS)


def at_middle(reader: Reader, room: Room, tag: str, why: str) -> None:
    reader.check(abs(room.last_top - room.middle) <= MIDDLE_PX,
                 f"{tag}: {why} left the last card's top {room.last_top - room.middle:+.1f}px from the middle: {room}")


def at_foot_closed(reader: Reader, room: Room, tag: str, why: str) -> None:
    reader.check(room.room == 0 and room.end - room.top <= 1,
                 f"{tag}: {why} did not leave the list on its foot with no room past it: {room}")


def open_columns(reader: Reader) -> None:
    """Sign in, open the channel, and press the layout button: Main on the left and the cards on the right."""
    page = reader.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    reader.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                    "the channel did not open")
    page.click("#column-toggle")
    reader.wait_for("() => document.getElementById('side-column').getClientRects().length > 0"
                    " && document.querySelectorAll('#side-cards > li').length > 0",
                    "pressing the layout button did not draw the right column's cards")
    reader.quiet(800)


def cards_walk(reader: Reader) -> None:
    """The thread list on the right: the wheel past its foot to the middle, the room held, back, and a fling."""
    page = reader.page
    reader.wait_for("() => !document.getElementById('side-list').hidden"
                    f" && !!document.querySelector(\"#side-cards > li[data-id='{ROOM}']\")",
                    "the right column is not the cards, with the short thread's among them")
    reader.quiet(500)
    room = measure(reader, "side-cards")
    at_foot_closed(reader, room, "LANDING", "the thread list opening")
    reader.check(room.last_id == ROOM, f"the short thread's card is not the last: {room}")
    reader.check(room.last_top > room.middle + 60,
                 f"the last card is already near the middle at the foot, so this proves nothing: {room}")
    to_strip(reader)
    turn(reader, 120)
    room = measure(reader, "side-cards")
    reader.check(room.top > room.foot + 20 and room.room > 0,
                 f"WHEEL: a turn of the wheel at the foot of the thread list did not go on past it: {room}")
    for _ in range(12):
        was = room.top
        turn(reader, 150)
        room = measure(reader, "side-cards")
        if room.top <= was + 0.5:
            break
    at_middle(reader, room, "WHEEL", "the wheel turned until it went no further")
    reader.shot("1-cards-room")
    reader.notes.append(f"the wheel took the thread list {room.top - room.foot:.0f}px past its foot, the last card's "
                        "top to the middle")

    # The mouse resting on that card draws it whole (`#257 summary-truncation`): it grows, and nothing moves.
    before = reader.first("side-cards")
    reader.watch("side-cards")
    point = page.evaluate(CARD_JS, ROOM)
    assert isinstance(point, dict)
    page.mouse.move(float(point["x"]), float(point["y"]), steps=4)
    try:
        reader.held(before, "the mouse resting on the last card in the room", list_id="side-cards")
    except AssertionError as error:
        raise AssertionError(f"STILL: {error}") from error
    room = measure(reader, "side-cards")
    at_middle(reader, room, "STILL", "the last card drawn whole under the mouse")
    reader.notes.append("the mouse resting on it moved nothing")

    # Back up past the foot: the room goes behind the reader.
    to_strip(reader)
    turn(reader, -(room.top - room.foot + 200))
    room = measure(reader, "side-cards")
    reader.check(room.room == 0 and room.top <= max(0.0, room.foot - 10),
                 f"BACK: the wheel turned back up past the foot left the room open: {room}")
    # A fling down, its turns closer together than the rest the room asks for, and going on after the list has
    # come to its foot: it stops on the foot.
    page.wait_for_timeout(REST_MS)
    for _ in range(FLING_TURNS):
        page.mouse.wheel(0, 120)
        page.wait_for_timeout(FLING_GAP_MS)
    reader.quiet(SCROLL_MS)
    at_foot_closed(reader, measure(reader, "side-cards"), "DETENT", "a fling of the wheel down to the foot")
    reader.notes.append("back up past the foot closed the room, and a fling down stopped on the foot")
    page.mouse.move(5, 5)


def open_short_thread(reader: Reader) -> Room:
    """The short thread opened on the right by its card's replies button: at its foot, with no room past it and
    no jump, as a thread with nothing unread opens."""
    reader.page.locator(f"#side-cards > li[data-id='{ROOM}'] .side-card-open").click()
    reader.wait_for("() => document.querySelectorAll('#side-log > li[data-id]').length >= 8",
                    "the short thread's card did not open it on the right")
    reader.quiet(1500)
    room = measure(reader, "side-log")
    at_foot_closed(reader, room, "LANDING", "the short thread opening")
    reader.check(not room.jump, f"LANDING: the short thread opened with its jump up: {room}")
    return room


def thread_without_room(reader: Reader) -> None:
    """The short thread where its last reply's top is above the middle at the foot already: PgDn and a turn of
    the wheel at the foot open no room, as the room only ever goes as far as the last card's top at the
    middle."""
    page = reader.page
    room = open_short_thread(reader)
    reader.check(room.last_top <= room.middle,
                 f"the last reply is below the middle at the foot, so the room is not the one to check here: {room}")
    key(reader, "PageDown")
    at_foot_closed(reader, measure(reader, "side-log"), "LIMIT", "PgDn at the foot, the last reply above the middle")
    to_strip(reader)
    turn(reader, 120)
    at_foot_closed(reader, measure(reader, "side-log"), "LIMIT", "the wheel at the foot, the last reply above the middle")
    reader.notes.append("the short thread, its last reply above the middle at the foot, opened no room")
    page.keyboard.press("Escape")
    reader.wait_for("() => document.getElementById('side-pane').hidden", "Esc did not go back to the cards")


def thread_walk(reader: Reader) -> None:
    """The short thread on the right: a little way into the room, a reply followed, PgDn to the middle, a reply
    held, the room fitted to it, and End."""
    page, api = reader.page, reader.api
    room = open_short_thread(reader)
    reader.check(room.last_top > room.middle + 90,
                 f"the last reply is already near the middle at the foot, so this proves nothing: {room}")

    # A little way into the room, by a small turn of the wheel: the jump stays down.
    to_strip(reader)
    turn(reader, 30)
    room = measure(reader, "side-log")
    reader.check(room.top > room.foot + 10 and room.room > room.top - room.foot + BOTTOM_SLACK,
                 f"a small turn of the wheel did not take the reader a little way into the room: {room}")
    reader.check(not room.jump, f"JUMP: the jump came up in the room past the foot: {room}")

    # A reply arriving then is followed as at the foot.
    first = api.add("A short reply, arriving.", ROOM)
    api.push("message", {"message": first}, str(first["id"]))
    reader.wait_for(f"() => !!document.querySelector(\"#side-log > li[data-id='{first['id']}']\")",
                    "the live reply never reached the thread on the right")
    reader.quiet(900)
    room = measure(reader, "side-log")
    reader.check(room.top >= room.foot - 1 and not room.jump and not room.arrivals and room.last_id == first["id"]
                 and room.last_bottom <= room.height,
                 f"FOLLOW: a reply arriving with the reader in the room was not followed as at the foot: {room}")
    reader.notes.append("a reply arriving with the reader a little way into the room was followed, the jump down")

    # PgDn to the end of the room.
    key(reader, "PageDown")
    room = measure(reader, "side-log")
    reader.check(room.top > room.foot + 20, f"PAGING: PgDn at the foot of the thread did not go on past it: {room}")
    at_middle(reader, room, "PAGING", "PgDn")
    reader.check(not room.jump, f"JUMP: the jump came up at the end of the room: {room}")
    reader.shot("2-thread-room")

    # A short reply arriving there opens in the room under the reader, and nothing moves.
    before = reader.first("side-log")
    reader.watch("side-log")
    second = api.add("Another short reply.", ROOM)
    api.push("message", {"message": second}, str(second["id"]))
    try:
        reader.held(before, "a reply arriving with the reader at the end of the room", hold_ms=1500, list_id="side-log")
    except AssertionError as error:
        raise AssertionError(f"STILL: {error}") from error
    room = measure(reader, "side-log")
    reader.check(room.last_id == second["id"] and room.last_bottom <= room.height and not room.arrivals,
                 f"STILL: the reply that arrived in the room is not on screen, or was offered as new: {room}")
    key(reader, "PageDown")
    room = measure(reader, "side-log")
    reader.check(room.last_id == second["id"], f"FIT: the reply that arrived is not the last row: {room}")
    at_middle(reader, room, "FIT", "PgDn after a reply arrived in the room")
    reader.shot("3-thread-fitted")
    reader.notes.append("PgDn took the last reply to the middle; a reply arriving there moved nothing, and the room "
                        "was fitted to it")

    key(reader, "End")
    at_foot_closed(reader, measure(reader, "side-log"), "END", "End")
    reader.notes.append("End came back to the foot")
    page.keyboard.press("Escape")
    reader.wait_for("() => document.getElementById('side-pane').hidden", "Esc did not go back to the cards")


# The page's own slack for "at the newest" (BOTTOM_SLACK_PX in web/voice.js): a reader a little way into the
# room must be further than this from its end, or the control that counts only the end as the foot sees
# them there too.
BOTTOM_SLACK = 24


if __name__ == "__main__":
    raise SystemExit(main())
