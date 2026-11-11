#!/usr/bin/env python3
"""Collapse a long message in real Chromium: which row the reader keeps, at phone and desk widths.

`#1215 collapse-anchor-below` (owner fast-track, reply 380). The rule, all three cases:
(a) the collapsed message's top is on screen: its top does not move;
(b) its top is above the screen and the next message is on screen: the next message does not move
    (the history above absorbs the change);
(c) neither is on screen (inside a message taller than the viewport): the collapsed message
    ends centered in the room.
Each case collapses with the message's own fold control and judges only drawn geometry —
offsets sampled after the page settles, never elapsed time or frame counts. It serves the
checked-out page and the fake API of `offline_cache_browser.py`: twelve long messages,
the fourth a giant several viewports tall, alternating speakers so no two combine into
one row. At 393x852 by touch and at 1280x800 on a desk.
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
import page_sources
from offline_cache_browser import TOKEN, Json, check, handler_for, said

if TYPE_CHECKING:
    from playwright.sync_api import Page

PROSE = (
    "The overnight run finished on all four shards. The second shard replayed its write-ahead "
    "log after the restart, so it took a little longer than usual, but every checksum matched "
    "and the restore drill passed on the first try. The credentials the job uses were rotated, "
    "the runbook has the new paths, and a follow-up is filed. "
)
GIANT = PROSE * 60
LONG = PROSE * 2
COUNT = 12
GIANT_INDEX = 3
STATE_S = 30.0
STILL_PX = 1.0
CENTER_PX = 2.0


def message_id(index: int) -> str:
    return str(200 + index)


class AnchorApi(fixture.FakeApi):
    """One channel of long messages from alternating speakers, the fourth a giant."""

    def __init__(self) -> None:
        super().__init__()
        self.messages: list[Json] = [
            said(index, (GIANT if index == GIANT_INDEX else LONG) + f"(message {index})",
                 "coder" if index % 2 == 0 else "human")
            for index in range(COUNT)
        ]
        self.threads = []


GEOMETRY = """(id) => {
  const area = document.getElementById('scroll-area');
  const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
  if (!area || !row) return null;
  const a = area.getBoundingClientRect();
  const r = row.getBoundingClientRect();
  const next = row.nextElementSibling;
  const n = next ? next.getBoundingClientRect() : null;
  let head = a.top;
  for (const floating of ['channel-freshness', 'search-float']) {
    const node = document.getElementById(floating);
    if (node && !node.hidden) head = Math.max(head, node.getBoundingClientRect().bottom);
  }
  let over = 0;
  for (const tools of ['scroll-tools', 'read-steer', 'status-line']) {
    const node = document.getElementById(tools);
    if (node && !node.hidden) over += node.getBoundingClientRect().height;
  }
  return {
    top: r.top - a.top, bottom: r.bottom - a.top,
    center: (r.top + r.bottom) / 2 - a.top,
    nextTop: n ? n.top - a.top : null,
    roomCenter: ((head - a.top) + (a.height - over)) / 2,
    areaHeight: a.height, collapsed: row.getAttribute('data-collapsed'),
  };
}"""


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    return parser.parse_args()


def until(page: Page, script: str, what: str) -> None:
    deadline = time.monotonic() + STATE_S
    while time.monotonic() < deadline and not page.evaluate(script):
        page.wait_for_timeout(25)
    check(bool(page.evaluate(script)), what)


def settled(page: Page) -> None:
    """Wait until the first on-screen row stands still across two sampled frames."""
    script = """() => {
      const area = document.getElementById('scroll-area');
      const list = document.getElementById('discord-log');
      if (!area || !list) return null;
      const top = area.getBoundingClientRect().top;
      for (const row of list.children) {
        if (!row.hasAttribute('data-id') || !row.getClientRects().length) continue;
        const box = row.getBoundingClientRect();
        if (box.bottom > top) return [row.getAttribute('data-id'), Math.round((box.top - top) * 10) / 10, area.scrollTop];
      }
      return null;
    }"""
    deadline = time.monotonic() + STATE_S
    previous: object = page.evaluate(script)
    stable = 0
    while time.monotonic() < deadline:
        page.wait_for_timeout(100)
        current: object = page.evaluate(script)
        stable = stable + 1 if current == previous and current is not None else 0
        if stable >= 2:
            return
        previous = current
    raise AssertionError("the list never stood still")


def geometry(page: Page, index: int) -> dict[str, object]:
    found = page.evaluate(GEOMETRY, message_id(index))
    check(isinstance(found, dict), f"message {index} is not drawn")
    assert isinstance(found, dict)
    return found


def scroll_row_to(page: Page, index: int, offset: float) -> None:
    """Put message `index`'s top at `offset` px from the scroller's top, by the reader's own scroll."""
    page.evaluate("""([id, offset]) => {
      const area = document.getElementById('scroll-area');
      const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
      area.scrollTop += row.getBoundingClientRect().top - area.getBoundingClientRect().top - offset;
    }""", [message_id(index), offset])
    settled(page)


def ensure_expanded(page: Page, index: int) -> None:
    """Long messages may open folded; expand the target so the case's act is a collapse."""
    if page.evaluate(f"() => document.querySelector('#discord-log > li[data-id=\"{message_id(index)}\"]')"
                     ".getAttribute('data-collapsed')") == "true":
        page.evaluate(f"() => document.querySelector('#discord-log > li[data-id=\"{message_id(index)}\"] button.fold').click()")
        until(page, f"() => document.querySelector('#discord-log > li[data-id=\"{message_id(index)}\"]')"
                    ".getAttribute('data-collapsed') !== 'true'", f"message {index} never expanded")
        settled(page)


def collapse(page: Page, index: int) -> dict[str, object]:
    """Tap message `index`'s fold control; answer its geometry once drawn collapsed."""
    until(page, f"() => !!document.querySelector('#discord-log > li[data-id=\"{message_id(index)}\"] button.fold')",
          f"message {index} has no fold control")
    page.evaluate(f"() => document.querySelector('#discord-log > li[data-id=\"{message_id(index)}\"] button.fold').click()")
    until(page, f"() => document.querySelector('#discord-log > li[data-id=\"{message_id(index)}\"]')"
                ".getAttribute('data-collapsed') === 'true'", f"message {index} never folded")
    settled(page)
    return geometry(page, index)


def case_a(page: Page, width: int) -> str:
    """(a) Top on screen: the collapsed message's top does not move."""
    ensure_expanded(page, 6)
    scroll_row_to(page, 6, 120.0)
    before = geometry(page, 6)
    after = collapse(page, 6)
    moved = abs(float(after["top"]) - float(before["top"]))  # type: ignore[arg-type]
    check(moved <= STILL_PX, f"(a) the top moved {moved:.1f}px ({before['top']} -> {after['top']}) at {width}px")
    return f"(a) top held ({moved:.1f}px)"


def case_b(page: Page, width: int) -> str:
    """(b) Top above, next on screen: the next message does not move."""
    ensure_expanded(page, 7)
    scroll_row_to(page, 7, -90.0)
    before = geometry(page, 7)
    check(isinstance(before["nextTop"], (int, float)) and 0 <= float(before["nextTop"]) < float(before["areaHeight"]),  # type: ignore[arg-type]
          f"(b) the next message is not on screen before the fold: {before} at {width}px")
    after = collapse(page, 7)
    moved = abs(float(after["nextTop"]) - float(before["nextTop"]))  # type: ignore[arg-type]
    check(moved <= STILL_PX, f"(b) the next message moved {moved:.1f}px ({before['nextTop']} -> {after['nextTop']}) at {width}px")
    return f"(b) next held ({moved:.1f}px)"


def case_c(page: Page, width: int) -> str:
    """(c) Inside the giant: the collapsed message ends centered in the room."""
    ensure_expanded(page, GIANT_INDEX)
    scroll_row_to(page, GIANT_INDEX, 0.0)
    page.evaluate("""(id) => {
      const area = document.getElementById('scroll-area');
      const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
      area.scrollTop += row.getBoundingClientRect().height / 3;
    }""", message_id(GIANT_INDEX))
    settled(page)
    before = geometry(page, GIANT_INDEX)
    check(float(before["top"]) < 0 and float(before["bottom"]) > float(before["areaHeight"]),  # type: ignore[arg-type]
          f"(c) the giant does not span the room before the fold: {before} at {width}px")
    after = collapse(page, GIANT_INDEX)
    off = abs(float(after["center"]) - float(after["roomCenter"]))  # type: ignore[arg-type]
    check(off <= CENTER_PX, f"(c) the collapsed giant is {off:.1f}px off center at {width}px "
                            f"(center {after['center']}, room {after['roomCenter']})")
    return f"(c) centered ({off:.1f}px off)"


def open_main(page: Page, touch: bool) -> None:
    """Sign in and open the channel's Main, as the stability walk opens it."""
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    until(page, "() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
          "the channel did not open")
    if touch:
        page.select_option("#thread-select", "main")
    else:
        page.click("#column-toggle")
        until(page, "() => document.getElementById('side-column').getClientRects().length > 0",
              "the desk layout did not draw two columns")
    until(page, f"() => document.querySelectorAll('#discord-log > li[data-id]').length >= {COUNT}",
          "Main did not draw every message")
    settled(page)


def run(chromium: object, args: argparse.Namespace, label: str, width: int, height: int, touch: bool,
        case: object) -> str:
    """One case at one size, against a fresh fake API and a fresh browser profile."""
    from playwright.sync_api import BrowserType  # noqa: F401  (typing only)
    api = AnchorApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-collapse-") as profile:
            context = chromium.launch_persistent_context(  # type: ignore[attr-defined]
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark",
                device_scale_factor=2.625 if touch else 1, is_mobile=touch, has_touch=touch)
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            open_main(page, touch)
            note = case(page, width)  # type: ignore[operator]
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{label} {width}x{height}: {note}"
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    report: list[str] = []
    with sync_playwright() as playwright:
        for width, height, touch in ((393, 852, True), (1280, 800, False)):
            for case in (case_a, case_b, case_c):
                report.append(run(playwright.chromium, args, case.__name__, width, height, touch, case))
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
