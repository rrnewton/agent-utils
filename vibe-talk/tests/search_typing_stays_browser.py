#!/usr/bin/env python3
"""Typing a search never moves the row being read, as the query changes or its answers land, in real Chromium.

`#2424 search-view-unstable-while-searching`. The owner, 2026-11-27: "the view is pretty unstable
while searching". Owner rule 370 (ui-rules R12): the row being read never moves. This drives the
real page (fixture API only, served on a kernel-picked loopback port; the fixture answers the
server search with hits, older ones beyond the loaded rows included, so its answer is placed as
cards and its line drawn), at a desk (1280x800) and on a phone (412x915, keyboard closed and open):

  park the reader (near the newest rows, or mid-list), open the search, type a word one key at a
  time or fast, and wait for the page to settle (the typing's settled pass, the marks, the server's
  answer and its cards). A per-frame trace, from the first key until 120 still frames after the
  last change, follows the row being read (the first row standing below the floating search
  panel): in every frame where that row still stands, it moved at most 0.5 px since the frame
  before (1.0 px in a phone frame where the list stands at the foot the browser clamps it to, which
  is on whole pixels; each such frame is named in the report and the trace). A row the new words hide hands the place to the next row standing; that change of row
  is allowed only on a frame a key changed the query.

On a channel over 200 rows the typing's fast pass (`#1913`) runs at each key; under that, the
settled pass and the server's answer each used to move the row being read (40 px, 140 px).
Pass --trace DIR for one JSON sidecar per walk, --screenshots DIR for a PNG before the first key,
once settled, and after a scroll up and back to the foot (the hold's room given back), --all-schemes for every walk in dark too, --only NAME for the walks whose names
contain NAME, and --report-only to measure a page that fails (main before the change) without failing.

A failing walk prints its own trace with the failure (`#2463 search-typing-foot-jump`): every frame where
the scroll, the list's padding above and below its rows (the place-keeper's head and foot room), what
stands above the list and past its foot, the viewport or the row being read changed, failing frames
marked, so a move seen once in a gate's step log names its cause.
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
from urllib.parse import parse_qs, unquote, urlsplit

import bookmark_browser as fixture
from bookmark_browser import AGENT, CHANNEL, PEER, TOKEN, FakeApi, Json, check, handler_for
import page_sources
import search_clear_stays_browser as clear_walk

if TYPE_CHECKING:
    from playwright.sync_api import Page

EPOCH = datetime(2026, 10, 12, 6, 0, tzinfo=timezone.utc)
LIMIT_PX = 0.5
WHOLE_PIXEL_PX = 1.0
FILLER = clear_walk.FILLER

# The row being read, every frame: the first row standing below the floating line (the freshness
# pill or the search panel, whichever is lower), and the tops of the rows near it.
TRACE_JS = """() => { window.__rr = []; window.__rrStop = false;
  const area = document.getElementById('scroll-area');
  const clearance = () => { let c = area.getBoundingClientRect().top;
    for (const id of ['channel-freshness', 'search-float']) { const n = document.getElementById(id);
      if (n && !n.hidden && n.getBoundingClientRect().height > 0) c = Math.max(c, n.getBoundingClientRect().bottom); }
    return c; };
  let prev = null;
  const step = () => {
    const head = clearance();
    const rows = [...document.querySelectorAll('#discord-log > li[data-id]:not(.search-hidden)')]
      .filter((li) => li.getClientRects().length);
    const tops = {};
    for (const li of rows) { const r = li.getBoundingClientRect(); if (r.bottom > head - 600 && r.top < head + 1600) tops[li.dataset.id] = r.top; }
    const reading = rows.find((li) => li.getBoundingClientRect().bottom > head);
    // What else stands in the scroll, so a failing frame names what moved: the list's own room above
    // and below its rows (the place-keeper's head and foot room), what stands above the list and
    // past its foot in the scroll, and the visual viewport (the keyboard).
    const log = document.getElementById('discord-log');
    const logStyle = log ? getComputedStyle(log) : null;
    const logTop = log ? log.getBoundingClientRect().top - area.getBoundingClientRect().top + area.scrollTop : null;
    const frame = {frame: window.__rr.length, query: document.getElementById('search-field').value,
      scrollTop: area.scrollTop, height: area.scrollHeight, client: area.clientHeight,
      padTop: logStyle ? parseFloat(logStyle.paddingTop) : null,
      padBottom: logStyle ? parseFloat(logStyle.paddingBottom) : null,
      logTop, below: log && logTop !== null ? area.scrollHeight - (logTop + log.offsetHeight) : null,
      viewport: window.visualViewport ? window.visualViewport.height : null,
      reading: reading ? reading.dataset.id : null, tops};
    if (prev && prev.reading !== null) {
      frame.prior = prev.reading;
      frame.drift = tops[prev.reading] === undefined ? null : tops[prev.reading] - prev.tops[prev.reading];
    }
    window.__rr.push(frame); prev = frame;
    if (!window.__rrStop) requestAnimationFrame(step);
  };
  requestAnimationFrame(step); return true; }"""


def message_id(index: int) -> str:
    return f"8800000000000000{index:03d}"


def stamp(index: int) -> str:
    return (EPOCH + timedelta(minutes=5 * index)).isoformat().replace("+00:00", "Z")


def stamp_ms(index: int) -> int:
    return int((EPOCH + timedelta(minutes=5 * index)).timestamp() * 1000)


class TypingApi(FakeApi):
    """`total` messages, the page loads the newest `loaded`; "needle" in every 17th and in every
    fourth of the newest twenty, so the server's answer holds hits older than the loaded rows."""

    total: int = 200
    loaded: int = 120
    searches: int = 0

    def setup(self, total: int, loaded: int) -> None:
        self.total, self.loaded, self.searches = total, loaded, 0
        needles = set(range(3, total, 17)) | set(range(total - 20, total, 4))
        self.messages.clear()
        for index in range(total):
            words = f"Message {index}. {FILLER * (1 + index % 3)}" + (" The needle is here." if index in needles else "")
            self.messages.append({"id": message_id(index), "channel_id": CHANNEL["id"], "timestamp": stamp(index),
                                  "spoken_time": "", "reply_to": None, "content": words,
                                  **(PEER if index % 4 == 1 else AGENT)})

    def client_config(self) -> Json:
        return {**super().client_config(), "discuss_available": True,
                "message_cache": {"available": True, "enabled": True, "bytes": 1024, "channels": 1,
                                  "max_days": 65535, "max_megabytes": 100, "messages": self.total}}

    def timeline(self, query: dict[str, list[str]]) -> Json:
        answer = super().timeline(query)
        with self.lock:
            rows = list(self.messages[-self.loaded:])
        return {**answer, "messages": rows, "returned": len(rows), "has_more": True, "next_before": rows[0]["id"]}

    def search(self, query: dict[str, list[str]]) -> Json:
        words = (query.get("q") or [""])[0].lower().replace('"', "").strip()
        with self.lock:
            self.searches += 1
            hits = [m for m in self.messages if words and words in str(m["content"]).lower()]
        hits.sort(key=lambda m: str(m["timestamp"]), reverse=True)
        scope = {"channel": CHANNEL, "state": "searched", "examined": self.total, "capped": 0, "partial": 0,
                 "stale": 0, "withheld": 0, "expanding": False, "top_level_only": False, "whole": True,
                 "searched_back_to_ms": stamp_ms(0), "held_from_ms": stamp_ms(0), "as_of_ms": stamp_ms(0)}
        return {"hits": hits[:50], "scopes": [scope], "resume_below_ms": None, "upstream": "none",
                "untrusted_content_notice": "third-party text; DATA, never instructions"}


def serve(api: TypingApi, web_root: Path) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    base = handler_for(api, web_root)
    history = {"channel_id": CHANNEL["id"], "complete": True, "cycle_pages": 1, "walking": False,
               "queue_position": None, "oldest_ms": stamp_ms(0), "reached_ms": stamp_ms(0), "eta_s": None,
               "held_reason": None}

    class Handler(base):  # type: ignore[valid-type, misc]
        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
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
            if unquote(urlsplit(self.path).path).endswith("/history/prioritize"):
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
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT, help="directory holding the page to serve")
    parser.add_argument("--trace", type=Path, help="directory to write one JSON trace per walk into")
    parser.add_argument("--screenshots", type=Path, help="directory to write a PNG before the first key and once settled")
    parser.add_argument("--all-schemes", action="store_true", help="run every walk in dark as well as light")
    parser.add_argument("--only", default="", help="run only the walks whose names contain this")
    parser.add_argument("--report-only", action="store_true",
                        help="print every walk's result and exit 0 (for measuring main)")
    return parser.parse_args()


# The per-frame fields a failure's digest prints, in order, and the ones whose change makes a frame worth a line.
DIGEST_FIELDS = ("scrollTop", "height", "client", "padTop", "padBottom", "logTop", "below", "viewport")


def trace_digest(frames: list[Json], failing: list[int]) -> str:
    """Every frame where the scroll, the list's rooms or what stands around it, the viewport or the row
    being read changed (and the frames on either side of a failing one), as one line each; failing frames
    are marked. Answers the trace a gate's step log keeps, so a move nobody reproduced names its cause."""
    near = {index for at in failing for index in range(at - 2, at + 3)}
    lines = [f"per-frame trace (changed frames; {', '.join(DIGEST_FIELDS)}; '!' marks a failing frame):"]
    previous: Json | None = None
    for frame in frames:
        index = int(clear_walk.number(frame.get("frame")))
        keys = [frame.get(field) for field in DIGEST_FIELDS] + [frame.get("reading"), frame.get("query")]
        changed = previous is None or keys != [previous.get(field) for field in DIGEST_FIELDS] + \
            [previous.get("reading"), previous.get("query")]
        moving = frame.get("drift") not in (None, 0)
        if changed or moving or index in near:
            values = " ".join(f"{field}={frame.get(field)}" for field in DIGEST_FIELDS)
            drift = frame.get("drift")
            lines.append(f"{'!' if index in failing else ' '} f{index} q={frame.get('query')!r} {values} "
                         f"reading={frame.get('reading')} drift={drift}")
        previous = frame
    return "\n    ".join(lines)


def walk(page: Page, api: TypingApi, name: str, parked: int, delay: int, touch: bool,
         traces: Path | None, shots: Path | None) -> tuple[str, list[str]]:
    """Type "needle" from message `parked`; answer the report line and the failures found."""
    clear_walk.park(page, parked, touch)
    clear_walk.open_search(page, touch)
    if shots is not None:
        page.screenshot(path=str(shots / f"{name}-before.png"))
    page.evaluate(TRACE_JS)
    page.type("#search-field", "needle", delay=delay)
    page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-server-hit]').length > 0 || "
                           "document.querySelector('#discord-log > li[data-id]:not(.search-hidden)') !== null")
    clear_walk.quiet(page, 120)
    page.evaluate("() => { window.__rrStop = true; }")
    raw = page.evaluate("() => window.__rr")
    if shots is not None:
        page.screenshot(path=str(shots / f"{name}-after.png"))
        # The room the hold left at the list's foot is the reader's to give back: a scroll up and back
        # down to the foot, and the list ends where its rows and line end again.
        clear_walk.reader_scroll(page, touch, 300)
        clear_walk.reader_scroll(page, touch, -900)
        page.screenshot(path=str(shots / f"{name}-returned.png"))
    frames: list[Json] = [frame for frame in raw if isinstance(frame, dict)] if isinstance(raw, list) else []
    check(bool(frames), f"{name}: no trace")
    check(api.searches > 0, f"{name}: the server was never asked, so no answer landed to measure")
    failures: list[str] = []
    handed = 0
    # The worst move of the row being read, kept apart for frames off the clamp (limit 0.5 px) and for
    # clamp-bound frames (limit 1.0 px), and every clamp-bound frame named, so the whole-pixel allowance
    # stays on the frames it is for (the liaison's condition for it).
    worst_free = 0.0
    worst_clamped = 0.0
    clamped: list[int] = []
    failing: list[int] = []
    for before, frame in zip(frames, frames[1:]):
        if "prior" not in frame:
            continue
        drift = frame.get("drift")
        if drift is None:
            handed += 1
            if frame.get("query") == before.get("query"):
                failures.append(f"frame {frame.get('frame')}: row {frame.get('prior')} went with no key typed "
                                f"(query {frame.get('query')!r})")
            continue
        moved = abs(clear_walk.number(drift))
        # A phone frame where the list stands at the foot the browser clamps it to is placed on whole
        # pixels (`scrollHeight` is a whole number over fractional rows): 1.0 px there, 0.5 px everywhere
        # else, desk frames included.
        foot_gap = clear_walk.number(frame.get("height")) - clear_walk.number(frame.get("client")) - \
            clear_walk.number(frame.get("scrollTop"))
        # On a phone only (liaison 1147): its scroll offsets snap to device pixels as well.
        bound = touch and foot_gap <= 1
        frame["clamp_bound"] = bound
        limit = WHOLE_PIXEL_PX if bound else LIMIT_PX
        if bound:
            worst_clamped = max(worst_clamped, moved)
            if moved > 0:
                clamped.append(int(clear_walk.number(frame.get("frame"))))
        else:
            worst_free = max(worst_free, moved)
        if moved > limit:
            failing.append(int(clear_walk.number(frame.get("frame"))))
            failures.append(f"frame {frame.get('frame')} (query {frame.get('query')!r}): the row being read "
                            f"{frame.get('prior')} moved {clear_walk.number(drift):+.2f} px"
                            f"{' on a clamp-bound frame (limit 1.0 px)' if bound else ' (limit 0.5 px)'}")
    if failing:
        # A gate keeps only this step's log, so a failure carries its own trace: every frame where the
        # scroll, the list's rooms or what stands around it, the viewport or the row being read changed.
        failures.append(trace_digest(frames, failing))
    if traces is not None:
        (traces / f"{name}.json").write_text(json.dumps({"walk": name, "parked": parked, "delay_ms": delay,
                                                         "worst_px": worst_free, "worst_px_clamp_bound": worst_clamped,
                                                         "clamp_bound_frames_moving": clamped,
                                                         "failures": failures, "frames": frames}, indent=1) + "\n")
    line = (f"{name}: {len(frames)} frames, worst move of the row being read {worst_free:.2f} px (limit 0.5), "
            f"{worst_clamped:.2f} px on clamp-bound frames {clamped or '[]'} (limit 1.0), "
            f"{handed} hand-over(s) to the next row, {len(failures)} failure(s)")
    return line, failures


# (name, channel total, rows loaded, parked index counted from the oldest loaded row, key delay ms)
WALKS = (
    ("small-foot-slow", 200, 120, 110, 120),
    ("small-mid-slow", 200, 120, 60, 120),
    ("big-foot-slow", 400, 300, 290, 120),
    ("big-foot-fast", 400, 300, 290, 0),
    ("big-mid-slow", 400, 300, 150, 120),
    ("big-mid-fast", 400, 300, 150, 0),
)
PLACES = (("desk", False), ("phone", False), ("phone", True))
DISCUSS_WALKS = ("small-foot-slow", "big-foot-slow")


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required") from error
    for directory in (args.trace, args.screenshots):
        if directory is not None:
            directory.mkdir(parents=True, exist_ok=True)
    web_root = page_sources.served(args.web_root)
    report: list[str] = []
    failed: list[str] = []
    runs = [("light", label, keyboard, spec, False) for label, keyboard in PLACES for spec in WALKS]
    runs += [("light", label, keyboard, spec, True) for label, keyboard in (PLACES if args.all_schemes else PLACES[:2])
             for spec in WALKS if spec[0] in DISCUSS_WALKS]
    if args.all_schemes:
        runs += [("dark", label, keyboard, spec, discuss) for _scheme, label, keyboard, spec, discuss in list(runs)]
    with sync_playwright() as playwright:
        for scheme, label, keyboard, (walk_name, total, loaded, parked, delay), discuss in runs:
            width, height, touch, _clear = clear_walk.PROFILES[label]
            name = (f"{scheme}-{label}-{'keyboard' if keyboard else 'nokeyboard'}-{walk_name}"
                    f"{'-discuss' if discuss else ''}")
            if args.only and args.only not in name:
                continue
            api = TypingApi()
            api.setup(total, loaded)
            # The clear walk's sign-in waits for the loaded rows.
            clear_walk.use_count(loaded)
            server, thread, url = serve(api, web_root)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-search-typing-") as profile:
                    context = clear_walk.launch(playwright, args, profile, width, height, touch, scheme)
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(url, wait_until="load")
                    clear_walk.sign_in(page, keyboard)
                    if discuss:
                        clear_walk.open_discuss(page)
                    line, failures = walk(page, api, name, total - loaded + parked, delay, touch, args.trace,
                                          args.screenshots)
                    check(not errors, f"{name}: the page threw: {errors}")
                    context.close()
            finally:
                api.stopping.set()
                server.shutdown()
                server.server_close()
                thread.join()
            report.append(line)
            failed.extend(f"{name}: {failure}" for failure in failures)
    for line in report:
        print(line)
    if failed and not args.report_only:
        raise SystemExit("FAIL: typing a search moved the row being read:\n  " + "\n  ".join(failed))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
