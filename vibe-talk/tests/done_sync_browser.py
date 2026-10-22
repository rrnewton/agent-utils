#!/usr/bin/env python3
"""A Done reaches every view at once, and a send's mark in the chat service waits for its Done, in real Chromium.

`#306 done-stale-across-views`. Found in review of #295 shift-e-read-collapse: a message marked Done in
one view still showed as not Done in another view loaded earlier in the session, until some later read
redrew it. `#311 sent-read-waits-for-done`: a send confirming while the Done of the message it answers
was still being saved moved the chat service's read marker through it at once, so a Done that then
failed left the message unread here and read in the service, whose marker cannot be moved back. The
fake-DOM suite (tests/js/voice_page.test.mjs) pins each path with negative controls; this shows what
only a browser engine can: what is painted, frame by frame, as a view is shown again, and what reaches
the server, and when, while a write is really held on the wire.

It serves the checked-out page and small fake APIs from one loopback origin.

  On a 412x915 phone, by touch, over the channel of tests/view_switch_browser.py: opened in All, Main
  chosen, then reopened in Main with no saved rows, so Main is read for itself, and All chosen and read
  behind the rows it is drawn from: each view now keeps its own copy of what is Done. Done tapped on a
  message in All -> Main chosen in the bar's picker: the message is drawn Done on every frame from the
  switch on, and no read is sent -> Undo in Main -> All chosen: drawn not Done on every frame, no read.

  At 1280x800 in two columns, over the channel of tests/reply_marks_read_browser.py, whose provider can
  move its read marker, every Done write held on the server until released: Done clicked on a Main
  message, then Reply on it, typed and sent with Enter -> the reply posts, and nothing is asked of the
  provider while the Done is held -> the Done refused -> still nothing asked, the message drawn unread,
  the failure said. Then the same on another message with the Done saved -> the provider is asked once,
  after the Done is saved, and the status line says it is marked in the chat service.

  `#327 done-races-remaining`: first of all, in All in one column, Done clicked on the canary thread's
  root and held; two columns turned on with the layout button, their cards on the right and Main on the
  left; and that thread opened on the right, for the first time in this page, from the root's Thread tag
  -> the root is drawn Done on the right on every frame from the first it shows there, and still once the
  Done is saved. (The page before this fix drew it as the thread's read did, not Done, and kept it so
  once the Done was saved.) Then the cards again, and the runner's thread selected on the right for the
  walk above.

No page error at any step. Pass --web-root DIR to run it against another copy of the page -- the page
before these fixes fails at the first switch, and at the first held Done -- and --screenshots DIR to
keep a PNG of each stage.
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
from urllib.parse import parse_qs, urlsplit

import offline_cache_browser as fixture
from offline_cache_browser import CACHE_KEY, TOKEN, Json, check, handler_for
from inline_reply_browser import place_row, reply_button
from reply_marks_read_browser import MarkerApi, drawn_read
from two_column_browser import CANARY, RUNNER, THREAD_ROWS, Desk
from view_switch_browser import PHONE, SwitchApi

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Route

WIDTH, HEIGHT = 1280, 800
# The message posted to the channel itself that the phone walk marks Done in All and finds in Main.
MARKED = "212"
# The Main messages the desk walk answers: the first while its Done is refused, the second saved.
REFUSED, SAVED = "203", "205"
# The canary thread's root on Main, marked Done with the Done held while its thread is opened on the right
# for the first time. `#327 done-races-remaining`.
OPENED = "215"
# Where a pressed row's top is put, below the top of its column's list, in px.
ROW_OFFSET_PX = 90
# How long a mark would take to reach the server, had one been asked: everything else here is
# answered within a few milliseconds on loopback.
QUIET_MS = 800


class DoneApi(SwitchApi):
    """`view_switch_browser.SwitchApi`, keeping each Done and Undo for the reads after it, as the server does."""

    def __init__(self) -> None:
        super().__init__()
        self.dismissed: set[str] = set()

    def post(self, path: str, body: Json) -> tuple[int, Json]:
        ids = body.get("messages")
        named = [str(one) for one in ids] if isinstance(ids, list) else []
        if path.endswith("/dismiss"):
            with self.lock:
                self.dismissed.update(named)
            return 200, {"dismissed": named}
        if path.endswith("/restore"):
            with self.lock:
                self.dismissed.difference_update(named)
            return 200, {"restored": named}
        return super().post(path, body)

    def timeline(self, query: dict[str, list[str]]) -> Json:
        answer = super().timeline(query)
        rows = answer.get("messages")
        with self.lock:
            done = [str(m["id"]) for m in rows if isinstance(m, dict) and str(m["id"]) in self.dismissed] \
                if isinstance(rows, list) else []
        return {**answer, "dismissed": done}

    def is_done(self, message_id: str) -> bool:
        with self.lock:
            return message_id in self.dismissed


class HeldDoneApi(MarkerApi):
    """`reply_marks_read_browser.MarkerApi`, holding each Done write on the wire until the walk releases
    it, then saving it or refusing it as the server's store would; "saved" goes into the writes when a
    held Done is saved, so a mark's place after it is an observation."""

    def __init__(self) -> None:
        super().__init__()
        self.release = threading.Event()
        self.refuse = False
        self.held = 0

    def post(self, path: str, body: Json) -> tuple[int, Json]:
        if not path.endswith("/dismiss"):
            return super().post(path, body)
        with self.lock:
            self.held += 1
            self.writes.append("dismiss")
        self.release.wait(20)
        self.release.clear()
        if self.refuse:
            return 502, {"error": "store_error", "detail": "the store is read-only"}
        ids = body.get("messages")
        done = [str(one) for one in ids] if isinstance(ids, list) else []
        with self.lock:
            self.dismissed.update(done)
            self.writes.append("saved")
        return 200, {"dismissed": done}

    def marked_after_saved(self, message_id: str) -> bool:
        """Whether the provider's mark for `message_id` came after the last Done saved, and after no other."""
        with self.lock:
            writes = list(self.writes)
            marks = [str(mark.get("message_id")) for mark in self.marks]
        if marks.count(message_id) != 1:
            return False
        at = [index for index, write in enumerate(writes) if write == "upstream-read"][marks.index(message_id)]
        return "saved" in writes[:at]


# From now on, on every frame and on every change to the list: the view shown and how row `id` is drawn
# in it (its `data-archived`, or null where no row holds it). A list drawn and drawn again inside one
# task is never painted, and never seen here.
WATCH_JS = """(id) => {
    const seen = window.__doneSeen = [];
    const log = document.getElementById('discord-log');
    const look = () => {
        const row = [...log.querySelectorAll(':scope > li[data-id]')]
            .find((li) => (li.getAttribute('data-ids') || li.getAttribute('data-id')).split(' ').includes(id));
        const state = [document.getElementById('thread-select').value, row ? row.getAttribute('data-archived') : null];
        const last = seen[seen.length - 1];
        if (!last || last[0] !== state[0] || last[1] !== state[1]) seen.push(state);
    };
    new MutationObserver(look).observe(log, {childList: true, subtree: true, attributes: true, attributeFilter: ['data-archived']});
    const frame = () => { look(); requestAnimationFrame(frame); };
    requestAnimationFrame(frame);
}"""


# From now on, on every frame and on every change to the right column's list: how row `id` is drawn there
# (its `data-archived`, or null while no row holds it). `#327 done-races-remaining`.
SIDE_WATCH_JS = """(id) => {
    const seen = window.__sideSeen = [];
    const log = document.getElementById('side-log');
    const look = () => {
        const row = log.querySelector(`:scope > li[data-id="${id}"]`);
        const state = row ? row.getAttribute('data-archived') : null;
        if (!seen.length || seen[seen.length - 1] !== state) seen.push(state);
    };
    new MutationObserver(look).observe(log, {childList: true, subtree: true, attributes: true, attributeFilter: ['data-archived']});
    const frame = () => { look(); requestAnimationFrame(frame); };
    requestAnimationFrame(frame);
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
        help="directory to write one PNG per stage into (default: none are kept)",
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
    # The fake APIs' handler serves the page from this module attribute.
    fixture.WEB_ROOT = args.web_root.resolve()
    with sync_playwright() as playwright:
        phone = views_walk(playwright.chromium, args)
        desk = sends_walk(playwright.chromium, args)
    print(f"{phone}; {desk}")
    return 0


def views_walk(chromium: BrowserType, args: argparse.Namespace) -> str:
    """`#306`: a Done in All and an Undo in Main, each drawn in the other view from the first frame it shows."""
    label, width, height = PHONE
    api = DoneApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-done-views-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, device_scale_factor=2.625,
                is_mobile=True, has_touch=True,
            )
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            # Every read of a view of the channel. While `fail_counts_read` is set, the read of All that a
            # read of Main asks for behind it, for its roots' reply counts (`#222 unread-replies`), is
            # failed, so that All is first read as All, when it is chosen.
            reads: list[str] = []
            fail_counts_read = threading.Event()

            def counted(route: Route) -> None:
                query = parse_qs(urlsplit(route.request.url).query)
                view = query.get("view", [""])[0]
                if view == "flat" and "before" not in query and fail_counts_read.is_set():
                    fail_counts_read.clear()
                    route.abort()
                    return
                if view != "threads":
                    reads.append(view)
                route.continue_()

            page.route(lambda address: "/timeline" in address, counted)

            def shot(name: str) -> None:
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / f"{label}-done-views-{name}.png"))

            def rows() -> list[str]:
                return [str(row) for row in page.eval_on_selector_all(
                    "#discord-log > li[data-id]", "items => items.map(i => i.getAttribute('data-id'))")]

            def wait_until(test: object, why: str, seconds: float = 10) -> None:
                assert callable(test)
                deadline = time.monotonic() + seconds
                while time.monotonic() < deadline and not test():
                    page.wait_for_timeout(25)
                check(bool(test()), f"{label}: {why}")

            def settled() -> None:
                page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")

            def drawn(message_id: str) -> str | None:
                found = page.evaluate(f"() => {{ const li = document.querySelector('#discord-log > li[data-id=\"{message_id}\"]');"
                                      " return li ? li.getAttribute('data-archived') : null; }")
                return str(found) if found is not None else None

            def pick(view: str) -> None:
                page.select_option("#thread-select", view)

            def switch_seen(view: str, expected: str, why: str) -> None:
                """`view` chosen with the watch on: every state seen once it shows has the row drawn `expected`."""
                page.evaluate(WATCH_JS, MARKED)
                settled()
                before = len(reads)
                pick(view)
                wait_until(lambda: rows() == api.ids(view), f"{view} was not drawn")
                settled()
                settled()
                seen = [(str(v), s) for v, s in page.evaluate("() => window.__doneSeen")]
                shown = [state for v, state in seen if v == view]
                check(bool(shown) and all(state == expected for state in shown),
                      f"{label}: {why}: {MARKED} drawn {shown} in {view} from the switch on (all seen: {seen})")
                check(reads[before:] == [], f"{label}: {why}: the switch read {reads[before:]}")

            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            page.click("#view-switch")
            wait_until(lambda: rows() == api.ids("flat"), "the channel did not open in All")
            pick("main")
            wait_until(lambda: rows() == api.ids("main"), "Main was not drawn")
            # Reopened in Main with no saved rows: Main is read for itself, then All behind its held rows.
            page.evaluate(f"() => localStorage.removeItem({json.dumps(CACHE_KEY)})")
            fail_counts_read.set()
            reads.clear()
            page.reload(wait_until="load")
            wait_until(lambda: rows() == api.ids("main"), "the reload did not reopen on Main")
            wait_until(lambda: not fail_counts_read.is_set(), "reopening on Main did not ask for All behind it")
            pick("flat")
            wait_until(lambda: rows() == api.ids("flat") and reads == ["main", "flat"], f"All was not read as All: {reads}")
            settled()

            # Done, tapped in All.
            page.locator(f'#discord-log > li[data-id="{MARKED}"] .done-button').tap()
            wait_until(lambda: api.is_done(MARKED) and drawn(MARKED) == "true", f"{MARKED} was not marked Done in All")
            shot("1-done-in-all")
            switch_seen("main", "true", "a Done made in All")
            shot("2-main")

            # Undo, in Main.
            page.locator("#undo-dismiss").tap()
            wait_until(lambda: not api.is_done(MARKED) and drawn(MARKED) == "false", f"Undo did not put {MARKED} back in Main")
            switch_seen("flat", "false", "an Undo made in Main")
            shot("3-all")
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return (f"{version} {label} at {width}x{height}: a Done tapped in All drawn in Main, and an Undo in Main"
                    " drawn in All, each from the first frame the view showed, with no read")
    finally:
        api.timeline_gate.set()
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def sends_walk(chromium: BrowserType, args: argparse.Namespace) -> str:
    """`#311`: a Reply sent while its message's Done is held asks the provider nothing until that Done is saved."""
    api = HeldDoneApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-done-sends-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": WIDTH, "height": HEIGHT}, color_scheme="dark",
            )
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"done-sends-{WIDTH}", WIDTH, HEIGHT)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            open_all(desk)
            held_open(desk, api)
            open_runner(desk)
            held_send(desk, api, REFUSED, refuse=True)
            page.click("#dismiss-error")
            held_send(desk, api, SAVED, refuse=False)
            check(not errors, f"the page threw: {errors}")
            context.close()
            return f"{version} at {WIDTH}x{HEIGHT}: " + "; ".join(desk.notes)
    finally:
        api.release.set()
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def held_send(desk: Desk, api: HeldDoneApi, message_id: str, refuse: bool) -> None:
    """Done clicked on `message_id`, held; a Reply on it sent; the Done then refused or saved."""
    page = desk.page
    where = f"Reply while the Done of {message_id} is held, then {'refused' if refuse else 'saved'}"
    api.refuse = refuse
    place_row(desk, "left", message_id, top=ROW_OFFSET_PX)
    desk.check(not drawn_read(desk, "left", message_id), f"{where}: the premise is an unread message")
    held = api.held
    page.locator(f"#discord-log > li[data-id='{message_id}'] .done-button").click()
    desk.wait_for(f"() => document.querySelector(\"#discord-log > li[data-id='{message_id}']\").getAttribute('data-archived') === 'true'",
                  f"{where}: the message was not drawn Done at once")
    for _ in range(200):
        if api.held > held:
            break
        page.wait_for_timeout(25)
    desk.check(api.held == held + 1, f"{where}: the Done did not reach the server")
    button = reply_button(desk, "left", message_id)
    page.mouse.click(button["x"], button["y"])
    desk.wait_for("() => !document.getElementById('column-reply').hidden",
                  f"{where}: the box did not open in the column")
    before = len(api.posted())
    page.keyboard.type(f"answering {message_id} while its Done is saved")
    page.keyboard.press("Enter")
    for _ in range(200):
        if len(api.posted()) > before:
            break
        page.wait_for_timeout(25)
    posted = api.posted()
    desk.check(len(posted) == before + 1 and posted[-1].get("reply_to") == message_id,
               f"{where}: the reply did not post as a reply to {message_id}: {posted[before:]}")
    page.wait_for_timeout(QUIET_MS)
    desk.check(message_id not in api.marked(), f"{where}: the provider was asked while the Done was held: {api.marked()}")
    desk.shot(f"{message_id}-held")
    api.release.set()
    if refuse:
        desk.wait_for("() => !document.getElementById('error').hidden", f"{where}: the refused Done was not said")
        desk.wait_for(f"() => document.querySelector(\"#discord-log > li[data-id='{message_id}']\").getAttribute('data-archived') === 'false'",
                      f"{where}: the refused Done left the message drawn Done")
        page.wait_for_timeout(QUIET_MS)
        desk.check(message_id not in api.marked(), f"{where}: the provider was asked after the Done was refused: {api.marked()}")
        desk.check(not page.evaluate("() => document.getElementById('status').textContent.startsWith('Marked read here and in')"),
                   f"{where}: a refused Done was said as a read in the chat service")
        desk.shot(f"{message_id}-refused")
        desk.notes.append(f"{message_id}: the reply posted, nothing asked of the provider while its Done was held nor after it was refused")
        return
    for _ in range(200):
        if message_id in api.marked():
            break
        page.wait_for_timeout(25)
    desk.check(api.marked_after_saved(message_id), f"{where}: the provider was not asked once, after the Done was saved: {api.writes}")
    desk.check(api.all_after_reply(), f"{where}: the mark was asked as the owner's own, not after a reply")
    desk.wait_for("() => document.getElementById('status').textContent.startsWith('Marked read here and in Discord.')",
                  f"{where}: the status line does not say the message is marked in the chat service")
    desk.check(drawn_read(desk, "left", message_id), f"{where}: the message is not drawn read")
    desk.shot(f"{message_id}-saved")
    desk.notes.append(f"{message_id}: the reply posted, the provider asked once, only after its held Done was saved")



def open_all(desk: Desk) -> None:
    """Signed in, on the channel, in All in one column."""
    page = desk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24",
                  "the channel did not open in All")
    desk.settled()


def open_runner(desk: Desk) -> None:
    """From a thread on the right back to the cards, and the runner's thread selected there."""
    page = desk.page
    page.click("#side-close")
    desk.wait_for("() => document.querySelectorAll('#side-cards > li').length === 3"
                  " && document.getElementById('side-cards').getClientRects().length > 0",
                  "the X did not bring the cards back")
    page.click(f"#side-cards > li[data-id='{RUNNER}'] .side-card-open")
    desk.wait_for(f"() => document.querySelectorAll('#side-log > li[data-id]').length === {THREAD_ROWS[RUNNER]}",
                  "the runner's card did not select its thread on the right")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 11", "the left column is not Main")
    page.mouse.move(WIDTH / 2, 5)
    desk.settled()


def held_open(desk: Desk, api: HeldDoneApi) -> None:
    """`#327`: Done clicked on the canary root in All, held; two columns on; its thread opened on the right
    for the first time."""
    page = desk.page
    where = f"the canary thread opened on the right while the Done of {OPENED}, made in All, is held"
    api.refuse = False
    page.evaluate(f"""() => document.querySelector("#discord-log > li[data-id='{OPENED}']").scrollIntoView({{block: 'center'}})""")
    desk.settled()
    desk.check(not drawn_read(desk, "left", OPENED), f"{where}: the premise is an unread root")
    held = api.held
    page.locator(f"#discord-log > li[data-id='{OPENED}'] .done-button").click()
    for _ in range(200):
        if api.held > held:
            break
        page.wait_for_timeout(25)
    desk.check(api.held == held + 1, f"{where}: the Done did not reach the server")
    page.click("#column-toggle")
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length > 0"
                  " && document.querySelectorAll('#side-cards > li').length === 3"
                  " && document.querySelectorAll('#discord-log > li[data-id]').length === 11",
                  f"{where}: the layout button did not give two columns with the cards, Main on the left")
    page.mouse.move(WIDTH / 2, 5)
    desk.settled()
    desk.check(page.evaluate(f"() => !document.querySelector(\"#side-log > li[data-id='{OPENED}']\")"),
               f"{where}: the premise is the thread not on the right")
    place_row(desk, "left", OPENED, top=ROW_OFFSET_PX)
    page.evaluate(SIDE_WATCH_JS, OPENED)
    desk.settled()
    page.locator(f"#discord-log > li[data-id='{OPENED}'] .thread-badge").click()
    desk.wait_for(f"() => document.querySelectorAll('#side-log > li[data-id]').length === {THREAD_ROWS[CANARY]}",
                  f"{where}: the thread did not open on the right")
    desk.settled()
    desk.settled()
    seen = page.evaluate("() => window.__sideSeen")
    shown = [state for state in seen if state is not None] if isinstance(seen, list) else []
    desk.check(bool(shown) and all(state == "true" for state in shown),
               f"{where}: {OPENED} drawn {seen} on the right from the first frame it showed, not Done")
    desk.shot(f"{OPENED}-opened-held")
    api.release.set()
    for _ in range(200):
        if OPENED in api.done():
            break
        page.wait_for_timeout(25)
    desk.check(OPENED in api.done(), f"{where}: the Done was not saved")
    page.wait_for_timeout(QUIET_MS)
    desk.settled()
    seen = page.evaluate("() => window.__sideSeen")
    shown = [state for state in seen if state is not None] if isinstance(seen, list) else []
    desk.check(bool(shown) and all(state == "true" for state in shown),
               f"{where}: {OPENED} drawn {seen} on the right once the Done was saved, not Done")
    desk.check(drawn_read(desk, "right", OPENED) and drawn_read(desk, "left", OPENED),
               f"{where}: {OPENED} is not drawn read in both columns once the Done was saved")
    desk.shot(f"{OPENED}-opened-saved")
    desk.notes.append(f"{OPENED}: drawn Done on the right from the first frame its thread showed there while its Done, made in All, was held,"
                      " and once it was saved")


if __name__ == "__main__":
    raise SystemExit(main())
