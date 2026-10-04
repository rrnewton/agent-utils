#!/usr/bin/env python3
"""Exercise /voice's saved-message snapshot in real Chromium at a phone's size.

The fake-DOM suite (tests/js/voice_page.test.mjs) covers every branch of the snapshot. This covers
what only a browser engine can: real localStorage, a real reload, real fetch failures, the Cache
Storage and service-worker registries, and layout at 412x915 and at a 1280x800 desk. For each of
those it serves the checked-out assets and a small fake API from one loopback origin, then walks
one reader through:

  sign in, open the channel — in All, the default where the provider has threads — and read Threads
  and Main too -> reload with the network held: the page reopens on the channel in All by itself,
  its saved rows drawn before any answer (`#189 restore-ui-state`) -> one bounded newest-page read
  of All merges a new row without duplicating -> switching views makes no request -> a reload with
  the API unreachable reopens on Main, the view last chosen, at the message a reader parked mid-list
  was on, keeps the rows and says so -> a reload whose refresh fails keeps them and says that ->
  signing out removes them from the screen and the device. Before the reloads, a refresh also fails
  and recovers in place, on the channel's own poll, under a reader at the top, in the middle and at
  the newest line.

While the freshness pill is up — refreshing, offline, failed — it must cover neither the view tabs
nor the list's header seam or first row, and #scroll-area must be the same box with it as without
it (`#32 freshness-pill-overlap`). As the pill's room and the error panel above the list come and
go, a scrolled reader's line stays where it is on the screen, and a reader at the newest line stays
there (`#33 error-banner-scroll-shift`).

On a touchscreen, a real finger parked at the newest line and drawn up past the end of the list
reads the channel once, says "Updated" at the foot of the list and does not reload the page; a drag
short of the threshold reads nothing (`#188 pull-refresh-bottom`).

Opening a thread whose title is longer than the screen, with a long unbroken token and inline code
in its reply, leaves the page no wider than the viewport and no shown part of the main screen past
its right edge; on a phone the title is ellipsised instead (`#36 thread-view-phone-overflow`). With
every floating chip showing at once and that thread scrolled to its end, no chip covers the reply
composer's text box or its Send button (`#194 thread-picker-polish`).

The search glass floats on the pill's line rather than costing a header row (`#197
floating-search`): on the main screen no header strip stands above the list, on the call view and
the channel alike; wherever the pill is measured the glass is centred on its line, drawn its height,
over the list and clear of it, and a 44px square around the disc presses it. A real tap below the
disc opens the search as a bar across the top of the list — no wider than the list, not hanging
over what is above it, the pill given way and the head of the list clear of it — the bar filters,
and the glass folds it back with #scroll-area unmoved. Settings still has its title bar and a way
back, and in a thread neither floating thing covers the heading's Back button or title.

Last, a fresh page signed in with a read-scope token reads the channel without a single 4xx answer
or console error: the stored-conversation routes are write-scope, answered 403 here as the server
answers them, and the page must not ask (`#38 read-token-conversation-probe`).

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
READ_TOKEN = "read-token-browser-cache-check"
SCOPES = {f"Bearer {TOKEN}": "write", f"Bearer {READ_TOKEN}": "read"}
CHANNEL = {"id": "1110000000000000001", "label": "lead team", "writable": True, "alias": None,
           "added": False}
CACHE_KEY = "vibe-talk.voice.message-cache"
EPOCH = datetime(2026, 1, 5, 9, 0, tzinfo=timezone.utc)
# `#36 thread-view-phone-overflow`: wider than a phone as words, and unbreakable at its tail.
LONG_TOKEN = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0-0f1e2d3c4b5a69788796a5b4c3d2e1f0"
LONG_TITLE = f"Overnight coordinator for the release lane #link {LONG_TOKEN}"
Json = dict[str, object]


def message(index: int, content: str, thread: Json | None = None) -> Json:
    row: Json = {
        "id": str(200 + index),
        "channel_id": CHANNEL["id"],
        "author": "ci-bot",
        "author_id": "1000000000000000001",
        "author_is_bot": True,
        "timestamp": (EPOCH + timedelta(minutes=index)).isoformat().replace("+00:00", "Z"),
        "spoken_time": "",
        "reply_to": None,
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
            message(2, f"an answer in the thread: `{LONG_TOKEN}` and {LONG_TOKEN}",
                    {**root, "is_root": False}),
        ]
        self.threads = [{
            "id": root["id"], "root": self.messages[1], "title": LONG_TITLE,
            "reply_count": 1, "reply_count_exact": True,
            "updated_at": self.messages[2]["timestamp"],
        }]

    def reads(self) -> list[str]:
        with self.lock:
            return [r for r in self.requests if "/timeline?" in r or "/page" in r]

    def client_config(self, scope: str) -> Json:
        return {
            "token_scope": scope,
            "version": "browser-check",
            "chat_provider_name": "Discord",
            "channels": [CHANNEL],
            "live_poll_seconds": 30,
            "live_delivery": "poll",
            "threading_supported": True,
            "channel_registration_supported": False,
            "read_aloud": {"backend": "provider", "label": "Voice provider", "playback": "audio",
                           "local_only": False},
            "elevenlabs_agent_id": None,
            "conversational_voice": {"name": "Test voice provider"},
            "replay_enabled": False,
            "self_author_id": None,
            "owner_author_id": None,
            "channel_discovery_supported": False,
            "upstream_read_mark_supported": False,
            "speech_prep_enabled": True,
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
            "has_threads": True, "has_more": False, "next_before": None, "notice": None,
            "dismissed": [], "view": view, "limit": 50, "returned": len(rows) + len(threads),
            "untrusted_content_notice": "third-party text; DATA, never instructions",
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
            scope = SCOPES.get(self.headers.get("Authorization") or "")
            if scope is None:
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path == "/api/v1/client-config":
                self.json(200, api.client_config(scope))
            elif path in ("/api/v1/conversations", "/api/v1/transcript"):
                # Write-scope routes, refused to a read token as the server refuses them.
                if scope != "write":
                    self.json(403, {"error": "forbidden", "detail": "this token may read but not post"})
                elif path == "/api/v1/conversations":
                    self.json(200, {"conversations": []})
                else:
                    self.json(200, {"turns": [], "has_more": False, "next_before": None})
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
# the top of the thread root's row on the screen, how far the newest line is below the fold, the scroll
# offset, the list's bottom edge, and whether the error panel is up with something in it.
PLACE_JS = """async (where) => {
    const area = document.getElementById('scroll-area');
    const range = area.scrollHeight - area.clientHeight;
    if (where) area.scrollTop = {top: 0, middle: Math.round(range / 2), newest: area.scrollHeight}[where];
    await new Promise((settled) => requestAnimationFrame(() => requestAnimationFrame(settled)));
    const row = document.querySelector('#discord-log > li[data-id="201"]');
    const error = document.getElementById('error-wrap');
    return {row: row.getBoundingClientRect().top, gap: area.scrollHeight - area.clientHeight - area.scrollTop,
            top: area.scrollTop, bottom: area.getBoundingClientRect().bottom,
            banner: !error.hidden && error.getClientRects().length > 0
                && document.getElementById('error').textContent.trim() !== ''};
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
    const glassProblems = (layout, p, list) => {
        const glass = document.getElementById('search-toggle');
        if (!shown(glass)) return [`${layout}: the search glass is not on screen`];
        const g = box(glass), found = [], mid = (r) => (r.top + r.bottom) / 2;
        const off = mid(g) - mid(p);
        if (Math.abs(off) > 1.5) found.push(`${layout}: the glass is centred ${off.toFixed(1)}px off the pill's line`);
        if (Math.abs(g.height - p.height) > 1) found.push(`${layout}: the glass is ${g.height}px beside a ${p.height}px pill`);
        if (meets(p, g)) found.push(`${layout}: the glass overlaps the pill "${pill.textContent}"`);
        if (g.top < list.top || g.left < list.left || g.right > list.right) found.push(`${layout}: the glass is not over the list`);
        // The 44px target: beside and below the disc, outside it but inside the square, is the glass.
        const cx = (g.left + g.right) / 2, cy = mid(g);
        for (const [dx, dy] of [[-20, 0], [20, 0], [0, 20]]) {
            const hit = document.elementFromPoint(cx + dx, cy + dy);
            if (!hit || !glass.contains(hit)) {
                const what = hit ? (hit.id ? `#${hit.id}` : hit.tagName.toLowerCase()) : 'nothing';
                found.push(`${layout}: a tap at (${dx}, ${dy})px from the glass's centre lands on ${what}`);
            }
        }
        return found;
    };
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
        // ...and to the fraction of a pixel, which the check above rounds away: a flex line that
        // took a quarter of a pixel off the pill ellipsised the whole of its time (`#197
        // floating-search`) — an ellipsis removes whole glyphs, however small the shortfall. Its
        // width with nothing constraining it is the width it needs; 1/64px is layout's own unit.
        const saved = pill.getAttribute('style');
        pill.style.cssText = 'flex: none; max-width: none; width: max-content';
        const natural = box(pill).width;
        if (saved === null) pill.removeAttribute('style'); else pill.setAttribute('style', saved);
        if (natural > p.width + 1 / 64) {
            problems.push(`${layout}: it needs ${natural.toFixed(2)}px and has ${p.width.toFixed(2)}: ${pill.textContent}`);
        }
        if (p.top < list.top || p.left < list.left || p.right > list.right) {
            problems.push(`${layout}: it is not over the list`);
        }
        // `#197 floating-search`: the glass on the pill's line, the pill's height, clear of it.
        // Measured in every pill state, because the longest sentences are the ones that reach it.
        glassProblems(layout, p, list).forEach((problem) => problems.push(problem));
    };
    // An empty header strip over the list is the defect `#197 floating-search` removed.
    const header = document.getElementById('topbar');
    if (shown(header)) {
        problems.push(`a ${Math.round(box(header).height)}px header strip stands over the main screen`);
    }
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
    // `gchat-thread-selector`: the tabs left the list for the bar's thread picker.
    const picker = document.getElementById('thread-select');
    if (shown(tabs)) problems.push('the Main/Threads/All tabs are back over the list');
    return {problems, area: measured, bare, picker: shown(picker)};
}"""

# `#36 thread-view-phone-overflow`. `problems` is empty when the page is no wider than the viewport
# and no shown element of the main screen reaches past its right edge; `clipped` says the thread
# title is ellipsised, which is what a title longer than the screen should do instead.
WIDTH_JS = """() => {
    const viewport = document.documentElement.clientWidth;
    const problems = [];
    const wide = document.documentElement.scrollWidth;
    if (wide > viewport) problems.push(`the page is ${wide}px wide in a ${viewport}px viewport`);
    for (const element of document.querySelectorAll('#screen-main, #screen-main *')) {
        const box = element.getBoundingClientRect();
        if (box.width === 0 || element.closest('[hidden]')) continue;
        if (box.right > viewport + 1) {
            const name = element.id ? `#${element.id}` : `${element.tagName.toLowerCase()}.${element.className}`;
            problems.push(`${name} reaches ${Math.round(box.right)}px`);
        }
    }
    const title = document.getElementById('thread-title');
    return {problems: problems.slice(0, 8), clipped: title.scrollWidth > title.clientWidth};
}"""

# `#194 thread-picker-polish`. The owner photographed Summaries, Undo, Expand all and Collapse all
# sitting on a thread's "Reply in this thread" box. Run with every chip forced up — the worst case —
# this scrolls the thread to its end and returns what covers the composer: empty when no chip meets
# the text box or Send, and both sit inside the list rather than below its foot.
COMPOSER_JS = """async () => {
    const area = document.getElementById('scroll-area');
    const frames = () => new Promise((settled) => requestAnimationFrame(() => requestAnimationFrame(settled)));
    await frames();
    area.scrollTop = area.scrollHeight;
    await frames();
    const box = (e) => e.getBoundingClientRect();
    const meets = (a, b) => a.top < b.bottom && b.top < a.bottom && a.left < b.right && b.left < a.right;
    const chips = [...document.querySelectorAll('#scroll-tools .chip')].filter((c) => c.getClientRects().length > 0);
    const problems = [];
    if (chips.length < 4) problems.push(`only ${chips.length} chips are showing`);
    if (area.scrollHeight - area.clientHeight - area.scrollTop > 2) problems.push('the thread is not at its end');
    for (const id of ['channel-compose-text', 'channel-send']) {
        const target = box(document.getElementById(id));
        chips.filter((chip) => meets(box(chip), target))
            .forEach((chip) => problems.push(`"${chip.textContent.trim()}" covers #${id}`));
        if (target.bottom > box(area).bottom + 1) problems.push(`#${id} ends below the list`);
    }
    return problems;
}"""

# `#197 floating-search`. Shared by the checks below: is an element really laid out, its box, and
# whether two boxes meet.
FLOAT_HELPERS_JS = """
    const shown = (e) => Boolean(e) && !e.hidden && e.getClientRects().length > 0;
    const box = (e) => e.getBoundingClientRect();
    const meets = (a, b) => a.top < b.bottom && b.top < a.bottom && a.left < b.right && b.left < a.right;
    const name = (e) => e ? (e.id ? `#${e.id}` : e.tagName.toLowerCase()) : 'nothing';
"""

# The call view has no pill, so the glass is measured against the line it shares with one: centred
# on the zero-height anchor (whose top IS the line), over the list's top-right corner, and no header
# strip standing above it. `problems` is empty when all of that holds.
GLASS_ALONE_JS = """() => {""" + FLOAT_HELPERS_JS + """
    const problems = [];
    const header = document.getElementById('topbar');
    if (shown(header)) problems.push(`a ${Math.round(box(header).height)}px header strip stands over the call view`);
    const glass = document.getElementById('search-toggle');
    if (!shown(glass)) return {problems: [...problems, 'the search glass is not on the call view']};
    const g = box(glass), list = box(document.getElementById('scroll-area'));
    const line = box(document.getElementById('channel-freshness-anchor'));
    const off = (g.top + g.bottom) / 2 - line.top;
    if (Math.abs(off) > 1) problems.push(`the glass is centred ${off.toFixed(1)}px off the pill's line`);
    if (g.top < list.top || g.right > list.right) problems.push('the glass is not over the list');
    // The line is the list's width on a phone and the reading column's on a desk.
    if (line.right - g.right > 24) problems.push(`the glass is ${Math.round(line.right - g.right)}px from the corner`);
    return {problems};
}"""

# The search open over the channel: a bar across the top of the list, no wider than it and not
# hanging over whatever is above it, with the field focused and wide enough to read, the pill given
# way, and the first thing the list shows clear of the bar. `query`, when given, has just been typed;
# `rows` is what is left on screen and `count` what the bar says about it.
SEARCH_OPEN_JS = """(query) => {""" + FLOAT_HELPERS_JS + """
    const area = document.getElementById('scroll-area');
    area.scrollTop = 0;
    const problems = [];
    const bar = document.getElementById('search-float'), field = document.getElementById('search-field');
    const count = document.getElementById('search-count'), glass = document.getElementById('search-toggle');
    if (glass.getAttribute('aria-pressed') !== 'true') problems.push('the glass does not read as open');
    if (!shown(field)) return {problems: [...problems, 'the field is not open'], rows: [], count: ''};
    const list = box(area), b = box(bar), f = box(field), g = box(glass);
    if (b.top < list.top - 0.5) problems.push(`the bar hangs ${(list.top - b.top).toFixed(1)}px over what is above the list`);
    if (b.left < list.left - 0.5 || b.right > list.right + 0.5) problems.push('the bar is wider than the list');
    if (f.width < 150) problems.push(`the field is ${Math.round(f.width)}px wide`);
    if (f.left < b.left || g.right > b.right + 0.5 || g.top < b.top - 0.5 || g.bottom > b.bottom + 0.5) {
        problems.push('the glass or the field is outside the bar');
    }
    if (query && (!shown(count) || meets(box(count), f) || meets(box(count), g))) problems.push('the count is not on the bar beside the field');
    if (shown(document.getElementById('channel-freshness'))) problems.push('the pill is drawn under the open bar');
    if (document.activeElement !== field) problems.push(`the field did not take focus: ${name(document.activeElement)}`);
    const first = [...document.querySelectorAll('#pane-discord .seam, #discord-log > li')].find(shown);
    if (!first) problems.push('there is nothing at the head of the list to measure against');
    else if (box(first).top < b.bottom) problems.push(`the open bar covers ${(b.bottom - box(first).top).toFixed(1)}px of the head of the list`);
    const rows = [...document.querySelectorAll('#discord-log > li[data-id]')].filter(shown).map((li) => li.getAttribute('data-id'));
    return {problems, rows, count: count.textContent};
}"""

# In a thread the heading row is above the list; neither floating thing may cover its Back button
# or title, nor take the Back button's tap.
HEADING_JS = """() => {""" + FLOAT_HELPERS_JS + """
    const problems = [];
    const glass = document.getElementById('search-toggle'), pill = document.getElementById('channel-freshness');
    const back = document.getElementById('thread-back');
    for (const id of ['thread-back', 'thread-title']) {
        const target = box(document.getElementById(id));
        if (meets(box(glass), target)) problems.push(`the glass covers #${id}`);
        if (shown(pill) && meets(box(pill), target)) problems.push(`the pill covers #${id}`);
    }
    if (box(glass).top < box(document.getElementById('channel-navigation')).bottom) {
        problems.push('the glass is drawn over the thread heading row');
    }
    const b = box(back), hit = document.elementFromPoint((b.left + b.right) / 2, (b.top + b.bottom) / 2);
    if (!back.contains(hit)) problems.push(`a tap on Back lands on ${name(hit)}`);
    return problems;
}"""

# Settings is a destination: the header comes back as a title bar with a way back.
TITLE_BAR_JS = """() => {""" + FLOAT_HELPERS_JS + """
    const header = document.getElementById('topbar');
    return {shown: shown(header), height: box(header).height,
            title: shown(document.getElementById('topbar-title')) ? document.getElementById('topbar-title').textContent : '',
            back: shown(document.getElementById('close-settings')),
            glass: shown(document.getElementById('search-toggle'))};
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
            print(f"{browser_version} {label} at {width}x{height}: All by default, a reload reopened on the"
                  " channel, its view and the reader's message, snapshot drawn before the network,"
                  " one newest-page read of the"
                  " view on screen, local view switches, offline and failed states, the pill"
                  " clear of the tabs and the header with #scroll-area unmoved, a scrolled reader"
                  " held in place as it and the error panel come and go, the search glass on the pill's"
                  " line with no header strip and a 44px target, the search bar opened by a real tap and"
                  " folded with #scroll-area unmoved, Settings' title bar,"
                  f"{' a pull up past the newest line refreshes in place,' if mobile else ''} no Cache Storage or"
                  " service worker, a long thread title ellipsised within the viewport, the thread"
                  " composer clear of every floating chip, sign-out clears,"
                  " a read-scope token reads with no refused request or console error")
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
            # Everything the browser itself calls an error, and every refused answer: a 4xx logs a
            # console error however the page handles it, so the only fix is not to ask.
            console_errors: list[str] = []
            page.on("console", lambda entry: console_errors.append(entry.text) if entry.type == "error" else None)
            refused: list[str] = []
            page.on("response", lambda answer: refused.append(
                f"{answer.status} {answer.request.method} {urlsplit(answer.url).path}")
                if answer.status >= 400 else None)

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

            def current(text: str) -> bool:
                # A current list says so with the time of its last read; blank means unknown.
                return re.fullmatch(r"(Live · u|U)pdated \d{2}:\d{2}", text) is not None

            def view(name: str) -> None:
                page.evaluate(f"() => document.getElementById('channel-view-{name}').click()")

            def reopened(why: str) -> None:
                """`#189 restore-ui-state`: a reload of a page left on the channel comes back on it."""
                shown = page.evaluate("() => !document.getElementById('pane-discord').hidden")
                check(bool(shown), f"{label}: {why}: the reload did not reopen on the channel")

            def failing(route: Route) -> None:
                route.fulfill(status=502, content_type="application/json",
                              body=json.dumps({"error": "discord_error", "detail": "the provider is down"}))

            def timeline_read(address: str) -> bool:
                return "/timeline" in address

            def wait_pill(prefix: str, why: str) -> None:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not (pill().startswith(prefix) if prefix else current(pill())):
                    page.wait_for_timeout(50)
                check(pill().startswith(prefix) if prefix else current(pill()), f"{label}, {why}: pill {pill()!r}")

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
                return {key: float(found[key]) for key in ("row", "gap", "top", "bottom", "banner")}

            def geometry(state: str, name: str) -> str:
                # At the head of the list, where the header is: a pill over row 40 of 90 is the
                # accepted cost of an overlay, a pill over the header is `#32 freshness-pill-overlap`.
                found = page.evaluate(GEOMETRY_JS)
                shot(name)
                problems = [str(problem) for problem in found["problems"]]
                check(not problems, f"{label}, {state}: {problems}")
                check(str(found["area"]) == str(found["bare"]),
                      f"{label}, {state}: #scroll-area is {found['area']}, {found['bare']} without the pill")
                check(bool(found["picker"]), f"{label}, {state}: the bar's thread picker was not on screen")
                return str(found["area"])

            # 1. Sign in and open the channel: in All, the default where the provider has threads
            # (`#189 restore-ui-state`); then Threads and Main. The call view comes up first: its
            # glass floats over the transcript with no header strip above it (`#197`).
            page.goto(url, wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            alone = page.evaluate(GLASS_ALONE_JS)
            shot("1-call-view-glass")
            check(not alone["problems"], f"{label}, the call view: {alone['problems']}")
            page.click("#view-switch")
            wait_rows(["200", "201", "202"], "All after sign-in")
            check(page.evaluate("() => document.getElementById('thread-select').value") == "flat",
                  f"{label}: a channel with threads did not open in All")
            view("threads")
            page.wait_for_selector("#thread-list > li", timeout=10_000)
            view("main")
            wait_rows(["200", "201"], "Main after sign-in")
            view("flat")
            wait_rows(["200", "201", "202"], "All again")
            saved = json.loads(page.evaluate(f"() => localStorage.getItem({json.dumps(CACHE_KEY)})") or "{}")
            scope = saved.get("scopes", {}).get(CHANNEL["id"], {})
            check(sorted(scope.get("views", {})) == ["flat", "main", "threads"],
                  f"the snapshot did not cover each view read: {sorted(scope.get('views', {}))}")
            shot("1-signed-in")
            baseline = str(page.evaluate(GEOMETRY_JS)["area"])

            # 2. Reload with the history read held: the page reopens on the channel, in All, and the
            # saved rows draw before any answer.
            api.timeline_gate.clear()
            with api.lock:
                api.requests.clear()
                api.messages.append(message(3, "posted while the page was closed"))
            page.reload(wait_until="load")
            reopened("the reload with the read held")
            check(page.evaluate("() => document.getElementById('thread-select').value") == "flat",
                  f"{label}: the reload did not reopen in All")
            wait_rows(["200", "201", "202"], "the snapshot was not drawn before the network answered")
            text = pill()
            check(text.startswith("Saved ") and "refreshing" in text, f"pill while refreshing: {text!r}")
            check(geometry("refreshing", "2-saved-before-network") == baseline,
                  f"{label}: #scroll-area is not the size it was before the reload")

            # 3. One bounded newest page for the view on screen — All — merges without duplicating.
            api.timeline_gate.set()
            wait_rows(["200", "201", "202", "203"], "the refresh did not merge")
            reads = api.reads()
            check(len(reads) == 1, f"a cold start made {len(reads)} history reads: {reads}")
            check("view=flat" in reads[0] and "before=" not in reads[0],
                  f"the cold-start read was not the newest page of All: {reads[0]}")
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline and not current(pill()):
                page.wait_for_timeout(50)
            check(current(pill()), f"a refreshed view still says {pill()!r}")
            check(str(page.evaluate(GEOMETRY_JS)["area"]) == baseline,
                  f"{label}: #scroll-area changed size when the pill turned current")
            shot("3-merged")

            # 4. Switching among views already covered is local. It ends on Main, the reader's
            # choice, which the reloads below reopen on.
            view("threads")
            page.wait_for_selector("#thread-list > li", timeout=5_000)
            view("main")
            wait_rows(["200", "201", "203"], "Main after the refresh")
            view("flat")
            wait_rows(["200", "201", "202", "203"], "All after switching back")
            view("main")
            wait_rows(["200", "201", "203"], "Main again")
            check(len(api.reads()) == 1, f"switching views read history again: {api.reads()}")
            shot("4-switched-locally")

            # 4a. `#197 floating-search`. Settings is still a title bar with a way back, and leaving
            # it leaves no strip behind. A real tap BELOW the disc — outside it, inside its 44px
            # square — opens the search as a bar over the top of the list; the bar filters; the
            # glass folds it again and the list is the box it was.
            page.click("#open-settings")
            title_bar = page.evaluate(TITLE_BAR_JS)
            shot("4a-settings-title-bar")
            check(bool(title_bar["shown"]) and float(title_bar["height"]) > 30
                  and title_bar["title"] == "Settings" and bool(title_bar["back"])
                  and not title_bar["glass"], f"{label}, Settings lost its title bar: {title_bar}")
            page.click("#close-settings")
            check(not page.evaluate(TITLE_BAR_JS)["shown"],
                  f"{label}: coming back from Settings left a header strip over the channel")
            centre = page.evaluate("() => { const b = document.getElementById('search-toggle')"
                                   ".getBoundingClientRect(); return [b.left + b.width / 2, b.bottom + 5]; }")
            if mobile:
                page.touchscreen.tap(float(centre[0]), float(centre[1]))
            else:
                page.mouse.click(float(centre[0]), float(centre[1]))
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline and page.evaluate("() => document.getElementById('search-field').hidden"):
                page.wait_for_timeout(50)
            opened = page.evaluate(SEARCH_OPEN_JS, "")
            shot("4a-search-open")
            check(not opened["problems"], f"{label}, a tap 5px below the glass's disc: {opened['problems']}")
            page.fill("#search-field", "thread root")
            found = page.evaluate(SEARCH_OPEN_JS, "thread root")
            shot("4a-search-filtered")
            check(not found["problems"], f"{label}, the search filtered: {found['problems']}")
            check(found["rows"] == ["201"] and found["count"] == "1 of 3 loaded",
                  f"{label}: the floating search left {found['rows']} saying {found['count']!r}")
            page.click("#search-toggle")
            check(bool(page.evaluate("() => document.getElementById('search-field').hidden")),
                  f"{label}: the glass did not fold the search bar")
            wait_rows(["200", "201", "203"], "folding the search did not put every row back")
            check(geometry("search folded", "4a-search-folded") == baseline,
                  f"{label}: #scroll-area changed size across opening and folding the search")

            # 4b. A failed refresh IN PLACE, over a list long enough to scroll: the pill's room is made
            # at the head of the list and the error panel takes a row above it, and both go again
            # when the next read succeeds. Browser scroll anchoring holds for neither, so the page
            # must: the row being read stays where it is on the screen, and a reader at the newest
            # line stays there (`#32 freshness-pill-overlap`, `#33 error-banner-scroll-shift`). At the
            # very top the header moves down out from under them instead. Tall rows, so that three
            # of them overflow.
            page.add_style_tag(content="#discord-log > li[data-id] { min-height: 70vh; }")
            for where in ("middle", "newest", "top"):
                before = place(where)
                parked = {"middle": 0 < before["top"] and before["gap"] > 100,
                          "newest": before["gap"] <= 2, "top": before["top"] == 0}[where]
                check(parked, f"{label}: could not park the reader at the {where}: {before}")
                # A failed POLL is carried by the pill alone since `#195 send-resilience`: nobody asked
                # for that read, so the error panel stays down and only the pill's room is made at the
                # head of the list. Twice, so a second failure after a recovery is held the same way.
                for step in ("fail", "recover", "fail", "recover"):
                    poll_channel(step == "fail")
                    after = place("")
                    state = f"{label}, reader at the {where}: {step}"
                    check(after["banner"] == 0.0, f"{state} but a background refresh raised the error panel: {after}")
                    check(abs(after["bottom"] - before["bottom"]) < 1, f"{state} and moved the list's foot: {after}")
                    if where == "middle":
                        moved = after["row"] - before["row"]
                        # Under 2px a step: a refresh makes several position fixes (its loading line
                        # arriving, the pill's room, the panel, the loading line going), each landing
                        # on a whole pixel. The defects were the pill's height, ~32px, and the
                        # panel's, 74-89px.
                        check(abs(moved) < 2, f"{state} and moved the row being read by {moved:.1f}px: {after}")
                    elif where == "newest":
                        check(after["gap"] <= 2, f"{state} and left the newest line {after['gap']:.1f}px below")
                    else:
                        check(after["top"] == 0, f"{state} and scrolled the header away: {after}")
                        if step == "fail":
                            geometry("in place at the top", "4b-top")
                    before = after
            shot("4b-in-place")

            # 4c. `#188 pull-refresh-bottom`, with a real finger: parked at the newest line, a drag UP
            # past the end of the list reads the channel once and says so at the foot of the list,
            # in this same page — no reload — and a drag short of the threshold reads nothing. The
            # touches go through Chromium's own input pipeline, so this is also the check that the
            # passive listeners still hear the overscroll at the end of a list that cannot scroll.
            if mobile:
                cdp = context.new_cdp_session(page)

                def drag_up(travel: float) -> None:
                    """One finger, landed on a message low in the list and drawn up, then lifted."""
                    box = page.evaluate("() => { const r = document.getElementById('scroll-area')"
                                        ".getBoundingClientRect(); return [r.left + r.width / 2,"
                                        " r.top + r.height * 0.6]; }")
                    x, y = float(box[0]), float(box[1])
                    steps = 12
                    cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": x, "y": y}]})
                    for step in range(1, steps + 1):
                        page.wait_for_timeout(16)
                        cdp.send("Input.dispatchTouchEvent", {
                            "type": "touchMove", "touchPoints": [{"x": x, "y": y - travel * step / steps}]})
                    page.wait_for_timeout(16)
                    cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})

                def affordance() -> tuple[str, str, str]:
                    found = page.evaluate("() => { const p = document.getElementById('pull-refresh');"
                                          " return [p.hidden ? '' : p.textContent,"
                                          " p.getAttribute('data-state'), p.getAttribute('data-edge')]; }")
                    return str(found[0]), str(found[1]), str(found[2])

                resting = place("newest")
                check(resting["gap"] <= 2, f"{label}: could not park the reader at the newest line: {resting}")
                page.evaluate("() => { window.pullRefreshPageMark = true; }")
                # The channel's own poll — or any other timer that re-reads it, such as the live
                # stream re-attaching — would be indistinguishable from the pull's read, so the page
                # clock stands still while the finger is down; the fetch itself needs no timer. Then
                # a moment for anything those timers already put on the wire to land before counting.
                page.clock.pause_at(int(page.evaluate("() => Date.now()")) + 1000)
                page.wait_for_timeout(500)
                with api.lock:
                    seen = len(api.requests)
                reads_before = len(api.reads())
                drag_up(30)
                page.wait_for_timeout(400)
                with api.lock:
                    during = api.requests[seen:]
                check(len(api.reads()) == reads_before,
                      f"{label}: a 30px drag at the newest line read the channel; requests since: {during}")
                check(affordance()[0] == "", f"{label}: a short drag left the affordance up: {affordance()}")
                drag_up(160)
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not affordance()[0].startswith("Updated"):
                    page.wait_for_timeout(50)
                shown = affordance()
                shot("4c-pulled-up")
                fresh = api.reads()[reads_before:]
                check(len(fresh) == 1 and "before=" not in fresh[0],
                      f"{label}: a pull up past the newest line made {len(fresh)} reads: {fresh}")
                check(re.fullmatch(r"Updated \d{2}:\d{2} · nothing new", shown[0]) is not None
                      and shown[2] == "end", f"{label}: the foot of the list said {shown}")
                check(bool(page.evaluate("() => window.pullRefreshPageMark === true")),
                      f"{label}: the pull up reloaded the page instead of re-reading the channel")
                page.clock.resume()
                # After the resume: `place` waits on animation frames, which the paused clock holds.
                check(place("")["gap"] <= 2, f"{label}: the refresh moved the reader off the newest line")
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and affordance()[0]:
                    page.wait_for_timeout(50)
                check(affordance()[0] == "", f"{label}: the pull's result never went: {affordance()}")
                cdp.detach()

            # 4d. `#189 restore-ui-state`. A reader parked mid-list, whose app goes away, comes back
            # on the same message. The tall rows are served in the stylesheet from here on, so every
            # later load lays them out before the page's script runs, as a phone does; the reload
            # is step 5's, with the API unreachable, so only the device can put the reader back.
            def tall_rows(route: Route) -> None:
                served = route.fetch()
                route.fulfill(response=served,
                              body=served.text() + "\n#discord-log > li[data-id] { min-height: 70vh; }\n")

            page.route("**/voice.css", tall_rows)
            reading = place("middle")
            check(reading["top"] > 0 and reading["gap"] > 100, f"{label}: could not park the reader mid-list: {reading}")

            # 5. The API unreachable: the rows stay, and the pill says they are old.
            page.route("**/api/**", lambda route: route.abort("internetdisconnected"))
            page.reload(wait_until="load")
            reopened("offline")
            wait_rows(["200", "201", "203"], "the snapshot was not kept while offline")
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and not pill().startswith("Offline"):
                page.wait_for_timeout(50)
            check(pill().startswith("Offline · showing messages saved "), f"offline pill: {pill()!r}")
            back = place("")
            check(abs(back["row"] - reading["row"]) < 2,
                  f"{label}: the reopen moved the reader's message by {back['row'] - reading['row']:.1f}px: {back}")
            geometry("offline", "5-offline")
            page.unroute("**/api/**")

            # 5b. The API reachable but failing: the rows stay, and the pill says the refresh failed.
            page.route(timeline_read, failing)
            page.reload(wait_until="load")
            reopened("a failing refresh")
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

            # 6b. A thread named longer than the screen: its title is ellipsised, not the page widened.
            view("threads")
            page.click("#thread-list > li button.thread-open")
            page.wait_for_selector("#thread-heading:not([hidden])", timeout=5_000)
            wait_rows(["201", "202"], "the long-titled thread did not open")
            found = page.evaluate(WIDTH_JS)
            shot("6b-long-thread-title")
            check(not found["problems"], f"{label}, a long-titled thread: {found['problems']}")
            check(bool(found["clipped"]) or not mobile, f"{label}: the long title was not ellipsised: {found}")
            heading = page.evaluate(HEADING_JS)
            check(not heading, f"{label}, a thread's heading and the floating line: {heading}")

            # 6c. Every floating chip up at once, over that thread scrolled to its end: none of them
            # covers the reply composer, which keeps their measured height as room under itself
            # (`#194 thread-picker-polish`). The rows are still 70vh tall from 4b, so the composer
            # really is at the foot of the list, where the chips float.
            forced = page.add_style_tag(content="#scroll-tools .chip[hidden] { display: inline-flex !important; }")
            covered = [str(problem) for problem in page.evaluate(COMPOSER_JS)]
            shot("6c-composer-clear-of-chips")
            check(not covered, f"{label}, the thread composer under the floating chips: {covered}")
            forced.evaluate("element => element.remove()")
            page.click("#thread-back")

            # 7. Signing out takes the rows off the screen and off the device.
            page.evaluate("() => document.getElementById('forget-token').click()")
            check(page.evaluate(f"() => localStorage.getItem({json.dumps(CACHE_KEY)})") is None,
                  "signing out left the snapshot on the device")
            check(rows() == [], f"signing out left rows on the screen: {rows()}")
            shot("7-signed-out")

            # 8. A fresh page and a READ-scope token: it reads the channel and asks for nothing it
            # cannot have. Reloaded first, so the stored record has not been looked for yet. Signing
            # out forgot where the reader was, so this opens on the call view, and the channel in All.
            page.reload(wait_until="load")
            console_errors.clear()
            refused.clear()
            page.fill("#api-token", READ_TOKEN)
            page.click("#save-token")
            page.click("#view-switch")
            wait_rows(["200", "201", "202", "203"], "a read-scope token did not read the channel")
            def storage_state() -> str:
                return str(page.evaluate("() => document.getElementById('storage-state').textContent"))

            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and not refused and "write-scope token" not in storage_state():
                page.wait_for_timeout(50)
            page.wait_for_timeout(300)
            shot("8-read-scope")
            check(not refused, f"{label}, read-scope token: the page was refused {refused}")
            check(not console_errors, f"{label}, read-scope token: console errors {console_errors}")
            check("write-scope token" in storage_state(),
                  f"{label}, read-scope token: Settings says {storage_state()!r} about the stored record")
            check(bool(page.evaluate("() => document.getElementById('error-wrap').hidden")),
                  f"{label}, read-scope token: the error panel is up")

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
