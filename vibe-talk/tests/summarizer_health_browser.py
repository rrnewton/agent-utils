#!/usr/bin/env python3
"""Summariser health banner in real Chromium. `#1564 summarizer-loud-failure-and-fallback-chain`.

The fake API answers client-config with a degraded `summarizer_warning` naming the fallback
model in use. At phone (390x844) and desktop (1280x800): sign in, and the one persistent
health banner is visible and names the model; a healthy config shows no banner. No page
error at any step. Pass --screenshots DIR to keep a PNG of each state.
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
from offline_cache_browser import TOKEN, FakeApi, Json, handler_for
import page_sources
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

WARNING = (
    "using muse-spark-1.4-internal for message: "
    "small-elk-internal failed: the AI gateway could not be reached"
)
SIZES = (
    ("phone-390", 390, 844, True),
    ("desk-1280", 1280, 800, False),
)


class HealthApi(FakeApi):
    """Client-config degraded, naming the fallback model."""

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope), "summarizer_warning": WARNING}


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter
    )
    parser.add_argument(
        "--browser-executable",
        default=os.environ.get("VIBE_TALK_CHROMIUM"),
        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)",
    )
    parser.add_argument(
        "--web-root",
        type=Path,
        default=fixture.WEB_ROOT,
        help="directory holding the page to serve",
    )
    parser.add_argument("--screenshots", type=Path, help="directory to write PNGs into")
    return parser.parse_args()


def check(condition: bool, why: str) -> None:
    if not condition:
        raise AssertionError(why)


def walk(chromium: "BrowserType", args: argparse.Namespace, label: str,
         width: int, height: int, mobile: bool, api: FakeApi) -> str:
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{server.server_port}/"
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-health-") as profile:
            context = chromium.launch_persistent_context(
                profile,
                headless=True,
                executable_path=args.browser_executable,
                viewport={"width": width, "height": height},
                is_mobile=mobile,
                has_touch=mobile,
            )
            browser_waits.hot_host_timeouts(context)
            page = context.pages[0] if context.pages else context.new_page()
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(url, wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            try:
                page.wait_for_function("() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden", timeout=3_000)
                if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
                    page.click("#welcome-start")
            except Exception:
                pass  # no first-run Welcome on this path (#1781: a returning reader, a refusal, or a saved choice)
            # Wait on the condition (banner state follows client-config), never a budget.
            deadline = time.monotonic() + 30
            while time.monotonic() < deadline:
                shown = page.evaluate(
                    "() => { const b = document.getElementById('summarizer-health-banner');"
                    " return b && !b.hidden; }"
                )
                if shown:
                    break
                page.wait_for_timeout(50)
            text = page.evaluate(
                "() => document.getElementById('summarizer-health-detail').textContent"
            )
            check("muse-spark-1.4-internal" in str(text),
                  f"{label}: banner does not name the model in use: {text!r}")
            check(not errors, f"{label}: page errors: {errors}")
            if args.screenshots:
                page.screenshot(path=str(args.screenshots / f"{label}-health-banner.png"))
            context.close()
    finally:
        server.shutdown()
    return f"{label}: health banner names the model in use"


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
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, mobile in SIZES:
            report.append(
                walk(playwright.chromium, args, label, width, height, mobile, HealthApi())
            )
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
