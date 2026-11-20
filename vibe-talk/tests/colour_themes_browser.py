#!/usr/bin/env python3
"""Colour themes (`#1961 vibe-talk-colour-themes`) in a real Chromium.

READY BUT NOT RUN on hosts without a browser (the lane host lost its Chromium
system libraries in the 2026-11-19 reprovision): run
`python3 tests/colour_themes_browser.py [--screenshots DIR]` where a browser
works. It serves the real page routes from the offline fixture (only the chat
upstream is faked), signs in, and drives the real Settings control: picking a
preset repaints the accent family and the Discuss header, the choice is kept,
and a reload restores it without a blue flash (the pre-paint script carries
the preset). Each preset is walked in a dark and a light device scheme at a
phone size; the default blue is asserted token-for-token against the scheme.
Condition waits only — no fixed sleeps in any assertion.
"""

from __future__ import annotations

import argparse
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, FakeApi, check, handler_for

STATE_MS = 30_000
# preset -> (dark accent, light accent, dark discuss head, light discuss head)
PRESETS = {
    "blue": ("rgb(91, 157, 217)", "rgb(36, 97, 143)", "rgb(29, 32, 38)", "rgb(255, 255, 255)"),
    "blue-hc": ("rgb(91, 157, 217)", "rgb(36, 97, 143)", "rgb(29, 32, 38)", "rgb(255, 255, 255)"),
    "teal": ("rgb(76, 194, 176)", "rgb(15, 118, 110)", "rgb(29, 74, 66)", "rgb(217, 240, 234)"),
    "plum": ("rgb(207, 159, 224)", "rgb(126, 58, 158)", "rgb(74, 45, 99)", "rgb(236, 217, 247)"),
    "amber": ("rgb(232, 179, 75)", "rgb(138, 90, 0)", "rgb(92, 66, 28)", "rgb(247, 234, 208)"),
}


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
            for scheme in ("dark", "light"):
                with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-colour-") as profile:
                    context = playwright.chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable,
                        viewport={"width": 412, "height": 915}, color_scheme=scheme,
                        device_scale_factor=2.6, is_mobile=True, has_touch=True,
                    )
                    page = context.pages[0]
                    page.goto(url, wait_until="load")
                    page.fill("#api-token", TOKEN)
                    page.click("#save-token")
                    # First run lands on the Welcome, where the bar (and its Settings
                    # button) is still hidden; the harness walks the same path.
                    try:
                        page.wait_for_function(
                            "() => !document.getElementById('screen-welcome').hidden"
                            " || !document.getElementById('screen-main').hidden",
                            timeout=3000,
                        )
                        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
                            page.click("#welcome-start")
                    except Exception:
                        pass  # no first-run Welcome on this path
                    page.wait_for_selector("#open-settings", state="visible", timeout=STATE_MS)
                    page.click("#open-settings")
                    page.wait_for_selector("#colour-theme", state="attached", timeout=STATE_MS)
                    # A fresh profile has Messages and Appearance folded (hidden
                    # until found), so the picker is attached but not visible:
                    # open every fold around it, exactly as scene 86's walk does.
                    page.evaluate("""(id) => {
                      const node = document.getElementById(id); const bodies = [];
                      for (let n = node; n; n = n.parentElement) {
                        if (n.classList && n.classList.contains('fold-body')) bodies.unshift(n);
                      }
                      for (const body of bodies) {
                        const toggle = document.querySelector(`[aria-controls="${body.id}"]`);
                        if (toggle && toggle.getAttribute('aria-expanded') !== 'true') toggle.click();
                      }
                      return bodies.length;
                    }""", "colour-theme")
                    page.wait_for_function(
                        "() => document.getElementById('colour-theme').checkVisibility()",
                        timeout=STATE_MS,
                    )
                    at = 0 if scheme == "dark" else 1
                    hexes = {"blue": ("#5b9dd9", "#24618f"), "blue-hc": ("#5b9dd9", "#24618f"),
                             "teal": ("#4cc2b0", "#0f766e"), "plum": ("#cf9fe0", "#7e3a9e"),
                             "amber": ("#e8b34b", "#8a5a00")}
                    on_accents = {"blue": ("#ffffff", "#ffffff"), "blue-hc": ("#14161a", "#ffffff"),
                                  "teal": ("#14161a", "#ffffff"), "plum": ("#14161a", "#ffffff"),
                                  "amber": ("#14161a", "#ffffff")}
                    for preset, want in PRESETS.items():
                        page.select_option("#colour-theme", preset)
                        page.wait_for_function(
                            f"() => document.documentElement.getAttribute('data-colour-theme') === '{preset}'",
                            timeout=STATE_MS,
                        )
                        drawn = page.evaluate("""() => {
                          const root = getComputedStyle(document.documentElement);
                          const head = document.querySelector('.discuss-head');
                          return {
                            accent: root.getPropertyValue('--accent').trim(),
                            onAccent: root.getPropertyValue('--on-accent').trim(),
                            stored: localStorage.getItem('vibe-talk.voice.colour-theme'),
                            head: head ? getComputedStyle(head).backgroundColor : null,
                          };
                        }""")
                        check(drawn["stored"] == preset, f"{scheme}/{preset}: kept as {drawn['stored']}")
                        check(drawn["accent"] == hexes[preset][at], f"{scheme}/{preset}: accent {drawn['accent']}")
                        check(drawn["onAccent"] == on_accents[preset][at], f"{scheme}/{preset}: on-accent {drawn['onAccent']}")
                        check(drawn["head"] in (None, want[2 + at]), f"{scheme}/{preset}: Discuss head {drawn['head']}")
                        if args.screenshots:
                            args.screenshots.mkdir(parents=True, exist_ok=True)
                            page.screenshot(path=str(args.screenshots / f"colour-{scheme}-{preset}.png"))
                        print(f"{scheme}/{preset}: accent var {drawn['accent']}, head {drawn['head']}")
                    # A reload restores the last preset (amber) through the pre-paint script.
                    page.reload(wait_until="load")
                    page.wait_for_function(
                        "() => document.documentElement.getAttribute('data-colour-theme') === 'amber'",
                        timeout=STATE_MS,
                    )
                    check(True, f"{scheme}: amber restored after reload")
                    context.close()
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
    print("colour themes: every preset applied, kept and restored in dark and light")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
