#!/usr/bin/env python3
"""A late live read never takes away a message the stream drew after it was asked, in real Chromium at 1280x800.

`#310 stale-live-read-flicker`. A live message is drawn the moment the stream delivers it, and the page then
reads the channel again (`reason=live`). When a second live message arrives while the first message's read
is still on the wire, that read's answer, taken before the second message existed, used to land after the
stream had drawn it and redraw the list without it for a dozen frames, until the second message's own read
put it back: a message the reader saw arrive disappeared and came back. The fake-DOM suite
(tests/js/voice_page.test.mjs) pins what the page does with such a read; it has no frames, so it cannot say
whether the row was missing from any frame Chromium painted. This does, sampling every animation frame.

It serves the checked-out page and the fake API of tests/all_thread_strip_browser.py — a channel taller
than the window with three answered threads and a live stream the check writes into — with each live read
answered `LIVE_LATE_S` late, as the server had the channel when the read arrived.

At a 1280x800 desk, in All, on the newest line: a first message onto the stream; once its read has reached
the server, a second message from someone else -> the second message's row is drawn and laid out in every
frame from the one it first appears in until both reads have been answered and 30 more frames have passed;
the first message's row likewise -> both stay.

THE NEGATIVE CONTROL runs the same walk against a copy of the page whose newest pages are folded whatever
the store gained since they were sent (the `#310` fix taken out). It must miss the second message's row in
some frame: that is what proves this harness can see the flicker at all, rather than passing because the
reads were not late or not overtaken.

No page error at any step. Pass --screenshots DIR to keep a PNG of each state for review; --web-root DIR
to run the same walk against another copy of the page.
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
import time
from dataclasses import dataclass
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, urlsplit

import offline_cache_browser as fixture
from all_thread_strip_browser import StripApi, handler
from offline_cache_browser import Json, check
import page_sources
from two_column_browser import Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

# How late each live read is answered: well past the time the stream takes to draw the second message, and
# long enough that a list redrawn without it stays so for many frames.
LIVE_LATE_S = 0.3
# Frames past the last read's answer that the rows must still hold for.
AFTER_FRAMES = 30
# What the `#310` fix's control takes out of the page: the store's copies recorded as each read of a view
# goes out, over which its newest page is folded.
FIX = "      const held = heldNow(canon);\n"
UNFIXED = "      const held = null;\n"

# Watches the rows of messages `ids` at every animation frame: in how many frames since each first appeared
# it was not drawn — no row in the list, or none laid out — and in how many it was.
PROBE_JS = """(ids) => {
    const probe = {frames: 0, rows: {}, stop: false};
    for (const id of ids) probe.rows[id] = {seen: 0, missing: 0, firstAt: null, lastMissingAt: null};
    const drawn = (id) => {
        const row = document.querySelector(`#discord-log > li[data-id='${id}']`);
        return Boolean(row) && row.getClientRects().length > 0;
    };
    const step = () => {
        if (probe.stop) return;
        probe.frames += 1;
        for (const id of ids) {
            const row = probe.rows[id];
            if (drawn(id)) {
                row.seen += 1;
                if (row.firstAt === null) row.firstAt = probe.frames;
            } else if (row.firstAt !== null) {
                row.missing += 1;
                row.lastMissingAt = probe.frames;
            }
        }
        requestAnimationFrame(step);
    };
    window.staleReadProbe = probe;
    requestAnimationFrame(step);
}"""


@dataclass
class Seen:
    """What the probe saw in one walk: frames sampled, and in how many of those from the frame each row first
    appeared in it was drawn and missing."""

    version: str
    frames: int
    seen: int
    missing: int
    first_seen: int
    first_missing: int


def count(value: object) -> int:
    """A count the probe reported."""
    assert isinstance(value, (int, float)), f"the probe reported {value!r}"
    return int(value)


class LateApi(StripApi):
    """The strip check's channel, its live reads answered late, as the server had them on arrival."""

    def __init__(self) -> None:
        super().__init__()
        self.late = False
        self.live_arrived = 0


def late_handler(api: LateApi) -> type[BaseHTTPRequestHandler]:
    """The strip check's routes, with each live read, while `api.late`, answered `LIVE_LATE_S` late."""

    class Handler(handler(api)):  # type: ignore[misc]
        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            parts = urlsplit(self.path)
            query = parse_qs(parts.query)
            if not (parts.path.endswith("/timeline") and api.late and "live" in query.get("reason", [])):
                super().do_GET()
                return
            with api.lock:
                api.requests.append(f"GET {self.path}")
                api.reads_in_flight += 1
                api.live_arrived += 1
            try:
                # Taken now, handed over late: a read asked before a message existed does not hold it.
                answer = api.timeline(query)
                time.sleep(LIVE_LATE_S)
                self.json(200, answer)
            finally:
                with api.lock:
                    api.reads_in_flight -= 1
                    api.live_answered += 1

    return Handler


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--browser-executable",
        default=os.environ.get("VIBE_TALK_CHROMIUM"),
        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)",
    )
    parser.add_argument(
        "--web-root",
        type=Path,
        default=fixture.WEB_ROOT,
        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})",
    )
    parser.add_argument(
        "--screenshots",
        type=Path,
        help="directory to write one PNG per state into (default: none are kept)",
    )
    return parser.parse_args()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    root = args.web_root.resolve()
    with sync_playwright() as playwright, tempfile.TemporaryDirectory(prefix="vibe-talk-unfixed-") as copy:
        fixture.WEB_ROOT = root
        fixed = walk(playwright.chromium, args, "desk-1280")
        check(fixed.missing == 0, f"the second live message's row was not drawn in {fixed.missing} frames after it appeared: {fixed}")
        check(fixed.first_missing == 0, f"the first live message's row was not drawn in {fixed.first_missing} frames: {fixed}")
        # THE NEGATIVE CONTROL: the same walk against the page with the fix taken out.
        # Built from patched sources exactly as the page is (`#624 voice-page-modularity`); FIX must
        # occur once, or the control is refused rather than run unchanged.
        fixture.WEB_ROOT = page_sources.variant(root, Path(copy), [("voice.js", FIX, UNFIXED)])
        control = walk(playwright.chromium, args, "control-1280")
        fixture.WEB_ROOT = root
        check(control.missing > 0,
              f"the page without the fix drew the second message in every frame, so this check cannot see the flicker: {control}")
    print(f"{fixed.version} at 1280x800: two live messages, the first one's read answered {LIVE_LATE_S * 1000:.0f} ms "
          f"late after the second was drawn; the second drawn in all {fixed.seen} frames from its first, "
          f"the first in all {fixed.first_seen}; without the fix the second was missing from {control.missing} "
          f"of {control.seen + control.missing}")
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str) -> Seen:
    """One reader at a desk, against a fresh fake API and a fresh browser profile: what the probe saw."""
    api = LateApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), late_handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-stale-read-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": 1280, "height": 800}, color_scheme="dark",
            )
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, label, 1280, 800)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            found = two_late_messages(desk, api)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return Seen(version, *found)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def two_late_messages(desk: Desk, api: LateApi) -> tuple[int, int, int, int, int]:
    """Open All on its newest line, push two messages with the first one's read answered late, and say in
    how many frames each row was drawn and missing from the frame it first appeared in."""
    page = desk.page
    page.fill("#api-token", fixture.TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    desk.wait_for(f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {len(api.messages)}",
                  "the channel did not open")
    if page.evaluate("() => document.getElementById('thread-select').value") != "flat":
        page.select_option("#thread-select", "flat")
    desk.wait_for("() => document.getElementById('thread-select').value === 'flat'"
                  " && document.getElementById('discord-log').getAttribute('data-view') === 'flat'", "All did not open")
    page.evaluate("() => { const area = document.getElementById('scroll-area'); area.scrollTop = area.scrollHeight; }")
    quiet(desk, api, "before the first message")
    desk.settled()
    desk.shot("before")
    api.late = True
    # From two speakers, so each is a row of its own (`said` alternates them by index).
    first = api.add("The first live message, whose read the server answers late.")
    second_id = str(200 + api.next_index)
    page.evaluate(PROBE_JS, [str(first["id"]), second_id])
    with api.lock:
        arrived = api.live_arrived
    api.push(first)
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        with api.lock:
            if api.live_arrived > arrived:
                break
        time.sleep(0.002)
    with api.lock:
        desk.check(api.live_arrived > arrived, "the first message's read never reached the server")
    # Its read is at the server, answered with the channel as it is now: the second message comes after.
    second = api.add("The second live message, drawn while the first one's read is out.")
    desk.check(str(second["id"]) == second_id, f"the second message is {second['id']}, not {second_id}")
    api.push(second)
    desk.wait_for(f"() => Boolean(document.querySelector(\"#discord-log > li[data-id='{second_id}']\"))",
                  "the stream did not draw the second message")
    desk.shot("second-drawn")
    quiet(desk, api, "after the second message")
    api.late = False
    page.evaluate(f"""() => new Promise((done) => {{
        let frames = 0;
        const step = () => (++frames >= {AFTER_FRAMES} ? done() : requestAnimationFrame(step));
        requestAnimationFrame(step);
    }})""")
    desk.shot("after")
    probe: Json = page.evaluate("() => { window.staleReadProbe.stop = true; return window.staleReadProbe; }")
    rows = probe["rows"]
    assert isinstance(rows, dict)
    first_row, second_row = rows[str(first["id"])], rows[second_id]
    assert isinstance(first_row, dict) and isinstance(second_row, dict)
    desk.check(second_row["firstAt"] is not None, "the probe never saw the second message's row")
    desk.check(first_row["firstAt"] is not None, "the probe never saw the first message's row")
    return (count(probe["frames"]), count(second_row["seen"]), count(second_row["missing"]), count(first_row["seen"]),
            count(first_row["missing"]))


def quiet(desk: Desk, api: LateApi, why: str) -> None:
    """Wait until no read of the channel is on the wire, and a little longer for the page to draw."""
    deadline = time.monotonic() + 15
    idle_since: float | None = None
    while time.monotonic() < deadline:
        with api.lock:
            idle = api.reads_in_flight == 0
        now = time.monotonic()
        if idle:
            idle_since = idle_since if idle_since is not None else now
            if now - idle_since >= 0.4:
                return
        else:
            idle_since = None
        desk.page.wait_for_timeout(25)
    desk.check(False, f"a read of the channel was still on the wire {why}")


if __name__ == "__main__":
    raise SystemExit(main())
