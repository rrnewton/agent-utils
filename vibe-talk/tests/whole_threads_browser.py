#!/usr/bin/env python3
"""Whole threads on the search bar (`#1852 vibe-talk-search-whole-threads-toggle`).

Serves the checked-out page with a fake API (the labels fixture's, restocked):
two threads, one holding the only word/label hits, plus unthreaded notes.
At a 412x915 phone and a 1280x800 desk, in light and dark, with the on-screen
keyboard up (a real `visualViewport` shrink) and down, Discuss open on the
phone:

  the search-bar toggle (greyed thread emoji off, circled on, tooltip
  "Whole threads: if any msg matches", `aria-pressed`) widens a word search
  and a `label:` search to whole threads in All, in the thread list (opening
  the card shows the thread whole, filter still on), and in two columns;
  every search-bar icon carries a tooltip and an accessible label; the old
  results-area chip is gone. `--screenshots DIR` keeps one PNG per stage.

  `#1864 vibe-talk-whole-threads-no-duplicate-hits`: with the toggle on, the
  hit thread renders once — each visible message id exactly once, the hit's
  text once, the hit marked in place, and its non-matching thread-mates
  standing normally rather than greyed as unmatched. The real-server half
  (the server-hit merge drawing an on-screen hit again) is
  `tests/whole_threads_server_browser.py`.
"""

from __future__ import annotations

import argparse
import os
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import labels_browser as labels_fixture
import offline_cache_browser as fixture
from keyboard_hides_bar_browser import IOS_VIEWPORT_JS, KEYBOARD_PX
from labels_browser import LabelsApi, Walk, labels_handler
from offline_cache_browser import CHANNEL, TOKEN, check, message
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import Page

ALPHA = "spaces/A/threads/alpha"
BETA = "spaces/A/threads/beta"


def _thread(thread_id: str, root_id: str, is_root: bool, count: int) -> dict[str, object]:
    return {"id": thread_id, "root_message_id": root_id,
            "is_root": is_root, "reply_count": count, "reply_count_exact": True}


class WholeApi(LabelsApi):
    """Two threads: alpha holds every hit, beta none; two unthreaded notes."""

    def __init__(self) -> None:
        super().__init__()
        rows = [
            message(10, "Alpha thread root about deployment.", _thread(ALPHA, "210", True, 2)),
            message(11, "the quixotic plan is ready #kvm", _thread(ALPHA, "210", False, 2)),
            message(12, "ordinary follow up in alpha.", _thread(ALPHA, "210", False, 2)),
            message(13, "Beta thread root about storage.", _thread(BETA, "213", True, 1)),
            message(14, "another ordinary reply in beta.", _thread(BETA, "213", False, 1)),
            message(15, "quixotic standalone note."),
            message(16, "plain channel note."),
        ]
        rows[1]["labels"] = ["kvm"]
        self.messages = rows
        self.threads = [
            {"id": ALPHA, "root": rows[0], "title": "Alpha thread",
             "reply_count": 2, "reply_count_exact": True, "updated_at": rows[2]["timestamp"]},
            {"id": BETA, "root": rows[3], "title": "Beta thread",
             "reply_count": 1, "reply_count_exact": True, "updated_at": rows[4]["timestamp"]},
        ]
        self.reply_count = 0

    def post(self, path: str, body: dict[str, object]) -> tuple[int, dict[str, object]]:
        if path.endswith("/reply"):
            self.reply_count += 1
            posted = message(100 + self.reply_count, str(body.get("text", "")),
                             _thread(ALPHA, "210", False, 3))
            self.messages.append(posted)
            return 200, {"posted": posted, "parts": [posted]}
        return super().post(path, body)


ICONS = ("author-filter", "links-filter", "labels-button", "search-entire-threads", "search-toggle")


def whole_handler(api: WholeApi):  # type: ignore[no-untyped-def]
    """The labels fixture's handler, plus the reply write it does not serve."""
    import json as _json
    from urllib.parse import urlsplit as _split
    base = labels_handler(api)

    class Handler(base):  # type: ignore[valid-type, misc]
        def do_POST(self) -> None:  # noqa: N802
            if _split(self.path).path.endswith("/reply"):
                raw = self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"{}"
                try:
                    parsed = _json.loads(raw)
                except ValueError:
                    parsed = {}
                self.json(*api.post(_split(self.path).path, parsed if isinstance(parsed, dict) else {}))
                return
            super().do_POST()

    return Handler


def walk_through(walk: Walk) -> None:
    page: Page = walk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function("() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden", timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    walk.until("() => document.querySelectorAll('#discord-log > li[data-id]').length === 7",
               "the channel did not open with its seven messages")

    # Tooltips: every search-bar icon has a title and an accessible label.
    walk.press("#search-toggle")
    walk.until("() => !document.getElementById('search-entire-threads').hidden",
               "the Whole-threads toggle did not stand on the search bar")
    tips = page.evaluate("""() => Object.fromEntries(['author-filter','links-filter','labels-button','search-entire-threads','search-toggle']
      .map((id) => { const b = document.getElementById(id);
        return [id, {title: b.getAttribute('title'), label: b.getAttribute('aria-label')}]; }))""")
    for icon in ICONS:
        walk.check(bool(tips[icon]["title"]) and bool(tips[icon]["label"]),
                   f"{icon} lacks a tooltip or accessible label: {tips[icon]}")
    walk.check(tips["search-entire-threads"] == {"title": "Whole threads: if any msg matches",
                                                 "label": "Whole threads: if any msg matches"},
               f"Whole-threads tooltip says {tips['search-entire-threads']}")
    walk.check(tips["labels-button"] == {"title": "Filter by label",
                                          "label": "Labels. Filter by label"},
               f"Labels tooltip says {tips['labels-button']}")
    walk.check(page.evaluate("() => document.querySelectorAll('#search-bucket-row #search-entire-threads').length") == 0,
               "the old results-area chip still stands")
    walk.check(page.evaluate("() => document.querySelectorAll('#search-entire-threads').length") == 1,
               "there is not exactly one Whole-threads control")
    if not walk.touch:
        page.hover("#search-entire-threads")
        walk.settled()
    walk.shot("1-toggle-off")

    # Word search in All: hits alone, then whole threads.
    page.fill("#search-field", "quixotic")
    walk.until("() => document.getElementById('search-count').textContent.includes('of')",
               "the word search was not counted")
    walk.check(walk.shown() == ["211", "215"], f"word hits alone show {walk.shown()}")
    if walk.touch:
        # `#1853` on the combined bar: while the field holds the focus it
        # keeps a readable row of its own, every toggle below it.
        room = page.evaluate("""() => { const field = document.getElementById('search-field');
          const whole = document.getElementById('search-entire-threads');
          const f = field.getBoundingClientRect(), w = whole.getBoundingClientRect();
          return {width: field.clientWidth, scroll: field.scrollWidth, below: w.top >= f.bottom - 1,
                  focused: document.activeElement === field}; }""")
        walk.check(room["focused"] and room["width"] >= 240 and room["scroll"] <= room["width"] + 1 and room["below"],
                   f"the focused phone field has no readable room: {room}")
        walk.shot("1c-field-focused")
    walk.press("#search-entire-threads")
    walk.until("() => document.getElementById('search-entire-threads').getAttribute('aria-pressed') === 'true'",
               "the toggle did not press on")
    walk.check(walk.shown() == ["210", "211", "212", "215"],
               f"whole threads (word, All) show {walk.shown()}")
    # `#1864`: the thread renders once — every visible id exactly once, the
    # hit's text once, the hit marked in place, its thread-mates standing
    # normally (shown, unmarked) rather than greyed as unmatched.
    once = page.evaluate("""() => {
      const rows = [...document.querySelectorAll('#discord-log > li[data-id]')]
        .filter((li) => li.getClientRects().length);
      const counts = {};
      for (const li of rows) counts[li.dataset.id] = (counts[li.dataset.id] || 0) + 1;
      const hit = rows.find((li) => li.dataset.id === '211');
      const mate = rows.find((li) => li.dataset.id === '212');
      return {
        dupes: Object.entries(counts).filter(([, n]) => n > 1).map(([id]) => id),
        textOnce: rows.filter((li) => li.textContent.includes('the quixotic plan is ready')).length,
        mateShown: !!mate && !mate.classList.contains('search-hidden'),
        mateUnmarked: !!mate && mate.querySelectorAll('mark.search-hit').length === 0,
      }; }""")
    walk.check(once["dupes"] == [], f"whole threads (word, All) shows ids twice: {once['dupes']}")
    walk.check(once["textOnce"] == 1, f"the hit's text stands {once['textOnce']} times in the visible list")
    # The marks are drawn once the typing pauses (`searchField.marked()`).
    walk.until("""() => { const hit = document.querySelector('#discord-log > li[data-id="211"]');
      return !!hit && [...hit.querySelectorAll('mark.search-hit')].some((m) => m.textContent === 'quixotic'); }""",
               "the hit is not marked in place in its thread")
    walk.check(once["mateShown"] and once["mateUnmarked"],
               "a non-matching thread-mate is hidden, greyed as unmatched, or marked as a hit")
    if not walk.touch:
        page.hover("#search-entire-threads")
        walk.settled()
    walk.shot("2-whole-word-all")

    # Opening the hit thread (from Main) keeps the filter and shows it whole.
    page.evaluate("() => document.getElementById('channel-view-main').click()")
    walk.until("() => document.querySelector('#discord-log > li[data-id=\"210\"]') !== null",
               "Main did not show the hit thread's root")
    page.evaluate("() => document.querySelector('#discord-log > li[data-id=\"210\"] .thread-badge').click()")
    walk.until("() => [...document.querySelectorAll('#discord-log > li[data-id]')]"
               ".filter((li) => li.getClientRects().length).length >= 3",
               "opening the thread did not show it whole")
    walk.check(walk.shown() == ["210", "211", "212"], f"the opened thread shows {walk.shown()}")
    walk.check(page.evaluate("() => document.getElementById('search-field').value") == "quixotic"
               and page.evaluate("() => document.getElementById('search-entire-threads').getAttribute('aria-pressed')") == "true",
               "reading the thread reset the search or the toggle")
    page.evaluate("() => document.getElementById('channel-view-flat').click()")
    walk.until("() => document.querySelectorAll('#discord-log > li[data-id]').length === 7",
               "returning to All did not restore the list")
    walk.check(walk.shown() == ["210", "211", "212", "215"],
               f"back in All, whole threads show {walk.shown()}")

    # Label search, same widening.
    page.fill("#search-field", "label:kvm")
    walk.until("() => document.getElementById('search-count').textContent.includes('of')",
               "the label search was not counted")
    walk.check(walk.shown() == ["210", "211", "212"], f"whole threads (label, All) show {walk.shown()}")
    walk.shot("3-whole-label-all")
    walk.press("#search-entire-threads")
    walk.check(walk.shown() == ["211"], f"label hits alone show {walk.shown()}")
    walk.press("#search-entire-threads")

    # Thread list: only the hit thread's card stands; opening it shows it whole.
    page.fill("#search-field", "quixotic")
    walk.until("() => document.getElementById('search-count').textContent.includes('of')",
               "the word search was not recounted")
    page.evaluate("() => document.getElementById('channel-view-threads').click()")
    walk.until("() => document.querySelectorAll('#thread-list > li').length > 0",
               "the thread list did not stand")
    cards = page.evaluate("""() => [...document.getElementById('thread-list').children]
      .filter((li) => li.getClientRects().length).map((li) => li.getAttribute('data-context-id'))""")
    walk.check(cards == [ALPHA], f"thread-list cards stand as {cards}")
    walk.shot("4-whole-word-threads")
    page.evaluate("() => document.querySelector('#thread-list > li .thread-card-open').click()")
    walk.until("() => [...document.querySelectorAll('#discord-log > li[data-id]')]"
               ".filter((li) => li.getClientRects().length).length >= 3",
               "opening the card did not show the thread whole")
    walk.check(walk.shown() == ["210", "211", "212"], f"the card's thread shows {walk.shown()}")
    walk.check(page.evaluate("() => document.getElementById('search-entire-threads').getAttribute('aria-pressed')") == "true",
               "opening the card reset the toggle")

    # Replying in the thread leaves the filter and the toggle on.
    page.evaluate("() => document.querySelector('#discord-log > li[data-id=\"212\"] .reply-button').click()")
    walk.until("() => ['column-reply', 'side-reply'].some((id) => !document.getElementById(id).hidden)",
               "Reply did not open the reply editor")
    page.fill("#reply-text", "replying while the filter stands")
    page.click("#reply-send")
    walk.until("() => [...document.querySelectorAll('#discord-log > li[data-id]')]"
               ".some((li) => li.getClientRects().length && li.textContent.includes('replying while the filter stands'))",
               "the reply did not stand in the filtered thread")
    walk.check(page.evaluate("() => document.getElementById('search-field').value") == "quixotic"
               and page.evaluate("() => document.getElementById('search-entire-threads').getAttribute('aria-pressed')") == "true",
               "replying reset the search or the toggle")
    walk.shot("5-reply-filtered")

    # Two columns (desk): the side cards keep only the hit thread.
    if not walk.touch:
        page.evaluate("() => document.getElementById('channel-view-flat').click()")
        page.click("#column-toggle")
        walk.until("() => document.getElementById('side-column').getClientRects().length > 0",
                   "the second column did not stand")
        walk.until("() => document.querySelectorAll('#side-cards > li').length > 0",
                   "the side cards did not stand")
        side = page.evaluate("""() => [...document.getElementById('side-cards').children]
          .filter((li) => li.getClientRects().length).map((li) => li.getAttribute('data-id'))""")
        walk.check(side == [ALPHA], f"two-column side cards stand as {side}")
        page.evaluate("() => { const b = document.getElementById('dismiss-error'); if (b && b.getClientRects().length) b.click(); }")
        walk.settled()
        walk.shot("5-whole-two-column")
        page.click("#column-toggle")

    page.evaluate("() => document.getElementById('channel-view-flat').click()")
    if walk.touch:
        page.evaluate("() => document.getElementById('discuss-open').click()")
        walk.until("() => !document.getElementById('discuss-panel').hidden", "Discuss did not open")
        # The fixture serves no Discuss scope, so its 404 stands in the panel
        # (as in the labels walk); the page-level banner is dismissed so the
        # shot shows the search bar and the open half-pane, not the alert.
        page.evaluate("() => { const b = document.getElementById('dismiss-error'); if (b && b.getClientRects().length) b.click(); }")
        walk.settled()
        walk.shot("6-discuss")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"))
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    try:
        from playwright.sync_api import sync_playwright
    except ImportError:
        raise SystemExit("playwright is not installed: python3 -m playwright install chromium")
    sizes = (("phone", 412, 915, True), ("desk", 1280, 800, False))
    with sync_playwright() as playwright:
        for label, width, height, touch in sizes:
            for theme in ("light", "dark"):
                api = WholeApi()
                server = ThreadingHTTPServer(("127.0.0.1", 0), whole_handler(api))
                threading.Thread(target=server.serve_forever, daemon=True).start()
                origin = f"http://127.0.0.1:{server.server_port}"
                try:
                    import tempfile
                    with tempfile.TemporaryDirectory() as profile:
                        context = playwright.chromium.launch_persistent_context(
                            profile, headless=True, executable_path=args.browser_executable,
                            viewport={"width": width, "height": height},
                            device_scale_factor=2.6 if touch else 1,
                            has_touch=touch, is_mobile=touch, color_scheme=theme)
                        browser_waits.hot_host_timeouts(context)
                        page = context.pages[0] if context.pages else context.new_page()
                        page.add_init_script(IOS_VIEWPORT_JS)
                        page.goto(origin)
                        walk = Walk(page, f"{label}-{theme}", touch, args.screenshots)
                        if touch:
                            walk.page.evaluate(f"() => window.__iosKeyboard({KEYBOARD_PX})")
                        walk_through(walk)
                        if touch:
                            walk.shot("9-keyboard")
                            walk.page.evaluate("() => window.__iosClose()")
                        context.close()
                        print(f"{label}-{theme}: whole-threads walk passed")
                finally:
                    server.shutdown()
    print("whole threads browser: all walks passed")


if __name__ == "__main__":
    main()
