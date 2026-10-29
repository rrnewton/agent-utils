#!/usr/bin/env python3
"""`#522 thread-card-max-lines` in real Chromium at 393x852 and 1280x800.

The owner, 2026-10-25: "I HATE having big thread cards... if the message-root is bigger than a
certain size (let's say K lines, with default K set to 5 lines), let's summarize it even with a
single-response and push the root message inward to be the first child. If summarization is
broken/disabled then I still want to SPLIT this over-large message. Put only the first K lines in
the thread card and append "..." to the end. Then display the remainder of the message as a virtual
first child underneath with "..." prepended. So when I fully pack all threads I still get a fairly
compact thread view without a giant thread card dominating it."

Against the fake API of tests/threads_polish_browser.py, whose replica thread opens here with a long
Markdown message (`#528 thread-card-markdown-and-cap`: two paragraphs, a numbered list of six and a
closing line, tests/thread_card_markdown_browser.py's) and has no summary, in the Threads view:

* packed, the replica's card is held to the collapsed lines as before, its words ending "…";
* unpacked (Unpack all), the card shows at most K = 5 drawn lines, cut between lines and ending "…";
  the first row under it is the message's continuation — the root's own row, its author, starting
  "…" with the lines the card left out, its list numbered on, and none of the row buttons
  (Reply, Done, Thread, ⋯) drawn — then the replies; no sideways scroll;
* the pack triangle on the continuation still packs the thread with its card held still (`#440`,
  `#459`), and a tap on the bridge's spine from the thread's foot still goes back to the card
  (`#501`);
* the summariser names the thread while it is unpacked and on screen: the next read of the thread
  list switches it to the summarised layout — the card says the summary, the whole question is the
  first child — and the card's top does not move by more than a pixel;
* Thread card lines set to 2 from Settings holds the unpacked card to two lines;
* a link to the question (`?v=threads&m=...`, `#462`) opened with every thread unpacked and the list
  at its foot lands on the replica's card at the head of the list, the continuation below it.

Pass --screenshots DIR to keep a PNG of each state; --web-root DIR to run against another copy of
the page (an older one fails it at the first unpacked card).
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
from inline_reply_browser import sign_in
from offline_cache_browser import check, handler_for
from thread_card_markdown_browser import MD_ITEMS, MD_ROOT, MD_ROOT_HTML
from threads_polish_browser import PolishApi, to_threads, unpack_to
from two_column_browser import REPLICA, Desk
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

SIZES = (("phone-393", 393, 852, True), ("desk", 1280, 800, False))

# K until the owner says otherwise (web/voice.js `DEFAULT_CARD_MAX_LINES`).
K = 5

# The replica's card, its text's drawn lines, and the rows unpacked under it.
CARD_JS = """(id) => {
    const card = document.querySelector(`#side-cards > li[data-card][data-id='${id}']`);
    const text = card.querySelector('.side-card-text');
    const style = getComputedStyle(text);
    const rows = [...document.querySelectorAll(`#side-cards > li[data-unpack-thread='${id}']`)];
    const first = rows[0] || null;
    const box = (node) => { const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height}; };
    const acts = first ? first.querySelector('.row-acts') : null;
    return {
        card: box(card), state: card.getAttribute('data-card-state'), split: card.hasAttribute('data-split-card'),
        summarised: card.hasAttribute('data-summarised-card'), words: text.textContent,
        lines: Math.round(text.getBoundingClientRect().height / parseFloat(style.lineHeight)),
        rows: rows.length,
        first: first ? {ids: first.getAttribute('data-ids'), author: first.getAttribute('data-author'),
            continuation: first.hasAttribute('data-root-continuation'), child: first.hasAttribute('data-root-child'),
            body: (first.querySelector('.body') || first).innerText,
            actsShown: Boolean(acts && acts.getClientRects().length > 0), box: box(first)} : null,
        scroller: box(document.getElementById('side-scroll')),
        overflow: document.documentElement.scrollWidth - window.innerWidth,
    };
}"""

SUMMARY = {"display_name": "replica-before-release",
           "summary": "Whether the replica migration, the dashboards and the silenced alert are done before release."}


class MarkdownReplicaApi(PolishApi):
    """`PolishApi`, the replica's question a long Markdown message, rendered as the server renders it."""

    def __init__(self) -> None:
        super().__init__()
        for index, row in enumerate(self.messages):
            thread = row.get("thread")
            if isinstance(thread, dict) and thread.get("id") == REPLICA and thread.get("is_root"):
                self.messages[index] = {**row, "content": MD_ROOT, "content_html": MD_ROOT_HTML}
                for at, held in enumerate(self.threads):
                    if held["id"] == REPLICA:
                        self.threads[at] = {**held, "root": self.messages[index]}


def card(desk: Desk) -> dict[str, object]:
    found = desk.page.evaluate(CARD_JS, REPLICA)
    assert isinstance(found, dict)
    desk.check(float(str(found["overflow"])) <= 0, f"the page scrolls sideways: {found['overflow']}")
    return found


def first_row(found: dict[str, object]) -> dict[str, object]:
    row = found["first"]
    assert isinstance(row, dict), f"nothing is unpacked under the card: {found}"
    return row


def number(found: dict[str, object], *path: str) -> float:
    at: object = found
    for key in path:
        assert isinstance(at, dict)
        at = at[key]
    return float(str(at))


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
        for label, width, height, touch in SIZES:
            if args.size and label not in args.size:
                continue
            report.append(walk(playwright.chromium, args, label, width, height, touch))
    for line in report:
        print(line)
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
         touch: bool) -> str:
    api = MarkdownReplicaApi()
    server = ThreadingHTTPServer(("127.0.0.1", args.port), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-card-lines-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark",
                has_touch=touch, is_mobile=touch)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"card-lines-{label}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            sign_in(desk)
            to_threads(desk)

            # Packed: the collapsed clamp, as before, over words that already stop at K lines.
            packed = card(desk)
            desk.check(packed["state"] == "collapsed" and bool(packed["split"]),
                       f"the premise: the replica's card packed and split, found {packed['state']}, split {packed['split']}")
            desk.check(str(packed["words"]).rstrip().endswith("…"), f"the card's words do not end in …: {packed['words']!r}")
            desk.check(number(packed, "lines") <= 3, f"the packed card shows {packed['lines']} lines")
            desk.shot("1-packed")

            # Unpacked: at most K lines on the card, the rest of the question first under it.
            unpack_to(desk, "true")
            desk.wait_for(f"() => document.querySelectorAll(\"#side-cards > li[data-unpack-thread='{REPLICA}']\").length >= 2",
                          "the replica did not unpack under its card")
            page.evaluate(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{REPLICA}']\")"
                          ".scrollIntoView({block: 'start'})")
            desk.settled()
            opened = card(desk)
            rest = first_row(opened)
            desk.check(opened["state"] == "unpacked", f"the replica is {opened['state']}, not unpacked")
            desk.check(1 <= number(opened, "lines") <= K,
                       f"the unpacked card shows {opened['lines']} lines, more than K = {K}")
            desk.check(bool(rest["continuation"]) and bool(rest["child"]),
                       f"the first row under the card is not the question's continuation: {rest}")
            body = str(rest["body"]).strip()
            head = str(opened["words"])
            desk.check(body.startswith("…"), f"the continuation does not start with …: {body[:60]!r}")
            desk.check(head.startswith("MORNING SNAPSHOT") and MD_ITEMS[0] in head and MD_ITEMS[1] not in head,
                       f"the card's words are not the message's first lines, to its first item: {head!r}")
            desk.check(MD_ITEMS[1] in body and "docs/plan.md" in body and "MORNING SNAPSHOT" not in body,
                       f"the continuation is not exactly the rest of the message: {body!r}")
            desk.check(rest["author"] == "ci-bot", f"the continuation is not the question's author's row: {rest}")
            desk.check(not rest["actsShown"], "the continuation draws row buttons of its own")
            desk.shot("2-unpacked-split")
            desk.notes.append(f"unpacked, the split card is {number(opened, 'card', 'height'):.0f}px, "
                              f"{opened['lines']} lines (K = {K}), the rest of the question under it in "
                              f"{number(rest, 'box', 'height'):.0f}px with no buttons")

            # The summariser names the thread while it is on screen: the layout switches, the card stays.
            page.evaluate(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{REPLICA}']\")"
                          ".scrollIntoView({block: 'center'})")
            desk.settled()
            top = number(card(desk), "card", "top")
            with api.lock:
                for index, held in enumerate(api.threads):
                    if held["id"] == REPLICA:
                        api.threads[index] = {**held, **SUMMARY}
            page.evaluate("() => { threadDirectoryReadAt.clear(); return refreshThreadDirectory(); }")
            desk.wait_for(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{REPLICA}']\")"
                          ".hasAttribute('data-summarised-card')", "the summary never reached the card")
            desk.settled()
            named = card(desk)
            whole = first_row(named)
            moved = number(named, "card", "top") - top
            desk.check(abs(moved) <= 1, f"the summary landing moved the card by {moved:.1f}px")
            desk.check(not named["split"] and str(named["words"]).startswith(SUMMARY["display_name"]),
                       f"the card does not say the summary: {named['words']!r}")
            desk.check(bool(whole["child"]) and not whole["continuation"],
                       f"the whole question is not the first child: {whole}")
            desk.check(str(whole["body"]).strip().startswith("MORNING SNAPSHOT"),
                       f"the first child is not the whole message: {whole['body']!r}")
            desk.shot("3-summary-landed")
            desk.notes.append(f"the summary landed with the card moved {moved:.1f}px, the whole question its first child")

            # Without the summary again, K set to 2 in Settings: the unpacked card is two lines.
            with api.lock:
                for index, held in enumerate(api.threads):
                    if held["id"] == REPLICA:
                        api.threads[index] = {key: value for key, value in held.items() if key not in SUMMARY}
            page.evaluate("() => { threadDirectoryReadAt.clear(); return refreshThreadDirectory(); }")
            desk.wait_for(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{REPLICA}']\")"
                          ".hasAttribute('data-split-card')", "the card did not split again without its summary")
            page.evaluate("() => { const input = document.getElementById('card-max-lines'); input.value = '2';"
                          " input.dispatchEvent(new Event('input', {bubbles: true})); }")
            desk.settled()
            two = card(desk)
            desk.check(number(two, "lines") <= 2, f"at K = 2 the unpacked card shows {two['lines']} lines")
            desk.check(bool(first_row(two)["continuation"]), "at K = 2 the continuation is gone")
            desk.notes.append(f"at K = 2 the unpacked card shows {two['lines']} lines")
            desk.shot("4-two-lines")

            # The triangle on the continuation still packs the thread, its card held still.
            page.evaluate(f"() => document.querySelector(\"#side-cards > li[data-card][data-id='{REPLICA}']\")"
                          ".scrollIntoView({block: 'center'})")
            desk.settled()
            top = number(card(desk), "card", "top")
            page.click(f"#side-cards > li[data-unpack-thread='{REPLICA}'][data-root-continuation] .unpack-pack")
            desk.wait_for(f"() => document.querySelectorAll(\"#side-cards > li[data-unpack-thread='{REPLICA}']\").length === 0",
                          "the triangle on the continuation did not pack the thread")
            desk.settled()
            moved = number(card(desk), "card", "top") - top
            desk.check(abs(moved) <= 2, f"packing moved the card by {moved:.1f}px")
            desk.notes.append(f"the triangle on the continuation packed the thread, the card moved {moved:.1f}px")
            desk.shot("5-packed-again")

            # A link to the question lands on its card, not on its continuation nor out of sight. Round
            # the chip to Unpack all again, which gives the packed replica back to it (`#440`).
            unpack_to(desk, "false")
            unpack_to(desk, "true")
            desk.wait_for(f"() => document.querySelector(\"#side-cards > li[data-unpack-thread='{REPLICA}'][data-root-continuation]\")",
                          "the replica did not unpack split again")
            link = page.evaluate("() => `/?c=${encodeURIComponent(document.getElementById('discord-channel').value)}`"
                                 " + `&v=threads&m=${deepLinkRefText(deepLinkRefOf(channelCanon.messages.find("
                                 f"(m) => m.thread && m.thread.id === '{REPLICA}' && m.thread.is_root)))}}`")
            page.goto(f"http://127.0.0.1:{server.server_port}{link}", wait_until="load")
            desk.wait_for("() => /linked message's card/.test(document.getElementById('status').textContent)",
                          "the link did not land on a card", seconds=20)
            desk.settled()
            landed = card(desk)
            gap = number(landed, "card", "top") - number(landed, "scroller", "top")
            desk.check(0 <= gap <= 24, f"the linked question's card is {gap:.0f}px from the head of the list")
            desk.check(bool(first_row(landed)["continuation"]) and number(first_row(landed), "box", "top") > number(landed, "card", "bottom") - 1,
                       "the continuation is not under the landed card")
            desk.notes.append(f"a link to the question landed on its card {gap:.0f}px from the head of the list")
            desk.shot("6-linked")
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
