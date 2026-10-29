#!/usr/bin/env python3
"""Eight threads posted in a row, after a day of remembered ones, drawn in clearly different colours in real Chromium.

`#414 thread-hue-spread`. The owner, 2026-10-21: three threads whose roots were consecutive in time came
out in VERY similar colours. Since `#396 thread-color-memory` a channel remembers two dozen thread
colours, and a new thread takes the middle of the widest arc they leave, which can be 15 degrees wide
and right beside the threads drawn either side of it. The fake-DOM suite (tests/js/voice_page.test.mjs)
pins the colours the page hands out; it paints nothing, so it cannot show what a reader sees. This does,
reading each thread's colour back from the badge Chromium drew.

The page is served against a fake API holding 32 threads, one root and one reply each, the roots a
minute apart. Before it loads, the browser profile is given what an earlier day's page left in
localStorage: colours for the 24 oldest, handed out as the page before `#414` hands them out — each the
widest arc's middle, so the last eight sweep round the wheel 22.5 degrees at a time. The eight newest
threads are new to the page.

At 1280x800: the channel opened in All, scrolled to its newest roots -> the 24 remembered threads keep
the colours stored for them -> each of the eight new threads is at least 45 degrees of hue from every
thread within three places of it in root order, remembered or new. Against a page from before `#414`
(--web-root) the screenshot is still taken and the check then fails, naming the closest pair.

No page error. Pass --screenshots DIR to keep a PNG of the newest roots, taken 1300 pixels tall so the
last two remembered threads and all eight new ones are in it.
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
from offline_cache_browser import CHANNEL, TOKEN, FakeApi, Json, check, handler_for, message
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

WIDTH, HEIGHT = 1280, 800
SHOT_HEIGHT = 1300
REMEMBERED = 24
NEW = 8
APART = 45
NEIGHBOURS = 3
HUES_KEY = "vibe-talk.thread-hues"
# The page keeps a channel's colours under its provider and id; this fixture's channel names no provider.
SCOPE = json.dumps(["", CHANNEL["id"]], separators=(",", ":"))
TOPICS = ("the overnight runner", "the reporting replica", "the canary rollout", "the flaky shard",
          "the artifact upload", "the release notes", "the staging deploy", "the cache warmup")

# Each root's id and thread id and the hue its badge was drawn in, top to bottom.
BADGES_JS = """() => [...document.querySelectorAll('#discord-log > li[data-id]')].map((row) => {
    const badge = row.querySelector('.thread-badge');
    return {id: row.getAttribute('data-id'),
            hue: badge ? badge.style.getPropertyValue('--thread-hue') : ''};
}).filter((row) => row.hue !== '')"""


def thread_id(index: int) -> str:
    return f"spaces/A/threads/t{index + 1:02d}"


def widest_middle(given: list[float]) -> float:
    """The colour the page before `#414` gives the next thread: the middle of the widest arc left."""
    ordered = sorted(given)
    hue, widest = 210.0, -1.0
    for i, at in enumerate(ordered):
        following = ordered[i + 1] if i + 1 < len(ordered) else ordered[0] + 360
        if following - at > widest:
            widest = following - at
            hue = (at + widest / 2) % 360
    return hue


def roots_drawn(badges: list[Json]) -> list[Json]:
    """The rows of `BADGES_JS` that are thread roots: the replies follow every root, from id 200 + 32."""
    return [row for row in badges if int(str(row["id"])) < 200 + REMEMBERED + NEW]


def apart(a: float, b: float) -> float:
    gap = abs(a - b) % 360
    return min(gap, 360 - gap)


class SpreadApi(FakeApi):
    """32 threads, one root and one reply each, the roots a minute apart and the replies after them all."""

    def __init__(self) -> None:
        super().__init__()
        total = REMEMBERED + NEW
        self.messages = []
        self.threads = []
        roots: list[Json] = []
        records: list[Json] = []
        for i in range(total):
            record: Json = {"id": thread_id(i), "root_message_id": str(200 + i), "is_root": True,
                            "reply_count": 1, "reply_count_exact": True}
            root = message(i, f"Thread {i + 1}: can somebody look at {TOPICS[i % len(TOPICS)]}?", record)
            roots.append(root)
            records.append(record)
            self.messages.append(root)
        for i in range(total):
            reply = message(total + i, f"Looking at {TOPICS[i % len(TOPICS)]} now.", {**records[i], "is_root": False})
            self.messages.append(reply)
            self.threads.append({"id": thread_id(i), "root": roots[i], "title": f"Thread {i + 1}",
                                 "reply_count": 1, "reply_count_exact": True, "updated_at": reply["timestamp"]})


def remembered_hues() -> dict[str, float]:
    """What a page before `#414` left stored for the 24 oldest threads, drawn in root order."""
    hues: dict[str, float] = {}
    for i in range(REMEMBERED):
        hues[thread_id(i)] = widest_middle(list(hues.values()))
    return hues


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
        help="directory to write the PNG of the newest roots into (default: none is kept)",
    )
    parser.add_argument(
        "--port",
        type=int,
        default=0,
        help="local TCP port to serve the page and fake API on (default: 0, any free port)",
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
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    with sync_playwright() as playwright:
        print(walk(playwright.chromium, args))
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace) -> str:
    """One reader, against a fresh fake API and a browser profile holding an earlier day's colours."""
    api = SpreadApi()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler_for(api))
    server.daemon_threads = True
    serving = threading.Thread(target=server.serve_forever, daemon=True)
    serving.start()
    origin = f"http://127.0.0.1:{server.server_port}"
    stored = remembered_hues()
    yesterday = int((time.time() - 24 * 60 * 60) * 1000)
    memory = {"v": 1, "channels": {SCOPE: {tid: [hue, yesterday] for tid, hue in stored.items()}}}
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-hues-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": WIDTH, "height": HEIGHT}, color_scheme="dark",
            )
            version = context.browser.version if context.browser else "Chromium"
            context.add_init_script(
                f"if (!localStorage.getItem({json.dumps(HUES_KEY)})) "
                f"localStorage.setItem({json.dumps(HUES_KEY)}, {json.dumps(json.dumps(memory))});")
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(f"{origin}/", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            page.click("#view-switch")
            total = REMEMBERED + NEW
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and len(roots_drawn(page.evaluate(BADGES_JS))) < total:
                page.wait_for_timeout(25)
            badges = roots_drawn(page.evaluate(BADGES_JS))
            check(len(badges) == total, f"All drew {len(badges)} thread roots, not {total}")
            page.evaluate("(id) => document.querySelector(`#discord-log > li[data-id='${id}']`)"
                          ".scrollIntoView({block: 'start'})", str(200 + REMEMBERED - 2))
            page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")
            if args.screenshots:
                # Tall enough for the last two remembered roots and all eight new ones in one picture.
                page.set_viewport_size({"width": WIDTH, "height": SHOT_HEIGHT})
                page.evaluate("(id) => document.querySelector(`#discord-log > li[data-id='${id}']`)"
                              ".scrollIntoView({block: 'start'})", str(200 + REMEMBERED - 2))
                page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")
                page.screenshot(path=str(args.screenshots / f"desk-{WIDTH}-newest-roots.png"))
            check(not errors, f"the page threw: {errors}")
            context.close()
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        serving.join()

    hues = {thread_id(int(str(row["id"])) - 200): float(str(row["hue"])) for row in badges}
    for tid, hue in stored.items():
        check(hues[tid] == hue, f"{tid}, remembered as {hue}, was drawn in {hues[tid]}")
    order = [thread_id(i) for i in range(total)]
    new = order[REMEMBERED:]
    closest = (360.0, "", "")
    for tid in new:
        at = order.index(tid)
        for other in order[max(0, at - NEIGHBOURS):at + NEIGHBOURS + 1]:
            if other != tid and apart(hues[tid], hues[other]) < closest[0]:
                closest = (apart(hues[tid], hues[other]), tid, other)
    drawn = ", ".join(f"{hues[tid]:g}" for tid in new)
    # Asked where it can be had, not promised: the closest of each new thread to the one right before it.
    beside = min(apart(hues[order[order.index(tid) - 1]], hues[tid]) for tid in new)
    check(closest[0] >= APART,
          f"{closest[1]} ({hues[closest[1]]:g}) is {closest[0]:g} degrees from {closest[2]} "
          f"({hues[closest[2]]:g}), under {APART}; the eight new threads were drawn in {drawn}")
    return (f"{version} at {WIDTH}x{HEIGHT}: the {REMEMBERED} remembered threads kept their colours; the "
            f"{NEW} new ones were drawn in {drawn}, the closest pair within {NEIGHBOURS} places "
            f"{closest[0]:g} degrees apart ({closest[1]}, {closest[2]}), each at least {beside:g} degrees "
            "from the thread right before it")


if __name__ == "__main__":
    raise SystemExit(main())
