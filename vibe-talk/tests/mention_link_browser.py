#!/usr/bin/env python3
"""Mention links in real Chromium: a mention that renders blue is a real link.

`#1722 vibe-talk-mention-link-not-clickable`. The server draws a user mention it can
place as an anchor to the person's Slack profile (`{workspace}/team/{user id}`, the
display name as its label), and the profile-link form Slack itself gives
(`<https://…|label>`) as an anchor to the address in the stored link. This walk serves
the checked-out page against the fake API of `offline_cache_browser.py`, whose
messages carry the server-rendered bodies, and checks at phone and desk widths:

- each form is exactly one anchor with the right href, label and new-tab target;
- hovering the mention anchor underlines it;
- tapping it opens the profile in a new tab and neither folds the row, moves the
  list's scroll position, nor makes a selection.

All waits are on drawn state, never on elapsed time.
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
import page_sources
from offline_cache_browser import TOKEN, Json, check, handler_for, said

if TYPE_CHECKING:
    from playwright.sync_api import Page

STATE_S = 30.0
PROFILE = "https://acme.slack.com/team/U0ADA0001"
STORED = "https://profiles.example/careers/profile/10700614"
LINK_ATTRS = 'target="_blank" rel="noopener noreferrer nofollow"'
MENTION_HTML = (
    f'<p>hi <a class="mention" href="{PROFILE}" {LINK_ATTRS}>@Ada Lovelace</a>, '
    f'see <a href="{STORED}" {LINK_ATTRS}>Ada Lovelace</a> for the details</p>'
)


class MentionApi(fixture.FakeApi):
    """Two messages whose rendered bodies hold a mention link and a profile link."""

    def __init__(self) -> None:
        super().__init__()
        first = said(0, "hi <@U0ADA0001>, see the profile for the details", "human")
        first["content_html"] = MENTION_HTML
        second = said(1, "Second message, so the first row has a neighbour.", "coder")
        self.messages = [first, second]
        self.threads = []


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    return parser.parse_args()


def until(page: Page, script: str, what: str) -> None:
    deadline = time.monotonic() + STATE_S
    while time.monotonic() < deadline and not page.evaluate(script):
        page.wait_for_timeout(25)
    check(bool(page.evaluate(script)), what)


def open_main(page: Page, touch: bool) -> None:
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    until(page, "() => document.querySelectorAll('#discord-log > li[data-id]').length >= 2",
          "the channel did not open with both messages")
    if touch:
        page.select_option("#thread-select", "main")
    else:
        page.click("#column-toggle")
        until(page, "() => document.getElementById('side-column').getClientRects().length > 0",
              "the desk layout did not draw two columns")


def walk(page: Page, label: str) -> str:
    mention = page.locator("#discord-log a.mention")
    check(mention.count() == 1, f"{label}: expected exactly one mention anchor, got {mention.count()}")
    check(mention.get_attribute("href") == PROFILE,
          f"{label}: mention href is {mention.get_attribute('href')}, not {PROFILE}")
    check((mention.text_content() or "").strip() == "@Ada Lovelace",
          f"{label}: mention label is {mention.text_content()!r}")
    check(mention.get_attribute("target") == "_blank", f"{label}: mention does not open in a new tab")
    stored = page.locator(f'#discord-log a[href="{STORED}"]')
    check(stored.count() == 1, f"{label}: expected exactly one stored-link anchor, got {stored.count()}")
    check((stored.text_content() or "").strip() == "Ada Lovelace",
          f"{label}: stored-link label is {stored.text_content()!r}")

    mention.hover()
    until(page, """() => {
      const a = document.querySelector('#discord-log a.mention');
      return a && getComputedStyle(a).textDecorationLine.includes('underline');
    }""", f"{label}: hovering the mention did not underline it")

    before = page.evaluate("""() => {
      const row = document.querySelector('#discord-log > li[data-id="200"]');
      const area = document.getElementById('scroll-area');
      return {collapsed: row.getAttribute('data-collapsed'), scroll: area.scrollTop,
              selection: String(getSelection())};
    }""")
    with page.context.expect_page(timeout=10_000) as opened:
        mention.click()
    profile = opened.value
    profile.wait_for_load_state("load")
    check(profile.url.startswith(PROFILE), f"{label}: the new tab went to {profile.url}, not {PROFILE}")
    after = page.evaluate("""() => {
      const row = document.querySelector('#discord-log > li[data-id="200"]');
      const area = document.getElementById('scroll-area');
      return {collapsed: row.getAttribute('data-collapsed'), scroll: area.scrollTop,
              selection: String(getSelection())};
    }""")
    check(after == before,
          f"{label}: tapping the mention changed the row or the list: {before} -> {after}")
    profile.close()
    return "one anchor per form, hover underlines, tap opens the profile and folds nothing"


def run(chromium: object, args: argparse.Namespace, label: str, width: int, height: int, touch: bool) -> str:
    api = MentionApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-mention-") as profile:
            context = chromium.launch_persistent_context(  # type: ignore[attr-defined]
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark",
                device_scale_factor=2.625 if touch else 1, is_mobile=touch, has_touch=touch)
            page = context.pages[0]
            # The workspace host is invented; answer its profile page here so the
            # new tab lands on the profile address instead of an error page.
            context.route("https://acme.slack.com/**", lambda route: route.fulfill(
                status=200, content_type="text/html", body="<title>profile</title>profile"))
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            open_main(page, touch)
            note = walk(page, label)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{label} {width}x{height}: {note}"
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    report: list[str] = []
    with sync_playwright() as playwright:
        report.append(run(playwright.chromium, args, "phone", 412, 915, True))
        report.append(run(playwright.chromium, args, "desk", 1280, 800, False))
    for line in report:
        print(line)
    print("mention_link_browser: PASSED")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
