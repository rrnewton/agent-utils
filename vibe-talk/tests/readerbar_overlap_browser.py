#!/usr/bin/env python3
"""Sound and the cloud/device voice selector, side by side in the phone reader bar.

`#1710 vibe-talk-readerbar-sound-overlap`. The owner, 2026-11-16 on Android: "the sound on off
button is overlapping the selector between cloud and device voice" — in the reader bar's left
column only the top of the cloud icon showed beneath SOUND. `#1355` had placed `#audio-source`
across both rows of that column, the cell `#speaker` already holds while reading, and Sound,
being positioned, painted over its top half.

The fix stacks them: Sound keeps row 1, the selector takes row 2, the row Clear vacates while
reading. This walk holds the result to it at the owner's size — 412x915 CSS px at device scale
2.6, touch — and again under a 130% root font: the two boxes never intersect, each centre hit
(`elementFromPoint`) lands on its own control, and a real tap flips each control's own state.
On a desk the reading pane still hides Sound (the desktop regime's own rule), so the pair is
checked there as selector-visible and nothing overlapping Read or Pace.

All waits are on page conditions; there are no timed sleeps. Pass --screenshots DIR to keep the
phone PNGs (before/after evidence), --web-root DIR to walk another copy of the page.
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import keyboard_browser as kb
from keyboard_browser import TOKEN, check
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import Page

MEASURE_JS = """() => {
  const pick = (id) => {
    const el = document.getElementById(id);
    const r = el.getBoundingClientRect();
    const hit = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);
    const owner = hit ? (hit.closest('button')?.id ?? hit.id) : null;
    return {id, x: r.x, y: r.y, width: r.width, height: r.height, right: r.right,
            bottom: r.bottom, hit: owner, display: getComputedStyle(el).display};
  };
  return {speaker: pick('speaker'), audio: pick('audio-source'),
          speed: pick('read-speed'), read: pick('read-aloud')};
}"""

VISIBLE_JS = """() => {
  const shown = (id) => {
    const el = document.getElementById(id);
    const r = el.getBoundingClientRect();
    return getComputedStyle(el).display !== 'none' && r.width > 0 && r.height > 0;
  };
  return shown('speaker') && shown('audio-source') && shown('read-aloud');
}"""

AUDIO_JS = """() => {
  const el = document.getElementById('audio-source');
  const r = el.getBoundingClientRect();
  return getComputedStyle(el).display !== 'none' && r.width > 0 && r.height > 0
    && !document.getElementById('read-aloud').hidden;
}"""


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=page_sources.served_root(),
                        help="the page assets to serve (default: this checkout's vibe-talk/web)")
    parser.add_argument("--screenshots", type=Path, default=None,
                        help="a directory to keep one PNG per state in (default: none kept)")
    return parser.parse_args()


def open_channel(page: Page, url: str, *, stacked: bool = True) -> None:
    page.goto(url, wait_until="load")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    page.click("#view-switch")
    # The switcher's cards float over the dock until one is picked; picking Channel both
    # lands on the channel view and puts the cards away, as a reader's tap does.
    page.wait_for_selector("#view-cards", state="visible", timeout=10_000)
    page.click("#view-cards >> text=Channel")
    # On a desk the reading pane hides Sound itself, so there only the selector is awaited.
    page.wait_for_function(VISIBLE_JS if stacked else AUDIO_JS, timeout=10_000)


def assert_stacked(page: Page, where: str) -> None:
    boxes: dict[str, dict[str, float | str]] = page.evaluate(MEASURE_JS)
    speaker, audio = boxes["speaker"], boxes["audio"]

    def num(box: dict[str, float | str], key: str) -> float:
        return float(box[key])

    for box in (speaker, audio):
        check(box["display"] != "none" and num(box, "width") > 0 and num(box, "height") >= 36,
              f"{where}: #{box['id']} is not a visible, tappable box: {box}")
        check(box["hit"] == box["id"],
              f"{where}: the centre of #{box['id']} is covered by #{box['hit']}: {box}")
    overlap_x = min(num(speaker, "right"), num(audio, "right")) - max(num(speaker, "x"), num(audio, "x"))
    overlap_y = min(num(speaker, "bottom"), num(audio, "bottom")) - max(num(speaker, "y"), num(audio, "y"))
    check(not (overlap_x > 1 and overlap_y > 1),
          f"{where}: #speaker and #audio-source intersect by {overlap_x}x{overlap_y}px: "
          f"{speaker} vs {audio}")
    check(num(audio, "y") >= num(speaker, "bottom") - 1,
          f"{where}: #audio-source is not stacked below #speaker: {speaker} vs {audio}")


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)

    # The owner's phone has a cloud voice configured, which is what makes the selector a
    # choice at all: the fake API's browser-only read_aloud leaves it hidden by design.
    class ReaderApi(kb.FakeApi):
        def client_config(self) -> kb.Json:
            config = super().client_config()
            config["read_aloud"] = {"backend": "test-agent", "label": "Agent voice",
                                    "playback": "audio", "local_only": False}
            return config

    api = ReaderApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), kb.handler_for(api, page_sources.served(args.web_root)))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/"

    try:
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch(headless=True, executable_path=args.browser_executable)

            def phone(root_percent: int, shot: str) -> None:
                context = browser.new_context(
                    viewport={"width": 412, "height": 915}, device_scale_factor=2.6,
                    is_mobile=True, has_touch=True)
                page = context.new_page()
                errors: list[str] = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                if root_percent != 100:
                    page.add_init_script(
                        "document.addEventListener('DOMContentLoaded', () => {"
                        " const larger = document.createElement('style');"
                        f" larger.textContent = 'html {{ font-size: {root_percent}% !important; }}';"
                        " document.head.append(larger); });")
                open_channel(page, url)
                assert_stacked(page, f"phone 412x915 at {root_percent}% font")
                if args.screenshots is not None:
                    page.screenshot(path=str(args.screenshots / shot))
                # A real tap on each control flips its own state: the selector is a switch,
                # Sound a pressed toggle, and neither tap may land on the other control.
                checked = page.get_attribute("#audio-source", "aria-checked")
                page.tap("#audio-source")
                page.wait_for_function(
                    "(was) => document.getElementById('audio-source')"
                    ".getAttribute('aria-checked') !== was",
                    arg=checked, timeout=10_000)
                page.tap("#speaker")
                page.wait_for_function(
                    "() => document.getElementById('speaker').getAttribute('aria-pressed') === 'true'",
                    timeout=10_000)
                check(not errors, f"the page threw: {errors}")
                context.close()

            phone(100, "phone-412x915.png")
            phone(130, "phone-412x915-font130.png")

            desk = browser.new_context(viewport={"width": 1280, "height": 800})
            page = desk.new_page()
            errors = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            open_channel(page, url, stacked=False)
            boxes = page.evaluate(MEASURE_JS)
            check(boxes["speaker"]["display"] == "none",
                  f"desktop unchanged: Sound should stay hidden while reading: {boxes['speaker']}")
            check(boxes["audio"]["hit"] == "audio-source",
                  f"desktop unchanged: the selector's centre is covered: {boxes['audio']}")
            for other in ("speed", "read"):
                a, b = boxes["audio"], boxes[other]
                overlap_x = min(a["right"], b["right"]) - max(a["x"], b["x"])
                overlap_y = min(a["bottom"], b["bottom"]) - max(a["y"], b["y"])
                check(not (overlap_x > 1 and overlap_y > 1),
                      f"desktop unchanged: #audio-source intersects #{other}: {a} vs {b}")
            check(not errors, f"the page threw: {errors}")
            desk.close()
            version = browser.version
            browser.close()

        print(f"{version} at 412x915 (device scale 2.6, touch) and 130% font: Sound and the "
              "cloud/device selector are stacked, non-overlapping and each tappable; "
              "at 1280x800 the desktop reading pane is unchanged")
        return 0
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
