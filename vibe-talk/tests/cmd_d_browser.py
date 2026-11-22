#!/usr/bin/env python3
"""Cmd+D dock/float (`#1978 vibe-talk-cmd-d-dock-toggle`) in a real Chromium.

Run against the built release binary (deps: build.release):

    python3 tests/cmd_d_browser.py [--screenshots DIR]

The walk serves the real server (`--fake-discord`, `--skip-startup-probe`)
from a throwaway config on a loopback port the kernel picks, seeds two
messages through its own write API, and answers only Discuss's catchup
preview/record and client-config's `discuss_available` at the network edge
(a real Discuss deployment's capability; the model is nobody's to run in a
walk). Everything else — sign-in, the channel, the quote-reply chip, the
Discuss pane and every key — is the shipped page.

At a desk (1280x800) and on a phone (412x915), in the off-Mac chord
(Ctrl+D) and in a second desk tab whose `navigator.platform` is emulated
as a Mac's (Cmd+D, the keyboard_browser.py precedent):

  * outside both surfaces (focus in the channel composer) the plain chord
    does nothing: no surface opens, docks or floats, and the words typed
    stay;
  * Discuss: focus in its follow-up composer, the Shift variant does
    nothing, the plain chord docks the floating pane (data-mode docked)
    and again floats it (overlay);
  * the quote reply: words selected in a message, the chip opens the
    floating box, focus in its composer; the other platform's chord and
    the Shift variant do nothing, the plain chord docks it under its
    message (the stream reply, focused, quote and words carried), and
    again in that composer floats it back out (quote shown, words kept,
    focus kept).

Condition waits only — no fixed sleeps in any assertion.
"""

from __future__ import annotations

import argparse
import json
import socket
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import Page, Route, sync_playwright
import browser_waits

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "cmd-d-check-read-token-not-a-real-credential"
WRITE_TOKEN = "cmd-d-check-write-token-not-a-real-credential"
QUOTE_WORDS = "checking the runner logs carefully"
LONG_TEXT = (
    "Ada shipped the fix after checking the runner logs carefully "
    "and restarting the queue for the lead team this morning."
)


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port: int = sock.getsockname()[1]
        return port


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


def wire(page: Page, base: str) -> None:
    """Only Discuss's model answers and its capability are stubbed, at the edge."""

    def client_config(route: Route) -> None:
        body = call(base, "GET", "/api/v1/client-config", WRITE_TOKEN)
        body["discuss_available"] = True
        route.fulfill(status=200, content_type="application/json",
                      body=json.dumps(body))

    def preview(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "channel": {"id": CHANNEL, "label": "lead team"},
            "thread_id": None, "view": "flat",
            "window": {"count": 2, "unread_count": 1, "read_count": 1,
                       "from_id": "100000000000000101", "through_id": "100000000000000102",
                       "truncated": False, "noise_hidden": 0},
            "unread": [{"id": "100000000000000101", "at_ms": 1760000000000, "label": "09:51 · Ada"}],
        }))

    def record(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "exists": False, "thread_id": None, "current_summary": None,
            "timeline": [], "staleness": None,
        }))

    page.route("**/api/v1/client-config", client_config)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/preview*", preview)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/record*", record)
    page.add_init_script(
        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
    page.add_init_script(
        "try { localStorage.setItem('vibe-talk.welcome.dismissed', '1'); } catch (e) {}")


def to_channel(page: Page, base: str) -> None:
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
    page.wait_for_selector("#discord-log [data-id]", state="visible", timeout=20_000)


def mode(page: Page) -> str:
    return page.evaluate(
        "() => document.getElementById('discuss-panel').getAttribute('data-mode')") or ""


def active_id(page: Page) -> str:
    return page.evaluate("() => (document.activeElement || {}).id || ''") or ""


def shot(page: Page, shots: Path | None, name: str) -> None:
    if shots is not None:
        page.screenshot(path=str(shots / f"{name}.png"))


def walk(page: Page, base: str, mod: str, other_mod: str, where: str,
         shots: Path | None, tag: str) -> None:
    """One Cmd+D walk. `mod` is this keyboard's plain chord modifier."""
    chord = f"{mod}+d"
    shifted = f"{mod}+Shift+d"
    other = f"{other_mod}+d"
    to_channel(page, base)

    # Negative: in the channel composer the plain chord does nothing new.
    page.click("#channel-compose-text")
    page.fill("#channel-compose-text", "outside the surfaces")
    page.keyboard.press(chord)
    check(page.locator("#quick-reply").is_hidden(), f"{where}: the chord outside opened a quick reply")
    check(page.locator("#discuss-panel").is_hidden(), f"{where}: the chord outside opened Discuss")
    check(page.locator("#reply-text").is_hidden(), f"{where}: the chord outside docked a reply")
    check(page.input_value("#channel-compose-text") == "outside the surfaces",
          f"{where}: the chord outside touched the channel draft")

    # Discuss: floating -> docked -> floating, focus in its follow-up composer.
    if page.locator("#discuss-open").is_visible():
        page.click("#discuss-open")
    else:
        page.click("#action-menu-open")
        page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
        page.click("#discuss-menu-open")
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
    check(mode(page) == "overlay", f"{where}: Discuss did not open floating (mode {mode(page)})")
    shot(page, shots, f"{tag}-discuss-floating")
    page.click("#discuss-followup")
    page.keyboard.press(shifted)
    check(mode(page) == "overlay", f"{where}: the Shift variant docked Discuss")
    page.keyboard.press(other)
    check(mode(page) == "overlay", f"{where}: the other platform's chord docked Discuss")
    page.keyboard.press(chord)
    page.wait_for_function(
        "() => document.getElementById('discuss-panel').getAttribute('data-mode') === 'docked'",
        timeout=20_000)
    shot(page, shots, f"{tag}-discuss-docked")
    page.click("#discuss-followup")
    page.keyboard.press(chord)
    page.wait_for_function(
        "() => document.getElementById('discuss-panel').getAttribute('data-mode') === 'overlay'",
        timeout=20_000)
    page.click("#discuss-close")
    page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)

    # Quote reply: floating -> docked -> floating, focus in the composer.
    selected = page.evaluate("""() => {
      const rows = [...document.querySelectorAll('#discord-log [data-id]')];
      const row = rows.find((r) => r.textContent.includes('checking the runner logs carefully'));
      if (!row) return false;
      const walker = document.createTreeWalker(row, NodeFilter.SHOW_TEXT);
      let node = walker.nextNode();
      while (node) {
        const at = node.data.indexOf('checking the runner logs carefully');
        if (at >= 0) {
          const range = document.createRange();
          range.setStart(node, at);
          range.setEnd(node, at + 'checking the runner logs carefully'.length);
          const sel = getSelection();
          sel.removeAllRanges();
          sel.addRange(range);
          return true;
        }
        node = walker.nextNode();
      }
      return false;
    }""")
    check(bool(selected), f"{where}: the quoteable message was not found for selection")
    page.wait_for_selector("#selection-chip", state="visible", timeout=20_000)
    page.click("#selection-chip")
    page.wait_for_selector("#quick-reply", state="visible", timeout=20_000)
    check(active_id(page) == "quick-reply-text",
          f"{where}: the quick reply did not take the focus (on {active_id(page)})")
    page.fill("#quick-reply-text", "dock me")
    shot(page, shots, f"{tag}-quote-floating")
    page.click("#quick-reply-text")
    page.keyboard.press(shifted)
    check(page.locator("#quick-reply").is_visible(), f"{where}: the Shift variant docked the quick reply")
    page.keyboard.press(other)
    check(page.locator("#quick-reply").is_visible(),
          f"{where}: the other platform's chord docked the quick reply")
    page.keyboard.press(chord)
    page.wait_for_selector("#quick-reply", state="hidden", timeout=20_000)
    page.wait_for_selector("#reply-text", state="visible", timeout=20_000)
    docked = page.input_value("#reply-text")
    check(QUOTE_WORDS in docked and "dock me" in docked,
          f"{where}: the docked reply lost its quote or words: {docked!r}")
    check(active_id(page) == "reply-text",
          f"{where}: docking did not keep the focus in the composer (on {active_id(page)})")
    shot(page, shots, f"{tag}-quote-docked")
    page.keyboard.press(chord)
    page.wait_for_selector("#quick-reply", state="visible", timeout=20_000)
    check(page.locator("#reply-text").is_hidden(), f"{where}: floating left the reply docked as well")
    check(page.input_value("#quick-reply-text") == "dock me",
          f"{where}: the floated box lost what was written")
    quote = page.text_content("#quick-reply-quote") or ""
    check(QUOTE_WORDS in quote, f"{where}: the floated box lost its quote: {quote!r}")
    check(active_id(page) == "quick-reply-text",
          f"{where}: floating did not keep the focus in the composer (on {active_id(page)})")
    shot(page, shots, f"{tag}-quote-refloated")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--screenshots", type=Path, default=None,
                        help="directory for a PNG of each dock/float state")
    args = parser.parse_args()
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="cmd-d-check-") as tmp:
        work = Path(tmp)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "cmd-d-check-bot-token-never-sent"\n'
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
                 {"text": LONG_TEXT})
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                 {"text": "Ben is reviewing it."})

            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                desk = browser.new_context(viewport={"width": 1280, "height": 800},
                                            color_scheme="dark")
                browser_waits.hot_host_timeouts(desk)
                desk_page = desk.new_page()
                browser_waits.hot_host_timeouts(desk_page)
                wire(desk_page, base)
                walk(desk_page, base, "Control", "Meta", "desk (Ctrl+D)", args.screenshots, "desk")
                # The same on a Mac, emulated as keyboard_browser.py emulates it:
                # plain Cmd+D acts, plain Ctrl+D in the box is the box's.
                mac_page = desk.new_page()
                browser_waits.hot_host_timeouts(mac_page)
                mac_cdp = desk.new_cdp_session(mac_page)
                agent = str(desk_page.evaluate("() => navigator.userAgent"))
                mac_cdp.send("Emulation.setUserAgentOverride",
                             {"userAgent": agent, "platform": "MacIntel"})
                wire(mac_page, base)
                walk(mac_page, base, "Meta", "Control", "desk Mac-emulated (Cmd+D)",
                     args.screenshots, "mac")
                desk.close()
                phone = browser.new_context(viewport={"width": 412, "height": 915},
                                             has_touch=True, is_mobile=True, color_scheme="light")
                browser_waits.hot_host_timeouts(phone)
                phone_page = phone.new_page()
                browser_waits.hot_host_timeouts(phone_page)
                wire(phone_page, base)
                walk(phone_page, base, "Control", "Meta", "phone (Ctrl+D)", args.screenshots, "phone")
                phone.close()
                browser.close()
            print("cmd-d browser: desk Ctrl+D, desk Mac-emulated Cmd+D and phone Ctrl+D "
                  "walks passed (quote reply and Discuss, docked<->floating, negative outside)")
            return 0
        finally:
            server.terminate()


if __name__ == "__main__":
    raise SystemExit(main())
