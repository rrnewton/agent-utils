#!/usr/bin/env python3
"""Settings kept on the server, in real Chromium on two devices at once.

`#457 settings-server-side`. Every setting outside Settings' Screen and keys section lives on
the server, shared by all the owner's devices; that one section stays on the device and says
"(client side)" beside its title. The fake-DOM suite cannot run two pages against one server,
so this opens two persistent browser contexts — two devices — against one fake API holding one
settings document, and walks the whole agreement:

  device A loads with a local choice already made -> the server is seeded from it, once ->
  device B loads fresh -> A's choice arrives and its checkbox follows, with no reload staged by
  hand -> B changes another setting at its own control's storage -> the change is pushed and A,
  still open, takes it from the live stream: its kept value and its checkbox both move ->
  a Screen and keys choice (text size) changed on A never reaches the server, and the section
  heading says "(client side)" small beside Screen and keys, with Collapsed message lines
  sitting under Messages, where it moved so that it syncs.

It serves the checked-out assets and the fake API from one loopback origin. Pass --web-root DIR
to run the same check against another copy of the page; against one from before the sync it
fails at the seed.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import queue
import tempfile
import threading
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, unquote, urlsplit

TOKEN = "write-token-browser-settings-sync-check"
CHANNEL = {"id": "1110000000000000001", "label": "lead team", "writable": True, "alias": None,
           "added": False}
QUESTION = {"id": "1110000000000000300", "channel_id": CHANNEL["id"], "author": "ci-bot",
            "author_id": "1110000000000000002", "author_is_bot": True,
            "timestamp": "2026-01-05T09:00:00Z", "spoken_time": "", "reply_to": None,
            "content": "Is the release branch green?"}
MARK_OWN = "vibe-talk.voice.mark-own-read"
COMBINE = "vibe-talk.voice.combine"
MSG_SCALE = "vibe-talk.voice.msg-scale"
Json = dict[str, object]


@dataclass
class FakeApi:
    """One server: one settings document, and the streams that carry its changes."""

    lock: threading.Lock = field(default_factory=threading.Lock)
    stopping: threading.Event = field(default_factory=threading.Event)
    settings: dict[str, tuple[str, int]] = field(default_factory=dict)
    clock: int = 1_700_000_000_000
    streams: list[queue.Queue[str]] = field(default_factory=list)
    writes: list[Json] = field(default_factory=list)

    def client_config(self) -> Json:
        return {
            "version": "browser-check",
            "chat_provider_name": "Discord",
            "channels": [CHANNEL],
            "live_poll_seconds": 30,
            "live_delivery": "push",
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

    def document(self) -> Json:
        with self.lock:
            return {"settings": [
                {"key": key, "value": value, "updated_ms": stamp}
                for key, (value, stamp) in sorted(self.settings.items())
            ]}

    def apply(self, body: Json) -> Json:
        """One page's write, the real server's rules: seed fills gaps only, set overwrites,
        this clock stamps, and what changed goes out on every open stream."""
        changed: list[Json] = []
        with self.lock:
            self.writes.append(body)
            self.clock += 1000
            stamp = self.clock
            for entry in body.get("seed") or []:
                assert isinstance(entry, dict)
                key, value = str(entry["key"]), entry.get("value")
                if key not in self.settings and isinstance(value, str):
                    self.settings[key] = (value, stamp)
                    changed.append({"key": key, "value": value, "updated_ms": stamp})
            for entry in body.get("set") or []:
                assert isinstance(entry, dict)
                key, value = str(entry["key"]), entry.get("value")
                if value is None:
                    if key in self.settings:
                        del self.settings[key]
                        changed.append({"key": key, "value": None, "updated_ms": stamp})
                elif self.settings.get(key, (None, 0))[0] != value:
                    self.settings[key] = (str(value), stamp)
                    changed.append({"key": key, "value": value, "updated_ms": stamp})
            held = {"settings": [
                {"key": key, "value": value, "updated_ms": held_stamp}
                for key, (value, held_stamp) in sorted(self.settings.items())
            ]}
            streams = list(self.streams)
        if changed:
            frame = "event: settings\ndata: " + json.dumps({"settings": changed}) + "\n\n"
            for stream in streams:
                stream.put(frame)
        return held

    def value_of(self, key: str) -> object:
        with self.lock:
            held = self.settings.get(key)
            return held[0] if held else None


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
            elif path == "/api/v1/client-settings":
                self.json(200, api.document())
            elif path.endswith("/stream"):
                self.stream()
            elif path.endswith("/timeline"):
                self.json(200, api.timeline(parse_qs(url.query).get("view", ["main"])[0]))
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def do_PUT(self) -> None:  # noqa: N802
            path = unquote(urlsplit(self.path).path)
            raw: object = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"null")
            if self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path == "/api/v1/client-settings" and isinstance(raw, dict):
                self.json(200, api.apply(raw))
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def stream(self) -> None:
            frames: queue.Queue[str] = queue.Queue()
            with api.lock:
                api.streams.append(frames)
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            try:
                while not api.stopping.is_set():
                    try:
                        frame = frames.get(timeout=1)
                    except queue.Empty:
                        frame = ": keep-alive\n\n"
                    self.wfile.write(frame.encode())
                    self.wfile.flush()
            except OSError:
                pass  # the page went away; its stream goes with it
            finally:
                with api.lock:
                    if frames in api.streams:
                        api.streams.remove(frames)

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
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=Path(__file__).resolve().parents[1] / "web",
                        help="the page assets to serve (default: this checkout's vibe-talk/web)")
    return parser.parse_args()


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import Page, sync_playwright
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
    url = f"http://127.0.0.1:{server.server_port}/"

    def kept(page: Page, key: str) -> object:
        return page.evaluate("(key) => localStorage.getItem(key)", key)

    def wait(page: Page, script: str, what: str) -> None:
        page.wait_for_function(script, timeout=15_000)

    try:
        with sync_playwright() as playwright:
            def device(profile: str, preset: dict[str, str]) -> tuple[object, Page, list[str]]:
                context = playwright.chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": 1280, "height": 800})
                page = context.pages[0]
                errors: list[str] = []
                page.on("pageerror", lambda error: errors.append(str(error)))
                script = "localStorage.setItem('vibe-talk.token', %s);" % json.dumps(TOKEN)
                for key, value in preset.items():
                    script += "localStorage.setItem(%s, %s);" % (json.dumps(key), json.dumps(value))
                page.add_init_script(script)
                page.goto(url, wait_until="load")
                return context, page, errors

            with tempfile.TemporaryDirectory(prefix="vibe-talk-device-a-") as profile_a, \
                    tempfile.TemporaryDirectory(prefix="vibe-talk-device-b-") as profile_b:
                # A loads with a choice already made locally: the migration seeds the server.
                context_a, page_a, errors_a = device(profile_a, {MARK_OWN: "0", MSG_SCALE: "1.25"})
                wait(page_a, "() => !document.getElementById('screen-main').hidden",
                     "device A did not reach the main screen")
                for _ in range(250):
                    if api.value_of(MARK_OWN) == "0":
                        break
                    page_a.wait_for_timeout(20)
                check(api.value_of(MARK_OWN) == "0",
                      f"the server was not seeded from device A's local value: {api.document()}")
                check(api.value_of(MSG_SCALE) is None,
                      f"a Screen and keys choice reached the server: {api.document()}")

                # B loads fresh: A's choice arrives and its control follows.
                context_b, page_b, errors_b = device(profile_b, {})
                wait(page_b, f"() => localStorage.getItem({json.dumps(MARK_OWN)}) === '0'",
                     "device B never took the server's value")
                check(page_b.evaluate("() => document.getElementById('mark-own-read').checked") is False,
                      "device B kept the value but its checkbox did not follow")

                # B changes a setting; A, still open, converges from the stream.
                page_b.evaluate("(key) => localStorage.setItem(key, '0')", COMBINE)
                wait(page_a, f"() => localStorage.getItem({json.dumps(COMBINE)}) === '0'",
                     "device A never converged on B's change")
                check(page_a.evaluate("() => document.getElementById('combine-messages').checked") is False,
                      "device A kept B's value but its checkbox did not follow")
                check(api.value_of(COMBINE) == "0", f"B's change was not pushed: {api.document()}")

                # The one per-device section says so, and collapsed lines sits with Messages.
                placed = page_a.evaluate("""() => {
                    const heading = document.getElementById('settings-screen-title');
                    const note = heading.querySelector('.settings-scope-note');
                    const sectionOf = (id) => document.getElementById(id)
                        .closest('.settings-section').id;
                    const style = note ? getComputedStyle(note) : null;
                    return {
                        note: note ? note.textContent : null,
                        noteSize: style ? parseFloat(style.fontSize) : null,
                        headingSize: parseFloat(getComputedStyle(heading).fontSize),
                        noteOpacity: style ? parseFloat(style.opacity) : null,
                        collapsedIn: sectionOf('fold-collapsed-lines'),
                    };
                }""")
                assert isinstance(placed, dict)
                check(placed["note"] == "(client side)", f"no (client side) beside Screen and keys: {placed}")
                check(float(placed["noteSize"]) < float(placed["headingSize"]),
                      f"the (client side) note is not small: {placed}")
                check(float(placed["noteOpacity"]) < 1, f"the (client side) note is not faint: {placed}")
                check(placed["collapsedIn"] == "settings-messages",
                      f"Collapsed message lines is not under Messages: {placed}")

                check(not errors_a and not errors_b, f"a page threw: A {errors_a} B {errors_b}")
                context_a.close()
                context_b.close()
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()
    print("two devices, one settings document: A seeded the server, B took A's choice on load, "
          "A converged on B's change from the stream, text size never left A, and Screen and keys "
          "says (client side) with Collapsed message lines under Messages")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
