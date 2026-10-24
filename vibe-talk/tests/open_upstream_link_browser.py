#!/usr/bin/env python3
"""Open a message in its service from its ⋯ menu, in real Chromium, and read the new page back.

`#408 copy-upstream-link` as `#409 open-upstream-link` leaves it. The owner, 2026-10-22: "can't we JUST
have a hyperlink that says 'Open in Slack' and then you can right-click / copy link it in the browser?"
The server forms the address and carries it on the message (`upstream_url`); the menu holds one real
hyperlink to it, "Open in <service>", only where an address came back, with "Copy <service> link" under it (`#461`). The fake-DOM
suite (tests/js/voice_page.test.mjs) pins the link's presence, absence and attributes; it lays nothing out,
opens nothing and touches no real clipboard, so it cannot say whether the menu still fits a phone, whether
the link opens a new page holding nothing of this one, or whether the `l` key's copy lands whole. This
does, with `clipboard-read` granted to the page's origin and the clipboard read back after the key.

It serves the checked-out page and the fake API of tests/two_column_browser.py — a channel taller than
the window with three threads — naming Slack as its provider and giving the long question that opens
one thread (209) an `upstream_url`, as a served read of a Slack channel would. Every other message
carries none. The Slack address itself is answered locally (a route fulfils it): what is under test is
which address the new page is opened at and what it can see of this page, never Slack's own answer.

`#461 upstream-link-native`: beside the link stands "Copy Slack link" again. The link itself was
verified native — a right-button press reaches an unprevented `contextmenu` on the anchor with the
menu still open, which this walk now presses for real — but a phone's long press and an installed
app's menus cannot be counted on to offer copy-link-address, so the copy is an item once more.

At 1280x800 and at 393x852, the same walk:

  the channel opened in All -> the long question's ⋯ menu opened with the mouse: the "Open in Slack"
  link second under Copy text and "Copy Slack link" under it, holding the address with target and rel
  as served, the whole menu inside the window, no sideways scroll -> a real right-button press on
  the link: a `contextmenu` on the anchor, not prevented, the menu still open -> the Copy item:
  the clipboard holds the address -> the link followed: a new page at the address whose `opener` is null,
  this page where it was, the menu shut, nothing copied -> the question highlighted by the keys and a
  real `l` pressed through CDP: the clipboard holds the address whole and the status pill says so ->
  a real Ctrl+O: the same new page -> a message with no address: its ⋯ menu offers neither item, `l`
  on it highlighted copies nothing and Ctrl+O opens nothing.

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
    from playwright.sync_api import BrowserType, CDPSession, Page, Route

# (label, width, height, phone). The phone is the owner's: 393x852.
SIZES = (("desk-1280", 1280, 800, False), ("phone-393", 393, 852, True))
# The long question that opens a thread in the fake channel, and the address the fake server gives it:
# its own in Slack, as a served read would carry it.
QUESTION_ID = "209"
PLAIN_ID = "200"
ADDRESS = "https://acme.slack.com/archives/C0123ABCD/p1700000000000100"
# What the clipboard holds before each copy: a copy of nothing leaves it there.
SENTINEL = "open-upstream-link-browser: the clipboard before the copy"

# One row's ⋯ menu: its items in order (the link among the buttons), its box, and whether the page
# scrolls sideways.
MENU_JS = """(id) => {
    const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
    const menu = row ? row.querySelector('.row-more-menu') : null;
    const rect = menu ? menu.getBoundingClientRect() : null;
    return {
        items: menu ? [...menu.querySelectorAll('button, a')].filter((one) => !one.hidden)
            .map((one) => ({className: one.className, text: one.textContent, tag: one.tagName,
                href: one.getAttribute('href'), target: one.getAttribute('target'),
                rel: one.getAttribute('rel')})) : [],
        open: menu ? !menu.hidden : false,
        left: rect ? rect.left : null,
        right: rect ? rect.right : null,
        innerWidth: window.innerWidth,
        overflow: document.documentElement.scrollWidth - window.innerWidth,
        status: document.getElementById('status').textContent,
    };
}"""


class LinkedApi(ColumnsApi):
    """The columns channel as Slack, its long question carrying its own address there."""

    def __init__(self) -> None:
        super().__init__()
        self.provider_name = "Slack"
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
            report.append(deep_walk(playwright.chromium, args, label, width, height, phone))
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
            # The Slack address is answered here, so the new page's address and what it can see
            # of this one are read without reaching Slack. Registered before the page opens, so
            # it holds for the popup the link opens too.
            def in_slack(route: Route) -> None:
                route.fulfill(content_type="text/html", body="<title>the message in Slack</title>opened")

            context.route("https://acme.slack.com/**", in_slack)
            context.grant_permissions(["clipboard-read", "clipboard-write"], origin=origin)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            reader = Reader(Desk(page, args.screenshots, label, width, height), context.new_cdp_session(page))
            reader.origin = origin
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
        self.origin = ""

    @property
    def page(self) -> Page:
        return self.desk.page

    def key(self, name: str, shift: bool = False, ctrl: bool = False) -> None:
        """Press one key (`l`, `j`, `Home`, `Escape`, Ctrl+`O`) through CDP, as a keyboard sends it."""
        named = {"Home": ("Home", 36), "Escape": ("Escape", 27)}
        modifiers = (8 if shift else 0) | (2 if ctrl else 0)  # CDP's modifier bits: 8 Shift, 2 Ctrl.
        if name in named:
            code, vk = named[name]
            text = ""
            key_name = name
        else:
            code, vk = f"Key{name.upper()}", ord(name.upper())
            text = key_name = name.upper() if shift else name
            if ctrl:
                text = ""
        down: Json = {"type": "keyDown" if text else "rawKeyDown", "modifiers": modifiers,
                      "key": key_name, "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk}
        if text:
            down["text"] = text
            down["unmodifiedText"] = text
        self.cdp.send("Input.dispatchKeyEvent", down)
        self.cdp.send("Input.dispatchKeyEvent", {"type": "keyUp", "modifiers": modifiers, "key": key_name,
                                                 "code": code, "windowsVirtualKeyCode": vk,
                                                 "nativeVirtualKeyCode": vk})
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

    def opened_at_the_address(self, act: str) -> Page:
        """The new page `act` ("item" or "key") opens: at the address, holding nothing of this page."""
        context = self.page.context
        here = self.page.url
        with context.expect_page(timeout=10_000) as tab:
            if act == "item":
                self.page.locator(f"#discord-log > li[data-id='{QUESTION_ID}'] .row-open-link-button").click()
            else:
                self.key("o", ctrl=True)
        opened_page = tab.value
        opened_page.wait_for_load_state()
        self.check(opened_page.url == ADDRESS, f"{act} opened {opened_page.url}, not the address")
        self.check(bool(opened_page.evaluate("() => window.opener === null")),
                   f"the page {act} opened can reach back into this one (opener is not null)")
        self.check(self.page.url == here, f"{act} navigated this page itself to {self.page.url}")
        self.desk.settled()
        return opened_page

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
        self.check([item["className"] for item in items[:4]] == ["row-copy-button", "row-open-link-button", "row-copy-link-button", "row-copy-deep-link-button"],
                   f"the link and its copies are not under Copy text: {items}")
        self.check(items[2]["text"] == "Copy Slack link" and items[2]["tag"] == "BUTTON",
                   f"the Copy item is not named for its service: {items}")
        self.check(items[3]["text"] == "Copy vibe-talk link",
                   f"the app's own copy is not offered: {items}")
        link = items[1]
        self.check(link["text"] == "Open in Slack", f"the item is not named for its service: {items}")
        self.check(link["tag"] == "A" and link["href"] == ADDRESS and link["target"] == "_blank"
                   and link["rel"] == "noopener noreferrer",
                   f"the item is not a real link to the address opening apart: {link}")
        self.check(float(str(opened["left"])) >= 0 and float(str(opened["right"])) <= float(str(opened["innerWidth"])),
                   f"the open menu runs off the window: {opened}")
        self.check(float(str(opened["overflow"])) <= 0, "the page scrolls sideways with the menu open")
        self.desk.shot("1-menu")
        self.desk.notes.append("the ⋯ menu offered the \"Open in Slack\" link second under Copy text, "
                               "\"Copy Slack link\" under it, wholly inside the window")

        # A real right-button press on the link: the browser's own menu is native UI this walk
        # cannot read, but its precondition is page-visible — a contextmenu event on the anchor
        # itself, not prevented, with the ⋯ menu still open around it (`#461`).
        page.evaluate("() => { window.__probeCm = [];"
                      " document.addEventListener('contextmenu', (e) => window.__probeCm.push("
                      "{cls: e.target.className, prevented: e.defaultPrevented}), true); }")
        link_box = page.locator(f"#discord-log > li[data-id='{QUESTION_ID}'] .row-open-link-button").bounding_box()
        assert link_box is not None
        page.mouse.move(link_box["x"] + link_box["width"] / 2, link_box["y"] + link_box["height"] / 2)
        page.mouse.down(button="right")
        page.wait_for_timeout(300)
        probed = page.evaluate(f"() => ({{cm: window.__probeCm,"
                               f" open: !document.querySelector('#discord-log > li[data-id=\"{QUESTION_ID}\"] .row-more-menu').hidden}})")
        page.mouse.up(button="right")
        self.check(probed["cm"] == [{"cls": "row-open-link-button", "prevented": False}],
                   f"a right-click on the link reached no native contextmenu: {probed}")
        self.check(bool(probed["open"]), "a right-click shut the ⋯ menu under the browser's own menu")
        self.desk.notes.append("a right-click on the link reached an unprevented contextmenu on the anchor")

        # The Copy item: the clipboard holds the address, as l leaves it.
        page.evaluate("(text) => navigator.clipboard.writeText(text)", SENTINEL)
        page.locator(f"#discord-log > li[data-id='{QUESTION_ID}'] .row-copy-link-button").click()
        self.desk.settled()
        self.check(self.clipboard() == ADDRESS, "the Copy item did not copy the address")
        self.check(not self.menu(QUESTION_ID)["open"], "the ⋯ menu stayed open after the Copy item")
        page.locator(menu_button).click()
        self.desk.settled()
        self.desk.notes.append("the Copy item copied the same address")

        # `#462`: this app's own link for the same message, from the reopened menu.
        page.evaluate("(text) => navigator.clipboard.writeText(text)", SENTINEL)
        page.locator(f"#discord-log > li[data-id='{QUESTION_ID}'] .row-copy-deep-link-button").click()
        self.desk.settled()
        deep = self.clipboard()
        self.check(deep.startswith(self.origin + "/?") and "v=main" in deep and "%3A209" in deep
                   and "migration" not in deep,
                   f"Copy vibe-talk link copied no durable link: {deep}")
        page.locator(menu_button).click()
        self.desk.settled()
        self.desk.notes.append("Copy vibe-talk link copied this app's durable address for it")

        # Following it opens the address in a new page, copies nothing, and shuts the menu.
        page.evaluate("(text) => navigator.clipboard.writeText(text)", SENTINEL)
        self.opened_at_the_address("item").close()
        shut = self.menu(QUESTION_ID)
        self.check(self.clipboard() == SENTINEL, "following the link changed the clipboard")
        self.check(not shut["open"], "the ⋯ menu stayed open after following the link")
        self.desk.notes.append("the link opened the message's Slack address in a new page whose opener "
                               "is null, this page unmoved")

        # The keys: highlight the question, press l (the copy a pointer takes from the link),
        # then Ctrl+O (the opening itself).
        self.highlight(QUESTION_ID)
        self.check(self.copied_after("l") == ADDRESS, "l on the highlighted question did not copy its link")
        self.check(self.menu(QUESTION_ID)["status"] == "Copied the link in Slack.", "l did not say it copied")
        self.desk.notes.append("l on the highlighted question copied the same address")
        self.opened_at_the_address("key").close()
        self.desk.notes.append("Ctrl+O on the highlighted question opened the same address")

        # A message the server formed no address for: no item, and both keys are nothing.
        plain_button = f"#discord-log > li[data-id='{PLAIN_ID}'] .row-more-button"
        page.locator(plain_button).scroll_into_view_if_needed()
        page.locator(plain_button).click()
        self.desk.settled()
        plain = self.menu(PLAIN_ID)
        plain_items = plain["items"]
        assert isinstance(plain_items, list)
        self.check(all(item["className"] not in ("row-open-link-button", "row-copy-link-button") for item in plain_items),
                   f"a message with no address was offered one: {plain_items}")
        self.check(any(item["className"] == "row-copy-deep-link-button" for item in plain_items),
                   f"a message with no upstream address was not offered its vibe-talk link: {plain_items}")
        self.key("Escape")
        self.highlight(PLAIN_ID)
        self.check(self.copied_after("l") == SENTINEL, "l on a message with no address changed the clipboard")
        pages_before = len(page.context.pages)
        self.key("o", ctrl=True)
        page.wait_for_timeout(500)
        self.check(len(page.context.pages) == pages_before, "Ctrl+O on a message with no address opened a page")
        self.check(float(str(self.menu(PLAIN_ID)["overflow"])) <= 0, "the page scrolls sideways")
        self.desk.shot("2-no-link")
        self.desk.notes.append("a message with no address offered no upstream item (its vibe-talk link "
                               "still stood), l on it copied nothing and Ctrl+O opened nothing")



# A thread reply the deep-link walk lands on: REPLICA's second answer (its root is the long question).
DEEP_REPLY_ID = "212"
DEEP_THREAD = "spaces/A/threads/replica"
DEEP_QUERY = ("c=1110000000000000001&v=thread&t=spaces%2FA%2Fthreads%2Freplica"
              "&m=slack%3A1110000000000000001%3A212%3Aspaces%252FA%252Fthreads%252Freplica")


def deep_walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
              phone: bool) -> str:
    """`#462 webapp-deep-links` at one size: links in, addresses out, and a second server.

    A fresh profile opens a link naming a thread reply: the thread opens over the restored
    state and the reply is landed on. The views round-trip through the address (open `v=main`,
    walk to All by the picker, the address follows; reload-standing query reopens the same
    place). Then the SAME query is opened against a second, fresh server instance on the same
    fake upstream — a reboot by another name — and lands on the same message.
    """
    servers: list[tuple[ThreadingHTTPServer, threading.Thread]] = []

    def serve() -> str:
        api = LinkedApi()
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
        server.daemon_threads = True
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        servers.append((server, thread))
        return f"http://127.0.0.1:{server.server_port}"

    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-deep-") as profile:
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
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"{label}-deep", width, height)

            def open_at(origin: str, query: str) -> None:
                page.goto(f"{origin}/?{query}", wait_until="load")
                if page.locator("#api-token").is_visible():
                    page.fill("#api-token", TOKEN)
                    page.click("#save-token")
                    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
                if page.evaluate("() => document.getElementById('pane-discord').hidden"):
                    page.click("#view-switch")
                desk.settled()

            origin = serve()
            open_at(origin, DEEP_QUERY)
            desk.wait_for(f"() => {{ const row = document.querySelector('#discord-log > li[data-id=\"{DEEP_REPLY_ID}\"]');"
                          " return row && row.getAttribute('data-landed') === 'true'; }",
                          "the deep link did not land on its reply")
            desk.check(page.evaluate("() => !document.getElementById('thread-heading').hidden"),
                       "the deep link did not open its thread")
            status = page.text_content("#status") or ""
            desk.check("linked message" in status, f"the landing was not said: {status!r}")
            desk.check("v=thread" in page.url and "m=slack%3A" in page.url,
                       f"the address let go of the link's place: {page.url}")
            desk.shot("3-deep-landed")
            desk.notes.append("a deep link opened its thread and landed on its reply, the address still naming it")

            # Views round-trip: in by link, out by address.
            open_at(origin, "c=1110000000000000001&v=main")
            desk.wait_for("() => document.getElementById('thread-select').value === 'main'",
                          "v=main did not open Main")
            page.select_option("#thread-select", "flat")
            desk.wait_for("() => location.search.includes('v=all')", "the address did not follow into All")
            page.reload(wait_until="load")
            desk.settled()
            if page.evaluate("() => document.getElementById('pane-discord').hidden"):
                page.click("#view-switch")
                desk.settled()
            desk.wait_for("() => document.getElementById('thread-select').value === 'flat'",
                          "a reload did not reopen the addressed view")
            desk.notes.append("v=main opened Main, the address followed a move to All, a reload reopened All")

            # Durability: the same query against a fresh server instance lands on the same reply.
            other = serve()
            page2 = context.new_page()
            page2.on("pageerror", lambda error: errors.append(str(error)))
            page2.goto(f"{other}/?{DEEP_QUERY}", wait_until="load")
            if page2.locator("#api-token").is_visible():
                page2.fill("#api-token", TOKEN)
                page2.click("#save-token")
                page2.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            if page2.evaluate("() => document.getElementById('pane-discord').hidden"):
                page2.click("#view-switch")
            page2.wait_for_function(
                f"() => {{ const row = document.querySelector('#discord-log > li[data-id=\"{DEEP_REPLY_ID}\"]');"
                " return row && row.getAttribute('data-landed') === 'true'; }", timeout=15_000)
            desk.notes.append("the same query on a fresh server instance landed on the same reply")
            check(not errors, f"{label} deep: the page threw: {errors}")
            context.close()
            return f"{version} deep at {width}x{height}: " + "; ".join(desk.notes)
    finally:
        for server, thread in servers:
            server.shutdown()
            server.server_close()
            thread.join()

if __name__ == "__main__":
    raise SystemExit(main())
