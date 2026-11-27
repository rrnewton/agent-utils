#!/usr/bin/env python3
"""Reply never puts its box over the row it answers (`#2395`).

`#2395 vibe-talk-reply-box-covers-last-card`. The owner, 2026-11-27: "clicking reply on a main
channel message (which itself was a promote-to-main reply to an EARLIER thread message) put me in
the reply box but with bad scroll so my reply box is COVERING half of the message I was replying
too. ... After ANOTHER message appeared in the All channel -- so that the target of the reply was no
longer the LAST card in the history -- then the reply button places correctly."

The box drawn under its row stood docked at the list's foot (`#2063`) whenever it would end below
the foot, and nothing checked the row it answers: the last card, read anywhere short of the list's
very end, had the box drawn over it. And Reply opened at the very foot hid the column's own box
after the row was held, so the browser's scroll clamp moved every row down.

This walk serves the real page with the threaded walks' fake API: a channel whose last card in All
is a promoted (also-in-the-channel) reply to an earlier thread message. At 412x915 (dark, touch) and
1280x800 (light), it presses Reply as the reader does and checks, by geometry only (no clocks):

  the last card, plain and promoted, with the list at its end and 40 px and 120 px short of it: the
  row stands wholly above the box (its foot at or above the box's head);
  the second-to-last card with one card below: the same, and the card below is on screen or the
  Newest chip offers it;
  a card with room under it: the row does not move at all (0.5 px), the clamp included;
  the phone keyboard really up after Reply (the viewport 336 px shorter): the box never over the row;
  after the open, 30 frames with no drift (0.5 px), and a message arriving while the box is open
  moves neither the row nor the box over it.

Pass --screenshots DIR to keep a PNG per case; --web-root DIR to run it against another copy of the
page (main before the change fails the last-card cases).
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
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, Json, check, handler_for, said
import page_sources
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page, Playwright

THREAD = "spaces/A/threads/early"
LAST_ID = "212"
WAIT_S = 90.0
KEYBOARD_PX = 336
# How many animation frames the open must hold still for, once settled.
STILL_FRAMES = 30

MEASURE_JS = """(id) => {
  const box = (n) => { if (!n || !n.getClientRects().length) return null; const r = n.getBoundingClientRect();
    return {top: r.top, bottom: r.bottom, height: r.height}; };
  const rows = [...document.querySelectorAll('#discord-log > li[data-ids]')];
  const at = rows.findIndex((li) => li.getAttribute('data-ids').split(' ').includes(id));
  const row = at < 0 ? null : rows[at];
  const below = at < 0 ? null : rows.slice(at + 1).find((li) => li.getClientRects().length) || null;
  const mount = ['column-reply', 'side-reply'].map((x) => document.getElementById(x)).find((x) => !x.hidden) || null;
  const area = document.getElementById('scroll-area');
  const a = area.getBoundingClientRect();
  const jump = document.getElementById('jump-newest');
  return {row: box(row), box: box(mount), below: box(below), area: {top: a.top, bottom: a.top + area.clientHeight},
    docked: mount ? mount.hasAttribute('data-foot-docked') : false,
    newest: Boolean(jump && !jump.hidden && jump.getClientRects().length),
    scroll: area.scrollTop, max: area.scrollHeight - area.clientHeight};
}"""

# Where a row's top (its header and first two lines) ends: as the page reckons it (`answeredTopFoot`).
TOP_FOOT_JS = """(id) => {
  const row = [...document.querySelectorAll('#discord-log > li[data-ids]')]
    .find((li) => li.getAttribute('data-ids').split(' ').includes(id));
  if (!row) return null;
  const r = row.getBoundingClientRect();
  const meta = row.querySelector('.meta');
  const head = meta && meta.getClientRects().length ? meta.getBoundingClientRect().bottom : r.top;
  const style = getComputedStyle(row);
  const line = parseFloat(style.lineHeight);
  const lines = line > 0 ? line : (parseFloat(style.fontSize) * 1.4 || 20);
  return Math.min(r.bottom, head + 2 * lines);
}"""

SAMPLE_JS = """([id, frames]) => new Promise((done) => {
  const tops = [];
  const tick = () => {
    const row = [...document.querySelectorAll('#discord-log > li[data-ids]')]
      .find((li) => li.getAttribute('data-ids').split(' ').includes(id));
    tops.push(row ? row.getBoundingClientRect().top : null);
    if (tops.length >= frames) done(tops); else requestAnimationFrame(tick);
  };
  requestAnimationFrame(tick);
})"""


class Channel(fixture.FakeApi):
    """Notes, an earlier thread (root and one reply), later notes, and last a card that is either a
    plain channel message or a promoted reply of that thread; optionally one card after it."""

    def __init__(self, promoted: bool, extra: bool, tall: bool = False) -> None:
        super().__init__()
        root: Json = {"id": THREAD, "root_message_id": "204", "is_root": True,
                      "reply_count": 2 if promoted else 1, "reply_count_exact": True}
        rows = [said(i, f"Channel note {i}: routine status, filed and done. " * 2) for i in range(4)]
        rows.append({**said(4, "Thread root: the earlier plan for the release lane."), "thread": root})
        rows.append({**said(5, "Thread reply: the step that the main channel needs to hear about."),
                     "thread": {**root, "is_root": False}})
        for i in range(6, 12):
            rows.append(said(i, f"Later channel message {i}: more routine status. " * 2))
        if promoted:
            rows.append({**said(12, "Promoted to main: the step from the thread, now for everyone. " * 2),
                         "thread": {**root, "is_root": False, "broadcast": True}})
        elif tall:
            # Taller than the room above a reply box, in short lines no fold shortens.
            rows.append(said(12, "A tall last card, line by line:\n" + "\n".join(f"step {n} done" for n in range(1, 31))))
        else:
            rows.append(said(12, "A plain channel message, the last card. " * 2))
        if extra:
            rows.append(said(13, "One more card after it."))
        self.messages = rows
        self.threads = [{"id": THREAD, "root": rows[4], "title": "Earlier plan", "reply_count": root["reply_count"],
                         "reply_count_exact": True, "updated_at": rows[5]["timestamp"]}]

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope), "broadcast_supported": True}

    def arrive(self) -> None:
        with self.lock:
            self.messages.append(said(len(self.messages), "A message arriving while the reply is open."))


def num(found: Json, part: str, key: str) -> float:
    value = found.get(part)
    if not isinstance(value, dict):
        raise AssertionError(f"no {part} measured: {found}")
    got = value.get(key)
    return float(got) if isinstance(got, (int, float)) else float("nan")


def covered(found: Json) -> float:
    """How far the box stands over the row's foot, in px (0 or less: the row is wholly above it)."""
    return num(found, "row", "bottom") - num(found, "box", "top")


class Run:
    def __init__(self, pw: Playwright, args: argparse.Namespace, label: str, width: int, height: int,
                 scheme: str) -> None:
        self.pw, self.args, self.label = pw, args, label
        self.width, self.height, self.scheme = width, height, scheme
        self.mobile = width < 600

    def open(self, api: Channel, profile: str, clock: bool = False) -> tuple[BrowserContext, Page, ThreadingHTTPServer]:
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
        server.daemon_threads = True
        threading.Thread(target=server.serve_forever, daemon=True).start()
        context = self.pw.chromium.launch_persistent_context(
            profile, headless=True, executable_path=self.args.browser_executable,
            viewport={"width": self.width, "height": self.height}, device_scale_factor=2.625 if self.mobile else 1,
            is_mobile=self.mobile, has_touch=self.mobile, color_scheme=self.scheme,  # type: ignore[arg-type]
            timezone_id="America/New_York")
        browser_waits.hot_host_timeouts(context)
        page = context.pages[0] if context.pages else context.new_page()
        if clock:
            page.clock.install()
        page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
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
        page.select_option("#thread-select", "flat")
        until(page, f"() => document.querySelectorAll('#discord-log > li[data-id]').length >= {len(api.messages)}",
              f"{self.label}: All did not draw the channel")
        return context, page, server

    def park(self, page: Page, up: int) -> None:
        """The list at its end, or `up` px short of it."""
        page.evaluate("(up) => { const a = document.getElementById('scroll-area');"
                      " a.scrollTop = a.scrollHeight - a.clientHeight - up; }", up)
        settled(page)
        settled(page)

    def reply(self, page: Page, message_id: str) -> None:
        selector = f"#discord-log > li[data-id='{message_id}'] .reply-button"
        found = page.locator(selector).first.bounding_box()
        if found is None:
            page.evaluate("(selector) => document.querySelector(selector).click()", selector)
        elif self.mobile:
            page.touchscreen.tap(found["x"] + found["width"] / 2, found["y"] + found["height"] / 2)
        else:
            page.mouse.click(found["x"] + found["width"] / 2, found["y"] + found["height"] / 2)
        until(page, "() => ['column-reply', 'side-reply'].some((id) => !document.getElementById(id).hidden)",
              f"{self.label}: Reply on {message_id} opened no box")
        settled(page)
        settled(page)

    def shot(self, page: Page, name: str) -> None:
        if self.args.screenshots is not None:
            page.screenshot(path=str(self.args.screenshots / f"{self.label}-{name}.png"))


def until(page: Page, script: str, why: str) -> None:
    deadline = time.monotonic() + WAIT_S
    while time.monotonic() < deadline:
        if page.evaluate(script):
            return
        page.wait_for_timeout(25)
    check(bool(page.evaluate(script)), why)


def settled(page: Page) -> None:
    page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")


def measure(page: Page, message_id: str) -> Json:
    found = page.evaluate(MEASURE_JS, message_id)
    assert isinstance(found, dict)
    return found


def still(page: Page, message_id: str) -> float:
    """The most the row's top moves over `STILL_FRAMES` frames."""
    tops = [float(t) for t in page.evaluate(SAMPLE_JS, [message_id, STILL_FRAMES]) if isinstance(t, (int, float))]
    check(len(tops) == STILL_FRAMES, f"the row went missing while it was watched: {tops}")
    return max(abs(t - tops[0]) for t in tops)


def last_card(run: Run, promoted: bool, up: int) -> str:
    """Reply on the last card: wholly above the box, and still afterwards."""
    name = f"last-{'promoted' if promoted else 'plain'}-up{up}"
    api = Channel(promoted, extra=False)
    with tempfile.TemporaryDirectory(prefix="vibe-talk-reply-clear-") as profile:
        context, page, server = run.open(api, profile)
        try:
            run.park(page, up)
            run.reply(page, LAST_ID)
            found = measure(page, LAST_ID)
            run.shot(page, name)
            over = covered(found)
            check(over <= 0.5, f"{run.label} {name}: the box stands {over:.1f} px over the row it answers: {found}")
            check(num(found, "row", "top") >= num(found, "area", "top") - 0.5,
                  f"{run.label} {name}: the row's top is above the list: {found}")
            drift = still(page, LAST_ID)
            check(drift <= 0.5, f"{run.label} {name}: the row drifted {drift:.2f} px after the open")
            return f"{run.label} {name}: row foot {-over:.1f} px above the box; still {drift:.2f} px over {STILL_FRAMES} frames"
        finally:
            context.close()
            server.shutdown()


def second_to_last(run: Run) -> str:
    """Reply on the second-to-last card, one card below: clear of the box, the card below offered."""
    api = Channel(promoted=True, extra=True)
    with tempfile.TemporaryDirectory(prefix="vibe-talk-reply-clear-") as profile:
        context, page, server = run.open(api, profile)
        try:
            run.park(page, 0)
            run.reply(page, LAST_ID)
            found = measure(page, LAST_ID)
            run.shot(page, "second-to-last")
            over = covered(found)
            check(over <= 0.5, f"{run.label} second-to-last: the box stands {over:.1f} px over the row: {found}")
            below = found.get("below")
            # On screen and clear of the box (above it or below it), or offered by the Newest chip.
            clear = isinstance(below, dict) and (float(below["bottom"]) <= num(found, "box", "top") + 0.5 or
                                                 float(below["top"]) >= num(found, "box", "bottom") - 0.5)
            shown = isinstance(below, dict) and float(below["bottom"]) <= num(found, "area", "bottom") + 0.5
            offered = bool(found.get("newest")) or (clear and shown)
            check(offered, f"{run.label} second-to-last: the card below is neither on screen nor offered by Newest: {found}")
            return f"{run.label} second-to-last: row foot {-over:.1f} px above the box; the card below offered"
        finally:
            context.close()
            server.shutdown()


def room_below(run: Run) -> str:
    """A card with room under it for the box: nothing moves, the clamp at the list's end included."""
    api = Channel(promoted=True, extra=False)
    target = "208"
    with tempfile.TemporaryDirectory(prefix="vibe-talk-reply-clear-") as profile:
        context, page, server = run.open(api, profile)
        try:
            run.park(page, 0)
            before = measure(page, target)
            run.reply(page, target)
            found = measure(page, target)
            run.shot(page, "room-below")
            moved = abs(num(found, "row", "top") - num(before, "row", "top"))
            check(covered(found) <= 0.5, f"{run.label} room-below: the box covers the row: {found}")
            check(moved <= 0.5, f"{run.label} room-below: the row moved {moved:.1f} px with room for the box under it "
                  f"(before {before.get('row')}, after {found.get('row')})")
            return f"{run.label} room-below: row moved {moved:.2f} px (box under it, list at its end)"
        finally:
            context.close()
            server.shutdown()


def tall_row(run: Run, top_at: float) -> str:
    """A row taller than the room above the box: its body is not moved for, but its top (header and
    first lines), on screen before Reply, stays above the box; a top already clear does not move."""
    name = f"tall-top-at-{int(top_at * 100)}"
    api = Channel(promoted=False, extra=False, tall=True)
    with tempfile.TemporaryDirectory(prefix="vibe-talk-reply-clear-") as profile:
        context, page, server = run.open(api, profile)
        try:
            # Opened in full, as a reader opens a folded message: a row taller than the room.
            page.evaluate("(id) => { const row = [...document.querySelectorAll('#discord-log > li[data-ids]')]"
                          "  .find((li) => li.getAttribute('data-ids').split(' ').includes(id));"
                          " const fold = row && row.getAttribute('data-collapsed') === 'true' ? row.querySelector('.fold') : null;"
                          " if (fold) fold.click(); }", LAST_ID)
            settled(page)
            settled(page)
            page.evaluate("([id, at]) => { const a = document.getElementById('scroll-area');"
                          " const r = a.getBoundingClientRect();"
                          " const row = [...document.querySelectorAll('#discord-log > li[data-ids]')]"
                          "   .find((li) => li.getAttribute('data-ids').split(' ').includes(id));"
                          " a.scrollTop += row.getBoundingClientRect().top - (r.top + a.clientHeight * at); }",
                          [LAST_ID, top_at])
            settled(page)
            settled(page)
            before = measure(page, LAST_ID)
            run.reply(page, LAST_ID)
            found = measure(page, LAST_ID)
            run.shot(page, name)
            head = page.evaluate(TOP_FOOT_JS, LAST_ID)
            check(isinstance(head, (int, float)), f"{run.label} {name}: no row top measured")
            clear = num(found, "box", "top") - float(head)
            moved = num(before, "row", "top") - num(found, "row", "top")
            check(num(found, "row", "height") > num(found, "box", "top") - num(found, "area", "top"),
                  f"{run.label} {name}: the row is not taller than the room above the box: {found}")
            check(clear >= -0.5, f"{run.label} {name}: the box stands {-clear:.1f} px over the row's top (header and first lines)")
            covered_before = float(head) + moved - num(found, "box", "top")
            if covered_before <= 0.5:
                check(abs(moved) <= 0.5, f"{run.label} {name}: a row whose top cleared the box moved {moved:.1f} px")
            return f"{run.label} {name}: the row's top {clear:.1f} px clear of the box; moved {moved:.1f} px"
        finally:
            context.close()
            server.shutdown()


def keyboard(run: Run) -> str:
    """The phone keyboard really up after Reply: the box never over the row."""
    api = Channel(promoted=True, extra=False)
    with tempfile.TemporaryDirectory(prefix="vibe-talk-reply-clear-") as profile:
        context, page, server = run.open(api, profile)
        try:
            run.park(page, 40)
            run.reply(page, LAST_ID)
            page.set_viewport_size({"width": run.width, "height": run.height - KEYBOARD_PX})
            until(page, f"() => (window.visualViewport ? window.visualViewport.height : innerHeight) === "
                        f"{run.height - KEYBOARD_PX}", f"{run.label} keyboard: the viewport did not take the keyboard")
            settled(page)
            settled(page)
            found = measure(page, LAST_ID)
            run.shot(page, "keyboard-open")
            over = covered(found)
            check(over <= 0.5, f"{run.label} keyboard: with the keyboard up the box stands {over:.1f} px over the row: {found}")
            return f"{run.label} keyboard up: box never over the row ({-over:.1f} px clear)"
        finally:
            context.close()
            server.shutdown()


def arrival(run: Run) -> str:
    """A message arriving while the box is open: neither the row nor the box moves over it."""
    api = Channel(promoted=True, extra=False)
    with tempfile.TemporaryDirectory(prefix="vibe-talk-reply-clear-") as profile:
        context, page, server = run.open(api, profile, clock=True)
        try:
            run.park(page, 40)
            run.reply(page, LAST_ID)
            before = measure(page, LAST_ID)
            count = len(api.messages)
            api.arrive()
            page.clock.fast_forward(60_000)
            until(page, f"() => document.querySelectorAll('#discord-log > li[data-id]').length > {count}",
                  f"{run.label} arrival: the arriving message was never drawn")
            settled(page)
            settled(page)
            found = measure(page, LAST_ID)
            run.shot(page, "arrival")
            moved = abs(num(found, "row", "top") - num(before, "row", "top"))
            check(moved <= 0.5, f"{run.label} arrival: the row moved {moved:.1f} px when a message arrived")
            check(covered(found) <= 0.5, f"{run.label} arrival: the box stands over the row after the arrival: {found}")
            return f"{run.label} arrival: row moved {moved:.2f} px; box still clear of it"
        finally:
            context.close()
            server.shutdown()


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
    report: list[str] = []
    with sync_playwright() as pw:
        for label, width, height, scheme in (("dark-phone", 412, 915, "dark"), ("light-desk", 1280, 800, "light")):
            run = Run(pw, args, label, width, height, scheme)
            for promoted in (False, True):
                for up in (0, 40, 120):
                    report.append(last_card(run, promoted, up))
            report.append(second_to_last(run))
            report.append(room_below(run))
            report.append(tall_row(run, 0.72))
            report.append(tall_row(run, 0.05))
            if run.mobile:
                report.append(keyboard(run))
            report.append(arrival(run))
    for line in report:
        print(line)
    print("reply box clear browser walk: ok")
    print(json.dumps({"cases": len(report)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
