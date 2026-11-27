#!/usr/bin/env python3
"""Reading mode turned off is announced, never floated over the row sent next (`#2375`).

`#2375 vibe-talk-reading-off-toast-covers-sent-row`. Seen in the #2367 evidence: a read stopped
at 393x852, then a message sent from the channel's box, and the "reading mode off." toast floated
over the sent row's header (its time and Unarchive control). Settled as #2365 settled its jump
toast: announced, not floated. The status line still says the words (a polite live region a
screen reader hears), clipped to nothing on screen like `.sr-only`.

This walk serves the real page with a fake API that keeps what is posted, and a device voice that
says nothing aloud. At 393x852 and 412x915 (touch) and 1280x800, in light and dark: Read
on, Read off again ("reading mode off."), a message typed and sent, and then, by geometry only:

  the status line says "reading mode off." and is marked to wait (`data-idle`), and the
  accessibility tree still holds it as a polite status (a screen reader hears it);
  it covers no row: its box meets no drawn row's box, the sent row's header (its time and its
  controls) included; and the sent row stands with its header fully inside the list.

Pass --screenshots DIR to keep a PNG per size and scheme; --web-root DIR to run it against another
copy of the page (main before the change floats the toast over the sent row).
"""

from __future__ import annotations

import argparse
import json
import os
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from offline_cache_browser import TOKEN, Json, OWNER_ID, check, handler_for, said
import page_sources
import browser_scratch
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import Page

WAIT_S = 90.0
SENT = "Sent after the read was stopped: the next step is ready."

# A device voice that says nothing aloud and answers as a voice does.
DEVICE_VOICE = """(() => {
  const voice = {name: 'Check voice', lang: 'en-US', localService: true, default: true, voiceURI: 'check'};
  const engine = {
    paused: false, speaking: false, pending: false,
    getVoices: () => [voice], addEventListener() {}, removeEventListener() {},
    speak(utterance) { setTimeout(() => { if (utterance.onstart) utterance.onstart({}); }, 30); },
    cancel() {}, pause() { this.paused = true; }, resume() { this.paused = false; },
  };
  Object.defineProperty(window, 'speechSynthesis', {value: engine, configurable: true});
  window.SpeechSynthesisUtterance = function (text) { this.text = text; };
  try { localStorage.setItem('vibe-talk.voice.read-audio-source', 'device'); } catch (e) {}
})();"""

MEASURE_JS = """(text) => {
  const box = (n) => { if (!n || !n.getClientRects().length) return null; const r = n.getBoundingClientRect();
    return {top: r.top, bottom: r.bottom, left: r.left, right: r.right, width: r.width, height: r.height}; };
  const line = document.getElementById('status-line');
  const rows = [...document.querySelectorAll('#discord-log > li[data-id]')].filter((li) => li.getClientRects().length);
  const sent = rows.find((li) => li.textContent.includes(text)) || null;
  const area = document.getElementById('scroll-area').getBoundingClientRect();
  return {status: (document.getElementById('status') || {}).textContent || '', idle: line.hasAttribute('data-idle'),
    hidden: line.hidden, line: box(line), sent: box(sent), meta: box(sent ? sent.querySelector('.meta') : null),
    area: {top: area.top, bottom: area.bottom}, rows: rows.map((li) => box(li))};
}"""


class Channel(fixture.FakeApi):
    """A short channel; what the reader posts is kept and shown as theirs."""

    def __init__(self) -> None:
        super().__init__()
        self.messages = [said(i, f"Channel note {i}: routine status, filed and done.", "coder") for i in range(8)]
        self.threads = []

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope), "owner_author_id": OWNER_ID, "self_author_id": OWNER_ID}

    def post(self, path: str, body: Json) -> tuple[int, Json]:
        if path.endswith("/reply"):
            with self.lock:
                posted = said(len(self.messages), str(body.get("text", "")), "me")
                self.messages.append(posted)
            return 200, {"posted": posted, "parts": [posted]}
        return super().post(path, body)


def overlaps(a: object, b: object) -> bool:
    if not isinstance(a, dict) or not isinstance(b, dict):
        return False
    return (float(a["left"]) < float(b["right"]) and float(b["left"]) < float(a["right"])
            and float(a["top"]) < float(b["bottom"]) and float(b["top"]) < float(a["bottom"]))


def until(page: Page, script: str, why: str, arg: object = None) -> None:
    deadline = time.monotonic() + WAIT_S
    while time.monotonic() < deadline:
        if page.evaluate(script, arg):
            return
        page.wait_for_timeout(25)
    check(bool(page.evaluate(script, arg)), why)


def settled(page: Page) -> None:
    page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")


def walk(pw: object, args: argparse.Namespace, work: Path, label: str, width: int, height: int, scheme: str) -> str:
    api = Channel()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    threading.Thread(target=server.serve_forever, daemon=True).start()
    mobile = width < 600
    try:
        profile = work / f"profile-{label}"
        profile.mkdir()
        context = pw.chromium.launch_persistent_context(  # type: ignore[attr-defined]
            str(profile), headless=True, executable_path=args.browser_executable,
            viewport={"width": width, "height": height}, device_scale_factor=2.625 if mobile else 1,
            is_mobile=mobile, has_touch=mobile, color_scheme=scheme, timezone_id="America/New_York")
        try:
            browser_waits.hot_host_timeouts(context)
            page = context.pages[0] if context.pages else context.new_page()
            page.add_init_script(DEVICE_VOICE)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            try:
                page.wait_for_function("() => !document.getElementById('screen-welcome').hidden "
                                       "|| !document.getElementById('screen-main').hidden", timeout=10_000)
                if page.evaluate("() => !document.getElementById('screen-welcome').hidden"):
                    page.click("#welcome-start")
            except Exception:
                pass
            page.wait_for_selector("#read-aloud", state="visible", timeout=int(WAIT_S * 1000))
            until(page, "() => document.querySelectorAll('#discord-log > li[data-id]').length >= 8",
                  f"{label}: the channel was not drawn")
            # Read on, then Read off: the read stopped, and the page says "reading mode off."
            page.click("#read-aloud")
            until(page, "() => document.getElementById('read-aloud').getAttribute('aria-pressed') === 'true'"
                        " || (document.getElementById('status') || {}).textContent.startsWith('Device voice')"
                        " || (document.getElementById('status') || {}).textContent.startsWith('reading from')",
                  f"{label}: Read did not turn reading mode on")
            # Stop, as the reader stops a read: it ends the read and leaves the mode in one tap
            # (`#442 read-strip-order`). With nothing playing, Read itself leaves the mode.
            stop = page.locator("#read-aloud-stop")
            (stop if stop.is_visible() else page.locator("#read-aloud")).click()
            until(page, "() => (document.getElementById('status') || {}).textContent === 'reading mode off.'",
                  f"{label}: stopping the read did not say reading mode off.")
            # A message typed and sent from the channel's box.
            page.fill("#channel-compose-text", SENT)
            page.click("#channel-send")
            until(page, "(text) => [...document.querySelectorAll('#discord-log > li[data-id]')]"
                        ".some((li) => li.textContent.includes(text))", f"{label}: the sent row was not drawn", SENT)
            settled(page)
            settled(page)
            found = page.evaluate(MEASURE_JS, SENT)
            assert isinstance(found, dict)
            # Reachable by a screen reader: the accessibility tree still holds the polite status
            # with the words (a role query skips what is hidden from assistive technology).
            found["heard"] = page.get_by_role("status").filter(has_text="reading mode off.").count()
            found["live"] = page.evaluate("() => document.getElementById('status').getAttribute('aria-live')")
            if args.screenshots is not None:
                page.screenshot(path=str(args.screenshots / f"{label}.png"))
        finally:
            context.close()
    finally:
        server.shutdown()
    covered = [row for row in found["rows"] if overlaps(found["line"], row)]
    meta, area = found["meta"], found["area"]
    check(isinstance(meta, dict) and float(meta["top"]) >= float(area["top"]) - 0.5
          and float(meta["bottom"]) <= float(area["bottom"]) + 0.5, f"{label}: the sent row's header is not on screen: {meta}")
    check(not overlaps(found["line"], meta), f"{label}: \"reading mode off.\" {found['line']} covers the sent row's header {meta}")
    check(not covered, f"{label}: \"reading mode off.\" {found['line']} stands over {len(covered)} row(s)")
    check(found["status"] == "reading mode off.", f"{label}: the status line no longer says reading mode off.: {found['status']!r}")
    check(found["idle"] is True, f"{label}: \"reading mode off.\" was floated, not announced (no data-idle)")
    check(found["heard"] == 1 and found["live"] == "polite",
          f"{label}: \"reading mode off.\" is not in the accessibility tree as a polite status ({found['heard']}, {found['live']!r})")
    return f"{label}: reading mode off. announced (data-idle), over no row; the sent row's header on screen"


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT, help="directory holding the page to serve")
    parser.add_argument("--screenshots", type=Path, help="directory to write the PNGs into")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    from playwright.sync_api import sync_playwright
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    report: list[str] = []
    # `#2382 summary-link-test-host-tmp`: never the host temp dir; see browser_scratch.
    with browser_scratch.run_scratch("reading-off-toast-") as work:
        fixture.WEB_ROOT = page_sources.served(args.web_root)
        with sync_playwright() as pw:
            for width, height in ((393, 852), (412, 915), (1280, 800)):
                for scheme in ("light", "dark"):
                    report.append(walk(pw, args, work, f"{width}x{height}-{scheme}", width, height, scheme))
    for line in report:
        print(line)
    print("reading off toast browser walk: ok")
    print(json.dumps({"cases": len(report)}))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
