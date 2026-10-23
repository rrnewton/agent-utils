#!/usr/bin/env python3
"""The Threads view's chips, Ctrl/Cmd+S there, the owner's centred main rows, and Back and Forward, in real
Chromium at 1280x800 and 393x852.

The owner, 2026-10-22, of the Threads view (Cmd+3) after `#419 threads-view-main-messages`:

* `#421 chip-states`: "The Main messages toggle ... doesn't change its visual style when it is toggled on
  off", and the chips are renamed Main channel and Unpack msgs. One rule presses every toggle chip.
* `#422 composer-key-stay`: "when I press Cmd-S even if I have Main messages view on, it jumps to a
  DIFFERENT view". The chord never changes the view when the view has a box.
* `#424 threads-main-own-center`: the owner's main-channel rows among the cards looked like the previous
  thread's follow-ups; they are centred, at their width.
* `#426 threads-sort-toggle`: "order by thread creation vs latest update", a chip saying the order.
* `#430 threads-main-own-sent`: "I still don't see the message that I just sent to the main channel":
  under Hide read the Threads view's main rows left the owner's own message out.
* `#432 read-mode-unify`: one read mode for every view, the cards included, key h saying it; Fold read
  (was Collapse read) folds runs of read threads into "... N read threads ...".
* `#440 thread-card-pack-toggle`: each thread packed and unpacked from its card, three ways; Unpack three
  ways, Unpack unread greying the read cards' edge; an up triangle packing a thread in place.
* `#423 back-key-consistency`: "if I press Cmd-[ on the thread list view it jumps back to the Main view but
  it does NOT have kind of navigation (or back?) effect on other views". Ctrl/Cmd+[ and ] walk back and
  forward through the places visited, in every view.

The fake-DOM suite (tests/js/voice_page.test.mjs) pins the rules; this measures them as drawn and pressed.
It serves the checked-out page and the fake API of tests/two_column_browser.py (24 messages, three
threads), two of Main's messages (220 and 223) the owner's, from one loopback origin. At each size: sign in
(All), the Threads view from the picker ->

1. the three chips at rest look alike, each pressed looks like the others pressed and unlike at rest
   (border, background and words, as computed), and each says its state in aria-pressed; named Pack all
   (Unpack's first state, #440), Main channel and Latest update (Main and Latest on a phone);
2. Main channel on -> Cmd+S from a card: the focus in the box at the foot of the cards, the Threads view
   kept; Reply on a main-channel message, Cmd+S: the reply's box, still the Threads view; Main channel off
   -> Cmd+S goes back to the view the list was chosen from (negative control);
3. Main channel and Unpack msgs on -> 220 (the owner's) centred in the cards' column within 2px, as wide as
   in All, its left edge at least 16px from the previous thread's last unpacked row's; 203 (the agent's)
   with the margins it has in All; 220 in All on the owner's side as before (negative controls);
4. Back and Forward: Ctrl+1 (Main), Ctrl+2 (All), Ctrl+3 (Threads), on a desk Ctrl+4 (two columns), a
   thread opened from the list, Settings -> Cmd+[ walks back through each in turn to where it started,
   Cmd+] forward again to the last, where it says so and stays; the bare ] is not Forward (the page
   tests cover Back at the start, Esc as a step back and a page opened on the thread list);
5. the order chip: the cards by latest update (replica, canary, runner), the chip saying so; d -> by
   creation (runner, replica, canary), the chip "Thread created" ("Created" on a phone) and pressed, the
   highlighted card still on screen; a reload keeps it; a press goes back; the chips beside the glass;
6. Hide read, Main channel on: a message sent from the box at the foot is among the main rows at once;
7. the replica and canary threads read on the server, reloaded: in All, h -> the button says Fold read, the
   status line says so, and run lines are drawn; the Threads view -> "... 2 read threads" over their two
   cards, the runner's card (unread, newest) drawn; a tap on the line opens it; h -> Hide read: the two read
   cards gone, the runner's left; h -> Show read: all three;
8. Unpack unread: the runner unpacked, the read replica and canary packed, their edges one grey and the
   runner's its colour; the read replica unpacked by a click on its own card there: its triangle grey as
   its edge (`#450 pack-triangle-hue`); Pack all: every edge its own colour; Unpack all, the triangle on
   the replica's first unpacked row (aria-label Pack thread): no disc behind it, larger than the triangle
   the disc held, a hit target of 32px at least, in the replica's own colour as its edge and bridge are;
   pressed, the replica packed, its card within a pixel of where it
   was; a click on the replica's card goes whole, unpacked, collapsed.

No page error and no sideways scroll at any step. Pass --screenshots DIR to keep a PNG of each state;
--web-root DIR to run against another copy of the page (an older one fails it at the chips).
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
from inline_reply_browser import ReplyColumnsApi, sign_in
from offline_cache_browser import OWNER_ID, SPEAKERS, Json, check, handler_for
from two_column_browser import CANARY, REPLICA, RUNNER, Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

SIZES = (("desk", 1280, 800, False), ("phone", 393, 852, True))

CHIPS = ("side-unpack", "side-main", "side-order")

# Main's messages the owner wrote: 220 between the canary thread's last move (218) and the runner's (221).
OWN_MAIN = ("220", "223")

# A chip's look and state, as computed.
CHIP_JS = """(id) => {
    const n = document.getElementById(id); const s = getComputedStyle(n);
    const r = n.getBoundingClientRect();
    return {pressed: n.getAttribute('aria-pressed'), text: n.innerText.trim(), drawn: n.getClientRects().length > 0,
            look: [s.borderTopColor, s.backgroundColor, s.color].join(' | '), left: r.left, right: r.right};
}"""

# Where the focus is, and the view.
STATE_JS = """() => ({focus: document.activeElement ? document.activeElement.id : null,
    layout: document.getElementById('screen-main').getAttribute('data-columns'),
    view: document.getElementById('thread-select').value,
    screen: document.getElementById('frame-body').getAttribute('data-screen'),
    overflow: document.documentElement.scrollWidth - window.innerWidth})"""

# Row `id` in list `list`, and the column it stands in, as drawn; with the last unpacked row before it.
ROW_JS = """([list, id]) => {
    const host = document.getElementById(list);
    const rows = [...host.children];
    const at = rows.findIndex((li) => li.getAttribute('data-id') === id);
    if (at < 0) return null;
    const row = rows[at]; const r = row.getBoundingClientRect(); const s = getComputedStyle(row);
    const h = host.getBoundingClientRect(); const hs = getComputedStyle(host);
    const left = h.left + parseFloat(hs.paddingLeft) + parseFloat(hs.borderLeftWidth);
    const right = h.right - parseFloat(hs.paddingRight) - parseFloat(hs.borderRightWidth);
    const prior = rows.slice(0, at).reverse().find((li) => li.hasAttribute('data-unpack-thread') && li.getClientRects().length > 0);
    const p = prior ? prior.getBoundingClientRect() : null;
    return {left: r.left, right: r.right, width: r.width, who: row.getAttribute('data-who'),
            main: row.hasAttribute('data-main-row'), marginLeft: s.marginLeft, marginRight: s.marginRight,
            column: {left, right}, prior: p ? {id: prior.getAttribute('data-id'), left: p.left,
            thread: prior.getAttribute('data-unpack-thread')} : null};
}"""


class PolishApi(ReplyColumnsApi):
    """`ReplyColumnsApi`, with two of Main's messages the owner's."""

    def __init__(self) -> None:
        super().__init__()
        for index, row in enumerate(self.messages):
            if str(row.get("id")) in OWN_MAIN:
                self.messages[index] = {**row, **SPEAKERS["me"]}

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope), "owner_author_id": OWNER_ID}


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per state into (default: none)")
    parser.add_argument("--port", type=int, default=0,
                        help="loopback port for the fake server (default: 0, any free port)")
    parser.add_argument("--only", choices=("chips", "compose", "center", "back", "order", "sent", "readmode", "pack"), action="append",
                        help="run only these parts of the walk, repeatable (default: all of them)")
    parser.add_argument("--size", choices=[size[0] for size in SIZES], action="append",
                        help="walk only at these sizes, repeatable: desk is 1280x800, phone 393x852 (default: both)")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required: python3 -m pip install --user playwright && "
                         "python3 -m playwright install chromium") from error
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = args.web_root.resolve()
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch in SIZES:
            if args.size and label not in args.size:
                continue
            report.append(walk(playwright.chromium, args, label, width, height, touch))
    for line in report:
        print(line)
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, touch: bool) -> str:
    api = PolishApi()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-threads-polish-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark", has_touch=touch, is_mobile=touch)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"threads-polish-{label}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            sign_in(desk)
            parts = args.only or ["chips", "compose", "center", "back", "order", "sent", "readmode", "pack"]
            if "chips" in parts:
                chips_walk(desk, touch)
            if "compose" in parts:
                compose_walk(desk)
            if "center" in parts:
                center_walk(desk)
            if "back" in parts:
                back_walk(desk)
            if "order" in parts:
                order_walk(desk, touch)
            if "sent" in parts:
                sent_walk(desk)
            # Last: it marks two threads read on the server, which every later step would see.
            if "readmode" in parts or "pack" in parts:
                read_mode_walk(desk, api)
            # After it, with the replica and canary threads read and the runner's not.
            if "pack" in parts:
                pack_walk(desk)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def state(desk: Desk) -> dict[str, object]:
    found = desk.page.evaluate(STATE_JS)
    assert isinstance(found, dict)
    desk.check(float(found["overflow"]) <= 0, f"the page scrolls sideways: {found}")
    return found


def to_threads(desk: Desk) -> None:
    """The Threads view from the picker, its three cards drawn."""
    desk.page.select_option("#thread-select", "threads")
    desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
                  " && document.querySelectorAll('#side-cards > li[data-card]').length === 3",
                  "the view picker did not show the thread list")
    desk.settled()


def set_chip(desk: Desk, chip: str, on: bool) -> None:
    """Press `chip` until it is on or off; Unpack goes round three states (`#440`), passing "mixed"."""
    page = desk.page
    for _ in range(3):
        if (page.get_attribute(f"#{chip}", "aria-pressed") == "true") == on and page.get_attribute(f"#{chip}", "aria-pressed") != "mixed":
            break
        page.click(f"#{chip}")
        desk.settled()
    desk.wait_for(f"() => document.getElementById('{chip}').getAttribute('aria-pressed') === '{'true' if on else 'false'}'",
                  f"#{chip} did not turn {'on' if on else 'off'}")
    desk.settled()


def chip(desk: Desk, name: str) -> dict[str, object]:
    found = desk.page.evaluate(CHIP_JS, name)
    assert isinstance(found, dict)
    return found


def chips_walk(desk: Desk, touch: bool) -> None:
    to_threads(desk)
    for name in CHIPS:
        set_chip(desk, name, False)
    rest = {name: chip(desk, name) for name in CHIPS}
    for name, found in rest.items():
        desk.check(found["drawn"] is True and found["pressed"] == "false", f"#{name} is not offered at rest: {found}")
    looks = {str(found["look"]) for found in rest.values()}
    desk.check(len(looks) == 1, f"the chips at rest do not look alike: {rest}")
    names = {name: str(found["text"]) for name, found in rest.items()}
    desk.check(names == ({"side-unpack": "Pack all", "side-main": "Main", "side-order": "Latest"} if touch else
                         {"side-unpack": "Pack all", "side-main": "Main channel", "side-order": "Latest update"}),
               f"the chips' names: {names}")
    pressed: dict[str, dict[str, object]] = {}
    for name in CHIPS:
        set_chip(desk, name, True)
        pressed[name] = chip(desk, name)
        desk.check(pressed[name]["look"] != rest[name]["look"], f"#{name} pressed looks as it does at rest: {pressed[name]['look']}")
        desk.shot(f"1-{name}-pressed")
        set_chip(desk, name, False)
        desk.check(chip(desk, name)["look"] == rest[name]["look"], f"#{name} let go does not look as it did at rest")
    looks = {str(found["look"]) for found in pressed.values()}
    desk.check(len(looks) == 1, f"the chips pressed do not look alike: {pressed}")
    for name in CHIPS:
        set_chip(desk, name, True)
    desk.shot("1-all-pressed")
    for name in CHIPS:
        set_chip(desk, name, False)
    state(desk)
    desk.notes.append(f"chips at rest [{rest['side-main']['look']}], pressed [{pressed['side-main']['look']}], all three alike each way")


def chord(desk: Desk, key: str) -> None:
    """Ctrl/Cmd+`key` as a Mac reader presses it: Cmd."""
    desk.page.keyboard.press(f"Meta+{key}")
    desk.settled()


def compose_walk(desk: Desk) -> None:
    page = desk.page
    to_threads(desk)
    set_chip(desk, "side-main", True)
    desk.wait_for("() => document.getElementById('channel-composer').parentNode.id === 'side-list'",
                  "Main channel did not put the main channel's box at the foot of the cards")
    page.focus("#side-cards > li[data-card] .side-card")
    chord(desk, "s")
    found = state(desk)
    desk.check(found["layout"] == "list" and found["view"] == "threads", f"Cmd+S left the Threads view: {found}")
    desk.check(found["focus"] == "channel-compose-text", f"Cmd+S did not focus the box at the foot: {found}")
    desk.shot("2-cmd-s-foot-box")
    # A reply open under a main-channel message is the box.
    page.evaluate("() => document.querySelector(\"#side-cards > li[data-main-row][data-id='203']\").scrollIntoView({block: 'center'})")
    page.evaluate("() => document.querySelector(\"#side-cards > li[data-id='203'] .reply-button\").click()")
    desk.wait_for("() => !document.getElementById('side-reply').hidden", "Reply on a main-channel message did not open the reply")
    page.focus("#side-scroll")
    chord(desk, "s")
    found = state(desk)
    desk.check(found["layout"] == "list" and found["focus"] == "reply-text", f"Cmd+S with a reply open: {found}")
    desk.shot("2-cmd-s-reply")
    page.click("#reply-cancel")
    desk.wait_for("() => document.getElementById('side-reply').hidden", "Cancel did not put the reply away")
    # Negative control: Main channel off, no box in the Threads view: back to the view it was chosen from (All).
    set_chip(desk, "side-main", False)
    page.focus("#side-scroll")
    chord(desk, "s")
    found = state(desk)
    desk.check(found["layout"] != "list" and found["view"] == "flat" and found["focus"] == "channel-compose-text",
               f"Cmd+S with Main channel off did not go back to All and its box: {found}")
    desk.notes.append("Cmd+S in the Threads view with Main channel on: the box at the foot, then the open reply, the view kept; "
                      "off, back to All")


def number(value: object) -> float:
    """A measurement the page answered, as a number."""
    return float(str(value))


def row(desk: Desk, list_id: str, message: str) -> dict[str, object]:
    found = desk.page.evaluate(ROW_JS, [list_id, message])
    desk.check(isinstance(found, dict), f"{message} is not in #{list_id}")
    assert isinstance(found, dict)
    return found


def center_walk(desk: Desk) -> None:
    page = desk.page
    page.select_option("#thread-select", "flat")
    desk.wait_for("() => document.getElementById('thread-select').value === 'flat'", "the picker did not show All")
    desk.settled()
    own_all, agent_all = row(desk, "discord-log", "220"), row(desk, "discord-log", "203")
    desk.check(own_all["who"] == "me" and agent_all["who"] == "coder", f"the premise: 220 the owner's, 203 the agent's: {own_all} {agent_all}")
    to_threads(desk)
    set_chip(desk, "side-main", True)
    set_chip(desk, "side-unpack", True)
    desk.wait_for(f"() => document.querySelectorAll(\"#side-cards > li[data-unpack-thread='{CANARY}']\").length >= 2",
                  "Unpack msgs did not unpack the canary thread")
    page.evaluate("() => document.querySelector(\"#side-cards > li[data-id='220']\").scrollIntoView({block: 'center'})")
    desk.settled()
    desk.shot("3-own-main-centred")
    own, agent = row(desk, "side-cards", "220"), row(desk, "side-cards", "203")
    desk.check(own["main"] is True and own["who"] == "me", f"the premise is 220 the owner's main row among the cards: {own}")
    column = own["column"]
    assert isinstance(column, dict)
    middle = (number(column["left"]) + number(column["right"])) / 2
    centre = (number(own["left"]) + number(own["right"])) / 2
    desk.check(abs(centre - middle) <= 2, f"220 is not centred: its centre {centre:.1f}px, the column's {middle:.1f}px")
    desk.check(abs(number(own["width"]) - number(own_all["width"])) <= 1.5,
               f"220 is not as wide as in All: {own['width']} against {own_all['width']}")
    prior = own["prior"]
    desk.check(isinstance(prior, dict) and prior["thread"] == CANARY, f"the premise: the canary thread's rows before 220: {prior}")
    assert isinstance(prior, dict)
    apart = abs(number(own["left"]) - number(prior["left"]))
    desk.check(apart >= 16, f"220's left edge is {apart:.1f}px from {prior['id']}'s, the canary thread's last unpacked row")
    # Negative controls: the agent's main row has the margins it has in All, and the owner's row in All is on his side.
    desk.check((agent["marginLeft"], agent["marginRight"]) == (agent_all["marginLeft"], agent_all["marginRight"]),
               f"203, the agent's, moved: {agent['marginLeft']}/{agent['marginRight']} against {agent_all['marginLeft']}/{agent_all['marginRight']} in All")
    desk.check(float(str(own_all["marginRight"]).removesuffix("px")) == 0 and float(str(own_all["marginLeft"]).removesuffix("px")) > 0,
               f"220 in All is not on the owner's side: {own_all['marginLeft']}/{own_all['marginRight']}")
    state(desk)
    set_chip(desk, "side-unpack", False)
    set_chip(desk, "side-main", False)
    desk.notes.append(f"220 centred ({centre:.1f}px against {middle:.1f}px), {number(own['width']):.0f}px wide ({number(own_all['width']):.0f}px "
                      f"in All), {apart:.0f}px left of {prior['id']}; 203 as in All")


def where(desk: Desk) -> str:
    """Where the reader is, as the walk names places: a screen, or the view on the channel."""
    found = state(desk)
    if found["screen"] != "main":
        return str(found["screen"])
    if found["layout"] == "two":
        return "two+thread" if desk.page.evaluate("() => !document.getElementById('side-pane').hidden") else "two"
    return "threads" if found["layout"] == "list" else str(found["view"])


def back_walk(desk: Desk) -> None:
    page = desk.page
    to_threads(desk)
    page.keyboard.press("Control+1")
    desk.wait_for("() => document.getElementById('thread-select').value === 'main'", "Ctrl+1 did not show Main")
    visits = ["main"]
    page.keyboard.press("Control+2")
    desk.wait_for("() => document.getElementById('thread-select').value === 'flat'", "Ctrl+2 did not show All")
    visits.append("flat")
    if desk.width >= 1000:
        page.keyboard.press("Control+4")
        desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'two'", "Ctrl+4 did not show two columns")
        visits.append("two")
    page.keyboard.press("Control+3")
    desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'", "Ctrl+3 did not show the thread list")
    visits.append("threads")
    desk.settled()
    page.click(f"#side-cards > li[data-id='{CANARY}'] .side-card-open")
    desk.wait_for("() => document.getElementById('thread-select').value.startsWith('thread:')", "the card did not open its thread")
    visits.append(str(page.evaluate("() => document.getElementById('thread-select').value")))
    desk.settled()
    page.keyboard.press(",")
    desk.wait_for("() => document.getElementById('frame-body').getAttribute('data-screen') === 'settings'", ", did not open Settings")
    visits.append("settings")
    desk.settled()
    desk.check(where(desk) == visits[-1], f"the premise is {visits[-1]}: {where(desk)}")
    walked = [where(desk)]
    for at in range(len(visits) - 2, -1, -1):
        chord(desk, "BracketLeft")
        now = where(desk)
        walked.append(now)
        desk.check(now == visits[at], f"Cmd+[ from {visits[at + 1]} went to {now}, not {visits[at]}")
        if at == len(visits) - 3:
            desk.shot("4-back-to-thread-list")
    # The bare ] is not Forward (it is the columns' key).
    page.keyboard.press("]")
    desk.settled()
    desk.check(where(desk) == "main", f"the bare ] moved: {where(desk)}")
    for at in range(1, len(visits)):
        chord(desk, "BracketRight")
        now = where(desk)
        desk.check(now == visits[at], f"Cmd+] from {visits[at - 1]} went to {now}, not {visits[at]}")
    chord(desk, "BracketRight")
    desk.check(where(desk) == "settings", "Forward past the last place moved")
    status = str(page.evaluate("() => document.getElementById('status').textContent"))
    desk.check(status == "Nothing to go forward to.", f"Forward at the end said {status!r}")
    desk.shot("4-forward-at-end")
    page.keyboard.press("Escape")
    desk.settled()
    desk.notes.append("Cmd+[ back through " + " <- ".join(reversed(walked)) + ", Cmd+] forward again; at the end it says so")



CARD_ORDER_JS = "() => [...document.querySelectorAll('#side-cards > li[data-card]')].map((li) => li.getAttribute('data-id'))"

# The order chip, the glass and the heading's other chips, as drawn.
ORDER_CHIP_JS = """() => {
    const box = (id) => { const n = document.getElementById(id); const r = n.getBoundingClientRect();
        return {left: r.left, right: r.right, drawn: n.getClientRects().length > 0}; };
    const chip = document.getElementById('side-order');
    return {text: chip.innerText.trim(), label: chip.getAttribute('aria-label'), pressed: chip.getAttribute('aria-pressed'),
            glass: box('search-toggle'), chips: ['side-unpack', 'side-main', 'side-order'].map(box)};
}"""


def order_walk(desk: Desk, touch: bool) -> None:
    page = desk.page
    to_threads(desk)
    updated, created = [REPLICA, CANARY, RUNNER], [RUNNER, REPLICA, CANARY]

    def chip_state() -> dict[str, object]:
        found = page.evaluate(ORDER_CHIP_JS)
        assert isinstance(found, dict)
        glass = found["glass"]
        assert isinstance(glass, dict)
        for one in found["chips"]:
            assert isinstance(one, dict)
            desk.check(one["drawn"] is True and float(one["right"]) <= float(glass["left"]) - 2 and float(one["left"]) >= 0,
                       f"a chip of the heading runs into the glass or off the window: {found['chips']} beside {glass}")
        return found

    found = chip_state()
    desk.check(page.evaluate(CARD_ORDER_JS) == updated, f"the cards are not by latest update: {page.evaluate(CARD_ORDER_JS)}")
    desk.check(found["text"] == ("Latest" if touch else "Latest update") and found["label"] == "Order: Latest update"
               and found["pressed"] == "false", f"the order chip at the default: {found}")
    # The canary card highlighted, then d: by creation, the card still on screen.
    page.click(f"#side-cards > li[data-id='{CANARY}'] .side-card")
    desk.settled()
    page.focus("#side-scroll")
    page.keyboard.press("d")
    desk.wait_for("() => document.getElementById('side-order').getAttribute('aria-pressed') === 'true'", "d did not change the order")
    desk.settled()
    found = chip_state()
    desk.check(page.evaluate(CARD_ORDER_JS) == created, f"the cards are not by creation: {page.evaluate(CARD_ORDER_JS)}")
    desk.check(found["text"] == ("Created" if touch else "Thread created") and found["label"] == "Order: Thread created",
               f"the order chip by creation: {found}")
    seen = page.evaluate(f"""() => {{ const a = document.getElementById('side-scroll').getBoundingClientRect();
        const r = document.querySelector("#side-cards > li[data-id='{CANARY}']").getBoundingClientRect();
        return r.bottom > a.top && r.top < a.bottom; }}""")
    desk.check(seen is True, "the highlighted card went off screen when the order changed")
    desk.shot("5-order-created")
    page.reload(wait_until="load")
    desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-card]').length === 3", "the reload did not show the cards")
    desk.settled()
    desk.check(page.evaluate(CARD_ORDER_JS) == created, "a reload forgot the order")
    page.click("#side-order")
    desk.wait_for("() => document.getElementById('side-order').getAttribute('aria-pressed') === 'false'", "a press did not go back to latest update")
    desk.check(page.evaluate(CARD_ORDER_JS) == updated, "a press did not put the cards back by latest update")
    desk.notes.append(f"cards by latest update {updated}, d by creation {created} with the highlighted card on screen, kept "
                      f"across a reload; the chips before the glass")



def sent_walk(desk: Desk) -> None:
    page = desk.page
    to_threads(desk)
    for _ in range(3):
        if page.get_attribute("#todo-filter", "data-read-mode") == "hide":
            break
        page.click("#todo-filter")
        desk.settled()
    desk.check(page.get_attribute("#todo-filter", "data-read-mode") == "hide", "the read mode button never reached Hide read")
    set_chip(desk, "side-main", True)
    words = f"sent from the foot at {desk.width}px"
    page.fill("#channel-compose-text", words)
    page.click("#channel-send")
    desk.wait_for(f"() => [...document.querySelectorAll('#side-cards > li[data-main-row]')].some((li) => li.innerText.includes({words!r}))",
                  "under Hide read the message just sent is not among the main rows")
    found = state(desk)
    desk.check(found["layout"] == "list", f"sending left the Threads view: {found}")
    desk.shot("6-own-sent-hide-read")
    set_chip(desk, "side-main", False)
    desk.notes.append("under Hide read the message sent from the foot box among the main rows at once")



# The replica's and the canary's messages, root and replies: their two threads read whole.
READ_THREADS = ("209", "210", "212", "214", "215", "216", "218")

READ_MODE_JS = """() => ({label: document.getElementById('todo-filter-label').innerText.replace(/\\s+/g, ' ').trim(),
    mode: document.getElementById('todo-filter').getAttribute('data-read-mode'),
    status: document.getElementById('status').textContent,
    lines: [...document.querySelectorAll('#discord-log > li[data-read-run=head], #side-cards > li[data-read-run=head]')]
        .filter((li) => li.getClientRects().length > 0).map((li) => li.querySelector('.read-run').innerText.replace(/\\s+/g, ' ').trim()).filter(Boolean),
    cards: [...document.querySelectorAll('#side-cards > li[data-card]')].filter((li) => li.getClientRects().length > 0
        && !li.hasAttribute('data-read-run')).map((li) => li.getAttribute('data-id'))})"""


def read_mode(desk: Desk) -> dict[str, object]:
    found = desk.page.evaluate(READ_MODE_JS)
    assert isinstance(found, dict)
    return found


def read_mode_walk(desk: Desk, api: PolishApi) -> None:
    page = desk.page
    with api.lock:
        api.dismissed.update(READ_THREADS)
    # Reloaded in All, so All is read afresh and knows its replies are Done; reopened on the thread list it
    # would be drawn from the rows held, the replies' Done arriving with All's own read later.
    page.select_option("#thread-select", "flat")
    desk.wait_for("() => document.getElementById('thread-select').value === 'flat'", "the picker did not show All")
    page.reload(wait_until="load")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id], #side-cards > li[data-card]').length > 0",
                  "the reload drew no list")
    page.select_option("#thread-select", "flat")
    desk.wait_for("() => document.getElementById('thread-select').value === 'flat'", "the picker did not show All")
    desk.settled()
    while page.get_attribute("#todo-filter", "data-read-mode") != "show":
        page.click("#todo-filter")
        desk.settled()
    page.focus("#scroll-area")
    page.keyboard.press("h")
    desk.settled()
    found = read_mode(desk)
    desk.check(found["mode"] == "fold" and found["label"] == "Fold read", f"h did not move the button to Fold read: {found}")
    desk.check(str(found["status"]).startswith("Fold read:"), f"h did not say Fold read: {found['status']!r}")
    lines = found["lines"]
    assert isinstance(lines, list)
    desk.check(len(lines) > 0 and all("read message" in str(line) for line in lines), f"Fold read by h drew no run line in All: {lines}")
    desk.shot("7-fold-read-all")
    to_threads(desk)
    found = read_mode(desk)
    desk.check(found["lines"] == ["… 2 read threads"], f"the two read threads are not one line: {found}")
    desk.check(found["cards"] == [RUNNER], f"the unread runner's card is not the one card drawn whole: {found}")
    desk.shot("7-fold-read-threads")
    page.click(f"#side-cards > li[data-id='{REPLICA}'] .read-run")
    desk.settled()
    found = read_mode(desk)
    desk.check(found["lines"] == [] and found["cards"] == [REPLICA, CANARY, RUNNER], f"a tap on the line did not open it: {found}")
    page.focus("#side-scroll")
    page.keyboard.press("h")
    desk.settled()
    found = read_mode(desk)
    desk.check(found["mode"] == "hide" and found["cards"] == [RUNNER], f"Hide read left a read thread's card: {found}")
    desk.shot("7-hide-read-threads")
    page.keyboard.press("h")
    desk.settled()
    found = read_mode(desk)
    desk.check(found["mode"] == "show" and found["cards"] == [REPLICA, CANARY, RUNNER], f"Show read did not bring every card back: {found}")
    state(desk)
    desk.notes.append("h in All: Fold read said and drawn; Threads view: '… 2 read threads' over the read cards, opened by a tap; "
                      "Hide read leaves the runner alone; Show read all three")



PACK_JS = """() => {
    const card = (id) => document.querySelector(`#side-cards > li[data-card][data-id='${id}']`);
    const info = (id) => { const li = card(id); const r = li.getBoundingClientRect();
        return {state: li.getAttribute('data-card-state'), edge: getComputedStyle(li).borderLeftColor, top: r.top,
                rows: document.querySelectorAll(`#side-cards > li[data-unpack-thread='${id}']`).length}; };
    return {mode: document.getElementById('side-cards').getAttribute('data-unpack'),
            chip: document.getElementById('side-unpack').innerText.trim(),
            pressed: document.getElementById('side-unpack').getAttribute('aria-pressed'),
            runner: info('RUNNER'), replica: info('REPLICA'), canary: info('CANARY')};
}""".replace("RUNNER", RUNNER).replace("REPLICA", REPLICA).replace("CANARY", CANARY)


# `#450 pack-triangle-hue`. A thread's pack triangle as drawn: the button's box and colour, the mark
# behind the triangle (there is to be none), the triangle itself (the mark's ::before: its width is its
# two transparent sides, its height its coloured base), and the card edge it is to match.
TRIANGLE_JS = """(id) => {
    const button = document.querySelector(`#side-cards > li[data-unpack-thread='${id}'] .unpack-pack`);
    if (!button) return null;
    const mark = button.querySelector('.unpack-pack-mark');
    const tri = getComputedStyle(mark, '::before');
    const card = document.querySelector(`#side-cards > li[data-card][data-id='${id}']`);
    const r = button.getBoundingClientRect(); const s = getComputedStyle(button); const m = getComputedStyle(mark);
    return {width: r.width, height: r.height, color: s.color, background: s.backgroundColor,
            outline: s.outlineColor, title: button.getAttribute('title'),
            markBorder: m.borderTopWidth, markBackground: m.backgroundColor, markRadius: m.borderTopLeftRadius,
            triWidth: parseFloat(tri.borderLeftWidth) + parseFloat(tri.borderRightWidth),
            triHeight: parseFloat(tri.borderBottomWidth), triColor: tri.borderBottomColor,
            edge: getComputedStyle(card).borderLeftColor};
}"""


def triangle(desk: Desk, thread_id: str) -> dict[str, object]:
    found = desk.page.evaluate(TRIANGLE_JS, thread_id)
    assert isinstance(found, dict), f"no triangle on {thread_id}'s first unpacked row"
    return found


def packs(desk: Desk) -> dict[str, object]:
    found = desk.page.evaluate(PACK_JS)
    assert isinstance(found, dict)
    return found


def unpack_to(desk: Desk, pressed: str) -> None:
    for _ in range(3):
        if desk.page.get_attribute("#side-unpack", "aria-pressed") == pressed:
            return
        desk.page.click("#side-unpack")
        desk.settled()
    desk.check(desk.page.get_attribute("#side-unpack", "aria-pressed") == pressed, f"Unpack never reached {pressed}")


def pack_walk(desk: Desk) -> None:
    page = desk.page
    to_threads(desk)
    unpack_to(desk, "mixed")
    desk.wait_for(f"() => document.querySelectorAll(\"#side-cards > li[data-unpack-thread='{RUNNER}']\").length > 0",
                  "Unpack unread did not unpack the unread runner")
    desk.settled()
    found = packs(desk)
    runner, replica, canary = found["runner"], found["replica"], found["canary"]
    assert isinstance(runner, dict) and isinstance(replica, dict) and isinstance(canary, dict)
    desk.check(found["chip"] == "Unpack unread" and found["mode"] == "unread", f"the chip in Unpack unread: {found}")
    desk.check(replica["rows"] == 0 and canary["rows"] == 0, f"Unpack unread unpacked a read thread: {found}")
    desk.check(replica["edge"] == canary["edge"] and replica["edge"] != runner["edge"],
               f"the read cards' edges are not one grey beside the runner's colour: {replica['edge']}, {canary['edge']}, {runner['edge']}")
    desk.shot("8-unpack-unread")
    grey = replica["edge"]
    # `#450 pack-triangle-hue`. The read replica unpacked by its own card under Unpack unread: its
    # triangle follows its edge to grey, where the unpacked runner's keeps its thread's colour.
    for want in ("expanded", "unpacked"):
        page.click(f"#side-cards > li[data-card][data-id='{REPLICA}'] .side-card")
        desk.wait_for(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{REPLICA}']\").getAttribute('data-card-state') === '{want}'",
                      f"a click on the read replica's card did not go to {want} under Unpack unread")
        desk.settled()
    held = triangle(desk, REPLICA)
    desk.check(held["color"] == grey and held["triColor"] == grey and held["edge"] == grey,
               f"under Unpack unread the read replica's triangle is {held['triColor']}, its edge {grey}")
    lively = triangle(desk, RUNNER)
    desk.check(lively["triColor"] == lively["edge"] and lively["triColor"] != grey,
               f"the unread runner's triangle is {lively['triColor']}, its edge {lively['edge']}")
    desk.shot("8-triangle-grey")
    page.click(f"#side-cards > li[data-unpack-thread='{REPLICA}'] .unpack-pack")
    desk.wait_for(f"() => document.querySelectorAll(\"#side-cards > li[data-unpack-thread='{REPLICA}']\").length === 0",
                  "the grey triangle did not pack the replica")
    desk.settled()
    unpack_to(desk, "false")
    found = packs(desk)
    replica, canary = found["replica"], found["canary"]
    assert isinstance(replica, dict) and isinstance(canary, dict)
    desk.check(found["chip"] == "Pack all" and replica["edge"] != grey and replica["edge"] != canary["edge"],
               f"Pack all left a read card's edge grey: {found}")
    # The triangle packs a thread in place: the replica's, mid-list, under Unpack all. (The runner's card is
    # the last: packing it shortens the list under the reader, and at the list's end the browser clamps.)
    unpack_to(desk, "true")
    desk.wait_for(f"() => document.querySelector(\"#side-cards > li[data-unpack-thread='{REPLICA}'] .unpack-pack\") !== null",
                  "the replica's first unpacked row holds no triangle")
    label = page.get_attribute(f"#side-cards > li[data-unpack-thread='{REPLICA}'] .unpack-pack", "aria-label")
    desk.check(label == "Pack thread", f"the triangle is named {label!r}")
    # `#450 pack-triangle-hue`. Plain, larger, and in the thread's own colour. The triangle the disc
    # held was 0.6rem wide and 0.42rem tall (9.6px by 6.7px at the page's root size); this one is at
    # least half as wide again each way, with nothing drawn behind it, on a target a thumb can hit.
    drawn = triangle(desk, REPLICA)
    desk.check(drawn["markBorder"] == "0px" and drawn["markBackground"] in ("rgba(0, 0, 0, 0)", "transparent")
               and drawn["markRadius"] == "0px",
               f"the triangle still stands in a disc: border {drawn['markBorder']}, background {drawn['markBackground']}, radius {drawn['markRadius']}")
    desk.check(float(str(drawn["triWidth"])) >= 15 and float(str(drawn["triHeight"])) >= 10,
               f"the triangle is {drawn['triWidth']}px by {drawn['triHeight']}px, no bigger than the one in the disc")
    desk.check(float(str(drawn["width"])) >= 32 and float(str(drawn["height"])) >= 32,
               f"the triangle's hit target is {drawn['width']}px by {drawn['height']}px")
    desk.check(drawn["color"] == drawn["edge"] and drawn["triColor"] == drawn["edge"],
               f"the triangle is {drawn['triColor']}, the replica's edge {drawn['edge']}")
    desk.check("Pack" in str(drawn["title"]) and "^" in str(drawn["title"]), f"the triangle's tooltip: {drawn['title']!r}")
    desk.notes.append(f"the pack triangle {float(str(drawn['triWidth'])):.0f}px by {float(str(drawn['triHeight'])):.0f}px on a "
                      f"{float(str(drawn['width'])):.0f}px target, in its card edge's colour {drawn['edge']}, grey {grey} under Unpack unread")
    page.evaluate(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{REPLICA}']\").scrollIntoView({{block: 'start'}})")
    desk.settled()
    before = packs(desk)["replica"]
    assert isinstance(before, dict)
    desk.shot("8-triangle")
    page.click(f"#side-cards > li[data-unpack-thread='{REPLICA}'] .unpack-pack")
    desk.wait_for(f"() => document.querySelectorAll(\"#side-cards > li[data-unpack-thread='{REPLICA}']\").length === 0",
                  "the triangle did not pack the replica")
    desk.settled()
    after = packs(desk)["replica"]
    assert isinstance(after, dict)
    moved = float(str(after["top"])) - float(str(before["top"]))
    desk.check(abs(moved) <= 1, f"packing moved the replica's card by {moved:.1f}px")
    unpack_to(desk, "false")
    # A click on the replica's card, which says its first message (the runner's says a summary, so goes two
    # ways): whole, unpacked, collapsed.
    for want in ("expanded", "unpacked", "collapsed"):
        page.click(f"#side-cards > li[data-card][data-id='{REPLICA}'] .side-card")
        desk.wait_for(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{REPLICA}']\").getAttribute('data-card-state') === '{want}'",
                      f"a click on the replica's card did not go to {want}")
        desk.settled()
        lit = page.evaluate("() => document.querySelectorAll('#side-cards > li[data-highlighted]').length")
        desk.check(lit == 0, "a click on a card of the thread list highlighted it")
    unpack_to(desk, "false")
    state(desk)
    desk.notes.append(f"Unpack unread: the runner unpacked, the read cards' edge {grey}; Pack all: their own colours; the "
                      f"triangle packed the replica, its card moved {moved:.1f}px; clicks on its card went whole, unpacked, collapsed")


if __name__ == "__main__":
    raise SystemExit(main())
