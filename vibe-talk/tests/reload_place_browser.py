#!/usr/bin/env python3
"""A reload keeps the view and the scroll place (`#1849
vibe-talk-reload-keeps-view-and-place`), against the REAL server.

The owner (thread 187): a refresh after reading mid-channel — most
recently after a search — put them back at the bottom of the Main view.
On this threadless channel (the fake Discord upstream has no threads,
so Main is the whole channel) the test seeds a tall channel through the
real reply route, reads mid-channel, filters by a search, scrolls, and
reloads: the view stays Main on the same channel and the top-visible
message (the session scroll anchor, never in the URL) is where it was.
No wall-clock waits: every step waits on a selector or a page condition.
"""

import json
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path
from typing import cast

from playwright.sync_api import Page, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "reload-place-read-token-not-a-real-credential"
WRITE_TOKEN = "reload-place-write-token-not-a-real-credential"
COUNT = 40

TOP_VISIBLE = """() => {
  const area = document.getElementById('scroll-area');
  const edge = area.getBoundingClientRect().top;
  for (const li of document.getElementById('discord-log').children) {
    if (li.getBoundingClientRect().bottom > edge) {
      return { id: li.getAttribute('data-id'), top: li.getBoundingClientRect().top,
               url: location.search, view: document.getElementById('thread-select').value,
               channel: document.getElementById('discord-channel').value };
    }
  }
  return null;
}"""


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def free_port() -> int:
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = int(sock.getsockname()[1])
    sock.close()
    return port


def call(base: str, method: str, path: str, token: str,
         body: dict[str, object] | None = None) -> dict[str, object]:
    req = urllib.request.Request(
        base + path, method=method,
        data=json.dumps(body).encode() if body is not None else None,
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as resp:
        return cast(dict[str, object], json.loads(resp.read().decode() or "{}"))


def walk(page: Page, base: str, label: str) -> None:
    page.goto(base + "/", wait_until="load")
    try:
        page.wait_for_function(
            "() => !document.getElementById('screen-welcome').hidden || "
            "!document.getElementById('screen-main').hidden", timeout=3_000)
        if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
            page.click("#welcome-start")
    except Exception:
        pass
    page.wait_for_selector("#screen-main:not([hidden])", timeout=20_000)
    page.click("#view-switch")
    page.click("#view-switch")
    page.wait_for_selector("#pane-voice:not([hidden])", timeout=20_000)
    page.click("#view-switch")
    page.wait_for_selector("#pane-discord:not([hidden])", timeout=20_000)
    page.wait_for_function(
        "() => document.getElementById('discord-log').children.length >= 10",
        timeout=20_000)
    # Read mid-channel, then filter with a search, as the owner did.
    page.evaluate("""() => {
      const area = document.getElementById('scroll-area');
      area.scrollTop = Math.round((area.scrollHeight - area.clientHeight) * 0.5);
      area.dispatchEvent(new Event('scroll'));
    }""")
    page.wait_for_function(
        "() => document.getElementById('scroll-area').scrollTop > 0", timeout=20_000)
    page.click("#search-toggle")
    page.wait_for_selector("#search-field:not([hidden])", timeout=20_000)
    search = page.locator("#search-field")
    search.fill("message 2")
    page.wait_for_function(
        "() => [...document.getElementById('discord-log').children]"
        ".some((li) => li.className.includes('search-hidden'))", timeout=20_000)
    search.fill("")
    page.wait_for_function(
        "() => ![...document.getElementById('discord-log').children]"
        ".some((li) => li.className.includes('search-hidden'))", timeout=20_000)
    page.evaluate("() => document.getElementById('scroll-area').dispatchEvent(new Event('scroll'))")
    before = page.evaluate(TOP_VISIBLE)
    check(before and before["id"], f"{label}: no top-visible message before reload")
    # A beat for the throttled scroll-place note, waited on by condition:
    # the session record must hold this anchor before the reload.
    page.wait_for_function(
        """(id) => {
          document.getElementById('scroll-area').dispatchEvent(new Event('scroll'));
          try {
            const raw = sessionStorage.getItem('vibe-talk.voice.ui-state');
            return raw !== null && raw.includes(id);
          } catch (e) { return false; }
        }""", arg=before["id"], timeout=20_000)
    page.reload(wait_until="load")
    page.wait_for_selector("#pane-discord:not([hidden])", timeout=20_000)
    page.wait_for_function(
        "() => document.getElementById('discord-log').children.length >= 10",
        timeout=20_000)
    after = page.evaluate(TOP_VISIBLE)
    check(after["channel"] == before["channel"],
          f"{label}: the reload changed channel: {before} -> {after}")
    check(after["view"] == before["view"],
          f"{label}: the reload changed view: {before} -> {after}")
    check(after["id"] == before["id"],
          f"{label}: the reload lost the scroll anchor: {before} -> {after}")
    check(abs(after["top"] - before["top"]) <= 2,
          f"{label}: the anchor moved: {before} -> {after}")
    check("scroll" not in after["url"],
          f"{label}: the scroll place leaked into the URL: {after['url']}")


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="reload-place-") as tmp:
        work = Path(tmp)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "reload-place-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n')
        server = subprocess.Popen(
            [str(ROOT / "target/release/vibe-talk"), "--config", str(work / "vibe-talk.toml"),
             "--fake-discord", "--skip-startup-probe"],
            cwd=work, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        try:
            deadline = time.time() + 30
            while True:
                try:
                    call(base, "GET", "/healthz", READ_TOKEN)
                    break
                except Exception:
                    check(time.time() < deadline, "the server did not come up")
                    time.sleep(0.25)
            for n in range(1, COUNT + 1):
                call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                     {"text": f"reload place message {n} with enough words to take a line or two on a phone screen"})
            with sync_playwright() as pw:
                browser = pw.chromium.launch(args=["--autoplay-policy=no-user-gesture-required"])
                for (width, height, touch, label) in [
                        (412, 915, True, "phone"), (1280, 800, False, "desktop")]:
                    ctx = browser.new_context(
                        viewport={"width": width, "height": height},
                        has_touch=touch, is_mobile=touch)
                    page = ctx.new_page()
                    page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }}"
                        " catch (e) {}")
                    walk(page, base, label)
                    ctx.close()
                browser.close()
        finally:
            server.terminate()
    print("reload place check: view, channel and scroll anchor survive a reload")
    return 0


if __name__ == "__main__":
    sys.exit(main())
