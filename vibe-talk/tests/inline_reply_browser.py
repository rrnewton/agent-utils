#!/usr/bin/env python3
"""Reply in the stream, under the message and its replies, in real Chromium at 1280x800 and 393x852.

`#410 inline-reply-editor`. The owner, 2026-10-21: "When I press 'reply' on a message inside a thread in
two column view, I see a reply box pop up AT THE BOTTOM. But I STILL see the other 'inline' reply box at
the bottom of the thread message series ... Why don't we move the 'Reply' box to also be an inline box and
NOT show two at once ... gather the existing replies underneath the message we just pressed Reply on, and
then our new Reply interface is just the existing inline-message editor (with Grow button) inserted in
the stream." The fake-DOM suite (tests/js/voice_page.test.mjs) pins where the box is put and what it
posts; it lays nothing out, so it cannot say that the box is drawn under its row, with the later rows
below it, and one box on screen. This does, in the engine.

It serves the checked-out page and the fake API of tests/two_column_browser.py — a channel taller than
the window with three threads — answering a reply as the service does, from one loopback origin.

At 1280x800 with a mouse, in two columns (Main on the left, the runner's thread on the right):

  Reply on an old message mid-thread on the right, scrolled into the middle of the column -> still the
  main screen and two columns; the reply's box directly under the row, as wide as the thread's rows, the
  later rows of the thread below it and the row after it moved down by exactly the box; the row itself
  not moved; one message box on screen, the thread's own "Reply in this thread…" hidden -> Grow ticked
  and three lines typed: the box grows and the rows below go down with it, never under it -> Cancel:
  the box gone, the thread's box back, the row where it was, a Draft on the row; Reply again: the words.
  Then Reply on the replica thread's root in Main on the left: its three replies come up under it as
  rows of Main, joined by the bridge, the box under the last of them, later messages below -> Send:
  posted where Reply on Main posts, the reply joining the replies under the root, the box gone and
  Main's box back, the root where it was.

At 1280x800 in one column, All: Reply on the runner's root, mid-stream: its replies gathered under it on
the bridge, the box under the last, later rows below; Escape puts the box away and the replies back in
time order.

At 393x852 by touch, All: Reply on an old message mid-stream: the box under the row, one box on screen,
later rows below -> Send: posted, the box gone and the channel's box back, the row not moved.

Then, in two columns, the service refusing a reply sent from the right column: the column not moved, the
status line saying the reply was not sent and that its Retry is under the message it answers, and the row
with Retry drawn there, directly under the row the box was under (`#467 inline-send-in-place`), with no
Newest chip leading down to it.

`#467 inline-send-in-place`, at 1280x800 in All: a reply sent from the box in the stream while the service
holds its post: its "Sending…" row directly under the row the box was under, as wide as that row, the rows
after it moved down by it and no more, the row answered not moved; the post answered: the row gone, and
nothing above it moved.

`#466 reply-promotes-thread`, at 1280x800 on a Slack-like service, where every message heads a thread of its
own nobody has answered and a reply is posted in it: Reply on such a message in Main, sent while the post is
held: the message's tag reads Thread(1) at once, in a thread's colour, the "Sending…" row under it; its tag
opens the thread, which shows that row under its root; the post answered, the reply is drawn in the thread
and the row is gone; back on Main the tag still reads Thread(1). And the view keys pressed in a reply open on the left: a
real Ctrl+4 leaves it open, its words and the focus in the box; Ctrl+2 is All in one column with the reply
put down, its words the row's draft.

Then two columns collapsing with a reply open, at 1280x800 narrowed to 393x852: from All, the reply open
on the right stays open under its message in the one column, with its words and the focus; from Main,
where the message is not drawn, it is put away and opens again, with its words, once its thread is
opened. No horizontal overflow and no page error at any step.

Pass --screenshots DIR to keep a PNG of each open reply; --web-root DIR to run the walk against another
copy of the page (an older one fails it at the first open reply).
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
import page_sources

if TYPE_CHECKING:
    from collections.abc import Callable

    from playwright.sync_api import BrowserType

DESK = (1280, 800)
PHONE = (393, 852)
# The gap the stylesheet leaves between a row and the box under it (0.25rem), and a pixel either way.
UNDER_PX = (0, 8)


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
        # While set, a reply's post waits for it before it is answered, as a slow chat service's does.
        self.holding: threading.Event | None = None

    def post(self, path: str, body: Json) -> tuple[int, Json]:
        if path.endswith("/reply"):
            holding = self.holding
            if holding is not None:
                holding.wait(timeout=30)
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
                if root is None and reply_to and self.replies_in_thread:
                    # Slack's server posts a reply in the thread of the message it answers.
                    root = next((m for m in self.messages if str(m["id"]) == str(reply_to)
                                 and thread_of(m).get("is_root") is True), None)
                if root is not None:
                    posted["thread"] = {**thread_of(root), "is_root": False, "reply_count": None,
                                        "reply_count_exact": False}
                self.messages.append(posted)
                return 200, {"posted": posted, "parts": [posted]}
        if path.endswith("/dismiss"):
            ids = body.get("messages")
            done = [str(one) for one in ids] if isinstance(ids, list) else []
            with self.lock:
                self.dismissed.update(done)
            return 200, {"dismissed": done}
        return super().post(path, body)

    replies_in_thread = False

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


class SlackLikeApi(ReplyColumnsApi):
    """`#466 reply-promotes-thread`. The same channel on a service like Slack: a message in no thread heads
    a thread of its own nobody has answered (its ts the thread's id, a count of none, exact), and a reply
    to it is posted in that thread."""

    replies_in_thread = True

    def __init__(self) -> None:
        super().__init__()
        for row in self.messages:
            if not thread_of(row):
                row["thread"] = {"id": f"ts-{row['id']}", "root_message_id": row["id"], "is_root": True,
                                 "reply_count": 0, "reply_count_exact": True}

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope), "replies_in_thread": True}


# The open reply and what is around it: the row answered, the rows after it, the box, which message
# boxes are drawn, the list's box and where it is scrolled. `list` is the list the row is in.
MEASURE_JS = """([list, id]) => {
    const box = (node) => {
        if (!node || !node.getClientRects().length) return null;
        const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height};
    };
    const drawn = (id) => { const n = document.getElementById(id); return Boolean(n) && n.getClientRects().length > 0; };
    const rows = [...document.querySelectorAll(`#${list} > li[data-id]`)];
    const at = rows.findIndex((li) => (li.getAttribute('data-ids') || li.getAttribute('data-id')).split(' ').includes(id));
    const row = at < 0 ? null : rows[at];
    const anchor = document.querySelector(`#${list} > li[data-inline-reply]`);
    const after = anchor ? rows.slice(rows.indexOf(anchor) + 1).find((li) => li.getClientRects().length > 0) : null;
    const mount = ['column-reply', 'side-reply'].map((one) => document.getElementById(one)).find((one) => !one.hidden) || null;
    const scroller = document.getElementById(list === 'discord-log' ? 'scroll-area' : 'side-scroll');
    return {
        main: !document.getElementById('screen-main').hidden,
        columns: document.getElementById('screen-main').getAttribute('data-columns'),
        row: box(row), anchor: box(anchor), anchorId: anchor ? anchor.getAttribute('data-id') : null,
        after: box(after), afterId: after ? after.getAttribute('data-id') : null,
        log: box(document.getElementById(list)), mount: box(mount), mountId: mount ? mount.id : null,
        boxes: ['channel-compose-text', 'side-compose-text', 'reply-text', 'compose-text'].filter(drawn),
        text: document.getElementById('reply-text').value,
        focus: document.activeElement ? document.activeElement.id : null,
        listTop: scroller.scrollTop,
        // The list's foot less what floats over it, as the page reckons it (`channelPane`, `sidePane`).
        foot: scroller.getBoundingClientRect().top + scroller.clientHeight - (list === 'discord-log'
            ? ['scroll-tools', 'status-line'] : ['side-tools']).reduce((room, one) => {
                const n = document.getElementById(one);
                return room + (n && !n.hidden && n.getClientRects().length ? n.getBoundingClientRect().height : 0);
            }, 0),
        stack: rows.filter((li) => li.hasAttribute('data-stack-under')).map((li) => li.getAttribute('data-id')),
        brought: rows.filter((li) => li.hasAttribute('data-gathered-thread')).map((li) => li.getAttribute('data-id')),
        overflow: document.documentElement.scrollWidth - window.innerWidth,
    };
}"""


# `#467 inline-send-in-place`. The rows drawn where a reply's box was, the row they are named to, the row
# after that one, the row `id` answered, and how far its scroller is down.
IN_PLACE_JS = """([list, id]) => {
    const box = (node) => {
        if (!node || !node.getClientRects().length) return null;
        const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height};
    };
    const log = document.getElementById(list);
    const group = [...log.parentNode.children].find((node) => node.classList.contains('outgoing-inplace')) || null;
    const name = group ? group.style.getPropertyValue('position-anchor') : '';
    const rows = [...log.children];
    const anchor = name ? rows.find((li) => li.style.getPropertyValue('anchor-name') === name) || null : null;
    const after = anchor ? rows.slice(rows.indexOf(anchor) + 1).find((li) => li.getClientRects().length > 0) : null;
    const row = rows.find((li) => (li.getAttribute('data-ids') || li.getAttribute('data-id') || '').split(' ').includes(id));
    const scroller = document.getElementById(list === 'discord-log' ? 'scroll-area' : 'side-scroll');
    return {
        group: box(group), text: group ? group.textContent : '', anchor: box(anchor),
        anchorId: anchor ? anchor.getAttribute('data-id') : null, after: box(after), row: box(row),
        anchored: rows.filter((li) => li.hasAttribute('data-outgoing-anchor')).length,
        foot: document.getElementById(list === 'discord-log' ? 'outgoing-log' : 'side-outgoing-log').children.length,
        listTop: scroller.scrollTop, overflow: document.documentElement.scrollWidth - window.innerWidth,
    };
}"""


def in_place(desk: Desk, list_id: str, message_id: str) -> Json:
    found = desk.page.evaluate(IN_PLACE_JS, [list_id, message_id])
    assert isinstance(found, dict)
    return found


def check_in_place(desk: Desk, found: Json, where: str, words: str) -> None:
    """A reply's row where its box was: directly under the row it is named to, as wide, rows after below."""
    desk.check(found["group"] is not None, f"{where}: the reply's row is not drawn where its box was: {found}")
    desk.check(words in str(found["text"]), f"{where}: the row there says {found['text']!r}")
    desk.check(number(found, "foot") == 0, f"{where}: the reply's row is at the foot of the list as well")
    group, anchor = rect(found, "group"), rect(found, "anchor")
    gap = group["top"] - anchor["bottom"]
    desk.check(UNDER_PX[0] - 1 <= gap <= UNDER_PX[1] + 1, f"{where}: the row is {gap:.1f}px under its row, not directly under it")
    desk.check(abs(group["left"] - anchor["left"]) <= 1 and abs(group["right"] - anchor["right"]) <= 1,
               f"{where}: the row is not as wide as the row it is under: {group} {anchor}")
    if found["after"] is not None:
        after = rect(found, "after")
        desk.check(after["top"] >= group["bottom"] - 0.5,
                   f"{where}: the next row is at {after['top']:.1f}, under the reply's row ending at {group['bottom']:.1f}")
        desk.check(after["top"] - group["bottom"] <= 16, f"{where}: a gap of {after['top'] - group['bottom']:.1f}px under the reply's row")
    desk.check(number(found, "overflow") <= 0, f"{where}: the page scrolls sideways")


def send_held(desk: Desk, api: ReplyColumnsApi, words: str) -> threading.Event:
    """Type `words` in the open reply and send it while the service holds its post; answer the hold."""
    held = threading.Event()
    api.holding = held
    desk.page.keyboard.type(words)
    desk.page.click("#reply-send")
    desk.wait_for("() => document.getElementById('column-reply').hidden && document.getElementById('side-reply').hidden",
                  "sending did not put the reply's box away")
    desk.settled()
    return held


def in_place_walk(desk: Desk, api: ReplyColumnsApi) -> None:
    """`#467 inline-send-in-place` in All: the reply's row where its box was, until it is answered."""
    sign_in(desk)
    target = "205"
    before = place(desk, "discord-log", target, 0.3)
    press_reply(desk, "discord-log", target)
    held = send_held(desk, api, "answered right here")
    try:
        found = in_place(desk, "discord-log", target)
        check_in_place(desk, found, "on its way", "Sending")
        desk.check(abs(rect(found, "row")["top"] - rect(before, "row")["top"]) <= 1, "sending moved the row answered")
        desk.shot("2-in-place-sending")
    finally:
        held.set()
        api.holding = None
    desk.wait_for("() => !document.querySelector('.outgoing-inplace')", "the reply's row stayed once the post was answered")
    desk.settled()
    landed = in_place(desk, "discord-log", target)
    desk.check(number(landed, "anchored") == 0, "a row still keeps room for a reply that went")
    desk.check(abs(rect(landed, "row")["top"] - rect(before, "row")["top"]) <= 1, "the reply landing moved the row answered")
    desk.notes.append(f"in All, a reply on {target} sent from the box in the stream: its row directly under the row, "
                      "the rows after it below, nothing moved as it went")


def promotes_walk(desk: Desk, api: ReplyColumnsApi) -> None:
    """`#466 reply-promotes-thread` on a Slack-like service, in Main and the thread the reply starts."""
    page = desk.page
    sign_in(desk)
    page.select_option("#thread-select", "main")
    desk.wait_for("() => document.getElementById('thread-select').value === 'main'", "Main did not open")
    desk.settled()
    target = "205"
    tag = f"#discord-log > li[data-id='{target}'] .thread-badge"
    desk.check(page.eval_on_selector(tag, "(n) => n.getAttribute('data-thread-state')") == "start",
               "the premise is a message nobody has answered")
    place(desk, "discord-log", target, 0.3)
    press_reply(desk, "discord-log", target)
    held = send_held(desk, api, "the first answer to it")
    try:
        state = page.eval_on_selector(tag, "(n) => ({state: n.getAttribute('data-thread-state'), text: n.textContent,"
                                      " hue: n.style.getPropertyValue('--thread-hue')})")
        desk.check(state["state"] == "open" and state["text"] == "Thread(1)" and state["hue"] != "",
                   f"on its way: the message answered is not a thread with one reply: {state}")
        check_in_place(desk, in_place(desk, "discord-log", target), "on its way in Main", "Sending")
        desk.shot("3-promoted-main")
        page.click(tag)
        desk.wait_for(f"() => document.getElementById('thread-select').value === 'thread:ts-{target}'",
                      "the tag did not open the thread the reply starts")
        desk.settled()
        check_in_place(desk, in_place(desk, "discord-log", target), "on its way in its thread", "Sending")
        desk.shot("4-promoted-thread")
    finally:
        held.set()
        api.holding = None
    desk.wait_for("() => !document.querySelector('.outgoing-inplace')"
                  " && document.querySelectorAll('#discord-log > li[data-id]').length === 2",
                  "the reply was not drawn in its thread once its post was answered")
    page.select_option("#thread-select", "main")
    desk.wait_for("() => document.getElementById('thread-select').value === 'main'", "Main did not open again")
    desk.settled()
    after = page.eval_on_selector(tag, "(n) => n.textContent")
    desk.check(after == "Thread(1)", f"sent: the message answered reads {after!r}")
    desk.notes.append(f"on a Slack-like service, a reply to {target} in Main made it Thread(1) at once, its row under "
                      "it there and in the thread it opened, then the reply itself in the thread")


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per state into (default: none)")
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
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    report: list[str] = []
    with sync_playwright() as playwright:
        report.append(run(playwright.chromium, args, "desk-columns", DESK, False, columns_walk))
        report.append(run(playwright.chromium, args, "desk-all", DESK, False, all_walk))
        report.append(run(playwright.chromium, args, "phone-all", PHONE, True, phone_walk))
        report.append(run(playwright.chromium, args, "refused", DESK, False, refused_walk))
        report.append(run(playwright.chromium, args, "view-keys", DESK, False, view_keys_walk))
        report.append(run(playwright.chromium, args, "collapse-from-all", DESK, False, collapse_from_all))
        report.append(run(playwright.chromium, args, "collapse-from-main", DESK, False, collapse_from_main))
        report.append(run(playwright.chromium, args, "in-place", DESK, False, in_place_walk))
        report.append(run(playwright.chromium, args, "promotes", DESK, False, promotes_walk, SlackLikeApi))
    for line in report:
        print(line)
    return 0


def run(chromium: BrowserType, args: argparse.Namespace, label: str, size: tuple[int, int], touch: bool,
        walk: Callable[[Desk, ReplyColumnsApi], None], api_class: type[ReplyColumnsApi] = ReplyColumnsApi) -> str:
    """One walk, against a fresh fake API and a fresh browser profile."""
    api = api_class()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    width, height = size
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-inline-reply-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark",
                has_touch=touch, is_mobile=touch)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"inline-reply-{label}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            walk(desk, api)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} {label} at {width}x{height}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def measure(desk: Desk, list_id: str, message_id: str) -> Json:
    found = desk.page.evaluate(MEASURE_JS, [list_id, message_id])
    assert isinstance(found, dict)
    return found


def strings(found: Json, name: str) -> list[str]:
    part = found.get(name)
    return [str(one) for one in part] if isinstance(part, list) else []


def number(found: Json, name: str) -> float:
    part = found.get(name)
    check(isinstance(part, (int, float)), f"{name} is not a number: {found}")
    assert isinstance(part, (int, float))
    return float(part)


def rect(found: Json, name: str) -> dict[str, float]:
    part = found.get(name)
    check(isinstance(part, dict), f"nothing to measure for {name}: {found}")
    assert isinstance(part, dict)
    return {str(key): float(value) for key, value in part.items()}


def sign_in(desk: Desk) -> None:
    page = desk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24",
                  "the channel did not open in All")
    desk.settled()


def two_columns(desk: Desk) -> None:
    """Two columns, the runner's thread on the right, Main on the left."""
    page = desk.page
    page.click("#column-toggle")
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length > 0"
                  " && document.querySelectorAll('#side-cards > li').length === 3",
                  "the layout button did not give two columns with the cards")
    page.click(f"#side-cards > li[data-id='{RUNNER}'] .side-card-open")
    desk.wait_for(f"() => document.querySelectorAll('#side-log > li[data-id]').length === {THREAD_ROWS[RUNNER]}",
                  "the runner's card did not open its thread on the right")
    page.mouse.move(desk.width / 2, 5)
    desk.settled()


def place(desk: Desk, list_id: str, message_id: str, fraction: float) -> Json:
    """Scroll `list_id`'s row for `message_id` to `fraction` of its scroller's height; then measure."""
    desk.page.evaluate("""([list, id, fraction]) => {
        const area = document.getElementById(list === 'discord-log' ? 'scroll-area' : 'side-scroll');
        const row = [...document.querySelectorAll(`#${list} > li[data-id]`)].find((li) => li.dataset.id === id);
        const top = area.getBoundingClientRect().top + area.clientHeight * fraction;
        area.scrollTop += row.getBoundingClientRect().top - top;
    }""", [list_id, message_id, fraction])
    desk.settled()
    return measure(desk, list_id, message_id)


def press_reply(desk: Desk, list_id: str, message_id: str, touch: bool = False) -> None:
    """Reply on the row, as the reader presses it: a finger or the mouse on the button, where it is drawn;
    by the button itself where a phone's folded row draws none."""
    selector = f"#{list_id} > li[data-id='{message_id}'] .reply-button"
    found = desk.page.locator(selector).first.bounding_box()
    if found is None:
        desk.page.evaluate("(selector) => document.querySelector(selector).click()", selector)
    elif touch:
        desk.page.touchscreen.tap(found["x"] + found["width"] / 2, found["y"] + found["height"] / 2)
    else:
        desk.page.mouse.click(found["x"] + found["width"] / 2, found["y"] + found["height"] / 2)
    desk.wait_for("() => ['column-reply', 'side-reply'].some((id) => !document.getElementById(id).hidden)",
                  f"Reply on {message_id} did not open the reply in the list")
    desk.settled()


def check_open(desk: Desk, found: Json, where: str, list_id: str, anchor_id: str) -> None:
    """The open reply as the owner asked for it: in the list, under its row, later rows below, one box."""
    desk.check(found["main"] is True, f"{where}: Reply left the main screen")
    desk.check(found["anchorId"] == anchor_id, f"{where}: the box is under {found['anchorId']}, not {anchor_id}")
    anchor, mount, log = rect(found, "anchor"), rect(found, "mount"), rect(found, "log")
    gap = mount["top"] - anchor["bottom"]
    desk.check(UNDER_PX[0] - 1 <= gap <= UNDER_PX[1] + 1, f"{where}: the box is {gap:.1f}px under its row, not directly under it")
    desk.check(abs(mount["left"] - log["left"]) <= 1 and abs(mount["right"] - log["right"]) <= 1,
               f"{where}: the box is not as wide as the list: {mount} {log}")
    if found["after"] is not None:
        after = rect(found, "after")
        desk.check(after["top"] >= mount["bottom"] - 0.5,
                   f"{where}: the row after it ({found['afterId']}) is at {after['top']:.1f}, under the box ending at {mount['bottom']:.1f}")
        desk.check(after["top"] - mount["bottom"] <= 16, f"{where}: a gap of {after['top'] - mount['bottom']:.1f}px under the box")
    desk.check(strings(found, "boxes") == ["reply-text"], f"{where}: the message boxes on screen are {strings(found, 'boxes')}, not the reply's alone")
    desk.check(number(found, "overflow") <= 0, f"{where}: the page scrolls sideways")


def check_held(desk: Desk, before: Json, found: Json, where: str) -> None:
    """The row answered not moved, or moved up just as far as shows the whole box under it."""
    was, now = rect(before, "row")["top"], rect(found, "row")["top"]
    if abs(now - was) <= 1:
        return
    desk.check(now < was, f"{where}: the row moved down, from {was:.1f} to {now:.1f}")
    desk.check(abs(rect(found, "mount")["bottom"] - number(found, "foot")) <= 2,
               f"{where}: the row moved from {was:.1f} to {now:.1f}, more than shows the box: "
               f"{rect(found, 'mount')} foot {number(found, 'foot'):.1f}")


def columns_walk(desk: Desk, api: ReplyColumnsApi) -> None:
    page = desk.page
    sign_in(desk)
    two_columns(desk)
    # An old message mid-thread on the right: the runner's reply 208, its replies after it below.
    before = place(desk, "side-log", "208", 0.3)
    press_reply(desk, "side-log", "208")
    found = measure(desk, "side-log", "208")
    desk.check(found["columns"] == "two", "Reply took the page out of two columns")
    check_open(desk, found, "right 208", "side-log", "208")
    check_held(desk, before, found, "right 208")
    desk.check(found["focus"] == "reply-text", f"right 208: the focus is on {found['focus']}, not the box")
    desk.shot("1-right-mid-thread")
    desk.notes.append(f"right 208: the box {rect(found, 'mount')['top'] - rect(found, 'row')['bottom']:.1f}px under "
                      f"the row, {found['afterId']} below, the thread's own box hidden")

    # Grow ticked and eight lines typed: the box grows, and the rows below go down with it.
    page.click("#reply-grow")
    page.focus("#reply-text")
    page.keyboard.type("first line")
    for words in ("second line", "third line", "fourth line", "fifth line", "sixth line", "seventh line", "eighth line"):
        page.keyboard.press("Shift+Enter")
        page.keyboard.type(words)
    desk.settled()
    grown = measure(desk, "side-log", "208")
    desk.check(rect(grown, "mount")["height"] > rect(found, "mount")["height"] + 10,
               f"right 208: Grow did not grow the box: {rect(found, 'mount')['height']:.1f} -> {rect(grown, 'mount')['height']:.1f}")
    check_open(desk, grown, "right 208 grown", "side-log", "208")
    desk.shot("2-right-grown")
    # Grow unticked: the box back to its height, the checkbox kept under the pointer (`#340`).
    page.click("#reply-grow")
    desk.settled()
    grown = measure(desk, "side-log", "208")

    # Cancel: the thread's box back, the row where it was, its words a Draft on the row.
    page.click("#reply-cancel")
    desk.wait_for("() => document.getElementById('side-reply').hidden", "Cancel did not put the box away")
    desk.settled()
    back = measure(desk, "side-log", "208")
    desk.check("side-compose-text" in strings(back, "boxes"), f"after Cancel the thread's box is not back: {strings(back, 'boxes')}")
    desk.check(abs(rect(back, "row")["top"] - rect(grown, "row")["top"]) <= 1,
               f"Cancel moved the row from {rect(grown, 'row')['top']:.1f} to {rect(back, 'row')['top']:.1f}")
    desk.check(page.evaluate("() => !document.querySelector(\"#side-log > li[data-id='208'] .draft-marker\").hidden"),
               "the words were not kept as the row's draft")
    press_reply(desk, "side-log", "208")
    desk.check(str(measure(desk, "side-log", "208")["text"]).startswith("first line"), "the draft did not come back with Reply")
    page.fill("#reply-text", "")
    page.click("#reply-cancel")
    desk.wait_for("() => document.getElementById('side-reply').hidden", "Cancel did not put the box away")

    # Main's replica root on the left: its thread's replies brought under it as rows, on the bridge.
    root = "209"
    before = place(desk, "discord-log", root, 0.2)
    press_reply(desk, "discord-log", root)
    found = measure(desk, "discord-log", root)
    desk.check(strings(found, "brought") == ["210", "212", "214"], f"left {root}: the thread's replies brought were {strings(found, 'brought')}")
    desk.check(set(strings(found, "brought")) <= set(strings(found, "stack")), f"left {root}: the thread's replies are not on the bridge")
    check_open(desk, found, f"left {root}", "discord-log", "214")
    check_held(desk, before, found, f"left {root}")
    desk.shot("3-left-main-root-gathered")
    sent = len(api.posted())
    page.fill("#reply-text", "into the replica thread, from Main")
    page.click("#reply-send")
    desk.wait_for("() => document.getElementById('column-reply').hidden", "Send did not put the box away")
    desk.settled()
    posted = api.posted()[sent:]
    # Where Reply on Main posts is the route's, as it always was: here a reply by pointer on the main
    # channel, which the gathering then holds under the root with the thread's replies.
    desk.check(len(posted) == 1 and posted[0].get("reply_to") == root, f"left {root}: what was posted was {posted}")
    desk.wait_for("() => [...document.querySelectorAll('#discord-log > li[data-stack-under]')].length >= 4",
                  f"left {root}: the sent reply did not join the replies gathered under the root")
    desk.settled()
    after = measure(desk, "discord-log", root)
    desk.check({"210", "212", "214"} <= set(strings(after, "stack")), f"left {root}: Send put the gathered replies away: {strings(after, 'stack')}")
    desk.check("channel-compose-text" in strings(after, "boxes"), f"after Send Main's box is not back: {strings(after, 'boxes')}")
    desk.check(abs(rect(after, "row")["top"] - rect(found, "row")["top"]) <= 1,
               f"left {root}: Send moved the root from {rect(found, 'row')['top']:.1f} to {rect(after, 'row')['top']:.1f}")
    desk.shot("4-left-main-root-sent")
    desk.notes.append(f"Main's root {root}: its thread's replies brought under it on the bridge, the box under 214; "
                      "Send posted, the reply joined them, and nothing moved")


def all_walk(desk: Desk, api: ReplyColumnsApi) -> None:
    del api
    page = desk.page
    sign_in(desk)
    root = "202"
    place(desk, "discord-log", root, 0.15)
    press_reply(desk, "discord-log", root)
    found = measure(desk, "discord-log", root)
    gathered = ["204", "206", "208", "211", "213", "217", "219", "221"]
    desk.check(strings(found, "stack") == gathered, f"All {root}: the replies gathered under it are {strings(found, 'stack')}")
    check_open(desk, found, f"All {root}", "discord-log", "221")
    desk.shot("1-all-root-gathered")
    page.keyboard.press("Escape")
    desk.wait_for("() => document.getElementById('column-reply').hidden", "Escape did not put the box away")
    desk.settled()
    back = measure(desk, "discord-log", root)
    desk.check(strings(back, "stack") == [], f"Escape left the replies gathered: {strings(back, 'stack')}")
    desk.check("channel-compose-text" in strings(back, "boxes"), "after Escape the channel's box is not back")
    desk.notes.append(f"All {root}: its eight replies gathered under it, the box under the last; Escape put them back")


def phone_walk(desk: Desk, api: ReplyColumnsApi) -> None:
    page = desk.page
    sign_in(desk)
    message_id = "205"
    before = place(desk, "discord-log", message_id, 0.25)
    press_reply(desk, "discord-log", message_id, touch=True)
    found = measure(desk, "discord-log", message_id)
    check_open(desk, found, f"phone {message_id}", "discord-log", message_id)
    check_held(desk, before, found, f"phone {message_id}")
    desk.shot("1-phone-mid-stream")
    sent = len(api.posted())
    page.fill("#reply-text", "from the phone")
    box = page.locator("#reply-send").bounding_box()
    assert box is not None
    page.touchscreen.tap(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
    desk.wait_for("() => document.getElementById('column-reply').hidden", "Send did not put the box away")
    desk.settled()
    posted = api.posted()[sent:]
    desk.check(len(posted) == 1 and posted[0].get("reply_to") == message_id, f"phone: what was posted was {posted}")
    after = measure(desk, "discord-log", message_id)
    desk.check("channel-compose-text" in strings(after, "boxes"), f"phone: after Send the channel's box is not back: {strings(after, 'boxes')}")
    moved = rect(after, "row")["top"] - rect(found, "row")["top"]
    desk.check(abs(moved) <= 24, f"phone: Send moved the row by {moved:.1f}px")
    desk.shot("2-phone-sent")
    desk.notes.append(f"phone {message_id}: the box under the row, one box on screen; Send posted it, the row "
                      f"moved {moved:+.1f}px (its Done redrawn)")

    # The newest message, at the very foot of the list: the box under it is clear of the chips and the
    # status pill floating over the foot, as the channel's own box was (`#245 done-feedback`).
    newest = page.evaluate("() => [...document.querySelectorAll('#discord-log > li[data-id]')].at(-1).dataset.id")
    assert isinstance(newest, str)
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    desk.settled()
    press_reply(desk, "discord-log", newest, touch=True)
    found = measure(desk, "discord-log", newest)
    check_open(desk, found, f"phone newest {newest}", "discord-log", newest)
    floating = page.evaluate("""() => ['scroll-tools', 'status-line'].map((id) => document.getElementById(id))
        .filter((n) => !n.hidden && n.getClientRects().length > 0)
        .flatMap((n) => [...n.querySelectorAll('button, .status-text, #status')].filter((b) => b.getClientRects().length > 0))
        .map((b) => { const r = b.getBoundingClientRect(); return {top: r.top, bottom: r.bottom, left: r.left, right: r.right}; })""")
    send = page.locator("#reply-send").bounding_box()
    assert send is not None and isinstance(floating, list)
    for over in floating:
        crosses = not (over["bottom"] <= send["y"] or over["top"] >= send["y"] + send["height"]
                       or over["right"] <= send["x"] or over["left"] >= send["x"] + send["width"])
        desk.check(not crosses, f"phone newest: something floating over the foot covers Send: {over} over {send}")
    desk.shot("3-phone-newest")
    page.click("#reply-cancel")
    desk.notes.append(f"phone newest {newest}: the box under it at the foot, Send clear of the chips and the pill")


def refused_walk(desk: Desk, api: ReplyColumnsApi) -> None:
    """`#277 reply-in-column`: a reply that does not go says so where the reader is."""
    page = desk.page
    sign_in(desk)
    two_columns(desk)
    place(desk, "side-log", "206", 0.2)
    press_reply(desk, "side-log", "206")
    opened = measure(desk, "side-log", "206")
    page.keyboard.type("this one will not go")
    page.evaluate("() => { document.getElementById('status').textContent = ''; }")
    api.refusing = (422, {"error": "chat_error", "detail": "Google Chat returned HTTP 400"})
    try:
        page.click("#reply-send")
        desk.wait_for("() => /^Your reply /.test(document.getElementById('status').textContent)"
                      " && !document.getElementById('status-line').hidden",
                      "refused: nothing on screen said the reply did not go")
    finally:
        api.refusing = None
    desk.settled()
    after = measure(desk, "side-log", "206")
    desk.check(abs(number(after, "listTop") - number(opened, "listTop")) <= 1, "refused: Send moved the column")
    status = str(page.evaluate("() => document.getElementById('status').textContent"))
    desk.check(status == "Your reply was not sent. Retry is under the message it answers.", f"refused: the status line says {status!r}")
    # `#467 inline-send-in-place`. The row with Retry where the box was, and no chip leading down to it.
    check_in_place(desk, in_place(desk, "side-log", "206"), "refused", "Not sent")
    desk.check(page.evaluate("() => !!document.querySelector('.outgoing-inplace .outgoing-retry')"),
               "refused: the row where the box was offers no Retry")
    chip = page.evaluate("() => document.getElementById('side-jump-newest-label').textContent")
    desk.check(chip != "Reply not sent", "refused: the right column's chip leads down to a row the reader is at")
    desk.shot("1-refused")
    desk.notes.append(f"a refused reply from the right column: the column unmoved, {status!r}, its Retry where the box was")


def view_keys_walk(desk: Desk, api: ReplyColumnsApi) -> None:
    """`#281 view-keys` in a reply open in the stream."""
    del api
    page = desk.page
    sign_in(desk)
    two_columns(desk)
    place(desk, "discord-log", "203", 0.2)
    press_reply(desk, "discord-log", "203")
    page.keyboard.type("left words")
    page.keyboard.press("Control+4")
    desk.settled()
    kept = measure(desk, "discord-log", "203")
    desk.check(kept["mountId"] == "column-reply" and kept["focus"] == "reply-text" and kept["text"] == "left words",
               f"view keys: Ctrl+4 in two columns did not leave the reply as it was: {kept}")
    page.keyboard.press("Control+2")
    desk.wait_for("() => document.getElementById('thread-select').value === 'flat'"
                  " && document.getElementById('side-column').getClientRects().length === 0"
                  " && document.getElementById('column-reply').hidden",
                  "view keys: Ctrl+2 in the left column's box did not show All and put the reply down")
    desk.settled()
    draft = page.evaluate("() => { const m = document.querySelector(\"#discord-log > li[data-id='203'] .draft-marker\");"
                          " return Boolean(m) && !m.hidden; }")
    desk.check(draft is True, "view keys: the words of the reply Ctrl+2 put down were not kept as the row's draft")
    desk.check("channel-compose-text" in strings(measure(desk, "discord-log", "203"), "boxes"), "view keys: All's box is not back")
    desk.shot("1-view-keys-all")
    desk.notes.append("view keys in a reply open in the stream: Ctrl+4 left it open; Ctrl+2 showed All in one column, "
                      "the reply put down with its words kept as the row's draft")


def collapse_from_all(desk: Desk, api: ReplyColumnsApi) -> None:
    del api
    page = desk.page
    sign_in(desk)
    two_columns(desk)
    press_reply(desk, "side-log", "206")
    page.keyboard.type("kept words")
    page.set_viewport_size({"width": PHONE[0], "height": PHONE[1]})
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length === 0",
                  "narrowing to a phone's width did not give one column")
    desk.wait_for("() => !document.getElementById('column-reply').hidden", "the reply did not stay open in the one column")
    desk.settled()
    found = measure(desk, "discord-log", "206")
    desk.check(found["text"] == "kept words", f"the words did not stay with the reply: {found['text']!r}")
    desk.check(found["focus"] == "reply-text", f"the reader typing lost the box: the focus is on {found['focus']}")
    check_open(desk, found, "collapsed from All", "discord-log", "206")
    desk.shot("1-collapsed-inline")
    desk.notes.append("two columns narrowed to 393 with a reply open on the right: open under its message in All, "
                      "words and focus kept")


def collapse_from_main(desk: Desk, api: ReplyColumnsApi) -> None:
    del api
    page = desk.page
    sign_in(desk)
    page.select_option("#thread-select", "main")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 11", "Main did not open")
    two_columns(desk)
    press_reply(desk, "side-log", "206")
    page.keyboard.type("wait for me")
    page.focus("#scroll-area")
    page.set_viewport_size({"width": PHONE[0], "height": PHONE[1]})
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length === 0",
                  "narrowing to a phone's width did not give one column")
    desk.settled()
    desk.check(page.evaluate("() => document.getElementById('column-reply').hidden"),
               "a reply was left open in Main, which does not draw its message")
    page.select_option("#thread-select", f"thread:{RUNNER}")
    desk.wait_for("() => !document.getElementById('column-reply').hidden",
                  "the reply did not open again once its message was drawn")
    desk.settled()
    found = measure(desk, "discord-log", "206")
    desk.check(found["text"] == "wait for me", f"the reply opened again without its words: {found['text']!r}")
    check_open(desk, found, "reopened in the thread", "discord-log", "206")
    desk.shot("1-reopened-in-thread")
    desk.notes.append("narrowed from Main with a reply open on the right: put away, and opened again with its words "
                      "when its thread was opened")


# --- shared with tests/reply_marks_read_browser.py and tests/done_sync_browser.py ---------------------

def open_columns(desk: Desk) -> None:
    """Signed in, on the channel, in two columns with the runner's thread on the right, Main on the left."""
    sign_in(desk)
    two_columns(desk)
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 11",
                  "the left column is not Main")


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


def reply_button(desk: Desk, column: str, message_id: str) -> dict[str, float]:
    """Where the row's Reply is drawn, to press it as a reader does."""
    found = desk.page.evaluate(f"""() => {{
        const b = document.querySelector("#{list_of(column)} > li[data-id='{message_id}'] > .meta .reply-button");
        if (!b) return null;
        const r = b.getBoundingClientRect();
        return {{x: r.left + r.width / 2, y: r.top + r.height / 2, text: b.textContent}};
    }}""")
    desk.check(isinstance(found, dict), f"{column} {message_id}: the row has no Reply")
    assert isinstance(found, dict)
    return {"x": float(found["x"]), "y": float(found["y"])}


if __name__ == "__main__":
    raise SystemExit(main())
