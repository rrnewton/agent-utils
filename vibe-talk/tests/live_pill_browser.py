#!/usr/bin/env python3
"""The "Live · updated" pill over the head of the channel's list, in real Chromium at a 412x915 phone.

`#324 live-pill-covers-row`. Found by a read-only exercise of a Slack deployment on 2026-10-14: on a
phone the floating pill covered the top row. In a thread it hid the date rule, with a clipped strip of
the list's header showing between the thread's card and the pill; in Main it covered an unread chip.
The pill's room was padding at the head of the list, inside the scroller, and room at the head scrolls
away: whenever the list was not at its very top, the pill and the search glass beside it stood over
whatever row was passing, and on a live channel the pill is nearly always up. Now the list's top edge
moves down to the floating line's foot, so nothing is ever drawn or tapped under either, and a landing
puts a row by where it starts, the date rule it draws above its tile included. The fake-DOM suite
(tests/js/voice_page.test.mjs) pins the landing and the rule's selector; it lays nothing out, so it
cannot say what Chromium draws under the pill. This does.

It serves the checked-out page and the fake API of tests/two_column_browser.py -- a channel taller than
the window with three threads -- from one loopback origin, at a 412x915 phone with touch, in the dark
scheme and the light one, the live stream attached so the pill says "Live · updated":

  Main, the list scrolled so the summed-up thread's "8 unread" chip stands on the pill's line -> the
  chip is not under the pill: a tap where it was finds nothing of the list, and the list's top edge is
  at the floating line's foot, below the pill and the glass -> the list scrolled through nine places
  from its top to its foot -> at every one, a tap anywhere on the pill's line, beside the pill and the
  glass, finds nothing of the list, and every pixel there is the screen's own background, nothing of a
  row showing through -> the root of the long question's thread tapped in All: its thread, landed on
  its root, whose date rule is wholly below the floating line and inside the list, with nothing of the
  list showing between the thread's card and the pill -> the thread scrolled through nine places,
  checked as Main was.

THE CONTROLS: the pill's room put back as it was, padding at the head of the list (a style sheet laid
over the page's): the same walk then finds the list under the pill, in Main and in the thread, so the
checks above can see the defect they guard against.

No page error at any step. Pass --screenshots DIR to keep a PNG of each state for review; --web-root
DIR to run the same walk against another copy of the page.
"""

from __future__ import annotations

import argparse
import base64
import os
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, Json, check, handler_for
from thread_card_focus_browser import PIXELS_JS
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
# How far a pixel on the pill's line may be from the screen's background, per channel.
PIXEL_TOLERANCE = 4
# Room left around the pill and the glass when their line is sampled, for their shadow.
SHADOW_PX = 10

# The pill's room as it was before `#324`: padding at the head of the list, inside the scroller, and
# the list's top edge left at the top of its cell.
OLD_ROOM_CSS = """
#screen-main > #scroll-area {
  margin-top: 0 !important;
  -webkit-mask-image: linear-gradient(to bottom, transparent 0, #000 1.1rem) !important;
  mask-image: linear-gradient(to bottom, transparent 0, #000 1.1rem) !important;
}
#pane-discord[data-freshness] {
  padding-top: calc(var(--float-line) + max(var(--freshness-box), var(--search-disc)) / 2) !important;
}
"""

# The floating line and what is under it: the pill and the glass, the list's box and the top of its
# cell (its box less the room its top edge was moved down by), the screen's own background, and, at
# points across the line and the band above it, beside the pill and the glass, what a tap there finds
# when that is part of the channel's list.
LINE_JS = """() => {
    const box = (e) => { const r = e.getBoundingClientRect();
        return {top: r.top, bottom: r.bottom, left: r.left, right: r.right}; };
    const area = document.getElementById('scroll-area');
    const pane = document.getElementById('pane-discord');
    const pill = document.getElementById('channel-freshness');
    const glass = document.getElementById('search-toggle');
    const list = box(area);
    const cell = list.top - (parseFloat(getComputedStyle(area).marginTop) || 0);
    const p = box(pill), g = box(glass);
    const foot = Math.max(p.bottom, g.bottom);
    let painted = document.getElementById('screen-main');
    while (painted && /^(transparent|rgba\\(0, 0, 0, 0\\))$/.test(getComputedStyle(painted).backgroundColor)) {
        painted = painted.parentElement;
    }
    const background = (painted ? getComputedStyle(painted).backgroundColor : 'rgb(255, 255, 255)')
        .match(/[\\d.]+/g).slice(0, 3).map(Number);
    const away = (x, y, b, pad) => x < b.left - pad || x > b.right + pad || y < b.top - pad || y > b.bottom + pad;
    const name = (e) => e.id ? `#${e.id}` : `${e.tagName.toLowerCase()}.${String(e.className).split(' ')[0]}`;
    const hits = [], samples = [];
    const rows = [...new Set([cell + 1, (cell + p.top) / 2, p.top + 3, (p.top + p.bottom) / 2, p.bottom - 3, foot - 1])]
        .filter((y) => y > cell && y < foot);
    for (const y of rows) {
        for (let x = 2; x < window.innerWidth - 1; x += 12) {
            if (!away(x, y, g, 6)) continue;
            const hit = document.elementFromPoint(x, y);
            if (hit && pane.contains(hit)) {
                const row = hit.closest('#discord-log > li');
                hits.push(`${name(hit)}${row ? ` of row ${row.getAttribute('data-id')}` : ''} at (${x}, ${y.toFixed(0)})`);
            }
            if (away(x, y, p, SHADOW) && away(x, y, g, SHADOW)) samples.push([x, y]);
        }
    }
    return {text: pill.textContent, hidden: pill.hidden, list, cell, pill: p, glass: g, foot, background,
            hits, samples, scrollTop: area.scrollTop, scrollMax: area.scrollHeight - area.clientHeight};
}""".replace("SHADOW", str(SHADOW_PX))

# Where the unread chip of row `id` is, and what a tap at its middle finds.
CHIP_JS = """(id) => {
    const chip = document.querySelector(`#discord-log > li[data-id="${id}"] > .thread-unread`);
    if (!chip || chip.getClientRects().length === 0) return null;
    const r = chip.getBoundingClientRect();
    const x = (r.left + r.right) / 2, y = (r.top + r.bottom) / 2;
    const hit = document.elementFromPoint(x, y);
    return {top: r.top, bottom: r.bottom, left: r.left, right: r.right, x, y, text: chip.textContent.trim(),
            hit: !!hit && chip.contains(hit)};
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


def pixels(desk: Desk, points: list[list[float]]) -> list[list[float]]:
    """The colour at each CSS-pixel point of a screenshot of the viewport."""
    png = base64.b64encode(desk.page.screenshot()).decode("ascii")
    found: list[list[float]] = desk.page.evaluate(PIXELS_JS, {"png": png, "points": points})
    return found


def tap(desk: Desk, selector: str) -> None:
    box = desk.page.locator(selector).first.bounding_box()
    desk.check(box is not None, f"{selector} is not drawn")
    assert box is not None
    desk.page.touchscreen.tap(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)


def line_problems(desk: Desk, where: str) -> list[str]:
    """What the list puts under the floating line here: a tap that finds part of it, a pixel that is not
    the screen's own background, or the list's top edge above the line's foot. Empty when nothing is."""
    found = line(desk)
    problems: list[str] = []
    if found["hidden"]:
        return [f"{where}: the pill is not shown"]
    top, foot = edge(found, "list", "top"), number(found, "foot")
    if top < foot - 0.5:
        problems.append(f"{where}: the list's top edge is at {top:.1f}px, above the floating line's foot at {foot:.1f}px")
    hits = found["hits"]
    assert isinstance(hits, list)
    problems.extend(f"{where}: a tap on the pill's line finds {hit}" for hit in hits[:4])
    samples = found["samples"]
    background = found["background"]
    assert isinstance(samples, list) and isinstance(background, list)
    off = [(point, colour) for point, colour in zip(samples, pixels(desk, samples))
           if any(abs(float(c) - float(b)) > PIXEL_TOLERANCE for c, b in zip(colour, background))]
    if off:
        point, colour = off[0]
        problems.append(f"{where}: {len(off)} of {len(samples)} pixels on the pill's line are not the screen's "
                        f"background {background}, such as {colour} at {point}")
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


def chip_on_the_line(desk: Desk) -> list[str]:
    """Main scrolled so the summed-up thread's unread chip stands on the pill's line: what covers it."""
    page = desk.page
    chip = page.evaluate(CHIP_JS, CHIP_ROOT)
    desk.check(isinstance(chip, dict), f"fixture: row {CHIP_ROOT} shows no unread chip in Main")
    assert isinstance(chip, dict)
    found = line(desk)
    middle = (edge(found, "pill", "top") + edge(found, "pill", "bottom")) / 2
    page.evaluate(f"() => {{ document.getElementById('scroll-area').scrollTop += {float(chip['y']) - middle}; }}")
    desk.settled()
    chip = page.evaluate(CHIP_JS, CHIP_ROOT)
    assert isinstance(chip, dict)
    found = line(desk)
    pill = found["pill"]
    assert isinstance(pill, dict)
    desk.check(float(chip["left"]) < float(pill["right"]) and float(chip["right"]) > float(pill["left"]),
               f"fixture: the chip {chip} is not under the pill {pill} across the screen")
    problems: list[str] = []
    if chip["hit"]:
        problems.append(f"the \"{chip['text']}\" chip is under the pill, and a tap through the pill reaches it")
    problems.extend(line_problems(desk, "Main with the unread chip on the pill's line"))
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
    """Run each named check with the pill's room put back as padding inside the scroller."""
    page = desk.page
    page.evaluate("(css) => { const s = document.createElement('style'); s.id = 'old-room'; s.textContent = css;"
                  " document.head.append(s); }", OLD_ROOM_CSS)
    desk.settled()
    problems: list[str] = []
    try:
        for step in steps:
            problems.extend(chip_on_the_line(desk) if step == "chip" else scroll_walk(desk, step))
    finally:
        page.evaluate("() => document.getElementById('old-room').remove()")
        desk.settled()
    return problems


def pill_walk(desk: Desk) -> None:
    page = desk.page
    open_channel(desk)

    # Main, the unread chip on the pill's line, then the whole list.
    page.select_option("#thread-select", "main")
    desk.wait_for(f"() => !!document.querySelector(\"#discord-log > li[data-id='{CHIP_ROOT}'] > .thread-unread\")",
                  "Main did not draw the thread's unread chip")
    desk.settled()
    problems = chip_on_the_line(desk)
    desk.shot("1-main-chip")
    desk.check(not problems, f"Main: {problems}")
    problems = scroll_walk(desk, "Main")
    desk.check(not problems, f"Main: {problems}")
    found = line(desk)
    desk.notes.append(f"Main: the list starts at the floating line's foot ({edge(found, 'list', 'top'):.1f}px), the "
                      f"unread chip on the pill's line is clipped rather than covered, and at {PLACES} places nothing "
                      "of the list is under the pill or the glass")
    control = with_old_room(desk, ["chip", "Main"])
    desk.check(any("chip is under the pill" in problem for problem in control),
               "THE CONTROL: with the old room the chip was not under the pill, so the check above proves nothing")
    desk.check(sum("scrolled to" in problem for problem in control) >= PLACES // 2,
               f"THE CONTROL: with the old room the list was under the pill at too few places: {control[:3]}")

    # A thread, landed on its root: the date rule clear, nothing of the list between the card and the pill.
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
                      f"line's foot at {foot:.1f}px, nothing of the list between the card and the pill, and nothing "
                      f"under the line at {PLACES} places")
    control = with_old_room(desk, ["thread"])
    desk.check(sum("scrolled to" in problem for problem in control) >= PLACES // 2,
               f"THE CONTROL: with the old room the thread was under the pill at too few places: {control[:3]}")
    desk.notes.append(f"with the old room the same checks found the list under the pill ({len(control)} findings in the "
                      "thread alone)")


if __name__ == "__main__":
    raise SystemExit(main())
