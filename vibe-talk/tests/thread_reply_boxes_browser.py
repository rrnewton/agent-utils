#!/usr/bin/env python3
"""Replies on their way stand as boxes from the first packets (`#2385`).

`#2385 vibe-talk-thread-open-layout-first`. The owner, 2026-11-27, on a weak connection, opened
a thread of a root and 9 replies and for a long while saw ONE reply, then the rest at once:
"blank boxes would have appeared from the very first packets that are received instead of
waiting in this ambiguous state for messages to then all the sudden appear later."

This walk serves the real page with the threaded walks' fake API. The device first sees the
thread with its root and one reply, eight replies are posted, and the page reopens in Main.
Every read the page sends is HELD by the fake API until the walk releases it, so "before the
bodies" is an order of events, never a clock (owner rule 346). At 412x915 (dark, touch) and
1280x800 (light), it checks:

  late count (the owner's case): the reader opens the thread from this device's saved copy
  before Main's read lands; nothing reads the thread then. When Main's read lands with the
  count of 9, the open thread draws 8 boxes AND sends its read at once (reason "enter", not
  the poll), with no reply body folded yet; the rows on screen do not move, on any frame,
  while the boxes come and while the replies fill them;
  count known at entry: entering the thread after Main's read draws 8 boxes in the first frame
  that shows the thread, before its read lands; the entry lands as on a first unread reply,
  and when the bodies land the first new reply stands exactly where the first box stood
  (within 0.5 px) and the row at the head does not move on any frame;
  a badly-off estimate: the same with long replies, several times a box's height;
  a landing on the box: a held reply too tall for the root to stay above the first unread,
  so the entry brings the first box to the head, and the reply that takes its place stays;
  one announcement: the boxes' status line says "8 more replies loading" once, and no box is
  a live region;
  the kill switch (negative control): with "vibe-talk.voice.thread-reply-boxes" kept "0", the
  late-count case is the old behaviour: no box, and no read of the thread when the count lands.

Pass --screenshots DIR to keep a PNG with the boxes up and one after the fill, per case.
"""

from __future__ import annotations

import argparse
import json
import os
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING, Callable

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, Json, check, handler_for, message
import page_sources
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page, Playwright

# One frame of the sampler's trace: the view, row and box counts, and the screen tops it records.
Frame = dict[str, object]

THREAD_ID = "spaces/A/threads/night"
KILL_SWITCH = "vibe-talk.voice.thread-reply-boxes"
NEW_REPLIES = 8
ROOT_ID = "206"
HELD_ID = "207"
LINE = f"{NEW_REPLIES} more replies loading"
# Generous condition timeouts for a hot host (rule 346): a wait ends on its condition.
WAIT_S = 90.0

# Per frame, whatever changed: the rows the thread list shows, the boxes, the status line, and the
# screen tops of the root, the held reply, the first box and the first new reply.
SAMPLER = r"""
(() => {
  const trace = []; window.__trace = trace; let last = '';
  const said = []; window.__said = said;
  const top = (el) => el ? Math.round(el.getBoundingClientRect().top * 100) / 100 : null;
  const row = (id) => [...document.querySelectorAll('#discord-log > li[data-ids]')]
    .find((li) => li.getAttribute('data-ids').split(' ').includes(id) && li.getClientRects().length) || null;
  let watched = null;
  const tick = () => {
    const pending = document.getElementById('pending-replies');
    if (pending && pending !== watched) {
      watched = pending;
      new MutationObserver(() => {
        const line = pending.querySelector('.pending-replies-said');
        const text = line ? line.textContent : '';
        if (said[said.length - 1] !== text) said.push(text);
      }).observe(pending, {subtree: true, childList: true, characterData: true});
    }
    const sel = document.getElementById('thread-select');
    const ids = [...document.querySelectorAll('#discord-log > li[data-id]')]
      .filter((li) => li.getClientRects().length).map((li) => li.dataset.id);
    const boxes = [...document.querySelectorAll('#pending-replies .pending-reply')];
    const f = {view: sel ? sel.value : null, rows: ids.length, boxes: boxes.length,
      root: top(row('%ROOT%')), held: top(row('%HELD%')), box: top(boxes[0]),
      seam: Math.round(((document.getElementById('channel-summary') || {getBoundingClientRect: () => ({height: 0})}).getBoundingClientRect().height) * 100) / 100,
      scroll: Math.round((document.getElementById('scroll-area') || {scrollTop: 0}).scrollTop)};
    const key = JSON.stringify(f);
    if (key !== last) { last = key; f.t = Math.round(performance.now()); trace.push(f); }
    requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
})();
""".replace("%ROOT%", ROOT_ID).replace("%HELD%", HELD_ID)


class NightApi(fixture.FakeApi):
    """A channel of six notes and one thread: its root and one reply, then eight more posted."""

    def __init__(self, long_replies: bool, tall_held: bool) -> None:
        super().__init__()
        self.long_replies = long_replies
        self.gates = {"main": threading.Event(), "thread": threading.Event()}
        for gate in self.gates.values():
            gate.set()
        self.root_thread = {"id": THREAD_ID, "root_message_id": ROOT_ID, "is_root": True,
                            "reply_count": 1, "reply_count_exact": True}
        rows = [message(index, f"Channel note {index}: routine status, filed.") for index in range(6)]
        rows.append(message(6, "Thread root: overnight run of the release lane, replies below.", self.root_thread))
        # Held replies: one, or (a landing on the box) four just under the fold's length, tall enough
        # together that the root no longer fits above the first unread.
        self.held = 4 if tall_held else 1
        for k in range(self.held):
            words = f"Reply {k + 1}, seen earlier: started the overnight run."
            if tall_held:
                words += " Then the next stage of the start ran clean." * 5
            rows.append(message(7 + k, words, {**self.root_thread, "is_root": False}))
        self.root_thread["reply_count"] = self.held
        self.messages = rows
        self.first_new = str(200 + 7 + self.held)
        self.read_ids = [ROOT_ID, *(str(200 + 7 + k) for k in range(self.held))]
        self.threads = [{"id": THREAD_ID, "root": rows[6], "title": "Overnight run", "reply_count": self.held,
                         "reply_count_exact": True, "updated_at": rows[-1]["timestamp"]}]

    def hold(self, view: str) -> None:
        self.gates[view].clear()

    def release(self, view: str) -> None:
        self.gates[view].set()

    def timeline(self, query: dict[str, list[str]]) -> Json:
        gate = self.gates.get(query.get("view", ["main"])[0])
        if gate is not None:
            gate.wait(WAIT_S)
        answer = super().timeline(query)
        # The reader dealt with the root and the replies held on the earlier visit: Done, read. Only
        # the replies posted since are unread, so the entry's first unread is the first new one.
        dismissed = answer.get("dismissed")
        answer["dismissed"] = [*(dismissed if isinstance(dismissed, list) else []), *self.read_ids]
        return answer

    def add_replies(self) -> None:
        with self.lock:
            start = len(self.messages)
            for k in range(NEW_REPLIES):
                words = f"Reply {k + 2}: overnight step {k + 2} finished. "
                words += ("The step's output, line by line, as the run wrote it. " * 14) if self.long_replies \
                    else "Details of the step. " * (k % 3 + 1)
                self.messages.append(message(start + k, words, {**self.root_thread, "is_root": False}))
            count = self.held + NEW_REPLIES
            for row in self.messages:
                thread = fixture.thread_of(row)
                if thread.get("id") == THREAD_ID:
                    thread["reply_count"] = count
            self.threads[0]["reply_count"] = count
            self.threads[0]["updated_at"] = self.messages[-1]["timestamp"]

    def thread_reads(self) -> list[str]:
        with self.lock:
            return [r for r in self.requests if "/timeline" in r and "view=thread" in r]

    def main_reads(self) -> list[str]:
        with self.lock:
            return [r for r in self.requests if "/timeline" in r and "view=main" in r]


def until(page: Page, script: str, why: str) -> None:
    deadline = time.monotonic() + WAIT_S
    while time.monotonic() < deadline:
        if page.evaluate(script):
            return
        page.wait_for_timeout(25)
    check(bool(page.evaluate(script)), why)


def settled(page: Page) -> None:
    page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")


def sign_in(page: Page) -> None:
    if page.evaluate("() => { const t = document.getElementById('api-token'); return !!t && !!t.offsetParent; }"):
        page.fill("#api-token", TOKEN)
        page.click("#save-token")
    try:
        page.wait_for_function("() => !document.getElementById('screen-welcome').hidden "
                               "|| !document.getElementById('screen-main').hidden", timeout=10_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass
    page.wait_for_selector("#search-toggle", state="visible", timeout=int(WAIT_S * 1000))


def frames(page: Page) -> list[Frame]:
    return list(page.evaluate("() => window.__trace"))


def num(frame: Frame, key: str) -> float | None:
    """The frame's number under `key`, or None where it recorded none."""
    value = frame.get(key)
    return float(value) if isinstance(value, (int, float)) else None


def dump(name: str, trace: list[Frame]) -> None:
    """With VIBE_TALK_TRACE_DIR set, keep a case's per-frame trace there (a debugging aid)."""
    where = os.environ.get("VIBE_TALK_TRACE_DIR")
    if where:
        Path(where, f"{name.replace(' ', '-')}.json").write_text(json.dumps(trace, indent=0))


def drift(trace: list[Frame], key: str, start: int) -> float:
    """The most the screen top `key` moved from its value at frame `start`, over the frames after."""
    tops = [top for top in (num(f, key) for f in trace[start:]) if top is not None]
    return max((abs(t - tops[0]) for t in tops), default=0.0)


class Case:
    def __init__(self, pw: Playwright, args: argparse.Namespace, label: str, size: tuple[int, int], scheme: str,
                 name: str, long_replies: bool = False, tall_held: bool = False, switch_off: bool = False) -> None:
        self.pw, self.args, self.label, self.size, self.scheme = pw, args, label, size, scheme
        self.name, self.switch_off = name, switch_off
        self.api = NightApi(long_replies, tall_held)
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(self.api))
        self.server.daemon_threads = True
        threading.Thread(target=self.server.serve_forever, daemon=True).start()
        self.url = f"http://127.0.0.1:{self.server.server_port}/"
        self.tag = f"{label} {name}"

    def context(self, profile: str) -> tuple[BrowserContext, Page]:
        width, height = self.size
        mobile = width < 600
        context = self.pw.chromium.launch_persistent_context(
            profile, headless=True, executable_path=self.args.browser_executable,
            viewport={"width": width, "height": height}, device_scale_factor=2.625 if mobile else 1,
            is_mobile=mobile, has_touch=mobile, color_scheme=self.scheme,  # type: ignore[arg-type]
            timezone_id="America/New_York")
        browser_waits.hot_host_timeouts(context)
        page = context.pages[0] if context.pages else context.new_page()
        page.add_init_script(SAMPLER)
        if self.switch_off:
            page.add_init_script(f"try {{ localStorage.setItem({json.dumps(KILL_SWITCH)}, '0'); }} catch (e) {{}}")
        return context, page

    def shot(self, page: Page, stage: str) -> None:
        if self.args.screenshots is not None:
            page.screenshot(path=str(self.args.screenshots / f"{self.label}-{self.name}-{stage}.png"))

    def earlier_visit(self, profile: str) -> None:
        """The device sees the thread with its root and one reply, and goes back to Main."""
        context, page = self.context(profile)
        page.goto(self.url, wait_until="load")
        sign_in(page)
        page.select_option("#thread-select", "main")
        until(page, f"() => [...document.querySelectorAll('#thread-select option')].some((o) => o.value === 'thread:{THREAD_ID}')",
              f"{self.tag}: the thread never reached the picker")
        page.select_option("#thread-select", f"thread:{THREAD_ID}")
        until(page, f"() => document.querySelectorAll('#discord-log > li[data-id]').length >= {1 + self.api.held}",
              f"{self.tag}: the earlier visit did not draw the root and its reply")
        settled(page)
        page.select_option("#thread-select", "main")
        until(page, "() => document.getElementById('thread-select').value === 'main'", f"{self.tag}: Main did not come back")
        # The device keeps its copy on settle; a beat for that settle (not a budget: nothing is timed).
        page.wait_for_timeout(1500)
        context.close()

    def close(self) -> None:
        self.server.shutdown()
        self.server.server_close()


def self_held(case: Case) -> int:
    """How many of the thread's replies the device held before the new ones."""
    return case.api.held


def late_count(case: Case, profile: str) -> str:
    """The owner's case: the thread opened from the saved copy before Main's read lands."""
    case.earlier_visit(profile)
    case.api.add_replies()
    case.api.hold("main")
    case.api.hold("thread")
    context, page = case.context(profile)
    try:
        page.goto(case.url, wait_until="load")
        sign_in(page)
        until(page, f"() => [...document.querySelectorAll('#thread-select option')].some((o) => o.value === 'thread:{THREAD_ID}')",
              f"{case.tag}: the saved copy did not offer the thread")
        # The owner's order: Main's first read is on the wire (held here) when the reader taps the
        # thread. Waited for at the fake API, never timed: tapped earlier, the page's first read
        # (reason "visible", the view on screen) goes out for the thread itself, and that is not
        # this case.
        deadline = time.monotonic() + WAIT_S
        while not case.api.main_reads() and time.monotonic() < deadline:
            page.wait_for_timeout(25)
        check(bool(case.api.main_reads()), f"{case.tag}: Main's first read never went out")
        page.select_option("#thread-select", f"thread:{THREAD_ID}")
        until(page, f"() => document.querySelectorAll('#discord-log > li[data-id]').length >= {1 + self_held(case)}",
              f"{case.tag}: the thread was not drawn from the saved copy")
        settled(page)
        check(page.evaluate("() => document.querySelectorAll('#pending-replies .pending-reply').length") == 0,
              f"{case.tag}: boxes before anything said the thread is short")
        check(not case.api.thread_reads(), f"{case.tag}: the thread was read at entry from the saved copy, "
              f"so this is not the owner's case: {case.api.thread_reads()}")
        start = len(frames(page))
        case.api.release("main")
        if case.switch_off:
            until(page, "() => performance.getEntriesByType('resource').some((e) => e.name.includes('view=main') && e.responseEnd > 0)",
                  f"{case.tag}: Main's read never landed")
            settled(page)
            page.wait_for_timeout(500)
            settled(page)
            boxes = page.evaluate("() => document.querySelectorAll('#pending-replies .pending-reply').length")
            reads = case.api.thread_reads()
            check(boxes == 0 and not reads, f"{case.tag}: with the switch off the old behaviour did not come back "
                  f"(boxes {boxes}, thread reads {reads})")
            case.api.release("thread")
            return f"{case.tag}: switch off -> old behaviour (0 boxes, no thread read when the count landed)"
        until(page, f"() => document.querySelectorAll('#pending-replies .pending-reply').length === {NEW_REPLIES}",
              f"{case.tag}: the count landed and no {NEW_REPLIES} boxes stood")
        reads = case.api.thread_reads()
        check(len(reads) == 1 and "reason=enter" in reads[0], f"{case.tag}: the count did not read the thread at once "
              f"(reason enter, not the poll): {reads}")
        rows = page.evaluate("() => document.querySelectorAll('#discord-log > li[data-id]').length")
        check(rows == 1 + self_held(case), f"{case.tag}: {rows} rows with the boxes up: a body folded before its box stood")
        # The boxes show where the first unread reply will stand, so the entry's landing — owed since
        # the thread was entered with nothing unread held — is paid on the first box now (the root to
        # the head when it fits), not when the bodies land. From here nothing may move.
        settled(page)
        settled(page)
        before = frames(page)
        landing = max(drift(before, "root", start), drift(before, "held", start))
        filling = len(before) - 1
        case.shot(page, "boxes")
        case.api.release("thread")
        until(page, f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {1 + self_held(case) + NEW_REPLIES} "
                    "&& document.querySelectorAll('#pending-replies .pending-reply').length === 0",
              f"{case.tag}: the replies did not take the boxes' places")
        settled(page)
        case.shot(page, "filled")
        trace = frames(page)
        dump(case.tag, trace[start:])
        moved = max(drift(trace, "root", filling), drift(trace, "held", filling))
        # KNOWN, NOT THIS CHANGE'S (`#2391 vibe-talk-thread-coverage-line-shifts-rows`): the coverage
        # line above the thread ("the most recent messages; older ones are not loaded") becomes
        # "10 messages from ..." when the whole thread lands, a line shorter on a phone, and with the
        # list at its top nothing can hold the rows through that. The same move happens with the
        # boxes switched off, at the poll's fill. What the fill may not add is any move of its own.
        seam = abs((num(trace[-1], "seam") or 0.0) - (num(trace[filling], "seam") or 0.0))
        own = abs(moved - seam) if num(trace[filling], "scroll") == 0 else moved
        check(own <= 0.5, f"{case.tag}: the replies' fill moved the rows {moved:.2f} px, {own:.2f} px of it its own "
              f"(the coverage line above changed {seam:.2f} px) (rule 370)")
        said = page.evaluate("() => window.__said")
        check([text for text in said if text] == [LINE], f"{case.tag}: the status line said {said}, not {LINE!r} once")
        return (f"{case.tag}: count landed -> {NEW_REPLIES} boxes + thread read at once (landing {landing:.2f} px); "
                f"fill moved the rows {own:.2f} px of its own (coverage line {seam:.2f} px, #2391); said once")
    finally:
        case.api.release("main")
        case.api.release("thread")
        context.close()


def count_at_entry(case: Case, profile: str) -> str:
    """The thread entered after Main's read: boxes in the first frame that shows it."""
    case.earlier_visit(profile)
    case.api.add_replies()
    case.api.hold("thread")
    context, page = case.context(profile)
    try:
        page.goto(case.url, wait_until="load")
        sign_in(page)
        until(page, "() => performance.getEntriesByType('resource').some((e) => e.name.includes('view=main') && e.responseEnd > 0)",
              f"{case.tag}: Main's read never landed")
        settled(page)
        start = len(frames(page))
        page.select_option("#thread-select", f"thread:{THREAD_ID}")
        until(page, f"() => document.querySelectorAll('#pending-replies .pending-reply').length === {NEW_REPLIES}",
              f"{case.tag}: entering the thread drew no {NEW_REPLIES} boxes")
        settled(page)
        trace = frames(page)
        held_rows = 1 + self_held(case)
        shown = [f for f in trace[start:] if str(f.get("view")).startswith("thread:") and (num(f, "rows") or 0) >= held_rows]
        check(bool(shown) and num(shown[0], "boxes") == NEW_REPLIES,
              f"{case.tag}: the first frame showing the thread had {shown[0]['boxes'] if shown else None} boxes")
        check(all(num(f, "rows") == held_rows for f in shown), f"{case.tag}: a body folded before the read was released")
        reads = case.api.thread_reads()
        check(len(reads) == 1 and "reason=enter" in reads[0], f"{case.tag}: entering read: {reads}")
        box_top = page.evaluate("() => document.querySelector('#pending-replies .pending-reply').getBoundingClientRect().top")
        landed = len(frames(page)) - 1
        case.shot(page, "boxes")
        case.api.release("thread")
        until(page, f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {1 + self_held(case) + NEW_REPLIES} "
                    "&& document.querySelectorAll('#pending-replies .pending-reply').length === 0",
              f"{case.tag}: the replies did not take the boxes' places")
        settled(page)
        case.shot(page, "filled")
        trace = frames(page)
        dump(case.tag, trace[start:])
        first_top = page.evaluate(f"() => {{ const li = [...document.querySelectorAll('#discord-log > li[data-ids]')]"
                                  f".find((r) => r.getAttribute('data-ids').split(' ').includes('{case.api.first_new}'));"
                                  f" return li ? li.getBoundingClientRect().top : null; }}")
        check(first_top is not None and abs(first_top - box_top) <= 0.5,
              f"{case.tag}: the first new reply stands at {first_top} where its box stood at {box_top:.2f} (first-unread landing)")
        moved = max(drift(trace, "root", landed), drift(trace, "held", landed))
        check(moved <= 0.5, f"{case.tag}: the head moved {moved:.2f} px while the replies filled the boxes (rule 370)")
        # Which row the entry brought to the head: the root while it still stands on screen in full,
        # else the first box (the root scrolled off above it).
        area_top = page.evaluate("() => document.getElementById('scroll-area').getBoundingClientRect().top")
        root_top = num(trace[landed], "root")
        head = "root" if root_top is not None and root_top >= float(area_top) else "box"
        if case.name == "box-at-head":
            check(head == "box", f"{case.tag}: the root stayed at the head; the case meant to land on the box did not")
        said = page.evaluate("() => window.__said")
        check([text for text in said if text] == [LINE], f"{case.tag}: the status line said {said}, not {LINE!r} once")
        live = page.evaluate("() => document.querySelectorAll('#pending-replies [aria-live]').length")
        check(live == 1, f"{case.tag}: {live} live regions among the boxes: one status line, none per box")
        return (f"{case.tag}: {NEW_REPLIES} boxes in the first frame; landed with the {head} at the head; first reply "
                f"stands where its box stood ({abs(first_top - box_top):.2f} px); head moved {moved:.2f} px; said once")
    finally:
        case.api.release("thread")
        context.close()


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT, help="directory holding the page to serve")
    parser.add_argument("--screenshots", type=Path, help="directory to write the PNGs into")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    from playwright.sync_api import sync_playwright
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    runs = (("dark-phone", (412, 915), "dark"), ("light-desk", (1280, 800), "light"))
    cases: tuple[tuple[str, Callable[[Case, str], str], dict[str, bool]], ...] = (
        ("late-count", late_count, {}),
        ("count-at-entry", count_at_entry, {}),
        ("long-replies", count_at_entry, {"long_replies": True}),
        ("box-at-head", count_at_entry, {"tall_held": True}),
        ("switch-off", late_count, {"switch_off": True}),
    )
    report: list[str] = []
    with sync_playwright() as pw:
        for label, size, scheme in runs:
            for name, walk, options in cases:
                case = Case(pw, args, label, size, scheme, name, **options)
                try:
                    with tempfile.TemporaryDirectory(prefix="vibe-talk-reply-boxes-") as profile:
                        report.append(walk(case, profile))
                finally:
                    case.close()
    for line in report:
        print(line)
    print("thread reply boxes browser walk: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
