#!/usr/bin/env python3
"""Pause a read, reply to the message by the phone's box, send it, and resume the read, in real Chromium.

`#554 read-pause-reply-resume`. The owner, 2026-10-26, in a car with the device voice: "When reading a
long message I hear something I want to reply to in the middle ... I press pause on the audio and then I
press the tiny little reply button. And then I press the microphone button and then I speak my reply and
press send ... when I dictated my message and pressed resume on the reading control it did nothing. No
sound came out."

Chrome on Android has no pause for speech: its `speechSynthesis.pause()` stops the voice, and `resume()`
does nothing (Chromium's `TtsPlatformImplAndroid::Pause` calls `StopSpeaking`, and its `Resume` is empty).
The stopped utterance is never ended, so the engine still counts it as current: a Resume that asks the
engine to resume is silence, until the page's watchdog gives up on the read a minute later.

It serves the checked-out page and the fake API of tests/read_aloud_views_browser.py (the agent's voice
streamed at speaking pace from its speech routes) on a phone's screen with touch, and puts in, before the
page loads, a device voice that behaves as Chrome's on Android does: each utterance starts, and ends
`MS_PER_CHAR` per character later; `pause()` silences the utterance under way and ends nothing;
`resume()` does nothing; `cancel()` empties the engine. Every utterance is recorded with its text and
when it started and ended.

For each voice: Read on, a tap on the replica's long first message, and once the read is into its second
sentence (the device's) or second block (the agent's), a tap on Read, which says Pause; Reply on the same
message, the reply typed into the box as dictation leaves it, and Send; then a tap on Read, which says
Resume. The read must go on within a few seconds — the device voice from the start of the sentence it was
paused in, every later sentence after it in order; the agent's voice with its audio clock moving on to its
third block — with no error shown, and the reply posted to the message being read.

Pass --web-root DIR to run against another copy of the page (the one before #554 fails at the device
voice's resume), and --screenshots DIR to keep PNGs.
"""

from __future__ import annotations

import argparse
import os
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING

import offline_cache_browser as fixture
from inline_reply_browser import PHONE, press_reply, sign_in
from offline_cache_browser import check
from read_aloud_views_browser import AUDIO_PROBE, VoicesApi, handler, paragraphs_of
from two_column_browser import Desk

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page

MESSAGE = "209"
LIST = "discord-log"
MS_PER_CHAR = 25
# How long the read may take to be heard again after Resume.
RESUME_WITHIN_S = 4

# Chrome's device voice on Android: pause stops the voice and ends nothing; resume does nothing.
ANDROID_VOICE = """(() => {
  const spoken = [];
  window.deviceSpeech = spoken;
  const voice = {name: 'Check voice', lang: 'en-US', localService: true, default: true, voiceURI: 'check'};
  let current = null;
  const queue = [];
  const engine = {
    paused: false, pending: false,
    get speaking() { return current !== null; },
    getVoices: () => [voice], addEventListener() {}, removeEventListener() {},
    speak(utterance) {
      const entry = {text: String(utterance.text), started: null, ended: null, silenced: null, utterance};
      spoken.push(entry);
      queue.push(entry);
      if (current === null && !this.paused) next();
    },
    cancel() {
      for (const entry of [current, ...queue]) if (entry && entry.ended === null) entry.silenced = performance.now();
      current = null;
      queue.length = 0;
      this.paused = false;
    },
    pause() {
      this.paused = true;
      if (current !== null && current.silenced === null) current.silenced = performance.now();
    },
    resume() { this.paused = false; },
  };
  const next = () => {
    current = queue.shift() || null;
    if (current === null) return;
    const entry = current;
    setTimeout(() => {
      if (entry.silenced !== null) return;
      entry.started = performance.now();
      if (entry.utterance.onstart) entry.utterance.onstart({});
      setTimeout(() => {
        if (entry.silenced !== null) return;
        entry.ended = performance.now();
        current = null;
        if (entry.utterance.onend) entry.utterance.onend({});
        if (!engine.paused) next();
      }, %d * entry.text.length);
    }, 30);
  };
  Object.defineProperty(window, 'speechSynthesis', {value: engine, configurable: true});
  window.SpeechSynthesisUtterance = function (text) { this.text = text; };
})();""" % MS_PER_CHAR

SPOKEN_JS = "() => window.deviceSpeech.map(({text, started, ended, silenced}) => ({text, started, ended, silenced}))"


def wait_until(page: Page, condition: str, why: str, seconds: float = 8) -> None:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline and not page.evaluate(condition):
        page.wait_for_timeout(25)
    check(bool(page.evaluate(condition)), why)


def tap(page: Page, selector: str) -> None:
    box = page.locator(selector).first.bounding_box()
    check(box is not None, f"{selector} is not drawn to tap")
    assert box is not None
    page.touchscreen.tap(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)


def label(page: Page) -> str:
    return str(page.evaluate("() => document.getElementById('read-aloud-label').textContent"))


def walk(desk: Desk, api: VoicesApi, voice: str) -> None:
    page = desk.page
    sign_in(desk)
    if voice == "device":
        page.click("#audio-source")
        desk.wait_for("() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
                      "the audio switch did not choose the device voice")
    tap(page, "#read-aloud")
    desk.settled()
    row = f"#{LIST} > li[data-id='{MESSAGE}']"
    page.evaluate("(selector) => document.querySelector(selector).scrollIntoView({block: 'center'})", row)
    desk.settled()
    body = page.locator(f"{row} .body").first.bounding_box()
    assert body is not None
    page.touchscreen.tap(body["x"] + 12, body["y"] + min(10, body["height"] / 2))

    # Into the message: its second sentence being said, or its second block heard.
    if voice == "device":
        wait_until(page, "() => window.deviceSpeech.filter((one) => one.started !== null).length >= 2",
                   "device: the read never reached its second sentence")
        page.wait_for_timeout(200)
    else:
        desk.wait_for("() => !document.getElementById('read-steer').hidden"
                      " && document.getElementById('read-steer-count').textContent === '2 of 3'",
                      "agent: the read never reached its second block", 10)
        page.wait_for_timeout(100)
    desk.check(label(page) == "Pause", f"{voice}: Read says {label(page)!r} while the read plays")
    tap(page, "#read-aloud")
    desk.settled()
    if voice == "agent":
        # Held in its second block, so the third is still to come after Resume.
        count = page.evaluate("() => document.getElementById('read-steer-count').textContent")
        desk.check(count == "2 of 3", f"agent: the read was paused at {count!r}, not in its second block")
    desk.check(label(page) == "Resume", f"{voice}: Pause left Read saying {label(page)!r}")
    held = page.evaluate(SPOKEN_JS) if voice == "device" else []
    paused_in = next((one["text"] for one in reversed(held) if one["started"] is not None), "")
    sources_at_pause = page.evaluate("() => window.audioProbe.sources.length")
    desk.shot("1-paused")

    # Reply to the message being read, as the phone's box takes dictation, and Send.
    sent = len(api.posted())
    press_reply(desk, LIST, MESSAGE, touch=True)
    page.fill("#reply-text", "Yes, the migration finished overnight")
    desk.shot("2-reply")
    tap(page, "#reply-send")
    desk.wait_for("() => document.getElementById('column-reply').hidden", f"{voice}: Send did not put the box away")
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline and len(api.posted()) <= sent:
        page.wait_for_timeout(25)
    posted = api.posted()[sent:]
    desk.check(len(posted) == 1 and posted[0].get("reply_to") == MESSAGE, f"{voice}: what was posted was {posted}")
    page.wait_for_timeout(800)
    desk.settled()
    desk.check(label(page) == "Resume", f"{voice}: after the reply was sent Read says {label(page)!r}, not Resume")
    desk.shot("3-sent")

    # Resume: the read is heard again, from where it was held.
    resumed_at = page.evaluate("() => performance.now()")
    tap(page, "#read-aloud")
    if voice == "device":
        wait_until(page, f"() => window.deviceSpeech.some((one) => one.started !== null && one.started > {resumed_at})",
                   "device: Resume was silent: no utterance started after it", RESUME_WITHIN_S)
        after = [one for one in page.evaluate(SPOKEN_JS) if one["started"] is not None and one["started"] > resumed_at]
        desk.check(after[0]["text"] == paused_in,
                   f"device: Resume began with {after[0]['text']!r}, not the sentence paused in, {paused_in!r}")
        words = " ".join(" ".join(paragraphs_of(MESSAGE)).split())
        tail = words[words.index(paused_in):]
        wait_until(page, f"() => window.deviceSpeech.filter((one) => one.ended !== null).map((one) => one.text)"
                   f".join(' ').endsWith({tail[-40:]!r})", "device: the resumed read did not reach the message's end",
                   MS_PER_CHAR * len(tail) / 1000 + 8)
        heard = [one["text"] for one in page.evaluate(SPOKEN_JS)
                 if one["started"] is not None and one["started"] > resumed_at and one["ended"] is not None]
        desk.check(" ".join(heard) == tail, f"device: after Resume the voice said {heard}, not the rest from the "
                   f"sentence paused in: {tail!r}")
        desk.notes.append(f"resumed {len(heard)} sentence(s) from {paused_in[:40]!r}")
    else:
        desk.wait_for(f"() => window.audioProbe.sources.length > {sources_at_pause}"
                      " || (window.audioProbe.contexts.slice(-1)[0] || {}).state === 'running'",
                      "agent: Resume was silent", RESUME_WITHIN_S)
        clock = page.evaluate("() => window.audioProbe.contexts.slice(-1)[0].currentTime")
        page.wait_for_timeout(300)
        moved = page.evaluate("() => window.audioProbe.contexts.slice(-1)[0].currentTime") - clock
        desk.check(moved > 0.15, f"agent: after Resume the audio clock stood still ({moved:.3f}s in 300ms)")
        desk.wait_for("() => document.getElementById('read-steer-count').textContent === '3 of 3'"
                      " || document.getElementById('read-steer').hidden",
                      "agent: the resumed read never reached its third block", 10)
        desk.notes.append(f"resumed, the clock moved {moved:.2f}s in 300ms and the read went on to its last block")
    error = page.evaluate("() => document.getElementById('error').textContent")
    desk.check(error == "", f"{voice}: an error was shown: {error!r}")
    desk.shot("4-resumed")


def run(chromium: BrowserType, args: argparse.Namespace, voice: str) -> str:
    api = VoicesApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-pause-reply-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": PHONE[0], "height": PHONE[1]}, color_scheme="dark", has_touch=True, is_mobile=True)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.add_init_script(AUDIO_PROBE)
            page.add_init_script(ANDROID_VOICE)
            desk = Desk(page, args.screenshots, f"pause-reply-{voice}", PHONE[0], PHONE[1])
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            walk(desk, api, voice)
            check(not errors, f"{voice}: the page threw: {errors}")
            context.close()
            return f"{version}, the {voice} voice: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=fixture.WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {fixture.WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path, help="directory to write the walk's PNGs into (default: none)")
    parser.add_argument("--voice", choices=("agent", "device", "both"), default="both",
                        help="which voice to pause, reply and resume with (default: both, one browser each)")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit("Python Playwright is required: python3 -m pip install --user playwright && "
                         "python3 -m playwright install chromium") from error
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    fixture.WEB_ROOT = args.web_root.resolve()
    report = []
    with sync_playwright() as playwright:
        for voice in (("agent", "device") if args.voice == "both" else (args.voice,)):
            report.append(run(playwright.chromium, args, voice))
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
