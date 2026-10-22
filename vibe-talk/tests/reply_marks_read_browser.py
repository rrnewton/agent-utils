#!/usr/bin/env python3
"""A reply marks read what it answers, whichever way it was started, in two columns in real Chromium.

`#300 reply-marks-read`. The owner, 2026-10-14: Reply on a message and opening its thread to write in it
both make a reply to the message before, and they marked read differently. The rule: the first reply
that starts a thread marks its root read, whether it was started from Reply or from the thread; a later
message in an existing thread's box marks nothing on its own; a targeted Reply marks the message it
answers. Marking read is the page's own read action: Done, saved to the server, and the provider's read
marker moved through the message where the provider has one, asked with `after_reply` so the server can
keep it forward and ask once. The fake-DOM suite (tests/js/voice_page.test.mjs) pins each path; this
drives the two the owner uses on a desk with a real mouse and keyboard, where the box is docked in a
column (`#277 reply-in-column`) and the thread opens on the right.

It serves the checked-out page and the fake API of tests/inline_reply_browser.py — a channel taller
than the window with three threads, one message more heading a thread nobody has answered, as Google
Chat names one for every message — whose provider can move its read marker, from one loopback origin.
At 1280x800, in two columns, the runner's thread on the right:

  Reply on a Main message in the left column, typed and sent with Enter -> the message is Done on the
  server, the provider was asked to mark it `after_reply`, after the Done, the status line says it is
  marked read here and in the chat service, and the row is drawn read in the left column -> Reply on a reply in the right column -> the same, for that reply, drawn read
  on the right -> a message written in the right column's box, in the runner's thread, which has
  replies -> nothing more is marked, here or at the provider (the negative control for the rule) ->
  the grey Thread on the unanswered message -> its empty thread opens on the right with the box
  focused; the first reply typed there and sent -> its root is Done, marked at the provider, and
  drawn read in BOTH columns -> a second message in that thread -> nothing more is marked.

No page error and no sideways scroll at any step. Pass --screenshots DIR to keep a PNG of each state for
review; --web-root DIR to run the same walk against another copy of the page.
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
from offline_cache_browser import CHANNEL, Json, check, handler_for, thread_of
from inline_reply_browser import ReplyColumnsApi, open_columns, place_row, reply_button
from two_column_browser import RUNNER, Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

WIDTH, HEIGHT = 1280, 800
# The message heading a thread nobody has answered, and that thread.
LONE = "222"
LONE_THREAD = "spaces/A/threads/lone"
# The Main message answered from the left column, and the runner's reply answered from the right.
LEFT_TARGET = "203"
RIGHT_TARGET = "206"
# Where a pressed row's top is put, below the top of its column's list, in px.
ROW_OFFSET_PX = 90


class MarkerApi(ReplyColumnsApi):
    """`ReplyColumnsApi`, on a provider that can move its own read marker: every mark asked is kept, in
    order, beside every Done; and message 222 heads a thread with no replies."""

    def __init__(self) -> None:
        super().__init__()
        self.upstream_read = True
        self.marks: list[Json] = []
        # Every write, in the order it arrived, so "after the Done" is an observation.
        self.writes: list[str] = []
        lone = next(m for m in self.messages if m["id"] == LONE)
        lone["thread"] = {"id": LONE_THREAD, "root_message_id": LONE, "is_root": True,
                          "reply_count": 0, "reply_count_exact": True}

    def post(self, path: str, body: Json) -> tuple[int, Json]:
        with self.lock:
            self.writes.append(path.rsplit("/", 1)[-1])
        if path.endswith("/upstream-read"):
            with self.lock:
                self.marks.append(dict(body))
            return 200, {"channel": CHANNEL, "through": body.get("message_id"), "outcome": "marked",
                         "upstream_read_notice": "Read cursors only move forward."}
        answer = super().post(path, body)
        if path.endswith("/reply") and answer[0] == 200:
            # The thread the reply went into now has one, as the service's next read would say.
            thread_id = body.get("thread_id")
            with self.lock:
                for row in self.messages:
                    record = thread_of(row)
                    if thread_id and record.get("id") == thread_id and record.get("is_root") is True:
                        record["reply_count"] = int(str(record.get("reply_count") or 0)) + 1
        return answer

    def done(self) -> list[str]:
        with self.lock:
            return sorted(self.dismissed)

    def marked(self) -> list[str]:
        with self.lock:
            return [str(mark.get("message_id")) for mark in self.marks]

    def all_after_reply(self) -> bool:
        with self.lock:
            return all(mark.get("after_reply") is True for mark in self.marks)

    def mark_follows_done(self, message_id: str) -> bool:
        """Whether the mark for `message_id` was asked after a Done was written for it."""
        with self.lock:
            writes = list(self.writes)
            marks = [str(mark.get("message_id")) for mark in self.marks]
        at = marks.index(message_id) if message_id in marks else -1
        if at < 0:
            return False
        # The at-th mark's place among the writes, and a Done before it.
        seen = -1
        for index, write in enumerate(writes):
            if write == "upstream-read":
                seen += 1
                if seen == at:
                    return "dismiss" in writes[:index]
        return False


ROW_JS = """([list, id]) => {
    const row = [...document.querySelectorAll(`#${list} > li[data-id]`)]
        .find((li) => (li.getAttribute('data-ids') || li.getAttribute('data-id')).split(' ').includes(id));
    return row ? {archived: row.getAttribute('data-archived'),
                  done: (row.querySelector('.done-button') || {}).textContent || ''} : null;
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
    with sync_playwright() as playwright:
        print(walk(playwright.chromium, args))
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace) -> str:
    """One reader at the laptop's size, against a fresh fake API and a fresh browser profile."""
    api = MarkerApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-reply-marks-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": WIDTH, "height": HEIGHT}, color_scheme="dark",
            )
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"reply-marks-{WIDTH}", WIDTH, HEIGHT)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            open_columns(desk)
            docked_reply(desk, api, "left", LEFT_TARGET)
            docked_reply(desk, api, "right", RIGHT_TARGET)
            answered_thread_box(desk, api)
            new_thread_box(desk, api)
            overflow = float(page.evaluate("() => document.documentElement.scrollWidth - window.innerWidth"))
            check(overflow <= 0, f"the page scrolls sideways by {overflow}px")
            check(not errors, f"the page threw: {errors}")
            context.close()
            return f"{version} at {WIDTH}x{HEIGHT}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def list_of(column: str) -> str:
    return "discord-log" if column == "left" else "side-log"


def row_state(desk: Desk, column: str, message_id: str) -> Json | None:
    found = desk.page.evaluate(ROW_JS, [list_of(column), message_id])
    return found if isinstance(found, dict) else None


def drawn_read(desk: Desk, column: str, message_id: str) -> bool:
    state = row_state(desk, column, message_id)
    return state is not None and state.get("archived") == "true" and state.get("done") == "Unarchive"


def wait_marked(desk: Desk, api: MarkerApi, message_id: str, where: str) -> None:
    """Until the server holds `message_id` Done and the provider was asked to mark it, and the status
    line, which the Done left saying the chat service was not marked, says it was."""
    for _ in range(400):
        if message_id in api.done() and message_id in api.marked():
            break
        desk.page.wait_for_timeout(25)
    check(message_id in api.done() and message_id in api.marked(),
          f"{where}: {message_id} is not marked read — Done {api.done()}, provider {api.marked()}")
    desk.wait_for("() => document.getElementById('status').textContent.startsWith('Marked read here and in Discord.')",
                  f"{where}: the status line still says the message was not marked in the chat service")


def docked_reply(desk: Desk, api: MarkerApi, column: str, message_id: str) -> None:
    """Reply on `message_id` in `column`, docked there, sent with Enter: the message it answers is read."""
    page = desk.page
    where = f"{column} Reply"
    place_row(desk, column, message_id, top=ROW_OFFSET_PX)
    desk.check(not drawn_read(desk, column, message_id), f"{where}: the premise is an unread message")
    button = reply_button(desk, column, message_id)
    page.mouse.click(button["x"], button["y"])
    mount = "column-reply" if column == "left" else "side-reply"
    desk.wait_for(f"() => !document.getElementById('{mount}').hidden && document.querySelector('.reply-compose').parentElement.id === '{mount}'",
                  f"{where}: the box did not open in the column")
    before = len(api.posted())
    page.keyboard.type(f"answering {message_id}")
    desk.shot(f"{column}-reply-open")
    page.keyboard.press("Enter")
    desk.wait_for(f"() => document.getElementById('{mount}').hidden",
                  f"{where}: Enter did not send the reply and put the box back")
    for _ in range(200):
        if len(api.posted()) > before:
            break
        page.wait_for_timeout(25)
    posted = api.posted()
    desk.check(len(posted) == before + 1 and posted[-1].get("reply_to") == message_id,
               f"{where}: the reply did not post as a reply to {message_id}: {posted[before:]}")
    wait_marked(desk, api, message_id, where)
    desk.check(api.all_after_reply(), f"{where}: a mark was asked as the owner's own, not after a reply")
    desk.check(api.mark_follows_done(message_id), f"{where}: the provider was asked before the Done was saved")
    desk.wait_for(f"() => {{ const s = ({ROW_JS})(['{list_of(column)}', '{message_id}']);"
                  " return Boolean(s) && s.archived === 'true' && s.done === 'Unarchive'; }",
                  f"{where}: {message_id} is not drawn read in the column it was answered in")
    desk.shot(f"{column}-reply-marked")
    desk.notes.append(f"{where} on {message_id}: Done, marked at the provider, drawn read")


def write_on_right(desk: Desk, text: str) -> None:
    page = desk.page
    page.click("#side-compose-text")
    page.keyboard.type(text)
    page.click("#side-send")


def wait_posted(desk: Desk, api: MarkerApi, count: int, where: str) -> Json:
    for _ in range(200):
        if len(api.posted()) >= count:
            break
        desk.page.wait_for_timeout(25)
    posted = api.posted()
    desk.check(len(posted) >= count, f"{where}: nothing was posted")
    return posted[-1]


def answered_thread_box(desk: Desk, api: MarkerApi) -> None:
    """A message in the runner's thread's box: a later reply in an answered thread marks nothing."""
    where = "answered thread's box"
    done, marked, count = api.done(), api.marked(), len(api.posted())
    write_on_right(desk, "a later message in the runner's thread")
    last = wait_posted(desk, api, count + 1, where)
    desk.check(last.get("thread_id") == RUNNER and not last.get("reply_to"),
               f"{where}: it did not post into the thread from its box: {last}")
    # As long as a mark would take to arrive, had one been asked: the Done and the mark above came
    # well within it.
    desk.page.wait_for_timeout(800)
    desk.check(api.done() == done, f"{where}: a later message in a thread marked something Done: {api.done()}")
    desk.check(api.marked() == marked, f"{where}: a later message in a thread asked the provider: {api.marked()}")
    desk.notes.append(f"{where}: posted, nothing marked")


def new_thread_box(desk: Desk, api: MarkerApi) -> None:
    """The grey Thread on 222, its first reply typed in the right column's box: the root is read."""
    page = desk.page
    where = "new thread's box"
    place_row(desk, "left", LONE, top=ROW_OFFSET_PX)
    tag = f"#discord-log > li[data-id='{LONE}'] .thread-badge[data-thread-state='start']"
    desk.check(page.locator(tag).count() == 1, f"{where}: {LONE} has no grey Thread to start its thread")
    desk.check(not drawn_read(desk, "left", LONE), f"{where}: the premise is an unread root")
    page.click(tag)
    desk.wait_for(f"() => [...document.querySelectorAll('#side-log > li[data-id]')].map((li) => li.getAttribute('data-id')).join() === '{LONE}'",
                  f"{where}: the grey Thread did not open the empty thread on the right")
    desk.wait_for("() => document.activeElement && document.activeElement.id === 'side-compose-text'",
                  f"{where}: the right column's box was not focused")
    count = len(api.posted())
    page.keyboard.type("the first reply, which starts the thread")
    page.click("#side-send")
    last = wait_posted(desk, api, count + 1, where)
    desk.check(last.get("thread_id") == LONE_THREAD and not last.get("reply_to"),
               f"{where}: the first reply did not post into the new thread from its box: {last}")
    wait_marked(desk, api, LONE, where)
    desk.check(api.all_after_reply(), f"{where}: the root's mark was asked as the owner's own")
    desk.check(api.mark_follows_done(LONE), f"{where}: the provider was asked before the root's Done was saved")
    for column in ("left", "right"):
        desk.wait_for(f"() => {{ const s = ({ROW_JS})(['{list_of(column)}', '{LONE}']);"
                      " return Boolean(s) && s.archived === 'true' && s.done === 'Unarchive'; }",
                      f"{where}: the root is not drawn read in the {column} column")
    desk.shot("new-thread-marked")
    # A second message there is a later one.
    done, marked = api.done(), api.marked()
    write_on_right(desk, "and a second message in it")
    wait_posted(desk, api, count + 2, where)
    page.wait_for_timeout(800)
    desk.check(api.done() == done and api.marked() == marked,
               f"{where}: the second message in the new thread marked something: Done {api.done()}, provider {api.marked()}")
    desk.notes.append(f"{where}: the first reply marked {LONE} read in both columns, the second nothing")


if __name__ == "__main__":
    raise SystemExit(main())
