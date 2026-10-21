#!/usr/bin/env python3
"""Every thread unpacked under its card in real Chromium, at 1280x800 and 393x852, dark and light.

`#404 threads-unpack-all` and `#405 root-under-summary`. The owner, 2026-10-21: in All, **N replies**
bridges one thread at a time and bridging another breaks the first; he wants every thread chained at
once. In the Threads view (Cmd+3), **Unpack all** (key u) draws each thread's messages under its card
in the bridged style, respecting Show / Collapse / Hide read; and under a summarized card the raw root
is the first child. The fake-DOM suite pins what is drawn; this proves where it is drawn and in what
colour, that the bridge is continuous, and what unpacking forty threads costs.

It serves the checked-out page and the fake API of tests/two_column_browser.py — three threads, one
summarised — with the summarised thread's root and first two replies, and one raw thread's first two
replies, already Done on the server.

At each size and scheme: sign in, All, the thread list from the picker -> three cards, Unpack all
offered and unpressed, no unpacked row (negative control) -> press it: every card gains its messages,
the summarised block starting with its root as a bridged child in the thread's colour, a raw block
with no root row (negative control), each bridge's spine left of its boxes and continuous over the
block -> Collapse read: each block's two read replies are one grey line joined to its spine -> Hide
read: read replies gone from every block, the summarised root still first -> Show read, Unpack all
off: cards alone again -> u (desk) or the chip (phone) on, reload: still on. No horizontal overflow
and no page error at any step.

Then, at the desk in the dark, a channel of forty threads: the time from pressing Unpack all to every
block drawn, and the row count, said in the notes.

Pass --screenshots DIR to keep a PNG of each state; --web-root DIR for another copy of the page.
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
from offline_cache_browser import Json, handler_for, said
from thread_card_header_browser import phone_thread_list
from two_column_browser import CANARY, REPLICA, RUNNER, ColumnsApi, Desk, thread_record

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

SIZES = (
    ("desk-1280-dark", 1280, 800, "dark", False),
    ("desk-1280-light", 1280, 800, "light", False),
    ("phone-393-dark", 393, 852, "dark", True),
    ("phone-393-light", 393, 852, "light", True),
)

# Done on the server: the summarised root and two replies of each of two threads.
DISMISSED = ["202", "204", "206", "210", "212"]


class UnpackApi(ColumnsApi):
    """ColumnsApi with a read run in the summarised thread and in a raw one."""

    def timeline(self, query: dict[str, list[str]]) -> Json:
        return {**super().timeline(query), "dismissed": DISMISSED}


class ManyApi(ColumnsApi):
    """Forty threads of a root and four replies, for the cost of unpacking them all."""

    def __init__(self) -> None:
        super().__init__()
        self.messages = []
        self.threads = []
        index = 0
        filler = "The overnight run is still going and the shard was retried. "
        for thread in range(40):
            thread_id = f"spaces/A/threads/m{thread}"
            root_at = index
            root = said(index, f"Thread {thread} root. {filler}", "coder")
            root["thread"] = thread_record(thread_id, index, 4)
            self.messages.append(root)
            index += 1
            last = root
            for reply in range(4):
                row = said(index, f"Thread {thread} reply {reply}. {filler}", "coder")
                row["thread"] = thread_record(thread_id, root_at, 4, is_root=False)
                self.messages.append(row)
                last = row
                index += 1
            self.threads.append({"id": thread_id, "root": root, "title": "Thread",
                                  "reply_count": 4, "reply_count_exact": True,
                                  "updated_at": last["timestamp"]})


STATE_JS = """(threadId) => {
    const box = (node) => { const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height}; };
    const rows = [...document.querySelectorAll(`#side-cards > li[data-unpack-thread="${threadId}"]`)];
    return rows.map((li) => {
        const bridge = li.querySelector(':scope > .reply-bridge');
        const run = li.querySelector('.read-run');
        return {ids: (li.getAttribute('data-ids') || '').split(' ').filter(Boolean),
                rootChild: li.hasAttribute('data-root-child'),
                coalesce: li.getAttribute('data-coalesce'),
                hidden: li.hidden, box: box(li),
                bridge: bridge ? {...box(bridge), color: getComputedStyle(bridge).color,
                                  spine: getComputedStyle(bridge).borderLeftColor} : null,
                run: run ? {...box(run), text: run.textContent} : null};
    });
}"""


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per state into")
    return parser.parse_args()


def serve(api: ColumnsApi) -> ThreadingHTTPServer:
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def open_page(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
              scheme: str, touch: bool, api: ColumnsApi) -> Desk:
    server = serve(api)
    profile = tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-unpack-")
    context = chromium.launch_persistent_context(
        profile.name, headless=True, executable_path=args.browser_executable,
        viewport={"width": width, "height": height}, color_scheme=scheme,  # type: ignore[arg-type]
        has_touch=touch, is_mobile=touch)
    page = context.pages[0] if context.pages else context.new_page()
    desk = Desk(page, args.screenshots, label, width, height)
    desk.page_errors = []  # type: ignore[attr-defined]
    page.on("pageerror", lambda error: desk.page_errors.append(str(error)))  # type: ignore[attr-defined]
    page.goto(f"http://127.0.0.1:{server.server_address[1]}/")
    return desk


def ids_of(desk: Desk, thread_id: str) -> list[str]:
    return [one for row in desk.page.evaluate(STATE_JS, thread_id) for one in row["ids"]]


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
         scheme: str, touch: bool) -> str:
    desk = open_page(chromium, args, label, width, height, scheme, touch, UnpackApi())
    page = desk.page
    phone_thread_list(desk)
    toggle = page.locator("#side-unpack")
    desk.check(toggle.is_visible(), "Unpack all is not offered over the thread list")
    desk.check(toggle.get_attribute("aria-pressed") == "false", "Unpack all starts pressed")
    desk.check(page.evaluate("() => document.querySelectorAll('#side-cards > li[data-unpack-thread]').length") == 0,
               "rows are unpacked before the toggle is pressed")
    desk.shot("1-packed")

    toggle.click()
    desk.wait_for(f"() => document.querySelectorAll('#side-cards > li[data-unpack-thread=\"{RUNNER}\"]').length >= 9",
                  "the summarised thread did not unpack under its card")
    desk.settled()
    desk.check(toggle.get_attribute("aria-pressed") == "true", "the toggle does not say it is pressed")
    runner = page.evaluate(STATE_JS, RUNNER)
    desk.check(ids_of(desk, RUNNER)[0] == "202" and runner[0]["rootChild"],
               f"the summarised block does not start with its root child: {runner[0]}")
    desk.check(all(row["bridge"] for row in runner), "an unpacked row has no bridge")
    card_edge = page.evaluate(
        f"() => getComputedStyle(document.querySelector('#side-cards > li[data-id=\"{RUNNER}\"]')).borderLeftColor")
    desk.check(runner[0]["bridge"]["spine"] == card_edge,
               f"the bridge {runner[0]['bridge']['spine']} is not the card edge {card_edge}")
    spines = [row["bridge"]["left"] for row in runner]
    desk.check(max(spines) - min(spines) <= 2, f"the bridge spine wanders: {spines}")
    desk.check(all(row["bridge"]["left"] < row["box"]["left"] + 40 for row in runner),
               "a bridge is not in its row's gutter")
    desk.check("209" not in ids_of(desk, REPLICA), "the raw block duplicates the root on its card")
    desk.check(not any(row["rootChild"] for row in page.evaluate(STATE_JS, REPLICA)),
               "a raw card grew a root child")
    desk.check(len(ids_of(desk, CANARY)) == 2, "the third thread did not unpack its replies")
    desk.shot("2-unpacked-show")

    page.click("#todo-filter")
    desk.wait_for("() => document.getElementById('todo-filter').getAttribute('data-read-mode') === 'collapse'",
                  "Collapse read was not reached")
    desk.settled()
    for thread_id in (RUNNER, REPLICA):
        runs = [row for row in page.evaluate(STATE_JS, thread_id) if row["run"]]
        desk.check(len(runs) == 1 and "2 read messages" in str(runs[0]["run"]["text"]),
                   f"{thread_id}: its read run is not one grey line: {runs}")
        desk.check(runs[0]["bridge"] is not None and runs[0]["run"]["left"] > runs[0]["bridge"]["left"],
                   "the run line hangs off no bridge")
    desk.shot("3-unpacked-collapse")

    page.click("#todo-filter")
    desk.wait_for("() => document.getElementById('todo-filter').getAttribute('data-read-mode') === 'hide'",
                  "Hide read was not reached")
    desk.settled()
    desk.check(not set(ids_of(desk, RUNNER)) & {"204", "206"}, "Hide read kept a read reply unpacked")
    desk.check(ids_of(desk, RUNNER)[0] == "202", "Hide read took the summarised root away")
    desk.check(not set(ids_of(desk, REPLICA)) & {"210", "212"}, "Hide read kept a raw read reply")
    desk.shot("4-unpacked-hide")

    page.click("#todo-filter")
    desk.wait_for("() => document.getElementById('todo-filter').getAttribute('data-read-mode') === 'show'",
                  "Show read was not reached")
    toggle.click()
    desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-unpack-thread]').length === 0",
                  "packing away left rows behind")
    desk.shot("5-packed-again")

    if touch:
        toggle.click()
    else:
        page.keyboard.press("u")
    desk.wait_for(f"() => document.querySelectorAll('#side-cards > li[data-unpack-thread=\"{RUNNER}\"]').length >= 9",
                  "u did not unpack again")
    page.reload()
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
                  f" && document.querySelectorAll('#side-cards > li[data-unpack-thread=\"{RUNNER}\"]').length >= 9",
                  "a reload forgot Unpack all")
    desk.shot("6-reloaded-unpacked")
    overflow = page.evaluate("() => document.documentElement.scrollWidth - window.innerWidth")
    desk.check(overflow <= 0, f"horizontal overflow of {overflow}px")
    desk.check(not desk.page_errors, f"page errors: {desk.page_errors}")  # type: ignore[attr-defined]
    page.context.close()
    return f"{label}: unpacked, collapsed, hidden, packed and reloaded, no page error"


def perf(chromium: BrowserType, args: argparse.Namespace) -> str:
    desk = open_page(chromium, args, "perf-1280-dark", 1280, 800, "dark", False, ManyApi())
    page = desk.page
    page.fill("#api-token", fixture.TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length >= 50", "All did not open")
    page.select_option("#thread-select", "threads")
    desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-card]').length === 40",
                  "forty cards were not listed")
    started = time.monotonic()
    page.click("#side-unpack")
    desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-unpack-thread]').length >= 160",
                  "forty threads did not unpack", seconds=30)
    desk.settled()
    elapsed = time.monotonic() - started
    rows = page.evaluate("() => document.querySelectorAll('#side-cards > li[data-unpack-thread]').length")
    desk.shot("1-forty-unpacked")
    page.context.close()
    return f"perf: 40 threads, {rows} unpacked rows drawn in {elapsed:.2f}s from the press"


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
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, scheme, touch in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, scheme, touch))
        report.append(perf(playwright.chromium, args))
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
