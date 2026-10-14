#!/usr/bin/env python3
"""A thread's colour strip on the rows of All, and selecting a thread by it, in real Chromium at 1280x800 and 412x915.

`#299 all-thread-strip`. The owner, 2026-10-14: "In the interleaved All view I would like a little color
strip along the left edge of the rectangular messages that are messages in threads. Not the curved style
of the thread cards, just the normal message view but with a color accent on the left. Somewhat thicker
than the normal border so it is clickable. But when this border is clicked instead of drilling INTO the
thread (like clicking the existing "(Thread(N))" button) I want it to just SELECT that thread, dimming all
messages that are not part of the thread but keeping us in the global view." The fake-DOM suite
(tests/js/voice_page.test.mjs) pins what the page does; it lays nothing out and draws nothing, so it
cannot say how wide a strip is, whether a row moved, or whether a strip is coloured. This does, from the
boxes and colours Chromium computes and the pixels a screenshot holds.

It serves the checked-out page and the fake API of tests/two_column_browser.py — a channel taller than
the window with three answered threads — with one more message, heading a thread nobody has answered,
two of the summarised thread's replies already Done, and a live stream the check writes into.

At a 1280x800 desk with a mouse, in the dark scheme and the light, in All:

  every row of the three answered threads has a strip, and no other row: not a message in no thread,
  not the root nobody has answered -> each strip's bar at least 5px and four times the row's border,
  its hit box at least 16px wide and the row's full height, in its Thread(N)'s colour by computed
  colour, and by its pixels — on a read row too -> every row's words right of the hit box, at the same
  inset whether the row has a strip or not -> a resting mouse: a pointer, and the bar wider -> the keys:
  a ring on the strip -> a click on the summarised thread's strip: nothing scrolls, no row moves by a
  pixel, still All; the rows of other threads and of none recede to 0.3 and drain, their strips grey
  by pixels; the thread's own at full strength, their strips its colour -> a live reply of the thread
  arrives at full strength, a live message of no thread dimmed -> the strip again lets go; Esc lets go;
  another thread's strip moves the selection; t on a message selects its thread and t again lets go ->
  Thread(N) opens the thread, and Back finds All with the thread still selected -> the first reply to
  the thread nobody had answered, live: its root gets a strip, and its words do not move.

On a 412x915 phone by touch, in the dark scheme and the light: the strips a 22px hit box and a 6px
bar -> a tap selects the thread -> a finger drawn sideways from a strip archives nothing and opens no
details -> no horizontal overflow.

No page error at any step. Pass --screenshots DIR to keep a PNG of each state for review; --web-root
DIR to run the same walk against another copy of the page.
"""

from __future__ import annotations

import argparse
import base64
import json
import os
import queue
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, Json, check, handler_for, said, thread_of
from thread_card_focus_browser import COLOURED, COLOURED_PIXEL, GREY, PIXELS_JS, chroma
from two_column_browser import CANARY, REPLICA, RUNNER, ColumnsApi, Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, BrowserType

# (label, width, height, colour scheme, touch).
SIZES = (
    ("desk-1280-dark", 1280, 800, "dark", False),
    ("desk-1280-light", 1280, 800, "light", False),
    ("phone-412-dark", 412, 915, "dark", True),
    ("phone-412-light", 412, 915, "light", True),
)

# A thread nobody has answered: its root is the channel's newest message, with a count of nothing.
LONE = "spaces/A/threads/lone"
LONE_ROOT = "224"
# A reply of the summarised thread the walk starts at the top of the screen, with the long question's
# thread's rows among the rows under it; and two of the summarised thread's replies already marked Done.
PIVOT = "208"
READ_REPLIES = ("208", "211")


class StripApi(ColumnsApi):
    """The two-column channel, one more message heading a thread nobody has answered, two read replies,
    and a live stream the check writes events into."""

    def __init__(self) -> None:
        super().__init__()
        self.frames: queue.Queue[bytes] = queue.Queue()
        lone = said(24, "A brand-new question that nobody has answered yet.", "human")
        lone["thread"] = {"id": LONE, "root_message_id": LONE_ROOT, "is_root": True,
                          "reply_count": 0, "reply_count_exact": True}
        self.messages.append(lone)
        self.next_index = 25

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope), "live_delivery": "push"}

    def timeline(self, query: dict[str, list[str]]) -> Json:
        answer = super().timeline(query)
        ids = {str(m["id"]) for m in answer["messages"]} if isinstance(answer["messages"], list) else set()
        return {**answer, "dismissed": [one for one in READ_REPLIES if one in ids]}

    def add(self, text: str, thread_id: str | None = None) -> Json:
        """A new message, posted to the channel or into a thread, as the provider now has it."""
        with self.lock:
            index = self.next_index
            self.next_index += 1
            row = said(index, text, "human" if index % 2 else "coder")
            if thread_id is not None:
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


def handler(api: StripApi) -> type[BaseHTTPRequestHandler]:
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


# Every row of All as Chromium lays it out and colours it: its strip's hit box and bar, its tag's
# colour, where its words start, and how far it recedes.
ROWS_JS = """() => {
    const rgb = (text) => (String(text).match(/[0-9.]+/g) || []).slice(0, 3).map(Number);
    const box = (node) => { const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height}; };
    const list = document.getElementById('discord-log');
    const rows = [...list.children].filter((li) => li.matches('li[data-id]') && li.getClientRects().length > 0
        && li.getAttribute('data-read-run') !== 'head').map((li) => {
        const strip = [...li.children].find((kid) => kid.classList.contains('thread-strip'));
        const shown = Boolean(strip) && strip.getClientRects().length > 0;
        const bar = shown ? getComputedStyle(strip, '::before') : null;
        const badge = li.querySelector('.thread-badge');
        const style = getComputedStyle(li);
        // How strongly the row is drawn: its opacity, or, for a row that recedes in its box, the veil of
        // the page's colour over it, as opaque as the row would have been transparent.
        const after = getComputedStyle(li, '::after');
        const veil = String(after.backgroundColor).match(/[0-9.]+/g) || [];
        const alpha = after.content === 'none' ? 0 : veil.length >= 4 ? Number(veil[3]) : veil.length === 3 ? 1 : 0;
        return {
            drawn: parseFloat(style.opacity) * (1 - alpha),
            id: li.getAttribute('data-id'), box: box(li), meta: box(li.querySelector(':scope > .meta')),
            border: parseFloat(style.borderLeftWidth), opacity: parseFloat(style.opacity),
            rowOpacity: style.getPropertyValue('--row-opacity').trim(), rowFilter: style.getPropertyValue('--row-filter').trim(),
            within: li.hasAttribute('data-thread-in'),
            strip: shown ? {box: box(strip), bar: parseFloat(bar.width), colour: rgb(bar.backgroundColor),
                            cursor: getComputedStyle(strip).cursor, pressed: strip.getAttribute('aria-pressed'),
                            label: strip.getAttribute('aria-label')} : null,
            tag: badge && badge.getAttribute('data-thread-state') === 'open' ? rgb(getComputedStyle(badge).borderLeftColor) : null,
        };
    });
    const area = document.getElementById('scroll-area');
    return {rows, view: document.getElementById('thread-select').value, focus: list.getAttribute('data-focus-thread'),
            scrollTop: area.scrollTop, overflow: document.documentElement.scrollWidth - window.innerWidth};
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
        for label, width, height, scheme, touch in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, scheme, touch))
    for line in report:
        print(line)
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, scheme: str,
         touch: bool) -> str:
    """One reader at one size in one scheme, against a fresh fake API and a fresh browser profile."""
    api = StripApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-strip-") as profile:
            if touch:
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
            desk = Desk(page, args.screenshots, label, width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            open_all(desk, api)
            if touch:
                phone_walk(desk, api, context)
            else:
                desk_walk(desk, api)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}, {scheme}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


# --- reading what was measured -----------------------------------------------------------------------


def state(desk: Desk) -> Json:
    found: Json = desk.page.evaluate(ROWS_JS)
    return found


def rows_of(found: Json) -> list[Json]:
    value = found["rows"]
    assert isinstance(value, list)
    return [row for row in value if isinstance(row, dict)]


def part(item: Json, key: str) -> Json:
    value = item[key]
    assert isinstance(value, dict), f"{key} is {value!r}"
    return value


def number(item: Json, key: str) -> float:
    value = item[key]
    assert isinstance(value, (int, float)), f"{key} is {value!r}"
    return float(value)


def colour(item: Json, key: str) -> list[float]:
    value = item[key]
    assert isinstance(value, list)
    return [float(channel) for channel in value]


def row_by_id(found: Json, row_id: str) -> Json:
    row = next((row for row in rows_of(found) if row["id"] == row_id), None)
    check(row is not None, f"row {row_id} is not drawn")
    assert row is not None
    return row


def pixels(desk: Desk, points: list[tuple[float, float]]) -> list[list[float]]:
    """The colour drawn at each viewport point, from one screenshot: what the reader sees there."""
    png = base64.b64encode(desk.page.screenshot()).decode("ascii")
    found = desk.page.evaluate(PIXELS_JS, {"png": png, "points": [list(point) for point in points]})
    assert isinstance(found, list)
    return [[float(channel) for channel in point] for point in found]


def bar_point(row: Json) -> tuple[float, float]:
    """A point inside a row's strip's bar, halfway down the row."""
    hit, box = part(row, "strip"), part(row, "box")
    return number(part(hit, "box"), "left") + 2.5, (number(box, "top") + number(box, "bottom")) / 2


def in_view(desk: Desk, row: Json) -> bool:
    box = part(row, "box")
    return number(box, "top") >= 0 and number(box, "bottom") <= desk.height - 120


def threads_of(api: StripApi) -> dict[str, str]:
    """Each message's thread, by message id, for the answered threads."""
    out: dict[str, str] = {}
    for message in api.messages:
        record = message.get("thread")
        if isinstance(record, dict) and record["id"] in (RUNNER, REPLICA, CANARY):
            out[str(message["id"])] = str(record["id"])
    return out


# --- the walks -----------------------------------------------------------------------------------------


def open_all(desk: Desk, api: StripApi) -> None:
    page = desk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    desk.wait_for(f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {len(api.messages)}",
                  "the channel did not open")
    if page.evaluate("() => document.getElementById('thread-select').value") != "flat":
        page.select_option("#thread-select", "flat")
    desk.wait_for("() => document.getElementById('thread-select').value === 'flat'"
                  " && document.getElementById('discord-log').getAttribute('data-view') === 'flat'", "All did not open")
    page.evaluate(f"() => document.querySelector(\"#discord-log > li[data-id='{PIVOT}']\").scrollIntoView({{block: 'start'}})")
    desk.settled()
    desk.settled()


def check_strips(desk: Desk, api: StripApi, found: Json, touch: bool) -> str:
    """Which rows have a strip, how big and what colour each is, and where every row's words start."""
    threaded = threads_of(api)
    rows = rows_of(found)
    striped = sorted(str(row["id"]) for row in rows if row["strip"])
    expected = sorted(one for one in (str(row["id"]) for row in rows) if one in threaded)
    desk.check(striped == expected, f"the strips are on {striped}, not the answered threads' rows {expected}")
    desk.check(row_by_id(found, LONE_ROOT)["strip"] is None, "the root nobody has answered has a strip")
    want_bar, want_hit = (6, 22) if touch else (5, 16)
    insets: set[float] = set()
    for row in rows:
        box, meta = part(row, "box"), part(row, "meta")
        insets.add(round(number(meta, "left") - number(box, "left"), 1))
        if not row["strip"]:
            continue
        strip = part(row, "strip")
        hit = part(strip, "box")
        desk.check(number(strip, "bar") >= want_bar and number(strip, "bar") >= 4 * number(row, "border"),
                   f"{row['id']}: the bar is {strip['bar']}px, the row's border {row['border']}px")
        desk.check(number(hit, "width") >= want_hit, f"{row['id']}: the strip is hit over {hit['width']}px")
        desk.check(number(hit, "height") >= number(box, "height") and number(hit, "top") <= number(box, "top"),
                   f"{row['id']}: the strip is not the row's full height: {hit} on {box}")
        desk.check(number(hit, "left") <= number(box, "left") + 0.5, f"{row['id']}: the strip is not on the row's left edge")
        desk.check(number(meta, "left") >= number(hit, "right") - 0.5, f"{row['id']}: the words start under the strip")
        desk.check(row["tag"] is not None and colour(strip, "colour") == colour(row, "tag"),
                   f"{row['id']}: the bar is {strip['colour']}, its Thread(N) {row['tag']}")
        desk.check(chroma(colour(strip, "colour")) >= COLOURED, f"{row['id']}: the bar is not coloured: {strip['colour']}")
        desk.check(strip["cursor"] == "pointer" and strip["label"] == "Select this thread" and strip["pressed"] == "false",
                   f"{row['id']}: the strip is {strip}")
    desk.check(len(insets) == 1, f"the rows' words start at different insets, strip or none: {sorted(insets)}")
    # Drawn: each strip on screen in its colour, a read reply's too.
    shown = [row for row in rows if row["strip"] and in_view(desk, row)]
    desk.check(any(row["id"] in READ_REPLIES for row in shown), "no read reply is on screen, so this proves nothing")
    for row, drawn in zip(shown, pixels(desk, [bar_point(row) for row in shown])):
        desk.check(chroma(drawn) >= COLOURED_PIXEL, f"{row['id']}: its strip is drawn {drawn}, not its colour")
    desk.check(number(found, "overflow") <= 0, "the page scrolls sideways")
    return (f"{len(striped)} rows striped, the bar {number(part(shown[0], 'strip'), 'bar'):.0f}px in a "
            f"{number(part(part(shown[0], 'strip'), 'box'), 'width'):.0f}px hit box, every row's words at one inset, "
            f"{len(shown)} strips drawn in their colour, the read replies' too")


def check_selected(desk: Desk, api: StripApi, found: Json, thread_id: str, where: str) -> None:
    """Thread `thread_id` selected: its rows at full strength and their strips in its colour; every other
    row at 0.3 and drained, the strips of other threads grey."""
    threaded = threads_of(api)
    desk.check(found["focus"] == thread_id and found["view"] == "flat", f"{where}: selected {found['focus']} in {found['view']}")
    rows = rows_of(found)
    for row in rows:
        mine = threaded.get(str(row["id"])) == thread_id
        desk.check(bool(row["within"]) == mine, f"{where}: {row['id']} in the thread is {row['within']}")
        if mine:
            desk.check(row["rowOpacity"] == "1" and abs(number(row, "drawn") - 1) < 0.01,
                       f"{where}: {row['id']} of the thread is drawn at {row['drawn']}")
        else:
            desk.check(row["rowOpacity"] == "0.3" and row["rowFilter"] == "grayscale(1)",
                       f"{where}: {row['id']} out of the thread is at {row['rowOpacity']} {row['rowFilter']}")
            desk.check(abs(number(row, "drawn") - 0.3) < 0.02, f"{where}: {row['id']} out of the thread is drawn at {row['drawn']}")
    shown = [row for row in rows if row["strip"] and in_view(desk, row)]
    ours = [row for row in shown if threaded.get(str(row["id"])) == thread_id]
    theirs = [row for row in shown if threaded.get(str(row["id"])) != thread_id]
    desk.check(bool(ours) and bool(theirs), f"{where}: the thread's strips and another's are not both on screen")
    for row, drawn in zip(shown, pixels(desk, [bar_point(row) for row in shown])):
        if row in ours:
            desk.check(chroma(drawn) >= COLOURED_PIXEL, f"{where}: {row['id']}'s strip is drawn {drawn}, not its colour")
        else:
            desk.check(chroma(drawn) <= GREY, f"{where}: {row['id']}'s strip is drawn {drawn}, not grey")


def desk_walk(desk: Desk, api: StripApi) -> None:
    page = desk.page
    page.mouse.move(5, desk.height - 5)
    desk.settled()
    found = state(desk)
    desk.notes.append(check_strips(desk, api, found, touch=False))
    desk.shot("c0-all")

    # A resting mouse on a strip: the bar widens. The keys on it: a ring.
    root = row_by_id(found, PIVOT)
    x, y = bar_point(root)
    page.mouse.move(x + 4, y)
    desk.settled()
    widened = float(page.evaluate(f"() => parseFloat(getComputedStyle(document.querySelector(\"#discord-log > li[data-id='{PIVOT}'] > .thread-strip\"), '::before').width)"))
    desk.check(widened > number(part(root, "strip"), "bar"), f"a resting mouse did not widen the bar: {widened}px")
    page.mouse.move(5, desk.height - 5)
    page.keyboard.press("Shift")
    ring = page.evaluate(f"""() => {{ const strip = document.querySelector("#discord-log > li[data-id='{PIVOT}'] > .thread-strip");
        strip.focus({{preventScroll: true}}); const style = getComputedStyle(strip);
        return {{visible: strip.matches(':focus-visible'), outline: style.outlineStyle, width: parseFloat(style.outlineWidth)}}; }}""")
    desk.check(bool(ring["visible"]) and ring["outline"] == "solid" and float(ring["width"]) >= 2, f"the keys on the strip draw {ring}")
    page.evaluate("() => document.getElementById('scroll-area').focus({preventScroll: true})")
    page.mouse.move(5, desk.height - 5)
    desk.settled()

    # A click on the summarised thread's strip: nothing moves, and the rest recede.
    before = state(desk)
    page.mouse.click(x, y)
    page.mouse.move(5, desk.height - 5)
    desk.wait_for(f"() => document.getElementById('discord-log').getAttribute('data-focus-thread') === '{RUNNER}'",
                  "a click on the strip did not select its thread")
    desk.settled()
    after = state(desk)
    desk.check(number(after, "scrollTop") == number(before, "scrollTop"), "selecting a thread scrolled the list")
    moved = [str(row["id"]) for row in rows_of(after)
             if any(abs(number(part(row, "box"), k) - number(part(row_by_id(before, str(row["id"])), "box"), k)) >= 0.5 for k in ("top", "left", "height"))]
    desk.check(not moved, f"selecting a thread moved rows {moved}")
    check_selected(desk, api, after, RUNNER, "selected by its strip")
    desk.shot("c1-selected")
    desk.notes.append("a click on a strip selected its thread in All: nothing scrolled or moved, the rest at 0.3 and drained "
                      "with grey strips, the thread's own rows and strips at full strength")

    # Live: a reply of the thread arrives in it, a message of no thread out of it.
    reply = api.add("A reply arriving live in the selected thread.", RUNNER)
    api.push(reply)
    desk.wait_for(f"() => !!document.querySelector(\"#discord-log > li[data-id='{reply['id']}'][data-thread-in]\")",
                  "a live reply of the selected thread did not arrive in it")
    other = api.add("A message arriving live, in no thread.")
    api.push(other)
    desk.wait_for(f"() => !!document.querySelector(\"#discord-log > li[data-id='{other['id']}']\")", "the live message did not arrive")
    desk.settled()
    live = state(desk)
    desk.check(row_by_id(live, str(other["id"]))["rowOpacity"] == "0.3", "a live message of no thread arrived undimmed")
    desk.check(row_by_id(live, str(reply["id"]))["rowOpacity"] == "1", "a live reply of the thread arrived dimmed")

    # Again: let go. Esc: let go. Another thread's strip: moved. t: selects, and again lets go.
    page.evaluate(f"() => document.querySelector(\"#discord-log > li[data-id='{PIVOT}']\").scrollIntoView({{block: 'start'}})")
    desk.settled()
    root = row_by_id(state(desk), PIVOT)
    x, y = bar_point(root)
    page.mouse.click(x, y)
    desk.wait_for("() => !document.getElementById('discord-log').hasAttribute('data-focus-thread')", "a second click did not let go")
    page.mouse.click(x, y)
    desk.wait_for(f"() => document.getElementById('discord-log').getAttribute('data-focus-thread') === '{RUNNER}'", "a click did not select")
    page.keyboard.press("Escape")
    desk.wait_for("() => !document.getElementById('discord-log').hasAttribute('data-focus-thread')", "Esc did not let go")
    desk.check(page.evaluate("() => document.getElementById('thread-select').value") == "flat", "Esc left All")
    page.mouse.click(x, y)
    desk.wait_for(f"() => document.getElementById('discord-log').getAttribute('data-focus-thread') === '{RUNNER}'", "a click did not select")
    replica = next(row for row in rows_of(state(desk)) if threads_of(api).get(str(row["id"])) == REPLICA)
    page.evaluate(f"() => document.querySelector(\"#discord-log > li[data-id='{replica['id']}']\").scrollIntoView({{block: 'center'}})")
    desk.settled()
    page.mouse.click(*bar_point(row_by_id(state(desk), str(replica["id"]))))
    desk.wait_for(f"() => document.getElementById('discord-log').getAttribute('data-focus-thread') === '{REPLICA}'",
                  "another thread's strip did not move the selection")
    page.keyboard.press("Escape")
    desk.wait_for("() => !document.getElementById('discord-log').hasAttribute('data-focus-thread')", "Esc did not let go")
    page.focus(f"#discord-log > li[data-id='{replica['id']}']")
    page.keyboard.press("t")
    desk.wait_for(f"() => document.getElementById('discord-log').getAttribute('data-focus-thread') === '{REPLICA}'",
                  "t did not select the message's thread")
    page.keyboard.press("t")
    desk.wait_for("() => !document.getElementById('discord-log').hasAttribute('data-focus-thread')", "t again did not let go")

    # Thread(N) opens the thread; Back comes to All with the thread still selected.
    page.mouse.click(*bar_point(row_by_id(state(desk), str(replica["id"]))))
    desk.wait_for(f"() => document.getElementById('discord-log').getAttribute('data-focus-thread') === '{REPLICA}'", "no selection")
    page.click(f"#discord-log > li[data-id='{replica['id']}'] .thread-badge")
    desk.wait_for("() => !document.getElementById('thread-heading').hidden"
                  f" && document.getElementById('thread-card').getAttribute('data-thread') === '{REPLICA}'",
                  "Thread(N) did not open the thread")
    desk.check(not page.evaluate("() => document.getElementById('discord-log').hasAttribute('data-focus-thread')"),
               "the thread view is dimmed by All's selection")
    page.click("#thread-back")
    desk.wait_for("() => document.getElementById('thread-select').value === 'flat'"
                  f" && document.getElementById('discord-log').getAttribute('data-focus-thread') === '{REPLICA}'",
                  "Back from the thread did not find All with the thread still selected")
    page.keyboard.press("Escape")
    desk.wait_for("() => !document.getElementById('discord-log').hasAttribute('data-focus-thread')", "Esc did not let go")
    desk.notes.append("a live reply of the thread arrived at full strength and a live message of no thread dimmed; a second "
                      "click, Esc, another strip and t each let go or moved it; Thread(N) and Back kept it")

    # The first reply to the thread nobody had answered, live: its root gets a strip, its words unmoved.
    page.evaluate(f"() => document.querySelector(\"#discord-log > li[data-id='{LONE_ROOT}']\").scrollIntoView({{block: 'center'}})")
    desk.settled()
    lone = row_by_id(state(desk), LONE_ROOT)
    desk.check(lone["strip"] is None, "the root nobody has answered has a strip")
    first = api.add("The first answer to the brand-new question.", LONE)
    api.push(first)
    desk.wait_for(f"() => !!document.querySelector(\"#discord-log > li[data-id='{LONE_ROOT}'] > .thread-strip:not([hidden])\")",
                  "the first reply gave its root no strip")
    desk.settled()
    now = row_by_id(state(desk), LONE_ROOT)
    desk.check(abs(number(part(now, "meta"), "left") - number(part(lone, "meta"), "left")) < 0.5,
               f"the strip moved its row's words: {lone['meta']} -> {now['meta']}")
    desk.shot("c2-first-reply")
    desk.notes.append("the first reply to a thread nobody had answered gave its root a strip without moving its words")


def phone_walk(desk: Desk, api: StripApi, context: BrowserContext) -> None:
    page = desk.page
    found = state(desk)
    desk.notes.append(check_strips(desk, api, found, touch=True))
    desk.shot("c0-all")

    # A tap on the summarised thread's strip: selected, still All.
    root = row_by_id(found, PIVOT)
    x, y = bar_point(root)
    page.touchscreen.tap(x + 6, y)
    desk.wait_for(f"() => document.getElementById('discord-log').getAttribute('data-focus-thread') === '{RUNNER}'",
                  "a tap on the strip did not select its thread")
    desk.settled()
    selected = state(desk)
    check_selected(desk, api, selected, RUNNER, "a tap on its strip")
    desk.shot("c1-selected")

    # A finger drawn sideways from a strip: nothing archived, no details.
    reply = next(row for row in rows_of(selected) if row["strip"] and in_view(desk, row) and row["id"] not in READ_REPLIES
                 and row["id"] != PIVOT)
    sx, sy = bar_point(reply)
    posts = sum(1 for request in api.requests if request.startswith("POST"))
    cdp = context.new_cdp_session(page)
    cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": sx + 6, "y": sy}]})
    for step in range(1, 13):
        page.wait_for_timeout(16)
        cdp.send("Input.dispatchTouchEvent", {"type": "touchMove", "touchPoints": [{"x": sx + 6 + 14 * step, "y": sy}]})
    page.wait_for_timeout(16)
    cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
    page.wait_for_timeout(700)
    row = f"#discord-log > li[data-id='{reply['id']}']"
    after = page.evaluate(f"""() => {{ const li = document.querySelector("{row}");
        return {{archived: li.getAttribute('data-archived'), details: !!li.querySelector('.msg-details'), transform: li.style.transform}}; }}""")
    desk.check(after["archived"] != "true" and not after["details"] and not after["transform"],
               f"a finger drawn sideways from the strip did something to the row: {after}")
    desk.check(sum(1 for request in api.requests if request.startswith("POST")) == posts, "a swipe from the strip posted something")
    desk.check(number(state(desk), "overflow") <= 0, "the page scrolls sideways")
    desk.notes.append("a tap on a strip selected its thread, the rest at 0.3 with grey strips; a finger drawn sideways "
                      "from a strip archived nothing and opened nothing")


if __name__ == "__main__":
    raise SystemExit(main())
