#!/usr/bin/env python3
"""`#1843 vibe-talk-discuss-uncovered-messages-line` in a real Chromium.

The walk boots the real server on a loopback port (`--fake-discord`, a
throwaway store, and `[summaries.llm]` pointed at a loopback model stub:
the model is the only thing stubbed). Every Discuss route the feature uses
— preview, record and both kinds of Summarize — is the server's own.

It seeds non-owner messages through the test-only fake-seed route, with
current timestamps so the message cache holds them: Summarize, seed more,
see the next default bound start where the first summary ended, narrow
that bound to the newest message so a gap is left, then see and press the
"Summarize N messages not included since MM/DD" cleanup at the pane's
foot. The missed message alone is asked for, by id, and the line goes.

Condition waits only. `--screenshots DIR` keeps a PNG of each state;
`--all-themes` also walks the other two theme/size corners for that
matrix (the default gate walk covers one desk and one phone corner).
"""

import argparse
import json
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from datetime import datetime
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

from playwright.sync_api import Page, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "discuss-uncovered-read-token-not-a-real-credential"
WRITE_TOKEN = "discuss-uncovered-write-token-not-a-real-credential"
THREAD_ID = "walk-uncovered-thread"


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def free_port() -> int:
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return int(probe.getsockname()[1])


Json = dict[str, object]


def call(base: str, method: str, path: str, token: str, body: Json | None = None) -> Json:
    data = json.dumps(body).encode() if body is not None else None
    request = urllib.request.Request(base + path, data=data, method=method)
    request.add_header("Authorization", f"Bearer {token}")
    if data:
        request.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(request, timeout=15) as response:
        parsed: Json = json.loads(response.read() or b"null")
        return parsed


def hhmm(ms: int) -> str:
    return datetime.fromtimestamp(ms / 1000).strftime("%H:%M")


def month_day(ms: int) -> str:
    when = datetime.fromtimestamp(ms / 1000)
    return f"{when.month}/{when.day}"


class ModelStub(BaseHTTPRequestHandler):
    seen: list[Json] = []

    def do_POST(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
        raw = self.rfile.read(int(self.headers.get("Content-Length") or 0))
        ModelStub.seen.append(json.loads(raw or b"{}"))
        body = json.dumps({
            "content": [{"type": "text", "text": "Real-route summary of the covered messages."}],
            "stop_reason": "end_turn",
            "usage": {"input_tokens": 300, "output_tokens": 20},
        }).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args: object) -> None:
        pass


class Instance:
    """One throwaway real server with the model stubbed, and its base URL."""

    def __init__(self) -> None:
        self.model = ThreadingHTTPServer(("127.0.0.1", free_port()), ModelStub)
        self.model_port = int(self.model.server_address[1])
        threading.Thread(target=self.model.serve_forever, daemon=True).start()
        self.port = free_port()
        self.base = f"http://127.0.0.1:{self.port}"
        self.tmp = tempfile.TemporaryDirectory(prefix="discuss-uncovered-")
        work = Path(self.tmp.name)
        (work / "model-key").write_text("discuss-uncovered-stub-key\n")
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{self.port}"\n'
            '[discord]\nbot_token = "discuss-uncovered-bot-token-never-sent"\n'
            "live_poll_seconds = 5\n"
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\nserve = true\npath = "{work}/cache.sqlite3"\n'
            f'[summaries.llm]\ndialect = "anthropic"\n'
            f'base_url = "http://127.0.0.1:{self.model_port}"\n'
            f'api_key_file = "{work}/model-key"\n')
        self.server = subprocess.Popen(
            [str(ROOT / "target/release/vibe-talk"), "--config", str(work / "vibe-talk.toml"),
             "--fake-discord", "--skip-startup-probe"],
            cwd=work, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        deadline = time.time() + 30
        while True:
            try:
                call(self.base, "GET", "/healthz", READ_TOKEN)
                return
            except Exception:
                check(time.time() < deadline, "the walk server did not come up")
                time.sleep(0.25)

    def seed(self, text: str, at_ms: int, thread: str | None = None,
             root_id: str | None = None) -> str:
        body: Json = {"author": "alice", "text": text, "at_ms": at_ms}
        if thread is not None:
            body["thread_id"] = thread
            body["thread_root"] = root_id is None
            if root_id is not None:
                body["root_message_id"] = root_id
        answer = call(self.base, "POST", f"/api/v1/channels/{CHANNEL}/fake-seed",
                      WRITE_TOKEN, body)
        return str(answer["id"])

    def close(self) -> None:
        self.server.terminate()
        try:
            self.server.wait(timeout=10)
        except Exception:
            self.server.kill()
        self.model.shutdown()
        self.tmp.cleanup()


class Walk:
    """One browser corner's drive, recording every real Summarize body."""

    def __init__(self, page: Page, instance: Instance, shots: Path | None, tag: str) -> None:
        self.page = page
        self.instance = instance
        self.shots = shots
        self.tag = tag
        self.posts: list[Json] = []

        def record(route: object) -> None:
            request = route.request  # type: ignore[attr-defined]
            if request.method == "POST" and request.url.rstrip("/").endswith("/catchup"):
                self.posts.append(json.loads(request.post_data or "{}"))
            route.continue_()  # type: ignore[attr-defined]

        page.route("**/api/v1/channels/*/catchup", record)
        page.add_init_script(
            f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)});"
            " localStorage.setItem('vibe-talk.welcome.dismissed', '1'); } catch (e) {}")
        page.goto(instance.base + "/", wait_until="load")
        page.wait_for_selector("#view-switch", state="visible", timeout=20_000)
        for _ in range(3):
            if not page.locator("#discord-channel").is_hidden():
                break
            page.click("#view-switch")
            try:
                page.wait_for_selector("#discord-channel", state="visible", timeout=5_000)
                break
            except Exception:
                continue

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.tag}-{name}.png"))

    def open_discuss(self) -> None:
        if self.page.locator("#discuss-panel").is_visible():
            return
        for attempt in range(3):
            self.page.wait_for_function(
                "() => { const b = document.getElementById('discuss-open');"
                " return b && b.getAttribute('aria-disabled') !== 'true'; }",
                timeout=20_000)
            if self.page.locator("#discuss-open").is_visible():
                self.page.click("#discuss-open")
            else:
                if not self.page.locator("#discuss-menu-open").is_visible():
                    self.page.click("#action-menu-open")
                self.page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
                self.page.click("#discuss-menu-open")
            try:
                self.page.wait_for_selector("#discuss-panel", state="visible", timeout=10_000)
                return
            except Exception:
                if attempt == 2:
                    raise

    def wait_status(self, text: str, timeout: int = 30_000) -> None:
        self.page.wait_for_function(
            f"() => document.getElementById('discuss-status').textContent.includes({json.dumps(text)})",
            timeout=timeout)

    def reload_until_status(self, text: str, thread: bool = False) -> None:
        self.reload_until(
            f"() => document.getElementById('discuss-status').textContent.includes({json.dumps(text)})",
            thread)

    def reload_until(self, predicate: str, thread: bool = False) -> None:
        # A reload's first preview can race the timeline read that fills
        # the cache behind it; reload until the preview itself says so.
        for attempt in range(5):
            self.page.reload(wait_until="load")
            if thread:
                self.page.wait_for_selector("#thread-select", state="visible", timeout=20_000)
                self.page.select_option("#thread-select", f"thread:{THREAD_ID}")
            self.open_discuss()
            try:
                self.page.wait_for_function(predicate, timeout=10_000)
                return
            except Exception:
                if attempt == 4:
                    raise

    def scope(self, thread: bool, first_ms: int, gap_texts: tuple[str, str, str, str],
              label: str) -> None:
        """Summarize, seed a gap and a newest, narrow, see and press the line."""
        page = self.page

        def seed(text: str, at: int) -> str:
            if thread:
                return self.instance.seed(text, at, THREAD_ID, self.root_id)
            return self.instance.seed(text, at)
        if thread:
            self.root_id = self.instance.seed(gap_texts[0], first_ms, THREAD_ID)
            seed(gap_texts[1], first_ms + 10 * 60_000)
            page.reload(wait_until="load")
            page.wait_for_selector("#thread-select", state="visible", timeout=20_000)
            page.wait_for_function(
                f"() => [...document.getElementById('thread-select').options]"
                f".some(o => o.value === 'thread:{THREAD_ID}')", timeout=20_000)
            page.select_option("#thread-select", f"thread:{THREAD_ID}")
            page.wait_for_function(
                "() => document.querySelector('.discuss-title, #thread-heading') !== null",
                timeout=20_000)
        else:
            seed(gap_texts[0], first_ms)
            seed(gap_texts[1], first_ms + 10 * 60_000)
            page.reload(wait_until="load")
            page.wait_for_selector("#discord-channel", state="visible", timeout=20_000)
        self.open_discuss()
        self.wait_status("2 unread ready")
        check(page.locator("#discuss-uncovered").is_hidden(),
              f"{label}: the uncovered line showed before any summary")
        self.shot(f"{label}-01-first-bound")
        page.click("#discuss-summarize")
        self.wait_status("Read ")
        check("message_ids" not in self.posts[-1],
              f"{label}: the first summary named an explicit set")
        check(self.posts[-1].get("since_ms") == first_ms,
              f"{label}: the first summary did not start at the scope start: {self.posts[-1]}")
        self.shot(f"{label}-02-first-summary")

        missed_ms = first_ms + 30 * 60_000
        newest_ms = first_ms + 40 * 60_000
        missed_id = seed(gap_texts[2], missed_ms)
        seed(gap_texts[3], newest_ms)
        self.reload_until("() => document.getElementById('discuss-count').value === '3'", thread)
        check(page.locator("#discuss-count").input_value() == "3",
              f"{label}: the second default bound did not start where the first ended "
              f"(count {page.locator('#discuss-count').input_value()})")
        check(page.locator("#discuss-since").input_value() == hhmm(first_ms + 10 * 60_000),
              f"{label}: the second default bound's since was "
              f"{page.locator('#discuss-since').input_value()}, not the first summary's end")
        self.shot(f"{label}-03-second-bound")
        page.locator("#discuss-count").fill("1")
        page.click("#discuss-summarize")
        self.wait_status("Read ")
        self.shot(f"{label}-04-newest-only-summary")

        self.reload_until(
            "() => !document.getElementById('discuss-uncovered').hidden", thread)
        page.wait_for_selector("#discuss-uncovered", state="visible", timeout=30_000)
        expected = f"1 message not included since {month_day(first_ms)}"
        check(page.locator("#discuss-uncovered-text").text_content() == expected,
              f"{label}: the uncovered line said "
              f"{page.locator('#discuss-uncovered-text').text_content()!r}, not {expected!r}")
        page.locator("#discuss-uncovered").scroll_into_view_if_needed()
        self.shot(f"{label}-05-gap-line")
        viewport = page.viewport_size
        if viewport is not None and viewport["width"] == 412:
            # The keyboard shot stands with the Ask field focused, as a
            # reader mid-question would have it.
            page.locator("#discuss-followup").click()
            # The phone corner with the keyboard really open: the viewport
            # shrinks by the keyboard's 336px and the visual viewport with it.
            page.set_viewport_size({"width": 412, "height": viewport["height"] - 336})
            page.wait_for_function(
                "() => (window.visualViewport ? window.visualViewport.height : window.innerHeight)"
                f" === {viewport['height'] - 336}", timeout=20_000)
            # Focus stays in the Ask field; the pane scrolls to its foot,
            # where the line stands and the field is still in view.
            page.evaluate(
                "() => { const p = document.getElementById('discuss-panel');"
                " p.scrollTop = p.scrollHeight; }")
            self.shot(f"{label}-05b-gap-line-keyboard-open")
            page.set_viewport_size(viewport)
            page.wait_for_function(
                "() => (window.visualViewport ? window.visualViewport.height : window.innerHeight)"
                f" === {viewport['height']}", timeout=20_000)
        page.click("#discuss-uncovered-go")
        page.wait_for_selector("#discuss-uncovered", state="hidden", timeout=30_000)
        check(self.posts[-1].get("message_ids") == [missed_id],
              f"{label}: the cleanup did not name exactly the missed message: {self.posts[-1]}")
        check(self.posts[-1].get("since_ms") == missed_ms,
              f"{label}: the cleanup did not start at the missed message")
        check(gap_texts[2] in json.dumps(ModelStub.seen[-1]),
              f"{label}: the model was not given the missed message")
        self.shot(f"{label}-06-cleaned")

    root_id: str | None = None


def walk(page: Page, shots: Path | None, tag: str) -> None:
    instance = Instance()
    try:
        drive = Walk(page, instance, shots, tag)
        now = int(time.time() * 1000)
        drive.scope(False, now - 240 * 60_000, (
            "Main first note: the rollout order for the cache change.",
            "Main second note: who reviews the rollout order.",
            "Main missed note: the one nobody summarized between the two.",
            "Main newest note: the latest word on the rollout.",
        ), "main")
        drive.root_id = None
        drive.scope(True, now - 120 * 60_000, (
            "Thread alpha root: the alpha plan starts here.",
            "Thread alpha reply: drafting the alpha steps.",
            "Thread alpha missed: the alpha risk nobody summarized.",
            "Thread alpha newest: the latest alpha word.",
        ), "thread")
    finally:
        instance.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--screenshots", type=Path, default=None,
                        help="directory to keep a PNG of each state in")
    parser.add_argument("--all-themes", action="store_true",
                        help="also walk the other two theme/size corners")
    args = parser.parse_args()
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    ModelStub.seen = []
    corners = [("desktop-dark", 1280, 800, 1.0, False, "dark"),
               ("phone-light", 412, 915, 2.6, True, "light")]
    if args.all_themes:
        corners += [("desktop-light", 1280, 800, 1.0, False, "light"),
                    ("phone-dark", 412, 915, 2.6, True, "dark")]
    with sync_playwright() as pw:
        browser = pw.chromium.launch()
        for tag, width, height, scale, touch, scheme in corners:
            context = browser.new_context(
                viewport={"width": width, "height": height}, device_scale_factor=scale,
                has_touch=touch, is_mobile=touch, color_scheme="dark" if scheme == "dark" else "light")
            walk(context.new_page(), args.screenshots, tag)
            context.close()
        browser.close()
    print("discuss uncovered browser: real-route seed -> summarize -> gap -> cleanup walks passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
