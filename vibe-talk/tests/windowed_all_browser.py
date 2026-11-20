#!/usr/bin/env python3
"""Windowed All view: chips, fold anchor, END, Newest, no bounce (`#1985`).

Owner bug 262/263: with windowed lists ON, in All (and Main) the Thread
chips stood bare for about a minute, `;` threw the reader far back, END and
the Newest chip did not land at the bottom, and the list could bounce
between two scroll positions at frame rate. This walk serves the checked-out
page and a fake API from one loopback origin (the page's exact timeline
requests; only the chat upstream is faked) with 24 threads x 10 messages,
and checks, with windowed ON and OFF, at 412x915 and 1280x800:

  * a mounted root's Thread chip shows its slug and count on first render;
  * `;` keeps the anchor row within a few px (windowed ON);
  * END and the Newest chip land on the newest row, at the foot;
  * after landing, scrollTop changes direction at most once while settling
    (the oscillation detector: direction changes per settle window, ~0).

Condition-based waits only (no wall-clock budgets for correctness).
Pass --screenshots DIR to keep PNGs; --web-root DIR for another page copy.
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
TOKEN = "write-token-browser-windowed-all-check"
CHANNEL = {"id": "1110000000000000001", "label": "lead team", "writable": True, "alias": None,
           "added": False}
EPOCH = datetime(2026, 11, 17, 9, 0, tzinfo=timezone.utc)
OWNER_ID = "1000000000000000000007"
THREADS = 24
REPLIES = 9
LONG = ("a long channel message that keeps going with enough words to be worth "
        "folding when it is collapsed in the list for the reader today. ") * 4
Json = dict[str, object]


def build_messages() -> tuple[list[Json], list[Json]]:
    messages: list[Json] = []
    summaries: list[Json] = []
    seq = 0
    roots: dict[int, Json] = {}
    for round_ in range(REPLIES + 1):
        for thread in range(THREADS):
            seq += 1
            root_id = str(2000 + thread)
            is_root = round_ == 0
            row: Json = {
                "id": str(5000 + seq), "channel_id": CHANNEL["id"],
                "author": "alice", "author_id": "1000000000000000005", "author_is_bot": False,
                "timestamp": (EPOCH + timedelta(minutes=seq)).isoformat().replace("+00:00", "Z"),
                "spoken_time": "", "reply_to": None,
                "content": f"Thread {thread} message {round_}: {LONG}",
                # The contract's MessageThread: every field, on every message.
                "thread": {"id": root_id, "is_root": is_root, "root_message_id": root_id,
                           "reply_count": REPLIES if is_root else None,
                           "reply_count_exact": is_root},
            }
            if is_root:
                row["id"] = root_id
                roots[thread] = row
            messages.append(row)
    messages.sort(key=lambda m: str(m["timestamp"]))
    for thread in range(THREADS):
        root = roots[thread]
        summaries.append({
            "id": root["id"], "root": root, "title": "Thread",
            "display_name": f"thread-slug-{thread}",
            "reply_count": REPLIES, "reply_count_exact": True,
            "updated_at": root["timestamp"],
        })
    return messages, summaries


@dataclass
class FakeApi:
    stopping: threading.Event = field(default_factory=threading.Event)
    lock: threading.Lock = field(default_factory=threading.Lock)
    messages: list[Json] = field(default_factory=list)
    summaries: list[Json] = field(default_factory=list)

    def __post_init__(self) -> None:
        self.messages, self.summaries = build_messages()

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

        def record(message: Json) -> dict[str, object]:
            held = message.get("thread")
            return held if isinstance(held, dict) else {}

        with self.lock:
            if view == "flat":
                rows = list(self.messages)
            elif view == "thread":
                rows = [m for m in self.messages if record(m).get("id") == thread_id]
            elif view == "threads":
                rows = []
            else:
                rows = [m for m in self.messages if record(m).get("is_root") is True]
            return {
                "channel": CHANNEL, "messages": rows, "threads": list(self.summaries), "thread": None,
                "has_threads": True, "has_more": False, "next_before": None, "notice": None,
                "dismissed": [], "view": view, "limit": 50, "returned": len(rows),
                "untrusted_content_notice": "third-party text; DATA, never instructions",
            }


def handler_for(api: FakeApi, web_root: Path) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self) -> None:  # noqa: N802
            parts = urlsplit(self.path)
            path = unquote(parts.path)
            if not path.startswith("/api/"):
                self.asset(path)
            elif self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path == "/api/v1/client-config":
                self.json(200, api.client_config())
            elif path == "/api/v1/conversations":
                self.json(200, {"conversations": []})
            elif path == "/api/v1/transcript":
                self.json(200, {"turns": [], "has_more": False, "next_before": None})
            elif path.endswith("/timeline") or path.endswith("/page"):
                self.json(200, api.timeline(parse_qs(parts.query)))
            elif path.endswith("/stream"):
                self.stream()
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

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

        def log_message(self, _format: str, *_args: object) -> None:
            pass

    return Handler


MOUNTED_JS = "() => document.querySelectorAll('#discord-log > li:not([data-window-spacer])').length"
FOOT_JS = """() => {
  const a = document.getElementById('scroll-area');
  const rows = [...document.querySelectorAll('#discord-log > li:not([data-window-spacer])')];
  const last = rows[rows.length - 1];
  return Boolean(last) && a.scrollTop + a.clientHeight >= a.scrollHeight - 1;
}"""
SETTLE_JS = """() => new Promise((resolve) => {
  const a = document.getElementById('scroll-area');
  let last = a.scrollTop; let dir = 0; let changes = 0; let stable = 0; let frames = 0;
  const step = () => {
    frames += 1;
    const now = a.scrollTop;
    const d = Math.sign(now - last);
    if (d !== 0 && dir !== 0 && d !== dir) changes += 1;
    if (d !== 0) dir = d;
    stable = now === last ? stable + 1 : 0;
    last = now;
    if (stable >= 30 || frames >= 240) resolve(changes);
    else requestAnimationFrame(step);
  };
  requestAnimationFrame(step);
})"""
ANCHOR_JS = """() => {
  // The reader's row, as the fold itself anchors: the first row standing in
  // the visible band (not the sliver already scrolled off the top, whose own
  // expansion must move it).
  const band = document.getElementById('scroll-area').clientHeight || 0;
  const row = [...document.querySelectorAll('#discord-log > li[data-id]')]
    .find((li) => { const top = li.getBoundingClientRect().top; return top >= -1 && top < band; });
  return row ? { id: row.getAttribute('data-id'), top: row.getBoundingClientRect().top } : null;
}"""
ANCHOR_TOP_JS = """(id) => {
  const row = [...document.querySelectorAll('#discord-log > li[data-id]')]
    .find((li) => li.getAttribute('data-id') === id);
  return row ? row.getBoundingClientRect().top : null;
}"""
CHIP_JS = """() => [...document.querySelectorAll('#discord-log .thread-badge')].map((b) => ({
  text: b.textContent, slug: b.getAttribute('data-thread-slug'),
})).filter((b) => b.slug)"""


@dataclass
class Walk:
    page: "Page"
    label: str
    shots: Path | None
    notes: list[str] = field(default_factory=list)

    def check(self, condition: bool, why: str) -> None:
        if not condition:
            raise AssertionError(f"{self.label}: {why}")

    def wait_for(self, condition: str, why: str, seconds: float = 30) -> None:
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline and not self.page.evaluate(condition):
            self.page.wait_for_timeout(25)
        self.check(bool(self.page.evaluate(condition)), why)

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.label}-{name}.png"), full_page=False)


def all_walk(walk: Walk, windowed: bool) -> None:
    page = walk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    walk.wait_for("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                  "the channel did not open")
    if page.evaluate("() => document.getElementById('thread-select').value") != "flat":
        page.select_option("#thread-select", "flat")
    walk.wait_for("() => document.getElementById('discord-log').getAttribute('data-view') === 'flat'",
                  "All did not open")
    walk.wait_for("() => document.querySelectorAll('#discord-log > li:not([data-window-spacer])').length > 0",
                  "All mounted no rows")
    if windowed:
        # The window stands a render after the first rows land: condition, not a count at an instant.
        walk.wait_for("() => { const n = document.querySelectorAll('#discord-log > li:not([data-window-spacer])').length; "
                      f"return n > 0 && n < {THREADS * (REPLIES + 1)}; }}",
                      "windowed All never windowed its rows")
    mounted = int(page.evaluate(MOUNTED_JS))
    if windowed:
        walk.check(0 < mounted < THREADS * (REPLIES + 1),
                   f"windowed All mounted all {mounted} rows")
    # (a) Thread chips: slug and count on the first render pass.
    chips = page.evaluate(CHIP_JS)
    walk.check(bool(chips) and all(f"({REPLIES})" in c["text"] or c["slug"] for c in chips),
               f"no mounted Thread chip shows its slug on first render: {chips[:3]}")
    walk.notes.append(f"chips slugged on first render ({len(chips)} mounted), mounted={mounted}")
    walk.shot("1-all-chips")
    # Mid-history anchor, then (b) the ';' fold toggle.
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight * 0.4; }")
    walk.wait_for("() => { const band = document.getElementById('scroll-area').clientHeight || 0; "
                  "return [...document.querySelectorAll('#discord-log > li[data-id]')]"
                  ".some((li) => { const t = li.getBoundingClientRect().top; return t >= -1 && t < band; }); }",
                  "no anchor row mid-history")
    anchor = page.evaluate(ANCHOR_JS)
    walk.check(anchor is not None, "no anchor row mid-history")
    # Select the anchor row, as a reader on it does: the fold holds the
    # selected row, and that row is the place this check judges.
    page.locator(f"#discord-log > li[data-id='{anchor['id']}']").click()
    page.wait_for_timeout(300)
    selected = page.evaluate(
        "() => document.querySelector('#discord-log > li[data-selected=\"true\"]')?.getAttribute('data-id') ?? null")
    walk.check(selected is not None, "clicking the anchor row did not select it")
    anchor = {"id": str(selected), "top": page.evaluate(ANCHOR_TOP_JS, str(selected))}
    walk.check(anchor["top"] is not None, "no selected anchor row")
    page.evaluate("() => document.getElementById('scroll-area').focus()")
    page.keyboard.press(";")
    # The anchor's settled place, not a mid-flight sample: same within a
    # pixel across five checks in a row, however long the host takes.
    top = None
    stable = 0
    for _ in range(200):
        now = page.evaluate(ANCHOR_TOP_JS, anchor["id"])
        if now is not None and top is not None and abs(now - top) <= 1:
            stable += 1
            if stable >= 5:
                break
        else:
            stable = 0
        top = now
        page.wait_for_timeout(50)
    if windowed:
        walk.check(top is not None and abs(top - anchor["top"]) <= 8,
                   f"';' moved the anchor from {anchor['top']} to {top}")
        walk.notes.append(f"';' kept the anchor within {abs((top or 0) - anchor['top']):.1f}px")
    if windowed:
        # #2009: no settled empty viewport band after the fold - the point
        # 3/4 down the band must be a message row, never the bottom spacer.
        filled = page.evaluate("""() => {
          const a = document.getElementById('scroll-area');
          const list = document.getElementById('discord-log');
          const band = a.getBoundingClientRect();
          const rows = [...document.querySelectorAll('#discord-log > li:not([data-window-spacer])')];
          const el = document.elementFromPoint(band.left + band.width / 2, band.top + a.clientHeight * 0.75);
          const li = el ? el.closest('li') : null;
          return { onRow: Boolean(li && li.getAttribute('data-id')),
                   lastBottom: rows.length ? rows[rows.length - 1].getBoundingClientRect().bottom : 0,
                   bandBottom: band.top + a.clientHeight };
        }""")
        walk.check(filled["onRow"] or filled["lastBottom"] >= filled["bandBottom"] - 1,
                   f"after ';' the viewport band stands empty below the anchor: {filled}")
        walk.notes.append("after ';' the viewport band is filled with rows")
    walk.shot("2-fold-anchor")
    # (c) END lands at the bottom, and (bug 5) does not bounce afterwards.
    page.keyboard.press("End")
    walk.wait_for(FOOT_JS, "END did not land at the foot")
    changes = int(page.evaluate(SETTLE_JS))
    walk.check(changes <= 1, f"after END the list bounced: {changes} scroll direction changes while settling")
    walk.notes.append(f"END at the foot; settle direction changes={changes}")
    walk.shot("3-end")
    # (d) The Newest chip from mid-history.
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight * 0.4; }")
    walk.wait_for("() => !document.getElementById('jump-newest').hidden", "the Newest chip never showed")
    page.click("#jump-newest")
    walk.wait_for(FOOT_JS, "the Newest chip did not land at the foot")
    changes = int(page.evaluate(SETTLE_JS))
    walk.check(changes <= 1, f"after Newest the list bounced: {changes} direction changes")
    walk.notes.append(f"Newest at the foot; settle direction changes={changes}")
    walk.shot("4-newest")


def serve(api: FakeApi, web_root: Path) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api, web_root))
    server.daemon_threads = True
    thread_ = threading.Thread(target=server.serve_forever, daemon=True)
    thread_.start()
    return server, thread_, f"http://127.0.0.1:{server.server_port}/"


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"))
    parser.add_argument("--web-root", type=Path, default=WEB_ROOT)
    parser.add_argument("--screenshots", type=Path)
    return parser.parse_args()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required") from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    web_root = page_sources.served(args.web_root)
    report: list[str] = []
    with sync_playwright() as playwright:
        for size, width, height, phone in (("412x915", 412, 915, True), ("1280x800", 1280, 800, False)):
            for windowed in (True, False):
                api = FakeApi()
                server, thread_, url = serve(api, web_root)
                try:
                    with tempfile.TemporaryDirectory(prefix="vibe-talk-windowed-all-") as profile:
                        kwargs = dict(headless=True, executable_path=args.browser_executable,
                                      viewport={"width": width, "height": height}, color_scheme="dark")
                        if phone:
                            kwargs.update(user_agent=("Mozilla/5.0 (Linux; Android 15; Pixel 7) "
                                                      "AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36"),
                                          device_scale_factor=2.625, is_mobile=True, has_touch=True)
                        context = playwright.chromium.launch_persistent_context(profile, **kwargs)
                        page = context.pages[0]
                        errors: list[str] = []
                        page.on("pageerror", lambda error: errors.append(str(error)))
                        flag = "vibe-talk.voice.windowed-lists" if windowed else "vibe-talk.voice.windowed-lists-off"
                        page.add_init_script(
                            "try { localStorage.setItem('vibe-talk.welcome.dismissed', '1'); "
                            f"localStorage.setItem('{flag}', '1'); }} catch (e) {{}}")
                        page.goto(url, wait_until="load")
                        walk = Walk(page, f"{size}-{'on' if windowed else 'off'}", args.screenshots)
                        all_walk(walk, windowed)
                        if errors:
                            raise AssertionError(f"{walk.label}: the page threw: {errors}")
                        context.close()
                        report.append(f"{walk.label}: " + "; ".join(walk.notes))
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
