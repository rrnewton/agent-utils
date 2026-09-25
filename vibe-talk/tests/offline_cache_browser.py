#!/usr/bin/env python3
"""Exercise /voice's saved-message snapshot in real Chromium at a phone's size.

The fake-DOM suite (tests/js/voice_page.test.mjs) covers every branch of the snapshot. This covers
what only a browser engine can: real localStorage, a real reload, real fetch failures, the Cache
Storage and service-worker registries, and layout at 412x915. It serves the checked-out assets
and a small fake API from one loopback origin, then walks one reader through:

  sign in, read a channel in Main, Threads and All -> reload with the network held: the saved rows
  are drawn before any answer -> one bounded newest-page read merges a new row without
  duplicating -> switching views makes no request -> a reload with the API unreachable keeps the
  rows and says so -> signing out removes them from the screen and the device.

Pass --screenshots DIR to keep a PNG of each step for review.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import tempfile
import threading
import time
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, unquote, urlsplit


WEB_ROOT = Path(__file__).resolve().parents[1] / "web"
TOKEN = "write-token-browser-cache-check"
CHANNEL = {"id": "1110000000000000001", "label": "lead team", "writable": True}
CACHE_KEY = "vibe-talk.voice.message-cache"
EPOCH = datetime(2026, 1, 5, 9, 0, tzinfo=timezone.utc)
Json = dict[str, object]


def message(index: int, content: str, thread: Json | None = None) -> Json:
    row: Json = {
        "id": str(200 + index),
        "channel_id": CHANNEL["id"],
        "author": "ci-bot",
        "author_id": "1000000000000000001",
        "author_is_bot": True,
        "timestamp": (EPOCH + timedelta(minutes=index)).isoformat().replace("+00:00", "Z"),
        "content": content,
    }
    if thread is not None:
        row["thread"] = thread
    return row


def thread_of(row: Json) -> Json:
    """The row's thread summary, or an empty one for a message posted to the channel itself."""
    thread = row.get("thread")
    return thread if isinstance(thread, dict) else {}


class FakeApi:
    """The routes /voice reads, answering from one mutable little store."""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.stopping = threading.Event()
        self.timeline_gate = threading.Event()
        self.timeline_gate.set()
        self.requests: list[str] = []
        root = {"id": "spaces/A/threads/one", "root_message_id": "201", "is_root": True,
                "reply_count": 1, "reply_count_exact": True}
        self.messages = [
            message(0, "main channel announcement"),
            message(1, "a thread root", root),
            message(2, "an answer in the thread", {**root, "is_root": False}),
        ]
        self.threads = [{
            "id": root["id"], "root": self.messages[1], "title": "A discussion",
            "reply_count": 1, "reply_count_exact": True,
            "updated_at": self.messages[2]["timestamp"],
        }]

    def reads(self) -> list[str]:
        with self.lock:
            return [r for r in self.requests if "/timeline?" in r or "/page" in r]

    def client_config(self) -> Json:
        return {
            "version": "browser-check",
            "chat_provider_name": "Discord",
            "channels": [CHANNEL],
            "live_poll_seconds": 30,
            "live_delivery": "poll",
            "threading_supported": True,
            "channel_registration_supported": False,
            "read_aloud": False,
        }

    def timeline(self, query: dict[str, list[str]]) -> Json:
        view = query.get("view", ["main"])[0]
        thread_id = query.get("thread_id", [None])[0]
        with self.lock:
            rows = [m for m in self.messages if view == "flat"
                    or (view == "thread" and thread_of(m).get("id") == thread_id)
                    or (view == "main" and (not thread_of(m) or thread_of(m)["is_root"]))]
            threads = list(self.threads) if view == "threads" else []
            thread = next((t for t in self.threads if t["id"] == thread_id), None)
        return {
            "channel": CHANNEL, "messages": rows, "threads": threads,
            "thread": thread if view == "thread" else None,
            "has_threads": True, "has_more": False, "next_before": None, "dismissed": [],
        }


def handler_for(api: FakeApi) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            parts = urlsplit(self.path)
            path = unquote(parts.path)
            if not path.startswith("/api/"):
                self.asset(path)
                return
            with api.lock:
                api.requests.append(f"GET {self.path}")
            if self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path == "/api/v1/client-config":
                self.json(200, api.client_config())
            elif path.endswith("/timeline"):
                api.timeline_gate.wait(20)
                self.json(200, api.timeline(parse_qs(parts.query)))
            elif path.endswith("/stream"):
                self.stream()
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def do_POST(self) -> None:  # noqa: N802
            with api.lock:
                api.requests.append(f"POST {self.path}")
            self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def asset(self, request_path: str) -> None:
            relative = "voice.html" if request_path == "/voice" else request_path.lstrip("/")
            target = (WEB_ROOT / (relative or "index.html")).resolve()
            if WEB_ROOT not in target.parents or not target.is_file():
                self.send_error(404)
                return
            body = target.read_bytes()
            content_type = mimetypes.guess_type(target.name)[0] or "application/octet-stream"
            self.send_response(200)
            self.send_header("Content-Type", content_type)
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
                pass  # a held read the browser gave up on when the page reloaded

        def stream(self) -> None:
            # Held open with keep-alive comments, as the live route is; nothing arrives on it.
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


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--browser-executable",
        default=os.environ.get("VIBE_TALK_CHROMIUM"),
        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)",
    )
    parser.add_argument(
        "--screenshots",
        type=Path,
        help="directory to write one PNG per step into (default: none are kept)",
    )
    return parser.parse_args()


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error

    api = FakeApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/voice"
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)

    try:
        with sync_playwright() as playwright, tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-") as profile:
            context = playwright.chromium.launch_persistent_context(
                profile,
                headless=True,
                executable_path=args.browser_executable,
                user_agent=(
                    "Mozilla/5.0 (Linux; Android 15; Pixel 7) AppleWebKit/537.36 "
                    "(KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36"
                ),
                viewport={"width": 412, "height": 915},
                device_scale_factor=2.625,
                is_mobile=True,
                has_touch=True,
            )
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))

            def shot(name: str) -> None:
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / f"{name}.png"))

            def rows() -> list[str]:
                return [str(row) for row in page.eval_on_selector_all(
                    "#discord-log > li[data-id]", "items => items.map(i => i.getAttribute('data-id'))")]

            def wait_rows(expected: list[str], why: str) -> None:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and rows() != expected:
                    page.wait_for_timeout(50)
                check(rows() == expected, f"{why}: showing {rows()}, expected {expected}")

            def pill() -> str:
                return str(page.evaluate(
                    "() => { const p = document.getElementById('channel-freshness');"
                    " return p.hidden ? '' : p.textContent; }"))

            def view(name: str) -> None:
                page.evaluate(f"() => document.getElementById('channel-view-{name}').click()")

            # 1. Sign in and read the channel in Main, Threads and All.
            page.goto(url, wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.click("#view-switch")
            wait_rows(["200", "201"], "Main after sign-in")
            view("threads")
            page.wait_for_selector("#thread-list > li", timeout=10_000)
            view("flat")
            wait_rows(["200", "201", "202"], "All after sign-in")
            view("main")
            wait_rows(["200", "201"], "Main again")
            saved = json.loads(page.evaluate(f"() => localStorage.getItem({json.dumps(CACHE_KEY)})") or "{}")
            scope = saved.get("scopes", {}).get(CHANNEL["id"], {})
            check(sorted(scope.get("views", {})) == ["flat", "main", "threads"],
                  f"the snapshot did not cover each view read: {sorted(scope.get('views', {}))}")
            shot("1-signed-in")

            # 2. Reload with the history read held: the saved rows draw before any answer.
            api.timeline_gate.clear()
            with api.lock:
                api.requests.clear()
                api.messages.append(message(3, "posted while the page was closed"))
            page.reload(wait_until="load")
            page.click("#view-switch")
            wait_rows(["200", "201"], "the snapshot was not drawn before the network answered")
            text = pill()
            check(text.startswith("Saved ") and "refreshing" in text, f"pill while refreshing: {text!r}")
            overlap = page.evaluate("""() => {
                const box = (id) => document.getElementById(id).getBoundingClientRect();
                const pill = box('channel-freshness'), tabs = box('channel-view-tabs');
                const list = box('scroll-area');
                return {covers: pill.top < tabs.bottom, inside: pill.left >= list.left &&
                  pill.right <= list.right && pill.top >= list.top};
            }""")
            check(not overlap["covers"], "the freshness pill covers the Main/Threads/All tabs")
            check(overlap["inside"], "the freshness pill is not over the list")
            shot("2-saved-before-network")

            # 3. One bounded newest page for the view on screen merges without duplicating.
            api.timeline_gate.set()
            wait_rows(["200", "201", "203"], "the refresh did not merge")
            reads = api.reads()
            check(len(reads) == 1, f"a cold start made {len(reads)} history reads: {reads}")
            check("view=main" in reads[0] and "before=" not in reads[0],
                  f"the cold-start read was not the newest page of Main: {reads[0]}")
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline and pill():
                page.wait_for_timeout(50)
            check(pill() == "", f"a refreshed view still says {pill()!r}")
            shot("3-merged")

            # 4. Switching among views already covered is local.
            view("threads")
            page.wait_for_selector("#thread-list > li", timeout=5_000)
            view("flat")
            wait_rows(["200", "201", "202", "203"], "All after the refresh")
            view("main")
            wait_rows(["200", "201", "203"], "Main after switching back")
            check(len(api.reads()) == 1, f"switching views read history again: {api.reads()}")
            shot("4-switched-locally")

            # 5. The API unreachable: the rows stay, and the pill says they are old.
            page.route("**/api/**", lambda route: route.abort("internetdisconnected"))
            page.reload(wait_until="load")
            page.click("#view-switch")
            wait_rows(["200", "201", "203"], "the snapshot was not kept while offline")
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and not pill().startswith("Offline"):
                page.wait_for_timeout(50)
            check(pill().startswith("Offline · showing messages saved "), f"offline pill: {pill()!r}")
            shot("5-offline")
            page.unroute("**/api/**")

            # 6. Nothing private is in Cache Storage, and no service worker holds one.
            cache_names = page.evaluate("() => caches.keys()")
            check(cache_names == [], f"Cache Storage holds {cache_names}")
            workers = page.evaluate("() => navigator.serviceWorker.getRegistrations().then(r => r.length)")
            check(workers == 0, f"{workers} service workers are registered")

            # 7. Signing out takes the rows off the screen and off the device.
            page.evaluate("() => document.getElementById('forget-token').click()")
            check(page.evaluate(f"() => localStorage.getItem({json.dumps(CACHE_KEY)})") is None,
                  "signing out left the snapshot on the device")
            check(rows() == [], f"signing out left rows on the screen: {rows()}")
            shot("7-signed-out")

            check(not errors, f"the page threw: {errors}")
            browser_version = context.browser.version if context.browser else "Chromium"
            context.close()

        print(f"{browser_version} at 412x915: snapshot drawn before the network, one newest-page read,"
              " local view switches, offline state, no Cache Storage or service worker, sign-out clears")
        return 0
    finally:
        api.stopping.set()
        api.timeline_gate.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
