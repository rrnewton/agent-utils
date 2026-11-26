#!/usr/bin/env python3
"""The page's measured geometry restyles only its readers, and one foot keeps the composer still while typing.

`#2277 vibe-talk-key-scroll-freezes-11-26`. The owner, 2026-11-26, on a busy channel at phone size: "Very laggy
client right now ... Screen jumped a lot while I was trying to type a message. And the typing itself was laggy
and got frozen some." The page's freeze reports said 4-5 s long tasks after a key or a scroll. Two causes, each
pinned here in real Chromium, counted, never timed (owner rule 346):

  * THE GEOMETRY RESTYLED EVERY ROW. The box's top edge (`--composer-top`), the chips' room and height, the
    read-aloud strip's room and height and the keyboard's room were written on #frame-body, #screen-main or the
    root, where every element inherits custom properties: each write restyled the whole document (12.5k elements
    at 192 rows), and the box's top edge changes on every scroll at the foot. They are registered not inherited
    and written on their readers alone (web/src/view/dom/geometry.ts). Here, with 260 rows in Main: each reader
    sees what the page writes and no row inherits it; one write on each reader restyles at most
    `WRITE_CEILING` elements (the trace's own style-recalc element counts); and the page's own passes (a scroll
    at the foot with the box in the chips' strip, the chips row growing) never restyle the whole document.
  * TWO KEEPERS, TWO FEET. The keyboard's jump put the box's foot on the keys and parked its Send row under
    them; the keeper holding the composer while rows change brought the whole composer, row included, into
    view. Every row change while typing raised the list by the row and the next jump dropped it back: a 70 px
    bounce. One foot now, the composer's own, Send row included (the owner keeps Send in sight). Here, at
    412x915 with the keyboard up, typing at the foot while messages arrive: no two programmatic scroll writes
    in opposite directions, and the composer's top held within `STILL_PX` at every sampled frame.

THE KEYBOARD IS SIMULATED by resizing the viewport, as Chrome on Android does with the page's
`interactive-widget=resizes-content`. The fake API and the reader are tests/scroll_stability_browser.py's.
Pass --web-root DIR to run against another copy of the page (the page before this fix fails both halves).
"""

from __future__ import annotations

import argparse
import json
import os
import re
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import browser_waits
import offline_cache_browser as fixture
import page_sources
from offline_cache_browser import Json
import scroll_stability_browser as stability
from scroll_stability_browser import FILLER, READS_JS, Reader, StabilityApi, handler, open_main

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, CDPSession

KEYBOARD_PX = 336
SIZES = (("phone-412", 412, 915, True), ("desk-1280", 1280, 800, False))
# Messages added to the fixture's channel, so Main holds over 200 rows (the cost was linear in rows).
EXTRA_ROWS = 220
# The most elements one geometry write on one reader may restyle: the reader, and its direct children where
# they inherit something from it explicitly (33 measured on #screen-main at a desk). Bounded by the page's
# shell, never by its rows: the whole document is thousands, and before this fix one write on #pane-discord
# or #screen-main restyled every row.
WRITE_CEILING = 64
# A style recalc that touches at least this share of the document's elements is a whole-document restyle.
WHOLE_SHARE = 0.5
# How far the composer may move while typing at the foot with the keyboard up (sub-pixel placement).
STILL_PX = 0.5
# The most a drawn state is waited for (#998 STATE_S precedent); passes take as long as the page does.
STATE_S = 30.0
GEOMETRY_TS = Path(__file__).resolve().parents[1] / "web" / "src" / "view" / "dom" / "geometry.ts"


def census() -> dict[str, list[str]]:
    """Who reads each geometry property, by element id, read from the page's own census
    (`GEOMETRY_CONSUMERS` in web/src/view/dom/geometry.ts), so this check and the page cannot drift."""
    text = GEOMETRY_TS.read_text()
    chips = re.search(r"CHIPS_FOOT_CONSUMERS[^=]*=\s*Object\.freeze\(\[(.*?)\]\)", text, re.S)
    assert chips, "no CHIPS_FOOT_CONSUMERS in geometry.ts"
    chips_ids = re.findall(r'"([\w-]+)"', chips.group(1))
    found: dict[str, list[str]] = {"--chips-foot": chips_ids}
    for name, body in re.findall(r'"(--[\w-]+)":\s*\{(.*?)\}', text, re.S):
        ids: list[str] = []
        for part in re.findall(r"(?:live|held):\s*(CHIPS_FOOT_CONSUMERS|\[[^\]]*\])", body):
            ids += chips_ids if part == "CHIPS_FOOT_CONSUMERS" else re.findall(r'"([\w-]+)"', part)
        found[name] = ids
    assert len(found) == 7, f"the census read {sorted(found)}"
    return found


class LongApi(StabilityApi):
    """The stability fixture's channel with `EXTRA_ROWS` more messages in Main, all of them in the first read
    of All (its page size raised for this check alone), so the long list is on the page at once."""

    def __init__(self) -> None:
        super().__init__()
        for index in range(EXTRA_ROWS):
            self.add(f"Long channel row {index}. {FILLER * (1 + index % 3)}")


class Trace:
    """The trace's style recalcs between two marks, by element count (`devtools.timeline` UpdateLayoutTree)."""

    def __init__(self, cdp: "CDPSession") -> None:
        self.cdp = cdp
        self.events: list[Json] = []
        self.done = threading.Event()
        cdp.on("Tracing.dataCollected", lambda event: self.events.extend(event["value"]))
        cdp.on("Tracing.tracingComplete", lambda _event: self.done.set())

    def start(self) -> None:
        self.events.clear()
        self.done.clear()
        self.cdp.send("Tracing.start", {"traceConfig": {"includedCategories": ["devtools.timeline"]},
                                        "transferMode": "ReportEvents"})

    def stop(self, reader: Reader) -> None:
        self.cdp.send("Tracing.end")
        deadline = time.monotonic() + STATE_S
        while not self.done.is_set() and time.monotonic() < deadline:
            reader.page.wait_for_timeout(25)
        reader.check(self.done.is_set(), "the trace never completed")

    def recalcs(self, mark: str) -> list[int]:
        """The element counts of every style recalc between `mark`-start and `mark`-end."""
        stamps: dict[str, float] = {}
        counts: list[tuple[float, int]] = []
        for event in self.events:
            args = event.get("args")
            args = args if isinstance(args, dict) else {}
            stamp = event.get("ts")
            if not isinstance(stamp, (int, float)):
                continue
            if event.get("name") == "TimeStamp":
                data = args.get("data")
                if isinstance(data, dict):
                    stamps[str(data.get("message"))] = float(stamp)
            elif event.get("name") == "UpdateLayoutTree" and event.get("ph") == "X":
                count = args.get("elementCount")
                counts.append((float(stamp), int(count) if isinstance(count, int) else 0))
        start, end = stamps.get(f"{mark}-start"), stamps.get(f"{mark}-end")
        assert start is not None and end is not None, f"the marks of {mark} are not in the trace"
        return [count for at, count in counts if start <= at <= end]


WRITE_JS = """([id, name, value, mark]) => {
    const node = document.getElementById(id);
    if (!node) return null;
    const read = () => getComputedStyle(document.getElementById('channel-compose-text')).color + document.body.offsetHeight;
    read();
    const was = node.style.getPropertyValue(name);
    console.timeStamp(mark + '-start');
    node.style.setProperty(name, value);
    const seen = getComputedStyle(node).getPropertyValue(name).trim();
    read();
    console.timeStamp(mark + '-end');
    if (was) node.style.setProperty(name, was); else node.style.removeProperty(name);
    read();
    return seen;
}"""

# Each reader's computed value of each property against what is written on it, and what the rows compute.
COMPUTED_JS = """(census) => {
    const out = {readers: [], rows: []};
    for (const [name, ids] of Object.entries(census)) {
        for (const id of ids) {
            const node = document.getElementById(id);
            if (!node) continue;
            out.readers.push([name, id, node.style.getPropertyValue(name).trim(), getComputedStyle(node).getPropertyValue(name).trim()]);
        }
        const rows = [...document.querySelectorAll('#discord-log > li[data-id]')].slice(-3);
        for (const node of [document.getElementById('frame-body'), document.getElementById('discord-log'), ...rows]) {
            out.rows.push([name, node.id || node.getAttribute('data-id'), getComputedStyle(node).getPropertyValue(name).trim()]);
        }
    }
    out.elements = document.getElementsByTagName('*').length;
    out.rowCount = document.querySelectorAll('#discord-log > li[data-id]').length;
    return out;
}"""


def restyle_walk(reader: Reader, trace: Trace, names: dict[str, list[str]]) -> None:
    """Every reader sees what is written on it, no row inherits it, and one write restyles a handful."""
    page = reader.page
    found = page.evaluate(COMPUTED_JS, names)
    assert isinstance(found, dict)
    reader.check(int(found["rowCount"]) >= 200, f"Main holds {found['rowCount']} rows, not a long list")
    for name, id_, written, computed in found["readers"]:
        if written:
            reader.check(computed == written, f"#{id_} computes {name} as {computed!r}, not the {written!r} written on it")
    for name, id_, computed in found["rows"]:
        if name != "--chips-foot":
            reader.check(computed == "", f"{id_} inherits {name} ({computed!r}): every row would restyle on a write")
    # One write on each reader, counted from the trace.
    trace.start()
    marks: list[tuple[str, str, str]] = []
    for name, ids in names.items():
        for id_ in ids:
            mark = f"w{len(marks)}"
            seen = page.evaluate(WRITE_JS, [id_, name, "17px", mark])
            if seen is None:
                continue
            reader.check(seen == "17px", f"#{id_} does not compute {name} as written on it: {seen!r}")
            marks.append((mark, name, id_))
    trace.stop(reader)
    worst = 0
    for mark, name, id_ in marks:
        restyled = sum(trace.recalcs(mark))
        worst = max(worst, restyled)
        reader.check(restyled <= WRITE_CEILING,
                     f"one write of {name} on #{id_} restyled {restyled} elements (ceiling {WRITE_CEILING}, the page "
                     f"holds {found['elements']})")
    reader.notes.append(f"{len(marks)} geometry writes on their readers restyled at most {worst} elements each "
                        f"of {found['elements']} ({found['rowCount']} rows); no row inherits any of them")


PASS_JS = """async ([mark, act]) => {
    const frame = () => new Promise((done) => requestAnimationFrame(() => done()));
    const area = document.getElementById('scroll-area');
    console.timeStamp(mark + '-start');
    if (act === 'foot') { area.scrollTop = area.scrollHeight; }
    else if (act === 'strip') { area.scrollTop = area.scrollHeight - area.clientHeight - 60; }
    else if (act === 'up') { area.scrollTop = Math.max(0, area.scrollTop - 1500); }
    for (let i = 0; i < 4; i++) await frame();
    await new Promise((done) => setTimeout(done, 0));
    await frame();
    console.timeStamp(mark + '-end');
    return document.getElementsByTagName('*').length;
}"""


def passes_walk(reader: Reader, trace: Trace) -> None:
    """The page's own passes: a scroll at the foot with the box in the chips' strip, and the chips row
    changing as the reader leaves the foot, never restyle the whole document."""
    page = reader.page
    trace.start()
    elements = 0
    acts = ("foot", "strip", "foot", "up", "foot")
    for index, act in enumerate(acts):
        elements = int(page.evaluate(PASS_JS, [f"p{index}", act]))
    trace.stop(reader)
    for index, act in enumerate(acts):
        recalcs = trace.recalcs(f"p{index}")
        whole = [count for count in recalcs if count >= elements * WHOLE_SHARE]
        reader.check(not whole, f"the page's own pass '{act}' restyled the whole document {len(whole)} times "
                                f"({whole} of {elements} elements)")
    reader.notes.append(f"{len(acts)} scroll passes at and from the foot: no whole-document restyle")


RECORD_JS = """() => {
    const area = document.getElementById('scroll-area');
    const desc = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollTop');
    const keep = window.__clfWrites = {writes: [], tops: [], on: true};
    Object.defineProperty(area, 'scrollTop', {configurable: true, get() { return desc.get.call(this); },
        set(v) { const b = desc.get.call(this); desc.set.call(this, v); const d = desc.get.call(this) - b;
            if (keep.on && Math.abs(d) > 0.5) keep.writes.push([performance.now(), d, (new Error().stack || '').split('\\n').slice(2, 7).join(' | ')]); }});
    for (const m of ['scrollTo', 'scrollBy']) {
        const orig = area[m].bind(area);
        area[m] = (...args) => { const b = desc.get.call(area); const r = orig(...args); const d = desc.get.call(area) - b;
            if (keep.on && Math.abs(d) > 0.5) keep.writes.push([performance.now(), d]); return r; };
    }
    const composer = document.getElementById('channel-composer');
    const sample = () => { if (!keep.on) return; keep.tops.push(composer.getBoundingClientRect().top); requestAnimationFrame(sample); };
    requestAnimationFrame(sample);
}"""


def typing_walk(reader: Reader) -> None:
    """Typing at the foot with the keyboard up while messages arrive: no two scroll writes in opposite
    directions, and the composer held within `STILL_PX` at every frame."""
    page = reader.page
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    reader.quiet(300)
    page.tap("#channel-compose-text")
    page.set_viewport_size({"width": 412, "height": 915 - KEYBOARD_PX})
    reader.wait_for(f"() => Math.round(window.visualViewport.height) === {915 - KEYBOARD_PX}"
                    " && document.documentElement.getAttribute('data-keyboard') === 'open'",
                    "the keyboard never came up", STATE_S)
    reader.settled(seconds=STATE_S)
    page.keyboard.type("ok ", delay=20)
    reader.settled(seconds=STATE_S)
    page.evaluate(RECORD_JS)
    for turn in range(4):
        page.keyboard.type("go ", delay=20)
        arrived = reader.api.add(f"An arrival while the owner types, number {turn}. {FILLER}")
        reader.api.push("message", {"message": arrived}, str(arrived["id"]))
        reader.wait_for(f"() => !!document.querySelector(\"#discord-log > li[data-id='{arrived['id']}']\")",
                        "the live message never arrived", STATE_S)
        reader.settled(seconds=STATE_S)
    reader.frames(3)
    found = page.evaluate("() => { const k = window.__clfWrites; k.on = false; return k; }")
    assert isinstance(found, dict)
    if os.environ.get("GEOMETRY_DEBUG"):
        for one in found["writes"]:
            print("WRITE", one)
    writes = [float(one[1]) for one in found["writes"]]
    flips = sum(1 for a, b in zip(writes, writes[1:]) if a * b < 0)
    reader.check(flips == 0, f"typing at the foot while messages arrived, the list was written back and forth "
                             f"{flips} times: {writes}")
    tops = [float(t) for t in found["tops"]]
    reader.check(len(tops) > 0, "no frame was sampled")
    spread = max(tops) - min(tops) if tops else 0.0
    reader.check(spread <= STILL_PX, f"the composer moved {spread:.2f}px while typing at the foot "
                                     f"(min {min(tops):.2f}, max {max(tops):.2f}, {len(tops)} frames)")
    composer_send = page.evaluate("""() => { const s = document.getElementById('channel-send').getBoundingClientRect();
        return {bottom: s.bottom, keys: window.visualViewport.height, corner: !document.getElementById('keyboard-send').hidden}; }""")
    assert isinstance(composer_send, dict)
    reader.check(float(composer_send["bottom"]) <= float(composer_send["keys"]) + 1,
                 f"the Send row is under the keys while typing: {composer_send}")
    reader.check(composer_send["corner"] is False, f"a corner Send stands beside the row's own: {composer_send}")
    reader.notes.append(f"typing at the foot with the keyboard up through 4 arrivals: {len(writes)} scroll writes, "
                        f"none reversing another; the composer moved {spread:.2f}px over {len(tops)} frames; "
                        "its Send row whole above the keys, no corner Send")


def walk(chromium: "BrowserType", args: argparse.Namespace, label: str, width: int, height: int, touch: bool) -> str:
    stability.FLAT_PAGE = 1000
    api = LongApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-geometry-") as profile:
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
            names = census()
            if args.only in (None, "restyle"):
                restyle_walk(reader, trace, names)
            if args.only in (None, "passes"):
                passes_walk(reader, trace)
            if touch and args.only in (None, "typing"):
                typing_walk(reader)
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
    parser.add_argument("--only", choices=("restyle", "passes", "typing"),
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
