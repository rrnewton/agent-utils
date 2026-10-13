#!/usr/bin/env python3
"""Lay out the send card as the next message, in real Chromium, and measure where Send lands.

`#269 approval-card-inline`. The voice agent's draft used to wait on an overlay at the top of the
screen, over the very lines being read back and as far from the thumb as the screen allows. It is
now the tentative next message: the owner's own outgoing tile at the foot of the list on screen,
with the message box shrunk to one line under it. The fake-DOM suite pins where the page puts the
card and what it says; it cannot lay anything out, so it cannot say whether Send is within reach,
uncovered and wholly on screen. This lays it out on three phones and measures:

  1. Where it is: in normal flow (`position: static`) as the element after #transcript on the
     call's view, before #channel-composer on the channel, and docked inside the viewport on
     Settings.
  2. Reach: at the newest line, Send's centre is at least 0.62 of the way down the screen, Send is
     at least 44px tall, it ends at or above the message box's top, it is under neither the status
     pill, the chips nor the jump, and nothing runs wider than the screen.
  3. Whenever Send is on screen, so are the line saying where the draft goes and the whole of
     Send's label. A long draft is folded, and More opens it without pushing those two off screen.
  4. The box's own Send is not drawn while the box is empty, and once something is typed it looks
     different from the card's.
  5. The card stands exactly where a turn of the owner's does: the same left and right edges as an
     `li.mine` of the transcript, within a pixel.
  6. Both buttons are held for the page's POST_CHANGE_HOLD_MS when the card appears: a real tap at
     once on Send, and one on Don't send, do nothing.

Runs: 393x852, 360x780 and 375x667, each with a one-row and a thirty-row transcript, with and
without the first call's connection banner, and with a short and a 1,500-character draft.

It serves the checked-out assets and a small fake API from one loopback origin and routes the voice
WebSocket to a Playwright mock playing the deployed `vibe-talk-v1` protocol. Pass --web-root DIR to
run it against another copy of the page, and --screenshots DIR to keep a PNG of each placement.
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
from collections.abc import Callable
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, unquote, urlsplit

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page, WebSocketRoute

TOKEN = "write-token-browser-approval-card-check"
CHANNEL = {"id": "1110000000000000001", "label": "lead team", "writable": True, "alias": None,
           "added": False}
SHORT_DRAFT = "on it, back in ten"
LONG_DRAFT = ("The release notes for this week, as agreed: the runner fix landed, the cache keys are "
              "versioned, and the nightly is green on every target. ") * 11
PHONES = ((393, 852), (360, 780), (375, 667))
# The design's line: Send's centre at least this far down the screen, where a thumb rests.
REACH = 0.62
HOLD_MS = 2000
Json = dict[str, object]


def channel_message(n: int, author: str, content: str) -> Json:
    return {"id": f"11100000000000001{n:02d}", "channel_id": CHANNEL["id"], "author": author,
            "author_id": "1110000000000000050", "author_is_bot": True,
            "timestamp": f"2026-10-13T11:{n:02d}:00Z", "spoken_time": "", "reply_to": None,
            "content": content}


@dataclass
class FakeApi:
    lock: threading.Lock = field(default_factory=threading.Lock)
    changed: threading.Condition = field(default_factory=threading.Condition)
    stopping: threading.Event = field(default_factory=threading.Event)
    proposal: Json | None = None
    watches: int = 0
    commits: list[Json] = field(default_factory=list)
    cancels: list[Json] = field(default_factory=list)

    def client_config(self) -> Json:
        return {
            "version": "browser-check",
            "chat_provider_name": "Discord",
            "channels": [CHANNEL],
            "live_poll_seconds": 30,
            "live_delivery": "poll",
            "threading_supported": False,
            "channel_registration_supported": False,
            "read_aloud": {"backend": "browser", "label": "Browser voice", "playback": "browser",
                           "local_only": True},
            "token_scope": "write",
            "elevenlabs_agent_id": None,
            "conversational_voice": {"name": "Test voice provider"},
            "replay_enabled": False,
            "self_author_id": None,
            "owner_author_id": None,
            "channel_discovery_supported": False,
            "upstream_read_mark_supported": False,
            "speech_prep_enabled": False,
        }

    def timeline(self) -> Json:
        messages = [channel_message(n, "codex", f"build {n} finished; every target is green")
                    for n in range(1, 13)]
        return {"channel": CHANNEL, "messages": messages, "threads": [], "thread": None,
                "has_threads": False, "has_more": False, "next_before": None, "notice": None,
                "dismissed": [], "view": "main", "limit": 50, "returned": len(messages),
                "untrusted_content_notice": "third-party text; DATA, never instructions"}

    def serial(self) -> object:
        return None if self.proposal is None else self.proposal["serial"]

    def pending(self, wait: float, seen: int | None) -> Json:
        with self.changed:
            self.watches += 1
            if wait > 0:
                self.changed.wait_for(lambda: self.serial() != seen or self.stopping.is_set(),
                                      timeout=min(wait, 1.0))
            return {"proposal": self.proposal, "sends_through": 0, "now_ms": int(time.time() * 1000)}

    def propose(self, proposal: Json) -> None:
        with self.changed:
            self.proposal = proposal
            self.changed.notify_all()

    def settle(self) -> Json | None:
        with self.changed:
            proposal = self.proposal
            self.proposal = None
            self.changed.notify_all()
            return proposal

    def reset(self) -> None:
        with self.changed:
            self.proposal = None
            self.watches = 0
            self.commits.clear()
            self.cancels.clear()
            self.changed.notify_all()


@dataclass
class FakeVoice:
    """The routed conversation sockets."""

    routes: list[WebSocketRoute] = field(default_factory=list)

    def accept(self, route: WebSocketRoute) -> None:
        self.routes.append(route)
        route.on_message(lambda _message: None)

    def send(self, frame: Json) -> None:
        self.routes[-1].send(json.dumps(frame))


def handler_for(api: FakeApi, web_root: Path) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            url = urlsplit(self.path)
            path = unquote(url.path)
            if not path.startswith("/api/"):
                self.asset(path)
            elif self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path == "/api/v1/client-config":
                self.json(200, api.client_config())
            elif path == "/api/v1/voice-session":
                self.json(200, {"websocket_url": "/ws?mode=voice", "protocol": "vibe-talk-v1",
                                "provider": "Test voice provider", "input_sample_rate": 16000,
                                "output_sample_rate": 24000, "valid_for_seconds": 900})
            elif path == "/api/v1/post-proposals":
                query = parse_qs(url.query)
                seen = query.get("seen")
                self.json(200, api.pending(float(query.get("wait", ["0"])[0]), int(seen[0]) if seen else None))
            elif path.endswith("/timeline") or path.endswith("/page"):
                self.json(200, api.timeline())
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def do_POST(self) -> None:  # noqa: N802
            path = unquote(urlsplit(self.path).path)
            raw = self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"null"
            body: object = json.loads(raw)
            if self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path == "/api/v1/post-proposals/commit" and isinstance(body, dict):
                api.commits.append({str(key): value for key, value in body.items()})
                proposal = api.settle()
                if proposal is None or body.get("handle") != proposal["handle"]:
                    self.json(409, {"error": "proposal_used", "detail": "already settled"})
                else:
                    message = channel_message(99, "bot", str(proposal["text"]))
                    self.json(200, {"parts": [message], "posted": message, "serial": proposal["serial"]})
            elif path == "/api/v1/post-proposals/cancel" and isinstance(body, dict):
                api.cancels.append({str(key): value for key, value in body.items()})
                api.settle()
                self.json(204, {})
            else:
                # Timing, health and the transcript record: accepted and dropped.
                self.json(204, {})

        def asset(self, request_path: str) -> None:
            relative = "voice.html" if request_path == "/voice" else request_path.lstrip("/")
            target = (web_root / (relative or "index.html")).resolve()
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
            content = b"" if status == 204 else json.dumps(payload).encode()
            self.send_response(status)
            if status != 204:
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


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--browser-executable",
        default=os.environ.get("VIBE_TALK_CHROMIUM"),
        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)",
    )
    parser.add_argument(
        "--web-root",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "web",
        help="the page assets to serve (default: this checkout's vibe-talk/web)",
    )
    parser.add_argument(
        "--screenshots",
        type=Path,
        help="keep a PNG of the card on the call's view, the channel and Settings at each size here "
             "(default: none kept)",
    )
    return parser.parse_args()


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def until(page: Page, condition: Callable[[], bool], what: str, timeout_ms: int = 10_000) -> None:
    """Wait, while the browser runs (so routed sockets are served), until `condition()` holds."""
    for _ in range(timeout_ms // 20):
        if condition():
            return
        page.wait_for_timeout(20)
    raise AssertionError(f"timed out waiting for {what}")


# Every number this check judges, read in one go from the laid-out page.
MEASURE_JS = """() => {
  const box = (id) => { const n = document.getElementById(id); return n ? n.getBoundingClientRect() : null; };
  const shown = (id) => { const n = document.getElementById(id); if (!n || n.hidden) return false;
    const s = getComputedStyle(n); const r = n.getBoundingClientRect();
    return s.display !== 'none' && s.visibility !== 'hidden' && r.width > 0 && r.height > 0; };
  const card = document.getElementById('post-confirm');
  const send = box('post-confirm-send'), label = document.getElementById('post-confirm-send-label');
  const mine = [...document.querySelectorAll('#transcript > li.mine')].pop();
  return {
    innerHeight: window.innerHeight, innerWidth: window.innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
    position: getComputedStyle(card).position,
    parent: card.parentElement ? card.parentElement.id : '',
    previous: card.previousElementSibling ? card.previousElementSibling.id : '',
    next: card.nextElementSibling ? card.nextElementSibling.id : '',
    card: box('post-confirm'), send, where: box('post-confirm-where'),
    label: label.getBoundingClientRect(),
    labelCut: label.scrollWidth > label.clientWidth + 1,
    composer: box(card.parentElement && card.parentElement.id === 'pane-discord' ? 'channel-composer' : 'voice-composer'),
    pill: shown('status-line') ? box('status-line') : null,
    tools: shown('scroll-tools') ? box('scroll-tools') : null,
    jump: shown('jump-newest') ? box('jump-newest') : null,
    mine: mine ? mine.getBoundingClientRect() : null,
    boxSendShown: shown('send-text'),
    sendBackground: getComputedStyle(document.getElementById('post-confirm-send')).backgroundColor,
    boxSendBackground: getComputedStyle(document.getElementById('send-text')).backgroundColor,
    sendDisabled: document.getElementById('post-confirm-send').disabled,
    cancelDisabled: document.getElementById('post-confirm-cancel').disabled,
    folded: card.hasAttribute('data-folded'),
  };
}"""

TO_NEWEST_JS = "() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }"


def rect(value: object, key: str) -> float:
    check(isinstance(value, dict), f"no box measured for {key}")
    assert isinstance(value, dict)
    number = value[key]
    check(isinstance(number, (int, float)), f"{key} is not a number: {number!r}")
    assert isinstance(number, (int, float))
    return float(number)


def overlaps(a: object, b: object) -> bool:
    if not isinstance(a, dict) or not isinstance(b, dict):
        return False
    return (min(rect(a, "right"), rect(b, "right")) - max(rect(a, "left"), rect(b, "left")) > 0.5 and
            min(rect(a, "bottom"), rect(b, "bottom")) - max(rect(a, "top"), rect(b, "top")) > 0.5)


def inside(value: object, height: float, width: float) -> bool:
    return (rect(value, "top") >= -0.5 and rect(value, "bottom") <= height + 0.5 and
            rect(value, "left") >= -0.5 and rect(value, "right") <= width + 0.5)


def measure(page: Page) -> dict[str, object]:
    measured = page.evaluate(MEASURE_JS)
    check(isinstance(measured, dict), "the page measured nothing")
    assert isinstance(measured, dict)
    return {str(key): value for key, value in measured.items()}


def open_call(page: Page, url: str, voice: FakeVoice, api: FakeApi, rows: int, banner: bool) -> None:
    """Sign in, open a typed call with `rows` turns in it, and leave the banner up or dismissed."""
    api.reset()
    voice.routes.clear()
    # A fresh page each run: nothing kept on the device from the run before.
    page.goto(url, wait_until="load")
    page.evaluate("() => { localStorage.clear(); sessionStorage.clear(); }")
    page.goto(url, wait_until="load")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    page.fill("#compose-text", "question 0")
    page.click("#send-text")
    until(page, lambda: len(voice.routes) == 1, "the first send to open a conversation")
    voice.send({"type": "session_started", "greeting": False})
    until(page, lambda: api.watches >= 1, "the call to start watching for proposals")
    for turn in range(1, rows):
        role = "assistant" if turn % 2 else "user"
        text = (f"Answer {turn}: the nightly ran green on every target." if role == "assistant"
                else f"question {turn}")
        voice.send({"type": "transcript", "role": role, "turn": turn, "text": text})
        voice.send({"type": "turn_complete", "turn": turn})
    if rows > 1:
        page.wait_for_function(f"() => document.querySelectorAll('#transcript > li').length >= {rows}",
                               timeout=10_000)
    if banner:
        page.evaluate("() => { const b = document.getElementById('connection-banner'); b.hidden = false; }")
    else:
        page.evaluate("() => { document.getElementById('connection-banner').hidden = true; }")
    page.wait_for_timeout(100)


def propose(page: Page, api: FakeApi, serial: int, text: str) -> None:
    api.propose({"serial": serial, "handle": f"hAnDlE-browser-check-{serial:04d}", "channel_id": CHANNEL["id"],
                 "channel_name": "lead team", "text": text, "reply_to": None, "expires_in_ms": 120_000})
    page.locator("#post-confirm").wait_for(state="visible", timeout=10_000)


def reach_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int],
               rows: int, banner: bool, draft: str) -> float:
    """One run of points 1 to 5 on the call's view. Answers Send's centre as a share of the screen."""
    width, height = size
    where = f"{width}x{height}, {rows} row(s), banner {'up' if banner else 'gone'}, " \
            f"{'long' if draft is LONG_DRAFT else 'short'} draft"
    open_call(page, url, voice, api, rows, banner)
    propose(page, api, 7, draft)
    page.wait_for_timeout(HOLD_MS + 200)
    page.evaluate(TO_NEWEST_JS)
    page.wait_for_timeout(150)
    m = measure(page)

    # 1. In flow, the next thing after the transcript, before the box.
    check(m["position"] == "static", f"{where}: the card is positioned ({m['position']}), not in the list")
    check(m["parent"] == "pane-voice" and m["previous"] == "transcript" and m["next"] == "voice-composer",
          f"{where}: the card is not between the transcript and the box: {m['previous']} < card < {m['next']}")

    # 2. Reach, size, and clear of everything floating over the foot of the list.
    inner_height = float(str(m["innerHeight"]))
    inner_width = float(str(m["innerWidth"]))
    centre = (rect(m["send"], "top") + rect(m["send"], "bottom")) / 2 / inner_height
    check(centre >= REACH, f"{where}: Send's centre is at {centre:.2f} of the screen, above {REACH}")
    check(rect(m["send"], "height") >= 43.5, f"{where}: Send is {rect(m['send'], 'height'):.0f}px tall")
    check(rect(m["send"], "bottom") <= rect(m["composer"], "top") + 0.5,
          f"{where}: Send ends below the message box's top")
    for floating in ("pill", "tools", "jump"):
        check(not overlaps(m["send"], m[floating]), f"{where}: the {floating} is drawn over Send")
    check(float(str(m["scrollWidth"])) <= inner_width + 0.5, f"{where}: the page is wider than the screen")

    # 3. Send on screen means where it goes, and all of Send's label, on screen too.
    if inside(m["send"], inner_height, inner_width):
        check(inside(m["where"], inner_height, inner_width), f"{where}: Send is on screen and where it goes is not")
        check(inside(m["label"], inner_height, inner_width) and m["labelCut"] is False,
              f"{where}: Send's label is cut or off screen")
    if draft is LONG_DRAFT:
        check(m["folded"] is True, f"{where}: a long draft is not folded")
        page.click("#post-confirm-more")
        page.wait_for_timeout(150)
        opened = measure(page)
        check(opened["folded"] is False, f"{where}: More did not open the draft")
        if inside(opened["send"], inner_height, inner_width):
            check(inside(opened["where"], inner_height, inner_width),
                  f"{where}: opening the draft pushed where it goes off screen")
        page.click("#post-confirm-more")
        page.wait_for_timeout(100)

    # 4. The box's own Send: not drawn while the box is empty, and unlike the card's once typed.
    check(m["boxSendShown"] is False, f"{where}: the box's Send is drawn beside the card's")
    page.fill("#compose-text", "and one more thing")
    page.wait_for_timeout(100)
    typed = measure(page)
    check(typed["boxSendShown"] is True, f"{where}: typing in the box did not bring its Send back")
    check(typed["sendBackground"] != typed["boxSendBackground"],
          f"{where}: the box's Send looks like the card's ({typed['sendBackground']})")
    page.fill("#compose-text", "")

    # 5. Where a turn of the owner's stands.
    if m["mine"] is not None:
        for side in ("left", "right"):
            check(abs(rect(m["card"], side) - rect(m["mine"], side)) <= 1,
                  f"{where}: the card's {side} edge is {rect(m['card'], side):.1f}, a turn of yours "
                  f"{rect(m['mine'], side):.1f}")
    return centre


def placement_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int],
                   screenshots: Path | None) -> None:
    """Point 1 on the channel and Settings, and point 6, the hold, with real taps."""
    width, height = size
    where = f"{width}x{height}"
    open_call(page, url, voice, api, 30, False)
    propose(page, api, 9, SHORT_DRAFT)
    # 6. Both buttons held: real taps at once land on nothing.
    held = measure(page)
    check(held["sendDisabled"] is True and held["cancelDisabled"] is True,
          f"{where}: a button was live on the card's first frame")
    for button in ("post-confirm-cancel", "post-confirm-send"):
        b = page.locator(f"#{button}").bounding_box()
        check(b is not None, f"{where}: #{button} has no box")
        assert b is not None
        page.touchscreen.tap(b["x"] + b["width"] / 2, b["y"] + b["height"] / 2)
    page.wait_for_timeout(HOLD_MS // 2)
    check(not api.commits and not api.cancels,
          f"{where}: a tap during the hold reached the server: {api.commits or api.cancels}")
    page.wait_for_timeout(HOLD_MS)
    if screenshots:
        page.evaluate(TO_NEWEST_JS)
        page.screenshot(path=str(screenshots / f"card-voice-{width}x{height}.png"), animations="disabled")

    page.click("#view-switch")
    page.wait_for_function("() => !document.getElementById('pane-discord').hidden", timeout=10_000)
    page.wait_for_function("() => document.querySelectorAll('#discord-log > li').length > 0", timeout=10_000)
    page.evaluate(TO_NEWEST_JS)
    page.wait_for_timeout(200)
    # The status pill up, as it is for six seconds after almost anything: it stands on the card,
    # never on its buttons.
    page.evaluate("() => setStatus('fetching the channel…')")
    page.wait_for_timeout(150)
    m = measure(page)
    check(m["parent"] == "pane-discord" and m["next"] == "channel-composer",
          f"{where}: on the channel the card is not just above the box: {m['parent']}, before {m['next']}")
    check(m["position"] == "static", f"{where}: on the channel the card is positioned")
    for floating in ("pill", "tools", "jump"):
        check(not overlaps(m["send"], m[floating]), f"{where}: on the channel the {floating} is drawn over Send")
    if screenshots:
        page.screenshot(path=str(screenshots / f"card-channel-{width}x{height}.png"), animations="disabled")

    page.click("#open-settings")
    page.wait_for_selector("#screen-settings", state="visible", timeout=10_000)
    page.wait_for_timeout(200)
    page.evaluate("() => setStatus('fetching the channel…')")
    page.wait_for_timeout(150)
    m = measure(page)
    check(not overlaps(m["send"], m["pill"]), f"{where}: on Settings the status pill is drawn over Send")
    inner_height = float(str(m["innerHeight"]))
    inner_width = float(str(m["innerWidth"]))
    check(m["parent"] == "frame-body", f"{where}: on Settings the card is in #{m['parent']}, not docked")
    check(inside(m["card"], inner_height, inner_width), f"{where}: the docked card runs off the screen")
    check(inside(m["send"], inner_height, inner_width), f"{where}: the docked card's Send is off the screen")
    if screenshots:
        page.screenshot(path=str(screenshots / f"card-settings-{width}x{height}.png"), animations="disabled")
    page.click("#post-confirm-send")
    page.locator("#post-confirm").wait_for(state="hidden", timeout=5000)
    check([commit.get("confirmed_by") for commit in api.commits] == ["ui"],
          f"{where}: Send on the docked card did not commit once, as the UI: {api.commits}")
    page.click("#close-settings")


def walk(chromium: BrowserType, args: argparse.Namespace, url: str, api: FakeApi,
         size: tuple[int, int]) -> str:
    width, height = size
    voice = FakeVoice()
    with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-approval-card-") as directory:
        context = chromium.launch_persistent_context(
            directory,
            headless=True,
            executable_path=args.browser_executable,
            viewport={"width": width, "height": height},
            is_mobile=True,
            has_touch=True,
            device_scale_factor=2.75,
            user_agent=(
                "Mozilla/5.0 (Linux; Android 15; Pixel 7) AppleWebKit/537.36 "
                "(KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36"
            ),
        )
        try:
            context.route_web_socket(re.compile(r"/ws\?mode=voice$"), voice.accept)
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            lowest = 1.0
            for rows in (1, 30):
                for banner in (False, True):
                    for draft in (SHORT_DRAFT, LONG_DRAFT):
                        lowest = min(lowest, reach_case(page, url, voice, api, size, rows, banner, draft))
            placement_case(page, url, voice, api, size, args.screenshots)
            check(not errors, f"{width}x{height}: the page threw: {errors}")
        finally:
            context.close()
    return f"{width}x{height}: Send's centre at >= {lowest:.3f} of the screen in every run"


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

    api = FakeApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api, args.web_root.resolve()))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/voice"
    try:
        with sync_playwright() as playwright:
            lines = [walk(playwright.chromium, args, url, api, size) for size in PHONES]
        for line in lines:
            print(line)
        return 0
    finally:
        api.stopping.set()
        with api.changed:
            api.changed.notify_all()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
