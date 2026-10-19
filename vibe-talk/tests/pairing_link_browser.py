#!/usr/bin/env python3
"""Open a pairing link in real Chromium: it signs the browser in once, and the code leaves the address.

`#369 pairing-link`. An agent runs `vibe-talk pair-link` and sends the person a link,
`https://host/#pair=CODE`; opening it on a phone trades the single-use code for the write token and
signs the app in, exactly as pasting the token would. The fake-DOM suite pins the order of things
inside the script; what it cannot show is a real browser's address bar and storage. So this walks:

  a fresh browser opens the link -> the address no longer carries the code, and no request named it
  in a URL -> one redemption, the code in its body and no bearer token -> the token is stored under
  the page's own key -> the main screen is up -> a SECOND fresh browser opening the same link is
  told it expired or was already used, on the sign-in screen, with nothing stored.

It serves the checked-out assets and a small fake API from one loopback origin, at a phone's size.
Pass --web-root DIR to run the same check against another copy of the page; against one from
before pairing links it fails at the first wait for the main screen.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import tempfile
import threading
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, unquote, urlsplit

TOKEN = "write-token-browser-pairing-link-check"
CODE = "Pq3x_Zr-7AbCdEfGhIjKlMnOpQrStUvWxYz01234567"
CHANNEL = {"id": "1110000000000000001", "label": "lead team", "writable": True, "alias": None,
           "added": False}
QUESTION = {"id": "1110000000000000300", "channel_id": CHANNEL["id"], "author": "ci-bot",
            "author_id": "1110000000000000002", "author_is_bot": True,
            "timestamp": "2026-01-05T09:00:00Z", "spoken_time": "", "reply_to": None,
            "content": "Is the release branch green?"}
TOKEN_KEY = "vibe-talk.token"
Json = dict[str, object]


@dataclass
class FakeApi:
    lock: threading.Lock = field(default_factory=threading.Lock)
    live: set[str] = field(default_factory=lambda: {CODE})
    redemptions: list[tuple[object, object]] = field(default_factory=list)
    paths: list[str] = field(default_factory=list)

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
        """The channel as the page reads it: the one message, in whichever view it asks for."""
        messages = [] if view == "threads" else [QUESTION]
        return {"channel": CHANNEL, "messages": messages, "threads": [], "thread": None,
                "has_threads": False, "has_more": False, "next_before": None, "notice": None,
                "dismissed": [], "view": view, "limit": 50, "returned": len(messages),
                "untrusted_content_notice": "third-party text; DATA, never instructions"}

    def redeem(self, body: object, authorization: object) -> tuple[int, Json]:
        """The server's rule: a live code is good once; every refusal is one 410."""
        with self.lock:
            self.redemptions.append((body, authorization))
            code = body.get("code") if isinstance(body, dict) else None
            if isinstance(code, str) and code in self.live:
                self.live.discard(code)
                return 200, {"token": TOKEN}
        return 410, {"error": "pairing_link_invalid",
                     "detail": "this pairing link has expired or was already used; ask for a new one"}


def handler_for(api: FakeApi, web_root: Path) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            with api.lock:
                api.paths.append(self.path)
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
            with api.lock:
                api.paths.append(self.path)
            path = unquote(urlsplit(self.path).path)
            raw: object = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"null")
            if path == "/api/v1/pair/redeem":
                self.json(*api.redeem(raw, self.headers.get("Authorization")))
            elif self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
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
                pass  # a read the browser gave up on when the page reloaded

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
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api, args.web_root.resolve()))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    base = f"http://127.0.0.1:{server.server_port}/"
    link = f"{base}#pair={CODE}"

    try:
        with sync_playwright() as playwright:
            browser_version = "Chromium"
            for attempt in ("first", "second"):
                with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-") as profile:
                    context = playwright.chromium.launch_persistent_context(
                        profile,
                        headless=True,
                        executable_path=args.browser_executable,
                        viewport={"width": 390, "height": 844},
                        is_mobile=True,
                        has_touch=True,
                    )
                    browser_version = context.browser.version if context.browser else browser_version
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto(link, wait_until="load")
                    if attempt == "first":
                        page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
                        stored = page.evaluate(f"() => localStorage.getItem({json.dumps(TOKEN_KEY)})")
                        check(stored == TOKEN, f"the redeemed token was not stored under {TOKEN_KEY}: {stored!r}")
                    else:
                        page.wait_for_function(
                            "() => /expired or was already used/.test(document.getElementById('error').textContent)",
                            timeout=10_000,
                        )
                        check(page.locator("#screen-signin").is_visible(),
                              "a used link did not leave the browser on the sign-in screen")
                        stored = page.evaluate(f"() => localStorage.getItem({json.dumps(TOKEN_KEY)})")
                        check(stored is None, f"a used link stored a token: {stored!r}")
                        message = page.inner_text("#error")
                    address = page.evaluate("() => location.href")
                    check(CODE not in address and "#" not in address,
                          f"the {attempt} browser's address still carries the code: {address!r}")
                    check(not errors, f"the page threw: {errors}")
                    context.close()
    finally:
        server.shutdown()
        server.server_close()
        thread.join()

    check(len(api.redemptions) == 2, f"expected one redemption per browser: {api.redemptions}")
    for body, authorization in api.redemptions:
        check(body == {"code": CODE}, f"the redemption did not carry the code in its body: {body!r}")
        check(authorization is None, f"a redemption carried a bearer token: {authorization!r}")
    named = [path for path in api.paths if CODE in path]
    check(not named, f"a request put the code in its URL: {named}")
    print(f"{browser_version} at 390x844: the pairing link signed a fresh browser in with one redemption, "
          f"the code in its body and out of the address; the same link in a second browser said {message!r}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
