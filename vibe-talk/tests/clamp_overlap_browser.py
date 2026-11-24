#!/usr/bin/env python3
"""No two lines of a folded preview are ever drawn in the same place, in real Chromium.

`#2203 vibe-talk-folded-card-preview-clamp-overlap`. The owner, 2026-11-24: in a selected
folded (clamped-preview) bullet card, two lines of text are drawn on top of each other inside
the preview ("the loaded-host repro is understood and has" over "...has an issue filed against
it"), on the phone, pre-existing on builds either side of the chip-row work over the same cards.
The cause was the preview's own overflow: `-webkit-line-clamp` lays the lines past the clamp
out collapsed at the clamp line's place, and `overflow: hidden` made the clamped body a scroll
box, so selecting words in the preview scrolled those collapsed lines into the box, where they
painted on top of each other. The preview now clips with `overflow: clip`
(`07-long-messages.css`), which no selection reveal can scroll.

This walk serves the checked-out page and a small fake API through Playwright's router (no
port is opened): one channel holding a long bullet message in the seeded scene-92 shape, a
long plain-paragraph message, and a short control. At a 412x915 phone by touch and a 1280x800
desk it measures the client rects of the text lines standing in each folded preview — a line
the clip keeps wholly outside the box paints nothing, so only those can overlap — and requires:

  folded, and again with the bullet row selected by a text drag (the state the owner saw),
  no two line boxes overlap, exactly the reader's three lines stand inside the preview, and
  no shown line is cut in half at the preview's foot -> the same for the plain message ->
  opened, the bullet body is whole and its lines still do not overlap (the negative control:
  a walk that clamped everything would fail it) -> no page error at any step.

Pass --screenshots DIR to keep a PNG of each state; --web-root DIR to run against another
copy of the page; --report to print every measure.
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
from typing import TYPE_CHECKING, TypedDict
from urllib.parse import parse_qs, unquote, urlsplit

import page_sources
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import Page, Route

WEB_ROOT = page_sources.served_root()
TOKEN = "write-token-browser-clamp-overlap-check"
CHANNEL = {"id": "1120000000000000001", "label": "dev lane", "writable": True, "alias": None, "added": False}
EPOCH = datetime(2026, 10, 22, 9, 0, tzinfo=timezone.utc)
OWNER_ID = "1000000000000000007"

BULLET_TEXT = ("seeded: where the release stands this morning:\n\n"
               "- the build is green on both runners\n"
               "- the flaky ordering test is quarantined until the loaded-host repro is understood\n"
               "  and has an issue filed against it, with both logs\n"
               "- docs are updated\n"
               "- release notes drafted\n"
               "- the tag is not pushed yet\n  because it waits on your review\n"
               "- nothing else is open\n\n\n"
               "I will push the tag when you say so.\n\nThanks!")
BULLET_HTML = ("<p>seeded: where the release stands this morning:</p>"
               "<ul><li>the build is green on both runners</li>"
               "<li>the flaky ordering test is quarantined until the loaded-host repro is understood "
               "and has an issue filed against it, with both logs</li>"
               "<li>docs are updated</li><li>release notes drafted</li>"
               "<li>the tag is not pushed yet because it waits on your review</li>"
               "<li>nothing else is open</li></ul>"
               "<p>I will push the tag when you say so.</p><p>Thanks!</p>")
PLAIN = ("The overnight run finished green on every shard, the upload completed after one retry, "
         "and the release lane is clear to ship. Nothing here should land silently: every rough "
         "edge met on the way gets an issue with the steps that reproduce it, the version it was "
         "seen on, and what should have happened instead, so the next person to trip over the same "
         "edge finds the record rather than the surprise. The canary held at five percent for a "
         "full hour with no error budget spent, and the rollback drill took ninety seconds.")

ROWS: list[tuple[str, str, str | None]] = [
    ("700", BULLET_TEXT, BULLET_HTML),
    ("701", PLAIN, None),
    ("702", "A short note in passing.", None),
]


def message(index: int, row: tuple[str, str, str | None]) -> Json:
    ident, words, html = row
    out: Json = {
        "id": ident, "channel_id": CHANNEL["id"], "author": "lane-bot",
        "author_id": "1000000000000000001", "author_is_bot": True,
        "timestamp": (EPOCH + timedelta(minutes=index * 4)).isoformat().replace("+00:00", "Z"),
        "spoken_time": "", "reply_to": None, "content": words,
    }
    if html is not None:
        out["content_html"] = html
    return out


Json = dict[str, object]


@dataclass
class FakeApi:
    """The routes the page reads for one channel with a folded bullet and a folded plain row."""

    web_root: Path
    messages: list[Json] = field(default_factory=lambda: [message(at, row) for at, row in enumerate(ROWS)])
    posts: list[str] = field(default_factory=list)

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
            shown = [] if view == "threads" else self.messages
            self.json(route, 200, {
                "channel": CHANNEL, "messages": shown, "threads": [],
                "thread": None, "has_threads": False, "has_more": False, "next_before": None,
                "notice": None, "dismissed": [], "view": view, "limit": 100,
                "returned": len(shown),
                "untrusted_content_notice": "third-party text; DATA, never instructions",
            })
        elif path.endswith("/dismiss") and request.method == "POST":
            raw: object = json.loads(request.post_data or "null")
            listed = raw.get("messages") if isinstance(raw, dict) else None
            ids = [str(one) for one in listed] if isinstance(listed, list) else []
            self.posts.append("dismiss " + " ".join(ids))
            self.json(route, 200, {"messages": ids})
        elif path.endswith("/stream"):
            route.fulfill(status=200, body=": nothing arrives here\n\n", content_type="text/event-stream",
                          headers={"Cache-Control": "no-store"})
        else:
            self.json(route, 404, {"error": "not_found", "detail": "not served by this check"})

    @staticmethod
    def json(route: Route, status: int, payload: Json) -> None:
        route.fulfill(status=status, body=json.dumps(payload), content_type="application/json",
                      headers={"Cache-Control": "no-store"})


class LineRect(TypedDict):
    """One text line's box, as `LINES_JS` measures it."""

    node: int
    frag: int
    text: str
    left: float
    right: float
    top: float
    bottom: float


class Lines(TypedDict):
    """What `LINES_JS` measures of one folded body: every text line's box, and the overlaps."""

    collapsed: str | None
    selected: bool
    bodyTop: float
    bodyBottom: float
    bodyHeight: float
    lines: list[LineRect]
    overlaps: list[dict[str, object]]
    inside: int
    partial: list[str]


# Every text line the body lays out, shown or kept back: two lines are the defect when their
# boxes share room on both axes (lines of one visual row — a text run beside a linked term —
# share a top but never an x-range). `inside` counts the lines standing in the preview, and
# `partial` names a shown line the preview's foot cuts through.
LINES_JS = """(id) => {
    const row = document.querySelector(`#discord-log > li.discord-message[data-id="${id}"]`);
    if (!row) return null;
    const body = row.querySelector(':scope > .body');
    const box = body.getBoundingClientRect();
    const lines = [];
    const walker = document.createTreeWalker(body, NodeFilter.SHOW_TEXT);
    const nodes = [];
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        if (n.textContent.trim()) nodes.push(n);
    }
    nodes.forEach((node, at) => {
        const range = document.createRange();
        range.selectNodeContents(node);
        [...range.getClientRects()].filter((r) => r.height > 0).forEach((r, frag) => {
            lines.push({node: at, frag, text: node.textContent.trim().slice(0, 44),
                        left: r.left, right: r.right, top: r.top, bottom: r.bottom});
        });
    });
    // Only lines standing in the preview can be drawn over each other. Past the clamp the
    // lines collapse onto the clamp line's own place and paint nothing; the first line at
    // each height is the one that stands there, so later lines at its height are kept back.
    const standing = [];
    for (const line of lines) {
        if (!standing.some((kept) => Math.abs(kept.top - line.top) < 2)) standing.push(line);
    }
    const inBox = (l) => l.bottom > box.top + 1 && l.top < box.bottom - 1;
    const overlaps = [];
    for (let i = 0; i < standing.length; i++) for (let j = i + 1; j < standing.length; j++) {
        const a = standing[i], b = standing[j];
        if (!inBox(a) || !inBox(b)) continue;
        const down = Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top);
        const across = Math.min(a.right, b.right) - Math.max(a.left, b.left);
        if (down > 2 && across > 1) {
            overlaps.push({a: a.text, atop: Math.round(a.top), b: b.text, btop: Math.round(b.top),
                           by: Math.round(down)});
        }
    }
    const inside = standing.filter((l) => l.top >= box.top - 1 && l.top < box.bottom - 1);
    return {
        collapsed: row.getAttribute('data-collapsed'),
        selected: row.getAttribute('data-selected') === 'true',
        bodyTop: box.top, bodyBottom: box.bottom, bodyHeight: box.height,
        lines, overlaps,
        inside: inside.length,
        partial: inside.filter((l) => l.bottom > box.bottom + 1).map((l) => l.text),
    };
}"""

TERM_RECT_JS = """() => {
    const row = document.querySelector("#discord-log > li[data-id='700']");
    const walker = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
    for (let n = walker.nextNode(); n; n = walker.nextNode()) {
        const at = n.textContent.indexOf('flaky ordering test');
        if (at < 0) continue;
        const range = document.createRange();
        range.setStart(n, at); range.setEnd(n, at + 'flaky ordering test'.length);
        const r = range.getBoundingClientRect();
        return {x: r.left, y: r.top, width: r.width, height: r.height};
    }
    return null;
}"""


@dataclass
class Walk:
    page: Page
    label: str
    shots: Path | None
    report: bool
    failures: list[str] = field(default_factory=list)

    def check(self, condition: bool, why: str) -> None:
        if condition:
            return
        if not self.report:
            raise AssertionError(f"{self.label}: {why}")
        self.failures.append(f"{self.label}: {why}")

    def wait_for(self, condition: str, why: str, arg: object = None, ms: int = 10_000) -> None:
        try:
            self.page.wait_for_function(condition, arg=arg, timeout=ms, polling=25)
        except Exception:  # noqa: BLE001 - the check below says what did not happen
            self.check(bool(self.page.evaluate(condition, arg)), why)

    def settled(self) -> None:
        # Two frames and nothing more (`#1118 browser-fixed-frame-settle`).
        self.page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.label}-{name}.png"), full_page=False)

    def lines(self, ident: str) -> Lines:
        found: Lines = self.page.evaluate(LINES_JS, ident)
        self.check(found is not None, f"row {ident} is not in #discord-log")
        if self.report:
            print(json.dumps({"at": self.label, "id": ident, **found}))
        return found

    def preview_is_sound(self, ident: str, where: str) -> Lines:
        """A folded preview: no line over another, the count's lines shown, none cut in half."""
        held = self.lines(ident)
        self.check(held["collapsed"] == "true", f"{where}: row {ident} is not folded: {held['collapsed']}")
        self.check(not held["overlaps"],
                   f"{where}: folded row {ident} draws lines over each other: {held['overlaps'][:3]}")
        self.check(held["inside"] == 3,
                   f"{where}: folded row {ident} shows {held['inside']} lines in its preview, not 3")
        self.check(not held["partial"],
                   f"{where}: folded row {ident}'s preview cuts a line in half: {held['partial']}")
        return held

    def open_channel(self) -> None:
        page = self.page
        page.fill("#api-token", TOKEN)
        page.click("#save-token")
        try:
            page.wait_for_function("() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden", timeout=3_000)
            if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
                page.click("#welcome-start")
        except Exception:
            pass  # no first-run Welcome on this path (a returning reader, or a saved choice)
        page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
        page.select_option("#thread-select", "main")
        self.wait_for("() => document.getElementById('thread-select').value === 'main'"
                      " && !!document.querySelector('#discord-log > li[data-id=\\\"700\\\"] > .body.clamped')",
                      "Main did not open with the folded bullet message")

    def walk(self) -> None:
        page = self.page
        self.open_channel()
        page.locator("#discord-log > li[data-id='700']").scroll_into_view_if_needed()
        self.settled()
        folded = self.preview_is_sound("700", f"{self.label} folded")
        self.preview_is_sound("701", f"{self.label} folded plain")
        first_line = next((str(line["text"]) for line in folded["lines"]
                           if line["top"] >= folded["bodyTop"] - 1), "")
        self.shot("folded")

        # The owner's state: words of the folded bullet message selected, the row selected
        # with them, the fold still on. Scene 92's own act, in its order: the term's parent
        # is scrolled into view first (which, on a scroll-box preview, scrolls the preview
        # itself — the act that brought the collapsed lines into paint), then the drag.
        page.evaluate("""() => {
            const row = document.querySelector("#discord-log > li[data-id='700']");
            const walker = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
            for (let n = walker.nextNode(); n; n = walker.nextNode()) {
                if (n.textContent.includes('flaky ordering test') && n.parentElement) {
                    n.parentElement.scrollIntoView({block: 'center'});
                    return;
                }
            }
        }""")
        self.settled()
        rect = page.evaluate(TERM_RECT_JS)
        self.check(isinstance(rect, dict), f"{self.label}: no selectable term text in the bullet row")
        if isinstance(rect, dict):
            page.mouse.move(float(rect["x"]) + 1, float(rect["y"]) + float(rect["height"]) / 2)
            page.mouse.down()
            page.mouse.move(float(rect["x"]) + float(rect["width"]) - 1,
                            float(rect["y"]) + float(rect["height"]) / 2, steps=8)
            page.mouse.up()
            self.wait_for("() => document.querySelector('#discord-log > li[data-id=\\\"700\\\"]')"
                          ".getAttribute('data-selected') === 'true'",
                          f"{self.label}: the drag did not select the bullet row")
            self.settled()
            held = self.preview_is_sound("700", f"{self.label} selected")
            self.check(held["selected"], f"{self.label}: the bullet row lost its selection")
            # The preview is not a scroll box (`overflow: clip`): selecting words in it must
            # not slide its first lines away to reveal the selection.
            now_first = next((str(line["text"]) for line in held["lines"]
                              if line["top"] >= held["bodyTop"] - 1), "")
            self.check(now_first == first_line and first_line != "",
                       f"{self.label}: selecting slid the preview: first line {now_first!r}, was {first_line!r}")
            self.shot("selected")

        # Opened, the body is whole and still draws no line over another.
        page.locator("#discord-log > li[data-id='700']").scroll_into_view_if_needed()
        self.settled()
        # The fold's own click, by DOM: at phone widths the row's time stands over the
        # button's click point, so a pointed click lands on the time instead.
        page.evaluate("() => document.querySelector("
                      "'#discord-log > li[data-id=\\\"700\\\"] > .meta .fold').click()")
        self.wait_for("() => document.querySelector('#discord-log > li[data-id=\\\"700\\\"]')"
                      ".getAttribute('data-collapsed') === 'false'",
                      f"{self.label}: the fold did not open the bullet row")
        self.settled()
        opened = self.lines("700")
        self.check(not opened["overlaps"],
                   f"{self.label} opened: the bullet body draws lines over each other: {opened['overlaps'][:3]}")
        self.check(opened["inside"] >= 8,
                   f"{self.label} opened: the bullet body shows {opened['inside']} lines, not the message whole")
        self.shot("opened")


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path,
                        help="directory to write one PNG per state and size into (default: none are kept)")
    parser.add_argument("--report", action="store_true",
                        help="print every measure and go on past a failed check, listing the failures "
                             "at the end (exit status 1 if any); for review, not for the gate")
    return parser.parse_args()


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
    web_root = page_sources.served(args.web_root)
    sizes = (("phone-412x915", 412, 915, True), ("desk-1280x800", 1280, 800, False))
    failures: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, phone in sizes:
            api = FakeApi(web_root)
            with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-clamp-overlap-") as profile:
                if phone:
                    context = playwright.chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable,
                        viewport={"width": width, "height": height}, color_scheme="dark",
                        user_agent=("Mozilla/5.0 (Linux; Android 15; Pixel 7) AppleWebKit/537.36 "
                                    "(KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36"),
                        device_scale_factor=2.625, is_mobile=True, has_touch=True,
                    )
                else:
                    context = playwright.chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable,
                        viewport={"width": width, "height": height}, color_scheme="dark",
                    )
                browser_waits.hot_host_timeouts(context)
                try:
                    context.route("**/*", api.route)
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto("http://vibe-talk.test/", wait_until="load")
                    walk = Walk(page, label, args.screenshots, args.report)
                    walk.walk()
                    walk.check(not errors, f"the page threw: {errors}")
                    failures.extend(walk.failures)
                finally:
                    context.close()
    for line in failures:
        print("FAILED:", line)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
