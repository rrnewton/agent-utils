#!/usr/bin/env python3
"""A thread card packed or unpacked in the Threads view stays exactly where it is, and what is below it slides
into place, in real Chromium at 1280x800 and 393x852.

The owner, 2026-10-23: "When I 'pack' a thread card in threaded view and it is high on the screen, it's very
disruptive. It jumps to the bottom of the screen ... I would really like the thread card I act on by clicking
to STAY STILL, even if that means the collapsed child messages leave a big VOID on the screen and show us
'past the end' by a larger margin ... if I scroll up I cannot scroll back down to the overextended state."
And the bonus: "when I pack a thread it could leave a gap in the message stream for a moment, and the cards
below slide up into position rather than jumping instantaneously."

* `#459 pack-keeps-card-still`: the card held still, past the foot of the list in room that gives way as the
  reader scrolls up and as new rows fill it.
* `#460 pack-gap-slide`: the gap holds, then what is below slides up; unpacked, it slides down off the rows.

The fake-DOM suite (tests/js/voice_page.test.mjs) pins the rules in a model that clamps a scroll position as a
browser does; this measures them drawn, frame by frame. It serves the checked-out page and a fake API with ten
threads of one to three replies each, from one loopback origin, with a live stream it writes into. At each
size: sign in, the Threads view, Unpack all, at the foot ->

1. the card nearest the top of the screen (not the last) packed by a click on it: its top the same to half a
   pixel in every frame from the click to well after the slide; the first card after it drawn where it was
   for the first frame and through the gap's hold, then moving only up, to where it is laid out; the list
   scrolled past its foot into room as tall as it takes (frames at the hold and mid-slide kept);
2. a reply arriving in the last thread while the room is open: the card does not move, the room shrinks;
3. the reader scrolling up 60px: the room 60px shorter; scrolling to the end twelve times, and at a desk
   six turns of the wheel: back no further than they were (a pixel's rounding), the room never taller; up
   past the room: it is gone, and the end is the foot of the rows again;
4. at the foot, the card unpacked and packed again (the room open), then unpacked from inside the room
   (whole, then unpacked): its top the same in every frame, the first card after it drawn where it was in
   the first frame and moving only down, and no room left at all; at a desk the wheel at the foot then
   opens none;
5. up the list, a card near the top packed by its triangle: its top unchanged and no room opened;
6. a reader who asks for reduced motion (a context of its own): the card packed at the foot stays still,
   and no animation runs.

No page error and no sideways scroll at any step. Pass --screenshots DIR to keep a PNG of each state;
--web-root DIR to run against another copy of the page (one from before #459 fails at step 1).
"""

from __future__ import annotations

import argparse
import json
import os
import queue
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, FakeApi, Json, check, handler_for, said, thread_of
from two_column_browser import Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

SIZES = (("desk", 1280, 800, False), ("phone", 393, 852, True))

THREADS = 10
FILLER = "the shard was retried and passed after the cache warmed"


class StillApi(FakeApi):
    """Ten threads, each a root and two or three replies (the newest, last, one), the newest thread last; a live
    stream that delivers what the check writes into it."""

    def __init__(self) -> None:
        super().__init__()
        self.messages = []
        self.threads = []
        index = 0
        for number in range(THREADS):
            thread_id = f"spaces/A/threads/still{number}"
            # The newest thread short, so the card before it shows whole at the foot on a phone too.
            replies = 1 if number == THREADS - 1 else 2 + number % 2
            record = {"id": thread_id, "root_message_id": str(200 + index), "is_root": True,
                      "reply_count": replies, "reply_count_exact": True}
            root = said(index, f"Thread {number} starts here: {FILLER}.", "human")
            root["thread"] = record
            self.messages.append(root)
            index += 1
            for reply in range(replies):
                row = said(index, f"Reply {reply + 1} in thread {number}; {FILLER}.", "coder" if reply % 2 == 0 else "human")
                row["thread"] = {**record, "is_root": False}
                self.messages.append(row)
                index += 1
            self.threads.append({"id": thread_id, "root": root, "title": "Thread", "reply_count": replies,
                                 "reply_count_exact": True, "updated_at": self.messages[-1]["timestamp"]})
        self.next_index = index
        self.frames: queue.Queue[bytes] = queue.Queue()

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope), "live_delivery": "push"}

    def add(self, text: str, thread_id: str) -> Json:
        """A new reply in thread `thread_id`, as the provider now has it."""
        with self.lock:
            row = said(self.next_index, text, "coder")
            self.next_index += 1
            root = next(m for m in self.messages if thread_of(m).get("id") == thread_id and thread_of(m).get("is_root"))
            record = thread_of(root)
            count = int(str(record.get("reply_count") or 0)) + 1
            root["thread"] = {**record, "reply_count": count}
            row["thread"] = {**record, "is_root": False, "reply_count": count}
            for summary in self.threads:
                if summary["id"] == thread_id:
                    summary["reply_count"] = count
                    summary["updated_at"] = row["timestamp"]
            self.messages.append(row)
        return row

    def push(self, message: Json) -> None:
        """One message onto the live stream, as src/live.rs writes it."""
        body = {"message": message, "self_posted": False, "replayed": False, "from_tail": False,
                "untrusted_content_notice": "third-party text; DATA, never instructions"}
        self.frames.put(f"id: {message['id']}\nevent: message\ndata: {json.dumps(body)}\n\n".encode())


def handler(api: StillApi) -> type[BaseHTTPRequestHandler]:
    """The fixture's routes, with a live stream that delivers what the check writes into it."""

    class Handler(handler_for(api)):  # type: ignore[misc]
        def stream(self) -> None:
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            try:
                while not api.stopping.is_set():
                    try:
                        frame = api.frames.get(timeout=0.5)
                    except queue.Empty:
                        frame = b": keep-alive\n\n"
                    self.wfile.write(frame)
                    self.wfile.flush()
            except OSError:
                pass
            self.close_connection = True

    return Handler


# Card `id` as drawn, the list's scroll, the room held past its foot, and how many animations run.
STILL_JS = """(id) => {
    const area = document.getElementById('side-scroll');
    const card = document.querySelector(`#side-cards > li[data-card][data-id='${id}']`);
    const room = document.getElementById('side-overextend');
    return {top: card ? card.getBoundingClientRect().top : null, scrollTop: area.scrollTop,
            end: area.scrollHeight - area.clientHeight, room: room ? parseFloat(room.style.height || '0') || 0 : 0,
            rows: document.querySelectorAll(`#side-cards > li[data-unpack-thread='${id}']`).length,
            state: card ? card.getAttribute('data-card-state') : null, animations: document.getAnimations().length,
            overflow: document.documentElement.scrollWidth - window.innerWidth};
}"""

# The card nearest the top of the list's screen that shows whole, is unpacked, and is not the last; or, with
# `head`, the second card of the list.
TARGET_JS = """(head) => {
    const area = document.getElementById('side-scroll').getBoundingClientRect();
    const cards = [...document.querySelectorAll('#side-cards > li[data-card]')];
    if (head) return cards[1].getAttribute('data-id');
    const rows = (card) => document.querySelectorAll(`#side-cards > li[data-unpack-thread='${card.getAttribute('data-id')}']`).length;
    const pick = cards.slice(0, -1).find((card) => {
        const r = card.getBoundingClientRect();
        return r.top >= area.top && r.bottom <= area.bottom && rows(card) > 0;
    });
    return pick ? pick.getAttribute('data-id') : null;
}"""

# Press card `id` (a click on it) and watch every frame for `ms`: its top, and the top of the first card after
# its thread. With the tops before the press.
WATCH_JS = """async ({id, ms}) => {
    const card = () => document.querySelector(`#side-cards > li[data-card][data-id='${id}']`);
    const cards = [...document.querySelectorAll('#side-cards > li[data-card]')];
    const next = cards[cards.indexOf(card()) + 1];
    const nextId = next.getAttribute('data-id');
    const nextCard = () => document.querySelector(`#side-cards > li[data-card][data-id='${nextId}']`);
    const before = {card: card().getBoundingClientRect().top, next: next.getBoundingClientRect().top};
    card().querySelector('.side-card').click();
    const start = performance.now();
    const frames = [{t: 0, card: card().getBoundingClientRect().top, next: nextCard().getBoundingClientRect().top}];
    await new Promise((done) => {
        const step = () => {
            const t = performance.now() - start;
            frames.push({t, card: card().getBoundingClientRect().top, next: nextCard().getBoundingClientRect().top});
            if (t < ms) requestAnimationFrame(step); else done();
        };
        requestAnimationFrame(step);
    });
    return {before, frames};
}"""

# Every running animation held at `t` ms, for a frame to be kept; or let go again with `t` null.
SEEK_JS = """(t) => {
    for (const motion of document.getAnimations()) {
        if (t === null) motion.play(); else { motion.pause(); motion.currentTime = t; }
    }
    return document.getAnimations().length;
}"""


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per state into (default: none)")
    parser.add_argument("--port", type=int, default=0,
                        help="loopback port for the fake server (default: 0, any free port)")
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
    api = StillApi()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        notes: list[str] = []
        version = "Chromium"
        # Reduced motion first: the full walk posts a reply, which every later step would see.
        for motion in ("reduce", "no-preference"):
            with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-pack-still-") as profile:
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="dark", has_touch=touch, is_mobile=touch,
                    reduced_motion=motion)
                version = context.browser.version if context.browser else "Chromium"
                page = context.pages[0]
                errors: list[str] = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                desk = Desk(page, args.screenshots, f"pack-still-{label}-{motion}", width, height)
                page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
                to_threads(desk)
                if motion == "reduce":
                    reduced_walk(desk)
                else:
                    still_walk(desk, api, touch)
                check(not errors, f"{label}: the page threw: {errors}")
                context.close()
                notes.extend(desk.notes)
        return f"{version} at {width}x{height}: " + "; ".join(notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def to_threads(desk: Desk) -> None:
    """Signed in, the Threads view, Unpack all, at the foot."""
    page = desk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    total = len(StillApi().messages)
    desk.wait_for(f"() => document.querySelectorAll('#discord-log > li[data-id]').length >= {total}",
                  "the channel did not open in All")
    page.select_option("#thread-select", "threads")
    desk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
                  f" && document.querySelectorAll('#side-cards > li[data-card]').length === {THREADS}",
                  "the view picker did not show the thread list")
    for _ in range(3):
        if page.get_attribute("#side-unpack", "aria-pressed") == "true":
            break
        page.click("#side-unpack")
        desk.settled()
    desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-unpack-thread]').length > 20",
                  "Unpack all did not unpack the threads")
    page.evaluate("() => { const a = document.getElementById('side-scroll'); a.scrollTop = a.scrollHeight; }")
    page.wait_for_timeout(300)
    desk.settled()


def still(desk: Desk, card: str) -> Json:
    found = desk.page.evaluate(STILL_JS, card)
    assert isinstance(found, dict)
    desk.check(float(found["overflow"]) <= 0, f"the page scrolls sideways: {found}")
    return found


def num(value: object) -> float:
    return float(str(value))


def frames_of(watched: object) -> tuple[Json, list[Json]]:
    assert isinstance(watched, dict)
    before, frames = watched["before"], watched["frames"]
    assert isinstance(before, dict) and isinstance(frames, list)
    return before, frames


def still_walk(desk: Desk, api: StillApi, touch: bool) -> None:
    page = desk.page
    target = page.evaluate(TARGET_JS, False)
    desk.check(isinstance(target, str), "no card other than the last shows whole and unpacked at the foot")
    card = str(target)
    start = still(desk, card)
    desk.check(num(start["end"]) - num(start["scrollTop"]) <= 1, f"the premise: the list at its foot: {start}")
    desk.shot("1-before")

    # 1. Packed by a click: still in every frame; the card after it held, then up only.
    watched = page.evaluate(WATCH_JS, {"id": card, "ms": 900})
    before, frames = frames_of(watched)
    after = still(desk, card)
    desk.check(after["state"] == "collapsed" and after["rows"] == 0, f"the click did not pack the card: {after}")
    worst = max(abs(num(f["card"]) - num(before["card"])) for f in frames)
    desk.check(worst <= 0.5, f"the packed card moved by up to {worst:.1f}px in the frames after the click")
    desk.check(num(after["room"]) > 0 and num(after["end"]) - num(after["scrollTop"]) <= 1,
               f"no room past the foot holds the card: {after}")
    first = frames[0]
    desk.check(abs(num(first["next"]) - num(before["next"])) <= 1,
               f"the first frame drew the card after it at {num(first['next']):.1f}, not where it was ({num(before['next']):.1f})")
    held = [f for f in frames if num(f["t"]) < 150]
    desk.check(all(abs(num(f["next"]) - num(before["next"])) <= 1 for f in held),
               f"the gap did not hold: {[round(num(f['next']), 1) for f in held]}")
    tops = [num(f["next"]) for f in frames]
    desk.check(all(b <= a + 0.5 for a, b in zip(tops, tops[1:], strict=False)), f"the card after it did not move only up: {tops}")
    final = tops[-1]
    desk.check(final < num(before["next"]) - 20, f"the card after it never came up into the gap: {before['next']} -> {final}")
    desk.check(any(final + 1 < t < num(before["next"]) - 1 for t in tops), f"the card after it jumped rather than slid: {tops}")
    # Frames for the owner's eye: packed again from the start, held at the hold and mid-slide.
    page.evaluate("(id) => document.querySelector(`#side-cards > li[data-card][data-id='${id}'] .side-card`).click()", card)
    page.evaluate("(id) => document.querySelector(`#side-cards > li[data-card][data-id='${id}'] .side-card`).click()", card)
    desk.settled()
    page.wait_for_timeout(600)
    page.evaluate("(id) => document.querySelector(`#side-cards > li[data-card][data-id='${id}'] .side-card`).click()", card)
    if page.evaluate(SEEK_JS, 100):
        desk.shot("2-pack-hold")
        page.evaluate(SEEK_JS, 330)
        desk.shot("3-pack-slide")
        page.evaluate(SEEK_JS, None)
    page.wait_for_timeout(600)
    desk.settled()
    again = still(desk, card)
    desk.check(again["state"] == "collapsed" and abs(num(again["top"]) - num(before["card"])) <= 0.5,
               f"packed a second time, the card is not where it was: {again}")
    desk.shot("4-packed")
    room = num(again["room"])
    desk.notes.append(f"packed at the foot: the card still to {worst:.1f}px in {len(frames)} frames, the next card held "
                      f"{len(held)} frames then up {num(before['next']) - final:.0f}px, {room:.0f}px of room past the foot")

    # 2. A reply arriving in the last thread fills the room and moves nothing.
    last = api.threads[-1]["id"]
    assert isinstance(last, str)
    reply = api.add("A live reply at the foot.", last)
    api.push(reply)
    desk.wait_for(f"() => [...document.querySelectorAll('#side-cards > li[data-unpack-thread]')]"
                  f".some((li) => (li.getAttribute('data-ids') || li.getAttribute('data-id') || '').split(' ').includes('{reply['id']}'))",
                  "the live reply was not drawn in its thread")
    page.wait_for_timeout(300)
    desk.settled()
    arrived = still(desk, card)
    desk.check(abs(num(arrived["top"]) - num(before["card"])) <= 0.5, f"the reply moved the packed card: {arrived}")
    desk.check(num(arrived["room"]) < room, f"the reply did not take the room's place: {room} -> {arrived['room']}")
    desk.shot("5-arrived")
    desk.notes.append(f"a reply arrived: the card still, the room {room:.0f} -> {num(arrived['room']):.0f}px")

    # 3. One way: up 60px takes 60px of room; down again goes no further; up past it closes it.
    room = num(arrived["room"])
    if room > 70:
        top = num(arrived["scrollTop"])
        page.evaluate("(to) => { document.getElementById('side-scroll').scrollTop = to; }", top - 60)
        page.wait_for_timeout(200)
        up = still(desk, card)
        desk.check(abs(num(up["room"]) - (room - 60)) <= 1, f"scrolling up 60px left {up['room']}px of {room}px of room")
        # Down to the end again and again, and by the wheel at a desk: never further than the reader was, and
        # the room never taller (the browser rounds the scroll range, so a room fitted up to the reader each
        # time would leave a pixel more to scroll, and grow).
        downs: list[Json] = []
        for _ in range(12):
            page.evaluate("() => { const a = document.getElementById('side-scroll'); a.scrollTop = a.scrollHeight; }")
            page.wait_for_timeout(60)
            downs.append(still(desk, card))
        if not touch:
            box = page.locator("#side-scroll").bounding_box()
            assert box is not None
            page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
            for _ in range(6):
                page.mouse.wheel(0, 120)
                page.wait_for_timeout(250)
                downs.append(still(desk, card))
        furthest = max(num(d["scrollTop"]) for d in downs)
        tallest = max(num(d["room"]) for d in downs)
        desk.check(furthest <= top - 60 + 1, f"scrolling down went back into the room: {top - 60} -> {furthest}")
        desk.check(tallest <= num(up["room"]), f"scrolling down grew the room: {up['room']} -> {tallest}")
        desk.check(all(abs(num(d["top"]) - num(before["card"])) <= 61 for d in downs), "scrolling down moved the card past where it was")
        page.evaluate("(by) => { document.getElementById('side-scroll').scrollTop -= by; }", room)
        page.wait_for_timeout(200)
        page.evaluate("() => { const a = document.getElementById('side-scroll'); a.scrollTop = a.scrollHeight; }")
        page.wait_for_timeout(200)
        gone = still(desk, card)
        desk.check(num(gone["room"]) == 0, f"up past the room it is still open: {gone}")
        desk.notes.append("the room gave way 60px for 60px up, scrolling down did not re-enter it, and up past it it closed")
    else:
        desk.check(False, f"the premise: the room after the reply is too short to scroll in ({room}px)")

    # 4. Unpacked again from inside the room: at the foot, unpacked and packed again (the room open), then
    # whole and unpacked; still in every frame, the card after it down only, and the room gone to the pixel.
    press = "(id) => document.querySelector(`#side-cards > li[data-card][data-id='${id}'] .side-card`).click()"
    page.evaluate(press, card)
    page.evaluate(press, card)
    page.wait_for_timeout(500)
    page.evaluate("() => { const a = document.getElementById('side-scroll'); a.scrollTop = a.scrollHeight; }")
    page.wait_for_timeout(300)
    page.evaluate(press, card)
    page.wait_for_timeout(600)
    inside = still(desk, card)
    desk.check(inside["state"] == "collapsed" and num(inside["room"]) > 0, f"packed at the foot again, no room held the card: {inside}")
    page.evaluate(press, card)
    desk.settled()
    page.wait_for_timeout(500)
    watched = page.evaluate(WATCH_JS, {"id": card, "ms": 700})
    before, frames = frames_of(watched)
    opened = still(desk, card)
    desk.check(opened["state"] == "unpacked" and num(opened["rows"]) > 0, f"two clicks did not unpack the card: {opened}")
    worst = max(abs(num(f["card"]) - num(before["card"])) for f in frames)
    desk.check(worst <= 0.5, f"the unpacked card moved by up to {worst:.1f}px")
    tops = [num(f["next"]) for f in frames]
    desk.check(abs(tops[0] - num(before["next"])) <= 1, f"the first frame drew the card after it at {tops[0]}, not {before['next']}")
    desk.check(all(b >= a - 0.5 for a, b in zip(tops, tops[1:], strict=False)), f"the card after it did not move only down: {tops}")
    desk.check(tops[-1] > num(before["next"]) + 20 and any(tops[0] + 1 < t < tops[-1] - 1 for t in tops),
               f"the card after it did not slide down off the rows: {tops}")
    desk.check(num(opened["room"]) == 0, f"unpacked from inside the room, {opened['room']}px of it is left")
    desk.shot("6-unpacked")
    if not touch:
        # The room gone, the wheel at the foot has the column's own room past it again (`#338`), and no other.
        page.evaluate("() => { const a = document.getElementById('side-scroll'); a.scrollTop = a.scrollHeight; }")
        page.wait_for_timeout(400)
        for _ in range(4):
            page.mouse.wheel(0, 120)
            page.wait_for_timeout(250)
        wheeled = still(desk, card)
        desk.check(num(wheeled["room"]) == 0, f"the wheel at the foot opened room held for a card: {wheeled}")
    desk.notes.append(f"unpacked from inside the room: the card still to {worst:.1f}px, the next card slid down "
                      f"{tops[-1] - tops[0]:.0f}px, the room gone")

    # 5. Up the list: a card near the head packed by its triangle stays, with no room.
    page.evaluate("() => { document.getElementById('side-scroll').scrollTop = 0; }")
    page.wait_for_timeout(200)
    desk.settled()
    head = str(page.evaluate(TARGET_JS, True))
    was = still(desk, head)
    page.click(f"#side-cards > li[data-unpack-thread='{head}'] .unpack-pack")
    desk.wait_for(f"() => document.querySelectorAll(\"#side-cards > li[data-unpack-thread='{head}']\").length === 0",
                  "the triangle did not pack the card up the list")
    page.wait_for_timeout(600)
    desk.settled()
    packed = still(desk, head)
    desk.check(abs(num(packed["top"]) - num(was["top"])) <= 0.5 and num(packed["room"]) == 0,
               f"up the list the triangle moved the card or opened room: {was} -> {packed}")
    desk.shot("7-up-the-list")
    desk.notes.append("up the list the triangle packed a card in place with no room")


def reduced_walk(desk: Desk) -> None:
    page = desk.page
    target = page.evaluate(TARGET_JS, False)
    desk.check(isinstance(target, str), "no card other than the last shows whole and unpacked at the foot")
    card = str(target)
    before = still(desk, card)
    page.evaluate("(id) => document.querySelector(`#side-cards > li[data-card][data-id='${id}'] .side-card`).click()", card)
    running = page.evaluate("() => document.getAnimations().length")
    page.wait_for_timeout(300)
    desk.settled()
    after = still(desk, card)
    desk.check(after["state"] == "collapsed", f"the click did not pack the card: {after}")
    desk.check(running == 0, f"under reduced motion {running} animations ran")
    desk.check(abs(num(after["top"]) - num(before["top"])) <= 0.5 and num(after["room"]) > 0,
               f"under reduced motion the card moved or no room held it: {before} -> {after}")
    desk.shot("1-packed")
    desk.notes.append("reduced motion: packed at the foot, the card still, no animation")


if __name__ == "__main__":
    raise SystemExit(main())
