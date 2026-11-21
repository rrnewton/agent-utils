"""The Documents reader reading aloud, against the REAL server (`#1039` stage 3).

Modelled on documents_browser.py's seeding (the release binary with
--fake-discord; a paste link served by a local cat command) with the audio
plumbing of the read-aloud browser checks: a fake `speechSynthesis` put in
before the page loads records every device utterance, and the Media Session
handlers the page installs are captured as they are set. The cloud arm is
real too: the server runs `read_aloud.backend = "local"` against a fake
sidecar on loopback (the local_tts_sidecar.py protocol: GET /voices,
POST /synthesize answered with a one-second tone), which records every text
it is asked to say — so the bar's device/cloud toggle flipping the NEXT
document read's voice is checked at both ends, not just its state.

At 393x852 (touch) and again at 1280x800 (mouse): open the document, flip
the toggle to the device voice, tap Read — the steering strip appears
counting its blocks, the device voice says the document's words a block at
a time, Next and Previous step it, the media nexttrack steps it, and Next
on the last block finishes it (the strip clears). Flipping the toggle back
makes the next read the server voice's: the sidecar is asked for the
document's words and the device says nothing new. Back stops a read.

The block being read is lit in the reader and kept in view (`#1330
doc-read-highlight`), as a message's block is lit: starting Read lights the
first block, Next moves the light, a scrubber-chunk jump to the last block (off
screen in a document this long) brings the lit block inside the scroller,
and finishing, stopping and Back all take the light off.
"""

import argparse
import io
import json
import math
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
import wave
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Literal, cast

from playwright.sync_api import Browser, BrowserContext, Page, sync_playwright

from browser_audio import CHROMIUM_AUDIO_ARGS

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "documents-read-check-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-read-check-write-token-not-a-real-credential"
# One document per viewport, differently worded: the server caches synthesized
# speech, so a second walk over the same words would be served from the cache
# and the sidecar would honestly have nothing new to record.
#
# Twelve blocks, each one sentence under the device voice's utterance cap, so
# a block is said in one utterance and the document stands well taller than
# either viewport: the last block starts off screen, which is what makes the
# reading light's follow observable (`#1330 doc-read-highlight`).
def make_blocks(label: str) -> list[str]:
    return [
        f"Block {number} of the {label} document carries its own plain words"
        " along the page so the reading light has somewhere far to travel"
        " before it rests here."
        for number in range(1, 13)
    ]


DOCS: list[tuple[str, list[str]]] = [
    ("read-doc", make_blocks("reading")),
    ("desk-doc", make_blocks("desk")),
]
# Slow enough that no block ends by itself while the walk steps it: every
# move the walk sees is one it asked for. Cancellation is the owner's to do.
DEVICE_MS_PER_CHAR = 400

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

# Records the Media Session handlers the page installs, keeping the real session working.
MEDIA_PROBE = """(() => {
  const session = navigator.mediaSession;
  if (!session) return;
  const handlers = {};
  window.mediaProbe = { handlers };
  const set = session.setActionHandler.bind(session);
  session.setActionHandler = (action, fn) => { handlers[action] = fn; return set(action, fn); };
})();"""

OWNER_STATE = """() => {
  const owner = readAloudOwner();
  const playing = owner.nowPlaying;
  return {
    id: playing ? playing.id : null,
    kind: playing ? (playing.kind || null) : null,
    pending: owner.pendingRead,
  };
}"""


def tone_wav(seconds: float = 1.0, rate: int = 16000) -> bytes:
    """One second of unmistakable tone, as the sidecar answers a synthesis."""
    frames = b"".join(
        struct.pack("<h", int(8000 * math.sin(2 * math.pi * 440 * i / rate)))
        for i in range(int(seconds * rate))
    )
    out = io.BytesIO()
    with wave.open(out, "wb") as wav:
        wav.setnchannels(1)
        wav.setsampwidth(2)
        wav.setframerate(rate)
        wav.writeframes(frames)
    return out.getvalue()


TONE = tone_wav()


class SidecarHandler(BaseHTTPRequestHandler):
    """The local-TTS sidecar's protocol, with every asked-for text recorded."""

    texts: list[str] = []
    lock = threading.Lock()

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
        if self.path == "/voices":
            self.reply_json(200, {"voices": [{"name": "check-voice", "locale": "en"}]})
        elif self.path == "/health":
            self.reply_json(200, {"ok": True, "engine": "check"})
        else:
            self.reply_json(404, {"error": "not found"})

    def do_POST(self) -> None:  # noqa: N802
        if self.path != "/synthesize":
            self.reply_json(404, {"error": "not found"})
            return
        length = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(length) or b"{}")
        with SidecarHandler.lock:
            SidecarHandler.texts.append(str(body.get("q") or ""))
        self.send_response(200)
        self.send_header("Content-Type", "audio/wav")
        self.send_header("Content-Length", str(len(TONE)))
        self.end_headers()
        self.wfile.write(TONE)

    def reply_json(self, status: int, payload: dict[str, object]) -> None:
        data = json.dumps(payload).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, format: str, *args: object) -> None:  # noqa: A002 - the base name
        pass


def sidecar_texts() -> list[str]:
    with SidecarHandler.lock:
        return list(SidecarHandler.texts)


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port: int = sock.getsockname()[1]
        return port


def call(base: str, method: str, path: str, token: str, body: dict[str, object] | None = None) -> dict[str, object]:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method)
    req.add_header("Authorization", f"Bearer {token}")
    if data:
        req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, timeout=15) as res:
        parsed: object = json.loads(res.read() or b"null")
    assert isinstance(parsed, dict)
    return parsed


def wait_count(page: Page, want: str) -> None:
    page.wait_for_function(
        """(want) => {
          const strip = document.getElementById('read-steer');
          const count = document.getElementById('read-steer-count');
          return !!strip && !strip.hidden && !!count && count.textContent.trim() === want;
        }""",
        arg=want,
        timeout=20_000,
    )


def wait_stopped(page: Page) -> None:
    page.wait_for_function(
        """() => {
          const owner = readAloudOwner();
          const strip = document.getElementById('read-steer');
          return owner.nowPlaying === null && owner.pendingRead === null && !!strip && strip.hidden;
        }""",
        timeout=20_000,
    )


def wait_lit(page: Page, block: str) -> None:
    """The one block lit in the reader is `block` (`#1330 doc-read-highlight`)."""
    page.wait_for_function(
        """(words) => {
          const lit = [...document.querySelectorAll('#doc-body [data-reading-block="current"]')];
          return lit.length === 1 && lit[0].textContent.trim().startsWith(words);
        }""",
        arg=block[:30],
        timeout=20_000,
    )


def wait_unlit(page: Page) -> None:
    """No block in the reader carries the reading light any more."""
    page.wait_for_function(
        "() => document.querySelectorAll('#doc-body [data-reading-block]').length === 0",
        timeout=20_000,
    )


def wait_lit_in_view(page: Page) -> None:
    """The lit block's box stands inside the scroller's box."""
    page.wait_for_function(
        """() => {
          const lit = document.querySelector('#doc-body [data-reading-block="current"]');
          const area = document.getElementById('scroll-area');
          if (!lit || !area) return false;
          const box = lit.getBoundingClientRect();
          const view = area.getBoundingClientRect();
          return box.top >= view.top - 1 && box.bottom <= view.bottom + 1;
        }""",
        timeout=20_000,
    )


def block_off_screen(page: Page, block: str) -> bool:
    """Whether the drawn block saying `block` stands outside the scroller."""
    off: bool = page.evaluate(
        """(words) => {
          const area = document.getElementById('scroll-area');
          const drawn = [...document.querySelectorAll('#doc-body > p')];
          const el = drawn.find((one) => one.textContent.trim().startsWith(words));
          if (!el || !area) return false;
          const box = el.getBoundingClientRect();
          const view = area.getBoundingClientRect();
          return box.bottom <= view.top || box.top >= view.bottom;
        }""",
        block[:30],
    )
    return off


def owner_state(page: Page) -> dict[str, object]:
    state: dict[str, object] = page.evaluate(OWNER_STATE)
    return state


def utterances(page: Page) -> list[str]:
    said: list[str] = page.evaluate("() => window.deviceSpeech.map((u) => u.text)")
    return said


def walk(page: Page, base: str, touch: bool, name: str, blocks: list[str]) -> None:
    """One reader's read-aloud walk, the same by touch and by mouse."""

    def act(selector: str) -> None:
        if touch:
            page.tap(selector)
        else:
            page.click(selector)

    page.goto(base + "/?v=docs", wait_until="load")
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    card = page.locator("#documents-list .doc-card", has_text=name)
    if touch:
        card.tap()
    else:
        card.click()
    page.wait_for_function(
        "(words) => (document.getElementById('doc-body')?.textContent || '').includes(words)",
        arg=blocks[0][:20],
        timeout=20_000)
    # `#1515 page-update-reload-loses-controls`: the reader's transport
    # pane stands over the reader in every state, read or no read.
    check(page.locator("#control-pane").is_visible(), "the reader's controls do not stand over the reader")

    # The device/cloud toggle is the Documents view's too now, starting on the
    # server voice; one tap picks the device for the read that follows. The
    # picked source is decided asynchronously (the server's client-config
    # answer, then any synced choice it carries), so wait for that decision
    # to land on the toggle instead of reading it mid-flight: under load the
    # document can render before the decision does (`#1549
    # documents-read-browser-voice-premise-flake`).
    page.wait_for_function(
        """() => {
          const toggle = document.getElementById('audio-source');
          return !!toggle && !toggle.hidden && toggle.getAttribute('aria-checked') === 'true';
        }""",
        timeout=20_000)
    toggle = page.locator("#audio-source")
    check(toggle.is_visible(), "the audio source toggle is not offered over the Documents view")
    check(toggle.get_attribute("aria-checked") == "true",
          "the premise: the server voice is not the picked source")
    act("#audio-source")
    page.wait_for_function(
        "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
        timeout=20_000)

    # `#2003 vibe-talk-read-starts-on-screen`: Read starts at the first
    # meaningful visible block with no scroll change, Pause beside it,
    # and the header's own Read button is gone.
    check(page.locator("#doc-read").count() == 0,
          "the redundant header Read button still stands")
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 400; }")
    before = page.evaluate(
        """() => {
          const area = document.getElementById('scroll-area');
          const view = area.getBoundingClientRect();
          const blocks = [...document.querySelectorAll('#doc-body > p')];
          let first = -1;
          blocks.forEach((el, at) => {
            if (first !== -1) return;
            const r = el.getBoundingClientRect();
            const visible = Math.min(r.bottom, view.bottom) - Math.max(r.top, view.top);
            if (visible > 0 && visible >= Math.min(40, (r.bottom - r.top) / 2)) first = at;
          });
          return { top: area.scrollTop, first };
        }""")
    check(before["first"] > 0,
          f"the premise: no later block is first on screen after scrolling: {before}")
    act("#read-aloud")
    wait_lit(page, blocks[before["first"]])
    after_top = page.evaluate("() => document.getElementById('scroll-area').scrollTop")
    check(after_top == before["top"],
          f"Read moved the scroll: {before['top']} -> {after_top}")
    check(page.locator("#read-aloud-label").inner_text().strip() == "Pause",
          "Pause does not stand beside the started read")
    act("#read-aloud-stop")
    wait_stopped(page)
    wait_unlit(page)
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
    # The walk below counts the device voice from silence: forget this
    # segment's utterances, as if Read had not been pressed yet.
    page.evaluate("() => { window.deviceSpeech.length = 0; }")

    count = len(blocks)
    # `#1543`: inactive — tapping document text starts no read.
    inactive_block = page.locator("#doc-body > p", has_text=blocks[1][:20])
    if touch:
        inactive_block.tap()
    else:
        inactive_block.click()
    state = owner_state(page)
    check(state["id"] is None and state["pending"] is None,
          f"an inactive tap on document text started a read: {state}")
    check(page.locator("#read-steer").is_hidden(), "an inactive tap on document text showed the read strip")
    check(utterances(page) == [], "an inactive tap on document text spoke")
    # Read: the strip counts the blocks, the device voice says them in order,
    # and the read is the owner's document read — no queue of the reader's own.
    # The block being read is lit in the reader (`#1330 doc-read-highlight`).
    act("#read-aloud")
    wait_count(page, f"1 of {count}")
    wait_lit(page, blocks[0])
    wait_lit_in_view(page)
    state = owner_state(page)
    check(state["id"] == f"doc:paste:{name}", f"Read started {state}, not the document read")
    check(state["kind"] == "document", f"the playing read is not a document: {state}")
    page.wait_for_function("() => window.deviceSpeech.length >= 1", timeout=20_000)
    check(utterances(page) == [blocks[0]], f"the device voice said {utterances(page)}, not the first block alone")
    handlers: list[str] = page.evaluate(
        "() => Object.keys(window.mediaProbe.handlers).filter((k) => window.mediaProbe.handlers[k]).sort()")
    check(handlers == ["nexttrack", "pause", "play", "previoustrack", "stop"],
          f"the document read holds media handlers {handlers}")
    # `#1543`: active — tapping document text jumps the read there, and the
    # scrubber's first chunk brings it back for the steps that follow.
    # (`#1541`: a document read's jumps ride the navigator's chunk row in
    # the scrubber; the flat per-block timeline stands down for documents.)
    active_block = page.locator("#doc-body > p", has_text=blocks[1][:20])
    if touch:
        active_block.tap()
    else:
        active_block.click()
    wait_count(page, f"2 of {count}")
    wait_lit(page, blocks[1])
    page.wait_for_function("() => window.deviceSpeech.length >= 2", timeout=20_000)
    check(utterances(page)[-1] == blocks[1], "an active tap on document text did not jump the read there")
    act('#doc-nav .doc-nav-chunks .doc-nav-chunk:text-is("1")')
    wait_count(page, f"1 of {count}")
    wait_lit(page, blocks[0])

    # Pause and Resume from the pane, over the reader (`#1505`): the pane
    # stands while the read does — where it used to leave with the reader
    # and take play/pause with it — the button says the act, the read
    # holds, and the strip says it is held.
    check(page.locator("#control-pane").is_visible(), "the call controls leave during the document read")
    check(page.locator("#read-aloud-label").inner_text().strip() == "Pause",
          "the pane's Read does not say Pause over the document read")
    act("#read-aloud")
    page.wait_for_function(
        "() => readAloudOwner().nowPlaying && readAloudOwner().nowPlaying.paused === true",
        timeout=20_000)
    check(page.evaluate("() => document.getElementById('read-steer').getAttribute('data-paused')") == "true",
          "the strip does not say the document read is held")
    act("#read-aloud")
    page.wait_for_function(
        "() => readAloudOwner().nowPlaying && readAloudOwner().nowPlaying.paused === false",
        timeout=20_000)
    check(page.locator("#read-aloud-label").inner_text().strip() == "Pause",
          "the resumed document read does not say Pause again")

    act("#read-next")
    wait_count(page, f"2 of {count}")
    wait_lit(page, blocks[1])
    page.wait_for_function("() => window.deviceSpeech.length >= 2", timeout=20_000)
    check(utterances(page)[-1] == blocks[1], "Next did not say the second block")
    page.evaluate("() => window.mediaProbe.handlers.nexttrack({action: 'nexttrack'})")
    wait_count(page, f"3 of {count}")
    wait_lit(page, blocks[2])
    page.wait_for_function("() => window.deviceSpeech.length >= 3", timeout=20_000)
    check(utterances(page)[-1] == blocks[2], "the media Next did not say the third block")
    # A jump to the last block, off screen until now: the light lands on it
    # and the reader follows it into the scroller's view (`#1330`).
    check(block_off_screen(page, blocks[-1]),
          "the premise: the last block already stands in view before the jump")
    act(f'#doc-nav .doc-nav-chunks .doc-nav-chunk:text-is("{count}")')
    wait_count(page, f"{count} of {count}")
    wait_lit(page, blocks[-1])
    wait_lit_in_view(page)
    # On the last block the strip's Next stays offered, as a message's does:
    # pressing it finishes the read (`#1035 read-next-finish`).
    check(page.evaluate("() => document.getElementById('read-next').disabled") is False,
          "Next is not offered on the document's last block")
    act("#read-prev")
    wait_count(page, f"{count - 1} of {count}")
    wait_lit(page, blocks[-2])
    act("#read-next")
    wait_count(page, f"{count} of {count}")
    wait_lit(page, blocks[-1])
    said_before_finish = len(utterances(page))
    # The media Next on the last block finishes the read (`#1035`): strip
    # gone, owner idle, and the reading light off with it (`#1330`).
    page.evaluate("() => window.mediaProbe.handlers.nexttrack({action: 'nexttrack'})")
    wait_stopped(page)
    wait_unlit(page)
    check(len(utterances(page)) == said_before_finish, "finishing the read said something after it")
    # The strip's own Next on the last block finishes it the same way, with a
    # real press (Playwright tap/click, no wall-clock wait: `wait_stopped`
    # waits on the owner's read state). Back at the document's top first:
    # Read starts at the first visible block (`#2003`), and the read just
    # finished left the last block in view.
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
    act("#read-aloud")
    wait_count(page, f"1 of {count}")
    act(f'#doc-nav .doc-nav-chunks .doc-nav-chunk:text-is("{count}")')
    wait_count(page, f"{count} of {count}")
    said_before_finish = len(utterances(page))
    act("#read-next")
    wait_stopped(page)
    wait_unlit(page)
    check(len(utterances(page)) == said_before_finish, "finishing the read with Next said something after it")

    # Flip back: the next read is the server voice's — the sidecar is asked
    # for the document's words, the strip counts cloud blocks, the device rests.
    said = len(utterances(page))
    asked = len(sidecar_texts())
    act("#audio-source")
    page.wait_for_function(
        "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'true'",
        timeout=20_000)
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
    act("#read-aloud")
    page.wait_for_function(
        """(tail) => {
          const strip = document.getElementById('read-steer');
          const count = document.getElementById('read-steer-count');
          return !!strip && !strip.hidden && !!count && count.textContent.trim().endsWith(tail);
        }""",
        arg=f"of {count}",
        timeout=20_000)
    # The server voice's read lights its block in the reader too (`#1330`).
    wait_lit(page, blocks[0])
    deadline = time.time() + 20
    while len(sidecar_texts()) <= asked and time.time() < deadline:
        time.sleep(0.1)
    fresh = sidecar_texts()[asked:]
    check(len(fresh) >= 1, "the server voice was never asked to say the document")
    check(any(blocks[0] in text for text in fresh),
          f"the server voice was asked for {fresh}, not the document's opening")
    check(len(utterances(page)) == said, "the server read also spoke on the device")
    state = owner_state(page)
    check(state["kind"] == "document", f"the cloud read is not the document's: {state}")
    act("#read-aloud-stop")
    wait_stopped(page)
    wait_unlit(page)

    # Back stops a document read where it stands, its light with it; and a
    # document opened afresh carries no light from the last read (`#1330`).
    act("#audio-source")
    page.wait_for_function(
        "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
        timeout=20_000)
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
    act("#read-aloud")
    wait_count(page, f"1 of {count}")
    wait_lit(page, blocks[0])
    act("#doc-back")
    wait_stopped(page)
    wait_unlit(page)
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    card = page.locator("#documents-list .doc-card", has_text=name)
    if touch:
        card.tap()
    else:
        card.click()
    page.wait_for_function(
        "(words) => (document.getElementById('doc-body')?.textContent || '').includes(words)",
        arg=blocks[0][:20],
        timeout=20_000)
    check(page.evaluate("() => document.querySelectorAll('#doc-body [data-reading-block]').length") == 0,
          "a freshly opened document still carries the last read's light")
    act("#doc-back")
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)


# The list document (`#1505`): a heading, an intro, a long bulleted list
# with a nested item, a code fence and a closing paragraph — the shape the
# highlight drift was reported on. Spoken (device) a block at a time, each
# block one short utterance, so the lit block and the spoken block can be
# compared at every step.
LIST_ITEMS = [f"List item {number} carries plain words" for number in range(1, 11)]
LIST_DOC = (
    "# List notes\n\nIntro paragraph before the list.\n\n"
    + "\n".join(f"- {item}" for item in LIST_ITEMS[:5])
    + "\n  - nested item under five\n"
    + "\n".join(f"- {item}" for item in LIST_ITEMS[5:])
    + "\n\n```\nconst answer = 42;\n```\n\nClosing paragraph after the code.\n"
)
LIST_BLOCKS = (
    ["List notes", "Intro paragraph before the list."]
    + LIST_ITEMS[:5] + ["nested item under five"] + LIST_ITEMS[5:]
    + ["const answer = 42;", "Closing paragraph after the code."]
)


def lit_words(page: Page) -> str:
    """The lit block's words, whitespace flattened, as the model speaks them."""
    words: str = page.evaluate(
        """() => {
          const lit = document.querySelector('#doc-body [data-reading-block="current"]');
          return lit ? lit.textContent.replace(/\\s+/g, ' ').trim() : '';
        }""")
    return words


def forget_synced_source(base: str) -> None:
    """Forget any synced voice-source choice the server holds (`#1549`).

    The walks share one server, so one walk's final toggle would otherwise
    be carried into the next walk's fresh browser context by the settings
    sync (`#457 settings-server-side`) and legitimately picked there —
    racing the next walk's premise about the default source. Each walk
    starts from the shipped default instead.
    """
    call(base, "PUT", "/api/v1/client-settings", WRITE_TOKEN,
         {"set": [{"key": "vibe-talk.voice.read-audio-source", "value": None, "updated_ms": 0}],
          "seed": []})


def walk_list(page: Page, base: str) -> None:
    """The list document by touch: lit == spoken through the list, tap-to-read."""
    page.goto(base + "/?v=docs", wait_until="load")
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    page.locator("#documents-list .doc-card", has_text="list-doc").tap()
    page.wait_for_function(
        "(words) => (document.getElementById('doc-body')?.textContent || '').includes(words)",
        arg="nested item under five", timeout=20_000)
    # Wait for the voice-source decision (server capability known, toggle
    # drawn) before reading the toggle, as in walk() (`#1549`).
    page.wait_for_function(
        """() => {
          const toggle = document.getElementById('audio-source');
          return !!toggle && !toggle.hidden && readAloudOwner().agentReadAloud !== null
            && readAloudOwner().agentReadAloud !== undefined;
        }""",
        timeout=20_000)
    toggle = page.locator("#audio-source")
    if toggle.get_attribute("aria-checked") == "true":
        page.tap("#audio-source")
        page.wait_for_function(
            "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
            timeout=20_000)
    count = len(LIST_BLOCKS)
    page.tap("#read-aloud")
    wait_count(page, f"1 of {count}")
    # Step the whole list: at every block the voice's latest words are the
    # lit block's words — asserted on the read's own state, never on time.
    for at in range(count):
        page.wait_for_function(
            """(words) => {
              const said = window.deviceSpeech;
              return said.length > 0 && said[said.length - 1].text === words;
            }""",
            arg=LIST_BLOCKS[at], timeout=20_000)
        said = utterances(page)
        check(said[-1] == LIST_BLOCKS[at],
              f"block {at + 1}: the voice said {said[-1]!r}, not {LIST_BLOCKS[at]!r}")
        check(lit_words(page) == LIST_BLOCKS[at],
              f"block {at + 1}: the light is on {lit_words(page)!r} while the voice says {LIST_BLOCKS[at]!r}")
        if at + 1 < count:
            page.tap("#read-next")
            wait_count(page, f"{at + 2} of {count}")
    page.evaluate("() => readAloudOwner().stopReading()")
    wait_stopped(page)
    wait_unlit(page)
    # `#1543`: stopped — a tap on text starts nothing again.
    said_before = len(utterances(page))
    page.locator("#doc-body > p", has_text="Closing paragraph").tap()
    state = owner_state(page)
    check(state["id"] is None and state["pending"] is None,
          f"a tap on document text after the read stopped started a read: {state}")
    check(len(utterances(page)) == said_before, "a tap after the read stopped spoke")
    # Tap-to-read while active (`#1505`, `#1543`): Read restarts the read,
    # then a tap on the closing paragraph jumps there, and a tap on a list
    # item jumps to that item.
    page.tap("#read-aloud")
    wait_count(page, f"1 of {count}")
    said_before = len(utterances(page))
    page.locator("#doc-body > p", has_text="Closing paragraph").tap()
    wait_count(page, f"{count} of {count}")
    page.wait_for_function(
        "(n) => window.deviceSpeech.length > n", arg=said_before, timeout=20_000)
    check(utterances(page)[-1] == LIST_BLOCKS[-1],
          f"a tap on the closing paragraph said {utterances(page)[-1]!r}")
    check(lit_words(page) == LIST_BLOCKS[-1], "the tapped paragraph is not the lit block")
    page.locator("#doc-body li", has_text="List item 3").tap()
    wait_count(page, "5 of " + str(count))
    page.wait_for_function(
        "(words) => window.deviceSpeech.some((u) => u.text === words)",
        arg="List item 3 carries plain words", timeout=20_000)
    check(lit_words(page) == "List item 3 carries plain words",
          "the tapped list item is not the lit block")
    page.tap("#doc-back")
    wait_stopped(page)
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)


def write_build_stamp(folder: Path) -> None:
    """The shot folder's BUILD.txt: what build the shots show."""
    def git(*argv: str) -> str:
        return subprocess.run(["git", "-C", str(Path(__file__).resolve().parent.parent.parent), *argv],
                              capture_output=True, text=True).stdout.strip() or "unknown"

    folder.mkdir(parents=True, exist_ok=True)
    (folder / "BUILD.txt").write_text(
        f"commit {git('rev-parse', 'HEAD')}\n"
        f"commit-date {git('log', '-1', '--format=%cI', 'HEAD')}\n"
        "server target/release/vibe-talk (this checkout)\n",
        encoding="utf-8",
    )


def screenshot_matrix(browser: Browser, base: str, out: Path) -> None:
    """The #2004/#2003 shot matrix (coordb, 2026-11-20): Documents and a
    channel, before and after pressing Read with the lit block visible,
    light and dark at phone 412 and desktop 1280, Discuss open on the
    phone, and the lit block in every colour preset. Photographic only:
    the behaviour is asserted by the walks above; bar presses that an
    open panel would cover go through the element's own click.
    """
    name, blocks = DOCS[0]

    def stamp(*parts: str) -> Path:
        folder = out.joinpath(*parts)
        write_build_stamp(folder)
        return folder

    def new_page(ctx: BrowserContext) -> Page:
        page = ctx.new_page()
        page.add_init_script(
            f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
        page.add_init_script(DEVICE_VOICE)
        page.add_init_script(MEDIA_PROBE)
        return page

    def pick_device(page: Page) -> None:
        page.wait_for_function(
            "() => { const t = document.getElementById('audio-source');"
            " return !!t && !t.hidden && t.getAttribute('aria-checked') !== null; }",
            timeout=20_000)
        if page.evaluate("() => document.getElementById('audio-source').getAttribute('aria-checked')") == "true":
            page.evaluate("() => document.getElementById('audio-source').click()")
            page.wait_for_function(
                "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
                timeout=20_000)

    def open_document(page: Page) -> None:
        page.goto(base + "/?v=docs", wait_until="load")
        page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
        page.locator("#documents-list .doc-card", has_text=name).first.click()
        page.wait_for_function(
            "(words) => (document.getElementById('doc-body')?.textContent || '').includes(words)",
            arg=blocks[0][:20], timeout=20_000)
        pick_device(page)

    def open_channel(page: Page) -> None:
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
        page.wait_for_selector("#discord-log > li[data-id]", state="visible", timeout=20_000)

    def press(page: Page, element_id: str) -> None:
        page.evaluate(f"() => document.getElementById('{element_id}').click()")

    schemes: tuple[Literal["dark", "light"], ...] = ("dark", "light")
    for scheme in schemes:
        for label, width, height, touch in (("phone412", 412, 915, True), ("desktop1280", 1280, 800, False)):
            ctx = browser.new_context(
                viewport={"width": width, "height": height},
                has_touch=touch, is_mobile=touch, color_scheme=scheme)
            page = new_page(ctx)
            docs = stamp("documents")
            open_document(page)
            page.screenshot(path=str(docs / f"documents-{scheme}-{label}-before.png"))
            press(page, "read-aloud")
            wait_lit(page, blocks[0])
            page.screenshot(path=str(docs / f"documents-{scheme}-{label}-after.png"))
            press(page, "read-aloud-stop")
            wait_stopped(page)

            chan = stamp("channel")
            open_channel(page)
            page.screenshot(path=str(chan / f"channel-{scheme}-{label}-before.png"))
            press(page, "read-aloud")
            page.wait_for_function(
                "() => readAloudOwner().nowPlaying !== null"
                " && document.querySelector('#discord-log [data-reading-block=\"current\"]') !== null",
                timeout=20_000)
            page.screenshot(path=str(chan / f"channel-{scheme}-{label}-after.png"))
            press(page, "read-aloud-stop")
            wait_stopped(page)

            if touch:
                # Discuss open over the channel on the phone, before/after
                # Read. This fixture server has no summariser, so answer
                # Discuss's routes with a saved summary, as
                # discuss_readjump_browser does.
                summary: dict[str, object] = {
                    "backend": "fake", "model": "fake-model", "view": "flat",
                    "since_ms": None, "created_ms": 1760000099999,
                    "window": {"count": 120, "unread_count": 2, "read_count": 118,
                               "from_id": "x", "through_id": "y", "truncated": False,
                               "noise_hidden": 0},
                    "covered_ids": [], "markable_ids": [], "cited_ids": [],
                    "fresh_ids": [], "footer_links": [],
                    "text": "Saved summary for the screenshot walk.",
                    "text_html": "<p>Saved summary for the screenshot walk.</p>",
                    "unknown_terms": [],
                    "stamp": {"context_ms": 1, "model_ms": 1, "first_token_ms": None,
                              "total_ms": 2, "input_tokens": None, "output_tokens": None,
                              "input_chars": 10, "output_chars": 10},
                }

                def discuss_config(route: object) -> None:
                    body = call(base, "GET", "/api/v1/client-config", WRITE_TOKEN)
                    body["discuss_available"] = True
                    route.fulfill(status=200, content_type="application/json",  # type: ignore[attr-defined]
                                  body=json.dumps(body))

                def discuss_preview(route: object) -> None:
                    route.fulfill(status=200, content_type="application/json",  # type: ignore[attr-defined]
                                  body=json.dumps({
                                      "channel": {"id": CHANNEL, "label": "lead team"},
                                      "thread_id": None, "view": "flat",
                                      "window": summary["window"], "unread": [],
                                  }))

                def discuss_record(route: object) -> None:
                    route.fulfill(status=200, content_type="application/json",  # type: ignore[attr-defined]
                                  body=json.dumps({
                                      "exists": True, "thread_id": None,
                                      "current_summary": summary,
                                      "timeline": [{"kind": "summary", "summary": summary}],
                                      "staleness": None,
                                  }))

                page.route("**/api/v1/client-config", discuss_config)
                page.route(f"**/api/v1/channels/{CHANNEL}/catchup/preview*", discuss_preview)
                page.route(f"**/api/v1/channels/{CHANNEL}/catchup/record*", discuss_record)
                open_channel(page)
                if page.locator("#discuss-open").is_visible():
                    page.click("#discuss-open")
                else:
                    page.click("#action-menu-open")
                    page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
                    page.click("#discuss-menu-open")
                page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
                page.screenshot(path=str(chan / f"channel-{scheme}-{label}-discuss-before.png"))
                # With Discuss open the bar's Read reads the summary
                # (`#1531`); on the device voice it speaks the panel's
                # saved words, which this fixture's routes provide.
                pick_device(page)
                press(page, "read-aloud")
                try:
                    page.wait_for_function(
                        "() => readAloudOwner().nowPlaying !== null", timeout=20_000)
                except Exception:
                    state = page.evaluate("""() => ({
                      playing: readAloudOwner().nowPlaying ? readAloudOwner().nowPlaying.id : null,
                      pending: readAloudOwner().pendingRead,
                      mode: readAloudOwner().readingMode,
                      label: document.getElementById('read-aloud-label')?.textContent,
                      rows: document.querySelectorAll('#discord-log > li[data-id]').length,
                      status: document.getElementById('status')?.textContent,
                      error: document.getElementById('error')?.textContent,
                    })""")
                    check(False, f"discuss-after: Read started nothing with Discuss open: {state}")
                page.screenshot(path=str(chan / f"channel-{scheme}-{label}-discuss-after.png"))
                press(page, "read-aloud-stop")
                wait_stopped(page)
            ctx.close()

    # The lit block in every colour preset (owner 273: "no highlight"),
    # dark and light on the phone.
    presets = stamp("presets")
    for scheme in schemes:
        ctx = browser.new_context(
            viewport={"width": 412, "height": 915}, has_touch=True, is_mobile=True,
            color_scheme=scheme)
        page = new_page(ctx)
        open_document(page)
        for preset in ("blue", "blue-hc", "teal", "plum", "amber"):
            page.evaluate("() => document.getElementById('open-settings').click()")
            page.wait_for_selector("#colour-theme", state="attached", timeout=20_000)
            page.evaluate("""(id) => {
              const node = document.getElementById(id); const bodies = [];
              for (let n = node; n; n = n.parentElement) {
                if (n.classList && n.classList.contains('fold-body')) bodies.unshift(n);
              }
              for (const body of bodies) {
                const toggle = document.querySelector(`[aria-controls="${body.id}"]`);
                if (toggle && toggle.getAttribute('aria-expanded') !== 'true') toggle.click();
              }
            }""", "colour-theme")
            page.wait_for_function(
                "() => document.getElementById('colour-theme').checkVisibility()",
                timeout=20_000)
            page.select_option("#colour-theme", preset)
            page.wait_for_function(
                f"() => document.documentElement.getAttribute('data-colour-theme') === '{preset}'",
                timeout=20_000)
            page.evaluate("() => document.getElementById('close-settings').click()")
            press(page, "read-aloud")
            wait_lit(page, blocks[0])
            # The light must be a colour of its own, not the block's ground.
            distinct = page.evaluate("""() => {
              const lit = document.querySelector('#doc-body [data-reading-block="current"]');
              if (!lit) return false;
              const bg = getComputedStyle(lit).backgroundColor;
              const parent = getComputedStyle(lit.parentElement).backgroundColor;
              const box = lit.getBoundingClientRect();
              return box.width > 0 && box.height > 0 && bg !== parent && bg !== 'rgba(0, 0, 0, 0)';
            }""")
            check(distinct, f"presets/{scheme}-{preset}: the lit block has no distinct visible colour")
            page.screenshot(path=str(presets / f"presets-{scheme}-{preset}.png"))
            press(page, "read-aloud-stop")
            wait_stopped(page)
        ctx.close()
    print(f"screenshot matrix written under {out} (documents, channel, presets; BUILD.txt per folder)")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--screenshots", type=Path, default=None,
                        help="also write the #2004/#2003 shot matrix under this directory")
    args = parser.parse_args()
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    sidecar = ThreadingHTTPServer(("127.0.0.1", 0), SidecarHandler)
    threading.Thread(target=sidecar.serve_forever, daemon=True).start()
    sidecar_base = f"http://127.0.0.1:{sidecar.server_address[1]}"
    with tempfile.TemporaryDirectory(prefix="documents-read-check-") as tmp:
        work = Path(tmp)
        for doc_name, doc_blocks in DOCS:
            (work / doc_name).write_text("\n\n".join(doc_blocks) + "\n")
        (work / "list-doc").write_text(LIST_DOC)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "documents-read-check-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
            '[documents]\n'
            'paste_url_prefixes = ["https://paste.example/"]\n'
            'paste_command = ["/bin/cat", "--", "{}"]\n'
            '[read_aloud]\nbackend = "local"\n'
            f'[local_tts]\nendpoint = "{sidecar_base}"\nvoice = "check-voice"\n'
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
                 {"text": "Notes: https://paste.example/read-doc and https://paste.example/desk-doc"
                          " and https://paste.example/list-doc"})
            docs = []
            deadline = time.time() + 30
            while time.time() < deadline:
                call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=10&render=html", WRITE_TOKEN)
                answer = call(base, "GET", "/api/v1/documents?limit=10", READ_TOKEN)
                docs = cast(list[dict[str, object]], answer["documents"])
                if len(docs) == 3:
                    break
                time.sleep(0.5)
            check(len(docs) == 3, f"the fold registered {len(docs)} documents, not 3: {docs}")

            with sync_playwright() as pw:
                browser = pw.chromium.launch(args=CHROMIUM_AUDIO_ARGS)
                for width, height, touch, (doc_name, doc_blocks) in (
                    (393, 852, True, DOCS[0]),
                    (1280, 800, False, DOCS[1]),
                ):
                    forget_synced_source(base)
                    ctx = browser.new_context(
                        viewport={"width": width, "height": height}, has_touch=touch, is_mobile=touch)
                    page = ctx.new_page()
                    page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                    page.add_init_script(DEVICE_VOICE)
                    page.add_init_script(MEDIA_PROBE)
                    walk(page, base, touch, doc_name, doc_blocks)
                    ctx.close()
                # The list document by touch (`#1505`): lit == spoken through
                # the list, pause from the pane, and tap-to-read.
                forget_synced_source(base)
                ctx = browser.new_context(
                    viewport={"width": 393, "height": 852}, has_touch=True, is_mobile=True)
                page = ctx.new_page()
                page.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                page.add_init_script(DEVICE_VOICE)
                page.add_init_script(MEDIA_PROBE)
                walk_list(page, base)
                ctx.close()
                if args.screenshots is not None:
                    screenshot_matrix(browser, base, args.screenshots)
                browser.close()
            print("documents read browser check: strip, device blocks, Next/Prev, media next, "
                  "finish-on-last, device/cloud toggle, Back-stop, pause from the pane, "
                  "lit == spoken through the list and tap-to-read all seen at 393x852 and 1280x800")
            return 0
        finally:
            server.terminate()
            server.wait(timeout=10)
            sidecar.shutdown()


if __name__ == "__main__":
    sys.exit(main())
