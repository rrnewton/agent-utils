#!/usr/bin/env python3
"""Floating chips and the status toast never cover a card's controls (`#1711 vibe-talk-floating-chip-overlap`).

The owner, 2026-11-16 on an Android phone: at the top of the channel the "New version — reload"
chip ran over the "Live · updated" pill's line to the first card's Thread(N) chip; at the foot
the "Agent voice on." toast lay over the last card's reaction chips. So, at a 412x915 phone
(device scale 2.6, touch):

  the channel opened with both top chips shown -> the pill, the reload chip, the search glass
  and a Thread(N) chip do not intersect, and the reload chip, the glass and Thread(N) are each
  hittable at their centres -> the same with a Thread(N) chip scrolled onto the pill's own line,
  the owner's screenshot's alignment -> the toast shown at the foot of the list, the list
  scrolled to its foot -> the toast does not intersect the last card's reaction chips, and each
  reaction chip and the toast's dismiss control are hittable at their centres.

At a 1280x800 desk the two top chips stay on one shared line, as before, and still do not
intersect each other, the glass, or a Thread(N) chip.

Every wait is on a page condition; no wall-clock wait decides a result. The page and fake API
are those of tests/live_pill_browser.py, with reactions on the first and last messages.
Pass --screenshots DIR to keep a PNG of each state for review.
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
from two_column_browser import ColumnsApi, Desk
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

MEASURE_JS = """() => {
    const drawn = (e) => Boolean(e) && e.getClientRects().length > 0;
    const box = (e) => { const r = e.getBoundingClientRect();
        return {left: r.left, top: r.top, right: r.right, bottom: r.bottom}; };
    const hit = (e) => { if (!drawn(e)) return false; const r = e.getBoundingClientRect();
        const found = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
        return found === e || e.contains(found); };
    const area = document.getElementById('scroll-area').getBoundingClientRect();
    const inList = (e) => { if (!drawn(e)) return false; const r = e.getBoundingClientRect();
        return r.bottom > area.top && r.top < area.bottom; };
    const badge = [...document.querySelectorAll('#discord-log .thread-badge')].find(inList) || null;
    const strips = [...document.querySelectorAll('#discord-log .reactions')].filter(inList);
    const strip = strips.length ? strips[strips.length - 1] : null;
    const reactions = strip ? [...strip.querySelectorAll('.reaction')] : [];
    const byId = (id) => document.getElementById(id);
    return {
        fresh: drawn(byId('channel-freshness')) ? box(byId('channel-freshness')) : null,
        update: drawn(byId('page-update')) ? box(byId('page-update')) : null,
        glass: drawn(byId('search-toggle')) ? box(byId('search-toggle')) : null,
        badge: badge ? {...box(badge), id: badge.closest('li')?.getAttribute('data-id')} : null,
        status: drawn(byId('status-line')) && !byId('status-line').hidden ? box(byId('status-line')) : null,
        reactions: reactions.map(box),
        hits: {update: hit(byId('page-update')), glass: hit(byId('search-toggle')), badge: hit(badge),
               reactions: reactions.map(hit), dismiss: hit(byId('dismiss-status'))},
    };
}"""

BADGE_ON_LINE_JS = """() => {
    const pill = document.getElementById('channel-freshness');
    const area = document.getElementById('scroll-area');
    const badge = [...document.querySelectorAll('#discord-log .thread-badge')]
        .find((e) => e.getClientRects().length && e.getBoundingClientRect().bottom > area.getBoundingClientRect().top);
    return Boolean(badge) && Math.abs(badge.getBoundingClientRect().top - pill.getBoundingClientRect().top) < 2;
}"""

FOOT_CLEAR_JS = """() => {
    const toast = document.getElementById('status-line');
    if (toast.hidden || !toast.getClientRects().length) return false;
    const area = document.getElementById('scroll-area').getBoundingClientRect();
    const strips = [...document.querySelectorAll('#discord-log .reactions')]
        .filter((e) => { const r = e.getBoundingClientRect(); return r.bottom > area.top && r.top < area.bottom; });
    if (!strips.length) return false;
    const t = toast.getBoundingClientRect();
    // Clear is the condition the walk asserts below -- no reaction chip of the last card
    // intersects the toast -- not the strip ending above the toast's top. On a loaded host
    // the pill's rise over the composer (`placeChipsOverComposer`, driven by resize
    // observers a starved main thread delivers late) lands after the scroll: the strip
    // then stands wholly below the pill, clear of it, while its bottom is still below
    // the pill's top, so the old proxy never held and the wait ran out its deadline
    // against a state that was already good (#1765).
    return [...strips[strips.length - 1].querySelectorAll('.reaction')].every((chip) => {
        const r = chip.getBoundingClientRect();
        return !(r.left < t.right - 0.5 && t.left < r.right - 0.5
                 && r.top < t.bottom - 0.5 && t.top < r.bottom - 0.5);
    });
}"""


class ChipApi(ColumnsApi):
    """The live-pill channel, with reaction chips on its first and last messages."""

    def __init__(self) -> None:
        super().__init__()
        for index in (0, len(self.messages) - 1):
            self.messages[index]["reactions"] = [{"emoji": "👀", "count": 1}, {"emoji": "✅", "count": 2}]


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per state into")
    return parser.parse_args()


def num(value: object) -> float:
    assert isinstance(value, (int, float)), f"{value!r} is not a number"
    return float(value)


def box_of(found: Json, name: str) -> dict[str, object]:
    value = found[name]
    assert isinstance(value, dict), f"{name} is {value!r}, not a box"
    return value


def meets(one: dict[str, object], other: dict[str, object]) -> bool:
    return (num(one["left"]) < num(other["right"]) - 0.5 and num(other["left"]) < num(one["right"]) - 0.5
            and num(one["top"]) < num(other["bottom"]) - 0.5 and num(other["top"]) < num(one["bottom"]) - 0.5)


def clear(desk: Desk, found: Json, names: list[str], where: str) -> None:
    for index, name in enumerate(names):
        for other in names[index + 1:]:
            desk.check(found[name] is not None, f"{where}: {name} is not drawn: {found}")
            desk.check(found[other] is not None, f"{where}: {other} is not drawn: {found}")
            desk.check(not meets(box_of(found, name), box_of(found, other)),
                       f"{where}: {name} covers {other}: {found[name]} / {found[other]}")


def open_channel(desk: Desk, touch: bool) -> None:
    page = desk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function("() => !document.getElementById('screen-welcome').hidden", timeout=3_000)
        page.click("#welcome-start")
    except Exception:
        pass  # no first-run Welcome on this path (#1781: a returning reader, a refusal, or a saved choice)
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    if touch:
        box = page.locator("#view-switch").first.bounding_box()
        desk.check(box is not None, "#view-switch is not drawn")
        assert box is not None
        page.touchscreen.tap(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
    else:
        page.click("#view-switch")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24",
                  "the channel did not open in All")
    desk.wait_for("() => document.getElementById('channel-freshness').textContent.startsWith('Live')",
                  "the pill does not say the channel is live")
    page.evaluate("() => { document.getElementById('page-update').hidden = false; }")
    desk.wait_for("() => !document.getElementById('page-update').hidden"
                  " && document.getElementById('page-update').getClientRects().length > 0",
                  "the reload chip did not appear beside the pill")
    desk.settled()


def top_walk(desk: Desk, *, stacked: bool) -> None:
    page = desk.page
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
    desk.wait_for("() => { const a = document.getElementById('scroll-area');"
                  " const b = [...document.querySelectorAll('#discord-log .thread-badge')]"
                  " .find((e) => e.getClientRects().length && e.getBoundingClientRect().bottom > a.getBoundingClientRect().top);"
                  " return Boolean(b); }", "no Thread(N) chip is drawn at the head of the list")
    desk.settled()
    found: Json = page.evaluate(MEASURE_JS)
    clear(desk, found, ["fresh", "update", "glass", "badge"], "the head of the list")
    hits = found["hits"]
    assert isinstance(hits, dict)
    desk.check(bool(hits["update"]) and bool(hits["glass"]) and bool(hits["badge"]),
               f"the head of the list: a top control is covered at its centre: {hits} in {found}")
    if stacked:
        fresh, update = box_of(found, "fresh"), box_of(found, "update")
        desk.check(num(update["top"]) >= num(fresh["bottom"]) - 0.5,
                   f"phone: the reload chip is not stacked under the pill: {fresh} / {update}")
        desk.notes.append("phone: the reload chip stacks under the pill at the left, clear of Thread(N) and the glass")
    else:
        fresh, update = box_of(found, "fresh"), box_of(found, "update")
        desk.check(abs((num(update["top"]) + num(update["bottom"])) - (num(fresh["top"]) + num(fresh["bottom"]))) < 2,
                   f"desk: the two top chips no longer share one line: {fresh} / {update}")
        desk.notes.append("desk: the two top chips still share one line, clear of Thread(N) and the glass")
    desk.shot("1-top-chips")
    if stacked:
        # The owner's alignment: a Thread(N) chip scrolled onto the pill's own line.
        page.evaluate("""() => {
            const area = document.getElementById('scroll-area');
            const pill = document.getElementById('channel-freshness');
            const badge = [...document.querySelectorAll('#discord-log .thread-badge')]
                .find((e) => e.getClientRects().length && e.getBoundingClientRect().bottom > area.getBoundingClientRect().top);
            area.scrollTop += badge.getBoundingClientRect().top - pill.getBoundingClientRect().top;
        }""")
        desk.wait_for(BADGE_ON_LINE_JS, "the Thread(N) chip did not land on the pill's line")
        desk.settled()
        found = page.evaluate(MEASURE_JS)
        clear(desk, found, ["fresh", "update", "glass", "badge"], "a Thread(N) chip on the pill's line")
        hits = found["hits"]
        assert isinstance(hits, dict)
        desk.check(bool(hits["update"]) and bool(hits["glass"]) and bool(hits["badge"]),
                   f"a Thread(N) chip on the pill's line: a control is covered at its centre: {hits} in {found}")
        desk.shot("2-top-aligned")
        desk.notes.append("phone: with Thread(N) on the pill's line, no top chip covers it and each control is hittable")


def toast_walk(desk: Desk) -> None:
    page = desk.page
    page.evaluate("""() => {
        document.getElementById('status').textContent = 'Agent voice on.';
        document.getElementById('status-line').hidden = false;
    }""")
    desk.wait_for("() => !document.getElementById('status-line').hidden"
                  " && document.getElementById('status').textContent === 'Agent voice on.'",
                  "the toast did not appear")
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    desk.wait_for(FOOT_CLEAR_JS, "the last card's reaction chips did not scroll clear of the toast", seconds=60)
    desk.settled()
    found: Json = page.evaluate(MEASURE_JS)
    desk.check(found["status"] is not None, f"the toast is not drawn: {found}")
    reactions = found["reactions"]
    assert isinstance(reactions, list)
    desk.check(len(reactions) >= 2, f"the last card draws no reaction chips: {found}")
    for chip in reactions:
        assert isinstance(chip, dict)
        desk.check(not meets(box_of(found, "status"), chip), f"the toast covers a reaction chip: {found['status']} / {chip}")
    hits = found["hits"]
    assert isinstance(hits, dict)
    desk.check(all(bool(one) for one in hits["reactions"]), f"a reaction chip is covered at its centre: {hits}")
    desk.check(bool(hits["dismiss"]), f"the toast's dismiss control is covered at its centre: {hits}")
    desk.shot("3-toast-foot")
    desk.notes.append("phone: at the foot the toast stands clear of the last card's reaction chips, each hittable")


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
         scale: float, touch: bool, stacked: bool) -> str:
    api = ChipApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-floating-chips-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark",
                device_scale_factor=scale, is_mobile=touch, has_touch=touch)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, label, width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            open_channel(desk, touch)
            top_walk(desk, stacked=stacked)
            if stacked:
                toast_walk(desk)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}, {label}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


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
    report = []
    with sync_playwright() as playwright:
        report.append(walk(playwright.chromium, args, "phone-412", 412, 915, 2.6, True, True))
        report.append(walk(playwright.chromium, args, "desk-1280", 1280, 800, 1.0, False, False))
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
