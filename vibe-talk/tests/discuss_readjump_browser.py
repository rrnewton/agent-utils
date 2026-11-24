#!/usr/bin/env python3
"""One click from a Discuss summary to reading the linked message (`#1959`).

`#1959 vibe-talk-reader-one-click-jump`. The owner, 2026-11-20: while the
reader reads a summary aloud, clicking a message linked from it should, in
that one click, put the top of the message on screen, expand it, and start
reading it from its start. Before the fix the click only landed the message;
reading it took another tap on its text.

This serves the real server (fake chat upstream) with 120 seeded messages,
so the citation target stands outside the loaded window (the seek walk must
load it). The catchup routes are answered at the network edge with
contract-shaped bodies and the device voice is a recording stub, as in
tests/discuss_link_browser.py and tests/discuss_browser.py; everything else
— sign-in, the channel, the panel, the jump, the reader — is the shipped
page. At 412x915 and 1280x800:

  reader OFF: the citation click lands the message and starts no read, as
  before -> the summary read starts (#read-aloud, device voice) -> ONE click
  on the citation: condition waits then assert the reader is reading THAT
  message (nowPlaying id), the row's top is on screen above the Discuss
  sheet, the row is expanded, the device voice spoke the message's text, and
  the page logged its click-to-reading-start timing ([readjump]).

Condition waits only (owner rule 346). Pass --screenshots DIR to keep PNGs.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path
from typing import TypedDict

from playwright.sync_api import Page, Route, sync_playwright
import browser_waits

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "readjump-check-read-token-not-a-real-credential"
WRITE_TOKEN = "readjump-check-write-token-not-a-real-credential"
STAMP = {"context_ms": 1, "model_ms": 1, "first_token_ms": None, "total_ms": 2,
         "input_tokens": None, "output_tokens": None, "input_chars": 10, "output_chars": 10}
WINDOW = {"count": 120, "unread_count": 2, "read_count": 118,
          "from_id": "x", "through_id": "y", "truncated": False, "noise_hidden": 0}

DEVICE_VOICE = """(() => {
  const spoken = [];
  window.deviceSpeech = spoken;
  const voice = {name: 'Check voice', lang: 'en-US', localService: true, default: true, voiceURI: 'check'};
  const engine = {
    paused: false, speaking: false, pending: false,
    getVoices: () => [voice], addEventListener() {}, removeEventListener() {},
    speak(utterance) {
      const entry = {text: String(utterance.text), started: null, ended: null, cancelled: false};
      spoken.push(entry);
      setTimeout(() => {
        if (entry.cancelled) return;
        entry.started = performance.now();
        if (utterance.onstart) utterance.onstart({});
        setTimeout(() => {
          if (entry.cancelled) return;
          entry.ended = performance.now();
          if (utterance.onend) utterance.onend({});
        }, 400 * entry.text.length);
      }, 30);
    },
    cancel() { for (const entry of spoken) if (entry.ended === null) entry.cancelled = true; },
    pause() { this.paused = true; },
    resume() { this.paused = false; },
  };
  Object.defineProperty(window, 'speechSynthesis', {value: engine, configurable: true});
  window.SpeechSynthesisUtterance = function (text) { this.text = text; };
})();"""


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def free_port() -> int:
    import socket
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


class LandedGeo(TypedDict):
    top: float
    bottom: float
    vh: float
    panelTop: float | None
    sheetSpansList: bool
    collapsed: str | None
    closedDetails: int
    text: str


def citation(target: str, label: str) -> str:
    return (f"<a href=\"?c={CHANNEL}&v=all&m=discord:{CHANNEL}:{target}\" "
            f"target=\"_blank\" rel=\"noopener noreferrer nofollow\">({label} ↩)</a>")


def saved_answer(text_html: str) -> dict[str, object]:
    return {
        "backend": "fake", "model": "fake-model", "view": "flat", "since_ms": None,
        "created_ms": 1760000099999, "window": WINDOW,
        "covered_ids": [], "markable_ids": [], "cited_ids": [], "fresh_ids": [],
        "footer_links": [], "text": "Old news landed.", "text_html": text_html,
        "unknown_terms": [], "stamp": STAMP,
    }


def walk(page: Page, base: str, label: str, shots: Path | None) -> None:
    targets: dict[str, str] = {}
    console_lines: list[str] = []
    page.on("console", lambda msg: console_lines.append(msg.text))

    def record(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "exists": True, "thread_id": None,
            "current_summary": saved_answer(
                "<h2>Progress</h2><ul><li>Old news landed "
                + citation(targets["answer"], "09:12") + "</li></ul>"),
            "timeline": [{
                "kind": "summary",
                "summary": saved_answer(
                    "<p>Earlier point " + citation(targets["timeline"], "09:11") + "</p>"),
            }],
            "staleness": None,
        }))

    def preview(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "channel": {"id": CHANNEL, "label": "lead team"},
            "thread_id": None, "view": "flat", "window": WINDOW,
            "unread": [
                {"id": "100000000000000101", "at_ms": 1760000000000, "label": "09:51 · Ada"},
                {"id": "100000000000000102", "at_ms": 1760000060000, "label": "09:52 · Ben"},
            ],
        }))

    def client_config(route: Route) -> None:
        body = call(base, "GET", "/api/v1/client-config", WRITE_TOKEN)
        body["discuss_available"] = True
        route.fulfill(status=200, content_type="application/json", body=json.dumps(body))

    page.route("**/api/v1/client-config", client_config)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/preview*", preview)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/record*", record)
    page.add_init_script(DEVICE_VOICE)
    page.add_init_script(
        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
    page.add_init_script(
        "try { localStorage.setItem('vibe-talk.welcome.dismissed', '1'); } catch (e) {}")
    page.goto(base + "/", wait_until="load")
    page.wait_for_selector("#view-switch", state="visible", timeout=browser_waits.HANG_BACKSTOP_MS)
    for _ in range(3):
        if not page.locator("#discord-channel").is_hidden():
            break
        page.click("#view-switch")
        try:
            page.wait_for_selector("#discord-channel", state="visible", timeout=5_000)
            break
        except Exception:
            continue
    page.wait_for_selector("#discord-channel", state="visible", timeout=browser_waits.HANG_BACKSTOP_MS)
    page.wait_for_selector("#discord-log > li[data-id]", state="visible", timeout=browser_waits.HANG_BACKSTOP_MS)
    loaded = page.evaluate(
        "() => [...document.querySelectorAll('#discord-log > li[data-id]')]"
        ".map(r => r.getAttribute('data-id'))")
    check(len(loaded) >= 10, f"{label}: the loaded window is too small: {len(loaded)}")
    targets["answer"] = str(int(loaded[0]) - 10)
    targets["timeline"] = str(int(loaded[0]) - 20)
    check(all(t not in loaded for t in targets.values()),
          f"{label}: fixture targets are already loaded: {targets}")

    if page.locator("#discuss-open").is_visible():
        page.click("#discuss-open")
    else:
        page.click("#action-menu-open")
        page.wait_for_selector("#discuss-menu-open", state="visible", timeout=browser_waits.HANG_BACKSTOP_MS)
        page.click("#discuss-menu-open")
    page.wait_for_selector("#discuss-panel", state="visible", timeout=browser_waits.HANG_BACKSTOP_MS)
    page.wait_for_selector("#discuss-answer a", state="attached", timeout=browser_waits.HANG_BACKSTOP_MS)
    page.wait_for_selector("#discuss-timeline a", state="attached", timeout=browser_waits.HANG_BACKSTOP_MS)
    page.evaluate("() => { for (const d of document.querySelectorAll('#discuss-answer details, #discuss-timeline details')) d.open = true; }")

    def landed_geo(target: str) -> LandedGeo | None:
        geo: LandedGeo | None = page.evaluate(f"""() => {{
          const row = document.querySelector('#discord-log > li[data-id="{target}"]');
          if (!row) return null;
          const r = row.getBoundingClientRect();
          const panel = document.getElementById('discuss-panel');
          const pr = panel && !panel.hidden ? panel.getBoundingClientRect() : null;
          const area = document.getElementById('scroll-area');
          const ar = area ? area.getBoundingClientRect() : null;
          // `#2147`: only a sheet spanning the list's whole width (the phone
          // sheet) stands over the list; a desk panel stands beside it.
          const sheetSpansList = Boolean(pr && ar && pr.height > 0
            && pr.left <= ar.left + 1 && pr.right >= ar.right - 1);
          return {{top: r.top, bottom: r.bottom, vh: innerHeight,
                   panelTop: pr ? pr.top : null, sheetSpansList,
                   collapsed: row.getAttribute('data-collapsed'),
                   closedDetails: [...row.querySelectorAll('details')].filter(d => !d.open).length,
                   text: row.textContent}};
        }}""")
        return geo

    def click_and_land(selector: str, target: str) -> None:
        page.click(selector)
        page.wait_for_function(
            f"() => document.querySelector('#discord-log > li[data-id=\"{target}\"]') !== null"
            f" && document.querySelector('#discord-log > li[data-id=\"{target}\"]')"
            ".getAttribute('data-linked') === 'true'",
            timeout=browser_waits.HANG_BACKSTOP_MS)

    # Reader OFF: the old behaviour, landing only, no read started.
    click_and_land("#discuss-timeline a", targets["timeline"])
    check(page.evaluate("() => readAloudOwner().nowPlaying === null && readAloudOwner().pendingRead === null"),
          f"{label}: a citation click with the reader off started a read")
    geo = landed_geo(targets["timeline"])
    check(geo is not None and geo["top"] < geo["vh"] and geo["bottom"] > 0,
          f"{label}: reader-off landing is not on screen: {geo}")

    # Reader ON: start the summary read on the device voice.
    toggle = page.locator("#audio-source")
    if toggle.is_visible() and toggle.get_attribute("aria-checked") == "true":
        page.click("#audio-source")
        page.wait_for_function(
            "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
            timeout=browser_waits.HANG_BACKSTOP_MS)
    page.click("#read-aloud")
    page.wait_for_function(
        "() => { const o = readAloudOwner(); return o.nowPlaying !== null || o.pendingRead !== null; }",
        timeout=browser_waits.HANG_BACKSTOP_MS)

    # ONE click on the summary's citation: land expanded, top on screen, read it.
    said_before = page.evaluate("() => window.deviceSpeech.length")
    click_and_land("#discuss-answer a", targets["answer"])
    page.wait_for_function(
        f"() => readAloudOwner().nowPlaying?.id === '{targets['answer']}'"
        f" || readAloudOwner().pendingRead === '{targets['answer']}'",
        timeout=browser_waits.HANG_BACKSTOP_MS)
    page.wait_for_function(f"() => window.deviceSpeech.length > {said_before}", timeout=browser_waits.HANG_BACKSTOP_MS)
    geo = landed_geo(targets["answer"])
    check(geo is not None, f"{label}: the read target row vanished")
    assert geo is not None
    check(geo["top"] >= 0 and geo["top"] < geo["vh"], f"{label}: message top is not on screen: {geo}")
    # `#2147`: centred in the visible area — above the sheet only where the
    # sheet spans the list (phone); a desk panel beside the list covers no
    # part of the row the reader reads, so on-screen is the whole claim there.
    check(geo["panelTop"] is None or not geo["sheetSpansList"] or geo["bottom"] <= geo["panelTop"] + 2,
          f"{label}: message stands under the Discuss sheet: {geo}")
    check(geo["collapsed"] != "true" and geo["closedDetails"] == 0,
          f"{label}: landed message is not expanded: {geo}")
    said = page.evaluate("() => window.deviceSpeech.map((u) => u.text)")
    check(any("seed message" in text for text in said[said_before:]),
          f"{label}: the device voice did not speak the linked message: {said[said_before:]}")
    timing = [line for line in console_lines if "[readjump]" in line]
    check(bool(timing) and re.search(r"click-to-reading-start \d+ms", timing[-1]) is not None,
          f"{label}: no click-to-reading-start timing was logged: {console_lines}")
    print(f"discuss readjump ({label}): one click landed expanded and reading, {timing[-1]}")
    if shots is not None:
        page.screenshot(path=str(shots / f"{label}-readjump.png"))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--screenshots", type=Path, default=None)
    args = parser.parse_args()
    shots: Path | None = args.screenshots
    if shots is not None:
        shots.mkdir(parents=True, exist_ok=True)
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="discuss-readjump-check-") as tmp:
        work = Path(tmp)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "readjump-check-bot-token-never-sent"\n'
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
            for i in range(120):
                call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                     {"text": f"seed message {i} with enough words to take a row in the list"})
            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                phone = browser.new_context(
                    viewport={"width": 412, "height": 915}, has_touch=True, is_mobile=True,
                    color_scheme="dark")
                browser_waits.hot_host_timeouts(phone)
                walk(phone.new_page(), base, "phone-412", shots)
                desk = browser.new_context(viewport={"width": 1280, "height": 800},
                                            color_scheme="dark")
                browser_waits.hot_host_timeouts(desk)
                walk(desk.new_page(), base, "desk-1280", shots)
                browser.close()
            print("discuss readjump browser: phone (412x915) and desk (1280x800) walks passed")
            return 0
        finally:
            server.terminate()


if __name__ == "__main__":
    raise SystemExit(main())
