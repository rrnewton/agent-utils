#!/usr/bin/env python3
"""Pace greyed with its reason when the voice cannot be paced, in real Chromium.

`#2053 vibe-talk-pace-has-no-effect`. The owner's deployment reads through the
conversational cascade, whose wire carries no rate: the server says so in
client-config (`read_aloud.pace_supported: false`), and the page must grey the
Pace controls and say why instead of letting the reader move a control that
does nothing. A backend that can be paced (the field absent, as every capable
backend and every older server sends it) keeps the controls live.

At a 412-wide phone and a 1280-wide desk, in the light and the dark theme,
against the fake API of tests/read_aloud_views_browser.py: Read on, Pace
opened, then — for the incapable voice — the box and both steppers disabled,
the popover's help carrying the reason, and the Pace button's title and aria
carrying it too; for the capable voice, everything live. A PNG of each of the
eight popovers goes to --screenshots when a directory is named.
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
from inline_reply_browser import sign_in
from offline_cache_browser import Json, check
import page_sources
import read_aloud_views_browser
from read_aloud_views_browser import VoicesApi, handler
from two_column_browser import Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

PHONE = (412, 915)
DESK_SIZE = (1280, 800)
STATE_S = 30.0
REASON = "Pace isn't available with this voice"


class PaceApi(VoicesApi):
    """The fake voice, with or without a pace it can honour."""

    def __init__(self, pace_supported: bool) -> None:
        super().__init__()
        self.pace_supported = pace_supported

    def client_config(self, scope: str) -> Json:
        config = super().client_config(scope)
        if not self.pace_supported:
            config = {**config, "read_aloud": {
                "backend": "conversation", "label": "Check voice",
                "playback": "audio", "local_only": False, "pace_supported": False}}
        return config


def press(page: Page, selector: str, touch: bool) -> None:
    if touch:
        found = page.locator(selector).bounding_box()
        check(found is not None, f"{selector} is not drawn")
        assert found is not None
        page.touchscreen.tap(found["x"] + found["width"] / 2, found["y"] + found["height"] / 2)
    else:
        page.click(selector)


def walk(desk: Desk, touch: bool, pace_supported: bool) -> None:
    page = desk.page
    sign_in(desk)
    press(page, "#read-aloud", touch)
    desk.settled()
    # The CLOSED bar first: the tile itself is muted for a voice that cannot
    # be paced (aria-disabled at the bar's 0.45 fade), with the reason on its
    # title, and still tappable — its tap opens the popover below.
    tile = page.evaluate("() => { const tile = document.getElementById('read-speed'); "
                         "return {disabled: tile.getAttribute('aria-disabled'), "
                         "title: tile.title, opacity: getComputedStyle(tile).opacity}; }")
    if pace_supported:
        desk.check(tile["disabled"] is None and tile["opacity"] == "1",
                   f"a capable voice's Pace tile is muted: {tile}")
        desk.check(tile["title"] == "", f"a capable voice's Pace tile is titled: {tile}")
        desk.notes.append("Pace tile live on the closed bar")
    else:
        desk.check(tile["disabled"] == "true" and tile["opacity"] == "0.45",
                   f"an incapable voice's Pace tile is not muted: {tile}")
        desk.check(tile["title"] == REASON, f"the Pace tile's title is {tile['title']!r}")
        desk.notes.append("Pace tile muted on the closed bar, reason on its title")
    desk.shot("toolbar")
    # `#2216`: readiness is a condition, never a sleep — the tile says when
    # it is wired (`data-wired`, set by its mount), and the tap loop below
    # re-reads the tile's box and taps again until the popover's own
    # condition holds, bounded by the shared hang backstop. A first tap
    # lands at once, as early as the tile is drawn: if it beat the wiring,
    # the page's early-tap replay (`createEarlyTaps`) owes the popover, and
    # the loop's condition is what proves it.
    # The earliest tap: as soon as the tile is drawn, before its handler can
    # be wired. If it beat the wiring, the replay owes the popover.
    earliest = page.locator("#read-speed").bounding_box()
    check(earliest is not None, "#read-speed is not drawn at the earliest moment")
    assert earliest is not None
    if touch:
        page.touchscreen.tap(earliest["x"] + earliest["width"] / 2, earliest["y"] + earliest["height"] / 2)
    else:
        page.mouse.click(earliest["x"] + earliest["width"] / 2, earliest["y"] + earliest["height"] / 2)
    desk.wait_for("() => document.getElementById('read-speed').getAttribute('data-wired') === 'true'",
                  "the Pace tile never said it was wired", STATE_S)
    deadline = time.monotonic() + 120
    while page.evaluate("() => document.getElementById('speed-popover').hidden"):
        check(time.monotonic() < deadline, "Pace did not open its popover")
        tile_box = page.locator("#read-speed").bounding_box()
        check(tile_box is not None, "#read-speed is not drawn")
        assert tile_box is not None
        centre_x, centre_y = tile_box["x"] + tile_box["width"] / 2, tile_box["y"] + tile_box["height"] / 2
        if touch:
            page.touchscreen.tap(centre_x, centre_y)
        else:
            page.mouse.click(centre_x, centre_y)
        page.wait_for_timeout(250)
    state = "() => ({box: document.getElementById('read-speed-box').disabled, " \
            "down: document.getElementById('read-speed-down').disabled, " \
            "up: document.getElementById('read-speed-up').disabled, " \
            "help: document.getElementById('read-pace-help').textContent, " \
            "title: document.getElementById('read-speed').title, " \
            "aria: document.getElementById('read-speed').getAttribute('aria-label')})"
    deadline = time.monotonic() + STATE_S
    found = page.evaluate(state)
    expected_disabled = not pace_supported
    while time.monotonic() < deadline and found["box"] != expected_disabled:
        page.wait_for_timeout(25)
        found = page.evaluate(state)
    desk.check(found["box"] == expected_disabled and found["down"] == expected_disabled
               and found["up"] == expected_disabled,
               f"pace controls disabled={found}, expected disabled={expected_disabled}")
    if pace_supported:
        desk.check(REASON not in str(found["help"]), "a capable voice carries the greyed reason")
        desk.check(found["title"] == "", "a capable voice titles its Pace button with the reason")
        desk.check(found["aria"] is None, "a capable voice labels its Pace button with the reason")
        desk.notes.append("Pace live, no reason shown")
    else:
        desk.check(found["help"] == f"{REASON}.", f"the popover's reason is {found['help']!r}")
        desk.check(found["title"] == REASON, f"the Pace button's title is {found['title']!r}")
        desk.check(found["aria"] == f"Pace. {REASON}.", f"the Pace button's aria is {found['aria']!r}")
        desk.notes.append("Pace greyed with its reason in help, title and aria")
    desk.settled()
    desk.shot("popover")


def run(chromium: BrowserType, args: argparse.Namespace, pace_supported: bool) -> list[str]:
    api = PaceApi(pace_supported)
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    report: list[str] = []
    try:
        for touch, (width, height) in ((True, PHONE), (False, DESK_SIZE)):
            for scheme in ("light", "dark"):
                with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-pace-") as profile:
                    context = chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable,
                        viewport={"width": width, "height": height}, color_scheme=scheme,
                        has_touch=touch, is_mobile=touch)
                    version = context.browser.version if context.browser else "Chromium"
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    label = f"pace-{'enabled' if pace_supported else 'disabled'}-{scheme}-{width}"
                    desk = Desk(page, args.screenshots, label, width, height)
                    page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
                    walk(desk, touch, pace_supported)
                    check(not errors, f"{label}: the page threw: {errors}")
                    context.close()
                    report.append(f"{version} {label}: " + "; ".join(desk.notes))
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()
    return report


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write the walk's PNGs into (default: none)")
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
    report: list[str] = []
    with sync_playwright() as playwright:
        for pace_supported in (True, False):
            report.extend(run(playwright.chromium, args, pace_supported))
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
