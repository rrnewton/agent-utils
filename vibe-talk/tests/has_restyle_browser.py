#!/usr/bin/env python3
"""A small show or hide restyles only its neighbourhood, never the whole document (`#2283`).

`#2283 vibe-talk-root-has-rules`. The page's stylesheets held 115 rules with `:has()`, many
anchored at the root, the body or the main frame with an argument looking anywhere below the
anchor: the browser could only check such a rule by searching the anchor's whole subtree, so
showing or hiding one small control — which the page does on its own while the reader types
and scrolls — made it search the whole document again. Measured on the deployed build then:
one hidden toggle plus one forced style read cost 21 ms unthrottled over 39,863 elements,
0.0-0.2 ms with every `:has()` rule disabled, and about 70% of the worst remaining frame
after `#2277` was such recalcs touching only 16-45 elements apiece. The root-anchored rules
now read `data-*` flags their one owner writes beside the element's own `hidden` (C1-C3), and
only local direct-child `:has()` remains (the page's style tests refuse a new root one).

Here, with 260 rows in Main at phone and desk sizes, counted from the trace's style-recalc
element counts and never wall time (owner rule 346): a hidden toggle of each control the
page toggles while typing and scrolling restyles at most `WRITE_CEILING` elements, and the
page's own passes (typing in the box and in search, scrolls at and from the foot) never
restyle the whole document. The trace, the fixture and the reader are
geometry_restyle_browser.py's and scroll_stability_browser.py's. Pass --web-root DIR to run
against another copy of the page (a page from before the flags fails the probes).
"""

from __future__ import annotations

import argparse
import json
import os
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import browser_waits
import geometry_restyle_browser as geometry
import offline_cache_browser as fixture
import page_sources
import scroll_stability_browser as stability
from geometry_restyle_browser import LongApi, Trace
from scroll_stability_browser import READS_JS, Reader, handler, open_main

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

SIZES = geometry.SIZES
STATE_S = geometry.STATE_S
# The most elements one hidden toggle may restyle: geometry_restyle_browser.py's ceiling for
# one geometry write (33 measured there, 64 allowed), against thousands for a whole document.
WRITE_CEILING = geometry.WRITE_CEILING
WHOLE_SHARE = geometry.WHOLE_SHARE

# Each control the page itself shows or hides while the reader types and scrolls (`#2283`'s
# acceptance): the keyboard's corner Send, the action menu (its width measure), the error
# banner, the jump chips, the Labels button, a bucket chip, the freshness pill, the
# page-update chip, the Undo chip, Hang up, Stop, the Documents filter bar.
PROBES = (
    "keyboard-send", "action-menu", "error-wrap", "jump-newest", "jump-bookmark",
    "labels-button", "search-bucket-main", "channel-freshness", "page-update",
    "undo-dismiss", "hang-up", "read-aloud-stop", "docs-filter-bar",
)

TOGGLE_JS = """([id, mark]) => {
    const node = document.getElementById(id);
    if (!node) return null;
    const read = () => getComputedStyle(document.getElementById('channel-compose-text')).color + document.body.offsetHeight;
    read();
    console.timeStamp(mark + '-start');
    node.hidden = !node.hidden;
    read();
    console.timeStamp(mark + '-end');
    node.hidden = !node.hidden;
    read();
    return document.getElementsByTagName('*').length;
}"""

CENSUS_JS = """() => ({
    elements: document.getElementsByTagName('*').length,
    rowCount: document.querySelectorAll('#discord-log > li[data-id]').length,
})"""

PASS_JS = """async ([mark, act]) => {
    const frame = () => new Promise((done) => requestAnimationFrame(() => done()));
    const area = document.getElementById('scroll-area');
    console.timeStamp(mark + '-start');
    if (act === 'foot') { area.scrollTop = area.scrollHeight; }
    else if (act === 'up') { area.scrollTop = Math.max(0, area.scrollTop - 1500); }
    for (let i = 0; i < 4; i++) await frame();
    await new Promise((done) => setTimeout(done, 0));
    await frame();
    console.timeStamp(mark + '-end');
    return document.getElementsByTagName('*').length;
}"""


def probe_walk(reader: Reader, trace: Trace) -> None:
    """One hidden toggle of each control, counted: a handful of elements, never the document."""
    page = reader.page
    census = page.evaluate(CENSUS_JS)
    assert isinstance(census, dict)
    reader.check(int(census["rowCount"]) >= 200, f"Main holds {census['rowCount']} rows, not a long list")
    trace.start()
    marks: list[tuple[str, str]] = []
    elements = int(census["elements"])
    for probe in PROBES:
        mark = f"t{len(marks)}"
        seen = page.evaluate(TOGGLE_JS, [probe, mark])
        if seen is None:
            continue
        elements = int(seen)
        marks.append((mark, probe))
    trace.stop(reader)
    reader.check(len(marks) >= 8, f"only {len(marks)} of the page's toggles were on the page to probe")
    worst = 0
    for mark, probe in marks:
        restyled = sum(trace.recalcs(mark))
        worst = max(worst, restyled)
        reader.check(restyled <= WRITE_CEILING,
                     f"one hidden toggle of #{probe} restyled {restyled} elements (ceiling {WRITE_CEILING}, "
                     f"the page holds {elements})")
    reader.notes.append(f"{len(marks)} hidden toggles restyled at most {worst} elements each "
                        f"of {elements} ({census['rowCount']} rows)")


def passes_walk(reader: Reader, trace: Trace) -> None:
    """The page's own typing and scrolling: no pass restyles the whole document."""
    page = reader.page
    # Typing in the box and in search, as the reader does (the search open and its first
    # keys churn the labels button, the bucket row and the freshness pill).
    trace.start()
    elements = 0
    acts = ("foot", "up", "foot")
    for index, act in enumerate(acts):
        elements = int(page.evaluate(PASS_JS, [f"p{index}", act]))
    page.locator("#channel-compose-text").click()
    console_mark = "type"
    page.evaluate(f"() => console.timeStamp('{console_mark}-start')")
    page.keyboard.type("hi there ", delay=20)
    reader.wait_for("() => document.getElementById('channel-compose-text').value.includes('hi there')",
                    "the typing never landed in the box", STATE_S)
    page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")
    page.evaluate(f"() => console.timeStamp('{console_mark}-end')")
    search_mark = "search"
    page.evaluate(f"() => console.timeStamp('{search_mark}-start')")
    page.locator("#search-toggle").click()
    reader.wait_for("() => !document.getElementById('search-field').hidden",
                    "the search field never opened", STATE_S)
    page.keyboard.type("kestrel", delay=20)
    reader.wait_for("() => document.getElementById('search-field').value.includes('kestrel')",
                    "the typing never landed in the search field", STATE_S)
    page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")
    page.evaluate(f"() => console.timeStamp('{search_mark}-end')")
    trace.stop(reader)
    for index, act in enumerate(acts):
        recalcs = trace.recalcs(f"p{index}")
        whole = [count for count in recalcs if count >= elements * WHOLE_SHARE]
        reader.check(not whole, f"the page's own scroll pass '{act}' restyled the whole document "
                                f"{len(whole)} times ({whole} of {elements} elements)")
    for mark, what in ((console_mark, "typing in the box"), (search_mark, "opening search and typing")):
        recalcs = trace.recalcs(mark)
        whole = [count for count in recalcs if count >= elements * WHOLE_SHARE]
        reader.check(not whole, f"{what} restyled the whole document {len(whole)} times "
                                f"({whole} of {elements} elements)")
    reader.notes.append("scroll passes at and from the foot, typing in the box and in search: "
                        "no whole-document restyle")


def walk(chromium: "BrowserType", args: argparse.Namespace, label: str, width: int, height: int, touch: bool) -> str:
    stability.FLAT_PAGE = 1000
    api = LongApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-has-") as profile:
            if touch:
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="dark",
                    device_scale_factor=2.625, is_mobile=True, has_touch=True)
            else:
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="dark")
            browser_waits.hot_host_timeouts(context)
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            reader = Reader(page, api, None, label, touch)
            reader.width = width  # type: ignore[attr-defined]
            page.add_init_script(READS_JS)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            open_main(reader)
            trace = Trace(context.new_cdp_session(page))
            if args.only in (None, "probes"):
                probe_walk(reader, trace)
            if args.only in (None, "passes"):
                passes_walk(reader, trace)
            reader.check(not errors, f"the page threw: {errors}")
            version = context.browser.version if context.browser else "Chromium"
            context.close()
            return f"{version} at {width}x{height} (load1 {os.getloadavg()[0]:.0f}): " + "; ".join(reader.notes)
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
    parser.add_argument("--only", choices=("probes", "passes"),
                        help="walk one part alone (for a negative control against an older page)")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required: python3 -m pip install --user playwright && "
                         "python3 -m playwright install chromium") from error
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, touch))
    for line in report:
        print(line)
    print(json.dumps({"ok": True}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
