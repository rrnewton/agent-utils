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
  closes, the app is not navigated away from (the same document, at /), and All is back at the
  reader's place, its first row and that row's offset within a pixel -> the thread list from the
  view picker: one entry -> a card's replies: its thread across the window, one more -> history.back():
  the thread list again -> the browser's Back: All, at the reader's place, with no entry of the
  page's above its own -> a thread again, and the ⋯ menu of one of its rows: one entry more, as a
  close watcher would not hear the browser's Back -> the browser's Back: the menu closes and the
  thread stays -> the thread closed by its on-screen Back: the page goes back through the thread's
  entry itself -> so the browser's Back at the top leaves the app, for the page it was opened from.

Then, in a fresh profile: the thread list, a thread opened from it, and the page reloaded on its entry
-> the thread reopened (`#189 restore-ui-state`) standing on the earlier load's two entries, this load's
own now, with nothing pushed -> with no touch, as when a phone reloads an app it put away, the
browser's Back: the thread alone closes, back to the list -> the browser's Back: All -> the browser's
Back: the app is left, in one press.

Then the same in an installed app's own window, whose history starts at the app (DevTools'
`Page.resetNavigationHistory` after loading /, so the app is the tab's first entry): reloaded in a
thread opened from the thread list, two Backs close the thread and the list and leave the browser on
the tab's first entry, so the next Back is the system's, with no press spent on an entry the earlier
load left -> Settings, and the page reloaded on its entry: Settings is not reopened, and the page goes
back through its entry at once, to the first entry again.

And at a 1440x900 desk with a mouse, two columns with a thread on the right and the ⋯ menu of a row
of the left column's: the browser's Back -- the toolbar's, a mouse's back button, Alt+Left, which no
close watcher hears -- closes the menu alone, the thread still on the right; the next takes the right
column back to its cards, and the next leaves the app.

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
from two_column_browser import REPLICA, RUNNER, THREAD_ROWS, ColumnsApi, Desk
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, BrowserType, Page

WIDTH, HEIGHT = 412, 915
DESK_WIDTH, DESK_HEIGHT = 1440, 900
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
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    report: list[str] = []
    with sync_playwright() as playwright:
        report.append(walk(playwright.chromium, args, "phone-412", gesture_walk))
        report.append(walk(playwright.chromium, args, "phone-412-reload", reload_walk))
        report.append(walk(playwright.chromium, args, "phone-412-installed", installed_walk, home=False))
        report.append(walk(playwright.chromium, args, "desk-1440", desk_walk, desk=True))
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


def desk_context(chromium: BrowserType, args: argparse.Namespace, profile: str) -> BrowserContext:
    """A desk's Chrome at 1440x900, with a mouse: wide enough for two columns."""
    return chromium.launch_persistent_context(
        profile, headless=True, executable_path=args.browser_executable,
        viewport={"width": DESK_WIDTH, "height": DESK_HEIGHT}, color_scheme="dark",
    )


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, steps: Callable[[Desk], None],
         home: bool = True, desk: bool = False) -> str:
    """One reader, against a fresh fake API and a fresh browser profile. With `home`, a plain page is
    opened before the app, as the home screen it was launched from; without, the app at / is made the
    tab's first entry, as in an installed app's own window. With `desk`, at a desk rather than a phone."""
    api = ColumnsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-back-gesture-") as profile:
            context = desk_context(chromium, args, profile) if desk else phone_context(chromium, args, profile)
            width, height = (DESK_WIDTH, DESK_HEIGHT) if desk else (WIDTH, HEIGHT)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            reader = Desk(page, args.screenshots, label, width, height)
            if home:
                page.goto(HOME, wait_until="load")
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            if not home:
                cdp = page.context.new_cdp_session(page)
                try:
                    cdp.send("Page.resetNavigationHistory")
                finally:
                    cdp.detach()
                check(navigation(page) == (0, 1), f"{label}: fixture: the app is not the tab's only entry: {navigation(page)}")
            steps(reader)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}, {label}: " + "; ".join(reader.notes)
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


def navigation(page: Page) -> tuple[int, int]:
    """Where the browser is in the tab's history, and how many entries it has, as its DevTools protocol says."""
    cdp = page.context.new_cdp_session(page)
    try:
        found = cdp.send("Page.getNavigationHistory")
    finally:
        cdp.detach()
    return int(found["currentIndex"]), len(found["entries"])


def history_index(page: Page) -> int:
    """Where the browser is in the tab's history, as its DevTools protocol says."""
    return navigation(page)[0]


def mark_document(page: Page) -> None:
    """Mark the document that is loaded: a navigation away, or a reload, takes the mark with it."""
    page.evaluate("() => { window.__backWalk = 'this document'; }")


def at_entry(desk: Desk, depth: int, tag: str, why: str) -> Json:
    """Wait until the browser is on the page's entry `depth` deep, standing for `tag`; return the state there."""
    desk.wait_for(f"() => {{ const f = history.state && history.state.vibeTalkBack;"
                  f" return !!f && f.depth === {depth} && f.tag === '{tag}'; }}", why)
    found = state(desk.page)
    desk.check(found["path"] == "/", f"the address changed to {found['url']}")
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
    desk.check(opened["path"] == "/", f"the thread's entry changed the address to {opened['url']}")
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

    # A thread again, and the ⋯ menu of one of its rows: an entry of its own, as the browser's Back is
    # no close request. The browser's Back closes the menu alone; the thread stays.
    open_root_thread(desk)
    at_entry(desk, 1, "thread", "the thread opened again is not one entry")
    desk.settled()
    menu = "#discord-log > li[data-id] .row-more-menu"
    tap(desk, "#discord-log > li[data-id] .row-more-button")
    desk.wait_for(f"() => !document.querySelector('{menu}').hidden", "the ⋯ did not open its menu")
    desk.settled()
    with_menu = state(page)
    desk.shot("5-menu-in-thread")
    # What the menu did to the history is checked after the Back, which is where a reader would see a
    # menu with no entry: the Back closes the thread under it and leaves the menu open.
    page.go_back()
    desk.wait_for(f"() => document.querySelector('{menu}').hidden || document.getElementById('thread-heading').hidden",
                  "the browser's Back with the ⋯ menu open changed nothing")
    desk.settled()
    desk.check(not page.url.startswith("data:") and state(page)["thread"] is True,
               "the browser's Back with the ⋯ menu open closed the thread under it")
    desk.check(bool(page.evaluate(f"() => document.querySelector('{menu}').hidden")), "the browser's Back did not close the ⋯ menu")
    at_entry(desk, 1, "thread", "the browser's Back on the ⋯ menu did not land on the thread's entry")
    desk.check(with_menu["depth"] == 2 and with_menu["tag"] == "menu:row",
               f"the ⋯ menu over the thread is not an entry over the thread's: {with_menu}")

    # The thread closed by its on-screen Back: the page goes back through the thread's entry itself.
    tap(desk, "#thread-back")
    desk.wait_for("() => document.getElementById('thread-heading').hidden", "the on-screen Back did not close the thread")
    at_entry(desk, 0, "top", "the on-screen Back did not go back through the thread's entry")
    desk.check(history_index(page) == base_index, "the on-screen Back left the browser off the page's own entry")
    desk.notes.append("the ⋯ menu in a thread was an entry, and the browser's Back closed it alone; the on-screen "
                      "← Back went back through the thread's entry")

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


def open_list_thread(desk: Desk) -> None:
    """The thread list from the view picker, and a card's replies: its thread across the window, two entries deep."""
    page = desk.page
    page.select_option("#thread-select", "threads")
    desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
                  " && document.querySelectorAll('#side-cards > li[data-card]').length === 3",
                  "the view picker did not show the thread list")
    desk.settled()
    tap(desk, f"#side-cards > li[data-id='{REPLICA}'] .side-card-open")
    desk.wait_for("() => !document.getElementById('thread-heading').hidden"
                  " && !document.getElementById('screen-main').hasAttribute('data-columns')",
                  "the card's replies did not open its thread across the window")


def reload_quietly(desk: Desk, reopened: str, why: str) -> tuple[Quiet, Json]:
    """Reload, and wait for `reopened` to hold, reading only through `Quiet`: no user activation, as when a
    phone reloads an app it put away. Mark the new document, and return the state there."""
    page = desk.page
    quiet = Quiet(page)
    page.reload(wait_until="load")
    quiet.wait_for(reopened, f"{desk.label}: {why}")
    quiet.eval("window.__backWalk = 'this document'")
    desk.check(quiet.eval("navigator.userActivation.hasBeenActive") is False,
               "fixture: the reloaded page had a user activation before any touch")
    page.wait_for_timeout(300)
    found = quiet.eval(f"({STATE_JS})()")
    assert isinstance(found, dict)
    return quiet, found


REOPENED_LIST_THREAD = ("!document.getElementById('thread-heading').hidden && !document.getElementById('screen-main')"
                        f".hasAttribute('data-columns') && document.querySelectorAll('#discord-log > li[data-id]').length"
                        f" === {THREAD_ROWS[REPLICA]}")


def stood_on_earlier(desk: Desk, before: Json, reopened: Json) -> None:
    """The thread reopened from the thread list stood on the earlier load's two entries, made this load's own."""
    desk.check(reopened["depth"] == 2 and reopened["tag"] == "thread",
               f"the reopened thread did not stand on the earlier load's two entries: {reopened}")
    desk.check(reopened["session"] != before["session"], "the reloaded page kept the earlier load's name")


def back_through_reopened(desk: Desk, reopened: Json) -> None:
    """Two Backs close the thread reopened from the thread list, and then the list."""
    page = desk.page
    page.go_back()
    desk.wait_for("() => document.getElementById('thread-heading').hidden", "the first Back after the reload did not close the thread")
    desk.settled()
    layout = page.evaluate("() => document.getElementById('screen-main').getAttribute('data-columns')")
    desk.check(layout == "list", f"the first Back after the reload closed more than the thread: the layout is {layout!r}, "
               "not the thread list it was opened from")
    first = at_entry(desk, 1, "list", "the first Back after the reload did not land on the thread list's entry")
    desk.check(first["session"] == reopened["session"], "the earlier load's entry was not made this load's")
    desk.shot("2-back-to-list")
    page.go_back()
    desk.wait_for("() => !document.getElementById('screen-main').hasAttribute('data-columns')",
                  "the second Back after the reload did not leave the thread list")
    out = at_entry(desk, 0, "top", "the second Back after the reload did not land on the base")
    desk.check(out["picker"] == "flat" and not out["thread"], f"the thread list went back to {out['picker']}")


def reload_walk(desk: Desk) -> None:
    page = desk.page
    open_channel(desk)
    open_list_thread(desk)
    before = at_entry_unmarked(desk, 2, "thread")
    length = navigation(page)[1]
    # The page reloaded on the thread's entry: the thread and the list reopen, standing on the earlier
    # load's two entries, and nothing is pushed. Read without a gesture until the end.
    _quiet, reopened = reload_quietly(desk, REOPENED_LIST_THREAD, "the reload did not reopen the thread from the list")
    desk.check(navigation(page)[1] == length, "the reload changed how many entries the tab has")
    desk.shot("1-reopened")
    back_through_reopened(desk, reopened)
    stood_on_earlier(desk, before, reopened)
    page.go_back()
    left_app(desk, "the third Back after the reload did not leave the app")
    desk.notes.append("reloaded in a thread opened from the thread list, both reopened on the earlier load's two "
                      "entries; with no touch, the browser's Back closed the thread alone, the next the list, and the "
                      "next left the app")


def installed_walk(desk: Desk) -> None:
    """The app the tab's first entry, as in an installed app's own window."""
    page = desk.page
    open_channel(desk)
    open_list_thread(desk)
    before = at_entry_unmarked(desk, 2, "thread")
    desk.check(navigation(page) == (2, 3), f"fixture: the thread from the list is not two entries over the app's first: "
               f"{navigation(page)}")
    _quiet, reopened = reload_quietly(desk, REOPENED_LIST_THREAD, "the reload did not reopen the thread from the list")
    # Here the reader touches the page first, so whatever the page still wanted to push is pushed. On the
    # thread card's age, which pushes nothing: it folds the card to a bar (`#388 thread-header-collapse`).
    tap(desk, "#thread-card-age")
    desk.settled()
    back_through_reopened(desk, reopened)
    desk.check(history_index(page) == 0, f"the earlier load left entries under the app's base: the browser is at "
               f"{navigation(page)}, and each is a press that changes nothing")
    desk.check(navigation(page)[1] == 3, f"the reload, or the touch, changed how many entries the tab has: {navigation(page)}")
    stood_on_earlier(desk, before, reopened)
    desk.notes.append("with the app the tab's first entry, a reload in a thread from the thread list, a touch, and two "
                      "Backs closed the thread and the list and left the browser on the first entry: the next is the system's")

    # Settings, then a reload on its entry: Settings is not reopened, so the page goes back through its
    # entry at once, without a touch, to the first entry again.
    tap(desk, "#open-settings")
    desk.wait_for("() => !document.getElementById('screen-settings').hidden", "the gear did not open Settings")
    at_entry(desk, 1, "settings", "Settings is not one entry over the app's first")
    _quiet, found = reload_quietly(desk, "!document.getElementById('screen-main').hidden",
                                   "the reload on Settings' entry did not open the channel")
    desk.wait_for("() => { const f = history.state && history.state.vibeTalkBack; return !!f && f.depth === 0; }",
                  "the page did not go back through Settings' entry after the reload")
    desk.check(history_index(page) == 0, f"after a reload on Settings' entry the browser is at {navigation(page)}")
    desk.check(found["session"] is not None, "the reloaded page made no entry its own")
    desk.notes.append("reloaded on Settings' entry, which nothing reopens, the page went back through it at once, "
                      "to the first entry")


def desk_walk(desk: Desk) -> None:
    """Two columns at a desk, a thread on the right, and the ⋯ menu of a row of the left column's."""
    page = desk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24",
                  "the channel did not open in All")
    mark_document(page)
    page.click("#column-toggle")
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length > 0"
                  " && document.querySelectorAll('#side-cards > li[data-card]').length === 3",
                  "the layout button did not draw the right column and its cards")
    at_entry(desk, 0, "top", "two columns pushed an entry")
    page.click(f"#side-cards > li[data-id='{RUNNER}'] .side-card-open")
    desk.wait_for(f"() => document.querySelectorAll('#side-log > li[data-id]').length === {THREAD_ROWS[RUNNER]}",
                  "the card did not select its thread on the right")
    at_entry(desk, 1, "side", "the right column's thread is not one entry")
    desk.settled()
    row = page.evaluate("() => { const row = [...document.querySelectorAll('#discord-log > li[data-id]')]"
                        ".find((li) => li.getClientRects().length && li.querySelector('.row-more-button'));"
                        " return row ? row.getAttribute('data-id') : null; }")
    desk.check(isinstance(row, str), "no row of the left column has a ⋯")
    menu = f"#discord-log > li[data-id='{row}'] .row-more-menu"
    page.hover(f"#discord-log > li[data-id='{row}']")
    page.click(f"#discord-log > li[data-id='{row}'] .row-more-button")
    desk.wait_for(f"() => !document.querySelector(\"{menu}\").hidden", "the ⋯ did not open its menu")
    desk.settled()
    with_menu = state(page)
    desk.shot("1-menu")
    # As in the phone's walk, the menu's entry is checked after the Back, where a reader sees its absence.
    page.go_back()
    desk.wait_for(f"() => document.querySelector(\"{menu}\").hidden || document.getElementById('side-list').hidden === false",
                  "the browser's Back with the ⋯ menu open changed nothing")
    desk.settled()
    desk.check(len(desk.rows("side-log")) == THREAD_ROWS[RUNNER] and not page.url.startswith("data:"),
               "the browser's Back with the ⋯ menu open took the thread off the right column")
    desk.check(bool(page.evaluate(f"() => document.querySelector(\"{menu}\").hidden")), "the browser's Back did not close the ⋯ menu")
    at_entry(desk, 1, "side", "the browser's Back on the ⋯ menu did not land on the right column's thread")
    desk.check(with_menu["depth"] == 2 and with_menu["tag"] == "menu:row",
               f"the ⋯ menu over two columns is not an entry over the right column's thread: {with_menu}")
    desk.shot("2-menu-closed")
    page.go_back()
    desk.wait_for("() => document.getElementById('side-list').hidden === false", "the next Back did not bring the cards back")
    at_entry(desk, 0, "top", "the Back on the right column's thread did not land on the base")
    page.go_back()
    left_app(desk, "the browser's Back at the top did not leave the app")
    desk.notes.append(f"two columns, a thread on the right and the ⋯ menu of row {row}: the browser's Back closed the "
                      "menu alone, the next the thread, the next left the app")


def at_entry_unmarked(desk: Desk, depth: int, tag: str) -> Json:
    """`at_entry` before the document is marked: the entry, and the address."""
    desk.wait_for(f"() => {{ const f = history.state && history.state.vibeTalkBack;"
                  f" return !!f && f.depth === {depth} && f.tag === '{tag}'; }}",
                  f"the page is not on its entry {depth} deep for {tag}")
    found = state(desk.page)
    desk.check(found["path"] == "/", f"the address changed to {found['url']}")
    return found


if __name__ == "__main__":
    raise SystemExit(main())
