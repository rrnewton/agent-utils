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
  the view picker greyed out and the reading-width handle gone -> the right column lists the threads
  as cards, newest activity first, every card the same height, the summarised one saying its name and
  summary, the long first message cut to three lines -> a card selects its thread: its rows in the
  right column under a heading naming it, with the X -> each column scrolls on its own, under the
  wheel over it -> the X goes back to the cards, and a Main message's Thread(N) selects its thread on
  the right -> the window narrowed to 900px: one column, as the channel was in one, the button not
  drawn; widened again: two columns, the same thread selected -> at a phone's width the button is not
  drawn. No horizontal overflow and no page error at any step.

Pass --screenshots DIR to keep a PNG of each state (the list and a selected thread, at each size) for
review; --web-root DIR to run the same walk against another copy of the page.
"""

from __future__ import annotations

import argparse
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
        picker: {disabled: document.getElementById('thread-select').disabled,
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
        part = self.found["picker"]
        assert isinstance(part, dict)
        return bool(part["disabled"]), str(part["value"])


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
    disabled, value = two.picker()
    desk.check(disabled and value == "main", f"the view picker is not greyed on Main: {disabled} {value}")
    desk.check(not two.drawn("grip"), "the reading-width handle is still drawn in two columns")
    desk.check(float(str(two.value("overflow"))) <= 0, "the page scrolls sideways in two columns")
    desk.check(desk.rows("discord-log") == [str(200 + i) for i in range(24) if i not in (4, 6, 8, 10, 11, 12, 13, 14, 16, 17, 18, 19, 21)],
               f"the left column is not Main: {desk.rows('discord-log')}")
    desk.notes.append(f"two columns side by side, {left['width']:.0f}px and {right['width']:.0f}px, the picker greyed")

    # The cards: newest activity first, one height, the summarised one by its name and summary, and the
    # long question cut to three lines.
    cards = desk.cards()
    ids = [str(card["id"]) for card in cards]
    desk.check(ids == [RUNNER, CANARY, REPLICA], f"the cards are not newest activity first: {ids}")
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
    desk.notes.append(f"three cards {heights.pop()}px each, newest first, the long one cut to three lines")

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
    disabled, value = narrow.picker()
    desk.check(not disabled and value == "flat", f"the view picker is still greyed in one column: {disabled} {value}")
    desk.check(not narrow.drawn("toggle"), "the layout button is drawn at 900px")
    desk.check(narrow.drawn("grip"), "the reading-width handle did not come back")
    page.set_viewport_size({"width": desk.width, "height": desk.height})
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length > 0",
                  "widening the window again did not bring two columns back")
    desk.wait_for("() => document.querySelectorAll('#side-log > li[data-id]').length === 4",
                  "widening again lost the thread selected")
    desk.notes.append("at 900px one column in All with the button gone; wide again, two with the same thread")

    # A phone's width: the button is never drawn.
    page.set_viewport_size({"width": 412, "height": 915})
    desk.wait_for("() => document.getElementById('column-toggle').getClientRects().length === 0",
                  "the layout button is drawn at 412px")
    phone = desk.layout()
    desk.check(not phone.drawn("right"), "the right column is drawn at 412px")
    desk.check(float(str(phone.value("overflow"))) <= 0, "the page scrolls sideways at 412px")
    desk.notes.append("at 412px neither the button nor the right column is drawn")


if __name__ == "__main__":
    raise SystemExit(main())
