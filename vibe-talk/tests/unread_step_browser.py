#!/usr/bin/env python3
"""Shift+Down and Shift+Up through only the unread messages of a mixed list, in real Chromium.

`#1314 unread-step-keys`. The owner: "when I use arrow keys I step up and down by one message...
If I press shift-up / shift-down it currently does nothing. I would like to use shift-up/down to
navigate ONLY unread messages. This makes it easier for me to clean out the unreads in a thread."

Its own step rather than more of keyboard-browser, which is at its CPU limit: the fake API's
channel with every message but three marked Done on the server (its own dismissals route), so
the list the keys walk is mostly read. At 1280x800: Home selects the oldest row; Shift+Down
visits the three unread in order, Shift+Up comes back the same way, and at either end the
selection stays where it is and the status line says there are no more; plain arrows afterwards
still step one message, onto a read one. The keys are pressed through CDP
`Input.dispatchKeyEvent`, the path a hardware keyboard takes.

What it cannot show: whether a real Shift+Arrow chord reaches the page past an operating
system's or browser's own use of it is not something CDP can see.
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

import keyboard_browser as kb
from keyboard_browser import TOKEN, Json, Walk, check, press, ring, ringed
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import Page


FIRST_ROW = '#discord-log > li[data-id]'


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=page_sources.served_root(),
                        help="the page assets to serve (default: this checkout's vibe-talk/web)")
    parser.add_argument("--screenshots", type=Path, default=None,
                        help="a directory to keep one PNG per step in (default: none kept)")
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
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)

    api = kb.FakeApi()
    # The mixed list: every channel message but three already Done (read) on the server. The
    # three left unread stand further apart than a screenful, since a landing marks the rows
    # the reader can see as read.
    keep = {"880000000000000005", "880000000000000025", "880000000000000045"}
    api.dismiss({"messages": [str(m["id"]) for m in api.messages
                              if str(m["id"]) not in keep]})
    server = ThreadingHTTPServer(("127.0.0.1", 0), kb.handler_for(api, page_sources.served(args.web_root)))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/"

    try:
        with sync_playwright() as playwright, tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-") as profile:
            context = playwright.chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": 1280, "height": 800},
            )
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            cdp = context.new_cdp_session(page)
            walk = Walk(page, cdp, args.screenshots)

            page.goto(url, wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
            page.click("#view-switch")
            page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)
            page.evaluate(kb.RECORD_KEYS_JS)
            walk.settled()
            walk.shot("loaded")

            oldest = ringed(walk, "Home", "unread steps: Home")
            check(str(oldest["id"]).endswith("0000"),
                  f"unread steps: Home did not select the oldest message: {oldest}")
            for chord, want in (("Shift+ArrowDown", "0005"), ("Shift+ArrowDown", "0025"), ("Shift+ArrowDown", "0045")):
                row = ringed(walk, chord, f"unread steps: {chord} to {want}")
                check(str(row["id"]).endswith(want),
                      f"unread steps: {chord} selected {row['id']}, not the unread {want}")
            press(walk.cdp, "Shift+ArrowDown")
            walk.wait_for("() => document.getElementById('status').textContent.startsWith('No more unread below.')",
                          "unread steps: the last unread below did not say so on the status line")
            walk.settled()
            check(str(ring(walk)["id"]).endswith("0045"),
                  "unread steps: the selection moved past the last unread")
            for chord, want in (("Shift+ArrowUp", "0025"), ("Shift+ArrowUp", "0005")):
                row = ringed(walk, chord, f"unread steps: {chord} to {want}")
                check(str(row["id"]).endswith(want),
                      f"unread steps: {chord} selected {row['id']}, not the unread {want}")
            press(walk.cdp, "Shift+ArrowUp")
            walk.wait_for("() => document.getElementById('status').textContent.startsWith('No more unread above.')",
                          "unread steps: the first unread above did not say so on the status line")
            walk.settled()
            check(str(ring(walk)["id"]).endswith("0005"),
                  "unread steps: the selection moved above the first unread")
            # Plain arrows are unchanged: one message at a time, onto a read one and back.
            row = ringed(walk, "ArrowDown", "unread steps: a plain arrow after them")
            check(str(row["id"]).endswith("0006"),
                  f"unread steps: a plain ArrowDown selected {row['id']}, not the next message")
            row = ringed(walk, "ArrowUp", "unread steps: a plain arrow back")
            check(str(row["id"]).endswith("0005"),
                  f"unread steps: a plain ArrowUp selected {row['id']}, not the message above")

            check(not errors, f"the page threw: {errors}")
            browser_version = context.browser.version if context.browser else "Chromium"
            context.close()

        print(f"{browser_version} at 1280x800, keys through CDP Input.dispatchKeyEvent: "
              "Shift+Down and Shift+Up visited only the unread messages of a mixed list in order, "
              "stopped with a hint at either end, and plain arrows still stepped one message")
        return 0
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
