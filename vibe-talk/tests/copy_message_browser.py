#!/usr/bin/env python3
"""Copy the highlighted message with real keys in real Chromium, and read the clipboard back.

`#296 copy-highlighted-message`. The owner, 2026-10-14: a keystroke that copies the highlighted message
without making a text selection first. Cmd+C (Ctrl+C) stays the browser's wherever the browser has
something to copy: text selected on the page, or the focus in a text field. Otherwise, with a message
highlighted, it copies all of that message as written, a folded one too; a bare y does the same outside
the text fields; with nothing highlighted both keys are the browser's. The fake-DOM suite
(tests/js/voice_page.test.mjs) pins what the page does with the `copy` event it is handed; only a
browser can say whether Chromium fires that event for a chord with nothing selected, and what lands on
the system clipboard. This presses the keys through CDP `Input.dispatchKeyEvent`, the path a hardware
keyboard takes into the renderer, with `clipboard-read` granted to the page's origin, and reads the
clipboard back through `navigator.clipboard` after every press.

It serves the checked-out page and the fake API of tests/two_column_browser.py (a channel taller than
the window, with three threads, one opened by a long question that arrives folded) from one loopback
origin, and puts a sentinel on the clipboard before every press, so a key that copied nothing is seen
to have copied nothing.

At 1280x800 on Linux, with Ctrl:

  the channel opened in All, nothing highlighted: Ctrl+C leaves the clipboard alone -> Home and j to the
  long question, folded, its body clamped: Ctrl+C puts all of it on the clipboard as written, a rich
  flavour beside it; the status pill says "Copied message."; the list, the row and the highlight do
  not move -> the channel's box focused with a draft selected in it: Ctrl+C copies the draft; the
  caret alone in it: nothing; y typed into it -> Esc back to the list, the question still highlighted:
  words of it selected, Ctrl+C copies those words; the selection let go of, the message again -> y
  copies the message -> the thread list (Ctrl+3), ↓ onto a card: Ctrl+C and y copy its thread's first
  message.

At 1280x800 in a second tab whose `navigator.platform` is emulated as a Mac's: the list of keys says
Cmd+C; Cmd+C sent with the "copy" editing command, as a Mac's Edit menu answers it, copies the
highlighted message; Cmd+C sent without it, as no menu answers it off a Mac, copies the next one, by
the page's own way a task later.

At 412x915, a phone with a hardware keyboard attached: a tap on a message selects it silently, and Ctrl+C
then copies nothing; j from it highlights the next message, and Ctrl+C and y copy that one.

No page error at any step. Pass --screenshots DIR to keep a PNG of each state; --web-root DIR to run the
same walk against another copy of the page.
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
from two_column_browser import LONG_FIRST, REPLICA, ColumnsApi, Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, BrowserType, CDPSession, Page

# (label, width, height, phone).
SIZES = (("desk-1280", 1280, 800, False), ("phone-412", 412, 915, True))
# What the clipboard holds before each press: a key that copied nothing leaves it there.
SENTINEL = "copy-message-browser: the clipboard before the key"
# The long question that opens REPLICA, as it stands in the fake channel: folded when it arrives.
QUESTION_ID = "209"
MODIFIERS = {"Alt": 1, "Control": 2, "Meta": 4, "Shift": 8}
NAMED_KEYS: dict[str, tuple[str, int]] = {
    "Home": ("Home", 36), "End": ("End", 35), "ArrowDown": ("ArrowDown", 40), "Escape": ("Escape", 27),
}

# The rows drawn highlighted (`data-selected`), across every list; the focus; where the list and the
# question's row stand; and what the status pill says.
STATE_JS = """() => {
    const lit = [...document.querySelectorAll('[data-selected="true"]')].map((row) =>
        ({list: row.parentNode ? row.parentNode.id : '', id: row.getAttribute('data-id')}));
    const held = document.activeElement;
    const question = document.querySelector('#discord-log > li[data-id="209"]');
    const body = question ? question.querySelector('.body') : null;
    const area = document.getElementById('scroll-area');
    return {
        lit,
        focus: held ? (held.id || held.getAttribute('data-id') || held.tagName) : '',
        scrollTop: area.scrollTop,
        questionTop: question ? question.getBoundingClientRect().top : null,
        questionFolded: question ? question.getAttribute('data-collapsed') === 'true' : false,
        questionClamped: body ? body.scrollHeight > body.clientHeight + 2 : false,
        status: document.getElementById('status').textContent,
        statusShown: !document.getElementById('status-line').hidden,
        overflow: document.documentElement.scrollWidth - window.innerWidth,
    };
}"""

# Every flavour on the clipboard, as text by its type.
CLIPBOARD_JS = """async () => {
    const out = {};
    for (const item of await navigator.clipboard.read()) {
        for (const type of item.types) out[type] = await (await item.getType(type)).text();
    }
    return out;
}"""

# Select `count` characters of the question's text, from its start, as a mouse drag would.
SELECT_WORDS_JS = """(count) => {
    const body = document.querySelector('#discord-log > li[data-id="209"] .body');
    const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
    let node = walker.nextNode();
    while (node && node.textContent.trim() === '') node = walker.nextNode();
    const range = document.createRange();
    range.setStart(node, 0);
    range.setEnd(node, Math.min(count, node.textContent.length));
    const selection = getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    return String(selection);
}"""


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
    api = ColumnsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    origin = f"http://127.0.0.1:{server.server_port}"
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-copy-") as profile:
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
            # Reading the clipboard back is what this check is for; writing the sentinel needs the other.
            context.grant_permissions(["clipboard-read", "clipboard-write"], origin=origin)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            reader = Reader(Desk(page, args.screenshots, label, width, height), context.new_cdp_session(page), api)
            page.goto(f"{origin}/voice", wait_until="load")
            open_all(reader)
            if phone:
                phone_walk(reader)
            else:
                desk_walk(reader)
                mac_walk(reader, context, origin, errors)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(reader.desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


class Reader:
    """The page, the keyboard plugged into it, and the fake channel it reads."""

    def __init__(self, desk: Desk, cdp: CDPSession, api: ColumnsApi) -> None:
        self.desk = desk
        self.cdp = cdp
        self.api = api

    @property
    def page(self) -> Page:
        return self.desk.page

    def key(self, chord: str, commands: list[str] | None = None) -> None:
        """Press `chord` ("Control+c", "Meta+c", "j", "Home") through CDP, down and up, as a keyboard
        sends it: a chord with Ctrl or Cmd held carries no text. `commands` are the editing commands the
        browser's own key handling sends with it, as a Mac's Edit menu sends "copy" for Cmd+C."""
        *held, name = chord.split("+")
        modifiers = sum(MODIFIERS[one] for one in held)
        if name in NAMED_KEYS:
            code, vk = NAMED_KEYS[name]
            text = ""
        else:
            code, vk = f"Key{name.upper()}" if name.isalpha() else f"Digit{name}", ord(name.upper())
            text = "" if modifiers & (MODIFIERS["Control"] | MODIFIERS["Meta"] | MODIFIERS["Alt"]) else name
        down: Json = {"type": "keyDown" if text else "rawKeyDown", "modifiers": modifiers, "key": name,
                      "code": code, "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk}
        if text:
            down["text"] = text
            down["unmodifiedText"] = text
        if commands:
            down["commands"] = commands
        self.cdp.send("Input.dispatchKeyEvent", down)
        self.cdp.send("Input.dispatchKeyEvent", {"type": "keyUp", "modifiers": modifiers, "key": name, "code": code,
                                                 "windowsVirtualKeyCode": vk, "nativeVirtualKeyCode": vk})
        self.desk.settled()

    def state(self) -> Json:
        found: Json = self.page.evaluate(STATE_JS)
        return found

    def lit(self) -> list[tuple[str, str]]:
        """The rows drawn highlighted, as (list, id)."""
        found = self.state()["lit"]
        assert isinstance(found, list)
        return [(str(row["list"]), str(row["id"])) for row in found if isinstance(row, dict)]

    def arm(self) -> None:
        """Put the sentinel on the clipboard, and see that it is there."""
        self.page.evaluate("(text) => navigator.clipboard.writeText(text)", SENTINEL)
        check(self.clipboard() == SENTINEL, f"{self.desk.label}: the sentinel did not reach the clipboard")

    def clipboard(self) -> str:
        return str(self.page.evaluate("() => navigator.clipboard.readText()"))

    def flavours(self) -> dict[str, str]:
        found = self.page.evaluate(CLIPBOARD_JS)
        assert isinstance(found, dict)
        return {str(kind): str(text) for kind, text in found.items()}

    def copied(self, chord: str, commands: list[str] | None = None) -> str:
        """The clipboard after `chord`, with the sentinel put there first: the moment it changes, or
        after a second if it does not (the page's own way runs a task after the key, well within it)."""
        self.arm()
        self.key(chord, commands)
        for _ in range(40):
            now = self.clipboard()
            if now != SENTINEL:
                return now
            self.page.wait_for_timeout(25)
        return self.clipboard()

    def content(self, message_id: str) -> str:
        """The message as the fake server holds it: what was written."""
        found = next(row for row in self.api.messages if row["id"] == message_id)
        return str(found["content"])

    def check(self, condition: bool, why: str) -> None:
        self.desk.check(condition, why)


def number(state: Json, key: str) -> float:
    value = state[key]
    assert isinstance(value, (int, float)), f"{key} is {value!r}"
    return float(value)


def open_all(reader: Reader) -> None:
    page = reader.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    reader.desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24",
                         "the channel did not open in All")
    reader.desk.settled()


def highlight(reader: Reader, message_id: str) -> None:
    """Home, then j until message `message_id` is the one highlighted."""
    reader.key("Home")
    for _ in range(30):
        if reader.lit() == [("discord-log", message_id)]:
            return
        reader.key("j")
    raise AssertionError(f"{reader.desk.label}: j never highlighted message {message_id}: {reader.lit()}")


# --- at a desk, on Linux ---------------------------------------------------------------------------


def desk_walk(reader: Reader) -> None:
    page = reader.page
    # Nothing highlighted on load: the browser's copy, which has nothing to copy.
    reader.check(reader.lit() == [], f"a message is highlighted on load: {reader.lit()}")
    reader.check(reader.copied("Control+c") == SENTINEL, "Ctrl+C with nothing highlighted changed the clipboard")
    reader.desk.notes.append("with nothing highlighted, Ctrl+C left the clipboard alone")

    # The long question, folded, highlighted by the keys: all of it, as written.
    highlight(reader, QUESTION_ID)
    before = reader.state()
    reader.check(bool(before["questionFolded"]) and bool(before["questionClamped"]),
                 f"the long question is not folded and clamped; this proves nothing about a fold: {before}")
    text = reader.copied("Control+c")
    reader.check(text == LONG_FIRST, f"Ctrl+C on the folded question copied {text!r}")
    flavours = reader.flavours()
    reader.check(flavours.get("text/html", "").count(LONG_FIRST[:60]) == 1 and "<p>" in flavours.get("text/html", ""),
                 f"the rich flavour is not the body as drawn: {flavours}")
    after = reader.state()
    reader.check(after["status"] == "Copied message." and bool(after["statusShown"]), f"the copy was not said: {after}")
    reader.check(abs(number(after, "scrollTop") - number(before, "scrollTop")) <= 1
                 and abs(number(after, "questionTop") - number(before, "questionTop")) <= 1,
                 f"the copy moved the list: {before['scrollTop']}/{before['questionTop']} -> "
                 f"{after['scrollTop']}/{after['questionTop']}")
    reader.check(reader.lit() == [("discord-log", QUESTION_ID)] and after["focus"] == QUESTION_ID,
                 f"the copy moved the highlight or the focus: {reader.lit()}, focus {after['focus']}")
    reader.desk.shot("1-copied")
    reader.desk.notes.append(f"Ctrl+C on the folded long question copied all {len(text)} characters of it, with a "
                             "text/html flavour, said \"Copied message.\" and moved nothing")

    # The channel's box focused, the question still highlighted: the box's own copy.
    page.focus("#channel-compose-text")
    page.fill("#channel-compose-text", "half a thought")
    page.evaluate("() => document.getElementById('channel-compose-text').select()")
    reader.check(reader.copied("Control+c") == "half a thought", "Ctrl+C in the box did not copy the box's selected text")
    page.evaluate("() => { const box = document.getElementById('channel-compose-text'); "
                  "box.setSelectionRange(box.value.length, box.value.length); }")
    reader.check(reader.copied("Control+c") == SENTINEL, "Ctrl+C in the box with nothing selected in it copied something")
    reader.check(reader.copied("y") == SENTINEL, "y in the box copied something")
    typed = str(page.evaluate("() => document.getElementById('channel-compose-text').value"))
    reader.check(typed == "half a thoughty", f"y was not typed into the box: {typed!r}")
    reader.desk.notes.append("in the message box Ctrl+C copied the box's selected text, nothing with the caret "
                             "alone, and y was typed")

    # Back to the list, the draft kept: the question is still the highlighted message.
    reader.key("Escape")
    reader.check(reader.lit() == [("discord-log", QUESTION_ID)], f"Esc out of the box lost the highlight: {reader.lit()}")
    words = str(page.evaluate(SELECT_WORDS_JS, 24))
    reader.check(len(words) == 24, f"the drag selected {words!r}")
    reader.check(reader.copied("Control+c") == words, "with words of the question selected, Ctrl+C did not copy them")
    page.evaluate("() => getSelection().removeAllRanges()")
    reader.check(reader.copied("Control+c") == LONG_FIRST, "with the selection let go of, Ctrl+C did not copy the question")
    reader.check(reader.copied("y") == LONG_FIRST, "y did not copy the highlighted question")
    reader.desk.notes.append(f"with {len(words)} characters of it selected, Ctrl+C copied those; let go of, the "
                             "message; y copied it too")

    # The thread list: a card copies its thread's first message.
    reader.key("Control+3")
    reader.desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
                         " && document.querySelectorAll('#side-cards > li[data-card]').length === 3",
                         "Ctrl+3 did not show the thread list")
    reader.desk.settled()
    reader.key("ArrowDown")
    lit = reader.lit()
    reader.check(len(lit) == 1 and lit[0][0] == "side-cards", f"↓ in the thread list did not highlight a card: {lit}")
    card = lit[0][1]
    summary = next(one for one in reader.api.threads if one["id"] == card)
    root = summary["root"]
    assert isinstance(root, dict)
    first = str(root["content"])
    reader.check(reader.copied("Control+c") == first, f"Ctrl+C on the card of {card} did not copy its first message")
    reader.check(reader.copied("y") == first, f"y on the card of {card} did not copy its first message")
    reader.desk.shot("2-card")
    reader.desk.notes.append(f"on the thread list, Ctrl+C and y on the card of {card.rsplit('/', 1)[-1]} copied its "
                             f"first message ({len(first)} characters)")
    reader.check(number(reader.state(), "overflow") <= 0, "the page scrolls sideways")
    # Back to All, which the profile keeps for the next tab.
    reader.key("Control+2")
    reader.desk.wait_for("() => !document.getElementById('screen-main').hasAttribute('data-columns')"
                         " && document.querySelectorAll('#discord-log > li[data-id]').length === 24",
                         "Ctrl+2 did not come back to All")


# --- at a desk, on a Mac ---------------------------------------------------------------------------


def mac_walk(reader: Reader, context: BrowserContext, origin: str, errors: list[str]) -> None:
    """A second tab of the signed-in profile, its platform a Mac's: Cmd+C with the Edit menu's copy and without it."""
    mac = context.new_page()
    mac.on("pageerror", lambda error: errors.append(str(error)))
    cdp = context.new_cdp_session(mac)
    agent = str(reader.page.evaluate("() => navigator.userAgent"))
    cdp.send("Emulation.setUserAgentOverride", {"userAgent": agent, "platform": "MacIntel"})
    mac.goto(f"{origin}/voice", wait_until="load")
    mac.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    if mac.evaluate("() => document.getElementById('view-switch').getAttribute('aria-checked')") != "true":
        mac.click("#view-switch")
    other = Reader(Desk(mac, reader.desk.screenshots, f"{reader.desk.label}-mac", reader.desk.width, reader.desk.height),
                   cdp, reader.api)
    other.desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24",
                        "the Mac's tab did not open the channel in All")
    other.desk.settled()
    other.check(mac.evaluate("() => navigator.platform") == "MacIntel", "the Mac's platform was not emulated")
    listed = mac.evaluate("() => [...document.querySelectorAll('#keyboard-keys kbd')].map((kbd) => kbd.textContent)")
    assert isinstance(listed, list)
    other.check("Cmd+C or y" in listed, f"on a Mac the list of keys does not say Cmd+C or y: {listed}")
    # The Edit menu's copy: the browser's own copy command, whose event the page answers.
    highlight(other, "201")
    text = other.copied("Meta+c", ["copy"])
    other.check(text == other.content("201"), f"Cmd+C with the Edit menu's copy copied {text!r}")
    other.check(other.state()["status"] == "Copied message.", "Cmd+C did not say it copied")
    # No menu copying with it: the page's own way, a task after the key.
    other.key("j")
    reached = other.lit()
    other.check(len(reached) == 1, f"j highlighted {reached}")
    text = other.copied("Meta+c")
    other.check(text == other.content(reached[0][1]), f"Cmd+C with no copy command copied {text!r}")
    other.desk.shot("3-mac")
    reader.desk.notes.append("on an emulated Mac the list said Cmd+C or y; Cmd+C copied the highlighted message both "
                             "with the Edit menu's copy command and, a task later by the page's own way, without it")
    mac.close()


# --- a phone with a hardware keyboard -----------------------------------------------------------------


def phone_walk(reader: Reader) -> None:
    page = reader.page
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
    reader.desk.settled()
    # A tap on a short message: selected silently, nothing drawn, so nothing is highlighted to copy.
    box = page.locator("#discord-log > li[data-id='200']").bounding_box()
    reader.check(box is not None, "message 200 is not drawn")
    assert box is not None
    page.touchscreen.tap(box["x"] + box["width"] / 2, box["y"] + min(box["height"] / 2, 40))
    reader.desk.settled()
    reader.check(reader.lit() == [], f"a tap lit the message it selected: {reader.lit()}")
    reader.check(reader.copied("Control+c") == SENTINEL, "Ctrl+C after a tap's silent selection changed the clipboard")
    # The keyboard's j: on from the message tapped, highlighted.
    reader.key("j")
    lit = reader.lit()
    reader.check(len(lit) == 1 and lit[0][0] == "discord-log", f"j after the tap highlighted {lit}")
    message_id = lit[0][1]
    reader.check(reader.copied("Control+c") == reader.content(message_id),
                 f"Ctrl+C on the highlighted message {message_id} did not copy it")
    reader.check(reader.state()["status"] == "Copied message.", "the copy was not said on the phone")
    reader.check(reader.copied("y") == reader.content(message_id), f"y on the highlighted message {message_id} did not copy it")
    reader.check(number(reader.state(), "overflow") <= 0, "the phone's page scrolls sideways")
    reader.desk.shot("4-phone")
    reader.desk.notes.append(f"a tap selected silently and Ctrl+C copied nothing; j highlighted message {message_id}, "
                             "and Ctrl+C and y from the hardware keyboard copied it")


if __name__ == "__main__":
    raise SystemExit(main())
