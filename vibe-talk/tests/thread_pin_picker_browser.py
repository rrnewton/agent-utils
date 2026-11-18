#!/usr/bin/env python3
"""Pinnable threads in the thread picker, in real Chromium at 1280x800 and 390x852.

`#1550 thread-list-pins`. The spool picker opens its own pop-up list: a pin
on the LEFT of each thread row, gold while pinned (the documents star's
dress), threads bound to a subagent (root `!@liteinst`, #917) pinned by
default and labelled `@liteinst`, pinned threads grouped after the fixed
options and before the recent threads, each group newest first. Pins live
on the server (this fake keeps them as the store does, unpins recorded), so
they survive a reload; message pins stay a separate list.

Pass --screenshots DIR to keep a PNG of each step for review.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import re
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

BANG = "spaces/A/threads/bang"
INCIDENT = "spaces/A/threads/incident"
RELEASE = "spaces/A/threads/release"


def root(thread_id: str) -> Json:
    return {"id": thread_id, "root_message_id": None, "is_root": True,
            "reply_count": 1, "reply_count_exact": True}


class PinApi(fixture.FakeApi):
    """Three threads (bang-bound oldest, two plain) and a thread-pin store."""

    def __init__(self) -> None:
        super().__init__()
        bang, incident, release = root(BANG), root(INCIDENT), root(RELEASE)
        self.messages = [
            message(0, "!@liteinst please check the deploy", bang),
            message(1, "looking now", {**bang, "is_root": False}),
            message(2, "incident notes", incident),
            message(3, "an update on the incident", {**incident, "is_root": False}),
            message(4, "release checklist", release),
            message(5, "the newest reply", {**release, "is_root": False}),
        ]
        for row, thread_id in ((self.messages[0], BANG), (self.messages[2], INCIDENT),
                               (self.messages[4], RELEASE)):
            thread = row["thread"]
            assert isinstance(thread, dict)
            thread["root_message_id"] = row["id"]
        long_title = ("Release checklist for the November rollout with every "
                      "gate and owner named in full")
        self.threads = [
            {"id": BANG, "root": self.messages[0], "title": "deploy",
             "reply_count": 1, "reply_count_exact": True,
             "updated_at": self.messages[1]["timestamp"]},
            {"id": INCIDENT, "root": self.messages[2],
             "title": "Incident notes from the overnight database failover review",
             "reply_count": 1, "reply_count_exact": True,
             "updated_at": self.messages[3]["timestamp"]},
            {"id": RELEASE, "root": self.messages[4], "title": long_title,
             "display_name": "release-day",
             "reply_count": 1, "reply_count_exact": True,
             "updated_at": self.messages[5]["timestamp"]},
        ]
        # The server's thread-pin store: explicit records only. An unpin of a
        # default-pinned thread is a kept `pinned: false`, never an absence.
        self.thread_pins: dict[str, bool] = {}
        self.thread_pins_revision = 0

    def thread_pin_list(self) -> Json:
        with self.lock:
            records = [{"thread_id": t, "pinned": p}
                       for t, p in sorted(self.thread_pins.items())]
            return {"channel": CHANNEL, "records": records,
                    "revision": self.thread_pins_revision}

    def set_thread_pin(self, thread_id: str, pinned: bool) -> Json:
        with self.lock:
            changed = self.thread_pins.get(thread_id) is not pinned
            if self.thread_pins.get(thread_id) != pinned:
                self.thread_pins[thread_id] = pinned
                changed = True
                self.thread_pins_revision += 1
            return {"channel": CHANNEL, "thread_id": thread_id, "pinned": pinned,
                    "changed": changed, "revision": self.thread_pins_revision}


def handler_for(api: PinApi) -> type[BaseHTTPRequestHandler]:
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
            elif path.endswith("/thread-pins"):
                self.json(200, api.thread_pin_list())
            elif path.endswith("/pins"):
                self.json(200, api.pin_list())
            elif path.endswith("/stream"):
                self.stream()
            else:
                self.json(404, {"error": "not_found", "detail": "not served"})

        def do_PUT(self) -> None:  # noqa: N802
            self.thread_pin_write(True)

        def do_DELETE(self) -> None:  # noqa: N802
            self.thread_pin_write(False)

        def do_POST(self) -> None:  # noqa: N802
            self.rfile.read(int(self.headers.get("Content-Length") or 0))
            self.json(404, {"error": "not_found", "detail": "not served"})

        def thread_pin_write(self, pinned: bool) -> None:
            self.rfile.read(int(self.headers.get("Content-Length") or 0))
            path = unquote(urlsplit(self.path).path)
            found = re.fullmatch(r"/api/v1/channels/[^/]+/thread-pins/(.+)", path)
            if not found:
                self.json(404, {"error": "not_found", "detail": "not served"})
            elif SCOPES.get(self.headers.get("Authorization") or "") != "write":
                self.json(403, {"error": "forbidden", "detail": "read only"})
            else:
                self.json(200, api.set_thread_pin(found.group(1), pinned))

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


POPUP_JS = """() => {
    const popup = document.querySelector('.thread-picker-popup');
    if (!popup || popup.hidden) return null;
    const rows = [...popup.querySelectorAll('.thread-picker-row')].map((row) => {
        const pin = row.querySelector('.thread-pin');
        const open = row.querySelector('.thread-picker-open');
        const pinRect = pin ? pin.getBoundingClientRect() : null;
        const openRect = open ? open.getBoundingClientRect() : null;
        const slug = row.querySelector('.thread-picker-slug');
        const title = row.querySelector('.thread-picker-title');
        return {
            text: (open ? open.textContent : row.textContent).trim(),
            slug: slug ? slug.textContent.trim() : null,
            title: title ? title.textContent.trim() : null,
            pinned: pin ? pin.getAttribute('aria-pressed') : null,
            pinLeft: pinRect ? pinRect.left : null,
            openLeft: openRect ? openRect.left : null,
            gold: pin ? getComputedStyle(pin).color : null,
            goldBg: pin ? getComputedStyle(pin).backgroundColor : null,
        };
    });
    return {rows, overflow: document.documentElement.scrollWidth - window.innerWidth,
            popupWidth: popup.getBoundingClientRect().width, innerWidth: window.innerWidth};
}"""


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
    api = PinApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-threadpin-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark",
                has_touch=touch, is_mobile=touch)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            desk = Desk(page, args.screenshots, f"threadpin-{label}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            picker_walk(desk, api)
            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
            return f"{version} at {width}x{height}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def open_popup(desk: Desk) -> list[Json]:
    page: Page = desk.page
    page.click("#thread-select")
    desk.wait_for("() => { const p = document.querySelector('.thread-picker-popup');"
                  " return p && !p.hidden && p.querySelectorAll('.thread-picker-thread').length === 3; }",
                  "the picker's pop-up did not list three threads", seconds=120)
    desk.settled()
    found = page.evaluate(POPUP_JS)
    assert isinstance(found, dict)
    rows = found["rows"]
    assert isinstance(rows, list)
    return rows


def texts(rows: list[Json]) -> list[str]:
    return [str(row["text"]) for row in rows]


def picker_walk(desk: Desk, api: PinApi) -> None:
    page: Page = desk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    try:
        page.wait_for_function("() => !document.getElementById('screen-welcome').hidden || !document.getElementById('screen-main').hidden", timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass  # no first-run Welcome on this path (#1781: a returning reader, a refusal, or a saved choice)
    page.wait_for_selector("#search-toggle", state="visible", timeout=120_000)
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length >= 3",
                  "the channel did not open", seconds=120)
    desk.settled()

    rows = open_popup(desk)
    names = texts(rows)
    desk.check(names[:2] == ["Main", "All"], f"fixed options are not first: {names}")
    thread_rows = [r for r in rows if r["pinned"] is not None]
    desk.check(len(thread_rows) == 3, f"expected three thread rows: {rows}")
    # Pinned first: the bang-bound thread, oldest of all, leads the threads.
    desk.check("@liteinst" in str(thread_rows[0]["text"]),
               f"the bang-bound thread is not first with its @label: {names}")
    desk.check(thread_rows[0]["pinned"] == "true",
               f"the bang-bound thread is not pinned by default: {thread_rows[0]}")
    desk.check("release" in str(thread_rows[1]["text"]).lower()
               and "incident" in str(thread_rows[2]["text"]).lower(),
               f"recent threads are not newest first after the pinned group: {names}")
    for row in thread_rows:
        desk.check(row["pinLeft"] is not None and row["openLeft"] is not None
                   and float(str(row["pinLeft"])) < float(str(row["openLeft"])),
                   f"the pin is not on the row's left: {row}")
    gold = str(thread_rows[0]["gold"])
    desk.check(gold not in ("", "rgb(231, 233, 238)"),
               f"a pinned pin is not gold-dressed: {thread_rows[0]}")
    # `#1563`: slug plus readable title; an unnamed thread shows its full title.
    named = next(r for r in thread_rows if r["slug"] == "release-day")
    desk.check("Release checklist for the November rollout" in str(named["title"]),
               f"the named row does not show slug plus full title: {named}")
    unnamed = next(r for r in thread_rows if r["slug"] in (None, ""))
    desk.check(str(unnamed["title"]).startswith("Incident notes from the overnight"),
               f"the unnamed row does not show its full title: {unnamed}")
    found = page.evaluate(POPUP_JS)
    want = min(560, int(found["innerWidth"]) - 16)
    desk.check(abs(float(found["popupWidth"]) - want) <= 2,
               f"pop-up width {found['popupWidth']} is not min(560, viewport-16)={want}")
    desk.shot("1-popup-bang-pinned")
    desk.notes.append(f"pop-up order {names}; @liteinst pinned by default, gold, pin left")

    # Pin the newest plain thread: it joins the pinned group, ahead by recency.
    page.click(".thread-picker-popup .thread-pin[aria-label^='Pin release']")
    desk.wait_for("() => document.querySelector(\".thread-pin[aria-label^='Unpin release']\") !== null",
                  "pinning the release thread did not take", seconds=120)
    desk.settled()
    rows = open_popup_rows(page)
    thread_rows = [r for r in rows if r["pinned"] is not None]
    desk.check("release" in str(thread_rows[0]["text"]).lower()
               and "@liteinst" in str(thread_rows[1]["text"]),
               f"pinned group is not recency-ordered: {texts(rows)}")
    desk.shot("2-popup-two-pinned")

    # Persistence: a reload reads the server's records back.
    page.reload(wait_until="load")
    page.wait_for_selector("#search-toggle", state="visible", timeout=120_000)
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length >= 3",
                  "the channel did not reopen after reload", seconds=120)
    desk.settled()
    rows = open_popup(desk)
    thread_rows = [r for r in rows if r["pinned"] is not None]
    desk.check(thread_rows[0]["pinned"] == "true" and "release" in str(thread_rows[0]["text"]).lower(),
               f"the pin did not survive a reload: {texts(rows)}")
    desk.notes.append("pins survive a reload from the server's records")

    # Unpin the bang-bound thread: it leaves the group and stays unpinned.
    page.click(".thread-picker-popup .thread-pin[aria-label^='Unpin @liteinst']")
    desk.wait_for("() => document.querySelector(\".thread-pin[aria-label^='Pin @liteinst']\") !== null",
                  "unpinning the bang-bound thread did not take", seconds=120)
    desk.settled()
    rows = open_popup_rows(page)
    thread_rows = [r for r in rows if r["pinned"] is not None]
    desk.check(thread_rows[0]["pinned"] == "true" and "release" in str(thread_rows[0]["text"]).lower()
               and "@liteinst" in str(thread_rows[-1]["text"])
               and thread_rows[-1]["pinned"] == "false",
               f"the unpinned bang thread did not fall to the recents: {texts(rows)}")
    desk.shot("3-popup-bang-unpinned")
    page.reload(wait_until="load")
    page.wait_for_selector("#search-toggle", state="visible", timeout=120_000)
    desk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length >= 3",
                  "the channel did not reopen after the second reload", seconds=120)
    desk.settled()
    rows = open_popup(desk)
    page.wait_for_function(
        "() => { const rows = [...document.querySelectorAll('.thread-picker-thread')];"
        " const last = rows[rows.length - 1];"
        " return last && last.querySelector('.thread-pin')?.getAttribute('aria-pressed') === 'false'"
        " && last.textContent.includes('@liteinst'); }", timeout=120_000)
    rows = open_popup_rows(page)
    thread_rows = [r for r in rows if r["pinned"] is not None]
    desk.check(thread_rows[-1]["pinned"] == "false" and "@liteinst" in str(thread_rows[-1]["text"]),
               f"the bang thread's unpin did not stick across a reload: {texts(rows)}")
    desk.notes.append("a bang-bound thread's unpin sticks across a reload")

    # `#1563`: inside the thread, the header leads with the slug.
    page.evaluate("""() => {
        const open = [...document.querySelectorAll('.thread-picker-open')]
            .find((el) => el.querySelector('.thread-picker-slug')?.textContent === 'release-day');
        open.click();
    }""")
    page.wait_for_function(
        "() => (document.querySelector('#thread-card-text')?.textContent || '')"
        ".includes('release-day')", timeout=120_000)
    desk.shot("4-thread-header-slug")
    desk.notes.append("thread header leads with the slug")
    with api.lock:
        check(api.pins == {}, "message pins were touched by thread pins")


def open_popup_rows(page: Page) -> list[Json]:
    found = page.evaluate(POPUP_JS)
    assert isinstance(found, dict)
    rows = found["rows"]
    assert isinstance(rows, list)
    return rows


if __name__ == "__main__":
    raise SystemExit(main())
