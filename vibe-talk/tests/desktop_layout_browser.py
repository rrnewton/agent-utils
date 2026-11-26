#!/usr/bin/env python3
"""The head of Settings, and two columns on a wide desk, laid out in real Chromium.

`#372 settings-header-layout`. The owner, 2026-10-19, of Settings on a desktop window: "the machine URL
doesn't need to be in a big font!! Expand all certainly doesn't need its own line." The server's host
was the loudest line on the screen, in large bold type wrapping over two lines in a card of its own;
Expand all stood alone on a row above the sections and Open help alone on a row below them.

`#373 two-column-full-width`. The owner, the same day: "I asked a while ago for full-width messages
from the coding agent in two-column mode. Why don't I have this? In this screenshot it shows wasted
space." On a 2000px desk in two columns the agent's long reply stopped about 160px short of the right
column, in a strip holding only the floating search glass, and the right column's empty thread list
was held to the same narrow measure.

The fake-DOM suite (tests/js/voice_page.test.mjs) pins the markup and the rules; it lays nothing out,
so it cannot say where anything lands. This does. It serves the checked-out page and the small fake
API of tests/offline_cache_browser.py from one loopback origin, opened under a long host name (any
`*.localhost` name is loopback to Chromium), so the host really is too long for a phone's line.

  Settings, at 1280x800 and 1920x1080 with a mouse and at a 393x852 phone by touch: the host is one
  line of ordinary weight no larger than the page's text, whole in its tooltip, its name cut with an
  ellipsis where it does not fit (on the phone it does not) and its port whole after it, and selected
  whole it copies whole -> `#2297 vibe-talk-settings-header-layout`: Expand all, Welcome and Open help
  are one row under the host's line, each the same width and the row spanning the column to within a
  pixel, on the phone and the desks alike; up to date, the head says nothing about the build; all of it
  above the sections, nothing wider than the screen -> Tab from Back goes to Expand all, Welcome, Open
  help, then the first section, and the document order is the same -> Expand all still opens
  everything and reads Fold all, on the same filled row. And "Channel Settings: [channel] ?" is one line,
  the ? on the select's line at the row's end (on the phone it used to drop to a line of its own).

  `#2297` again, at the phone and 1280x800, against a fake API whose timeline reads name a newer page
  build, as a deploy's do: the server line ends in the "Update available · Reload" chip, on the host's
  line, at most 25 characters, both builds in its tooltip; the long host is cut with an ellipsis before
  it and its port stays whole; the buttons still fill the column -> a tap on the chip reloads the page.

  Two columns, at 1920x1080 and 2560x1080 with a mouse, over a channel ending in the agent's long boxed
  replies and the owner's answer, with the thread list empty: every row of Main but the owner's spans
  its column to within the list's own gutter, the agent's tiles wider than the reading width a single
  column keeps, and the owner's rows keep their partial width, set in from the left (the owner,
  correcting the first version: "MY messages are still partial width to make them visually distinct")
  -> the glass sits at the left column's top right, by the divider, over
  no row, and no strip as wide as it is left between the rows and the divider -> the right column's
  heading and its "No threads" line span their column -> one column at the same desk still holds the
  channel to its reading width. And at 1920x1080 over a channel with threads, a thread selected on the
  right: its rows span the right column as Main's span the left.

No horizontal overflow and no page error at any step. Pass --screenshots DIR to keep a PNG of each
state for review; --web-root DIR to run the same walk against another copy of the page (an older one,
to see these checks fail before the change).
"""

from __future__ import annotations

import argparse
import os
import re
import tempfile
import threading
import time
from collections.abc import Callable
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import OWNER_ID, TOKEN, FakeApi, Json, check, handler_for, said
import page_sources
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

# `#2297 vibe-talk-settings-header-layout`: the page build a deploy's timeline read would name.
NEWER_PAGE_BUILD = "19fe934a" + "0" * 24

# A host too long for a phone's line, and loopback to Chromium as every `*.localhost` name is.
LONG_HOST = "build-host-0042-rack7-8422--dev-proxy.region-two.example.localhost"

# (label, width, height, touch)
SETTINGS_SIZES = (("laptop-1280", 1280, 800, False), ("desk-1920", 1920, 1080, False), ("phone-393", 393, 852, True))
UPDATE_SIZES = (("laptop-1280", 1280, 800, False), ("phone-393", 393, 852, True))
COLUMN_SIZES = (("desk-1920", 1920, 1080), ("wide-2560", 2560, 1080))
THREAD_SIZE = ("desk-1920", 1920, 1080)

# How much narrower than its column a row other than the owner's may be in two columns: the list's
# gutter inside each edge (0.75rem), and a pixel of rounding. Such a row stops at the gutter alone.
ROW_SLACK_PX = 2 * 12 + 1
# ...and a thread on the right stands 2.5rem further in, level with the card at its top, so the line from
# its root on Main has a gutter of its own beside the rows (`#385 thread-line-overlap`).
THREAD_GUTTER_PX = 40
AGENT_SLACK_PX = 12 + 1
# ...and how far in from the left the owner's own rows stand, at least: they keep their partial width
# (the desk's inset, 6rem in a half of a desk), so they read as the owner's beside the others.
OWNER_INSET_MIN_PX = 64

LONG_REPLY = "\n".join([
    "Status",
    "",
    "- Post-land full validation of the release candidate on this host: 10,387 of 10,392 passed. All five "
    "failures look like flakes or load: a fifo test (two failures in eighty retained runs), a core-capture "
    "cap test (one in 649), a known inherited-terminal hang, and two timeouts at the 57 s limit. None of them "
    "touches the change, and each is tracked.",
    "- The data volume is at 94% even after twenty gigabytes of stale test temporaries were deleted; a large "
    "validation checkout frees itself when it is done, and the scratch area is large and untouched.",
    "",
    "In flight",
    "",
    "- Signal continuations under record: in dual review, and its version bump will be redone at land.",
    "- Caching the 4.7 s capability check: in review.",
    "- The crash evaluation: a scheduler crash memory dump, and the emulator's instruction-budget patch.",
    "",
    "Needs you",
    "",
    "- A larger machine for the v3 test, or a go-ahead on the counter-based fallback that would let it run "
    "here instead, which is slower but needs nothing from you at all.",
])


class LongReplyApi(FakeApi):
    """A short exchange, then the agent's long boxed replies and the owner's answer, and no threads."""

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope), "owner_author_id": OWNER_ID}

    def __init__(self) -> None:
        super().__init__()
        self.messages = [said(index, f"Short note {index} from the channel.", "human" if index % 2 else "coder")
                         for index in range(5)]
        self.messages.append(said(5, LONG_REPLY, "coder"))
        self.messages.append(said(6, "Thanks, go ahead with the fallback.", "me"))
        self.messages.append(said(7, LONG_REPLY, "coder"))
        self.threads = []


class NewerBuildApi(LongReplyApi):
    """The same channel, read from a server that now serves a newer page build than the page runs."""

    def timeline(self, query: dict[str, list[str]]) -> Json:
        return {**super().timeline(query), "page_build": NEWER_PAGE_BUILD}


HEAD_JS = """() => {
    const box = (id) => { const r = document.getElementById(id).getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height}; };
    const host = document.getElementById('settings-server-host');
    const name = host.querySelector('.settings-server-name');
    const port = host.querySelector('.settings-server-port');
    const style = getComputedStyle(host);
    const range = document.createRange();
    range.selectNodeContents(host);
    const selection = getSelection();
    selection.removeAllRanges();
    selection.addRange(range);
    const copied = selection.toString();
    // Why a copy came out wrong, said with it (#998).
    const copyState = copied === location.host ? null : {
        ranges: selection.rangeCount, type: selection.type, innerText: host.innerText,
        rects: host.getClientRects().length, height: host.offsetHeight, width: host.getBoundingClientRect().width,
        nameWidth: name.getBoundingClientRect().width, portWidth: port.getBoundingClientRect().width,
        visible: host.checkVisibility({visibilityProperty: true, opacityProperty: true, contentVisibilityAuto: true}),
        focused: document.hasFocus(), active: document.activeElement ? document.activeElement.id : null,
        animations: document.getAnimations().length, userSelect: getComputedStyle(host).userSelect,
    };
    selection.removeAllRanges();
    const order = ['settings-server-host', 'fold-settings-all', 'open-welcome', 'open-help', 'settings-panes']
        .map((id) => document.getElementById(id));
    const settings = document.getElementById('screen-settings');
    return {
        host: box('settings-server-host'), expand: box('fold-settings-all'), help: box('open-help'),
        welcome: box('open-welcome'), server: box('settings-server'), chip: box('settings-page-update'),
        chipShown: document.getElementById('settings-page-update').checkVisibility(),
        chipText: document.getElementById('settings-page-update').textContent.trim(),
        chipTitle: document.getElementById('settings-page-update').title,
        headText: document.querySelector('#screen-settings .settings-head').innerText,
        panes: box('settings-panes'), column: box('screen-settings'),
        hostSize: parseFloat(style.fontSize), hostWeight: Number(style.fontWeight),
        hostLine: parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.25,
        textSize: parseFloat(getComputedStyle(document.body).fontSize),
        ellipsis: getComputedStyle(name).textOverflow === 'ellipsis' && style.whiteSpace === 'nowrap',
        cut: name.scrollWidth > name.clientWidth + 1,
        port: port.textContent, portWhole: port.scrollWidth <= port.clientWidth + 1
            && port.getBoundingClientRect().right <= host.getBoundingClientRect().right + 1,
        text: host.textContent, title: host.title, location: location.host, copied, copyState,
        inOrder: order.every((node, i) => i === 0
            || Boolean(order[i - 1].compareDocumentPosition(node) & Node.DOCUMENT_POSITION_FOLLOWING)),
        overflow: settings.scrollWidth - settings.clientWidth,
        pageOverflow: document.documentElement.scrollWidth - innerWidth,
        expandText: document.getElementById('fold-settings-all').textContent.trim(),
    };
}"""

COLUMNS_JS = """() => {
    const box = (node) => { const r = node.getBoundingClientRect();
        return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height}; };
    const byId = (id) => box(document.getElementById(id));
    const rows = (list) => [...document.querySelectorAll(`#${list} > li.discord-message`)]
        .filter((row) => row.getClientRects().length > 0)
        .map((row) => ({id: row.getAttribute('data-id'), who: row.getAttribute('data-who'), ...box(row)}));
    const side = document.getElementById('side-column');
    return {
        drawn: side.getClientRects().length > 0,
        left: byId('scroll-area'), right: side.getClientRects().length > 0 ? box(side) : null,
        pane: byId('pane-discord'), glass: byId('search-toggle'),
        heading: byId('side-heading'), state: byId('side-list-state'),
        stateText: document.getElementById('side-list-state').textContent.trim(),
        main: rows('discord-log'), thread: rows('side-log'),
        paneCap: getComputedStyle(document.getElementById('pane-discord')).maxWidth,
        readingWidth: (() => { const probe = document.createElement('div');
            probe.style.width = 'var(--reading-width)'; probe.style.position = 'absolute';
            document.getElementById('pane-discord').append(probe);
            const width = probe.getBoundingClientRect().width; probe.remove(); return width; })(),
        overflow: document.documentElement.scrollWidth - innerWidth,
    };
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
    report: list[str] = []
    with sync_playwright() as playwright:
        # The page build, served as the server serves it, so the page knows its build as production's does
        # (`#676 settings-head-page-build`: the head no longer grows a row for it, `#2297`).
        fixture.WEB_ROOT = page_sources.served(args.web_root)
        for label, width, height, touch in SETTINGS_SIZES:
            report.append(run(playwright.chromium, args, LongReplyApi(), label, width, height, touch, settings_walk))
        for label, width, height, touch in UPDATE_SIZES:
            report.append(run(playwright.chromium, args, NewerBuildApi(), label, width, height, touch, settings_update_walk))
        for label, width, height in COLUMN_SIZES:
            report.append(run(playwright.chromium, args, LongReplyApi(), label, width, height, False, columns_walk))
        from two_column_browser import ColumnsApi

        label, width, height = THREAD_SIZE
        report.append(run(playwright.chromium, args, ColumnsApi(), label, width, height, False, thread_walk))
    for line in report:
        print(line)
    return 0


class Desk:
    """The page at one size, and what the walk has found there."""

    def __init__(self, page: Page, screenshots: Path | None, label: str, width: int, height: int) -> None:
        self.page = page
        self.screenshots = screenshots
        self.label = label
        self.width = width
        self.height = height
        self.notes: list[str] = []

    def shot(self, name: str) -> None:
        if self.screenshots:
            self.page.screenshot(path=str(self.screenshots / f"{name}-{self.label}.png"))

    def settled(self) -> None:
        """Two animation frames and nothing more (`#1118 browser-fixed-frame-settle`).

        Only for a frame to paint after a `wait_for` has pinned the state, before a geometry or
        screenshot read, where nothing is asserted on the next render, or where frames passing is
        itself the point. Never the only wait before reading what a render draws: `wait_for` that
        thing instead."""
        self.page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")

    def wait_for(self, condition: str, why: str, seconds: float = 10) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and not self.page.evaluate(condition):
            self.page.wait_for_timeout(25)
        check(bool(self.page.evaluate(condition)), f"{self.label}: {why}")

    def check(self, condition: bool, why: str) -> None:
        check(condition, f"{self.label}: {why}")

    def signed_in(self) -> None:
        self.page.fill("#api-token", TOKEN)
        self.page.click("#save-token")
        try:
            self.page.wait_for_function("() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden", timeout=3_000)
            if self.page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
                self.page.click("#welcome-start")
        except Exception:
            pass  # no first-run Welcome on this path (#1781: a returning reader, a refusal, or a saved choice)
        self.page.wait_for_selector("#open-settings", state="visible", timeout=10_000)
        self.settled()


def run(chromium: BrowserType, args: argparse.Namespace, api: FakeApi, label: str, width: int, height: int,
        touch: bool, walk: Callable[[Desk], None]) -> str:
    """One reader at one size, against a fresh fake API and a fresh browser profile."""
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-layout-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark",
                has_touch=touch, is_mobile=touch,
            )
            browser_waits.hot_host_timeouts(context)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, label, width, height)
            page.goto(f"http://{LONG_HOST}:{server.server_port}/", wait_until="load")
            desk.signed_in()
            walk(desk)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}, {walk.__name__}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def mid(box: dict[str, float]) -> float:
    return (box["top"] + box["bottom"]) / 2


def open_settings_head(desk: Desk) -> None:
    """Settings opened, its head laid out."""
    desk.page.click("#open-settings")
    desk.wait_for("() => document.getElementById('screen-settings').checkVisibility()", "Settings did not open")
    desk.wait_for("() => { const h = document.getElementById('settings-server-host'); "
                  "return h && h.textContent !== '' && h.getBoundingClientRect().width > 0 "
                  "&& document.getElementById('fold-settings-all').getBoundingClientRect().width > 0 "
                  "&& document.getElementById('open-help').getBoundingClientRect().width > 0 "
                  "&& document.getElementById('settings-panes').getBoundingClientRect().width > 0; }",
                  "the Settings head did not lay out")
    desk.settled()


# `#2297`: where the button row stands up to date, by size, for the update walk to hold it to.
UP_TO_DATE_ROW_TOP: dict[str, float] = {}


def check_buttons_fill(desk: Desk, when: str) -> None:
    """`#2297 vibe-talk-settings-header-layout`: Expand all (or Fold all), Welcome and Open help, one row of
    equal widths under the host's line, spanning the column (the panes' width) to within a pixel."""
    head = desk.page.evaluate(HEAD_JS)
    expand, welcome, helps, host, panes = (head[k] for k in ("expand", "welcome", "help", "host", "panes"))
    widths = [b["width"] for b in (expand, welcome, helps)]
    desk.check(max(widths) - min(widths) <= 1, f"{when}: the three buttons are not one width: {widths}")
    desk.check(abs(expand["left"] - panes["left"]) <= 1 and abs(helps["right"] - panes["right"]) <= 1,
               f"{when}: the button row spans {expand['left']:.1f}-{helps['right']:.1f}, "
               f"not the column's {panes['left']:.1f}-{panes['right']:.1f}")
    desk.check(all(abs(mid(b) - mid(expand)) < 1 for b in (welcome, helps))
               and expand["right"] <= welcome["left"] and welcome["right"] <= helps["left"],
               f"{when}: Expand all, Welcome and Open help are not one row, in that order: {expand} {welcome} {helps}")
    desk.check(host["bottom"] <= expand["top"] + 1, f"{when}: the buttons are not under the host's line: {host} {expand}")
    desk.check(helps["bottom"] <= panes["top"], f"{when}: Open help is not above the sections: {helps} {panes}")


CHANNEL_ROW_JS = """() => {
    const box = (id) => document.getElementById(id).getBoundingClientRect();
    const title = box('settings-channels-title'), select = box('settings-channel'), help = box('help-link-channel-alias');
    const section = box('settings-channels');
    const mid = (b) => (b.top + b.bottom) / 2;
    return {title: [title.left, title.right, mid(title)], select: [select.left, select.right, select.top, select.bottom],
        help: [help.left, help.right, mid(help)], section: [section.left, section.right],
        oneLine: mid(help) > select.top && mid(help) < select.bottom && mid(title) > select.top && mid(title) < select.bottom
            && title.right <= select.left && select.right <= help.left && help.right <= section.right};
}"""


def check_channel_row(desk: Desk) -> None:
    """`#2297 vibe-talk-settings-header-layout`: "Channel Settings: [channel] ?" is one line at every size,
    the `?` on the select's line at the row's end (on a phone it used to drop to a line of its own)."""
    row = desk.page.evaluate(CHANNEL_ROW_JS)
    desk.check(bool(row["oneLine"]), f"the Channel Settings row is not one line, title, select, then ?: {row}")


def settings_walk(desk: Desk) -> None:
    """`#372 settings-header-layout` and `#2297`: the head of Settings is two quiet lines, the server's
    and the buttons filling the column."""
    page = desk.page
    open_settings_head(desk)
    head = page.evaluate(HEAD_JS)
    desk.shot("settings")
    host, expand, helps, panes = head["host"], head["expand"], head["help"], head["panes"]
    phone = desk.width < 900

    # The host: quiet, one line, whole in its tooltip and when copied.
    desk.check(head["text"] == head["location"] == head["title"],
               f"the host line does not say the host whole in its text and tooltip: {head['text']!r} {head['title']!r}")
    desk.check(head["hostSize"] <= head["textSize"] + 0.01,
               f"the host is drawn at {head['hostSize']}px, larger than the page's {head['textSize']}px text")
    desk.check(head["hostWeight"] <= 400, f"the host is drawn in weight {head['hostWeight']}")
    desk.check(host["height"] < 1.5 * head["hostLine"], f"the host wraps: {host['height']:.0f}px tall at a {head['hostLine']:.0f}px line")
    desk.check(bool(head["ellipsis"]), "a host name too long for its line is not cut with an ellipsis")
    if phone:
        desk.check(bool(head["cut"]), "on a phone the long host was expected not to fit its line")
    desk.check(head["port"] == ":" + head["location"].rsplit(":", 1)[1] and bool(head["portWhole"]),
               f"the port is not shown whole after the name: {head['port']!r}")
    desk.check(head["copied"] == head["location"], f"the host selected whole copies {head['copied']!r} ({head['copyState']})")
    desk.check(host["right"] <= desk.width and host["left"] >= 0, f"the host runs off the screen: {host}")

    # The buttons: one row of equal widths under the host's line, filling the column, phone and desk alike.
    check_buttons_fill(desk, "up to date")
    UP_TO_DATE_ROW_TOP[desk.label] = float(expand["top"])
    desk.check(abs(head["server"]["height"] - 32) <= 0.5, f"the server line is {head['server']['height']}px, not the chip's 2rem")
    check_channel_row(desk)
    # Up to date, and the page knowing its build as production's does: nothing about the build in the head.
    desk.check(not head["chipShown"] and not re.search(r"page is build|now serves|update available", str(head["headText"]), re.I),
               f"up to date, the head says something about the build: {head['headText']!r}")
    rows = 2
    head_height = max(expand["bottom"], helps["bottom"]) - host["top"]
    desk.check(head_height < rows * 52, f"the head takes {head_height:.0f}px, more than {rows} row(s)")
    desk.check(head["overflow"] <= 1 and head["pageOverflow"] <= 0, "something on Settings is wider than the screen")
    desk.check(bool(head["inOrder"]), "the document order is not the host, Expand all, Welcome, Open help, the sections")
    widths = "/".join(f"{head[k]['width']:.1f}" for k in ("expand", "welcome", "help"))
    desk.notes.append(f"host {head['hostSize']:.1f}px weight {head['hostWeight']} on one line"
                      f"{', cut' if head['cut'] else ''}; buttons {widths}px across the {panes['width']:.1f}px column; "
                      f"the head {head_height:.0f}px in {rows} rows")

    # The keys: Tab from Back goes to Expand all, Welcome, Open help, then the first section. Into the screen, that
    # is: the transient status line's X, when it is up, stands between the title bar and the screen.
    page.focus("#close-settings")
    walked: list[str] = []
    for _ in range(6):
        page.keyboard.press("Tab")
        focused = page.evaluate("() => document.activeElement && document.activeElement.closest('#screen-settings')"
                                " ? document.activeElement.id : null")
        if focused:
            walked.append(str(focused))
        if len(walked) == 3:
            break
    desk.check(walked == ["fold-settings-all", "open-welcome", "open-help"], f"Tab from Back went to {walked}")
    desk.notes.append("Tab: Back, Expand all, Welcome, Open help, Voice agent")

    # Expand all still opens everything, and its label's change keeps the row.
    page.click("#fold-settings-all")
    desk.wait_for("() => document.getElementById('fold-settings-all').textContent.trim() === 'Fold all'",
                  "Expand all did not open everything")
    page.evaluate("() => { document.getElementById('screen-settings').scrollTop = 0; }")
    desk.settled()
    opened = page.evaluate(HEAD_JS)
    check_buttons_fill(desk, "everything open")
    desk.check(opened["overflow"] <= 1, "something on Settings opened is wider than the screen")
    desk.shot("settings-expanded")
    desk.notes.append("Expand all read Fold all on the same filled row")


def settings_update_walk(desk: Desk) -> None:
    """`#2297 vibe-talk-settings-header-layout`: after a deploy, the head says so in a short chip on the
    server's line, the long host cut cleanly before it, and a tap on it reloads the page."""
    page = desk.page
    open_settings_head(desk)
    desk.wait_for("() => document.getElementById('settings-page-update').checkVisibility()",
                  "a read naming a newer page build did not put the update chip up")
    desk.settled()
    head = page.evaluate(HEAD_JS)
    desk.shot("settings-update")
    host, chip, server, panes = (head[k] for k in ("host", "chip", "server", "panes"))
    text = str(head["chipText"])
    desk.check(text == "Update available · Reload" and len(text) <= 25, f"the update chip says {text!r}")
    desk.check(chip["left"] >= host["right"] - 0.5 and chip["top"] < host["bottom"] and chip["bottom"] > host["top"],
               f"the chip is not on the host's line, after it: {host} {chip}")
    desk.check(chip["right"] <= panes["right"] + 1, f"the chip runs past the column: {chip} {panes}")
    # Right after the host it is about, at the line's gap (0.5rem), not at the line's far end (ui-rules R1).
    desk.check(4 <= chip["left"] - host["right"] <= 12, f"the chip is not right after the host: {host} {chip}")
    # Rule 370: the chip coming up moved nothing below the server line.
    before = UP_TO_DATE_ROW_TOP.get(desk.label)
    desk.check(before is not None and abs(head["expand"]["top"] - before) <= 0.5,
               f"the chip moved the button row: {before} -> {head['expand']['top']}")
    desk.check(server["height"] <= chip["height"] + 2, f"the server line wraps: {server['height']:.0f}px tall, the chip {chip['height']:.0f}px")
    title = str(head["chipTitle"])
    desk.check(bool(re.fullmatch(r"This page is build [0-9a-f]{8}; the server now serves 19fe934a\. Tap to reload\.", title)),
               f"the chip's tooltip does not name both builds: {title!r}")
    desk.check(bool(head["ellipsis"]) and bool(head["portWhole"]), "the host is not cut cleanly: an ellipsis, then its port whole")
    if desk.width < 900:
        desk.check(bool(head["cut"]), "on a phone the long host was expected to be cut before the chip")
    check_buttons_fill(desk, "update available")
    check_channel_row(desk)
    desk.check(head["overflow"] <= 1 and head["pageOverflow"] <= 0, "something on Settings is wider than the screen")
    # A tap reloads: a new document, with the marker this one carried gone.
    page.evaluate("() => { window.__beforeReload = true; }")
    page.click("#settings-page-update")
    desk.wait_for("() => document.readyState === 'complete' && window.__beforeReload === undefined",
                  "a tap on the update chip did not reload the page", 20)
    desk.notes.append(f"chip {text!r} {chip['width']:.0f}px on the host's line{', host cut' if head['cut'] else ''}; "
                      "the buttons still fill the column; a tap reloaded")


def two_columns(desk: Desk, rows: int) -> None:
    """Open the channel in All, then two columns."""
    page = desk.page
    # `#1781`: sign-in lands on the channel already.

    desk.wait_for(f"() => document.querySelectorAll('#discord-log > li[data-id]').length >= {rows}",
                  "the channel did not open")
    page.click("#column-toggle")
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length > 0",
                  "the layout button did not draw two columns")
    desk.settled()


def columns_walk(desk: Desk) -> None:
    """`#373 two-column-full-width`: Main's rows, the agent's long tiles first, span the left column."""
    page = desk.page
    two_columns(desk, 8)
    desk.wait_for("() => document.getElementById('side-list-state').textContent.trim() !== ''",
                  "the empty thread list says nothing")
    # The agent's replies opened whole, so the boxes are the long ones the owner photographed.
    # `#820 voice-page-action-menu`: Expand all is an item of the action menu now, opened for it.
    if page.evaluate("() => !document.getElementById('expand-all').hidden"):
        page.click("#action-menu-open")
        page.click("#expand-all")
    page.evaluate("() => { const s = document.getElementById('scroll-area'); s.scrollTop = s.scrollHeight; }")
    desk.wait_for("() => { const rows = [...document.querySelectorAll('#discord-log > li.discord-message')]"
                  ".filter((row) => row.getClientRects().length > 0); "
                  "return rows.length >= 3 && rows.some((row) => row.getAttribute('data-who') === 'coder') "
                  "&& rows.some((row) => row.getAttribute('data-who') === 'me'); }",
                  "too few of Main's rows are on screen to measure")
    desk.settled()
    found = page.evaluate(COLUMNS_JS)
    left, right = found["left"], found["right"]
    desk.check(right is not None, "the right column is not drawn")
    divider = right["left"]
    column = divider - left["left"]
    main = found["main"]
    desk.check(len(main) >= 3, f"too few of Main's rows are on screen to measure: {main}")
    others = [row for row in main if row["who"] != "me"]
    short = [row for row in others if row["width"] < column - ROW_SLACK_PX or row["right"] < divider - AGENT_SLACK_PX]
    desk.check(not short, f"rows of Main stop short of the {column:.0f}px column: "
               + ", ".join(f"{row['who']} {row['id']} {row['left']:.0f}-{row['right']:.0f}" for row in short))
    # The owner's own rows keep their partial width, set in from the left, their right edge with the others'.
    mine = [row for row in main if row["who"] == "me"]
    desk.check(bool(mine), f"no row of the owner's is on screen: {[row['who'] for row in main]}")
    wide = [row for row in mine if row["left"] - left["left"] < 12 + OWNER_INSET_MIN_PX or row["right"] < divider - AGENT_SLACK_PX]
    desk.check(not wide, "the owner's rows do not keep their partial width: "
               + ", ".join(f"{row['id']} {row['left']:.0f}-{row['right']:.0f}" for row in wide))
    agents = [row for row in main if row["who"] == "coder"]
    desk.check(bool(agents), "no agent row is on screen")
    gap = max(divider - row["right"] for row in agents)
    desk.check(gap <= AGENT_SLACK_PX, f"the agent's tiles stop {gap:.0f}px short of the divider")
    desk.check(min(row["width"] for row in agents) > found["readingWidth"] + 1,
               f"the agent's tiles are no wider than one column's reading width ({found['readingWidth']:.0f}px)")

    # The glass: at the left column's top right, by the divider, floating over the list with no
    # strip kept for it (`#1320 no-freshness-band`: it may stand over the row at the head, as the
    # freshness pill does; what is kept for it is nothing).
    glass = found["glass"]
    desk.check(divider - 24 <= glass["right"] <= divider, f"the glass is not at the divider: {glass} {divider}")
    desk.check(glass["top"] >= left["top"] - 1 and glass["bottom"] > left["top"],
               f"the glass is not floating over the list: {glass} {left}")
    desk.check(gap < glass["width"], "a strip as wide as the glass is left between the rows and the divider")

    # The right column: its heading and its empty list's line span it.
    heading, state = found["heading"], found["state"]
    desk.check(heading["left"] <= divider + 2 and heading["right"] >= desk.width - 2,
               f"the right column's heading does not span it: {heading}")
    desk.check(found["stateText"].startswith("No threads"), f"the empty thread list says {found['stateText']!r}")
    desk.check(state["width"] >= right["width"] - 2 * 12 - 2, f"the empty thread list's line does not span its column: {state}")
    desk.check(found["overflow"] <= 0, "the page scrolls sideways in two columns")
    desk.shot("two-columns")
    desk.notes.append(f"two {column:.0f}px columns: the owner's rows {min(row['width'] for row in mine):.0f}px, "
                      f"the agent's tiles {min(row['width'] for row in agents):.0f}px, "
                      f"{gap:.0f}px from the divider, the glass floating over the list")

    # One column at the same desk keeps its reading width.
    page.click("#column-toggle")
    desk.wait_for("() => document.getElementById('side-column').getClientRects().length === 0",
                  "the layout button did not go back to one column")
    desk.wait_for("() => getComputedStyle(document.getElementById('pane-discord')).maxWidth !== 'none'",
                  "one column did not regain its reading-width cap")
    desk.settled()
    one = page.evaluate(COLUMNS_JS)
    desk.check(one["paneCap"] != "none" and one["pane"]["width"] <= one["readingWidth"] + 1,
               f"one column is no longer held to its reading width: {one['pane']} {one['paneCap']}")
    desk.check(one["overflow"] <= 0, "the page scrolls sideways in one column")
    desk.shot("one-column")
    desk.notes.append(f"one column held to {one['pane']['width']:.0f}px")


def thread_walk(desk: Desk) -> None:
    """`#373 two-column-full-width`: a thread's rows span the right column too."""
    from two_column_browser import RUNNER

    page = desk.page
    two_columns(desk, 11)
    desk.wait_for("() => document.querySelectorAll('#side-cards > li').length === 3", "the threads are not listed")
    page.click(f"#side-cards > li[data-id='{RUNNER}'] .side-card-open")
    desk.wait_for("() => document.querySelectorAll('#side-log > li.discord-message').length >= 3",
                  "the thread did not open on the right")
    desk.wait_for("() => [...document.querySelectorAll('#side-log > li.discord-message')]"
                  ".filter((row) => row.getClientRects().length > 0).length >= 3 "
                  "&& document.getElementById('side-column').getBoundingClientRect().width > 0",
                  "the thread's rows are not drawn on the right")
    desk.settled()
    found = page.evaluate(COLUMNS_JS)
    right = found["right"]
    desk.check(right is not None, "the right column is not drawn")
    rows = found["thread"]
    short = [row for row in rows if row["width"] < right["width"] - ROW_SLACK_PX - THREAD_GUTTER_PX]
    desk.check(not short, f"rows of the thread stop short of the {right['width']:.0f}px column, less the line's gutter: "
               + ", ".join(f"{row['who']} {row['id']} {row['left']:.0f}-{row['right']:.0f}" for row in short))
    desk.check(found["overflow"] <= 0, "the page scrolls sideways with a thread open")
    page.wait_for_timeout(300)  # the thread's 220 ms fade in under its card (`#298 thread-card-header`)
    desk.shot("two-columns-thread")
    desk.notes.append(f"{len(rows)} rows of the thread across the {right['width']:.0f}px right column")


if __name__ == "__main__":
    raise SystemExit(main())
