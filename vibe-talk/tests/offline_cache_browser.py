#!/usr/bin/env python3
"""Exercise /voice's saved-message snapshot in real Chromium at a phone's size.

The fake-DOM suite (tests/js/voice_page.test.mjs) covers every branch of the snapshot. This covers
what only a browser engine can: real localStorage, a real reload, real fetch failures, the Cache
Storage and service-worker registries, and layout at 412x915 and at a 1280x800 desk. For each of
those it serves the checked-out assets and a small fake API from one loopback origin, then walks
one reader through:

  sign in, read a channel in Main, Threads and All -> reload with the network held: the saved rows
  are drawn before any answer -> one bounded newest-page read merges a new row without
  duplicating -> switching views makes no request -> a reload with the API unreachable keeps the
  rows and says so -> a reload whose refresh fails keeps them and says that -> signing out removes
  them from the screen and the device. Before the reloads, the pill also comes and goes in place,
  on the channel's own poll, under a reader at the top, in the middle and at the newest line.

While the freshness pill is up — refreshing, offline, failed — it must cover neither the view tabs
nor the list's header seam or first row, and #scroll-area must be the same box with it as without
it (`#32 freshness-pill-overlap`).

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
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, unquote, urlsplit

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Route


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


# Park the reader (`where`: "top", "middle", "newest", or "" to stay put), then say where they are:
# the top of the thread root's row within the list, how far the newest line is below the fold, and the
# scroll offset.
PLACE_JS = """async (where) => {
    const area = document.getElementById('scroll-area');
    const range = area.scrollHeight - area.clientHeight;
    if (where) area.scrollTop = {top: 0, middle: Math.round(range / 2), newest: area.scrollHeight}[where];
    await new Promise((settled) => requestAnimationFrame(() => requestAnimationFrame(settled)));
    const row = document.querySelector('#discord-log > li[data-id="201"]');
    return {row: row.getBoundingClientRect().top - area.getBoundingClientRect().top,
            gap: area.scrollHeight - area.clientHeight - area.scrollTop, top: area.scrollTop};
}"""

# Everything that can be covered, measured at the head of the list, with the view tabs and without
# them. `problems` is empty when the pill covers none of the tabs, the header seams or the first row,
# fits on its one line, and sits over the list; `area` is #scroll-area's box and `bare` the same box without the pill.
# Scrolling to the top is free only because FakeApi answers has_more: False; with more history it
# would page older rows in and spend a read the "one newest-page read" check counts.
GEOMETRY_JS = """() => {
    const area = document.getElementById('scroll-area');
    area.scrollTop = 0;
    const shown = (e) => Boolean(e) && !e.hidden && e.getClientRects().length > 0;
    const box = (e) => e.getBoundingClientRect();
    const meets = (a, b) => a.top < b.bottom && b.top < a.bottom && a.left < b.right && b.left < a.right;
    const pill = document.getElementById('channel-freshness');
    const tabs = document.getElementById('channel-view-tabs');
    const problems = [];
    const covering = (layout) => {
        const p = box(pill), list = box(area);
        if (shown(tabs) && meets(p, box(tabs))) problems.push(`${layout}: it covers the Main/Threads/All tabs`);
        const seams = [...document.querySelectorAll('#pane-discord .seam')].filter(shown);
        if (seams.length === 0) problems.push(`${layout}: there is no header seam to measure against`);
        seams.forEach((seam) => {
            if (meets(p, box(seam))) problems.push(`${layout}: it covers the seam "${seam.textContent.trim()}"`);
        });
        const first = document.querySelector('#discord-log > li');
        if (!shown(first)) problems.push(`${layout}: there is no first row to measure against`);
        else if (meets(p, box(first))) problems.push(`${layout}: it covers the first row`);
        if (pill.scrollWidth > pill.clientWidth + 1) problems.push(`${layout}: it is cut short: ${pill.textContent}`);
        if (p.top < list.top || p.left < list.left || p.right > list.right) {
            problems.push(`${layout}: it is not over the list`);
        }
    };
    if (!shown(pill)) {
        problems.push('the freshness pill is not shown');
    } else {
        covering('with tabs');
        // A source without threads hides the tabs by this same attribute, and the grid row they
        // held collapses: the layout the overlap was first seen in.
        const tabsHidden = tabs.hidden;
        tabs.hidden = true;
        covering('without tabs');
        tabs.hidden = tabsHidden;
    }
    const size = () => {
        const a = box(area);
        return [a.top, a.left, a.width, a.height, area.clientWidth, area.clientHeight]
            .map((n) => n.toFixed(2)).join(',');
    };
    // The same box with the pill and its room taken away, in the same state, so that a banner
    // appearing for its own reasons is not mistaken for the pill resizing the list.
    const measured = size(), pane = document.getElementById('pane-discord');
    const wasHidden = pill.hidden, reserved = pane.hasAttribute('data-freshness');
    pill.hidden = true;
    pane.removeAttribute('data-freshness');
    const bare = size();
    pill.hidden = wasHidden;
    if (reserved) pane.setAttribute('data-freshness', '');
    return {problems, area: measured, bare, tabs: shown(tabs)};
}"""

# A phone, the common narrow Android width, and a desk: the desktop regime is `(min-width: 900px) and
# (pointer: fine)`, so the last is a fine pointer without touch, not a wide phone. Below 360 the
# pill's longest line is ellipsised, which the "cut short" check would report.
PROFILES = (("phone", 412, 915, True), ("phone-360", 360, 800, True), ("desktop", 1280, 800, False))


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)

    with sync_playwright() as playwright:
        for label, width, height, mobile in PROFILES:
            browser_version = walk(playwright.chromium, args, label, width, height, mobile)
            print(f"{browser_version} {label} at {width}x{height}: snapshot drawn before the network,"
                  " one newest-page read, local view switches, offline and failed states, the pill"
                  " clear of the tabs and the header with #scroll-area unmoved, a scrolled reader"
                  " held in place as it comes and goes, no Cache Storage or"
                  " service worker, sign-out clears")
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
         mobile: bool) -> str:
    """One reader, start to finish, against a fresh fake API and a fresh browser profile."""
    api = FakeApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/voice"
    platform = "Linux; Android 15; Pixel 7" if mobile else "X11; Linux x86_64"
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-") as profile:
            context = chromium.launch_persistent_context(
                profile,
                headless=True,
                executable_path=args.browser_executable,
                user_agent=(
                    f"Mozilla/5.0 ({platform}) AppleWebKit/537.36 (KHTML, like Gecko)"
                    f" Chrome/151.0.0.0 {'Mobile ' if mobile else ''}Safari/537.36"
                ),
                viewport={"width": width, "height": height},
                device_scale_factor=2.625 if mobile else 1,
                is_mobile=mobile,
                has_touch=mobile,
            )
            page = context.pages[0]
            # Time flows as normal; `#32`'s in-place step jumps it past the channel's poll.
            page.clock.install()
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))

            def shot(name: str) -> None:
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / f"{label}-{name}.png"))

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

            def failing(route: Route) -> None:
                route.fulfill(status=502, content_type="application/json",
                              body=json.dumps({"error": "discord_error", "detail": "the provider is down"}))

            def timeline_read(address: str) -> bool:
                return "/timeline" in address

            def wait_pill(prefix: str, why: str) -> None:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not (pill().startswith(prefix) if prefix else pill() == ""):
                    page.wait_for_timeout(50)
                check(pill().startswith(prefix) if prefix else pill() == "", f"{label}, {why}: pill {pill()!r}")

            def poll_channel(fail: bool) -> None:
                """The channel's own re-read, brought forward on the page clock rather than awaited."""
                if fail:
                    page.route(timeline_read, failing)
                page.clock.fast_forward(60_000)
                if fail:
                    wait_pill("Refresh failed", "the in-place refresh did not fail")
                    page.unroute(timeline_read, failing)
                else:
                    wait_pill("", "the in-place refresh did not recover")
                page.wait_for_timeout(100)

            def place(where: str) -> dict[str, float]:
                found = page.evaluate(PLACE_JS, where)
                return {"row": float(found["row"]), "gap": float(found["gap"]), "top": float(found["top"])}

            def geometry(state: str, name: str) -> str:
                # At the head of the list, where the header is: a pill over row 40 of 90 is the
                # accepted cost of an overlay, a pill over the header is `#32 freshness-pill-overlap`.
                found = page.evaluate(GEOMETRY_JS)
                shot(name)
                problems = [str(problem) for problem in found["problems"]]
                check(not problems, f"{label}, {state}: {problems}")
                check(str(found["area"]) == str(found["bare"]),
                      f"{label}, {state}: #scroll-area is {found['area']}, {found['bare']} without the pill")
                check(bool(found["tabs"]), f"{label}, {state}: the view tabs were not on screen to measure")
                return str(found["area"])

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
            baseline = str(page.evaluate(GEOMETRY_JS)["area"])

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
            check(geometry("refreshing", "2-saved-before-network") == baseline,
                  f"{label}: #scroll-area is not the size it was before the reload")

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
            check(str(page.evaluate(GEOMETRY_JS)["area"]) == baseline,
                  f"{label}: #scroll-area changed size when the pill cleared")
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

            # 4b. The pill arriving and leaving IN PLACE, over a list long enough to scroll. Its room
            # is made above a reader who has scrolled down, where browser scroll anchoring does not
            # hold, so the page must: the row being read stays put, and a reader at the newest line
            # stays there. At the very top the header moves down out from under the pill instead.
            # Tall rows, so that three of them overflow. The failed read's error banner is kept off
            # the screen: it is a row ABOVE the list, and moves the reader by its own height as it
            # comes and goes, which is its behaviour and not the pill's.
            page.add_style_tag(content="#discord-log > li[data-id] { min-height: 70vh; }"
                                       " #error-wrap { display: none !important; }")
            for where in ("middle", "newest", "top"):
                before = place(where)
                parked = {"middle": 0 < before["top"] and before["gap"] > 100,
                          "newest": before["gap"] <= 2, "top": before["top"] == 0}[where]
                check(parked, f"{label}: could not park the reader at the {where}: {before}")
                for fail in (True, False):
                    poll_channel(fail)
                    after = place("")
                    state = f"{label}, reader at the {where}: the pill {'appeared' if fail else 'cleared'}"
                    if where == "middle":
                        moved = after["row"] - before["row"]
                        # Under 2px: a refresh makes three position fixes (its loading line
                        # arriving, the pill's room, the loading line going), each landing on a
                        # whole pixel. The defect was the pill's whole height, ~32px.
                        check(abs(moved) < 2, f"{state} and moved the row being read by {moved:.1f}px: {after}")
                    elif where == "newest":
                        check(after["gap"] <= 2, f"{state} and left the newest line {after['gap']:.1f}px below")
                    elif fail:
                        check(after["top"] == 0, f"{state} and scrolled the header away: {after}")
                        geometry("in place at the top", "4b-top")
            shot("4b-in-place")

            # 5. The API unreachable: the rows stay, and the pill says they are old.
            page.route("**/api/**", lambda route: route.abort("internetdisconnected"))
            page.reload(wait_until="load")
            page.click("#view-switch")
            wait_rows(["200", "201", "203"], "the snapshot was not kept while offline")
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and not pill().startswith("Offline"):
                page.wait_for_timeout(50)
            check(pill().startswith("Offline · showing messages saved "), f"offline pill: {pill()!r}")
            geometry("offline", "5-offline")
            page.unroute("**/api/**")

            # 5b. The API reachable but failing: the rows stay, and the pill says the refresh failed.
            page.route(timeline_read, failing)
            page.reload(wait_until="load")
            page.click("#view-switch")
            wait_rows(["200", "201", "203"], "the snapshot was not kept after a failed refresh")
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and not pill().startswith("Refresh failed"):
                page.wait_for_timeout(50)
            check(pill().startswith("Refresh failed · showing messages from "), f"failed pill: {pill()!r}")
            geometry("failed", "5b-failed")
            page.unroute(timeline_read, failing)

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

            desktop = bool(page.evaluate("() => matchMedia('(min-width: 900px) and (pointer: fine)').matches"))
            check(desktop != mobile, f"{label}: the page chose the wrong layout regime")
            check(not errors, f"the page threw: {errors}")
            browser_version = context.browser.version if context.browser else "Chromium"
            context.close()
        return browser_version
    finally:
        api.stopping.set()
        api.timeline_gate.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
