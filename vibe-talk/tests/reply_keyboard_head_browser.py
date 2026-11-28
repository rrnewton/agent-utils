#!/usr/bin/env python3
"""The phone's keyboard coming up over a Reply keeps the answered message's header on screen (`#2437`).

`#2437 vibe-talk-reply-keyboard-hides-message-top`. Found in #2428's owner-UI matrix, on main alike: on a
phone, Reply on a message whose replies are gathered under it, then the keyboard up. The list moved
to stand the whole reply box above the keys (`#429 keyboard-hides-bar`), and with the replies between
the message and the box that pushed the message's header out over the top (at -14 px).

This walk serves the real page and the long channel of tests/reply_open_instant_browser.py (every
fiftieth message answered three times). At 412x915 and 393x852 by touch, light and dark: Reply on a
message with replies and on one without, then the keyboard up (the viewport 336 px shorter, the box
focused). By geometry only, every wait a condition:

  the answered message's header (its time and controls) stands wholly inside the list; the reply's
  text field stands above the keys; and the box never stands over the message. Where the message, its
  gathered replies and the box cannot all stand above the keys, the replies give way (reported).

Pass --screenshots DIR to keep a PNG per case; --web-root DIR to run it against another copy of the
page (main before the change puts the header above the screen).
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, check
import browser_scratch
import browser_waits
import page_sources
import reply_open_instant_browser as long_channel
import search_typing_fast_browser as big

if TYPE_CHECKING:
    from playwright.sync_api import Page

WAIT_MS = int(browser_waits.HANG_BACKSTOP_MS)
KEYBOARD_PX = 336

MEASURE_JS = """(ids) => {
  const box = (n) => { if (!n || !n.getClientRects().length) return null; const r = n.getBoundingClientRect();
    return {top: r.top, bottom: r.bottom}; };
  const row = document.querySelector(`#discord-log > li[data-id='${ids.target}']`);
  const area = document.getElementById('scroll-area').getBoundingClientRect();
  const replies = ids.replies.map((id) => box(document.querySelector(`#discord-log > li[data-id='${id}']`))).filter(Boolean);
  return {meta: box(row && row.querySelector('.meta')), row: box(row), field: box(document.getElementById('reply-text')),
    reply: box(document.getElementById('column-reply')), replies, areaTop: area.top, areaBottom: area.bottom,
    viewport: window.visualViewport ? visualViewport.height : innerHeight,
    focused: document.activeElement && document.activeElement.id};
}"""


def frames(page: Page) -> None:
    page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(done, 0))))")


def judge(tag: str, found: dict[str, object]) -> str:
    meta, field, reply, row = found["meta"], found["field"], found["reply"], found["row"]
    check(isinstance(meta, dict) and isinstance(field, dict) and isinstance(reply, dict) and isinstance(row, dict),
          f"{tag}: the message or the reply is not drawn: {found}")
    assert isinstance(meta, dict) and isinstance(field, dict) and isinstance(reply, dict) and isinstance(row, dict)
    area_top, viewport = float(str(found["areaTop"])), float(str(found["viewport"]))
    check(float(meta["top"]) >= area_top - 0.5 and float(meta["bottom"]) <= viewport + 0.5,
          f"{tag}: the answered message's header is off the screen with the keyboard up: {meta} (list top {area_top})")
    check(float(field["top"]) >= area_top - 0.5 and float(field["top"]) + 24 <= viewport + 0.5,
          f"{tag}: the reply's text field is not above the keys: {field} (visible to {viewport})")
    def under_box(one: object) -> bool:
        return (isinstance(one, dict) and float(one["bottom"]) > float(reply["top"]) + 0.5
                and float(one["top"]) < float(reply["bottom"]) - 0.5)

    check(not under_box(row), f"{tag}: the box stands over the message it answers: {row} under {reply}")
    # Where the message, its gathered replies and the box cannot all stand above the keys, the replies
    # give way (they are a scroll away); the message and the field never do. Said, not judged.
    replies = found["replies"]
    hidden = sum(1 for one in replies if under_box(one)) if isinstance(replies, list) else 0
    return (f"{tag}: header at {float(meta['top']):.1f}px, field at {float(field['top']):.1f}px of {viewport:.0f}; "
            f"replies under the box: {hidden}")


def walk(pw: object, args: argparse.Namespace, work: Path, label: str, width: int, height: int, scheme: str,
         answered: bool) -> str:
    api = long_channel.Channel()
    server, _thread, origin = big.serve(api, fixture.WEB_ROOT)
    tag = f"{label}-{scheme}-{'with-replies' if answered else 'no-replies'}"
    try:
        profile = work / f"profile-{tag}"
        profile.mkdir()
        context = pw.chromium.launch_persistent_context(  # type: ignore[attr-defined]
            str(profile), headless=True, executable_path=args.browser_executable,
            viewport={"width": width, "height": height}, device_scale_factor=2.625, is_mobile=True, has_touch=True,
            color_scheme=scheme)
        try:
            browser_waits.hot_host_timeouts(context)
            page = context.pages[0] if context.pages else context.new_page()
            page.goto(f"{origin}/", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_function("() => !document.getElementById('screen-welcome').hidden"
                                   " || !document.getElementById('screen-main').hidden", timeout=WAIT_MS)
            if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
                page.evaluate("() => document.getElementById('welcome-start').click()")
            page.wait_for_selector("#search-toggle", state="visible", timeout=WAIT_MS)
            page.wait_for_function(f"() => document.querySelectorAll('#discord-log > li[data-id]').length >= {long_channel.EXTRA_ROWS}",
                                   timeout=WAIT_MS)
            target = api.answered if answered else api.plain
            replies = api.answered_replies if answered else []
            page.evaluate(f"() => document.querySelector(\"#discord-log > li[data-id='{target}']\").scrollIntoView({{block: 'center'}})")
            frames(page)
            page.evaluate(f"() => document.querySelector(\"#discord-log > li[data-id='{target}'] .reply-button\").click()")
            page.wait_for_function("() => document.activeElement && document.activeElement.id === 'reply-text'", timeout=WAIT_MS)
            for _ in range(3):
                frames(page)
            # The keyboard comes up: the visual viewport is 336 px shorter.
            page.set_viewport_size({"width": width, "height": height - KEYBOARD_PX})
            page.wait_for_function(f"() => Math.round(window.visualViewport ? visualViewport.height : innerHeight) === {height - KEYBOARD_PX}",
                                   timeout=WAIT_MS)
            for _ in range(3):
                frames(page)
            found = page.evaluate(MEASURE_JS, {"target": target, "replies": replies})
            assert isinstance(found, dict)
            if args.screenshots is not None:
                page.screenshot(path=str(args.screenshots / f"{tag}.png"))
        finally:
            context.close()
    finally:
        server.shutdown()
    return judge(tag, found)


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT, help="directory holding the page to serve")
    parser.add_argument("--screenshots", type=Path, help="directory to write the PNGs into")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    from playwright.sync_api import sync_playwright
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    report: list[str] = []
    # `#2382 summary-link-test-host-tmp`: never the host temp dir; see browser_scratch.
    with browser_scratch.run_scratch("reply-keyboard-head-") as work:
        fixture.WEB_ROOT = page_sources.served(args.web_root)
        with sync_playwright() as pw:
            for label, width, height in (("phone-412", 412, 915), ("phone-393", 393, 852)):
                for scheme in ("light", "dark"):
                    for answered in (True, False):
                        report.append(walk(pw, args, work, label, width, height, scheme, answered))
    for line in report:
        print(line)
    print("reply keyboard head browser walk: ok")
    print(json.dumps({"cases": len(report)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
