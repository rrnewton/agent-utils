#!/usr/bin/env python3
"""One page, complete thread rows (`#1986 vibe-talk-one-page-complete-rows`).

Serves the checked-out page with a fake API whose one thread reports 120
replies while the store holds only three of them — as a server that has read
the thread holds all 120 in its cache and counts them there. The thread's
summary carries what slice 1's server counts onto it (`unread_count` 7 of
120, its slug, its last reply and its participants).

At a 412x915 phone and a 1280x800 desk, in dark: the root's row on the FIRST
page shows the slug, "120 replies" and "7 unread" — 7 being a count no
counting of the three held replies could reach (they would say at most 3
unread, or 117 "new") — the thread card says who last replied and who is in
it, and the fake API's own request log (the event count) holds no thread
read and no step back (`before=`) at all: nothing was reached back for.
`--screenshots DIR` keeps one PNG per stage.
"""

from __future__ import annotations

import argparse
import os
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import labels_browser as labels_fixture
import offline_cache_browser as fixture
from keyboard_hides_bar_browser import IOS_VIEWPORT_JS
from labels_browser import LabelsApi, Walk, labels_handler
from offline_cache_browser import CHANNEL, TOKEN, check, message
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import Page

THREAD = "spaces/A/threads/big"


def _thread(root_id: str, is_root: bool, count: int) -> dict[str, object]:
    record: dict[str, object] = {"id": THREAD, "root_message_id": root_id,
                                 "is_root": is_root, "reply_count": count, "reply_count_exact": True}
    if is_root:
        # As the real server serves a filed name: on the root's own record, on
        # every timeline answer (summarize annotate's root naming), not only
        # on the thread list's summary.
        record["display_name"] = "runner-wedged"
        # Slice 2: the serve-time facts on the root's own record too, counted
        # by the server over all 120 replies it holds, not the 3 held here.
        record.update({
            "unread_count": 7, "unread_exact": True,
            "last_reply_author": "grace", "last_reply_author_id": "1000000000000000009",
            "last_reply_excerpt": "third reply the page holds",
            "participants": [
                {"author": "grace", "author_id": "1000000000000000009", "reply_count": 70},
                {"author": "ada", "author_id": "1000000000000000008", "reply_count": 50},
            ],
            "participants_more": 0, "pinned": False, "root_done": False, "replies_done": 113,
        })
    return record


class OnePageApi(LabelsApi):
    """One thread of 120 reported replies, three held; the summary counts the rest."""

    def __init__(self) -> None:
        super().__init__()
        rows = [
            message(10, "Big thread root about the wedged runner.", _thread("210", True, 120)),
            message(11, "first reply the page holds", _thread("210", False, 120)),
            message(12, "second reply the page holds", _thread("210", False, 120)),
            message(13, "third reply the page holds", _thread("210", False, 120)),
            message(14, "an unthreaded channel note."),
        ]
        self.messages = rows
        self.threads = [{
            "id": THREAD, "root": rows[0], "title": "Big thread",
            "reply_count": 120, "reply_count_exact": True,
            "updated_at": rows[3]["timestamp"],
            "display_name": "runner-wedged",
            "summary": "The runner is wedged on a stale lock.",
            "unread_count": 7, "unread_exact": True,
            "last_reply_at": rows[3]["timestamp"], "last_reply_author": "grace",
            "last_reply_author_id": "1000000000000000009",
            "last_reply_excerpt": "third reply the page holds",
            "participants": [
                {"author": "grace", "author_id": "1000000000000000009", "reply_count": 70},
                {"author": "ada", "author_id": "1000000000000000008", "reply_count": 50},
            ],
            "participants_more": 0, "pinned": False, "root_done": False, "replies_done": 113,
        }]
        self.thread_pins_revision = 3
        self.channel_summary = {
            "thread_count": 1, "unread_reply_count": 7, "unread_exact": True,
            "latest_activity_at": rows[3]["timestamp"],
        }

    def timeline(self, query: dict[str, list[str]]) -> dict[str, object]:
        answer = super().timeline(query)
        answer["thread_pins_revision"] = self.thread_pins_revision
        answer["channel_summary"] = self.channel_summary
        return answer


def walk_through(walk: Walk, api: OnePageApi) -> None:
    page: Page = walk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function("() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden", timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    walk.until("() => document.querySelectorAll('#discord-log > li[data-id]').length >= 2",
               "the channel did not open")

    # The row is complete from the summaries the first pages carried: slug,
    # the reported 120 (not the 3 held), and the server's 7 unread.
    walk.until("""() => { const row = document.querySelector('#discord-log > li[data-id="210"]');
      return !!row && row.textContent.includes('runner-wedged'); }""",
               "the thread slug did not stand on the first page")
    walk.until("""() => { const row = document.querySelector('#discord-log > li[data-id="210"]');
      return !!row && row.textContent.includes('120'); }""",
               "the row does not say 120 replies on the first page")
    # Slice 2: the unread chip stands on Main from the FIRST Main answer, on
    # the root's own record — no summaries page has been read yet, and the
    # picker already says who last replied, from the same record.
    page.evaluate("() => document.getElementById('channel-view-main')?.click()")
    walk.until("""() => { const chip = document.querySelector('#discord-log > li[data-id="210"] .thread-unread');
      return !!chip && chip.textContent.trim() === '7 unread'; }""",
               "the unread chip is not the server's 7 from the first Main answer")
    reads = api.reads()
    walk.check(not any("view=threads" in read for read in reads),
               f"a summaries page was read before the chip stood: {reads}")
    walk.check(page.evaluate("() => document.querySelector('#thread-select')?.textContent.includes('last by grace')"),
               "the picker does not say who last replied from the first answer")

    # The event count: no thread read, no step back was spent on any of it.
    reads = api.reads()
    walk.check(not any("view=thread&" in read or "view=thread?" in read or "thread_id=" in read for read in reads),
               f"a thread read was spent reaching the counts: {reads}")
    walk.check(not any("before=" in read for read in reads),
               f"a step back was spent reaching the counts: {reads}")
    walk.shot("1-first-page-complete")

    # The thread list's card says who last replied and who is in the thread.
    # (Touching the picker first lands the thread directory behind it, as it
    # does for a person; the chip above needed none of it.)
    page.focus("#thread-select")
    page.evaluate("() => document.getElementById('channel-view-threads')?.click()")
    walk.until("""() => document.body.textContent.includes('last by grace')
      && document.body.textContent.includes('grace, ada')""",
               "the thread card does not say its last reply or participants")
    walk.until("() => document.querySelector('#thread-select')?.textContent.includes('7 unread') || document.body.textContent.includes('Thread list (7 unread)')",
               "the picker does not carry the channel summary's unread")
    reads = api.reads()
    walk.check(not any("before=" in read for read in reads),
               f"a step back was spent on the thread list: {reads}")
    walk.shot("2-thread-card")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"))
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        raise SystemExit("playwright is not installed: python3 -m playwright install chromium")
    sizes = (("phone", 412, 915, True), ("desk", 1280, 800, False))
    with sync_playwright() as playwright:
        for label, width, height, touch in sizes:
            api = OnePageApi()
            server = ThreadingHTTPServer(("127.0.0.1", 0), labels_handler(api))
            threading.Thread(target=server.serve_forever, daemon=True).start()
            origin = f"http://127.0.0.1:{server.server_port}"
            try:
                import tempfile
                with tempfile.TemporaryDirectory() as profile:
                    context = playwright.chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable,
                        viewport={"width": width, "height": height},
                        device_scale_factor=2.6 if touch else 1,
                        has_touch=touch, is_mobile=touch, color_scheme="dark")
                    browser_waits.hot_host_timeouts(context)
                    page = context.pages[0] if context.pages else context.new_page()
                    page.add_init_script(IOS_VIEWPORT_JS)
                    page.goto(origin)
                    walk = Walk(page, label, touch, args.screenshots)
                    walk_through(walk, api)
                    context.close()
                    print(f"{label}: one-page-complete walk passed")
            finally:
                server.shutdown()
    print("one page complete browser: all walks passed")


if __name__ == "__main__":
    main()
