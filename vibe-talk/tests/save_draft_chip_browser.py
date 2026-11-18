#!/usr/bin/env python3
"""`#1867 vibe-talk-save-draft-chip-covers-text`: the Save draft chip never
covers typed words, in real Chromium at 412x915 and 1280x800.

The chip stands over the box's top corner; the box keeps its room as
padding, so a long first line wraps before it reaches the chip. The check
is geometric: the chip's rect against the textarea's text area (its
content box, padding excluded), with a first line long enough to run
under where the chip stands.
"""
from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ = "savedraft-check-read-token-not-real"
WRITE = "savedraft-check-write-token-not-real"
LONG_LINE = (
    "Continuing the release notes from my phone — the rollout section "
    "still needs the numbers and the checklist owners before it goes out."
)


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def call(base: str, method: str, path: str, token: str, body: object = None) -> object:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method)
    req.add_header("Authorization", f"Bearer {token}")
    if data:
        req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, timeout=15) as r:
        return json.loads(r.read() or b"null")


def check(condition: bool, why: str) -> None:
    if not condition:
        raise AssertionError(why)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"))
    parser.add_argument("--screenshots", type=Path, default=Path("/tmp/save-draft-chip-shots"))
    args = parser.parse_args()
    from playwright.sync_api import sync_playwright

    args.screenshots.mkdir(parents=True, exist_ok=True)
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="savedraft-check-") as tmp:
        work = Path(tmp)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "savedraft-check-never-sent"\n'
            f'[auth]\nread_token = "{READ}"\nwrite_token = "{WRITE}"\n'
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
                    call(base, "GET", "/healthz", READ)
                    break
                except Exception:
                    check(time.time() < deadline, "server did not come up")
                    time.sleep(0.25)
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE,
                 {"text": "Ada shipped the fix."})
            with sync_playwright() as pw:
                for label, width, height in (("phone", 412, 915), ("desktop", 1280, 800)):
                    browser = pw.chromium.launch(executable_path=args.browser_executable)
                    ctx = browser.new_context(
                        viewport={"width": width, "height": height},
                        color_scheme="dark", has_touch=width < 600, is_mobile=width < 600)
                    page = ctx.new_page()
                    page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE)}); }} catch(e) {{}}")
                    page.goto(base + "/", wait_until="load")
                    try:
                        page.wait_for_selector("#welcome-start", state="visible", timeout=5_000)
                        page.click("#welcome-start")
                    except Exception:
                        pass
                    page.wait_for_selector("#channel-compose-text", state="visible", timeout=20_000)
                    box = page.locator("#channel-compose-text")
                    box.click()
                    box.fill(LONG_LINE)
                    page.wait_for_function(
                        "() => document.getElementById('channel-compose-text').value.length > 40",
                        timeout=20_000)
                    rects = page.evaluate("""() => {
                      const chip = document.getElementById('channel-save-draft').getBoundingClientRect();
                      const area = document.getElementById('channel-compose-text');
                      const b = area.getBoundingClientRect();
                      const cs = getComputedStyle(area);
                      const text = {
                        left: b.left + parseFloat(cs.borderLeftWidth) + parseFloat(cs.paddingLeft),
                        right: b.right - parseFloat(cs.borderRightWidth) - parseFloat(cs.paddingRight),
                        top: b.top + parseFloat(cs.borderTopWidth) + parseFloat(cs.paddingTop),
                        bottom: b.bottom - parseFloat(cs.borderBottomWidth) - parseFloat(cs.paddingBottom),
                      };
                      return { chip: {left: chip.left, right: chip.right, top: chip.top, bottom: chip.bottom}, text };
                    }""")
                    chip, text = rects["chip"], rects["text"]
                    overlap = not (
                        chip["right"] <= text["left"] or chip["left"] >= text["right"]
                        or chip["bottom"] <= text["top"] or chip["top"] >= text["bottom"])
                    check(not overlap,
                          f"{label}: the Save draft chip covers typed text: chip={chip} text={text}")
                    page.screenshot(path=str(args.screenshots / f"{label}-chip-clear.png"))
                    print(f"{label} at {width}x{height}: chip clear of the text area {chip} vs {text}")
                    ctx.close()
                    browser.close()
                # `#1871`: the compact keyboard layout on a phone — the
                # viewport shrunk as a keyboard shrinks it, the box focused.
                browser = pw.chromium.launch(executable_path=args.browser_executable)
                ctx = browser.new_context(
                    viewport={"width": 412, "height": 915},
                    color_scheme="dark", has_touch=True, is_mobile=True)
                page = ctx.new_page()
                page.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE)}); }} catch(e) {{}}")
                page.goto(base + "/", wait_until="load")
                try:
                    page.wait_for_selector("#welcome-start", state="visible", timeout=5_000)
                    page.click("#welcome-start")
                except Exception:
                    pass
                page.wait_for_selector("#channel-compose-text", state="visible", timeout=20_000)
                box = page.locator("#channel-compose-text")
                box.click()
                box.fill(LONG_LINE)
                page.set_viewport_size({"width": 412, "height": 915 - 336})
                page.wait_for_function(
                    "() => (window.visualViewport ? window.visualViewport.height : window.innerHeight) === 579",
                    timeout=20_000)
                page.wait_for_selector("#keyboard-send:not([hidden])", state="visible", timeout=20_000)
                rects = page.evaluate("""() => {
                  const read = (id) => {
                    const e = document.getElementById(id);
                    if (!e) return null;
                    const b = e.getBoundingClientRect();
                    return {left: b.left, right: b.right, top: b.top, bottom: b.bottom};
                  };
                  const chip = read('channel-save-draft');
                  const send = read('keyboard-send');
                  const area = document.getElementById('channel-compose-text');
                  const b = area.getBoundingClientRect();
                  const cs = getComputedStyle(area);
                  const text = {
                    left: b.left + parseFloat(cs.borderLeftWidth) + parseFloat(cs.paddingLeft),
                    right: b.right - parseFloat(cs.borderRightWidth) - parseFloat(cs.paddingRight),
                    top: b.top + parseFloat(cs.borderTopWidth) + parseFloat(cs.paddingTop),
                    bottom: b.bottom - parseFloat(cs.borderBottomWidth) - parseFloat(cs.paddingBottom),
                  };
                  return { chip, send, text };
                }""")
                text = rects["text"]
                for name in ("chip", "send"):
                    rect = rects[name]
                    check(rect is not None, f"phone keyboard open: no {name} stands over the box")
                    overlap = not (
                        rect["right"] <= text["left"] or rect["left"] >= text["right"]
                        or rect["bottom"] <= text["top"] or rect["top"] >= text["bottom"])
                    check(not overlap,
                          f"phone keyboard open: the {name} covers typed text: {rect} vs {text}")
                page.screenshot(path=str(args.screenshots / "phone-kbopen-chip-clear.png"))
                print(f"phone keyboard open: chip and keyboard Send clear of the text area {rects}")
                ctx.close()
                browser.close()
            print(f"save-draft chip browser: passed; shots in {args.screenshots}")
            return 0
        finally:
            server.terminate()


if __name__ == "__main__":
    sys.exit(main())
