#!/usr/bin/env python3
"""A message's ellipsis menu above the floating chips, in real Chromium on a desk and on a phone.

`#1228 menu-above-chips`. The owner, 2026-11-10 (reply 324): on desktop the message drop down is
still below the floating chips, which obscure the item being clicked ("Open in Slack" and its
neighbours). The fake-DOM suite pins the overlay scale's numbers; only a browser engine can say
whether an item is the topmost element at its own centre where the menu overlaps the chips row,
because the list is one stacking context (contain and mask) and a read row is another (opacity and
filter), so an in-list z-index cannot rise above a sibling of the list.

It serves the checked-out page and the fake API of tests/two_column_browser.py (a channel taller
than the window), naming the provider as the open-upstream walk does and giving the long question
(209) its own address there, so the menu holds the owner's item. At 1280x800 and at 393x852:

  the channel opened in All, scrolled so the floating chips stand over the foot of the list -> the
  question's row brought to the foot, where its menu hangs over the chips row -> its ellipsis
  opened: the menu overlaps the chips, stands in the top layer, and every visible item — the
  "Open in ..." item among them — is the topmost element at its own centre
  (`document.elementFromPoint` returns the item or a descendant).

No page error at any step. Pass --screenshots DIR to keep a PNG of the open menu at each size.
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

# (label, width, height, phone). The phone is the owner's: 393x852.
SIZES = (("desk-1280", 1280, 800, False), ("phone-393", 393, 852, True))
QUESTION_ID = "209"
ADDRESS = "https://acme.slack.com/archives/C0123ABCD/p1700000000000100"

# The open menu where it overlaps the chips: whether a chip button itself stands over an item's
# centre at all (the walk is vacuous without one: the chips row spans the list, but only its
# buttons cover anything), whether the menu stands in the top layer, and for every visible item
# whether the topmost element at its centre is the item or inside it.
TOPMOST_JS = """(id) => {
    const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
    const menu = row ? row.querySelector('.row-more-menu') : null;
    if (!menu || menu.hidden) return {open: false, overlap: false, covered: 0, topLayer: false, checks: []};
    const menuRect = menu.getBoundingClientRect();
    const chips = document.getElementById('scroll-tools');
    const chipsRect = chips.getBoundingClientRect();
    const overlap = menuRect.bottom > chipsRect.top && menuRect.top < chipsRect.bottom
        && menuRect.right > chipsRect.left && menuRect.left < chipsRect.right;
    const chipBoxes = [...chips.querySelectorAll('.chip')]
        .filter((one) => !one.hidden && one.getClientRects().length)
        .map((one) => one.getBoundingClientRect());
    let topLayer = false;
    try { topLayer = menu.matches(':popover-open'); } catch (error) { topLayer = false; }
    const checks = [...menu.querySelectorAll('button, a')]
        .filter((one) => !one.hidden && one.getClientRects().length)
        .map((item) => {
            const r = item.getBoundingClientRect();
            const x = r.left + r.width / 2;
            const y = r.top + r.height / 2;
            const top = document.elementFromPoint(x, y);
            const overChip = chipBoxes.some((box) => x >= box.left && x <= box.right && y >= box.top && y <= box.bottom);
            return {text: item.textContent, className: item.className, overChip,
                ok: top === item || (top !== null && item.contains(top)),
                top: top ? String(top.className || top.tagName) : null};
        });
    return {open: true, overlap, covered: checks.filter((one) => one.overChip).length, topLayer,
        menu: {top: menuRect.top, bottom: menuRect.bottom},
        chips: {top: chipsRect.top, bottom: chipsRect.bottom}, checks};
}"""


class LinkedApi(ColumnsApi):
    """The columns channel as the owner's service, its long question carrying its own address."""

    def __init__(self) -> None:
        super().__init__()
        self.provider_name = "Slack"
        for row in self.messages:
            if row["id"] == QUESTION_ID:
                row["upstream_url"] = ADDRESS


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per state into (default: none)")
    return parser.parse_args()


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int, phone: bool) -> str:
    api = LinkedApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    origin = f"http://127.0.0.1:{server.server_port}"
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-menu-") as profile:
            if phone:
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
            desk = Desk(page, args.screenshots, label, width, height)
            page.goto(f"{origin}/", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            page.click("#view-switch")
            desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length === 24",
                          "the channel did not open in All")
            desk.settled()
            # The chips over the foot of the list: scrolled away from the newest message, the way
            # back stands at the left of the chips row.
            page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 900; }")
            desk.wait_for("() => !document.getElementById('jump-newest').hidden",
                          "the floating chips did not stand over the list")
            desk.settled()
            # The owner's row low in the list, where its menu hangs down over the chips row and
            # still fits the window below it (at the very foot the menu flips above the row
            # instead, which the walk would then prove nothing about).
            page.evaluate(
                "(id) => document.querySelector(`#discord-log > li[data-id=\"${id}\"]`)"
                ".scrollIntoView({block: 'center'})", QUESTION_ID)
            desk.settled()
            button = f"#discord-log > li[data-id='{QUESTION_ID}'] .row-more-button"
            if phone:
                found = page.locator(button).bounding_box()
                check(found is not None, f"{label}: the ellipsis is not drawn")
                assert found is not None
                page.touchscreen.tap(found["x"] + found["width"] / 2, found["y"] + found["height"] / 2)
            else:
                page.click(button)
            desk.wait_for(
                f"() => !document.querySelector('#discord-log > li[data-id=\"{QUESTION_ID}\"] .row-more-menu').hidden",
                "the ellipsis menu did not open")
            desk.settled()
            found: Json = page.evaluate(TOPMOST_JS, QUESTION_ID)
            assert isinstance(found, dict)
            checks = found["checks"]
            assert isinstance(checks, list)
            desk.check(bool(found["open"]), f"the menu is not open: {found}")
            desk.check(bool(found["overlap"]),
                       f"the menu does not overlap the chips row, so nothing was proved: {found}")
            desk.check(bool(found["topLayer"]), f"the menu is not in the top layer: {found}")
            desk.check(any("Open in" in str(one["text"]) for one in checks),
                       f"the owner's item is not in the menu: {checks}")
            for one in checks:
                desk.check(bool(one["ok"]),
                           f"menu item {one['text']!r} is covered at its centre by {one['top']!r}")
            desk.shot("menu-over-chips")
            desk.notes.append(f"{len(checks)} menu items topmost over the chips, "
                              f"menu {found['menu']} over chips {found['chips']}")
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(desk.notes)
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
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, phone in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, phone))
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
