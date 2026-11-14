"""The document scrubber navigator, against the REAL server (`#1541`).

Seeds one large synthetic document (hundreds of blocks, nested sections,
one huge leaf section of 118 paragraphs), one author-numbered document
and one document with a table between two paragraphs, then at phone
(393x852) and desktop (1280x800) widths checks: section numbers drawn
ON the document before each heading (and never duplicated beside an
author's own numbers); no navigator at the reader's head; while a
document read stands, exactly two navigator rows inside the orange
scrubber (the section tree unfolded to the current section, over that
section's chunk blocks), fitting on screen with the flat per-block
timeline hidden; tapping a section or a chunk starts the read at
exactly that block with the reading light on it, the table's chunk
between its neighbouring paragraphs. All waits are on page state,
never on the clock (owner rule 346). Screenshots land in
$DOCNAV_SCREENSHOTS when that names a directory.
"""

import json
import os
import sys
import tempfile
import threading
import time
from http.server import ThreadingHTTPServer
from pathlib import Path
from typing import cast

from playwright.sync_api import Locator, Page, sync_playwright

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


TABLE_DOC = """# Table notes

## Results

Before the table there is a paragraph with plain words to read aloud here now.

| Lever | Cost |
| --- | --- |
| Alpha | 10 |
| Beta | 20 |

After the table there is a paragraph with plain words to read aloud here now.
"""


def walk_big(page: Page, touch: bool, card_name: str, label: str, width: int) -> None:
    def act(selector: str) -> None:
        if touch:
            page.tap(selector)
        else:
            page.click(selector)

    def tap_cell(locator: Locator) -> None:
        locator.tap() if touch else locator.click()

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

    # `#1541`: before any read, no navigator at the reader's head — it
    # lives in the scrubber, which is not up yet.
    at_head = page.evaluate("() => document.querySelectorAll('#docs-reader #doc-nav').length")
    check(at_head == 0, "the navigator is still mounted at the reader's head")
    nav_hidden = page.evaluate("() => document.getElementById('doc-nav').hidden === true")
    check(nav_hidden, "the navigator shows before a document read stands")

    # Read with the device voice: the scrubber comes up carrying the
    # two rows (sections over the current section's chunks), and the
    # flat per-block timeline gives way to them.
    toggle = page.locator("#audio-source")
    if toggle.get_attribute("aria-checked") == "true":
        act("#audio-source")
        page.wait_for_function(
            "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
            timeout=20_000)
    act("#doc-read")
    page.wait_for_function(
        "() => document.querySelectorAll('#doc-body [data-reading-block=\"current\"]').length === 1",
        timeout=20_000)
    page.wait_for_selector("#read-steer #doc-nav:not([hidden]) .doc-nav-row", timeout=20_000)
    rows = page.evaluate("() => document.querySelectorAll('#doc-nav .doc-nav-row').length")
    check(rows == 2, f"the scrubber navigator shows {rows} rows at {width}px, want exactly 2")
    nav_text = page.evaluate("() => document.getElementById('doc-nav').textContent")
    check("3" in nav_text, f"section numbers are missing from the navigator rows: {nav_text[:80]!r}")
    timeline_display = page.evaluate(
        "() => getComputedStyle(document.getElementById('read-timeline')).display")
    check(timeline_display == "none",
          f"the flat timeline still shows beside the document rows: display={timeline_display}")
    fit = page.evaluate("""() => {
      const nav = document.getElementById('doc-nav');
      const steer = document.getElementById('read-steer');
      const box = steer.getBoundingClientRect();
      return { scroll: nav.scrollWidth, client: nav.clientWidth,
               left: box.left, right: box.right, vw: window.innerWidth };
    }""")
    check(fit["scroll"] <= fit["client"] + 1,
          f"the navigator spills sideways at {width}px: scrollWidth {fit['scroll']} > clientWidth {fit['client']}")
    check(fit["left"] >= -1 and fit["right"] <= fit["vw"] + 1,
          f"the scrubber spills off screen at {width}px: {fit}")

    # Tap a section: the read starts at that section's first block and
    # the light lands on it.
    marker = page.evaluate("() => window.deviceSpeech.length")
    tap_cell(page.locator('#doc-nav .doc-nav-section:text-is("2")').first)
    page.wait_for_function(
        """() => {
          const lit = document.querySelector('#doc-body [data-reading-block="current"]');
          return lit !== null && (lit.textContent || '').includes('Part 2 of');
        }""", timeout=20_000)
    spoken = first_spoken_after(page, marker)
    check("Part 2 of" in spoken, f"tapping the section did not start reading section 2: {spoken[:60]!r}")

    # Deep navigation: into section 3 (it unfolds in place), then its
    # child 3.2, which stands current and highlighted.
    tap_cell(page.locator('#doc-nav .doc-nav-section:text-is("3")').first)
    page.wait_for_function(
        """() => {
          const lit = document.querySelector('#doc-body [data-reading-block="current"]');
          return lit !== null && (lit.textContent || '').includes('Part 3 of');
        }""", timeout=20_000)
    tap_cell(page.locator('#doc-nav .doc-nav-section:text-is("3.2")').first)
    page.wait_for_function(
        """() => {
          const lit = document.querySelector('#doc-body [data-reading-block="current"]');
          return lit !== null && (lit.textContent || '').includes('Part 3.2 of');
        }""", timeout=20_000)
    deep_rows = page.evaluate("() => document.querySelectorAll('#doc-nav .doc-nav-row').length")
    check(deep_rows == 2, f"deep in 3.2 the navigator shows {deep_rows} rows at {width}px, want 2")
    current_section = page.evaluate(
        "() => document.querySelector('#doc-nav .doc-nav-section[aria-current=\"true\"]')?.textContent")
    check(current_section == "3.2", f"the current section is {current_section!r}, want 3.2 highlighted")
    fit = page.evaluate("""() => {
      const nav = document.getElementById('doc-nav');
      const steer = document.getElementById('read-steer');
      const box = steer.getBoundingClientRect();
      return { scroll: nav.scrollWidth, client: nav.clientWidth,
               left: box.left, right: box.right, vw: window.innerWidth };
    }""")
    check(fit["scroll"] <= fit["client"] + 1 and fit["left"] >= -1 and fit["right"] <= fit["vw"] + 1,
          f"deep in 3.2 the scrubber spills at {width}px: {fit}")

    # A chunk tap lands on exactly its own block: chunk 3 of section
    # 3.2 is its second paragraph (chunk 1 is the heading).
    tap_cell(page.locator('#doc-nav .doc-nav-chunks .doc-nav-chunk:text-is("3")').first)
    page.wait_for_function(
        f"""() => {{
          const lit = document.querySelector('#doc-body [data-reading-block="current"]');
          return lit !== null && (lit.textContent || '').includes('Paragraph 3.2 number 1 of {label}');
        }}""", timeout=20_000)

    shots = os.environ.get("DOCNAV_SCREENSHOTS", "")
    if shots:
        Path(shots).mkdir(parents=True, exist_ok=True)
        page.screenshot(path=str(Path(shots) / f"scrubber-{label}-{width}.png"))


def walk_table(page: Page, touch: bool) -> None:
    def tap_cell(locator: Locator) -> None:
        locator.tap() if touch else locator.click()

    card = page.locator("#documents-list .doc-card", has_text="table-doc")
    if touch:
        card.tap()
    else:
        card.click()
    page.wait_for_function(
        "(words) => (document.getElementById('doc-body')?.textContent || '').includes(words)",
        arg="After the table", timeout=20_000)
    toggle = page.locator("#audio-source")
    if toggle.get_attribute("aria-checked") == "true":
        page.tap("#audio-source") if touch else page.click("#audio-source")
        page.wait_for_function(
            "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
            timeout=20_000)
    page.tap("#doc-read") if touch else page.click("#doc-read")
    page.wait_for_selector("#read-steer #doc-nav:not([hidden]) .doc-nav-chunk", timeout=20_000)
    # Section 1's chunks in order: 1 heading, 2 before, 3 the table,
    # 4 after. Each tap must light exactly that block.
    for local, words in (("2", "Before the table"), ("4", "After the table")):
        tap_cell(page.locator(f'#doc-nav .doc-nav-chunks .doc-nav-chunk:text-is("{local}")').first)
        page.wait_for_function(
            """(words) => {
              const lit = document.querySelector('#doc-body [data-reading-block="current"]');
              return lit !== null && (lit.textContent || '').includes(words);
            }""", arg=words, timeout=20_000)
    tap_cell(page.locator('#doc-nav .doc-nav-chunks .doc-nav-chunk:text-is("3")').first)
    page.wait_for_function(
        """() => {
          const lit = document.querySelector('#doc-body [data-reading-block="current"]');
          return lit !== null && lit.tagName === 'TABLE' && (lit.textContent || '').includes('Alpha');
        }""", timeout=20_000)
    shots = os.environ.get("DOCNAV_SCREENSHOTS", "")
    if shots:
        Path(shots).mkdir(parents=True, exist_ok=True)
        page.screenshot(path=str(Path(shots) / "scrubber-table-393.png"))

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
        (work / "table-doc").write_text(TABLE_DOC)
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
                          " and https://paste.example/authored-doc and https://paste.example/table-doc"})
            docs = []
            deadline = time.time() + 30
            while time.time() < deadline:
                call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=10&render=html", WRITE_TOKEN)
                answer = call(base, "GET", "/api/v1/documents?limit=10", READ_TOKEN)
                docs = cast(list[dict[str, object]], answer["documents"])
                if len(docs) == 4:
                    break
                time.sleep(0.5)
            check(len(docs) == 4, f"the fold registered {len(docs)} documents, not 4: {docs}")
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
                ctx = browser.new_context(
                    viewport={"width": 393, "height": 852}, has_touch=True, is_mobile=True)
                page = ctx.new_page()
                page.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                page.add_init_script(harness.DEVICE_VOICE)
                page.add_init_script(harness.MEDIA_PROBE)
                page.goto(base + "/?v=docs", wait_until="load")
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                walk_table(page, True)
                ctx.close()
                browser.close()
            print("documents scrubber browser check: numbers on the document (author's win, title "
                  "unnumbered), two scrubber rows from the section tree fitting 393x852 and 1280x800, "
                  "section and chunk taps land on the right block, table chunk between its neighbours")
            return 0
        finally:
            server.terminate()
            server.wait(timeout=10)
            sidecar.shutdown()


if __name__ == "__main__":
    sys.exit(main())
