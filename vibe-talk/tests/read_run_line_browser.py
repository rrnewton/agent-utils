#!/usr/bin/env python3
"""A folded run's line with pins stays one centred sentence between its two rules (`#2419`).

`#2419 vibe-talk-read-run-separator-over-label`. The owner, 2026-11-27: "Corrupt rendering of
separator line . "23 read messages" not centered.. looks like separator line is rendered OVER the
text labels." A folded run that holds pinned messages draws its sentence in pieces (`#1854`): the
line "… N read messages,", the "M pinned" target and the days tail, with First 5 and Last 5 at the
margins of a long run (`#1718`). Its two dashed rules are the head row's own ::before and ::after.
Row states draw on those same pseudo-elements: the pin edge, the read veil of a reply or of a row
under a seam, the selection. Each made the rule `position: absolute`, so when the run's first
message was pinned, the sentence stood off centre and a rule was drawn over the line.

This walk serves the real page with a fake API: 23 read messages and one unread, Fold read on, the
run's line naming its pins. At 412x915 (touch) and 1280x800, light and dark, it checks four heads:
the run's first message pinned, a reply (the read veil's state), selected, and an ordinary one. By geometry and
computed style only, every wait a condition:

  the sentence (line, pinned target, tail) stands centred between the First and Last buttons, to a
  pixel; both rules are in the line's flow (static, never absolute) and as wide as each other, so
  neither can be drawn over the words or the buttons; the line stays one row tall.

Pass --screenshots DIR to keep a PNG per case; --web-root DIR to run it against another copy of the
page (main before the change fails the pinned-head case).
"""

from __future__ import annotations

import argparse
import json
import os
import threading
from datetime import timedelta
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING, cast

import offline_cache_browser as fixture
from offline_cache_browser import EPOCH, TOKEN, FakeApi, Json, check, handler_for, message
import browser_scratch
import browser_waits
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import Page

WAIT_MS = int(browser_waits.HANG_BACKSTOP_MS)
N_READ = 23

HEAD_JS = """() => [...document.querySelectorAll('#discord-log > li[data-read-run="head"]')]
  .find((li) => li.querySelector(':scope > .read-run-pins:not([hidden])')) || null"""

MEASURE_JS = """() => {
  const li = (""" + HEAD_JS + """)();
  if (!li) return null;
  const box = (el) => { if (!el || !el.getClientRects().length) return null; const r = el.getBoundingClientRect();
    return {left: r.left, right: r.right, top: r.top, bottom: r.bottom}; };
  const rule = (which) => { const s = getComputedStyle(li, which);
    return {content: s.content, position: s.position, width: parseFloat(s.width) || 0, border: s.borderTopStyle}; };
  const part = (cls) => box(li.querySelector(':scope > .' + cls));
  return {row: box(li), list: box(document.getElementById('discord-log')), line: part('read-run'), pins: part('read-run-pins'), tail: part('read-run-tail'),
    first: box(li.querySelector('.read-run-peel[data-peel="first"]')), last: box(li.querySelector('.read-run-peel[data-peel="last"]')),
    before: rule('::before'), after: rule('::after'), gap: parseFloat(getComputedStyle(li).columnGap) || 0,
    selected: li.getAttribute('data-selected'), pinned: li.getAttribute('data-pinned'), reply: li.getAttribute('data-is-reply')};
}"""

# (label, read indices pinned, the run's first message a reply to an earlier unread one, select the
# run's head).
CASES = (
    ("head-pinned", (0, 11), False, False),
    ("head-reply", (4, 11), True, False),
    ("head-selected", (0, 11), False, True),
    ("plain-head", (4, 11), False, False),
)

# The owner's view (2026-11-27, desk, dark, about 1117 px): the line selected (its dashed ring) after
# its Last 5 was pressed, a rule running through the words and the sentence far to the right. The
# run's first message is a reply, the read veil's state, which drew that rule across the whole row.
OWNER_CASE = ("owner-reply-peeled-selected", (4, 11), True, True)


class Channel(FakeApi):
    """A run of read messages, some pinned, and the newest unread; optionally an earlier unread
    message the run's first message replies to, so the run's head row is a reply."""

    def __init__(self, pinned: tuple[int, ...], head_reply: bool) -> None:
        super().__init__()
        rows = [message(i, f"run message {i}: a routine read note") for i in range(N_READ + 1)]
        if head_reply:
            older = message(N_READ + 1, "an earlier note, still unread")
            older["id"] = "190"
            older["timestamp"] = (EPOCH - timedelta(minutes=5)).isoformat().replace("+00:00", "Z")
            rows.insert(0, older)
            rows[1]["reply_to"] = "190"
        self.messages = rows
        self.read = [str(200 + i) for i in range(N_READ)]
        self.threads = []
        self.serve_pins_revision = True
        self.pins = {}
        for index in pinned:
            held = next(row for row in rows if row["id"] == str(200 + index))
            self.pins[str(held["id"])] = {
                "message_id": held["id"], "author": held["author"], "author_id": held["author_id"],
                "author_is_bot": held["author_is_bot"], "content": held["content"], "timestamp": held["timestamp"],
                "thread_id": None, "thread_root": False, "truncated": False, "pinned_at_ms": 1_790_000_000_000}
        self.pins_revision = 1

    def timeline(self, query: dict[str, list[str]]) -> Json:
        answer = super().timeline(query)
        return {**answer, "dismissed": list(self.read), "threads": []}


def frames(page: Page) -> None:
    page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")


def fold_read(page: Page, rows: int) -> None:
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_function("() => !document.getElementById('screen-welcome').hidden"
                           " || !document.getElementById('screen-main').hidden", timeout=WAIT_MS)
    if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
        page.evaluate("() => document.getElementById('welcome-start').click()")
    page.wait_for_selector("#search-toggle", state="visible", timeout=WAIT_MS)
    page.wait_for_function(f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {rows}",
                           timeout=WAIT_MS)
    page.evaluate("() => document.getElementById('action-menu-open').click()")
    page.wait_for_function("() => document.getElementById('todo-filter').getClientRects().length > 0", timeout=WAIT_MS)
    page.evaluate("() => document.getElementById('todo-filter').click()")
    page.wait_for_function("() => document.getElementById('todo-filter').dataset.readMode === 'fold'", timeout=WAIT_MS)
    page.wait_for_function(f"() => !!({HEAD_JS})()", timeout=WAIT_MS)
    page.keyboard.press("Escape")
    frames(page)


def peel_last(page: Page) -> None:
    """Last 5 pressed: five messages from the run's end stand open, and the line counts the rest."""
    before = page.evaluate(f"() => ({HEAD_JS})().querySelector('.read-run').textContent")
    page.evaluate(f"() => ({HEAD_JS})().querySelector('.read-run-peel[data-peel=\"last\"]').click()")
    page.wait_for_function(f"(was) => {{ const li = ({HEAD_JS})(); return !!li && li.querySelector('.read-run').textContent !== was; }}",
                           arg=before, timeout=WAIT_MS)
    frames(page)


def select_head(page: Page) -> None:
    """The keyboard's selection walked up onto the run's line, one ArrowUp and its frames at a time."""
    selected = f"() => {{ const li = ({HEAD_JS})(); return !!li && li.getAttribute('data-selected') === 'true'; }}"
    for _ in range(N_READ + 2):
        if page.evaluate(selected):
            return
        page.keyboard.press("ArrowUp")
        frames(page)
    check(bool(page.evaluate(selected)), "the selection never reached the run's line")


def judge(label: str, found: Json | None) -> str:
    check(found is not None, f"{label}: no folded run with pins was drawn")
    assert found is not None

    def part(key: str) -> dict[str, float]:
        value = found.get(key)
        check(isinstance(value, dict), f"{label}: the run's {key} is not drawn: {found}")
        return cast("dict[str, float]", value)

    def rule(key: str) -> dict[str, str | float]:
        return cast("dict[str, str | float]", found[key])

    line, pins, tail, first, last = (part(key) for key in ("line", "pins", "tail", "first", "last"))
    before, after = rule("before"), rule("after")
    # Both rules in the line's own flow: a rule taken out of it is drawn where some row state puts it.
    for side, drawn in (("left", before), ("right", after)):
        check(drawn["content"] != "none" and drawn["border"] == "dashed",
              f"{label}: the line's {side} rule is not drawn: {drawn}")
        check(drawn["position"] == "static",
              f"{label}: the line's {side} rule left the line's flow ({drawn['position']}), so it is drawn over the line: {drawn}")
    left_width, right_width = float(before["width"]), float(after["width"])
    check(abs(left_width - right_width) <= 1.0,
          f"{label}: the rules are {left_width:.1f}px and {right_width:.1f}px, so the sentence is off centre")
    left_room = line["left"] - first["right"]
    right_room = last["left"] - tail["right"]
    check(abs(left_room - right_room) <= 1.0,
          f"{label}: the sentence is not centred between First and Last ({left_room:.1f}px left, {right_room:.1f}px right)")
    row, rows = part("row"), part("list")
    check(row["bottom"] - row["top"] < 80, f"{label}: the line grew to {row['bottom'] - row['top']:.0f}px")
    # As wide as the list and no wider, as every line in the list is (`#250`): never out past it into
    # the scrubber's gutter.
    check(row["left"] >= rows["left"] - 0.5 and row["right"] <= rows["right"] + 0.5 and last["right"] <= rows["right"] + 0.5,
          f"{label}: the line runs out of the list: row {row}, list {rows}, Last {last}")
    check(pins["left"] >= line["right"] - 0.5 and tail["left"] >= pins["right"] - 0.5,
          f"{label}: the sentence's pieces overlap: {line} {pins} {tail}")
    return (f"{label}: sentence centred ({left_room:.1f}px / {right_room:.1f}px), rules static "
            f"{left_width:.1f}px / {right_width:.1f}px")


def walk(pw: object, args: argparse.Namespace, work: Path, label: str, width: int, height: int, touch: bool,
         scheme: str, case: tuple[str, tuple[int, ...], bool, bool]) -> str:
    name, pinned, head_reply, select = case
    peel = name == OWNER_CASE[0]
    api = Channel(pinned, head_reply)
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    tag = f"{label}-{scheme}-{name}"
    try:
        profile = work / f"profile-{tag}"
        profile.mkdir()
        context = pw.chromium.launch_persistent_context(  # type: ignore[attr-defined]
            str(profile), headless=True, executable_path=args.browser_executable,
            viewport={"width": width, "height": height}, device_scale_factor=2.625 if touch else 1,
            is_mobile=touch, has_touch=touch, color_scheme=scheme)
        try:
            browser_waits.hot_host_timeouts(context)
            page = context.pages[0] if context.pages else context.new_page()
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            fold_read(page, len(api.messages))
            if peel:
                peel_last(page)
            if select:
                select_head(page)
            frames(page)
            found = page.evaluate(MEASURE_JS)
            if args.screenshots is not None:
                page.locator(f"#discord-log > li[data-read-run=\"head\"]:has(> .read-run-pins:not([hidden]))").first \
                    .screenshot(path=str(args.screenshots / f"{tag}.png"))
            if name == "head-pinned":
                check(found is not None and found["pinned"] == "true", f"{tag}: the run's first message is not pinned: {found}")
            if name == "head-reply":
                check(found is not None and found["reply"] == "true", f"{tag}: the run's first row is not a reply: {found}")
            if name == "head-selected":
                check(found is not None and found["selected"] == "true", f"{tag}: the run's line is not selected: {found}")
        finally:
            context.close()
    finally:
        server.shutdown()
    return judge(tag, found)


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT, help="directory holding the page to serve")
    parser.add_argument("--screenshots", type=Path, help="directory to write the PNGs into")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    from playwright.sync_api import sync_playwright
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    report: list[str] = []
    # `#2382 summary-link-test-host-tmp`: never the host temp dir; see browser_scratch.
    with browser_scratch.run_scratch("read-run-line-") as work:
        fixture.WEB_ROOT = page_sources.served(args.web_root)
        with sync_playwright() as pw:
            for label, width, height, touch in (("phone", 412, 915, True), ("desk", 1280, 800, False)):
                for scheme in ("light", "dark"):
                    for case in CASES:
                        report.append(walk(pw, args, work, label, width, height, touch, scheme, case))
            for scheme in ("dark", "light"):
                report.append(walk(pw, args, work, "desk-1117", 1117, 800, False, scheme, OWNER_CASE))
    for line in report:
        print(line)
    print("read run line browser walk: ok")
    print(json.dumps({"cases": len(report)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
