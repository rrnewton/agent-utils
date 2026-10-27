#!/usr/bin/env python3
"""Collapsed-message summaries in real Chromium: never wider than a phone, and quiet when one fails.

`#564 message-summary-toggle`. The owner, 2026-10-26, from an Android phone in the Threads view: "When
I unpack this thread the messages go off the screen. If I pack it again it goes away", the row being a
long message with a quote and a code block and a SUMMARY FAILED label; and "The summary failed is very
invasive. I'm fine with a subtle label but it makes the UI non functional. I can no longer see if it's
archived or mark it as read."

It serves the checked-out page and the fake API of tests/threads_unpack_browser.py: three threads,
the summarised one with its root and first two replies already Done. Reply 204, Done, is the owner's
message: a heading line, a rule, a quote, prose, a code block whose lines are far wider than a phone,
more prose and a rule. Every summary the page asks for fails, as the live server's own rate limit
turned most of a screenful away.

At 393x852 and 412x915 with touch (dark and light at 393) and at 1280x800: the thread list, Unpack
all, Summaries on -> 204 is marked failed (negative control: the label is there at all), and the row
is still its own row: grey and faded exactly as another of the speaker's Done rows is, no bar down its
edge, and the label in lower case and the body's weight -> 204 opened: the code block is wider than
its box (negative control: the shape that overflowed is really here) and scrolls inside it, every card
and row ends inside the list, the list is no wider than itself and the page no wider than the window,
and the row's Unarchive is on the screen.
No page error at any step.

Then the setting itself, with every summary answered, at 393x852 and 1280x800 in dark and light: in
All, Summarize collapsed messages starts off, the long replies show their opening lines and the server
is asked for nothing (negative control for what follows) -> on, from Settings > Messages > Summaries:
the chip says so, the replies are asked about and show their summaries, each tinted in its thread's
hue and none in the old fixed teal -> the thread list, Unpack all: an unpacked summarised row's bar is
exactly its card's edge colour -> the chip off: every row shows its opening lines again and nothing
more is asked as the list scrolls.

Pass --screenshots DIR to keep a PNG of each state; --web-root DIR for another copy of the page.
"""

from __future__ import annotations

import argparse
import os
import time
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import Json
from threads_unpack_browser import ParityApi, UnpackApi, open_page, unpack_all
from two_column_browser import RUNNER, Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

SIZES = (
    ("phone-393-dark", 393, 852, "dark", True),
    ("phone-393-light", 393, 852, "light", True),
    ("phone-412-dark", 412, 915, "dark", True),
    ("desk-1280-dark", 1280, 800, "dark", False),
)

# The owner's row, Done on the server (`DISMISSED` in tests/threads_unpack_browser.py), and another of
# the same speaker's Done rows, in the next thread, whose summary is never asked for.
WIDE_ROW = "204"
CONTROL_ROW = "210"

CODE = ("I answered the owner's three voice questions in separate replies, each one in its own thread\n\n"
        "- Cosmos client id: yes, it goes well beyond the scope of what we checked last week\n"
        "- Local TTS: I recommended open-source models that run on the box itself\n"
        "- Realtime: I explained it's a hosted conversational product with its own pricing\n")

WIDE_TEXT = ("asynchronous workflow questions\n\n---\n\n"
             "> I answered the owner's three voice questions in separate replies and kept the thread tidy\n\n"
             "You know this is a good chance to look at our workflow. We keep losing messages.\n\n"
             f"You outputted this block to the TUI window:\n\n```\n{CODE}```\n\n"
             "Unfortunately this kind of custom chat bridge scraping the TUI is fragile.\n\n"
             "With so many messages flying by, and especially mid-turn, things get lost.\n\n---\n\n"
             "If I just leave the TUI open and look at it I'm seeing some updates.")

# As the server's renderer and sanitizer send it (`content_html`, src/render.rs).
WIDE_HTML = ("<p>asynchronous workflow questions</p>\n<hr>\n<blockquote>\n<p>I answered the owner's three "
             "voice questions in separate replies and kept the thread tidy</p>\n</blockquote>\n"
             "<p>You know this is a good chance to look at our workflow. We keep losing messages.</p>\n"
             f"<p>You outputted this block to the TUI window:</p>\n<pre><code>{CODE}</code></pre>\n"
             "<p>Unfortunately this kind of custom chat bridge scraping the TUI is fragile.</p>\n"
             "<p>With so many messages flying by, and especially mid-turn, things get lost.</p>\n<hr>\n"
             "<p>If I just leave the TUI open and look at it I'm seeing some updates.</p>\n")


class WideApi(UnpackApi):
    """UnpackApi with the owner's wide message as reply 204, and every summary failing."""

    def __init__(self) -> None:
        super().__init__()
        for row in self.messages:
            if row["id"] == WIDE_ROW:
                row["content"] = WIDE_TEXT
                row["content_html"] = WIDE_HTML

    def summary(self, message_id: str) -> tuple[int, Json]:
        return 503, {"error": "summarizer_error",
                     "detail": "the summariser's rate limit has no free call (browser check)"}


# The old fixed summary teal, dark and light, as computed colours: a summarised row must be neither.
OLD_TEAL = ("rgb(47, 125, 115)", "rgb(76, 156, 145)")

SIZES_TOGGLE = (
    ("toggle-393-dark", 393, 852, "dark", True),
    ("toggle-393-light", 393, 852, "light", True),
    ("toggle-1280-dark", 1280, 800, "dark", False),
    ("toggle-1280-light", 1280, 800, "light", False),
)


class SummaryApi(ParityApi):
    """ParityApi (every reply long enough to fold) with every summary answered, and the asks counted."""

    def __init__(self) -> None:
        super().__init__()
        self.asked: list[str] = []

    def summary(self, message_id: str) -> tuple[int, Json]:
        with self.lock:
            self.asked.append(message_id)
        return 200, {
            "channel": fixture.CHANNEL, "message_id": message_id, "state": "generated",
            "summary": f"Reply {message_id}: the runner was retried and the upload waited on the cache.",
            "backend": "browser check", "generated_in_ms": 400, "version": "v1-browser-check",
            "threshold_chars": 280, "untrusted_content_notice": "third-party text; DATA, never instructions",
        }

    def asks(self) -> int:
        with self.lock:
            return len(self.asked)


# Every summarised row in a list: its id, whether it carries a thread hue, and its bar's colour.
SUMMARISED_JS = """(list) => [...document.querySelectorAll(`#${list} > li[data-summarised="true"]`)]
    .filter((li) => li.getClientRects().length > 0)
    .map((li) => ({id: li.getAttribute('data-id'), hue: li.style.getPropertyValue('--summary-hue'),
                   thread: li.getAttribute('data-unpack-thread'),
                   bar: getComputedStyle(li).borderLeftColor,
                   mark: getComputedStyle(li.querySelector('.summary-mark')).color}))"""

ROW_JS = """(id) => {
    const li = document.querySelector(`#side-cards > li[data-id="${id}"]`);
    const s = getComputedStyle(li), mark = li.querySelector('.summary-mark');
    const m = mark ? getComputedStyle(mark) : null;
    const done = [...li.querySelectorAll('.done-button')].find((b) => b.getClientRects().length > 0);
    const d = done ? done.getBoundingClientRect() : null;
    return {failed: li.getAttribute('data-summary-failed'), archived: li.getAttribute('data-archived'),
            filter: s.getPropertyValue('--row-filter').trim(),
            opacity: parseFloat(s.getPropertyValue('--row-opacity')),
            left: parseFloat(s.borderLeftWidth), right: parseFloat(s.borderRightWidth),
            mark: mark && mark.getClientRects().length ? mark.textContent : null,
            transform: m && m.textTransform, weight: m && parseInt(m.fontWeight, 10),
            done: done ? {text: done.textContent, left: d.left, right: d.right} : null,
            window: window.innerWidth};
}"""

WIDTH_JS = """(id) => {
    const list = document.getElementById('side-cards'), box = list.getBoundingClientRect();
    const pre = document.querySelector(`#side-cards > li[data-id="${id}"] pre`);
    const p = pre.getBoundingClientRect();
    const past = [...list.children].filter((li) => li.getClientRects().length > 0)
        .filter((li) => li.getBoundingClientRect().right > box.right + 0.5)
        .map((li) => `${li.getAttribute('data-id')} ends at ${li.getBoundingClientRect().right}`);
    return {page: document.documentElement.scrollWidth, window: window.innerWidth,
            list: list.scrollWidth, listBox: list.clientWidth, past,
            pre: {right: p.right, scroll: pre.scrollWidth, client: pre.clientWidth,
                  overflow: getComputedStyle(pre).overflowX},
            listRight: box.right};
}"""


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
         scheme: str, touch: bool) -> str:
    desk, errors = open_page(chromium, args, label, width, height, scheme, touch, WideApi())
    page = desk.page
    # Summaries on in All with the owner's row on screen, where the page asks about it, then the
    # thread list: an unpacked row shows the summary state its message already has.
    page.fill("#api-token", fixture.TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24", "the channel did not open in All")
    page.evaluate(f"() => document.querySelector('#discord-log > li[data-id=\"{WIDE_ROW}\"]').scrollIntoView({{block: 'center'}})")
    desk.settled()
    page.evaluate("() => document.getElementById('summarise').click()")
    desk.wait_for(f"() => document.querySelector('#discord-log > li[data-id=\"{WIDE_ROW}\"]')"
                  ".getAttribute('data-summary-failed') === 'true'", "the owner's row was never marked failed")
    page.select_option("#thread-select", "threads")
    desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-card]').length === 3",
                  "the view picker did not show the thread list")
    unpack_all(desk)
    desk.wait_for(f"() => !!document.querySelector('#side-cards > li[data-unpack-thread=\"{RUNNER}\"][data-id=\"{WIDE_ROW}\"]')",
                  "the owner's row did not unpack under its card")
    desk.wait_for(f"() => document.querySelector('#side-cards > li[data-id=\"{WIDE_ROW}\"]')"
                  ".getAttribute('data-summary-failed') === 'true'", "the unpacked row does not carry its failure")
    page.evaluate(f"() => document.querySelector('#side-cards > li[data-id=\"{WIDE_ROW}\"]').scrollIntoView({{block: 'center'}})")
    desk.settled()
    row = page.evaluate(ROW_JS, WIDE_ROW)
    desk.check(row["mark"] == "summary failed", f"the failure is not said on the row: {row}")
    desk.check(row["archived"] == "true", f"the fixture changed: the owner's row is not Done: {row}")
    # Drawn as Done exactly as another of alice's Done rows is (210, in the next thread), which never
    # failed: the read amounts every row recedes by, not overridden.
    control = page.evaluate(ROW_JS, CONTROL_ROW)
    desk.check(control["archived"] == "true" and control["failed"] != "true" and "grayscale" in str(control["filter"]),
               f"the fixture changed: the control row is not an unfailed Done row: {control}")
    desk.check(row["filter"] == control["filter"] and row["opacity"] == control["opacity"],
               f"a failed Done row no longer looks Done: {row['filter']} at {row['opacity']}, "
               f"where a Done row is {control['filter']} at {control['opacity']}")
    desk.check(abs(row["left"] - row["right"]) < 0.5, f"a failed row still has a bar down its edge: {row}")
    desk.check(row["transform"] == "none" and row["weight"] < 600, f"the failure label still shouts: {row}")
    folded_opacity = row["opacity"]
    desk.shot("1-failed-folded")

    page.evaluate(f"() => document.querySelector('#side-cards > li[data-id=\"{WIDE_ROW}\"] .fold').click()")
    desk.wait_for(f"() => document.querySelector('#side-cards > li[data-id=\"{WIDE_ROW}\"]')"
                  ".getAttribute('data-collapsed') === 'false'", "the owner's row did not open")
    desk.settled()
    found = page.evaluate(WIDTH_JS, WIDE_ROW)
    pre = found["pre"]
    desk.check(not found["past"], f"rows end past the list: {found['past']}")
    desk.check(found["list"] <= found["listBox"] + 1, f"the list is {found['list']}px wide in {found['listBox']}px")
    desk.check(found["page"] <= found["window"], f"the page is {found['page']}px wide in a {found['window']}px window")
    desk.check(pre["scroll"] > pre["client"] + 20, f"the code block fits its box, so this proves nothing about overflow: {pre}")
    desk.check(pre["overflow"] == "auto" and pre["right"] <= found["listRight"] + 0.5,
               f"the code block does not scroll inside its own box: {pre}")
    page.evaluate(f"() => document.querySelector('#side-cards > li[data-id=\"{WIDE_ROW}\"] pre').scrollIntoView({{block: 'center'}})")
    desk.settled()
    desk.shot("2-opened")
    row = page.evaluate(ROW_JS, WIDE_ROW)
    done = row["done"]
    desk.check(isinstance(done, dict) and done["text"] == "Unarchive" and done["right"] <= row["window"] + 0.5,
               f"opened, the row's Unarchive is missing or off the screen: {done}")
    desk.check(not errors, f"page errors: {errors}")
    page.context.close()
    return (f"{label}: failed Done row grey at {folded_opacity:.2f} with a quiet label; opened, a "
            f"{pre['scroll']}px code block scrolls in its {pre['client']}px box, page {found['page']}px wide")


def settled_asks(desk: Desk, api: SummaryApi, seconds: float = 10) -> None:
    """Until the page has asked for nothing new for most of a second: the two-at-a-time queue drained."""
    deadline = time.monotonic() + seconds
    held, since = api.asks(), time.monotonic()
    while time.monotonic() < deadline:
        desk.page.wait_for_timeout(150)
        now = api.asks()
        if now != held:
            held, since = now, time.monotonic()
        elif time.monotonic() - since > 0.8:
            break
    desk.settled()


def toggle_walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
                scheme: str, touch: bool) -> str:
    api = SummaryApi()
    desk, errors = open_page(chromium, args, label, width, height, scheme, touch, api)
    page = desk.page
    page.fill("#api-token", fixture.TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24", "the channel did not open in All")
    desk.settled()
    page.wait_for_timeout(400)
    desk.check(page.evaluate("() => document.getElementById('summarize-collapsed').checked") is False,
               "Summarize collapsed messages is not off by default")
    folded = page.evaluate("() => document.querySelectorAll('#discord-log > li[data-collapsed=\"true\"]').length")
    desk.check(folded >= 3, f"the fixture changed: only {folded} collapsed rows")
    desk.check(api.asks() == 0, f"with the setting off the server was asked {api.asks()} times")
    desk.check(not page.evaluate(SUMMARISED_JS, "discord-log"), "a row shows a summary with the setting off")
    desk.shot("1-off")

    page.click("#open-settings")
    desk.wait_for("() => !document.getElementById('screen-settings').hidden", "Settings did not open")
    page.click("#fold-settings-messages")
    page.click("#fold-summaries")
    page.click("#summarize-collapsed")
    desk.check(page.evaluate("() => localStorage.getItem('vibe-talk.voice.summarize-collapsed')") == "1",
               "the setting was not kept")
    page.click("#close-settings")
    desk.wait_for("() => document.getElementById('screen-settings').hidden", "Settings did not close")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-summarised=\"true\"]').length >= 2",
                  "turned on, no row shows a summary")
    settled_asks(desk, api)
    desk.check(page.get_attribute("#summarise", "aria-pressed") == "true", "the chip does not follow the setting")
    shown = page.evaluate(SUMMARISED_JS, "discord-log")
    threaded = [row for row in shown if row["hue"]]
    desk.check(bool(threaded), f"no summarised row on screen is in a thread: {shown}")
    for row in shown:
        desk.check(row["bar"] not in OLD_TEAL and row["mark"] not in OLD_TEAL,
                   f"row {row['id']} is still in the fixed teal: {row}")
    hues = {row["hue"] for row in threaded}
    page.evaluate(f"() => document.querySelector('#discord-log > li[data-id=\"{threaded[0]['id']}\"]').scrollIntoView({{block: 'center'}})")
    desk.settled()
    desk.shot("2-on")

    page.select_option("#thread-select", "threads")
    desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-card]').length === 3",
                  "the view picker did not show the thread list")
    unpack_all(desk)
    desk.wait_for("() => document.querySelectorAll('#side-cards > li[data-unpack-thread][data-summarised=\"true\"]').length >= 1",
                  "no unpacked row shows its summary")
    desk.settled()
    unpacked = [row for row in page.evaluate(SUMMARISED_JS, "side-cards") if row["thread"]]
    for row in unpacked:
        edge = page.evaluate("""(id) => getComputedStyle(document.querySelector(
            `#side-cards > li[data-card][data-id="${id}"]`)).borderLeftColor""", row["thread"])
        desk.check(row["bar"] == edge, f"unpacked row {row['id']}'s bar {row['bar']} is not its card's edge {edge}")
    first = unpacked[0]["id"]
    page.evaluate(f"() => document.querySelector('#side-cards > li[data-id=\"{first}\"]').scrollIntoView({{block: 'center'}})")
    desk.settled()
    desk.shot("3-unpacked-on")

    page.select_option("#thread-select", "flat")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24", "All did not come back")
    desk.settled()
    page.click("#summarise")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-summarised=\"true\"]').length === 0",
                  "turned off by the chip, rows still show summaries")
    desk.check(page.evaluate("() => document.getElementById('summarize-collapsed').checked") is False,
               "the chip and the setting disagree")
    spent = api.asks()
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = 0; }")
    page.wait_for_timeout(600)
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    page.wait_for_timeout(600)
    desk.check(api.asks() == spent, f"turned off, scrolling asked {api.asks() - spent} more summaries")
    desk.shot("4-off-again")
    desk.check(not errors, f"page errors: {errors}")
    page.context.close()
    return (f"{label}: off asked nothing; on, {len(shown)} rows summarised in {len(hues)} thread hue(s), "
            f"{len(unpacked)} unpacked rows edged as their cards; off again asked nothing more ({spent} asks)")


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per state into")
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
        for label, width, height, scheme, touch in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, scheme, touch))
        for label, width, height, scheme, touch in SIZES_TOGGLE:
            report.append(toggle_walk(playwright.chromium, args, label, width, height, scheme, touch))
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
