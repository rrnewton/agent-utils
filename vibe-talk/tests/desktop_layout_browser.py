#!/usr/bin/env python3
"""The head of Settings, laid out in real Chromium.

`#372 settings-header-layout`. The owner, 2026-10-19, of Settings on a desktop window: "the machine URL
doesn't need to be in a big font!! Expand all certainly doesn't need its own line." The server's host
was the loudest line on the screen, in large bold type wrapping over two lines in a card of its own;
Expand all stood alone on a row above the sections and Open help alone on a row below them.

The fake-DOM suite (tests/js/voice_page.test.mjs) pins the markup and the rules; it lays nothing out,
so it cannot say where anything lands. This does. It serves the checked-out page and the small fake
API of tests/offline_cache_browser.py from one loopback origin, opened under a long host name (any
`*.localhost` name is loopback to Chromium), so the host really is too long for a phone's line.

  Settings, at 1280x800 and 1920x1080 with a mouse and at a 393x852 phone by touch: the host is one
  line of ordinary weight no larger than the page's text, whole in its tooltip, its name cut with an
  ellipsis where it does not fit (on the phone it does not) and its port whole after it, and selected
  whole it copies whole -> Expand all
  and Open help share one row, at its end; on a desk that is the host's row, and on the phone the row
  under it; all of it above the sections, nothing wider than the screen -> Tab from Back goes to Expand
  all, Open help, then the first section, and the document order is the same -> Expand all still opens
  everything and reads Fold all, on the same row.

No horizontal overflow and no page error at any step. Pass --screenshots DIR to keep a PNG of each
state for review; --web-root DIR to run the same walk against another copy of the page (an older one,
to see these checks fail before the change).
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
import time
from collections.abc import Callable
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, FakeApi, Json, check, handler_for, said

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

# A host too long for a phone's line, and loopback to Chromium as every `*.localhost` name is.
LONG_HOST = "build-host-0042-rack7-8422--dev-proxy.region-two.example.localhost"

# (label, width, height, touch)
SETTINGS_SIZES = (("laptop-1280", 1280, 800, False), ("desk-1920", 1920, 1080, False), ("phone-393", 393, 852, True))

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
    """A short exchange, then the agent's long boxed replies, and no threads at all."""

    def __init__(self) -> None:
        super().__init__()
        self.messages = [said(index, f"Short note {index} from the channel.", "human" if index % 2 else "coder")
                         for index in range(5)]
        self.messages.append(said(5, LONG_REPLY, "coder"))
        self.messages.append(said(6, "Thanks, go ahead with the fallback.", "me"))
        self.messages.append(said(7, LONG_REPLY, "coder"))
        self.threads = []


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
    selection.removeAllRanges();
    const order = ['settings-server-host', 'fold-settings-all', 'open-help', 'settings-panes']
        .map((id) => document.getElementById(id));
    const settings = document.getElementById('screen-settings');
    return {
        host: box('settings-server-host'), expand: box('fold-settings-all'), help: box('open-help'),
        panes: box('settings-panes'), column: box('screen-settings'),
        hostSize: parseFloat(style.fontSize), hostWeight: Number(style.fontWeight),
        hostLine: parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.25,
        textSize: parseFloat(getComputedStyle(document.body).fontSize),
        ellipsis: getComputedStyle(name).textOverflow === 'ellipsis' && style.whiteSpace === 'nowrap',
        cut: name.scrollWidth > name.clientWidth + 1,
        port: port.textContent, portWhole: port.scrollWidth <= port.clientWidth + 1
            && port.getBoundingClientRect().right <= host.getBoundingClientRect().right + 1,
        text: host.textContent, title: host.title, location: location.host, copied,
        inOrder: order.every((node, i) => i === 0
            || Boolean(order[i - 1].compareDocumentPosition(node) & Node.DOCUMENT_POSITION_FOLLOWING)),
        overflow: settings.scrollWidth - settings.clientWidth,
        pageOverflow: document.documentElement.scrollWidth - innerWidth,
        expandText: document.getElementById('fold-settings-all').textContent.trim(),
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
    fixture.WEB_ROOT = args.web_root.resolve()
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch in SETTINGS_SIZES:
            report.append(run(playwright.chromium, args, LongReplyApi(), label, width, height, touch, settings_walk))
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


def settings_walk(desk: Desk) -> None:
    """`#372 settings-header-layout`: the head of Settings is one quiet row (two on a phone)."""
    page = desk.page
    page.click("#open-settings")
    desk.wait_for("() => document.getElementById('screen-settings').checkVisibility()", "Settings did not open")
    desk.settled()
    head = page.evaluate(HEAD_JS)
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
    desk.check(head["copied"] == head["location"], f"the host selected whole copies {head['copied']!r}")
    desk.check(host["right"] <= desk.width and host["left"] >= 0, f"the host runs off the screen: {host}")

    # The buttons: one row, at its end, beside the host on a desk and under it on a phone.
    desk.check(abs(mid(expand) - mid(helps)) < 2 and expand["right"] <= helps["left"],
               f"Expand all and Open help are not side by side, in that order: {expand} {helps}")
    if phone:
        desk.check(host["bottom"] <= expand["top"] + 1, f"on a phone the buttons are not under the host: {host} {expand}")
    else:
        desk.check(abs(mid(host) - mid(expand)) < 4 and host["right"] <= expand["left"],
                   f"on a desk the buttons are not on the host's row: {host} {expand}")
    desk.check(helps["bottom"] <= panes["top"], f"Open help is not above the sections: {helps} {panes}")
    desk.check(abs(helps["right"] - panes["right"]) < 2, f"the buttons are not at the row's end: {helps} {panes}")
    rows = 2 if phone else 1
    head_height = max(expand["bottom"], helps["bottom"]) - host["top"]
    desk.check(head_height < rows * 52, f"the head takes {head_height:.0f}px, more than {rows} row(s)")
    desk.check(head["overflow"] <= 1 and head["pageOverflow"] <= 0, "something on Settings is wider than the screen")
    desk.check(bool(head["inOrder"]), "the document order is not the host, Expand all, Open help, the sections")
    desk.shot("settings")
    desk.notes.append(f"host {head['hostSize']:.1f}px weight {head['hostWeight']} on one line"
                      f"{', cut' if head['cut'] else ''}; the head {head_height:.0f}px in {rows} row(s)")

    # The keys: Tab from Back goes to Expand all, Open help, then the first section. Into the screen, that
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
    desk.check(walked == ["fold-settings-all", "open-help", "fold-settings-voice-agent"], f"Tab from Back went to {walked}")
    desk.notes.append("Tab: Back, Expand all, Open help, Voice agent")

    # Expand all still opens everything, and its label's change keeps the row.
    page.click("#fold-settings-all")
    desk.wait_for("() => document.getElementById('fold-settings-all').textContent.trim() === 'Fold all'",
                  "Expand all did not open everything")
    page.evaluate("() => { document.getElementById('screen-settings').scrollTop = 0; }")
    desk.settled()
    opened = page.evaluate(HEAD_JS)
    desk.check(abs(mid(opened["expand"]) - mid(opened["help"])) < 2, "Fold all and Open help are no longer side by side")
    if not phone:
        desk.check(abs(mid(opened["host"]) - mid(opened["expand"])) < 4, "Fold all left the host's row")
    desk.check(opened["overflow"] <= 1, "something on Settings opened is wider than the screen")
    desk.shot("settings-expanded")
    desk.notes.append("Expand all read Fold all on the same row")



if __name__ == "__main__":
    raise SystemExit(main())
