#!/usr/bin/env python3
"""The Send a phone's keyboard promises, in WebKit at an iPhone's size.

`ios-keyboard-send` (owner request 048, on iOS): the keyboard showed no Send at
all, and the page's Send sat below the box, off screen or under the keyboard, so
sending meant first shrinking the keyboard to go and find it. Three things are
checked here, in Playwright's WebKit — the engine iOS Safari and every iOS
browser shell run — with an iPhone viewport and touch:

  * every box that sends (`COMPOSER_FIELDS`: the call's, the channel's, its
    right-column twin, the reply in the stream, the floating quick reply and
    the Discuss follow-up) asks its keyboard for that key the same way:
    `enterkeyhint` says what Enter does for the setting as it stands, over a
    text keyboard (`inputmode`);
  * a real Enter in the reply box sends, once, with Enter to send on (the
    per-device default: nothing kept reads as on), as it does in Chromium;
  * with the visual viewport shrunk as a keyboard shrinks it, a Send stands at
    the keyboard's top edge (`#keyboard-send`, put there from the visual
    viewport) while the box's own Send is outside the visible part, and stands
    down again when that Send is scrolled into it — never two Sends together.

What this cannot say, and only a real iPhone can: what label the phone's own
keyboard draws for `enterkeyhint="send"` in a multiline box (WebKit has drawn
a plain return there while still sending on Enter), and how the real keyboard
and Safari's pan move the visual viewport as it comes up. The shrink here is
simulated on the page's own viewport numbers; the corner Send's promise —
a Send above where the keyboard will be — is the part that does not depend
on that drawing.

Serves the checked-out assets and a small fake API from one loopback origin.
Pass --web-root DIR to run against another copy of the page; against one from
before the change it fails on the quick reply's unnamed return key.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import threading
from dataclasses import dataclass, field
from datetime import datetime, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, unquote, urlsplit

import page_sources

TOKEN = "write-token-browser-ios-keyboard-send-check"
CHANNEL = {"id": "1110000000000000001", "label": "lead team", "writable": True, "alias": None,
           "added": False}
QUESTION = {"id": "1110000000000000300", "channel_id": CHANNEL["id"], "author": "ci-bot",
            "author_id": "1110000000000000002", "author_is_bot": True,
            "timestamp": "2026-01-05T09:00:00Z", "spoken_time": "", "reply_to": None,
            "content": "Is the release branch green?"}
BOXES = ["compose-text", "channel-compose-text", "side-compose-text", "reply-text",
         "quick-reply-text", "discuss-followup"]
Json = dict[str, object]


@dataclass
class FakeApi:
    lock: threading.Lock = field(default_factory=threading.Lock)
    posts: list[Json] = field(default_factory=list)

    def client_config(self) -> Json:
        return {
            "version": "browser-check",
            "chat_provider_name": "Discord",
            "channels": [CHANNEL],
            "live_poll_seconds": 30,
            "live_delivery": "poll",
            "threading_supported": True,
            "channel_registration_supported": False,
            "read_aloud": {"backend": "browser", "label": "Browser voice", "playback": "browser",
                           "local_only": True},
            "token_scope": "write",
            "conversational_voice": {"name": "Test voice provider"},
            "replay_enabled": False,
            "self_author_id": None,
            "owner_author_id": None,
            "channel_discovery_supported": False,
            "upstream_read_mark_supported": False,
            "speech_prep_enabled": False,
        }

    def timeline(self, view: str) -> Json:
        messages = [] if view == "threads" else [QUESTION]
        return {"channel": CHANNEL, "messages": messages, "threads": [], "thread": None,
                "has_threads": False, "has_more": False, "next_before": None, "notice": None,
                "dismissed": [], "view": view, "limit": 50, "returned": len(messages),
                "untrusted_content_notice": "third-party text; DATA, never instructions"}

    def post(self, body: Json) -> Json:
        with self.lock:
            self.posts.append(body)
            serial = len(self.posts)
        return {"posted": {
            "id": f"11100000000000009{serial:02d}", "channel_id": CHANNEL["id"], "author": "me",
            "author_id": "1110000000000000098", "author_is_bot": False,
            "timestamp": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
            "spoken_time": "", "reply_to": body.get("reply_to"), "content": body.get("text"),
        }}

    def sent(self) -> list[tuple[object, object]]:
        with self.lock:
            return [(body.get("text"), body.get("reply_to")) for body in self.posts]


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
            elif path.endswith("/timeline"):
                self.json(200, api.timeline(parse_qs(url.query).get("view", ["main"])[0]))
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def do_POST(self) -> None:  # noqa: N802
            path = unquote(urlsplit(self.path).path)
            raw: object = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"null")
            if self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path == f"/api/v1/channels/{CHANNEL['id']}/reply" and isinstance(raw, dict):
                self.json(200, api.post({str(key): value for key, value in raw.items()}))
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def asset(self, request_path: str) -> None:
            relative = "voice.html" if request_path == "/" else request_path.lstrip("/")
            target = (web_root / relative).resolve()
            if web_root not in target.parents or not target.is_file():
                self.send_error(404)
                return
            body = target.read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", mimetypes.guess_type(target.name)[0] or "application/octet-stream")
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

        def log_message(self, _format: str, *_args: object) -> None:
            pass

    return Handler


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--web-root",
        type=Path,
        default=page_sources.served_root(),
        help="the page assets to serve (default: this checkout's vibe-talk/web)",
    )
    parser.add_argument(
        "--browser",
        choices=["chromium", "webkit"],
        default="chromium",
        help="the engine to run (default: chromium, as validation runs it; webkit where that "
             "browser is installed — a missing WebKit is a skip with its reason, never a failure)",
    )
    return parser.parse_args()


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import Error as PlaywrightError
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error

    api = FakeApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api, page_sources.served(args.web_root)))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/"
    reply = QUESTION["id"]
    try:
        with sync_playwright() as playwright:
            iphone = playwright.devices.get("iPhone 14") or {
                "viewport": {"width": 390, "height": 844}, "is_mobile": True, "has_touch": True,
                "user_agent": "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) "
                              "AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1",
            }
            if args.browser == "webkit":
                try:
                    browser = playwright.webkit.launch(headless=True)
                except PlaywrightError as error:
                    if "Executable doesn't exist" not in str(error) and "executable doesn't exist" not in str(error):
                        raise
                    print("SKIP ios-keyboard-send: Playwright WebKit is not installed here, so the "
                          "WebKit run is skipped (never a failure); install it with "
                          "python3 -m playwright install webkit, or run --browser chromium")
                    return 0
            else:
                browser = playwright.chromium.launch(headless=True)
            context = browser.new_context(**iphone)
            page = context.new_page()
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(url, wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#view-switch", state="visible", timeout=15_000)

            # 1. Every sending box names its return key for the setting, over a text keyboard.
            for box_id in BOXES:
                hint = page.evaluate("(id) => document.getElementById(id).getAttribute('enterkeyhint')", box_id)
                mode = page.evaluate("(id) => document.getElementById(id).getAttribute('inputmode')", box_id)
                check(hint == "send", f"#{box_id} asks its keyboard for {hint!r}, not Send, with Enter to send on")
                check(mode == "text", f"#{box_id} has inputmode {mode!r}, not a text keyboard")
            check(page.is_checked("#enter-sends"), "Enter to send is not on with nothing kept on this device")

            # 2. A real Enter sends from the reply box, once. No wall-clock wait for the send
            # itself: the posted reply is the wait.
            button = page.locator(f'#discord-log li[data-id="{reply}"] .reply-button').first
            if not button.is_visible():
                page.click("#view-switch")
            button.wait_for(state="visible", timeout=10_000)
            button.click()
            page.locator("#column-reply").wait_for(state="visible", timeout=5000)
            page.click("#reply-text")
            page.keyboard.type("sent from the return key")
            page.keyboard.press("Enter")
            page.locator("#column-reply").wait_for(state="hidden", timeout=10_000)
            check(api.sent() == [("sent from the return key", reply)],
                  f"Enter in the reply box did not post the reply once: {api.sent()}")

            # 3. The corner Send under a simulated shrink. The full height is measured first
            # (the channel box focused, as a reader writing there), then the page's own visual
            # viewport numbers are stood in shrunk, as a keyboard standing up leaves them, and
            # the focus moving asks for the measure again — the resize and scroll the page
            # already hears, without waiting on any clock.
            page.click("#channel-compose-text")
            page.wait_for_function(
                "() => document.activeElement && document.activeElement.id === 'channel-compose-text'",
                timeout=10_000)
            page.keyboard.type("written with the keyboard up")
            shrunk = page.evaluate("""() => {
              const real = window.visualViewport;
              const fake = new EventTarget();
              fake.height = 460; fake.offsetTop = 0; fake.pageTop = 0; fake.scale = 1;
              try {
                Object.defineProperty(window, 'visualViewport', { value: fake, configurable: true });
              } catch (_error) { return false; }
              return window.visualViewport === fake;
            }""")
            check(shrunk, "the simulated visual viewport could not be stood in")
            page.evaluate("""() => {
              document.getElementById('reply-text').focus({ preventScroll: true });
              document.getElementById('channel-compose-text').focus({ preventScroll: true });
            }""")
            # The promise is that a Send is reachable, not which one: the corner Send at the
            # keyboard's top, or the row's own Send already in the visible part (the corner
            # Send stands down for it, and the fit may bring it there a measure later).
            page.wait_for_function(
                "() => { const send = document.getElementById('keyboard-send');"
                " const own = document.getElementById('channel-send').getBoundingClientRect();"
                " return (send && !send.hidden) || (own.bottom > own.top && own.bottom <= 461); }",
                timeout=10_000)
            placed = page.evaluate("""() => {
              const send = document.getElementById('keyboard-send');
              const own = document.getElementById('channel-send');
              const rect = send.getBoundingClientRect();
              const ownRect = own.getBoundingClientRect();
              return { bottom: send.style.bottom, hidden: send.hidden,
                       top: rect.top, foot: rect.bottom, ownFoot: ownRect.bottom };
            }""")
            inner = page.evaluate("() => window.innerHeight")
            # The layout viewport keeps its height (`innerHeight`): the simulated keyboard's
            # top stands at 460 in it, so the corner Send's foot is that far, plus its gap,
            # above the layout foot — wherever this browser puts the layout height. Where the
            # corner Send stands, that is where it stands; where the row's own Send already
            # stands in the visible part instead, it is the reachable one and is pressed.
            expect = f"{int(inner) - 460 + 8}px"
            if not placed["hidden"]:
                check(placed["bottom"] == expect,
                      f"the corner Send stands at bottom {placed['bottom']!r}, not the keyboard's top {expect}: {placed}")
                check(placed["foot"] <= 460 + 1 and placed["top"] < 460,
                      f"the corner Send is not above the simulated keyboard's top: {placed} (inner {inner})")
                page.click("#keyboard-send")
            else:
                check(placed["ownFoot"] <= 460 + 1,
                      f"neither Send is reachable above the simulated keyboard: {placed} (inner {inner})")
                page.click("#channel-send")
            page.wait_for_function(
                "() => document.getElementById('channel-compose-text').value === ''",
                timeout=10_000)
            check(api.sent()[-1] == ("written with the keyboard up", None),
                  f"the corner Send did not post the channel words once: {api.sent()}")
            check(not errors, f"the page threw: {errors}")
            context.close()
            browser.close()
    finally:
        server.shutdown()
    print(f"ios-keyboard-send: {args.browser} at an iPhone size names every sending box's return key, "
          "sends on a real Enter, and stands a Send at the simulated keyboard's top")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
