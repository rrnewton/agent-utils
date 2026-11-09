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

`#819 voice-page-quoted-reply-and-draft-chip`. The owner lost a dictated reply: in Reading mode with the
reader paused, the Quoted reply chip under selected words "cannot be pressed", as if something floating
covered it. So, with the agent's voice paused in its second block, on a phone's screen by touch and at a
desk by the mouse: words are selected near the top, the middle and the foot of the list, and at each the
chip is the topmost thing at its centre and around it, as is its twin, Quote reply in the read's
strip; a press of the chip opens the quick reply focused, which takes typed words and words committed as
dictation does, and sends them quoted; and a press of the twin opens it too.

`#821 voice-page-read-aloud-react`. With the agent's voice reading the long first message, at 360x780, 375x812
and 412x915 by touch and 1280x800 by mouse: the sentence being said is lit; Pause holds it, the same range lit
a second later, and React stands at the far right of the arrows' row, the arrows and the count between the
row's edge and React, every arrow and React at least 48px tall on a phone (52px drawn), the row one line.
React opens the quick reply quoting exactly the lit sentence, focused; a stand-in recognition (the browser's
`webkitSpeechRecognition`, which a phone's Chrome has) is started by the press, says it listens, and its words,
unsure then settled, land in the box, which the Draft chip then counts. And with no recognition at all, React
still opens the box focused and says to use the keyboard's microphone. The device voice, whose pause empties
its engine and lights no sentence, holds the block being read instead, and React quotes that.

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
import page_sources

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
    desk.wait_for("() => document.getElementById('read-aloud').getAttribute('data-active') === 'true'",
                  f"{voice}: Read did not turn on")
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
    desk.wait_for("() => document.getElementById('read-aloud-label').textContent === 'Resume'",
                  f"{voice}: Pause did not hold the read")
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


# `#819`: the words of the message being read, from offset `start` of its longest run of text to `end`,
# selected, and their box.
SELECT_JS = """([list, id, start, end]) => {
  const row = document.querySelector(`#${list} > li[data-id='${id}']`);
  const walker = document.createTreeWalker(row.querySelector('.body'), NodeFilter.SHOW_TEXT);
  let text = null;
  for (let node = walker.nextNode(); node; node = walker.nextNode()) if (!text || node.data.length > text.data.length) text = node;
  const range = document.createRange();
  range.setStart(text, Math.min(start, text.data.length - 1));
  range.setEnd(text, Math.min(end, text.data.length));
  getSelection().removeAllRanges();
  getSelection().addRange(range);
  const r = range.getBoundingClientRect();
  return {top: r.top, bottom: r.bottom};
}"""

# `#819`: whether `id`'s element is what a finger meets at its centre, at its top and foot in the middle, and at
# the middle of each rounded end (half its height in: a pill's corners are not its own), and if not, what is.
TOPMOST_JS = """(id) => {
  const node = document.getElementById(id);
  if (!node || node.hidden || !node.getClientRects().length) return {shown: false};
  const r = node.getBoundingClientRect();
  const [x, y] = [r.left + r.width / 2, r.top + r.height / 2];
  const points = [[x, y], [x, r.top + 2], [x, r.bottom - 2], [r.left + r.height / 2, y], [r.right - r.height / 2, y]];
  const over = points.map(([x, y]) => document.elementFromPoint(x, y)).filter((at) => !node.contains(at))
    .map((at) => (at && (at.id || String(at.className) || at.tagName)) || 'nothing');
  return {shown: true, over, box: {top: r.top, bottom: r.bottom, left: r.left, right: r.right}};
}"""


def press(page: Page, selector: str, touch: bool) -> None:
    box = page.locator(selector).first.bounding_box()
    check(box is not None, f"{selector} is not drawn to press")
    assert box is not None
    x, y = box["x"] + box["width"] / 2, box["y"] + box["height"] / 2
    if touch:
        page.touchscreen.tap(x, y)
    else:
        page.mouse.click(x, y)


def quote_walk(desk: Desk, api: VoicesApi, touch: bool) -> None:
    """`#819`: the chip, and its twin in the strip, pressed in a paused read; the quick reply typed and dictated into."""
    page = desk.page
    sign_in(desk)
    press(page, "#read-aloud", touch)
    desk.wait_for("() => document.getElementById('read-aloud').getAttribute('data-active') === 'true'",
                  "quote: Read did not turn on")
    row = f"#{LIST} > li[data-id='{MESSAGE}']"
    page.evaluate("(selector) => document.querySelector(selector).scrollIntoView({block: 'center'})", row)
    desk.settled()
    body = page.locator(f"{row} .body").first.bounding_box()
    assert body is not None
    press_at = (body["x"] + 12, body["y"] + min(10, body["height"] / 2))
    if touch:
        page.touchscreen.tap(*press_at)
    else:
        page.mouse.click(*press_at)
    desk.wait_for("() => !document.getElementById('read-steer').hidden"
                  " && document.getElementById('read-steer-count').textContent === '2 of 3'",
                  "quote: the read never reached its second block", 10)
    # Playing, the strip steers only: Quote reply waits for the read to be held.
    page.evaluate(SELECT_JS, [LIST, MESSAGE, 2, 22])
    desk.wait_for("() => !document.getElementById('selection-chip').hidden", "quote: no chip while the read plays")
    desk.check(page.evaluate("() => document.getElementById('read-steer-quote').getClientRects().length") == 0,
               "quote: the strip offered Quote reply while the read played")
    page.evaluate("() => getSelection().removeAllRanges()")
    press(page, "#read-aloud", touch)
    desk.wait_for("() => document.getElementById('read-aloud-label').textContent === 'Resume' "
                  "&& document.documentElement.getAttribute('data-read-strip') === 'paused'",
                  "quote: Pause did not hold the read")
    desk.check(label(page) == "Resume", f"quote: Pause left Read saying {label(page)!r}")
    desk.check(page.evaluate("() => document.documentElement.getAttribute('data-read-strip')") == "paused",
               "quote: the strip does not say the read is paused")
    area = page.locator("#scroll-area").bounding_box()
    assert area is not None
    places = []
    for share in (0.15, 0.5, 0.8):
        words = page.evaluate(SELECT_JS, [LIST, MESSAGE, 2, 22])
        page.evaluate("(dy) => { document.getElementById('scroll-area').scrollTop += dy; }",
                      words["top"] - (area["y"] + area["height"] * share))
        desk.settled()
        words = page.evaluate(SELECT_JS, [LIST, MESSAGE, 2, 22])
        desk.wait_for("() => !document.getElementById('selection-chip').hidden", f"quote at {share}: no chip under the words")
        desk.settled()
        for twin, other in (("selection-chip", "read-steer-quote"), ("read-steer-quote", "selection-chip")):
            found = page.evaluate(TOPMOST_JS, twin)
            desk.check(found["shown"] is True, f"quote at {share}: #{twin} is not shown")
            # Each may stand over the other near the foot: a press of either is the same press.
            over = [one for one in found["over"] if one != other]
            desk.check(over == [], f"quote at {share}: #{twin} is covered by {over} at {found['box']}")
        places.append(f"{words['top']:.0f}")
        if share == 0.5:
            desk.shot("quote-paused")
    # The chip pressed: the quick reply, focused, typed into and dictated into, and sent quoted.
    words = page.evaluate(SELECT_JS, [LIST, MESSAGE, 2, 22])
    desk.wait_for("() => !document.getElementById('selection-chip').hidden", "quote: no chip to press")
    desk.settled()
    sent = len(api.posted())
    press(page, "#selection-chip", touch)
    desk.wait_for("() => !document.getElementById('quick-reply').hidden", "quote: the chip opened no quick reply")
    desk.wait_for("() => document.activeElement === document.getElementById('quick-reply-text')",
                  "quote: the quick reply did not take the focus")
    page.keyboard.type("typed, ")
    # What a phone's dictation leaves: text committed whole, as an input method commits it.
    page.keyboard.insert_text("and dictated")
    value = page.evaluate("() => document.getElementById('quick-reply-text').value")
    desk.check(value.endswith("typed, and dictated"), f"quote: the quick reply holds {value!r}")
    desk.shot("quote-typed")
    press(page, "#quick-reply-send", touch)
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline and len(api.posted()) <= sent:
        page.wait_for_timeout(25)
    posted = api.posted()[sent:]
    text = str(posted[0].get("text", "")) if posted else ""
    desk.check(len(posted) == 1 and posted[0].get("reply_to") == MESSAGE and text.startswith("> ")
               and text.endswith("\n\ntyped, and dictated"), f"quote: what was posted was {posted}")
    # The twin in the strip, pressed: the quick reply too.
    # Once the reply has landed and its message's row is drawn again with it.
    page.wait_for_timeout(800)
    desk.settled()
    page.evaluate(SELECT_JS, [LIST, MESSAGE, 30, 50])
    desk.wait_for("() => !document.getElementById('read-steer-quote').hidden"
                  " && document.getElementById('read-steer-quote').getClientRects().length > 0",
                  "quote: no Quote reply in the strip")
    desk.settled()
    press(page, "#read-steer-quote", touch)
    desk.wait_for("() => !document.getElementById('quick-reply').hidden", "quote: the strip's Quote reply opened no quick reply")
    quoted = page.evaluate("() => document.getElementById('quick-reply-quote').textContent")
    desk.check(bool(quoted.strip()), "quote: the strip's Quote reply quoted nothing")
    press(page, "#quick-reply-cancel", touch)
    desk.wait_for("() => document.getElementById('quick-reply').hidden", "quote: Cancel left the quick reply up")
    desk.check(label(page) == "Resume", f"quote: the read was not left paused: Read says {label(page)!r}")
    error = page.evaluate("() => document.getElementById('error').textContent")
    desk.check(error == "", f"quote: an error was shown: {error!r}")
    desk.notes.append(f"the chip and the strip's Quote reply topmost with words at {', '.join(places)}px; the chip's "
                      "quick reply typed into, dictated into and sent quoted; the strip's opened it too")


def run_quote(chromium: BrowserType, args: argparse.Namespace, touch: bool) -> str:
    api = VoicesApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    width, height = PHONE if touch else (1280, 800)
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-pause-quote-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark", has_touch=touch, is_mobile=touch)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.add_init_script(AUDIO_PROBE)
            desk = Desk(page, args.screenshots, f"pause-quote-{width}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            quote_walk(desk, api, touch)
            check(not errors, f"quote {width}x{height}: the page threw: {errors}")
            context.close()
            return f"{version}, a paused read at {width}x{height} by {'touch' if touch else 'mouse'}: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


# `#821`: a stand-in for the browser's speech recognition, which says "please check" unsure and then settles on
# "please check the replica", and records that it was started.
FAKE_RECOGNITION = """(() => {
  window.recognitionStarts = 0;
  // `#829`: the box's inputmode each time it takes the focus, to show the keyboard was down before it did.
  window.focusModes = [];
  document.addEventListener('focusin', (event) => {
    if (event.target && event.target.id === 'quick-reply-text') window.focusModes.push(event.target.getAttribute('inputmode'));
  }, true);
  class FakeRecognition {
    constructor() { this.lang = ''; this.continuous = true; this.interimResults = false; this.onresult = null; this.onerror = null; this.onend = null; }
    start() {
      window.recognitionStarts += 1;
      const say = (words, isFinal) => this.onresult && this.onresult({resultIndex: 0, results: [Object.assign([{transcript: words}], {isFinal})]});
      setTimeout(() => say('please check', false), 150);
      setTimeout(() => say('please check the replica', true), 450);
      setTimeout(() => this.onend && this.onend(), window.recognitionLasts || 1500);
    }
    abort() { window.recognitionAborts = (window.recognitionAborts || 0) + 1; }
  }
  window.SpeechRecognition = FakeRecognition;
  window.webkitSpeechRecognition = FakeRecognition;
})();"""

# `#821`: a browser with no speech recognition at all.
NO_RECOGNITION = """(() => {
  for (const name of ['SpeechRecognition', 'webkitSpeechRecognition']) {
    try { Object.defineProperty(window, name, {value: undefined, configurable: true, writable: true}); } catch (_error) {}
  }
})();"""

# `#821`: the lit sentence's words, and the strip's controls' boxes.
LIT_JS = """() => {
  const lit = CSS.highlights && CSS.highlights.get('read-aloud-sentence');
  const range = lit ? [...lit][0] : null;
  return range ? range.toString().replace(/\\s+/g, ' ').trim() : null;
}"""
STRIP_JS = """() => {
  const box = (id) => { const n = document.getElementById(id); if (!n || n.hidden || !n.getClientRects().length) return null;
    const r = n.getBoundingClientRect(); return {left: r.left, right: r.right, top: r.top, bottom: r.bottom, width: r.width, height: r.height}; };
  return {strip: box('read-steer'), row: box('read-steer-row'), balance: box('read-steer-balance'),
          react: box('read-react'), count: box('read-steer-count'), autoplay: box('read-autoplay'),
          arrows: ['read-section-prev', 'read-section-next', 'read-prev', 'read-next'].map(box), timeline: box('read-timeline'),
          width: document.documentElement.clientWidth};
}"""


# `#829`: the microphone badge's place against the quick reply's lower-left corner, and the box's keyboard.
MIC_JS = """() => {
  const box = document.getElementById('quick-reply').getBoundingClientRect();
  const mic = document.getElementById('quick-reply-mic');
  const m = mic.getBoundingClientRect();
  const text = document.getElementById('quick-reply-text');
  const at = document.elementFromPoint((m.left + m.right) / 2, (m.top + m.bottom) / 2);
  return {shown: !mic.hidden && mic.getClientRects().length > 0, corner: m.left <= box.left && m.right >= box.left
            && m.top <= box.bottom && m.bottom >= box.bottom, onScreen: m.left >= 0 && m.bottom <= innerHeight,
          topmost: at === mic || mic.contains(at), inputmode: text.getAttribute('inputmode'),
          focused: document.activeElement === text, focusModes: window.focusModes || []};
}"""


def check_listening(desk: Desk, where: str) -> None:
    """`#829 voice-page-dictation-polish`: listening, the box keeps the focus with the keyboard down, and the
    microphone badge hangs off its lower-left corner, on screen and on top."""
    mic = desk.page.evaluate(MIC_JS)
    desk.check(mic["inputmode"] == "none" and mic["focused"], f"{where}: the keyboard is not kept down in a focused box: {mic}")
    desk.check(bool(mic["focusModes"]) and mic["focusModes"][-1] == "none",
               f"{where}: the box took the focus before the keyboard was kept down: {mic}")
    desk.check(mic["shown"] and mic["corner"] and mic["onScreen"] and mic["topmost"],
               f"{where}: the microphone badge is not on the box's lower-left corner: {mic}")


# `#837`: the chip back to the message being read, drawn and on top; and where that message stands in its list.
JUMP_JS = """() => {
  const chip = document.getElementById('jump-reading');
  const r = chip.getBoundingClientRect();
  const drawn = !chip.hidden && chip.getClientRects().length > 0;
  const at = drawn ? document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2) : null;
  return {shown: drawn, topmost: !!at && (at === chip || chip.contains(at))};
}"""
ROW_PLACE_JS = """(selector) => {
  const row = document.querySelector(selector);
  const area = document.getElementById('scroll-area');
  const r = row.getBoundingClientRect();
  const a = area.getBoundingClientRect();
  const bottom = a.top + area.clientHeight;
  return {off: (r.top + r.bottom) / 2 - (a.top + bottom) / 2, inside: r.top >= a.top - 1 && r.bottom <= bottom + 1,
          height: r.height, view: area.clientHeight, top: area.scrollTop};
}"""


def check_jump_to_reading(desk: Desk, where: str, touch: bool, row: str, step: bool = False) -> None:
    """`#837 voice-page-jump-to-reading`: the chip is down while the message being read is on screen, up once the
    list scrolls it away, on top of what is there; a tap brings the message back centred, and the chip goes."""
    page = desk.page
    desk.check(not page.evaluate(JUMP_JS)["shown"], f"{where}: the chip is up with the message being read on screen")
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
    desk.wait_for(f"() => ({JUMP_JS})().shown", f"{where}: no chip once the message being read left the screen")
    desk.check(page.evaluate(JUMP_JS)["topmost"], f"{where}: something covers the chip")
    if step:
        # A step to another block of the read, with the message still away: the chip stays up through the wait.
        was = page.evaluate("() => document.getElementById('read-steer-count').textContent")
        page.evaluate("() => { const chip = document.getElementById('jump-reading'); window.jumpReadingDrops = 0;"
                      " window.jumpReadingWatch = new MutationObserver(() => { if (chip.hidden) window.jumpReadingDrops += 1; });"
                      " window.jumpReadingWatch.observe(chip, {attributes: true, attributeFilter: ['hidden']}); }")
        press(page, "#read-prev", touch)
        desk.wait_for(f"() => document.getElementById('read-steer-count').textContent !== {was!r}",
                      f"{where}: the step back never landed")
        page.wait_for_timeout(300)
        drops = page.evaluate("() => { window.jumpReadingWatch.disconnect(); return window.jumpReadingDrops; }")
        desk.check(drops == 0, f"{where}: the chip dropped {drops} time(s) through a step's wait")
        wait_until(page, f"() => ({LIT_JS})() !== null", f"{where}: no sentence is lit after the step")
    desk.shot(f"jump-reading-{where.split(', ')[-1]}")
    press(page, "#jump-reading", touch)
    desk.wait_for(f"() => !({JUMP_JS})().shown", f"{where}: the chip stayed up once it brought the message back")
    place = page.evaluate(ROW_PLACE_JS, row)
    desk.check(place["inside"] and abs(place["off"]) <= 2, f"{where}: the message read is not centred in its list: {place}")


def check_nav_row(desk: Desk, where: str, touch: bool) -> None:
    """`#828 voice-page-read-nav-center`: the arrows' row is one line, "k of n" at its exact centre, two arrows a
    side of it, React at the right and its balance, as wide, at the left; on a phone every target a thumb's."""
    layout = desk.page.evaluate(STRIP_JS)
    react, count, row, balance = layout["react"], layout["count"], layout["row"], layout["balance"]
    arrows = [one for one in layout["arrows"] if one]
    desk.check(isinstance(react, dict) and isinstance(count, dict) and isinstance(row, dict) and isinstance(balance, dict)
               and len(arrows) == 4, f"{where}: the row lacks React, the count, the balance or an arrow: {layout}")
    middles = [(one["top"] + one["bottom"]) / 2 for one in [*arrows, react, count]]
    desk.check(max(middles) - min(middles) <= 4, f"{where}: the arrows' row is more than one line: {layout}")
    off = (count["left"] + count["right"]) / 2 - (row["left"] + row["right"]) / 2
    desk.check(abs(off) <= 2, f"{where}: the count stands {off:.1f}px off the row's centre: {layout}")
    desk.check(abs(balance["width"] - react["width"]) <= 1, f"{where}: the balance is not as wide as React: {layout}")
    lean = (count["left"] - arrows[0]["left"]) - (arrows[3]["right"] - count["right"])
    desk.check(abs(lean) <= 2, f"{where}: the arrows lean {lean:.1f}px to one side of the count: {layout}")
    desk.check(all(react["left"] >= one["right"] - 0.5 for one in arrows) and react["left"] >= count["right"] - 0.5,
               f"{where}: React is not at the right of the arrows and the count: {layout}")
    desk.check(arrows[0]["left"] - layout["strip"]["left"] >= react["width"],
               f"{where}: the first arrow stands too near the row's left edge: {layout}")
    # `#886 voice-page-autoplay`: React at the row's RIGHT EDGE, right-justified, and the balance at its left edge
    # holding Autoplay, both apart from the arrows and the count in the middle.
    desk.check(row["right"] - react["right"] <= 1, f"{where}: React is not at the row's right edge: {layout}")
    desk.check(balance["left"] - row["left"] <= 1, f"{where}: the balance is not at the row's left edge: {layout}")
    # ...the room left over shared between its two sides: none on a narrow phone, the same on each side on a desk.
    sides = (arrows[0]["left"] - balance["right"], react["left"] - arrows[3]["right"])
    desk.check(abs(sides[0] - sides[1]) <= 2, f"{where}: the room beside the arrows is not shared {sides}: {layout}")
    auto = layout["autoplay"]
    desk.check(isinstance(auto, dict) and auto["left"] >= balance["left"] - 0.5 and auto["right"] <= balance["right"] + 0.5,
               f"{where}: Autoplay is not drawn in the balance at the row's left: {layout}")
    desk.check(layout["strip"]["right"] <= layout["width"] + 0.5, f"{where}: the strip runs past the screen: {layout}")
    if touch:
        small = [one for one in [*arrows, react, auto] if one["height"] < 48 or (one in arrows and one["width"] < 44)]
        desk.check(not small, f"{where}: a phone's arrow or React is under its size: {small}")


def react_walk(desk: Desk, api: VoicesApi, voice: str, touch: bool, recognition: bool) -> None:
    """`#821`: a paused read's React, its held sentence, its layout, and the box it opens."""
    page = desk.page
    sign_in(desk)
    if voice == "device":
        page.click("#audio-source")
        desk.wait_for("() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
                      "react: the audio switch did not choose the device voice")
    press(page, "#read-aloud", touch)
    desk.wait_for("() => document.getElementById('read-aloud').getAttribute('data-active') === 'true'",
                  "react: Read did not turn on")
    row = f"#{LIST} > li[data-id='{MESSAGE}']"
    page.evaluate("(selector) => document.querySelector(selector).scrollIntoView({block: 'center'})", row)
    desk.settled()
    body = page.locator(f"{row} .body").first.bounding_box()
    assert body is not None
    if touch:
        page.touchscreen.tap(body["x"] + 12, body["y"] + min(10, body["height"] / 2))
    else:
        page.mouse.click(body["x"] + 12, body["y"] + min(10, body["height"] / 2))
    if voice == "device":
        wait_until(page, "() => window.deviceSpeech.filter((one) => one.started !== null).length >= 2",
                   "react, device: the read never reached its second sentence")
    else:
        desk.wait_for("() => !document.getElementById('read-steer').hidden"
                      " && document.getElementById('read-steer-count').textContent === '2 of 3'",
                      "react: the read never reached its second block", 10)
        wait_until(page, f"() => ({LIT_JS})() !== null", "react: no sentence is lit while the read plays")
    # `#828`: React is there while the read plays, and the row is centred with it.
    desk.check(page.evaluate("() => { const n = document.getElementById('read-react'); return !n.hidden && !n.disabled"
                             " && n.getClientRects().length > 0; }") is True, "react: React is not offered while the read plays: " + str(page.evaluate("() => { const s = document.getElementById('read-steer'); const n = document.getElementById('read-react'); return [s.hidden, s.getAttribute('data-react-only'), n.hidden, n.disabled, n.getClientRects().length, getComputedStyle(s).display]; }")))
    if voice == "agent":
        check_nav_row(desk, "react, playing", touch)
        check_jump_to_reading(desk, "jump, playing", touch, row, step=True)
    press(page, "#read-aloud", touch)
    desk.wait_for("() => document.getElementById('read-aloud-label').textContent === 'Resume'",
                  "react: Pause did not hold the read")
    desk.check(label(page) == "Resume", f"react: Pause left Read saying {label(page)!r}")
    held = page.evaluate(LIT_JS) if voice == "agent" else None
    page.wait_for_timeout(1000)
    if voice == "agent":
        desk.check(held is not None and page.evaluate(LIT_JS) == held,
                   f"react: the light did not hold on {held!r} through the pause: {page.evaluate(LIT_JS)!r}")
    layout = page.evaluate(STRIP_JS)
    react = layout["react"]
    desk.check(isinstance(react, dict), f"react: React is not offered while the read is paused: {layout}")
    arrows = [one for one in layout["arrows"] if one]
    if voice == "device":
        # A read of one block draws no arrows: its strip stands for React alone.
        desk.check(not arrows, f"react, device: a one-block read's strip draws arrows: {layout}")
        # `#886 voice-page-autoplay`: Autoplay stays at its left, React at its right edge.
        desk.check(isinstance(layout["autoplay"], dict) and isinstance(layout["row"], dict)
                   and layout["row"]["right"] - react["right"] <= 1 and layout["autoplay"]["left"] - layout["row"]["left"] <= 1,
                   f"react, device: a one-block read's strip lost Autoplay at its left or React at its right edge: {layout}")
    if voice == "agent":
        check_nav_row(desk, "react, paused", touch)
        check_jump_to_reading(desk, "jump, paused", touch, row)
        # A long read's count, "12 of 12", still leaves the row one line, centred, with React in it.
        page.evaluate("() => { document.getElementById('read-steer-count').textContent = '12 of 12'; }")
        check_nav_row(desk, "react, a two-digit count", touch)
        page.evaluate("() => { document.getElementById('read-steer-count').textContent = '2 of 3'; }")
    found = page.evaluate("() => { const n = document.getElementById('read-react'); const r = n.getBoundingClientRect();"
                          " const at = document.elementFromPoint(r.left + r.width / 2, r.top + r.height / 2);"
                          " return at === n || n.contains(at); }")
    desk.check(found is True, "react: something covers React")
    desk.shot("react-paused")
    sent_drafts = page.evaluate("() => document.getElementById('draft-chip').hidden")
    press(page, "#read-react", touch)
    desk.wait_for("() => !document.getElementById('quick-reply').hidden", "react: React opened no quick reply")
    desk.wait_for("() => document.activeElement === document.getElementById('quick-reply-text')",
                  "react: the quick reply did not take the focus")
    quote = page.evaluate("() => document.getElementById('quick-reply-quote').textContent.replace(/\\s+/g, ' ').trim()")
    if voice == "agent":
        desk.check(quote == held, f"react: the quick reply quotes {quote!r}, not the lit sentence {held!r}")
    else:
        words = " ".join(" ".join(paragraphs_of(MESSAGE)).split())
        desk.check(bool(quote) and quote in words, f"react, device: the quick reply quotes {quote!r}, not words of the message")
    if recognition:
        desk.wait_for("() => window.recognitionStarts === 1", "react: React started no recognition")
        check_listening(desk, "react, listening")
        desk.wait_for("() => document.getElementById('quick-reply').hasAttribute('data-listening')"
                      " || document.getElementById('quick-reply-text').value.includes('please')",
                      "react: the box shows no listening state")
        desk.wait_for("() => document.getElementById('quick-reply-text').value.endsWith('please check the replica')",
                      "react: the settled words did not land in the box", 5)
        desk.wait_for("() => !document.getElementById('quick-reply').hasAttribute('data-listening')",
                      "react: the box still listens after the recognition ended", 5)
        ended = page.evaluate(MIC_JS)
        desk.check(not ended["shown"] and ended["inputmode"] is None,
                   f"react: the badge or the kept-down keyboard outlived the recognition: {ended}")
        desk.shot("react-dictated")
        page.keyboard.press("Escape")
        desk.wait_for("() => document.getElementById('quick-reply').hidden", "react: Esc left the quick reply up")
        desk.wait_for("() => !document.getElementById('draft-chip').hidden", "react: the Draft chip does not count the reply")
        desk.check(sent_drafts is True, "react: a draft chip was up before React")
    else:
        state = page.evaluate("() => document.getElementById('quick-reply-state').textContent")
        desk.check("microphone on your keyboard" in str(state), f"react, no recognition: the box says {state!r}")
        # `#829`: nothing listens, so the keyboard, the way to speak here, is not kept down, and no badge shows.
        unheard = page.evaluate(MIC_JS)
        desk.check(unheard["inputmode"] is None and not unheard["shown"],
                   f"react, no recognition: the keyboard is kept down or the badge shows: {unheard}")
        page.keyboard.press("Escape")
    desk.check(label(page) == "Resume", f"react: the read did not stay paused: Read says {label(page)!r}")
    if voice == "agent":
        # `#828`: React pressed while the read plays pauses it on the sentence being heard and quotes that.
        press(page, "#read-aloud", touch)
        desk.wait_for("() => document.getElementById('read-aloud-label').textContent === 'Pause'",
                      "react: Resume did not carry the read on")
        wait_until(page, f"() => ({LIT_JS})() !== null", "react: no sentence is lit once the read carries on")
        page.evaluate("() => { window.recognitionLasts = 60000; }")
        press(page, "#read-react", touch)
        desk.wait_for("() => !document.getElementById('quick-reply').hidden", "react, playing: React opened no quick reply")
        desk.check(label(page) == "Resume", f"react, playing: React did not pause the read: Read says {label(page)!r}")
        lit = page.evaluate(LIT_JS)
        again = page.evaluate("() => document.getElementById('quick-reply-quote').textContent.replace(/\\s+/g, ' ').trim()")
        desk.check(lit is not None and again == lit, f"react, playing: the quick reply quotes {again!r}, not the lit sentence {lit!r}")
        desk.shot("react-playing")
        if recognition:
            # `#829`: a tap in the box brings the keyboard back and listens on; a tap on the badge stops it.
            desk.wait_for("() => window.recognitionStarts === 2", "react, playing: React started no recognition")
            check_listening(desk, "react, playing, listening")
            press(page, "#quick-reply-text", touch)
            tapped = page.evaluate(MIC_JS)
            desk.check(tapped["inputmode"] is None and tapped["shown"],
                       f"react: a tap in the box did not bring the keyboard back while listening on: {tapped}")
            press(page, "#quick-reply-mic", touch)
            desk.wait_for("() => document.getElementById('quick-reply-mic').hidden"
                          " && !document.getElementById('quick-reply').hasAttribute('data-listening')",
                          "react: a tap on the badge did not stop listening")
            stopped = page.evaluate("() => ({aborts: window.recognitionAborts || 0,"
                                    " open: !document.getElementById('quick-reply').hidden,"
                                    " focused: document.activeElement === document.getElementById('quick-reply-text'),"
                                    " inputmode: document.getElementById('quick-reply-text').getAttribute('inputmode')})")
            desk.check(stopped["aborts"] >= 1 and stopped["open"] and stopped["focused"] and stopped["inputmode"] is None,
                       f"react: the badge's tap did not stop only the listening, the box focused: {stopped}")
            desk.notes.append("listening kept the keyboard down under a corner badge that stopped it")
        page.keyboard.press("Escape")
        desk.notes.append(f"React while playing paused and quoted {str(lit)[:30]!r}")
    desk.notes.append(f"React {'quoted the held sentence ' + repr(held[:30]) if held else 'quoted the held block'}, "
                      + ("dictation landed in the box and the Draft chip counted it" if recognition else
                         "with no recognition the box focused and pointed at the keyboard's microphone"))


def run_react(chromium: BrowserType, args: argparse.Namespace, voice: str, size: tuple[int, int], touch: bool,
              recognition: bool) -> str:
    api = VoicesApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    width, height = size
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-react-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, color_scheme="dark", has_touch=touch, is_mobile=touch)
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.add_init_script(AUDIO_PROBE)
            if voice == "device":
                page.add_init_script(ANDROID_VOICE)
            page.add_init_script(FAKE_RECOGNITION if recognition else NO_RECOGNITION)
            desk = Desk(page, args.screenshots, f"react-{voice}-{width}", width, height)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            react_walk(desk, api, voice, touch, recognition)
            check(not errors, f"react {width}x{height}: the page threw: {errors}")
            context.close()
            return f"{version}, React at {width}x{height}, the {voice} voice: " + "; ".join(desk.notes)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


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
    fixture.WEB_ROOT = page_sources.served(args.web_root)
    report = []
    with sync_playwright() as playwright:
        for voice in (("agent", "device") if args.voice == "both" else (args.voice,)):
            report.append(run(playwright.chromium, args, voice))
        if args.voice == "both":
            for touch in (True, False):
                report.append(run_quote(playwright.chromium, args, touch))
            # `#821 voice-page-read-aloud-react`.
            # `#828 voice-page-read-nav-center`: and a narrow window with a mouse.
            for size, touch in (((360, 780), True), ((375, 812), True), ((412, 915), True), ((1280, 800), False),
                                ((400, 800), False)):
                report.append(run_react(playwright.chromium, args, "agent", size, touch, True))
            report.append(run_react(playwright.chromium, args, "agent", (412, 915), True, False))
            report.append(run_react(playwright.chromium, args, "device", (412, 915), True, True))
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
