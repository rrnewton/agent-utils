#!/usr/bin/env python3
"""Press Ctrl+S (Cmd+S) in real Chromium in two columns, from wherever the focus and the selection are.

`#403 composer-key-global`. The owner, on a desk in two columns: "I was just trying 'Cmd-s' to select the
message entry box... it didn't work. E.g. when a message in the right column is highlighted and focus is
elsewhere. But if focus was in the left column it worked. It seemed selective. It should be globally
applicable." The rule Help states: the box of the column the focus is in; with the focus in neither (the
bar, the page itself) the box of the column holding the selection; Main's from the thread cards, which
have none. The fake-DOM suite pins each state; this presses real keys through CDP
`Input.dispatchKeyEvent`, the path a hardware keyboard takes into the renderer, so the browser's own
default for the chord (its Save page) is what the page has to refuse.

At 1280x800, after each chord: the focus in the box the rule names, and the chord's default refused
(`defaultPrevented`, read by a listener on the window after the page's own):

  two columns, the right column on its thread cards: from a card and from the page body, Main's box ->
  a thread open on the right: from a message selected in the right column, the right column's box; from
  a message selected in the left column, Main's; from the search glass (a button on the bar) with the
  selection on the right, the right column's; from the page body with the selection on the left, Main's;
  on each column's Newest chip, outside its scroller, that column's box -> the thread list (Ctrl+3): left for the view it was chosen from, and Main's box -> the negative
  control: typing in the search field the chord still goes to a message box and never saves the page,
  and in a box with a draft it stays, the draft untouched.

Against the page from before the change it fails at the first chord, on a thread card, where the focus
stayed. It reuses keyboard_browser's fixture: its fake API, asset server and CDP key helpers. --web-root
DIR runs it against another copy of the page; --screenshots DIR keeps a PNG of each step; --port N
serves the fixture on that port (default: one the system picks).
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path

import keyboard_browser as kb
from keyboard_browser import TOKEN, Json, Walk, check
import page_sources

FIRST_ROW = "#discord-log > li[data-id]"
CARD = "#side-cards > li[data-id] .side-card"
SIDE_ROW = "#side-log > li[data-id]"

# Where the focus is, by id, or by tag and row id for a message.
FOCUS_JS = """() => {
  const held = document.activeElement;
  if (!held || held === document.body) return 'body';
  return held.id || held.tagName.toLowerCase() + (held.dataset && held.dataset.id ? '#' + held.dataset.id : '');
}"""

BLUR_JS = "() => { if (document.activeElement) document.activeElement.blur(); }"

SHOW_CHIPS_JS = "() => { for (const id of ['jump-newest', 'side-jump-newest']) document.getElementById(id).hidden = false; }"


def focus(walk: Walk) -> str:
    found = walk.page.evaluate(FOCUS_JS)
    assert isinstance(found, str)
    return found


def value(walk: Walk, element_id: str) -> str:
    found = walk.page.evaluate("(id) => document.getElementById(id).value", element_id)
    assert isinstance(found, str)
    return found


def chord(walk: Walk, where: str, box: str, keys: str = "Control+s") -> Json:
    """Press the chord and check the focus went to `box` with the Save page refused."""
    before = focus(walk)
    kb.press(walk.cdp, keys)
    state = walk.settled()
    last = walk.last_key()
    check(last["prevented"] is True, f"{keys} {where} was left to the browser, whose Save page it opens: {last}")
    check(state["screen"] == "main", f"{keys} {where} left the reader on {state['screen']!r}")
    check(state["focus"] == box, f"{keys} {where} (focus on {before}) left the focus on {state['focus']!r}, not #{box}")
    walk.shot(where.replace(" ", "-").replace("'", "")[:48])
    return state


def select_row(walk: Walk, selector: str) -> str:
    """Click a message to select it, as the reader would; return the focus it left."""
    walk.page.click(f"{selector} >> nth=-1", position={"x": 300, "y": 10})
    walk.settled()
    held = focus(walk)
    check(held.startswith("li#"), f"fixture: a click on {selector} selected nothing, the focus on {held!r}")
    return held


def thread_cards(walk: Walk) -> None:
    page = walk.page
    page.click("#column-toggle")
    page.wait_for_selector(CARD, state="visible", timeout=10_000)
    check(page.evaluate("() => document.getElementById('side-composer').checkVisibility()") is False,
          "fixture: the right column shows a box on its cards")
    page.focus(CARD)
    chord(walk, "on a thread card", "channel-compose-text")
    page.focus(CARD)
    page.evaluate(BLUR_JS)
    chord(walk, "from the page body after a card", "channel-compose-text")
    walk.notes.append("with the right column on its thread cards, Ctrl+S from a card and from the body went to Main's box")


def thread_open(walk: Walk) -> None:
    page = walk.page
    page.click("#side-cards > li[data-id] .side-card-open")
    page.wait_for_selector(SIDE_ROW, state="visible", timeout=10_000)
    page.wait_for_selector("#side-compose-text", state="visible", timeout=10_000)
    select_row(walk, SIDE_ROW)
    chord(walk, "from the right column's selected message", "side-compose-text")
    select_row(walk, FIRST_ROW)
    chord(walk, "from the left column's selected message", "channel-compose-text")
    select_row(walk, SIDE_ROW)
    page.focus("#search-toggle")
    chord(walk, "from the search glass, the selection on the right", "side-compose-text")
    select_row(walk, FIRST_ROW)
    page.evaluate(BLUR_JS)
    chord(walk, "from the page body, the selection on the left", "channel-compose-text")
    select_row(walk, SIDE_ROW)
    chord(walk, "Cmd+S from the right column's selected message", "side-compose-text", "Meta+s")
    # A column's chips are outside its scroller and still that column's: each chip's box, whichever
    # column holds the selection. Shown as when each list is scrolled up; a hidden chip takes no focus.
    page.evaluate(SHOW_CHIPS_JS)
    page.focus("#jump-newest")
    chord(walk, "on the left column's chip, the selection on the right", "channel-compose-text")
    select_row(walk, FIRST_ROW)
    page.evaluate(SHOW_CHIPS_JS)
    page.focus("#side-jump-newest")
    chord(walk, "on the right column's chip, the selection on the left", "side-compose-text")
    walk.notes.append("with a thread open on the right, Ctrl+S went to the right column's box from its selected "
                      "message and from the search glass with the selection there, and to Main's from the left "
                      "column's selected message and from the body with the selection there; Cmd+S as Ctrl+S; on each "
                      "column's Newest chip, that column's box")


def in_a_field(walk: Walk) -> None:
    """The negative control: in a text field the chord moves to a message box; in a box it stays."""
    page = walk.page
    select_row(walk, SIDE_ROW)
    page.click("#search-toggle")
    page.wait_for_selector("#search-field", state="visible", timeout=5000)
    page.fill("#search-field", "reply")
    page.focus("#search-field")
    chord(walk, "typing in the search field", "side-compose-text")
    check(value(walk, "search-field") == "reply", f"Ctrl+S threw the search away: {value(walk, 'search-field')!r}")
    page.focus("#search-field")
    walk.key("Escape")
    for box in ("side-compose-text", "channel-compose-text"):
        page.fill(f"#{box}", "half a thought")
        page.focus(f"#{box}")
        chord(walk, f"in #{box} with a draft", box)
        check(value(walk, box) == "half a thought", f"Ctrl+S in #{box} touched the draft: {value(walk, box)!r}")
        page.fill(f"#{box}", "")
    walk.notes.append("typing in the search field Ctrl+S went to the right column's box with the search kept, and in "
                      "each box with a draft it stayed with the draft untouched, the Save page refused every time")


def thread_list(walk: Walk) -> None:
    page = walk.page
    view = page.evaluate("() => document.getElementById('thread-select').value")
    page.focus("#scroll-area")
    walk.key("Control+3")
    page.wait_for_selector(CARD, state="visible", timeout=10_000)
    check(page.evaluate("() => document.getElementById('thread-select').value") == "threads",
          "fixture: Ctrl+3 did not open the thread list")
    page.focus(CARD)
    chord(walk, "on the thread list", "channel-compose-text")
    left = page.evaluate("() => document.getElementById('thread-select').value")
    check(left == view, f"Ctrl+S on the thread list went to {left!r}, not back to {view!r}")
    walk.notes.append(f"on the thread list Ctrl+S went back to {view!r} and Main's box")


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=page_sources.served_root(),
                        help="the page assets to serve (default: this checkout's vibe-talk/web)")
    parser.add_argument("--screenshots", type=Path, default=None,
                        help="a directory to keep one PNG per step in (default: none kept)")
    parser.add_argument("--port", type=int, default=0,
                        help="the local port to serve the fixture on (default: 0, one the system picks)")
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
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)

    api = kb.FakeApi()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), kb.handler_for(api, args.web_root.resolve()))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/"

    try:
        with sync_playwright() as playwright, tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-") as profile:
            context = playwright.chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": 1280, "height": 800},
            )
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            cdp = context.new_cdp_session(page)
            walk = Walk(page, cdp, args.screenshots)

            page.goto(url, wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
            page.click("#view-switch")
            page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)
            page.evaluate(kb.RECORD_KEYS_JS)
            walk.settled()
            walk.shot("loaded")

            thread_cards(walk)
            thread_open(walk)
            in_a_field(walk)
            page.click("#column-toggle")
            page.wait_for_function("() => document.getElementById('side-column').getClientRects().length === 0",
                                   timeout=5000)
            thread_list(walk)

            check(not errors, f"the page threw: {errors}")
            browser_version = context.browser.version if context.browser else "Chromium"
            context.close()

        print(f"{browser_version} at 1280x800, keys through CDP Input.dispatchKeyEvent: " + "; ".join(walk.notes))
        return 0
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
