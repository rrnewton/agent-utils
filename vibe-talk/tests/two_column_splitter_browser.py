#!/usr/bin/env python3
"""Drag the separator between the two columns, in real Chromium at 1280x800.

`#1186 two-column-splitter`. The owner, 2026-11-09: "in the two-column mode, let's go ahead and
make it possible to grab and drag the middle separator to change the width of the two columns."
The fake API and the page harness are tests/scroll_stability_browser.py's, with a thread open on
the right so both columns hold message rows. At the desk, with a mouse:

  sign in and open the channel in two columns, a thread selected on the right -> the separator
  stands between the columns with a grab pill and a col-resize cursor -> both columns scrolled
  into their history, their first on-screen rows noted -> the separator dragged left in steps:
  the left column narrows, the right widens, and each column's first on-screen row is the same
  row at the same height as before the drag (within a pixel), because dragging scrolls neither
  column and the anchor keep puts each reader back while the rows reflow -> the drag pushed past
  either edge stops with neither column under 300px -> the separator double-clicked: half and
  half again -> focused and moved with the arrow keys, its aria-valuenow saying where it stands
  -> the page reloaded: the split it was left at is the split it comes back at, kept on the
  device. No horizontal overflow and no page error at any step.

Pass --screenshots DIR to keep a PNG of each state; --web-root DIR to run against another copy of
the page.
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
import page_sources
from offline_cache_browser import Json
from scroll_stability_browser import READS_JS, Reader, StabilityApi, handler, open_main

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

RUNNER = "spaces/A/threads/long"

ANCHORS_JS = """() => {
    const anchor = (scrollerId, listId) => {
        const area = document.getElementById(scrollerId);
        const top = area.getBoundingClientRect().top;
        const bottom = top + area.clientHeight;
        for (const row of document.getElementById(listId).children) {
            const box = row.getBoundingClientRect();
            if (box.bottom > top && box.top < bottom) return { id: row.getAttribute('data-id'), top: box.top };
        }
        return null;
    };
    const box = (id) => { const r = document.getElementById(id).getBoundingClientRect();
        return { left: r.left, right: r.right, width: r.width, top: r.top, height: r.height }; };
    const split = document.getElementById('column-splitter');
    return {
        main: anchor('scroll-area', 'discord-log'),
        side: anchor('side-scroll', 'side-log'),
        left: box('scroll-area'), right: box('side-column'), separator: box('column-splitter'),
        split: Number(split.getAttribute('aria-valuenow')),
        cursor: document.getElementById('screen-main').style.cursor,
        stored: document.getElementById('screen-main').style.getPropertyValue('--two-column-split'),
    };
}"""


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
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    report: list[str] = []
    with sync_playwright() as playwright:
        report.append(walk(playwright.chromium, args))
    for line in report:
        print(line)
    return 0


def anchors(reader: Reader) -> Json:
    found = reader.page.evaluate(ANCHORS_JS)
    assert isinstance(found, dict)
    return found


def part(found: Json, key: str) -> dict[str, object]:
    value = found[key] if isinstance(found, dict) else None
    assert isinstance(value, dict), f"nothing at {key} in {found}"
    return value


def num(found: Json, *path: str) -> float:
    value: object = found
    for key in path:
        assert isinstance(value, dict), f"nothing at {path} in {found}"
        value = value[key]
    assert isinstance(value, (int, float)), f"{path} is not a number in {found}"
    return float(value)


def same_anchor(reader: Reader, before: Json, after: Json, column: str) -> None:
    was, now = part(before, column), part(after, column)
    reader.check(was["id"] == now["id"] and abs(num(now, "top") - num(was, "top")) <= 1,
                 f"the {column} column's anchor row moved in the drag: {was} -> {now}")


def walk(chromium: BrowserType, args: argparse.Namespace) -> str:
    api = StabilityApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-splitter-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": 1280, "height": 800}, color_scheme="dark")
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.add_init_script(READS_JS)
            reader = Reader(page, api, args.screenshots, "desk-1280", False)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            open_main(reader)

            # A thread on the right, so both columns hold rows whose places the drag must keep.
            page.click(f"#side-cards > li[data-id='{RUNNER}'] .side-card-open")
            reader.wait_for("() => document.querySelectorAll('#side-log > li[data-id]').length > 0",
                            "the card did not select its thread on the right")
            reader.quiet(600)

            sep_box = page.locator("#column-splitter").bounding_box()
            assert sep_box is not None
            page.mouse.move(sep_box["x"] + sep_box["width"] / 2, sep_box["y"] + sep_box["height"] / 2)
            reader.quiet(300)
            start = anchors(reader)
            reader.check(num(start, "separator", "width") >= 10, f"the separator has no grab strip: {part(start, 'separator')}")
            reader.check(part(start, "separator") is not None and start["cursor"] == "col-resize", f"the separator's cursor is {start['cursor']!r}")
            reader.check(abs(num(start, "left", "width") - num(start, "right", "width")) <= 2,
                         f"two columns do not open half and half: {part(start, 'left')} vs {part(start, 'right')}")

            # Both columns read into their history, as a reader caught up leaves them.
            page.evaluate("() => { const a = document.getElementById('scroll-area');"
                          " a.scrollTop = Math.round((a.scrollHeight - a.clientHeight) * 0.45); }")
            page.evaluate("() => { const a = document.getElementById('side-scroll');"
                          " a.scrollTop = Math.round((a.scrollHeight - a.clientHeight) * 0.45); }")
            reader.quiet(500)
            before = anchors(reader)
            reader.check(isinstance(before.get("main"), dict) and isinstance(before.get("side"), dict),
                         f"a column shows no row to anchor on: {before}")

            # The drag itself, in steps, as a hand moves: left, narrower Main.
            sep = part(before, "separator")
            x, y = num(sep, "left") + num(sep, "width") / 2, num(sep, "top") + num(sep, "height") / 2
            page.mouse.move(x, y)
            page.mouse.down()
            for step in range(1, 6):
                page.mouse.move(x - step * 36, y)
                page.wait_for_timeout(60)
            held = anchors(reader)
            reader.check(num(held, "left", "width") < num(before, "left", "width") - 100,
                         f"dragging the separator did not narrow the left column: {part(held, 'left')}")
            same_anchor(reader, before, held, "main")
            same_anchor(reader, before, held, "side")
            page.mouse.up()
            reader.quiet(400)
            dropped = anchors(reader)
            same_anchor(reader, before, dropped, "main")
            same_anchor(reader, before, dropped, "side")
            reader.notes.append(f"dragged to {num(dropped, 'split'):.0f}%, anchors held")

            # Past either edge, neither column goes under its minimum.
            sep = part(dropped, "separator")
            x, y = num(sep, "left") + num(sep, "width") / 2, num(sep, "top") + num(sep, "height") / 2
            page.mouse.move(x, y)
            page.mouse.down()
            page.mouse.move(4, y, steps=8)
            page.wait_for_timeout(120)
            edge = anchors(reader)
            reader.check(num(edge, "left", "width") >= 300, f"the left column was dragged under 300px: {part(edge, 'left')}")
            page.mouse.move(1276, y, steps=8)
            page.wait_for_timeout(120)
            edge = anchors(reader)
            reader.check(num(edge, "right", "width") >= 300, f"the right column was dragged under 300px: {part(edge, 'right')}")
            page.mouse.up()
            reader.quiet(400)

            # Double-click: half and half again. Then the keys, from the focus.
            sep_now = part(anchors(reader), "separator")
            page.mouse.dblclick(num(sep_now, "left") + 7, num(sep_now, "top") + num(sep_now, "height") / 2)
            reader.quiet(400)
            reset = anchors(reader)
            reader.check(num(reset, "split") == 50, f"double-click left the split at {num(reset, 'split')}")
            page.evaluate("() => document.getElementById('column-splitter').focus()")
            page.keyboard.press("ArrowRight")
            page.keyboard.press("ArrowRight")
            reader.quiet(300)
            keyed = anchors(reader)
            reader.check(num(keyed, "split") == 52, f"two ArrowRights left the split at {num(keyed, 'split')}")
            reader.notes.append("double-click reset to 50%, arrows moved it to 52%")
            reader.shot("split-52")

            # A reload keeps the split the reader left, on this device.
            page.reload(wait_until="load")
            reader.wait_for("() => document.getElementById('column-splitter').getAttribute('aria-valuenow') === '52'",
                            "the reloaded page forgot the split")
            reader.check(not errors, f"the page threw: {errors}")
            context.close()
            return f"{version} at 1280x800: " + "; ".join(reader.notes)
    finally:
        api.stopping.set()
        api.timeline_gate.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
