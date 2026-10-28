#!/usr/bin/env python3
"""`#528 thread-card-markdown-and-cap` in real Chromium at 393x852 and 1280x800.

The owner, 2026-10-26: "a long message with markdown formatting loses its formatting when it is
promoted to a thread card ... it was pushed down into a child message and it still was missing its
formatting. But then asynchronously updated several seconds later." And after a restart: "it had not
yet loaded the summary so it displayed the first message as split in the second half of that split
displayed without markdown formatting. I wanted to split only at the line granularity and always
retain markdown formatting of course. And also I don't understand why we would fail to load the
summaries just as fast as the other messages."

Against a fake API whose one thread is opened by a long Markdown message, rendered as the server
renders it, with one reply, as on Slack:

* format: with no summariser, the card is the message's own formatted body — paragraphs, a numbered
  list, code — cut between lines at no more than K = 5 lines and ending "…"; unpacked, the first row
  is the rest, formatted, its list numbered on from where the card stopped (`start`); the card's link
  is a link's words, the row's a link; no sideways scroll.
* pending: with a summariser and the reply just written, the card waits in the summary's layout — its
  first lines on the card, the message inward whole as its first child — and when the server files
  the name, the Threads list is read again within seconds and only the card's words change: the card
  and the row under it move by no more than a pixel.
* load: with the name filed and every read taking `--latency` seconds, a fresh load draws the summary
  card with the same read that draws the thread's card (the root's own record names it), and a reload
  whose reads are held draws the summary card from the device's copy before any read answers.
* table (`#533 card-split-atomic-blocks`): a nightly report opening with a 7x6 table, as the server renders it.
  From the first frame the card is drawn it says "Table (7 columns × 6 rows) …" on one line, split and never
  clamped, no cell of the table on it; unpacked, the continuation draws the table whole, seven cells to a row.
* build (`#529 page-build-stale-notice`): a read saying the server serves the page's own script shows
  nothing; one saying another shows "New version — reload" on the line over the list, inside the
  window and clear of the freshness pill, and a press reloads the page. The page's own build is the
  page build's script hash, read from the hashed name its shell gives it.
* old-shell (`#624 voice-page-modularity`): a shell from before the page build (two plain script tags,
  `/voice.js?v=<that build's hash>`, as a back/forward or restored tab can still hold) against
  today's files: the whole page build at /voice.js and the standalone validators at /contract.js. The
  page starts and signs in without an error, knows the old build it was named by, and offers
  "New version — reload" as soon as a read says the server serves the page build.

Pass --screenshots DIR to keep a PNG of each state, and --web-root DIR to run against another copy
of the page. --report-only measures the load part without failing on it, for an older page's numbers.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import tempfile
import threading
import time
from datetime import datetime, timedelta, timezone
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import FakeApi, Json, check, handler_for, message, thread_of
from two_column_browser import Desk
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

SIZES = (("phone-393", 393, 852, True), ("desk", 1280, 800, False))

# K until the owner says otherwise (web/voice.js `DEFAULT_CARD_MAX_LINES`).
K = 5

SNAPSHOT = "spaces/A/threads/snapshot"

# A long first message as an agent writes one, and as src/render.rs renders it.
MD_ITEMS = [
    "Land change 101 [YES: CI green]",
    "Land change 102 (fetch cache) and change 103 (scratch path) as accepted [YES: the newer drafts only "
    "changed the description]",
    "Ask for re-accepts of change 104 (problem filters) and change 105 (fixed volume id) [YES: both were "
    "accepted on older versions]",
    "Change 106, reproducible guest kernel [ask for a re-accept of the draft: it replaces a fixed 1970 date "
    "with the source epoch]",
    "Publish the 15 ready offline drafts [YES; HOLD change 107]",
    "Accept change 108, which unblocks 14 green fixup diffs",
]
MD_ROOT = "\n".join(["MORNING SNAPSHOT, Mon 10-26:", "", "DECISIONS FOR YOU (recommendation in brackets):", "",
                     *[f"{at + 1}. {item}" for at, item in enumerate(MD_ITEMS)], "",
                     "Details: `docs/plan.md` and https://example.test/plan"])
MD_ROOT_HTML = ("<p>MORNING SNAPSHOT, Mon 10-26:</p>\n<p>DECISIONS FOR YOU (recommendation in brackets):</p>\n<ol>\n"
                + "".join(f"<li>{item}</li>\n" for item in MD_ITEMS)
                + "</ol>\n<p>Details: <code>docs/plan.md</code> and "
                "<a href=\"https://example.test/plan\" rel=\"noopener noreferrer\" target=\"_blank\">"
                "https://example.test/plan</a></p>\n")
NAMED: Json = {"display_name": "morning-snapshot-decisions",
         "summary": "The owner is working through the morning's landing decisions, one change at a time."}


class MarkdownApi(FakeApi):
    """One thread opened by `MD_ROOT` with one reply, and Main's announcement. `recent` puts the reply
    a second ago; `names` says the server names threads; `latency` holds every timeline read that long."""

    def __init__(self, *, recent: bool = False, names: bool = False, latency: float = 0.0,
                 text: str = MD_ROOT, html: str = MD_ROOT_HTML) -> None:
        super().__init__()
        record = {"id": SNAPSHOT, "root_message_id": "201", "is_root": True, "reply_count": 1,
                  "reply_count_exact": True}
        root = {**message(1, text, record), "content_html": html}
        reply = message(2, "Which of these should land first?", {**record, "is_root": False})
        if recent:
            now = datetime.now(timezone.utc)
            root["timestamp"] = (now - timedelta(seconds=90)).isoformat().replace("+00:00", "Z")
            reply["timestamp"] = (now - timedelta(seconds=1)).isoformat().replace("+00:00", "Z")
        self.messages = [message(0, "main channel announcement"), root, reply]
        self.threads = [{"id": SNAPSHOT, "root": root, "title": "Thread", "reply_count": 1,
                         "reply_count_exact": True, "updated_at": reply["timestamp"]}]
        self.names = names
        self.latency = latency
        self.named: Json | None = None
        # `#529 page-build-stale-notice`: the page script every read says the server serves, if any.
        self.page_build: str | None = None

    def client_config(self, scope: str) -> Json:
        config = super().client_config(scope)
        return {**config, "thread_names_supported": True} if self.names else config

    def file_name(self, named: Json | None) -> None:
        """The summariser files the thread's name and sentence (None: takes them away)."""
        with self.lock:
            self.named = named

    def timeline(self, query: dict[str, list[str]]) -> Json:
        if self.latency:
            time.sleep(self.latency)
        answer = super().timeline(query)
        with self.lock:
            named = dict(self.named) if self.named else None
        with self.lock:
            build = self.page_build
        if build:
            answer["page_build"] = build
        if named:
            # As the server serves it: on the thread list's copy, and on the root's own record.
            threads = answer["threads"]
            rows = answer["messages"]
            assert isinstance(threads, list) and isinstance(rows, list)
            answer["threads"] = [{**thread, **named} for thread in threads]
            answer["messages"] = [
                {**row, "thread": {**thread_of(row), **named}} if thread_of(row).get("is_root") else row
                for row in rows]
        return answer


# The snapshot's card and the rows under it, measured.
CARD_JS = """(id) => {
    const card = document.querySelector(`#side-cards > li[data-card][data-id='${id}']`);
    if (!card) return null;
    const text = card.querySelector('.side-card-text');
    const style = getComputedStyle(text);
    const rows = [...document.querySelectorAll(`#side-cards > li[data-unpack-thread='${id}']`)];
    const first = rows[0] || null;
    const box = (node) => { const r = node.getBoundingClientRect(); return {top: r.top, bottom: r.bottom, height: r.height}; };
    const md = first ? first.querySelector('.md') : null;
    const ol = md ? md.querySelector('ol') : null;
    return {
        box: box(card), state: card.getAttribute('data-card-state'),
        split: card.hasAttribute('data-split-card'), pending: card.hasAttribute('data-summary-pending'),
        summarised: card.hasAttribute('data-summarised-card'), words: text.innerText,
        tags: [...text.querySelectorAll('*')].map((node) => node.tagName.toLowerCase()),
        cardLinks: text.querySelectorAll('a').length, cardLinkWords: text.querySelectorAll('.card-link').length,
        lines: text.getBoundingClientRect().height / parseFloat(style.lineHeight),
        rows: rows.length,
        first: first ? {continuation: first.hasAttribute('data-root-continuation'), child: first.hasAttribute('data-root-child'),
            words: md ? md.innerText : '', start: ol ? ol.start : null, items: ol ? ol.children.length : 0,
            links: md ? md.querySelectorAll('a[href]').length : 0, box: box(first)} : null,
        overflow: document.documentElement.scrollWidth - window.innerWidth,
    };
}"""


def number(found: Json, *path: str) -> float:
    """The number at `path` in what a script measured."""
    at: object = found
    for key in path:
        assert isinstance(at, dict)
        at = at[key]
    return float(str(at))


def strings(value: object) -> list[str]:
    """A measured list, as text."""
    assert isinstance(value, list)
    return [str(one) for one in value]


def card(desk: Desk) -> Json:
    found = desk.page.evaluate(CARD_JS, SNAPSHOT)
    desk.check(isinstance(found, dict), "the snapshot has no card")
    assert isinstance(found, dict)
    desk.check(float(found["overflow"]) <= 0, f"the page scrolls sideways: {found['overflow']}")
    return found


def first_row(found: Json) -> Json:
    row = found["first"]
    assert isinstance(row, dict), f"nothing is unpacked under the card: {found}"
    return row


def sign_in(desk: Desk) -> None:
    desk.page.fill("#api-token", fixture.TOKEN)
    desk.page.click("#save-token")
    desk.page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    desk.page.click("#view-switch")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length >= 2",
                  "the channel did not open", seconds=20)


def to_threads(desk: Desk) -> None:
    desk.page.select_option("#thread-select", "threads")
    desk.wait_for(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{SNAPSHOT}']\")",
                  "the Threads view has no card for the snapshot", seconds=20)
    desk.settled()


def unpack_all(desk: Desk) -> None:
    for _ in range(3):
        if desk.page.get_attribute("#side-unpack", "aria-pressed") == "true":
            break
        desk.page.click("#side-unpack")
        desk.settled()
    desk.wait_for(f"() => document.querySelectorAll(\"#side-cards > li[data-unpack-thread='{SNAPSHOT}']\").length >= 2",
                  "the snapshot did not unpack under its card")
    desk.page.evaluate(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{SNAPSHOT}']\")"
                       ".scrollIntoView({block: 'start'})")
    desk.settled()


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
                        help="walk only at these sizes, repeatable (default: all of them)")
    parser.add_argument("--only", choices=("format", "pending", "load", "table", "build", "old-shell"), action="append",
                        help="run only these parts of the walk, repeatable (default: all of them)")
    parser.add_argument("--latency", type=float, default=0.6,
                        help="seconds every timeline read takes in the load part (default: 0.6)")
    parser.add_argument("--report-only", action="store_true",
                        help="measure the load part without failing on what it finds (default: fail)")
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
    parts = args.only or ["format", "pending", "load", "table", "build", "old-shell"]
    with sync_playwright() as playwright:
        for label, width, height, touch in SIZES:
            if args.size and label not in args.size:
                continue
            for part in parts:
                report.append(walk(playwright.chromium, args, part, label, width, height, touch))
    for line in report:
        print(line)
    return 0


def hashed_web_root(source: Path, into: Path) -> Path:
    """The page at `source` with its shell naming its script by its content hash, as the server's shell
    does (src/http/assets.rs). A page build's shell already does (`/assets/voice.<hash>.js`, `#624
    voice-page-modularity`); a directory of sources is copied and its `/voice.js` given a `?v=`."""
    if (source / "assets.json").is_file():
        return source
    shutil.copytree(source, into)
    script = (into / "voice.js").read_bytes()
    build = hashlib.sha256(script).hexdigest()[:32]
    shell = into / "voice.html"
    shell.write_text(shell.read_text().replace('src="/voice.js"', f'src="/voice.js?v={build}"'))
    return into


def old_shell_root(source: Path, into: Path) -> tuple[Path, str, str]:
    """A copy of the page build at `source` served with the shell from before the page build: the
    source shell, its script named `/voice.js?v=<hash>` as that server named it, with the hash of the
    script it served then. Returns the directory, that hash, and the page build's own."""
    if not (source / "assets.json").is_file():
        raise SystemExit(f"the old-shell part needs a page build to serve, and {source} is not one")
    shutil.copytree(source, into)
    script = page_sources.source_text(source, "voice.js").encode()
    build = hashlib.sha256(script).hexdigest()[:32]
    shell = page_sources.source_text(source, "voice.html")
    check(shell.count('src="/voice.js"') == 1, "the source shell no longer loads /voice.js by a plain tag")
    (into / "voice.html").write_text(shell.replace('src="/voice.js"', f'src="/voice.js?v={build}"'))
    return into, build, str(json.loads((source / "assets.json").read_text())["page_build"])


# While the old-shell part runs: the build an old shell names its script by, and the page build.
OLD_SHELL_BUILD = ""
PAGE_BUILD_SERVED = ""


def walk(chromium: BrowserType, args: argparse.Namespace, part: str, label: str, width: int, height: int,
         touch: bool) -> str:
    global OLD_SHELL_BUILD, PAGE_BUILD_SERVED
    if part not in ("build", "old-shell"):
        return walk_part(chromium, args, part, label, width, height, touch)
    served = fixture.WEB_ROOT
    with tempfile.TemporaryDirectory(prefix="vibe-talk-hashed-web-") as copy:
        if part == "old-shell":
            root, OLD_SHELL_BUILD, PAGE_BUILD_SERVED = old_shell_root(served, Path(copy) / "web")
            fixture.WEB_ROOT = root.resolve()
        else:
            fixture.WEB_ROOT = hashed_web_root(served, Path(copy) / "web").resolve()
        try:
            return walk_part(chromium, args, part, label, width, height, touch)
        finally:
            fixture.WEB_ROOT = served


def walk_part(chromium: BrowserType, args: argparse.Namespace, part: str, label: str, width: int, height: int,
              touch: bool) -> str:
    api = (MarkdownApi() if part == "format" else MarkdownApi(recent=True, names=True) if part == "pending"
           else MarkdownApi(text=TABLE_ROOT, html=TABLE_ROOT_HTML) if part == "table"
           else MarkdownApi(names=True, latency=args.latency))
    if part == "load":
        api.file_name(NAMED)
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-card-markdown-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark", has_touch=touch, is_mobile=touch)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"card-markdown-{label}-{part}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            sign_in(desk)
            if part == "format":
                format_walk(desk)
            elif part == "pending":
                pending_walk(desk, api)
            elif part == "build":
                build_walk(desk, api)
            elif part == "old-shell":
                old_shell_walk(desk, api)
            elif part == "table":
                table_walk(desk)
            else:
                load_walk(desk, api, args, server.server_port)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}, {part}: " + "; ".join(desk.notes)
    finally:
        api.timeline_gate.set()
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def format_walk(desk: Desk) -> None:
    to_threads(desk)
    unpack_all(desk)
    found = card(desk)
    desk.check(bool(found["split"]), f"the premise: the snapshot's card is split, found {found}")
    tags = strings(found["tags"])
    desk.check("ol" in tags and "li" in tags and "p" in tags, f"the card lost its formatting: {tags}")
    lines = number(found, "lines")
    # K lines of words, and the gaps a card draws between its blocks: under one more line in all.
    desk.check(lines <= K + 1, f"the card draws {lines:.2f} lines, more than K = {K}")
    words = str(found["words"])
    desk.check(words.rstrip().endswith("…"), f"the card's words do not end in …: {words!r}")
    desk.check(MD_ITEMS[0] in words and MD_ITEMS[1] not in words,
               f"the card does not stop after the first item: {words!r}")
    desk.check(found["cardLinks"] == 0, "the card holds a link inside its button")
    rest = first_row(found)
    desk.check(bool(rest["continuation"]), f"the first row is not the message's continuation: {rest}")
    desk.check(rest["start"] == 2 and rest["items"] == len(MD_ITEMS) - 1,
               f"the continuation's list is not numbered on from 2 with the other five items: {rest}")
    desk.check(str(rest["words"]).lstrip().startswith("…") and "docs/plan.md" in str(rest["words"]),
               f"the continuation is not the rest of the message from …: {rest['words']!r}")
    desk.check(number(rest, "links") == 1, "the continuation's link is not a link")
    desk.shot("1-split-formatted")
    desk.notes.append(f"the split card draws {lines:.2f} lines (K = {K}) as paragraphs and a list, cut after item 1; "
                      f"the continuation's list starts at {rest['start']} with {rest['items']} items")


# `#533 card-split-atomic-blocks`. The owner's nightly report's shape, a 7x6 table first, its words scrubbed.
TABLE_HEAD = ["Suite", "Runs", "Passed", "Failed", "Flaky", "Wall time", "Notes"]
TABLE_ROWS = [["unit", "412", "412", "0", "0", "6m", "clean"], ["integration", "88", "86", "1", "1", "41m", "retried"],
              ["browser", "57", "57", "0", "0", "19m", "clean"], ["packaging", "12", "11", "1", "0", "8m", "timeout"],
              ["soak", "3", "3", "0", "0", "3h", "flat"]]
TABLE_ROOT = "\n".join([f"| {' | '.join(TABLE_HEAD)} |", "|" + "---|" * 7, *[f"| {' | '.join(row)} |" for row in TABLE_ROWS],
                        "", "All green but packaging; the mirror timed out once."])
TABLE_ROOT_HTML = ("<table>\n<thead>\n<tr>\n" + "".join(f"<th>{cell}</th>\n" for cell in TABLE_HEAD) + "</tr>\n</thead>\n<tbody>\n"
                   + "".join("<tr>\n" + "".join(f"<td>{cell}</td>\n" for cell in row) + "</tr>\n" for row in TABLE_ROWS)
                   + "</tbody>\n</table>\n<p>All green but packaging; the mirror timed out once.</p>\n")

# Every frame from the Threads view chosen: how the snapshot's card is drawn, while it is.
FRAMES_JS = """(id) => { window.__cardFrames = []; const look = () => {
    const card = document.querySelector(`#side-cards > li[data-card][data-id='${id}']`);
    if (card) window.__cardFrames.push([card.hasAttribute('data-split-card'), card.hasAttribute('data-card-clamp'),
        card.querySelectorAll('.side-card-text td, .side-card-text th').length, card.querySelector('.side-card-text').innerText]);
    if (window.__cardFrames.length < 120) requestAnimationFrame(look); };
  requestAnimationFrame(look); }"""


def table_walk(desk: Desk) -> None:
    page = desk.page
    page.evaluate(FRAMES_JS, SNAPSHOT)
    to_threads(desk)
    page.wait_for_timeout(500)
    frames = page.evaluate("() => window.__cardFrames")
    assert isinstance(frames, list)
    desk.check(len(frames) > 0, "the snapshot's card was never drawn")
    stub = "Table (7 columns × 6 rows) …"
    bad = [frame for frame in frames if not frame[0] or frame[1] or frame[2] or str(frame[3]).strip() != stub]
    desk.check(not bad, f"a frame drew the card other than split on one line: {bad[:3]} of {len(frames)}")
    found = card(desk)
    desk.check(number(found, "lines") <= 1.5, f"the card draws {number(found, 'lines'):.2f} lines, not one")
    desk.shot("1-card")
    unpack_all(desk)
    cells = page.evaluate(f"""() => [...document.querySelectorAll("#side-cards > li[data-unpack-thread='{SNAPSHOT}'][data-root-continuation] .md tr")]
        .map((row) => [...row.querySelectorAll('td, th')].map((cell) => cell.innerText))""")
    desk.check(cells == [TABLE_HEAD, *TABLE_ROWS], f"the continuation does not draw the table whole: {cells}")
    desk.shot("2-unpacked")
    desk.notes.append(f"{len(frames)} frames of the card, every one split on one line ({stub!r}); "
                      f"the continuation draws the table's {len(cells)} rows whole")


def pending_walk(desk: Desk, api: MarkdownApi) -> None:
    to_threads(desk)
    unpack_all(desk)
    found = card(desk)
    desk.check(bool(found["pending"]) and not found["split"],
               f"the card does not wait for its summary: pending {found['pending']}, split {found['split']}")
    root = first_row(found)
    desk.check(bool(root["child"]) and not root["continuation"] and str(root["words"]).startswith("MORNING SNAPSHOT"),
               f"the message is not inward whole while its summary is on its way: {root}")
    desk.check("ol" in strings(found["tags"]), "the waiting card lost its formatting")
    desk.shot("1-waiting")
    page = desk.page
    page.evaluate(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{SNAPSHOT}']\")"
                  ".scrollIntoView({block: 'center'})")
    desk.settled()
    before = card(desk)
    filed = time.monotonic()
    api.file_name(NAMED)
    desk.wait_for(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{SNAPSHOT}']\")"
                  ".hasAttribute('data-summarised-card')", "the summary never reached the card", seconds=15)
    took = time.monotonic() - filed
    desk.settled()
    after = card(desk)
    moved = number(after, "box", "top") - number(before, "box", "top")
    row_moved = number(first_row(after), "box", "top") - number(first_row(before), "box", "top")
    card_grew = number(after, "box", "height") - number(before, "box", "height")
    desk.check(abs(moved) <= 1, f"the summary landing moved the card by {moved:.1f}px")
    desk.check(str(after["words"]).startswith(str(NAMED["display_name"])), f"the card does not say the summary: {after['words']!r}")
    desk.check(bool(first_row(after)["child"]) and not first_row(after)["continuation"],
               "the message under the card changed as the summary landed")
    desk.check(took <= 6, f"the summary took {took:.1f}s to reach the card after it was filed")
    desk.shot("2-summary-landed")
    desk.notes.append(f"waiting in the summary's layout; filed, the summary reached the card in {took:.1f}s, "
                      f"the card moved {moved:.1f}px and grew {card_grew:.0f}px, the row under it moved {row_moved:.1f}px")


CHIP_JS = """() => {
    const chip = document.getElementById('page-update');
    const pill = document.getElementById('channel-freshness');
    const box = (node) => { const r = node.getBoundingClientRect(); return {left: r.left, right: r.right, top: r.top, bottom: r.bottom}; };
    return {shown: chip.getClientRects().length > 0, words: chip.textContent, chip: box(chip),
            pill: pill.getClientRects().length > 0 ? box(pill) : null, width: window.innerWidth,
            overflow: document.documentElement.scrollWidth - window.innerWidth};
}"""


def build_walk(desk: Desk, api: MarkdownApi) -> None:
    page = desk.page
    own = str(page.evaluate("() => PAGE_BUILD"))
    desk.check(len(own) == 32, f"the page does not know its own build: {own!r}")
    # A page build's shell names the script by its hash, which is the build every read reports.
    manifest = fixture.WEB_ROOT / "assets.json"
    if manifest.is_file():
        built = str(json.loads(manifest.read_text())["page_build"])
        desk.check(own == built, f"the page thinks it is build {own!r}, not the page build {built!r}")
    reread = "() => { threadDirectoryReadAt.clear(); return refreshThreadDirectory(); }"
    with api.lock:
        api.page_build = own
    page.evaluate(reread)
    desk.settled()
    same = page.evaluate(CHIP_JS)
    assert isinstance(same, dict)
    desk.check(not same["shown"], "the page offered a reload of the build it runs")
    with api.lock:
        api.page_build = "f" * 32
    page.evaluate(reread)
    desk.wait_for("() => !document.getElementById('page-update').hidden", "a newer build did not show the chip")
    desk.settled()
    found = page.evaluate(CHIP_JS)
    assert isinstance(found, dict)
    chip = found["chip"]
    pill = found["pill"]
    assert isinstance(chip, dict)
    desk.check(bool(found["shown"]) and "New version" in str(found["words"]), f"the chip is not drawn: {found}")
    desk.check(0 <= float(chip["left"]) and float(chip["right"]) <= float(str(found["width"])) and float(str(found["overflow"])) <= 0,
               f"the chip is outside the window: {found}")
    desk.check(not isinstance(pill, dict) or float(pill["right"]) <= float(chip["left"]) + 0.5
               or float(chip["right"]) <= float(pill["left"]) + 0.5, f"the chip covers the freshness pill: {found}")
    desk.shot("1-new-version")
    page.evaluate("() => { window.__beforeReload = true; }")
    with page.expect_navigation():
        page.click("#page-update")
    desk.check(not page.evaluate("() => Boolean(window.__beforeReload)"), "the chip did not reload the page")
    desk.notes.append(f"a newer build showed the chip at x {float(chip['left']):.0f}-{float(chip['right']):.0f}px "
                      "beside the pill, and a press reloaded the page")


def old_shell_walk(desk: Desk, api: MarkdownApi) -> None:
    """The old shell's page started (sign_in passed and nothing threw); it knows the build its shell
    named, and a read saying the server serves the page build offers the reload."""
    page = desk.page
    own = str(page.evaluate("() => PAGE_BUILD"))
    desk.check(own == OLD_SHELL_BUILD, f"the old shell's page thinks it is build {own!r}, not {OLD_SHELL_BUILD!r}")
    desk.check(bool(page.evaluate("() => typeof VibeTalkContract.decode === 'function'")),
               "the validators are not there for the old shell's page")
    served = PAGE_BUILD_SERVED
    desk.check(served != own, "the page build and the old shell's build are the same")
    with api.lock:
        api.page_build = served
    page.evaluate("() => { threadDirectoryReadAt.clear(); return refreshThreadDirectory(); }")
    desk.wait_for("() => !document.getElementById('page-update').hidden",
                  "the old shell's page did not offer the page build")
    desk.shot("1-old-shell")
    desk.notes.append(f"a shell from before the page build started on today's /voice.js and /contract.js, knew its "
                      f"build {own[:8]} and offered the page build {str(served)[:8]}")


def summarised_at(desk: Desk) -> str:
    return str(desk.page.evaluate(
        f"() => {{ const card = document.querySelector(\"#side-cards > li[data-card][data-id='{SNAPSHOT}']\");"
        " return !card ? 'none' : card.hasAttribute('data-summarised-card') ? 'summary' :"
        " card.hasAttribute('data-split-card') ? 'split' : card.hasAttribute('data-summary-pending') ? 'pending' : 'other'; }"))


def load_walk(desk: Desk, api: MarkdownApi, args: argparse.Namespace, port: int) -> None:
    page = desk.page
    # A fresh load: the Threads view chosen, its card and its summary timed from then.
    start = time.monotonic()
    page.select_option("#thread-select", "threads")
    first_seen = None
    summary_seen = None
    states: list[str] = []
    while time.monotonic() - start < 15 and summary_seen is None:
        state = summarised_at(desk)
        if not states or states[-1] != state:
            states.append(state)
        now = time.monotonic() - start
        if state != "none" and first_seen is None:
            first_seen = now
        if state == "summary":
            summary_seen = now
        page.wait_for_timeout(20)
    first = -1.0 if first_seen is None else first_seen
    named = -1.0 if summary_seen is None else summary_seen
    desk.notes.append(f"fresh load, reads {args.latency:.1f}s each: the card at {first:.2f}s, its summary at "
                      f"{named:.2f}s ({named - first:.2f}s later), states {' > '.join(states)}")
    desk.shot("1-fresh")
    # A reload with every read held: what the device's copy draws before any read answers.
    page.wait_for_timeout(500)
    api.timeline_gate.clear()
    page.goto(f"http://127.0.0.1:{port}/", wait_until="load")
    page.wait_for_timeout(1500)
    held = summarised_at(desk)
    desk.shot("2-reload-reads-held")
    api.timeline_gate.set()
    opened = time.monotonic()
    desk.wait_for(f"() => {{ const card = document.querySelector(\"#side-cards > li[data-card][data-id='{SNAPSHOT}']\");"
                  " return card && card.hasAttribute('data-summarised-card'); }",
                  "the reloaded card never said the summary", seconds=15)
    after = time.monotonic() - opened
    desk.notes.append(f"reloaded with its reads held the card said {held}; the summary {'was there at once' if held == 'summary' else f'came {after:.2f}s after the reads were let through'}")
    if not args.report_only:
        desk.check(states[0] == "summary", f"the fresh load drew the card before its summary: {states}")
        desk.check(held == "summary", f"the reload drew the card as {held} before any read answered")


if __name__ == "__main__":
    raise SystemExit(main())
