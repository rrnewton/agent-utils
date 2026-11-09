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
"""

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
from typing import cast

from playwright.sync_api import Page, sync_playwright

from browser_audio import CHROMIUM_AUDIO_ARGS

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "documents-read-check-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-read-check-write-token-not-a-real-credential"
# One document per viewport, differently worded: the server caches synthesized
# speech, so a second walk over the same words would be served from the cache
# and the sidecar would honestly have nothing new to record.
DOCS: list[tuple[str, list[str]]] = [
    ("read-doc", [
        "Opening block of the document says hello.",
        "Middle block of the document says more.",
        "Closing block of the document says goodbye.",
    ]),
    ("desk-doc", [
        "First part of the desk document begins.",
        "Second part of the desk document follows.",
        "Final part of the desk document ends.",
    ]),
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
    check(page.locator("#control-pane").is_hidden(), "the call controls show over the reader")

    # The device/cloud toggle is the Documents view's too now, starting on the
    # server voice; one tap picks the device for the read that follows.
    toggle = page.locator("#audio-source")
    check(toggle.is_visible(), "the audio source toggle is not offered over the Documents view")
    check(toggle.get_attribute("aria-checked") == "true",
          "the premise: the server voice is not the picked source")
    act("#audio-source")
    page.wait_for_function(
        "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
        timeout=20_000)

    # Read: the strip counts the blocks, the device voice says them in order,
    # and the read is the owner's document read — no queue of the reader's own.
    act("#doc-read")
    wait_count(page, "1 of 3")
    state = owner_state(page)
    check(state["id"] == f"doc:paste:{name}", f"Read started {state}, not the document read")
    check(state["kind"] == "document", f"the playing read is not a document: {state}")
    page.wait_for_function("() => window.deviceSpeech.length >= 1", timeout=20_000)
    check(utterances(page) == [blocks[0]], f"the device voice said {utterances(page)}, not the first block alone")
    handlers: list[str] = page.evaluate(
        "() => Object.keys(window.mediaProbe.handlers).filter((k) => window.mediaProbe.handlers[k]).sort()")
    check(handlers == ["nexttrack", "pause", "play", "previoustrack", "stop"],
          f"the document read holds media handlers {handlers}")

    act("#read-next")
    wait_count(page, "2 of 3")
    page.wait_for_function("() => window.deviceSpeech.length >= 2", timeout=20_000)
    check(utterances(page)[-1] == blocks[1], "Next did not say the second block")
    page.evaluate("() => window.mediaProbe.handlers.nexttrack({action: 'nexttrack'})")
    wait_count(page, "3 of 3")
    page.wait_for_function("() => window.deviceSpeech.length >= 3", timeout=20_000)
    check(utterances(page)[-1] == blocks[2], "the media Next did not say the last block")
    # On the last block the strip's Next is not offered, as a message's is not.
    check(page.evaluate("() => document.getElementById('read-next').disabled") is True,
          "Next is offered on the document's last block")
    act("#read-prev")
    wait_count(page, "2 of 3")
    act("#read-next")
    wait_count(page, "3 of 3")
    said_before_finish = len(utterances(page))
    # The media Next on the last block finishes the read (`#1035`): strip gone, owner idle.
    page.evaluate("() => window.mediaProbe.handlers.nexttrack({action: 'nexttrack'})")
    wait_stopped(page)
    check(len(utterances(page)) == said_before_finish, "finishing the read said something after it")

    # Flip back: the next read is the server voice's — the sidecar is asked
    # for the document's words, the strip counts cloud blocks, the device rests.
    said = len(utterances(page))
    asked = len(sidecar_texts())
    act("#audio-source")
    page.wait_for_function(
        "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'true'",
        timeout=20_000)
    act("#doc-read")
    page.wait_for_function(
        """() => {
          const strip = document.getElementById('read-steer');
          const count = document.getElementById('read-steer-count');
          return !!strip && !strip.hidden && !!count && count.textContent.trim().endsWith('of 3');
        }""",
        timeout=20_000)
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
    act("#doc-read")
    wait_stopped(page)

    # Back stops a document read where it stands.
    act("#audio-source")
    page.wait_for_function(
        "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
        timeout=20_000)
    act("#doc-read")
    wait_count(page, "1 of 3")
    act("#doc-back")
    wait_stopped(page)
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    sidecar = ThreadingHTTPServer(("127.0.0.1", 0), SidecarHandler)
    threading.Thread(target=sidecar.serve_forever, daemon=True).start()
    sidecar_base = f"http://127.0.0.1:{sidecar.server_address[1]}"
    with tempfile.TemporaryDirectory(prefix="documents-read-check-") as tmp:
        work = Path(tmp)
        for doc_name, doc_blocks in DOCS:
            (work / doc_name).write_text("\n\n".join(doc_blocks) + "\n")
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
                 {"text": "Notes: https://paste.example/read-doc and https://paste.example/desk-doc"})
            docs = []
            deadline = time.time() + 30
            while time.time() < deadline:
                call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=10&render=html", WRITE_TOKEN)
                answer = call(base, "GET", "/api/v1/documents?limit=10", READ_TOKEN)
                docs = cast(list[dict[str, object]], answer["documents"])
                if len(docs) == 2:
                    break
                time.sleep(0.5)
            check(len(docs) == 2, f"the fold registered {len(docs)} documents, not 2: {docs}")

            with sync_playwright() as pw:
                browser = pw.chromium.launch(args=CHROMIUM_AUDIO_ARGS)
                for width, height, touch, (doc_name, doc_blocks) in (
                    (393, 852, True, DOCS[0]),
                    (1280, 800, False, DOCS[1]),
                ):
                    ctx = browser.new_context(
                        viewport={"width": width, "height": height}, has_touch=touch, is_mobile=touch)
                    page = ctx.new_page()
                    page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                    page.add_init_script(DEVICE_VOICE)
                    page.add_init_script(MEDIA_PROBE)
                    walk(page, base, touch, doc_name, doc_blocks)
                    ctx.close()
                browser.close()
            print("documents read browser check: strip, device blocks, Next/Prev, media next, "
                  "finish-on-last, device/cloud toggle and Back-stop all seen at 393x852 and 1280x800")
            return 0
        finally:
            server.terminate()
            server.wait(timeout=10)
            sidecar.shutdown()


if __name__ == "__main__":
    sys.exit(main())
