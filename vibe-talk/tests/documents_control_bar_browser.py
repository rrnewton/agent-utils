"""The Documents desktop control bar (`#1544 documents-desktop-control-bar`,
`#2242 vibe-talk-documents-reader-toolbar`).

Against the REAL server (release binary, --fake-discord, one seeded paste
document with headings, mentioned by one channel message): at 1280x800
(mouse) the reader controls -- the two jumps (Mention, Discussion), the
device/cloud audio toggle, Pace and Read -- stand in ONE row inside the
bottom bar, in one centred box named Reader, each jump a compact button with
an icon, one word, a tooltip and an aria-label, and the tooltip drawn on
keyboard focus; Fetch stands at the bar's right in the list view and gives
way to Outline in the reader (never both), and the outline and pace popups
centre over their buttons inside the viewport. Enter on Mention lands on the
mentioning message. At 1000x800 the jumps are icons alone, the box stays one
row clear of the gear and the corner's hamburger with Stop shown while a read
plays, and Space on Discussion lands too. At 412x915 (touch) the jumps are the
tile's second column, never a row of their own, every reader control at least
44px, Pace and Read keeping their words; with no document open the tile is the
plain one; a tap on Mention lands on the message.
"""

import json
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path
from typing import cast

from playwright.sync_api import BrowserContext, Page, sync_playwright
import browser_waits

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "documents-control-bar-check-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-control-bar-check-write-token-not-a-real-credential"
DOC_BODY = (
    "# Control bar report\n\nThe nightly run finished green.\n\n"
    "## Details\n\nThe detail under the second heading.\n\n"
    "## Later\n\nThe words under the third heading.\n"
)


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


def bar_state(page: Page) -> dict[str, object]:
    result = cast(dict[str, object], page.evaluate(
        """() => {
          const box = (id) => {
            const el = document.getElementById(id);
            if (el === null) return null;
            const r = el.getBoundingClientRect();
            const s = getComputedStyle(el);
            return {
              hidden: el.hidden, display: s.display,
              left: r.left, right: r.right, top: r.top, bottom: r.bottom,
              width: r.width, height: r.height,
              centre: r.left + r.width / 2,
              parent: el.parentElement === null ? null : el.parentElement.id,
              border: parseFloat(s.borderTopWidth),
            };
          };
          const pane = document.getElementById('control-pane');
          return {
            innerWidth,
            fetch: box('docs-fetch'), outline: box('doc-outline-open'),
            pane: box('control-pane'), audio: box('audio-source'),
            speed: box('read-speed'), read: box('read-aloud'),
            stop: box('read-aloud-stop'), dock: box('dock'),
            mention: box('doc-jump-mention'), discussion: box('doc-jump-discussion'),
            jumps: box('doc-jumps'), gear: box('open-settings'),
            menu: box('action-menu-open'), outlinePop: box('doc-outline-pop'),
            speedPop: box('speed-popover'),
            paneLabel: pane === null ? null
              : getComputedStyle(pane, '::before').content,
          };
        }"""))
    return result


def num(box: dict[str, object], key: str) -> float:
    return float(cast(float, box[key]))


def flag(box: dict[str, object], key: str) -> bool:
    return cast(bool, box[key])


# A device voice that starts and never ends, so a read stands with Stop shown
# for as long as the walk needs it (the browser's speech engine is the only
# thing stubbed; the page and server are real).
ENDLESS_VOICE = """() => {
  const voice = {name: 'Check voice', lang: 'en-US', localService: true, default: true, voiceURI: 'check'};
  const engine = {
    paused: false, speaking: false, pending: false,
    getVoices: () => [voice], addEventListener() {}, removeEventListener() {},
    speak(utterance) { this.speaking = true; setTimeout(() => { if (utterance.onstart) utterance.onstart({}); }, 30); },
    cancel() { this.speaking = false; }, pause() { this.paused = true; }, resume() { this.paused = false; },
  };
  Object.defineProperty(window, 'speechSynthesis', {value: engine, configurable: true});
  window.SpeechSynthesisUtterance = function (text) { this.text = text; };
}"""

# The jump's landing (`#2006`): the channel shown, the mentioning message's row
# drawn and inside the scroller.
LANDED_JS = """(id) => {
  const pane = document.getElementById('pane-discord');
  const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
  const area = document.getElementById('scroll-area');
  if (!pane || pane.hidden || !row || !area) return false;
  const r = row.getBoundingClientRect(), a = area.getBoundingClientRect();
  return r.height > 0 && r.bottom > a.top && r.top < a.bottom;
}"""


def visible(box: dict[str, object]) -> bool:
    return box["display"] != "none" and not flag(box, "hidden") and num(box, "width") > 0


def open_reader(page: Page, touch: bool) -> None:
    card = page.locator("#documents-list .doc-card", has_text="control-doc")
    card.wait_for(state="visible", timeout=20_000)
    if touch:
        box = card.bounding_box()
        assert box is not None
        page.touchscreen.tap(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
    else:
        card.click()
    page.wait_for_selector("#doc-body h1", timeout=20_000)
    browser_waits.wait_until(
        page, "() => !document.getElementById('read-aloud').hidden && !document.getElementById('doc-jumps').hidden",
        "the reader's controls and its jumps to stand")


def tab_to(page: Page, target: str) -> None:
    """Tab forward until `target` holds the keyboard focus (a bounded walk of the page's stops)."""
    for _ in range(80):
        if page.evaluate("() => document.activeElement && document.activeElement.id") == target:
            return
        page.keyboard.press("Tab")
    raise AssertionError(f"Tab never reached #{target}")


def focus_tip(page: Page, target: str) -> str:
    return str(page.evaluate(
        "(id) => getComputedStyle(document.getElementById(id), '::after').content", target))


def check_reader_row(state: dict[str, object], words: bool, label: str) -> None:
    """#2242: one row of compact controls inside the bottom bar, clear of the gear and the corner."""
    pane = cast(dict[str, object], state["pane"])
    dock = cast(dict[str, object], state["dock"])
    menu = cast(dict[str, object], state["menu"])
    gear = cast(dict[str, object], state["gear"])
    parts = ["mention", "discussion", "audio", "speed", "read"]
    if visible(cast(dict[str, object], state["stop"])):
        parts.append("stop")
    tops = []
    for part in parts:
        part_box = cast(dict[str, object], state[part])
        check(visible(part_box), f"{label}: #{part} is not standing in the reader: {state}")
        check(num(part_box, "height") <= 40, f"{label}: #{part} is taller than a bar button: {state}")
        tops.append(num(part_box, "top"))
    check(max(tops) - min(tops) <= 2, f"{label}: the reader controls are not one row: {tops} {state}")
    check(num(pane, "height") <= 48, f"{label}: the Reader box is taller than one bar row: {state}")
    check(num(pane, "top") >= num(dock, "top") - 1,
          f"{label}: the Reader box stands above the bottom bar: {state}")
    check(num(pane, "right") <= num(menu, "left") - 2,
          f"{label}: the Reader box runs under the corner's hamburger: {state}")
    check(num(pane, "left") >= num(gear, "right") + 2,
          f"{label}: the Reader box runs under the gear: {state}")
    for jump in ["mention", "discussion"]:
        jump_box = cast(dict[str, object], state[jump])
        if words:
            check(num(jump_box, "width") < 140, f"{label}: #{jump} is not compact: {state}")
        else:
            check(num(jump_box, "width") <= 40, f"{label}: #{jump} is not its icon alone: {state}")


def check_jump_names(page: Page, words: bool) -> None:
    names = cast(dict[str, dict[str, object]], page.evaluate("""() => {
      const out = {};
      for (const id of ['doc-jump-mention', 'doc-jump-discussion']) {
        const el = document.getElementById(id);
        const word = el.querySelector('.doc-jump-word');
        out[id] = {label: el.getAttribute('aria-label'), title: el.getAttribute('title'),
                   icon: !!el.querySelector('svg'),
                   word: word && getComputedStyle(word).display !== 'none' ? word.textContent.trim() : ''};
      }
      return out;
    }"""))
    expected = {"doc-jump-mention": ("Jump to last mention", "Mention"),
                "doc-jump-discussion": ("Jump to discussion", "Discussion")}
    for jump, (label, word) in expected.items():
        got = names[jump]
        check(got["label"] == label, f"#{jump} lost its screen-reader name: {got}")
        check(str(got["title"]).startswith(label), f"#{jump} lost its tooltip: {got}")
        check(bool(got["icon"]), f"#{jump} has no icon: {got}")
        check(got["word"] == (word if words else ""), f"#{jump} shows the wrong word: {got}")


def check_desktop(page: Page, mention_id: str) -> None:
    # List view: Fetch at the right, no Outline, no Reader box yet.
    state = bar_state(page)
    fetch = cast(dict[str, object], state["fetch"])
    outline = cast(dict[str, object], state["outline"])
    menu = cast(dict[str, object], state["menu"])
    check(fetch["display"] != "none" and not flag(fetch, "hidden"),
          f"Fetch is not standing in the list view: {state}")
    check(outline["display"] == "none" or flag(outline, "hidden"),
          f"Outline shows in the list view beside Fetch: {state}")
    check(abs(num(fetch, "left") - num(menu, "right")) < 80,
          f"Fetch is not at the bar's right beside the hamburger: {state}")

    open_reader(page, touch=False)

    # Reader view: Outline replaces Fetch, and the reader controls stand in
    # one centred box named Reader, one row in the bar (`#2242`).
    state = bar_state(page)
    fetch = cast(dict[str, object], state["fetch"])
    outline = cast(dict[str, object], state["outline"])
    pane = cast(dict[str, object], state["pane"])
    check(flag(fetch, "hidden") or fetch["display"] == "none",
          f"Fetch still shows in the reader view: {state}")
    check(outline["display"] != "none" and not flag(outline, "hidden"),
          f"Outline is not standing in the reader view: {state}")
    for part in ["audio", "speed", "read", "jumps"]:
        part_box = cast(dict[str, object], state[part])
        check(part_box["parent"] == "control-pane",
              f"#{part} is not grouped in the Reader box: {state}")
    check(num(pane, "border") > 0, f"the Reader box has no border: {state}")
    check(state["paneLabel"] is not None and "Reader" in str(state["paneLabel"]),
          f"the Reader box is not named Reader: {state}")
    check(abs(num(pane, "centre") - num(state, "innerWidth") / 2) < 60,
          f"the Reader box is not centred: {state}")
    check_reader_row(state, words=True, label="1280")
    check_jump_names(page, words=True)

    # The outline popup centres over its button, on screen.
    page.click("#doc-outline-open")
    page.wait_for_selector("#doc-outline-pop", state="visible", timeout=20_000)
    state = bar_state(page)
    pop = cast(dict[str, object], state["outlinePop"])
    outline = cast(dict[str, object], state["outline"])
    check(abs(num(pop, "centre") - num(outline, "centre")) < 40,
          f"the outline popup is not centred over Outline: {state}")
    check(num(pop, "left") >= 0 and num(pop, "right") <= num(state, "innerWidth"),
          f"the outline popup spills off screen: {state}")
    page.click("#doc-outline-open")

    # The pace popup centres over Pace, on screen.
    page.click("#read-speed")
    page.wait_for_selector("#speed-popover", state="visible", timeout=20_000)
    state = bar_state(page)
    pop = cast(dict[str, object], state["speedPop"])
    speed = cast(dict[str, object], state["speed"])
    check(abs(num(pop, "centre") - num(speed, "centre")) < 60,
          f"the pace popup is not centred over Pace: {state}")
    check(num(pop, "left") >= 0 and num(pop, "right") <= num(state, "innerWidth"),
          f"the pace popup spills off screen: {state}")
    page.keyboard.press("Escape")
    browser_waits.wait_until(page, "() => document.getElementById('speed-popover').hidden",
                             "the pace popup to close")

    # The reader's own shortcut still answers beside the moved controls: g opens the outline.
    page.locator("#doc-body").click()
    page.keyboard.press("g")
    page.wait_for_selector("#doc-outline-pop", state="visible", timeout=20_000)
    page.keyboard.press("Escape")
    browser_waits.wait_until(page, "() => document.getElementById('doc-outline-pop').hidden",
                             "Escape to close the outline g opened")

    # The keyboard reaches Mention, its tooltip shows on focus, and Enter jumps.
    tab_to(page, "doc-jump-mention")
    tip = focus_tip(page, "doc-jump-mention")
    check("Jump to last mention" in tip, f"Mention's tooltip does not show on keyboard focus: {tip!r}")
    page.keyboard.press("Enter")
    browser_waits.wait_until(page, f"() => ({LANDED_JS})({json.dumps(mention_id)})",
                             "Enter on Mention to land on the mentioning message")


def check_narrow_desktop(page: Page, mention_id: str) -> None:
    """1000px: the jumps are icons alone; one row clear of the corner, a read playing with Stop shown."""
    open_reader(page, touch=False)
    check_reader_row(bar_state(page), words=False, label="1000")
    check_jump_names(page, words=False)
    page.evaluate(ENDLESS_VOICE)
    browser_waits.wait_until(
        page, "() => { const t = document.getElementById('audio-source');"
              " return !!t && !t.hidden && t.getAttribute('aria-checked') !== null; }",
        "the audio source to be decided")
    if page.evaluate("() => document.getElementById('audio-source').getAttribute('aria-checked')") == "true":
        page.click("#audio-source")
        browser_waits.wait_until(
            page, "() => document.getElementById('audio-source').getAttribute('aria-checked') === 'false'",
            "the device voice to be picked")
    page.click("#read-aloud")
    browser_waits.wait_until(page, "() => !document.getElementById('read-aloud-stop').hidden",
                             "Stop to stand beside Read while the document plays")
    state = bar_state(page)
    check(cast(dict[str, object], state["stop"])["parent"] == "control-pane",
          f"Stop is not in the Reader box: {state}")
    check_reader_row(state, words=False, label="1000 playing")
    # The read's shortcut still steps it: ArrowRight is the next block.
    page.locator("#doc-body").click()
    first = str(page.evaluate("() => document.getElementById('read-steer-count').textContent"))
    page.keyboard.press("ArrowRight")
    browser_waits.wait_until(
        page, f"() => document.getElementById('read-steer-count').textContent !== {json.dumps(first)}",
        "ArrowRight to step the read to its next block")
    tab_to(page, "doc-jump-discussion")
    tip = focus_tip(page, "doc-jump-discussion")
    check("Jump to discussion" in tip, f"Discussion's tooltip does not show on keyboard focus: {tip!r}")
    page.click("#read-aloud-stop")
    browser_waits.wait_until(page, "() => document.getElementById('read-aloud-stop').hidden",
                             "the read to stop")
    # Under the default policy the discussion is the mention's thread: Space jumps there.
    page.focus("#doc-jump-discussion")
    page.keyboard.press(" ")
    browser_waits.wait_until(page, f"() => ({LANDED_JS})({json.dumps(mention_id)})",
                             "Space on Discussion to land on the mention's thread")


def check_phone(page: Page, mention_id: str) -> None:
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    state = bar_state(page)
    fetch = cast(dict[str, object], state["fetch"])
    check(fetch["display"] != "none" and not flag(fetch, "hidden"),
          f"Fetch is not standing in the phone list view: {state}")
    # No document open: the tile is the plain one, the jumps' column nowhere.
    tracks = str(page.evaluate("() => getComputedStyle(document.getElementById('control-pane')).gridTemplateColumns"))
    check(len(tracks.split()) == 3 and flag(cast(dict[str, object], state["jumps"]), "hidden"),
          f"the phone tile changed with no document open: {tracks} {state}")
    open_reader(page, touch=True)
    state = bar_state(page)
    fetch = cast(dict[str, object], state["fetch"])
    pane = cast(dict[str, object], state["pane"])
    check(flag(fetch, "hidden") or fetch["display"] == "none",
          f"Fetch still shows in the phone reader view: {state}")
    check(num(pane, "border") == 0,
          f"the phone pane took the desktop Reader box: {state}")
    # `#2242`: the jumps are the tile's second column, not a row of their own.
    mention = cast(dict[str, object], state["mention"])
    discussion = cast(dict[str, object], state["discussion"])
    speed = cast(dict[str, object], state["speed"])
    audio = cast(dict[str, object], state["audio"])
    check(abs(num(mention, "left") - num(discussion, "left")) <= 1
          and num(discussion, "top") >= num(mention, "bottom"),
          f"the phone jumps are not stacked in one column: {state}")
    check(abs(num(mention, "top") - num(speed, "top")) <= 2
          and abs(num(discussion, "bottom") - num(speed, "bottom")) <= 2,
          f"the phone jumps are not inside the tile beside Pace: {state}")
    check(num(mention, "left") >= num(audio, "right") and num(mention, "right") <= num(speed, "left"),
          f"the phone jumps are not between the audio column and Pace: {state}")
    for part in ["mention", "discussion", "audio", "speed", "read"]:
        part_box = cast(dict[str, object], state[part])
        check(num(part_box, "width") >= 44 and num(part_box, "height") >= 44,
              f"#{part} is under a 44px touch target on the phone: {state}")
    words = cast(dict[str, str], page.evaluate("""() => ({
      pace: document.getElementById('read-speed-label').innerText.trim(),
      read: document.getElementById('read-aloud-label').innerText.trim(),
      mention: document.querySelector('#doc-jump-mention .doc-jump-word').innerText.trim(),
      discussion: document.querySelector('#doc-jump-discussion .doc-jump-word').innerText.trim()})"""))
    check(words["pace"] == "Pace" and words["read"] == "Read",
          f"Pace or Read lost its word on the phone: {words}")
    check(words["mention"].lower() == "mention" and words["discussion"].lower() == "discussion",
          f"a phone jump lost its caption: {words}")
    page.tap("#doc-outline-open")
    page.wait_for_selector("#doc-outline-pop", state="visible", timeout=20_000)
    state = bar_state(page)
    pop = cast(dict[str, object], state["outlinePop"])
    check(num(pop, "left") >= 0 and num(pop, "right") <= num(state, "innerWidth") + 1,
          f"the phone outline popup spills off screen: {state}")
    page.tap("#doc-outline-open")
    browser_waits.wait_until(page, "() => document.getElementById('doc-outline-pop').hidden",
                             "the outline popup to close")
    page.tap("#doc-jump-mention")
    browser_waits.wait_until(page, f"() => ({LANDED_JS})({json.dumps(mention_id)})",
                             "a tap on Mention to land on the mentioning message")


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="documents-control-bar-check-") as tmp:
        work = Path(tmp)
        (work / "control-doc").write_text(DOC_BODY)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "documents-control-bar-check-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
            '[documents]\n'
            'paste_url_prefixes = ["https://paste.example/"]\n'
            'paste_command = ["/bin/cat", "--", "{}"]\n'
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
                 {"text": "Report: https://paste.example/control-doc"})
            docs: list[dict[str, object]] = []
            deadline = time.time() + 30
            while time.time() < deadline:
                call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=10&render=html", WRITE_TOKEN)
                answer = call(base, "GET", "/api/v1/documents?limit=10", READ_TOKEN)
                docs = cast(list[dict[str, object]], answer["documents"])
                if len(docs) == 1:
                    break
                time.sleep(0.5)
            check(len(docs) == 1, f"registered {len(docs)} documents, not 1: {docs}")
            mentions = cast(list[dict[str, object]], docs[0].get("mentions") or [])
            check(len(mentions) == 1, f"the document is not mentioned once: {docs}")
            mention_id = str(mentions[0]["message_id"])

            with sync_playwright() as pw:
                browser = pw.chromium.launch()

                def desk(width: int) -> tuple[BrowserContext, Page]:
                    dctx = browser.new_context(viewport={"width": width, "height": 800})
                    browser_waits.hot_host_timeouts(dctx)
                    dpage = dctx.new_page()
                    browser_waits.hot_host_timeouts(dpage)
                    dpage.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                    dpage.goto(base + "/?v=docs", wait_until="load")
                    dpage.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                    return dctx, dpage

                dctx, dpage = desk(1280)
                check_desktop(dpage, mention_id)
                dctx.close()
                dctx, dpage = desk(1000)
                check_narrow_desktop(dpage, mention_id)
                dctx.close()

                ctx = browser.new_context(viewport={"width": 412, "height": 915}, has_touch=True, is_mobile=True)
                browser_waits.hot_host_timeouts(ctx)
                page = ctx.new_page()
                browser_waits.hot_host_timeouts(page)
                page.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                page.goto(base + "/?v=docs", wait_until="load")
                check_phone(page, mention_id)
                ctx.close()
                browser.close()
            print("documents control bar browser check: one-row centred Reader box in the bar with "
                  "compact jumps (1280 words, 1000 icons, Stop shown), focus tips, keyboard and tap "
                  "jumps landing, the phone tile's jump column at 44px, Fetch/Outline exclusivity, "
                  "and centred clamped popups")
            return 0
        finally:
            server.terminate()
            server.wait(timeout=10)


if __name__ == "__main__":
    sys.exit(main())
