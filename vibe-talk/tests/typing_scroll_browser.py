#!/usr/bin/env python3
"""Typing never loses the reader's place, in real Chromium at 412x915 and 360x800 by touch, and at 1280x800.

`#1144 scrollback-typing-jump`. The owner, 2026-11-09, on the phone and then "especially on desktop
right now": the list jumps while typing in the main channel's box, up into older messages, and back
to the typing part-shown; and, in two columns, simply moving the mouse between the columns scrolls
one of them. Latent, not a regression (the same failures reproduce on the page of the day before):
while a composer inside the list is focused, the browser owns the scroll - it reveals the caret on
every insertion, and content laid out again taller at the foot leaves the offset put and the composer
under the keys. The rules this walk pins, as the owner settled them that day:

  * Typing in the focused box may bring the composer into view, whole, above the keyboard; once
    there, the view never moves up away from it - arrivals and rows growing late are followed
    (follow is one state, from the reader's own scrolls and landings, never switched on by the
    browser revealing the caret), and the at-the-bottom test is of the rows' foot, keyboard
    geometry left out.
  * Reading with the box unfocused (or scrolled back from it), arrivals and re-renders never move
    the message being read; unfocused keys navigate, never type and never yank to the composer.
  * Mouse movement or hover never scrolls anything: resting on a thread card or its root in two
    columns draws its link and moves neither column (the dwell glide is gone).

The verdict is that drawn state, waited for (`#1250 vibe-talk-timing-robust`): after an arrival, a
resize or a row grown late, the walk waits for the composer's drawn place and the list's foot gap
themselves (30 s failure bounds) instead of reading them after a fixed quiet that a hot host can
outrun, and `Reader.quiet` here means the first row observed unchanged across frames. The hover
rests stay dwells — outwaiting the old glide is their point, and a loaded host only lengthens them.

The keyboard is simulated as Chrome on Android lays it out (the viewport 336px shorter), as in
tests/keyboard_hides_bar_browser.py; the late growth is a row at the foot given 180px more height
after it lands, as an image or embed settling does, with no redraw to correct it. The fake API and
the anchor measures are tests/scroll_stability_browser.py's.

Pass --screenshots DIR to keep a PNG of each state; --web-root DIR to run against another copy of
the page (one before this fix fails the late-growth and hover cases).
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
import page_sources
from offline_cache_browser import Json
from scroll_stability_browser import READS_JS, Reader, StabilityApi, handler, open_main

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

KEYBOARD_PX = 336
SIZES = (("phone-412", 412, 915, True), ("phone-360", 360, 800, True), ("desk-1280", 1280, 800, False))
# The most a walk waits for a drawn state it expects (#998 STATE_S precedent): passes take as long
# as the page does; this only bounds a failure.
STATE_S = 30.0


class TypingReader(Reader):
    """Reader with state waits instead of fixed quiets (scroll_stability's Reader is not edited).

    `quiet` waits for the first row to be observed unchanged across frames (Reader.settled), and
    every wait_for takes the STATE_S failure bound, whatever smaller bound a call site names.
    """

    def wait_for(self, condition: str, why: str, seconds: float = STATE_S) -> None:
        super().wait_for(condition, why, max(seconds, STATE_S))

    def quiet(self, settle_ms: int = 400) -> None:
        self.settled(seconds=STATE_S)

PLACE_JS = """() => {
    const area = document.getElementById('scroll-area');
    const rect = (id) => { const n = document.getElementById(id);
        if (!n || !n.getClientRects().length) return null;
        const r = n.getBoundingClientRect();
        return {top: r.top, bottom: r.bottom}; };
    return {
        gap: area.scrollHeight - area.scrollTop - area.clientHeight,
        composer: rect('channel-composer'), box: rect('channel-compose-text'),
        viewport: window.visualViewport ? window.visualViewport.height : window.innerHeight,
        active: document.activeElement ? document.activeElement.id : null,
    };
}"""

TOPS_JS = """() => ({ main: document.getElementById('scroll-area').scrollTop,
    side: document.getElementById('side-scroll').scrollTop })"""


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per state into (default: none)")
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
            report.append(walk(playwright.chromium, args, label, width, height, touch))
    for line in report:
        print(line)
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, touch: bool) -> str:
    api = StabilityApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-typing-scroll-") as profile:
            if touch:
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="dark",
                    device_scale_factor=2.625, is_mobile=True, has_touch=True)
            else:
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="dark")
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.add_init_script(READS_JS)
            reader = TypingReader(page, api, args.screenshots, label, touch)
            reader.width, reader.height = width, height  # type: ignore[attr-defined]
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            open_main(reader)
            if touch:
                foot_walk(reader)
                reading_walk(reader)
            else:
                selection_walk(reader)
                desk_walk(reader)
                hover_walk(reader)
            reader.check(not errors, f"the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(reader.notes)
    finally:
        api.stopping.set()
        api.timeline_gate.set()
        server.shutdown()
        server.server_close()
        thread.join()


def place(reader: Reader) -> Json:
    found = reader.page.evaluate(PLACE_JS)
    assert isinstance(found, dict)
    return found


def number(found: Json, *path: str) -> float:
    value: object = found
    for key in path:
        assert isinstance(value, dict), f"nothing at {path} in {found}"
        value = value[key]
    assert isinstance(value, (int, float)), f"{path} is not a number in {found}"
    return float(value)


def whole_composer(reader: Reader, found: Json, when: str) -> None:
    # The box whole above the visible foot, its composer not clipped at the top: a row parked
    # under the box (#1135) stands below the keys at rest by design, so the box is the measure.
    reader.check(found["box"] is not None and number(found, "box", "top") >= -1
                 and number(found, "box", "bottom") <= number(found, "viewport") + 1
                 and found["composer"] is not None and number(found, "composer", "top") >= -1,
                 f"{when}: the composer is not whole on screen: {found}")


def is_whole(found: Json) -> bool:
    return (found["box"] is not None and number(found, "box", "top") >= -1
            and number(found, "box", "bottom") <= number(found, "viewport") + 1
            and found["composer"] is not None and number(found, "composer", "top") >= -1)


def place_whole(reader: Reader, when: str) -> Json:
    """The composer's drawn place once it is whole on screen, waited for to STATE_S."""
    deadline = time.monotonic() + STATE_S
    found = place(reader)
    while time.monotonic() < deadline and not is_whole(found):
        reader.page.wait_for_timeout(25)
        found = place(reader)
    whole_composer(reader, found, when)
    return found


def wait_gap(reader: Reader, at_most: float, why: str) -> Json:
    """The list's drawn place once its foot gap is at most `at_most` px, waited for to STATE_S."""
    deadline = time.monotonic() + STATE_S
    found = place(reader)
    while time.monotonic() < deadline and number(found, "gap") > at_most:
        reader.page.wait_for_timeout(25)
        found = place(reader)
    reader.check(number(found, "gap") <= at_most, f"{why}: {found}")
    return found


def keyboard(reader: Reader, height: int, why: str) -> Json:
    """The viewport at `height`, as the keyboard (or its strip) resizes it, and the page once settled."""
    reader.page.set_viewport_size({"width": reader.width, "height": height})  # type: ignore[attr-defined]
    reader.wait_for(f"() => Math.round(window.visualViewport.height) === {height}", why)
    reader.quiet(500)
    return place(reader)


def arrive(reader: Reader, text: str) -> None:
    arrived = reader.api.add(text)
    reader.api.push("message", {"message": arrived}, str(arrived["id"]))
    reader.wait_for(f"() => !!document.querySelector(\"#discord-log > li[data-id='{arrived['id']}']\")",
                    "the live message never arrived")
    reader.quiet(700)


def grow_last_row(reader: Reader, px: int = 180) -> None:
    """A row at the foot laid out again taller after it landed, with no redraw to correct it."""
    reader.page.evaluate("""(px) => {
        const rows = document.querySelectorAll('#discord-log > li[data-id]');
        const last = rows[rows.length - 1];
        last.style.minHeight = (last.getBoundingClientRect().height + px) + 'px';
    }""", px)
    reader.quiet(600)


def to_foot(reader: Reader) -> None:
    reader.page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    reader.quiet(400)


def foot_walk(reader: Reader) -> None:
    """Typing at the foot with the keyboard up: arrivals followed, strip resizes ridden, late growth
    followed, and the whole composer on screen after every step - never a sink into older messages."""
    page = reader.page
    to_foot(reader)
    page.tap("#channel-compose-text")
    up = reader.height - KEYBOARD_PX  # type: ignore[attr-defined]
    found = keyboard(reader, up, "the keyboard did not come up")
    whole_composer(reader, found, "keyboard up")
    rest_gap = number(found, "gap")
    page.keyboard.type("a long reply being typed while the channel moves ", delay=20)
    arrive(reader, "An agent message arriving while the owner types at the foot.")
    found = wait_gap(reader, rest_gap + 17, "an arrival while typing left the reader above the foot")
    whole_composer(reader, found, "an arrival while typing")
    page.keyboard.type("and more words ", delay=20)
    keyboard(reader, up - 48, "the keyboard's strip did not grow")
    place_whole(reader, "the strip grown while typing")
    page.keyboard.type("still typing ", delay=20)
    keyboard(reader, up, "the keyboard's strip did not shrink back")
    found = place_whole(reader, "the strip shrunk while typing")
    reader.check(number(found, "gap") <= rest_gap + 17, f"the strip's comings and goings left the reader above the foot: {found}")
    grow_last_row(reader)
    found = wait_gap(reader, rest_gap + 17,
                     "a row grown late at the foot sank the composer and stopped the following")
    whole_composer(reader, found, "a row grown late while typing")
    page.keyboard.type("x", delay=20)
    reader.quiet(500)
    place_whole(reader, "the key after the growth")
    arrive(reader, "An arrival after the late growth, while typing at the foot.")
    wait_gap(reader, rest_gap + 17, "the arrival after the growth was not followed")
    reader.notes.append("typing at the foot: arrivals, strip resizes and a row grown 180px late all followed, "
                        "the whole composer on screen throughout")


def reading_walk(reader: Reader) -> None:
    """Scrolled back with a draft in the focused box: arrivals and strip resizes hold the message being
    read; typing again lands on the whole composer (the owner's settled rule), never part of it."""
    page = reader.page
    up = reader.height - KEYBOARD_PX  # type: ignore[attr-defined]
    found = place(reader)
    reader.check(found["active"] == "channel-compose-text", f"the box lost the focus: {found}")
    before_top = float(page.evaluate("() => document.getElementById('scroll-area').scrollTop"))
    page.mouse.move(reader.width // 2, 300)  # type: ignore[attr-defined]
    page.mouse.wheel(0, -700)
    reader.quiet(500)
    top = float(page.evaluate("() => document.getElementById('scroll-area').scrollTop"))
    reader.check(top < before_top - 300, f"the reader's wheel did not scroll up: {before_top} -> {top}")
    anchor = reader.first()
    arrive(reader, "A message arriving while the owner reads scrollback mid-draft.")
    reader.still(anchor, "an arrival while reading scrollback, the box focused")
    keyboard(reader, up - 44, "the keyboard's strip did not grow while reading")
    reader.still(anchor, "the keyboard grown while reading scrollback")
    keyboard(reader, up, "the keyboard's strip did not shrink back while reading")
    reader.still(anchor, "the keyboard shrunk while reading scrollback")
    page.keyboard.type("resuming the draft from scrollback ", delay=20)
    reader.quiet(600)
    place_whole(reader, "typing resumed from scrollback")
    found = keyboard(reader, reader.height, "the keyboard did not go down")  # type: ignore[attr-defined]
    reader.check(found["box"] is not None, f"the box went with the keyboard: {found}")
    reader.notes.append("reading scrollback mid-draft: arrivals and strip resizes held the anchor; typing "
                        "again landed on the whole composer")


def desk_walk(reader: Reader) -> None:
    """The same at a desk, no on-screen keyboard: typing at the foot followed across an arrival and a
    late growth; reading unfocused, an arrival holds and the arrow keys navigate, never typing."""
    page = reader.page
    to_foot(reader)
    page.click("#channel-compose-text")
    page.keyboard.type("a desktop draft being typed at the foot ", delay=15)
    arrive(reader, "A desktop arrival while typing at the foot.")
    found = wait_gap(reader, 25, "a desktop arrival while typing left the foot")
    whole_composer(reader, found, "a desktop arrival while typing")
    grow_last_row(reader)
    found = wait_gap(reader, 25, "a row grown late sank the desktop composer")
    whole_composer(reader, found, "a row grown late on the desktop")
    page.evaluate("() => document.getElementById('scroll-area').focus({preventScroll: true})")
    reader.scroll_main(0.45)
    anchor = reader.first()
    arrive(reader, "Arriving while reading on desktop, the box unfocused.")
    reader.still(anchor, "a desktop arrival while reading, unfocused")
    page.keyboard.press("ArrowDown")
    reader.quiet(400)
    found = place(reader)
    reader.check(found["active"] != "channel-compose-text",
                 f"an unfocused key took the composer instead of navigating: {found}")
    reader.notes.append("on the desktop: typing followed, reading held, unfocused keys navigated")


def selection_walk(reader: Reader) -> None:
    """`#1209 selection-survives-updates`: words selected in a row are still selected, in the
    rebuilt row, after a landing that rebuilds the rows."""
    page = reader.page
    row = page.locator("#discord-log > li[data-id]").filter(has_text="Message 232.").first
    row.scroll_into_view_if_needed()
    reader.quiet(400)
    box = row.bounding_box()
    reader.check(box is not None, "no row to select in")
    assert box is not None
    page.mouse.move(box["x"] + 90, box["y"] + 42)
    page.mouse.down()
    page.mouse.move(box["x"] + 170, box["y"] + 42, steps=8)
    page.mouse.up()
    reader.quiet(300)
    picked = page.evaluate("""() => {
        const sel = window.getSelection();
        if (!sel || sel.isCollapsed || !sel.rangeCount) return null;
        let at = sel.getRangeAt(0).startContainer;
        while (at && !(at instanceof Element && at.hasAttribute("data-ids"))) at = at.parentNode;
        return { text: String(sel), row: at ? at.getAttribute("data-ids") : null };
    }""")
    reader.check(picked is not None and len(picked["text"]) > 0, f"no selection was made: {picked}")
    page.evaluate("() => document.getElementById('channel-view-flat').click()")
    reader.quiet(600)
    page.evaluate("() => document.getElementById('channel-view-main').click()")
    reader.quiet(900)
    # The landing rebuilding the rows can land after the first row settles; wait for a selection
    # itself, to the same failure bound, before judging that it survived.
    reader.wait_for("() => { const sel = window.getSelection(); return !!sel && !sel.isCollapsed && sel.rangeCount > 0; }",
                    "the landing cleared the reader's selection")
    after = page.evaluate("""() => {
        const sel = window.getSelection();
        if (!sel || sel.isCollapsed || !sel.rangeCount) return null;
        let at = sel.getRangeAt(0).startContainer;
        while (at && !(at instanceof Element && at.hasAttribute("data-ids"))) at = at.parentNode;
        return { text: String(sel), row: at ? at.getAttribute("data-ids") : null, held: at };
    }""")
    reader.check(after is not None, "the landing cleared the reader's selection")
    assert after is not None and picked is not None
    # The words may stand a seam's width from where they were (a day rule drawn inside the row
    # on one view and not the other): the same row and the same length is the survival that counts.
    reader.check(after["row"] == picked["row"], f"the selection stands in another row: {picked} -> {after}")
    reader.check(len(after["text"]) == len(picked["text"]), f"the selection's length changed: {picked} -> {after}")
    reader.notes.append("a selection in a row survived a landing that rebuilt the rows")


def hover_walk(reader: Reader) -> None:
    """Two columns: the mouse crossing between them, and resting on cards and roots past the old
    glide's wait, scrolls neither column by a pixel. The links are still drawn."""
    page = reader.page
    reader.scroll_main(0.5)
    page.evaluate("() => { const a = document.getElementById('side-scroll'); a.scrollTop = (a.scrollHeight - a.clientHeight) * 0.3; }")
    reader.quiet(500)
    # Both columns observed still across further frames before the rests are judged against them:
    # a capture taken mid-settle would blame the hover for the settling scroll.
    _deadline = time.monotonic() + STATE_S
    _last: tuple[float, float] | None = None
    while time.monotonic() < _deadline:
        _now = page.evaluate(TOPS_JS)
        _pair = (float(_now["main"]), float(_now["side"]))
        if _pair == _last:
            break
        _last = _pair
        reader.frames(2)

    def tops() -> tuple[float, float]:
        found = page.evaluate(TOPS_JS)
        assert isinstance(found, dict)
        return float(found["main"]), float(found["side"])

    def hover(selector: str, ms: int) -> None:
        box = page.locator(selector).first.bounding_box()
        reader.check(box is not None, f"nothing to hover at {selector}")
        assert box is not None
        page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2, steps=12)
        page.wait_for_timeout(ms)

    before = tops()
    hover("#side-cards > li[data-card]", 2200)
    reader.check(tops() == before, f"resting on a card scrolled a column: {before} -> {tops()}")
    hover("#discord-log > li[data-id]", 2200)
    reader.check(tops() == before, f"resting on a root scrolled a column: {before} -> {tops()}")
    hover("#side-cards > li[data-card] >> nth=1", 300)
    hover("#discord-log > li[data-id] >> nth=2", 300)
    hover("#side-cards > li[data-card] >> nth=2", 2200)
    reader.check(tops() == before, f"crossing between the columns scrolled one: {before} -> {tops()}")
    reader.notes.append("hover crossings and rests past the old glide's wait scrolled neither column")


if __name__ == "__main__":
    raise SystemExit(main())
