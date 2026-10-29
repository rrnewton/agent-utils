#!/usr/bin/env python3
"""Say whose messages the search toggle shows, on hover and on press, in real Chromium.

`#520 search-whose-toggle-hint`. The owner, 2026-10-25: the search bar's whose-messages toggle
(Everyone / Mine / Theirs, `#251 author-filter`) is unintuitive — the icons are liked once understood
but forgotten. The fake-DOM suite (tests/js/voice_page.test.mjs) pins the words the page writes;
this lays them out for real at the two sizes the owner reads at, 393x852 (a phone, where the button
hangs under the bar) and 1280x800 (a desk, where it stands on the bar's line):

  sign in and open the channel -> the search glass -> at each size, the author filter pressed three
  times round its cycle: after each press a chip under the button, in the page's chip dress, naming
  the state landed on ("My messages", "Their messages", "Both"), centred under the button and
  touching nothing's place — the bar and the first row stand where they stood — and gone again
  after its fade (~1.5 s up, then out) -> on the desk, a pointer resting on the button and then
  keyboard focus on it each raise the tooltip above it, saying what the control does and the state
  it is in; the same words are its `title` and its tooltip's text.

Pass --web-root DIR to run it against another copy of the page, and --screenshots DIR to keep a PNG
of the chip up at each size.
"""

from __future__ import annotations

import tempfile
import time
from pathlib import Path
from typing import TYPE_CHECKING

import search_dates_browser as week
from bookmark_browser import TOKEN, check, serve
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserContext, Page, Playwright

# (label, width, height, touch): the phone the owner reads on, and his desk.
SIZES = (("phone", 393, 852, True), ("desk", 1280, 800, False))

CHIP_JS = """() => {
  const box = (node) => {
    if (!node || !node.getClientRects().length) return null;
    const r = node.getBoundingClientRect();
    return {top: r.top, bottom: r.bottom, left: r.left, right: r.right};
  };
  const button = document.getElementById('author-filter');
  const chip = document.getElementById('author-filter-chip');
  const tip = document.getElementById('author-filter-tip');
  const first = [...document.querySelectorAll('#discord-log > li[data-id]')]
    .find((li) => li.getClientRects().length);
  const tipStyle = getComputedStyle(tip);
  return {
    state: button.getAttribute('data-author'),
    title: button.getAttribute('title'),
    label: button.getAttribute('aria-label'),
    chip: {hidden: chip.hidden, text: chip.textContent, box: box(chip),
           centre: chip.getBoundingClientRect().left + chip.getBoundingClientRect().width / 2},
    button: box(button),
    buttonCentre: button.getBoundingClientRect().left + button.getBoundingClientRect().width / 2,
    bar: box(document.getElementById('search-float')),
    first: first ? box(first) : null,
    tip: {text: tip.textContent, box: box(tip), display: tipStyle.display},
  };
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


def walk(page: Page, label: str, touch: bool, shots: Path | None, width: int = 1280) -> str:
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    until(page, f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {len(week.SCRIPT)}",
          f"{label}: the channel did not open with the week in it")
    settled(page)

    press(page, "#search-toggle", touch)
    page.wait_for_selector("#author-filter", state="visible", timeout=10_000)
    before = page.evaluate(CHIP_JS)

    landed = []
    for words in ("My messages", "Their messages", "Both"):
        press(page, "#author-filter", touch)
        until(page, f"() => document.getElementById('author-filter-chip').textContent === '{words}'"
                    " && !document.getElementById('author-filter-chip').hidden",
              f"{label}: pressing the author filter left no chip saying {words!r}")
        now = page.evaluate(CHIP_JS)
        chip, button = now["chip"], now["button"]
        check(chip["box"] is not None and button is not None, f"{label}: the chip has no box")
        if chip["box"] is not None and button is not None:
            check(chip["box"]["top"] >= button["bottom"] - 1,
                  f"{label}: the chip saying {words!r} is not under the button: {chip['box']} vs {button}")
            check(chip["box"]["left"] >= 0 and chip["box"]["right"] <= width,
                  f"{label}: the chip saying {words!r} runs off the screen: {chip['box']}")
            if not touch:
                check(abs(chip["centre"] - now["buttonCentre"]) <= 8,
                      f"{label}: the chip saying {words!r} is not centred under the button")
        # The bar did not move for it (the press itself re-filters the list, as it should).
        check(now["bar"] == before["bar"],
              f"{label}: the chip moved the bar: {before['bar']} -> {now['bar']}")
        landed.append(f"{now['state']}={words}")
        if shots is not None and words == "My messages":
            page.screenshot(path=str(shots / f"{label}-search-whose-chip.png"))
        before = now

    # It goes again: ~1.5 s up, its fade, then hidden.
    until(page, "() => document.getElementById('author-filter-chip').hidden",
          f"{label}: the chip never went away", seconds=6)

    if not touch:
        # A pointer resting on the button raises the tooltip under it, saying the state it is in.
        box = page.locator("#author-filter").bounding_box()
        check(box is not None, f"{label}: the author filter has no box to rest on")
        if box is not None:
            page.mouse.move(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
            until(page, "() => getComputedStyle(document.getElementById('author-filter-tip')).display === 'block'",
                  f"{label}: resting on the author filter raised no tooltip")
            now = page.evaluate(CHIP_JS)
            check(now["tip"]["display"] == "block" and now["tip"]["box"] is not None
                  and now["tip"]["box"]["top"] >= now["button"]["bottom"] - 1,
                  f"{label}: the tooltip is not under the button: {now['tip']}")
            check(now["tip"]["text"] == now["title"] and now["tip"]["text"].startswith("Whose messages:"),
                  f"{label}: the tooltip and the title disagree: {now['tip']['text']!r} vs {now['title']!r}")
            check(now["label"] == "Everyone's messages. Press for only yours",
                  f"{label}: the button's name does not say its state: {now['label']!r}")
            if shots is not None:
                page.screenshot(path=str(shots / f"{label}-search-whose-tooltip.png"))
            page.mouse.move(10, 10)
        # ...and reaching it with the keyboard raises it too (Tab from the field, as a reader's would).
        page.focus("#search-field")
        page.keyboard.press("Tab")
        until(page, "() => document.activeElement && document.activeElement.id === 'author-filter'"
                    " && getComputedStyle(document.getElementById('author-filter-tip')).display === 'block'",
              f"{label}: tabbing to the author filter raised no tooltip")
    return "; ".join(landed)


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
    web_root = page_sources.served(args.web_root)
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch in SIZES:
            api = week.WeekApi()
            server, thread, url = serve(api, web_root)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-search-whose-") as profile:
                    context = launch(playwright, args.browser_executable, profile, width, height, touch)
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(url, wait_until="load")
                    note = walk(page, label, touch, args.screenshots, width)
                    check(not errors, f"{label}: the page threw: {errors}")
                    version = context.browser.version if context.browser else "Chromium"
                    context.close()
                    report.append(f"{version} {label} {width}x{height}: {note}; chip gone after its fade")
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
