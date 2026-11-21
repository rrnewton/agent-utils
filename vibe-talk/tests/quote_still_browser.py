#!/usr/bin/env python3
"""Quote-replying a long thread message never moves it, in real Chromium.

`#2063 vibe-talk-quote-reply-view-stability`. The owner, 2026-11-21, in two-column
mode reading a thread and responding to a long message: "when i quote reply ...
some junk updates lower on the screen and the post I'm replying to DOES NOT STAY
STILL on the screen. This violates our view stability principle I believe".

This drives the real page (fixture API only, served on a kernel-picked loopback
port): a thread of 35 rows with one long message in the middle, opened in the
right column at 1280x800 (two columns) and in the one thread column at 412x915,
with windowed lists on and off. The long message's words are selected as a
reader selects them, its Reply pressed as Quote reply, and every animation
frame from the press until 60 frames after the box has settled -- and again
through Cancel -- records the quoted post's `getBoundingClientRect().top`.

The invariant, counted in frames and conditions, never wall time:

  * the quoted post's top changes by 0 px in every captured frame (at most
    half a pixel of rounding), with its top in the middle of the column and
    low on the screen, windowed on and off, at both widths;
  * low on the screen the reply box is still visible inside the column's band
    (docked to its foot, over the rows), never by scrolling the post away;
  * no page error at any step.

Before the fix the same walk measured -12 px (middle) and -229 px (low) at
1280x800, -27 px at 412x915, in the first painted frame, from the one scroll
write that showed the box by moving the list.

Pass --screenshots DIR to keep a PNG of the open reply per case.
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
from offline_cache_browser import TOKEN, FakeApi, check, handler_for, said
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

WEB_ROOT = page_sources.served_root()
THREAD = "spaces/A/threads/one"
LONG_ID = "217"
# Frames captured after the reply box is open and focused.
AFTER_FRAMES = 60

PROBE_JS = """([listId, targetId]) => {
  const probe = {frames: [], stop: false};
  window.__quoteStill = probe;
  const scrollerId = listId === 'side-log' ? 'side-scroll' : 'scroll-area';
  const step = () => {
    if (probe.stop) return;
    const list = document.getElementById(listId);
    const target = [...list.children].find((li) => li.getAttribute('data-id') === targetId);
    const scroller = document.getElementById(scrollerId);
    const mount = document.getElementById(listId === 'side-log' ? 'side-reply' : 'column-reply');
    const mr = mount.getBoundingClientRect();
    const sr = scroller.getBoundingClientRect();
    probe.frames.push({
      top: target ? target.getBoundingClientRect().top : null,
      scrollTop: scroller.scrollTop,
      mountTop: mr.top, mountBottom: mr.bottom, mountHeight: mr.height,
      bandTop: sr.top, bandBottom: sr.bottom,
      footDocked: mount.hasAttribute('data-foot-docked'),
    });
    if (probe.frames.length < 3000) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
}"""

SELECT_JS = """([listId, id, start, end, select]) => {
  const row = [...document.querySelectorAll(`#${listId} > li[data-id]`)].find((li) => li.dataset.id === id);
  const body = row.querySelector('.body');
  const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
  let text = null;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) {
    if (!text || node.data.length > text.data.length) text = node;
  }
  const range = document.createRange();
  range.setStart(text, start); range.setEnd(text, end);
  if (select) { getSelection().removeAllRanges(); getSelection().addRange(range); }
  const rects = [...range.getClientRects()];
  const first = rects[0];
  return {left: first.left, right: first.right, top: first.top, bottom: first.bottom};
}"""


class StillApi(FakeApi):
    """One thread of 35 rows, its long message in the middle, plus two Main rows."""

    def __init__(self) -> None:
        super().__init__()
        filler = ("The overnight run is still going; the integration shard was retried and passed, "
                  "and the artifact upload waited on the cache for most of an hour. ")
        long_para = ("Before we cut the release, can somebody check whether the migration on the "
                     "reporting replica has finished, whether the dashboards picked up the new "
                     "partitions, and whether the alert we silenced on Friday is back on? I would "
                     "rather not find out on Monday that one of them was still pending, because "
                     "last time that cost us a morning of reruns and an apology. ")
        record = {"id": THREAD, "root_message_id": "200", "is_root": True,
                  "reply_count": 34, "reply_count_exact": True}
        messages = []
        root = said(0, f"Thread starts here. {filler}", "human")
        root["thread"] = record
        messages.append(root)
        for index in range(1, 35):
            text = long_para * 10 if str(200 + index) == LONG_ID else (
                f"Reply {200 + index} in the thread. " + filler * (1 + index % 2))
            row = said(index, text, "human" if index % 3 else "coder")
            row["thread"] = {**record, "is_root": False}
            messages.append(row)
        for index in (35, 36):
            messages.append(said(index, f"Main message {200 + index}. " + filler, "coder"))
        self.messages = messages
        self.threads = [{"id": THREAD, "root": messages[0], "title": "Thread",
                         "reply_count": 34, "reply_count_exact": True,
                         "updated_at": messages[20]["timestamp"]}]


def num(frame: dict[str, object], key: str) -> float:
    """One measured number from a captured frame."""
    value = frame[key]
    assert isinstance(value, (int, float))
    return float(value)


def wait_for(page: Page, condition: str, why: str, seconds: float = 15) -> None:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline and not page.evaluate(condition):
        page.wait_for_timeout(25)
    check(bool(page.evaluate(condition)), why)


def settled(page: Page) -> None:
    page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
         phone: bool, windowed: bool, place: float) -> str:
    api = StillApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-quote-still-") as profile:
            kwargs = dict(headless=True, executable_path=args.browser_executable,
                          viewport={"width": width, "height": height}, color_scheme="dark")
            if phone:
                kwargs.update(user_agent=("Mozilla/5.0 (Linux; Android 15; Pixel 7) "
                                          "AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36"),
                              device_scale_factor=2.0, is_mobile=True, has_touch=True)
            context = chromium.launch_persistent_context(profile, **kwargs)
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            flag = "vibe-talk.voice.windowed-lists" if windowed else "vibe-talk.voice.windowed-lists-off"
            page.add_init_script(
                "try { localStorage.setItem('vibe-talk.welcome.dismissed', '1'); "
                f"localStorage.setItem('{flag}', '1'); }} catch (e) {{}}")
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            wait_for(page, "() => document.querySelectorAll('#discord-log > li[data-id]').length >= 3",
                     f"{label}: the channel did not open")
            settled(page)
            if phone:
                page.click("#discord-log > li[data-id='200'] .thread-badge")
                list_id, scroller_id = "discord-log", "scroll-area"
                wait_for(page, "() => document.querySelectorAll('#discord-log > li[data-id]').length > 10",
                         f"{label}: the thread did not open")
            else:
                wait_for(page, "() => document.getElementById('column-toggle').getClientRects().length > 0",
                         f"{label}: the layout button is not drawn")
                page.click("#column-toggle")
                wait_for(page, "() => document.querySelectorAll('#side-cards > li').length === 1",
                         f"{label}: the threads are not listed")
                page.click(f"#side-cards > li[data-id='{THREAD}'] .side-card-open")
                list_id, scroller_id = "side-log", "side-scroll"
                wait_for(page, "() => document.querySelectorAll('#side-log > li[data-id]').length > 10",
                         f"{label}: the thread did not open on the right")
            settled(page)
            # The quoted post at `place` of its column (scroll roughly first, so a
            # windowed slice mounts it, then by its real rect).
            for _attempt in range(4):
                found = page.evaluate("""([listId, scrollerId, targetId, frac]) => {
                  const scroller = document.getElementById(scrollerId);
                  const row = [...document.getElementById(listId).children]
                    .find((li) => li.getAttribute('data-id') === targetId);
                  if (!row || row.hasAttribute('data-window-spacer')) {
                    scroller.scrollTop = scroller.scrollHeight * 0.45;
                    return false;
                  }
                  const frame = scroller.getBoundingClientRect();
                  const box = row.getBoundingClientRect();
                  scroller.scrollTop += box.top - (frame.top + scroller.clientHeight * frac);
                  return true;
                }""", [list_id, scroller_id, LONG_ID, place])
                settled(page)
                if found:
                    break
            check(bool(page.evaluate(
                f"() => {{ const row = [...document.getElementById('{list_id}').children]"
                f".find((li) => li.getAttribute('data-id') === '{LONG_ID}');"
                " return Boolean(row) && !row.hasAttribute('data-window-spacer'); }")),
                f"{label}: the long message is not mounted")
            page.evaluate(PROBE_JS, [list_id, LONG_ID])
            page.wait_for_timeout(120)
            if phone:
                page.evaluate(SELECT_JS, [list_id, LONG_ID, 12, 60, True])
            else:
                line = page.evaluate(SELECT_JS, [list_id, LONG_ID, 12, 60, False])
                y = (float(line["top"]) + float(line["bottom"])) / 2
                page.mouse.move(float(line["left"]) + 2, y)
                page.mouse.down()
                page.mouse.move(float(line["right"]) - 2, y, steps=8)
                page.mouse.up()
            wait_for(page, f"() => {{ const b = document.querySelector("
                           f"\"#{list_id} > li[data-id='{LONG_ID}'] .reply-button\");"
                           " return b && b.textContent === 'Quote reply'; }",
                     f"{label}: the selection did not make Reply Quote reply")
            settled(page)
            button = page.locator(f"#{list_id} > li[data-id='{LONG_ID}'] .reply-button")
            box = button.bounding_box()
            check(box is not None, f"{label}: Quote reply has no box to press")
            assert box is not None
            click_at = int(page.evaluate("() => window.__quoteStill.frames.length"))
            page.mouse.click(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
            wait_for(page, "() => { const b = document.getElementById('reply-text');"
                           " return b && b.value.startsWith('>') && document.activeElement === b; }",
                     f"{label}: Quote reply did not open the box with the quote")
            opened_at = int(page.evaluate("() => window.__quoteStill.frames.length"))
            wait_for(page, f"() => window.__quoteStill.frames.length >= {opened_at} + {AFTER_FRAMES}",
                     f"{label}: the frames after opening did not arrive")
            if args.screenshots:
                page.screenshot(path=str(args.screenshots / f"{label}-open.png"))
            # Cancel inside the same capture: putting the box away moves nothing either.
            # Pressed by the mouse where it is drawn, as a reader presses it (a
            # programmatic click that first scrolls it into view would move the
            # list itself and measure the harness, not the page).
            cancel = page.locator("#reply-cancel")
            cancel_box = cancel.bounding_box()
            check(cancel_box is not None, f"{label}: Cancel has no box to press")
            assert cancel_box is not None
            page.mouse.click(cancel_box["x"] + cancel_box["width"] / 2,
                             cancel_box["y"] + cancel_box["height"] / 2)
            wait_for(page, "() => document.getElementById('reply-text').value === ''"
                           " || document.getElementById('column-reply').hidden"
                           " && document.getElementById('side-reply').hidden",
                     f"{label}: Cancel did not put the box away")
            closed_at = int(page.evaluate("() => window.__quoteStill.frames.length"))
            wait_for(page, f"() => window.__quoteStill.frames.length >= {closed_at} + 30",
                     f"{label}: the frames after Cancel did not arrive")
            page.evaluate("() => { window.__quoteStill.stop = true; }")
            frames: list[dict[str, object]] = page.evaluate("() => window.__quoteStill.frames")
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()

            check(click_at >= 2 and len(frames) > click_at + AFTER_FRAMES,
                  f"{label}: too few frames captured ({len(frames)} from {click_at})")
            baseline = float(frames[click_at - 1]["top"])  # type: ignore[arg-type]
            worst = 0.0
            for frame in frames:
                check(frame["top"] is not None, f"{label}: the quoted post left the list")
                worst = max(worst, abs(float(frame["top"]) - baseline))  # type: ignore[arg-type]
            check(worst <= 0.5,
                  f"{label}: the quoted post moved {worst:.1f}px from {baseline:.1f} across "
                  f"{len(frames)} frames (windowed={windowed}, place={place})")
            open_frame = frames[opened_at] if opened_at < len(frames) else frames[-1]
            low = place >= 0.7
            if low:
                check(num(open_frame, "mountHeight") > 80
                      and num(open_frame, "mountBottom") <= num(open_frame, "bandBottom") + 2
                      and num(open_frame, "mountTop") >= num(open_frame, "bandTop") - 2,
                      f"{label}: low on screen the box is not visible in the column: {open_frame}")
            return (f"{label}: quoted post still at {baseline:.1f}px over {len(frames)} frames "
                    f"(worst {worst:.1f}px, windowed={'on' if windowed else 'off'}"
                    f"{', box docked at the foot' if open_frame['footDocked'] else ''})")
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per case into")
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
        for label, width, height, phone in (("desk-1280", 1280, 800, False), ("phone-412", 412, 915, True)):
            for windowed in (True, False):
                for place, where in ((0.35, "mid"), (0.7, "low")):
                    report.append(walk(playwright.chromium, args,
                                       f"{label}-{where}-{'win' if windowed else 'full'}",
                                       width, height, phone, windowed, place))
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
