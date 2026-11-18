#!/usr/bin/env python3
"""`#1606 message-rerender-loses-formatting`: a re-rendered block post at 390px and on a desk.

The server fixture answers with the exact row the live instance served for Slack ts
1794762378.392739 after the fix: the stored (flattened, double-converted) text beside the
rendered body the unit tests in src/slack/mrkdwn.rs pin. On screen the bullets must be on
separate lines and GH #329 must be a link's label, at phone width and at desk width alike.
A screenshot is kept with --screenshots.
"""
from __future__ import annotations

import argparse
import os
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, FakeApi, check, handler_for, message
from typing import TYPE_CHECKING

if TYPE_CHECKING:
    from playwright.sync_api import Playwright
from two_column_browser import Desk
import page_sources

STORED = (
    "[lane coordinator] _What's open on GitHub from alice, bot-two and "
    "bot-three (example/widgets):_ 43 issues and 5 PRs. These look new or active: "
    "• <[https://example.com/widgets/issues/329 GH]"
    "(https://example.com/widgets/issues/329%7CGH) #329> (yours, 12:03 PM ET "
    "today): the Documents view is empty apart from one broken entry. "
    "• <[https://example.com/widgets/pull/322 PR]"
    "(https://example.com/widgets/pull/322%7CPR) #322> container build "
    "(Rust path deps).\n\nI'm sending coordb all of this."
)
FIXED_HTML = (
    "<p><strong>What's open on GitHub from alice, bot-two and bot-three "
    "(example/widgets):</strong> 43 issues and 5 PRs. These look new or active:</p>\n"
    "<ul>\n"
    '<li><a href="https://example.com/widgets/issues/329" target="_blank" '
    'rel="noopener noreferrer nofollow">GH #329</a> (yours, 12:03 PM ET today): the '
    "Documents view is empty apart from one broken entry.</li>\n"
    '<li><a href="https://example.com/widgets/pull/322" target="_blank" '
    'rel="noopener noreferrer nofollow">PR #322</a> container build '
    "(Rust path deps).</li>\n</ul>\n<p>I'm sending coordb all of this.</p>\n"
)


class RerenderApi(FakeApi):
    def __init__(self) -> None:
        super().__init__()
        row = message(1, STORED)
        row["content_html"] = FIXED_HTML
        self.messages = [row]
        self.threads = []


def walk(playwright: Playwright, web_root: Path, shots: Path, width: int, height: int, mobile: bool) -> None:
    api = RerenderApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-rerender-") as profile:
            context = playwright.chromium.launch_persistent_context(
                profile, headless=True,
                executable_path=os.environ.get("VIBE_TALK_CHROMIUM"),
                viewport={"width": width, "height": height},
                is_mobile=mobile, has_touch=mobile)
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda e: errors.append(str(e)))
            desk = Desk(page, shots, f"rerender-{width}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            try:
                page.wait_for_function("() => !document.getElementById('screen-welcome').hidden", timeout=3_000)
                page.click("#welcome-start")
            except Exception:
                pass  # no first-run Welcome on this path (#1781: a returning reader, a refusal, or a saved choice)
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            # `#1781`: sign-in lands on the channel already.
            desk.wait_for(
                "() => document.querySelectorAll('.md li').length === 2",
                "the re-rendered bullets are not separate list items")
            if mobile:
                # The phone folds a long row; open it whole so the bullets are on screen.
                page.tap(".md")
                desk.wait_for(
                    "() => { const li = document.querySelector('.md li'); "
                    "const r = li && li.getBoundingClientRect(); "
                    "return Boolean(r && r.top >= 0 && r.top < innerHeight); }",
                    "the opened message's bullets are not on screen")
            desk.settled()
            found = page.evaluate("""() => {
              const items = [...document.querySelectorAll('.md li')];
              const link = document.querySelector('.md li a');
              return {
                items: items.length,
                tops: items.map((li) => Math.round(li.getBoundingClientRect().top)),
                label: link ? link.textContent : '',
                href: link ? link.getAttribute('href') : '',
                text: (document.querySelector('.md')?.textContent || ''),
              };
            }""")
            check(found["items"] == 2, f"bullets not separate items: {found}")
            check(found["tops"][1] > found["tops"][0], f"bullets share a line: {found}")
            check(found["label"] == "GH #329", f"link label wrong: {found}")
            check(found["href"] == "https://example.com/widgets/issues/329",
                  f"link address wrong: {found}")
            check("<https" not in found["text"], f"raw link markup on screen: {found}")
            check("%7C" not in found["text"], f"escaped pipe on screen: {found}")
            desk.shot("1-rerendered")
            check(not errors, f"the page threw: {errors}")
            print(f"Chromium at {width}x{height}: {found['items']} bullets on separate "
                  f"lines, GH #329 is a link label; shot in {shots}")
            context.close()
    finally:
        api.stopping.set()
        server.shutdown(); server.server_close(); thread.join()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT)
    parser.add_argument("--screenshots", type=Path,
                        default=Path("/tmp/message-rerender-shots"))
    args = parser.parse_args()
    from playwright.sync_api import sync_playwright
    args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    with sync_playwright() as playwright:
        walk(playwright, args.web_root, args.screenshots, 390, 844, True)
        walk(playwright, args.web_root, args.screenshots, 1280, 800, False)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
