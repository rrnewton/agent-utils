#!/usr/bin/env python3
"""Typing in a composer does constant work per keystroke, however long the channel (`#2429`).

`#2429 vibe-talk-typing-never-lags`. The owner, 2026-11-27: "the thing I hate most about this
lagginess is when the TYPING is actually laggy within the text entry box". The owner's traces put it
in selectionchange: the caret moving in a box is a collapsed selection, and the glossary relinked the
WHOLE list at each one, a walk over every row's text against every term. Its mutation observer did
the same after any change inside the list, the reply box's own included.

This walk serves the real page and a fake API: about 1,200 rows (over 100k px, windowed off as the
owner's lists are), the glossary on with terms the rows contain, so the lists are linked. Then 100
keystrokes in the channel composer and 100 in the reply box open in the stream, at 1280x800 and at
412x915 by touch. Counted, never timed, per keystroke, each counted through its frame:

  the text nodes the page walks (document.createTreeWalker), which must be none; the layout reads
  (getBoundingClientRect) and the elements the page queries the list for (querySelectorAll results),
  which must stay a small constant, not grow with the rows.

The script time per keystroke (p50, p95 and max, from the browser's own counters) is printed as a
report, never asserted: a hot host must not red this step on a clock. Pass --web-root DIR to run it
against another copy of the page (main before the change walks every row's text at each key).
"""

from __future__ import annotations

import argparse
import json
import os
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import unquote, urlsplit

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, Json, check, handler_for, message
import browser_scratch
import browser_waits
import page_sources
import search_typing_fast_browser as big

if TYPE_CHECKING:
    from playwright.sync_api import CDPSession, Page

WAIT_MS = int(browser_waits.HANG_BACKSTOP_MS)
EXTRA_ROWS = 1200
KEYS = 100
# Per keystroke, through its frame: what a box's own work can need, far below one per row of the
# 1,200. A whole-list pass reads or queries every row; a box reads a handful (a minute's tick of the
# rows' times landing inside a key's frame can add a few dozen, never a row's worth each).
MAX_RECT_READS = 300
MAX_QUERIED = 300
TERMS = [{"term": term, "definition": f"what {term} means", "source": None} for term in
         ("lane", "routine", "filed", "clause", "body", "note", "lantern", "gate", "deploy", "review", "shard", "queue")]

COUNTERS_JS = """(() => {
  const counts = {walked: 0, rects: 0, queried: 0};
  Object.defineProperty(window, '__typingWork', {value: counts});
  const walker = Document.prototype.createTreeWalker;
  Document.prototype.createTreeWalker = function (...args) {
    const made = walker.apply(this, args);
    const next = made.nextNode.bind(made);
    made.nextNode = () => { const node = next(); if (node) counts.walked += 1; return node; };
    return made;
  };
  const rect = Element.prototype.getBoundingClientRect;
  Element.prototype.getBoundingClientRect = function () { counts.rects += 1; return rect.call(this); };
  for (const proto of [Document.prototype, Element.prototype]) {
    const all = proto.querySelectorAll;
    proto.querySelectorAll = function (selector) { const found = all.call(this, selector); counts.queried += found.length; return found; };
  }
})();"""


class Channel(big.BigApi):
    """The lantern channel lengthened past 100k px of rows, with the glossary on."""

    def __init__(self) -> None:
        super(big.BigApi, self).__init__()
        base = len(self.messages)
        for extra in range(EXTRA_ROWS):
            self.messages.append(message(base + extra, f"Routine lane note {extra}, filed and done. "
                                         "A second clause so the row has some body to it."))

    def client_config(self, scope: str) -> Json:
        # No poll while the keys are counted: a re-read that redraws rows relinks them, rightly, and
        # that is the read's work, not a keystroke's.
        return {**super().client_config(scope), "glossary_available": True, "live_poll_seconds": 3600}


def serve(api: Channel) -> ThreadingHTTPServer:
    base = handler_for(api)

    class Handler(base):  # type: ignore[valid-type, misc]
        def do_GET(self) -> None:  # noqa: N802
            if unquote(urlsplit(self.path).path) == "/api/v1/glossary/terms":
                self.json(200, {"terms": TERMS})
                return
            super().do_GET()

    server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def frames(page: Page) -> None:
    """Two frames and a task: the key's selectionchange and the frame after it have run."""
    page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(done, 0))))")


def percentile(values: list[float], share: float) -> float:
    ordered = sorted(values)
    return ordered[min(len(ordered) - 1, int(round(share * (len(ordered) - 1))))]


def script_ms(cdp: CDPSession) -> float:
    metrics = {item["name"]: item["value"] for item in cdp.send("Performance.getMetrics")["metrics"]}
    return float(metrics.get("ScriptDuration", 0.0)) * 1000


def type_in(page: Page, cdp: CDPSession, selector: str, label: str) -> str:
    page.click(selector)
    frames(page)
    walked, rects, queried, script = [], [], [], []
    for index in range(KEYS):
        before = page.evaluate("() => ({...window.__typingWork})")
        started = script_ms(cdp)
        page.keyboard.type("abcdefghij"[index % 10])
        frames(page)
        script.append(script_ms(cdp) - started)
        after = page.evaluate("() => ({...window.__typingWork})")
        walked.append(after["walked"] - before["walked"])
        rects.append(after["rects"] - before["rects"])
        queried.append(after["queried"] - before["queried"])
    check(max(walked) == 0, f"{label}: a keystroke walked {max(walked)} text nodes (the whole list's text, at each key)")
    check(max(rects) <= MAX_RECT_READS, f"{label}: a keystroke read {max(rects)} element boxes (over {MAX_RECT_READS})")
    check(max(queried) <= MAX_QUERIED, f"{label}: a keystroke queried {max(queried)} elements (over {MAX_QUERIED})")
    return (f"{label}: {KEYS} keys, text nodes walked 0, box reads max {max(rects)}, elements queried max {max(queried)}; "
            f"script ms per key (report) p50 {percentile(script, .5):.2f} p95 {percentile(script, .95):.2f} max {max(script):.2f}")


def walk(pw: object, args: argparse.Namespace, work: Path, label: str, width: int, height: int, touch: bool) -> list[str]:
    api = Channel()
    server = serve(api)
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
            page.add_init_script(COUNTERS_JS)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
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
            cdp = context.new_cdp_session(page)
            cdp.send("Performance.enable")
            report = []
            # The caret's first move asks for the terms; the lists are linked once they come.
            page.click("#channel-compose-text")
            page.wait_for_function("() => document.querySelectorAll('#discord-log .glossary-term').length > 0", timeout=WAIT_MS)
            frames(page)
            report.append(type_in(page, cdp, "#channel-compose-text", f"{label} channel composer"))
            page.evaluate("() => { const area = document.getElementById('scroll-area'); area.scrollTop = area.scrollHeight / 2; }")
            frames(page)
            target = page.evaluate("""() => { const middle = innerHeight / 2;
              const row = [...document.querySelectorAll('#discord-log > li[data-id]')]
                .find((li) => { const r = li.getBoundingClientRect(); return r.top < middle && r.bottom > middle; });
              return row ? row.dataset.id : null; }""")
            check(target is not None, f"{label}: no row stood mid-screen to answer")
            page.evaluate(f"() => document.querySelector(\"#discord-log > li[data-id='{target}'] .reply-button\").click()")
            page.wait_for_function("() => { const box = document.getElementById('column-reply'); return !!box && !box.hidden; }",
                                   timeout=WAIT_MS)
            frames(page)
            report.append(type_in(page, cdp, "#reply-text", f"{label} reply box"))
            return report
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
    with browser_scratch.run_scratch("typing-lag-") as work:
        fixture.WEB_ROOT = page_sources.served(args.web_root)
        with sync_playwright() as pw:
            for label, width, height, touch in (("desk", 1280, 800, False), ("phone", 412, 915, True)):
                report += walk(pw, args, work, label, width, height, touch)
    for line in report:
        print(line)
    print("typing lag browser walk: ok")
    print(json.dumps({"cases": len(report)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
