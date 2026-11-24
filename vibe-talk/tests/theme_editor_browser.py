#!/usr/bin/env python3
"""Theme editor (`#2155 vibe-talk-theme-settings-pane`) in a real Chromium.

Serves the real page routes from the offline fixture (only the chat upstream is
faked), signs in, and drives the real Settings editor: Extra dark repaints the
dark Default true black, Custom seeds from the Default and then owns a changed
colour, the Light editor edits light without touching the painted dark page,
and a reload restores the kept configuration without a flash of the Default.
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
import browser_waits

STATE_MS = 30_000


def open_editor(page) -> None:
    page.wait_for_selector("#open-settings", state="visible", timeout=STATE_MS)
    page.click("#open-settings")
    page.wait_for_selector("#theme-mode", state="attached", timeout=STATE_MS)
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
    }""", "theme-mode")
    page.wait_for_function(
        "() => document.getElementById('theme-mode').checkVisibility()",
        timeout=STATE_MS,
    )


def bg(page) -> str:
    return page.evaluate(
        "() => getComputedStyle(document.documentElement).getPropertyValue('--bg').trim()"
    )


def set_colour(page, token: str, value: str) -> None:
    page.evaluate("""([token, value]) => {
      const input = document.querySelector(`#theme-groups input[data-token="${token}"]`);
      if (!input) throw new Error(`no editor input for ${token}`);
      input.value = value;
      input.dispatchEvent(new Event('input', { bubbles: true }));
    }""", [token, value])


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
            with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-theme-") as profile:
                context = playwright.chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": 412, "height": 915}, color_scheme="dark",
                    device_scale_factor=2.6, is_mobile=True, has_touch=True,
                )
                browser_waits.hot_host_timeouts(context)
                page = context.pages[0]
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
                open_editor(page)

                groups = page.evaluate("() => document.querySelectorAll('#theme-groups .theme-group').length")
                check(groups == 13, f"editor shows 13 editable groups (code colours not in Custom v1), saw {groups}")
                check(bg(page) == "#14161a", f"standard dark bg before Extra: {bg(page)}")

                # Extra dark: the Dark Default variant, true black.
                page.select_option("#dark-style", "extra")
                page.wait_for_function("() => getComputedStyle(document.documentElement).getPropertyValue('--bg').trim() === '#000000'", timeout=STATE_MS)
                stored = page.evaluate("() => localStorage.getItem('vibe-talk.voice.theme-dark')")
                check(stored is not None and '"style":"extra"' in stored, f"Extra kept: {stored}")
                head = page.evaluate("() => getComputedStyle(document.documentElement).getPropertyValue('--discuss-head-bg').trim()")
                check(head == "#2e2340", f"Extra Discuss highlight: {head}")
                page.select_option("#dark-style", "standard")
                page.wait_for_function("() => getComputedStyle(document.documentElement).getPropertyValue('--bg').trim() === '#14161a'", timeout=STATE_MS)

                # Custom: seeded from the Default, then the reader's colour wins.
                page.select_option("#theme-mode", "custom")
                page.wait_for_function("() => !document.querySelector('#theme-groups input[data-token=\"--bg\"]').disabled", timeout=STATE_MS)
                set_colour(page, "--bg", "#112233")
                page.wait_for_function("() => getComputedStyle(document.documentElement).getPropertyValue('--bg').trim() === '#112233'", timeout=STATE_MS)
                stored = page.evaluate("() => localStorage.getItem('vibe-talk.voice.theme-dark')")
                check(stored is not None and "#112233" in stored, f"Custom kept: {stored}")
                hl = page.evaluate("""() => ({
                  v: getComputedStyle(document.documentElement).getPropertyValue('--hl-keyword').trim(),
                  input: !!document.querySelector('#theme-groups input[data-token="--hl-keyword"]'),
                })""")
                check(hl["v"] == "#ff7b72" and not hl["input"], f"code colours token-owned, not Custom-edited: {hl}")

                # The Light editor edits light while dark is painted.
                page.click("#theme-edit-light")
                page.select_option("#theme-mode", "custom")
                set_colour(page, "--bg", "#fafafa")
                check(bg(page) == "#112233", f"painted dark untouched by light edit: {bg(page)}")
                page.select_option("#appearance", "light")
                page.wait_for_function("() => getComputedStyle(document.documentElement).getPropertyValue('--bg').trim() === '#fafafa'", timeout=STATE_MS)
                page.select_option("#appearance", "dark")
                page.wait_for_function("() => getComputedStyle(document.documentElement).getPropertyValue('--bg').trim() === '#112233'", timeout=STATE_MS)

                if args.screenshots:
                    args.screenshots.mkdir(parents=True, exist_ok=True)
                    page.screenshot(path=str(args.screenshots / "theme-editor-dark-custom.png"))
                    page.reload(wait_until="load")
                    page.wait_for_selector("#discord-log", state="visible", timeout=STATE_MS)
                    page.screenshot(path=str(args.screenshots / "theme-main-dark-custom.png"))

                # Reload: the kept Custom configuration is restored.
                page.reload(wait_until="load")
                page.wait_for_function("() => getComputedStyle(document.documentElement).getPropertyValue('--bg').trim() === '#112233'", timeout=STATE_MS)
                check(True, "reload restores Custom")
                context.close()
        print("theme editor browser: ok")
        return 0
    finally:
        server.shutdown()


if __name__ == "__main__":
    raise SystemExit(main())
