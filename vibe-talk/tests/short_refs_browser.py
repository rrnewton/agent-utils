#!/usr/bin/env python3
"""Short references in a message, in real Chromium at 1280x800 and 390x852.

`#1565 vibe-talk-short-ref-links`, `#1607 short-ref-emoji-render`,
`#1796 vibe-talk-bare-short-refs-render`. The server draws `:thread:060` as
a marked thread link labelled `🧵060/runner-wedged` (its filed slug),
`:msg:1847` as a marked message link labelled `✉️1847`, both titled
"Opens here", `:issue:1565` as a tracker link keeping its written label,
and an unresolvable reference as plain `🧵999` with a dotted underline
(tested in `src/render.rs`); this walk hands the page bodies as the server
writes them and checks the taps: a thread link drills into the thread, a
message link highlights its message in Main, and the unknown reference is
no link at all.

Pass --screenshots DIR to keep a PNG of each step for review.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, unquote, urlsplit

import offline_cache_browser as fixture
from offline_cache_browser import CHANNEL, SCOPES, TOKEN, Json, check, message
from two_column_browser import Desk
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

WEB_ROOT = fixture.WEB_ROOT
SIZES = (("desk", 1280, 800, False), ("phone", 390, 852, True))
THREAD = "spaces/A/threads/one"
CHAN = str(CHANNEL["id"])


def mark(channel: str, msg: str, thread: str = "") -> str:
    from urllib.parse import quote
    return ":".join(quote(part, safe="") for part in ("discord", channel, msg, thread))


class ShortRefApi(fixture.FakeApi):
    """One thread, and a Main message citing it by short reference."""

    def __init__(self) -> None:
        super().__init__()
        root = {"id": THREAD, "root_message_id": "201", "is_root": True,
                "reply_count": 1, "reply_count_exact": True}
        html = (
            '<p>see <a href="?c=' + CHAN + '&v=thread&t=spaces%2FA%2Fthreads%2Fone" '
            f'data-msg="{mark(CHAN, "", THREAD)}" data-shortref="thread" '
            'title="Opens here" '
            'target="_blank" rel="noopener noreferrer nofollow">🧵060/runner-wedged</a> and '
            f'<a href="?c={CHAN}&v=all&m=discord:{CHAN}:201" '
            f'data-msg="{mark(CHAN, "201")}" data-shortref="email" '
            'title="Opens here" '
            'target="_blank" rel="noopener noreferrer nofollow">✉️1847</a> and '
            '<a href="https://tracker.example.invalid/issues/1565" '
            'target="_blank" rel="noopener noreferrer nofollow">:issue:1565</a> and '
            '<span class="shortref-unknown">🧵999</span></p>\n'
        )
        citing = message(3, "see :thread:060 and :msg:1847 and :issue:1565 and :thread:999")
        citing["content_html"] = html
        self.messages = [
            message(0, "main channel announcement"),
            message(1, "a thread root", root),
            message(2, "an answer in the thread", {**root, "is_root": False}),
            citing,
        ]
        self.threads = [{
            "id": THREAD, "root": self.messages[1], "title": "a thread root",
            "reply_count": 1, "reply_count_exact": True,
            "updated_at": self.messages[2]["timestamp"],
        }]


def handler_for(api: ShortRefApi) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_args: object) -> None:
            pass

        def do_GET(self) -> None:  # noqa: N802
            parts = urlsplit(self.path)
            path = unquote(parts.path)
            if not path.startswith("/api/"):
                self.asset(path)
                return
            scope = SCOPES.get(self.headers.get("Authorization") or "")
            if scope is None:
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path == "/api/v1/client-config":
                self.json(200, api.client_config(scope))
            elif path.endswith("/timeline"):
                self.json(200, api.timeline(parse_qs(parts.query)))
            elif path.endswith("/pins"):
                self.json(200, api.pin_list())
            elif path.endswith("/stream"):
                self.stream()
            else:
                self.json(404, {"error": "not_found", "detail": "not served"})

        def do_POST(self) -> None:  # noqa: N802
            self.rfile.read(int(self.headers.get("Content-Length") or 0))
            self.json(404, {"error": "not_found", "detail": "not served"})

        def asset(self, request_path: str) -> None:
            relative = "voice.html" if request_path == "/" else request_path.lstrip("/")
            target = (WEB_ROOT / relative).resolve()
            if WEB_ROOT not in target.parents or not target.is_file():
                self.send_error(404)
                return
            body = target.read_bytes()
            self.send_response(200)
            self.send_header("Content-Type",
                             mimetypes.guess_type(target.name)[0] or "application/octet-stream")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def json(self, status: int, payload: Json) -> None:
            body = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
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

    return Handler


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=WEB_ROOT,
                        help="directory holding the page to serve")
    parser.add_argument("--screenshots", type=Path,
                        help="directory to write one PNG per state into (default: none)")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required") from error
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    globals()["WEB_ROOT"] = fixture.WEB_ROOT
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, touch in SIZES:
            report.append(walk(playwright.chromium, args, label, width, height, touch))
    for line in report:
        print(line)
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str,
         width: int, height: int, touch: bool) -> str:
    api = ShortRefApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-shortrefs-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark",
                has_touch=touch, is_mobile=touch)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"shortrefs-{label}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            short_ref_walk(desk, api)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def short_ref_walk(desk: Desk, api: ShortRefApi) -> None:
    page: Page = desk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length >= 3",
                  "the channel did not open")
    desk.settled()
    desk.shot("main")

    found = page.evaluate("""() => {
      const thread = document.querySelector('a[data-shortref="thread"]');
      const email = document.querySelector('a[data-shortref="email"]');
      const unknown = document.querySelector('.shortref-unknown');
      const issue = [...document.querySelectorAll('a')].find(a => a.textContent === ':issue:1565');
      return {
        thread: thread ? thread.textContent : null,
        threadTitle: thread ? thread.getAttribute('title') : null,
        email: email ? email.textContent : null,
        emailTitle: email ? email.getAttribute('title') : null,
        unknown: unknown ? unknown.textContent : null,
        unknownIsLink: unknown ? unknown.tagName === 'A' : null,
        issue: issue ? issue.getAttribute('href') : null,
        overflow: document.documentElement.scrollWidth - window.innerWidth,
      };
    }""")
    desk.check(found["thread"] == "🧵060/runner-wedged",
               f"the thread reference does not carry its slug: {found}")
    desk.check(found["threadTitle"] == "Opens here",
               f"the thread link is not titled Opens here: {found}")
    desk.check(found["email"] == "✉️1847", f"the message reference is not a link: {found}")
    desk.check(found["emailTitle"] == "Opens here",
               f"the message link is not titled Opens here: {found}")
    desk.check(found["unknown"] == "🧵999" and found["unknownIsLink"] is False,
               f"the unknown reference is not plain text: {found}")
    desk.check(found["issue"] == "https://tracker.example.invalid/issues/1565",
               f"the issue reference does not use the template: {found}")
    desk.check(found["overflow"] <= 0, f"the page overflows sideways: {found}")

    # A message link highlights its message where Main already shows it.
    page.click('a[data-shortref="email"]')
    desk.wait_for("""() => {
      const row = document.querySelector('#discord-log > li[data-id="201"]');
      return row && (row.querySelector('.linked-marker') || row.classList.contains('linked'));
    }""", "the message link did not highlight its message in Main")
    desk.settled()
    desk.shot("email-highlight")

    # A thread link drills into the thread.
    page.click('a[data-shortref="thread"]')
    desk.wait_for("""() => {
      const rows = [...document.querySelectorAll('#discord-log > li[data-id]')];
      return rows.some(r => r.getAttribute('data-id') === '202')
        && (document.body.textContent || '').includes('an answer in the thread');
    }""", "the thread link did not open the thread")
    desk.settled()
    desk.shot("thread-open")
    desk.notes.append("short references link, highlight, and drill in")


if __name__ == "__main__":
    raise SystemExit(main())
