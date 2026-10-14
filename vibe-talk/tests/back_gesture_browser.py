#!/usr/bin/env python3
"""The phone's back gesture inside the app, in real Chromium emulating an installed Android app.

`#304 back-gesture-in-app`. The owner, 2026-10-14: on the installed Android app the system back
gesture inside a thread left the app for the home screen; only the on-screen Back closed the thread.
The gesture is the browser's Back, and the page had no history entry of its own for it to go back
through. The page now keeps one for each thing open over the channel, and a popstate closes the top
one as its own Back does. The fake-DOM suite (tests/js/voice_page.test.mjs) pins that against a fake
session history; this drives Chromium's own, with `page.go_back()` (the browser's Back, which is what
the gesture sends) and `history.back()`.

It serves the checked-out page and the fake API of tests/two_column_browser.py -- a channel taller than
the window with three threads -- from one loopback origin, at a 412x915 Android phone: touch, a mobile
Chrome user agent, a 2.625 device pixel ratio, and the standalone display mode an installed app has.
Chromium's DevTools protocol accepts a `display-mode` media override and ignores it (measured on
Chromium 149), so `(display-mode: standalone)` is answered by an init script, and the walk checks that
the page sees it; the page itself reads no display mode, so this is the environment, not a switch.
A plain page is opened first, as the home screen the app was launched from: going back to it is
leaving the app.

  sign in and open the channel in All, scrolled to a reader's place mid-list -> a thread root's tag:
  the thread, and one history entry more, at the same address -> the browser's Back: the thread
  closes, the app is not navigated away from (the same document, at /voice), and All is back at the
  reader's place, its first row and that row's offset within a pixel -> the thread list from the
  view picker: one entry -> a card's replies: its thread across the window, one more -> history.back():
  the thread list again -> the browser's Back: All, at the reader's place, with no entry of the
  page's above its own -> the ⋯ menu of a row: no entry, as Chromium has close watchers -> a thread
  again, closed by its on-screen Back: the page goes back through the thread's entry itself -> so the
  browser's Back at the top leaves the app, for the page it was opened from.

Then, in a fresh profile: a thread open and the page reloaded on its entry -> the thread reopened
(`#189 restore-ui-state`) on an entry that is this load's own base, and no entry pushed before the
reader touches the page, as Chromium's back button skips one pushed earlier -> a tap: the thread's
entry -> the browser's Back: the reopened thread closes, and the app is not navigated away from -> the
browser's Back at the top: it lands on the entry the earlier load opened on (Chromium keeps it, and
lands on it without a load), and the page goes on back past it, so the one press leaves the app.

No page error at any step. Pass --screenshots DIR to keep a PNG of each state for review; --web-root
DIR to run the same walk against another copy of the page.
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING, Callable

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, Json, check, handler_for
from two_column_browser import REPLICA, THREAD_ROWS, ColumnsApi, Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, BrowserType, Page

WIDTH, HEIGHT = 412, 915
ANDROID_CHROME = ("Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) "
                  "Chrome/149.0.0.0 Mobile Safari/537.36")
# What was open before the app: a page of its own, so going back to it is visibly leaving the app.
HOME = "data:text/html,<title>home screen</title><p>home screen</p>"
# The root whose tag opens its thread, and the thread it opens.
ROOT_ID = "209"

# `(display-mode: standalone)` answered as an installed app's window answers it, where the browser
# offers no override for it; every other query is the browser's own.
STANDALONE_JS = """(() => {
  const own = window.matchMedia.bind(window);
  window.matchMedia = (query) => {
    const asked = String(query).replace(/\\s+/g, '');
    if (!asked.includes('display-mode')) return own(query);
    const answer = own('(min-width: 0px)');
    const matches = asked.includes('display-mode:standalone');
    return new Proxy(answer, {get: (target, key) => key === 'matches' ? matches
      : key === 'media' ? String(query) : Reflect.get(target, key)});
  };
})();"""

# The page's own facts on the entry the browser is on, and what is on screen.
STATE_JS = """() => {
  const facts = history.state && history.state.vibeTalkBack;
  return {
    depth: facts ? facts.depth : null, tag: facts ? facts.tag : null, session: facts ? facts.session : null,
    path: location.pathname, url: location.href, marker: window.__backWalk || null,
    thread: !document.getElementById('thread-heading').hidden,
    layout: document.getElementById('screen-main').getAttribute('data-columns'),
    picker: document.getElementById('thread-select').value,
  };
}"""

# Where the reader is in the channel's list: its first row on screen and how far that row's top is
# below the list's.
PLACE_JS = """() => {
  const area = document.getElementById('scroll-area');
  const top = area.getBoundingClientRect().top;
  const rows = [...document.querySelectorAll('#discord-log > li[data-id]')].filter((row) => row.getClientRects().length);
  const first = rows.find((row) => row.getBoundingClientRect().bottom > top + 1);
  return first ? {id: first.getAttribute('data-id'), offset: first.getBoundingClientRect().top - top,
                  scrollTop: area.scrollTop} : null;
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
    report: list[str] = []
    with sync_playwright() as playwright:
        report.append(walk(playwright.chromium, args, "phone-412", gesture_walk))
        report.append(walk(playwright.chromium, args, "phone-412-reload", reload_walk))
    for line in report:
        print(line)
    return 0


def phone_context(chromium: BrowserType, args: argparse.Namespace, profile: str) -> BrowserContext:
    """An Android phone's Chrome running the installed app: its viewport, touch, user agent and display mode."""
    context = chromium.launch_persistent_context(
        profile, headless=True, executable_path=args.browser_executable,
        viewport={"width": WIDTH, "height": HEIGHT}, color_scheme="dark",
        device_scale_factor=2.625, is_mobile=True, has_touch=True, user_agent=ANDROID_CHROME,
    )
    context.add_init_script(STANDALONE_JS)
    return context


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, steps: Callable[[Desk], None]) -> str:
    """One reader, against a fresh fake API and a fresh browser profile."""
    api = ColumnsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-back-gesture-") as profile:
            context = phone_context(chromium, args, profile)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, label, WIDTH, HEIGHT)
            page.goto(HOME, wait_until="load")
            page.goto(f"http://127.0.0.1:{server.server_port}/voice", wait_until="load")
            steps(desk)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {WIDTH}x{HEIGHT}, {label}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


# --- reading the page ---------------------------------------------------------------------------------


def state(page: Page) -> Json:
    found: Json = page.evaluate(STATE_JS)
    return found


def place(page: Page) -> Json:
    found = page.evaluate(PLACE_JS)
    check(isinstance(found, dict), "no row of the channel is on screen")
    assert isinstance(found, dict)
    return found


def number(found: Json, key: str) -> float:
    value = found[key]
    assert isinstance(value, (int, float)), f"{key} is {value!r}, not a number"
    return float(value)


def history_index(page: Page) -> int:
    """Where the browser is in the tab's history, as its DevTools protocol says."""
    cdp = page.context.new_cdp_session(page)
    try:
        found = cdp.send("Page.getNavigationHistory")
    finally:
        cdp.detach()
    return int(found["currentIndex"])


def mark_document(page: Page) -> None:
    """Mark the document that is loaded: a navigation away, or a reload, takes the mark with it."""
    page.evaluate("() => { window.__backWalk = 'this document'; }")


def at_entry(desk: Desk, depth: int, tag: str, why: str) -> Json:
    """Wait until the browser is on the page's entry `depth` deep, standing for `tag`; return the state there."""
    desk.wait_for(f"() => {{ const f = history.state && history.state.vibeTalkBack;"
                  f" return !!f && f.depth === {depth} && f.tag === '{tag}'; }}", why)
    found = state(desk.page)
    desk.check(found["path"] == "/voice", f"the address changed to {found['url']}")
    desk.check(found["marker"] == "this document", f"{why}: the app was navigated away from, or reloaded")
    return found


def same_place(desk: Desk, before: Json, where: str) -> None:
    now = place(desk.page)
    desk.check(now["id"] == before["id"] and abs(number(now, "offset") - number(before, "offset")) <= 1,
               f"{where}: the reader was at {before}, and is at {now}")


def left_app(desk: Desk, why: str) -> None:
    """Wait for the page the app was opened from: the app left. By the address, as the app's document is gone."""
    from playwright.sync_api import TimeoutError as Timeout

    try:
        desk.page.wait_for_url(lambda url: url.startswith("data:"), timeout=10_000)
    except Timeout as error:
        raise AssertionError(f"{desk.label}: {why}: still at {desk.page.url}") from error


def tap(desk: Desk, selector: str) -> None:
    box = desk.page.locator(selector).first.bounding_box()
    desk.check(box is not None, f"{selector} is not drawn")
    assert box is not None
    desk.page.touchscreen.tap(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)


def open_channel(desk: Desk) -> None:
    page = desk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    tap(desk, "#view-switch")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24",
                  "the channel did not open in All")
    desk.settled()


def open_root_thread(desk: Desk) -> None:
    """Bring the root to the screen, if it is not, and tap its tag: its thread, in one column."""
    page = desk.page
    page.evaluate(f"""() => {{
        const row = document.querySelector("#discord-log > li[data-id='{ROOT_ID}']");
        const area = document.getElementById('scroll-area');
        const box = row.getBoundingClientRect(), frame = area.getBoundingClientRect();
        if (box.top < frame.top || box.bottom > frame.bottom) area.scrollTop += box.top - frame.top - 120;
    }}""")
    desk.settled()
    tap(desk, f"#discord-log > li[data-id='{ROOT_ID}'] .thread-badge")
    desk.wait_for("() => !document.getElementById('thread-heading').hidden"
                  f" && document.querySelectorAll('#discord-log > li[data-id]').length === {THREAD_ROWS[REPLICA]}",
                  "the root's tag did not open its thread")


# --- the walk -----------------------------------------------------------------------------------------


def gesture_walk(desk: Desk) -> None:
    page = desk.page
    desk.check(bool(page.evaluate("() => matchMedia('(display-mode: standalone)').matches")),
               "the page does not see the standalone display mode of an installed app")
    open_channel(desk)
    mark_document(page)
    top = state(page)
    base_index = history_index(page)

    # The reader's place, mid-list: the root on screen, with rows above it.
    page.evaluate(f"""() => {{
        const row = document.querySelector("#discord-log > li[data-id='{ROOT_ID}']");
        const area = document.getElementById('scroll-area');
        area.scrollTop += row.getBoundingClientRect().top - area.getBoundingClientRect().top - 160;
    }}""")
    desk.settled()
    desk.settled()
    reader = place(page)
    desk.check(number(reader, "scrollTop") > 0, f"fixture: the reader's place is the top of the list: {reader}")

    # A thread, by its root's tag, and the browser's Back in it. What the thread's opening did to the
    # history is checked after the Back, so that a page making no entry fails where the reader would
    # see it: at the Back, which leaves the app.
    open_root_thread(desk)
    opened = state(page)
    opened_index = history_index(page)
    desk.shot("1-thread")
    page.go_back()
    desk.check(not page.url.startswith("data:"), "the browser's Back in a thread left the app for the page before it")
    desk.wait_for("() => document.getElementById('thread-heading').hidden", "the browser's Back did not close the thread")
    desk.check(top["depth"] == 0 and top["tag"] == "top", f"the page did not make the entry it opened on its base: {top}")
    desk.check(opened["depth"] == 1 and opened["tag"] == "thread",
               f"the thread opened is not one entry above the page's base: {opened}")
    desk.check(opened["session"] == top["session"], "the thread's entry is another page load's")
    desk.check(opened["path"] == "/voice", f"the thread's entry changed the address to {opened['url']}")
    desk.check(opened_index == base_index + 1, "the thread did not add exactly one entry to the tab's history")
    back = at_entry(desk, 0, "top", "the browser's Back in the thread did not land on the page's base")
    desk.check(back["picker"] == "flat", f"the browser's Back did not go back to All: {back['picker']}")
    desk.settled()
    desk.settled()
    same_place(desk, reader, "after the browser's Back in the thread")
    desk.shot("2-back-in-all")
    desk.notes.append(f"a thread opened from its root was one entry; the browser's Back closed it in the same document, "
                      f"All back on row {reader['id']} at {number(reader, 'offset'):.0f}px")

    # The thread list from the view picker, a card's thread across the window, and back twice.
    page.select_option("#thread-select", "threads")
    desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
                  " && document.querySelectorAll('#side-cards > li[data-card]').length === 3",
                  "the view picker did not show the thread list")
    at_entry(desk, 1, "list", "the thread list is not one entry above the page's base")
    desk.settled()
    tap(desk, f"#side-cards > li[data-id='{REPLICA}'] .side-card-open")
    desk.wait_for("() => !document.getElementById('thread-heading').hidden"
                  " && !document.getElementById('screen-main').hasAttribute('data-columns')",
                  "the card's replies did not open its thread across the window")
    at_entry(desk, 2, "thread", "the thread opened from the list is not one entry above it")
    desk.shot("3-thread-from-list")
    page.evaluate("() => history.back()")
    desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
                  " && document.getElementById('thread-heading').hidden",
                  "history.back() in the thread did not go back to the thread list")
    at_entry(desk, 1, "list", "history.back() did not land on the thread list's entry")
    desk.shot("4-back-to-list")
    page.go_back()
    desk.wait_for("() => !document.getElementById('screen-main').hasAttribute('data-columns')",
                  "the browser's Back on the thread list did not leave it")
    out = at_entry(desk, 0, "top", "the browser's Back on the thread list did not land on the page's base")
    desk.check(out["picker"] == "flat" and not out["thread"], f"the thread list went back to {out['picker']}")
    desk.settled()
    desk.settled()
    same_place(desk, reader, "after the thread list")
    desk.check(history_index(page) == base_index, "the page is not back on its own entry")
    desk.notes.append("the thread list and a thread opened from it were two entries: history.back() came back to "
                      "the list, the browser's Back to All at the reader's place")

    # The ⋯ menu: Chromium has close watchers, so it is no entry.
    page.evaluate("() => { window.__length = history.length; }")
    tap(desk, f"#discord-log > li[data-id='{ROOT_ID}'] .row-more-button")
    desk.wait_for(f"() => !document.querySelector(\"#discord-log > li[data-id='{ROOT_ID}'] .row-more-menu\").hidden",
                  "the ⋯ did not open its menu")
    desk.settled()
    desk.check(bool(page.evaluate("() => typeof CloseWatcher === 'function' && history.length === window.__length"
                                  " && history.state.vibeTalkBack.depth === 0")),
               "the ⋯ menu pushed an entry where the browser has close watchers")
    tap(desk, f"#discord-log > li[data-id='{ROOT_ID}'] .row-more-button")
    desk.wait_for(f"() => document.querySelector(\"#discord-log > li[data-id='{ROOT_ID}'] .row-more-menu\").hidden",
                  "the ⋯ did not close its menu")
    desk.settled()
    desk.check(state(page)["depth"] == 0, "the ⋯ menu closed left an entry")

    # A thread closed by its on-screen Back: the page goes back through the thread's entry itself.
    open_root_thread(desk)
    at_entry(desk, 1, "thread", "the thread opened again is not one entry")
    tap(desk, "#thread-back")
    desk.wait_for("() => document.getElementById('thread-heading').hidden", "the on-screen Back did not close the thread")
    at_entry(desk, 0, "top", "the on-screen Back did not go back through the thread's entry")
    desk.check(history_index(page) == base_index, "the on-screen Back left the browser off the page's own entry")
    desk.notes.append("the ⋯ menu was no entry (close watchers); the on-screen ← Back went back through the thread's entry")

    # At the top, the browser's Back leaves the app for the page it was opened from.
    page.go_back()
    left_app(desk, "the browser's Back at the top did not leave the app")
    desk.notes.append("at the top the browser's Back left the app for the page before it")


class Quiet:
    """Reads of the page through the DevTools protocol with no user gesture. Playwright's own `evaluate`
    hands the page a user activation, which is the very thing the reload step watches the page wait for."""

    def __init__(self, page: Page) -> None:
        self.cdp = page.context.new_cdp_session(page)

    def eval(self, expression: str) -> object:
        found = self.cdp.send("Runtime.evaluate", {"expression": expression, "returnByValue": True})
        check("exceptionDetails" not in found, f"{expression} threw: {found.get('exceptionDetails')}")
        return found["result"].get("value")

    def wait_for(self, expression: str, why: str, seconds: float = 10) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and not self.eval(expression):
            time.sleep(0.025)
        check(bool(self.eval(expression)), why)


def reload_walk(desk: Desk) -> None:
    page = desk.page
    open_channel(desk)
    open_root_thread(desk)
    before = at_entry_unmarked(desk, 1, "thread")
    # The page reloaded on the thread's entry: the thread reopens, on an entry this load makes its base,
    # and with nothing pushed before the reader touches it. Read without a gesture until the touch.
    quiet = Quiet(page)
    page.reload(wait_until="load")
    quiet.wait_for("!document.getElementById('thread-heading').hidden && document.querySelectorAll("
                   f"'#discord-log > li[data-id]').length === {THREAD_ROWS[REPLICA]}",
                   f"{desk.label}: the reload did not reopen the thread")
    quiet.eval("window.__backWalk = 'this document'")
    desk.check(quiet.eval("navigator.userActivation.hasBeenActive") is False,
               "fixture: the reloaded page had a user activation before any touch, so its wait cannot be seen")
    page.wait_for_timeout(300)
    reopened = quiet.eval(f"({STATE_JS})()")
    assert isinstance(reopened, dict)
    desk.check(reopened["depth"] == 0 and reopened["tag"] == "top",
               f"the entry the page reloaded on is not this load's base, or one was pushed before a touch: {reopened}")
    desk.check(reopened["session"] != before["session"], "the reloaded page kept the earlier load's name")
    desk.shot("1-reopened")
    # A touch: the reopened thread's entry, which the browser's Back then closes, in the same document.
    tap(desk, "#thread-title")
    at_entry(desk, 1, "thread", "the reader's first touch did not push the reopened thread's entry")
    page.go_back()
    desk.wait_for("() => document.getElementById('thread-heading').hidden",
                  "the browser's Back did not close the reopened thread")
    at_entry(desk, 0, "top", "the browser's Back in the reopened thread did not land on this load's base")
    desk.shot("2-closed")
    # At the top, the browser's Back lands on the entry the earlier load opened on, whose screens went with
    # it: the page goes on back past it, and the one press leaves the app.
    page.go_back()
    left_app(desk, "the browser's Back at the top did not leave the app past the earlier load's entry")
    desk.notes.append("reloaded in a thread, the page made its entry its own base and pushed the reopened thread's "
                      "entry at the first touch; the browser's Back closed the thread in the same document, and the "
                      "next left the app in one press, past the entry the earlier load had opened on")


def at_entry_unmarked(desk: Desk, depth: int, tag: str) -> Json:
    """`at_entry` before the document is marked: the entry, and the address."""
    desk.wait_for(f"() => {{ const f = history.state && history.state.vibeTalkBack;"
                  f" return !!f && f.depth === {depth} && f.tag === '{tag}'; }}",
                  f"the page is not on its entry {depth} deep for {tag}")
    found = state(desk.page)
    desk.check(found["path"] == "/voice", f"the address changed to {found['url']}")
    return found


if __name__ == "__main__":
    raise SystemExit(main())
