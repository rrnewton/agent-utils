#!/usr/bin/env python3
"""Turn the Links view's URL chip on and off in real Chromium, at a 393x852 phone and a 1280x800 desk.

`#620 link-url-chip`. The owner, in the Links view with Files on, every link reading the same
"REPO:README.md": "When we're in URL search mode I would like to have a little chip named URL
alongside the other filters for files and so on. When I click that it switches back to raw URLs
instead of the render hyperlink and that would at least let me see where these files live." In the
same screenshot the first "… unmatched messages …" line showed through between the pills of the chip
row. The fake-DOM suite (tests/js/voice_page.test.mjs) pins the rules; this lays them out for real.

It serves the checked-out page and the small fake API of tests/bookmark_browser.py from one loopback
origin, over a channel of twenty messages, six with a link: three READMEs as the server labels them
by their folders ("vibe-talk/README.md", the repository's own "widgets/README.md",
"nested/README.md"), one source file, and two documents on another host, one with an address longer
than a phone is wide. At each size:

  sign in and open the channel -> the search glass, Links: URL first in the row of chips, off and not
  struck through, the six kinds after it; each link written as its label -> the list scrolled so that
  the first unmatched line stands under the row: nothing of it, or of any row, drawn between or
  around the pills -> URL pressed: each link written as its whole address, wrapped inside the list at
  its slashes, the longest on more than one line and none wider than the list -> the page reloaded:
  URL still on -> "vibe-talk" typed, a word only in an address: that message's link alone, as its
  address; URL pressed again: the same link, as its label, and the device's choice forgotten. No
  horizontal overflow and no page error at any step.

Pass --web-root DIR to run it against another copy of the page, and --screenshots DIR to keep a PNG of
each stage. VIBE_TALK_TEST_PORT picks the loopback port (default: any free one).
"""

from __future__ import annotations

import os
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING, TypedDict

import search_dates_browser as week
from bookmark_browser import AGENT, CHANNEL, TOKEN, FakeApi, Json, check, handler_for, message

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page, Playwright

# (label, width, height, touch): the phone the issue names; a phone over 400px wide, where the author
# filter stands on the bar's line and the list no longer starts below a control hung under it, which is
# where the owner saw the list through the row; and a desk.
SIZES = (("phone", 393, 852, True), ("phone-412", 412, 915, True), ("desk", 1280, 800, False))

REPO = "https://github.com/example/widgets"
LONG = ("https://docs.example.com/handbook/operations/release-engineering/runbooks/2026/"
        "canary-promotion-and-rollback-procedure.html")
# (address, the label the server draws it with, the words around it) for the six messages with a
# link; every other message has none. The labels are src/link_labels.rs's for these addresses.
LINKS: tuple[tuple[str, str, str], ...] = (
    (f"{REPO}/blob/main/vibe-talk/README.md", "vibe-talk/README.md", "The page's notes are in"),
    (f"{REPO}/blob/main/README.md", "widgets/README.md", "Start from"),
    (f"{REPO}/blob/main/tools/deep/nested/README.md", "nested/README.md", "The nested tool says how, in"),
    (f"{REPO}/blob/main/src/render.rs#L10", "widgets:render.rs#L10", "The renderer starts at"),
    ("https://docs.example.com/guide", "the guide", "Background:"),
    (LONG, "the canary runbook", "Before promoting, read"),
)
# Where each linked message stands among the twenty: the rest are plain words between them, so the
# Links view leaves a counted line in every gap.
LINKED_AT = (3, 6, 9, 12, 15, 18)
STORAGE_KEY = "vibe-talk.voice.link-raw-urls"


def anchor(href: str, label: str) -> str:
    """A link as the server writes one: its label, its address in the title."""
    return (f'<a href="{href}" title="{href}" target="_blank" rel="noopener noreferrer nofollow">'
            f"{label}</a>")


class LinkApi(FakeApi):
    """The fake API of tests/bookmark_browser.py over the twenty messages above."""

    def __post_init__(self) -> None:
        links = iter(LINKS)
        for index in range(20):
            if index in LINKED_AT:
                href, label, words = next(links)
                row = message(index, f"{words} {href}", AGENT)
                row["content_html"] = f"<p>{words} {anchor(href, label)}</p>\n"
            else:
                row = message(index, f"Status line {index}: the shard is still warming.", AGENT)
            self.messages.append(row)


def serve(api: FakeApi, web_root: Path) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    port = int(os.environ.get("VIBE_TALK_TEST_PORT", "0"))
    server = ThreadingHTTPServer(("127.0.0.1", port), handler_for(api, web_root))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread, f"http://127.0.0.1:{server.server_port}/"


class Box(TypedDict):
    """An element's box on the screen, in CSS px."""

    top: float
    bottom: float
    left: float
    right: float
    height: float


class ShownLink(TypedDict):
    """One link the Links view shows, as `STATE_JS` reads it."""

    text: str
    href: str
    box: Box | None
    row: Box | None
    lines: int
    split: list[str]
    scrollWidth: int  # noqa: N815 - the DOM's name
    clientWidth: int  # noqa: N815 - the DOM's name
    raw: bool


class LinksState(TypedDict):
    """What `STATE_JS` answers."""

    links: list[ShownLink]
    overflow: float


# The chip row, its buttons, and every shown link: its text, its href and its box, and the list's.
STATE_JS = """() => {
  const box = (node) => {
    if (!node || !node.getClientRects().length) return null;
    const r = node.getBoundingClientRect();
    return {top: r.top, bottom: r.bottom, left: r.left, right: r.right, height: r.height};
  };
  const row = document.getElementById('link-kinds');
  const buttons = [...row.querySelectorAll('button')].map((b) => ({
    id: b.id, text: b.firstChild ? b.firstChild.textContent : '', pressed: b.getAttribute('aria-pressed'),
    struck: getComputedStyle(b).textDecorationLine.includes('line-through'), box: box(b)}));
  const links = [...document.querySelectorAll('#discord-log > li:not(.search-hidden) .row-link')]
    .filter((a) => a.getClientRects().length)
    .map((a) => {
      const lines = new Set([...a.getClientRects()].map((r) => Math.round(r.top))).size;
      const range = document.createRange();
      range.selectNodeContents(a);
      const textLines = new Set([...range.getClientRects()].map((r) => Math.round(r.top))).size;
      // Each place the text goes onto a new line inside one of its parts, rather than between the parts
      // the page offers breaks between (after a slash and the like) or after a hyphen, as the part and
      // the character before the break.
      const split = [];
      for (const node of [...a.childNodes].filter((one) => one.nodeType === 3)) {
        let top = null;
        for (let i = 0; i < node.data.length; i += 1) {
          const one = document.createRange();
          one.setStart(node, i);
          one.setEnd(node, i + 1);
          const rect = one.getClientRects()[0];
          if (!rect) continue;
          if (top !== null && Math.round(rect.top) > top && node.data[i - 1] !== '-') split.push(`${node.data} at ${i}`);
          top = Math.round(rect.top);
        }
      }
      return {text: a.textContent, href: a.getAttribute('href'), box: box(a), row: box(a.closest('li')), lines: textLines, split,
              scrollWidth: a.scrollWidth, clientWidth: a.clientWidth, raw: a.parentNode.hasAttribute('data-raw')};
    });
  return {
    hidden: row.hidden, row: box(row), buttons, links,
    overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    stored: localStorage.getItem('%s'),
  };
}""" % STORAGE_KEY

# Where the chip row is against the list's visible top edge, which clips every row: a list whose top
# is above the row's foot has rows drawn under the row, where they show between and around its pills
# (each button is a 44px target, clear around the pill drawn inside it, so no hit test sees this).
UNDER_ROW_JS = """() => {
  const row = document.getElementById('link-kinds').getBoundingClientRect();
  const area = document.getElementById('scroll-area').getBoundingClientRect();
  return {rowBottom: row.bottom, listTop: area.top, scrolled: document.getElementById('scroll-area').scrollTop};
}"""

# Scroll the list so that the first shown unmatched line stands under the chip row.
SCROLL_GAP_UNDER_ROW_JS = """() => {
  const area = document.getElementById('scroll-area');
  const row = document.getElementById('link-kinds').getBoundingClientRect();
  const seam = [...document.querySelectorAll('#discord-log > li')]
    .find((li) => li.getClientRects().length && /unmatched/.test(li.textContent) && li.getBoundingClientRect().top > row.bottom);
  if (!seam) return null;
  const s = seam.getBoundingClientRect();
  area.scrollTop += (s.top + s.height / 2) - (row.top + row.height / 2);
  const now = seam.getBoundingClientRect();
  return {seam: {top: now.top, bottom: now.bottom}, row: {top: row.top, bottom: row.bottom}, text: seam.textContent.trim()};
}"""


def settled(page: Page) -> None:
    page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")


def until(page: Page, script: str, why: str, seconds: float = 10) -> None:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline and not page.evaluate(script):
        page.wait_for_timeout(25)
    check(bool(page.evaluate(script)), why)


def press(page: Page, selector: str, touch: bool) -> None:
    box = page.locator(selector).bounding_box()
    check(box is not None, f"{selector} is not on the screen")
    if box is None:
        return
    x, y = box["x"] + box["width"] / 2, box["y"] + box["height"] / 2
    if touch:
        page.touchscreen.tap(x, y)
    else:
        page.mouse.click(x, y)
    settled(page)


def shot(page: Page, shots: Path | None, name: str) -> None:
    if shots is not None:
        page.screenshot(path=str(shots / f"{name}.png"))


def open_links(page: Page, label: str, touch: bool) -> None:
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    if page.evaluate("() => document.getElementById('discord-log').getClientRects().length === 0"):
        page.click("#view-switch")
    until(page, "() => document.querySelectorAll('#discord-log > li[data-id]').length === 20",
          f"{label}: the channel did not open with its twenty messages")
    settled(page)
    if page.evaluate("() => document.getElementById('search-toggle').getAttribute('aria-pressed') !== 'true'"):
        press(page, "#search-toggle", touch)
    page.wait_for_selector("#links-filter", state="visible", timeout=10_000)
    if page.evaluate("() => document.getElementById('links-filter').getAttribute('aria-pressed') !== 'true'"):
        press(page, "#links-filter", touch)
    until(page, "() => !document.getElementById('link-kinds').hidden",
          f"{label}: Links came on without its row of chips")
    settled(page)


def links_written(state: LinksState, label: str, raw: bool, stage: str) -> None:
    """Every link shown is written as its address (`raw`) or as its label, and fits the list."""
    want = {href: (href if raw else name) for href, name, _ in LINKS}
    shown = state["links"]
    check(len(shown) > 0, f"{label} {stage}: no link is shown")
    for link in shown:
        check(link["href"] in want, f"{label} {stage}: a link to an address the channel never held: {link}")
        if link["href"] not in want:
            continue
        check(link["text"] == want[link["href"]],
              f"{label} {stage}: {link['href']} is written {link['text']!r}, not {want[link['href']]!r}")
        check(link["raw"] == raw, f"{label} {stage}: the list's data-raw is {link['raw']}, URL is {raw}")
        box, row = link["box"], link["row"]
        if box is not None and row is not None:
            check(box["left"] >= row["left"] - 1 and box["right"] <= row["right"] + 1,
                  f"{label} {stage}: {link['href']} runs out of its row: {box} vs {row}")
        if raw:
            check(link["scrollWidth"] <= link["clientWidth"] + 1,
                  f"{label} {stage}: {link['href']} is cut off rather than wrapped")
            check(not link["split"], f"{label} {stage}: {link['href']} breaks inside {link['split']}")
    check(state["overflow"] <= 0, f"{label} {stage}: the page is {state['overflow']}px wider than the window")


def walk(page: Page, label: str, touch: bool, shots: Path | None, width: int) -> str:
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    open_links(page, label, touch)

    state = page.evaluate(STATE_JS)
    buttons = state["buttons"]
    check([b["text"] for b in buttons] == ["URL", "PRs", "Commits", "Actions", "Issues", "Files", "Other"],
          f"{label}: the chips are not URL and then the six kinds: {[b['text'] for b in buttons]}")
    url = buttons[0]
    check(url["id"] == "link-raw" and url["pressed"] == "false", f"{label}: URL is not first and off: {url}")
    check(not url["struck"], f"{label}: URL off is struck through, as a hidden kind is")
    check(url["box"] is not None and url["box"]["left"] >= 0 and url["box"]["right"] <= width,
          f"{label}: URL is not wholly on the screen: {url['box']}")
    check(state["stored"] is None, f"{label}: a choice was kept before any was made: {state['stored']!r}")
    links_written(state, label, False, "labels")
    shot(page, shots, f"{label}-link-url-chip-labels")

    # `#620`: the row's band is the row's own; nothing scrolled under it shows between the pills.
    placed = page.evaluate(SCROLL_GAP_UNDER_ROW_JS)
    settled(page)
    check(placed is not None, f"{label}: no unmatched line to scroll under the row")
    under = page.evaluate(UNDER_ROW_JS)
    check(under["scrolled"] > 0, f"{label}: the list did not scroll under the row: {under}")
    check(under["listTop"] >= under["rowBottom"] - 0.5,
          f"{label}: the list starts above the chip row's foot, so rows show through it: {under}")
    shot(page, shots, f"{label}-link-url-chip-row-over-gap")
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
    settled(page)

    press(page, "#link-raw", touch)
    until(page, "() => document.getElementById('link-raw').getAttribute('aria-pressed') === 'true'",
          f"{label}: pressing URL did not turn it on")
    state = page.evaluate(STATE_JS)
    links_written(state, label, True, "addresses")
    check(state["stored"] == "on", f"{label}: URL on was not kept: {state['stored']!r}")
    longest = next((link for link in state["links"] if link["href"] == LONG), None)
    check(longest is not None and (not touch or longest["lines"] > 1),
          f"{label}: the long address is not wrapped onto more lines on the phone: {longest}")
    shot(page, shots, f"{label}-link-url-chip-addresses")

    page.reload(wait_until="load")
    open_links(page, label, touch)
    state = page.evaluate(STATE_JS)
    check(state["buttons"][0]["pressed"] == "true", f"{label}: URL did not outlast a reload")
    links_written(state, label, True, "after the reload")

    # A word only an address holds still finds its message, and URL only changes how it is written.
    page.fill("#search-field", "vibe-talk")
    until(page, "() => [...document.querySelectorAll('#discord-log > li:not(.search-hidden) .row-link')]"
                ".filter((a) => a.getClientRects().length).length === 1",
          f"{label}: 'vibe-talk' did not leave the one message whose address holds it")
    state = page.evaluate(STATE_JS)
    check([link["text"] for link in state["links"]] == [LINKS[0][0]],
          f"{label}: the search under URL shows {[link['text'] for link in state['links']]}")
    shot(page, shots, f"{label}-link-url-chip-searched")
    press(page, "#link-raw", touch)
    until(page, "() => document.getElementById('link-raw').getAttribute('aria-pressed') === 'false'",
          f"{label}: pressing URL again did not turn it off")
    state = page.evaluate(STATE_JS)
    check([link["text"] for link in state["links"]] == [LINKS[0][1]],
          f"{label}: URL off under the search shows {[link['text'] for link in state['links']]}")
    check(state["stored"] is None, f"{label}: URL off was not forgotten: {state['stored']!r}")
    return f"{len(LINKS)} links, labels and addresses, kept over a reload, nothing under the row"


def launch(playwright: Playwright, browser: str | None, profile: str, width: int, height: int,
           touch: bool) -> BrowserContext:
    if touch:
        return playwright.chromium.launch_persistent_context(
            profile, headless=True, executable_path=browser, timezone_id=week.ZONE,
            viewport={"width": width, "height": height}, color_scheme="dark",
            device_scale_factor=2.625, is_mobile=True, has_touch=True,
        )
    return playwright.chromium.launch_persistent_context(
        profile, headless=True, executable_path=browser, timezone_id=week.ZONE,
        viewport={"width": width, "height": height}, color_scheme="dark",
    )


def main() -> int:
    args = week.arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    web_root = args.web_root.resolve()
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch in SIZES:
            api = LinkApi()
            server, thread, url = serve(api, web_root)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-link-url-chip-") as profile:
                    context = launch(playwright, args.browser_executable, profile, width, height, touch)
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(url, wait_until="load")
                    note = walk(page, label, touch, args.screenshots, width)
                    check(not errors, f"{label}: the page threw: {errors}")
                    version = context.browser.version if context.browser else "Chromium"
                    context.close()
                    report.append(f"{version} {label} {width}x{height}: {note}")
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
