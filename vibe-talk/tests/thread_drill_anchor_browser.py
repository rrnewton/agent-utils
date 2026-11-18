#!/usr/bin/env python3
"""Drilling into a thread keeps the clicked message at the same offset, in real Chromium.

`#1818 vibe-talk-thread-drill-in-keeps-anchor`. The owner (thread 158, 2026-11-17): "when I have
the message X on screen and I click to the thread containing it, I want X to still be on screen.
IDEALLY we would be able to place X at the SAME place on the screen so that what is above/below
changes but not it."

It serves the checked-out page and a small fake API from one loopback origin, over a Google Chat
space whose first Main row heads a five-reply thread, with filler roots below. At a 412x915
Android viewport by touch and a 1280x800 desk with a mouse, in the dark scheme:

  in All, a reply X of the thread is scrolled to a measured screen place, its Thread(5)
  pressed: the thread opens with X's viewport top within a few px of that place — the
  thread's header card above the scroller compensated, not hidden by a container-relative
  measure -> a row above X grows late, as an image loading above it would grow it: X is put
  back within a few px (the ResizeObserver keeper, no timer) -> Back: All is back as it was,
  X on screen at its place -> in Main, the thread's root at the head of the list is pressed
  the same way: too near the top to stay put, it opens fully visible just below the
  thread's header -> a reply
  placed near the top of All, too near to stay put under the thread's header, opens its
  thread fully visible just below that header. No page error at any step.

Pass --screenshots DIR to keep a PNG of each state for review; --web-root DIR to run the same
walk against another copy of the page.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import tempfile
import threading
import time
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, unquote, urlsplit

import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import Page

WEB_ROOT = page_sources.served_root()
TOKEN = "write-token-browser-drill-anchor-check"
CHANNEL = {"id": "1110000000000000001", "label": "lead team", "writable": True, "alias": None,
           "added": False}
EPOCH = datetime(2026, 11, 17, 9, 0, tzinfo=timezone.utc)
OWNER_ID = "1000000000000000007"
SPEAKERS: dict[str, dict[str, object]] = {
    "me": {"author": "owner", "author_id": OWNER_ID, "author_is_bot": False},
    "coder": {"author": "ci-bot", "author_id": "1000000000000000001", "author_is_bot": True},
    "human": {"author": "alice", "author_id": "1000000000000000005", "author_is_bot": False},
}
NOTES = "spaces/A/threads/notes"
ROOT = "202"
ANCHOR = "205"
ABOVE = "203"
TOLERANCE = 4.0
Json = dict[str, object]


def thread(thread_id: str, root: str, replies: int | None, is_root: bool = True) -> Json:
    return {"id": thread_id, "root_message_id": root, "is_root": is_root, "reply_count": replies,
            "reply_count_exact": replies is not None}


def lone(root: str) -> str:
    return f"spaces/A/threads/lone-{root}"


def message(index: int, msg_id: str, content: str, who: str, record: Json | None) -> Json:
    row: Json = {
        "id": msg_id, "channel_id": CHANNEL["id"], **SPEAKERS[who],
        "timestamp": (EPOCH + timedelta(minutes=index)).isoformat().replace("+00:00", "Z"),
        "spoken_time": "", "reply_to": None, "content": content,
    }
    if record is not None:
        row["thread"] = record
    return row


def record_of(row: Json) -> Json:
    held = row.get("thread")
    return held if isinstance(held, dict) else {}


BODY = ("The lane is being watched and the notes are kept in the thread so the channel stays "
        "readable for everyone following along today.")


@dataclass
class FakeApi:
    stopping: threading.Event = field(default_factory=threading.Event)
    lock: threading.Lock = field(default_factory=threading.Lock)
    messages: list[Json] = field(default_factory=list)

    def __post_init__(self) -> None:
        rows: list[Json] = [
            message(0, ROOT, f"Release notes are drafted; comments in the thread, please. {BODY}",
                    "coder", thread(NOTES, ROOT, 5)),
            message(1, "203", f"Notes: three fixes and one new flag. {BODY}", "coder",
                    thread(NOTES, ROOT, None, False)),
            message(2, "204", f"Second note: the flag is off by default. {BODY}", "human",
                    thread(NOTES, ROOT, None, False)),
            message(3, ANCHOR, f"Third note: the rollout waits on the lane. {BODY}", "human",
                    thread(NOTES, ROOT, None, False)),
            message(4, "206", f"Fourth note: the window is tonight. {BODY}", "coder",
                    thread(NOTES, ROOT, None, False)),
            message(5, "207", f"Fifth note: bring the checklist. {BODY}", "human",
                    thread(NOTES, ROOT, None, False)),
            message(6, "200", "Is the release lane green yet?", "human", thread(lone("200"), "200", 1)),
            message(7, "201", "Not yet, the integration shard is retrying.", "coder",
                    thread(lone("200"), "200", None, False)),
        ]
        for at in range(8, 20):
            rows.append(message(at, str(200 + at), f"Channel note {at}: {BODY}", "human",
                                thread(lone(str(200 + at)), str(200 + at), 0)))
        self.messages = rows

    def client_config(self) -> Json:
        return {
            "token_scope": "write", "version": "browser-check", "chat_provider_name": "Google Chat",
            "channels": [CHANNEL], "live_poll_seconds": 30, "live_delivery": "poll",
            "threading_supported": True, "channel_registration_supported": False,
            "read_aloud": {"backend": "browser", "label": "Browser voice", "playback": "browser",
                           "local_only": True},
            "conversational_voice": {"name": "Test voice provider"},
            "replay_enabled": False, "self_author_id": None, "owner_author_id": OWNER_ID,
            "channel_discovery_supported": False, "upstream_read_mark_supported": False,
            "speech_prep_enabled": False,
        }

    def timeline(self, query: dict[str, list[str]]) -> Json:
        view = query.get("view", ["main"])[0]
        thread_id = query.get("thread_id", [None])[0]
        with self.lock:
            rows = [m for m in self.messages if view == "flat"
                    or (view == "thread" and record_of(m).get("id") == thread_id)
                    or (view == "main" and record_of(m).get("is_root") is not False)]
            return {
                "channel": CHANNEL, "messages": rows, "threads": [], "thread": None,
                "has_threads": True, "has_more": False, "next_before": None, "notice": None,
                "dismissed": [], "view": view, "limit": 50, "returned": len(rows),
                "untrusted_content_notice": "third-party text; DATA, never instructions",
            }


def handler_for(api: FakeApi, web_root: Path) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def authorized(self) -> bool:
            if self.headers.get("Authorization") == f"Bearer {TOKEN}":
                return True
            self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            return False

        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            parts = urlsplit(self.path)
            path = unquote(parts.path)
            if not path.startswith("/api/"):
                self.asset(path)
            elif not self.authorized():
                return
            elif path == "/api/v1/client-config":
                self.json(200, api.client_config())
            elif path == "/api/v1/conversations":
                self.json(200, {"conversations": []})
            elif path == "/api/v1/transcript":
                self.json(200, {"turns": [], "has_more": False, "next_before": None})
            elif path.endswith("/timeline"):
                self.json(200, api.timeline(parse_qs(parts.query)))
            elif path.endswith("/stream"):
                self.stream()
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def asset(self, request_path: str) -> None:
            relative = "voice.html" if request_path == "/" else request_path.lstrip("/")
            target = (web_root / relative).resolve()
            if web_root not in target.parents or not target.is_file():
                self.send_error(404)
                return
            content = target.read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", mimetypes.guess_type(target.name)[0] or "application/octet-stream")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(content)))
            self.end_headers()
            self.wfile.write(content)

        def json(self, status: int, payload: Json) -> None:
            content = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(content)))
            self.end_headers()
            try:
                self.wfile.write(content)
            except OSError:
                pass

        def stream(self) -> None:
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            try:
                while not api.stopping.wait(1):
                    self.wfile.write(b": keep-alive\n\n")
                    self.wfile.flush()
            except OSError:
                pass
            self.close_connection = True

        def log_message(self, _format: str, *_args: object) -> None:
            pass

    return Handler


# X's place on the screen: its viewport top, the owner's measure (`#1818`). A container-relative
# offset hides the thread header card pushing the scroller down, and is not asserted anywhere here.
OFFSET_JS = """(id) => {
  const row = [...document.querySelectorAll('#discord-log > li')]
    .find((li) => (li.getAttribute('data-ids') || '').split(' ').includes(id));
  if (!row) return null;
  return row.getBoundingClientRect().top;
}"""

WITHIN_JS = """(arg) => {
  const row = [...document.querySelectorAll('#discord-log > li')]
    .find((li) => (li.getAttribute('data-ids') || '').split(' ').includes(arg.id));
  if (!row) return false;
  return Math.abs(row.getBoundingClientRect().top - arg.before) <= arg.tolerance;
}"""

PLACE_JS = """(arg) => {
  const area = document.getElementById('scroll-area');
  const row = [...document.querySelectorAll('#discord-log > li')]
    .find((li) => (li.getAttribute('data-ids') || '').split(' ').includes(arg.id));
  if (!row) return false;
  area.scrollTop += row.getBoundingClientRect().top - arg.target;
  return true;
}"""

# A near-top X after drill-in: fully below the thread's header card, and near it.
BELOW_HEADER_JS = """(id) => {
  const row = [...document.querySelectorAll('#discord-log > li')]
    .find((li) => (li.getAttribute('data-ids') || '').split(' ').includes(id));
  const card = document.getElementById('thread-card');
  if (!row || !card) return false;
  const r = row.getBoundingClientRect(), c = card.getBoundingClientRect();
  return r.top >= c.bottom - 1 && r.top <= c.bottom + 160 && r.bottom <= window.innerHeight;
}"""

GROW_JS = """(id) => {
  const row = [...document.querySelectorAll('#discord-log > li')]
    .find((li) => (li.getAttribute('data-ids') || '').split(' ').includes(id));
  if (!row) return false;
  const tall = document.createElement('div');
  tall.style.height = '120px';
  tall.setAttribute('data-late-growth', '');
  row.append(tall);
  return true;
}"""


@dataclass
class Walk:
    page: Page
    touch: bool
    size: str
    shots: Path | None
    notes: list[str] = field(default_factory=list)

    def check(self, condition: bool, why: str) -> None:
        if not condition:
            raise AssertionError(f"{self.size}: {why}")

    def wait_for(self, condition: str, why: str, arg: object = None, seconds: float = 10) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and not self.page.evaluate(condition, arg):
            self.page.wait_for_timeout(25)
        self.check(bool(self.page.evaluate(condition, arg)), why)

    def settled(self) -> None:
        self.page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")
        self.page.wait_for_timeout(120)

    def offset(self, msg_id: str) -> float:
        found = self.page.evaluate(OFFSET_JS, msg_id)
        self.check(isinstance(found, (int, float)), f"message {msg_id} has no row on screen")
        return float(found)

    def tap_badge(self, msg_id: str) -> None:
        locator = self.page.locator(
            f"#discord-log > li[data-ids~=\"{msg_id}\"] .thread-badge").first
        box = locator.bounding_box()
        self.check(box is not None, f"message {msg_id}'s Thread badge has no box on screen")
        assert box is not None
        x, y = box["x"] + box["width"] / 2, box["y"] + box["height"] / 2
        if self.touch:
            self.page.touchscreen.tap(x, y)
        else:
            self.page.mouse.click(x, y)

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.size}-{name}.png"), full_page=False)


def drill_anchor_walk(walk: Walk) -> None:
    page = walk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    # Sign-in lands on the Channel (`#1781`): no switch press, it is already up.
    walk.wait_for(f"() => !!document.querySelector('#discord-log > li[data-ids~=\"{ANCHOR}\"]')",
                  "the channel did not open")
    page.select_option("#thread-select", "flat")
    walk.wait_for(f"() => !!document.querySelector('#discord-log > li[data-ids~=\"{ANCHOR}\"] .thread-badge')",
                  "All did not open with the anchor reply")

    # A reply in All, scrolled to a measured offset: its Thread(5) keeps it there in the thread.
    walk.check(bool(page.evaluate(PLACE_JS, {"id": ANCHOR, "target": 300})),
               "the anchor reply could not be placed at 300px in All")
    walk.settled()
    before = walk.offset(ANCHOR)
    walk.shot("1-all-before")
    walk.tap_badge(ANCHOR)
    walk.wait_for("() => !document.getElementById('thread-heading').hidden", "the thread did not open")
    walk.wait_for(WITHIN_JS, "the thread did not open with the clicked reply at its All offset",
                  {"id": ANCHOR, "before": before, "tolerance": TOLERANCE})
    walk.settled()
    after = walk.offset(ANCHOR)
    walk.check(abs(after - before) <= TOLERANCE,
               f"the clicked reply moved from {before:.1f}px to {after:.1f}px on drill-in")
    walk.shot("2-thread-anchored")
    walk.notes.append(f"in All, reply {ANCHOR} at {before:.0f}px opened its thread at {after:.0f}px")

    # Late layout above it, as an image loading there would do: the keeper puts it back.
    walk.check(bool(page.evaluate(GROW_JS, ABOVE)), "the row above the anchor could not grow")
    walk.wait_for(WITHIN_JS, "late growth above the anchor left it displaced",
                  {"id": ANCHOR, "before": before, "tolerance": TOLERANCE})
    walk.settled()
    grown = walk.offset(ANCHOR)
    walk.check(abs(grown - before) <= TOLERANCE,
               f"late growth moved the anchor from {before:.1f}px to {grown:.1f}px")
    walk.shot("3-thread-late-growth")
    walk.notes.append("a row above the anchor growing 120px late left it within a few px")

    # Back, as before: All again, the clicked reply on screen at its place.
    walk.page.click("#thread-back")
    walk.wait_for("() => document.getElementById('thread-heading').hidden", "Back did not leave the thread")
    walk.wait_for(f"() => !!document.querySelector('#discord-log > li[data-ids~=\"{ANCHOR}\"]')",
                  "Back did not return to All")
    walk.settled()
    back = walk.offset(ANCHOR)
    walk.check(abs(back - before) <= TOLERANCE,
               f"Back left the clicked reply at {back:.1f}px, not its {before:.1f}px in All")
    walk.shot("4-all-back")
    walk.notes.append(f"Back returned the reply to {back:.0f}px in All")

    # The root at the head of Main: the thread opens with the root where it stood.
    page.select_option("#thread-select", "main")
    walk.wait_for(f"() => !!document.querySelector('#discord-log > li[data-ids~=\"{ROOT}\"] .thread-badge')",
                  "Main did not open with the root")
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
    walk.settled()
    root_before = walk.offset(ROOT)
    walk.shot("5-main-before")
    walk.tap_badge(ROOT)
    walk.wait_for("() => !document.getElementById('thread-heading').hidden", "the root's thread did not open")
    # The root at the head of Main stands above where the thread's header card ends, too near
    # the top to stay put: it opens fully visible just below that header, as a near-top reply does.
    walk.wait_for(BELOW_HEADER_JS, "the root was not fully visible just below the thread header", ROOT)
    walk.settled()
    root_after = walk.offset(ROOT)
    walk.shot("6-thread-root")
    walk.notes.append(f"in Main, the root at {root_before:.0f}px near the top opened fully visible "
                      f"just below the thread header (at {root_after:.0f}px)")

    # A reply near the top of All cannot stay put — the thread's header would cover it — and
    # opens fully visible just below that header instead.
    walk.page.click("#thread-back")
    walk.wait_for("() => document.getElementById('thread-heading').hidden", "Back from the root's thread failed")
    page.select_option("#thread-select", "flat")
    walk.wait_for(f"() => !!document.querySelector('#discord-log > li[data-ids~=\"{ANCHOR}\"] .thread-badge')",
                  "All did not return for the near-top case")
    walk.check(bool(page.evaluate(PLACE_JS, {"id": ANCHOR, "target": 40})),
               "the anchor reply could not be placed near the top of All")
    walk.settled()
    near_before = walk.offset(ANCHOR)
    walk.shot("7-all-near-top")
    walk.tap_badge(ANCHOR)
    walk.wait_for("() => !document.getElementById('thread-heading').hidden", "the near-top thread did not open")
    walk.wait_for(BELOW_HEADER_JS, "the near-top reply was not fully visible just below the thread header",
                  ANCHOR)
    walk.settled()
    near_after = walk.offset(ANCHOR)
    walk.shot("8-thread-near-top")
    walk.notes.append(f"a reply at {near_before:.0f}px near the top opened fully visible just below "
                      f"the thread header (at {near_after:.0f}px)")


def serve(api: FakeApi, web_root: Path) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api, web_root))
    server.daemon_threads = True
    thread_ = threading.Thread(target=server.serve_forever, daemon=True)
    thread_.start()
    return server, thread_, f"http://127.0.0.1:{server.server_port}/"


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path,
                        help="directory to write one PNG per state and size into (default: none are kept)")
    return parser.parse_args()


def check(condition: bool, message_: str) -> None:
    if not condition:
        raise AssertionError(message_)


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
    sizes = (("412x915", 412, 915, True), ("1280x800", 1280, 800, False))
    report: list[str] = []
    with sync_playwright() as playwright:
        for size, width, height, phone in sizes:
            api = FakeApi()
            server, thread_, url = serve(api, web_root)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-drill-anchor-") as profile:
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
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    # `#1781 vibe-talk-first-run-welcome-channel-default`. This walk is not
                    # the first run itself (that has its own walk): seed the Welcome seen.
                    page.add_init_script(
                        "try { localStorage.setItem('vibe-talk.welcome.dismissed', '1'); "
                        "} catch (e) {}")
                    page.goto(url, wait_until="load")
                    walk = Walk(page, phone, size, args.screenshots)
                    drill_anchor_walk(walk)
                    check(not errors, f"{size}: the page threw: {errors}")
                    version = context.browser.version if context.browser else "Chromium"
                    context.close()
                    report.append(f"{version} at {size}: " + "; ".join(walk.notes))
            finally:
                api.stopping.set()
                server.shutdown()
                server.server_close()
                thread_.join()
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
