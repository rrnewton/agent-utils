#!/usr/bin/env python3
"""Follow a link to a message inside the app, in real Chromium, and come back with Back.

`#543 in-app-message-links`. The owner, 2026-10-26: threads whose messages cite earlier ones by their
chat permalinks are "really horrible on mobile" — every tap leaves for the chat service — so a link to a
message in a channel the app reads opens the message in the app, and the Android back gesture returns.
The server marks such a link with what it names (src/message_links.rs); the fake-DOM suite
(tests/js/voice_page.test.mjs) pins what the page does with the mark, its history entries and their
undoing, but it lays nothing out and has no real history, so it cannot say whether the reader really
lands on the message, whether a real Back (a browser traversal, the gesture's own path) undoes the jump,
or whether the reader's message comes back at the same height. This does.

It serves the checked-out page and the fake API of tests/two_column_browser.py — a channel taller than
the window with three threads — and gives three messages bodies as the server renders a link to a
message: 220 a bare link to 206, a reply in the runner thread; 207 a named link to 201, six rows up;
and 206 itself a bare link to 201, for a second jump from the first's landing. Each anchor carries the
`data-msg` the server writes for that address.

At 1280x800 and at 393x852 with touch, the same walk:

  the channel opened in All, the reader scrolled up so 220 sits a third of the way down -> 220's link
  is drawn as the app's, "#lead team › reply" -> tapped: the runner thread opens and 206 is landed on,
  one history entry deeper, the address naming 206 -> a real Back (`page.go_back`): the thread closes,
  All is back, 220 at the height it was -> 207's link, "the plan", tapped: 201 landed on up the list
  -> a real Back: 207 at the height it was -> 220's link again, and from 206 in the thread its own link
  to 201, "#lead team › message": two entries over the start, All again on 201 -> Back: the thread, on
  206 -> Back: All, 220 at the height it was, on the entry the first jump was taken from.

No page error at any step. Pass --screenshots DIR to keep a PNG of each landing.
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import quote

import offline_cache_browser as fixture
from offline_cache_browser import CHANNEL, TOKEN, Json, check, handler_for
from two_column_browser import RUNNER, ColumnsApi, Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

# (label, width, height, phone). The phone is the owner's: 393x852.
SIZES = (("desk-1280", 1280, 800, False), ("phone-393", 393, 852, True))
LINK_ATTRS = 'target="_blank" rel="noopener noreferrer nofollow"'


def address(message_id: str) -> str:
    """The message's address in the service, as a Discord client hands one out."""
    return f"https://discord.com/channels/100000000000000009/{CHANNEL['id']}/{message_id}"


def mark(message_id: str, thread: str = "") -> str:
    """The `data-msg` the server writes on a link to the message, a reply in `thread`."""
    return ":".join(quote(part, safe="") for part in ("discord", str(CHANNEL["id"]), message_id, thread))


def link_body(href: str, data: str, text: str) -> str:
    return f'<p>see <a href="{href}" data-msg="{data}" {LINK_ATTRS}>{text}</a> above</p>\n'


class LinkedApi(ColumnsApi):
    """The columns channel, two late messages linking to earlier ones as the server renders them."""

    def __init__(self) -> None:
        super().__init__()
        for row in self.messages:
            if row["id"] == "220":
                row["content_html"] = link_body(address("206"), mark("206", RUNNER), address("206"))
            if row["id"] == "207":
                row["content_html"] = link_body(address("201"), mark("201"), "the plan")
            if row["id"] == "206":
                row["content_html"] = link_body(address("201"), mark("201"), address("201"))


# Where the reader is: the row's top in the window, the list's scroll, the thread heading, the picker,
# the status line, the history's length, where the browser is in it (forward entries count in the
# length, so a depth is read from the Navigation API's index) and the address.
SPOT_JS = """(id) => {
    const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
    return {
        top: row ? row.getBoundingClientRect().top : null,
        scroll: document.getElementById('scroll-area').scrollTop,
        thread: !document.getElementById('thread-heading').hidden,
        picker: document.getElementById('thread-select').value,
        status: document.getElementById('status').textContent,
        length: history.length,
        index: navigation.currentEntry.index,
        search: location.search,
    };
}"""

# A link in a row: its words, whether the app took it, and its middle, for a tap.
LINK_JS = """(id) => {
    const a = document.querySelector(`#discord-log > li[data-id="${id}"] .md a[data-msg]`);
    if (!a) return null;
    a.scrollIntoView({block: 'nearest'});
    const r = a.getBoundingClientRect();
    return {text: a.textContent, inApp: a.hasAttribute('data-in-app'), href: a.getAttribute('href'),
            x: r.left + Math.min(r.width / 2, 20), y: r.top + r.height / 2};
}"""

LANDED_JS = """(id) => document.querySelector(`#discord-log > li[data-id="${id}"]`)?.getAttribute('data-landed') === 'true'"""


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
        "--port",
        type=int,
        default=0,
        help="port for the fake API on 127.0.0.1 (default: 0, any free port)",
    )
    parser.add_argument(
        "--screenshots",
        type=Path,
        help="directory to write one PNG per landing into (default: none are kept)",
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
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    origin = f"http://127.0.0.1:{server.server_port}"
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-msglinks-") as profile:
            options: Json = {"viewport": {"width": width, "height": height}, "color_scheme": "dark"}
            if phone:
                options.update(device_scale_factor=2.625, is_mobile=True, has_touch=True)
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable, **options)
            # Nothing may leave for the service: a request there is a jump that did not stay.
            left: list[str] = []
            context.route("https://discord.com/**", lambda route: (left.append(route.request.url), route.abort()))
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            reader = Reader(Desk(page, args.screenshots, label, width, height), phone)
            page.goto(f"{origin}/", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            page.click("#view-switch")
            reader.desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24",
                                 "the channel did not open in All")
            reader.desk.settled()
            reader.walk()
            check(not left, f"{label}: a tap left for the service: {left}")
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(reader.desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


class Reader:
    """The page at one size, and a finger or a mouse on it."""

    def __init__(self, desk: Desk, phone: bool) -> None:
        self.desk = desk
        self.phone = phone

    @property
    def page(self) -> Page:
        return self.desk.page

    def spot(self, message_id: str) -> Json:
        found: Json = self.page.evaluate(SPOT_JS, message_id)
        return found

    def scroll_to_middle(self, message_id: str) -> Json:
        """Scroll the list so the row sits a third of the way down the window, as a reader reading it."""
        self.page.evaluate(
            """(id) => {
                const area = document.getElementById('scroll-area');
                const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
                area.scrollTop += row.getBoundingClientRect().top - area.getBoundingClientRect().top
                    - area.clientHeight / 3;
            }""",
            message_id,
        )
        self.desk.settled()
        self.page.wait_for_timeout(150)
        self.desk.settled()
        return self.spot(message_id)

    def tap_link(self, message_id: str) -> Json:
        link: Json | None = self.page.evaluate(LINK_JS, message_id)
        self.desk.check(link is not None, f"{message_id} draws no marked link")
        assert link is not None
        self.desk.settled()
        link = self.page.evaluate(LINK_JS, message_id)
        assert link is not None
        if self.phone:
            self.page.touchscreen.tap(float(link["x"]), float(link["y"]))
        else:
            self.page.mouse.click(float(link["x"]), float(link["y"]))
        return link

    def back(self) -> None:
        """The browser's own Back: a real traversal, the path the phone's gesture takes."""
        self.page.go_back(wait_until="commit")
        self.desk.settled()

    def wait_back_where(self, message_id: str, why: str) -> Json:
        self.desk.wait_for(
            """() => document.getElementById('status').textContent === 'Back where you were.'""", why)
        self.desk.settled()
        return self.spot(message_id)

    def same_height(self, before: Json, after: Json, what: str) -> None:
        self.desk.check(after["top"] is not None, f"{what}: the reader's message is not drawn after Back")
        drift = abs(float(after["top"]) - float(before["top"]))
        self.desk.check(drift <= 2, f"{what}: Back put the reader's message at {after['top']:.0f}, "
                                    f"not {before['top']:.0f} ({drift:.0f}px off)")

    def walk(self) -> None:
        notes = self.desk.notes
        start = int(self.spot("220")["index"])
        # The reply link: drawn as the app's, named by where it goes.
        before = self.scroll_to_middle("220")
        self.desk.check(not before["thread"] and before["picker"] == "flat", "the channel is not in All")
        self.desk.check(float(before["scroll"]) > 0, "the reader is not scrolled up the list")
        link = self.tap_link("220")
        self.desk.check(bool(link["inApp"]), "the link to a reply here is not the app's")
        self.desk.check(link["text"] == "#lead team › reply",
                        f"the link whose text was its address reads {link['text']!r}")
        self.desk.wait_for(f"() => ({LANDED_JS})('206')", "the tap did not land on the linked reply")
        landed = self.spot("206")
        self.desk.check(landed["thread"] and landed["picker"] == f"thread:{RUNNER}",
                        f"the reply's thread did not open: {landed['picker']}")
        self.desk.check(int(landed["index"]) == int(before["index"]) + 1,
                        f"the jump moved {int(landed['index']) - int(before['index'])} history entries, not one")
        self.desk.check("m=discord%3A" in str(landed["search"]) and "206" in str(landed["search"]),
                        f"the address does not name the landed message: {landed['search']}")
        self.desk.shot("landed-reply")
        notes.append(f"reply link {link['text']!r} landed 206 in its thread, +1 entry")

        self.back()
        after = self.wait_back_where("220", "Back after the jump did not come back")
        self.desk.check(not after["thread"] and after["picker"] == "flat", "Back did not close the thread to All")
        self.same_height(before, after, "the reply jump")
        notes.append(f"Back: All, 220 within {abs(float(after['top']) - float(before['top'])):.0f}px")

        # The root link, named by its writer, far up the list.
        before = self.scroll_to_middle("207")
        link = self.tap_link("207")
        self.desk.check(link["text"] == "the plan", f"a named link lost its name: {link['text']!r}")
        self.desk.wait_for(f"() => ({LANDED_JS})('201')", "the tap did not land on the linked root")
        landed = self.spot("201")
        self.desk.check(not landed["thread"] and landed["picker"] == "flat", "a jump to a root left All")
        self.desk.check(float(landed["scroll"]) < float(before["scroll"]),
                        "the jump to a message up the list did not move the list")
        self.desk.shot("landed-root")
        self.back()
        after = self.wait_back_where("207", "Back after the root jump did not come back")
        self.same_height(before, after, "the root jump")
        notes.append(f"root link landed 201, Back: 207 within {abs(float(after['top']) - float(before['top'])):.0f}px")

        # Two jumps one after another, the second from the first's landing, then two Backs.
        before = self.scroll_to_middle("220")
        self.tap_link("220")
        self.desk.wait_for(f"() => ({LANDED_JS})('206')", "the first of two jumps did not land")
        self.desk.settled()
        link = self.tap_link("206")
        self.desk.check(link["text"] == "#lead team › message",
                        f"the link to a root whose text was its address reads {link['text']!r}")
        self.desk.wait_for(f"() => ({LANDED_JS})('201') && document.getElementById('thread-heading').hidden",
                           "the second jump, from the thread to a root, did not land in All")
        self.desk.check(int(self.spot("201")["index"]) == int(before["index"]) + 2,
                        "two jumps are not two entries over the start")
        self.back()
        self.desk.wait_for("() => !document.getElementById('thread-heading').hidden && "
                           "document.getElementById('status').textContent === 'Back where you were.'",
                           "the first Back did not return to the thread the second jump was taken from")
        self.desk.check(self.spot("206")["top"] is not None, "the first Back's thread does not draw 206")
        self.page.evaluate("() => { document.getElementById('status').textContent = ''; }")
        self.back()
        after = self.wait_back_where("220", "the second Back did not return to the start")
        self.desk.check(not after["thread"] and after["picker"] == "flat", "the second Back did not return to All")
        self.same_height(before, after, "two stacked jumps")
        self.desk.check(int(after["index"]) == int(before["index"]),
                        "two Backs did not return to the entry the first jump was taken from")
        notes.append(f"two stacked jumps, two Backs: 220 within {abs(float(after['top']) - float(before['top'])):.0f}px")
        self.desk.check(int(after["index"]) == start, "the walk did not end on the entry it started on")


if __name__ == "__main__":
    raise SystemExit(main())
