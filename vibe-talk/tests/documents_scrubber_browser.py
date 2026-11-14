"""The multi-level document scrubber, against the REAL server (`#1508`).

Seeds one large synthetic document (hundreds of blocks, nested sections,
one huge leaf section of 118 paragraphs) and one author-numbered
document, then at phone (393x852) and desktop (1280x800) widths checks:
section numbers drawn ON the document before each heading (and never
duplicated beside an author's own numbers), the navigator's row count
(at most 2 on a phone, at most 3 on a desk), section numbers on the
navigator's rows, and that tapping a navigator unit starts the read at
that unit's first block with the reading light on the same block. All
waits are on page state, never on the clock (owner rule 346).
"""

import json
import sys
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import cast

from playwright.sync_api import Page, sync_playwright

from browser_audio import CHROMIUM_AUDIO_ARGS
import documents_read_browser as harness

ROOT = harness.ROOT
CHANNEL = harness.CHANNEL
READ_TOKEN = harness.READ_TOKEN
WRITE_TOKEN = harness.WRITE_TOKEN
check = harness.check
call = harness.call
free_port = harness.free_port


def big_document(label: str) -> str:
    lines = [f"# The {label} field report", "", f"Opening words of the {label} document.", ""]
    for s in range(1, 13):
        lines += [f"## Part {s} of {label}", ""]
        for ss in range(1, 4):
            lines += [f"### Part {s}.{ss} of {label}", ""]
            count = 118 if (s, ss) == (3, 2) else 6
            for b in range(count):
                lines += [f"Paragraph {s}.{ss} number {b} of {label} carries plain words "
                          "for the reading light to rest on here.", ""]
    return "\n".join(lines)


AUTHORED = """# Authored notes

## 3. First authored section

Authored opening paragraph with plain words to read aloud here.

## 4. Second authored section

Authored closing paragraph with plain words to read aloud here.
"""


def first_spoken_after(page: Page, marker: int) -> str:
    spoken = cast(list[dict[str, object]], page.evaluate("() => window.deviceSpeech"))
    texts = [str(entry["text"]) for entry in spoken[marker:]]
    return texts[0] if texts else ""


def walk_big(page: Page, touch: bool, card_name: str, label: str, width: int) -> None:
    def act(selector: str) -> None:
        if touch:
            page.tap(selector)
        else:
            page.click(selector)

    card = page.locator("#documents-list .doc-card", has_text=card_name)
    if touch:
        card.tap()
    else:
        card.click()
    page.wait_for_function(
        "(words) => (document.getElementById('doc-body')?.textContent || '').includes(words)",
        arg=f"Paragraph 3.2 number 0 of {label}", timeout=20_000)

    # (a) Numbers on the document itself, before the headings.
    secnums = page.evaluate("""() => [...document.querySelectorAll('#doc-body .doc-secnum')]
      .map((el) => [el.textContent, el.parentElement.textContent.replace(el.textContent, '').trim()])""")
    by_title = {title: number for number, title in secnums}
    check(by_title.get(f"Part 1 of {label}") == "1", f"heading Part 1 carries no number 1: {secnums[:4]}")
    check(by_title.get(f"Part 3.2 of {label}") == "3.2", f"heading Part 3.2 carries no number 3.2: {secnums[:6]}")
    title_numbered = page.evaluate("""() => {
      const h1 = document.querySelector('#doc-body h1');
      return h1 !== null && h1.querySelector('.doc-secnum') !== null;
    }""")
    check(not title_numbered, "the title heading is numbered, but the design leaves it unnumbered")

    # The navigator: rows within the cap for this width, numbers on them.
    page.wait_for_selector("#doc-nav:not([hidden]) .doc-nav-row", timeout=20_000)
    rows = page.evaluate("() => document.querySelectorAll('#doc-nav .doc-nav-row').length")
    cap = 2 if width < 600 else 3
    check(1 <= rows <= cap, f"the navigator shows {rows} rows at {width}px, cap is {cap}")
    nav_text = page.evaluate("() => document.getElementById('doc-nav').textContent")
    check("3" in nav_text, f"section numbers are missing from the navigator rows: {nav_text[:80]!r}")

    # Read with the device voice, then tap a navigator unit: the read
    # starts at that unit's first block and the light lands on it.
    toggle = page.locator("#audio-source")
    if toggle.get_attribute("aria-checked") == "true":
        act("#audio-source")
        page.wait_for_function(
            "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
            timeout=20_000)
    marker = page.evaluate("() => window.deviceSpeech.length")
    act("#doc-read")
    page.wait_for_function(
        "() => document.querySelectorAll('#doc-body [data-reading-block=\"current\"]').length === 1",
        timeout=20_000)
    marker = page.evaluate("() => window.deviceSpeech.length")
    unit = page.locator("#doc-nav .doc-nav-unit", has_text="2").first
    unit.tap() if touch else unit.click()
    page.wait_for_function(
        """() => {
          const lit = document.querySelector('#doc-body [data-reading-block="current"]');
          return lit !== null && (lit.textContent || '').includes('Part 2 of');
        }""", timeout=20_000)
    spoken = first_spoken_after(page, marker)
    check("Part 2 of" in spoken, f"tapping the unit did not start reading section 2: {spoken[:60]!r}")


def walk_authored(page: Page, touch: bool) -> None:
    card = page.locator("#documents-list .doc-card", has_text="authored-doc")
    if touch:
        card.tap()
    else:
        card.click()
    page.wait_for_function(
        "(words) => (document.getElementById('doc-body')?.textContent || '').includes(words)",
        arg="Authored opening paragraph", timeout=20_000)
    doubled = page.evaluate("""() => [...document.querySelectorAll('#doc-body h2')]
      .filter((h) => h.querySelector('.doc-secnum') !== null).length""")
    check(doubled == 0, "an author's own numbers gained a second computed number beside them")


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    sidecar = ThreadingHTTPServer(("127.0.0.1", 0), harness.SidecarHandler)
    threading.Thread(target=sidecar.serve_forever, daemon=True).start()
    sidecar_base = f"http://127.0.0.1:{sidecar.server_address[1]}"
    with tempfile.TemporaryDirectory(prefix="documents-scrubber-check-") as tmp:
        work = Path(tmp)
        (work / "phone-doc").write_text(big_document("phone"))
        (work / "desk-doc").write_text(big_document("desk"))
        (work / "authored-doc").write_text(AUTHORED)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "documents-scrubber-check-bot-token-never-sent"\n'
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
        import subprocess
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
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                 {"text": "Docs: https://paste.example/phone-doc and https://paste.example/desk-doc"
                          " and https://paste.example/authored-doc"})
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
                for width, height, touch, name in ((393, 852, True, "phone-doc"), (1280, 800, False, "desk-doc")):
                    ctx = browser.new_context(
                        viewport={"width": width, "height": height}, has_touch=touch, is_mobile=touch)
                    page = ctx.new_page()
                    page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                    page.add_init_script(harness.DEVICE_VOICE)
                    page.add_init_script(harness.MEDIA_PROBE)
                    page.goto(base + "/?v=docs", wait_until="load")
                    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                    walk_big(page, touch, name, name.replace("-doc", ""), width)
                    ctx.close()
                ctx = browser.new_context(
                    viewport={"width": 393, "height": 852}, has_touch=True, is_mobile=True)
                page = ctx.new_page()
                page.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                page.add_init_script(harness.DEVICE_VOICE)
                page.add_init_script(harness.MEDIA_PROBE)
                page.goto(base + "/?v=docs", wait_until="load")
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                walk_authored(page, True)
                ctx.close()
                browser.close()
            print("documents scrubber browser check: numbers on the document (author's win, title "
                  "unnumbered), navigator rows within cap with numbers at 393x852 and 1280x800, "
                  "tap-a-unit starts the read there with the light on the same block")
            return 0
        finally:
            server.terminate()
            server.wait(timeout=10)
            sidecar.shutdown()


if __name__ == "__main__":
    sys.exit(main())
