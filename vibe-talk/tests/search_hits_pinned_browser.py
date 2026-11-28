#!/usr/bin/env python3
"""Search results shown as cards stay while the search stands: a poll's redraw keeps them.

`#2422 search-hits-vanish-after-redraw`. The owner, 2026-11-27: the phrase "low urgency" in
quotes found the 11/14 message, "but then ALL RESULTS DISAPPEAR and I end up on a blank results
page. "4 found (0 loaded, 4 older)"".

One channel of 120 messages; the page loads the newest 60, none of which holds the phrase. Four
older messages, dated 11/14, well before the loaded window, hold it. At a desk (1280x800) and on
a phone (412x915), in light and dark, with the keyboard closed and open (a shrunken
`visualViewport`, played as tests/server_hits_browser.py plays it):

  search for "low urgency" (quoted) -> the server answers with the four, and they stand as four
  result cards in the timeline; the count line names them;
  a message arrives and the page's poll draws it -> the four cards still stand, in time order,
  drawn as they were placed (same place and width), and the count line is unchanged;
  two more polls with nothing new -> the same.

`#2435 search-copy-with-placed-hits`: throughout, the copy is true of the cards: the count reads
"4 found (4 older)" (no "0 loaded"), and the line under the field "No match in the N loaded
messages; 4 older matches shown below." (never "None match" or "scattered older ones" over them).

Each poll is brought forward on the page's clock (Playwright's `clock.fast_forward` past its 45 s
timer), and every wait is a condition: the poll's answer received whole, the arrived row drawn.

Before the fix the arrival's redraw (`patchRows`) removed the placed cards and placement, memoized
on the unchanged answer, never put them back: a blank page under "4 found (0 loaded, 4 older)".

Pass --screenshots DIR to keep a PNG of each stage.
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
import time
from datetime import datetime, timedelta, timezone
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING, TypedDict, cast
from urllib.parse import parse_qs, unquote, urlsplit

import bookmark_browser as fixture
from bookmark_browser import AGENT, CHANNEL, TOKEN, FakeApi, Json, check, handler_for
import page_sources
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page, Playwright

ZONE = "America/New_York"
OWNER = {"author": "owner", "author_id": "1000000000000000007", "author_is_bot": False}
KEYBOARD_PX = 340
TOTAL = 120
LOADED = 60
PHRASE = "low urgency"
HIT_INDICES = (3, 6, 9, 12)
# The page re-reads the channel every 45 s (core/sync/poll.ts DISCORD_POLL_MS); a walk brings each
# poll forward on the page's clock instead of waiting it out.
POLL_JUMP_MS = 46_000
CARDS_JS = "() => document.querySelectorAll('#discord-log > li[data-server-hit=\"card\"]').length"

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
    """The first twenty messages on 11/14 (the four hits among them), the rest on 11/27."""
    if index < 20:
        return datetime(2026, 11, 14, 14, 0, tzinfo=timezone.utc) + timedelta(minutes=index * 7)
    return datetime(2026, 11, 27, 13, 0, tzinfo=timezone.utc) + timedelta(minutes=(index - 20) * 3)


def stamp(index: int) -> str:
    return sent_at(index).isoformat().replace("+00:00", "Z")


def stamp_ms(index: int) -> int:
    return int(sent_at(index).timestamp() * 1000)


def message_id(index: int) -> str:
    return f"890000000000000{index:03d}"


class PinnedApi(FakeApi):
    """120 messages; the page loads the newest 60; four old ones hold the phrase."""

    searches: list[Json] = []
    reads: int = 0

    def __post_init__(self) -> None:
        self.searches = []
        self.reads = 0
        for index in range(TOTAL):
            self.messages.append(self.row(index))

    def row(self, index: int) -> Json:
        words = (f"note {index}: {PHRASE} follow-up for the weekly review" if index in HIT_INDICES
                 else f"note {index}: routine status")
        return {"id": message_id(index), "channel_id": CHANNEL["id"], "timestamp": stamp(index),
                "spoken_time": "", "reply_to": None, **(OWNER if index % 2 else AGENT), "content": words}

    def arrive(self) -> str:
        """A new message at the foot of the channel, as a poll will find it."""
        with self.lock:
            index = len(self.messages)
            row = self.row(index)
            row["timestamp"] = (datetime(2026, 11, 27, 16, 0, tzinfo=timezone.utc)
                                + timedelta(minutes=index)).isoformat().replace("+00:00", "Z")
            row["content"] = f"note {index}: arrived while the search stands"
            self.messages.append(row)
            return str(row["id"])

    def client_config(self) -> Json:
        return {**super().client_config(), "owner_author_id": OWNER["author_id"],
                "message_cache": {"available": True, "enabled": True, "bytes": 1024,
                                  "channels": 1, "max_days": 65535, "max_megabytes": 100,
                                  "messages": TOTAL}}

    def timeline(self, query: dict[str, list[str]]) -> Json:
        answer = super().timeline(query)
        with self.lock:
            self.reads += 1
            rows = list(self.messages[-LOADED:])
        return {**answer, "messages": rows, "returned": len(rows), "has_more": True,
                "next_before": rows[0]["id"] if rows else None}

    def search(self, query: dict[str, list[str]]) -> Json:
        with self.lock:
            self.searches.append(dict(query))
            held = list(self.messages)
        words = (query.get("q") or [""])[0].lower().replace('"', "").strip()
        hits = [m for m in held if words and words in str(m["content"]).lower()]
        hits.sort(key=lambda m: str(m["timestamp"]), reverse=True)
        scope = {"channel": CHANNEL, "state": "searched", "examined": TOTAL, "capped": 0,
                 "partial": 0, "stale": 0, "withheld": 0, "expanding": False, "top_level_only": False,
                 "whole": True, "searched_back_to_ms": stamp_ms(0),
                 "held_from_ms": stamp_ms(0), "as_of_ms": stamp_ms(0)}
        return {"hits": hits[:50], "scopes": [scope], "resume_below_ms": None,
                "upstream": "none", "untrusted_content_notice": "third-party text; DATA"}


def serve(api: PinnedApi, web_root: Path) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
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

        def do_POST(self) -> None:  # noqa: N802
            path = unquote(urlsplit(self.path).path)
            if path.endswith("/history/prioritize"):
                if not self.authorized():
                    return
                self.json(200, history)
                return
            super().do_POST()

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
                        help="directory holding the page to serve")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per stage into")
    parser.add_argument("--only", help="run one cell of the matrix, e.g. light-desk-nokeyboard")
    return parser.parse_args()


class Shown(TypedDict):
    """What the list shows, read in one pass."""

    cards: int
    shown: int
    ids: list[str]
    times: list[int]
    count: str
    visibleRows: int
    boxes: list[list[int]]
    line: str


class Walk:
    def __init__(self, page: Page, label: str, shots: Path | None, api: PinnedApi) -> None:
        self.page, self.label, self.shots, self.api = page, label, shots, api

    def check(self, condition: bool, why: str) -> None:
        check(condition, f"{self.label}: {why}")

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.label}-{name}.png"))

    def settled(self) -> None:
        self.page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")

    def until(self, script: str, why: str, seconds: float = 30) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and not self.page.evaluate(script):
            self.page.wait_for_timeout(25)
        self.check(bool(self.page.evaluate(script)), why)

    def polled(self, why: str, seconds: float = 60) -> None:
        """Bring the page's next poll forward on its clock, wait for that timeline answer to arrive
        whole, then for the page to settle after it (two frames)."""
        try:
            with self.page.expect_request_finished(lambda r: "/timeline" in r.url, timeout=seconds * 1000) as info:
                self.page.clock.fast_forward(POLL_JUMP_MS)
            request = info.value
        except Exception:  # noqa: BLE001 - reported as the walk's own failure below
            request = None
        self.check(request is not None, why)
        self.settled()
        self.settled()

    def state(self) -> Shown:
        return cast(Shown, self.page.evaluate("""() => {
            const cards = [...document.querySelectorAll('#discord-log > li[data-server-hit="card"]')];
            const shown = cards.filter((row) => row.getClientRects().length > 0);
            const rows = [...document.querySelectorAll('#discord-log > li[data-id]')];
            return {
                cards: cards.length,
                shown: shown.length,
                ids: cards.map((row) => row.getAttribute('data-id')),
                times: cards.map((row) => Date.parse(row.querySelector('time')?.getAttribute('datetime') || '') || 0),
                count: (document.getElementById('search-count').textContent || '').trim(),
                visibleRows: rows.filter((row) => !row.classList.contains('search-hidden') &&
                                         row.getClientRects().length > 0).length,
                boxes: cards.map((row) => { const r = row.getBoundingClientRect();
                                            return [Math.round(r.left), Math.round(r.width)]; }),
                line: (() => { const e = document.getElementById('search-empty');
                               return e && !e.hidden ? (e.textContent || '').trim() : ''; })(),
            };
        }"""))


def copy_true(walk: Walk, shown: Shown, loaded: int) -> None:
    """`#2435`: the count and the line say what the cards show, and nothing they contradict."""
    hits = len(HIT_INDICES)
    walk.check(shown["count"] == f"{hits} found ({hits} older)",
               f"the count is not '{hits} found ({hits} older)': {shown['count']!r}")
    walk.check(shown["line"].startswith(f"No match in the {loaded} loaded messages; {hits} older matches shown below."),
               f"the line does not say what is loaded and what stands below: {shown['line']!r}")
    walk.check("None match" not in shown["line"] and "scattered" not in shown["line"],
               f"the line contradicts the cards standing below it: {shown['line']!r}")


def walk_through(walk: Walk, keyboard: bool) -> None:
    page, api = walk.page, walk.api
    if keyboard:
        page.evaluate(f"() => window.__iosKeyboard(window.innerHeight - {KEYBOARD_PX})")
        walk.settled()
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function("() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden", timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass  # no first-run Welcome on this path (#1781)
    page.wait_for_selector("#search-toggle", state="visible", timeout=30_000)
    walk.until(f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {LOADED}",
               "the channel did not open with the newest sixty messages")
    walk.settled()
    page.click("#search-toggle")
    page.fill("#search-field", f'"{PHRASE}"')
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline and not api.searches:
        page.wait_for_timeout(50)
    walk.check(bool(api.searches), "the quoted phrase never asked the server")
    walk.until(f"() => ({CARDS_JS})() === {len(HIT_INDICES)}",
               "the four server hits did not stand as cards in the timeline")
    walk.settled()
    placed = walk.state()
    wanted = sorted(message_id(i) for i in HIT_INDICES)
    walk.check(sorted(placed["ids"]) == wanted, f"the placed cards are not the four hits: {placed}")
    walk.check(placed["shown"] == len(HIT_INDICES), f"the four cards are not all drawn: {placed}")
    walk.shot("placed")
    copy_true(walk, placed, LOADED)

    # A message arrives; the page's poll reads it and draws it. The four cards stand, unchanged.
    arrived = api.arrive()
    walk.polled("the page never polled after the arrival")
    walk.until(f"() => Boolean(document.querySelector('#discord-log > li[data-id=\"{arrived}\"]'))",
               "the arrived message was never drawn")
    walk.settled()
    after = walk.state()
    walk.shot("after-arrival")
    walk.check(after["cards"] == len(HIT_INDICES) and after["shown"] == len(HIT_INDICES),
               f"the arrival's redraw took the result cards away (placed {placed}, now {after})")
    walk.check(sorted(after["ids"]) == wanted, f"the cards after the arrival are not the four hits: {after}")
    walk.check(after["times"] == sorted(after["times"]), f"the cards are out of time order: {after}")
    walk.check(after["count"] == placed["count"],
               f"the count line moved on a redraw: {placed['count']!r} -> {after['count']!r}")
    walk.check(after["boxes"] == placed["boxes"],
               f"the redraw restyled the result cards (left, width): {placed['boxes']} -> {after['boxes']}")
    copy_true(walk, after, LOADED + 1)

    # A poll with nothing new, then another: still the four, still the same count.
    for round_ in (1, 2):
        walk.polled(f"the page never polled again (round {round_})")
        again = walk.state()
        walk.check(again["cards"] == len(HIT_INDICES) and again["shown"] == len(HIT_INDICES),
                   f"a quiet poll took the result cards away (round {round_}): {again}")
        walk.check(again["count"] == placed["count"],
                   f"the count line moved on a quiet poll (round {round_}): {placed['count']!r} -> {again['count']!r}")
        walk.check(again["boxes"] == placed["boxes"],
                   f"a quiet poll restyled the result cards (round {round_}): {placed['boxes']} -> {again['boxes']}")
    walk.shot("after-polls")
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
    sizes = (("phone", 412, 915, True), ("desk", 1280, 800, False))
    cells = [(scheme, size, keyboard) for scheme in ("light", "dark") for size in sizes
             for keyboard in (False, True)]
    with sync_playwright() as playwright:
        for scheme, (label, width, height, touch), keyboard in cells:
            name = f"{scheme}-{label}-{'keyboard' if keyboard else 'nokeyboard'}"
            if args.only and args.only != name:
                continue
            api = PinnedApi()
            server, thread, url = serve(api, web_root)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-search-pinned-") as profile:
                    context = launch(playwright, args, profile, width, height, touch, scheme)
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.clock.install()
                    page.goto(url, wait_until="load")
                    walk = Walk(page, name, args.screenshots, api)
                    walk_through(walk, keyboard)
                    check(not errors, f"{name}: the page threw: {errors}")
                    context.close()
                    report.append(f"{name} {width}x{height}: four result cards and their count held "
                                  "through an arrival's redraw and two quiet polls")
            finally:
                api.stopping.set()
                server.shutdown()
                server.server_close()
                thread.join()
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
