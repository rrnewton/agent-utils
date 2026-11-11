#!/usr/bin/env python3
"""The "Live · updated" pill floating over the head of the channel's list, in real Chromium at a phone.

`#1320 no-freshness-band`. The owner, 2026-11-11: a persistent header band had taken vertical space
for a chip that "is designed as a floating chip anyway" — since `#1210` made `data-freshness` static,
the rule that moved the list's top edge down to the floating line's foot (`#324 live-pill-covers-row`)
applied whenever the pane was shown. Now NO band is reserved while search is closed: the list's top
edge is at the top of its cell, the first message sits at the top of the scroll area, and the pill and
the search glass float over the list as an overlay that takes no layout room, so showing, clearing or
rewording the pill still moves nothing (`#1210`). A landing still puts a row by where it starts, below
the pill as drawn (`floatingClearance`), the date rule it draws above its tile included. The fake-DOM
suite (tests/js/voice_page.test.mjs) pins the rule's absence; it lays nothing out, so it cannot say
where Chromium puts the list's edge. This does.

It serves the checked-out page and the fake API of tests/two_column_browser.py -- a channel taller than
the window with three threads -- from one loopback origin, at a 412x915 phone with touch, in the dark
scheme and the light one, the live stream attached so the pill says "Live · updated":

  Main at its top -> the first row is at the top of the scroll area and the pill floats over the
  list -> the list scrolled through nine places from its top to its foot -> at every one, the list's
  top edge is still at the top of its cell (no band), the pill is still floating over the list, and
  hiding and showing the pill moves neither the list nor the row under it -> the root of the long
  question's thread tapped in All: its thread, landed on its root, whose date rule is wholly below
  the floating line as drawn -> the thread scrolled through nine places, checked as Main was.

THE CONTROLS: the old band put back (a style sheet laid over the page's, moving the list's top edge
down by the floating line's foot): the same walk then reports a reserved band at most places, so the
checks above can see the defect they guard against.

No page error at any step. Pass --screenshots DIR to keep a PNG of each state for review; --web-root
DIR to run the same walk against another copy of the page.
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
from offline_cache_browser import TOKEN, Json, check, handler_for
from two_column_browser import REPLICA, THREAD_ROWS, ColumnsApi, Desk
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

# (label, width, height, colour scheme).
SIZES = (
    ("phone-412-dark", 412, 915, "dark"),
    ("phone-412-light", 412, 915, "light"),
)
# The summed-up thread's root in Main, whose "8 unread" chip the pill covered.
CHIP_ROOT = "202"
# The long question's root, whose tag opens its thread; its row draws the list's first date rule.
THREAD_ROOT = "209"
# Places each list is scrolled through, its top and its foot included.
PLACES = 9

# The band as it was before `#1320`: the list's top edge moved down to the floating line's foot,
# with the shorter fade that went with it.
OLD_ROOM_CSS = """
#screen-main > #scroll-area {
  margin-top: calc(var(--float-line) + max(var(--freshness-box), var(--search-disc)) / 2) !important;
  -webkit-mask-image: linear-gradient(to bottom, transparent 0, #000 0.35rem) !important;
  mask-image: linear-gradient(to bottom, transparent 0, #000 0.35rem) !important;
}
"""

# The floating line and what stands on it: the pill and the glass, the list's box and the top of
# its cell (its box less any margin its top edge was moved down by — none since `#1320`), and how
# far the list can scroll.
LINE_JS = """() => {
    const box = (e) => { const r = e.getBoundingClientRect();
        return {top: r.top, bottom: r.bottom, left: r.left, right: r.right}; };
    const area = document.getElementById('scroll-area');
    const pill = document.getElementById('channel-freshness');
    const glass = document.getElementById('search-toggle');
    const list = box(area);
    const cell = list.top - (parseFloat(getComputedStyle(area).marginTop) || 0);
    const p = box(pill), g = box(glass);
    const foot = Math.max(p.bottom, g.bottom);
    return {text: pill.textContent, hidden: pill.hidden, list, cell, pill: p, glass: g, foot,
            scrollTop: area.scrollTop, scrollMax: area.scrollHeight - area.clientHeight};
}"""

# The first row of the list on screen that draws a date rule, its rule and its tile.
RULE_JS = """() => {
    const area = document.getElementById('scroll-area');
    const row = [...document.querySelectorAll('#discord-log > li[data-seam="day"]')]
        .find((li) => li.getClientRects().length && li.getBoundingClientRect().bottom > area.getBoundingClientRect().top);
    if (!row) return null;
    const rule = row.querySelector(':scope > .row-seam:not(.row-seam-below)');
    const r = rule.getBoundingClientRect(), t = row.getBoundingClientRect();
    return {id: row.getAttribute('data-id'), text: rule.textContent.trim(), top: r.top, bottom: r.bottom,
            tile: t.top};
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
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, scheme in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, scheme))
    for line in report:
        print(line)
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, scheme: str) -> str:
    """One reader on one phone, against a fresh fake API and a fresh browser profile."""
    api = ColumnsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-live-pill-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height},
                color_scheme="light" if scheme == "light" else "dark",
                device_scale_factor=2.625, is_mobile=True, has_touch=True,
            )
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, label, width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            pill_walk(desk)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}, {label}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


# --- reading the page ---------------------------------------------------------------------------------


def line(desk: Desk) -> Json:
    found: Json = desk.page.evaluate(LINE_JS)
    return found


def number(found: Json, key: str) -> float:
    value = found[key]
    assert isinstance(value, (int, float)), f"{key} is {value!r}, not a number"
    return float(value)


def edge(found: Json, part: str, side: str) -> float:
    value = found[part]
    assert isinstance(value, dict), f"{part} is {value!r}"
    return float(value[side])


def tap(desk: Desk, selector: str) -> None:
    box = desk.page.locator(selector).first.bounding_box()
    desk.check(box is not None, f"{selector} is not drawn")
    assert box is not None
    desk.page.touchscreen.tap(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)


def line_problems(desk: Desk, where: str) -> list[str]:
    """A reserved band here, or a pill that is not floating over the list. Empty when neither is:
    the list's top edge is at the top of its cell and the pill stands over the list (`#1320`)."""
    found = line(desk)
    problems: list[str] = []
    if found["hidden"]:
        return [f"{where}: the pill is not shown"]
    top, cell = edge(found, "list", "top"), number(found, "cell")
    if top > cell + 0.5:
        problems.append(f"{where}: the list's top edge is at {top:.1f}px, {top - cell:.1f}px below the top of "
                        f"its cell at {cell:.1f}px: a reserved band")
    pill_top, pill_bottom = edge(found, "pill", "top"), edge(found, "pill", "bottom")
    list_bottom = edge(found, "list", "bottom")
    if pill_top < top - 0.5 or pill_bottom > list_bottom + 0.5:
        problems.append(f"{where}: the pill at {pill_top:.1f}-{pill_bottom:.1f}px is not floating over "
                        f"the list at {top:.1f}-{list_bottom:.1f}px")
    return problems


def scroll_walk(desk: Desk, what: str) -> list[str]:
    """`line_problems` at `PLACES` places from the list's top to its foot."""
    page = desk.page
    found = line(desk)
    furthest = number(found, "scrollMax")
    desk.check(furthest > 200, f"fixture: the {what} is hardly taller than the screen: {furthest}px to scroll")
    problems: list[str] = []
    for step in range(PLACES):
        at = round(furthest * step / (PLACES - 1))
        page.evaluate(f"() => {{ document.getElementById('scroll-area').scrollTop = {at}; }}")
        desk.settled()
        problems.extend(line_problems(desk, f"the {what} scrolled to {at}px"))
    return problems


def head_at_top(desk: Desk) -> list[str]:
    """Main at its very top: the first row at the head of the scroll area, the pill floating over it,
    and hiding and showing the pill moving neither the list nor that row (`#1320`, `#1210`)."""
    page = desk.page
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
    desk.settled()
    problems = line_problems(desk, "Main at its top")
    found = line(desk)
    top = edge(found, "list", "top")
    # The head of the pane (its summary seam, then its first row), not the first row alone: the
    # summary is an entry at the head of the view and sits above the first message by design.
    first = page.evaluate("""() => {
        const head = [...document.querySelectorAll('#pane-discord .seam, #discord-log > li')]
            .find((e) => e.getClientRects().length > 0);
        return head ? head.getBoundingClientRect().top : null;
    }""")
    desk.check(isinstance(first, (int, float)), "fixture: Main draws nothing at the head of the list")
    if isinstance(first, (int, float)) and float(first) > top + 24:
        problems.append(f"Main at its top: the head of the list starts at {float(first):.1f}px, "
                        f"{float(first) - top:.1f}px below the scroll area's top: a reserved band")
    # The pill takes no layout room: its coming and going leaves the list and the row where they were.
    before = page.evaluate("""() => {
        const area = document.getElementById('scroll-area');
        const row = document.querySelector('#discord-log > li');
        const a = area.getBoundingClientRect();
        return [a.top, a.height, row ? row.getBoundingClientRect().top : 0, area.scrollTop];
    }""")
    page.evaluate("() => { document.getElementById('channel-freshness').hidden = true; }")
    desk.settled()
    after = page.evaluate("""() => {
        const area = document.getElementById('scroll-area');
        const row = document.querySelector('#discord-log > li');
        const a = area.getBoundingClientRect();
        return [a.top, a.height, row ? row.getBoundingClientRect().top : 0, area.scrollTop];
    }""")
    page.evaluate("() => { document.getElementById('channel-freshness').hidden = false; }")
    desk.settled()
    if before != after:
        problems.append(f"Main at its top: hiding the pill moved the list from {before} to {after}")
    return problems


# --- the walk -----------------------------------------------------------------------------------------


def open_channel(desk: Desk) -> None:
    page = desk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    tap(desk, "#view-switch")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24",
                  "the channel did not open in All")
    desk.wait_for("() => document.getElementById('channel-freshness').textContent.startsWith('Live · updated')",
                  "the pill does not say the channel is live")
    desk.settled()


def open_thread(desk: Desk) -> None:
    """From All, the long question's root brought on screen and its tag tapped: its thread, landed."""
    page = desk.page
    page.evaluate(f"""() => {{
        const row = document.querySelector("#discord-log > li[data-id='{THREAD_ROOT}']");
        const area = document.getElementById('scroll-area');
        area.scrollTop += row.getBoundingClientRect().top - area.getBoundingClientRect().top - 160;
    }}""")
    desk.settled()
    tap(desk, f"#discord-log > li[data-id='{THREAD_ROOT}'] .thread-badge")
    desk.wait_for("() => !document.getElementById('thread-heading').hidden"
                  f" && document.querySelectorAll('#discord-log > li[data-id]').length === {THREAD_ROWS[REPLICA]}",
                  "the root's tag did not open its thread")
    page.wait_for_timeout(300)
    desk.settled()


def with_old_room(desk: Desk, steps: list[str]) -> list[str]:
    """Run each named check with the old reserved band put back over the page's own sheet."""
    page = desk.page
    page.evaluate("(css) => { const s = document.createElement('style'); s.id = 'old-room'; s.textContent = css;"
                  " document.head.append(s); }", OLD_ROOM_CSS)
    desk.settled()
    problems: list[str] = []
    try:
        for step in steps:
            problems.extend(head_at_top(desk) if step == "head" else scroll_walk(desk, step))
    finally:
        page.evaluate("() => document.getElementById('old-room').remove()")
        desk.settled()
    return problems


def pill_walk(desk: Desk) -> None:
    page = desk.page
    open_channel(desk)

    # Main at its head, then the whole list.
    page.select_option("#thread-select", "main")
    desk.wait_for(f"() => !!document.querySelector(\"#discord-log > li[data-id='{CHIP_ROOT}'] > .thread-unread\")",
                  "Main did not draw the thread's unread chip")
    desk.settled()
    problems = head_at_top(desk)
    desk.shot("1-main-head")
    desk.check(not problems, f"Main: {problems}")
    problems = scroll_walk(desk, "Main")
    desk.check(not problems, f"Main: {problems}")
    found = line(desk)
    desk.notes.append(f"Main: the list starts at the top of its cell ({edge(found, 'list', 'top'):.1f}px) with no "
                      f"reserved band, the pill floats over it, and at {PLACES} places hiding it moves nothing")
    control = with_old_room(desk, ["head", "Main"])
    desk.check(any("reserved band" in problem for problem in control),
               "THE CONTROL: with the old band the head was not flagged, so the check above proves nothing")
    desk.check(sum("reserved band" in problem for problem in control) >= PLACES // 2,
               f"THE CONTROL: with the old band too few places were flagged: {control[:3]}")

    # A thread, landed on its root: its date rule below the pill as drawn, over a list with no band.
    page.select_option("#thread-select", "flat")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24", "All did not come back")
    desk.settled()
    open_thread(desk)
    desk.shot("2-thread-landed")
    rule = page.evaluate(RULE_JS)
    desk.check(isinstance(rule, dict) and rule["id"] == THREAD_ROOT,
               f"fixture: the landed thread's first row on screen is not its root, with its date rule: {rule}")
    assert isinstance(rule, dict)
    found = line(desk)
    foot, top = number(found, "foot"), edge(found, "list", "top")
    desk.check(float(rule["top"]) >= max(foot, top) - 0.5,
               f"the root's date rule \"{rule['text']}\" starts at {float(rule['top']):.1f}px, under the floating line "
               f"whose foot is at {foot:.1f}px (the list starts at {top:.1f}px)")
    desk.check(float(rule["tile"]) >= float(rule["bottom"]), "the root's tile is not under its date rule")
    card = page.evaluate("() => document.getElementById('thread-card').getBoundingClientRect().bottom")
    desk.check(float(card) <= number(found, "cell") + 0.5, "fixture: the thread's card is not above the list's cell")
    problems = line_problems(desk, "the thread as it landed")
    desk.check(not problems, f"thread: {problems}")
    problems = scroll_walk(desk, "thread")
    desk.check(not problems, f"thread: {problems}")
    desk.notes.append(f"a thread landed with its root's date rule at {float(rule['top']):.1f}px, below the floating "
                      f"line's foot at {foot:.1f}px, over a list whose top edge is at the top of its cell, at "
                      f"{PLACES} places")
    control = with_old_room(desk, ["thread"])
    desk.check(sum("reserved band" in problem for problem in control) >= PLACES // 2,
               f"THE CONTROL: with the old band too few places were flagged: {control[:3]}")
    desk.notes.append(f"with the old band the same checks flagged a reserved band ({len(control)} findings in the "
                      "thread alone)")


if __name__ == "__main__":
    raise SystemExit(main())
