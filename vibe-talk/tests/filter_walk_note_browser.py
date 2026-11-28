#!/usr/bin/env python3
"""The filter walk-back's landing line is announced, never floated over the rows, in real Chromium.

`#2453 filter-walk-landing-note-floats` (ui-rules R5; the owner's `#2314` decision: floating controls
may cover text, floating notes may not; same class as `#2350`, `#2365`, `#2375`, `#2426` R3). When a
filter change keeps a message older than everything loaded, the filter place walks back through older
pages and, when it lands, says "Here is the selected message." (the reader had chosen it) or "Here is
where you were." (they had not). Both floated as the status pill, over the row the walk had just put
the reader on and the rows around it. This drives the real page (fixture API only, served on a
kernel-picked loopback port; the channel of tests/filter_place_browser.py, read twenty at a time,
with a pin two pages older than its newest page), at a desk (1280x800) and a phone (412x915 by touch),
with Discuss open on two. No walk with a phone's keyboard open: the walk back starts from a tap in the
Labels drop-down, which takes the focus out of every text field, so the keyboard is down when the line
is said (a simulated keyboard here draws the same frame as a closed one):

  the search, then Pinned; the old pin chosen (a click or a tap), or parked at the head of the Pinned
  list with nothing chosen -> the filter cleared (Escape at the desk, the Labels drop-down on the
  phone) -> the walk reads the older pages and lands, and says its line: on every frame from the clear
  until the list is quiet, the line, while up, floats over no row's text, and it was said (in the
  polite live region).

The line's own timer would take it away after a few seconds, so how long the host takes would decide
what the walk sees; the walk lets that timer go the moment the line is said (`shellNotices.holdStatus`,
the page's hook for a picture of the line), so only the page's own acts end it.

Pass --screenshots DIR for a PNG once the walk has landed, --all-schemes for every walk in dark too,
--only NAME to run the walks whose names contain NAME, and --report-only to measure a page that fails
(main before the change) without failing.
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

import filter_place_browser as place
import offline_cache_browser as fixture
import search_clear_note_browser as note_walk
import search_clear_stays_browser as clear_walk
from offline_cache_browser import CHANNEL, TOKEN, Json, check, handler_for
import page_sources
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import Page

# The two lines the walk back says when it lands (`walkToFilterAnchor`).
SENTENCES = ("Here is the selected message.", "Here is where you were.")

# name, profile, keyboard open, Discuss open, whether the old pin is chosen (else parked at the head).
# Keyboard open is never asked for (see the module's note); the column stays for a flow that keeps a
# text field focused.
WALKS = (
    ("desk-selected", "desk", False, False, True),
    ("desk-where", "desk", False, False, False),
    ("desk-selected-discuss", "desk", False, True, True),
    ("phone-selected", "phone", False, False, True),
    ("phone-where", "phone", False, False, False),
    ("phone-selected-discuss", "phone", False, True, True),
)

PROFILES = {"desk": (1280, 800, False), "phone": (412, 915, True)}


class WalkApi(place.PlaceApi):
    """`PlaceApi`, with Discuss offered so the matrix can open it."""

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope), "discuss_available": True}


# Scroll the Pinned list so the old pin is the first row standing below the floating search bar, as
# the row a reader at the head of the list reads.
HEAD_PARK_JS = """(id) => {
  const area = document.getElementById('scroll-area');
  let head = area.getBoundingClientRect().top;
  for (const name of ['channel-freshness', 'search-float']) {
    const node = document.getElementById(name);
    if (node && !node.hidden && node.getClientRects().length) head = Math.max(head, node.getBoundingClientRect().bottom);
  }
  const row = [...document.querySelectorAll('#pinned-log > li[data-ids]')]
    .find((li) => li.getAttribute('data-ids').split(' ').includes(id));
  area.scrollTop += row.getBoundingClientRect().top - (head + 12);
}"""

# Every frame: whether a landing line is up, whether the status line floats (drawn larger than the
# 1px the announced-only line is clipped to, and visible), and the rows whose text it stands over. A
# line said has its own timer let go at once (`holdStatus`), so only the page ends it.
WATCH_JS = """(sentences) => {
  if (typeof shellNotices === 'undefined' || typeof shellNotices.holdStatus !== 'function') {
    throw new Error('the page offers no shellNotices.holdStatus to keep the line up for the walk');
  }
  const line = document.getElementById('status-line');
  const status = document.getElementById('status');
  const up = () => !line.hidden && sentences.includes(status.textContent || '');
  new MutationObserver(() => { if (up()) shellNotices.holdStatus(); })
    .observe(line, {attributes: true, childList: true, characterData: true, subtree: true});
  const textUnder = (box) => {
    const over = [];
    for (const li of document.querySelectorAll('#screen-main li[data-ids]')) {
      if (li.getClientRects().length === 0) continue;
      const walker = document.createTreeWalker(li, NodeFilter.SHOW_TEXT);
      for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        if (!(node.textContent || '').trim()) continue;
        const range = document.createRange();
        range.selectNodeContents(node);
        if ([...range.getClientRects()].some((r) => r.width >= 1 && r.height >= 1 && r.left < box.right &&
            r.right > box.left && r.top < box.bottom && r.bottom > box.top)) { over.push(li.getAttribute('data-ids')); break; }
      }
    }
    return over;
  };
  window.__noteFrames = [];
  const step = () => {
    const shown = !line.hidden && line.getClientRects().length > 0;
    const box = shown ? line.getBoundingClientRect() : null;
    const floats = Boolean(box && box.width > 1 && box.height > 1 && getComputedStyle(line).visibility !== 'hidden');
    window.__noteFrames.push({said: up(), text: status.textContent || '', floats,
      box: floats ? [box.left, box.top, box.width, box.height].map(Math.round) : null, over: floats && up() ? textUnder(box) : []});
    if (!window.__noteStop) requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
  return true;
}"""


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT, help="directory holding the page to serve")
    parser.add_argument("--screenshots", type=Path, help="directory to write PNGs into")
    parser.add_argument("--only", default="", help="run only the walks whose names contain this")
    parser.add_argument("--all-schemes", action="store_true", help="run every walk in dark as well as light")
    parser.add_argument("--report-only", action="store_true",
                        help="print every walk's findings and exit 0 (for measuring main)")
    return parser.parse_args()


def until(page: Page, script: str, why: str, arg: object = None, seconds: float = 30) -> None:
    """Wait for the page's own state (never a clock for what it shows): `script` true, or fail with `why`."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline and not page.evaluate(script, arg):
        page.wait_for_timeout(25)
    check(bool(page.evaluate(script, arg)), why)


def press(page: Page, selector: str, touch: bool) -> None:
    box = page.locator(selector).bounding_box()
    check(box is not None, f"{selector} is not on the screen")
    assert box is not None
    x, y = box["x"] + box["width"] / 2, box["y"] + box["height"] / 2
    if touch:
        page.touchscreen.tap(x, y)
    else:
        page.mouse.click(x, y)


def pick_label(page: Page, selector: str, touch: bool) -> None:
    press(page, "#labels-button", touch)
    until(page, "() => !document.getElementById('labels-panel').hidden", "the Labels panel did not open")
    page.locator(selector).click()
    clear_walk.quiet(page, 10)


def sign_in(page: Page, keyboard: bool) -> None:
    if keyboard:
        page.evaluate(f"() => window.__iosKeyboard(window.innerHeight - {clear_walk.KEYBOARD_PX})")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function("() => !document.getElementById('screen-welcome').hidden || "
                               "!document.getElementById('screen-main').hidden", timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass  # no first-run Welcome on this path (#1781)
    page.wait_for_selector("#search-toggle", state="visible")
    until(page, "() => [...document.querySelectorAll('#discord-log > li[data-ids]')]"
          f".reduce((n, li) => n + li.getAttribute('data-ids').split(' ').length, 0) === {place.PAGE_SIZE}",
          "the channel did not open on its newest page")
    clear_walk.quiet(page)


def walk(page: Page, api: WalkApi, name: str, touch: bool, chosen: bool, shots: Path | None) -> list[str]:
    """One walk; answer what it found wrong (nothing on a page that keeps the rule)."""
    press(page, "#search-toggle", touch)
    until(page, "() => !document.getElementById('labels-button').hidden", f"{name}: the open search bar has no Labels filter")
    pick_label(page, "#labels-option-pinned", touch)
    until(page, f"() => document.querySelectorAll('#pinned-log > li[data-ids]').length === {len(place.PINNED)}",
          f"{name}: the Pinned list does not show the pins")
    clear_walk.quiet(page)
    if chosen:
        page.evaluate(place.PARK_JS, ["pinned-log", place.OLD_PIN, 0.45])
        clear_walk.quiet(page)
        x, y = page.evaluate(place.PRESS_AT_JS, ["pinned-log", place.OLD_PIN])
        if touch:
            page.touchscreen.tap(float(x), float(y))
        else:
            page.mouse.move(float(x), float(y))
            page.mouse.click(float(x), float(y))
    else:
        # Nothing chosen: a desk's pointer rests off the list (it would light the row under it).
        if not touch:
            page.mouse.move(2, 2)
        page.evaluate(HEAD_PARK_JS, place.OLD_PIN)
    clear_walk.quiet(page)
    page.evaluate(WATCH_JS, list(SENTENCES))
    if touch:
        pick_label(page, "#labels-option-all", touch)
    else:
        page.keyboard.press("Escape")
    until(page, "() => document.getElementById('pinned-log').hidden", f"{name}: clearing the filter left the Pinned list up")
    until(page, "(id) => { const row = [...document.querySelectorAll('#discord-log > li[data-ids]')]"
          ".find((li) => li.getAttribute('data-ids').split(' ').includes(id));"
          " return Boolean(row && row.getClientRects().length) &&"
          " document.getElementById('load-older').textContent.trim() !== 'Loading earlier messages…'; }",
          f"{name}: the walk back never landed on the old pin", arg=place.OLD_PIN)
    clear_walk.quiet(page)
    if shots is not None:
        page.screenshot(path=str(shots / f"{name}-landed.png"))
    page.evaluate("() => { window.__noteStop = true; }")
    raw = page.evaluate("() => window.__noteFrames")
    frames: list[Json] = [frame for frame in raw if isinstance(frame, dict)] if isinstance(raw, list) else []
    expected = SENTENCES[0] if chosen else SENTENCES[1]
    said = [frame for frame in frames if frame.get("said")]
    check(any(frame.get("text") == expected for frame in said),
          f"{name}: the walk landed but never said {expected!r} (last line: {frames[-1].get('text') if frames else None!r})")
    found: list[str] = []
    over = [frame for frame in said if frame.get("over")]
    if over:
        worst = over[0]
        found.append(f"{name}: {worst.get('text')!r} floated over the text of rows {worst.get('over')} "
                     f"({len(over)} of {len(said)} frames it was up; box {worst.get('box')})")
    return found


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required") from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    # The fake API's handler serves the page from this module attribute.
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    report: list[str] = []
    failures: list[str] = []
    with sync_playwright() as playwright:
        for scheme in ("light", "dark"):
            for walk_name, label, keyboard, discuss, chosen in WALKS:
                if scheme == "dark" and not args.all_schemes and walk_name not in ("desk-selected", "phone-selected"):
                    continue
                name = f"{scheme}-{walk_name}"
                if args.only and args.only not in name:
                    continue
                width, height, touch = PROFILES[label]
                api = WalkApi()
                server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
                server.daemon_threads = True
                thread = threading.Thread(target=server.serve_forever, daemon=True)
                thread.start()
                try:
                    with tempfile.TemporaryDirectory(prefix="vibe-talk-filter-walk-note-") as profile:
                        context = playwright.chromium.launch_persistent_context(
                            profile, headless=True, executable_path=args.browser_executable,
                            viewport={"width": width, "height": height}, color_scheme=scheme,
                            device_scale_factor=2.625 if touch else 1, is_mobile=touch, has_touch=touch)
                        browser_waits.hot_host_timeouts(context)
                        context.add_init_script(clear_walk.IOS_VIEWPORT_JS)
                        page = context.pages[0]
                        errors: list[str] = []
                        page.on("pageerror", lambda error: errors.append(str(error)))
                        page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
                        sign_in(page, keyboard)
                        if discuss:
                            note_walk.quiet_discuss(page, CHANNEL)
                            clear_walk.open_discuss(page)
                        found = walk(page, api, name, touch, chosen, args.screenshots)
                        check(not errors, f"{name}: the page threw: {errors}")
                        context.close()
                finally:
                    api.timeline_gate.set()
                    api.stopping.set()
                    server.shutdown()
                    server.server_close()
                    thread.join()
                report.append(f"{name} ({width}x{height}): " + ("; ".join(found) if found else
                              "ok: the landing line was said and floated over no row's text"))
                failures.extend(found)
    for line in report:
        print(line)
    if failures and not args.report_only:
        raise SystemExit("FAIL: the walk back's landing line floated over a row's text:\n  " + "\n  ".join(failures))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
