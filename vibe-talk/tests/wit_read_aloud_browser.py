#!/usr/bin/env python3
"""Read-aloud with the Wit.ai backend, walked in real Chromium at phone and desktop sizes.

`#552 wit-tts-provider`. It serves the checked-out page and a small fake
API from one loopback origin. The fake API answers as a server whose
`read_aloud.backend = "wit"` would: client-config names the Wit.ai
voice, `GET /api/v1/read-aloud/voices` lists the fake catalogue, speech
preparation mints tickets, and a ticket plays a short WAV tone. The Rust
side against a fake Wit.ai server is covered by tests/wit_tts.rs; this
walk covers what only the page can show:

  in Settings, "Wit.ai voice" is the offered server voice and its picker
  lists the catalogue -> picking a voice there is what the next speech
  preparation asks for -> a tap is heard (the page reports it audible)
  and the read says it used the Wit.ai voice.

Both walks — 393x852 (a phone) and a desktop viewport — run the same
steps. Pass --screenshots DIR to keep a PNG of each step for review, and
--web-root DIR to run against another copy of the page.
"""

from __future__ import annotations

import argparse
import json
import math
import mimetypes
import re
import struct
import tempfile
import threading
import time
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import unquote, urlsplit

if TYPE_CHECKING:
    from playwright.sync_api import Page

TOKEN = "write-token-browser-wit-check"
CHANNEL = {"id": "1110000000000000001", "label": "lead team", "writable": True, "alias": None,
           "added": False}
EPOCH = datetime(2026, 1, 5, 9, 0, tzinfo=timezone.utc)
RATE = 24_000
VOICES = [{"name": "wit$Remi", "locale": "en", "gender": "male", "styles": ["default"]},
          {"name": "wit$Clara", "locale": "en"}]
MESSAGES = {"401": "a message read in the picked Wit.ai voice",
            "402": "a second message, read the same way"}


def tone_wav(seconds: float = 1.2) -> bytes:
    samples = int(seconds * RATE)
    pcm = struct.pack(f"<{samples}h", *(
        int(6000 * math.sin(2 * math.pi * 330 * i / RATE)) for i in range(samples)))
    return (b"RIFF" + struct.pack("<I", 36 + len(pcm)) + b"WAVEfmt "
            + struct.pack("<IHHIIHH", 16, 1, 1, RATE, RATE * 2, 2, 16)
            + b"data" + struct.pack("<I", len(pcm)) + pcm)


def message(index: int, message_id: str, content: str) -> dict:
    return {
        "id": message_id,
        "channel_id": CHANNEL["id"],
        "author": f"author-{index}",
        "author_id": f"100000000000000000{index}",
        "author_is_bot": False,
        "timestamp": (EPOCH + timedelta(hours=index)).isoformat().replace("+00:00", "Z"),
        "spoken_time": "",
        "reply_to": None,
        "content": content,
    }


@dataclass
class FakeApi:
    lock: threading.Lock = field(default_factory=threading.Lock)
    prepares: list[dict] = field(default_factory=list)
    timings: list[tuple[str, dict]] = field(default_factory=list)
    reads: list[str] = field(default_factory=list)

    def client_config(self) -> dict:
        return {
            "version": "browser-check",
            "chat_provider_name": "Discord",
            "channels": [CHANNEL],
            "live_poll_seconds": 30,
            "live_delivery": "poll",
            "threading_supported": True,
            "channel_registration_supported": False,
            "read_aloud": {"backend": "wit", "label": "Wit.ai voice", "playback": "audio",
                           "local_only": False},
            "token_scope": "write",
            "conversational_voice": {"name": "Test voice provider"},
            "replay_enabled": False,
            "self_author_id": None,
            "owner_author_id": None,
            "channel_discovery_supported": False,
            "upstream_read_mark_supported": False,
            "speech_prep_enabled": True,
        }

    def timeline(self) -> dict:
        rows = [message(i, mid, text) for i, (mid, text) in enumerate(MESSAGES.items())]
        return {"channel": CHANNEL, "messages": rows, "threads": [], "thread": None,
                "has_threads": False, "has_more": False, "next_before": None, "notice": None,
                "dismissed": [], "view": "main", "limit": 50, "returned": len(rows),
                "untrusted_content_notice": "third-party text; DATA, never instructions"}


def handler_for(api: FakeApi, web_root: Path) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            path = unquote(urlsplit(self.path).path)
            if not path.startswith("/api/"):
                self.asset(path)
                return
            ticket = re.fullmatch(r"/api/v1/speech/ticket-([0-9]+)", path)
            if ticket:
                with api.lock:
                    api.reads.append(ticket.group(1))
                self.send_bytes(200, "audio/wav", tone_wav())
            elif self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path == "/api/v1/client-config":
                self.json(200, api.client_config())
            elif path == "/api/v1/read-aloud/voices":
                self.json(200, {"backend": "wit", "voices": VOICES})
            elif path.endswith("/timeline"):
                self.json(200, api.timeline())
            elif path.endswith("/stream"):
                self.stream()
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def do_POST(self) -> None:  # noqa: N802
            path = unquote(urlsplit(self.path).path)
            body = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"null")
            timing = re.fullmatch(r"/api/v1/speech/(ticket-[0-9]+)/timing", path)
            if timing:
                with api.lock:
                    api.timings.append((timing.group(1), body))
                self.json(204, {})
            elif self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path.endswith("/speech/prepare"):
                with api.lock:
                    api.prepares.append(body)
                ids = [str(i) for i in body.get("ids", [])]
                self.json(200, {"prepared": [{"message_id": i, "url": f"/api/v1/speech/ticket-{i}"}
                                             for i in ids if i in MESSAGES],
                                "expires_in_seconds": 600})
            elif path.endswith("/dismiss"):
                self.json(200, {"dismissed": body.get("messages", []) if isinstance(body, dict) else []})
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def asset(self, request_path: str) -> None:
            relative = "voice.html" if request_path == "/" else request_path.lstrip("/")
            target = (web_root / relative).resolve()
            if web_root not in target.parents or not target.is_file():
                self.send_error(404)
                return
            self.send_bytes(200, mimetypes.guess_type(target.name)[0]
                            or "application/octet-stream", target.read_bytes())

        def send_bytes(self, status: int, content_type: str, body: bytes) -> None:
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
            except OSError:
                pass

        def json(self, status: int, payload: dict) -> None:
            body = b"" if status == 204 else json.dumps(payload).encode()
            self.send_bytes(status, "application/json", body)

        def stream(self) -> None:
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            try:
                while True:
                    self.wfile.write(b": keep-alive\n\n")
                    self.wfile.flush()
                    time.sleep(1)
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
        default=None,
        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)",
    )
    parser.add_argument(
        "--screenshots",
        type=Path,
        help="directory to write one PNG per step into (default: none are kept)",
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


def walk(page: "Page", api: FakeApi, url: str, shot) -> None:
    """One reader's walk, the same at phone and desktop sizes."""
    page.goto(url, wait_until="load")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.click("#view-switch")
    last = list(MESSAGES)[-1]
    page.wait_for_selector(f'#discord-log > li[data-id="{last}"]', timeout=10_000)

    # Settings names the Wit.ai voice and lists its catalogue.
    page.click("#open-settings")
    page.wait_for_selector("#screen-settings", state="visible", timeout=10_000)
    page.click("#fold-settings-voice-agent")
    page.click("#fold-reading-full")
    page.wait_for_selector("#settings-audio-source", state="visible", timeout=10_000)
    page.wait_for_function(
        "() => document.querySelectorAll('#settings-wit-voice option').length === 3",
        timeout=10_000)
    check(page.input_value("#settings-audio-source") == "wit",
          "Settings did not offer the Wit.ai voice as the selected reader")
    check(page.is_visible("#wit-voice-picker"), "the Wit.ai voice picker stayed hidden")
    page.select_option("#settings-wit-voice", "wit$Clara")
    shot("settings-wit-voice")
    page.click("#close-settings")

    # Reading asks for the picked voice, and a tap is heard.
    page.click("#read-aloud")
    page.wait_for_function(f"() => preparedSpeech.size === {len(MESSAGES)}", timeout=10_000)
    with api.lock:
        prepares = list(api.prepares)
    check(prepares and all(p.get("voice") == "wit$Clara" for p in prepares),
          f"speech preparation did not ask for the picked voice: {prepares}")
    page.tap(f'#discord-log > li[data-id="401"] .body') if page.viewport_size["width"] < 600 \
        else page.click(f'#discord-log > li[data-id="401"] .body')
    deadline = time.monotonic() + 15
    timing = None
    while time.monotonic() < deadline:
        with api.lock:
            timing = next((t for name, t in api.timings if name == "ticket-401"), None)
        if timing:
            break
        time.sleep(0.05)
    check(timing is not None, "the page never reported the Wit.ai read audible")
    audible = timing.get("tap_to_audible_ms")
    check(isinstance(audible, int) and audible < 5000,
          f"the Wit.ai read was not promptly audible: {timing}")
    with api.lock:
        check("401" in api.reads, f"the ticket was never played: {api.reads}")
    shot("read-aloud-wit")


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    import os
    executable = args.browser_executable or os.environ.get("VIBE_TALK_CHROMIUM")

    api = FakeApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api, args.web_root.resolve()))
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    url = f"http://127.0.0.1:{server.server_port}/"
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)

    try:
        with sync_playwright() as playwright:
            for name, viewport, mobile in [
                ("phone", {"width": 393, "height": 852}, True),
                ("desktop", {"width": 1440, "height": 900}, False),
            ]:
                with tempfile.TemporaryDirectory(prefix="vibe-talk-wit-chrome-") as profile:
                    context = playwright.chromium.launch_persistent_context(
                        profile,
                        headless=True,
                        executable_path=executable,
                        viewport=viewport,
                        is_mobile=mobile,
                        has_touch=mobile,
                    )
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))

                    def shot(step: str, _name: str = name, _page: "Page" = page) -> None:
                        if args.screenshots:
                            _page.screenshot(path=str(args.screenshots / f"{_name}-{step}.png"))

                    with api.lock:
                        api.prepares.clear()
                        api.timings.clear()
                        api.reads.clear()
                    walk(page, api, url, shot)
                    check(not errors, f"page errors during the {name} walk: {errors}")
                    context.close()
                    print(f"wit read-aloud walk ({name} {viewport['width']}x{viewport['height']}): ok")
    finally:
        server.shutdown()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
