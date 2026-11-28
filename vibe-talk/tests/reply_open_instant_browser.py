#!/usr/bin/env python3
"""Reply opens its box in the frame of the tap: nothing from the server, no whole-list work first (`#2428`).

`#2428 vibe-talk-reply-open-local-instant`. The owner, 2026-11-27: "Pressing reply and popping open the
reply window should be 100% LOCAL AND INSTANT. It should not depend on anything server side." On a long
channel a Reply on a message with replies gathered them under it first, and the gathering redrew the
whole list (every row's arrows measured), 0.6 s before the box could show.

This walk serves the real page and a fake API: about 1,200 rows (over 100k px, windowed off, as the
owner's lists are), every fiftieth message answered three times further down. At 1280x800 and at
412x915 by touch, a Reply on a message with replies, then one on a message with none. Counted, never
timed:

  from the tap to the frame after it, no request goes out; the box stands open and on screen at that
  first frame; the tap's own work reads a handful of element boxes and redraws no rows (not one per
  row of the list, as a whole-list redraw does); once that frame is painted the replies are gathered
  under the message and the box under them; the lift that keeps them clear of the box comes within
  two frames of the box's first, and no reply stands under the box on any frame (liaison 1112).

The tap's script time is printed as a report only. Pass --web-root DIR to run it against another
copy of the page (main before the change redraws the whole list inside the tap).
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, check, message
import browser_scratch
import browser_waits
import page_sources
import search_typing_fast_browser as big

if TYPE_CHECKING:
    from playwright.sync_api import Page

WAIT_MS = int(browser_waits.HANG_BACKSTOP_MS)
EXTRA_ROWS = 1200
# The tap's own work, up to its first frame: what opening one box needs, far below one per row.
MAX_TAP_RECT_READS = 80
MAX_TAP_ROWS_REDRAWN = 3

TAP_JS = r"""(() => {
  const tap = {armed: false, fetches: [], rects: 0, redrawn: new Set()};
  Object.defineProperty(window, '__replyTap', {value: tap});
  const send = window.fetch.bind(window);
  window.fetch = (input, init) => { if (tap.armed) tap.fetches.push(String((input && input.url) || input).replace(location.origin, '')); return send(input, init); };
  const rect = Element.prototype.getBoundingClientRect;
  Element.prototype.getBoundingClientRect = function () { if (tap.armed) tap.rects += 1; return rect.call(this); };
  const watch = () => {
    const log = document.getElementById('discord-log');
    if (!log) { requestAnimationFrame(watch); return; }
    new MutationObserver((records) => {
      if (!tap.armed) return;
      for (const record of records) {
        if (record.target === log) for (const node of [...record.addedNodes, ...record.removedNodes]) if (node.nodeType === 1 && node.hasAttribute('data-id')) tap.redrawn.add(node.getAttribute('data-id'));
      }
    }).observe(log, {childList: true});
  };
  watch();
  document.addEventListener('click', (event) => {
    const button = event.target && event.target.closest && event.target.closest('.reply-button');
    if (!button) return;
    tap.armed = true; tap.fetches = []; tap.rects = 0; tap.redrawn = new Set(); tap.started = performance.now();
    const row = button.closest('li[data-id]');
    const target = row ? row.getAttribute('data-id') : null;
    // `#2428` (liaison 1112): every frame from the first, where the message stands and whether any of
    // its replies stands under the box, read with the page's own geometry before anything else of ours.
    tap.frames = [];
    const look = () => {
      const box = document.getElementById('column-reply');
      const b = box && !box.hidden ? rect.call(box) : null;
      const li = target ? document.querySelector(`#discord-log > li[data-id='${target}']`) : null;
      const covered = (window.__replyIds || []).some((id) => {
        const reply = document.querySelector(`#discord-log > li[data-id='${id}']`);
        if (!reply || !b || !reply.getClientRects().length) return false;
        const r = rect.call(reply);
        return r.bottom > b.top + 0.5 && r.top < b.bottom - 0.5 && r.bottom > 0 && r.top < innerHeight;
      });
      tap.frames.push({top: li ? rect.call(li).top : null, covered});
      if (tap.frames.length < 8) requestAnimationFrame(look);
    };
    requestAnimationFrame(() => {
      const box = document.getElementById('column-reply');
      const r = box && !box.hidden ? box.getBoundingClientRect() : null;
      tap.firstFrame = performance.now() - tap.started;
      tap.shownAtFirstFrame = !!(r && r.height > 0 && r.bottom > 0 && r.top < innerHeight);
      tap.armed = false;
      tap.counted = {fetches: [...tap.fetches], rects: tap.rects, redrawn: tap.redrawn.size};
      look();
    });
  }, true);
})();"""


class Channel(big.BigApi):
    """The lantern channel lengthened past 100k px of rows; every fiftieth message answered three times."""

    def __init__(self) -> None:
        super(big.BigApi, self).__init__()
        base = len(self.messages)
        for extra in range(EXTRA_ROWS):
            self.messages.append(message(base + extra, f"Routine lane note {extra}, filed and done. "
                                         "A second clause so the row has some body to it."))
        ids = [str(row["id"]) for row in self.messages]
        for at in range(0, len(ids) - 10, 50):
            for step in (3, 6, 9):
                self.messages[at + step]["reply_to"] = ids[at]
        middle = (len(ids) // 2 // 50) * 50
        self.answered = ids[middle]
        self.answered_replies = [ids[middle + step] for step in (3, 6, 9)]
        self.plain = ids[middle + 20]


def frames(page: Page) -> None:
    page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(done, 0))))")


def tap_reply(page: Page, label: str, target: str, replies: list[str]) -> str:
    answered = bool(replies)
    page.evaluate("(ids) => { window.__replyIds = ids; }", replies)
    page.evaluate(f"() => document.querySelector(\"#discord-log > li[data-id='{target}']\").scrollIntoView({{block: 'center'}})")
    frames(page)
    top = float(page.evaluate(f"() => document.querySelector(\"#discord-log > li[data-id='{target}']\").getBoundingClientRect().top"))
    page.evaluate(f"() => document.querySelector(\"#discord-log > li[data-id='{target}'] .reply-button\").click()")
    page.wait_for_function("() => window.__replyTap.counted !== undefined && window.__replyTap.armed === false", timeout=WAIT_MS)
    tap = page.evaluate("() => ({counted: window.__replyTap.counted, shown: window.__replyTap.shownAtFirstFrame, first: window.__replyTap.firstFrame})")
    counted = tap["counted"]
    check(not counted["fetches"], f"{label}: requests went out before the box showed: {counted['fetches']}")
    check(tap["shown"] is True, f"{label}: the box was not open and on screen at the first frame after the tap")
    check(counted["redrawn"] <= MAX_TAP_ROWS_REDRAWN,
          f"{label}: the tap redrew {counted['redrawn']} rows before the box's first frame (the whole list redrawn)")
    check(counted["rects"] <= MAX_TAP_RECT_READS,
          f"{label}: the tap read {counted['rects']} element boxes before the box's first frame (over {MAX_TAP_RECT_READS})")
    # Then, past that frame: the replies gathered under the message, which has not moved, the box under them.
    if answered:
        page.wait_for_function(f"""() => {{ const rows = [...document.querySelectorAll('#discord-log > li[data-id]')];
            const at = rows.findIndex((li) => li.dataset.id === '{target}');
            return at >= 0 && rows.slice(at + 1, at + 4).every((li) => li.getAttribute('data-coalesce') !== null || li.querySelector('.reply-bridge')); }}""",
                               timeout=WAIT_MS)
    frames(page)
    # Where it all stands once settled: the message on screen, and the box over neither it nor the
    # replies gathered under it (the one move `#2395` allows to clear them, never a cover).
    now = page.evaluate(f"""() => {{ const rows = [...document.querySelectorAll('#discord-log > li[data-id]')];
        const at = rows.findIndex((li) => li.dataset.id === '{target}');
        const stack = rows.slice(at, at + ({4 if answered else 1})).filter((li) => li.getClientRects().length);
        const box = document.getElementById('column-reply').getBoundingClientRect();
        const row = rows[at].getBoundingClientRect();
        return {{top: row.top, bottom: Math.max(...stack.map((li) => li.getBoundingClientRect().bottom)), box: box.top,
          height: innerHeight}}; }}""")
    check(float(now["top"]) >= 0 and float(now["top"]) < float(now["height"]),
          f"{label}: the message left the screen: {now}")
    check(float(now["box"]) >= float(now["bottom"]) - 0.5,
          f"{label}: the box stands over the message or its gathered replies: {now}")
    moved = float(now["top"]) - top
    # Liaison 1112: the lift that clears the gathered replies comes within two frames of the box's first
    # frame, and no reply ever stands under the box on any frame from the first on.
    page.wait_for_function("() => window.__replyTap.frames.length >= 8", timeout=WAIT_MS)
    seen = page.evaluate("() => window.__replyTap.frames")
    first_top = seen[0]["top"]
    moved_at = next((index for index, frame in enumerate(seen) if frame["top"] is not None and first_top is not None
                     and abs(float(frame["top"]) - float(first_top)) > 0.5), None)
    check(moved_at is None or moved_at <= 2,
          f"{label}: the lift came {moved_at} frames after the box's first frame (over 2): {seen}")
    check(not any(frame["covered"] for frame in seen), f"{label}: a reply stood under the box on a frame: {seen}")
    page.keyboard.press("Escape")
    frames(page)
    return (f"{label}: box open at the first frame, no request, {counted['redrawn']} rows redrawn, "
            f"{counted['rects']} box reads in the tap; lift at frame {moved_at if moved_at is not None else '-'} of the box's; "
            f"settled clear of the box (message moved {moved:.1f}px); "
            f"first frame after {float(tap['first']):.1f} ms (report)")


def walk(pw: object, args: argparse.Namespace, work: Path, label: str, width: int, height: int, touch: bool) -> list[str]:
    api = Channel()
    server, _thread, origin = big.serve(api, fixture.WEB_ROOT)
    try:
        profile = work / f"profile-{label}"
        profile.mkdir()
        context = pw.chromium.launch_persistent_context(  # type: ignore[attr-defined]
            str(profile), headless=True, executable_path=args.browser_executable,
            viewport={"width": width, "height": height}, device_scale_factor=2.625 if touch else 1,
            is_mobile=touch, has_touch=touch)
        try:
            browser_waits.hot_host_timeouts(context)
            page = context.pages[0] if context.pages else context.new_page()
            page.add_init_script(TAP_JS)
            page.goto(f"{origin}/", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_function("() => !document.getElementById('screen-welcome').hidden"
                                   " || !document.getElementById('screen-main').hidden", timeout=WAIT_MS)
            if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
                page.evaluate("() => document.getElementById('welcome-start').click()")
            page.wait_for_selector("#search-toggle", state="visible", timeout=WAIT_MS)
            page.wait_for_function(f"() => document.querySelectorAll('#discord-log > li[data-id]').length >= {EXTRA_ROWS}",
                                   timeout=WAIT_MS)
            height_px = int(page.evaluate("() => document.getElementById('discord-log').scrollHeight"))
            check(height_px > 100_000, f"{label}: the list holds {height_px}px of rows, not the owner's 100k")
            return [tap_reply(page, f"{label} reply on a message with replies", api.answered, api.answered_replies),
                    tap_reply(page, f"{label} reply on a message with none", api.plain, [])]
        finally:
            context.close()
    finally:
        server.shutdown()


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT, help="directory holding the page to serve")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    from playwright.sync_api import sync_playwright
    report: list[str] = []
    # `#2382 summary-link-test-host-tmp`: never the host temp dir; see browser_scratch.
    with browser_scratch.run_scratch("reply-open-") as work:
        fixture.WEB_ROOT = page_sources.served(args.web_root)
        with sync_playwright() as pw:
            for label, width, height, touch in (("desk", 1280, 800, False), ("phone", 412, 915, True)):
                report += walk(pw, args, work, label, width, height, touch)
    for line in report:
        print(line)
    print("reply open instant browser walk: ok")
    print(json.dumps({"cases": len(report)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
