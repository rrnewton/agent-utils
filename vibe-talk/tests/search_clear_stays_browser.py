#!/usr/bin/env python3
"""Clearing a search keeps the found message where it stands on the screen, in real Chromium.

`#2423 search-escape-clear-moves-found-message`. The owner, 2026-11-27: "when I find a
message and I press escape to clear the search. The highlighted message doesn't stay put on
screen." Owner rule 370 (ui-rules R12): the row being read never moves. This drives the real
page (fixture API only, served on a kernel-picked loopback port), at a desk (1280x800) and on
a phone (412x915, keyboard closed and open), in light and dark:

  park the reader, open the search, type a word, find a hit (as the entry left it, after the
  reader scrolled to it, or tapped), then clear the search (Escape at the desk, the X on the
  phone, the magnifier, or the field emptied) -> every row is back, and a per-frame trace of
  the found message's top, from the frame before the clear until 60 frames after the last
  change, moves it at most 0.5 px.

A search typed where hits stood and closed at once, with nothing found (no landing, no scroll, no
result highlighted), keeps today's behaviour: the row read before the search comes back where it
stood (`#243`, `#1968`). Over 200 rows the walks run where the reader moved or highlighted a result
(the typing's own fast pass is `#2424`'s), and with the Discuss half-pane open.

Pass --screenshots DIR for a PNG before and after each clear, --trace DIR for one JSON
sidecar per walk, --only NAME to run the walks whose names contain NAME, --count N for one
channel size, --all-schemes for every walk in dark too, and --report-only to measure a page
that fails (main before the change) without failing.
"""

from __future__ import annotations

import argparse
import json
import os
import tempfile
import threading
from datetime import datetime, timedelta, timezone
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from zoneinfo import ZoneInfo

import bookmark_browser as fixture
from bookmark_browser import AGENT, CHANNEL, PEER, TOKEN, FakeApi, Json, check, serve
import page_sources
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page, Playwright

ZONE = "America/New_York"
NOW = datetime(2026, 10, 12, 12, 0, tzinfo=ZoneInfo(ZONE))
EPOCH = datetime(2026, 10, 12, 6, 0, tzinfo=timezone.utc)
KEYBOARD_PX = 340
COUNT = 120
# "needle" in a handful of old messages and in every third of the newest twenty-five: a reader
# parked mid-channel has no hit on the screen they type at, so the search's entry lands them on
# the newest hits (`#1734`); a reader parked among the old ones has hits on screen.
NEEDLES = frozenset({5, 10, 15, 20, 25, 30, *range(COUNT - 25, COUNT, 3)})


def use_count(count: int) -> None:
    """Size the channel (`--count`): over 200 rows the search's typing takes its fast pass (`#1913`)."""
    global COUNT, NEEDLES
    COUNT = count
    NEEDLES = frozenset({5, 10, 15, 20, 25, 30, *range(COUNT - 25, COUNT, 3)})
LIMIT_PX = 0.5
FILLER = "The overnight run is still going; the shard was retried twice."

IOS_VIEWPORT_JS = """(() => {
    const real = window.visualViewport;
    const events = new EventTarget();
    const sim = {height: null, offsetTop: 0};
    const fake = {
        get width() { return real.width; },
        get height() { return sim.height === null ? real.height : sim.height; },
        get offsetTop() { return sim.offsetTop; },
        get offsetLeft() { return 0; },
        get pageTop() { return window.scrollY + sim.offsetTop; },
        get pageLeft() { return window.scrollX; },
        get scale() { return real.scale; },
        addEventListener: (type, fn, options) => events.addEventListener(type, fn, options),
        removeEventListener: (type, fn, options) => events.removeEventListener(type, fn, options),
    };
    Object.defineProperty(window, 'visualViewport', {configurable: true, get: () => fake});
    window.__iosKeyboard = (height) => { sim.height = height; events.dispatchEvent(new Event('resize')); };
    window.__iosClose = () => { sim.height = null; events.dispatchEvent(new Event('resize')); };
})();"""

# Waits on the page's own frames, never on a clock: resolves once the list's scroll offset, its
# height and the shown rows have not changed for `still` frames in a row.
QUIET_JS = """(still) => new Promise((resolve) => {
  const area = document.getElementById('scroll-area');
  let last = null, calm = 0;
  const step = () => {
    const shown = document.querySelectorAll('#discord-log > li[data-id]:not(.search-hidden)').length;
    const now = `${area.scrollTop}|${area.scrollHeight}|${area.clientHeight}|${shown}`;
    calm = now === last ? calm + 1 : 0;
    last = now;
    if (calm >= still) resolve(now); else requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
})"""

# The shown hits on screen, by id, with each one's top and bottom, and the one nearest the middle
# of the screen the reader sees first (the message their eye is on): the band under the floating
# search bar, above the list's own tools at its foot, an open keyboard and the dock bar. A message
# cut by an edge counts by the middle of the part that shows.
HITS_JS = """() => {
  const area = document.getElementById('scroll-area');
  const clip = area.getBoundingClientRect();
  const bar = document.getElementById('search-float');
  const head = bar && !bar.hidden && bar.getClientRects().length ? Math.max(clip.top, bar.getBoundingClientRect().bottom) : clip.top;
  const over = ['scroll-tools', 'read-steer', 'status-line'].reduce((room, id) => {
    const node = document.getElementById(id);
    return room + (node && !node.hidden ? node.getBoundingClientRect().height : 0); }, 0);
  let foot = clip.top + area.clientHeight - over;
  const visual = window.visualViewport;
  if (visual && visual.height > 0) foot = Math.min(foot, visual.offsetTop + visual.height);
  const dock = document.getElementById('dock');
  if (dock && dock.getClientRects().length) { const top = dock.getBoundingClientRect().top; if (top > head && top < foot) foot = top; }
  const middle = (head + foot) / 2;
  const hits = [...document.querySelectorAll('#discord-log > li[data-id]:not(.search-hidden)')]
    .map((li) => ({id: li.dataset.id, top: li.getBoundingClientRect().top, bottom: li.getBoundingClientRect().bottom,
      text: li.textContent || ''}))
    .filter((hit) => /needle/.test(hit.text) && hit.bottom > head && hit.top < foot)
    .map((hit) => ({id: hit.id, top: hit.top, bottom: hit.bottom,
      gap: Math.abs((Math.max(hit.top, head) + Math.min(hit.bottom, foot)) / 2 - middle)}));
  hits.sort((a, b) => a.gap - b.gap);
  return {hits, head, foot, scrollTop: area.scrollTop, foot_px: area.scrollHeight - area.clientHeight};
}"""

# Per-frame trace of one message's top, armed before the clear. It records every frame until 60
# frames after the last change of the message's top or the list's scroll offset or height (or
# 1200 frames, a hang backstop), and resolves with the frames.
TRACE_JS = """(id) => { window.__traceDone = new Promise((resolve) => {
  const area = document.getElementById('scroll-area');
  const frames = [];
  let lastChange = 0, prev = null;
  const step = () => {
    const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
    const shown = row && row.getClientRects().length > 0;
    const top = shown ? row.getBoundingClientRect().top : null;
    const key = `${top}|${area.scrollTop}|${area.scrollHeight}`;
    frames.push({frame: frames.length, top, scrollTop: area.scrollTop, height: area.scrollHeight,
      searching: document.getElementById('screen-main').hasAttribute('data-searching')});
    if (key !== prev) lastChange = frames.length - 1;
    prev = key;
    if (frames.length - 1 - lastChange >= 60 || frames.length >= 1200) resolve(frames);
    else requestAnimationFrame(step);
  };
  step();
}); return true; }"""


def message_id(index: int) -> str:
    return f"8800000000000000{index:03d}"


class ClearApi(FakeApi):
    """A channel several screens tall; "needle" in `NEEDLES`."""

    def __post_init__(self) -> None:
        for index in range(COUNT):
            words = f"Message {index}. {FILLER * (1 + index % 3)}"
            if index in NEEDLES:
                words += " The needle is here."
            self.messages.append({
                "id": message_id(index), "channel_id": CHANNEL["id"],
                "timestamp": (EPOCH + timedelta(minutes=5 * index)).isoformat().replace("+00:00", "Z"),
                "spoken_time": "", "reply_to": None, "content": words, **(PEER if index % 4 == 1 else AGENT),
            })

    def client_config(self) -> Json:
        return {**super().client_config(), "discuss_available": True}


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help="directory holding the page to serve")
    parser.add_argument("--screenshots", type=Path, help="directory to write PNGs into")
    parser.add_argument("--trace", type=Path, help="directory to write one JSON trace per walk into")
    parser.add_argument("--only", default="", help="run only the walks whose names contain this")
    parser.add_argument("--count", type=int, default=None,
                        help="run every walk on a channel of this many messages only (default: 120, and 260 for "
                             "the clears the typing's fast pass changes; over 200 takes that pass)")
    parser.add_argument("--all-schemes", action="store_true", help="run every walk in dark as well as light")
    parser.add_argument("--report-only", action="store_true",
                        help="print every walk's drift and exit 0 (for measuring main)")
    return parser.parse_args()


def quiet(page: Page, frames: int = 30) -> None:
    page.evaluate(QUIET_JS, frames)


def sign_in(page: Page, keyboard: bool) -> None:
    if keyboard:
        page.evaluate(f"() => window.__iosKeyboard(window.innerHeight - {KEYBOARD_PX})")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function("() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden", timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass  # no first-run Welcome on this path (#1781)
    page.wait_for_selector("#search-toggle", state="visible")
    page.wait_for_function(f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {COUNT}")
    quiet(page)


def touch_scroll(page: Page, dy: float) -> None:
    """A reader's own swipe on a phone (`dy` > 0 scrolls toward older rows): raw touch events, the
    finger held still before it lifts so no fling carries the list on."""
    session = page.context.new_cdp_session(page)
    x, y, steps = 200.0, 420.0 - max(dy, 0), 12
    session.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": x, "y": y}]})
    for step in range(1, steps + 1):
        session.send("Input.dispatchTouchEvent", {"type": "touchMove", "touchPoints": [{"x": x, "y": y + dy * step / steps}]})
    for _ in range(8):
        session.send("Input.dispatchTouchEvent", {"type": "touchMove", "touchPoints": [{"x": x, "y": y + dy}]})
    session.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
    session.detach()


def reader_scroll(page: Page, touch: bool, dy: float) -> None:
    """The reader scrolls by about `dy` px toward older rows: a swipe on a phone, the wheel at a desk
    (whose pointer, resting on the list, lights the row under it as a desk reader's does)."""
    if touch:
        touch_scroll(page, dy)
    else:
        page.mouse.move(200, 400)
        page.mouse.wheel(0, -dy)
    quiet(page)


def open_discuss(page: Page) -> None:
    """The Discuss half-pane open beside the channel, as the owner-UI matrix asks."""
    if page.locator("#discuss-open").is_visible():
        page.click("#discuss-open")
    else:
        page.click("#action-menu-open")
        page.wait_for_selector("#discuss-menu-open", state="visible")
        page.click("#discuss-menu-open")
    page.wait_for_selector("#discuss-panel", state="visible")
    quiet(page)


def park(page: Page, index: int, touch: bool) -> None:
    """Put message `index` near the head of the list's box, ending in a reader's own scroll."""
    page.evaluate("""(id) => { const area = document.getElementById('scroll-area');
      const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
      area.scrollTop += row.getBoundingClientRect().top - area.getBoundingClientRect().top - 40; }""", message_id(index))
    quiet(page)
    reader_scroll(page, touch, -40)
    if not touch:
        # A desk reader's pointer rests on the row they read (it lights it, `pointRow`), not on
        # whatever the wheel left under it.
        x, y = page.evaluate("""(id) => { const r = document.querySelector(`#discord-log > li[data-id="${id}"]`)
          .getBoundingClientRect(); return [r.left + 40, r.top + Math.min(20, r.height / 2)]; }""", message_id(index))
        page.mouse.move(x, y)
        quiet(page)


def press(page: Page, selector: str, touch: bool) -> None:
    if touch:
        page.tap(selector)
    else:
        page.click(selector)


# The row the reader reads as they start typing: the first message row standing below the floating
# line (the freshness pill or the search bar, whichever is lower), with its top.
READING_JS = """() => {
  const area = document.getElementById('scroll-area');
  let clear = area.getBoundingClientRect().top;
  for (const id of ['channel-freshness', 'search-float']) {
    const node = document.getElementById(id);
    if (node && !node.hidden && node.getBoundingClientRect().height > 0) clear = Math.max(clear, node.getBoundingClientRect().bottom);
  }
  const row = [...document.querySelectorAll('#discord-log > li[data-id]:not(.search-hidden)')]
    .find((li) => li.getBoundingClientRect().bottom > clear);
  return row ? {id: row.dataset.id, top: row.getBoundingClientRect().top} : null;
}"""


def open_search(page: Page, touch: bool) -> Json:
    """Open the bar; answer the row the reader reads as they start typing."""
    press(page, "#search-toggle", touch)
    page.wait_for_selector("#search-field", state="visible")
    quiet(page)
    reading = page.evaluate(READING_JS)
    return reading if isinstance(reading, dict) else {}


def search(page: Page, word: str, touch: bool) -> Json:
    reading = open_search(page, touch)
    page.type("#search-field", word, delay=40)
    page.wait_for_function(f"() => document.querySelectorAll('#discord-log > li[data-id]:not(.search-hidden)').length === {len(NEEDLES)}")
    quiet(page, 45)
    return reading


def hits(page: Page) -> Json:
    found = page.evaluate(HITS_JS)
    return found if isinstance(found, dict) else {}


def hit_list(seen: Json) -> list[Json]:
    """The hits `HITS_JS` answered, nearest the middle first."""
    raw = seen.get("hits")
    return [hit for hit in raw if isinstance(hit, dict)] if isinstance(raw, list) else []


def number(value: object) -> float:
    return float(value) if isinstance(value, (int, float)) else 0.0


def clear(page: Page, how: str, touch: bool) -> None:
    if how == "escape":
        page.keyboard.press("Escape")
    elif how == "x":
        press(page, "#search-close", touch)
    elif how == "glass":
        press(page, "#search-toggle", touch)
    elif how == "emptied":
        page.fill("#search-field", "")
    else:
        raise ValueError(how)


def walk(page: Page, name: str, parked: int, find: str, how: str, touch: bool, shots: Path | None,
         traces: Path | None) -> tuple[str, float, bool]:
    park(page, parked, touch)
    reading = search(page, "needle", touch)
    if find == "scrolled":
        reader_scroll(page, touch, 260)
    seen = hits(page)
    found = hit_list(seen)
    check(bool(found), f"{name}: no hit stands on the screen to find ({seen})")
    # The found message: the one the reader tapped; else the one lit as selected (a desk reader's
    # pointer resting on it); else the one nearest the middle of the screen.
    lit = page.evaluate("""(ids) => { const row = document.querySelector('#discord-log > li[data-selected="true"]:not(.search-hidden)');
      return row && ids.includes(row.dataset.id) ? row.dataset.id : null; }""", [str(hit.get("id")) for hit in found])
    target = str(lit or found[0].get("id"))
    # Typed and closed with nothing found (the liaison's scope for `#2423`): no landing, no scroll, no
    # result highlighted. Today's behaviour stands: the row read before the search keeps its place.
    reference: float | None = None
    if find == "typed":
        check(lit is None, f"{name}: a result is lit ({lit}), so this walk found one")
        target = str(reading.get("id"))
        reference = number(reading.get("top"))
    if find == "tapped":
        target = str(found[0].get("id"))
        # A tap (a click at a desk) on the found message's body selects it (`#239`); the focus
        # leaves the field.
        x, y = page.evaluate("""(id) => { const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
          const r = row.getBoundingClientRect(); return [r.left + 40, r.top + 12]; }""", target)
        if touch:
            page.touchscreen.tap(x, y)
        else:
            page.mouse.click(x, y)
        quiet(page)
    if shots is not None:
        page.screenshot(path=str(shots / f"{name}-before.png"))
    page.evaluate(TRACE_JS, target)
    clear(page, how, touch)
    raw = page.evaluate("() => window.__traceDone")
    frames: list[Json] = [frame for frame in raw if isinstance(frame, dict)] if isinstance(raw, list) else []
    check(bool(frames), f"{name}: no trace")
    tops: list[float | None] = [number(f.get("top")) if f.get("top") is not None else None for f in frames]
    first = tops[0]
    gone = [index for index, top in enumerate(tops) if top is None]
    if reference is not None:
        # The row read before the search may be one the search hid: measured from the frame the clear
        # shows it again, against where it stood as the typing began.
        first = reference
        shown_at = next((i for i, top in enumerate(tops) if top is not None), None)
        gone = [0] if shown_at is None else [index for index in range(shown_at, len(tops)) if tops[index] is None]
    base = first
    drift = max((abs(top - base) for top in tops if top is not None), default=float("inf")) if base is not None else float("inf")
    shown = page.evaluate("() => document.querySelectorAll('#discord-log > li[data-id]:not(.search-hidden)').length")
    searching = page.evaluate("() => document.getElementById('screen-main').hasAttribute('data-searching')")
    if shots is not None:
        page.screenshot(path=str(shots / f"{name}-after.png"))
    if traces is not None:
        (traces / f"{name}.json").write_text(json.dumps({
            "walk": name, "parked": parked, "find": find, "clear": how, "target": target, "lit": lit,
            "start_top": first, "drift_px": drift if gone == [] else None, "frames_without_row": gone,
            "rows_after": shown, "frames": frames}, indent=1) + "\n")
    measured = [index for index, top in enumerate(tops) if top is not None] or [0]
    worst = max(measured, key=lambda index: abs((tops[index] or 0.0) - (base or 0.0)))
    line = (f"{name}: target {target} top {first} -> worst {tops[worst]} at frame {worst} of {len(frames)} "
            f"(drift {drift:.2f} px{', gone ' + str(len(gone)) + ' frames' if gone else ''}), rows {shown}")
    # Emptying the field clears the search but leaves its bar open; every other clear folds it.
    check(shown == COUNT and (how == "emptied" or not searching),
          f"{name}: the clear left {shown} rows shown, searching={searching}")
    return line, drift, bool(gone)


WALKS = (
    # name, parked index, how the hit was found, how the search is cleared
    # Found: the entry landed the reader on the newest hits, they moved in the results, or they
    # highlighted one; the found message keeps its top.
    ("landed", 60, "entry", None),
    ("landed-scrolled", 60, "scrolled", None),
    ("landed-tapped", 60, "tapped", None),
    ("onscreen-scrolled", 12, "scrolled", None),
    ("onscreen-tapped", 12, "tapped", None),
    ("landed-glass", 60, "entry", "glass"),
    ("landed-emptied", 60, "entry", "emptied"),
    ("landed-scrolled-emptied", 60, "scrolled", "emptied"),
    # Nothing found: typed where hits stood on the screen and closed at once; the row read before
    # the search comes back where it stood.
    ("typed-closed", 12, "typed", None),
)


def launch(playwright: Playwright, args: argparse.Namespace, profile: str, width: int, height: int,
           touch: bool, scheme: str) -> BrowserContext:
    context = playwright.chromium.launch_persistent_context(
        profile, headless=True, executable_path=args.browser_executable, timezone_id=ZONE,
        viewport={"width": width, "height": height}, color_scheme=scheme,  # type: ignore[arg-type]
        device_scale_factor=2.625 if touch else 1, is_mobile=touch, has_touch=touch,
    )
    browser_waits.hot_host_timeouts(context)
    context.add_init_script(IOS_VIEWPORT_JS)
    return context


# (scheme, profile, keyboard, walk, channel size): every walk in light on a 120-row channel; on a
# 260-row channel (the typing's fast pass, `#1913`) the clears where the reader moved or highlighted;
# dark once per profile. `--all-schemes` runs every walk in dark too. Over 200 rows the typing's fast
# pass hides rows before the place-keeper measures, so the entry's landing (`#1734`) and the typing's
# hold are not yet what they are on a short list: the landed and typed-closed walks join the 260-row
# set with `#2424 search-view-unstable-while-searching`.
PROFILES = {"desk": (1280, 800, False, "escape"), "phone": (412, 915, True, "x")}
BIG = 260
BIG_WALKS = ("landed-scrolled", "onscreen-scrolled", "landed-tapped", "landed-scrolled-emptied")


DISCUSS_WALKS = ("landed-scrolled", "onscreen-scrolled")

Run = tuple[str, str, bool, tuple[str, int, str, str | None], int, bool]


def plan(all_schemes: bool, count: int | None) -> list[Run]:
    """(scheme, profile, keyboard, walk, channel size, Discuss open) for every walk to run."""
    runs: list[Run] = []
    places = (("desk", False), ("phone", False), ("phone", True))
    for scheme in ("light", "dark"):
        for label, keyboard in places:
            for spec in WALKS:
                if scheme == "light" or all_schemes or spec[0] == "landed":
                    runs.append((scheme, label, keyboard, spec, count or COUNT, False))
    if count is None:
        for label, keyboard in places[:2]:
            runs.extend(("light", label, keyboard, spec, BIG, False) for spec in WALKS if spec[0] in BIG_WALKS)
    # The Discuss half-pane open beside the channel (desk and phone; every place and scheme with
    # `--all-schemes`).
    for scheme in ("light", "dark") if all_schemes else ("light",):
        for label, keyboard in places if all_schemes else places[:2]:
            runs.extend((scheme, label, keyboard, spec, count or COUNT, True) for spec in WALKS if spec[0] in DISCUSS_WALKS)
    return runs


def side_thread_walk(playwright: Playwright, args: argparse.Namespace) -> str:
    """Two columns at a desk (1920x1080): a search typed in the right column's cards past their foot and
    closed with Escape, then a thread opened on the right and a reply arriving with the reader a little
    way into its room: the reply is followed as at the thread's foot. The found message is the channel list's to keep; a reader working in
    the right column keeps its own close, and nothing the clear holds may outlive it there (the gate's
    catch on the first `#2423` head: column_overscroll_browser.py's FOLLOW)."""
    import column_overscroll_browser as overscroll
    from scroll_stability_browser import Reader, handler
    from two_column_browser import UNSPLIT_CARDS_JS

    api = overscroll.OverscrollApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-search-clear-columns-") as profile:
            context = playwright.chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": 1920, "height": 1080}, color_scheme="light")
            browser_waits.hot_host_timeouts(context)
            try:
                page = context.pages[0]
                errors: list[str] = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                context.add_init_script(UNSPLIT_CARDS_JS)
                page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
                reader = Reader(page, api, None, "two-columns", False)
                overscroll.open_columns(reader)
                overscroll.search_walk(reader)
                # The thread walk the gate caught: the short thread opened on the right, a little way into
                # its room past the foot, a reply arriving followed as at the foot (FOLLOW), and on.
                overscroll.thread_walk(reader)
                check(not errors, f"two-columns: the page threw: {errors}")
            finally:
                context.close()
        return "two-columns 1920x1080: a search closed in the right column, then the thread walk there: " + "; ".join(reader.notes[-4:])
    finally:
        api.stopping.set()
        api.timeline_gate.set()
        server.shutdown()
        server.server_close()
        thread.join()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required") from error
    for directory in (args.screenshots, args.trace):
        if directory is not None:
            directory.mkdir(parents=True, exist_ok=True)
    web_root = page_sources.served(args.web_root)
    report: list[str] = []
    failures: list[str] = []
    with sync_playwright() as playwright:
        for scheme, label, keyboard, (walk_name, parked, find, how), count, discuss in plan(args.all_schemes, args.count):
            width, height, touch, default_clear = PROFILES[label]
            name = (f"{scheme}-{label}-{'keyboard' if keyboard else 'nokeyboard'}-{count}-{walk_name}"
                    f"{'-discuss' if discuss else ''}")
            if args.only and args.only not in name:
                continue
            use_count(count)
            api = ClearApi()
            server, thread, url = serve(api, web_root)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-search-clear-") as profile:
                    context = launch(playwright, args, profile, width, height, touch, scheme)
                    context.clock.set_fixed_time(NOW)
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(url, wait_until="load")
                    sign_in(page, keyboard)
                    if discuss:
                        open_discuss(page)
                    line, drift, gone = walk(page, name, parked, find, how or default_clear, touch,
                                             args.screenshots, args.trace)
                    check(not errors, f"{name}: the page threw: {errors}")
                    context.close()
            finally:
                api.stopping.set()
                server.shutdown()
                server.server_close()
                thread.join()
            report.append(line)
            if drift > LIMIT_PX or gone:
                failures.append(line)
        if not args.only or args.only in "two-columns":
            try:
                report.append(side_thread_walk(playwright, args))
            except AssertionError as error:
                if not args.report_only:
                    raise
                report.append(f"two-columns: {error}")
    for line in report:
        print(line)
    if failures and not args.report_only:
        raise SystemExit("FAIL: the found message moved when the search cleared:\n  " + "\n  ".join(failures))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
