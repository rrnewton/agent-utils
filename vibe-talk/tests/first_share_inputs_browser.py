#!/usr/bin/env python3
"""The first-share guard's numbers as one typeable sentence, in real Chromium.

`#1562 settings-long-message-share-inputs` (owner UI request 084), over
`#1537 mark-read-on-reply-settings`, which added the two sliders. The owner:
the sliders are bad; the control should read `Not in the first [50]% of a msg
over [32] words` with two typeable number fields. This serves the checked-out
page and the fake API of tests/offline_cache_browser.py from one loopback
origin and, at phone and desktop widths, walks the sentence as a reader does:

  the guard starts ticked at 50% over 0 words, both fields enabled text fields
  with a numeric keypad hint -> typing 32 and pressing Enter keeps 32 ->
  typing 40 words and leaving the field keeps 40 -> 150 snaps to 100 with the
  field marked invalid -> emptying the field brings 100 back and keeps nothing
  new -> unticking the guard greys both fields out, ticking it frees them.

No page error and no sideways scroll at any step, at 390px and on the desktop,
and the sentence fits one row at 390px and wraps between words at 320px with
no field split from its unit. Pass --screenshots DIR to keep a PNG of each
state for review; --web-root DIR to run against another copy of the page.
"""

from __future__ import annotations

import argparse
import json
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING, Literal

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, FakeApi, handler_for
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

PERCENT_KEY = "vibe-talk.voice.mark-read-first-percent"
WORDS_KEY = "vibe-talk.voice.mark-read-long-words"
SIZES: list[tuple[str, int, int, bool, Literal["dark", "light"]]] = [
    ("phone", 390, 844, True, "dark"), ("desktop", 1280, 800, False, "light"),
    ("narrow", 320, 700, True, "dark")]


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=None,
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=page_sources.served_root(),
                        help="the page assets to serve (default: this checkout's vibe-talk/web)")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per state into")
    parser.add_argument("--size", action="append", choices=[s[0] for s in SIZES],
                        help="only this size (repeatable; default: all)")
    return parser.parse_args()


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
         mobile: bool, scheme: Literal["dark", "light"]) -> str:
    api = FakeApi()
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-first-share-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme=scheme,
                has_touch=mobile, is_mobile=mobile)
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.add_init_script("localStorage.setItem('vibe-talk.token', %s);" % json.dumps(TOKEN))
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            # `#1781`: a token-seeded first run meets the Welcome; dismiss it onto the Channel.
            try:
                page.wait_for_function("() => !document.getElementById('screen-welcome').hidden", timeout=3_000)
                page.click("#welcome-start")
            except Exception:
                pass
            page.wait_for_function("() => !document.getElementById('screen-main').hidden",
                                   timeout=15_000)
            page.click("#open-settings")
            page.wait_for_selector("#screen-settings", state="visible", timeout=10_000)
            page.evaluate("() => document.getElementById('fold-mark-read').click()")
            page.wait_for_function("() => { const n = document.getElementById('mark-read-first-percent');"
                                   " return n && n.getBoundingClientRect().height > 0; }", timeout=10_000)

            def kept(key: str) -> object:
                return page.evaluate("(k) => localStorage.getItem(k)", key)

            def type_into(element_id: str, text: str, key: str,
                          storage: tuple[str, str] | None = None) -> None:
                # Commit as the field's own change, the repo's pattern for deep
                # Settings fields (tests/thread_card_max_lines_browser.py): the
                # Settings screen holds focus on its container, so a synthetic
                # focus never sticks. An Enter keydown is dispatched first, as
                # a reader's Enter, then the value and its change. A settings
                # re-render can land mid-commit, so land it by condition,
                # retrying, never by waiting a fixed time.
                from playwright.sync_api import TimeoutError as PlaywrightTimeout
                for _attempt in range(4):
                    page.evaluate("""([id, text]) => { const n = document.getElementById(id);
                      n.dispatchEvent(new KeyboardEvent('keydown', {key: 'Enter', bubbles: true}));
                      n.value = text;
                      n.dispatchEvent(new Event('change', {bubbles: true})); }""",
                                  arg=[element_id, text])
                    if storage is None:
                        return
                    try:
                        page.wait_for_function(
                            "([k, want]) => localStorage.getItem(k) === want",
                            arg=list(storage), timeout=2_000)
                        return
                    except PlaywrightTimeout:
                        continue

            def wait_value(element_id: str, want: str, what: str) -> None:
                page.wait_for_function(
                    "([id, want]) => document.getElementById(id).value === want",
                    arg=[element_id, want], timeout=10_000)

            state = page.evaluate("""() => {
              const percent = document.getElementById('mark-read-first-percent');
              const words = document.getElementById('mark-read-long-words');
              const line = percent.closest('.first-share-line');
              return {
                percentType: percent.type, wordsType: words.type,
                percentMode: percent.getAttribute('inputmode'), wordsMode: words.getAttribute('inputmode'),
                percent: percent.value, words: words.value,
                percentDisabled: percent.disabled, wordsDisabled: words.disabled,
                guard: document.getElementById('mark-read-first-guard').checked,
                sliders: document.querySelectorAll('#mark-read-quoted input[type=range]').length,
                steppers: document.getElementById('mark-read-first-percent-smaller'),
                sentence: line ? line.textContent.replace(/\\s+/g, ' ').trim() : '',
                overflow: document.documentElement.scrollWidth - window.innerWidth,
              }; }""")
            check(state["percent"] == "50" and state["words"] == "0",
                  f"{label}: the sentence does not start at 50% over 0 words: {state}")
            check(state["percentType"] == "text" and state["wordsType"] == "text"
                  and state["percentMode"] == "numeric" and state["wordsMode"] == "numeric",
                  f"{label}: the fields are not typeable numeric fields: {state}")
            check(not state["percentDisabled"] and not state["wordsDisabled"] and state["guard"],
                  f"{label}: the fields do not start enabled under their ticked guard: {state}")
            check(state["steppers"] is None, f"{label}: a stepper button survived: {state}")
            check("Not in the first" in state["sentence"] and "% of a msg over" in state["sentence"]
                  and state["sentence"].endswith("words"),
                  f"{label}: the sentence does not read as asked: {state['sentence']!r}")
            check(state["overflow"] <= 1, f"{label}: sideways scroll of {state['overflow']}px")

            if label == "narrow":
                # Layout only at 320px: typing is walked at 390px and on the
                # desktop; here the sentence must wrap without splitting a field.
                boxes = page.evaluate("""() => {
                  const box = (id) => { const r = document.getElementById(id)
                    .closest('.first-share-field').getBoundingClientRect();
                    return { top: r.top, bottom: r.bottom }; };
                  const line = document.querySelector('.first-share-line').getBoundingClientRect();
                  return { percent: box('mark-read-first-percent'), words: box('mark-read-long-words'),
                           lineTop: line.top, lineBottom: line.bottom }; }""")
                check(boxes["percent"]["bottom"] <= boxes["lineBottom"] + 1
                      and boxes["words"]["bottom"] <= boxes["lineBottom"] + 1,
                      f"narrow: a field split from its unit: {boxes}")
                if args.screenshots:
                    page.evaluate("() => document.getElementById('mark-read-first-percent').focus()")
                    page.screenshot(path=str(args.screenshots / f"first-share-narrow-{width}.png"),
                                    animations="disabled")
                check(not errors, f"narrow: page errors: {errors}")
                context.close()
                return ("narrow at 320x700 (dark): the sentence wraps between words with no field "
                        "split from its unit and no sideways scroll")

            type_into("mark-read-first-percent", "32", "Enter", (PERCENT_KEY, "32"))
            type_into("mark-read-long-words", "40", "Enter", (WORDS_KEY, "40"))

            type_into("mark-read-first-percent", "150", "Enter", (PERCENT_KEY, "100"))
            wait_value("mark-read-first-percent", "100", "150 did not snap to 100")
            check(page.evaluate("() => document.getElementById('mark-read-first-percent')"
                                ".getAttribute('aria-invalid')") == "true",
                  f"{label}: the snapped field is not marked invalid")
            check(kept(PERCENT_KEY) == "100", f"{label}: the snap was not kept: {kept(PERCENT_KEY)}")

            type_into("mark-read-first-percent", "", "Enter")
            wait_value("mark-read-first-percent", "100", "emptying the field did not restore 100")
            check(kept(PERCENT_KEY) == "100", f"{label}: emptying kept a new value: {kept(PERCENT_KEY)}")

            page.evaluate("() => document.getElementById('mark-read-first-guard').click()")
            page.wait_for_function("() => document.getElementById('mark-read-first-percent').disabled"
                                   " && document.getElementById('mark-read-long-words').disabled",
                                   timeout=10_000)
            page.evaluate("() => document.getElementById('mark-read-first-guard').click()")
            page.wait_for_function("() => !document.getElementById('mark-read-first-percent').disabled",
                                   timeout=10_000)

            boxes = page.evaluate("""() => {
              const box = (id) => { const r = document.getElementById(id)
                .closest('.first-share-field').getBoundingClientRect();
                return { top: r.top, bottom: r.bottom }; };
              const line = document.querySelector('.first-share-line').getBoundingClientRect();
              return { percent: box('mark-read-first-percent'), words: box('mark-read-long-words'),
                       lineTop: line.top, lineBottom: line.bottom }; }""")
            if label == "phone":
                check(boxes["percent"]["top"] == boxes["words"]["top"],
                      f"phone: the sentence is not on one row: {boxes}")
            check(boxes["percent"]["bottom"] <= boxes["lineBottom"] + 1
                  and boxes["words"]["bottom"] <= boxes["lineBottom"] + 1,
                  f"{label}: a field split from its unit: {boxes}")

            if args.screenshots:
                page.evaluate("() => document.getElementById('mark-read-first-percent').focus()")
                page.screenshot(path=str(args.screenshots / f"first-share-{label}-{width}.png"),
                                animations="disabled")
            check(not errors, f"{label}: page errors: {errors}")
            context.close()
            return (f"{label} at {width}x{height} ({scheme}): Not in the first [50]% of a msg over "
                    "[0] words as two typed fields, 32 then 40 kept, 150 snapped to 100 marked "
                    "invalid, empty restored 100, the guard greying both fields, no sideways scroll")
    finally:
        server.shutdown()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required: python3 -m pip install --user playwright && "
                         "python3 -m playwright install chromium") from error
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    with sync_playwright() as playwright:
        for label, width, height, mobile, scheme in SIZES:
            if args.size and label not in args.size:
                continue
            print(walk(playwright.chromium, args, label, width, height, mobile, scheme))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
