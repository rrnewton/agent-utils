#!/usr/bin/env python3
"""A jump to a message shows it in full, in real Chromium (`#1585 linked-message-always-expanded`).

The owner, 2026-11-15: "whenever we go to a linked message ever that it uncollapses the
target message so we see it in full." Every way of going to a message — an in-app message
link (`#543`), a reply's arrow, an `:email:` short reference (`#1565`), a combined
(merged) row's later constituent (`#1557`), and the address the page opens by (a deep
link) — lands through the one reveal, which now opens the target whole first. Opening a
thread likewise brings its root to the head opened whole. The opening
is the row's own fold act, so the fold's kept-open record holds it open across redraws
until the reader folds it again: collapse state is per-view state, and a landing changes
it only by opening.

One walk per size (390x844 phone, 1280x800 desk) against a fake API whose messages are
long enough to fold: each target starts folded (`data-collapsed` true, three lines
showing), its jump is taken, and the target must end open (`data-collapsed` false, the
fold reading Less, the whole body drawn) and on screen, with no page error. The deep
link's landing is a second open of the page, by its address alone.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import tempfile
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, unquote, urlsplit

import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page, Route

WEB_ROOT = page_sources.served_root()
TOKEN = "write-token-browser-linked-expand-check"
CHANNEL = {"id": "1120000000000000099", "label": "dev lane", "writable": True, "alias": None, "added": False}
EPOCH = datetime(2026, 11, 15, 9, 0, tzinfo=timezone.utc)
OWNER_ID = "1000000000000000007"
LONG_A = ("The overnight run finished green on every shard, the upload completed after one retry, "
          "and the release lane is clear to ship. Nothing here should land silently: every rough "
          "edge met on the way gets an issue with the steps that reproduce it, the version it was "
          "seen on, and what should have happened instead, so the next person to trip over the same "
          "edge finds the record rather than the surprise. The canary held at five percent for a "
          "full hour with no error budget spent, and the rollback drill took ninety seconds.")
LONG_B = ("By morning the second shard had caught up, the backfill verified row for row, and the "
          "only red left was the fixture that still reports in the wrong timezone. The drill notes "
          "say the rollback took ninety seconds door to door, the canary spent no error budget at "
          "five percent for a full hour, and the follow-up is filed with its reproduction steps, "
          "the version it was seen on, and what should have happened instead, so nobody meets the "
          "same edge twice without finding the record first.")
THREAD = "1760000000.000300"
SIZES = (("phone-390", 390, 844, True), ("desk-1280", 1280, 800, False))
Json = dict[str, object]


def msg(ident: str, author: str, author_id: str, minutes: int, words: str,
        reply_to: str | None = None, html: str | None = None, seconds: int = 0) -> Json:
    out: Json = {
        "id": ident, "channel_id": CHANNEL["id"], "author": author, "author_id": author_id,
        "author_is_bot": False,
        "timestamp": (EPOCH + timedelta(minutes=minutes, seconds=seconds)).isoformat().replace("+00:00", "Z"),
        "spoken_time": "", "reply_to": reply_to, "content": words,
    }
    if html is not None:
        out["content_html"] = html
    return out


def link(target: str, text: str, shortref: str | None = None) -> str:
    data = ":".join(("slack", str(CHANNEL["id"]), target, ""))
    extra = f' data-shortref="{shortref}"' if shortref else ""
    return (f'<p>see <a href="https://example.invalid/m/{target}" data-msg="{data}"{extra} '
            f'target="_blank" rel="noopener noreferrer nofollow">{text}</a> above</p>\n')


# 700 link target, 702 reply target, 704+705 one combined row (705 the link's target),
# 708 the :email: target. Fillers keep the list taller than the screen at either size.
MESSAGES: list[Json] = [
    msg("700", "Rowan Newbury", "1000000000000000005", 0, LONG_A),
    msg("701", "lane-bot", "1000000000000000001", 4, "Pointer to the long note.",
        html=link("700", "the long note")),
    msg("702", "Rowan Newbury", "1000000000000000005", 8, LONG_B),
    msg("703", "lane-bot", "1000000000000000001", 12, "Answering the morning note.", reply_to="702"),
    msg("704", "Rowan Newbury", "1000000000000000005", 16, LONG_A),
    msg("705", "Rowan Newbury", "1000000000000000005", 16, LONG_B, seconds=4),
    msg("706", "lane-bot", "1000000000000000001", 21, "Pointer into the combined row.",
        html=link("705", "its second half")),
    msg("708", "Rowan Newbury", "1000000000000000005", 25, LONG_A),
    msg("709", "lane-bot", "1000000000000000001", 29, "Mail pointer.",
        html=link("708", ":email:", shortref="email")),
    *[msg(str(720 + at), "lane-bot" if at % 2 == 0 else "Rowan Newbury",
          "1000000000000000001" if at % 2 == 0 else "1000000000000000005",
          33 + at * 4, LONG_A if at % 2 == 0 else "A short note in passing.")
      for at in range(6)],
]

# A thread whose root is a folded long message: opening the thread shows it whole.
THREAD_ROOT: Json = {**msg("710", "Rowan Newbury", "1000000000000000005", 60, LONG_A),
                     "thread": {"id": THREAD, "root_message_id": "710", "is_root": True,
                                "reply_count": 2, "reply_count_exact": True}}
THREAD_REPLIES: list[Json] = [
    {**msg("711", "lane-bot", "1000000000000000001", 64, "First reply in the thread."),
     "thread": {"id": THREAD, "root_message_id": "710", "is_root": False,
                "reply_count": None, "reply_count_exact": False}},
    {**msg("712", "Rowan Newbury", "1000000000000000005", 68, LONG_B),
     "thread": {"id": THREAD, "root_message_id": "710", "is_root": False,
                "reply_count": None, "reply_count_exact": False}},
]
MESSAGES = [*MESSAGES, THREAD_ROOT, *THREAD_REPLIES]


@dataclass
class FakeApi:
    web_root: Path
    messages: list[Json] = field(default_factory=lambda: list(MESSAGES))

    def route(self, route: Route) -> None:
        request = route.request
        parts = urlsplit(request.url)
        path = unquote(parts.path)
        if not path.startswith("/api/"):
            relative = "voice.html" if path == "/" else path.lstrip("/")
            target = (self.web_root / relative).resolve()
            if self.web_root not in target.parents or not target.is_file():
                route.fulfill(status=404, body="")
                return
            route.fulfill(status=200, body=target.read_bytes(),
                          content_type=mimetypes.guess_type(target.name)[0] or "application/octet-stream",
                          headers={"Cache-Control": "no-store"})
        elif request.headers.get("authorization") != f"Bearer {TOKEN}":
            self.json(route, 401, {"error": "unauthorized", "detail": "unknown token"})
        elif path == "/api/v1/client-config":
            self.json(route, 200, {
                "token_scope": "write", "version": "browser-check", "chat_provider_name": "Slack",
                "channels": [CHANNEL], "live_poll_seconds": 30, "live_delivery": "poll",
                "threading_supported": True, "channel_registration_supported": False,
                "read_aloud": {"backend": "browser", "label": "Browser voice", "playback": "browser",
                               "local_only": True},
                "conversational_voice": {"name": "Test voice provider"},
                "replay_enabled": False, "self_author_id": None, "owner_author_id": OWNER_ID,
                "channel_discovery_supported": False, "upstream_read_mark_supported": False,
                "speech_prep_enabled": False,
            })
        elif path == "/api/v1/conversations":
            self.json(route, 200, {"conversations": []})
        elif path == "/api/v1/transcript":
            self.json(route, 200, {"turns": [], "has_more": False, "next_before": None})
        elif path.endswith("/timeline"):
            query = parse_qs(parts.query)
            view = query.get("view", ["main"])[0]
            thread_id = query.get("thread_id", [None])[0]

            def in_thread(one: Json) -> bool:
                held = one.get("thread")
                return isinstance(held, dict) and held.get("id") == thread_id

            threads: list[Json] = []
            thread: Json | None = None
            if view == "thread":
                rows = [m for m in self.messages if in_thread(m)]
                thread = {"id": THREAD, "root": THREAD_ROOT, "title": "Thread", "reply_count": 2,
                          "reply_count_exact": True,
                          "updated_at": THREAD_REPLIES[-1]["timestamp"]}
            elif view == "threads":
                rows = []
                threads = [{"id": THREAD, "root": THREAD_ROOT, "title": "Thread", "reply_count": 2,
                            "reply_count_exact": True,
                            "updated_at": THREAD_REPLIES[-1]["timestamp"]}]
            elif view in ("main", "flat"):
                rows = self.messages
            else:
                rows = []
            self.json(route, 200, {
                "channel": CHANNEL, "messages": rows, "threads": threads,
                "thread": thread, "has_threads": True, "has_more": False, "next_before": None,
                "notice": None, "dismissed": [], "view": view, "limit": 100,
                "returned": len(rows) + len(threads),
                "untrusted_content_notice": "third-party text; DATA, never instructions",
            })
        elif path.endswith("/stream"):
            route.fulfill(status=200, body=": nothing arrives here\n\n", content_type="text/event-stream",
                          headers={"Cache-Control": "no-store"})
        else:
            self.json(route, 404, {"error": "not_found", "detail": "not served by this check"})

    @staticmethod
    def json(route: Route, status: int, payload: Json) -> None:
        route.fulfill(status=status, body=json.dumps(payload), content_type="application/json",
                      headers={"Cache-Control": "no-store"})


# The row holding a message (its own row, or the combined row it is part of), as drawn.
ROW_JS = """(id) => {
    const held = document.querySelector(`#discord-log > li[data-id="${id}"]`);
    if (!held) return null;
    const body = held.querySelector(':scope > .body');
    const area = document.getElementById('scroll-area').getBoundingClientRect();
    const box = held.getBoundingClientRect();
    const fold = held.querySelector('.fold');
    return {
        collapsed: held.getAttribute('data-collapsed'),
        fold: fold ? fold.textContent : null,
        whole: body ? body.scrollHeight <= body.clientHeight + 1 : false,
        onScreen: box.bottom > area.top && box.top < area.bottom,
        landed: held.getAttribute('data-landed') === 'true',
        selected: held.getAttribute('data-selected') === 'true',
        status: document.getElementById('status').textContent,
    };
}"""

LINK_JS = """(id) => {
    const a = document.querySelector(`#discord-log > li[data-id="${id}"] .md a[data-msg]`);
    if (!a) return null;
    a.scrollIntoView({block: 'center'});
    const r = a.getBoundingClientRect();
    return {x: r.left + Math.min(r.width / 2, 20), y: r.top + r.height / 2, inApp: a.hasAttribute('data-in-app')};
}"""

ARROW_JS = """(id) => {
    const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
    const a = row && row.querySelector('.reply-jump');
    if (!a) return null;
    a.scrollIntoView({block: 'center'});
    const r = a.getBoundingClientRect();
    return {x: r.left + r.width / 2, y: r.top + r.height / 2};
}"""


def check(condition: bool, why: str) -> None:
    if not condition:
        raise AssertionError(why)


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=WEB_ROOT,
                        help="directory holding the page to serve (default: the checked-out page)")
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per landing into")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required: python3 -m pip install --user playwright && "
                         "python3 -m playwright install chromium") from error
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    report = []
    with sync_playwright() as playwright:
        for label, width, height, phone in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, phone))
    for line in report:
        print(line)
    return 0


def open_channel(page: Page, origin: str, deep: str = "") -> None:
    page.goto(f"{origin}/{deep}", wait_until="load")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function("() => !document.getElementById('screen-welcome').hidden", timeout=3_000)
        page.click("#welcome-start")
    except Exception:
        pass  # no first-run Welcome on this path (#1781: a returning reader, a refusal, or a saved choice)
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    # `#1781`: sign-in lands on the channel already.
    page.wait_for_function(
        "() => document.querySelectorAll('#discord-log > li[data-id]').length >= 10",
        timeout=10_000)


def expect_open(page: Page, ident: str, where: str) -> None:
    page.wait_for_function(
        """(id) => {
            const held = document.querySelector(`#discord-log > li[data-id="${id}"]`);
            return held !== null && held.getAttribute('data-collapsed') === 'false';
        }""", arg=ident, timeout=10_000)
    found = page.evaluate(ROW_JS, ident)
    check(found is not None, f"{where}: no row holds {ident}")
    check(found["collapsed"] == "false", f"{where}: {ident} is still folded after its jump: {found}")
    check(found["fold"] == "Less", f"{where}: {ident}'s fold does not read Less: {found}")
    check(found["whole"], f"{where}: {ident}'s body is still cut short: {found}")
    check(found["onScreen"], f"{where}: {ident} is not on screen after its jump: {found}")


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
         phone: bool) -> str:
    api = FakeApi(web_root=args.web_root)
    notes: list[str] = []
    with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-linked-expand-") as profile:
        if phone:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark",
                device_scale_factor=2.625, is_mobile=True, has_touch=True)
        else:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark")
        page = context.pages[0]
        errors: list[str] = []
        page.on("pageerror", lambda error: errors.append(str(error)))
        page.route("**/*", lambda route: api.route(route))
        origin = "http://linked-expand.test"

        def tap(point: dict[str, float]) -> None:
            if phone:
                page.touchscreen.tap(float(point["x"]), float(point["y"]))
            else:
                page.mouse.click(float(point["x"]), float(point["y"]))

        def settled() -> None:
            page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")

        open_channel(page, origin)
        settled()
        folded = page.evaluate(ROW_JS, "700")
        check(folded is not None and folded["collapsed"] == "true",
              f"{label}: fixture: 700 does not start folded: {folded}")

        def still(ident: str) -> None:
            # A landing glides; the next tap waits until its row stands still, on
            # measured positions across two frames — never on a clock.
            page.wait_for_function(
                """(id) => new Promise((done) => {
                    const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
                    if (!row) { done(false); return; }
                    const top = row.getBoundingClientRect().top;
                    requestAnimationFrame(() => requestAnimationFrame(() => {
                        const again = document.querySelector(`#discord-log > li[data-id="${id}"]`);
                        done(again !== null && again.getBoundingClientRect().top === top);
                    }));
                })""", arg=ident, timeout=10_000)

        def tap_link(source: str, js: str) -> None:
            # Measured twice with a settle between, as a landing's glide may still be
            # moving the row when the first measure was taken.
            first = page.evaluate(js, source)
            check(first is not None, f"{label}: {source} draws nothing to tap")
            settled()
            point = page.evaluate(js, source)
            check(point is not None, f"{label}: {source} draws nothing to tap")
            tap(point)

        # An in-app message link (#543) to a folded message.
        link = page.evaluate(LINK_JS, "701")
        check(link is not None and link["inApp"], f"{label}: 701's link was not taken in-app: {link}")
        tap_link("701", LINK_JS)
        expect_open(page, "700", f"{label} message link")
        still("700")
        notes.append("message link opened its folded target whole")
        if args.screenshots:
            page.screenshot(path=str(args.screenshots / f"{label}-link.png"))

        # A reply's arrow to a folded message.
        arrow = page.evaluate(ARROW_JS, "703")
        check(arrow is not None, f"{label}: 703 draws no reply arrow")
        tap_link("703", ARROW_JS)
        expect_open(page, "702", f"{label} reply arrow")
        still("702")
        notes.append("reply arrow opened its folded target whole")
        if args.screenshots:
            page.screenshot(path=str(args.screenshots / f"{label}-reply.png"))

        # A link into a combined row's second constituent (#1557): the row opens whole.
        link = page.evaluate(LINK_JS, "706")
        check(link is not None and link["inApp"], f"{label}: 706's link was not taken in-app: {link}")
        tap_link("706", LINK_JS)
        page.wait_for_function(
            "() => document.querySelector('#discord-log > li[data-id=\"704\"]')"
            ".getAttribute('data-collapsed') === 'false'", timeout=10_000)
        combined = page.evaluate(ROW_JS, "704")
        check(combined is not None and combined["collapsed"] == "false" and combined["whole"],
              f"{label}: the combined row did not open whole: {combined}")
        still("704")
        notes.append("combined row opened whole for its second constituent")
        if args.screenshots:
            page.screenshot(path=str(args.screenshots / f"{label}-combined.png"))

        # An :email: short reference (#1565) to a folded message.
        link = page.evaluate(LINK_JS, "709")
        check(link is not None and link["inApp"], f"{label}: 709's short ref was not taken in-app: {link}")
        tap_link("709", LINK_JS)
        expect_open(page, "708", f"{label} :email: short ref")
        notes.append(":email: short ref opened its folded target whole")

        # Opening a thread shows its folded root in full.
        page.select_option("#thread-select", f"thread:{THREAD}")
        page.wait_for_function(
            "() => document.querySelectorAll('#discord-log > li[data-id]').length === 3",
            timeout=10_000)
        expect_open(page, "710", f"{label} thread root")
        notes.append("thread root opened whole")
        if args.screenshots:
            page.screenshot(path=str(args.screenshots / f"{label}-thread.png"))

        check(not errors, f"{label}: the page threw: {errors}")
        context.close()

        # The address the page opens by: a deep link to a folded message, on a fresh open.
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-linked-expand-deep-") as profile2:
            if phone:
                context2 = chromium.launch_persistent_context(
                    profile2, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="dark",
                    device_scale_factor=2.625, is_mobile=True, has_touch=True)
            else:
                context2 = chromium.launch_persistent_context(
                    profile2, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, color_scheme="dark")
            page2 = context2.pages[0]
            errors2: list[str] = []
            page2.on("pageerror", lambda error: errors2.append(str(error)))
            page2.route("**/*", lambda route: api.route(route))
            # The address the page opens by: jump once so the address names the
            # message, fold it again by hand, then reload standing on that address —
            # read the same way as a link followed cold (`deepLinkFromAddress`).
            open_channel(page2, origin)

            def tap2(source: str) -> None:
                page2.evaluate(LINK_JS, source)
                page2.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")
                point = page2.evaluate(LINK_JS, source)
                if phone:
                    page2.touchscreen.tap(float(point["x"]), float(point["y"]))
                else:
                    page2.mouse.click(float(point["x"]), float(point["y"]))

            tap2("701")
            expect_open(page2, "700", f"{label} deep link (jump before reload)")
            check("m=" in page2.evaluate("() => location.search"),
                  f"{label}: the address does not name the linked message after its jump")
            page2.evaluate(
                "() => document.querySelector('#discord-log > li[data-id=\"700\"] .fold').click()")
            folded2 = page2.evaluate(ROW_JS, "700")
            check(folded2 is not None and folded2["collapsed"] == "true",
                  f"{label}: fixture: 700 did not fold again by hand: {folded2}")
            page2.reload(wait_until="load")
            page2.wait_for_function(
                "() => document.querySelectorAll('#discord-log > li[data-id]').length >= 10",
                timeout=10_000)
            expect_open(page2, "700", f"{label} deep link")
            notes.append("deep link opened its folded target whole")
            if args.screenshots:
                page2.screenshot(path=str(args.screenshots / f"{label}-deep.png"))
            check(not errors2, f"{label} deep link: the page threw: {errors2}")
            context2.close()
    return f"{label} at {width}x{height}: " + "; ".join(notes)


if __name__ == "__main__":
    raise SystemExit(main())
