#!/usr/bin/env python3
"""A reply's Sent receipt in the Threads view goes, and stays gone across a reload, in real Chromium at 1280x800.

`#500 sent-receipts-pile-up`. The owner, 2026-10-24, in the Threads view on a desk: "Some weird bug where
messages I sent stack up in the 'Sent.' cards at the bottom of the screen, and they survive even the page
restart." A receipt went only once a read carried its reply, which no read of that view does for a
thread's replies, and it was saved, so every reload drew it again. The fake-DOM suite
(tests/js/voice_page.test.mjs) pins the rule; this walks it in the browser engine, with real time and the
browser's own storage.

It serves the checked-out page and the fake API of tests/two_column_browser.py (24 messages, three
threads) from one loopback origin, and walks: the device holding three receipts an earlier build saved for
replies long confirmed -> sign in: none drawn, and the saved outbox has let them go -> the thread list,
Main channel among the cards, every thread unpacked -> a reply under an unpacked row, its post held: its
"Sending…" row directly under that row, not at the foot -> let go -> two more replies, into two threads ->
Pack all: the replies drawn nowhere in the view, their "Sent." receipts at the foot of the cards, as the
owner saw them, and none saved -> SENT_RECEIPT_MS later: no receipt anywhere -> a reload: none drawn, and
each reply in its thread. No page error at any step.

Pass --screenshots DIR to keep a PNG of each state; --web-root DIR to run against another copy of the page
(an older one fails it at the first step).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from inline_reply_browser import ReplyColumnsApi, sign_in
from offline_cache_browser import CHANNEL, check, handler_for, said, thread_of
from two_column_browser import CANARY, REPLICA, Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

WIDTH, HEIGHT = 1280, 800
OUTBOX_KEY = "vibe-talk.outgoing-messages"

# The receipts drawn, wherever they are: at the foot of the cards, or in place under a row.
RECEIPTS_JS = """() => [...document.querySelectorAll('.outgoing-message')]
    .filter((li) => li.getClientRects().length > 0)
    .map((li) => ({text: li.querySelector('.msg-text')?.textContent || '', state: li.getAttribute('data-send-state'),
                   inPlace: Boolean(li.closest('.outgoing-inplace')), foot: Boolean(li.closest('#outgoing-log')),
                   thread: Boolean(li.querySelector('.thread-badge'))}))"""


def receipt_ms(web_root: Path) -> int:
    """The page's SENT_RECEIPT_MS, read from the page served: the walk waits it out for real. A page
    without one (before #500) is given the 15 s this one has, and fails the walk by what it draws."""
    found = re.search(r"^const SENT_RECEIPT_MS = (\d+);$", (web_root / "voice.js").read_text(), re.MULTILINE)
    return int(found.group(1)) if found else 15000


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per state into (default: none)")
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
    with sync_playwright() as playwright:
        print(walk(playwright.chromium, args))
    return 0


def legacy_pile(api: ReplyColumnsApi) -> list[dict[str, object]]:
    """Three receipts as an earlier build saved them: replies "sent", with their parts, and no time."""
    pile = []
    for at, answers in enumerate(("212", "218", "214")):
        answered = next(m for m in api.messages if m["id"] == answers)
        part = {**said(60 + at, f"An old answer {at}, posted long ago.", "me", answers),
                "thread": {**thread_of(answered), "is_root": False}}
        pile.append({"id": f"outgoing-old-{at}", "channel": CHANNEL["id"], "text": part["content"],
                     "remaining": part["content"], "state": "sent", "threadId": thread_of(answered)["id"],
                     "replyTo": answers, "originView": "main", "createdAt": 1, "postedCount": 0,
                     "parts": [part], "observedParts": [], "detail": ""})
    return pile


def walk(chromium: BrowserType, args: argparse.Namespace) -> str:
    api = ReplyColumnsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    showing = receipt_ms(fixture.WEB_ROOT)
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-sent-receipts-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": WIDTH, "height": HEIGHT}, color_scheme="dark")
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, "sent-receipts", WIDTH, HEIGHT)
            origin = f"http://127.0.0.1:{server.server_port}/"
            page.goto(origin, wait_until="load")
            # The owner's device: receipts an earlier build saved, put where it kept them, then a load.
            page.evaluate("([key, value]) => localStorage.setItem(key, value)", [OUTBOX_KEY, json.dumps(legacy_pile(api))])
            page.reload(wait_until="load")
            receipts_walk(desk, api, showing)
            check(not errors, f"the page threw: {errors}")
            context.close()
            return f"{version} at {WIDTH}x{HEIGHT}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        if api.holding is not None:
            api.holding.set()
        server.shutdown()
        server.server_close()
        thread.join()


def receipts(desk: Desk) -> list[dict[str, object]]:
    found = desk.page.evaluate(RECEIPTS_JS)
    assert isinstance(found, list)
    return found


def saved_states(desk: Desk) -> list[str]:
    saved = desk.page.evaluate(f"() => JSON.parse(localStorage.getItem('{OUTBOX_KEY}') || '[]')")
    assert isinstance(saved, list)
    return [str(entry.get("state")) for entry in saved if isinstance(entry, dict)]


def unpack_to(desk: Desk, pressed: str) -> None:
    """Press Unpack until it says `pressed`: "true" Unpack all, "false" Pack all, "mixed" Unpack unread."""
    for _ in range(3):
        if desk.page.get_attribute("#side-unpack", "aria-pressed") == pressed:
            break
        desk.page.click("#side-unpack")
        desk.settled()
    desk.check(desk.page.get_attribute("#side-unpack", "aria-pressed") == pressed, f"Unpack did not come to {pressed}")


def reply_under(desk: Desk, answers: str, text: str) -> None:
    """Reply `text` from the box under the unpacked row of message `answers`."""
    page = desk.page
    row = f"#side-cards > li[data-unpack-thread][data-id='{answers}']"
    desk.wait_for(f"() => Boolean(document.querySelector(\"{row}\"))", f"message {answers} is not unpacked among the cards")
    page.evaluate(f"() => document.querySelector(\"{row}\").scrollIntoView({{block: 'center'}})")
    desk.settled()
    page.evaluate(f"() => document.querySelector(\"{row} .reply-button\").click()")
    desk.wait_for("() => !document.getElementById('side-reply').hidden", f"Reply on {answers} did not open among the cards")
    page.fill("#reply-text", text)
    page.click("#reply-send")


def receipts_walk(desk: Desk, api: ReplyColumnsApi, showing: int) -> None:
    page = desk.page
    sign_in(desk)
    desk.check(receipts(desk) == [], f"receipts an earlier build saved were drawn again: {receipts(desk)}")
    desk.check(saved_states(desk) == [], f"the saved outbox still holds the confirmed sends: {saved_states(desk)}")
    desk.notes.append("three receipts an earlier build saved for confirmed replies: none drawn, none kept")

    page.select_option("#thread-select", "threads")
    desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
                  " && document.querySelectorAll('#side-cards > li[data-card]').length === 3",
                  "the view picker did not show the thread list")
    page.click("#side-main")
    desk.wait_for("() => document.getElementById('channel-composer').parentNode.id === 'side-list'",
                  "Main channel did not put the main channel's box at the foot of the cards")
    unpack_to(desk, "true")

    # A reply going, from the box under an unpacked row: drawn under that row while it goes.
    api.holding = threading.Event()
    reply_under(desk, "212", "Held a moment on its way.")
    desk.wait_for("() => document.querySelector('.outgoing-inplace .outgoing-message') !== null",
                  "the reply going is not drawn under the row it answers")
    desk.settled()
    placed = page.evaluate("""() => {
        const row = document.querySelector("#side-cards > li[data-unpack-thread][data-id='212']").getBoundingClientRect();
        const group = document.querySelector('.outgoing-inplace').getBoundingClientRect();
        return {gap: group.top - row.bottom, left: group.left - row.left,
                foot: [...document.querySelectorAll('#outgoing-log > li')].length};
    }""")
    assert isinstance(placed, dict)
    desk.check(-1 <= float(str(placed["gap"])) <= 12, f"the reply going is not directly under the row it answers: {placed}")
    desk.check(placed["foot"] == 0, f"the reply going is at the foot of the cards as well: {placed}")
    desk.shot("1-going-in-place")
    desk.notes.append(f"a reply going drawn {float(str(placed['gap'])):.1f}px under the row it answers, nothing at the foot")
    api.holding.set()
    api.holding = None
    desk.wait_for("() => ![...document.querySelectorAll('.outgoing-message')].some((li) => li.getAttribute('data-send-state') === 'sending')",
                  "the held reply did not finish sending")

    # Two more replies, into two threads; then Pack all, which draws none of them: the owner's view.
    reply_under(desk, "218", "An answer in the canary thread.")
    desk.wait_for(f"() => document.querySelectorAll('#side-cards > li[data-unpack-thread=\"{CANARY}\"]').length > 0", "the cards went")
    reply_under(desk, "214", "An answer in the replica thread.")
    desk.wait_for(f"() => [...document.querySelectorAll('#side-cards > li[data-unpack-thread=\"{REPLICA}\"]')]"
                  ".some((li) => li.textContent.includes('An answer in the replica thread.'))",
                  "the last reply was not drawn among the cards")
    sent_at = time.monotonic()
    check(len(api.posted()) == 3, f"the server did not get three replies: {api.posted()}")
    unpack_to(desk, "false")
    desk.wait_for("() => document.querySelectorAll('#outgoing-log > li').length >= 2",
                  "Pack all left the replies drawn nowhere and said nothing of them")
    piled = receipts(desk)
    desk.check(all(one["state"] == "sent" and one["foot"] and one["thread"] for one in piled),
               f"the receipts at the foot are not the replies' Sent receipts: {piled}")
    desk.check(all(state != "sent" for state in saved_states(desk)), f"a confirmed send was saved: {saved_states(desk)}")
    page.evaluate("() => { const a = document.getElementById('side-scroll'); a.scrollTop = a.scrollHeight; }")
    desk.settled()
    desk.shot("2-sent-receipts-at-the-foot")

    # Their showing over, with no read of this view carrying any of them: gone.
    page.wait_for_timeout(max(0.0, showing / 1000 + 1.5 - (time.monotonic() - sent_at)) * 1000)
    desk.wait_for("() => document.querySelectorAll('.outgoing-message').length === 0",
                  f"the Sent receipts outlived their showing: {receipts(desk)}", seconds=5)
    desk.shot("3-receipts-gone")
    desk.notes.append(f"{len(piled)} Sent receipts at the foot of the packed cards, gone {showing / 1000:.0f} s on, none saved")

    # A reload: nothing to draw again, and each reply in its thread.
    page.reload(wait_until="load")
    desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-card]').length === 3",
                  "the reload did not come back to the thread list")
    desk.settled()
    desk.check(receipts(desk) == [], f"the reload drew receipts again: {receipts(desk)}")
    unpack_to(desk, "true")
    for thread_id, text in ((CANARY, "An answer in the canary thread."), (REPLICA, "An answer in the replica thread.")):
        desk.wait_for(f"() => [...document.querySelectorAll('#side-cards > li[data-unpack-thread=\"{thread_id}\"]')]"
                      f".some((li) => li.textContent.includes('{text}'))",
                      f"after the reload the reply \"{text}\" is not in its thread")
    desk.check(receipts(desk) == [], f"the replies are drawn twice after the reload: {receipts(desk)}")
    desk.shot("4-after-reload")
    desk.notes.append("after a reload no receipt, each reply drawn in its thread")


if __name__ == "__main__":
    raise SystemExit(main())
