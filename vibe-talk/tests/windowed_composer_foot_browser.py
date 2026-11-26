#!/usr/bin/env python3
"""The composer's foot has one writer while the reader types, windowed lists on or off (`#2292`).

`#2292 vibe-talk-windowed-composer-foot-fight`: with windowed lists on, typing at the foot while
messages arrived moved the composer 46 px: the windowed renderer held a row (its anchor nudge, its
foot lock), a read's landing restored a row (`settleAfterRead`), and the scroll owner kept the
composer (`keepComposerInView`), three writers partly undoing each other. And a jump onto the
middle of a windowed list waited a frame's deferred re-slice with no row in the band.

This walk serves the checked-out page and the scroll-stability fake API (one channel taller than
any window, a live stream the walk writes events into) from one loopback origin, and checks:

  * phone (412x915, touch, keyboard up: the visual viewport shrunk as by a keyboard) and desk
    (1280x800, composer focused), each with windowed lists ON and OFF: the reader at the foot types
    while four messages arrive, each followed by its read's landing; every scroll write in that
    time is the scroll owner's composer keep (renderer, landing and foot-keeper writes counted:
    zero), and the composer moves at most 1.0 px, between two frames or in all. The bar is the
    whole-pixel one (ui-rules R11): Chromium lands every scroll position on a whole CSS pixel and
    clamps it to a whole-pixel foot, while the composer's own top is fractional, so a keep can
    leave it up to (never) one pixel off; anything a second writer does moves it far more (46 px
    with windowing on, 8.7 px per arrival at the desk, before `#2292`);
  * phone, windowed ON: programmatic jumps into the list (a quarter, half, three quarters down)
    have a mounted row in the band by the time the jump's scroll event has been answered, and in
    the frame it paints.

Counts and geometry only; waits are on page conditions, with the hang backstop as their deadline.
Pass --web-root DIR to run it against another copy of the page.
"""

from __future__ import annotations

import argparse
import os
import sys
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import browser_waits
import offline_cache_browser as fixture
import page_sources
import scroll_stability_browser as stability
from offline_cache_browser import check
from scroll_stability_browser import FILLER, READS_JS, Reader, StabilityApi, handler, open_main

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

# More rows than the windowed renderer's cap, all in the first read, so the windowed list stands
# as a window with spacers on both sides.
EXTRA_ROWS = 160
KEYBOARD_PX = 336
ARRIVALS = 4
BACKSTOP_S = browser_waits.HANG_BACKSTOP_MS / 1000
FLAG = "vibe-talk.voice.windowed-lists"
# Writers that hold a ROW: none may write while the composer holds the view.
ROW_WRITERS = ("renderWindowed", "settleAfterRead")
COMPOSER_WRITER = "keepComposerInView"

# Every write of the list's scroll position while armed, as the innermost named function that made
# it (the scroll owner's own write helpers skipped); and the composer's top once per frame.
ARM_JS = r"""() => {
  const area = document.getElementById('scroll-area');
  const composer = document.getElementById('channel-composer');
  const desc = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollTop');
  const skip = new Set(['writeTo', 'writeDelta', 'writeHold', 'nudge', 'toPosition', 'set', 'Object.nudge',
    'Object.toPosition', 'Object.keepComposer', 'keepComposer']);
  const probe = window.__footProbe = { writes: [], tops: [], on: true };
  Object.defineProperty(area, 'scrollTop', { configurable: true, get() { return desc.get.call(this); },
    set(value) {
      const before = desc.get.call(this);
      desc.set.call(this, value);
      if (!probe.on || Math.abs(desc.get.call(this) - before) <= 0.01) return;
      const names = (new Error().stack || '').split('\n').slice(1).map((line) => line.trim().split(' ')[1] || '');
      probe.writes.push(names.filter((name) => name && !skip.has(name)).slice(0, 6).join(' < '));
    } });
  const sample = () => { if (!probe.on) return; probe.tops.push(composer.getBoundingClientRect().top); requestAnimationFrame(sample); };
  requestAnimationFrame(sample);
}"""
DISARM_JS = r"""() => { const probe = window.__footProbe; probe.on = false;
  delete document.getElementById('scroll-area').scrollTop; return { writes: probe.writes, tops: probe.tops }; }"""

# A jump into the list, with whether a mounted row stands in the band when the jump's scroll event
# has been answered (a listener added after the page's own) and in the frame that paints it (a frame
# callback that listener asks for, so it runs after any the page asked for in the same event).
JUMP_JS = r"""(at) => new Promise((done) => {
  const area = document.getElementById('scroll-area');
  const covered = () => { const box = area.getBoundingClientRect(); const foot = box.top + area.clientHeight;
    return [...document.querySelectorAll('#discord-log > li[data-id]')]
      .some((row) => { const r = row.getBoundingClientRect(); return r.height > 0 && r.bottom > box.top + 1 && r.top < foot; }); };
  const onScroll = () => {
    area.removeEventListener('scroll', onScroll);
    const atEvent = covered();
    requestAnimationFrame(() => done({ atEvent, painted: covered(), spacers: document.querySelectorAll('[data-window-spacer]').length }));
  };
  area.addEventListener('scroll', onScroll, { passive: true });
  area.scrollTop = Math.round((area.scrollHeight - area.clientHeight) * at);
})"""


def writer_of(stack: str) -> str:
    """The writer a stack names: the composer keep, a row writer, or the stack itself."""
    names = stack.split(" < ")
    for name in names:
        if COMPOSER_WRITER in name:
            return COMPOSER_WRITER
        for row_writer in ROW_WRITERS:
            if row_writer in name:
                return row_writer
    return stack


def arrive(reader: Reader, index: int) -> None:
    """One message arrives on the stream while the reader types, and the read it starts lands."""
    page, api = reader.page, reader.api
    page.keyboard.type("go ", delay=10)
    asked = api.timeline_requests()
    arrived = api.add(f"An arrival while the reader types, number {index}. {FILLER}")
    api.push("message", {"message": arrived}, str(arrived["id"]))
    reader.wait_for(f"() => !!document.querySelector(\"#discord-log > li[data-id='{arrived['id']}']\")",
                    f"arrival {index} was never drawn", seconds=BACKSTOP_S)
    deadline = time.monotonic() + BACKSTOP_S
    while time.monotonic() < deadline and not (api.timeline_requests() > asked and
                                               page.evaluate("() => window.__timelineReads === 0")):
        page.wait_for_timeout(25)
    reader.check(api.timeline_requests() > asked, f"arrival {index} started no read")
    reader.check(bool(page.evaluate("() => window.__timelineReads === 0")), f"arrival {index}'s read never landed")
    reader.frames(3)
    reader.settled()


def typing_walk(reader: Reader, windowed: bool, keyboard: bool) -> str:
    """Typing at the foot through arrivals: one composer writer, the composer still."""
    page = reader.page
    page.evaluate("() => { const area = document.getElementById('scroll-area'); area.scrollTop = area.scrollHeight; }")
    reader.frames(2)
    reader.settled()
    spacers = page.evaluate("() => document.querySelectorAll('#discord-log > [data-window-spacer]').length")
    reader.check((spacers > 0) == windowed, f"windowed lists {'on' if windowed else 'off'} but {spacers} spacers stand")
    if reader.touch:
        page.tap("#channel-compose-text")
    else:
        page.click("#channel-compose-text")
    if keyboard:
        size = page.viewport_size or {"width": 412, "height": 915}
        page.set_viewport_size({"width": size["width"], "height": size["height"] - KEYBOARD_PX})
        browser_waits.wait_until(page, "() => document.documentElement.getAttribute('data-keyboard') === 'open'",
                                 "the keyboard never fitted the composer")
    page.keyboard.type("ok ", delay=10)
    reader.settled()
    reader.frames(3)
    page.evaluate(ARM_JS)
    for index in range(ARRIVALS):
        arrive(reader, index)
    reader.settled()
    reader.frames(3)
    probe = page.evaluate(DISARM_JS)
    writers: dict[str, int] = {}
    for stack in probe["writes"]:
        name = writer_of(stack)
        writers[name] = writers.get(name, 0) + 1
    tops = [float(top) for top in probe["tops"]]
    reader.check(len(tops) >= 8, f"only {len(tops)} frames sampled")
    step = max((abs(b - a) for a, b in zip(tops, tops[1:])), default=0.0)
    spread = max(tops) - min(tops)
    print(f"{reader.label} {'windowed' if windowed else 'full'}: writers {writers}, step {step:.2f} px, spread {spread:.2f} px, "
          f"{len(tops)} frames", file=sys.stderr, flush=True)
    reader.check(writers.get(COMPOSER_WRITER, 0) >= 1,
                 f"no write was the composer keep's (writers {writers}): the stack names this check reads are gone")
    rows = {name: writers.get(name, 0) for name in ROW_WRITERS}
    reader.check(not any(rows.values()), f"row writers moved the list while the composer held it: {rows} (all {writers})")
    reader.check(set(writers) <= {COMPOSER_WRITER}, f"a writer other than the composer keep moved the list: {writers}")
    reader.check(step < 1.0, f"the composer moved {step:.2f} px between two frames")
    reader.check(spread < 1.0, f"the composer moved {spread:.2f} px in all")
    focused = page.evaluate("() => document.activeElement && document.activeElement.id")
    reader.check(focused == "channel-compose-text", f"the composer lost the focus to {focused!r}")
    return (f"typing {'windowed' if windowed else 'full'}{' keyboard up' if keyboard else ''}: "
            f"{writers.get(COMPOSER_WRITER, 0)} composer keeps, 0 row writes, step {step:.2f} px, spread {spread:.2f} px "
            f"over {len(tops)} frames")


def jump_walk(reader: Reader) -> str:
    """Jumps onto the middle of a windowed list: a row in the band as each is answered and painted."""
    found = []
    for at in (0.5, 0.25, 0.75):
        result = reader.page.evaluate(JUMP_JS, at)
        reader.check(result["spacers"] > 0, "the list is not windowed")
        reader.check(bool(result["atEvent"]), f"a jump {at:.2f} down had no row in the band when its scroll was answered")
        reader.check(bool(result["painted"]), f"a jump {at:.2f} down painted a frame with no row in the band")
        reader.settled()
        found.append(f"{at:.2f}")
    return "jumps " + ", ".join(found) + " down: a row in the band as answered and painted"


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, touch: bool,
         windowed: bool) -> str:
    stability.FLAT_PAGE = 2000
    api = StabilityApi()
    for index in range(EXTRA_ROWS):
        api.add(f"Long channel row {index}. {FILLER * (1 + index % 3)}")
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-foot-") as profile:
            options: dict[str, object] = {"viewport": {"width": width, "height": height}, "color_scheme": "dark"}
            if touch:
                options.update(device_scale_factor=2.625, is_mobile=True, has_touch=True)
            context = chromium.launch_persistent_context(profile, headless=True, executable_path=args.browser_executable,
                                                         **options)  # type: ignore[arg-type]
            browser_waits.hot_host_timeouts(context)
            context.add_init_script(f"try {{ localStorage.setItem('{FLAG}', '{'1' if windowed else '0'}'); }} catch (e) {{}}")
            page: Page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            reader = Reader(page, api, None, label, touch)
            page.add_init_script(READS_JS)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            open_main(reader)
            notes = []
            if windowed and touch:
                notes.append(jump_walk(reader))
            notes.append(typing_walk(reader, windowed, keyboard=touch))
            reader.check(not errors, f"the page threw: {errors}")
            context.close()
            return f"{label} {'windowed' if windowed else 'full'}: " + "; ".join(notes)
    finally:
        api.stopping.set()
        api.timeline_gate.set()
        server.shutdown()
        server.server_close()
        thread.join()


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required: python3 -m pip install --user playwright && "
                         "python3 -m playwright install chromium") from error
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    report = []
    with sync_playwright() as playwright:
        for label, width, height, touch in (("phone-412", 412, 915, True), ("desk-1280", 1280, 800, False)):
            for windowed in (True, False):
                report.append(walk(playwright.chromium, args, label, width, height, touch, windowed))
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
