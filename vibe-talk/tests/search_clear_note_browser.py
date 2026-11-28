#!/usr/bin/env python3
"""The place-keeper's substitute sentence floats over no row, and goes when a clear shows the selection, in real Chromium.

`#2442` R5 (ui-rules R5: status is never stale; liaison 1156: "announce, don't float; and it
shouldn't survive the clear at all"). A search that hides the selected message puts the nearest
message shown in its place, and the status line says so: "Filtered out — the nearest is here."
That sentence is true only while the selection is hidden. It used to float as the status pill,
and the clear that brought every row back left it standing over a row's text, false. This drives
the real page (fixture API only, served on a kernel-picked loopback port), at a desk (1280x800),
a desk with two columns (1920x1080) and a phone (412x915 by touch, keyboard closed and open), with
Discuss open on three of them (one with the phone's keyboard open):

  select a message (a desk's pointer resting on it lights it; a phone's tap chooses it), type a
  search that hides it -> the sentence is said, and on no frame while the selection is hidden does
  a floating note stand over any row's text; then clear the search (Escape, the X, the magnifier,
  the field emptied) -> on no frame after the clear brings the selection back does the sentence
  still stand, and no floating note stands over a row's text.

The sentence's own timer would take it away after a few seconds, so how long the host takes between
the search and the clear would decide what the walk sees. It does not: the walk lets that timer go
the moment the sentence is said (`shellNotices.holdStatus`, the page's hook for a picture of the
line), so only the page's own acts end it, and the walk is the same on a quiet host and a hot one.

Pass --screenshots DIR for a PNG while the selection is hidden and after the clear, --all-schemes
for every walk in dark too, --only NAME to run the walks whose names contain NAME, and
--report-only to measure a page that fails (main before the change) without failing.
"""

from __future__ import annotations

import argparse
import json
import os
import tempfile
from pathlib import Path
from typing import TYPE_CHECKING

import bookmark_browser as fixture
import search_clear_stays_browser as clear_walk
from bookmark_browser import CHANNEL, Json, check, serve
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import Page, Route

# Every sentence the place-keeper says for a selection a filter hides (`filterHiddenSentence`);
# this walk's search can only say the first.
SENTENCES = (
    "Filtered out — the nearest is here.",
    "Not pinned — the nearest pin is here.",
    "Hide read hides it — nearest is here.",
    "Not in this view — nearest is here.",
)

# Watches the status line from before the search to after the clear. Each frame: whether the selected
# message is hidden, whether a substitute sentence is up, whether the line floats (drawn larger than
# the 1px the page's announced-only line is clipped to, and visible), and the rows whose text it stands
# over. A sentence said has its own timer let go at once (`holdStatus`), so only the page ends it.
WATCH_JS = """([selected, sentences]) => {
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
    for (const li of document.querySelectorAll('#screen-main li[data-id]')) {
      if (li.getClientRects().length === 0) continue;
      const walker = document.createTreeWalker(li, NodeFilter.SHOW_TEXT);
      for (let node = walker.nextNode(); node; node = walker.nextNode()) {
        if (!(node.textContent || '').trim()) continue;
        const range = document.createRange();
        range.selectNodeContents(node);
        if ([...range.getClientRects()].some((r) => r.width >= 1 && r.height >= 1 && r.left < box.right &&
            r.right > box.left && r.top < box.bottom && r.bottom > box.top)) { over.push(li.dataset.id); break; }
      }
    }
    return over;
  };
  const sample = () => {
    const row = document.querySelector(`#discord-log > li[data-id="${selected}"]`);
    const hidden = !row || row.getClientRects().length === 0;
    const shown = !line.hidden && line.getClientRects().length > 0;
    const box = shown ? line.getBoundingClientRect() : null;
    const floats = Boolean(box && box.width > 1 && box.height > 1 && getComputedStyle(line).visibility !== 'hidden');
    return {hidden, said: up(), text: status.textContent || '', lineHidden: line.hidden, idle: line.hasAttribute('data-idle'),
            floats, box: floats ? [box.left, box.top, box.width, box.height].map(Math.round) : null,
            over: floats ? textUnder(box) : [], phase: window.__notePhase};
  };
  window.__notePhase = 'search';
  window.__noteFrames = [];
  window.__noteSample = sample;
  const step = () => { window.__noteFrames.push(sample()); if (!window.__noteStop) requestAnimationFrame(step); };
  requestAnimationFrame(step);
  return true;
}"""

PROFILES = {"desk": (1280, 800, False), "desk2": (1920, 1080, False), "phone": (412, 915, True)}

# name, profile, keyboard open, Discuss open, how the search is cleared. The X, the magnifier and the
# field emptied everywhere they are offered; Escape at a desk.
WALKS = (
    ("desk-escape", "desk", False, False, "escape"),
    ("desk-x", "desk", False, False, "x"),
    ("desk-glass", "desk", False, False, "glass"),
    ("desk-emptied", "desk", False, False, "emptied"),
    ("desk-escape-discuss", "desk", False, True, "escape"),
    ("desk2-escape", "desk2", False, False, "escape"),
    ("desk2-glass", "desk2", False, False, "glass"),
    ("desk2-emptied", "desk2", False, False, "emptied"),
    ("phone-x", "phone", False, False, "x"),
    ("phone-glass", "phone", False, False, "glass"),
    ("phone-emptied", "phone", False, False, "emptied"),
    ("phone-keyboard-x", "phone", True, False, "x"),
    ("phone-x-discuss", "phone", False, True, "x"),
    ("phone-keyboard-x-discuss", "phone", True, True, "x"),
)

# The message the reader selects: a desk reader's pointer rests on the row parked (it lights it); a
# phone reader taps the first one below it whose text stands clear on the screen (a silent choice,
# `#239`): under the floating line, above the dock bar and an open Discuss sheet. Neither holds the
# word searched for (rows 61 to 94 hold none), so the search hides it.
PARKED = 60

# The first row after the parked one whose text's first line stands in the band the reader sees, and
# where to tap it.
TAP_JS = """(parked) => {
  const area = document.getElementById('scroll-area').getBoundingClientRect();
  let head = area.top, foot = area.bottom;
  for (const id of ['channel-freshness', 'search-float']) {
    const node = document.getElementById(id);
    if (node && !node.hidden && node.getClientRects().length) head = Math.max(head, node.getBoundingClientRect().bottom);
  }
  for (const id of ['dock', 'discuss-panel', 'scroll-tools']) {
    const node = document.getElementById(id);
    if (node && !node.hidden && node.getClientRects().length) {
      const top = node.getBoundingClientRect().top;
      if (top > head) foot = Math.min(foot, top);
    }
  }
  const visual = window.visualViewport;
  if (visual && visual.height > 0) foot = Math.min(foot, visual.offsetTop + visual.height);
  for (const li of document.querySelectorAll('#discord-log > li[data-id]')) {
    const index = Number(li.dataset.id.slice(-3));
    if (index <= parked || index > 94) continue;
    const body = li.querySelector('.body');
    if (!body) continue;
    const r = body.getBoundingClientRect();
    if (r.top >= head + 8 && r.top + 24 <= foot - 8) return {id: li.dataset.id, x: r.left + 20, y: r.top + Math.min(10, r.height / 2)};
  }
  return null;
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


def select(page: Page, touch: bool) -> str:
    """Select the message the walk hides; answer its id."""
    clear_walk.park(page, PARKED, touch)
    if not touch:
        return clear_walk.message_id(PARKED)
    spot = page.evaluate(TAP_JS, PARKED)
    check(isinstance(spot, dict), "no row after the parked one stands clear on the screen to tap")
    assert isinstance(spot, dict)
    page.touchscreen.tap(float(spot["x"]), float(spot["y"]))
    clear_walk.quiet(page)
    return str(spot["id"])


def quiet_discuss(page: Page, channel: Json) -> None:
    """Answer the Discuss panel's two reads for `channel` (its preview of what is unread, and its saved
    record) with nothing unread and nothing saved, as a reader who has read the channel sees it, so the
    matrix's Discuss cells show the panel at rest rather than the fixture's refusal of a route it does
    not serve (liaison 1166)."""
    channel_id = str(channel["id"])

    def preview(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "channel": {"id": channel_id, "label": str(channel["label"])}, "thread_id": None, "view": "flat",
            "window": {"count": 0, "unread_count": 0, "read_count": 0, "from_id": "", "through_id": "",
                       "truncated": False, "noise_hidden": 0},
            "unread": []}))

    def record(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "exists": False, "thread_id": None, "current_summary": None, "timeline": [], "staleness": None}))

    page.route(f"**/api/v1/channels/{channel_id}/catchup/preview*", preview)
    page.route(f"**/api/v1/channels/{channel_id}/catchup/record*", record)


def frames(page: Page) -> list[Json]:
    raw = page.evaluate("() => window.__noteFrames")
    return [frame for frame in raw if isinstance(frame, dict)] if isinstance(raw, list) else []


def walk(page: Page, name: str, touch: bool, how: str, shots: Path | None) -> list[str]:
    """One walk; answer what it found wrong (nothing on a page that keeps the rule)."""
    selected = select(page, touch)
    page.evaluate(WATCH_JS, [selected, list(SENTENCES)])
    clear_walk.search(page, "needle", touch)
    now = page.evaluate("() => window.__noteSample()")
    check(isinstance(now, dict), f"{name}: no sample")
    assert isinstance(now, dict)
    if shots is not None:
        page.screenshot(path=str(shots / f"{name}-hidden.png"))
    page.evaluate("() => { window.__notePhase = 'clear'; }")
    clear_walk.clear(page, how, touch)
    clear_walk.quiet(page)
    if shots is not None:
        page.screenshot(path=str(shots / f"{name}-after.png"))
    page.evaluate("() => { window.__notePhase = 'after'; }")
    clear_walk.quiet(page, 10)
    page.evaluate("() => { window.__noteStop = true; }")
    seen = frames(page)
    found: list[str] = []
    hidden = [frame for frame in seen if frame.get("hidden")]
    said = [frame for frame in hidden if frame.get("said")]
    # The case must arise: the search hid the selection, and the sentence said so.
    check(bool(hidden), f"{name}: the search never hid the selected message {selected}")
    check(bool(said), f"{name}: the sentence was never said while the selection was hidden (last: {now.get('text')!r})")
    floated = [frame for frame in hidden if frame.get("over")]
    if floated:
        worst = floated[0]
        found.append(f"{name}: while the selection was hidden, the line floated over rows {worst.get('over')} "
                     f"({len(floated)} frames, box {worst.get('box')}, {worst.get('text')!r})")
    back = [frame for frame in seen if frame.get("phase") != "search" and not frame.get("hidden")]
    check(bool(back), f"{name}: the clear never showed the selected message {selected} again")
    stale = [frame for frame in back if frame.get("said")]
    if stale:
        worst = next((frame for frame in stale if frame.get("over")), stale[-1])
        found.append(f"{name}: after the clear showed the selection, {worst.get('text')!r} still stood "
                     f"({len(stale)} of {len(back)} frames; floating {worst.get('floats')}, over rows {worst.get('over')})")
    over = [frame for frame in back if frame.get("over")]
    if over and not stale:
        found.append(f"{name}: after the clear, the line {over[-1].get('text')!r} floated over rows {over[-1].get('over')}")
    return found


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required") from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    web_root = page_sources.served(args.web_root)
    report: list[str] = []
    failures: list[str] = []
    with sync_playwright() as playwright:
        for scheme in ("light", "dark"):
            for walk_name, label, keyboard, discuss, how in WALKS:
                if scheme == "dark" and not args.all_schemes and walk_name not in ("desk-escape", "phone-x"):
                    continue
                name = f"{scheme}-{walk_name}"
                if args.only and args.only not in name:
                    continue
                width, height, touch = PROFILES[label]
                two = label == "desk2"
                api = clear_walk.ClearThreadsApi() if two else clear_walk.ClearApi()
                server, thread, url = serve(api, web_root)
                try:
                    with tempfile.TemporaryDirectory(prefix="vibe-talk-search-note-") as profile:
                        context = clear_walk.launch(playwright, args, profile, width, height, touch, scheme)
                        context.clock.set_fixed_time(clear_walk.NOW)
                        page = context.pages[0]
                        errors: list[str] = []
                        page.on("pageerror", lambda error: errors.append(str(error)))
                        page.goto(url, wait_until="load")
                        clear_walk.sign_in(page, keyboard)
                        if two:
                            page.click("#column-toggle")
                            page.wait_for_function("() => document.getElementById('screen-main')"
                                                   ".getAttribute('data-columns') === 'two'")
                            page.wait_for_function(f"() => document.querySelectorAll('#discord-log > li[data-id]')"
                                                   f".length === {clear_walk.COUNT - 1}")
                            clear_walk.quiet(page)
                        if discuss:
                            quiet_discuss(page, CHANNEL)
                            clear_walk.open_discuss(page)
                        found = walk(page, name, touch, how, args.screenshots)
                        check(not errors, f"{name}: the page threw: {errors}")
                        context.close()
                finally:
                    api.stopping.set()
                    server.shutdown()
                    server.server_close()
                    thread.join()
                report.append(f"{name} ({width}x{height}): " + ("; ".join(found) if found else "ok: the sentence floated "
                              "over no row while the selection was hidden, and went with the clear"))
                failures.extend(found)
    for line in report:
        print(line)
    if failures and not args.report_only:
        raise SystemExit("FAIL: the substitute sentence floated over a row or outlived the clear:\n  " + "\n  ".join(failures))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
