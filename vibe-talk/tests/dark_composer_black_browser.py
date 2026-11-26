#!/usr/bin/env python3
"""`#2294 vibe-talk-dark-composer-black`: in dark the message box is the message cards' black.

The owner, 2026-11-26: "I would like the black background to apply to the COMPOSE window too
because I look at that a lot." Since `#2154` the dark message cards are true black (`--theirs`);
the boxes the reader writes in took a grey (`--panel`; Discuss's field `--row-hover`). In dark
they now take the cards' own token, so a theme that repaints the cards repaints the boxes with
them; light is unchanged.

Here, in real Chromium at 412x915 by touch and 1280x800, against the real page served from the
offline fixture: the channel box's computed background equals a drawn message card's, unfocused,
focused with words in it and (phone) with the keyboard up; every other box the reader writes in
(the thread box, the call's box, the reply box, the quick reply, Discuss's "Ask about this")
resolves to the same; each box's words and placeholder keep 4.5:1 on it. In light, system or
forced, every box keeps its own light surface. A forced Dark on a light system and a forced
Light on a dark system are walked too, since the rule keys on the theme the page paints
(`data-theme`), not on the system's scheme. Condition waits only (owner rule 346).
"""

from __future__ import annotations

import argparse
import json
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING, Literal

import browser_waits
import offline_cache_browser as fixture
import page_sources
from offline_cache_browser import TOKEN, FakeApi, check, handler_for

if TYPE_CHECKING:
    from playwright.sync_api import Browser, BrowserContext, Page

STATE_MS = 30_000
KEYBOARD_PX = 336
# Every box the reader writes in: the channel's, the thread's, the call's, the reply in the stream,
# the quick reply and Discuss's "Ask about this", each with the light surface it keeps (`--panel`;
# Discuss's field `--row-hover`).
LIGHT_PANEL = "rgb(255, 255, 255)"
LIGHT_SURFACE = {
    "channel-compose-text": LIGHT_PANEL,
    "side-compose-text": LIGHT_PANEL,
    "compose-text": LIGHT_PANEL,
    "reply-text": LIGHT_PANEL,
    "quick-reply-text": LIGHT_PANEL,
    "discuss-followup": "rgb(237, 241, 247)",
}
BOXES = tuple(LIGHT_SURFACE)
DARK_CARD = "rgb(0, 0, 0)"
Scheme = Literal["dark", "light"]
# (label, system scheme, stored Appearance choice or None, painted theme)
CASES: tuple[tuple[str, Scheme, str | None, Scheme], ...] = (
    ("system-dark", "dark", None, "dark"),
    ("system-light", "light", None, "light"),
    ("forced-dark-on-light", "light", "dark", "dark"),
    ("forced-light-on-dark", "dark", "light", "light"),
)
SIZES = (("phone-412", 412, 915, True), ("desk-1280", 1280, 800, False))

READ_JS = """(ids) => {
  const card = [...document.querySelectorAll('#discord-log li.discord-message')]
    .find((li) => li.getClientRects().length > 0);
  const out = {theme: document.documentElement.getAttribute('data-theme'),
               card: card ? getComputedStyle(card).backgroundColor : null, boxes: {}};
  for (const id of ids) {
    const el = document.getElementById(id);
    if (!el) { out.boxes[id] = null; continue; }
    const s = getComputedStyle(el);
    out.boxes[id] = {background: s.backgroundColor, ink: s.color, border: s.borderTopColor,
                     placeholder: getComputedStyle(el, '::placeholder').color};
  }
  return out;
}"""


def channels(colour: str) -> tuple[float, float, float, float]:
    """A computed colour as 0-255 channels and alpha: `rgb()`/`rgba()`, or `color(srgb ...)` (0-1)."""
    inside = colour[colour.index("(") + 1:colour.rindex(")")]
    srgb = inside.startswith("srgb")
    parts = [float(p) for p in inside.removeprefix("srgb").replace("/", " ").replace(",", " ").split()]
    scale = 255.0 if srgb else 1.0
    alpha = parts[3] if len(parts) > 3 else 1.0
    return parts[0] * scale, parts[1] * scale, parts[2] * scale, alpha


def over(top: str, ground: str) -> tuple[float, float, float]:
    """`top` composited over the opaque `ground`, as the eye sees it."""
    r, g, b, a = channels(top)
    gr, gg, gb, _ = channels(ground)
    return r * a + gr * (1 - a), g * a + gg * (1 - a), b * a + gb * (1 - a)


def luminance(rgb: tuple[float, float, float]) -> float:
    def linear(c: float) -> float:
        c /= 255.0
        return c / 12.92 if c <= 0.03928 else ((c + 0.055) / 1.055) ** 2.4
    r, g, b = rgb
    return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)


def contrast(ink: str, ground: str) -> float:
    """WCAG contrast of `ink` (alpha composited) on the opaque `ground`."""
    hi, lo = sorted((luminance(over(ink, ground)), luminance(over(ground, ground))), reverse=True)
    return (hi + 0.05) / (lo + 0.05)


def open_page(browser: Browser, url: str, scheme: Scheme, choice: str | None, width: int, height: int,
              phone: bool) -> tuple[BrowserContext, Page]:
    context = browser.new_context(
        viewport={"width": width, "height": height}, color_scheme=scheme,
        device_scale_factor=2.6 if phone else 1, is_mobile=phone, has_touch=phone,
    )
    browser_waits.hot_host_timeouts(context)
    page = context.new_page()
    browser_waits.hot_host_timeouts(page)
    stored = {"vibe-talk.token": TOKEN}
    if choice:
        stored["vibe-talk.voice.appearance"] = choice
    page.add_init_script(
        "try { for (const [k, v] of Object.entries(" + json.dumps(stored) + ")) localStorage.setItem(k, v); }"
        " catch (e) {}")
    page.goto(url, wait_until="load")
    try:
        page.wait_for_function(
            "() => !document.getElementById('screen-welcome').hidden"
            " || !document.getElementById('screen-main').hidden", timeout=5_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass  # no first-run Welcome on this path
    page.wait_for_selector("#channel-compose-text", state="visible", timeout=STATE_MS)
    page.wait_for_function(
        "() => [...document.querySelectorAll('#discord-log li.discord-message')]"
        ".some((li) => li.getClientRects().length > 0)", timeout=STATE_MS)
    return context, page


def judge(where: str, painted: str, drawn: object, report: list[str]) -> None:
    check(isinstance(drawn, dict), f"{where}: the page read back {drawn!r}")
    assert isinstance(drawn, dict)
    check(drawn.get("theme") == painted, f"{where}: the page painted {drawn.get('theme')}, expected {painted}")
    boxes, card = drawn.get("boxes"), drawn.get("card")
    assert isinstance(boxes, dict)
    for box in BOXES:
        got = boxes.get(box)
        check(isinstance(got, dict), f"{where}: #{box} is missing from the page")
        assert isinstance(got, dict)
        if painted == "dark":
            check(card == DARK_CARD, f"{where}: the drawn card is {card}, not the dark cards' black")
            check(got["background"] == card, f"{where}: #{box} is {got['background']}, not the card's {card}")
        else:
            check(got["background"] == LIGHT_SURFACE[box],
                  f"{where}: light #{box} changed to {got['background']} (it keeps {LIGHT_SURFACE[box]})")
        ground = str(got["background"])
        ink = contrast(str(got["ink"]), ground)
        hint = contrast(str(got["placeholder"]), ground)
        edge = contrast(str(got["border"]), ground)
        if painted == "dark":
            check(ink >= 4.5, f"{where}: #{box}'s words are {ink:.2f}:1 on it, under 4.5:1")
            check(hint >= 4.5, f"{where}: #{box}'s placeholder is {hint:.2f}:1 on it, under 4.5:1")
        if box in ("channel-compose-text", "discuss-followup"):
            report.append(f"{where}: #{box} {ground} card {card}; words {got['ink']} {ink:.2f}:1, "
                          f"placeholder {got['placeholder']} {hint:.2f}:1, border {got['border']} {edge:.2f}:1")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--browser-executable", default=None)
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, default=None)
    args = parser.parse_args()
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    from playwright.sync_api import sync_playwright

    api = FakeApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{server.server_port}/"
    report: list[str] = []
    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(headless=True, executable_path=args.browser_executable)
            for label, scheme, choice, painted in CASES:
                # The system schemes at both sizes; the forced choices at the phone, where the owner reads.
                sizes = SIZES if choice is None else SIZES[:1]
                for size, width, height, phone in sizes:
                    where = f"{label} {size}"
                    context, page = open_page(browser, url, scheme, choice, width, height, phone)
                    judge(f"{where} at rest", painted, page.evaluate(READ_JS, list(BOXES)), report)
                    box = page.locator("#channel-compose-text")
                    box.click()
                    box.fill("A draft the owner reads while writing it.")
                    page.wait_for_function(
                        "() => document.activeElement === document.getElementById('channel-compose-text')"
                        " && document.getElementById('channel-compose-text').value.length > 0",
                        timeout=STATE_MS)
                    judge(f"{where} focused", painted, page.evaluate(READ_JS, list(BOXES)), report)
                    if phone:
                        shrunk = height - KEYBOARD_PX
                        page.set_viewport_size({"width": width, "height": shrunk})
                        page.wait_for_function(
                            f"() => (window.visualViewport ? window.visualViewport.height : window.innerHeight) === {shrunk}",
                            timeout=STATE_MS)
                        page.wait_for_function(
                            "() => document.documentElement.getAttribute('data-keyboard') === 'open'",
                            timeout=STATE_MS)
                        judge(f"{where} keyboard open", painted, page.evaluate(READ_JS, list(BOXES)), report)
                    if args.screenshots:
                        args.screenshots.mkdir(parents=True, exist_ok=True)
                        page.screenshot(path=str(args.screenshots / f"{label}-{size}.png"))
                    context.close()
            browser.close()
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
    for line in report:
        print(line)
    print("dark composer black: every box the reader writes in is the cards' black in dark and its own surface in light")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
