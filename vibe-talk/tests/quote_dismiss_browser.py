#!/usr/bin/env python3
"""A dismiss X beside the inline quote-reply chip, tapped away on a phone.

`#1717 vibe-talk-quote-reply-dismiss-x`. The owner, :thread:117 at 9:45 AM ET: "Give me a
little X next to the inline quote reply button. When I do a selection accidentally, I don't
have a good way to clear it. If I tap the message then I collapse a very big message and
have to open it up again and scroll."

At 412x915 CSS px at device scale 2.6, touch: a long folded message is opened, words in it
are selected as a long press leaves them, and the Quoted reply chip (`#726
vt-selection-chip`) and its dismiss X stand beside each other without overlapping, the X at
least 32px in each dimension. A real tap on the X then empties the selection, the chip and
the X go, the card stays expanded and the list's scrollTop does not move. All waits are on
page conditions; there are no timed sleeps.

Pass --screenshots DIR to keep the before/after PNGs; --web-root DIR to walk another copy
of the page.
"""

from __future__ import annotations

import argparse
import os
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import keyboard_browser as kb
from keyboard_browser import TOKEN, check
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import Page

LONG_INDEX = kb.MESSAGES - 8
LONG_ID = f"88000000000000{LONG_INDEX:04d}"

SELECT_JS = """(id) => {
  const row = [...document.querySelectorAll('#discord-log > li[data-id]')].find((li) => li.dataset.id === id);
  const body = row.querySelector('.body');
  const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
  let text = null;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (!text || node.data.length > text.data.length) text = node;
  }
  const range = document.createRange();
  range.setStart(text, 12);
  range.setEnd(text, 60);
  getSelection().removeAllRanges();
  getSelection().addRange(range);
  return String(getSelection());
}"""

STATE_JS = """(id) => {
  const area = document.getElementById('scroll-area');
  const row = [...document.querySelectorAll('#discord-log > li[data-id]')].find((li) => li.dataset.id === id);
  const box = (el) => {
    if (!el || el.hidden || !el.getClientRects().length) return null;
    const r = el.getBoundingClientRect();
    return {top: r.top, bottom: r.bottom, left: r.left, right: r.right, width: r.width, height: r.height};
  };
  return {
    selected: String(getSelection()),
    collapsed: row ? row.getAttribute('data-collapsed') : null,
    scrollTop: area.scrollTop,
    chip: box(document.getElementById('selection-chip')),
    dismiss: box(document.getElementById('selection-chip-dismiss')),
  };
}"""

SHOWN_JS = ("() => { const shown = (id) => { const el = document.getElementById(id); "
            "return el && !el.hidden && !!el.getClientRects().length; }; "
            "return shown('selection-chip') && shown('selection-chip-dismiss'); }")
CLEARED_JS = ("() => { const gone = (id) => { const el = document.getElementById(id); "
              "return !el || el.hidden || !el.getClientRects().length; }; "
              "return String(getSelection()) === '' && gone('selection-chip') "
              "&& gone('selection-chip-dismiss'); }")


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=page_sources.served_root(),
                        help="the page assets to serve (default: this checkout's vibe-talk/web)")
    parser.add_argument("--screenshots", type=Path, default=None,
                        help="a directory to keep the before/after PNGs in (default: none kept)")
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
    server = ThreadingHTTPServer(("127.0.0.1", 0), kb.handler_for(api, page_sources.served(args.web_root)))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/"

    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(headless=True, executable_path=args.browser_executable)
            context = browser.new_context(
                viewport={"width": 412, "height": 915}, device_scale_factor=2.6,
                is_mobile=True, has_touch=True)
            page: Page = context.new_page()
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(url, wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            try:
                page.wait_for_function("() => !document.getElementById('screen-welcome').hidden", timeout=3_000)
                page.click("#welcome-start")
            except Exception:
                pass  # no first-run Welcome on this path (#1781: a returning reader, a refusal, or a saved choice)
            page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
            page.click("#view-switch")
            page.wait_for_function(
                "() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                timeout=10_000)

            # The long message, folded as it arrives: bring it on screen and open it with a
            # tap on the card, as a reader does.
            page.evaluate(
                "(id) => { const area = document.getElementById('scroll-area'); "
                "const row = [...document.querySelectorAll('#discord-log > li[data-id]')]"
                ".find((li) => li.dataset.id === id); "
                "area.scrollTop += row.getBoundingClientRect().top - area.getBoundingClientRect().top - 24; }",
                LONG_ID)
            page.wait_for_function(
                f"() => document.querySelector(\"#discord-log > li[data-id='{LONG_ID}']\")"
                ".dataset.collapsed === 'true'", timeout=10_000)
            page.tap(f"#discord-log > li[data-id='{LONG_ID}'] .body")
            page.wait_for_function(
                f"() => document.querySelector(\"#discord-log > li[data-id='{LONG_ID}']\")"
                ".dataset.collapsed === 'false'", timeout=10_000)

            # Words selected as a long press leaves them: chip and X appear, side by side.
            picked = page.evaluate(SELECT_JS, LONG_ID)
            check(isinstance(picked, str) and len(picked) > 10, f"no words were selected: {picked!r}")
            page.wait_for_function(SHOWN_JS, timeout=10_000)
            before = page.evaluate(STATE_JS, LONG_ID)
            chip, dismiss = before["chip"], before["dismiss"]
            check(chip is not None and dismiss is not None, f"chip or X not drawn: {before}")
            overlap_x = min(chip["right"], dismiss["right"]) - max(chip["left"], dismiss["left"])
            overlap_y = min(chip["bottom"], dismiss["bottom"]) - max(chip["top"], dismiss["top"])
            check(not (overlap_x > 1 and overlap_y > 1),
                  f"the X overlaps the chip by {overlap_x}x{overlap_y}px: {chip} vs {dismiss}")
            check(dismiss["width"] >= 32 and dismiss["height"] >= 32,
                  f"the X is under a 32px touch target: {dismiss}")
            check(before["collapsed"] == "false", f"the card is not expanded before the tap: {before}")
            if args.screenshots is not None:
                page.screenshot(path=str(args.screenshots / "quote-dismiss-before.png"))

            # The tap on the X: selection empty, both gone, card and scroll unmoved.
            page.tap("#selection-chip-dismiss")
            page.wait_for_function(CLEARED_JS, timeout=10_000)
            after = page.evaluate(STATE_JS, LONG_ID)
            check(after["selected"] == "", f"the selection survived the tap: {after}")
            check(after["chip"] is None and after["dismiss"] is None,
                  f"the chip or X survived the tap: {after}")
            check(after["collapsed"] == "false",
                  f"the tap folded the card: {before['collapsed']} -> {after['collapsed']}")
            check(after["scrollTop"] == before["scrollTop"],
                  f"the tap moved the list: {before['scrollTop']} -> {after['scrollTop']}")
            if args.screenshots is not None:
                page.screenshot(path=str(args.screenshots / "quote-dismiss-after.png"))
            check(not errors, f"the page threw: {errors}")
            context.close()
            version = browser.version
            browser.close()

        print(f"{version} at 412x915 (device scale 2.6, touch): the dismiss X stands beside "
              "the Quoted reply chip without overlap and at least 32px square; tapping it "
              "clears the selection and hides both, the card still expanded, scroll unmoved")
        return 0
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
