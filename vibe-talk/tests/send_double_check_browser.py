#!/usr/bin/env python3
"""`#621 send-double-check`: the Sending card mid-send at 393x852, in real Chromium.

Phase one (server accept) answers at once; the upstream outcome is held, so the
card shows tick one green and tick two gray. A screenshot is kept with --screenshots.
"""
from __future__ import annotations

import argparse
import os
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path

import offline_cache_browser as fixture
from inline_reply_browser import ReplyColumnsApi, sign_in
from offline_cache_browser import check, handler_for, said
from two_column_browser import Desk

WIDTH, HEIGHT = 393, 852


class TickApi(ReplyColumnsApi):
    def __init__(self) -> None:
        super().__init__()
        self.release = threading.Event()

    def post(self, path, body):
        if path.endswith("/reply/outcome"):
            if not self.release.is_set():
                return 202, {"state": "posting", "posted": 0, "parts": 1,
                             "timing": {"server_ms": 120, "upstream_ms": None}}
            posted = said(900, str(body.get("text", "")), "me")
            return 200, {"posted": posted, "parts": [posted],
                         "timing": {"server_ms": 120, "upstream_ms": 1900}}
        if path.endswith("/reply"):
            return 202, {"state": "accepted", "op_key": str(body.get("idempotency_key", "tick")),
                         "server_ms": 120, "parts": 1}
        return super().post(path, body)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"))
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT)
    parser.add_argument("--screenshots", type=Path, default=Path("/tmp/send-double-check-shots"))
    args = parser.parse_args()
    from playwright.sync_api import sync_playwright
    args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = args.web_root.resolve()
    api = TickApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with sync_playwright() as playwright, tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-tick-") as profile:
            context = playwright.chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": WIDTH, "height": HEIGHT}, is_mobile=True, has_touch=True)
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda e: errors.append(str(e)))
            desk = Desk(page, args.screenshots, "send-double-check", WIDTH, HEIGHT)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            sign_in(desk)
            page.wait_for_selector("#channel-compose-text", state="visible", timeout=10_000)
            page.fill("#channel-compose-text", "A message held mid-send for the ticks.")
            page.click("#channel-send")
            desk.wait_for("() => document.querySelector('.outgoing-message .send-tick[data-tick=\"server\"]') !== null",
                          "the Sending card has no server tick")
            desk.settled()
            states = page.evaluate("""() => [...document.querySelectorAll('.outgoing-message .send-tick')]
                .map((t) => t.className)""")
            check(states == ["send-tick ok", "send-tick"], f"mid-send ticks wrong: {states}")
            desk.shot("1-mid-send")
            # Tap readout: durations, no clock times.
            page.evaluate("() => document.querySelector('.outgoing-message .send-ticks').click()")
            desk.settled()
            shown = page.evaluate("() => document.querySelector('.outgoing-message .send-times')?.textContent || ''")
            check("server 120 ms" in shown and "total" in shown, f"tap readout wrong: {shown}")
            check(":" not in shown.split("total")[0].replace("server", ""), f"tap readout has a clock time: {shown}")
            api.release.set()
            desk.wait_for("() => ![...document.querySelectorAll('.outgoing-message')].some((li) => li.getAttribute('data-send-state') === 'sending')",
                          "the held send did not finish")
            check(not errors, f"the page threw: {errors}")
            print(f"Chromium at {WIDTH}x{HEIGHT}: mid-send ticks {states}; tap readout {shown.strip()}; shot in {args.screenshots}")
            context.close()
        return 0
    finally:
        api.release.set()
        api.stopping.set()
        server.shutdown(); server.server_close(); thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
