"""The document scrubber navigator, against the REAL server (`#1541`).

Seeds large synthetic documents (hundreds of blocks, nested sections,
one huge leaf section of 118 paragraphs), one author-numbered document,
one document with a table between two paragraphs, and canary documents
with a long paragraph and a four-row table BEFORE their target sections
(`#2266`: the server reads those as several spoken pieces, so a seek in
the prepared list's own numbering lands early). Every read walk runs on
BOTH voices — the cloud voice (the default; the fake sidecar records
every text it is asked to say) and the device voice — at phone
(393x852) and desktop (1280x800) widths, checking: section numbers drawn
ON the document before each heading (and never duplicated beside an
author's own numbers); no navigator at the reader's head; while a
document read stands, exactly two navigator rows inside the orange
scrubber (the section tree unfolded to the current section, over that
section's chunk blocks), fitting on screen with the flat per-block
timeline hidden; tapping a section, a chunk or the document body starts
the read at exactly that block with the reading light on it (the FIRST
block lit after the tap, recorded in the page, and the first words
spoken), the table's chunk between its neighbouring paragraphs. All
waits are on page state, never on the clock (owner rule 346).
Screenshots land in $DOCNAV_SCREENSHOTS when that names a directory.
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

from playwright.sync_api import BrowserContext, Locator, Page, sync_playwright

from browser_audio import CHROMIUM_AUDIO_ARGS
import documents_read_browser as harness
import browser_waits

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


def sidecar_texts() -> list[str]:
    with harness.SidecarHandler.lock:
        return list(harness.SidecarHandler.texts)


def spoken_marker(page: Page, voice: str) -> int:
    if voice == "cloud":
        return len(sidecar_texts())
    return int(page.evaluate("() => window.deviceSpeech.length"))


def wait_spoken(page: Page, voice: str, marker: int, words: str) -> str:
    """The first words said after `marker` that name `words`, either voice.

    Device speech is already recorded by the time its light lands; cloud
    speech is waited for on the sidecar's record. A restart's first
    synthesis can queue behind a few the previous read had in flight
    (it fetched ahead while the tap was still landing), so the first
    few new texts are looked through; a wrong landing says its own
    block there and reaches `words` only by playing on to it.
    """
    if voice != "cloud":
        spoken = first_spoken_after(page, marker)
        check(words in spoken, f"the device voice did not start at {words!r}: {spoken[:60]!r}")
        return spoken
    deadline = time.time() + 20
    while time.time() < deadline:
        new = sidecar_texts()[marker:]
        for text in new[:6]:
            if words in text:
                return text
        if len(new) >= 6:
            break
        time.sleep(0.1)
    check(False, f"the cloud voice did not start at {words!r}; new texts: {sidecar_texts()[marker:marker + 6]!r}")
    return ""


def wait_said(page: Page, voice: str, marker: int, words: str) -> None:
    """`words` are said at all after `marker` (the canary's cloud arm:
    its document is small enough that synthesis lookahead cannot hide
    a landing, and the lit log + count carry the exact order)."""
    if voice != "cloud":
        wait_spoken(page, voice, marker, words)
        return
    deadline = time.time() + 20
    while time.time() < deadline:
        if any(words in text for text in sidecar_texts()[marker:]):
            return
        time.sleep(0.1)
    check(False, f"the cloud voice never said {words!r} after the tap; "
                 f"new texts: {sidecar_texts()[marker:marker + 6]!r}")


def set_voice(page: Page, touch: bool, voice: str) -> None:
    """The bar's voice toggle: checked is the cloud (agent) voice."""
    want = "true" if voice == "cloud" else "false"
    if page.locator("#audio-source").get_attribute("aria-checked") == want:
        return
    page.tap("#audio-source") if touch else page.click("#audio-source")
    page.wait_for_function(
        "(want) => document.getElementById('audio-source').getAttribute('aria-checked') === want",
        arg=want, timeout=20_000)


# The first block lit after a tap, recorded in the page itself: a cloud
# read's one-second blocks move on while a busy host is still polling, so
# the landing is judged from this log, not from whichever block is lit by
# the time the check looks.
LIT_RECORDER = """() => {
  if (window.__litLog) return;
  window.__litLog = [];
  let last = null;
  const grab = () => {
    const lit = document.querySelector('#doc-body [data-reading-block="current"]');
    const text = lit ? (lit.textContent || '').trim() : null;
    if (text !== last) { last = text; if (text) window.__litLog.push(text); }
  };
  new MutationObserver(grab).observe(document.getElementById('doc-body'),
    {attributes: true, subtree: true, attributeFilter: ['data-reading-block']});
  grab();
}"""


def install_lit_log(page: Page) -> None:
    page.evaluate(LIT_RECORDER)


def lit_marker(page: Page) -> int:
    return int(page.evaluate("() => (window.__litLog || []).length"))


def wait_first_lit(page: Page, marker: int, words: str, what: str) -> None:
    """`words` is lit as the tap's landing: within the first few blocks lit
    after `marker` (a cloud read already playing may advance one block
    between marking and tapping; a wrong landing lights its own block
    there instead and reaches `words` only much later, if ever)."""
    try:
        page.wait_for_function(
            """([marker, words]) => {
              const log = window.__litLog || [];
              return log.slice(marker, marker + 3).some((text) => text.includes(words));
            }""", arg=[marker, words], timeout=20_000)
    except Exception:
        log = page.evaluate("([marker]) => (window.__litLog || []).slice(marker, marker + 6)", arg=[marker])
        check(False, f"{what} did not light {words!r} on landing; lit after the tap: {log!r}")


def click_centre(page: Page, locator: Locator, touch: bool, what: str) -> None:
    """Tap `locator` by coordinates, after proving the point hits it."""
    box = locator.bounding_box(timeout=20_000)
    check(box is not None, f"{what} has no box to tap")
    assert box is not None
    x = box["x"] + box["width"] / 2
    y = box["y"] + box["height"] / 2
    handle = locator.element_handle(timeout=20_000)
    check(handle is not None, f"{what} is not on the page")
    hit = handle.evaluate(
        "(el, [x, y]) => document.elementFromPoint(x, y) === el || el.contains(document.elementFromPoint(x, y))",
        [x, y])
    check(hit is True, f"the centre of {what} is covered by something else")
    if touch:
        page.touchscreen.tap(x, y)
    else:
        page.mouse.click(x, y)


TABLE_DOC = """# Table notes

## Results

Before the table there is a paragraph with plain words to read aloud here now.

| Lever | Cost |
| --- | --- |
| Alpha | 10 |
| Beta | 20 |

After the table there is a paragraph with plain words to read aloud here now.
"""


def canary_document(label: str) -> str:
    """`#2266`: a paragraph over 60 words and a four-row table BEFORE the
    target sections, so the server's spoken pieces outnumber the drawn
    blocks (9 drawn, 13 spoken) and a seek in the wrong numbering lands
    early. Every block carries `label`, so the server's speech cache can
    never answer one voice's walk with the other voice's words."""
    long_para = (
        f"LONG{label}START This opening sentence of the long canary paragraph "
        "carries enough plain words for the reading light to rest on while "
        f"it waits here in the {label} document. The second sentence keeps "
        "adding ordinary words about the weather over the harbour and the "
        "boats coming home in the evening light again today. The third "
        "sentence talks about stores and ledgers and careful counting of "
        "every crate that arrived on the morning cart from the farm beyond "
        "the hill. The final sentence closes the paragraph with a few more "
        f"quiet words so the whole block stands well past sixty words LONG{label}END.")
    return f"""# Canary {label} report

## Alpha {label} start

{long_para}

| Lever | Cost |
| --- | --- |
| ROW-ALPHA-{label} | 10 |
| ROW-BETA-{label} | 20 |
| ROW-GAMMA-{label} | 30 |
| ROW-DELTA-{label} | 40 |

## Beta {label} section

TARGET-{label}-BETA-1 short paragraph carries plain words here.

TARGET-{label}-BETA-2 short paragraph carries plain words here.

### Gamma {label} subsection

TARGET-{label}-GAMMA-1 short paragraph carries plain words here.
"""


def walk_big(page: Page, touch: bool, card_name: str, label: str, width: int, voice: str) -> None:
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
    install_lit_log(page)

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

    # Read with this walk's voice: the scrubber comes up carrying the
    # two rows (sections over the current section's chunks), and the
    # flat per-block timeline gives way to them.
    set_voice(page, touch, voice)
    act("#read-aloud")
    page.wait_for_function(
        "() => document.querySelectorAll('#doc-body [data-reading-block=\"current\"]').length === 1",
        timeout=20_000)
    page.wait_for_selector("#read-steer #doc-nav:not([hidden]) .doc-nav-row", timeout=20_000)
    rows = page.evaluate("() => document.querySelectorAll('#doc-nav .doc-nav-row').length")
    check(rows == 2, f"the scrubber navigator shows {rows} rows at {width}px, want exactly 2")
    # `#2266`: every section stands in the row as its § number, nothing
    # folded behind a gap cell, and chunk ticks carry no numbers.
    secnums = page.evaluate(
        "() => [...document.querySelectorAll('#doc-nav .doc-nav-num')].map((el) => el.textContent)")
    check(secnums[:4] == ["§0", "§1", "§1.1", "§1.2"] and "§3.2" in secnums and "§12.3" in secnums,
          f"the section row does not stand flat in order: {secnums[:6]!r}... total {len(secnums)}")
    nav_text = page.evaluate("() => document.getElementById('doc-nav').textContent")
    check("…" not in nav_text, f"a gap cell still hides sections or chunks: {nav_text[:80]!r}")
    tick_texts = page.evaluate(
        "() => [...document.querySelectorAll('#doc-nav .doc-nav-chunk')].map((el) => el.textContent)")
    check(all(text == "" for text in tick_texts), f"chunk ticks still carry numbers: {tick_texts[:6]!r}")
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
    # Rows scroll sideways inside the strip by design (`#2266`); the
    # strip itself must stay on screen.
    check(fit["left"] >= -1 and fit["right"] <= fit["vw"] + 1,
          f"the scrubber spills off screen at {width}px: {fit}")

    # Tap a section: the read starts at that section's first block and
    # the light lands on it — first lit after the tap, first words said.
    marker = spoken_marker(page, voice)
    lit_at = lit_marker(page)
    tap_cell(page.locator('#doc-nav .doc-nav-section[aria-label^="Read from section 2 "]').first)
    wait_first_lit(page, lit_at, "Part 2 of", "tapping section 2")
    # Spoken order proves the start only on the device voice: cloud
    # synthesis runs ahead of playback, so its record cannot order a
    # restart against the read it replaced (the canary walk asserts
    # cloud speech on a document small enough to bound that).
    if voice != "cloud":
        wait_spoken(page, voice, marker, "Part 2 of")

    # Deep navigation: into section 3, then its child 3.2, which
    # stands current and highlighted (scrolled into its row's view).
    lit_at = lit_marker(page)
    tap_cell(page.locator('#doc-nav .doc-nav-section[aria-label^="Read from section 3 "]').first)
    wait_first_lit(page, lit_at, "Part 3 of", "tapping section 3")
    lit_at = lit_marker(page)
    tap_cell(page.locator('#doc-nav .doc-nav-section[aria-label^="Read from section 3.2 "]').first)
    wait_first_lit(page, lit_at, "Part 3.2 of", "tapping section 3.2")
    deep_rows = page.evaluate("() => document.querySelectorAll('#doc-nav .doc-nav-row').length")
    check(deep_rows == 2, f"deep in 3.2 the navigator shows {deep_rows} rows at {width}px, want 2")
    current_section = page.evaluate(
        "() => document.querySelector('#doc-nav .doc-nav-section[aria-current=\"true\"] .doc-nav-num')?.textContent")
    check(current_section == "§3.2", f"the current section is {current_section!r}, want §3.2 highlighted")
    in_row = page.evaluate("""() => {
      const cell = document.querySelector('#doc-nav .doc-nav-section[aria-current="true"]');
      const row = cell?.closest('.doc-nav-row');
      if (!cell || !row) return false;
      const cellBox = cell.getBoundingClientRect();
      const rowBox = row.getBoundingClientRect();
      return cellBox.left >= rowBox.left - 1 && cellBox.right <= rowBox.right + 1;
    }""")
    check(in_row is True, "the current section cell is scrolled out of its row")
    fit = page.evaluate("""() => {
      const nav = document.getElementById('doc-nav');
      const steer = document.getElementById('read-steer');
      const box = steer.getBoundingClientRect();
      return { scroll: nav.scrollWidth, client: nav.clientWidth,
               left: box.left, right: box.right, vw: window.innerWidth };
    }""")
    check(fit["left"] >= -1 and fit["right"] <= fit["vw"] + 1,
          f"deep in 3.2 the scrubber spills at {width}px: {fit}")

    # A chunk tap lands on exactly its own block: the tick addressed
    # 3.2 ¶ 2 is the section's second paragraph (the heading tick
    # addresses as the section itself).
    lit_at = lit_marker(page)
    tap_cell(page.locator('#doc-nav .doc-nav-chunks .doc-nav-chunk[aria-label="Read from 3.2 ¶ 2"]').first)
    wait_first_lit(page, lit_at, f"Paragraph 3.2 number 1 of {label}", "tapping chunk ¶ 2 of 3.2")

    shots = os.environ.get("DOCNAV_SCREENSHOTS", "")
    if shots:
        Path(shots).mkdir(parents=True, exist_ok=True)
        page.screenshot(path=str(Path(shots) / f"scrubber-{label}-{width}.png"))


def walk_table(page: Page, touch: bool, voice: str) -> None:
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
    install_lit_log(page)
    set_voice(page, touch, voice)
    page.tap("#read-aloud") if touch else page.click("#read-aloud")
    page.wait_for_selector("#read-steer #doc-nav:not([hidden]) .doc-nav-chunk", timeout=20_000)
    # Section 1's ticks by address: its heading, ¶ 1 before, ¶ 2 the
    # table, ¶ 3 after. Each tap must light exactly that block. On the
    # cloud voice the table is spoken a row at a time (`#2266`): ¶ 3 is
    # the block AFTER those rows, not the fourth spoken piece.
    for address, words in (("Read from 1 ¶ 1", "Before the table"), ("Read from 1 ¶ 3", "After the table")):
        lit_at = lit_marker(page)
        tap_cell(page.locator(f'#doc-nav .doc-nav-chunks .doc-nav-chunk[aria-label="{address}"]').first)
        wait_first_lit(page, lit_at, words, f"tapping table tick {address}")
    lit_at = lit_marker(page)
    tap_cell(page.locator('#doc-nav .doc-nav-chunks .doc-nav-chunk[aria-label="Read from 1 ¶ 2"]').first)
    wait_first_lit(page, lit_at, "Alpha", "tapping the table chunk")
    lit_tag = page.evaluate(
        "() => document.querySelector('#doc-body [data-reading-block=\"current\"]')?.tagName")
    check(lit_tag == "TABLE", f"the table chunk lit a {lit_tag}, not the table")
    shots = os.environ.get("DOCNAV_SCREENSHOTS", "")
    if shots:
        Path(shots).mkdir(parents=True, exist_ok=True)
        page.screenshot(path=str(Path(shots) / "scrubber-table-393.png"))

def walk_canary(page: Page, touch: bool, card_name: str, label: str, voice: str) -> None:
    """`#2266`: every control after the long paragraph and the table
    lands on its own block — thin arrows a visible chunk a press, fat
    arrows the top row's sections, cells and body by coordinates —
    lit, spoken and counted in model blocks, either voice."""
    def act(selector: str) -> None:
        page.tap(selector) if touch else page.click(selector)

    card = page.locator("#documents-list .doc-card", has_text=card_name)
    card.tap() if touch else card.click()
    page.wait_for_function(
        "(words) => (document.getElementById('doc-body')?.textContent || '').includes(words)",
        arg=f"TARGET-{label}-GAMMA-1", timeout=20_000)
    install_lit_log(page)
    set_voice(page, touch, voice)
    act("#read-aloud")
    page.wait_for_function(
        "() => document.querySelectorAll('#doc-body [data-reading-block=\"current\"]').length === 1",
        timeout=20_000)
    page.wait_for_selector("#read-steer #doc-nav:not([hidden]) .doc-nav-row", timeout=20_000)

    # Labels and count: § sections flat, ticks unnumbered, and the
    # count in drawn blocks (9) on either voice — never spoken pieces.
    secnums = page.evaluate(
        "() => [...document.querySelectorAll('#doc-nav .doc-nav-num')].map((el) => el.textContent)")
    check(secnums == ["§1", "§2", "§2.1"], f"the canary section row is {secnums!r}")
    tick_texts = page.evaluate(
        "() => [...document.querySelectorAll('#doc-nav .doc-nav-chunk')].map((el) => el.textContent)")
    check(all(text == "" for text in tick_texts), f"canary ticks carry numbers: {tick_texts!r}")
    count = str(page.evaluate("() => document.getElementById('read-steer-count')?.textContent || ''"))
    check(count.endswith(" of 9"), f"the {voice} count {count!r} does not count the 9 drawn blocks")

    # A body tap first, while the fresh read stands at the top (and
    # before synthesis lookahead can cache its target away).
    lit_at = lit_marker(page)
    marker = spoken_marker(page, voice)
    para = page.locator("#doc-body > p", has_text=f"TARGET-{label}-BETA-2").first
    para.scroll_into_view_if_needed(timeout=20_000)
    click_centre(page, para, touch, "the BETA-2 paragraph")
    wait_first_lit(page, lit_at, f"TARGET-{label}-BETA-2", "tapping the BETA-2 paragraph")
    wait_said(page, voice, marker, f"TARGET-{label}-BETA-2")

    # A section cell by coordinates, back to the top section.
    lit_at = lit_marker(page)
    click_centre(page, page.locator('#doc-nav .doc-nav-section[aria-label^="Read from section 1 "]').first,
                 touch, "section cell 1")
    wait_first_lit(page, lit_at, f"Alpha {label} start", "tapping section §1")

    # Thin arrows step one visible chunk a press, the table whole.
    for words in (f"LONG{label}START", f"ROW-ALPHA-{label}", f"Beta {label} section"):
        lit_at = lit_marker(page)
        act("#read-next")
        wait_first_lit(page, lit_at, words, f"thin Next to {words!r}")

    # Fat arrows step the top row's sections.
    lit_at = lit_marker(page)
    act("#read-section-next")
    wait_first_lit(page, lit_at, f"Gamma {label} subsection", "fat Next section")

    # Later taps assert speech only on the device voice: a cloud
    # restart replays the server's speech cache without asking the
    # sidecar again, so its record goes quiet where the landing is
    # right; the lit log carries those landings.
    lit_at = lit_marker(page)
    marker = spoken_marker(page, voice)
    click_centre(page, page.locator('#doc-nav .doc-nav-chunks .doc-nav-chunk[aria-label="Read from 2.1 ¶ 1"]').first,
                 touch, "the subsection's paragraph tick")
    wait_first_lit(page, lit_at, f"TARGET-{label}-GAMMA-1", "tapping the subsection's paragraph tick")
    if voice != "cloud":
        wait_spoken(page, voice, marker, f"TARGET-{label}-GAMMA-1")

    lit_at = lit_marker(page)
    marker = spoken_marker(page, voice)
    click_centre(page, page.locator('#doc-nav .doc-nav-section[aria-label^="Read from section 2 "]').first,
                 touch, "section cell 2")
    wait_first_lit(page, lit_at, f"Beta {label} section", "tapping section §2")
    if voice != "cloud":
        wait_spoken(page, voice, marker, f"Beta {label} section")

    shots = os.environ.get("DOCNAV_SCREENSHOTS", "")
    if shots:
        Path(shots).mkdir(parents=True, exist_ok=True)
        page.screenshot(path=str(Path(shots) / f"scrubber-canary-{label}-{voice}.png"))


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
        # One big document a (width, voice) walk, differently worded: the
        # server caches synthesized speech, so a second cloud walk over the
        # same words would honestly have nothing new to record.
        for name in ("phonecloud", "phonedevice", "deskcloud", "deskdevice"):
            (work / f"big-{name}").write_text(big_document(name))
        (work / "authored-doc").write_text(AUTHORED)
        (work / "table-doc").write_text(TABLE_DOC)
        (work / "canary-cloud").write_text(canary_document("cloudcanary"))
        (work / "canary-device").write_text(canary_document("devicecanary"))
        (work / "canary-phone").write_text(canary_document("phonecanary"))
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
            names = ("big-phonecloud", "big-phonedevice", "big-deskcloud", "big-deskdevice",
                     "authored-doc", "table-doc", "canary-cloud", "canary-device", "canary-phone")
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                 {"text": "Docs: " + " and ".join(f"https://paste.example/{name}" for name in names)})
            docs = []
            deadline = time.time() + 30
            while time.time() < deadline:
                call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=20&render=html", WRITE_TOKEN)
                answer = call(base, "GET", "/api/v1/documents?limit=20", READ_TOKEN)
                docs = cast(list[dict[str, object]], answer["documents"])
                if len(docs) == len(names):
                    break
                time.sleep(0.5)
            check(len(docs) == len(names), f"the fold registered {len(docs)} documents, not {len(names)}: {docs}")
            with sync_playwright() as pw:
                browser = pw.chromium.launch(args=CHROMIUM_AUDIO_ARGS)

                def fresh_page(width: int, height: int, touch: bool) -> tuple[BrowserContext, Page]:
                    ctx = browser.new_context(
                        viewport={"width": width, "height": height}, has_touch=touch, is_mobile=touch)
                    browser_waits.hot_host_timeouts(ctx)
                    page = ctx.new_page()
                    browser_waits.hot_host_timeouts(page)
                    page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                    page.add_init_script(harness.DEVICE_VOICE)
                    page.add_init_script(harness.MEDIA_PROBE)
                    page.goto(base + "/?v=docs", wait_until="load")
                    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                    return ctx, page

                for width, height, touch, name, voice in (
                        (393, 852, True, "big-phonecloud", "cloud"),
                        (1280, 800, False, "big-deskcloud", "cloud"),
                        (393, 852, True, "big-phonedevice", "device"),
                        (1280, 800, False, "big-deskdevice", "device")):
                    ctx, page = fresh_page(width, height, touch)
                    walk_big(page, touch, name, name.removeprefix("big-"), width, voice)
                    ctx.close()
                ctx, page = fresh_page(393, 852, True)
                walk_authored(page, True)
                ctx.close()
                for voice in ("cloud", "device"):
                    ctx, page = fresh_page(393, 852, True)
                    walk_table(page, True, voice)
                    ctx.close()
                for width, height, touch, name, label, voice in (
                        (1280, 800, False, "canary-cloud", "cloudcanary", "cloud"),
                        (1280, 800, False, "canary-device", "devicecanary", "device"),
                        (393, 852, True, "canary-phone", "phonecanary", "cloud")):
                    ctx, page = fresh_page(width, height, touch)
                    walk_canary(page, touch, name, label, voice)
                    ctx.close()
                browser.close()
            print("documents scrubber browser check: numbers on the document (author's win, title "
                  "unnumbered), two scrubber rows from the section tree fitting 393x852 and 1280x800, "
                  "section, chunk and body taps land on the right block on both voices (canary: "
                  "after a split paragraph and a table), table chunk between its neighbours")
            return 0
        finally:
            server.terminate()
            server.wait(timeout=10)
            sidecar.shutdown()


if __name__ == "__main__":
    sys.exit(main())
