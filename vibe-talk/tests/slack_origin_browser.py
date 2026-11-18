#!/usr/bin/env python3
"""`#1847 vibe-talk-slack-communication-audit`: the page only talks to its own server.

Real Chromium against the real server binary (its shipped routes, an in-memory
fake chat upstream behind it — no route is stubbed at the network edge). Every
request the page makes is recorded; the walk signs in, opens the channel, reads
it and sends a message from the composer, then asserts every recorded request
went to the server's own origin and none names Slack. A direct client-to-Slack
path (a Slack API host in the bundle, a fetch or socket to Slack) fails here.
Condition waits only.
"""

import json
import socket
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "slack-origin-check-read-token-not-a-real-credential"
WRITE_TOKEN = "slack-origin-check-write-token-not-a-real-credential"


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def call(base: str, method: str, path: str, token: str,
         body: dict[str, object] | None = None) -> dict[str, object]:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method)
    req.add_header("Authorization", f"Bearer {token}")
    if data:
        req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, timeout=15) as res:
        parsed: object = json.loads(res.read() or b"null")
    assert isinstance(parsed, dict)
    return parsed


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="slack-origin-check-") as tmp:
        work = Path(tmp)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "slack-origin-check-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
        )
        server = subprocess.Popen(
            [str(ROOT / "target/release/vibe-talk"), "--config", str(work / "vibe-talk.toml"),
             "--fake-discord", "--skip-startup-probe"],
            cwd=work, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        try:
            deadline = time.time() + 30
            while True:
                try:
                    call(base, "GET", "/healthz", READ_TOKEN)
                    break
                except Exception:
                    check(time.time() < deadline, "the server did not come up")
                    time.sleep(0.25)
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                 {"text": "Ada shipped the fix."})

            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                context = browser.new_context(viewport={"width": 412, "height": 915})
                page = context.new_page()
                seen: list[str] = []
                page.on("request", lambda request: seen.append(request.url))
                errors: list[str] = []
                page.on("pageerror", lambda e: errors.append(str(e)))
                page.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                page.add_init_script(
                    "try { localStorage.setItem('vibe-talk.welcome.dismissed', '1'); } catch (e) {}")
                page.goto(base + "/", wait_until="load")
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
                page.wait_for_selector("#discord-channel", state="visible", timeout=20_000)
                page.wait_for_function(
                    "() => document.body.innerText.includes('Ada shipped the fix.')",
                    timeout=20_000)
                # Send from the composer: the page's exact send request.
                page.wait_for_selector("#channel-compose-text", state="visible", timeout=20_000)
                page.fill("#channel-compose-text", "sent from the page in the origin walk")
                page.click("#channel-send")
                page.wait_for_function(
                    "() => document.body.innerText.includes('sent from the page in the origin walk')",
                    timeout=20_000)
                check(not errors, f"the page threw: {errors}")
                browser.close()

            check(bool(seen), "the walk recorded no requests")
            off_origin = [url for url in seen if not url.startswith(base + "/") and url != base + "/"]
            check(not off_origin, f"the page contacted another origin: {off_origin}")
            slack = [url for url in seen if "slack" in url]
            check(not slack, f"the page contacted Slack: {slack}")
            print(f"slack origin browser: {len(seen)} requests, all to {base}; "
                  "channel read and composer send passed")
            return 0
        finally:
            server.terminate()


if __name__ == "__main__":
    raise SystemExit(main())
