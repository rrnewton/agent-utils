#!/usr/bin/env python3
"""Links always work: a link to a message older than what is held lands as an island (`#2426` R3).

The owner (request 383): every message link lands. Before this, a link walked back at most five pages
and then said "Older than what is loaded." This walk serves the checked-out page and the islands
fake API (a threading provider, one channel of 1,200 messages in All, newest pages of 50, a
backward cursor and the id-based `around` read; islands_browser.py) from one loopback origin, opens
a deep link to a message about 1,000 messages back, and checks at 412x915 by touch and 1280x800,
by counting:

  * while the read is out, its box stands on screen and no floating note stands over any row's words
    (the status is announced, not floated: liaison 1150);
  * the target lands with exactly ONE `around` read (ten each way) and ZERO steps back;
  * its row stands on screen, with a seam between its island and the newest window ("Messages not
    loaded": the page does not know how many lie between), and no window spacer;
  * from the target, scrolling up steps back from the island (the served cursor its answer brought),
    and the target row moves at most 1.0 px while that step lands;
  * Newest then lands on the newest message with zero timeline reads;
  * when the around read fails, the boxes go and the reader stays where they were (at the foot), within
    one pixel, before the link falls back to the walk back.

Negative control: against main's page (no around read) the link walks back five pages and fails.
Counts and geometry; waits are on page conditions. Pass --web-root DIR for another page copy.
"""

from __future__ import annotations

import argparse
import os
import re
import sys
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import browser_waits
import offline_cache_browser as fixture
import page_sources
from islands_browser import ARM_JS, CENSUS_JS, DISARM_JS, IslandApi, frames, island_handler, quiet, settle, worst_move
from offline_cache_browser import TOKEN, check, handler_for

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

TARGET_INDEX = 200

# Floating notes (the status pill, the freshness line) that stand over any row's words in the list:
# each one's words and the row text it covers. Clipped-to-nothing notes (announced, not floated) cover none.
FLOATING_OVER_TEXT_JS = """() => {
  const notes = ['status-line', 'channel-freshness'].map((id) => document.getElementById(id)).filter((n) => {
    if (!n || n.hidden) return false;
    const cs = getComputedStyle(n); const r = n.getBoundingClientRect();
    return (cs.position === 'absolute' || cs.position === 'fixed') && cs.visibility !== 'hidden' && r.width > 2 && r.height > 2;
  });
  const texts = [...document.querySelectorAll('#discord-log > li .body, #discord-log > li .meta')];
  const hits = [];
  for (const n of notes) {
    const a = n.getBoundingClientRect();
    for (const t of texts) {
      const b = t.getBoundingClientRect();
      if (b.width > 0 && b.height > 0 && a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom) {
        hits.push({ note: n.id, words: n.textContent.trim().slice(0, 40), row: (t.closest('li') || {}).getAttribute ? t.closest('li').getAttribute('data-id') : null });
        break;
      }
    }
  }
  return hits;
}"""


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, touch: bool) -> list[str]:
    api = IslandApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread_ = threading.Thread(target=server.serve_forever, daemon=True)
    thread_.start()
    target = str(fixture.message(TARGET_INDEX, "")["id"])
    channel = fixture.CHANNEL["id"]
    notes: list[str] = []
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-link-island-") as profile:
            options: dict[str, object] = {"viewport": {"width": width, "height": height}, "color_scheme": "dark"}
            if touch:
                options.update(device_scale_factor=2.625, is_mobile=True, has_touch=True)
            context = chromium.launch_persistent_context(profile, headless=True, executable_path=args.browser_executable,
                                                         **options)  # type: ignore[arg-type]
            browser_waits.hot_host_timeouts(context)
            context.add_init_script("try { localStorage.setItem('vibe-talk.welcome.dismissed', '1'); } catch (e) {}")
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            base = f"http://127.0.0.1:{server.server_port}/"
            page.goto(base, wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            browser_waits.wait_until(page, "() => document.querySelectorAll('#discord-log > li[data-id]').length > 5",
                                     f"{label}: the channel did not open")
            settle(page)
            reads = len(api.read_log)
            # The around read held: the link's box must stand on screen at once, before any answer.
            api.hold.clear()
            page.goto(f"{base}?c={channel}&v=all&m=discord:{channel}:{target}", wait_until="load")
            box = None
            for _ in range(900):
                box = page.evaluate("""() => { const b = document.querySelector('#discord-log > li.link-box');
                  if (!b) return null; const r = b.getBoundingClientRect(); const a = document.getElementById('scroll-area').getBoundingClientRect();
                  return { top: r.top, h: r.height, on: r.bottom > a.top && r.top < a.bottom, words: b.textContent.trim() }; }""")
                if box and box["on"] and api.count("around") > 0:
                    break
                frames(page, 1)
            if box is None or not box["on"]:
                api.hold.set()
                raise AssertionError(f"{label}: no box stood on screen while the link's read was out: {box}; reads {api.read_log[reads:]}")
            # Nothing floats over the rows' words while the read is out (liaison 1150): the box says it,
            # and the status is announced (a polite live region), not floated.
            covered = page.evaluate(FLOATING_OVER_TEXT_JS)
            check(not covered, f"{label}: a floating note stands over row text while the link's read is out: {covered}")
            if args.screenshots:
                page.screenshot(path=str(args.screenshots / f"{label}-link-box.png"))
            api.hold.set()
            landed = False
            for _ in range(900):
                if page.evaluate(f"""() => {{ const li = document.querySelector('#discord-log > li[data-id="{target}"]');
                    if (!li) return false; const r = li.getBoundingClientRect(); const a = document.getElementById('scroll-area').getBoundingClientRect();
                    return r.bottom > a.top && r.top < a.bottom; }}"""):
                    landed = True
                    break
                frames(page, 1)
            asked = api.read_log[reads:]
            status = page.evaluate("() => (document.getElementById('status') || {}).textContent || ''")
            check(landed, f"{label}: the link to message {target} never landed on screen; reads {asked}; status {status!r}")
            settle(page)
            asked = api.read_log[reads:]
            arounds = [one for one in asked if one.startswith("around")]
            befores = [one for one in asked if one.startswith("before")]
            check(arounds == ["around:10:10"], f"{label}: the link asked {arounds}, not one around read ten each way")
            check(not befores, f"{label}: the link walked back: {befores}")
            census = page.evaluate(CENSUS_JS)
            check(census["spacers"] == 0, f"{label}: a window spacer stands")
            check(len(census["seams"]) >= 1 and "not loaded" in census["seams"][0],
                  f"{label}: no seam between the link's island and the newest window: {census['seams']}")
            rect = page.evaluate(f"""() => {{ const r = document.querySelector('#discord-log > li[data-id="{target}"]').getBoundingClientRect();
                return {{ top: r.top, h: r.height }}; }}""")
            box_mid = box["top"] + box["h"] / 2
            row_mid = rect["top"] + rect["h"] / 2
            # The seam below the link's island has no count: the jump to the newest says a floor (liaison 1150 (B)(a)).
            jump = str(page.evaluate("() => document.getElementById('jump-newest-label').textContent || ''"))
            check(re.search(r"\(\d+\+\)$", jump) is not None, f"{label}: the jump says {jump!r}, not a floor ('(N+)') over the uncounted seam")
            notes.append(f"link to {target}: box stood at once ({box['words']!r}); landed with {arounds} and no step back; jump {jump!r}; "
                         f"the message took the box's place (tops {rect['top']:.1f} vs {box['top']:.1f}, box {box['h']:.0f} px tall, "
                         f"centres {row_mid:.1f} vs {box_mid:.1f}); seams {census['seams']}")
            if args.screenshots:
                page.screenshot(path=str(args.screenshots / f"{label}-link-landed.png"))
            # Up from the target: a step back from the island, by the cursor its answer brought.
            quiet(page)
            asked_before = api.count("before")
            api.hold.clear()
            page.locator("#load-older").click()
            waited = 0
            while api.count("before") == asked_before and waited < 300:
                frames(page, 1)
                waited += 1
            check(api.count("before") > asked_before, f"{label}: no step back from the link's island; reads {api.read_log[-3:]}")
            frames(page, 2)
            top = page.evaluate(f"""() => {{ const li = document.querySelector('#discord-log > li[data-id="{target}"]');
                return li ? li.getBoundingClientRect().top : null; }}""")
            page.evaluate(ARM_JS, target)
            api.hold.set()
            settle(page)
            frames(page, 2)
            moved = worst_move(page.evaluate(DISARM_JS), top)
            check(moved <= 1.0, f"{label}: the step back moved the link's target {moved:.2f} px")
            notes.append(f"step back from the island: {api.read_log[-1]}, target moved at most {moved:.2f} px")
            # Newest: the newest window is still held, so no read.
            reads = len(api.read_log)
            page.keyboard.press("End")
            browser_waits.wait_until(page, """() => { const a = document.getElementById('scroll-area');
                return a.scrollHeight - a.scrollTop - a.clientHeight < 4; }""", f"{label}: End did not reach the foot")
            settle(page)
            newest = str(fixture.message(len(api.messages) - 1, "")["id"])
            last = page.evaluate("() => { const rows = document.querySelectorAll('#discord-log > li[data-id]'); "
                                 "return rows.length ? rows[rows.length - 1].getAttribute('data-id') : null; }")
            check(last == newest, f"{label}: the foot is {last}, not the newest {newest}")
            timeline = [one for one in api.read_log[reads:] if one in ("newest",) or one.startswith(("before", "around"))]
            check(not timeline, f"{label}: Newest read the timeline: {timeline}")
            notes.append("Newest after the link: the newest message with 0 timeline reads")
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
    finally:
        api.stopping.set()
        api.hold.set()
        server.shutdown()
        server.server_close()
        thread_.join()
    return [f"{label}: {note}" for note in notes]


def failed_walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, touch: bool) -> list[str]:
    """The around read fails: the boxes go and the reader stays where they were (at the foot, where the
    channel opened), within one pixel, as on main; the link then falls back to the walk back."""
    api = IslandApi()
    api.error_around = True
    server = ThreadingHTTPServer(("127.0.0.1", 0), island_handler(api))
    server.daemon_threads = True
    thread_ = threading.Thread(target=server.serve_forever, daemon=True)
    thread_.start()
    target = str(fixture.message(TARGET_INDEX, "")["id"])
    channel = fixture.CHANNEL["id"]
    notes: list[str] = []
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-link-island-failed-") as profile:
            options: dict[str, object] = {"viewport": {"width": width, "height": height}, "color_scheme": "dark"}
            if touch:
                options.update(device_scale_factor=2.625, is_mobile=True, has_touch=True)
            context = chromium.launch_persistent_context(profile, headless=True, executable_path=args.browser_executable,
                                                         **options)  # type: ignore[arg-type]
            browser_waits.hot_host_timeouts(context)
            context.add_init_script("try { localStorage.setItem('vibe-talk.welcome.dismissed', '1'); } catch (e) {}")
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            base = f"http://127.0.0.1:{server.server_port}/"
            page.goto(base, wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            browser_waits.wait_until(page, "() => document.querySelectorAll('#discord-log > li[data-id]').length > 5",
                                     f"{label}: the channel did not open")
            settle(page)
            # The read answers an error at once: the box stands, goes, and the walk back takes over.
            page.goto(f"{base}?c={channel}&v=all&m=discord:{channel}:{target}", wait_until="load")
            browser_waits.wait_until(page, "() => /Further back|Older than|Could not/.test((document.getElementById('status') || {}).textContent || '')",
                                     f"{label}: the failed link never said so")
            settle(page)
            check(api.count("around-error") >= 1, f"{label}: the link asked no around read")
            gap = page.evaluate("""() => { const a = document.getElementById('scroll-area');
                return a.scrollHeight - a.scrollTop - a.clientHeight; }""")
            newest = str(fixture.message(len(api.messages) - 1, "")["id"])
            on = page.evaluate(f"""() => {{ const li = document.querySelector('#discord-log > li[data-id="{newest}"]');
                if (!li) return false; const r = li.getBoundingClientRect(); const a = document.getElementById('scroll-area').getBoundingClientRect();
                return r.bottom > a.top && r.top < a.bottom; }}""")
            check(gap <= 1.0 and on, f"{label}: after the failed read the reader is {gap:.1f} px above the foot (newest on screen: {on}), not where they were")
            boxes = page.evaluate("() => document.querySelectorAll('#discord-log > li.link-box, #discord-log > li.link-box-neighbour').length")
            check(boxes == 0, f"{label}: {boxes} link boxes still stand after the failed read")
            check(not errors, f"{label}: the page threw: {errors}")
            notes.append(f"failed around read: the boxes went and the reader stayed at the foot ({gap:.1f} px), then the walk back said so")
            context.close()
    finally:
        api.stopping.set()
        api.hold.set()
        server.shutdown()
        server.server_close()
        thread_.join()
    return [f"{label}: {note}" for note in notes]


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"))
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT)
    parser.add_argument("--screenshots", type=Path)
    return parser.parse_args()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required") from error
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch in (("phone-412", 412, 915, True), ("desk-1280", 1280, 800, False)):
            report += walk(playwright.chromium, args, label, width, height, touch)
            report += failed_walk(playwright.chromium, args, label, width, height, touch)
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    sys.exit(main())
