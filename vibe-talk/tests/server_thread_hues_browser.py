#!/usr/bin/env python3
"""Two browsers, two different colour memories, one server: the same threads in the same colours.

`#427 server-thread-hues`. Before it, each browser chose thread colours itself and remembered them
in its own localStorage (`#396 thread-color-memory`), so the owner's phone and desk could draw the
same thread differently and neither was wrong. Now the server decides, once, and every message and
thread card the page receives carries its thread's hue. The fake-DOM suite (tests/js/voice_page
.test.mjs) pins what the page does with those hues; this shows what two readers actually see in real
Chromium, reading each thread's colour back from the badge drawn for it.

The page is served against a fake API holding six threads, one root and one reply each, whose
"server" assigns a thread's hue the first time anybody decides it and stamps it on every record and
card from then on, as the real one does. Two persistent browser contexts — two devices — open the
channel in All, each with its own localStorage memory of the six threads in DIFFERENT colours, as
two browsers that coloured them independently before `#427` would hold.

The first device offers its colours before its first read (the `#427` migration) and the server
takes them; the second offers its own, takes nothing, and is handed the first's. Both must draw
every thread in the first device's colours — the ones the owner already saw — and a reload of the
second device, with no offer left to make, must draw them still.

No page error in either context. Pass --screenshots DIR to keep a PNG from each device.
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
    from playwright.sync_api import BrowserContext, BrowserType

WIDTH, HEIGHT = 1280, 800
THREADS = 6
HUES_KEY = "vibe-talk.thread-hues"
# The page keeps a channel's colours under its provider and id; this fixture's channel names no provider.
SCOPE = json.dumps(["", CHANNEL["id"]], separators=(",", ":"))
TOPICS = ("the overnight runner", "the reporting replica", "the canary rollout",
          "the flaky shard", "the artifact upload", "the release notes")

# Each root's id and the hue its badge was drawn in, top to bottom.
BADGES_JS = """() => [...document.querySelectorAll('#discord-log > li[data-id]')].map((row) => {
    const badge = row.querySelector('.thread-badge');
    return {id: row.getAttribute('data-id'),
            hue: badge ? badge.style.getPropertyValue('--thread-hue') : ''};
}).filter((row) => row.hue !== '')"""


def thread_id(index: int) -> str:
    return f"spaces/A/threads/t{index + 1:02d}"


class ServerHueApi(FakeApi):
    """Six threads, and a server's one word on their colours: first decider wins, forever."""

    def __init__(self) -> None:
        super().__init__()
        self.hues: dict[str, float] = {}
        self.offers: list[Json] = []
        self.messages = []
        self.threads = []
        roots: list[Json] = []
        records: list[Json] = []
        for i in range(THREADS):
            record: Json = {"id": thread_id(i), "root_message_id": str(200 + i), "is_root": True,
                            "reply_count": 1, "reply_count_exact": True}
            root = message(i, f"Thread {i + 1}: can somebody look at {TOPICS[i]}?", record)
            roots.append(root)
            records.append(record)
            self.messages.append(root)
        for i in range(THREADS):
            reply = message(THREADS + i, f"Looking at {TOPICS[i]} now.", {**records[i], "is_root": False})
            self.messages.append(reply)
            self.threads.append({"id": thread_id(i), "root": roots[i], "title": f"Thread {i + 1}",
                                  "reply_count": 1, "reply_count_exact": True,
                                  "updated_at": reply["timestamp"]})

    def stamp(self) -> None:
        """Put the server's colours on every record and card, as the real server serves them."""
        for held in self.messages:
            raw = held.get("thread")
            if not isinstance(raw, dict):
                continue
            thread: Json = raw
            if str(thread.get("id")) in self.hues:
                thread["hue"] = self.hues[str(thread["id"])]
        for summary in self.threads:
            if str(summary["id"]) in self.hues:
                summary["hue"] = self.hues[str(summary["id"])]

    def post(self, path: str, body: Json) -> tuple[int, Json]:
        if not path.endswith("/thread-hues"):
            return super().post(path, body)
        with self.lock:
            self.offers.append(body)
            seeded = 0
            offered = body.get("hues")
            for one in offered if isinstance(offered, list) else []:
                if not isinstance(one, dict):
                    continue
                tid = str(one.get("thread_id"))
                hue = one.get("hue")
                # First writer wins, exactly as the store holds it; whole degrees, rounded.
                if tid not in self.hues and isinstance(hue, int | float):
                    self.hues[tid] = float(round(hue) % 360)
                    seeded += 1
            self.stamp()
            return 200, {"channel": CHANNEL, "seeded": seeded, "hues": dict(self.hues)}


def remembered(offset: float) -> dict[str, float]:
    """One device's own colours for the six threads, as its page handed them out on its own."""
    return {thread_id(i): (offset + i * 47.0) % 360 for i in range(THREADS)}


def open_device(chromium: BrowserType, args: argparse.Namespace, origin: str, memory: dict[str, float],
                profile: str, shot: str | None) -> tuple[dict[str, float], str, BrowserContext]:
    """One device: its own profile and colour memory, the channel in All, its badges' hues."""
    yesterday = int((time.time() - 24 * 60 * 60) * 1000)
    stored = {"v": 1, "channels": {SCOPE: {tid: [hue, yesterday] for tid, hue in memory.items()}}}
    context = chromium.launch_persistent_context(
        profile, headless=True, executable_path=args.browser_executable,
        viewport={"width": WIDTH, "height": HEIGHT}, color_scheme="dark",
    )
    version = context.browser.version if context.browser else "Chromium"
    context.add_init_script(
        f"if (!localStorage.getItem({json.dumps(HUES_KEY)})) "
        f"localStorage.setItem({json.dumps(HUES_KEY)}, {json.dumps(json.dumps(stored))});")
    page = context.pages[0]
    errors: list[str] = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(f"{origin}/", wait_until="load")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    deadline = time.monotonic() + 10
    badges: list[Json] = []
    while time.monotonic() < deadline:
        badges = [row for row in page.evaluate(BADGES_JS) if int(str(row["id"])) < 200 + THREADS]
        if len(badges) >= THREADS:
            break
        page.wait_for_timeout(25)
    check(len(badges) == THREADS, f"All drew {len(badges)} thread roots, not {THREADS}")
    if shot and args.screenshots:
        page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")
        page.screenshot(path=str(args.screenshots / shot))
    check(not errors, f"the page threw: {errors}")
    hues = {thread_id(int(str(row["id"])) - 200): float(str(row["hue"])) for row in badges}
    return hues, version, context


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
        help="directory to write one PNG per device into (default: none is kept)",
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
    """Two devices against one fresh fake API, each remembering the threads differently."""
    api = ServerHueApi()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler_for(api))
    server.daemon_threads = True
    serving = threading.Thread(target=server.serve_forever, daemon=True)
    serving.start()
    origin = f"http://127.0.0.1:{server.server_port}"
    first_memory = remembered(200.0)
    second_memory = remembered(20.0)
    check(first_memory != second_memory, "the premise: the two devices remember different colours")
    contexts: list[BrowserContext] = []
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-server-hues-a-") as profile_a, \
                tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-server-hues-b-") as profile_b:
            first, version, context_a = open_device(
                chromium, args, origin, first_memory, profile_a, "first-device.png")
            contexts.append(context_a)
            second, _version, context_b = open_device(
                chromium, args, origin, second_memory, profile_b, "second-device.png")
            contexts.append(context_b)
            # A reload of the second device: no offer left to make, the same colours still.
            page_b = context_b.pages[0]
            page_b.reload(wait_until="load")
            page_b.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            deadline = time.monotonic() + 10
            reloaded: list[Json] = []
            while time.monotonic() < deadline:
                reloaded = [row for row in page_b.evaluate(BADGES_JS)
                            if int(str(row["id"])) < 200 + THREADS]
                if len(reloaded) >= THREADS:
                    break
                page_b.wait_for_timeout(25)
            check(len(reloaded) == THREADS, f"the reload drew {len(reloaded)} thread roots")
            again = {thread_id(int(str(row["id"])) - 200): float(str(row["hue"])) for row in reloaded}
            for context in contexts:
                context.close()
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        serving.join()

    expected = {tid: float(round(hue) % 360) for tid, hue in first_memory.items()}
    check(len(api.offers) == 2, f"the colours were offered {len(api.offers)} times, not once per device")
    check(first == expected, f"the first device's own colours did not survive: drew {first}, offered {expected}")
    check(second == expected,
          f"the second device drew its own memory over the server's: drew {second}, the server's are {expected}")
    check(again == expected, f"a reload of the second device recoloured the threads: {again}")
    drawn = ", ".join(f"{expected[thread_id(i)]:g}" for i in range(THREADS))
    return (f"{version} at {WIDTH}x{HEIGHT}: two devices remembering the six threads differently both drew "
            f"them in the first device's colours, {drawn}, offered once and taken by the server; a reload "
            "of the second device drew them still")


if __name__ == "__main__":
    raise SystemExit(main())
