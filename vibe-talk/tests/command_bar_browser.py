#!/usr/bin/env python3
"""Measure the command bar in real Chromium: one row across the window on a desk, the phone's as it was.

`#406 command-bar-one-row`. The owner, on a desk: "Thread list view (because of the length of the words
'Thread list') spills onto two rows in the command bar even though there is PLENTY of horizontal space..
an artificial width construction. The other views don't do this but selecting a specific long thread name
does it. Let's keep that bar one row that can go up to full width on desktop and size our widgets
appropriately." The dock used to be held to the reading column and to wrap the reading buttons onto a
second row when the pickers and they did not fit it; on a desk it now spans the window and does not wrap,
and a picker too long for the room left ends in an ellipsis, its tooltip naming the whole thread.

At 1280x800 and 1920x1080, and at 900x700, the narrowest window the desk's layout is drawn in, on a thread whose name is 120 characters, the thread list, Main and All: the
gear, the pickers, the layout button, the reading buttons and the switch on one row (their vertical
centres within 4px of each other), the dock one control tall, nothing past the window's edges, the
switch in the window's lower right corner, and for the thread its whole name in the view picker's
tooltip, and the picker set to end a label too long for it in an ellipsis. In two columns (1000px and
wider) the same. And the widest the pane gets, the call view after a call with its note under Talk, in a
24px default font at 900x700 and 1280x800: nothing drawn over anything, the pane wrapping its own
buttons as a last resort once the bar is at its floor.

The negative control, a 393x852 phone: the bar is one row above the pane's tile, the dock spans the
window, and the dock is the same height whichever view is chosen. With --baseline-web-root DIR (a copy
of the page from before the change) the phone's dock is measured under that page too, and every
control's box must be the same to the pixel.

Against the page from before the change it fails at 1280x800 on the long thread, whose reading buttons
wrap onto a second row. It reuses keyboard_browser's fixture: its fake API and asset server. --web-root DIR runs it
against another copy of the page; --screenshots DIR keeps a PNG of each step; --port N serves the
fixture on that port (default: one the system picks).
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import keyboard_browser as kb
from keyboard_browser import TOKEN, Json, check
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import Browser, Page

FIRST_ROW = "#discord-log > li[data-id]"

# The 120-character name the owner's long thread stands for: the thread's title and its first message.
LONG_NAME = ("Release train for the autumn platform update: canary widening, staging migration check, "
             "cache headers, retry wrapper fix")
assert len(LONG_NAME) == 120, len(LONG_NAME)

# Every control of the dock that is drawn, by id, with its box; and the dock's own box.
DOCK_JS = """() => {
  const box = (node) => { const r = node.getBoundingClientRect();
    return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height}; };
  const drawn = (node) => node.getClientRects().length > 0 && getComputedStyle(node).visibility !== 'hidden';
  // The pack scrolls sideways: a member is drawn only as far as the pack shows it, and not at all past it.
  const pack = document.getElementById('bar-pack').getBoundingClientRect();
  const clipped = (node, r) => {
    if (!node.parentNode || node.parentNode.id !== 'bar-pack') return r;
    const left = Math.max(r.left, pack.left), right = Math.min(r.right, pack.right);
    return right - left > 0.5 ? {...r, left, right, width: right - left} : null;
  };
  const controls = [...document.querySelectorAll(
    '#open-settings, #bar-pack > *, #view-switch, #control-pane > button, #control-pane > .control')]
    .filter(drawn).map((node) => [node, clipped(node, box(node))]).filter(([, r]) => r)
    .map(([node, r]) => ({id: node.id || node.className, ...r}));
  const select = document.getElementById('thread-select');
  return {dock: box(document.getElementById('dock')), pane: box(document.getElementById('control-pane')),
          bar: box(document.getElementById('control-bar')), controls, width: innerWidth, height: innerHeight,
          view: select.value, label: select.selectedOptions[0] ? select.selectedOptions[0].textContent : '',
          title: select.title, pickerOverflows: select.scrollWidth > select.clientWidth + 1};
}"""


def dock(page: Page) -> Json:
    found = page.evaluate(DOCK_JS)
    assert isinstance(found, dict)
    return found


def controls(state: Json) -> list[dict[str, float | str]]:
    found = state["controls"]
    assert isinstance(found, list)
    return [one for one in found if isinstance(one, dict)]


def number(box: object, key: str) -> float:
    assert isinstance(box, dict)
    value = box[key]
    assert isinstance(value, (int, float))
    return float(value)


def overlaps(state: Json, where: str) -> None:
    """No two controls of the dock drawn over each other, and none past the window's edges."""
    drawn = controls(state)
    width = float(state["width"])  # type: ignore[arg-type]
    for i, one in enumerate(drawn):
        check(number(one, "left") >= -0.5 and number(one, "right") <= width + 0.5,
              f"{where}: #{one['id']} runs past the window: {one}")
        for other in drawn[i + 1:]:
            across = min(number(one, "right"), number(other, "right")) - max(number(one, "left"), number(other, "left"))
            down = min(number(one, "bottom"), number(other, "bottom")) - max(number(one, "top"), number(other, "top"))
            check(across <= 0.5 or down <= 0.5, f"{where}: #{one['id']} and #{other['id']} overlap: {one} {other}")


def one_row(state: Json, where: str) -> None:
    """Every control of the dock on one row, inside the window, overlapping none, the switch in its corner."""
    overlaps(state, where)
    drawn = controls(state)
    check(len(drawn) >= 5, f"{where}: fixture: only {len(drawn)} controls drawn in the dock: {drawn}")
    centres = [(number(one, "top") + number(one, "bottom")) / 2 for one in drawn]
    spread = max(centres) - min(centres)
    check(spread <= 4, f"{where}: the dock is more than one row, its controls' centres {spread:.0f}px apart: "
          + ", ".join(f"{one['id']}@{number(one, 'top'):.0f}" for one in drawn))
    tallest = max(number(one, "height") for one in drawn)
    check(number(state["dock"], "height") <= tallest + 20,
          f"{where}: the dock is {number(state['dock'], 'height'):.0f}px tall for {tallest:.0f}px controls")
    width = float(state["width"])  # type: ignore[arg-type]
    for one in drawn:
        check(number(one, "left") >= 0 and number(one, "right") <= width + 0.5,
              f"{where}: #{one['id']} runs past the window: {one}")
    switch = next((one for one in drawn if one["id"] == "view-switch"), None)
    check(switch is not None, f"{where}: the switch is not drawn")
    assert switch is not None
    check(width - number(switch, "right") <= 16, f"{where}: the switch is not at the window's right edge: {switch}")


def choose(page: Page, value: str) -> Json:
    page.select_option("#thread-select", value)
    page.wait_for_function("(v) => document.getElementById('thread-select').value === v", arg=value, timeout=10_000)
    page.wait_for_timeout(300)
    return dock(page)


def shot(page: Page, shots: Path | None, name: str) -> None:
    if shots is not None:
        page.screenshot(path=str(shots / f"{name}.png"))


def open_page(browser: Browser, url: str, width: int, height: int, mobile: bool = False) -> Page:
    context = browser.new_context(viewport={"width": width, "height": height}, is_mobile=mobile, has_touch=mobile)
    page = context.new_page()
    page.goto(url, wait_until="load")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    page.click("#view-switch")
    page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)
    return page


def thread_value(page: Page) -> str:
    value = page.evaluate("() => [...document.getElementById('thread-select').options]"
                          ".map((o) => o.value).find((v) => v.startsWith('thread:'))")
    check(isinstance(value, str), "fixture: the view picker offers no thread")
    assert isinstance(value, str)
    return value


def desk(browser: Browser, url: str, width: int, height: int, shots: Path | None, notes: list[str]) -> None:
    page = open_page(browser, url, width, height)
    errors: list[str] = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    size = f"{width}x{height}"
    # The owner's two first: the long thread, then the thread list; then the views that never wrapped.
    state = choose(page, thread_value(page))
    shot(page, shots, f"{size}-long-thread")
    one_row(state, f"{size} on a thread named in 120 characters")
    label = str(state["label"])
    check(label.endswith("…"), f"{size}: fixture: the long thread's option is not cut: {label!r}")
    check(LONG_NAME in str(state["title"]), f"{size}: the view picker's tooltip does not name the whole thread: "
          f"{state['title']!r}")
    cut = page.evaluate("() => getComputedStyle(document.getElementById('thread-select')).textOverflow")
    check(cut == "ellipsis", f"{size}: a view picker too long for the bar is not cut with an ellipsis: {cut!r}")
    for value, name in (("threads", "Thread list"), ("main", "Main"), ("flat", "All")):
        state = choose(page, value)
        check(state["label"] == name, f"{size}: fixture: the picker shows {state['label']!r}, not {name!r}")
        shot(page, shots, f"{size}-{value}")
        one_row(state, f"{size} in {name}")
    # Two columns where the window draws them, at 1000px and wider.
    columns = width >= 1000
    if columns:
        page.click("#column-toggle")
        page.wait_for_function("() => document.getElementById('screen-main').getAttribute('data-columns') === 'two'",
                               timeout=10_000)
        page.wait_for_timeout(300)
        state = dock(page)
        shot(page, shots, f"{size}-two-columns")
        one_row(state, f"{size} in two columns")
    check(not errors, f"{size}: the page threw: {errors}")
    notes.append(f"{size}: one row across the window in Main, All, Thread list, a 120-character thread "
                 f"(its option cut to {label!r}, the whole name in the tooltip){' and two columns' if columns else ''},"
                 f" dock {number(state['dock'], 'height'):.0f}px tall")
    page.context.close()


# After a call, as renderControls draws it: Talk says "Start a new call" with its one-clause note under it.
AFTER_CALL_JS = """() => { document.getElementById('hang-up').hidden = true;
  document.getElementById('control-pane').className = 'solo';
  document.getElementById('talk').className = 'control control-talk';
  document.getElementById('talk-label').textContent = 'Start a new call';
  const note = document.getElementById('talk-note');
  note.textContent = 'the agent starts fresh — the earlier conversation was too long to replay';
  note.hidden = false; }"""


def large_font_after_call(browser: Browser, url: str, width: int, height: int, notes: list[str]) -> None:
    """The widest the pane gets — after a call, its note under Talk — in a 24px default font: past the
    bar's floor the pane wraps its own buttons, as a last resort, and nothing is drawn over anything."""
    page = open_page(browser, url, width, height)
    page.click("#view-switch")
    page.wait_for_function("() => document.getElementById('view-switch').getAttribute('aria-checked') !== 'true'",
                           timeout=10_000)
    page.evaluate("() => { document.documentElement.style.fontSize = '24px'; }")
    page.evaluate(AFTER_CALL_JS)
    page.wait_for_timeout(300)
    state = dock(page)
    overlaps(state, f"{width}x{height}, 24px type, after a call")
    bar = next((one for one in controls(state) if one["id"] == "open-settings"), None)
    check(bar is not None, f"{width}x{height}, 24px type, after a call: the gear is not drawn")
    notes.append(f"{width}x{height} at a 24px default font after a call: nothing drawn over anything, the dock "
                 f"{number(state['dock'], 'height'):.0f}px tall")
    page.context.close()


def phone_boxes(browser: Browser, url: str, shots: Path | None, tag: str) -> dict[str, list[dict[str, float | str]]]:
    page = open_page(browser, url, 393, 852, mobile=True)
    found: dict[str, list[dict[str, float | str]]] = {}
    for value in ("main", "flat", "threads", thread_value(page)):
        state = choose(page, value)
        key = "thread" if value.startswith("thread:") else value
        shot(page, shots, f"393x852-{tag}-{key}")
        drawn = controls(state)
        bar = [one for one in drawn if one["id"] not in ("todo-filter", "read-speed", "read-aloud", "speaker",
                                                          "clear-view", "hang-up", "talk")]
        centres = [(number(one, "top") + number(one, "bottom")) / 2 for one in bar]
        check(max(centres) - min(centres) <= 6, f"393x852 {key}: the phone's bar is more than one row: {bar}")
        check(number(state["pane"], "top") >= number(state["bar"], "bottom") - 1,
              f"393x852 {key}: the pane is no longer under the bar: {state['pane']} {state['bar']}")
        check(number(state["dock"], "left") <= 0.5 and number(state["dock"], "right") >= 392.5,
              f"393x852 {key}: the phone's dock does not span the window: {state['dock']}")
        found[key] = [{**one, "dock": number(state["dock"], "height")} for one in drawn]
    heights = {key: boxes[0]["dock"] if boxes else 0 for key, boxes in found.items()}
    check(len(set(heights.values())) == 1, f"393x852: the phone's dock changes height with the view: {heights}")
    page.context.close()
    return found


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=page_sources.served_root(),
                        help="the page assets to serve (default: this checkout's vibe-talk/web)")
    parser.add_argument("--baseline-web-root", type=Path, default=None,
                        help="a copy of the page from before the change: the phone's dock must match it to the "
                             "pixel (default: none, the phone checked on its own)")
    parser.add_argument("--screenshots", type=Path, default=None,
                        help="a directory to keep one PNG per step in (default: none kept)")
    parser.add_argument("--port", type=int, default=0,
                        help="the first local port to serve a fixture on, the baseline on the next "
                             "(default: 0, ones the system picks)")
    return parser.parse_args()


def serve(web_root: Path, port: int) -> tuple[ThreadingHTTPServer, threading.Thread, kb.FakeApi]:
    api = kb.FakeApi()
    for message in api.messages:
        if message["id"] == api.threads[0]["root"]["id"]:  # type: ignore[index]
            message["content"] = LONG_NAME
    api.threads[0]["title"] = LONG_NAME
    api.threads[0]["root"] = {**api.threads[0]["root"], "content": LONG_NAME}  # type: ignore[dict-item]
    server = ThreadingHTTPServer(("127.0.0.1", port), kb.handler_for(api, web_root.resolve()))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread, api


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
    servers = [serve(page_sources.served(args.web_root), args.port)]
    if args.baseline_web_root is not None:
        servers.append(serve(args.baseline_web_root, args.port + 1 if args.port else 0))
    urls = [f"http://127.0.0.1:{server.server_port}/" for server, _, _ in servers]
    notes: list[str] = []
    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(headless=True, executable_path=args.browser_executable)
            for width, height in ((1280, 800), (1920, 1080), (900, 700)):
                desk(browser, urls[0], width, height, args.screenshots, notes)
            for width, height in ((900, 700), (1280, 800)):
                large_font_after_call(browser, urls[0], width, height, notes)
            phone = phone_boxes(browser, urls[0], args.screenshots, "page")
            if len(urls) > 1:
                baseline = phone_boxes(browser, urls[1], args.screenshots, "baseline")
                check(phone == baseline, f"393x852: the phone's dock differs from the baseline page's:\n{phone}\n"
                      f"{baseline}")
                notes.append("393x852 phone: bar one row above the pane's tile, each control's box the same as the "
                             "baseline page's in Main, All, Thread list and the long thread")
            else:
                notes.append("393x852 phone: bar one row above the pane's tile, the dock spanning the window and the "
                             "same height in Main, All, Thread list and the long thread")
            version = browser.version
            browser.close()
        print(f"Chromium {version}: " + "; ".join(notes))
        return 0
    finally:
        for server, thread, api in servers:
            api.stopping.set()
            server.shutdown()
            server.server_close()
            thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
