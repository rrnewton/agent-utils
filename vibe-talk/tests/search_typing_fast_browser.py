#!/usr/bin/env python3
"""Typing in search stays off the layout path, and bucket chips follow the query (`#1913`).

`#1913 vibe-talk-search-typing-lag`. The owner (thread 224, 2026-11-19): "serious lag while
typing during search... Anything slow should be async and not slow my typing". This walk serves
the real page and a fake chat upstream (only the upstream is stubbed; the server search answers
through the real route) over a channel of 800+ messages with threads and links, and checks, at
phone and desktop sizes:

  typing "lantern" key by key (real `input` events dispatched on the field) does a bounded,
  O(1) number of synchronous layout reads (`getBoundingClientRect` calls) per keystroke — the
  bound does not grow with the 800 rows on screen, where the old per-keystroke full pass read
  one rectangle per row (800+) plus its seams and place-keeping; after the typing pauses the
  full result lands exactly ("4 of N loaded", the lantern rows shown);
  with the pinned bucket chips standing for "lantern" ("2 in main" / "5 in threads" — the
  fold counts loaded and fresh together, `#2200`), typing a new query retires those chips at
  once — the previous query's counts never stand for the new words — and after the debounced
  server answer the chips stand for the new query's own fold, summing to its count;
  `#2249`: while the list's head stays reserved over the retired chips (`data-bucket-head`,
  the answer pending), the "On this screen" signpost takes no layout box at the key or at any
  frame until the chips stand again (it used to come back over the reserved room and push the
  reader's rows +23 px for the debounce window); settled on a query with no hit anywhere (no
  chip, no reservation) the signpost shows;
  `#2256`: the chip panel's head room is decided at the key, in the same pass as the key's own
  filter: a mid-list reader and a reader at the foot keep the row being read at its top from the
  first key through the settled answer (the server line at the foot lands below the newest row),
  and words with no loaded match drop the room inside the input event, never later.

The gate is a count of synchronous work per input event, never a wall-clock budget (owner
rule 346); every wait is a condition wait. Pass --screenshots DIR to keep a PNG of each stage.
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
from urllib.parse import parse_qs, unquote, urlsplit

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, Json, check, handler_for, message
import page_sources
import search_unmatched_browser as unmatched
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import Page, Playwright

# Synchronous layout reads allowed inside one input event, however many rows stand on screen.
# The fast pass reads the search panel's own neighbourhood (the noted screen, the scroll
# anchor): tens, not the 800+ a per-row pass reads. Generous on purpose (rule 346).
MAX_RECT_READS_PER_KEY = 120

RECT_INSTALL_JS = """() => {
    window.__rectReads = 0;
    const orig = Element.prototype.getBoundingClientRect;
    Element.prototype.getBoundingClientRect = function () {
        window.__rectReads += 1;
        return orig.call(this);
    };
}"""

TYPE_KEYS_JS = """() => {
    const field = document.getElementById('search-field');
    const values = ['l', 'la', 'lan', 'lant', 'lante', 'lanter', 'lantern'];
    const deltas = [];
    for (const value of values) {
        const before = window.__rectReads;
        field.value = value;
        field.dispatchEvent(new Event('input', { bubbles: true }));
        deltas.push(window.__rectReads - before);
    }
    return deltas;
}"""

CHIP_JS = """(id) => {
    const chip = document.getElementById(id);
    if (!chip) return null;
    return { text: (chip.textContent || '').trim(), hidden: chip.hidden,
             pressed: chip.getAttribute('aria-pressed') };
}"""


class BigApi(unmatched.GapApi):
    """The lantern channel lengthened to 800 routine messages, every tenth holding a link."""

    def __init__(self) -> None:
        super().__init__()
        base = len(self.messages)
        for extra in range(800):
            words = (f"Routine lane note {extra}, filed and done. See https://example.test/{extra}."
                     if extra % 10 == 0 else f"Routine lane note {extra}, filed and done.")
            self.messages.append(message(base + extra, words))


def serve(api: BigApi, web_root: Path) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    base = handler_for(api)

    class Handler(base):  # type: ignore[valid-type, misc]
        def do_GET(self) -> None:  # noqa: N802
            parts = urlsplit(self.path)
            path = unquote(parts.path)
            if path.endswith("/search") or path.endswith("/history"):
                with api.lock:
                    api.requests.append(f"GET {self.path}")
                if path.endswith("/search"):
                    self.json(200, api.search(path, parse_qs(parts.query)))
                    return
                self.json(200, api.history_state())
                return
            super().do_GET()

        def do_POST(self) -> None:  # noqa: N802
            path = unquote(urlsplit(self.path).path)
            if path.endswith("/history/prioritize"):
                if not self.authorized():
                    return
                self.json(202, api.history_state())
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
    return parser.parse_args()


class Walk:
    def __init__(self, page: Page, label: str, shots: Path | None) -> None:
        self.page, self.label, self.shots = page, label, shots

    def check(self, condition: bool, why: str) -> None:
        check(condition, f"{self.label}: {why}")

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.label}-{name}.png"))

    def until(self, script: str, why: str, seconds: float = browser_waits.HANG_BACKSTOP_MS / 1000) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and not self.page.evaluate(script):
            self.page.wait_for_timeout(25)
        self.check(bool(self.page.evaluate(script)), why)


def walk_through(walk: Walk, total: int) -> None:
    page = walk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function(
            "() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden",
            timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass
    page.wait_for_selector("#search-toggle", state="visible", timeout=browser_waits.HANG_BACKSTOP_MS)
    walk.until(f"() => document.querySelectorAll('#discord-log > li[data-id]').length >= {total - 20}",
               "the big channel did not open with its messages")
    page.click("#search-toggle")
    page.wait_for_selector("#search-field", state="visible", timeout=browser_waits.HANG_BACKSTOP_MS)
    page.evaluate(RECT_INSTALL_JS)
    deltas = page.evaluate(TYPE_KEYS_JS)
    walk.check(all(isinstance(one, int) for one in deltas) and len(deltas) == 7,
               f"the typing probe did not run: {deltas}")
    walk.check(all(one <= MAX_RECT_READS_PER_KEY for one in deltas),
               f"a keystroke read layout {deltas} times over {total} rows "
               f"(bound {MAX_RECT_READS_PER_KEY} per key, independent of the row count)")
    # After the pause the full result lands exactly as the synchronous pass lands it today.
    walk.until(f"() => document.getElementById('search-count').textContent === '4 of {total} loaded'",
               "the settled count does not speak for the lantern search")
    walk.until("() => [...document.querySelectorAll('#discord-log > li[data-id]')]"
               ".filter((row) => !row.classList.contains('search-hidden') && !row.hasAttribute('data-server-hit'))"
               ".map((row) => row.getAttribute('data-id')).join(',') === '201,204,209,214'",
               "the settled rows are not the lantern four")
    walk.shot("settled-lantern")
    # `#1984 vibe-talk-search-typing-changed-only`: after settling, refining keys whose hit set
    # barely moves write the hidden class only on changed rows (event counts, never wall time):
    # no fetch, no storage write and no settled (full) pass lands between keys typed at a steady
    # pace, and after the pause the marks are the settled query's.
    walk.check(page.evaluate("() => document.querySelectorAll('#discord-log .search-hit').length > 0"),
               "the settled lantern hits carry no marks")
    # Fully settled includes the server's answer (the bucket chips), so no ask for the old
    # query is still in flight to be counted against a refining key below.
    walk.until("() => { const row = document.getElementById('search-bucket-row');"
               " return row && !row.hidden && !document.getElementById('search-bucket-main').hidden; }",
               "the bucket chips never appeared for lantern")
    # `#2200`: the count and the chips are the fold — when the answer lands
    # they move from the loaded-only numbers (4; 1 in main, 3 in threads)
    # to the fold's (7; 2 and 5). Refining below starts from the settled
    # fold, so the answer must have landed before the first refining key.
    walk.until("() => document.getElementById('server-line').textContent === 'Server search details'",
               "the server's answer never settled before refining")
    walk.until(f"() => document.getElementById('search-count').textContent === '7 found (4 loaded, 3 older)'",
               "the settled fold count does not speak for the lantern search")
    page.evaluate("""() => {
        window.__keyCounts = { cls: 0, fetch: 0, lsSet: 0 };
        const log = document.getElementById('discord-log');
        new MutationObserver((ms) => { for (const m of ms) if (m.attributeName === 'class') window.__keyCounts.cls += 1; })
            .observe(log, { subtree: true, attributes: true, attributeFilter: ['class'] });
        const of = window.fetch; window.fetch = (...a) => { const u = String(a[0] && a[0].url || a[0]); if (u.includes('/search')) { window.__keyCounts.fetch += 1; (window.__keyCounts.urls = window.__keyCounts.urls || []).push(u.slice(-60)); } return of(...a); };
        const proto = Object.getPrototypeOf(localStorage); const os = proto.setItem;
        proto.setItem = function (k, v) { window.__keyCounts.lsSet += 1; return os.call(this, k, v); };
    }""")
    # Keys dispatched at a steady pace ON THE PAGE'S CLOCK (#2177): the page clock is
    # frozen and advanced 80 ms per key (about two frames plus the pace), so the pace
    # the page sees is the pace asserted no matter how starved the host is. Before
    # this, two real frames under load could take longer than the product's settle
    # pause, and the settled ask legitimately fired mid-burst on slow hosts.
    page.clock.install()
    deltas = []
    for value in ["lanter", "lantern", "lanter", "lantern"]:
        page.evaluate("""() => {
            window.__keyCounts.cls = 0; window.__keyCounts.fetch = 0; window.__keyCounts.lsSet = 0; window.__keyCounts.urls = [];
            const field = document.getElementById('search-field');
            field.value = %r; field.dispatchEvent(new Event('input', { bubbles: true }));
        }""" % value)
        page.clock.fast_forward(80)
        deltas.append(page.evaluate("() => ({ ...window.__keyCounts, count: document.getElementById('search-count').textContent })"))
    page.clock.fast_forward(10_000)  # let the settled pass land for the checks below
    for delta in deltas:
        walk.check(delta["cls"] <= max(8, total // 20),
                   f"a refining keystroke wrote {delta['cls']} row classes over {total} rows (changed-only bound {max(8, total // 20)})")
        walk.check(delta["fetch"] == 0 and delta["lsSet"] == 0,
                   f"a keystroke reached the server search or storage: {delta}")
        # No count-stability check between keys: since `#2200` the count is
        # the fold, and it legitimately moves when a server answer lands or
        # the fresh set resets for a refined query — the owner-approved
        # behaviour the old two-computation count never had. What a key may
        # not do is the event work asserted above (row classes, a fetch, a
        # storage write); the count is checked settled, before and after.
    walk.until("() => document.querySelectorAll('#discord-log .search-hit').length > 0",
               "the settled marks did not return after refining and retyping")
    # Backspacing the query away: every hit stands again and no mark outlives its words.
    page.evaluate("""() => { const f = document.getElementById('search-field'); f.value = ''; f.dispatchEvent(new Event('input', { bubbles: true })); }""")
    page.clock.fast_forward(10_000)  # the cleared query settles on the page clock too
    walk.until("() => document.getElementById('search-count').textContent === ''",
               "the cleared search left its settled count standing")
    walk.check(page.evaluate("() => document.querySelectorAll('#discord-log .search-hit').length === 0"),
               "marks outlived the cleared query")
    walk.check(page.evaluate(f"() => document.querySelectorAll('#discord-log > li[data-id]:not(.search-hidden)').length >= {total - 20}"),
               "clearing the search did not stand the rows back up")
    # Typing it again: the same four stand, marked, once the typing pauses.
    for value in ("l", "la", "lan", "lant", "lante", "lanter", "lantern"):
        page.evaluate("""(value) => { const f = document.getElementById('search-field'); f.value = value; f.dispatchEvent(new Event('input', { bubbles: true })); }""", value)
        page.wait_for_timeout(30)
    walk.until(f"() => document.getElementById('search-count').textContent === '7 found (4 loaded, 3 older)'",
               "the retyped lantern search did not settle to the same count")
    walk.until("() => document.querySelectorAll('#discord-log .search-hit').length > 0",
               "the retyped hits were not re-marked after the pause")
    walk.shot("retyped-lantern")
    # The pinned bucket chips for this query, from the server's debounced answer.
    walk.until("() => { const row = document.getElementById('search-bucket-row');"
               " return row && !row.hidden && !document.getElementById('search-bucket-main').hidden; }",
               "the bucket chips never appeared for lantern")
    main_chip = page.evaluate(CHIP_JS, "search-bucket-main")
    threads_chip = page.evaluate(CHIP_JS, "search-bucket-threads")
    walk.check(main_chip is not None and main_chip["text"] == "2 in main",
               f"the main chip says {main_chip}")
    walk.check(threads_chip is not None and threads_chip["text"] == "5 in threads",
               f"the threads chip says {threads_chip}")
    walk.shot("chips-lantern")
    # A new query: the standing chips retire inside the input event, before any answer lands.
    retired = page.evaluate("""() => {
        const field = document.getElementById('search-field');
        field.value = 'routine';
        field.dispatchEvent(new Event('input', { bubbles: true }));
        const row = document.getElementById('search-bucket-row');
        const mainChip = document.getElementById('search-bucket-main');
        const screenMain = document.getElementById('screen-main');
        const sep = document.getElementById('search-screen-sep');
        // `#2249`: every frame from here until the chips stand again, note any frame where
        // the signpost has a layout box while the head is reserved.
        window.__signpost = { frames: 0, shownWhileReserved: 0, running: true };
        const sample = () => {
            if (!window.__signpost.running) return;
            window.__signpost.frames += 1;
            if (screenMain.hasAttribute('data-bucket-head') && sep.getClientRects().length > 0) {
                window.__signpost.shownWhileReserved += 1;
            }
            requestAnimationFrame(sample);
        };
        requestAnimationFrame(sample);
        return { rowHidden: row ? row.hidden : null, chipHidden: mainChip ? mainChip.hidden : null,
                 chipText: mainChip ? (mainChip.textContent || '').trim() : null,
                 bucketHead: screenMain.hasAttribute('data-bucket-head'),
                 signpostBoxes: sep.getClientRects().length };
    }""")
    walk.check(retired["rowHidden"] is True and retired["chipHidden"] is True,
               f"the lantern chips still stand for 'routine': {retired}")
    walk.check(retired["chipText"] in ("", "2 in main"),
               f"the retired chip still speaks with fresh words: {retired}")
    # `#2249`: the head stays reserved over the retired chips (the answer pending), and the
    # signpost stays down with it, so nothing above the reader's rows grows at the key.
    walk.check(retired["bucketHead"] is True and retired["signpostBoxes"] == 0,
               f"at the key the 'On this screen' signpost came back over the reserved head: {retired}")
    # Settled on the new query: its count, and the chips standing for its own
    # fold (`#2200` — loaded matches count, so the row stands and sums to X).
    walk.until("() => document.getElementById('search-count').textContent.includes('of')"
               " && !document.getElementById('search-count').textContent.startsWith('7 found')",
               "the settled count does not speak for the routine search")
    walk.until("() => { const row = document.getElementById('search-bucket-row');"
               " return row && !row.hidden && !document.getElementById('search-bucket-main').hidden; }",
               "the bucket chips never stood for the routine search")
    summed = page.evaluate("""() => {
      const n = (id) => Number((document.getElementById(id).textContent || '').match(/^\\d+/)[0]);
      const x = Number((document.getElementById('search-count').textContent || '').match(/^\\d+/)[0]);
      return x === n('search-bucket-main') + n('search-bucket-threads') + n('search-bucket-transcripts');
    }""")
    walk.check(summed, "the routine chips do not sum to the count")
    signpost = page.evaluate("() => { window.__signpost.running = false; return window.__signpost; }")
    walk.check(signpost["frames"] > 0 and signpost["shownWhileReserved"] == 0,
               f"the signpost stood over the reserved head between the key and the chips: {signpost}")
    walk.shot("chips-retired")
    # `#2249`: settled on a query with no hit anywhere, no chip stands and the reservation is
    # released, and the signpost shows exactly as before.
    page.evaluate("""() => { const f = document.getElementById('search-field'); f.value = 'zzqv';
        f.dispatchEvent(new Event('input', { bubbles: true })); }""")
    walk.until("() => !document.getElementById('screen-main').hasAttribute('data-bucket-head')"
               " && document.getElementById('search-bucket-row').hidden"
               " && document.getElementById('search-count').textContent.startsWith('0 of')"
               " && /No more hits|Server search details/.test(document.getElementById('server-line').textContent)",
               "the search never settled with no chip for a query with no hit")
    shown = page.evaluate("""() => { const sep = document.getElementById('search-screen-sep');
        return { boxes: sep.getClientRects().length, text: (sep.textContent || '').trim() }; }""")
    walk.check(shown["boxes"] > 0 and shown["text"] == "On this screen",
               f"settled with no chip the 'On this screen' signpost does not show: {shown}")
    walk.shot("settled-no-chip")
    head_room_walk(walk)


# `#2256`: one key into the field, then every frame until the answer has settled: the reading
# row's top against its top at the key, and whether the head room stood at each frame.
HEAD_KEY_JS = """async ([value, rowId]) => {
    const area = document.getElementById('scroll-area');
    const main = document.getElementById('screen-main');
    const field = document.getElementById('search-field');
    field.value = value;
    field.dispatchEvent(new Event('input', { bubbles: true }));
    const row = rowId === null ? null : document.querySelector(`#discord-log > li[data-id="${rowId}"]`);
    const top = () => row && row.getClientRects().length ? row.getBoundingClientRect().top : null;
    const atKey = { top: top(), head: main.hasAttribute('data-bucket-head') };
    const settled = () => /Server search details|No more hits/.test(document.getElementById('server-line').textContent)
        && (value === 'zzqv' ? document.getElementById('search-count').textContent.startsWith('0 of')
            : !document.getElementById('search-bucket-row').hidden);
    let frames = 0; let moved = 0; let worst = 0; let headFlips = 0; let after = 0;
    while (frames < 6000 && after < 10) {
        await new Promise((resolve) => requestAnimationFrame(resolve));
        frames += 1;
        const now = top();
        if (atKey.top !== null && now !== null && Math.abs(now - atKey.top) > 1) { moved += 1; worst = Math.max(worst, Math.abs(now - atKey.top)); }
        if (main.hasAttribute('data-bucket-head') !== atKey.head) headFlips += 1;
        if (settled()) after += 1;
    }
    return { atKey, frames, moved, worst: Math.round(worst * 10) / 10, headFlips, settled: settled(),
             scrollTop: area.scrollTop };
}"""

PARK_JS = """(where) => {
    const area = document.getElementById('scroll-area');
    const rows = [...document.querySelectorAll('#discord-log > li[data-id]')].filter((row) => row.getClientRects().length);
    if (where === 'foot') {
        area.scrollTop = area.scrollHeight;
        const box = area.getBoundingClientRect();
        const last = [...rows].reverse().find((row) => row.getBoundingClientRect().bottom <= box.bottom + 1);
        return last ? last.getAttribute('data-id') : null;
    }
    const row = rows.find((one) => (one.textContent || '').includes('Routine lane note 400,'));
    const box = row.getBoundingClientRect(); const view = area.getBoundingClientRect();
    area.scrollTop += box.top - (view.top + view.height / 2);
    return row.getAttribute('data-id');
}"""


def head_room_walk(walk: Walk) -> None:
    """`#2256 vibe-talk-search-head-room-no-shift`: the chip panel's head room is decided at the
    key, in the same pass as the key's own filter, so neither the answer landing nor the search's
    foot line moves the row being read.

    Mid-list (the reading row matches, so it keeps its top): the first key of a search reserves the
    room inside the input event, and the row keeps its top from the key through the settled answer.
    At the foot (the newest row matches): the same, and the server's line appearing below the newest
    row lands past its bottom edge instead of taking the reader to the list's foot. Release (the new
    words match nothing loaded): the room goes inside the input event, with the rows the key took
    away, and no frame after it adds or drops it again.
    """
    page = walk.page

    def clear() -> None:
        page.evaluate("""() => { const f = document.getElementById('search-field'); f.value = '';
            f.dispatchEvent(new Event('input', { bubbles: true })); }""")
        walk.until("() => document.getElementById('search-count').textContent === ''"
                   " && !document.getElementById('screen-main').hasAttribute('data-bucket-head')",
                   "the cleared search kept its count or its head room")

    def first_key(where: str) -> None:
        clear()
        row_id = page.evaluate(PARK_JS, where)
        walk.check(row_id is not None, f"no {where} row to read")
        page.wait_for_function("() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r(true))))")
        seen = page.evaluate(HEAD_KEY_JS, ["routine", row_id])
        walk.check(seen["atKey"]["head"] is True,
                   f"{where}: the first key did not reserve the head room inside the input event: {seen}")
        walk.check(seen["settled"] and seen["moved"] == 0 and seen["headFlips"] == 0,
                   f"{where}: the reading row moved after the key ({seen['moved']} frames, up to {seen['worst']} px)"
                   f" or the head room flipped ({seen['headFlips']} frames) before the answer settled: {seen}")

    # Mid-list, the reading row a match: it keeps its top.
    first_key("mid")
    walk.shot("head-room-mid")
    # At the foot, the newest row a match: it keeps its top; the server line lands below it.
    first_key("foot")
    walk.shot("head-room-foot")
    # Release: words with no loaded match drop the room in the input event, once.
    seen = page.evaluate(HEAD_KEY_JS, ["zzqv", None])
    walk.check(seen["atKey"]["head"] is False and seen["headFlips"] == 0 and seen["settled"],
               f"release: the head room did not go with the key, or came back before the settle: {seen}")


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required") from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    web_root = page_sources.served(args.web_root)
    total = 800 + 16
    runs = (
        ("dark-phone", 412, 915, True, "dark"),
        ("light-desk", 1280, 800, False, "light"),
    )
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch, scheme in runs:
            api = BigApi()
            server, thread, url = serve(api, web_root)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-typing-fast-") as profile:
                    context = playwright.chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable,
                        timezone_id="America/New_York",
                        viewport={"width": width, "height": height}, color_scheme=scheme,  # type: ignore[arg-type]
                        device_scale_factor=2.625 if touch else 1, is_mobile=touch, has_touch=touch)
                    browser_waits.hot_host_timeouts(context)
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(url, wait_until="load")
                    walk = Walk(page, label, args.screenshots)
                    walk_through(walk, total)
                    check(not errors, f"{label}: the page threw: {errors}")
                    context.close()
                    report.append(f"{label} {width}x{height}: typing bounded per key, chips follow the query")
            finally:
                server.shutdown()
                server.server_close()
                thread.join()
    for line in report:
        print(line)
    print("search typing fast browser walk: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
