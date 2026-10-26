#!/usr/bin/env python3
"""Every thread unpacked under its card in real Chromium, at 1280x800 and 393x852, dark and light.

`#404 threads-unpack-all` and `#405 root-under-summary`. The owner, 2026-10-21: in All, **N replies**
bridges one thread at a time and bridging another breaks the first; he wants every thread chained at
once. In the Threads view (Cmd+3), **Unpack msgs** (key u) draws each thread's messages under its card
in the bridged style, respecting Show / Collapse / Hide read; and under a summarized card the raw root
is the first child. The fake-DOM suite pins what is drawn; this proves where it is drawn and in what
colour, that the bridge is continuous, and what unpacking forty threads costs.

It serves the checked-out page and the fake API of tests/two_column_browser.py — three threads, one
summarised — with the summarised thread's root and first two replies, and one raw thread's first two
replies, already Done on the server.

At each size and scheme: sign in, All, the thread list from the picker -> three cards, Unpack msgs
offered and unpressed, no unpacked row (negative control) -> press it: every card gains its messages,
the summarised block starting with its root as a bridged child in the thread's colour, a raw block
with no root row (negative control), each bridge's spine left of its boxes and continuous over the
block -> Fold read: each block's two read replies are one grey line joined to its spine -> Hide
read: read replies gone from every block, the summarised root still first -> Show read, Unpack msgs
off: cards alone again -> u (desk) or the chip (phone) on, reload: still on -> `#411
thread-view-row-padding`: every unpacked row's header buttons stand at least as far inside its top
border as the nearest of All's (before, 0px: the rows had no box). No horizontal overflow and no page
error at any step.

`#407 unpack-row-parity`. At each desk size an unpacked row is then driven as a row of any other list
is, with a real mouse and real keys: the mouse over an unpacked row lights it (one row lit on the page,
drawn with the selection's ring); ↓ and ↑ move the selection through the rows of a block and across to
the next card in reading order; e marks the selected row Done with the list left where it was, and z
puts it back; Enter opens and folds the selected row; : and ; collapse and expand
every unpacked row, saying so; under Fold read the block's run is the run line every list draws,
which Enter on it opens. Negative control: before any of it, ↓ over the cards packed goes card to card.

Then, at the desk in the dark, a channel of forty threads: the time from pressing Unpack msgs to every
block drawn, and the row count, said in the notes.

Pass --screenshots DIR to keep a PNG of each state; --web-root DIR for another copy of the page.
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
from offline_cache_browser import Json, handler_for, said
from thread_card_header_browser import phone_thread_list
from two_column_browser import CANARY, FILLER, REPLICA, RUNNER, ColumnsApi, Desk, thread_record

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

SIZES = (
    ("desk-1280-dark", 1280, 800, "dark", False),
    ("desk-1280-light", 1280, 800, "light", False),
    ("phone-393-dark", 393, 852, "dark", True),
    ("phone-393-light", 393, 852, "light", True),
)

# Done on the server: the summarised root and two replies of each of two threads.
DISMISSED = ["202", "204", "206", "210", "212"]


class UnpackApi(ColumnsApi):
    """ColumnsApi with a read run in the summarised thread and in a raw one."""

    def timeline(self, query: dict[str, list[str]]) -> Json:
        return {**super().timeline(query), "dismissed": DISMISSED}


class ParityApi(UnpackApi):
    """UnpackApi with every reply long enough to fold, for the keys that open and fold one, and Done
    and its Undo served, for the key that marks one."""

    def __init__(self) -> None:
        super().__init__()
        self.done = set(DISMISSED)
        for row in self.messages:
            thread = row.get("thread")
            if isinstance(thread, dict) and thread.get("is_root") is False:
                row["content"] = f"{row['content']} {FILLER * 6}"

    def post(self, path: str, body: Json) -> tuple[int, Json]:
        ids = body.get("messages")
        named = [str(one) for one in ids] if isinstance(ids, list) else []
        if path.endswith("/dismiss"):
            with self.lock:
                self.done.update(named)
            return 200, {"dismissed": named}
        if path.endswith("/restore"):
            with self.lock:
                self.done.difference_update(named)
            return 200, {"messages": named}
        return super().post(path, body)

    def timeline(self, query: dict[str, list[str]]) -> Json:
        with self.lock:
            done = sorted(self.done)
        return {**super().timeline(query), "dismissed": done}


class ManyApi(ColumnsApi):
    """Forty threads of a root and four replies, for the cost of unpacking them all."""

    def __init__(self) -> None:
        super().__init__()
        self.messages = []
        self.threads = []
        index = 0
        filler = "The overnight run is still going and the shard was retried. "
        for thread in range(40):
            thread_id = f"spaces/A/threads/m{thread}"
            root_at = index
            root = said(index, f"Thread {thread} root. {filler}", "coder")
            root["thread"] = thread_record(thread_id, index, 4)
            self.messages.append(root)
            index += 1
            last = root
            for reply in range(4):
                row = said(index, f"Thread {thread} reply {reply}. {filler}", "coder")
                row["thread"] = thread_record(thread_id, root_at, 4, is_root=False)
                self.messages.append(row)
                last = row
                index += 1
            self.threads.append({"id": thread_id, "root": root, "title": "Thread",
                                  "reply_count": 4, "reply_count_exact": True,
                                  "updated_at": last["timestamp"]})


STATE_JS = """(threadId) => {
    const box = (node) => { const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height}; };
    const rows = [...document.querySelectorAll(`#side-cards > li[data-unpack-thread="${threadId}"]`)];
    return rows.map((li) => {
        const bridge = li.querySelector(':scope > .reply-bridge');
        const run = li.querySelector('.read-run');
        return {ids: (li.getAttribute('data-ids') || '').split(' ').filter(Boolean),
                rootChild: li.hasAttribute('data-root-child'),
                continuation: li.hasAttribute('data-root-continuation'),
                coalesce: li.getAttribute('data-coalesce'),
                hidden: li.hidden, box: box(li),
                bridge: bridge ? {...box(bridge), color: getComputedStyle(bridge).color,
                                  spine: getComputedStyle(bridge).borderLeftColor} : null,
                run: run ? {...box(run), text: run.textContent} : null};
    });
}"""


# `#411 thread-view-row-padding`. How far under each drawn message row's top border its header's first
# button stands, in px, for the rows `selector` finds: the room the row's box gives the header.
INSETS_JS = """(selector) => [...document.querySelectorAll(selector)].flatMap((li) => {
    const button = [...li.querySelectorAll(':scope > .meta button')].find((one) => one.getClientRects().length > 0);
    if (!button || li.getClientRects().length === 0) return [];
    const border = parseFloat(getComputedStyle(li).borderTopWidth) || 0;
    return [button.getBoundingClientRect().top - (li.getBoundingClientRect().top + border)];
})"""


# The selection as drawn: every row lit on the page, by list and id, and the lit row's ring.
LIT_JS = """() => [...document.querySelectorAll('li[data-selected="true"]')].map((li) => ({
    list: li.parentElement.id, id: li.getAttribute('data-id'), card: li.hasAttribute('data-card'),
    unpacked: li.getAttribute('data-unpack-thread'), focused: document.activeElement === li,
    outline: getComputedStyle(li).outlineStyle, run: li.getAttribute('data-read-run')}))"""

# The rows the keys move through over the cards, in order: shown, with an id.
WALK_JS = """() => [...document.querySelectorAll('#side-cards > li[data-id]')]
    .filter((li) => !li.hidden && li.getClientRects().length > 0)
    .map((li) => (li.hasAttribute('data-card') ? 'card:' : '') + li.getAttribute('data-id'))"""

# The unpacked rows' folds: id -> data-collapsed, for the rows that fold.
FOLDS_JS = """() => Object.fromEntries([...document.querySelectorAll('#side-cards > li[data-unpack-thread][data-collapsed]')]
    .map((li) => [li.getAttribute('data-id'), li.getAttribute('data-collapsed')]))"""


def unpack_all(desk: Desk) -> None:
    """Unpack all: the chip pressed on round its states (`#440 thread-card-pack-toggle`) until it says so."""
    for _ in range(3):
        if desk.page.get_attribute("#side-unpack", "aria-pressed") == "true":
            return
        desk.page.click("#side-unpack")
        desk.settled()
    desk.check(desk.page.get_attribute("#side-unpack", "aria-pressed") == "true", "the chip never reached Unpack all")

def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per state into")
    return parser.parse_args()


def serve(api: ColumnsApi) -> ThreadingHTTPServer:
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server


def open_page(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
              scheme: str, touch: bool, api: ColumnsApi) -> tuple[Desk, list[str]]:
    server = serve(api)
    profile = tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-unpack-")
    context = chromium.launch_persistent_context(
        profile.name, headless=True, executable_path=args.browser_executable,
        viewport={"width": width, "height": height}, color_scheme=scheme,  # type: ignore[arg-type]
        has_touch=touch, is_mobile=touch)
    page = context.pages[0] if context.pages else context.new_page()
    desk = Desk(page, args.screenshots, label, width, height)
    errors: list[str] = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(f"http://127.0.0.1:{server.server_address[1]}/")
    return desk, errors


def ids_of(desk: Desk, thread_id: str) -> list[str]:
    return [one for row in desk.page.evaluate(STATE_JS, thread_id) for one in row["ids"]]


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
         scheme: str, touch: bool) -> str:
    desk, errors = open_page(chromium, args, label, width, height, scheme, touch, UnpackApi())
    page = desk.page
    phone_thread_list(desk)
    toggle = page.locator("#side-unpack")
    desk.check(toggle.is_visible(), "Unpack is not offered over the thread list")
    desk.check(toggle.get_attribute("aria-pressed") == "false", "Unpack does not start at Pack all")
    desk.check(page.evaluate("() => document.querySelectorAll('#side-cards > li[data-unpack-thread]').length") == 0,
               "rows are unpacked before the toggle is pressed")
    desk.shot("1-packed")

    unpack_all(desk)
    desk.wait_for(f"() => document.querySelectorAll('#side-cards > li[data-unpack-thread=\"{RUNNER}\"]').length >= 9",
                  "the summarised thread did not unpack under its card")
    desk.settled()
    desk.check(toggle.get_attribute("aria-pressed") == "true", "the toggle does not say it is pressed")
    runner = page.evaluate(STATE_JS, RUNNER)
    desk.check(ids_of(desk, RUNNER)[0] == "202" and runner[0]["rootChild"],
               f"the summarised block does not start with its root child: {runner[0]}")
    desk.check(all(row["bridge"] for row in runner), "an unpacked row has no bridge")
    card_edge = page.evaluate(
        f"() => getComputedStyle(document.querySelector('#side-cards > li[data-id=\"{RUNNER}\"]')).borderLeftColor")
    desk.check(runner[0]["bridge"]["spine"] == card_edge,
               f"the bridge {runner[0]['bridge']['spine']} is not the card edge {card_edge}")
    # `#450 pack-triangle-hue`, at every size and in both schemes (this walk's four): the pack
    # triangle in the first row's gutter is that same colour, with no disc drawn behind it.
    tri = page.evaluate("""(id) => {
        const b = document.querySelector(`#side-cards > li[data-unpack-thread="${id}"] .unpack-pack`);
        const m = b.querySelector('.unpack-pack-mark'); const s = getComputedStyle(b);
        return {color: s.color, markBorder: getComputedStyle(m).borderTopWidth,
                markBackground: getComputedStyle(m).backgroundColor,
                tri: getComputedStyle(m, '::before').borderBottomColor}; }""", RUNNER)
    desk.check(tri["color"] == card_edge and tri["tri"] == card_edge,
               f"the pack triangle {tri} is not its card edge {card_edge}")
    desk.check(tri["markBorder"] == "0px" and tri["markBackground"] in ("rgba(0, 0, 0, 0)", "transparent"),
               f"the pack triangle still stands in a disc: {tri}")
    spines = [row["bridge"]["left"] for row in runner]
    desk.check(max(spines) - min(spines) <= 2, f"the bridge spine wanders: {spines}")
    desk.check(all(row["bridge"]["left"] < row["box"]["left"] + 40 for row in runner),
               "a bridge is not in its row's gutter")
    # The long question, one paragraph longer than its card on its own, is clamped on the card and goes
    # under it whole (`#528 thread-card-markdown-and-cap`; `#522 thread-card-max-lines` split it inside
    # a line): its first row is the root's own row, and no other row repeats the root.
    replica = page.evaluate(STATE_JS, REPLICA)
    desk.check(replica[0]["ids"] == ["209"] and bool(replica[0]["rootChild"]) and not replica[0]["continuation"],
               f"the clamped question's block does not start with the whole question: {replica[0]}")
    desk.check("209" not in [one for row in replica[1:] for one in row["ids"]], "the raw block duplicates the root on its card")
    desk.check(not any(row["rootChild"] for row in replica[1:]), "a raw card grew a second root child")
    # Its first message is one paragraph longer than a card shows, so it goes under its card whole
    # (`#528 thread-card-markdown-and-cap`), then its two replies.
    canary = page.evaluate(STATE_JS, CANARY)
    desk.check(ids_of(desk, CANARY) == ["215", "216", "218"] and bool(canary[0]["rootChild"]),
               f"the third thread did not unpack its whole first message and its replies: {ids_of(desk, CANARY)}")
    desk.shot("2-unpacked-show")

    page.click("#todo-filter")
    desk.wait_for("() => document.getElementById('todo-filter').getAttribute('data-read-mode') === 'fold'",
                  "Fold read was not reached")
    desk.settled()
    for thread_id in (RUNNER, REPLICA):
        runs = [row for row in page.evaluate(STATE_JS, thread_id) if row["run"]]
        desk.check(len(runs) == 1 and "2 read messages" in str(runs[0]["run"]["text"]),
                   f"{thread_id}: its read run is not one grey line: {runs}")
        desk.check(runs[0]["bridge"] is not None and runs[0]["run"]["left"] > runs[0]["bridge"]["left"],
                   "the run line hangs off no bridge")
    desk.shot("3-unpacked-collapse")

    page.click("#todo-filter")
    desk.wait_for("() => document.getElementById('todo-filter').getAttribute('data-read-mode') === 'hide'",
                  "Hide read was not reached")
    desk.settled()
    desk.check(not set(ids_of(desk, RUNNER)) & {"204", "206"}, "Hide read kept a read reply unpacked")
    desk.check(ids_of(desk, RUNNER)[0] == "202", "Hide read took the summarised root away")
    desk.check(not set(ids_of(desk, REPLICA)) & {"210", "212"}, "Hide read kept a raw read reply")
    desk.shot("4-unpacked-hide")

    page.click("#todo-filter")
    desk.wait_for("() => document.getElementById('todo-filter').getAttribute('data-read-mode') === 'show'",
                  "Show read was not reached")
    toggle.click()
    desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-unpack-thread]').length === 0",
                  "packing away left rows behind")
    desk.shot("5-packed-again")

    if touch:
        toggle.click()
    else:
        page.keyboard.press("u")
    desk.wait_for(f"() => document.querySelectorAll('#side-cards > li[data-unpack-thread=\"{RUNNER}\"]').length >= 9",
                  "u did not unpack again")
    page.reload()
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
                  f" && document.querySelectorAll('#side-cards > li[data-unpack-thread=\"{RUNNER}\"]').length >= 9",
                  "a reload forgot Unpack msgs")
    desk.shot("6-reloaded-unpacked")
    # `#411 thread-view-row-padding`. The owner: in the thread view "the Reply '...' and Done buttons are
    # touching the top border of the message box and this is not happening in the All view with the
    # same messages rendered". An unpacked row's header stands as far inside its box as All's does.
    unpacked = page.evaluate(INSETS_JS, "#side-cards > li.discord-message[data-unpack-thread]")
    page.select_option("#thread-select", "flat")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24", "All did not come back")
    desk.settled()
    in_all = page.evaluate(INSETS_JS, "#discord-log > li.discord-message")
    desk.check(len(unpacked) >= 9 and len(in_all) >= 9, f"too few rows measured: {len(unpacked)} unpacked, {len(in_all)} in All")
    desk.check(min(unpacked) >= min(in_all) - 0.5 and min(unpacked) >= 4,
               f"an unpacked row's header buttons stand {min(unpacked):.1f}px under its top border, All's {min(in_all):.1f}px")
    desk.notes.append(f"header buttons {min(unpacked):.1f}px inside an unpacked row's top border, {min(in_all):.1f}px in All")
    overflow = page.evaluate("() => document.documentElement.scrollWidth - window.innerWidth")
    desk.check(overflow <= 0, f"horizontal overflow of {overflow}px")
    desk.check(not errors, f"page errors: {errors}")
    page.context.close()
    return f"{label}: unpacked, collapsed, hidden, packed and reloaded, no page error; " + "; ".join(desk.notes)


def lit(desk: Desk) -> list[dict[str, object]]:
    found: list[dict[str, object]] = desk.page.evaluate(LIT_JS)
    return found


def walk_order(desk: Desk) -> list[str]:
    found: list[str] = desk.page.evaluate(WALK_JS)
    return found


def mouse_onto(desk: Desk, selector: str) -> None:
    """A real mouse moved onto the row `selector` names, brought to the middle of the screen first."""
    page = desk.page
    page.locator(selector).evaluate("(li) => li.scrollIntoView({block: 'center'})")
    desk.settled()
    box = page.locator(selector).bounding_box()
    assert box is not None, f"{selector} has no box"
    page.mouse.move(box["x"] + box["width"] / 2, box["y"] + 12)
    page.mouse.move(box["x"] + box["width"] / 2 + 3, box["y"] + 14)
    desk.settled()


def selected_key(desk: Desk) -> str:
    rows = lit(desk)
    desk.check(len(rows) == 1, f"not one row lit: {rows}")
    row = rows[0]
    return ("card:" if row["card"] else "") + str(row["id"])


def parity(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
           scheme: str) -> str:
    """`#407 unpack-row-parity`: an unpacked row answers the mouse and the keys as any message row does."""
    desk, errors = open_page(chromium, args, f"parity-{label}", width, height, scheme, False, ParityApi())
    page = desk.page
    phone_thread_list(desk)
    # Negative control: the cards packed, ↓ goes card to card. Newest activity at the foot.
    cards = walk_order(desk)
    desk.check(len(cards) == 3 and all(key.startswith("card:") for key in cards), f"not three cards alone: {cards}")
    first, second = cards[0], cards[1]
    mouse_onto(desk, f'#side-cards > li[data-card][data-id="{first[5:]}"]')
    desk.check(selected_key(desk) == first, f"the mouse did not light the card: {lit(desk)}")
    page.keyboard.press("ArrowDown")
    desk.settled()
    desk.check(selected_key(desk) == second, f"↓ over packed cards went to {lit(desk)}, not {second}")

    unpack_all(desk)
    desk.wait_for(f"() => document.querySelectorAll('#side-cards > li[data-unpack-thread=\"{RUNNER}\"]').length >= 9",
                  "the summarised thread did not unpack under its card")
    desk.settled()
    order = walk_order(desk)
    desk.check([key for key in order if key.startswith("card:")] == cards, f"unpacking moved the cards: {order}")

    def block(card: str) -> list[str]:
        at = order.index(card) + 1
        rest = order[at:]
        return rest[:next((n for n, key in enumerate(rest) if key.startswith("card:")), len(rest))]

    runner_rows = block(f"card:{RUNNER}")
    desk.check(len(runner_rows) >= 9, f"the walk over the cards does not hold the unpacked rows: {order}")

    # The mouse over an unpacked row lights it, and only it, with the selection's ring and the focus.
    target = runner_rows[2]
    mouse_onto(desk, f'#side-cards > li[data-unpack-thread="{RUNNER}"][data-id="{target}"]')
    rows = lit(desk)
    desk.check(len(rows) == 1 and rows[0]["id"] == target and rows[0]["unpacked"] == RUNNER,
               f"the mouse over unpacked row {target} lit {rows}")
    desk.check(rows[0]["outline"] != "none", f"the lit unpacked row draws no ring: {rows[0]}")
    desk.check(bool(rows[0]["focused"]), "the row the mouse lit does not hold the focus")
    desk.shot("1-hover")

    # ↓ and ↑ through the block, in reading order, and across to the next card.
    page.keyboard.press("ArrowDown")
    desk.settled()
    desk.check(selected_key(desk) == runner_rows[3], f"↓ from {target} went to {lit(desk)}, not {runner_rows[3]}")
    page.keyboard.press("ArrowUp")
    desk.settled()
    desk.check(selected_key(desk) == target, f"↑ did not come back to {target}: {lit(desk)}")
    top_rows = block(first)
    desk.check(len(top_rows) >= 1, f"the first card's thread unpacked nothing: {order}")
    mouse_onto(desk, f'#side-cards > li[data-unpack-thread="{first[5:]}"][data-id="{top_rows[-1]}"]')
    page.keyboard.press("ArrowDown")
    desk.settled()
    desk.check(selected_key(desk) == second, f"↓ from a block's last row went to {lit(desk)}, not the next card")
    page.keyboard.press("ArrowDown")
    desk.settled()
    after = block(second)[0]
    desk.check(selected_key(desk) == after, f"↓ from the card went to {lit(desk)}, not its first row {after}")
    desk.shot("2-keys")

    # e on the selected row marks it Done as anywhere, and the list stays where the reader is.
    done_target = next(key for key in runner_rows[3:] if page.locator(
        f'#side-cards > li[data-unpack-thread="{RUNNER}"][data-id="{key}"][data-archived="false"]').count())
    done_row = f'#side-cards > li[data-unpack-thread="{RUNNER}"][data-id="{done_target}"]'
    mouse_onto(desk, done_row)
    was = page.locator(done_row).bounding_box()
    page.keyboard.press("e")
    desk.wait_for(f"() => document.querySelector('{done_row}[data-archived=\"true\"]') !== null",
                  f"e did not mark unpacked row {done_target} Done")
    desk.settled()
    now = page.locator(done_row).bounding_box()
    desk.check(was is not None and now is not None and abs(now["y"] - was["y"]) <= 2,
               f"the list moved under a Done on an unpacked row: {was} -> {now}")
    page.keyboard.press("z")
    desk.wait_for(f"() => document.querySelector('{done_row}[data-archived=\"false\"]') !== null",
                  "z did not put the Done back")
    desk.settled()

    # Enter on the selected row opens it, and again folds it; : and ; fold and open every unpacked row.
    folds = page.evaluate(FOLDS_JS)
    desk.check(len(folds) >= 6 and all(state == "true" for state in folds.values()),
               f"the unpacked replies do not arrive folded: {folds}")
    fold_target = next(key for key in runner_rows if key in folds)
    mouse_onto(desk, f'#side-cards > li[data-unpack-thread="{RUNNER}"][data-id="{fold_target}"]')
    page.keyboard.press("Enter")
    desk.settled()
    desk.check(page.evaluate(FOLDS_JS)[fold_target] == "false", f"Enter did not open unpacked row {fold_target}")
    page.keyboard.press("Enter")
    desk.settled()
    desk.check(page.evaluate(FOLDS_JS)[fold_target] == "true", f"Enter again did not fold unpacked row {fold_target}")
    page.keyboard.press(";")
    desk.settled()
    desk.check(all(state == "false" for state in page.evaluate(FOLDS_JS).values()), "; did not expand every unpacked row")
    desk.check(page.locator("#status").text_content() == "Long messages: all expanded", "; did not say what it did")
    desk.shot("3-expanded")
    page.keyboard.press(":")
    desk.settled()
    desk.check(all(state == "true" for state in page.evaluate(FOLDS_JS).values()), ": did not collapse every unpacked row")
    desk.check(page.locator("#status").text_content() == "Long messages: all collapsed", ": did not say what it did")

    # Fold read: the block's run is the run line every list draws, and Enter on it opens it.
    page.click("#todo-filter")
    desk.wait_for("() => document.getElementById('todo-filter').getAttribute('data-read-mode') === 'fold'",
                  "Fold read was not reached")
    desk.settled()
    head = f'#side-cards > li[data-unpack-thread="{REPLICA}"][data-read-run="head"]'
    desk.check(page.locator(head).count() == 1, "the raw block's read run is not the shared run line")
    run_box = page.locator(f"{head} > .read-run").bounding_box()
    bridge_box = page.locator(f"{head} > .reply-bridge").bounding_box()
    desk.check(run_box is not None and bridge_box is not None and bridge_box["x"] < run_box["x"],
               f"the run line hangs off no bridge: line {run_box}, bridge {bridge_box}")
    mouse_onto(desk, head)
    rows = lit(desk)
    desk.check(len(rows) == 1 and rows[0]["run"] == "head", f"the mouse did not light the run line: {rows}")
    desk.shot("4-run-lit")
    page.keyboard.press("Enter")
    desk.settled()
    desk.check(page.locator(head).count() == 0, "Enter on the run line did not open it")
    overflow = page.evaluate("() => document.documentElement.scrollWidth - window.innerWidth")
    desk.check(overflow <= 0, f"horizontal overflow of {overflow}px")
    desk.check(not errors, f"page errors: {errors}")
    page.context.close()
    return f"parity-{label}: hover, ↓/↑ across cards and rows, e and z, Enter, ; and :, and the run line, as elsewhere"


def perf(chromium: BrowserType, args: argparse.Namespace) -> str:
    desk, errors = open_page(chromium, args, "perf-1280-dark", 1280, 800, "dark", False, ManyApi())
    page = desk.page
    page.fill("#api-token", fixture.TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length >= 50", "All did not open")
    page.select_option("#thread-select", "threads")
    desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-card]').length === 40",
                  "forty cards were not listed")
    started = time.monotonic()
    unpack_all(desk)
    desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-unpack-thread]').length >= 160",
                  "forty threads did not unpack", seconds=30)
    desk.settled()
    elapsed = time.monotonic() - started
    rows = page.evaluate("() => document.querySelectorAll('#side-cards > li[data-unpack-thread]').length")
    desk.check(not errors, f"page errors: {errors}")
    desk.shot("1-forty-unpacked")
    page.context.close()
    return f"perf: 40 threads, {rows} unpacked rows drawn in {elapsed:.2f}s from the press"


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
        for label, width, height, scheme, touch in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, scheme, touch))
        for label, width, height, scheme, touch in SIZES:
            if not touch:
                report.append(parity(playwright.chromium, args, label, width, height, scheme))
        report.append(perf(playwright.chromium, args))
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
