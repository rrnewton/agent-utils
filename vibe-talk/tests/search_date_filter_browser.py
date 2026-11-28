#!/usr/bin/env python3
"""The search bar's dates in real Chromium: the hint, the calendar, and dated searches (`#2425`).

`#2425 vibe-talk-search-date-filter`. The owner, 2026-11-27: "I was getting old results I don't
want. The search interface should support "after:" like gmail and remind me the syntax. But also
there should be a calendar icon in the search bar and clicking it should provide an option for last
K days or since DATE (e.g. date range where you don't have to fill in both)".

One channel of 60 messages, two a day for thirty days, read on Friday 27 November 2026 at noon in New
York (the browser's clock is fixed). The page loads the newest 30; "kestrel" stands in two older
messages only the server holds, two loaded ones older than a week, and three loaded ones inside the
week. At a desk (1280x800) and on a phone (412x915, by touch), in light and dark, with the keyboard
closed and open (on the phone the hint steps aside while the keyboard is up), plus the narrow
phone (360x800, light) where the author filter hangs under the bar (a shrunken `visualViewport`, as tests/server_hits_browser.py plays it):

  open the bar -> the calendar stands on it, and the hint line under it shows the date syntax with a
  real example ("after:2026-11-20 · after:7d · before:2026-11-27"); the hint lies inside the bar's
  width and the viewport, below the bar, and the first row of the list stands below the hint;
  search "kestrel" -> five loaded rows and the server's two older hits (cards): the old results;
  the calendar -> the Dates panel, inside the viewport, Any time chosen; Last 7 -> the panel says
  "Searches after:2026-11-20" before writing it; Use dates -> the field reads
  "kestrel after:2026-11-20", the server is asked with that date inside q and the reader's zone, and
  every row the list shows (loaded or a server card) was written on or after Nov 20; the calendar is
  pressed and named "Dates: since Nov 20, 2026", the hint has stepped aside (it shows only while
  nothing is typed, floating with no room of its own), and the step back offers no search before
  the loaded floor ("Before ...: outside your dates", disabled);
  typed "kestrel after:7d" -> the same rows, and the calendar names "last 7 days" (its title:
  moving with today);
  the panel's Clear -> "kestrel" again, the old results back; the hint opens the panel; Between
  Nov 10 - Nov 15 -> "after:2026-11-10 before:2026-11-16", one hit, the calendar named "Nov 10 – Nov 15, 2026";
  reopened, pre-filled; a range the wrong way round refused in the panel; 3 days said as
  "after:2026-11-24" before it is written. The hint takes no room, so it moves nothing.

What the server reads is not this walk's to prove: tests/message_cache_serve.rs
(`a_dated_search_reads_only_its_window_and_counts_only_what_it_read`) asserts, through the real
router and the real cache, that a row outside the window is neither a hit nor counted in examined.
This fake server only answers by the same window, so the page's request is what is asserted here.

Main before the change is the negative control: run with --web-root pointing at its page and the bar
has no calendar or hint, and a typed after:7d is a word that hides every row.

Pass --screenshots DIR to keep a PNG of each stage, --only CELL for one cell.
"""

from __future__ import annotations

import argparse
import os
import re
import tempfile
import threading
import time
from datetime import datetime, timedelta, timezone
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, unquote, urlsplit
from zoneinfo import ZoneInfo

import bookmark_browser as fixture
from bookmark_browser import AGENT, CHANNEL, TOKEN, FakeApi, Json, check, handler_for
import page_sources
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page, Playwright

ZONE = "America/New_York"
NOW = datetime(2026, 11, 27, 12, 0, tzinfo=ZoneInfo(ZONE))
OWNER = {"author": "owner", "author_id": "1000000000000000007", "author_is_bot": False}
KEYBOARD_PX = 340
TOTAL = 60
LOADED = 30
WORD = "kestrel"
# Messages holding the word: two only the server holds (old), two loaded but older than a week, and
# three loaded inside the week (45 is exactly seven days back, at noon).
SERVER_ONLY = (5, 20)
LOADED_OLD = (35, 43)
RECENT = (45, 50, 58)
WEEK_START = datetime(2026, 11, 20, 0, 0, tzinfo=ZoneInfo(ZONE))
WEEK_TOKEN = "after:2026-11-20"

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


def sent_at(index: int) -> datetime:
    """Two a day, twelve hours apart, the newest at noon today."""
    return NOW - timedelta(hours=12 * (TOTAL - 1 - index))


def stamp(index: int) -> str:
    return sent_at(index).astimezone(timezone.utc).isoformat().replace("+00:00", "Z")


def stamp_ms(index: int) -> int:
    return int(sent_at(index).timestamp() * 1000)


def message_id(index: int) -> str:
    return f"891000000000000{index:03d}"


def day_start_ms(text: str) -> int | None:
    """The start of a `YYYY-MM-DD` or relative (`7d`, `today`, `yesterday`) day in the reader's zone."""
    match = re.fullmatch(r"(\d{4})-(\d{1,2})-(\d{1,2})", text)
    if match:
        day = datetime(int(match[1]), int(match[2]), int(match[3]), tzinfo=ZoneInfo(ZONE))
    else:
        lower = text.lower()
        back = 0 if lower == "today" else 1 if lower == "yesterday" else None
        relative = re.fullmatch(r"(\d{1,5})d", lower)
        if relative:
            back = int(relative[1])
        if back is None:
            return None
        day = (NOW - timedelta(days=back)).replace(hour=0, minute=0, second=0, microsecond=0)
    return int(day.timestamp() * 1000)


class DatesApi(FakeApi):
    """60 messages over thirty days; the page loads the newest 30; the server answers searches by the
    query's own window, as the real server does, and records each ask."""

    searches: list[Json] = []

    def __post_init__(self) -> None:
        self.searches = []
        for index in range(TOTAL):
            words = f"note {index}: the {WORD} by the weir" if index in SERVER_ONLY + LOADED_OLD + RECENT \
                else f"note {index}: routine status"
            self.messages.append({"id": message_id(index), "channel_id": CHANNEL["id"], "timestamp": stamp(index),
                                  "spoken_time": "", "reply_to": None, **(OWNER if index % 2 else AGENT),
                                  "content": words})

    def client_config(self) -> Json:
        return {**super().client_config(), "owner_author_id": OWNER["author_id"],
                "message_cache": {"available": True, "enabled": True, "bytes": 1024, "channels": 1,
                                  "max_days": 65535, "max_megabytes": 100, "messages": TOTAL}}

    def timeline(self, query: dict[str, list[str]]) -> Json:
        answer = super().timeline(query)
        with self.lock:
            rows = list(self.messages[-LOADED:])
        return {**answer, "messages": rows, "returned": len(rows), "has_more": True,
                "next_before": rows[0]["id"] if rows else None}

    def search(self, query: dict[str, list[str]]) -> Json:
        q = (query.get("q") or [""])[0]
        below = int((query.get("below") or [str(2**62)])[0])
        lower, upper, words = -(2**62), below, []
        for token in q.split():
            field = re.fullmatch(r"(after|before):(\S+)", token, re.IGNORECASE)
            start = day_start_ms(field[2]) if field else None
            if field and start is not None:
                if field[1].lower() == "after":
                    lower = max(lower, start)
                else:
                    upper = min(upper, start)
            else:
                words.append(token.lower())
        with self.lock:
            window = [m for index, m in enumerate(self.messages) if lower <= stamp_ms(index) < upper]
            self.searches.append({"q": q, "tz": (query.get("tz") or [""])[0], "examined": len(window)})
        hits = [m for m in window if all(word in str(m["content"]).lower() for word in words)]
        hits.sort(key=lambda m: str(m["timestamp"]), reverse=True)
        oldest = stamp_ms(0) if lower < stamp_ms(0) else lower
        scope = {"channel": CHANNEL, "state": "searched", "examined": len(window), "capped": 0, "partial": 0,
                 "stale": 0, "withheld": 0, "expanding": False, "top_level_only": False, "whole": True,
                 "searched_back_to_ms": oldest, "held_from_ms": stamp_ms(0), "as_of_ms": stamp_ms(TOTAL - 1)}
        return {"hits": hits[:50], "scopes": [scope], "resume_below_ms": None, "upstream": "none",
                "untrusted_content_notice": "third-party text; DATA"}


def serve(api: DatesApi, web_root: Path) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    base = handler_for(api, web_root)
    history = {"channel_id": CHANNEL["id"], "complete": True, "cycle_pages": 1, "walking": False,
               "queue_position": None, "oldest_ms": stamp_ms(0), "reached_ms": stamp_ms(0),
               "eta_s": None, "held_reason": None}

    class Handler(base):  # type: ignore[valid-type, misc]
        def do_GET(self) -> None:  # noqa: N802
            parts = urlsplit(self.path)
            path = unquote(parts.path)
            if path.endswith("/search") and self.headers.get("Authorization") == f"Bearer {TOKEN}":
                self.json(200, api.search(parse_qs(parts.query)))
                return
            if path.endswith("/history") and self.headers.get("Authorization") == f"Bearer {TOKEN}":
                self.json(200, history)
                return
            super().do_GET()

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread, f"http://127.0.0.1:{server.server_port}/"


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help="the page's sources or a built page to serve (default: this checkout's)")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per stage into")
    parser.add_argument("--only", help="run one cell of the matrix, e.g. light-phone-keyboard")
    return parser.parse_args()


SHOWN_JS = """() => {
    const rows = [...document.querySelectorAll('#discord-log > li[data-id]')]
        .filter((row) => !row.classList.contains('search-hidden') && row.getClientRects().length > 0);
    return rows.map((row) => ({
        id: row.getAttribute('data-id'),
        card: row.getAttribute('data-server-hit') === 'card',
        at: Date.parse(row.querySelector('time')?.getAttribute('datetime') || '') || 0,
    }));
}"""

BOXES_JS = """() => {
    const box = (id) => { const e = document.getElementById(id);
        if (!e || e.hidden || !e.getClientRects().length) return null;
        const r = e.getBoundingClientRect(); return {top: r.top, bottom: r.bottom, left: r.left, right: r.right}; };
    const first = [...document.querySelectorAll('#discord-log > li[data-id]')]
        .find((row) => !row.classList.contains('search-hidden') && row.getClientRects().length > 0);
    const r = first ? first.getBoundingClientRect() : null;
    return {bar: box('search-float'), hint: box('search-date-hint'), panel: box('search-dates-panel'),
            dates: box('search-dates'), firstRow: r ? {top: r.top, bottom: r.bottom} : null,
            width: window.innerWidth, height: window.innerHeight,
            hintText: (document.getElementById('search-date-hint')?.textContent || '').trim(),
            datesName: document.getElementById('search-dates')?.getAttribute('aria-label') || null,
            datesPressed: document.getElementById('search-dates')?.getAttribute('aria-pressed') || null,
            field: document.getElementById('search-field').value};
}"""


# The floating hint's box against every rendered line of text in the list's rows (and the lines
# between them): each text node's client rects, as the browser lays them out. Empty when the hint is
# not drawn, or overlaps no text; otherwise the rows (by id or kind) and the overlapping words.
HINT_OVER_TEXT_JS = """() => {
    const hint = document.getElementById('search-date-hint');
    if (!hint || !hint.getClientRects().length) return [];
    const h = hint.getBoundingClientRect();
    if (h.width === 0 || h.height === 0) return [];
    const over = [];
    for (const row of document.querySelectorAll('#discord-log > li')) {
        if (!row.getClientRects().length) continue;
        const r = row.getBoundingClientRect();
        if (r.bottom <= h.top || r.top >= h.bottom) continue;
        const walker = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
        for (let node = walker.nextNode(); node; node = walker.nextNode()) {
            if (!node.textContent.trim()) continue;
            const range = document.createRange();
            range.selectNodeContents(node);
            for (const box of range.getClientRects()) {
                if (box.width > 0 && box.height > 0 && box.left < h.right && box.right > h.left
                    && box.top < h.bottom && box.bottom > h.top) {
                    over.push({row: row.getAttribute('data-id') || row.className, text: node.textContent.trim().slice(0, 40)});
                    break;
                }
            }
        }
    }
    return over;
}"""


class Walk:
    def __init__(self, page: Page, label: str, shots: Path | None, api: DatesApi, touch: bool) -> None:
        self.page, self.label, self.shots, self.api, self.touch = page, label, shots, api, touch

    def check(self, condition: bool, why: str) -> None:
        check(condition, f"{self.label}: {why}")

    def shot(self, name: str) -> None:
        # The liaison's condition (#2425 v2): in every state this walk photographs, the floating hint
        # is a label under the bar, never a note over rows: it intersects no row's text.
        over = self.page.evaluate(HINT_OVER_TEXT_JS)
        self.check(over == [], f"{name}: the floating hint stands over a row's text: {over}")
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.label}-{name}.png"))

    def settled(self) -> None:
        self.page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")

    def until(self, script: str, why: str, seconds: float = 30) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and not self.page.evaluate(script):
            self.page.wait_for_timeout(25)
        self.check(bool(self.page.evaluate(script)), why)

    def press(self, selector: str) -> None:
        if self.touch:
            self.page.tap(selector)
        else:
            self.page.click(selector)

    def boxes(self) -> dict[str, object]:
        return dict(self.page.evaluate(BOXES_JS))

    def shown(self) -> list[dict[str, object]]:
        return list(self.page.evaluate(SHOWN_JS))

    def asked(self, query: str, why: str) -> Json:
        deadline = time.monotonic() + 30
        while time.monotonic() < deadline and not any(s["q"] == query for s in self.api.searches):
            self.page.wait_for_timeout(50)
        found = [s for s in self.api.searches if s["q"] == query]
        self.check(bool(found), f"{why}: the server was never asked {query!r} (asked {self.api.searches})")
        return found[-1] if found else {}


def ids(indices: tuple[int, ...]) -> set[str]:
    return {message_id(index) for index in indices}


def num(box: object, key: str) -> float:
    """A measured box's edge, or NaN (which fails every comparison) when it was not measured."""
    value = box.get(key) if isinstance(box, dict) else None
    return float(value) if isinstance(value, (int, float)) else float("nan")


def inside(box: object, width: object, height: object) -> bool:
    if not isinstance(box, dict) or not isinstance(width, (int, float)) or not isinstance(height, (int, float)):
        return False
    return bool(num(box, "left") >= -0.5 and num(box, "right") <= width + 0.5
                and num(box, "top") >= -0.5 and num(box, "bottom") <= height + 0.5)


def walk_through(walk: Walk, keyboard: bool) -> None:
    page, api = walk.page, walk.api
    # The hint steps aside on a phone while its on-screen keyboard is up (the calendar stays).
    hint_up = not (walk.touch and keyboard)
    if keyboard:
        page.evaluate(f"() => window.__iosKeyboard(window.innerHeight - {KEYBOARD_PX})")
        walk.settled()
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function("() => !document.getElementById('screen-welcome').hidden || "
                               "!document.getElementById('screen-main').hidden", timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass  # no first-run Welcome on this path (#1781)
    page.wait_for_selector("#search-toggle", state="visible", timeout=30_000)
    walk.until(f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {LOADED}",
               "the channel did not open with the newest thirty messages")
    walk.settled()

    # 1. The open bar: the calendar on it, and the hint with a real example under it.
    walk.press("#search-toggle")
    walk.until("() => !document.getElementById('search-field').hidden", "the search field did not open")
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
    walk.settled()
    walk.shot("1-bar-hint")
    boxes = walk.boxes()
    walk.check(boxes["dates"] is not None, "no calendar on the open bar")
    walk.check(boxes["hintText"] == "after:2026-11-20 · after:7d · before:2026-11-27",
               f"the hint does not show the date syntax with a real example: {boxes['hintText']!r}")
    bar, hint, first = boxes["bar"], boxes["hint"], boxes["firstRow"]
    assert isinstance(bar, dict)
    if hint_up:
        # It floats where the chips float and takes no room: below the bar, inside the viewport.
        walk.check(isinstance(hint, dict), "no hint line under the open bar")
        assert isinstance(hint, dict)
        walk.check(hint["top"] >= bar["bottom"] - 0.5, f"the hint stands over the bar: hint {hint}, bar {bar}")
        walk.check(inside(hint, boxes["width"], boxes["height"]), f"the hint leaves the viewport: {hint}")
    else:
        # A phone with the keyboard up: the hint steps aside.
        walk.check(hint is None, f"the hint stands while the phone's keyboard is up: {hint}")
    # Every toggle on the bar stays a whole target: a tap at its centre lands on it, never on the hint
    # (up to 400px the author filter hangs under the bar, where the hint would otherwise stand).
    covered = page.evaluate("""() => ['search-dates', 'search-entire-threads', 'author-filter', 'links-filter',
        'labels-button', 'search-toggle', 'search-close'].filter((id) => {
          const e = document.getElementById(id);
          if (!e || e.hidden || !e.getClientRects().length) return false;
          const r = e.getBoundingClientRect();
          const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
          return !(hit === e || e.contains(hit)); })""")
    walk.check(covered == [], f"a toggle's centre is covered: {covered}")
    dates = boxes["dates"]
    assert isinstance(dates, dict)
    walk.check(dates["right"] - dates["left"] >= 43.5 and dates["bottom"] - dates["top"] >= 43.5,
               f"the calendar is not the bar's whole 44px target: {dates}")

    # 2. The old results, as the owner met them.
    page.fill("#search-field", WORD)
    walk.asked(WORD, "the undated search")
    walk.until(f"() => ({SHOWN_JS})().filter((row) => row.card).length === {len(SERVER_ONLY)}",
               "the server's two older hits did not stand as cards")
    walk.settled()
    shown = {str(row["id"]) for row in walk.shown()}
    walk.check(shown == ids(SERVER_ONLY + LOADED_OLD + RECENT), f"the undated search did not show every hit: {sorted(shown)}")
    walk.shot("2-undated-old-results")

    # 3. The calendar's panel: Last 7, said before it is written.
    walk.press("#search-dates")
    walk.until("() => !document.getElementById('search-dates-panel').hidden", "the calendar opened no panel")
    walk.settled()
    boxes = walk.boxes()
    walk.check(inside(boxes["panel"], boxes["width"], boxes["height"]), f"the Dates panel leaves the viewport: {boxes['panel']}")
    walk.check(bool(page.evaluate("() => document.getElementById('search-dates-any').checked")),
               "with no date standing the panel does not say Any time")
    walk.shot("3-panel")
    walk.press("#search-dates-k7")
    walk.until("() => document.getElementById('search-dates-preview').textContent === 'Searches after:2026-11-20'",
               "the panel does not say what Last 7 will write")
    walk.shot("4-panel-last7")
    walk.press("#search-dates-use")
    walk.until("() => document.getElementById('search-dates-panel').hidden", "Use dates left the panel open")
    walk.until(f"() => document.getElementById('search-field').value === '{WORD} {WEEK_TOKEN}'",
               "Use dates did not write the absolute day into the field")
    asked = walk.asked(f"{WORD} {WEEK_TOKEN}", "the dated search")
    walk.check(asked.get("tz") == ZONE, f"the reader's zone did not go with the dated search: {asked}")
    walk.until(f"() => ({SHOWN_JS})().length === {len(RECENT)}", "the dated search did not leave only the week's hits")
    walk.settled()
    shown_rows = walk.shown()
    week_ms = int(WEEK_START.timestamp() * 1000)
    walk.check({str(row["id"]) for row in shown_rows} == ids(RECENT),
               f"the dated search shows other than the week's hits: {shown_rows}")
    walk.check(all(isinstance(row["at"], (int, float)) and row["at"] >= week_ms for row in shown_rows),
               f"a row older than Nov 20 stands: {shown_rows}")
    walk.check(not any(row["card"] for row in shown_rows), f"an old server hit still stands as a card: {shown_rows}")
    boxes = walk.boxes()
    walk.check(boxes["datesPressed"] == "true" and boxes["datesName"] == "Dates: since Nov 20, 2026",
               f"the calendar does not say the date standing: {boxes['datesName']!r} pressed={boxes['datesPressed']}")
    walk.check(boxes["hint"] is None, f"the hint stands over a typed search: {boxes['hint']}")
    # The step back offers no search before the loaded floor, which lies outside the dates.
    older = page.evaluate("() => { const b = document.getElementById('load-older');"
                          " return b && !b.hidden ? [b.textContent, b.disabled] : null; }")
    walk.check(older is None or (str(older[0]).endswith(": outside your dates") and older[1] is True),
               f"the step back still offers a search outside the dates: {older}")
    walk.shot("5-date-standing")

    # 4. Typed, the moving spelling: the same rows, and the hint says it moves.
    # The same search as Last 7 wrote (its tree is the same day), so the page need not ask again.
    page.fill("#search-field", f"{WORD} after:7d")
    walk.until(f"() => ({SHOWN_JS})().length === {len(RECENT)}", "a typed after:7d did not leave only the week's hits")
    walk.settled()
    walk.check({str(row["id"]) for row in walk.shown()} == ids(RECENT), "a typed after:7d shows other than the week's hits")
    walk.until("() => document.getElementById('search-dates').getAttribute('aria-label') === 'Dates: last 7 days' && "
               "document.getElementById('search-dates').title === 'Dates: last 7 days, moving with today'",
               "the calendar does not name the typed relative day, or its title does not say it moves with today")
    walk.shot("6-typed-relative")

    # 5. Clear: the old results again.
    walk.press("#search-dates")
    walk.until("() => !document.getElementById('search-dates-panel').hidden", "the calendar did not open again")
    walk.check(bool(page.evaluate("() => document.getElementById('search-dates-last').checked && "
                                  "document.getElementById('search-dates-days').value === '7'")),
               "the panel did not pre-fill the typed after:7d as Last 7")
    walk.press("#search-dates-clear")
    walk.until(f"() => document.getElementById('search-field').value === '{WORD}'", "Clear did not take the date out")
    walk.until(f"() => ({SHOWN_JS})().length === {len(SERVER_ONLY + LOADED_OLD + RECENT)}",
               "Clear did not bring the old results back")
    walk.shot("7-cleared")

    # 6. Between, either end: the panel says both tokens, Use writes them, the hint names the range;
    # reopened over a standing date it is pre-filled; a range the wrong way round is refused there;
    # a custom number of days is said before it is written. Opened by the hint this time.
    # The hint gives its line to the bucket chips while they stand; then the calendar opens it.
    hint_shown = bool(page.evaluate("() => { const h = document.getElementById('search-date-hint');"
                                    " return !!h && h.getClientRects().length > 0; }"))
    walk.press("#search-date-hint" if hint_up and hint_shown else "#search-dates")
    walk.until("() => !document.getElementById('search-dates-panel').hidden", "a tap on the hint did not open the panel")
    page.fill("#search-dates-from", "2026-11-10")
    page.fill("#search-dates-to", "2026-11-15")
    walk.until("() => document.getElementById('search-dates-preview').textContent === "
               "'Searches after:2026-11-10 before:2026-11-16'", "the panel does not say what Between will write")
    walk.check(bool(page.evaluate("() => document.getElementById('search-dates-between-mode').checked")),
               "filling a Between day did not choose Between")
    walk.shot("8-panel-between")
    walk.press("#search-dates-use")
    walk.until(f"() => document.getElementById('search-field').value === '{WORD} after:2026-11-10 before:2026-11-16'",
               "Use dates did not write the range")
    walk.until(f"() => ({SHOWN_JS})().length === 1", "the range did not leave its one hit")
    walk.check({str(row["id"]) for row in walk.shown()} == ids((35,)), f"the range shows other rows: {walk.shown()}")
    boxes = walk.boxes()
    walk.check(boxes["datesName"] == "Dates: Nov 10 – Nov 15, 2026", f"the range is not named: {boxes['datesName']!r}")
    walk.shot("9-between-standing")
    walk.press("#search-dates")
    walk.until("() => !document.getElementById('search-dates-panel').hidden && "
               "document.getElementById('search-dates-from').value === '2026-11-10' && "
               "document.getElementById('search-dates-to').value === '2026-11-15'",
               "reopened over the range, the panel is not pre-filled with it")
    walk.shot("10-panel-standing")
    page.fill("#search-dates-from", "2026-11-15")
    page.fill("#search-dates-to", "2026-11-10")
    walk.press("#search-dates-use")
    walk.until("() => document.getElementById('search-dates-error').textContent === 'The first day is after the last'",
               "a range the wrong way round was not refused in the panel")
    walk.check(bool(page.evaluate("() => !document.getElementById('search-dates-panel').hidden")),
               "the refusal closed the panel")
    walk.shot("11-panel-refusal")
    page.fill("#search-dates-days", "3")
    walk.until("() => document.getElementById('search-dates-preview').textContent === 'Searches after:2026-11-24'",
               "a custom number of days is not said before it is written")
    walk.shot("12-panel-custom-days")
    page.keyboard.press("Escape")
    walk.until("() => document.getElementById('search-dates-panel').hidden", "Escape did not put the panel away")
    if keyboard:
        page.evaluate("() => window.__iosClose()")


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
    failed: list[str] = []
    sizes = (("phone", 412, 915, True), ("desk", 1280, 800, False))
    cells = [(scheme, size, keyboard) for scheme in ("light", "dark") for size in sizes for keyboard in (False, True)]
    # The narrow phone, where the author filter hangs under the bar's left end.
    cells.append(("light", ("phone360", 360, 800, True), False))
    with sync_playwright() as playwright:
        for scheme, (label, width, height, touch), keyboard in cells:
            name = f"{scheme}-{label}-{'keyboard' if keyboard else 'nokeyboard'}"
            if args.only and args.only != name:
                continue
            api = DatesApi()
            server, thread, url = serve(api, web_root)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-search-dates-") as profile:
                    context = launch(playwright, args, profile, width, height, touch, scheme)
                    context.clock.set_fixed_time(NOW)
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(url, wait_until="load")
                    walk = Walk(page, name, args.screenshots, api, touch)
                    # Every cell is walked and said, a red one included, so the negative control
                    # (main before the change) shows each cell it fails in, not only the first.
                    try:
                        walk_through(walk, keyboard)
                        check(not errors, f"{name}: the page threw: {errors}")
                        report.append(f"{name} {width}x{height}: hint, calendar, Last 7 and a typed after:7d "
                                      f"left only the week's {len(RECENT)} hits; asked {[s['q'] for s in api.searches]}")
                    except AssertionError as error:
                        failed.append(f"{name}: FAIL {error}")
                    context.close()
            finally:
                api.stopping.set()
                server.shutdown()
                server.server_close()
                thread.join()
    for line in report + failed:
        print(line)
    if failed:
        return 1
    print("search date filter browser: all walks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
