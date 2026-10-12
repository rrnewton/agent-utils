#!/usr/bin/env python3
"""Read a channel in two columns in real Chromium, at 1440x900 and 1280x800.

`#229 desktop-two-column`. The owner, 2026-10-09: other chat clients' desktop apps give him a
two-column view of a channel and he uses it a lot, so on a desk the dock gets a layout button: Main in
the left column and a thread in the right, the view picker greyed out, and with no thread selected a
list of the threads, a few lines each; the way into a thread on a message selects it on the right, and
an X goes back to the list. The fake-DOM suite (tests/js/voice_page.test.mjs) pins what the page does;
it lays nothing out, so it cannot say where the columns are. This does: the layout the stylesheet
alone decides, measured in a browser engine.

It serves the checked-out page and the small fake API of tests/offline_cache_browser.py from one
loopback origin, over a channel taller than the window with three threads, one summarised. At each
desk size, with a mouse:

  sign in and open the channel, in All -> the layout button is drawn in the dock and pressed: two
  columns side by side, inside the window, neither overlapping the other, each at least 440px wide;
  the view picker greyed out and the reading-width handle gone -> the greyed picker still takes the
  focus, and a real down arrow and a real click on it choose nothing and say why (`#231
  two-column-polish`); the right column is one region, named "Threads" -> the right column lists the
  threads as cards, newest activity at the foot (`#261 two-column-connectors`), every card the same
  height, the summarised one saying its name and summary, the long first message cut to three lines,
  and shown whole on the card under the mouse (`#257 summary-truncation`) -> a card selects its
  thread: its rows in the right column under a heading naming it, its region "Thread: " and that
  name, with the X, and no line saying it is stale -> each column scrolls on its own, under the
  wheel over it -> the X goes back to the cards, and a Main message's Thread(N) selects its thread on
  the right -> the window narrowed to 900px: one column, as the channel was in one, the button not
  drawn; widened again: two columns, the same thread selected -> the keys, pressed for real: ] gives
  them to the right column, Esc there is its X, j rings the first card, Enter selects its thread and
  [ gives them back to Main -> at a phone's width the button is not drawn -> back at the desk's width
  over the cards, the links between the columns (`#261 two-column-connectors`): with Main scrolled to
  its foot, its head and two places between, the links drawn are exactly the pairs whose root's tag
  and card both show, each leaving its root's right edge level with its tag, arriving at its card's
  left edge with its arrowhead's point on that edge, its lane in the gap between them, and in the
  colour Chromium draws the card's edge and the tag in -> the mouse on a card whose root is above
  Main's top: its link drawn, pointing up at the columns' top edge, and after 1.5 s Main glides —
  through many scroll positions, not one jump — until the root is level with the card and the link
  whole -> a card whose root is below, and the reader scrolling Main a little before 1.5 s: nothing
  glides -> with reduced motion emulated, the same glide is one step. No horizontal overflow and no
  page error at any step.

Pass --screenshots DIR to keep a PNG of each state (the list, a selected thread, the links, the
selection's link off screen and after its glide, at each size) for review; --web-root DIR to run the
same walk against another copy of the page.
"""

from __future__ import annotations

import argparse
import json
import os
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, FakeApi, Json, check, handler_for, said

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

# The two desks the issue names: (label, width, height).
DESKS = (("desk-1440", 1440, 900), ("laptop-1280", 1280, 800))
# The narrowest a column may be drawn in two columns: about 45 characters and the row's insets.
COLUMN_MIN_PX = 440

FILLER = ("The overnight run is still going; the integration shard was retried and passed, and the artifact "
          "upload waited on the cache for most of an hour. ")
LONG_FIRST = ("Before we cut the release, can somebody check whether the migration on the reporting replica "
              "has finished, whether the dashboards picked up the new partitions, and whether the alert we "
              "silenced on Friday is back on? I would rather not find out on Monday that one of them was "
              "still pending, because last time that cost us a morning of reruns and an apology.")

RUNNER = "spaces/A/threads/runner"
REPLICA = "spaces/A/threads/replica"
CANARY = "spaces/A/threads/canary"
# How many rows each thread draws on the right: its root and its replies.
THREAD_ROWS = {RUNNER: 9, REPLICA: 4, CANARY: 3}


def thread_record(thread_id: str, root: int, replies: int, is_root: bool = True) -> Json:
    return {"id": thread_id, "root_message_id": str(200 + root), "is_root": is_root,
            "reply_count": replies, "reply_count_exact": True}


class ColumnsApi(FakeApi):
    """A channel taller than a desk's window, with three threads: one the summariser has named and
    summed up, one opened by a long question, and one named but not yet summarised."""

    def __init__(self) -> None:
        super().__init__()
        plan = {2: (RUNNER, 8), 9: (REPLICA, 3), 15: (CANARY, 2)}
        replies = {RUNNER: [4, 6, 8, 11, 13, 17, 19, 21], REPLICA: [10, 12, 14], CANARY: [16, 18]}
        reply_of = {index: thread_id for thread_id, indices in replies.items() for index in indices}
        self.messages = []
        for index in range(24):
            who = "human" if index % 3 == 1 else "coder"
            if index in plan:
                thread_id, count = plan[index]
                text = LONG_FIRST if thread_id == REPLICA else f"Message {200 + index}. Thread starts here. {FILLER}"
                row = said(index, text, who)
                row["thread"] = thread_record(thread_id, index, count)
            elif index in reply_of:
                thread_id = reply_of[index]
                root = next(i for i, (tid, _count) in plan.items() if tid == thread_id)
                row = said(index, f"Reply {200 + index} in the thread. {FILLER * (1 + index % 2)}", who)
                row["thread"] = thread_record(thread_id, root, plan[root][1], is_root=False)
            else:
                row = said(index, f"Message {200 + index}. {FILLER * (1 + index % 3)}", who)
            self.messages.append(row)

        def summary(thread_id: str, root: int, last: int, extra: Json) -> Json:
            return {"id": thread_id, "root": self.messages[root], "title": "Thread",
                    "reply_count": plan[root][1], "reply_count_exact": True,
                    "updated_at": self.messages[last]["timestamp"], **extra}

        self.threads = [
            summary(RUNNER, 2, 21, {"display_name": "runner-wedged",
                                    "summary": "The overnight runner hung twice and was restarted by hand."}),
            summary(REPLICA, 9, 14, {}),
            summary(CANARY, 15, 18, {"display_name": "canary-rollback", "summary": None}),
        ]


# The two columns' boxes and the things drawn in them, measured in the page.
LAYOUT_JS = """() => {
    const box = (id) => {
        const node = document.getElementById(id);
        const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height,
                drawn: node.getClientRects().length > 0};
    };
    return {
        left: box('scroll-area'), right: box('side-column'), scroller: box('side-scroll'),
        toggle: box('column-toggle'), grip: box('width-grip'), dock: box('dock'),
        pressed: document.getElementById('column-toggle').getAttribute('aria-pressed'),
        picker: {greyed: document.getElementById('thread-select').getAttribute('aria-disabled') === 'true'
                         && !document.getElementById('thread-select').disabled,
                 value: document.getElementById('thread-select').value},
        overflow: document.documentElement.scrollWidth - window.innerWidth,
        windowWidth: window.innerWidth,
    };
}"""

# The cards: each one's thread, its box, and whether its text is cut.
CARDS_JS = """() => [...document.querySelectorAll('#side-cards > li')].map((row) => {
    const text = row.querySelector('.side-card-text');
    const r = row.getBoundingClientRect();
    return {id: row.getAttribute('data-id'), height: r.height, text: text.textContent,
            clamped: text.scrollHeight > text.clientHeight + 2, lines: Math.round(text.clientHeight /
            parseFloat(getComputedStyle(text).lineHeight))};
})"""

ROWS_JS = """(list) => [...document.querySelectorAll(`#${list} > li[data-id]`)].map((row) => row.getAttribute('data-id'))"""


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
        for label, width, height in DESKS:
            report.append(walk(playwright.chromium, args, label, width, height))
    for line in report:
        print(line)
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int) -> str:
    """One reader at one desk, against a fresh fake API and a fresh browser profile."""
    api = ColumnsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-columns-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark",
            )
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, label, width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/voice", wait_until="load")
            columns_walk(desk)
            links_walk(desk)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


class Desk:
    """The page at one desk size, and what the walk has found there."""

    def __init__(self, page: Page, screenshots: Path | None, label: str, width: int, height: int) -> None:
        self.page = page
        self.screenshots = screenshots
        self.label = label
        self.width = width
        self.height = height
        self.notes: list[str] = []

    def shot(self, name: str) -> None:
        if self.screenshots:
            self.page.screenshot(path=str(self.screenshots / f"{self.label}-{name}.png"))

    def settled(self) -> None:
        self.page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")

    def rows(self, list_id: str) -> list[str]:
        return [str(row) for row in self.page.evaluate(ROWS_JS, list_id)]

    def wait_for(self, condition: str, why: str, seconds: float = 10) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and not self.page.evaluate(condition):
            self.page.wait_for_timeout(25)
        check(bool(self.page.evaluate(condition)), f"{self.label}: {why}")

    def layout(self) -> Layout:
        return Layout(self.page.evaluate(LAYOUT_JS))

    def cards(self) -> list[Json]:
        found: list[Json] = self.page.evaluate(CARDS_JS)
        return found

    def check(self, condition: bool, why: str) -> None:
        check(condition, f"{self.label}: {why}")


class Layout:
    """What `LAYOUT_JS` measured, read with the types each part has."""

    def __init__(self, found: Json) -> None:
        self.found = found

    def box(self, name: str) -> dict[str, float]:
        part = self.found[name]
        assert isinstance(part, dict)
        return {str(key): float(value) for key, value in part.items()}

    def drawn(self, name: str) -> bool:
        part = self.found[name]
        assert isinstance(part, dict)
        return bool(part["drawn"])

    def value(self, name: str) -> object:
        return self.found[name]

    def picker(self) -> tuple[bool, str]:
        """Whether the view picker is greyed (`aria-disabled`, still a stop for the keys), and its value."""
        part = self.found["picker"]
        assert isinstance(part, dict)
        return bool(part["greyed"]), str(part["value"])


def scroll_top(page: Page, element_id: str) -> float:
    return float(page.evaluate(f"() => document.getElementById('{element_id}').scrollTop"))


def columns_walk(desk: Desk) -> None:
    page = desk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24",
                  "the channel did not open in All")
    desk.settled()

    # The button: drawn in the dock on a desk this wide, off until pressed.
    one = desk.layout()
    desk.check(one.drawn("toggle") and one.box("toggle")["width"] >= 32, "the layout button is not drawn in the dock")
    desk.check(one.value("pressed") == "false", "the layout button starts pressed")
    desk.check(not one.drawn("right"), "the right column is drawn in one column")
    page.click("#column-toggle")
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length > 0",
                  "pressing the layout button did not draw the right column")
    desk.wait_for("() => document.querySelectorAll('#side-cards > li').length === 3", "the threads are not listed")
    desk.settled()
    two = desk.layout()
    left, right = two.box("left"), two.box("right")
    desk.check(two.value("pressed") == "true", "the layout button does not say it is pressed")
    desk.check(left["right"] <= right["left"] + 1, f"the columns overlap: {left} {right}")
    desk.check(left["left"] >= 0 and right["right"] <= desk.width + 0.5, f"a column runs off the window: {left} {right}")
    desk.check(min(left["width"], right["width"]) >= COLUMN_MIN_PX,
               f"a column is narrower than {COLUMN_MIN_PX}px: {left['width']:.0f} and {right['width']:.0f}")
    desk.check(abs(left["top"] - right["top"]) < 80 and min(left["height"], right["height"]) > desk.height / 2,
               f"the columns are not side by side down the window: {left} {right}")
    greyed, value = two.picker()
    desk.check(greyed and value == "main", f"the view picker is not greyed on Main: {greyed} {value}")
    desk.check(not two.drawn("grip"), "the reading-width handle is still drawn in two columns")
    desk.check(float(str(two.value("overflow"))) <= 0, "the page scrolls sideways in two columns")
    desk.check(desk.rows("discord-log") == [str(200 + i) for i in range(24) if i not in (4, 6, 8, 10, 11, 12, 13, 14, 16, 17, 18, 19, 21)],
               f"the left column is not Main: {desk.rows('discord-log')}")
    desk.notes.append(f"two columns side by side, {left['width']:.0f}px and {right['width']:.0f}px, the picker greyed")

    # `#231 two-column-polish`. The greyed picker is still a stop for the keys, and a real ↓ or a real
    # click on it chooses nothing and says why; the right column is one region, named for its cards.
    page.focus("#thread-select")
    desk.check(page.evaluate("() => document.activeElement && document.activeElement.id") == "thread-select",
               "the greyed view picker cannot hold the focus, so the keys cannot reach its reason")
    page.keyboard.press("ArrowDown")
    # The mouse, at the picker's middle: Playwright's own click refuses a control that says it is
    # unavailable, which is the state under test.
    picker_box = page.locator("#thread-select").bounding_box()
    desk.check(picker_box is not None, "the greyed view picker is not drawn")
    assert picker_box is not None
    page.mouse.click(picker_box["x"] + picker_box["width"] / 2, picker_box["y"] + picker_box["height"] / 2)
    desk.wait_for("() => document.getElementById('status').textContent.startsWith('Two columns: Main is on the left')",
                  "a press on the greyed picker did not say why it is greyed")
    desk.check(page.evaluate("() => document.getElementById('thread-select').value") == "main",
               "a press on the greyed picker chose another view")
    page.keyboard.press("Escape")
    regions = page.evaluate("() => [...document.querySelectorAll('#side-column, #side-column [role=region]')]"
                            ".filter((node) => node.getAttribute('role') === 'region')"
                            ".map((node) => [node.id, node.getAttribute('aria-label')])")
    desk.check(regions == [["side-scroll", "Threads"]], f"the right column is not one region named for its cards: {regions}")
    desk.notes.append("the greyed picker took the focus and refused ↓ and a click, saying why; one region on the right")

    # The cards: newest activity at the foot (`#261 two-column-connectors`), one height, the summarised
    # one by its name and summary, and the long question cut to three lines.
    cards = desk.cards()
    ids = [str(card["id"]) for card in cards]
    desk.check(ids == [REPLICA, CANARY, RUNNER], f"the cards are not newest activity at the foot: {ids}")
    heights = {round(float(str(card["height"]))) for card in cards}
    desk.check(len(heights) == 1, f"the cards are not one height: {heights}")
    text = {str(card["id"]): str(card["text"]) for card in cards}
    desk.check(text[RUNNER] == "runner-wedged: The overnight runner hung twice and was restarted by hand.",
               f"the summarised thread's card says {text[RUNNER]!r}")
    desk.check(text[CANARY].startswith("Message 215. Thread starts here."),
               f"a thread named but not summed up does not say its first message: {text[CANARY]!r}")
    replica = next(card for card in cards if card["id"] == REPLICA)
    desk.check(bool(replica["clamped"]) and int(str(replica["lines"])) == 3,
               f"the long question is not cut to three lines: {replica}")
    desk.shot("1-list")
    desk.notes.append(f"three cards {heights.pop()}px each, newest at the foot, the long one cut to three lines")

    # `#257 summary-truncation`: what the clamp cuts is not out of reach. The card under the mouse is
    # the selected one, and it shows its whole text; the others keep their three lines.
    replica_box = page.locator(f"#side-cards > li[data-id='{REPLICA}']").bounding_box()
    assert replica_box is not None
    page.mouse.move(replica_box["x"] + replica_box["width"] / 2, replica_box["y"] + 12)
    desk.wait_for(f"() => document.querySelector(\"#side-cards > li[data-id='{REPLICA}']\")"
                  ".getAttribute('data-selected') === 'true'", "the card under the mouse is not the selected one")
    desk.settled()
    lit = {str(card["id"]): card for card in desk.cards()}
    desk.check(not lit[REPLICA]["clamped"] and int(str(lit[REPLICA]["lines"])) > 3,
               f"the card under the mouse does not show its whole text: {lit[REPLICA]}")
    unlit = next(card for card in cards if card["id"] == CANARY)
    desk.check(round(float(str(lit[CANARY]["height"]))) == round(float(str(unlit["height"]))),
               f"a card the mouse is not on changed height: {lit[CANARY]}")
    desk.shot("1-list-lit")
    desk.notes.append(f"the card under the mouse showed its whole text in {lit[REPLICA]['lines']} lines")

    # A card selects its thread on the right, under a heading naming it, with the X.
    page.click(f"#side-cards > li[data-id='{RUNNER}'] .side-card")
    desk.wait_for("() => document.querySelectorAll('#side-log > li[data-id]').length === 9",
                  "the card did not select its thread on the right")
    desk.settled()
    desk.check(desk.rows("side-log") == ["202", "204", "206", "208", "211", "213", "217", "219", "221"],
               f"the right column is not the thread: {desk.rows('side-log')}")
    desk.check(page.is_visible("#side-close"), "the X is not drawn")
    title = page.text_content("#side-title") or ""
    desk.check(title == "runner-wedged", f"the heading calls the thread {title!r}")
    named = page.evaluate("() => document.getElementById('side-scroll').getAttribute('aria-label')")
    desk.check(named == "Thread: runner-wedged", f"the right column's region is called {named!r}")
    desk.check(not page.is_visible("#side-freshness"), "a thread read a moment ago is said to be stale")
    desk.check(len(desk.rows("discord-log")) == 11, "selecting a thread changed the left column")

    # Each column scrolls on its own, under the wheel over it.
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = document.getElementById('scroll-area').scrollHeight; }")
    page.evaluate("() => { document.getElementById('side-scroll').scrollTop = document.getElementById('side-scroll').scrollHeight; }")
    desk.settled()
    left_before, right_before = scroll_top(page, "scroll-area"), scroll_top(page, "side-scroll")
    desk.check(right_before > 0 and left_before > 0, "a column is too short to scroll, so this proves nothing")
    right_box = desk.layout().box("scroller")
    page.mouse.move(right_box["left"] + right_box["width"] / 2, right_box["top"] + right_box["height"] / 2)
    page.mouse.wheel(0, -500)
    desk.wait_for(f"() => document.getElementById('side-scroll').scrollTop < {right_before - 100}",
                  "the wheel over the right column did not scroll it")
    desk.check(abs(scroll_top(page, "scroll-area") - left_before) < 1, "the wheel over the right column scrolled the left")
    left_box = desk.layout().box("left")
    page.mouse.move(left_box["left"] + left_box["width"] / 2, left_box["top"] + left_box["height"] / 2)
    right_now = scroll_top(page, "side-scroll")
    page.mouse.wheel(0, -500)
    desk.wait_for(f"() => document.getElementById('scroll-area').scrollTop < {left_before - 100}",
                  "the wheel over the left column did not scroll it")
    desk.check(abs(scroll_top(page, "side-scroll") - right_now) < 1, "the wheel over the left column scrolled the right")
    page.evaluate("() => { document.getElementById('side-scroll').scrollTop = 0; }")
    desk.settled()
    desk.shot("2-thread")
    desk.notes.append("a card selected its thread under its name, and each column scrolled alone")

    # The X goes back to the cards, and a Main message's Thread(N) selects its thread on the right.
    page.click("#side-close")
    desk.wait_for("() => !document.getElementById('side-list').hidden && document.getElementById('side-pane').hidden",
                  "the X did not go back to the list of threads")
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
    page.click("#discord-log > li[data-id='209'] .thread-badge")
    desk.wait_for("() => document.querySelectorAll('#side-log > li[data-id]').length === 4",
                  "a Main message's Thread(N) did not select its thread on the right")
    desk.check(desk.rows("side-log") == ["209", "210", "212", "214"], f"the wrong thread: {desk.rows('side-log')}")
    desk.check(len(desk.rows("discord-log")) == 11, "Thread(N) changed the left column")

    # Narrowed below 1000px: one column, as the channel was in one; widened: two again, as they were.
    page.set_viewport_size({"width": 900, "height": desk.height})
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length === 0",
                  "a 900px window kept two columns")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24",
                  "one column again is not All, the view the channel was in")
    narrow = desk.layout()
    greyed, value = narrow.picker()
    desk.check(not greyed and value == "flat", f"the view picker is still greyed in one column: {greyed} {value}")
    desk.check(not narrow.drawn("toggle"), "the layout button is drawn at 900px")
    desk.check(narrow.drawn("grip"), "the reading-width handle did not come back")
    page.set_viewport_size({"width": desk.width, "height": desk.height})
    # The stylesheet draws the right column the moment the window is wide again; the page decides two
    # columns at the next frame, and its picker greying out is what says it has.
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length > 0"
                  " && document.getElementById('thread-select').getAttribute('aria-disabled') === 'true'"
                  " && document.querySelectorAll('#discord-log > li[data-id]').length === 11",
                  "widening the window again did not bring two columns back")
    desk.wait_for("() => document.querySelectorAll('#side-log > li[data-id]').length === 4",
                  "widening again lost the thread selected")
    desk.notes.append("at 900px one column in All with the button gone; wide again, two with the same thread")

    # The keys, pressed for real: ] to the right column, Esc its X, j to a card with a ring, Enter
    # selects it, [ back to the left column. The right column keeps a selected message per thread, and
    # opening this one from Thread(N) selected its root there, so ] lands on that message: the one row
    # lit on the page, and the focus on it (`#239 single-selection`).
    page.focus("#scroll-area")
    page.keyboard.press("]")
    desk.wait_for("() => { const held = document.activeElement;"
                  " const lit = [...document.querySelectorAll('[data-selected=\"true\"]')];"
                  " return held && held.matches(\"#side-log > li[data-id='209']\")"
                  " && lit.length === 1 && lit[0] === held; }",
                  "] did not give the keys to the right column's selected message, the thread's root, the one row lit")
    page.keyboard.press("Escape")
    desk.wait_for("() => !document.getElementById('side-list').hidden", "Esc in the right column did not close its thread")
    page.keyboard.press("j")
    desk.wait_for("() => document.activeElement && document.activeElement.matches('#side-cards > li')"
                  " && document.activeElement.matches(':focus-visible')",
                  "j did not put a ring round a card")
    ringed = str(page.evaluate("() => document.activeElement.getAttribute('data-id')"))
    page.keyboard.press("Enter")
    desk.wait_for("() => !document.getElementById('side-pane').hidden"
                  f" && document.querySelectorAll('#side-log > li[data-id]').length === {THREAD_ROWS[ringed]}",
                  f"Enter on the card of {ringed} did not select its thread")
    page.keyboard.press("[")
    desk.wait_for("() => document.getElementById('scroll-area').contains(document.activeElement)",
                  "[ did not give the keys back to the left column")
    desk.check(len(desk.rows("discord-log")) == 11, "the keys changed the left column")
    desk.notes.append("] Esc j Enter [ moved the keys between the columns and through the cards")

    # A phone's width: the button is never drawn.
    page.set_viewport_size({"width": 412, "height": 915})
    desk.wait_for("() => document.getElementById('column-toggle').getClientRects().length === 0",
                  "the layout button is drawn at 412px")
    phone = desk.layout()
    desk.check(not phone.drawn("right"), "the right column is drawn at 412px")
    desk.check(float(str(phone.value("overflow"))) <= 0, "the page scrolls sideways at 412px")
    desk.notes.append("at 412px neither the button nor the right column is drawn")


# `#261 two-column-connectors`. The links between the columns, measured where Chromium draws them: each
# link's points converted from the SVG's corner to the viewport, with the boxes it should touch.
LINKS_JS = """() => {
    const box = (node) => { const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom}; };
    const view = (node) => { const r = node.getBoundingClientRect(); return {top: r.top, bottom: r.top + node.clientHeight}; };
    const svg = document.getElementById('column-links');
    const corner = svg.getBoundingClientRect();
    const numbers = (path) => path.getAttribute('d').match(/-?\\d+(?:\\.\\d+)?/g).map(Number);
    const links = [...svg.querySelectorAll('path.column-link')].map((path) => ({
        thread: path.getAttribute('data-thread'), clip: path.getAttribute('data-link'),
        selected: path.hasAttribute('data-selected'), line: numbers(path), marks: numbers(path.nextElementSibling),
        stroke: getComputedStyle(path).stroke, fill: getComputedStyle(path.nextElementSibling).fill}));
    const ends = {};
    for (const card of document.querySelectorAll('#side-cards > li[data-card]')) {
        const id = card.getAttribute('data-id');
        const button = card.querySelector('.side-card');
        const root = [...document.querySelectorAll('#discord-log > li[data-id]')].find((row) => row.getAttribute('data-id') === ROOTS[id]);
        const badge = root ? root.querySelector('.thread-badge') : null;
        ends[id] = {card: box(card), edge: getComputedStyle(button).borderLeftColor,
                    root: root ? box(root) : null, badge: badge ? box(badge) : null,
                    tag: badge ? getComputedStyle(badge).borderTopColor : null};
    }
    return {corner: {left: corner.left, top: corner.top}, left: view(document.getElementById('scroll-area')),
            right: view(document.getElementById('side-scroll')), links, ends};
}"""

# The root each thread's card should link to.
ROOTS = {RUNNER: "202", REPLICA: "209", CANARY: "215"}

# How long the page waits on a selection before gliding to its other end (web/voice.js `COLUMN_GLIDE_MS`).
GLIDE_MS = 1500


def measure_links(desk: Desk) -> Json:
    found: Json = desk.page.evaluate(LINKS_JS.replace("ROOTS[id]", f"({{...{json.dumps(ROOTS)}}})[id]"))
    return found


def frame(desk: Desk) -> None:
    """Let a scroll's frame draw the links: two frames, the one asked for and the one after it."""
    desk.settled()
    desk.settled()


def numbers(link: Json, key: str) -> list[float]:
    """A link's path, `line` or `marks`, as its numbers."""
    values = link[key]
    assert isinstance(values, list)
    return [float(value) for value in values]


def links_of(found: Json) -> list[Json]:
    """The links `measure_links` found."""
    links = found["links"]
    assert isinstance(links, list)
    return [link for link in links if isinstance(link, dict)]


def selected_links(found: Json) -> list[Json]:
    """The selection's link, if it is drawn: one at most."""
    return [link for link in links_of(found) if link["selected"]]


def ends_of(found: Json, thread: str) -> Json:
    """The boxes the link of `thread` should touch."""
    ends = found["ends"]
    assert isinstance(ends, dict)
    end = ends[thread]
    assert isinstance(end, dict)
    return end


def link_points(found: Json, link: Json) -> tuple[float, float, float, float, float]:
    """A whole link's leaving point, lane, arriving height and arrowhead point, in the viewport."""
    corner = found["corner"]
    assert isinstance(corner, dict)
    line = numbers(link, "line")
    marks = numbers(link, "marks")
    left, top = float(corner["left"]), float(corner["top"])
    # M fx fy H lane V ty H end; the arrowhead's point is the first point of its triangle.
    return line[0] + left, line[1] + top, line[2] + left, line[3] + top, marks[-6] + left


def pairs_on_screen(found: Json) -> tuple[list[str], list[str]]:
    """The threads whose root's tag and card both show, by the middles a link meets, and those drawn whole:
    the selection's among them, which is drawn whole when both its ends show too."""
    ends = found["ends"]
    left, right = found["left"], found["right"]
    assert isinstance(ends, dict) and isinstance(left, dict) and isinstance(right, dict)
    # Both columns from where the right one's list starts, under its heading, as the page takes them.
    top = max(float(left["top"]), float(right["top"]))
    shown = []
    for thread, end in ends.items():
        assert isinstance(end, dict)
        if not end["badge"]:
            continue
        badge, card = end["badge"], end["card"]
        assert isinstance(badge, dict) and isinstance(card, dict)
        from_y = (float(badge["top"]) + float(badge["bottom"])) / 2
        to_y = (float(card["top"]) + float(card["bottom"])) / 2
        if top <= from_y <= float(left["bottom"]) and top <= to_y <= float(right["bottom"]):
            shown.append(str(thread))
    links = found["links"]
    assert isinstance(links, list)
    return sorted(shown), sorted(str(link["thread"]) for link in links if link["clip"] == "both")


def settled_links(desk: Desk, seconds: float = 3) -> Json:
    """The links once they are the pairs on screen: a scroll can bring a summary in, and rows move under it."""
    deadline = time.monotonic() + seconds
    found = measure_links(desk)
    while time.monotonic() < deadline:
        shown, drawn = pairs_on_screen(found)
        if shown == drawn:
            break
        desk.page.wait_for_timeout(50)
        found = measure_links(desk)
    return found


def check_whole_links(desk: Desk, found: Json, where: str) -> list[str]:
    """Every link drawn is a pair with both ends on screen, every such pair is drawn, and each touches its boxes."""
    ends = found["ends"]
    assert isinstance(ends, dict)
    shown, drawn = pairs_on_screen(found)
    desk.check(drawn == shown, f"{where}: the links drawn {drawn} are not the pairs on screen {shown}")
    links = found["links"]
    assert isinstance(links, list)
    for link in links:
        assert isinstance(link, dict)
        if link["clip"] != "both":
            continue
        end = ends[str(link["thread"])]
        assert isinstance(end, dict)
        root, badge, card = end["root"], end["badge"], end["card"]
        assert isinstance(root, dict) and isinstance(badge, dict) and isinstance(card, dict)
        fx, fy, lane, ty, tip = link_points(found, link)
        desk.check(abs(fx - float(root["right"])) <= 1, f"{where}: a link leaves at x {fx}, not its root's right edge {root['right']}")
        desk.check(float(badge["top"]) <= fy <= float(badge["bottom"]), f"{where}: a link leaves its root at y {fy}, not level with its tag {badge}")
        desk.check(abs(tip - float(card["left"])) <= 1, f"{where}: a link's arrowhead is at x {tip}, not its card's left edge {card['left']}")
        desk.check(float(card["top"]) <= ty <= float(card["bottom"]), f"{where}: a link arrives at y {ty}, outside its card {card}")
        desk.check(float(root["right"]) < lane < float(card["left"]), f"{where}: a link's lane at {lane} is not in the gap")
        desk.check(link["stroke"] == end["edge"] == end["tag"] and link["fill"] == end["edge"],
                   f"{where}: a link is {link['stroke']}, its card's edge {end['edge']} and its tag {end['tag']}")
    return drawn


def scroll_to(desk: Desk, element_id: str, where: str) -> None:
    """Scroll a column as a reader's wheel would end up: `where` is "top", "bottom" or a pixel offset."""
    target = {"top": "0", "bottom": "node.scrollHeight"}.get(where, where)
    desk.page.evaluate(f"() => {{ const node = document.getElementById('{element_id}'); node.scrollTop = {target}; }}")
    frame(desk)


def hover(desk: Desk, selector: str) -> None:
    """Move the mouse onto the middle of `selector`'s box, a real move, which selects what is under it."""
    box = desk.page.locator(selector).bounding_box()
    desk.check(box is not None, f"{selector} is not drawn")
    assert box is not None
    desk.page.mouse.move(box["x"] + box["width"] / 2 - 4, box["y"] + box["height"] / 2 - 4)
    desk.page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2, steps=3)
    frame(desk)


def watch_scrolls(desk: Desk, element_id: str) -> None:
    """Record each scroll position `element_id` passes through, to tell a glide from a jump."""
    desk.page.evaluate(f"""() => {{ const node = document.getElementById('{element_id}'); window.__glide = [];
        node.addEventListener('scroll', () => window.__glide.push(node.scrollTop), {{passive: true}}); }}""")


def links_walk(desk: Desk) -> None:
    """`#261 two-column-connectors`: the links over the cards, the selection's off screen, and its glide."""
    page = desk.page
    page.set_viewport_size({"width": desk.width, "height": desk.height})
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length > 0"
                  " && document.getElementById('thread-select').getAttribute('aria-disabled') === 'true'",
                  "two columns did not come back for the links")
    page.click("#side-close")
    desk.wait_for("() => !document.getElementById('side-list').hidden", "the X did not go back to the cards")
    page.mouse.move(5, desk.height - 5)

    # Whole links: every pair with both ends on screen, and only those, wherever Main is scrolled.
    seen: set[str] = set()
    for where in ("bottom", "top", "600", "1200"):
        scroll_to(desk, "scroll-area", where)
        seen.update(check_whole_links(desk, settled_links(desk), f"Main at {where}"))
    desk.check(len(seen) >= 2, f"so few links were ever drawn ({sorted(seen)}) that this proves little")
    scroll_to(desk, "scroll-area", "bottom")
    desk.shot("3-links")
    desk.notes.append(f"links drawn for exactly the pairs on screen, Main scrolled four ways, touching their boxes ({len(seen)} threads)")

    # The runner's card, its root (202) above Main's top: its link drawn up to Main's edge, pointing up.
    hover(desk, f"#side-cards > li[data-id='{RUNNER}'] .side-card")
    found = measure_links(desk)
    selected = selected_links(found)
    desk.check(len(selected) == 1 and selected[0]["thread"] == RUNNER and selected[0]["clip"] == "up",
               f"the selected card's link is not drawn up to its root off screen: {selected}")
    left, right = found["left"], found["right"]
    assert isinstance(left, dict) and isinstance(right, dict)
    marks = numbers(selected[0], "marks")
    corner = found["corner"]
    assert isinstance(corner, dict)
    # The marks of a link whose card shows: the arrowhead pointing the way to its root first, its point
    # first of all, then the one into the card. It points at the columns' shared top, under the right
    # one's heading, so it crosses no heading.
    edge = max(float(left["top"]), float(right["top"])) + 2
    desk.check(abs(marks[1] + float(corner["top"]) - edge) <= 1,
               f"the arrowhead does not point up at the columns' top edge {edge}: {marks[1]}")
    desk.shot("4-selected-off-screen")
    # Left selected, Main glides — smoothly, through positions between — until the root is level with it.
    watch_scrolls(desk, "scroll-area")
    before = scroll_top(page, "scroll-area")
    page.wait_for_timeout(GLIDE_MS + 900)
    frame(desk)
    passed = [float(value) for value in page.evaluate("() => window.__glide")]
    desk.check(scroll_top(page, "scroll-area") < before - 100, "Main did not glide up to the card's root")
    desk.check(len(set(passed)) >= 3, f"Main jumped rather than glided: {passed}")
    found = settled_links(desk)
    selected = selected_links(found)
    desk.check(len(selected) == 1 and selected[0]["clip"] == "both", f"the link is not whole after the glide: {selected}")
    check_whole_links(desk, found, "after the glide")
    end = ends_of(found, RUNNER)
    badge, card = end["badge"], end["card"]
    assert isinstance(badge, dict) and isinstance(card, dict)
    level = abs((float(badge["top"]) + float(badge["bottom"])) / 2 - (float(card["top"]) + float(card["bottom"])) / 2)
    # Within a few pixels: a smooth scroll ends on a whole pixel, and the tag's middle need not be on one.
    desk.check(level <= 4, f"the root was not brought level with its card: {level:.1f}px apart")
    desk.shot("5-after-glide")
    desk.notes.append(f"the selected card's link pointed up off screen, then Main glided {len(passed)} steps to level")

    # The reader's own scroll of Main while the selection waits — a little, its root still off screen,
    # and not to the top, which reads older messages — and nothing glides.
    hover(desk, f"#side-cards > li[data-id='{CANARY}'] .side-card")
    found = measure_links(desk)
    selected = selected_links(found)
    desk.check(len(selected) == 1 and selected[0]["clip"] == "down", f"the canary's root is on screen, so this proves nothing: {selected}")
    scroll_to(desk, "scroll-area", str(scroll_top(page, "scroll-area") + 60))
    put = scroll_top(page, "scroll-area")
    page.wait_for_timeout(GLIDE_MS + 500)
    desk.check(abs(scroll_top(page, "scroll-area") - put) < 1, "Main glided after the reader scrolled it")
    selected = selected_links(measure_links(desk))
    desk.check(len(selected) == 1 and selected[0]["clip"] == "down", f"the canary's root came on screen anyway: {selected}")

    # Reduced motion: the same glide, in one step. Main at its foot, the replica's root (209) above it.
    page.emulate_media(reduced_motion="reduce")
    scroll_to(desk, "scroll-area", "bottom")
    hover(desk, f"#side-cards > li[data-id='{REPLICA}'] .side-card")
    found = measure_links(desk)
    selected = selected_links(found)
    desk.check(len(selected) == 1 and selected[0]["clip"] == "up", f"the replica's link does not point up to its root: {selected}")
    watch_scrolls(desk, "scroll-area")
    page.wait_for_timeout(GLIDE_MS + 600)
    frame(desk)
    passed = [float(value) for value in page.evaluate("() => window.__glide")]
    desk.check(len(set(passed)) == 1, f"with reduced motion Main did not jump in one step: {passed}")
    found = settled_links(desk)
    selected = selected_links(found)
    desk.check(len(selected) == 1 and selected[0]["clip"] == "both", f"the link is not whole after the jump: {selected}")
    check_whole_links(desk, found, "after the jump")
    page.emulate_media(reduced_motion="no-preference")
    desk.notes.append("a reader's scroll cancelled a glide, and reduced motion jumped in one step")


if __name__ == "__main__":
    raise SystemExit(main())
