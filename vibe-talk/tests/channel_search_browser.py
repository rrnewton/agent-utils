#!/usr/bin/env python3
"""Finding a Slack channel by typing its name without the '#', in real Chromium.

`#371 channel-search-hash`. The owner, 2026-10-19, could not find a Slack channel by typing part of its
name. Slack writes every channel `#name`. The bar's channel picker is a native `<select>`, and a native
picker finds an option by the first letters of its text as they are typed, so an option reading
`#team-builds` was never reached by typing `team`. The fake-DOM suite (tests/js/voice_page.test.mjs)
pins the option text the page writes, but only a real picker's type-ahead proves that typing reaches
the channel. This does, with keys pressed through CDP `Input.dispatchKeyEvent`:

  1280x800, one Slack service with `#general` and `#team-builds`, signed in on `#general` -> the
  picker focused, as Tab leaves it, and typing t, e, a, m chooses `#team-builds`, which is then read
  (Chromium's open list finds an option by the same rule, its text's first letters; a list opened by
  Ctrl+K is drawn outside the page, where CDP's keys do not reach it headless) -> Settings,
  Channels unfolded, Browse channels: typing `Builds` into its search lists `#team-builds-archive`, and
  `#ops` lists `#eng-ops`; the first search is slow, and while it is on its way the line under the box
  says how many seconds it has taken.

The fake API answers the directory as the server does since the same change: a leading '#' is
optional and case does not matter. --web-root DIR runs it against another copy of the page; against
one from before the change it fails at the first check, typing `team` into the picker.
--screenshots DIR keeps a PNG of each step.
"""

from __future__ import annotations

import argparse
import re
import tempfile
import threading
import time
from dataclasses import dataclass
from http.server import ThreadingHTTPServer
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, unquote, urlsplit

import keyboard_browser as kb
from keyboard_browser import CHANNEL, SECOND, TOKEN, Json, check
import page_sources

if TYPE_CHECKING:
    from pathlib import Path

    from playwright.sync_api import BrowserContext, CDPSession, Page, Playwright

GENERAL = {**CHANNEL, "label": "#general"}
BUILDS = {**SECOND, "label": "#team-builds"}
DIRECTORY = ["#team-builds-archive", "#eng-ops", "#ops-private", "#random"]
# How long the fake takes over the first directory search, as Slack takes to list its channels.
SLOW_SECONDS = 2.5
FIRST_ROW = "#discord-log > li[data-id]"


@dataclass
class SearchApi(kb.FakeApi):
    """keyboard_browser's channel, on one Slack service that registers and lists channels."""

    directory_reads: int = 0

    def client_config(self) -> Json:
        return {**super().client_config(), "chat_provider_name": "Slack", "channels": [GENERAL, BUILDS],
                "channel_registration_supported": True, "channel_discovery_supported": True}

    def directory(self, query: dict[str, list[str]]) -> Json:
        with self.lock:
            self.directory_reads += 1
            first = self.directory_reads == 1
        if first:
            time.sleep(SLOW_SECONDS)
        wanted = query.get("q", [""])[0].strip().lstrip("#").strip().lower()
        names = [name for name in DIRECTORY if wanted in name.lower()]
        return {"entries": [{"source": f"C0000DIR{n:02d}", "name": name, "tracked": False, "channel_id": None}
                            for n, name in enumerate(names)],
                "next_cursor": None, "truncated": False}


def handler_for(api: SearchApi, web_root: Path) -> type:
    base = kb.handler_for(api, web_root)

    class Handler(base):  # type: ignore[valid-type,misc]
        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            parts = urlsplit(self.path)
            if unquote(parts.path) == "/api/v1/channel-directory" and \
                    self.headers.get("Authorization") == f"Bearer {TOKEN}":
                self.json(200, api.directory(parse_qs(parts.query)))
            else:
                super().do_GET()

    return Handler


def picked(page: Page) -> Json:
    found = page.evaluate("""() => {
      const picker = document.getElementById('discord-channel');
      return {value: picker.value, options: [...picker.options].map((o) => o.textContent),
              focus: document.activeElement ? document.activeElement.id : null};
    }""")
    assert isinstance(found, dict)
    return found


def shot(page: Page, shots: Path | None, name: str) -> None:
    if shots is not None:
        page.screenshot(path=str(shots / f"channel-search-{name}.png"))


def sign_in(page: Page, url: str) -> None:
    page.goto(url, wait_until="load")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    if page.evaluate("() => document.getElementById('view-switch').getAttribute('aria-checked')") != "true":
        page.click("#view-switch")
    page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)


def rows(page: Page) -> list[str]:
    found = page.evaluate(
        "() => [...document.querySelectorAll('#channel-directory-list .directory-name')].map((n) => n.textContent)")
    assert isinstance(found, list)
    return [str(name) for name in found]


def search(page: Page, text: str, expected: list[str], why: str) -> None:
    page.fill("#channel-directory-search", text)
    page.press("#channel-directory-search", "Enter")
    try:
        page.wait_for_function("(names) => JSON.stringify([...document.querySelectorAll("
                               "'#channel-directory-list .directory-name')].map((n) => n.textContent)) "
                               "=== JSON.stringify(names)", arg=expected, timeout=10_000)
    except Exception as error:  # noqa: BLE001 - say what was listed instead of a bare timeout
        raise AssertionError(f"{why}: searching {text!r} listed {rows(page)}, not {expected}") from error


def desk(context: BrowserContext, url: str, shots: Path | None) -> str:
    page = context.pages[0]
    cdp: CDPSession = context.new_cdp_session(page)
    sign_in(page, url)
    check(picked(page)["options"] == ["general", "team-builds"],
          f"the picker lists {picked(page)['options']}, not the names without their '#'")
    found = picked(page)
    check(found["value"] == GENERAL["id"], f"signed in on {found['value']!r}, not #general")

    # The picker focused, then the channel's name typed without its '#'.
    page.evaluate("() => document.getElementById('discord-channel').focus()")
    for key in "team":
        kb.press(cdp, key)
    page.wait_for_timeout(300)
    found = picked(page)
    check(found["value"] == BUILDS["id"],
          f"typing 'team' into the picker left it on {found['value']!r}; it lists {found['options']}")
    page.wait_for_function("(id) => [...document.querySelectorAll('#discord-log > li[data-id]')].length > 0 "
                           "&& document.getElementById('discord-channel').value === id",
                           arg=str(BUILDS["id"]), timeout=10_000)
    shot(page, shots, "picker-typed")

    # Settings: Browse channels, and a search typed in a case and without the '#'.
    page.click("#open-settings")
    page.wait_for_selector("#close-settings", state="visible", timeout=10_000)
    if page.get_attribute("#fold-settings-channels", "aria-expanded") != "true":
        page.click("#fold-settings-channels")
    page.wait_for_selector("#open-browse-channels", state="visible", timeout=10_000)
    page.click("#open-browse-channels")
    page.wait_for_selector("#channel-directory-search", state="visible", timeout=10_000)
    page.wait_for_function("() => /\\d s so far\\./.test(document.getElementById('channel-directory-state').textContent)",
                           timeout=10_000)
    waiting = str(page.text_content("#channel-directory-state"))
    check(re.match(r"Loading channels… \d s so far\.", waiting) is not None,
          f"while the first read was slow the line said {waiting!r}")
    shot(page, shots, "browse-waiting")
    page.wait_for_function("() => document.querySelectorAll('#channel-directory-list .directory-name').length === 4",
                           timeout=10_000)
    search(page, "Builds", ["#team-builds-archive"], "a part of the name, in another case")
    search(page, "#ops", ["#eng-ops", "#ops-private"], "with the '#'")
    shot(page, shots, "browse-searched")
    return (f"typing 'team' into the focused picker chose #team-builds; Browse said '{waiting}' while the "
            "first read was slow, then 'Builds' found #team-builds-archive and '#ops' found #eng-ops")


def browse(playwright: Playwright, args: argparse.Namespace, url: str) -> tuple[str, str]:
    with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-search-") as profile:
        context = playwright.chromium.launch_persistent_context(
            profile, headless=True, executable_path=args.browser_executable,
            viewport={"width": 1280, "height": 800},
        )
        errors: list[str] = []
        context.pages[0].on("pageerror", lambda error: errors.append(str(error)))
        note = desk(context, url, args.screenshots)
        check(not errors, f"the page threw: {errors}")
        version = context.browser.version if context.browser else "Chromium"
        context.close()
    return version, note


def main() -> int:
    args = kb.arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    # The fixture answers a channel's reads by its id; these are its two, named as Slack names them.
    kb.CHANNELS.update({str(GENERAL["id"]): GENERAL, str(BUILDS["id"]): BUILDS})

    api = SearchApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api, page_sources.served(args.web_root)))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/"
    try:
        with sync_playwright() as playwright:
            version, note = browse(playwright, args, url)
        print(f"{version}, keys through CDP Input.dispatchKeyEvent: at 1280x800 {note}")
        return 0
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
