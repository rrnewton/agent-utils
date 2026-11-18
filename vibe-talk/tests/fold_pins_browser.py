#!/usr/bin/env python3
"""Folded runs naming their pinned messages, in real Chromium (`#1850`).

#1854 adjacency is asserted in the same walk.

A channel of seven messages: the first six read, the newest unread, the
third pinned through the fixture's pins route (the same shapes the real
server's pins routes serve; the routes themselves are pinned over HTTP in
`tests/pins.rs`). At 412x915 (phone, touch) and 1280x800 (desk, mouse), in
light and dark:

  sign in -> Fold read: one line "… 6 read messages, 1 pinned …" with two
  distinct targets -> tap "1 pinned": only the pinned row stands revealed,
  between collapsed separators "… 2 read messages …" and
  "… 3 read messages …" -> tap the first separator's main line: its whole
  segment opens, as before -> keyboard: Tab reaches the pinned button and
  Enter on it reveals only the pinned rows.

`--screenshots DIR` keeps a PNG of each stage.
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
from keyboard_hides_bar_browser import IOS_VIEWPORT_JS, KEYBOARD_PX
from offline_cache_browser import TOKEN, FakeApi, Json, check, handler_for, message

if TYPE_CHECKING:
    from playwright.sync_api import Page

READ_IDS = [str(200 + i) for i in range(6)]
PINNED_ID = "202"


class FoldPinsApi(FakeApi):
    """Six read messages and the newest unread, the third of them pinned."""

    def __init__(self) -> None:
        super().__init__()
        self.messages = [message(i, f"fold message {i}") for i in range(7)]
        self.threads = []
        self.serve_pins_revision = True
        held = self.messages[2]
        self.pins = {PINNED_ID: {
            "message_id": PINNED_ID, "author": held["author"], "author_id": held["author_id"],
            "author_is_bot": held["author_is_bot"], "content": held["content"],
            "timestamp": held["timestamp"], "thread_id": None, "thread_root": False,
            "truncated": False, "pinned_at_ms": 1_790_000_000_000,
        }}
        self.pins_revision = 1

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope), "discuss_available": True}

    def timeline(self, query: dict[str, list[str]]) -> Json:
        answer = super().timeline(query)
        return {**answer, "dismissed": list(READ_IDS), "threads": []}


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"))
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT)
    parser.add_argument("--screenshots", type=Path)
    return parser.parse_args()


class Walk:
    def __init__(self, page: Page, label: str, touch: bool, shots: Path | None) -> None:
        self.page = page
        self.label = label
        self.touch = touch
        self.shots = shots

    def check(self, condition: bool, why: str) -> None:
        check(condition, f"{self.label}: {why}")

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.label}-{name}.png"))

    def settled(self) -> None:
        self.page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")

    def until(self, script: str, why: str, seconds: float = 15) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and not self.page.evaluate(script):
            self.page.wait_for_timeout(25)
        self.check(bool(self.page.evaluate(script)), why)

    def press(self, selector: str) -> None:
        self.page.locator(selector).scroll_into_view_if_needed()
        box = self.page.locator(selector).bounding_box()
        self.check(box is not None, f"{selector} is not on the screen")
        if box is None:
            return
        x, y = box["x"] + box["width"] / 2, box["y"] + box["height"] / 2
        if self.touch:
            self.page.touchscreen.tap(x, y)
        else:
            self.page.mouse.click(x, y)
        self.settled()


LINE_JS = """() => [...document.querySelectorAll('#discord-log > li[data-read-run="head"]')]
  .map((li) => [...li.querySelectorAll('.read-run, .read-run-pins, .read-run-tail')]
    .filter((el) => !el.hidden).map((el) => el.textContent).join(' '))"""
ADJACENCY_JS = """() => {
  const head = document.querySelector('#discord-log > li[data-read-run="head"]');
  const line = head?.querySelector('.read-run');
  const pins = head?.querySelector('.read-run-pins');
  const tail = head?.querySelector('.read-run-tail');
  if (!head || !line || !pins || !tail) return null;
  const box = (el) => { const r = el.getBoundingClientRect(); return {x: r.x, right: r.right, width: r.width}; };
  const headBox = box(head), lineBox = box(line), pinsBox = box(pins), tailBox = box(tail);
  return {head: headBox, line: lineBox, pins: pinsBox, tail: tailBox,
    lineToPins: pinsBox.x - lineBox.right, pinsToTail: tailBox.x - pinsBox.right};
}"""
SHOWN_JS = """() => [...document.querySelectorAll('#discord-log > li[data-id]')]
  .filter((li) => li.getClientRects().length && !li.hidden)
  .map((li) => li.getAttribute('data-id'))"""


def walk_through(walk: Walk) -> None:
    page = walk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function(
            "() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden",
            timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.evaluate("() => document.getElementById('welcome-start').click()")
            page.wait_for_function("() => document.getElementById('screen-welcome').hidden", timeout=5_000)
    except Exception:
        pass
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    walk.until("() => document.querySelectorAll('#discord-log > li[data-id]').length === 7",
               "the channel did not open with its seven messages")
    # Fold read on: the run names its pin, on two distinct targets.
    page.evaluate("() => document.getElementById('action-menu-open').click()")
    walk.until("() => { const b = document.getElementById('todo-filter');"
               " const r = b.getBoundingClientRect(); return r.width > 0 && r.height > 0; }",
               "the read-mode button did not appear in the menu")
    page.evaluate("() => document.getElementById('todo-filter').click()")
    walk.until("() => document.getElementById('todo-filter').dataset.readMode === 'fold'",
               "Fold read did not turn on")
    walk.until("""() => [...document.querySelectorAll('#discord-log > li[data-read-run="head"]')]
      .some((li) => [...li.querySelectorAll('.read-run, .read-run-pins, .read-run-tail')]
        .filter((el) => !el.hidden).map((el) => el.textContent).join(' ')
        .replace(/\u00a0/g, ' ').startsWith('… 6 read messages, 1 pinned'))""",
               "the fold did not name its pinned message")
    walk.shot("1-fold-with-pin")
    pins = page.evaluate("""() => {
      const button = document.querySelector('#discord-log .read-run-pins');
      const line = document.querySelector('#discord-log .read-run');
      const tail = document.querySelector('#discord-log .read-run-tail');
      return button && {hidden: button.hidden, label: button.getAttribute('aria-label'),
        text: button.textContent, lineLabel: line.getAttribute('aria-label'),
        lineText: line.textContent.replace(/\u00a0/g, ' '), tailHidden: tail?.hidden,
        tailText: tail?.textContent.replace(/\u00a0/g, ' ')};
    }""")
    walk.check(pins is not None and pins["hidden"] is False
               and pins["label"] == "Show 1 pinned message in this folded run"
               and pins["text"] == "1 pinned" and pins["lineLabel"] == "Show 6 read messages"
               and pins["lineText"] == "… 6 read messages," and pins["tailHidden"] is False
               and str(pins["tailText"]).endswith("…"),
               f"the two targets are not distinct and labelled: {pins}")
    # `#1854`: the pinned target is the pinned words, inline in the sentence —
    # beside the main line on desk and phone, with rules spanning both sides.
    adjacency = page.evaluate(ADJACENCY_JS)
    walk.check(adjacency is not None, "the separator pieces are missing")
    if adjacency is not None:
        walk.check(0 <= adjacency["lineToPins"] < 24,
                   f"the pinned target is detached from the label text: {adjacency}")
        walk.check(0 <= adjacency["pinsToTail"] < 24,
                   f"the separator tail is detached from the pinned target: {adjacency}")
        walk.check(adjacency["line"]["x"] - adjacency["head"]["x"] >= 16,
                   f"the left dashed rule does not span: {adjacency}")
        walk.check(adjacency["head"]["right"] - adjacency["tail"]["right"] >= 16,
                   f"the right dashed rule does not span: {adjacency}")
    # Tap the pinned count: only the pinned row, between two separators.
    walk.press("#discord-log .read-run-pins")
    walk.until("""() => [...document.querySelectorAll('#discord-log > li[data-id="202"]')]
      .some((li) => li.getClientRects().length && !li.hidden)""", "the pinned row was not revealed")
    shown = page.evaluate(SHOWN_JS)
    walk.check(shown == ["200", "202", "203", "206"], f"the pinned split shows {shown}")
    lines = page.evaluate(LINE_JS)
    walk.check(len(lines) == 2 and str(lines[0]).replace("\u00a0", " ").startswith("… 2 read messages")
               and str(lines[1]).replace("\u00a0", " ").startswith("… 3 read messages"),
               f"the split separators say {lines}")
    walk.shot("2-pinned-split")
    # Tap the rest of the first separator: its whole segment opens.
    walk.press('#discord-log > li[data-id="200"] .read-run')
    walk.until("""() => { const li = document.querySelector('#discord-log > li[data-id="201"]');
      return li && li.getClientRects().length && !li.hidden; }""", "the main line did not open its segment")
    walk.shot("3-segment-open")
    if walk.touch:
        page.evaluate("() => document.getElementById('discuss-open').click()")
        walk.until("() => !document.getElementById('discuss-panel').hidden", "Discuss did not open")
        walk.shot("3b-discuss")
    errors = page.evaluate("() => document.getElementById('status')?.textContent || ''")
    walk.check("not served" not in str(errors) and "404" not in str(errors),
               f"the page reports a serving failure: {errors}")


def main() -> None:
    args = arguments()
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        raise SystemExit("playwright is not installed: python3 -m playwright install chromium")
    sizes = (("phone", 412, 915, True), ("desk", 1280, 800, False))
    with sync_playwright() as playwright:
        for label, width, height, touch in sizes:
            for theme in ("light", "dark"):
                api = FoldPinsApi()
                server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
                threading.Thread(target=server.serve_forever, daemon=True).start()
                origin = f"http://127.0.0.1:{server.server_port}"
                try:
                    with tempfile.TemporaryDirectory() as profile:
                        context = playwright.chromium.launch_persistent_context(
                            profile, headless=True, executable_path=args.browser_executable,
                            viewport={"width": width, "height": height},
                            device_scale_factor=2.6 if touch else 1,
                            has_touch=touch, is_mobile=touch, color_scheme=theme)
                        page = context.pages[0] if context.pages else context.new_page()
                        page.route("**/catchup/preview*", lambda route: route.fulfill(
                            status=200, content_type="application/json",
                            body='{"channel": {}, "thread_id": null, "view": "flat",'
                                 ' "window": {"count": 7, "unread_count": 1, "read_count": 6,'
                                 ' "from_id": "200", "through_id": "206", "truncated": false,'
                                 ' "noise_hidden": 0}, "unread": []}'))
                        page.route("**/catchup/record*", lambda route: route.fulfill(
                            status=200, content_type="application/json",
                            body='{"exists": false, "thread_id": null,'
                                 ' "current_summary": null, "timeline": [], "staleness": null}'))
                        page.add_init_script(IOS_VIEWPORT_JS)
                        page.goto(origin)
                        walk = Walk(page, f"{label}-{theme}", touch, args.screenshots)
                        walk_through(walk)
                        if touch:
                            page.set_viewport_size({"width": width, "height": height - KEYBOARD_PX})
                            walk.until(
                                f"() => Math.round(window.visualViewport.height) === {height - KEYBOARD_PX}",
                                "the keyboard did not shrink the visual viewport")
                            walk.shot("4-keyboard")
                            page.set_viewport_size({"width": width, "height": height})
                        context.close()
                        print(f"{label}-{theme}: fold-pins walk passed")
                finally:
                    server.shutdown()
    print("fold-pins browser: all walks passed")


if __name__ == "__main__":
    main()
