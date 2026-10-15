#!/usr/bin/env python3
"""Reply and Quote reply in two columns, inside the column they were pressed in, in real Chromium.

`#277 reply-in-column`. The owner, 2026-10-13: "Reply and quote-reply destroy two column mode. Again I
expected minimal disruption. Reply in the right column or left column, whichever I clicked the reply
button in. But instead the whole screen got replaced with the reply interface." So in two columns the
reply screen's own box opens at the foot of the column Reply was pressed in, under that column's list;
both columns stay on screen as they were, and Send or Cancel puts the box back and the column where it
was. The fake-DOM suite (tests/js/voice_page.test.mjs) pins where the box goes, what it posts and where
the keys go; it lays nothing out, so it cannot say what is on screen. This does, in the engine.

It serves the checked-out page and the fake API of tests/two_column_browser.py — a channel taller than
the window with three threads — answering a reply and the Done it makes as the service does, from one
loopback origin. At 1280x800 with a mouse and the keys, in two columns, the runner's thread selected on
the right, each column scrolled part way down its list, for Reply and for Quote reply (words dragged
over with the mouse, so Reply reads "Quote reply") in EACH column:

  the press -> the main screen is still up and the reply screen is not; both columns are drawn where
  they were, side by side, the other column not moved by a pixel and the pressed column's list starting
  where it did; the box is inside the pressed column, under its list, across neither the divider nor the
  window's foot, its chips standing on it rather than over it; the message answered is edged in the
  accent and keeps its top within a pixel; the focus is in the box, under the quote for Quote reply ->
  Escape on an empty box, Cancel, Send, or Enter -> the box gone, the reply screen still down, both
  columns back to the pixel, the message where it stood, the keys on the pressed column's list, and a
  sent reply posted where the reply screen posts it: Main's message to Main, the thread's into it.

Then a message low in the left column, which the box would cover: the column moves up by as little as
shows it above the box and its chips, never taking its top under the column's, and Cancel puts it back
to the pixel. Then the box dragged far taller than the window: it stops at a quarter of the window, and
the list over it keeps its height. Then a reply open in the right column when the window is narrowed to
one column: the reply screen takes it, words and all. Then the same in each column with the page's frames
held back behind the browser's (`#314 reply-focus-flake`), which lays the narrower window out and takes the
focus off the box it no longer draws before the page hands the reply on, as a loaded host does by chance:
the box on the reply screen has the focus all the same.

Then the service refusing a reply sent from each column, as a chat service that is down does: the
column still goes back to where it was, and the failure is ON SCREEN — the status line, a polite live
region, says the reply did not go and where its Retry is, and the column's own Newest chip, inside the
column, reads "Reply not sent" or "Reply unconfirmed" in the orange of the outbox row's words. Pressing
it brings that row, with Retry, into view in the column, and the chip says Newest again. (Before this
step, the reply screen's Send landing on the newest message was what showed such a row; Send from a
column stays put, and the row sat unseen at the column's foot with nothing said.)

Then the view keys pressed in a reply open in a column (`#281 view-keys`), real chords: Ctrl+4, two columns
already, leaves it open, its words and the focus in the box; Ctrl+2 in the left column's box is All in one
column with no reply screen over it, the box back on the reply screen out of sight, the words kept as the
row's draft and the keys on the list; and Ctrl+3 in the right column's box is the thread list across the
window, the reply put down in the same way. Ctrl+4 brings two columns back each time, the runner's thread
on the right.

Then one column at the desk's width: Reply is the reply screen, as it was. No page error and no sideways
scroll at any step.

Pass --screenshots DIR to keep a PNG of each open reply for review; --web-root DIR to run the same walk
against another copy of the page.
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
from offline_cache_browser import TOKEN, Json, check, handler_for, said, thread_of
from two_column_browser import RUNNER, THREAD_ROWS, ColumnsApi, Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

WIDTH, HEIGHT = 1280, 800
# The rows pressed: (column, how, close, message). Main's plain messages on the left, the runner's
# replies on the right; each is put a little under the head of its column first, where the box at the
# foot cannot reach it, so nothing should move at all.
RUNS = (
    ("left", "reply", "escape", "203"),
    ("left", "quote", "send", "207"),
    ("right", "reply", "enter", "206"),
    ("right", "quote", "cancel", "213"),
)
# The message put low in the left column, where the box would cover it.
LOW = "220"
# The replies the service refuses: (column, message, its answer, what the column's chip then says). A
# chat service that is down (503: the reply may have gone, unconfirmed), and one that says no (422).
REFUSALS: tuple[tuple[str, tuple[str, tuple[int, Json]], str], ...] = (
    ("left", ("205", (503, {"error": "upstream_unavailable", "detail": "chat service down"})), "Reply unconfirmed"),
    ("right", ("208", (422, {"error": "chat_error", "detail": "Google Chat returned HTTP 400"})), "Reply not sent"),
)
# The rows a reply is opened on before the window is narrowed under a slow frame (`slow_frame_run`): one
# of Main's on the left, one of the runner's replies on the right, neither holding a draft.
SLOW_FRAME_ROWS = (("left", "207"), ("right", "206"))
# Where a pressed row's top is put, below the top of its column's list, in px.
ROW_OFFSET_PX = 90
# The words a drag selects in a message, from its first character.
QUOTE_CHARS = 22


class ReplyColumnsApi(ColumnsApi):
    """`two_column_browser.ColumnsApi`, keeping what is posted as the service does: a reply into a
    thread is a reply in it, one to Main is on Main, and a Done is remembered for the reads after it."""

    def __init__(self) -> None:
        super().__init__()
        self.replies: list[Json] = []
        self.dismissed: set[str] = set()
        # While set, a reply is refused with this answer, as a chat service that is down refuses one, and
        # kept apart from those posted.
        self.refusing: tuple[int, Json] | None = None
        self.refused: list[Json] = []

    def post(self, path: str, body: Json) -> tuple[int, Json]:
        if path.endswith("/reply"):
            with self.lock:
                if self.refusing is not None:
                    self.refused.append(body)
                    return self.refusing
                self.replies.append(body)
                thread_id = body.get("thread_id")
                root = next((m for m in self.messages if thread_id and thread_of(m).get("id") == thread_id
                             and thread_of(m).get("is_root") is True), None)
                reply_to = body.get("reply_to")
                posted = said(100 + len(self.replies), str(body.get("text", "")), "me",
                              str(reply_to) if reply_to else None)
                if root is not None:
                    posted["thread"] = {**thread_of(root), "is_root": False}
                self.messages.append(posted)
                return 200, {"posted": posted, "parts": [posted]}
        if path.endswith("/dismiss"):
            ids = body.get("messages")
            done = [str(one) for one in ids] if isinstance(ids, list) else []
            with self.lock:
                self.dismissed.update(done)
            return 200, {"dismissed": done}
        return super().post(path, body)

    def timeline(self, query: dict[str, list[str]]) -> Json:
        answer = super().timeline(query)
        rows = answer.get("messages")
        with self.lock:
            done = [str(m["id"]) for m in rows if isinstance(m, dict) and str(m["id"]) in self.dismissed] \
                if isinstance(rows, list) else []
        return {**answer, "dismissed": done}

    def posted(self) -> list[Json]:
        with self.lock:
            return list(self.replies)


# Everything a press and a close can move, measured in the page: both columns' lists and how far each
# is scrolled, the row pressed, the box and where it is, the chips over each column's foot, the focus.
MEASURE_JS = """([column, id]) => {
    const box = (node) => {
        if (!node || !node.getClientRects().length) return null;
        const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height};
    };
    // Null-safe, so that a page without the places for the box (an older copy, `--web-root`) fails on
    // what this check is about rather than on a missing element.
    const shown = (id) => { const node = document.getElementById(id); return Boolean(node) && node.getClientRects().length > 0; };
    const list = column === 'left' ? 'discord-log' : 'side-log';
    const row = [...document.querySelectorAll(`#${list} > li[data-id]`)]
        .find((li) => (li.getAttribute('data-ids') || li.getAttribute('data-id')).split(' ').includes(id)) || null;
    const left = document.getElementById('scroll-area');
    const right = document.getElementById('side-scroll');
    // Where each column's chips begin, as the page's own keys reckon the foot of a list (`channelPane`
    // and `sidePane` in web/voice.js): the list's bottom less what floats over it.
    const over = (ids) => ids.reduce((room, one) => {
        const node = document.getElementById(one);
        return room + (node.hidden || !node.getClientRects().length ? 0 : node.getBoundingClientRect().height);
    }, 0);
    const leftFoot = left.getBoundingClientRect().top + left.clientHeight - over(['scroll-tools', 'status-line']);
    const sideJump = document.getElementById('side-jump-newest');
    const rightFoot = right.getBoundingClientRect().top + right.clientHeight -
        (sideJump.hidden ? 0 : document.getElementById('side-tools').getBoundingClientRect().height);
    const chips = (selector) => [...document.querySelectorAll(selector)]
        .filter((node) => !node.hidden && node.getClientRects().length).map(box);
    const held = document.activeElement;
    const compose = document.querySelector('.reply-compose');
    const text = document.getElementById('reply-text');
    return {
        left: box(left), leftScroll: left.scrollTop, right: box(right), rightScroll: right.scrollTop,
        leftColumn: box(left), rightColumn: box(document.getElementById('side-column')),
        leftFoot, rightFoot,
        leftChips: chips('#scroll-tools > *, #status-line'), rightChips: chips('#side-tools > *'),
        row: box(row), replying: Boolean(row && row.hasAttribute('data-replying')),
        rowEdge: row ? getComputedStyle(row).boxShadow : '',
        compose: box(compose), composeIn: compose.parentElement ? compose.parentElement.id : '',
        mounts: [shown('column-reply'), shown('side-reply')],
        screens: {main: shown('screen-main'), reply: shown('screen-reply')},
        focus: held ? held.id || held.tagName : '',
        focusIn: held && left.contains(held) ? 'left' : held && right.contains(held) ? 'right' : '',
        value: text.value, caret: text.selectionStart,
        dock: box(document.getElementById('dock')),
        overflow: document.documentElement.scrollWidth - window.innerWidth,
        windowHeight: window.innerHeight,
    };
}"""


class Found:
    """What `MEASURE_JS` measured, read with the types each part has."""

    def __init__(self, found: Json) -> None:
        self.found = found

    def box(self, name: str) -> dict[str, float]:
        part = self.found[name]
        assert isinstance(part, dict), f"{name} is not drawn"
        return {str(key): float(value) for key, value in part.items()}

    def drawn(self, name: str) -> bool:
        return isinstance(self.found[name], dict)

    def num(self, name: str) -> float:
        return float(str(self.found[name]))

    def text(self, name: str) -> str:
        return str(self.found[name])

    def flag(self, name: str) -> bool:
        return bool(self.found[name])

    def boxes(self, name: str) -> list[dict[str, float]]:
        part = self.found[name]
        assert isinstance(part, list)
        return [{str(key): float(value) for key, value in one.items()} for one in part if isinstance(one, dict)]

    def pair(self, name: str) -> tuple[bool, bool]:
        part = self.found[name]
        assert isinstance(part, list) and len(part) == 2
        return bool(part[0]), bool(part[1])

    def screens(self) -> tuple[bool, bool]:
        part = self.found["screens"]
        assert isinstance(part, dict)
        return bool(part["main"]), bool(part["reply"])


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
        help="directory to write one PNG per open reply into (default: none are kept)",
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
    with sync_playwright() as playwright:
        print(walk(playwright.chromium, args))
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace) -> str:
    """One reader at the laptop's size, against a fresh fake API and a fresh browser profile."""
    api = ReplyColumnsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-reply-column-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": WIDTH, "height": HEIGHT}, color_scheme="dark",
            )
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"reply-column-{WIDTH}", WIDTH, HEIGHT)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            open_columns(desk)
            for column, how, close, message_id in RUNS:
                reply_run(desk, api, column, how, close, message_id)
            low_run(desk)
            tall_box_run(desk)
            narrowed_run(desk)
            for column, message_id in SLOW_FRAME_ROWS:
                slow_frame_run(desk, column, message_id)
            for column, refusal, said in REFUSALS:
                refused_run(desk, api, column, refusal, said)
            view_keys_run(desk)
            one_column_run(desk)
            check(not errors, f"the page threw: {errors}")
            context.close()
            return f"{version} at {WIDTH}x{HEIGHT}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def open_columns(desk: Desk) -> None:
    """Signed in, on the channel, in two columns with the runner's thread on the right."""
    page = desk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24",
                  "the channel did not open in All")
    page.click("#column-toggle")
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length > 0"
                  " && document.querySelectorAll('#side-cards > li').length === 3",
                  "pressing the layout button did not give two columns with the cards")
    page.click(f"#side-cards > li[data-id='{RUNNER}'] .side-card-open")
    desk.wait_for(f"() => document.querySelectorAll('#side-log > li[data-id]').length === {THREAD_ROWS[RUNNER]}",
                  "the runner's card did not select its thread on the right")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 11",
                  "the left column is not Main")
    # The mouse out of both lists, so that what it is over selects nothing while the walk measures.
    page.mouse.move(WIDTH / 2, 5)
    desk.settled()


def scroller_of(column: str) -> str:
    return "scroll-area" if column == "left" else "side-scroll"


def list_of(column: str) -> str:
    return "discord-log" if column == "left" else "side-log"


def place_row(desk: Desk, column: str, message_id: str, top: float | None = None, bottom: float | None = None) -> None:
    """Scroll `column` so its row of `message_id` has its top `top` px under the list's top, or its
    bottom `bottom` px over the list's foot; and the other column part way down its own list."""
    other = scroller_of("right" if column == "left" else "left")
    desk.page.evaluate(f"""() => {{
        const area = document.getElementById('{scroller_of(column)}');
        const row = document.querySelector("#{list_of(column)} > li[data-id='{message_id}']");
        const a = area.getBoundingClientRect(), r = row.getBoundingClientRect();
        area.scrollTop += {'r.top - (a.top + ' + str(top) + ')' if top is not None
                           else 'r.bottom - (a.top + area.clientHeight - ' + str(bottom) + ')'};
        const other = document.getElementById('{other}');
        other.scrollTop = Math.round((other.scrollHeight - other.clientHeight) / 3);
    }}""")
    desk.settled()
    desk.settled()


def measure(desk: Desk, column: str, message_id: str) -> Found:
    found = desk.page.evaluate(MEASURE_JS, [column, message_id])
    assert isinstance(found, dict)
    return Found({str(key): value for key, value in found.items()})


def reply_button(desk: Desk, column: str, message_id: str) -> dict[str, float]:
    found = desk.page.evaluate(f"""() => {{
        const b = document.querySelector("#{list_of(column)} > li[data-id='{message_id}'] > .meta > .reply-button");
        if (!b) return null;
        const r = b.getBoundingClientRect();
        return {{x: r.left + r.width / 2, y: r.top + r.height / 2, text: b.textContent}};
    }}""")
    desk.check(isinstance(found, dict), f"{column} {message_id}: the row has no Reply")
    assert isinstance(found, dict)
    return {"x": float(found["x"]), "y": float(found["y"])}


def drag_words(desk: Desk, column: str, message_id: str) -> str:
    """Select the first words of the message's body with a real drag of the mouse; what is selected."""
    ends = desk.page.evaluate(f"""() => {{
        const body = document.querySelector("#{list_of(column)} > li[data-id='{message_id}'] > .body");
        const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
        let node = walker.nextNode();
        while (node && !node.textContent.trim()) node = walker.nextNode();
        const range = document.createRange();
        range.setStart(node, 0);
        range.setEnd(node, Math.min({QUOTE_CHARS}, node.textContent.length));
        const rects = [...range.getClientRects()];
        const first = rects[0];
        return {{x1: first.left + 1, x2: first.right - 1, y: (first.top + first.bottom) / 2}};
    }}""")
    assert isinstance(ends, dict)
    y = float(ends["y"])
    desk.page.mouse.move(float(ends["x1"]), y)
    desk.page.mouse.down()
    desk.page.mouse.move(float(ends["x2"]), y, steps=6)
    desk.page.mouse.up()
    desk.wait_for(f"""() => {{
        const b = document.querySelector("#{list_of(column)} > li[data-id='{message_id}'] > .meta > .reply-button");
        return b && b.textContent.startsWith('Quote');
    }}""", f"{column} {message_id}: words dragged over did not make Reply Quote reply")
    return str(desk.page.evaluate("() => String(getSelection())"))


def same_box(a: dict[str, float], b: dict[str, float], edges: tuple[str, ...] = ("left", "right", "top", "bottom")) -> bool:
    return all(abs(a[edge] - b[edge]) <= 0.5 for edge in edges)


def reply_run(desk: Desk, api: ReplyColumnsApi, column: str, how: str, close: str, message_id: str) -> None:
    page = desk.page
    where = f"{column} {how}"
    place_row(desk, column, message_id, top=ROW_OFFSET_PX)
    words = drag_words(desk, column, message_id) if how == "quote" else ""
    before = measure(desk, column, message_id)
    other = "right" if column == "left" else "left"
    button = reply_button(desk, column, message_id)
    page.mouse.click(button["x"], button["y"])
    mount = "column-reply" if column == "left" else "side-reply"
    desk.wait_for(f"() => document.querySelector('.reply-compose').parentElement.id === '{mount}'",
                  f"{where}: the box did not open in the column Reply was pressed in")
    desk.settled()
    now = measure(desk, column, message_id)
    shot_name = f"{column}-{how}"
    desk.shot(shot_name)

    # Both columns on screen, where they were; the reply screen down.
    main, reply = now.screens()
    desk.check(main and not reply, f"{where}: the reply screen replaced the columns (main {main}, reply {reply})")
    desk.check(now.pair("mounts") == ((True, False) if column == "left" else (False, True)),
               f"{where}: the box's places are drawn as {now.pair('mounts')}")
    desk.check(same_box(now.box(f"{other}Column"), before.box(f"{other}Column")) and
               abs(now.num(f"{other}Scroll") - before.num(f"{other}Scroll")) < 1,
               f"{where}: the other column moved: {before.box(f'{other}Column')} -> {now.box(f'{other}Column')}")
    pressed, pressed_before = now.box(column), before.box(column)
    desk.check(same_box(pressed, pressed_before, ("left", "right", "top")),
               f"{where}: the pressed column's list moved: {pressed_before} -> {pressed}")
    # The box: inside the pressed column, under its list, above the dock, clear of the divider.
    compose = now.box("compose")
    columns = now.box(f"{column}Column")
    desk.check(compose["left"] >= columns["left"] - 0.5 and compose["right"] <= columns["right"] + 0.5,
               f"{where}: the box {compose} is not inside its column {columns}")
    desk.check(compose["top"] >= pressed["bottom"] - 0.5, f"{where}: the box {compose} is not under the list {pressed}")
    dock = now.box("dock")
    desk.check(compose["bottom"] <= dock["top"] + 0.5 and compose["bottom"] <= now.num("windowHeight"),
               f"{where}: the box runs under the dock or off the window: {compose} {dock}")
    desk.check(compose["height"] >= 120, f"{where}: the box is only {compose['height']:.0f}px tall")
    desk.check(pressed["height"] >= 200, f"{where}: the list left over the box is only {pressed['height']:.0f}px tall")
    for chip in now.boxes(f"{column}Chips"):
        desk.check(chip["bottom"] <= compose["top"] + 0.5 or chip["top"] >= compose["bottom"] - 0.5,
                   f"{where}: a chip {chip} floats over the box {compose}")
    # The message answered: edged, and its top where it was (the box at the foot does not reach it).
    row, row_before = now.box("row"), before.box("row")
    desk.check(row_before["bottom"] <= now.num(f"{column}Foot") + 1,
               f"{where}: the row was put where the box covers it, so this proves nothing: {row_before}")
    desk.check(abs(row["top"] - row_before["top"]) <= 1 and abs(now.num(f"{column}Scroll") - before.num(f"{column}Scroll")) < 1,
               f"{where}: the message moved from {row_before['top']:.1f} to {row['top']:.1f}")
    desk.check(now.flag("replying") and "none" not in now.text("rowEdge"),
               f"{where}: the message answered is not edged ({now.text('rowEdge')})")
    desk.check(now.text("focus") == "reply-text", f"{where}: the focus is on {now.text('focus')}, not the box")
    if how == "quote":
        quote = " ".join(words.split())
        desk.check(now.text("value") == f"> {quote}\n\n", f"{where}: the box opened with {now.text('value')!r}, not the quote of {words!r}")
        desk.check(int(now.num("caret")) == len(now.text("value")), f"{where}: the caret is not under the quote")
    else:
        desk.check(now.text("value") == "", f"{where}: the box opened with {now.text('value')!r}")
    desk.check(now.num("overflow") <= 0, f"{where}: the page scrolls sideways")

    # Closed: by Escape on the empty box, Cancel, Send, or Enter.
    sent_before = len(api.posted())
    if close == "escape":
        page.keyboard.press("Escape")
    elif close == "cancel":
        page.click("#reply-cancel")
    else:
        page.keyboard.type("agreed, on it")
        if close == "send":
            page.click("#reply-send")
        else:
            page.keyboard.press("Enter")
        # The service names the reply after the replies before it (`ReplyColumnsApi.post`).
        posted_id = str(300 + sent_before + 1)
        desk.wait_for(f"() => !!document.querySelector(\"#{list_of(column)} > li[data-id='{posted_id}']\")",
                      f"{where}: the reply was not posted and drawn in its column")
    desk.wait_for("() => document.querySelector('.reply-compose').parentElement.id === 'screen-reply'",
                  f"{where}: {close} left the box in the column")
    desk.settled()
    desk.settled()
    after = measure(desk, column, message_id)
    main, reply = after.screens()
    desk.check(main and not reply, f"{where}: {close} went to another screen")
    desk.check(after.pair("mounts") == (False, False), f"{where}: {close} left a place for the box drawn")
    for name in ("left", "right"):
        desk.check(same_box(after.box(name), before.box(name)) and abs(after.num(f"{name}Scroll") - before.num(f"{name}Scroll")) <= 1,
                   f"{where}: {close} did not put the {name} column back: {before.box(name)} at "
                   f"{before.num(f'{name}Scroll'):.0f} -> {after.box(name)} at {after.num(f'{name}Scroll'):.0f}")
    desk.check(abs(after.box("row")["top"] - row_before["top"]) <= 1, f"{where}: {close} left the message moved")
    desk.check(not after.flag("replying"), f"{where}: {close} left the message edged")
    desk.check(after.text("focusIn") == column, f"{where}: after {close} the keys are on {after.text('focus')}, not the column's list")
    if close in ("send", "enter"):
        body = api.posted()[-1]
        desk.check(body.get("reply_to") == message_id and str(body.get("text", "")).endswith("agreed, on it"),
                   f"{where}: posted {body}")
        desk.check(body.get("thread_id") == (RUNNER if column == "right" else None),
                   f"{where}: posted into {body.get('thread_id')!r}, not where the reply screen posts it")
        if how == "quote":
            desk.check(str(body.get("text", "")).startswith("> "), f"{where}: the quote was not posted: {body}")
    else:
        desk.check(len(api.posted()) == sent_before, f"{where}: {close} posted something")
    desk.notes.append(f"{where}: in its column, {compose['height']:.0f}px under a {pressed['height']:.0f}px list, "
                      f"nothing moved; {close} put it back")


def low_run(desk: Desk) -> None:
    """A message low in the left column: the column moves up by as little as shows it over the box."""
    page = desk.page
    place_row(desk, "left", LOW, bottom=24)
    before = measure(desk, "left", LOW)
    button = reply_button(desk, "left", LOW)
    page.mouse.click(button["x"], button["y"])
    desk.wait_for("() => document.querySelector('.reply-compose').parentElement.id === 'column-reply'",
                  "low: the box did not open in the left column")
    desk.settled()
    now = measure(desk, "left", LOW)
    desk.shot("left-low")
    row, row_before = now.box("row"), before.box("row")
    foot = now.num("leftFoot")
    needed = row_before["bottom"] - foot
    desk.check(needed > 20, f"low: the box does not reach the row, so this proves nothing ({needed:.0f}px)")
    head = now.box("left")["top"]
    desk.check(row["bottom"] <= foot + 1 or abs(row["top"] - head) <= 1,
               f"low: the message {row} is still under the box and its chips (foot {foot:.1f})")
    desk.check(row["top"] >= head - 1, f"low: the message's top went under the column's top: {row} {head}")
    moved = now.num("leftScroll") - before.num("leftScroll")
    desk.check(moved <= needed + 1, f"low: the column moved {moved:.0f}px, more than the {needed:.0f}px that shows the message")
    desk.check(abs(now.num("rightScroll") - before.num("rightScroll")) < 1, "low: the right column moved")
    page.click("#reply-cancel")
    desk.wait_for("() => document.querySelector('.reply-compose').parentElement.id === 'screen-reply'",
                  "low: Cancel left the box in the column")
    desk.settled()
    after = measure(desk, "left", LOW)
    desk.check(abs(after.num("leftScroll") - before.num("leftScroll")) <= 1 and abs(after.box("row")["top"] - row_before["top"]) <= 1,
               f"low: Cancel did not put the column back: {before.num('leftScroll'):.0f} -> {after.num('leftScroll'):.0f}")
    desk.notes.append(f"a message low on the left moved up {moved:.0f}px of the {needed:.0f}px the box covered, and back on Cancel")


def tall_box_run(desk: Desk) -> None:
    """The box dragged far taller than the window, as its resize handle does: the list keeps its room."""
    page = desk.page
    place_row(desk, "right", "206", top=ROW_OFFSET_PX)
    button = reply_button(desk, "right", "206")
    page.mouse.click(button["x"], button["y"])
    desk.wait_for("() => document.querySelector('.reply-compose').parentElement.id === 'side-reply'",
                  "tall box: the box did not open in the right column")
    # What dragging the handle down past the window's foot leaves: an inline height far over the window.
    page.evaluate("() => { document.getElementById('reply-text').style.height = '2000px'; }")
    desk.settled()
    now = measure(desk, "right", "206")
    text = page.evaluate("() => document.getElementById('reply-text').getBoundingClientRect().height")
    desk.shot("right-tall-box")
    cap = HEIGHT / 4
    desk.check(float(text) <= cap + 1, f"tall box: the box was dragged to {float(text):.0f}px, over a quarter of the window ({cap:.0f}px)")
    desk.check(now.box("right")["height"] >= 200, f"tall box: the list over the box is only {now.box('right')['height']:.0f}px tall")
    compose, dock = now.box("compose"), now.box("dock")
    desk.check(compose["bottom"] <= dock["top"] + 0.5, f"tall box: the box runs under the dock: {compose} {dock}")
    page.evaluate("() => { document.getElementById('reply-text').style.height = ''; }")
    page.click("#reply-cancel")
    desk.wait_for("() => document.querySelector('.reply-compose').parentElement.id === 'screen-reply'",
                  "tall box: Cancel left the box in the column")
    desk.settled()
    desk.notes.append(f"the box dragged to 2000px stopped at {float(text):.0f}px, a {now.box('right')['height']:.0f}px list over it")


# What the walk reads after a refused reply: the status line, the column's chip, the outbox row the chip
# leads to and whether it is inside the column's list as drawn, and the colours of the chip and the row.
REFUSED_JS = """([column, text]) => {
    const box = (node) => {
        if (!node || !node.getClientRects().length) return null;
        const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom};
    };
    const line = document.getElementById('status-line');
    const said = document.getElementById('status');
    const jump = document.getElementById(column === 'left' ? 'jump-newest' : 'side-jump-newest');
    const area = document.getElementById(column === 'left' ? 'scroll-area' : 'side-scroll');
    const host = document.getElementById(column === 'left' ? 'outgoing-log' : 'side-outgoing-log');
    const row = [...host.children].find((li) => li.textContent.includes(text)) || null;
    const a = area.getBoundingClientRect();
    const r = row && row.getClientRects().length ? row.getBoundingClientRect() : null;
    const retry = row ? row.querySelector('.outgoing-retry') : null;
    return {
        line: line.hidden ? null : box(line), status: said.textContent, live: said.getAttribute('aria-live'),
        role: said.getAttribute('role'),
        jump: jump.hidden ? null : box(jump), jumpText: jump.textContent.trim(), jumpName: jump.getAttribute('aria-label') || '',
        jumpColour: getComputedStyle(jump).color,
        column: box(column === 'left' ? area : document.getElementById('side-column')),
        rowColour: row ? getComputedStyle(row.querySelector('.outgoing-status')).color : '',
        retry: Boolean(retry && retry.getClientRects().length),
        rowInView: Boolean(r && r.top >= a.top - 0.5 && r.bottom <= a.top + area.clientHeight + 0.5),
        scroll: area.scrollTop,
        window: {width: innerWidth, height: innerHeight},
    };
}"""


def refused_run(desk: Desk, api: ReplyColumnsApi, column: str, refusal: tuple[str, tuple[int, Json]], said: str) -> None:
    """The service refuses a reply sent from `column`: the column goes back where it was, and the
    failure is on screen and spoken, with a way down to its Retry. `#195 send-resilience`'s outbox row is
    what offers Retry; it is the one thing this step leads to rather than checks."""
    page = desk.page
    message_id, answer = refusal
    where = f"refused {column}"
    words = f"this one will not go ({column})"
    place_row(desk, column, message_id, top=ROW_OFFSET_PX)
    before = measure(desk, column, message_id)
    refused_before = len(api.refused)
    api.refusing = answer
    try:
        button = reply_button(desk, column, message_id)
        page.mouse.click(button["x"], button["y"])
        mount = "column-reply" if column == "left" else "side-reply"
        desk.wait_for(f"() => document.querySelector('.reply-compose').parentElement.id === '{mount}'",
                      f"{where}: the box did not open in its column")
        page.keyboard.type(words)
        # Nothing said before the press, so what is said after it is about this reply.
        page.evaluate("() => { document.getElementById('status').textContent = ''; }")
        page.click("#reply-send")
        desk.wait_for("() => /^Your reply /.test(document.getElementById('status').textContent)"
                      " && !document.getElementById('status-line').hidden",
                      f"{where}: nothing on screen said the reply did not go")
    finally:
        api.refusing = None
    desk.settled()
    desk.settled()
    desk.check(len(api.refused) == refused_before + 1, f"{where}: the reply was asked {len(api.refused) - refused_before} times, not once")
    found = page.evaluate(REFUSED_JS, [column, words])
    assert isinstance(found, dict)
    desk.shot(f"{column}-refused")
    after = measure(desk, column, message_id)
    # Where the reader was, as on every Send from a column.
    desk.check(abs(after.num(f"{column}Scroll") - before.num(f"{column}Scroll")) <= 1,
               f"{where}: the column did not go back where it was: {before.num(f'{column}Scroll'):.0f} -> {after.num(f'{column}Scroll'):.0f}")
    desk.check(after.text("focusIn") == column, f"{where}: the keys are on {after.text('focus')}, not the column's list")
    # Said: the status line, on screen, a polite live region, naming the column the Retry is in.
    window = found["window"]
    line = found["line"]
    status = str(found["status"])
    side = "left" if column == "left" else "right"
    desk.check(isinstance(line, dict) and line["top"] >= 0 and line["bottom"] <= window["height"] and line["right"] <= window["width"],
               f"{where}: the status line is not on screen: {line}")
    desk.check(found["live"] == "polite" and found["role"] == "status", f"{where}: the status line is not announced ({found['role']}, {found['live']})")
    desk.check(status.startswith("Your reply ") and f"the foot of the {side} column" in status,
               f"{where}: the status line says {status!r}")
    # Shown: the column's own chip, inside the column, saying why, in the outbox row's orange.
    jump = found["jump"]
    columns = found["column"]
    desk.check(isinstance(jump, dict) and isinstance(columns, dict) and jump["left"] >= columns["left"] - 0.5
               and jump["right"] <= columns["right"] + 0.5 and jump["bottom"] <= window["height"],
               f"{where}: the column's chip is not on screen inside it: {jump} in {columns}")
    desk.check(found["jumpText"] == said, f"{where}: the chip says {found['jumpText']!r}, not {said!r}")
    desk.check(str(found["jumpName"]).startswith("Jump to your reply, which "), f"{where}: the chip is named {found['jumpName']!r}")
    desk.check(found["jumpColour"] == found["rowColour"] and found["rowColour"] != "",
               f"{where}: the chip is drawn in {found['jumpColour']}, not the outbox row's {found['rowColour']}")
    desk.check(not found["rowInView"], f"{where}: the outbox row is in view already, so the chip proves nothing")
    # Led: the chip brings the row with Retry into view in the column, and says Newest again.
    page.click(f"#{'jump-newest' if column == 'left' else 'side-jump-newest'}")
    desk.settled()
    desk.settled()
    down = page.evaluate(REFUSED_JS, [column, words])
    assert isinstance(down, dict)
    desk.shot(f"{column}-refused-down")
    desk.check(down["rowInView"] and down["retry"], f"{where}: the chip did not bring the row with Retry into view")
    desk.check(down["jump"] is None or down["jumpText"] in ("Newest", ""), f"{where}: down by the row, the chip still says {down['jumpText']!r}")
    # Out of the outbox, so the walk after this one reads the columns as they were.
    page.click(f"#{'outgoing-log' if column == 'left' else 'side-outgoing-log'} > li .outgoing-dismiss")
    desk.wait_for(f"() => ![...document.getElementById('{'outgoing-log' if column == 'left' else 'side-outgoing-log'}').children]"
                  f".some((li) => li.textContent.includes({words!r}))", f"{where}: Dismiss left the row")
    desk.notes.append(f"{where}: said \"{status}\", the chip read {said!r}, and led to Retry")


def narrowed_run(desk: Desk) -> None:
    """Two columns gone under a reply open in one: the reply screen takes it, with its words."""
    page = desk.page
    place_row(desk, "right", "211", top=ROW_OFFSET_PX)
    button = reply_button(desk, "right", "211")
    page.mouse.click(button["x"], button["y"])
    desk.wait_for("() => document.querySelector('.reply-compose').parentElement.id === 'side-reply'",
                  "narrowed: the box did not open in the right column")
    page.keyboard.type("still writing this")
    page.set_viewport_size({"width": 900, "height": HEIGHT})
    desk.wait_for("() => document.getElementById('screen-reply').getClientRects().length > 0"
                  " && document.querySelector('.reply-compose').parentElement.id === 'screen-reply'",
                  "narrowed: one column did not hand the reply to the reply screen")
    value = str(page.evaluate("() => document.getElementById('reply-text').value"))
    desk.check(value == "still writing this", f"narrowed: the reply screen has {value!r}, not the words written")
    desk.check(page.evaluate("() => document.activeElement && document.activeElement.id") == "reply-text",
               "narrowed: the box lost the focus on the way to the reply screen")
    desk.shot("narrowed")
    page.click("#reply-cancel")
    page.set_viewport_size({"width": WIDTH, "height": HEIGHT})
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length > 0"
                  " && document.getElementById('thread-select').value === 'columns'",
                  "narrowed: two columns did not come back")
    desk.notes.append("narrowed to 900px, the reply open on the right moved to the reply screen with its words")


# `#314 reply-focus-flake`. The page's own frames held back, each run this long after the browser's: the
# browser lays the narrowed window out without the column, and takes the focus off the box it no longer
# draws, before the page's frame that hands the reply to the reply screen. A loaded host does that now and
# then by chance (the narrowed step above failed so about one run in ten under contention); this does it
# every time.
HELD_FRAME_MS = 250

# Hold the page's frames, and note what happens to the box's focus before the first of them runs: a
# `focusout` then is the browser's alone, since the page hands the reply on only in a frame.
HOLD_FRAMES_JS = """(ms) => {
    const held = {original: window.requestAnimationFrame, waiting: 0, ran: false, dropped: null};
    const box = document.getElementById('reply-compose');
    held.note = (event) => {
        if (held.ran || held.dropped) return;
        held.dropped = {to: event.relatedTarget ? event.relatedTarget.id || event.relatedTarget.tagName : null,
                        in: box.parentElement ? box.parentElement.id : ''};
    };
    box.addEventListener('focusout', held.note);
    const frame = held.original.bind(window);
    window.requestAnimationFrame = (callback) => frame(() => {
        held.waiting += 1;
        setTimeout(() => {
            held.waiting -= 1;
            held.ran = true;
            callback(performance.now());
        }, ms);
    });
    window.__heldFrames = held;
}"""

# The page's frames as they were, the focus no longer watched.
RELEASE_FRAMES_JS = """() => {
    const held = window.__heldFrames;
    window.requestAnimationFrame = held.original;
    document.getElementById('reply-compose').removeEventListener('focusout', held.note);
}"""


def slow_frame_run(desk: Desk, column: str, message_id: str) -> None:
    """A window narrowed under a reply open in `column`, the page's frame later than the browser's own
    layout: the reply screen takes the reply with the focus in its box, as it does on a quick frame."""
    page = desk.page
    where = f"slow frame {column}"
    mount = "column-reply" if column == "left" else "side-reply"
    desk.wait_for(f"() => !!document.querySelector(\"#{list_of(column)} > li[data-id='{message_id}']\")",
                  f"{where}: the column does not show {message_id}")
    place_row(desk, column, message_id, top=ROW_OFFSET_PX)
    button = reply_button(desk, column, message_id)
    page.mouse.click(button["x"], button["y"])
    desk.wait_for(f"() => document.querySelector('.reply-compose').parentElement.id === '{mount}'",
                  f"{where}: the box did not open in the column")
    page.keyboard.type("halfway through a sentence")
    desk.settled()
    page.evaluate(HOLD_FRAMES_JS, HELD_FRAME_MS)
    page.set_viewport_size({"width": 900, "height": HEIGHT})
    desk.wait_for("() => document.getElementById('screen-reply').getClientRects().length > 0"
                  " && document.querySelector('.reply-compose').parentElement.id === 'screen-reply'",
                  f"{where}: one column did not hand the reply to the reply screen")
    found = page.evaluate("""() => ({
        held: window.__heldFrames.ran, dropped: window.__heldFrames.dropped,
        focus: document.activeElement ? document.activeElement.id || document.activeElement.tagName : '',
        value: document.getElementById('reply-text').value,
    })""")
    assert isinstance(found, dict)
    # The premise: the page's frame came after the browser had taken the focus off the box, still in its
    # column. Without it this step is the narrowed step again, and proves nothing about a slow frame.
    dropped = found["dropped"]
    desk.check(found["held"] is True and isinstance(dropped, dict) and dropped["to"] is None and dropped["in"] == mount,
               f"{where}: the browser had not taken the focus off the box before the page's frame ({dropped}), "
               "so this proves nothing")
    desk.check(found["value"] == "halfway through a sentence", f"{where}: the reply screen has {found['value']!r}, not the words written")
    desk.check(found["focus"] == "reply-text", f"{where}: the box lost the focus on the way to the reply screen, to {found['focus']!r}")
    desk.shot(f"slow-frame-{column}")
    page.evaluate(RELEASE_FRAMES_JS)
    desk.wait_for("() => window.__heldFrames.waiting === 0", f"{where}: the page's held frames did not run")
    # Emptied before Cancel, so that no draft is kept for the steps after this one to open.
    page.keyboard.press("Control+A")
    page.keyboard.press("Backspace")
    page.click("#reply-cancel")
    page.set_viewport_size({"width": WIDTH, "height": HEIGHT})
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length > 0"
                  " && document.getElementById('thread-select').value === 'columns'",
                  f"{where}: two columns did not come back")
    desk.settled()
    desk.notes.append(f"narrowed with the page's frame {HELD_FRAME_MS}ms behind the browser's, which had taken the "
                      f"focus off the box, the reply open on the {column} moved to the reply screen, the box keeping the focus")


# The view keys' state: the picker, the layout, where the reply's box is and what it holds, and the keys.
VIEW_STATE_JS = """() => ({
    picker: document.getElementById('thread-select').value,
    layout: document.getElementById('screen-main').getAttribute('data-columns'),
    box: document.querySelector('.reply-compose').parentElement.id,
    words: document.getElementById('reply-text').value,
    replyScreen: document.getElementById('screen-reply').getClientRects().length > 0,
    focus: document.activeElement ? document.activeElement.id || document.activeElement.className : null,
    leftKeys: document.getElementById('scroll-area').contains(document.activeElement),
    rightKeys: document.getElementById('side-scroll').contains(document.activeElement),
    across: document.getElementById('side-column').getBoundingClientRect().width,
})"""


def view_keys_run(desk: Desk) -> None:
    """`#281 view-keys`: real Ctrl+4, Ctrl+2 and Ctrl+3 in a reply open in a column."""
    page = desk.page

    def state() -> Json:
        found = page.evaluate(VIEW_STATE_JS)
        assert isinstance(found, dict)
        return found

    def back_to_columns() -> None:
        # The runner's thread on the right and Main on the left: its reply 213 on the right and not on the
        # left. Not counted: the replies this walk has sent are in both lists by now.
        page.keyboard.press("Control+4")
        desk.wait_for("() => document.getElementById('thread-select').value === 'columns'"
                      " && document.getElementById('side-column').getClientRects().length > 0"
                      " && !document.getElementById('side-pane').hidden"
                      " && Boolean(document.querySelector(\"#side-log > li[data-id='213']\"))"
                      " && document.querySelectorAll('#discord-log > li[data-id]').length > 0"
                      " && !document.querySelector(\"#discord-log > li[data-id='213']\")",
                      "view keys: Ctrl+4 did not bring two columns back with the runner's thread on the right")
        desk.settled()

    # The left column: Ctrl+4 keeps the reply; Ctrl+2 is All in one column, the reply put down.
    place_row(desk, "left", "203", top=ROW_OFFSET_PX)
    button = reply_button(desk, "left", "203")
    page.mouse.click(button["x"], button["y"])
    desk.wait_for("() => document.querySelector('.reply-compose').parentElement.id === 'column-reply'",
                  "view keys: the box did not open in the left column")
    page.keyboard.type("left words")
    page.keyboard.press("Control+4")
    desk.settled()
    kept = state()
    desk.check(kept["box"] == "column-reply" and kept["focus"] == "reply-text" and kept["words"] == "left words"
               and kept["layout"] == "two", f"view keys: Ctrl+4 in two columns did not leave the reply as it was: {kept}")
    page.keyboard.press("Control+2")
    desk.wait_for("() => document.getElementById('thread-select').value === 'flat'"
                  " && document.getElementById('side-column').getClientRects().length === 0"
                  " && document.querySelector('.reply-compose').parentElement.id === 'screen-reply'",
                  "view keys: Ctrl+2 in the left column's box did not show All and put the reply down")
    desk.settled()
    all_view = state()
    desk.check(not all_view["replyScreen"], f"view keys: Ctrl+2 showed the reply screen over All: {all_view}")
    desk.check(all_view["leftKeys"] is True, f"view keys: after Ctrl+2 the keys are on {all_view['focus']!r}, not the list")
    desk.check(all_view["words"] == "left words", f"view keys: a chord typed into the box: {all_view['words']!r}")
    draft = page.evaluate("() => { const m = document.querySelector(\"#discord-log > li[data-id='203'] .draft-marker\");"
                          " return Boolean(m) && !m.hidden; }")
    desk.check(draft is True, "view keys: the words of the reply Ctrl+2 put down were not kept as the row's draft")
    desk.shot("view-keys-all")
    back_to_columns()

    # The right column: Ctrl+3 is the thread list across the window, the reply put down as well.
    place_row(desk, "right", "213", top=ROW_OFFSET_PX)
    button = reply_button(desk, "right", "213")
    page.mouse.click(button["x"], button["y"])
    desk.wait_for("() => document.querySelector('.reply-compose').parentElement.id === 'side-reply'",
                  "view keys: the box did not open in the right column")
    page.keyboard.type("right words")
    page.keyboard.press("Control+3")
    desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
                  " && document.querySelectorAll('#side-cards > li[data-card]').length === 3"
                  " && document.querySelector('.reply-compose').parentElement.id === 'screen-reply'",
                  "view keys: Ctrl+3 in the right column's box did not show the thread list and put the reply down")
    desk.settled()
    listed = state()
    desk.check(not listed["replyScreen"] and listed["rightKeys"] is True and listed["words"] == "right words"
               and float(str(listed["across"])) >= WIDTH - 1,
               f"view keys: the thread list after Ctrl+3 in the box is not what it should be: {listed}")
    desk.shot("view-keys-thread-list")
    # Through one column, so that two columns' button goes back to one column again for the walk after this.
    page.keyboard.press("Control+1")
    desk.wait_for("() => document.getElementById('thread-select').value === 'main'"
                  " && document.getElementById('side-column').getClientRects().length === 0",
                  "view keys: Ctrl+1 did not leave the thread list for Main")
    back_to_columns()
    page.mouse.move(WIDTH / 2, 5)
    desk.settled()
    desk.notes.append("view keys in a reply open in a column: Ctrl+4 left it open; Ctrl+2 on the left was All in one "
                      "column and Ctrl+3 on the right the thread list across the window, each putting the reply down "
                      "with its words kept, no reply screen over the view, nothing typed")


def one_column_run(desk: Desk) -> None:
    """One column at the desk's width: Reply is the reply screen, as it was."""
    page = desk.page
    page.click("#column-toggle")
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length === 0",
                  "one column: the layout button did not go back to one column")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0", "one column: no rows")
    message_id = str(page.evaluate("""() => {
        const area = document.getElementById('scroll-area').getBoundingClientRect();
        const row = [...document.querySelectorAll('#discord-log > li[data-id]')].find((li) => {
            const b = li.querySelector(':scope > .meta > .reply-button');
            if (!b) return false;
            const r = b.getBoundingClientRect();
            return r.top > area.top + 60 && r.bottom < area.bottom - 120;
        });
        return row ? row.getAttribute('data-id') : '';
    }"""))
    desk.check(message_id != "", "one column: no Reply on screen to press")
    button = reply_button(desk, "left", message_id)
    page.mouse.click(button["x"], button["y"])
    desk.wait_for("() => document.getElementById('screen-reply').getClientRects().length > 0",
                  "one column: Reply did not open the reply screen")
    desk.check(page.evaluate("() => document.querySelector('.reply-compose').parentElement.id") == "screen-reply",
               "one column: the box is not on the reply screen")
    page.click("#reply-cancel")
    desk.wait_for("() => document.getElementById('screen-main').getClientRects().length > 0",
                  "one column: Cancel did not come back")
    desk.notes.append("one column: Reply is the reply screen, as before")


if __name__ == "__main__":
    raise SystemExit(main())
