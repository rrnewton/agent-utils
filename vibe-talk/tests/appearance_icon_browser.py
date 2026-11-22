#!/usr/bin/env python3
"""The thread picker's glyph stays visible under every Appearance choice, in real Chromium.

`#1861 vibe-talk-dark-bar-icon-invisible`. The picker's glyph is a scheme-keyed SVG
(`#thread-select` in web/styles/voice/18-control-bar.css): a pale stroke for a dark button, a
dark stroke for a light one. A forced Appearance that disagrees with the device scheme used to
leave the wrong glyph (or none) on the button. For each device scheme and each Appearance
choice, the glyph stroke's contrast against the button's background must clear 3:1.

Run: python3 tests/appearance_icon_browser.py [--screenshots DIR]
"""

from __future__ import annotations

import argparse
import re
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, FakeApi, check, handler_for
import browser_waits

STATE_MS = 30_000
# (device scheme, stored Appearance, expected effective theme)
CASES = (
    ("dark", "light", "light"),
    ("dark", "dark", "dark"),
    ("dark", "system", "dark"),
    ("light", "dark", "dark"),
    ("light", "light", "light"),
    ("light", "system", "light"),
)


def luminance(rgb: tuple[int, int, int]) -> float:
    def channel(value: int) -> float:
        scaled = value / 255
        return scaled / 12.92 if scaled <= 0.03928 else ((scaled + 0.055) / 1.055) ** 2.4

    red, green, blue = (channel(part) for part in rgb)
    return 0.2126 * red + 0.7152 * green + 0.0722 * blue


def contrast(one: tuple[int, int, int], two: tuple[int, int, int]) -> float:
    high, low = sorted((luminance(one), luminance(two)), reverse=True)
    return (high + 0.05) / (low + 0.05)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--screenshots", type=Path, default=None)
    parser.add_argument("--browser-executable", default=None)
    args = parser.parse_args()
    from playwright.sync_api import sync_playwright

    api = FakeApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{server.server_port}/"
    try:
        with sync_playwright() as playwright:
            for device, stored, want in CASES:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-appearance-icon-") as profile:
                    context = playwright.chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable,
                        viewport={"width": 412, "height": 915}, color_scheme="light" if device == "light" else "dark",
                        device_scale_factor=2.6, is_mobile=True, has_touch=True,
                    )
                    browser_waits.hot_host_timeouts(context)
                    page = context.pages[0]
                    page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.voice.appearance', '{stored}'); }} catch (e) {{}}"
                    )
                    page.goto(url, wait_until="load")
                    page.fill("#api-token", TOKEN)
                    page.click("#save-token")
                    try:
                        page.wait_for_function(
                            "() => !document.getElementById('screen-welcome').hidden"
                            " || !document.getElementById('screen-main').hidden",
                            timeout=3000,
                        )
                        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
                            page.click("#welcome-start")
                    except Exception:
                        pass
                    page.wait_for_selector("#thread-select", state="visible", timeout=STATE_MS)
                    page.wait_for_function(
                        f"() => document.documentElement.getAttribute('data-theme') === '{want}'",
                        timeout=STATE_MS,
                    )
                    drawn = page.evaluate("""() => {
                      const picker = document.getElementById('thread-select');
                      const style = getComputedStyle(picker);
                      return { image: style.backgroundImage, background: style.backgroundColor };
                    }""")
                    match = re.search(r"stroke='%23([0-9a-fA-F]{6})'", drawn["image"] or "")
                    if match is None:
                        raise AssertionError(f"{device}+{stored}: the picker has no glyph: {drawn}")
                    hexes = match.group(1)
                    stroke: tuple[int, int, int] = (int(hexes[0:2], 16), int(hexes[2:4], 16), int(hexes[4:6], 16))
                    rgba = re.findall(r"[\d.]+", drawn["background"] or "")
                    if len(rgba) < 3:
                        raise AssertionError(f"{device}+{stored}: no button background: {drawn}")
                    background: tuple[int, int, int] = (int(float(rgba[0])), int(float(rgba[1])), int(float(rgba[2])))
                    ratio = contrast(stroke, background)
                    check(ratio >= 3.0,
                          f"{device}+{stored}: glyph {stroke} on {background} is {ratio:.2f}:1, under 3:1")
                    print(f"{device}+{stored} -> {want}: glyph {stroke} on {background}, {ratio:.2f}:1")
                    if args.screenshots:
                        args.screenshots.mkdir(parents=True, exist_ok=True)
                        page.screenshot(path=str(args.screenshots / f"bar-{device}-{stored}.png"))
                    context.close()
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
    print("appearance icon: every case contrasts")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
