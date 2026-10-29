#!/usr/bin/env python3
"""Read a message aloud from every view, by both voices, in real Chromium at 1280x800 with a mouse.

`#540 read-aloud-desktop`. The owner, 2026-10-26, on a desk: in the thread list the device voice said
the message "wasn't loaded"; there the chunks moved on the timeline and the message itself was not lit;
the thread cards could not be read at all; and the agent's voice raced its light through every block
with no sound, then archived the message as heard. Each read the page made looked its message up in the
left column's lists alone, and the thread list draws its rows in the cards' list and the right column in
its own. The voice that raced was the server's, answering with silence (its logs carry no speech onset
for any read that day).

It serves the checked-out page and the fake API of tests/two_column_browser.py (24 messages, three
threads: the runner's summarised, the replica's opened by a long first message), with two voices whose
clocks this check can read:

  * the agent's, behind the page's speech routes: each block a tone streamed at speaking pace, its
    length known (`BLOCK_SECONDS`); every Web Audio source the page starts is recorded with the clock
    of the context it plays on;
  * the device's, a `speechSynthesis` put in before the page loads: each utterance starts, and ends
    `DEVICE_MS_PER_CHAR` per character later, and is recorded with its text and times.

For each voice, with Read on, a real click on: in Main a message of three paragraphs and a thread's
root; in All a reply; in the thread list each card (the summarised one, the split one), an unpacked
reply, the summarised card's root under it, and a Main message among the cards; one thread across the
window; and the right column's thread in two columns. After each: the voice really started on that
message (the agent's: its ticket opened, sources started on a running context whose clock moves; the
device's: an utterance of the message's own words started), no error, and the light is on that message
where the reader sees it — the drawn row (or the card) marked as being read, and, for a message read in
blocks by the agent's voice, its first block lit and the sentence lit inside that drawn row. Over the
thread list the agent's read must take its blocks at the audio's pace: block 2 no sooner than about a
block's length after block 1.

THE NEGATIVE CONTROL is a message the agent's voice answers with silence: the read stops when the
block has arrived, says so, and archives nothing — where the page used to step on through the blocks
and archive it. A short message heard to its end is archived, so the control is not passing because
nothing is ever archived. (`#548 cloud-read-failures`: the stop names the cloud voice, offers Read with
device voice, and reports the failure to the server under the read's ticket.)

Pass --screenshots DIR to keep a PNG of each read, and --web-root DIR to run against another copy of
the page (the one before #540 fails at the first card, by either voice).
"""

from __future__ import annotations

import argparse
import json
import math
import os
import re
import struct
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, unquote, urlsplit

import offline_cache_browser as fixture
from inline_reply_browser import ReplyColumnsApi, sign_in
from offline_cache_browser import Json, check, handler_for
from two_column_browser import CANARY, REPLICA, RUNNER, Desk

from browser_audio import CHROMIUM_AUDIO_ARGS
import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

WIDTH, HEIGHT = 1280, 800
RATE = 24_000
CHUNK_MS = 50
# Each block's audio, in seconds, and the light's least stay on one: most of it, against a loaded host.
BLOCK_SECONDS = 1.0
BLOCK_STAY_AT_LEAST_S = 0.7
DEVICE_MS_PER_CHAR = 12

# Messages of three paragraphs, read in three blocks: in Main, a reply in the runner's thread, the
# runner's root (its card summarised, so the root is the first row under it), and the replica's root
# (long, so its card is split and the rest is the row under it).
PARAGRAPHS = ("Message {id} opens here. It has two sentences.",
              "The second paragraph says more about the overnight run. Then it stops.",
              "A third paragraph ends it. Done now.")
LONG_PARAGRAPHS = (
    "Before we cut the release, is the migration on the reporting replica finished?",
    "Whether the dashboards picked up the new partitions matters too, because last time that cost us a "
    "morning of reruns and an apology to the people who read them first thing.",
    "And the alert we silenced on Friday should be back on. Somebody please say so here when it is.")
MULTI = {"205": PARAGRAPHS, "206": PARAGRAPHS, "202": PARAGRAPHS, "209": LONG_PARAGRAPHS}
# Answered with silence by the agent's voice: the negative control.
SILENT = "211"
# A short message heard to its end, and archived.
SHORT = "203"
SHORT_SECONDS = 0.6

# A `speechSynthesis` with a clock: each utterance recorded with its text, and when it started and ended.
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
        }, %d * entry.text.length);
      }, 30);
    },
    cancel() { for (const entry of spoken) if (entry.ended === null) entry.cancelled = true; },
    pause() { this.paused = true; },
    resume() { this.paused = false; },
  };
  Object.defineProperty(window, 'speechSynthesis', {value: engine, configurable: true});
  window.SpeechSynthesisUtterance = function (text) { this.text = text; };
})();""" % DEVICE_MS_PER_CHAR

# Every Web Audio source the page starts, with the clock and state of its context then; and the contexts.
AUDIO_PROBE = """(() => {
  const sources = [];
  const contexts = [];
  window.audioProbe = {sources, contexts};
  const start = AudioBufferSourceNode.prototype.start;
  AudioBufferSourceNode.prototype.start = function (when = 0, ...rest) {
    sources.push({due: when, seconds: this.buffer ? this.buffer.duration : 0, clock: this.context.currentTime,
                  state: this.context.state, wall: performance.now()});
    return start.call(this, when, ...rest);
  };
  // What the strip counts, each time it changes, on the page's clock: the light's pace, whenever asked.
  const steps = [];
  window.steerSteps = steps;
  setInterval(() => {
    const strip = document.getElementById('read-steer');
    const count = strip && !strip.hidden ? document.getElementById('read-steer-count').textContent : '';
    if (steps.length === 0 || steps[steps.length - 1].count !== count) steps.push({count, at: performance.now()});
  }, 20);
  const Context = window.AudioContext;
  window.AudioContext = function (...args) { const made = new Context(...args); contexts.push(made); return made; };
  window.AudioContext.prototype = Context.prototype;
})();"""

# What the page shows of the read of message `id` (or of the card of thread `card`): where it is marked,
# drawn; the block lit and the sentence lit, by the row they are in; the error; the steering count.
READ_STATE_JS = """([id, card]) => {
  const drawn = (node) => node.getClientRects().length > 0 && !node.closest('[hidden]');
  const rows = [...document.querySelectorAll('li[data-reading="true"]')].filter(drawn);
  const where = (node) => { const li = node.closest('li[data-id]'); return li ? `${li.getAttribute('data-id')}@${li.parentNode.id}` : null; };
  const sentence = CSS.highlights.has('read-aloud-sentence') ? [...CSS.highlights.get('read-aloud-sentence')] : [];
  return {
    reading: rows.map(where),
    pending: [...document.querySelectorAll('li[data-pending="true"]')].filter(drawn).map(where),
    blocks: [...document.querySelectorAll('[data-reading-block="current"]')].filter(drawn).map(where),
    sentences: sentence.filter((range) => drawn(range.startContainer.parentElement)).map((range) =>
      ({at: where(range.startContainer.parentElement), text: String(range)})),
    card: card ? (document.querySelector(`#side-cards > li[data-card][data-id="${card}"]`) || {getAttribute: () => null}).getAttribute('data-reading') : null,
    error: document.getElementById('error').textContent,
    count: document.getElementById('read-steer').hidden ? '' : document.getElementById('read-steer-count').textContent,
  };
}"""


def paragraphs_of(message_id: str) -> tuple[str, ...]:
    return tuple(text.replace("{id}", message_id) for text in MULTI[message_id])


class VoicesApi(ReplyColumnsApi):
    """`two_column_browser.ColumnsApi` with the messages above, and the routes the agent's voice is read by."""

    def __init__(self) -> None:
        super().__init__()
        for message in self.messages:
            if str(message["id"]) in MULTI:
                texts = paragraphs_of(str(message["id"]))
                message["content"] = "\n\n".join(texts)
                message["content_html"] = "".join(f"<p>{text}</p>" for text in texts)
            if str(message["id"]) == SHORT:
                message["content"] = "Short and done."
        by_id = {str(message["id"]): message for message in self.messages}
        for summary in self.threads:
            root = summary["root"]
            assert isinstance(root, dict)
            summary["root"] = by_id[str(root["id"])]
        self.opened: list[tuple[str, int | None, float]] = []
        # `#548 cloud-read-failures`: what the page reported of a read the voice failed, by ticket.
        self.failures: list[tuple[str, Json]] = []

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope),
                "read_aloud": {"backend": "conversation", "label": "Check voice", "playback": "audio", "local_only": False}}

    def post(self, path: str, body: Json) -> tuple[int, Json]:
        if path.endswith("/speech/prepare"):
            ids = body.get("ids")
            prepared: list[Json] = []
            for one in ids if isinstance(ids, list) else []:
                entry: Json = {"message_id": str(one), "url": f"/api/v1/speech/ticket-{one}"}
                if str(one) in MULTI:
                    entry["blocks"] = [{"kind": "paragraph", "spoken": True, "chars": len(text)}
                                       for text in paragraphs_of(str(one))]
                prepared.append(entry)
            return 200, {"prepared": prepared, "expires_in_seconds": 600}
        return super().post(path, body)

    def tickets(self, message_id: str) -> list[tuple[int | None, float]]:
        with self.lock:
            return [(block, at) for one, block, at in self.opened if one == message_id]

    def archived(self) -> set[str]:
        with self.lock:
            return set(self.dismissed)


def handler(api: VoicesApi) -> type[BaseHTTPRequestHandler]:
    class Handler(handler_for(api)):  # type: ignore[misc]
        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            parts = urlsplit(self.path)
            ticket = re.fullmatch(r"/api/v1/speech/ticket-([0-9]+)", unquote(parts.path))
            if not ticket:
                super().do_GET()
                return
            block = parse_qs(parts.query).get("block", [None])[0]
            self.speech(ticket.group(1), None if block is None else int(block))

        def do_POST(self) -> None:  # noqa: N802
            failed = re.fullmatch(r"/api/v1/speech/ticket-([0-9]+)/failure", unquote(urlsplit(self.path).path))
            if failed:
                report = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"{}")
                with api.lock:
                    api.failures.append((failed.group(1), report))
                self.send_response(204)
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            if re.fullmatch(r"/api/v1/speech/ticket-[0-9]+/timing", unquote(urlsplit(self.path).path)):
                self.rfile.read(int(self.headers.get("Content-Length") or 0))
                self.send_response(204)
                self.send_header("Content-Length", "0")
                self.end_headers()
                return
            super().do_POST()

        def speech(self, message_id: str, block: int | None) -> None:
            with api.lock:
                api.opened.append((message_id, block, time.monotonic()))
            seconds = SHORT_SECONDS if message_id == SHORT else BLOCK_SECONDS
            loud = 0 if message_id == SILENT else 6000
            self.send_response(200)
            self.send_header("Content-Type", "audio/wav")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Transfer-Encoding", "chunked")
            self.end_headers()

            def chunk(data: bytes) -> None:
                self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
                self.wfile.flush()

            try:
                # Unknown length, as the server streams it.
                chunk(b"RIFF" + struct.pack("<I", 0xFFFFFFFF) + b"WAVEfmt "
                      + struct.pack("<IHHIIHH", 16, 1, 1, RATE, RATE * 2, 2, 16) + b"data" + struct.pack("<I", 0xFFFFFFFF))
                total = int(seconds * RATE)
                per = RATE * CHUNK_MS // 1000
                pitch = 300 + 40 * (block or 0)
                began = time.monotonic()
                sent = 0
                # At speaking pace: each piece no sooner than the audio before it has been said.
                while sent < total and not api.stopping.is_set():
                    count = min(per, total - sent)
                    chunk(struct.pack(f"<{count}h", *(int(loud * math.sin(2 * math.pi * pitch * (sent + i) / RATE))
                                                     for i in range(count))))
                    sent += count
                    delay = began + sent / RATE - time.monotonic()
                    if delay > 0:
                        time.sleep(delay)
                self.wfile.write(b"0\r\n\r\n")
                self.wfile.flush()
            except OSError:
                self.close_connection = True

    return Handler


class Walk:
    """One voice's walk through the views, and what it found."""

    def __init__(self, desk: Desk, api: VoicesApi, voice: str) -> None:
        self.desk = desk
        self.page = desk.page
        self.api = api
        self.voice = voice

    def state(self, message_id: str, card: str | None = None) -> Json:
        found: Json = self.page.evaluate(READ_STATE_JS, [message_id, card])
        return found

    def click(self, selector: str) -> None:
        """A real click on the first paragraph of `selector` (a row's body, or a card's words)."""
        node = self.page.query_selector(selector)
        self.desk.check(node is not None, f"{self.voice}: nothing to click at {selector}")
        assert node is not None
        node.scroll_into_view_if_needed()
        self.desk.settled()
        box = node.bounding_box()
        assert box is not None
        self.page.mouse.click(box["x"] + 12, box["y"] + min(10, box["height"] / 2))

    def wait_for(self, why: str, condition: str, seconds: float = 8) -> None:
        self.desk.wait_for(condition, f"{self.voice}: {why}", seconds)

    def read(self, label: str, selector: str, message_id: str, mark: str, card: str | None = None,
             blocks: bool = False) -> Json:
        """Click `selector`; the voice must start on `message_id` and the light land on `mark` (`id@list`)."""
        audio_before = self.page.evaluate("() => window.audioProbe.sources.length")
        device_before = self.page.evaluate("() => window.deviceSpeech.length")
        tickets_before = len(self.api.tickets(message_id))
        self.click(selector)
        if self.voice == "agent":
            deadline = time.monotonic() + 8
            while time.monotonic() < deadline and len(self.api.tickets(message_id)) <= tickets_before:
                self.page.wait_for_timeout(25)
            self.desk.check(len(self.api.tickets(message_id)) > tickets_before,
                            f"{label}: the agent's voice was never asked for {message_id}")
            self.wait_for(f"{label}: no audio started", f"() => window.audioProbe.sources.length > {audio_before}"
                          " && window.audioProbe.sources.slice(-1)[0].state === 'running'")
            clock = self.page.evaluate("() => window.audioProbe.contexts.slice(-1)[0].currentTime")
            self.page.wait_for_timeout(300)
            moved = self.page.evaluate("() => window.audioProbe.contexts.slice(-1)[0].currentTime") - clock
            self.desk.check(moved > 0.15, f"{label}: the audio clock stood still ({moved:.3f}s in 300ms)")
        else:
            # The message's first sentence, or more (`#554 read-pause-reply-resume`: one sentence an utterance).
            words = json.dumps(self.message_words(message_id))
            self.wait_for(f"{label}: the device voice did not start on {message_id}",
                          f"() => window.deviceSpeech.slice({device_before}).some((one) => one.started !== null"
                          f" && one.text.length >= 10 && {words}.startsWith(one.text))")
        if blocks and self.voice == "agent":
            self.wait_for(f"{label}: no block lit in the drawn row {mark}",
                          f"() => [...document.querySelectorAll('[data-reading-block=\"current\"]')].some((n) =>"
                          f" n.getClientRects().length > 0 && `${{n.closest('li[data-id]').getAttribute('data-id')}}@"
                          f"${{n.closest('li[data-id]').parentNode.id}}` === {json.dumps(mark)})")
            self.wait_for(f"{label}: no sentence lit in the drawn row {mark}",
                          "() => CSS.highlights.has('read-aloud-sentence') && [...CSS.highlights.get('read-aloud-sentence')]"
                          ".some((range) => { const li = range.startContainer.parentElement.closest('li[data-id]');"
                          f" return li && li.getClientRects().length > 0 && `${{li.getAttribute('data-id')}}@${{li.parentNode.id}}` === {json.dumps(mark)}; }})")
        found = self.state(message_id, card)
        self.desk.check(found["error"] == "", f"{label}: an error was shown: {found['error']!r}")
        reading = found["reading"]
        assert isinstance(reading, list)
        self.desk.check(mark in reading, f"{label}: the light is not on {mark}: {found}")
        if card is not None:
            self.desk.check(found["card"] == "true", f"{label}: the card of {card} is not marked as being read: {found}")
        self.desk.shot(f"{self.voice}-{label}")
        sentences = found["sentences"]
        said = f", sentence {sentences[0]['text']!r}" if isinstance(sentences, list) and sentences else ""
        self.desk.notes.append(f"{label}: {message_id} on {mark}{said}")
        return found

    def message_words(self, message_id: str) -> str:
        message = next(one for one in self.api.messages if str(one["id"]) == message_id)
        return " ".join(str(message["content"]).split())

    def view(self, value: str, ready: str, why: str) -> None:
        self.page.select_option("#thread-select", value)
        self.wait_for(why, ready)
        self.desk.settled()
        if self.page.evaluate("() => document.getElementById('read-aloud').getAttribute('data-active') !== 'true'"):
            self.page.click("#read-aloud")
            self.desk.settled()


def walk(walk_: Walk) -> None:
    page = walk_.page
    desk = walk_.desk
    sign_in(desk)
    if walk_.voice == "device":
        page.click("#audio-source")
        desk.wait_for("() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
                      "the audio switch did not choose the device voice")
    page.click("#read-aloud")
    desk.settled()
    blocks = walk_.voice == "agent"

    # All: a reply in the runner's thread.
    walk_.read("all-reply", "#discord-log > li[data-id='206'] .body", "206", "206@discord-log", blocks=blocks)

    # Main: a message of three paragraphs, and a thread's root.
    walk_.view("main", "() => !document.querySelector(\"#discord-log > li[data-id='206']\")"
               " && document.querySelector(\"#discord-log > li[data-id='205']\")", "Main did not open")
    walk_.read("main-message", "#discord-log > li[data-id='205'] .body", "205", "205@discord-log", blocks=blocks)
    walk_.read("main-root", "#discord-log > li[data-id='202'] .body", "202", "202@discord-log", blocks=blocks)

    # The thread list: each card, its first message read.
    walk_.view("threads", "() => document.getElementById('screen-main').getAttribute('data-columns') === 'list'"
               " && document.querySelectorAll('#side-cards > li[data-card]').length === 3", "the thread list did not open")
    kinds = page.evaluate("() => Object.fromEntries([...document.querySelectorAll('#side-cards > li[data-card]')].map((card) =>"
                          " [card.getAttribute('data-id'), card.hasAttribute('data-summarised-card') ? 'summary'"
                          " : card.hasAttribute('data-split-card') ? 'split' : 'other']))")
    desk.check(kinds.get(RUNNER) == "summary" and kinds.get(REPLICA) == "split",
               f"the runner's card is not summarised or the replica's not split: {kinds}")
    # (Not the summarised card first: its root, 202, is being read, and a press on what is being read stops it.)
    walk_.read("card-split", f"#side-cards > li[data-card][data-id='{REPLICA}'] .side-card-text", "209",
               f"{REPLICA}@side-cards", card=REPLICA)
    walk_.read("card-summary", f"#side-cards > li[data-card][data-id='{RUNNER}'] .side-card-text", "202",
               f"{RUNNER}@side-cards", card=RUNNER)
    walk_.read("card-plain", f"#side-cards > li[data-card][data-id='{CANARY}'] .side-card-text", "215",
               f"{CANARY}@side-cards", card=CANARY)

    # ...unpacked: a reply under its card, the summarised card's root, and a Main message among the cards.
    page.click("#side-unpack")
    walk_.wait_for("Unpack drew no reply under the runner's card",
                   f"() => document.querySelector(\"#side-cards > li[data-unpack-thread='{RUNNER}'][data-id='206']\")")
    desk.settled()
    found = walk_.read("unpacked-reply", f"#side-cards > li[data-unpack-thread='{RUNNER}'][data-id='206'] .body", "206",
                       "206@side-cards", blocks=blocks)
    if walk_.voice == "agent":
        # The light keeps to the audio's pace: block 2 lit about a block's length, by the page's own record
        # of the strip (`steerSteps`), so how long this check took to look does not count.
        walk_.wait_for("the read never reached its third block",
                       "() => window.steerSteps.slice(-1)[0].count === '3 of 3'", 8)
        steps = page.evaluate("() => { const all = window.steerSteps; let at = all.length - 1;"
                              " while (at > 0 && all[at].count !== '1 of 3') at -= 1; return all.slice(at); }")
        second = next((step for step in steps if step["count"] == "2 of 3"), None)
        third = next((step for step in steps if step["count"] == "3 of 3"), None)
        desk.check(second is not None and third is not None, f"the strip did not count 2 then 3: {steps}")
        assert second is not None and third is not None
        stay = (float(third["at"]) - float(second["at"])) / 1000
        desk.check(stay >= BLOCK_STAY_AT_LEAST_S, f"the light left block 2 after {stay:.2f}s of a {BLOCK_SECONDS}s block")
        desk.notes.append(f"block 2 lit {stay:.2f}s of its {BLOCK_SECONDS}s")
        desk.check(found["count"] in ("1 of 3", "2 of 3"), f"the strip did not count the blocks: {found}")
    walk_.read("unpacked-root", f"#side-cards > li[data-unpack-thread='{RUNNER}'][data-root-child] .body", "202",
               "202@side-cards", card=RUNNER, blocks=blocks)
    # The chip goes round packed, unread, all: back to packed.
    for _ in range(3):
        if page.evaluate("() => document.getElementById('side-unpack').textContent.trim()") == "Pack all":
            break
        page.click("#side-unpack")
    desk.check(page.evaluate("() => document.getElementById('side-unpack').textContent.trim()") == "Pack all",
               "the Unpack chip never came back to Pack all")
    page.click("#side-main")
    walk_.wait_for("Main channel put no message among the cards",
                   "() => document.querySelector(\"#side-cards > li[data-main-row][data-id='205']\")")
    desk.settled()
    walk_.read("among-cards", "#side-cards > li[data-main-row][data-id='205'] .body", "205", "205@side-cards", blocks=blocks)
    page.click("#side-main")

    # One thread across the window.
    walk_.view(f"thread:{RUNNER}", f"() => document.querySelectorAll('#discord-log > li[data-id]').length === 9",
               "the runner's thread did not open")
    walk_.read("thread-reply", "#discord-log > li[data-id='206'] .body", "206", "206@discord-log", blocks=blocks)

    # Two columns, the runner's thread on the right.
    # (The thread just open across the window comes back on the right; else its card opens it.)
    walk_.view("columns", "() => document.getElementById('side-column').getClientRects().length > 0",
               "two columns did not open")
    if page.evaluate("() => document.querySelectorAll('#side-log > li[data-id]').length") != 9:
        page.click(f"#side-cards > li[data-id='{RUNNER}'] .side-card-open")
    walk_.wait_for("the runner's card did not open its thread on the right",
                   "() => document.querySelectorAll('#side-log > li[data-id]').length === 9")
    desk.settled()
    walk_.read("right-column", "#side-log > li[data-id='206'] .body", "206", "206@side-log", blocks=blocks)

    if walk_.voice == "agent":
        controls(walk_)


def controls(walk_: Walk) -> None:
    """A short message heard to its end is archived; one the voice answers with silence stops and is not."""
    desk = walk_.desk
    walk_.click("#side-log > li[data-id='203'] .body" if desk.page.query_selector("#side-log > li[data-id='203']")
                else "#discord-log > li[data-id='203'] .body")
    deadline = time.monotonic() + 8
    while time.monotonic() < deadline and SHORT not in walk_.api.archived():
        desk.page.wait_for_timeout(50)
    desk.check(SHORT in walk_.api.archived(), "a short message heard to its end was not archived")
    walk_.click(f"#side-log > li[data-id='{SILENT}'] .body")
    walk_.wait_for("a read answered with silence did not stop and say so",
                   "() => /^Cloud voice isn't producing audio: it sent silence/.test(document.getElementById('error').textContent)", 8)
    # `#548 cloud-read-failures`: the device voice offered for that message, and the failure told to the server.
    desk.check(desk.page.evaluate("() => !document.getElementById('read-on-device').hidden"),
               "a read answered with silence did not offer Read with device voice")
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline and not walk_.api.failures:
        desk.page.wait_for_timeout(50)
    with walk_.api.lock:
        failures = list(walk_.api.failures)
    desk.check([(one, report.get("reason")) for one, report in failures] == [(SILENT, "silent")],
               f"the silent read was not reported to the server as silent: {failures}")
    desk.page.wait_for_timeout(500)
    found = walk_.state(SILENT)
    desk.check(found["reading"] == [] and found["pending"] == [], f"the silent read is still lit: {found}")
    desk.check(SILENT not in walk_.api.archived(), "a message read in silence was archived as heard")
    desk.check(len(walk_.api.tickets(SILENT)) == 1, f"the silent read went on asking: {walk_.api.tickets(SILENT)}")
    desk.shot("agent-silent")
    desk.notes.append("silence stopped the read unarchived; a short read to its end archived")


def run(chromium: BrowserType, args: argparse.Namespace, voice: str) -> str:
    api = VoicesApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-read-views-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                args=CHROMIUM_AUDIO_ARGS,
                viewport={"width": WIDTH, "height": HEIGHT}, color_scheme="dark")
            version = context.browser.version if context.browser else "Chromium"
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.add_init_script(AUDIO_PROBE)
            page.add_init_script(DEVICE_VOICE)
            desk = Desk(page, args.screenshots, f"read-views-{voice}", WIDTH, HEIGHT)
            page.goto(f"http://127.0.0.1:{server.server_port}/", wait_until="load")
            walk(Walk(desk, api, voice))
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
    parser.add_argument("--screenshots", type=Path, help="directory to write one PNG per read into (default: none)")
    parser.add_argument("--voice", choices=("agent", "device", "both"), default="both",
                        help="which voice to walk the views with (default: both, one browser each)")
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
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
