#!/usr/bin/env python3
"""Empty messages left out of the page, and back at a switch in Settings, in real Chromium.

`#386 hide-empty-messages`. The owner, 2026-10-19: "there is a BLANK message from the coding agent...
along with auto-marking read on some trivial messages I want to have a checked-by-default box in that
same settings area that completely filters out empty messages". The fake-DOM suite pins which
messages are empty and what each list and count leaves out; this walks the switch where the owner
uses it and looks at what Chromium draws.

It serves the checked-out page and a small fake API from one loopback origin: a channel holding the
agent's words, an empty message from the agent (the owner's case), the owner's own words, a message of
spaces and line breaks, a file posted with no words as the server writes one, a message with no text
and one reaction, and a thread whose replies are an empty one, the owner's and the agent's.

At 1280x800 with a mouse, in the dark scheme and the light: All, two columns, the thread's card ->
neither empty message in All, the file and the reaction drawn (negative controls), All's heading
counting the 7 messages drawn and Main's the 5, the card counting 1 unread, not the empty reply -> its replies button: the thread on the right with no empty row ->
Settings > Messages > Empty messages: the switch on, beside Mark-read automatically -> off: both empty
rows in All and the empty reply on the right, the card counting 2 unread -> a reload: still off, the
empty rows drawn -> on again: gone.

At 393x852 by touch, in both schemes: All -> neither empty message, the file and the reaction drawn ->
the switch off: both drawn -> on: gone.

No horizontal overflow and no page error at any step. Pass --screenshots DIR to keep a PNG of each
state for review; --web-root DIR to run the same walk against another copy of the page.
"""

from __future__ import annotations

import argparse
import os
import re
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import OWNER_ID, TOKEN, FakeApi, Json, check, handler_for, said
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

# (label, width, height, colour scheme, phone).
SIZES = (
    ("desk-1280-dark", 1280, 800, "dark", False),
    ("desk-1280-light", 1280, 800, "light", False),
    ("phone-393-dark", 393, 852, "dark", True),
    ("phone-393-light", 393, 852, "light", True),
)

THREAD: Json = {"id": "spaces/A/threads/deploy", "root_message_id": "206", "is_root": True,
                "reply_count": 3, "reply_count_exact": True}
FILE_LINE = "📎 [screenshot.png](<https://chat.example/files/F0IMAGE01/screenshot.png>) · PNG"

# The empty messages: the agent's, in Main, and one of spaces, line breaks and a zero-width space; and
# the empty reply in the thread.
EMPTY = ("201", "203")
EMPTY_REPLY = "207"
# Not empty, though they have no words of their own.
FILE_ONLY, REACTION_ONLY = "204", "205"


class EmptyApi(FakeApi):
    """A channel with empty messages in it, and in a thread, beside messages that only look empty."""

    def __init__(self) -> None:
        super().__init__()
        under = {**THREAD, "is_root": False}
        self.messages = [
            said(0, "The overnight run finished; the release lane is green."),
            said(1, ""),
            said(2, "Thanks. Please post the notes.", "me"),
            said(3, "  \n​ "),
            said(4, FILE_LINE),
            {**said(5, ""), "reactions": [{"emoji": "👍", "count": 1}]},
            {**said(6, "Deploying the release branch now."), "thread": THREAD},
            {**said(7, ""), "thread": under},
            {**said(8, "Which commit?", "me"), "thread": under},
            {**said(9, "The one tagged this morning."), "thread": under},
        ]
        self.threads = [{"id": THREAD["id"], "root": self.messages[6], "title": "Deploying the release branch now.",
                         "reply_count": 3, "reply_count_exact": True,
                         "updated_at": self.messages[9]["timestamp"]}]

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope), "owner_author_id": OWNER_ID}


# The rows of a list, the card's words, and whether the page scrolls sideways.
DRAWN_JS = """(list) => ({
    ids: [...document.querySelectorAll('#' + list + ' > li[data-id]')].flatMap((li) => (li.getAttribute('data-ids') || li.getAttribute('data-id')).split(' ')),
    texts: Object.fromEntries([...document.querySelectorAll('#' + list + ' > li[data-id]')].map((li) => [li.getAttribute('data-id'), li.textContent])),
    card: (document.querySelector('#side-cards > li[data-card]') || {textContent: ''}).textContent,
    overflow: document.documentElement.scrollWidth - window.innerWidth,
})"""


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
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, scheme, phone in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, scheme, phone))
    for line in report:
        print(line)
    return 0


class Reader:
    """The page at one size, and what the walk has found there."""

    def __init__(self, page: Page, args: argparse.Namespace, label: str) -> None:
        self.page = page
        self.args = args
        self.label = label
        self.notes: list[str] = []

    def check(self, condition: bool, why: str) -> None:
        check(condition, f"{self.label}: {why}")

    def shot(self, name: str) -> None:
        if self.args.screenshots:
            self.page.screenshot(path=str(self.args.screenshots / f"{self.label}-{name}.png"))

    def settled(self) -> None:
        self.page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")

    def drawn(self, list_id: str = "discord-log") -> Json:
        found: Json = self.page.evaluate(DRAWN_JS, list_id)
        self.check(float(str(found["overflow"])) <= 0, "the page scrolls sideways")
        return found

    def ids(self, list_id: str = "discord-log") -> list[str]:
        found = self.drawn(list_id)["ids"]
        assert isinstance(found, list)
        return [str(each) for each in found]

    def wait_for(self, condition: str, why: str, arg: object = None) -> None:
        try:
            self.page.wait_for_function(condition, arg=arg, timeout=10_000)
        except Exception as error:  # noqa: BLE001 - the walk's own words say what did not happen
            raise AssertionError(f"{self.label}: {why}") from error

    def switch(self, on: bool) -> None:
        """Settings > Messages > Empty messages, set to `on`, and Settings closed."""
        page = self.page
        page.click("#open-settings")
        for fold in ("fold-settings-messages", "fold-empty-messages"):
            if page.get_attribute(f"#{fold}", "aria-expanded") != "true":
                page.click(f"#{fold}")
        page.wait_for_selector("#hide-empty-messages", state="visible", timeout=5_000)
        groups = page.evaluate("""() => [...document.querySelectorAll('#fold-settings-messages-body .settings-group h3')]
            .map((h) => h.textContent.trim())""")
        self.check(groups[1:3] == ["Mark-read automatically", "Empty messages"],
                   f"Empty messages is not beside Mark-read automatically: {groups}")
        self.shot(f"settings-{'on' if page.is_checked('#hide-empty-messages') else 'off'}")
        page.set_checked("#hide-empty-messages", on)
        page.click("#close-settings")
        self.settled()


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, scheme: str,
         phone: bool) -> str:
    """One reader at one size in one scheme, against a fresh fake API and a fresh browser profile."""
    api = EmptyApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-empty-messages-") as profile:
            if phone:
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="light" if scheme == "light" else "dark",
                    device_scale_factor=2.625, is_mobile=True, has_touch=True,
                )
            else:
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="light" if scheme == "light" else "dark",
                )
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            reader = Reader(page, args, label)
            url = f"http://127.0.0.1:{server.server_port}/"
            page.goto(url, wait_until="load")
            open_all(reader)
            if phone:
                phone_walk(reader)
            else:
                desk_walk(reader, url)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}, {scheme}: " + "; ".join(reader.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def open_all(reader: Reader) -> None:
    """Signed in, the channel in All."""
    page = reader.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    reader.wait_for("() => document.querySelector(\"#discord-log > li[data-id='209']\")", "the channel did not open in All")
    reader.settled()


def check_hidden(reader: Reader, where: str) -> None:
    """No empty message drawn; the file alone and the reaction alone drawn, saying what they say."""
    found = reader.drawn()
    ids = reader.ids()
    reader.check(not [id_ for id_ in (*EMPTY, EMPTY_REPLY) if id_ in ids], f"{where}: an empty message is drawn: {ids}")
    texts = found["texts"]
    assert isinstance(texts, dict)
    reader.check(FILE_ONLY in ids and "screenshot.png" in str(texts.get(FILE_ONLY, "")),
                 f"{where}: the file posted with no words is not drawn naming it: {texts.get(FILE_ONLY)!r}")
    reader.check(REACTION_ONLY in ids and "👍" in str(texts.get(REACTION_ONLY, "")),
                 f"{where}: the message with only a reaction is not drawn with it: {texts.get(REACTION_ONLY)!r}")


def check_shown(reader: Reader, where: str, replies: bool) -> None:
    """Every message drawn: the thread's empty reply too where the list holds replies (All, not Main)."""
    ids = reader.ids()
    reader.check(all(id_ in ids for id_ in (*EMPTY, *((EMPTY_REPLY,) if replies else ()), FILE_ONLY, REACTION_ONLY)),
                 f"{where}: not every message is drawn: {ids}")


def check_count(reader: Reader, count: int, where: str) -> None:
    """The heading over the list counts the messages it draws, not the empty ones it leaves out."""
    said_ = str(reader.page.text_content("#channel-summary") or "")
    found = re.search(r"(\d+) messages?", said_)
    reader.check(found is not None and int(found.group(1)) == count,
                 f"{where}'s heading says {said_!r}, not the {count} messages drawn")


def unread_on_card(reader: Reader) -> int:
    said_ = str(reader.drawn()["card"])
    found = re.search(r"(\d+) unread", said_)
    return int(found.group(1)) if found else 0


def desk_walk(reader: Reader, url: str) -> None:
    page = reader.page
    check_hidden(reader, "All")
    check_count(reader, 7, "All")
    page.click("#column-toggle")
    reader.wait_for("() => document.querySelectorAll('#side-cards > li[data-card]').length === 1", "the card is not listed")
    reader.settled()
    check_count(reader, 5, "Main")
    unread = unread_on_card(reader)
    reader.check(unread == 1, f"the card counts {unread} unread, not the agent's one answer")
    page.click("#side-cards > li[data-card] .side-card-open")
    reader.wait_for("() => document.querySelector(\"#side-log > li[data-id='209']\")"
                    " && !document.getElementById('side-pane').hasAttribute('data-arriving')"
                    " && !document.getElementById('side-thread-card').hasAttribute('data-flying')",
                    "the thread did not open on the right")
    reader.settled()
    right = reader.ids("side-log")
    reader.check(EMPTY_REPLY not in right and "208" in right, f"the thread on the right draws {right}")
    reader.shot("1-hidden")

    reader.switch(False)
    reader.wait_for(f"() => document.querySelector(\"#side-log > li[data-id='{EMPTY_REPLY}']\")",
                    "turned off, the empty reply did not come back on the right")
    # In two columns the left list is Main, which holds no replies.
    check_shown(reader, "turned off", replies=False)
    reader.shot("2-shown")
    page.click("#side-close")
    reader.wait_for("() => document.querySelectorAll('#side-cards > li[data-card]').length === 1", "the X did not go back to the card")
    reader.settled()
    unread_off = unread_on_card(reader)
    reader.check(unread_off == 2, f"turned off, the card counts {unread_off} unread, not the empty reply too")

    page.goto(url, wait_until="load")
    # It reopens as it was left, in two columns: Main on the left, whose newest row is the thread's root.
    reader.wait_for("() => document.querySelector(\"#discord-log > li[data-id='206']\")", "the reload did not reopen the channel")
    reader.settled()
    reader.check(not page.is_checked("#hide-empty-messages"), "a reload forgot the switch was off")
    reader.check(all(id_ in reader.ids() for id_ in EMPTY), f"after a reload with the switch off the rows are {reader.ids()}")

    reader.switch(True)
    reader.wait_for(f"() => !document.querySelector(\"#discord-log > li[data-id='{EMPTY[0]}']\")", "turned on, the empty row stayed")
    check_hidden(reader, "turned on again")
    reader.notes.append(f"the switch on by default: no empty row in All or on the right, the file and the reaction drawn, "
                        f"the card {unread} unread; off: every empty row back, {unread_off} unread, kept across a reload; on: gone")


def phone_walk(reader: Reader) -> None:
    check_hidden(reader, "All")
    reader.shot("1-hidden")
    reader.switch(False)
    reader.wait_for(f"() => document.querySelector(\"#discord-log > li[data-id='{EMPTY[0]}']\")", "turned off, the empty row did not come back")
    check_shown(reader, "turned off", replies=True)
    reader.shot("2-shown")
    reader.switch(True)
    reader.wait_for(f"() => !document.querySelector(\"#discord-log > li[data-id='{EMPTY[0]}']\")", "turned on, the empty row stayed")
    check_hidden(reader, "turned on again")
    reader.notes.append("no empty row in All, the file and the reaction drawn; off: back; on: gone")


if __name__ == "__main__":
    raise SystemExit(main())
