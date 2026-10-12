#!/usr/bin/env python3
"""Press Reply in real Chromium and measure what moved, at 1280x800 and 412x915.

`#247 reply-continuity`. The owner: "Reply jumps to a different view with different FONT SIZE. This is
jarring ... I'd rather the top of the message I press Reply on to stay still on the screen. Expand it
below and truncate below that (moving up the text entry box underneath)." The fake-DOM suite
(tests/js/voice_page.test.mjs) holds the rules and the arithmetic, in a model that lays out nothing
but heights and gives the list and the reply screen one origin. It cannot say what size the words
are, whether the reply screen's title bar moves the message, or where the box really ends up. This
does, in the engine.

It serves the checked-out page and a small fake API from one loopback origin: a channel of forty
messages, short and long, from the agent and from another person, one of them a reply and one taller
than a phone's screen. At a desk (mouse) and a phone (touch), in the dark scheme, at three of the text
sizes Settings offers, for a message whose top is in the middle of the list, just under where the
title bar will be, half under the list's top edge, low on the screen, and taller than the screen:

  scroll the row there and measure its top and the computed size of its words -> press its Reply ->
  the reply screen is up with its tint; the message is drawn at the same size and the same width;
  its top is where the row's was (within 1px), or at the top of the conversation where the row's
  was above that; where it fits, the conversation ends at its foot and the box is directly under
  it; low on the screen it moves up only as far as shows all of it above the box; taller than the
  screen it keeps its top and the box is still on screen -> Cancel, and the list is back where it
  was, to the pixel.

Then once more in the light scheme, to measure the tint there and keep a picture of it. No
horizontal overflow and no page error at any step.

Pass --screenshots DIR to keep a PNG of the reply screen for each case; --web-root DIR to run the
same walk against another copy of the page.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import tempfile
import threading
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING, Literal
from urllib.parse import parse_qs, unquote, urlsplit

if TYPE_CHECKING:
    from playwright.sync_api import Page

WEB_ROOT = Path(__file__).resolve().parents[1] / "web"
TOKEN = "write-token-browser-reply-continuity-check"
CHANNEL = {"id": "1110000000000000002", "label": "lead team", "writable": True, "alias": None,
           "added": False}
EPOCH = datetime(2026, 10, 12, 7, 0, tzinfo=timezone.utc)
AGENT = {"author": "ci-bot", "author_id": "1000000000000000001", "author_is_bot": True}
PEER = {"author": "alice", "author_id": "1000000000000000005", "author_is_bot": False}
COUNT = 40
# The message taller than a phone's screen, and the reply (to message 3).
TALL = 20
REPLY = 26
Json = dict[str, object]
Scheme = Literal["dark", "light"]

FILLER = ("The overnight run is still going: the integration shard has been retried twice and the "
          "artifact upload is waiting on the cache to warm. ")


def message_id(index: int) -> str:
    return f"89000000000000000{index:02d}"


def message(index: int) -> Json:
    if index == TALL:
        text = f"Message {index}, the long one. " + FILLER * 40
    elif index % 3 == 0:
        text = f"Message {index}: checked."
    else:
        text = f"Message {index}. " + FILLER * (1 + index % 2)
    return {
        "id": message_id(index), "channel_id": CHANNEL["id"],
        "timestamp": (EPOCH + timedelta(minutes=index)).isoformat().replace("+00:00", "Z"),
        "spoken_time": "", "reply_to": message_id(3) if index == REPLY else None, "content": text,
        **(PEER if index % 4 == 1 else AGENT),
    }


@dataclass
class FakeApi:
    """The routes /voice reads, over one channel with no threads, and the reply route."""

    stopping: threading.Event = field(default_factory=threading.Event)
    lock: threading.Lock = field(default_factory=threading.Lock)
    messages: list[Json] = field(default_factory=lambda: [message(index) for index in range(COUNT)])
    replies: list[Json] = field(default_factory=list)

    def client_config(self) -> Json:
        return {
            "token_scope": "write", "version": "browser-check", "chat_provider_name": "Google Chat",
            "channels": [CHANNEL], "live_poll_seconds": 30, "live_delivery": "poll",
            "threading_supported": True, "channel_registration_supported": False,
            "read_aloud": {"backend": "browser", "label": "Browser voice", "playback": "browser",
                           "local_only": True},
            "elevenlabs_agent_id": None, "conversational_voice": {"name": "Test voice provider"},
            "replay_enabled": False, "self_author_id": None, "owner_author_id": None,
            "channel_discovery_supported": False, "upstream_read_mark_supported": False,
            "speech_prep_enabled": False,
        }

    def timeline(self, query: dict[str, list[str]]) -> Json:
        view = query.get("view", ["main"])[0]
        with self.lock:
            rows = list(self.messages)
        return {
            "channel": CHANNEL, "messages": rows, "threads": [], "thread": None,
            "has_threads": False, "has_more": False, "next_before": None, "notice": None,
            "dismissed": [], "view": view, "limit": 50, "returned": len(rows),
            "bookmark": {"revision": 0, "anchor": None},
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
            elif path.endswith("/bookmark"):
                self.json(200, {"channel": CHANNEL, "bookmark": {"revision": 0, "anchor": None},
                                "changed": False, "bookmark_notice": "The bookmark is this check's own."})
            elif path.endswith("/stream"):
                self.stream()
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def do_POST(self) -> None:  # noqa: N802
            if not self.authorized():
                return
            raw = self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"{}"
            parsed: object = json.loads(raw)
            body: Json = parsed if isinstance(parsed, dict) else {}
            if unquote(urlsplit(self.path).path).endswith("/reply"):
                with api.lock:
                    api.replies.append(body)
                    posted = {**message(len(api.messages)), "content": str(body.get("text", "")),
                              "reply_to": body.get("reply_to")}
                    api.messages.append(posted)
                self.json(200, {"posted": posted, "parts": [posted]})
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

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


# The row on the list, the message on the reply screen, and what is around each, measured by the
# engine. `id` is the message's; the list row and the reply screen's row are found by it.
MEASURE_JS = """(id) => {
  const box = (node) => {
    if (!node || !node.getClientRects().length) return null;
    const r = node.getBoundingClientRect();
    return {top: r.top, bottom: r.bottom, left: r.left, right: r.right, width: r.width, height: r.height};
  };
  const shown = (name) => !document.getElementById(`screen-${name}`).hidden;
  const row = [...document.querySelectorAll('#discord-log > li[data-id]')].find((li) => li.dataset.id === id);
  const target = document.querySelector('#reply-target > li');
  const words = (li) => li ? li.querySelector('.body .md') || li.querySelector('.body') : null;
  const size = (li) => words(li) ? parseFloat(getComputedStyle(words(li)).fontSize) : null;
  const area = document.getElementById('scroll-area');
  const pane = document.getElementById('reply-scroll');
  const page = getComputedStyle(document.body).backgroundColor;
  return {
    screen: shown('reply') ? 'reply' : shown('main') ? 'main' : null,
    row: box(row), rowSize: size(row), rowWords: box(words(row)),
    above: box(row ? row.previousElementSibling : null),
    contextAbove: box(document.querySelector('#reply-context > li:last-child')),
    target: box(target), targetSize: size(target), targetWords: box(words(target)),
    targetId: target ? target.dataset.id : null,
    targetClamped: target ? !!target.querySelector('.body.clamped') : null,
    list: box(area), listTop: area.scrollTop, listMax: area.scrollHeight - area.clientHeight,
    pane: box(pane), fit: pane.getAttribute('data-fit'),
    paneScrolls: pane.scrollHeight > pane.clientHeight + 1,
    text: box(document.getElementById('reply-text')),
    send: box(document.getElementById('reply-send')),
    tint: getComputedStyle(document.getElementById('frame-body')).backgroundColor,
    page,
    scale: getComputedStyle(document.documentElement).getPropertyValue('--msg-scale').trim(),
    overflow: document.scrollingElement.scrollWidth - window.innerWidth,
    height: window.innerHeight,
  };
}"""


def check(condition: bool, message_text: str) -> None:
    if not condition:
        raise AssertionError(message_text)


@dataclass
class Measured:
    """What `MEASURE_JS` found, read with the types each part has."""

    found: Json

    def box(self, name: str) -> dict[str, float]:
        part = self.found.get(name)
        check(isinstance(part, dict), f"nothing to measure for {name}: {self.found}")
        assert isinstance(part, dict)
        return {str(key): float(value) for key, value in part.items() if isinstance(value, (int, float))}

    def number(self, name: str) -> float:
        value = self.found.get(name)
        check(isinstance(value, (int, float)), f"{name} is not a number: {self.found}")
        assert isinstance(value, (int, float))
        return float(value)

    def text(self, name: str) -> str:
        return str(self.found.get(name))


class Walk:
    """One reader at one size and scheme, and what the walk has found there."""

    def __init__(self, page: Page, api: FakeApi, touch: bool, label: str, shots: Path | None) -> None:
        self.page = page
        self.api = api
        self.touch = touch
        self.label = label
        self.shots = shots
        self.notes: list[str] = []

    def check(self, condition: bool, why: str) -> None:
        check(condition, f"{self.label}: {why}")

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"reply-continuity-{self.label}-{name}.png"))

    def measure(self, index: int) -> Measured:
        found: object = self.page.evaluate(MEASURE_JS, message_id(index))
        self.check(isinstance(found, dict), f"the page measured nothing: {found!r}")
        assert isinstance(found, dict)
        return Measured({str(key): value for key, value in found.items()})

    def settled(self) -> None:
        self.page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")
        self.page.wait_for_timeout(80)

    def wait_for(self, condition: str, why: str, seconds: float = 10) -> None:
        try:
            self.page.wait_for_function(condition, timeout=seconds * 1000)
        except Exception as error:  # noqa: BLE001 - Playwright's timeout, said as this check's failure
            raise AssertionError(f"{self.label}: {why}") from error

    def press(self, selector: str) -> None:
        """A finger on a phone, the mouse on a desk: at the centre of what `selector` draws, unscrolled."""
        box = self.page.locator(selector).first.bounding_box()
        self.check(box is not None, f"{selector} has no box on screen")
        assert box is not None
        x, y = box["x"] + box["width"] / 2, box["y"] + box["height"] / 2
        if self.touch:
            self.page.touchscreen.tap(x, y)
        else:
            self.page.mouse.click(x, y)

    def place(self, index: int, y: float) -> Measured:
        """Scroll the list so the row of message `index` has its top at window y `y`."""
        self.page.evaluate(
            """([id, y]) => { const area = document.getElementById('scroll-area');
              const row = [...document.querySelectorAll('#discord-log > li[data-id]')].find((li) => li.dataset.id === id);
              area.scrollTop += row.getBoundingClientRect().top - y; }""",
            [message_id(index), y])
        self.settled()
        return self.measure(index)


def reply_case(walk: Walk, index: int, y: float, case: str, scale: int) -> None:
    """Reply on message `index` with its row's top at `y`: what moved, then Cancel and what came back."""
    before = walk.place(index, y)
    row = before.box("row")
    list_box = before.box("list")
    walk.check(row["top"] >= list_box["top"] - row["height"] and row["bottom"] > list_box["top"],
               f"{case}: the row is not on screen to press: {row}")
    walk.check(abs(float(before.text("scale")) - scale / 100) < 1e-9, f"the text size is {before.text('scale')}, not {scale}%")
    list_top = before.number("listTop")
    # The Reply of that row, pressed where it is drawn. Where the row's top is above the list's edge
    # its buttons may be too: then by the button itself, which a keyboard reaches as well.
    button = f"#discord-log > li[data-id='{message_id(index)}'] .reply-button"
    reply_box = walk.page.locator(button).bounding_box()
    if reply_box is not None and reply_box["y"] >= list_box["top"] and reply_box["y"] + reply_box["height"] <= list_box["bottom"]:
        walk.press(button)
    else:
        walk.page.evaluate("(selector) => document.querySelector(selector).click()", button)
    walk.wait_for("() => !document.getElementById('screen-reply').hidden", f"{case}: the reply screen did not open")
    walk.settled()
    after = walk.measure(index)
    target, pane, text = after.box("target"), after.box("pane"), after.box("text")
    walk.check(after.text("targetId") == message_id(index), f"{case}: the reply screen answers {after.text('targetId')}")
    # The same words at the same size, as wide as they were.
    walk.check(abs(after.number("targetSize") - before.number("rowSize")) < 0.01,
               f"{case}: the words are {after.number('targetSize')}px on Reply and {before.number('rowSize')}px on the list")
    walk.check(abs(target["left"] - row["left"]) <= 1 and abs(target["width"] - row["width"]) <= 1,
               f"{case}: the message is {target['left']:.1f}+{target['width']:.1f} wide on Reply, "
               f"{row['left']:.1f}+{row['width']:.1f} on the list")
    walk.check(after.found["targetClamped"] is False, f"{case}: the message being answered is folded")
    # The tint, and the page's own colour under it.
    walk.check(after.text("tint") != after.text("page") and after.text("tint") not in ("", "rgba(0, 0, 0, 0)"),
               f"{case}: the reply screen is not tinted: {after.text('tint')} on {after.text('page')}")
    # The box is on the screen, under the message's conversation.
    walk.check(text["top"] >= pane["bottom"] - 1 and after.box("send")["bottom"] <= after.number("height") + 1,
               f"{case}: the box is not on screen under the conversation: {text} {pane}")
    expected = max(row["top"], pane["top"])
    if case == "low":
        # Moved up only as far as shows all of it over the box.
        walk.check(target["bottom"] <= pane["bottom"] + 1 and abs(target["bottom"] - pane["bottom"]) <= 1,
                   f"{case}: the message does not end directly over the box: {target} {pane}")
        walk.check(target["top"] < row["top"], f"{case}: the message did not move up, so this asks nothing: {target}")
        walk.check(target["height"] <= pane["height"] + 1, f"{case}: the message is taller than the room")
    elif case == "tall":
        walk.check(target["height"] > pane["height"], f"{case}: the message is not taller than the screen: {target} {pane}")
        walk.check(abs(target["top"] - expected) <= 1, f"{case}: a tall message's top moved from {row['top']:.1f} to {target['top']:.1f}")
        walk.check(after.found["fit"] is None and after.found["paneScrolls"] is True,
                   f"{case}: the rest of the message is not a scroll down in the whole room: {after.found}")
    else:
        walk.check(abs(target["top"] - expected) <= 1,
                   f"{case}: the message's top moved from {row['top']:.1f} (kept at {expected:.1f}) to {target['top']:.1f}")
        walk.check(after.found["fit"] == "message" and abs(pane["bottom"] - target["bottom"]) <= 1,
                   f"{case}: the conversation does not end at the message's foot: {target} {pane} {after.found['fit']}")
        walk.check(abs(after.box("targetWords")["top"] - before.box("rowWords")["top"] - (target["top"] - row["top"])) <= 1,
                   f"{case}: the words moved inside the message: {after.box('targetWords')} {before.box('rowWords')}")
    if case == "middle":
        # What came before it is where it was too: the earlier message over it, drawn as the list drew it.
        above, context_above = before.box("above"), after.box("contextAbove")
        walk.check(abs(context_above["top"] - above["top"]) <= 1 and abs(context_above["height"] - above["height"]) <= 1,
                   f"{case}: the message above moved from {above} to {context_above}")
    walk.check(after.number("overflow") <= 0, f"{case}: the page scrolls sideways")
    walk.shot(f"{scale}-{case}")
    moved = target["top"] - row["top"]
    # Cancel, and the list is where it was.
    walk.press("#reply-cancel")
    walk.wait_for("() => !document.getElementById('screen-main').hidden", f"{case}: Cancel did not go back")
    walk.settled()
    back = walk.measure(index)
    walk.check(abs(back.number("listTop") - list_top) <= 1, f"{case}: Cancel came back to {back.number('listTop')}, not {list_top}")
    walk.check(abs(back.box("row")["top"] - row["top"]) <= 1, f"{case}: after Cancel the row is at {back.box('row')['top']}, not {row['top']}")
    walk.check(back.text("tint") == back.text("page") or back.text("tint") == "rgba(0, 0, 0, 0)",
               f"{case}: the tint outlived the reply screen: {back.text('tint')}")
    walk.notes.append(f"{scale}% {case}: words {after.number('targetSize'):.2f}px both, top {row['top']:.1f} -> "
                      f"{target['top']:.1f} ({moved:+.1f})")


def continuity_walk(walk: Walk, scales: tuple[int, ...]) -> None:
    page = walk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    walk.wait_for(f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {COUNT}",
                  "the channel did not open")
    walk.settled()
    for scale in scales:
        # Through the Settings slider's own listener, as a drag of it does.
        page.evaluate("(pct) => { const s = document.getElementById('msg-scale'); s.value = String(pct); "
                      "s.dispatchEvent(new Event('input', {bubbles: true})); }", scale)
        walk.settled()
        list_box = walk.measure(30).box("list")
        top, height = list_box["top"], list_box["height"]
        reply_case(walk, 30, top + height * 0.4, "middle", scale)
        reply_case(walk, 33, top + 70, "under-title", scale)
        reply_case(walk, 12, top - 20, "clamped", scale)
        reply_case(walk, REPLY, top + height * 0.3, "reply-gutter", scale)
        reply_case(walk, 34, top + height * 0.86, "low", scale)
        reply_case(walk, TALL, top + height * 0.2, "tall", scale)


def light_walk(walk: Walk) -> None:
    """The light scheme: the tint is there too, and differs from the page; a picture of it."""
    page = walk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
    page.click("#view-switch")
    walk.wait_for(f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {COUNT}",
                  "the channel did not open")
    walk.settled()
    list_box = walk.measure(30).box("list")
    reply_case(walk, 30, list_box["top"] + list_box["height"] * 0.4, "middle", 100)


def serve(api: FakeApi, web_root: Path) -> tuple[ThreadingHTTPServer, threading.Thread, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api, web_root))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return server, thread, f"http://127.0.0.1:{server.server_port}/voice"


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
        default=WEB_ROOT,
        help=f"directory holding the page to serve (default: the checked-out {WEB_ROOT})",
    )
    parser.add_argument(
        "--screenshots",
        type=Path,
        help="directory to write a PNG of the reply screen for each case into (default: none are kept)",
    )
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
    web_root = args.web_root.resolve()
    # (label, width, height, a phone, the scheme)
    runs: tuple[tuple[str, int, int, bool, Scheme], ...] = (
        ("1280x800-dark", 1280, 800, False, "dark"),
        ("412x915-dark", 412, 915, True, "dark"),
        ("1280x800-light", 1280, 800, False, "light"),
        ("412x915-light", 412, 915, True, "light"),
    )
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, phone, scheme in runs:
            api = FakeApi()
            server, thread, url = serve(api, web_root)
            try:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-reply-") as profile:
                    if phone:
                        context = playwright.chromium.launch_persistent_context(
                            profile, headless=True, executable_path=args.browser_executable,
                            viewport={"width": width, "height": height}, color_scheme=scheme,
                            user_agent=("Mozilla/5.0 (Linux; Android 15; Pixel 7) AppleWebKit/537.36 "
                                        "(KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36"),
                            device_scale_factor=2.625, is_mobile=True, has_touch=True,
                        )
                    else:
                        context = playwright.chromium.launch_persistent_context(
                            profile, headless=True, executable_path=args.browser_executable,
                            viewport={"width": width, "height": height}, color_scheme=scheme,
                        )
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(url, wait_until="load")
                    walk = Walk(page, api, phone, label, args.screenshots)
                    if scheme == "dark":
                        continuity_walk(walk, (100, 80, 130))
                    else:
                        light_walk(walk)
                    check(not errors, f"{label}: the page threw: {errors}")
                    version = context.browser.version if context.browser else "Chromium"
                    context.close()
                    report.append(f"{version} at {label}: " + "; ".join(walk.notes))
            finally:
                api.stopping.set()
                server.shutdown()
                server.server_close()
                thread.join()
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
