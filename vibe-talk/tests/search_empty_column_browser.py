#!/usr/bin/env python3
"""Keep the search's None-match line inside the message column, in real Chromium.

`#2268 vibe-talk-search-none-match-line-overflow`. On a desk, a search that matches nothing loaded
puts one sentence at the top of the scroll area ("Searched N messages in this browser ... None
match."). It stood beside the panes rather than in one, so it took the whole scroll area's width:
at 1280x800 it ran from left of the message column to past its right edge, one long line. This
lays it out for real at the two desk sizes the owner reads at and on a phone:

  sign in and open the channel -> the search glass -> a word no message says -> the None-match
  line is up, its words wrap (the sentence is wider than the column), and its box lies within the
  message column's left and right edges (`#pane-discord`, the column the cards stand in) -> where
  the Search All chip stands beside it (one column on Main), the chip lies within them too. No
  horizontal overflow and no page error.

1280x800 and 1920x1080 with a mouse, 412x915 by touch. Main before the change is the negative
control: there the line is as wide as the scroll area, so the desk sizes fail at the edge check.

Pass --web-root DIR to run it against another copy of the page, and --screenshots DIR to keep a
PNG of each size.
"""

from __future__ import annotations

import tempfile
import time
from pathlib import Path
from typing import TYPE_CHECKING

import search_dates_browser as week
from bookmark_browser import TOKEN, check, serve
import page_sources
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page, Playwright

# (label, width, height, touch).
SIZES = (("desk", 1280, 800, False), ("wide-desk", 1920, 1080, False), ("phone", 412, 915, True))

# No message of the week says this.
NOTHING = "zebrafinchquill"

# Half a CSS pixel: the edges are whole pixels on both sides of the comparison (`R11`).
SLACK = 0.5

GEOMETRY_JS = """() => {
  const box = (node) => {
    if (!node || node.hidden || !node.getClientRects().length) return null;
    const r = node.getBoundingClientRect();
    return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height};
  };
  const empty = document.getElementById('search-empty');
  const line = parseFloat(getComputedStyle(empty).lineHeight) || 0;
  return {
    text: empty.textContent.trim(),
    empty: box(empty),
    lineHeight: line,
    chip: box(document.getElementById('search-in-all')),
    column: box(document.getElementById('pane-discord')),
    scroller: box(document.getElementById('scroll-area')),
    overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
  };
}"""


def settled(page: Page) -> None:
    """Two frames (`#1118 browser-fixed-frame-settle`), after `until` has pinned the state."""
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


def within(label: str, what: str, inner: dict[str, float], column: dict[str, float]) -> None:
    check(inner["left"] >= column["left"] - SLACK and inner["right"] <= column["right"] + SLACK,
          f"{label}: {what} runs past the message column: x {inner['left']:.1f}-{inner['right']:.1f}"
          f" against the column's x {column['left']:.1f}-{column['right']:.1f}")


def walk(page: Page, label: str, touch: bool, shots: Path | None) -> str:
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function("() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden", timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass  # no first-run Welcome on this path (#1781: a returning reader, a refusal, or a saved choice)
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    until(page, f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {len(week.SCRIPT)}",
          f"{label}: the channel did not open with the week in it")
    settled(page)

    press(page, "#search-toggle", touch)
    page.wait_for_selector("#search-field", state="visible", timeout=10_000)
    page.fill("#search-field", NOTHING)
    until(page, "() => !document.getElementById('search-empty').hidden"
                " && /None match/.test(document.getElementById('search-empty').textContent)",
          f"{label}: a search nothing matches left no None-match line")
    settled(page)
    now = page.evaluate(GEOMETRY_JS)
    empty, column = now["empty"], now["column"]
    check(empty is not None and column is not None, f"{label}: the line or the column has no box: {now}")
    if empty is None or column is None:
        return "no geometry"
    within(label, "the None-match line", empty, column)
    # Long enough to wrap: the sentence takes more than one line in the column, so the edge check
    # above is about a wrapping line, not a short one that would fit anywhere.
    check(now["lineHeight"] > 0 and empty["height"] >= 1.5 * now["lineHeight"],
          f"{label}: the None-match line does not wrap ({empty['height']:.1f} px tall at a"
          f" {now['lineHeight']:.1f} px line): {now['text']!r}")
    check(now["overflow"] <= 0, f"{label}: the page is {now['overflow']} px wider than the window")
    if shots is not None:
        page.screenshot(path=str(shots / f"{label}-search-empty-column.png"))
    # Main, one column: the same line, and the Search All chip under it (`#262 search-load-older`).
    page.select_option("#thread-select", "main")
    until(page, "() => document.getElementById('thread-select').value === 'main'"
                " && !document.getElementById('search-empty').hidden"
                " && !document.getElementById('search-in-all').hidden",
          f"{label}: Main with the search kept shows no None-match line and Search All chip")
    settled(page)
    main = page.evaluate(GEOMETRY_JS)
    check(main["empty"] is not None and main["chip"] is not None and main["column"] is not None,
          f"{label}: in Main the line, the chip or the column has no box: {main}")
    if main["empty"] is None or main["chip"] is None or main["column"] is None:
        return "no geometry in Main"
    within(label, "the None-match line in Main", main["empty"], main["column"])
    within(label, "the Search All chip", main["chip"], main["column"])
    check(main["overflow"] <= 0, f"{label}: in Main the page is {main['overflow']} px wider than the window")
    chip_note = (f"Main: line x {main['empty']['left']:.0f}-{main['empty']['right']:.0f}, chip x"
                 f" {main['chip']['left']:.0f}-{main['chip']['right']:.0f}")
    if shots is not None:
        page.screenshot(path=str(shots / f"{label}-search-empty-column-main.png"))
    return (f"line x {empty['left']:.0f}-{empty['right']:.0f}, {empty['height']:.0f} px tall, in column"
            f" x {column['left']:.0f}-{column['right']:.0f} (scroll area x {now['scroller']['left']:.0f}-"
            f"{now['scroller']['right']:.0f}); {chip_note}")


def launch(playwright: Playwright, browser: str | None, profile: str, width: int, height: int,
           touch: bool) -> BrowserContext:
    if touch:
        context = playwright.chromium.launch_persistent_context(
            profile, headless=True, executable_path=browser, timezone_id=week.ZONE,
            viewport={"width": width, "height": height}, color_scheme="dark",
            device_scale_factor=2.625, is_mobile=True, has_touch=True,
        )
    else:
        context = playwright.chromium.launch_persistent_context(
            profile, headless=True, executable_path=browser, timezone_id=week.ZONE,
            viewport={"width": width, "height": height}, color_scheme="dark",
        )
    browser_waits.hot_host_timeouts(context)
    return context


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
    web_root = page_sources.served(args.web_root)
    report: list[str] = []
    failed: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch in SIZES:
            api = week.WeekApi()
            server, thread, url = serve(api, web_root)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-search-empty-") as profile:
                    context = launch(playwright, args.browser_executable, profile, width, height, touch)
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(url, wait_until="load")
                    version = context.browser.version if context.browser else "Chromium"
                    # Every size is walked and said, a red one included, so the negative control
                    # (main before the change) shows each size it fails at, not only the first.
                    try:
                        note = walk(page, label, touch, args.screenshots)
                        check(not errors, f"{label}: the page threw: {errors}")
                    except AssertionError as error:
                        failed.append(f"{version} {label} {width}x{height}: FAIL {error}")
                    else:
                        report.append(f"{version} {label} {width}x{height}: {note}")
                    context.close()
            finally:
                api.stopping.set()
                server.shutdown()
                server.server_close()
                thread.join()
    for line in report + failed:
        print(line)
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
