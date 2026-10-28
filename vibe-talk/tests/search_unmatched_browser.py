#!/usr/bin/env python3
"""Open the runs a search leaves out, in real Chromium, at a 393x852 phone and a 1280x800 desk.

`#439 search-unmatched-separators`. The owner loves how instantly the search filters, and then
forgets it is on: drilled into a thread he is looking at a subset with no trace of the rest.
Where the search leaves messages out, a line already said how many (`#249 search-highlight`);
this issue makes that line a control like a collapsed read run's — a tap shows its run where it
stands — and stands the same line among the thread cards, counting the messages in the whole
threads it skips. The fake-DOM suite (tests/js/voice_page.test.mjs) pins the rules; this presses
the lines for real, with a finger on the phone and a mouse on the desk, and measures that each
line stands in the room its row keeps for it, touching neither neighbour.

It serves the checked-out page and the small fake API of tests/offline_cache_browser.py from one
loopback origin, over a channel of sixteen messages: three threads (the lantern thread, four
messages counted exactly; the harbor thread, three; the quarry thread, a root and a reply held
here under a count that is not exact) among five messages of the channel itself. At each size:

  sign in and open the channel in All -> the search glass, "lantern": the thread's four messages
  and a line in each gap, counting the messages between them and the ones at either end -> a real
  press on the middle line: its four messages stand where they were, its line gone, the other
  lines unmoved, and the bar still counting matches, "4 of 16 loaded" -> into the lantern thread
  by its root's Thread tag, the search kept: the field and its count still on screen over the
  thread -> the words changed to "bright": one message of the thread, and a line above it
  counting the thread's other three -> back to All, then the thread list, and the words changed
  to "harbor": one card, a line above it counting the quarry thread's messages as the least this
  page holds of it ("at least 2 unmatched messages in 1 thread"), one below it counting the
  lantern thread's four exactly -> a real press on the line below: the lantern card stands where
  it was. No horizontal overflow and no page error at any step.

`#605`-`#608 server-side-search` add a fifth stage over the same channel, on a server that keeps a
copy and answers its search: in All, "lantern" -> the line saying what was searched ("No upstream
search · this page: back to … · server: back to … (still expanding) · … messages searched") and the
server's one older hit drawn alone with its Show in channel button, a thumb tall, nothing wider than
the window -> All channels ticked: the server asked over every channel, its hit from another channel
named by it -> back to one channel, Main, "bright": the lantern root, which stands for its thread,
alone -> the Threads view unpacked, "bright": the lantern card alone, its block showing only the
reply that says it, a line below the card for the two before it -> a real press on that line: the
two stand where they were -> Search further back pressed: the server asked to gather the channel
next, and the line says how far it has got -> pressed again: Stop.

Pass --web-root DIR to run it against another copy of the page, and --screenshots DIR to keep a
PNG of each stage.
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
from urllib.parse import parse_qs, unquote, urlsplit

import offline_cache_browser as fixture
from offline_cache_browser import CHANNEL, TOKEN, FakeApi, Json, check, handler_for, message

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page, Playwright

# The phone the issue names and a desk: (label, width, height, touch).
SIZES = (("phone", 393, 852, True), ("desk", 1280, 800, False))

LANTERN = "spaces/A/threads/lantern"
HARBOR = "spaces/A/threads/harbor"
QUARRY = "spaces/A/threads/quarry"

# (thread or None, is_root, words), in the order the channel was posted. "lantern" is in the
# lantern thread's four messages and nowhere else; "bright" only in its last.
SCRIPT: tuple[tuple[str | None, bool, str], ...] = (
    (None, False, "Morning notes on the release lane."),
    (LANTERN, True, "Lantern thread root: the lantern battery question."),
    (HARBOR, True, "Harbor thread root: the harbor crane schedule."),
    (None, False, "The cache shard finished."),
    (LANTERN, False, "Lantern reply one: still dim after an hour."),
    (QUARRY, True, "Quarry thread root: the quarry truck rota."),
    (None, False, "Packaging job queued behind the shard."),
    (HARBOR, False, "Harbor reply one: crane booked for Thursday."),
    (None, False, "Dashboard refreshed with the new partitions."),
    (LANTERN, False, "Lantern reply two: swapped the cells over."),
    (QUARRY, False, "Quarry reply one: trucks at dawn, as rota'd."),
    (None, False, "The shard cache is warm again."),
    (HARBOR, False, "Harbor reply two: tide table says slack at nine."),
    (None, False, "Release notes drafted for review."),
    (LANTERN, False, "Lantern reply three: bright again, closing this out."),
    (None, False, "Last word on the lane for today."),
)
REPLIES = {LANTERN: 3, HARBOR: 2, QUARRY: 1}
ROOT_INDEX = {LANTERN: 1, HARBOR: 2, QUARRY: 5}
LAST_INDEX = {LANTERN: 14, HARBOR: 12, QUARRY: 10}


class GapApi(FakeApi):
    """The channel of the walk: three threads among the channel's own messages."""

    def __init__(self) -> None:
        super().__init__()
        self.messages = []
        for index, (thread_id, is_root, words) in enumerate(SCRIPT):
            thread = None
            if thread_id is not None:
                thread = {"id": thread_id, "root_message_id": str(200 + ROOT_INDEX[thread_id]),
                          "is_root": is_root, "reply_count": REPLIES[thread_id],
                          "reply_count_exact": thread_id != QUARRY}
            self.messages.append(message(index, words, thread))
        self.threads = [{
            "id": thread_id, "root": self.messages[ROOT_INDEX[thread_id]],
            "title": f"The {thread_id.rsplit('/', 1)[1]} thread",
            "reply_count": REPLIES[thread_id], "reply_count_exact": thread_id != QUARRY,
            "updated_at": self.messages[LAST_INDEX[thread_id]]["timestamp"],
        } for thread_id in (LANTERN, HARBOR, QUARRY)]
        # `#605 server-side-search`: an older lantern note the server holds and the page has not
        # loaded, and one in another channel for All channels.
        self.older = {**message(-90, "An old lantern note from the spring."), "id": "150"}
        self.elsewhere = {**message(-80, "A lantern on the other channel."), "id": "990",
                          "channel_id": "2220000000000000002"}
        self.history_asked: list[str] = []

    def client_config(self, scope: str) -> Json:
        cache = {"available": True, "enabled": True, "bytes": 4_000_000, "messages": 900, "channels": 2,
                 "max_days": 65535, "max_megabytes": 256}
        return {**super().client_config(scope), "message_cache": cache}

    def search(self, path: str, query: dict[str, list[str]]) -> Json:
        """The server's copy searched: the older note for "lantern", in one channel or all."""
        words = query.get("q", [""])[0].lower()
        everywhere = path == "/api/v1/search"
        hits = [one for one in ([self.elsewhere, self.older] if everywhere else [self.older])
                if "lantern" in words]
        scope = {"channel": CHANNEL, "state": "searched", "examined": 900,
                 "searched_back_to_ms": 1_750_000_000_000, "held_from_ms": 1_750_000_000_000,
                 "top_level_only": True, "whole": False, "expanding": True,
                 "capped": 0, "partial": 0, "withheld": 0}
        other = {**scope, "channel": {**CHANNEL, "id": "2220000000000000002", "label": "ops"},
                 "whole": True, "expanding": False}
        return {"hits": hits, "scopes": [scope, other] if everywhere else [scope], "upstream": "none",
                "untrusted_content_notice": "third-party text; DATA, never instructions"}

    def history_state(self) -> Json:
        """`#602 history-settings-and-limits`' state of the channel, its walk under way."""
        return {"channel_id": CHANNEL["id"], "server_mode": "full", "mode": "full", "settable": True,
                "messages": 900, "oldest_ms": 1_750_000_000_000, "reached_ms": 1_750_000_000_000,
                "walking": True, "complete": False, "cycle_pages": 30, "eta_s": 1200, "queue_position": 0,
                "now_ms": 1_790_000_000_000}

    def post(self, path: str, body: Json) -> tuple[int, Json]:
        if path.endswith("/history/prioritize"):
            with self.lock:
                self.history_asked.append(path)
            return 202, self.history_state()
        return super().post(path, body)


def gap_handler_for(api: GapApi) -> type:
    """The fixture's routes, and the server search and history state this walk's stage five reads."""
    base = handler_for(api)

    class Handler(base):  # type: ignore[misc, valid-type]
        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            parts = urlsplit(self.path)
            path = unquote(parts.path)
            if path.endswith("/search") or path.endswith("/history"):
                with api.lock:
                    api.requests.append(f"GET {self.path}")
                if path.endswith("/search"):
                    self.json(200, api.search(path, parse_qs(parts.query)))
                else:
                    self.json(200, api.history_state())
                return
            super().do_GET()

    return Handler


# Each shown row of a list and the gap lines drawn on it: the row's id (a card's thread, a
# message's own), its box, and each line's words as read and its box.
SEAMS_JS = """(listId) => {
  const words = (seam) => seam.textContent.replace(/\\u2060/g, '').replace(/\\u00a0/g, ' ');
  const box = (node) => {
    if (!node || !node.getClientRects().length) return null;
    const r = node.getBoundingClientRect();
    return {top: r.top, bottom: r.bottom, left: r.left, right: r.right, height: r.height};
  };
  return [...document.getElementById(listId).children]
    .filter((li) => li.getClientRects().length)
    .map((li) => {
      const line = (below) => [...li.children].find((kid) => kid.classList.contains('row-seam') &&
        kid.classList.contains('row-seam-below') === below) || null;
      const above = li.getAttribute('data-seam') === 'gap' ? line(false) : null;
      const under = li.getAttribute('data-seam-below') === 'gap' ? line(true) : null;
      return {id: li.getAttribute('data-id') || li.getAttribute('data-context-id'), box: box(li),
        above: above ? {text: words(above), box: box(above), tag: above.tagName.toLowerCase()} : null,
        below: under ? {text: words(under), box: box(under), tag: under.tagName.toLowerCase()} : null};
    });
}"""

SHOWN_JS = """(listId) => [...document.getElementById(listId).children]
  .filter((li) => li.getClientRects().length)
  .map((li) => li.getAttribute('data-id') || li.getAttribute('data-context-id'))"""

PICK_JS = """(value) => {
  const picker = document.getElementById('thread-select');
  picker.value = value;
  picker.dispatchEvent(new Event('change', {bubbles: true}));
}"""

OVERFLOW_JS = "() => document.documentElement.scrollWidth - window.innerWidth"


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
        help="directory to write one PNG per stage into (default: none are kept)",
    )
    return parser.parse_args()


class Walk:
    """One reader at one size."""

    def __init__(self, page: Page, label: str, touch: bool, shots: Path | None) -> None:
        self.page = page
        self.label = label
        self.touch = touch
        self.shots = shots
        self.notes: list[str] = []

    def check(self, condition: bool, why: str) -> None:
        check(condition, f"{self.label}: {why}")

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.label}-search-unmatched-{name}.png"))

    def settled(self) -> None:
        self.page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")

    def until(self, script: str, why: str, seconds: float = 10) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and not self.page.evaluate(script):
            self.page.wait_for_timeout(25)
        self.check(bool(self.page.evaluate(script)), why)

    def press(self, selector: str) -> None:
        self.page.locator(selector).scroll_into_view_if_needed()
        box = self.page.locator(selector).bounding_box()
        self.check(box is not None, f"{selector} is not on the screen")
        if box is None:
            return
        x, y = box["x"] + box["width"] / 2, box["y"] + box["height"] / 2
        if self.touch:
            self.page.touchscreen.tap(x, y)
        else:
            self.page.mouse.click(x, y)
        self.settled()

    def seams(self, list_id: str = "discord-log") -> list[tuple[str, str | None, str | None]]:
        found = self.page.evaluate(SEAMS_JS, list_id)
        rows = []
        for row in found if isinstance(found, list) else []:
            if not isinstance(row, dict):
                continue
            above = row.get("above")
            below = row.get("below")
            rows.append((str(row.get("id")),
                         str(above["text"]) if isinstance(above, dict) else None,
                         str(below["text"]) if isinstance(below, dict) else None))
        return [(row_id, above, below) for row_id, above, below in rows
                if above is not None or below is not None]

    def shown(self, list_id: str = "discord-log") -> list[str]:
        found = self.page.evaluate(SHOWN_JS, list_id)
        return [str(one) for one in found] if isinstance(found, list) else []

    def lines_between(self, list_id: str = "discord-log") -> None:
        """Every gap line stands between its neighbours, touching neither, a thumb tall."""
        found = self.page.evaluate(SEAMS_JS, list_id)
        rows = [row for row in found if isinstance(row, dict)] if isinstance(found, list) else []
        for at, row in enumerate(rows):
            for kind in ("above", "below"):
                line = row.get(kind)
                if not isinstance(line, dict):
                    continue
                self.check(line.get("tag") == "button", f"the line {kind} {row.get('id')} is not a button")
                box = line.get("box")
                own = row.get("box")
                if not isinstance(box, dict) or not isinstance(own, dict):
                    continue
                self.check(float(box["height"]) >= 30, f"the line {kind} {row.get('id')} is {box['height']:.0f}px tall")
                if kind == "above":
                    self.check(float(box["bottom"]) <= float(own["top"]) + 1.5,
                               f"the line above {row.get('id')} runs into its row")
                    if at > 0 and isinstance(rows[at - 1].get("box"), dict):
                        self.check(float(box["top"]) >= float(rows[at - 1]["box"]["bottom"]) - 1.5,
                                   f"the line above {row.get('id')} runs into the row before it")
                else:
                    self.check(float(box["top"]) >= float(own["bottom"]) - 1.5,
                               f"the line below {row.get('id')} runs into its row")

    def server_settled(self) -> None:
        """`#605 server-side-search`: the server's answer has landed, so the section above the list
        holds still before anything on the list is measured or pressed."""
        self.until("() => { const s = document.getElementById('server-search'); "
                   "return s.hidden || !document.getElementById('search-scope').textContent.includes('searching'); }",
                   "the server's search did not settle")
        self.settled()

    def no_overflow(self) -> None:
        over = float(self.page.evaluate(OVERFLOW_JS))
        self.check(over <= 1, f"the page is {over:.0f}px wider than the window")


def count_only(words: str | None) -> str | None:
    """A gap line's words without the days it reaches: "… 3 unmatched messages …"."""
    return words.split(" · ")[0] + " …" if words and " · " in words else words


def walk_through(walk: Walk) -> None:
    page = walk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    walk.until(f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {len(SCRIPT)}",
               "the channel did not open with its sixteen messages")
    walk.settled()

    # 1. "lantern" in All: its thread's four messages, a counted line in every gap.
    walk.press("#search-toggle")
    page.fill("#search-field", "lantern")
    walk.until("() => document.getElementById('search-count').textContent === '4 of 16 loaded'",
               "the count does not speak for the lantern search")
    walk.server_settled()
    walk.check(walk.shown() == ["201", "204", "209", "214"], f"lantern shows {walk.shown()}")
    seams = walk.seams()
    words_only = [(row_id, count_only(above), count_only(below)) for row_id, above, below in seams]
    walk.check(words_only == [
        ("201", "… 1 unmatched message …", None),
        ("204", "… 2 unmatched messages …", None),
        ("209", "… 4 unmatched messages …", None),
        ("214", "… 4 unmatched messages …", "… 1 unmatched message …"),
    ], f"the gaps say {seams}")
    walk.lines_between()
    walk.no_overflow()
    walk.shot("1-search")

    # 2. A real press on the middle line shows its run where it stands; the count is unmoved.
    walk.press("#discord-log > li[data-id='209'] > button.row-seam:not(.row-seam-below)")
    walk.check(walk.shown() == ["201", "204", "205", "206", "207", "208", "209", "214"],
               f"the opened run shows {walk.shown()}")
    walk.check(page.evaluate("() => document.getElementById('search-count').textContent") == "4 of 16 loaded",
               "opening a run changed what the bar counts as matched")
    rest = [(row_id, above, below) for row_id, above, below in walk.seams() if row_id != "209"]
    walk.check(len(rest) == 3 and all(row_id != "209" for row_id, _, _ in walk.seams()),
               f"another gap moved, or the opened one stayed: {walk.seams()}")
    walk.lines_between()
    walk.shot("2-opened")
    walk.notes.append("lantern 4 of 16, the run above 209 opened by its line")

    # 3. Into the lantern thread under the search: the bar stays, and the thread is searched.
    walk.press("#discord-log > li[data-id='201'] .thread-badge")
    walk.until("() => !document.getElementById('thread-heading').hidden", "the thread did not open")
    walk.check(not page.evaluate("() => document.getElementById('search-field').hidden"),
               "the search field left the screen inside the thread")
    walk.check(page.evaluate("() => document.getElementById('search-field').value") == "lantern",
               "the words were not kept into the thread")
    walk.check(page.evaluate("() => document.getElementById('search-count').textContent") == "4 of 4 loaded",
               "the count does not speak for the thread")
    page.fill("#search-field", "bright")
    walk.until("() => document.getElementById('search-count').textContent === '1 of 4 loaded'",
               "bright was not counted in the thread")
    walk.server_settled()
    in_thread = [(row_id, count_only(above), below) for row_id, above, below in walk.seams()]
    walk.check(in_thread == [("214", "… 3 unmatched messages …", None)],
               f"in the thread the gaps say {walk.seams()}")
    walk.lines_between()
    walk.no_overflow()
    walk.shot("3-thread")
    walk.notes.append("in the thread under 'bright', 1 of 4 with 3 said above it")

    # 4. The thread list's cards: whole threads counted, exactly or as at least, and opened.
    page.evaluate(PICK_JS, "flat")
    walk.until("() => document.getElementById('thread-heading').hidden", "All did not come back")
    page.fill("#search-field", "harbor")
    walk.server_settled()
    page.evaluate(PICK_JS, "threads")
    walk.until("() => document.getElementById('side-cards').children.length > 0",
               "the thread list drew no cards")
    walk.settled()
    walk.check(walk.shown("side-cards") == [HARBOR], f"harbor keeps {walk.shown('side-cards')}")
    card_seams = walk.seams("side-cards")
    walk.check(card_seams == [(HARBOR, "… at least 2 unmatched messages in 1 thread …",
                                "… 4 unmatched messages in 1 thread …")],
               f"among the cards the gaps say {card_seams}")
    walk.lines_between("side-cards")
    walk.no_overflow()
    walk.shot("4-cards")
    walk.press("#side-cards > li[data-id='" + HARBOR + "'] > button.row-seam.row-seam-below")
    walk.check(walk.shown("side-cards") == [HARBOR, LANTERN],
               f"the opened card run shows {walk.shown('side-cards')}")
    walk.check(walk.seams("side-cards") == [(HARBOR, "… at least 2 unmatched messages in 1 thread …", None)],
               f"the other card gap moved: {walk.seams('side-cards')}")
    walk.shot("5-cards-opened")
    walk.notes.append("cards: quarry at least 2 in 1 thread above, lantern 4 in 1 below, opened")


SCOPE_JS = "() => document.getElementById('search-scope').textContent"
HITS_JS = """() => [...document.querySelectorAll('#server-hits > li')]
  .filter((li) => li.getClientRects().length).map((li) => li.getAttribute('data-id'))"""
BOX_JS = """(selector) => {
  const node = document.querySelector(selector);
  if (!node || !node.getClientRects().length) return null;
  const r = node.getBoundingClientRect();
  return {top: r.top, bottom: r.bottom, left: r.left, right: r.right, height: r.height};
}"""


def walk_server_search(walk: Walk, api: GapApi) -> None:
    """`#605`-`#608 server-side-search`: the server's copy searched, at this size."""
    page = walk.page
    page.evaluate(PICK_JS, "flat")
    walk.until("() => document.getElementById('discord-log').children.length > 0", "All did not come back")
    page.fill("#search-field", "lantern")
    walk.until("() => document.querySelectorAll('#server-hits > li').length === 1",
               "the server's older hit was not drawn")
    walk.settled()
    scope = str(page.evaluate(SCOPE_JS))
    walk.check(scope.startswith("No upstream search · this page: back to ") and
               " · server: back to " in scope and "(still expanding)" in scope and
               scope.endswith("messages searched"), f"the line says {scope!r}")
    walk.check(page.evaluate(HITS_JS) == ["150"], f"the hits are {page.evaluate(HITS_JS)}")
    show = page.evaluate(BOX_JS, "#server-hits > li[data-id='150'] .server-hit-show")
    walk.check(isinstance(show, dict) and float(show["height"]) >= 30,
               f"Show in channel is not a thumb tall: {show}")
    line = page.evaluate(BOX_JS, "#search-scope")
    width = float(page.evaluate("() => window.innerWidth"))
    walk.check(isinstance(line, dict) and float(line["right"]) <= width + 1, f"the line runs off the screen: {line}")
    walk.no_overflow()
    walk.shot("6-server")
    walk.notes.append(f"server: {scope}")

    # All channels: every channel searched, the hit elsewhere named by its channel.
    walk.press("#search-all-channels")
    walk.until("() => document.querySelectorAll('#server-hits > li').length === 2",
               "All channels did not bring the other channel's hit")
    walk.check(any("GET /api/v1/search?" in one for one in api.requests), "All channels did not ask every channel")
    walk.check(" · server: 2 of 2 channels" in str(page.evaluate(SCOPE_JS)),
               f"the line says {page.evaluate(SCOPE_JS)!r}")
    walk.no_overflow()
    walk.shot("7-all-channels")
    walk.press("#search-all-channels")
    walk.until("() => document.querySelectorAll('#server-hits > li').length === 1", "one channel did not come back")

    # Main: a thread stands for its children, so the reply's word keeps the lantern root.
    page.evaluate(PICK_JS, "main")
    walk.settled()
    page.fill("#search-field", "bright")
    walk.until("() => document.getElementById('search-count').textContent !== ''", "Main did not count the search")
    walk.settled()
    walk.check(walk.shown() == ["201"], f"Main under 'bright' shows {walk.shown()}")
    walk.notes.append("Main keeps the lantern root for its reply's 'bright'")

    # Threads, unpacked: the lantern card alone, its block only the reply that says it.
    page.evaluate(PICK_JS, "threads")
    walk.until("() => document.getElementById('side-cards').children.length > 0", "the thread list drew no cards")
    if page.evaluate("() => !document.getElementById('side-unpack').hidden && "
                     "document.getElementById('side-unpack').getAttribute('aria-pressed') !== 'true'"):
        walk.press("#side-unpack")
    walk.until("() => document.querySelectorAll('#side-cards > li[data-unpack-thread]').length > 0",
               "the threads did not unpack")
    walk.settled()
    shown = walk.shown("side-cards")
    walk.check(shown[0] == LANTERN and "214" in shown and "204" not in shown and "209" not in shown,
               f"the unpacked lantern thread under 'bright' shows {shown}")
    seams = walk.seams("side-cards")
    walk.check(any(r == LANTERN and count_only(b) == "… 2 unmatched messages …" for r, _, b in seams),
               f"the block's line below its card says {seams}")
    walk.lines_between("side-cards")
    walk.no_overflow()
    walk.shot("8-unpacked")
    walk.press("#side-cards > li[data-id='" + LANTERN + "'] > button.row-seam.row-seam-below")
    opened = walk.shown("side-cards")
    walk.check("204" in opened and "209" in opened, f"the opened run shows {opened}")
    walk.notes.append("Threads unpacked: only the hit, a line for the two before it, opened")

    # Search further back: the server asked to gather this channel next, and the line says so.
    page.evaluate(PICK_JS, "flat")
    page.fill("#search-field", "lantern")
    walk.until("() => !document.getElementById('search-further').hidden", "Search further back was not offered")
    walk.press("#search-further")
    walk.until("() => document.getElementById('search-scope').textContent.includes('gathering older history')",
               "the line did not say the server is gathering")
    walk.check(len(api.history_asked) == 1, f"the bump was asked {len(api.history_asked)} times")
    walk.check(str(page.evaluate("() => document.getElementById('search-further').textContent")) ==
               "Stop gathering older history", "the button did not become Stop")
    walk.shot("9-further")
    walk.press("#search-further")
    walk.until("() => document.getElementById('search-further').textContent === 'Search further back'",
               "Stop did not end the watch")
    walk.no_overflow()
    walk.notes.append("Search further back asked once, then stopped")


def launch(playwright: Playwright, args: argparse.Namespace, profile: str, width: int, height: int,
           touch: bool) -> BrowserContext:
    if touch:
        return playwright.chromium.launch_persistent_context(
            profile, headless=True, executable_path=args.browser_executable,
            viewport={"width": width, "height": height}, color_scheme="dark",
            device_scale_factor=2.625, is_mobile=True, has_touch=True,
        )
    return playwright.chromium.launch_persistent_context(
        profile, headless=True, executable_path=args.browser_executable,
        viewport={"width": width, "height": height}, color_scheme="dark",
    )


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = args.web_root.resolve()
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch in SIZES:
            api = GapApi()
            server = ThreadingHTTPServer(("127.0.0.1", 0), gap_handler_for(api))
            server.daemon_threads = True
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-unmatched-") as profile:
                    context = launch(playwright, args, profile, width, height, touch)
                    version = context.browser.version if context.browser else "Chromium"
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
                    walk = Walk(page, label, touch, args.screenshots)
                    walk_through(walk)
                    walk_server_search(walk, api)
                    check(not errors, f"{label}: the page threw: {errors}")
                    context.close()
                    report.append(f"{version} {label} {width}x{height}: " + "; ".join(walk.notes))
            finally:
                api.stopping.set()
                server.shutdown()
                server.server_close()
                thread.join()
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
