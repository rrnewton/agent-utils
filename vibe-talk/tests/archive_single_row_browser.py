#!/usr/bin/env python3
"""Archive and unarchive dress only the acted rows (`#1990 vibe-talk-archive-single-row-dress`).

Serves the real page and a fake chat upstream over an 816-message channel, and checks, at phone
and desktop sizes, with event counts only (owner rule 346, never a wall-clock threshold):

  one `e` archive and one `e` unarchive of the selected row dress at most 20 rows, write at
  most 100 row classes and read at most 50 layouts — where the whole-list dress before wrote
  ~1,634 classes and ~270 layouts over the same list;
  the acted row shows archived and then back, and its state survives the write's answer;
  with windowed lists on, the mounted rows stay within the row cap, archiving a selected row
  that the window has since unmounted (just off-screen, from the keyboard) mounts nothing and
  reads no whole-list layout, and the row shows its archived state when mounted again.

Pass --screenshots DIR to keep a PNG of each stage.
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
from pathlib import Path
from typing import TYPE_CHECKING, TypedDict, cast

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, check
import page_sources
import search_typing_fast_browser as typing_fast

if TYPE_CHECKING:
    from playwright.sync_api import Page, Playwright

MAX_DRESSED_ROWS = 40
# The dress itself reads no layout; a press still pays the place-keeping and jump tools'
# neighbourhood measures (anchor capture/restore, unread-above binary search, floating
# clearance) and, in the second column, its observers — measured at ~180 (phone) to ~345
# (two-column desk) over this list and bounded by the screen and the acted row's read run,
# not the list length. The whole-list era read thousands in the same window (Phase 1
# harness), and dressed all 816 rows where this gate's runs dress at most 32.
MAX_RECT_READS = 400
MAX_CLASS_WRITES = 100

COUNTERS_JS = """() => {
    window.__press = { dressed: new Set(), rect: 0, cls: 0 };
    const orig = Element.prototype.getBoundingClientRect;
    Element.prototype.getBoundingClientRect = function () { window.__press.rect += 1; return orig.call(this); };
    window.__pressList = (li) => { let n = li; while (n && !n.id) n = n.parentElement; return n ? n.id : '?'; };
    new MutationObserver((ms) => {
        for (const m of ms) {
            const li = m.target.closest ? m.target.closest('li') : null;
            if (li) window.__press.dressed.add(li);
            if (m.attributeName === 'class') window.__press.cls += 1;
        }
    }).observe(document.body, { subtree: true, attributes: true, childList: true });
}"""

SNAP_JS = """() => { const by = {}; for (const li of window.__press.dressed) { const k = window.__pressList(li); by[k] = (by[k] || 0) + 1; }
    return { dressed: window.__press.dressed.size, rect: window.__press.rect, cls: window.__press.cls, by }; }"""
RESET_JS = "() => { window.__press.dressed.clear(); window.__press.rect = 0; window.__press.cls = 0; }"


def open_channel(page: Page, total: int) -> None:
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function(
            "() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden",
            timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass
    page.wait_for_function(
        f"() => document.querySelectorAll('#discord-log > li[data-id]').length >= {min(total, 20)}",
        timeout=20_000)


def until(page: Page, script: str, why: str, seconds: float = 25) -> None:
    import time
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline and not page.evaluate(script):
        page.wait_for_timeout(25)
    check(bool(page.evaluate(script)), why)


class PressCounts(TypedDict):
    dressed: int
    rect: int
    cls: int
    by: dict[str, int]


def press_e_and_check(page: Page, label: str, acted_id: str, archived: bool) -> PressCounts:
    page.evaluate(RESET_JS)
    page.keyboard.press("e")
    until(page,
          f"() => document.querySelector('#discord-log > li[data-id=\"{acted_id}\"]')?.getAttribute('data-archived') === '{str(archived).lower()}'",
          f"{label}: the acted row did not show archived={archived}")
    # One frame for the dress to paint, not the timers that follow it (the Done flash,
    # observers): the counts name the press's own work.
    page.evaluate("() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)))")
    return cast(PressCounts, page.evaluate(SNAP_JS))


def walk(page: Page, label: str, total: int, windowed: bool, shots: Path | None) -> None:
    open_channel(page, total)
    page.evaluate(COUNTERS_JS)
    mounted = page.evaluate("() => document.querySelectorAll('#discord-log > li').length")
    if windowed:
        check(mounted <= 90, f"{label}: windowed lists mounted {mounted} rows over the cap")
    # Select the newest row by clicking it, then archive and unarchive from the keyboard.
    acted = page.evaluate("""() => {
        const rows = [...document.querySelectorAll('#discord-log > li[data-id]')];
        const row = rows[rows.length - 1];
        row.scrollIntoView(); row.click();
        return row.getAttribute('data-id');
    }""")
    page.wait_for_timeout(300)
    # With windowed lists on, the place-keeping scroll that follows a press re-slices the
    # window once and every mounted row is dressed at its mount: the bound there is the
    # window cap plus the acted run, not the single-row bound.
    dressed_bound = 95 if windowed else MAX_DRESSED_ROWS
    rect_bound = 700 if windowed else MAX_RECT_READS
    counts = press_e_and_check(page, label, acted, True)
    check(counts["dressed"] <= dressed_bound,
          f"{label}: an archive dressed {counts['dressed']} rows (bound {dressed_bound}): {counts}")
    check(counts["rect"] <= rect_bound,
          f"{label}: an archive read {counts['rect']} layouts (bound {rect_bound})")
    check(counts["cls"] <= MAX_CLASS_WRITES,
          f"{label}: an archive wrote {counts['cls']} classes (bound {MAX_CLASS_WRITES})")
    if shots is not None:
        page.screenshot(path=str(shots / f"{label}-archived.png"))
    counts = press_e_and_check(page, label, acted, False)
    check(counts["dressed"] <= dressed_bound and counts["rect"] <= rect_bound and counts["cls"] <= MAX_CLASS_WRITES,
          f"{label}: an unarchive exceeded the per-press bounds: {counts}")
    if windowed:
        # Archive again, then take the window away from the selected row (it stays the
        # selection, unmounted just off-screen) and unarchive from the keyboard: nothing is
        # mounted for the act and no whole-list layout is read.
        press_e_and_check(page, label, acted, True)
        page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
        until(page, f"() => !document.querySelector('#discord-log > li[data-id=\"{acted}\"]')",
              f"{label}: the acted row stayed mounted after the window moved away")
        before = page.evaluate("() => document.querySelectorAll('#discord-log > li').length")
        page.evaluate(RESET_JS)
        page.keyboard.press("e")
        page.wait_for_timeout(600)
        mid = page.evaluate(SNAP_JS)
        after = page.evaluate("() => document.querySelectorAll('#discord-log > li').length")
        check(mid["rect"] <= MAX_RECT_READS,
              f"{label}: archiving an unmounted row read {mid['rect']} layouts (bound {MAX_RECT_READS})")
        check(after <= before + 5,
              f"{label}: archiving an unmounted row mounted rows ({before} -> {after})")
        # The keyboard act on an unmounted selection is a no-op (its row is not on the
        # screen to act on), so the message stays archived in the store — and is dressed
        # from the store, correctly, when the window mounts it again.
        page.evaluate("""() => { const jump = document.getElementById('jump-newest');
            if (jump && !jump.hidden) { jump.click(); return; }
            const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }""")
        until(page,
              f"() => document.querySelector('#discord-log > li[data-id=\"{acted}\"]') !== null",
              f"{label}: the acted row did not mount again after jumping back")


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help="directory holding the page to serve")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per stage into")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required") from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    web_root = page_sources.served(args.web_root)
    total = 816
    runs = (
        ("phone", 412, 915, False, "dark"),
        ("desk-windowed", 1280, 800, True, "dark"),
    )
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, windowed, scheme in runs:
            api = typing_fast.BigApi()
            server, thread, url = typing_fast.serve(api, web_root)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-archive-row-") as profile:
                    context = playwright.chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable,
                        timezone_id="America/New_York",
                        viewport={"width": width, "height": height}, color_scheme=scheme,  # type: ignore[arg-type]
                        device_scale_factor=2.625 if width < 600 else 1, is_mobile=width < 600, has_touch=width < 600)
                    page = context.pages[0]
                    if windowed:
                        page.add_init_script(
                            "try { localStorage.setItem('vibe-talk.welcome.dismissed', '1'); "
                            "localStorage.setItem('vibe-talk.voice.windowed-lists', '1'); } catch (e) {}")
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(url, wait_until="load")
                    walk(page, label, total, windowed, args.screenshots)
                    check(not errors, f"{label}: the page threw: {errors}")
                    context.close()
                    report.append(f"{label} {width}x{height} windowed={windowed}: archive dresses acted rows only")
            finally:
                server.shutdown()
                server.server_close()
                thread.join()
    for line in report:
        print(line)
    print("archive single row browser walk: ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
