#!/usr/bin/env python3
"""Hold a parked reader still for 60 s of background updates, in real Chromium at 393x852 and 1280x800.

`#1210 ui-stability-no-async-shift`. The owner, 2026-11-10 (reply 298): "Right now I can scroll
to a point and start reading and then pretty frequently I see something cause what I'm looking at
to move." The requirement is UI stability: nothing the user is looking at moves unless the user
acted. `tests/scroll_stability_browser.py` already parks a reader mid-Main and judges one act at a
time over about 2.5 s; this walk holds the same parked reader for a full minute of scripted
background traffic -- channel messages, thread replies, live-tail edits above and below, reactions
above, on the anchor row, and many at once on a folded card (the overlay demotion of `#1063`), read
state arriving by re-read, the Newest chip recounting, a draft held in the composer, and the page's
own 45 s poll -- sampling the anchor row's offset at every frame and collecting `layout-shift`
entries, and reports each act separately so a failure names which update moved the viewport. A
near-foot coda parks the reader inside the old 24 px slack (gap 18 px, outside the 12 px rest) while
messages arrive: the `#1185` slack capture must not snap them to the foot.

Collapsed-message summaries are OFF unless `--summaries on`: off is the owner's client and the
shipped default (`vibe-talk.voice.summarize-collapsed`, `#564`), at which no message summary is
fetched or drawn. With `--summaries on` the soak also toggles Summaries and lets them land late.

The anchor offset is the primary measure (the owner's complaint); layout shift is secondary
evidence, since a shift inside the scroller that scroll anchoring compensates may not register as
viewport shift. The minute runs in real time, with no fake clock, so the frame sampling, the real
poll, and Chromium scroll anchoring are all genuine. A closing control disables scroll anchoring and
grows a row above the anchor: the anchor must move, proving the watch can see a shift.

REPORT-ONLY until `#1210` part D lands: by default every act's worst movement and shift are printed
beside the recorded 2026-11-10 baseline and the walk passes (harness failures -- no frames, no
reads, a blind control, page errors -- still fail). Pass `--strict` to assert the stability
requirement itself (anchor within a pixel, shift at most 0.01 per act). TODO(#1210): make
`--strict` the gate default once the part-D fixes hold it green at both sizes.

Pass --size phone-393 / --size desk-1280 (repeatable) to run one size; --soak-seconds to compress
the schedule for a smoke run (the 45 s poll then does not fire); --screenshots DIR to keep a PNG of
each stage; --web-root DIR to run against another copy of the page.
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

import offline_cache_browser as fixture
import page_sources
from offline_cache_browser import Json
from scroll_stability_browser import (
    FILLER,
    MAIN_FROM,
    MAIN_TO,
    READS_JS,
    ROW_JS,
    SMALL_ROOTS,
    STILL_PX,
    Reader,
    StabilityApi,
    handler,
    open_main,
)

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

# (label, width, height, touch): the owner's phone in one column by touch, and a desk in two columns.
SIZES = (("phone-393", 393, 852, True), ("desk-1280", 1280, 800, False))
# The most layout shift (no recent input) a minute of background updates may add in the viewport.
CLS_MAX = 0.01
# Recorded 2026-11-10, summaries ON, full 60 s runs: the worst shift seen per act across both sizes.
# Report-only runs print measured beside baseline; TODO(#1210): delete with the --strict flip.
BASELINE_SHIFT = {"summaries-landing": 0.2448, "thread-reply": 0.0680}
# Four ordinary reactions on a folded card: few enough to be tried overlaid at draw time (the
# candidate cap, no custom names), wide enough at phone width that the measured settle demotes
# the strip to its own row a frame later (`#1063`). More chips, or a custom name, would keep the
# row from the start and test nothing.
DEMOTE_REACTIONS: list[Json] = [
    {"emoji": "\U0001f44d", "count": 12}, {"emoji": "\U0001f389", "count": 9},
    {"emoji": "\U0001f680", "count": 12}, {"emoji": "❤️", "count": 8},
]

# Every layout shift the browser reports, with its time, value, input flag, and sources, so the walk
# can charge shift to the act that caused it. Values with recent input are kept but never charged:
# the reader here does nothing after parking, so any charged shift is background-caused.
CLS_JS = """(() => {
    window.__clsEntries = [];
    const name = (node) => {
        if (!node || !node.tagName) return '?';
        let text = node.tagName.toLowerCase();
        if (node.id) text += '#' + node.id;
        const data = node.getAttribute && node.getAttribute('data-id');
        if (data) text += '[data-id=' + data + ']';
        return text;
    };
    try {
        new PerformanceObserver((list) => {
            for (const entry of list.getEntries()) {
                const sources = (entry.sources || []).map((one) => {
                    const previous = one.previousRect || {};
                    const current = one.currentRect || {};
                    return name(one.node) + ' y' + Math.round(previous.y || 0) + '->' + Math.round(current.y || 0);
                });
                window.__clsEntries.push({
                    t: entry.startTime, value: entry.value,
                    input: !!entry.hadRecentInput, sources: sources.slice(0, 4),
                });
            }
        }).observe({type: 'layout-shift', buffered: true});
    } catch (error) {
        window.__clsEntries = null;
    }
})()"""

# The anchor row's offset from the scroller's top, the first row on screen and its offset, the gap
# below, and the scroll offset, sampled at every frame until STOP_WATCH_JS. The anchor is found by
# id throughout, so rows arriving above it do not change what is measured.
WATCH_ROW_JS = """(id) => {
    const area = document.getElementById('scroll-area');
    const list = document.getElementById('discord-log');
    const sample = () => {
        const top = area.getBoundingClientRect().top;
        const anchorRow = [...list.children].find((one) => one.getAttribute('data-id') === id);
        const anchor = anchorRow && anchorRow.getClientRects().length
            ? anchorRow.getBoundingClientRect().top - top : null;
        let firstId = null, firstOffset = null;
        for (const row of list.children) {
            if (!row.hasAttribute('data-id') || !row.getClientRects().length) continue;
            const box = row.getBoundingClientRect();
            if (box.bottom > top) { firstId = row.getAttribute('data-id'); firstOffset = box.top - top; break; }
        }
        return [anchor, firstId, firstOffset,
                area.scrollHeight - area.scrollTop - area.clientHeight, area.scrollTop, performance.now()];
    };
    const watch = {samples: [], on: true};
    window.__soakWatch = watch;
    const tick = () => { if (!watch.on) return; watch.samples.push(sample()); requestAnimationFrame(tick); };
    tick();
}"""
WATCH_COUNT_JS = "() => window.__soakWatch ? window.__soakWatch.samples.length : 0"
STOP_WATCH_JS = "() => { const watch = window.__soakWatch; watch.on = false; return watch.samples; }"
CLS_NOW_JS = "() => ({now: performance.now(), entries: window.__clsEntries})"
CHIPS_JS = """() => {
    const label = document.getElementById('jump-newest-label');
    const button = document.getElementById('jump-newest');
    const draft = document.getElementById('draft-chip');
    return {
        newest: label ? label.textContent : null,
        newestShown: button ? !button.hidden : null,
        draftShown: draft ? !draft.hidden && !!draft.getClientRects().length : null,
    };
}"""
CONTROL_CSS_JS = """() => {
    const style = document.createElement('style');
    style.id = 'soak-control-css';
    style.textContent = '#scroll-area { overflow-anchor: none !important; }';
    document.head.append(style);
}"""
GROW_ABOVE_JS = """(id) => {
    const row = [...document.getElementById('discord-log').children]
        .find((one) => one.getAttribute('data-id') === id);
    if (!row) return false;
    row.style.minHeight = (row.getBoundingClientRect().height + 120) + 'px';
    return true;
}"""


class SoakApi(StabilityApi):
    """The stability channel, with reactions and read state as background acts the soak can fire."""

    def react(self, message_id: str, reactions: list[Json] | None) -> Json:
        """Message `message_id` with its reactions set (or cleared), as reads now answer it."""
        with self.lock:
            row = next(m for m in self.messages if str(m["id"]) == message_id)
            if reactions is None:
                row.pop("reactions", None)
            else:
                row["reactions"] = reactions
            return dict(row)

    def set_read(self, message_ids: list[str], done: bool) -> None:
        """Read state changed elsewhere: it reaches this page only in the next read's `dismissed`."""
        with self.lock:
            for message_id in message_ids:
                if done:
                    self.dismissed.add(message_id)
                else:
                    self.dismissed.discard(message_id)

    def touch(self, index: int) -> Json:
        """Message `200 + index` unchanged, to announce as an update that asks for a re-read."""
        with self.lock:
            row = next(m for m in self.messages if m["id"] == str(200 + index))
            return dict(row)


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path,
                        help="directory to write one PNG per stage into (default: none are kept)")
    parser.add_argument("--size", action="append", choices=[label for label, *_ in SIZES],
                        help="run only this size (repeatable; default: every size)")
    parser.add_argument("--soak-seconds", type=float, default=60.0,
                        help="soak length in seconds (default: 60; shorter compresses the schedule "
                             "and the page's 45 s poll does not fire)")
    parser.add_argument("--summaries", choices=("on", "off"), default="off",
                        help="collapsed-message summaries (default: off, the owner's client and the "
                             "shipped default; on adds the Summaries toggle act)")
    parser.add_argument("--strict", action="store_true",
                        help="assert the stability requirement (default: report-only, see module docstring)")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required: python3 -m pip install --user playwright && "
                         "python3 -m playwright install chromium") from error
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    sizes = [size for size in SIZES if not args.size or size[0] in args.size]
    failures: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch in sizes:
            try:
                print(walk(playwright.chromium, args, label, width, height, touch), flush=True)
            except AssertionError as error:
                failures.append(f"{label}: {error}")
                print(f"FAIL {label}: {error}", flush=True)
    return 1 if failures else 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, touch: bool) -> str:
    """One parked reader at one size, against a fresh fake API and browser profile."""
    api = SoakApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-scroll-soak-") as profile:
            if touch:
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="dark",
                    device_scale_factor=2.625, is_mobile=True, has_touch=True)
            else:
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="dark")
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.add_init_script(READS_JS)
            page.add_init_script(CLS_JS)
            reader = Reader(page, api, args.screenshots, label, touch)  # type: ignore[arg-type]
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            open_main(reader)
            soak_walk(reader, args.soak_seconds, summaries_on=args.summaries == "on", strict=args.strict)
            reader.check(not errors, f"the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(reader.notes)
    finally:
        api.stopping.set()
        api.timeline_gate.set()
        server.shutdown()
        server.server_close()
        thread.join()


def reads_zero(reader: Reader, seconds: float = 5.0) -> None:
    """Wait until no channel read is on the wire, so the next act starts from a landed page."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        if int(reader.page.evaluate("() => window.__timelineReads || 0")) == 0:
            return
        reader.page.wait_for_timeout(25)


def soak_walk(reader: Reader, soak_seconds: float, *, summaries_on: bool, strict: bool) -> None:
    """Park mid-Main with a draft held, then drive a minute of background updates under the watch."""
    page = reader.page
    api: SoakApi = reader.api  # type: ignore[assignment]
    # A draft held in the composer for the whole soak: typed before parking, so the draft chip and
    # the chip row's height are part of the baseline the reader is judged from, not an act in it.
    if reader.touch:
        page.tap("#channel-compose-text")
    else:
        page.click("#channel-compose-text")
    page.keyboard.type("A draft held while reading.", delay=10)
    reader.quiet(300)
    reader.scroll_main(0.5)
    reader.check(reader.gap("scroll-area") > 400, "Main is too short to park in, so this proves nothing")
    reader.settled()
    anchor_id, baseline = reader.first()
    reader.shot("1-parked")
    above_edit = MAIN_FROM + 1
    above_react = MAIN_FROM + 3
    below_edit = MAIN_TO - 2
    reads_at_start = api.timeline_requests()
    summaries_at_start = api.summaries_asked

    def channel_message() -> None:
        arrived = api.add(f"A message posted to the channel during the soak. {FILLER}")
        api.push("message", {"message": arrived}, str(arrived["id"]))
        reader.wait_for(f"() => !!document.querySelector(\"#discord-log > li[data-id='{arrived['id']}']\")",
                        "the soak channel message never reached Main")

    def thread_reply() -> None:
        reply = api.add(f"A reply arriving in a thread during the soak. {FILLER}",
                        f"spaces/A/threads/t{SMALL_ROOTS[1]}")
        api.push("message", {"message": reply}, str(reply["id"]))

    def edit_above() -> None:
        edited = api.edit(above_edit, f"Message {200 + above_edit}, edited taller above the reader. {FILLER * 5}")
        api.push("message_update", {"message": edited}, "soak-edit-above")
        reader.wait_for(f"() => document.querySelector(\"#discord-log > li[data-id='{200 + above_edit}']\")"
                        ".textContent.includes('edited taller above')", "the edit above never landed")

    def react_above() -> None:
        reacted = api.react(str(200 + above_react), [{"emoji": "\U0001f44d", "count": 2}])
        api.push("message_update", {"message": reacted}, "soak-react-above")

    def react_anchor() -> None:
        reacted = api.react(anchor_id, [{"emoji": "\U0001f44d", "count": 1}])
        api.push("message_update", {"message": reacted}, "soak-react-anchor")

    def react_folded_demote() -> None:
        # A folded row above the anchor (fixture rows over 280 chars start folded; every fourth
        # Main row is longest), given more reactions than its elision line can share: the strip is
        # first overlaid at zero height, then demoted to its own row a frame later (`#1063`).
        anchor_index = int(anchor_id) - 200
        folded = [i for i in range(MAIN_FROM, anchor_index) if i % 4 == 3]
        reader.check(bool(folded), "no folded row above the anchor to demote, so this proves nothing")
        target = str(200 + folded[-1])
        reader.check(bool(page.evaluate(
            f"() => document.querySelector(\"#discord-log > li[data-id='{target}']\")"
            ".getAttribute('data-collapsed') === 'true'")),
            f"row {target} is not folded, so the demotion act proves nothing")
        reacted = api.react(target, DEMOTE_REACTIONS)
        api.push("message_update", {"message": reacted}, "soak-react-folded")

    def read_state() -> None:
        api.set_read([str(200 + above_edit), anchor_id], True)
        api.push("message_update", {"message": api.touch(below_edit)}, "soak-read-state")

    def edit_below() -> None:
        edited = api.edit(below_edit, f"Message {200 + below_edit}, edited taller below the reader. {FILLER * 4}")
        api.push("message_update", {"message": edited}, "soak-edit-below")
        reader.wait_for(f"() => document.querySelector(\"#discord-log > li[data-id='{200 + below_edit}']\")"
                        ".textContent.includes('edited taller below')", "the edit below never landed")

    def summaries() -> None:
        asked = api.summaries_asked
        page.click("#action-menu-open")
        page.click("#summarise")
        deadline = time.monotonic() + 5
        while api.summaries_asked <= asked and time.monotonic() < deadline:
            page.wait_for_timeout(25)
        reader.quiet(int(api.summary_delay * 1000) + 900)

    # (offset in a 60 s soak, name, act). Offsets scale with --soak-seconds; idle windows before the
    # first act and across the 45 s poll prove stillness with nothing scripted at all.
    schedule: list[tuple[float, str, object]] = [
        (5, "channel-message", channel_message),
        (12, "thread-reply", thread_reply),
        (19, "edit-taller-above", edit_above),
        (26, "reaction-above", react_above),
        (31, "reaction-on-anchor", react_anchor),
        (36, "reaction-folded-demote", react_folded_demote),
        (41, "read-state-by-reread", read_state),
        (46, "edit-taller-below", edit_below),
    ]
    if summaries_on:
        schedule.append((52, "summaries-landing", summaries))
    scale = soak_seconds / 60.0
    page.evaluate(WATCH_ROW_JS, anchor_id)
    # Marks charge samples and shift to an act: (name, first sample index, performance.now()).
    marks: list[tuple[str, int, float]] = []

    def mark(name: str) -> None:
        count = int(page.evaluate(WATCH_COUNT_JS))
        now = float(page.evaluate("() => performance.now()"))
        marks.append((name, count, now))

    mark("idle-before-first-act")
    started = time.monotonic()
    for offset, name, act in schedule:
        due = started + offset * scale
        while time.monotonic() < due:
            page.wait_for_timeout(50)
        mark(name)
        act()  # type: ignore[operator]
        reads_zero(reader)
        reader.quiet(400)
    mark("poll-window-idle")
    while time.monotonic() < started + soak_seconds:
        page.wait_for_timeout(50)
    mark("end")
    samples = page.evaluate(STOP_WATCH_JS)
    cls_state = page.evaluate(CLS_NOW_JS)
    chips = page.evaluate(CHIPS_JS)
    reader.shot("2-after-soak")

    entries = cls_state["entries"] if isinstance(cls_state, dict) else None
    reader.check(len(samples) > 100, f"only {len(samples)} frames were sampled, so this proves nothing")
    reader.check(api.timeline_requests() > reads_at_start,
                 "no channel read landed during the soak, so this proves nothing")
    lines: list[str] = []
    failures: list[str] = []
    for at, (name, first, perf_start) in enumerate(marks[:-1]):
        last, perf_end = marks[at + 1][1], marks[at + 1][2]
        window = samples[first:last]
        drawn = [float(s[0]) for s in window if isinstance(s[0], (int, float))]
        undrawn = len(window) - len(drawn)
        worst = max((abs(offset - baseline) for offset in drawn), default=float("inf"))
        shift = 0.0
        worst_source = ""
        worst_value = 0.0
        if isinstance(entries, list):
            for entry in entries:
                if entry["input"] or not perf_start <= float(entry["t"]) < perf_end:
                    continue
                shift += float(entry["value"])
                if float(entry["value"]) > worst_value:
                    worst_value = float(entry["value"])
                    worst_source = "; ".join(str(s) for s in entry["sources"][:2])
        moved = worst > STILL_PX or undrawn > 0
        shifted = shift > CLS_MAX
        verdict = "MOVED" if moved or shifted else "still"
        recorded = BASELINE_SHIFT.get(name)
        lines.append(f"soak {reader.label} {name}: {verdict} worst {worst:.1f}px over {len(window)} frames"
                     f" ({undrawn} undrawn), shift {shift:.4f}"
                     + (f" (baseline {recorded:.4f})" if recorded is not None else "")
                     + (f" largest {worst_value:.4f} e.g. {worst_source}" if worst_source else ""))
        if verdict == "MOVED":
            failures.append(name)
    for line in lines:
        print(line, flush=True)
    reader.notes.append(f"parked on row {anchor_id}; chips at end {chips}; "
                        f"{api.summaries_asked - summaries_at_start} summaries asked")
    reader.notes.extend(lines)
    end_offset = page.evaluate(ROW_JS, ["discord-log", anchor_id])
    if isinstance(end_offset, (int, float)):
        print(f"soak {reader.label} end-state: anchor row {anchor_id} at {float(end_offset):.1f}px "
              f"(parked at {baseline:.1f}px)", flush=True)
    reader.overflow()
    control(reader, str(200 + above_edit), anchor_id)
    near_foot(reader, strict=strict)
    if strict:
        reader.check(not failures,
                     f"background updates moved the parked reader (anchor row {anchor_id}, parked at "
                     f"{baseline:.1f}px): {', '.join(failures)}; per-act lines above name each mover")
    elif failures:
        print(f"soak {reader.label} report-only: movers {', '.join(failures)} (baseline recorded; "
              "--strict asserts)", flush=True)
        reader.notes.append(f"report-only movers: {', '.join(failures)}")


def near_foot(reader: Reader, *, strict: bool) -> None:
    """Inside the old slack but outside the rest: chip resizes must not snap the reader to the foot.

    The `#1185` capture: chip and strip resizes wrote the list to its end whenever a re-derived
    at-the-bottom test (24 px slack, plus however much the chips grew) read true. A reader resting
    at gap 18 px — inside that slack, outside the 12 px rest the follow state is noted from — is
    not following, and a thread reply's recount plus an edit's re-read must leave the gap put. (A
    channel arrival would follow by the newest line's own rule, so none is sent here.)
    """
    page = reader.page
    api: SoakApi = reader.api  # type: ignore[assignment]
    # The control's anchoring-off sheet and grown row belong to the control alone.
    page.evaluate("() => { const s = document.getElementById('soak-control-css'); if (s) s.remove(); }")
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    reader.quiet(500)
    reader.check(reader.gap("scroll-area") <= 1, "near-foot: the reader never reached the foot")
    if not reader.touch:
        box = page.locator("#scroll-area").bounding_box()
        assert box is not None
        page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
        page.mouse.wheel(0, -60)
        reader.quiet(300)
    page.evaluate("() => { const a = document.getElementById('scroll-area');"
                  " a.scrollTop = a.scrollHeight - a.clientHeight - 18; }")
    reader.quiet(500)
    gap_before = reader.gap("scroll-area")
    reader.check(12 < gap_before <= 24,
                 f"near-foot: parked gap is {gap_before:.0f}px, not inside the slack outside the rest")
    anchor_id, baseline = reader.first()
    page.evaluate(WATCH_ROW_JS, anchor_id)
    # No channel arrival here: one of those follows a reader inside the slack by design (the newest
    # line's own rule), which would mask the chip capture. A thread reply (counts and chips only)
    # and an edit below (a re-read) resize the same chip row without that follow.
    reply = api.add(f"A thread reply while resting inside the slack. {FILLER}",
                    f"spaces/A/threads/t{SMALL_ROOTS[1]}")
    api.push("message", {"message": reply}, str(reply["id"]))
    reads_zero(reader)
    reader.quiet(600)
    gap_reply = reader.gap("scroll-area")
    below = MAIN_TO - 2
    edited = api.edit(below, f"Message {200 + below}, edited near the foot. {FILLER * 4}")
    api.push("message_update", {"message": edited}, "near-foot-edit")
    reader.wait_for(f"() => document.querySelector(\"#discord-log > li[data-id='{200 + below}']\")"
                    ".textContent.includes('edited near the foot')", "near-foot: the edit never landed")
    reads_zero(reader)
    reader.quiet(600)
    samples = page.evaluate(STOP_WATCH_JS)
    gap_after = reader.gap("scroll-area")
    drawn = [float(s[0]) for s in samples if isinstance(s[0], (int, float))]
    worst = max((abs(offset - baseline) for offset in drawn), default=float("inf"))
    snapped = gap_after < gap_before - STILL_PX
    line = (f"soak {reader.label} near-foot-slack: {'SNAPPED' if snapped else 'held'} "
            f"gap {gap_before:.0f}px -> {gap_reply:.0f}px (thread reply) -> {gap_after:.0f}px (edit), "
            f"anchor worst {worst:.1f}px over {len(samples)} frames")
    print(line, flush=True)
    reader.notes.append(line)
    if strict:
        reader.check(not snapped and worst <= STILL_PX,
                     "near-foot: arrivals snapped a reader resting inside the slack to the foot")


def control(reader: Reader, above_id: str, anchor_id: str) -> None:
    """With anchoring off, growing a row above the anchor must move it: the watch can see a shift."""
    page = reader.page
    before = page.evaluate(ROW_JS, ["discord-log", anchor_id])
    reader.check(isinstance(before, (int, float)), "control: the anchor row is not drawn")
    page.evaluate(CONTROL_CSS_JS)
    reader.check(bool(page.evaluate(GROW_ABOVE_JS, above_id)), "control: no row above to grow")
    reader.frames(4)
    after = page.evaluate(ROW_JS, ["discord-log", anchor_id])
    assert isinstance(before, (int, float))
    moved = abs(float(after) - float(before)) if isinstance(after, (int, float)) else 0.0
    reader.check(moved > 5,
                 f"control: growing a row above with anchoring off moved the anchor only {moved:.1f}px, "
                 "so the soak could not have seen a shift")
    reader.notes.append(f"control moved the anchor {moved:.0f}px with anchoring off, so the watch sees shifts")


if __name__ == "__main__":
    raise SystemExit(main())
