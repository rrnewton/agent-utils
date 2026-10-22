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

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, FakeApi, Json, check, handler_for, message

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

    def no_overflow(self) -> None:
        over = float(self.page.evaluate(OVERFLOW_JS))
        self.check(over <= 1, f"the page is {over:.0f}px wider than the window")


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
    walk.settled()
    walk.check(walk.shown() == ["201", "204", "209", "214"], f"lantern shows {walk.shown()}")
    seams = walk.seams()
    count_only = lambda words: words.split(" · ")[0] + " …" if words and " · " in words else words
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
    walk.settled()
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
    walk.settled()
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
            server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
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
