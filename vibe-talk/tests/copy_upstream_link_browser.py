#!/usr/bin/env python3
"""Copy a message's own link in its service from its ⋯ menu, in real Chromium, and read the clipboard back.

`#408 copy-upstream-link`. The owner, 2026-10-21: for either Slack or Google Chat a URL can be had for a
message, and he would like to copy it from vibe-talk where it is accessible, as one of the options in a
message row's ⋯ menu. The server forms the address and carries it on the message (`upstream_url`); the
menu offers "Copy link in <service>" only where one came back. The fake-DOM suite
(tests/js/voice_page.test.mjs) pins what the page does with such a message; it lays nothing out and
touches no real clipboard, so it cannot say whether the menu holding the new item still fits a phone, or
whether what lands on the system clipboard is the address. This does, with `clipboard-read` granted to
the page's origin and the clipboard read back after every copy.

It serves the checked-out page and the fake API of tests/two_column_browser.py — a channel taller than
the window with three threads — naming Google Chat as its provider and giving the long question that
opens one thread (209) an `upstream_url`, as a served read of a Google Chat channel would. Every other
message carries none.

At 1280x800 and at 393x852, the same walk:

  the channel opened in All -> the long question's ⋯ menu opened with the mouse: "Copy link in Google
  Chat" second under Copy text, the whole menu inside the window, no sideways scroll -> the item
  pressed: the clipboard holds the address whole, the status pill says "Copied the link in Google
  Chat.", the menu is shut -> the question highlighted by the keys and a real `l` pressed through CDP:
  the address again -> a message with no address: its ⋯ menu offers no such item, and `l` on it
  highlighted copies nothing.

No page error at any step. Pass --screenshots DIR to keep a PNG of the open menu at each size.
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, Json, check, handler_for
from two_column_browser import ColumnsApi, Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, CDPSession, Page

# (label, width, height, phone). The phone is the owner's: 393x852.
SIZES = (("desk-1280", 1280, 800, False), ("phone-393", 393, 852, True))
# The long question that opens a thread in the fake channel, and the address the fake server gives it:
# its own in Google Chat, as a served read would carry it.
QUESTION_ID = "209"
PLAIN_ID = "200"
ADDRESS = "https://chat.google.com/room/AAQAJ9CXYcg/ZpNybdmNiAw/ZpNybdmNiAw"
# What the clipboard holds before each copy: a copy of nothing leaves it there.
SENTINEL = "copy-upstream-link-browser: the clipboard before the copy"

# One row's ⋯ menu: its items in order, its box, and whether the page scrolls sideways.
MENU_JS = """(id) => {
    const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
    const menu = row ? row.querySelector('.row-more-menu') : null;
    const rect = menu ? menu.getBoundingClientRect() : null;
    return {
        items: menu ? [...menu.querySelectorAll('button')].filter((one) => !one.hidden)
            .map((one) => ({className: one.className, text: one.textContent})) : [],
        open: menu ? !menu.hidden : false,
        left: rect ? rect.left : null,
        right: rect ? rect.right : null,
        innerWidth: window.innerWidth,
        overflow: document.documentElement.scrollWidth - window.innerWidth,
        status: document.getElementById('status').textContent,
    };
}"""


class LinkedApi(ColumnsApi):
    """The columns channel as Google Chat, its long question carrying its own address there."""

    def __init__(self) -> None:
        super().__init__()
        self.provider_name = "Google Chat"
        for row in self.messages:
            if row["id"] == QUESTION_ID:
                row["upstream_url"] = ADDRESS
        # The thread summaries hold their root by reference, as the fake builds them, so the root a
        # card carries is this same row.


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--browser-executable",
        default=os.environ.get("VIBE_TALK_CHROMIUM"),
        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)",
    )
    parser.add_argument(
        "--web-root",
        type=Path,
        default=fixture.WEB_ROOT,
        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})",
    )
    parser.add_argument(
        "--screenshots",
        type=Path,
        help="directory to write one PNG per state into (default: none are kept)",
    )
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
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = args.web_root.resolve()
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, phone in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, phone))
    for line in report:
        print(line)
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, phone: bool) -> str:
    """One reader at one size, against a fresh fake API and a fresh browser profile."""
    api = LinkedApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    origin = f"http://127.0.0.1:{server.server_port}"
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-link-") as profile:
            if phone:
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="dark",
                    device_scale_factor=2.625, is_mobile=True, has_touch=True,
                )
            else:
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="dark",
                )
            context.grant_permissions(["clipboard-read", "clipboard-write"], origin=origin)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            reader = Reader(Desk(page, args.screenshots, label, width, height), context.new_cdp_session(page))
            page.goto(f"{origin}/", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            page.click("#view-switch")
            reader.desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24",
                                 "the channel did not open in All")
            reader.desk.settled()
            reader.walk()
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(reader.desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


class Reader:
    """The page and the keyboard plugged into it, at one size."""

    def __init__(self, desk: Desk, cdp: CDPSession) -> None:
        self.desk = desk
        self.cdp = cdp

    @property
    def page(self) -> Page:
        return self.desk.page

    def key(self, name: str) -> None:
        """Press one key (`l`, `j`, `Home`, `Escape`) through CDP, down and up, as a keyboard sends it."""
        named = {"Home": ("Home", 36), "Escape": ("Escape", 27)}
        if name in named:
            code, vk = named[name]
            text = ""
        else:
            code, vk, text = f"Key{name.upper()}", ord(name.upper()), name
        down: Json = {"type": "keyDown" if text else "rawKeyDown", "modifiers": 0,
                      "key": name, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk}
        if text:
            down["text"] = text
            down["unmodifiedText"] = text
        self.cdp.send("Input.dispatchKeyEvent", down)
        self.cdp.send("Input.dispatchKeyEvent", {"type": "keyUp", "modifiers": 0, "key": name, "code": code,
                                                 "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk})
        self.desk.settled()

    def menu(self, message_id: str) -> Json:
        found: Json = self.page.evaluate(MENU_JS, message_id)
        return found

    def clipboard(self) -> str:
        return str(self.page.evaluate("() => navigator.clipboard.readText()"))

    def copied_after(self, act: str) -> str:
        """The clipboard once it changes from the sentinel put there for `act`, or after a second."""
        self.page.evaluate("(text) => navigator.clipboard.writeText(text)", SENTINEL)
        check(self.clipboard() == SENTINEL, f"{self.desk.label}: the sentinel did not reach the clipboard")
        if act == "l":
            self.key("l")
        for _ in range(40):
            now = self.clipboard()
            if now != SENTINEL:
                return now
            self.page.wait_for_timeout(25)
        return self.clipboard()

    def check(self, condition: bool, why: str) -> None:
        self.desk.check(condition, why)

    def highlight(self, message_id: str) -> None:
        """Home, then j until message `message_id` is the one highlighted."""
        self.key("Home")
        for _ in range(30):
            lit = self.page.evaluate(
                "() => [...document.querySelectorAll('[data-selected=\"true\"]')]"
                ".map((row) => row.getAttribute('data-id'))")
            if lit == [message_id]:
                return
            self.key("j")
        raise AssertionError(f"{self.desk.label}: j never highlighted message {message_id}")

    def walk(self) -> None:
        page = self.page
        menu_button = f"#discord-log > li[data-id='{QUESTION_ID}'] .row-more-button"
        page.locator(menu_button).scroll_into_view_if_needed()
        page.locator(menu_button).click()
        self.desk.settled()
        opened = self.menu(QUESTION_ID)
        items = opened["items"]
        assert isinstance(items, list)
        self.check(bool(opened["open"]), f"the ⋯ menu did not open: {opened}")
        self.check([item["className"] for item in items[:2]] == ["row-copy-button", "row-copy-link-button"],
                   f"the link is not second under Copy text: {items}")
        self.check(items[1]["text"] == "Copy link in Google Chat", f"the item is not named for its service: {items}")
        self.check(float(str(opened["left"])) >= 0 and float(str(opened["right"])) <= float(str(opened["innerWidth"])),
                   f"the open menu runs off the window: {opened}")
        self.check(float(str(opened["overflow"])) <= 0, "the page scrolls sideways with the menu open")
        self.desk.shot("1-menu")
        self.desk.notes.append("the ⋯ menu offered \"Copy link in Google Chat\" second under Copy text, "
                               "wholly inside the window")

        page.locator(f"#discord-log > li[data-id='{QUESTION_ID}'] .row-copy-link-button").click()
        self.desk.settled()
        for _ in range(40):
            if self.clipboard() == ADDRESS:
                break
            page.wait_for_timeout(25)
        shut = self.menu(QUESTION_ID)
        self.check(self.clipboard() == ADDRESS, f"the item copied {self.clipboard()!r}, not the address")
        self.check(shut["status"] == "Copied the link in Google Chat.", f"the copy was not said: {shut}")
        self.check(not shut["open"], "the ⋯ menu stayed open after copying")
        self.desk.notes.append("the item put the message's Google Chat address on the clipboard whole and said so")

        # The same by the keys: highlight the question, press l.
        self.highlight(QUESTION_ID)
        self.check(self.copied_after("l") == ADDRESS, "l on the highlighted question did not copy its link")
        self.check(self.menu(QUESTION_ID)["status"] == "Copied the link in Google Chat.", "l did not say it copied")
        self.desk.notes.append("l on the highlighted question copied the same address")

        # A message the server formed no address for: no item, and l is nothing.
        plain_button = f"#discord-log > li[data-id='{PLAIN_ID}'] .row-more-button"
        page.locator(plain_button).scroll_into_view_if_needed()
        page.locator(plain_button).click()
        self.desk.settled()
        plain = self.menu(PLAIN_ID)
        plain_items = plain["items"]
        assert isinstance(plain_items, list)
        self.check(all(item["className"] != "row-copy-link-button" for item in plain_items),
                   f"a message with no address was offered one: {plain_items}")
        self.key("Escape")
        self.highlight(PLAIN_ID)
        self.check(self.copied_after("l") == SENTINEL, "l on a message with no address changed the clipboard")
        self.check(float(str(self.menu(PLAIN_ID)["overflow"])) <= 0, "the page scrolls sideways")
        self.desk.shot("2-no-link")
        self.desk.notes.append("a message with no address offered no item, and l on it copied nothing")


if __name__ == "__main__":
    raise SystemExit(main())
