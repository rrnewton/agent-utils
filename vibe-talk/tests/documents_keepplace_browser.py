"""Documents keep-place against the REAL server (`#2005 vibe-talk-documents-keep-place`).

Seeds 24 paste documents through the reply route, then at phone 412x915 and
desktop 1280, in light and dark: with a document open mid-scroll, toggling
Documents -> channel -> Documents restores the same document with its top
paragraph within 3px; with no document open, the list returns at its prior
place; and Back from the document after such a toggle lands on the list's
own place. Screenshots (with --screenshots DIR) are taken before the toggle
and after the return, with BUILD.txt. No documents API is stubbed.
"""

import argparse
import json
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path
from typing import cast

from playwright.sync_api import Page, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "documents-keepplace-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-keepplace-write-token-not-a-real-credential"
DOCS = 24
_FILLER = (
    "A filler paragraph long enough to take a few lines on a phone, so the "
    "document below scrolls for several viewports and the list of documents "
    "is far taller than the screen that shows it.\n\n"
)


def write_build_stamp(folder: Path, page_build: str) -> Path:
    """The shot folder's BUILD.txt, in run.sh's format: what build the shots show."""
    def git(*argv: str) -> str:
        return subprocess.run(["git", "-C", str(ROOT.parent), *argv],
                              capture_output=True, text=True).stdout.strip() or "unknown"

    stamp = folder / "BUILD.txt"
    stamp.write_text(
        f"commit {git('rev-parse', 'HEAD')}\n"
        f"commit-date {git('log', '-1', '--format=%cI', 'HEAD')}\n"
        f"page-build {page_build}\n",
        encoding="utf-8",
    )
    return stamp


def doc_body(index: int) -> str:
    # Every paragraph is numbered (document + paragraph), so the paragraph at
    # the top of the screen is identifiable in a screenshot, before and after.
    parts = [f"# Keep-place document {index:02d}\n\n"]
    for part in range(18):
        parts.append(f"Document {index:02d} paragraph {part + 1:02d}. " + _FILLER)
    parts.append(f"The final line of document {index:02d}.\n")
    return "".join(parts)


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


def scroll_state(page: Page) -> dict[str, float]:
    state = page.evaluate(
        """() => {
          const root = document.getElementById('scroll-area');
          return { top: root.scrollTop, height: root.scrollHeight, view: root.clientHeight };
        }""")
    return cast(dict[str, float], state)


def top_card(page: Page) -> tuple[str, float] | None:
    """The list's top card as (key, offset from the scroller's top), or None."""
    raw: object = page.evaluate(
        """() => {
          const root = document.getElementById('scroll-area');
          const band = root.getBoundingClientRect();
          const found = [...document.querySelectorAll('#documents-list .doc-card')]
            .find((el) => el.getBoundingClientRect().bottom > band.top);
          if (!found) return null;
          return { key: found.getAttribute('data-key'), offset: found.getBoundingClientRect().top - band.top };
        }""")
    if not isinstance(raw, dict):
        return None
    key = raw.get("key")
    offset = raw.get("offset")
    if not isinstance(key, str) or not isinstance(offset, (int, float)):
        return None
    return key, float(offset)


def top_paragraph(page: Page) -> tuple[str, float] | None:
    """The reader's top paragraph as (text head, offset), or None."""
    raw: object = page.evaluate(
        """() => {
          const root = document.getElementById('scroll-area');
          const band = root.getBoundingClientRect();
          const blocks = [...document.querySelectorAll(
            '#doc-body > p, #doc-body > h1, #doc-body > h2, #doc-body > h3, #doc-body > blockquote, #doc-body > pre')];
          const found = blocks.find((el) => el.getBoundingClientRect().bottom > band.top);
          if (!found) return null;
          return { text: (found.textContent || '').slice(0, 48), offset: found.getBoundingClientRect().top - band.top };
        }""")
    if not isinstance(raw, dict):
        return None
    text = raw.get("text")
    offset = raw.get("offset")
    if not isinstance(text, str) or not isinstance(offset, (int, float)):
        return None
    return text, float(offset)


def wait_top_card(page: Page, want: tuple[str, float], label: str, what: str) -> tuple[str, float]:
    """Wait until the list's top card is `want`'s card within 3px.

    The return landing is async (a refresh renders, then the place is put
    back), so a one-shot read can catch the list mid-landing; poll for the
    settled place itself, then assert on a final read.
    """
    page.wait_for_function(
        """(want) => {
          const root = document.getElementById('scroll-area');
          if (!root || document.getElementById('docs-list-view').hidden) return false;
          const band = root.getBoundingClientRect();
          const found = [...document.querySelectorAll('#documents-list .doc-card')]
            .find((el) => el.getBoundingClientRect().bottom > band.top);
          return !!found && found.getAttribute('data-key') === want.key
            && Math.abs(found.getBoundingClientRect().top - band.top - want.offset) <= 3;
        }""",
        arg={"key": want[0], "offset": want[1]},
        timeout=20_000,
    )
    back = top_card(page)
    check(back is not None, f"{label}: {what} left no card at the top")
    back = cast(tuple[str, float], back)
    check(back[0] == want[0] and abs(back[1] - want[1]) <= 3,
          f"{label}: {what} landed at {back}, not the list's place {want}")
    return back


def wait_settled(page: Page) -> None:
    """The settled screen: the view-switch tab row has put itself away.

    The row stays up 600ms after the last switch press by design (its settle),
    so an after shot taken at once would show it open over the bar without
    anything being wrong; the shots show the settled screen instead.
    """
    page.wait_for_function(
        "() => { const cards = document.getElementById('view-cards'); return !cards || cards.hidden; }",
        timeout=20_000)


def show_pane(page: Page, pane: str, has_touch: bool, label: str) -> None:
    """Tap the view switch until `pane` is the visible one (at most a full cycle)."""
    for _ in range(4):
        if page.evaluate(f"() => !document.getElementById('{pane}').hidden"):
            return
        page.tap("#view-switch") if has_touch else page.click("#view-switch")
        try:
            page.wait_for_function(f"() => !document.getElementById('{pane}').hidden", timeout=3_000)
            return
        except Exception:
            continue
    check(False, f"{label}: could not reach {pane} with the view switch")


def tap_visible_card(page: Page, has_touch: bool) -> None:
    """Tap the first card wholly inside the scroller's viewport, without scrolling."""
    box = page.evaluate(
        """() => {
          const root = document.getElementById('scroll-area');
          const view = root.getBoundingClientRect();
          const card = [...document.querySelectorAll('#documents-list .doc-card')]
            .find((el) => {
              const at = el.getBoundingClientRect();
              return at.top >= view.top && at.bottom <= view.bottom;
            });
          if (!card) return null;
          const at = card.getBoundingClientRect();
          return { x: at.x + at.width / 2, y: at.y + at.height / 2 };
        }""")
    check(box is not None, "no list card is wholly on screen to tap")
    if has_touch:
        page.touchscreen.tap(box["x"], box["y"])
    else:
        page.mouse.click(box["x"], box["y"])


def walk(page: Page, base: str, has_touch: bool, label: str, shots: Path | None) -> None:
    def shoot(name: str) -> None:
        if shots is None:
            return
        shots.mkdir(parents=True, exist_ok=True)
        if not (shots / "BUILD.txt").exists():
            build = page.evaluate("""() => {
              const m = document.body.innerText.match(/build\\s+(\\d+)/i);
              return m ? m[1] : 'unknown';
            }""")
            write_build_stamp(shots, build)
        page.screenshot(path=str(shots / f"keepplace-{label}-{name}.png"))

    page.goto(base + "/?v=docs", wait_until="load")
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    check(page.locator("#documents-list .doc-card").count() == DOCS, f"{label}: the list did not draw every row")
    # A FRESH load lands at the bottom (the #2002 split: fresh = bottom).
    page.wait_for_function(
        """() => {
          const root = document.getElementById('scroll-area');
          return root.scrollTop + root.clientHeight >= root.scrollHeight - 2;
        }""", timeout=20_000)

    # The list mid-way, for both list checks below.
    page.evaluate("""() => {
      const root = document.getElementById('scroll-area');
      root.scrollTop = Math.floor(root.scrollHeight / 3);
    }""")
    page.wait_for_function(
        """(want) => Math.abs(document.getElementById('scroll-area').scrollTop - want) <= 2""",
        arg=int(scroll_state(page)["height"] // 3), timeout=20_000)
    list_place_opt = top_card(page)
    check(list_place_opt is not None, f"{label}: no list card at the top to anchor on")
    list_place = cast(tuple[str, float], list_place_opt)
    shoot("list-before")

    # (A) No document open: toggle to the channel and back; the list returns.
    show_pane(page, "pane-discord", has_touch, label)
    show_pane(page, "pane-docs", has_touch, label)
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    check(page.evaluate("() => document.getElementById('docs-reader').hidden"),
          f"{label}: a list return opened the reader")
    wait_top_card(page, list_place, label, "the list return")
    wait_settled(page)
    shoot("list-after")

    # (B) A document open mid-scroll: toggle away and back; same paragraph.
    tap_visible_card(page, has_touch)
    page.wait_for_selector("#doc-body h1", timeout=20_000)
    page.wait_for_function("() => document.getElementById('scroll-area').scrollTop === 0", timeout=20_000)
    page.evaluate("""() => {
      const root = document.getElementById('scroll-area');
      root.scrollTop = Math.floor((root.scrollHeight - root.clientHeight) / 2);
    }""")
    doc_place_opt = top_paragraph(page)
    check(doc_place_opt is not None, f"{label}: no paragraph at the top to anchor on")
    doc_place = cast(tuple[str, float], doc_place_opt)
    shoot("reader-before")
    show_pane(page, "pane-discord", has_touch, label)
    show_pane(page, "pane-docs", has_touch, label)
    page.wait_for_function("() => !document.getElementById('docs-reader').hidden", timeout=20_000)
    again = top_paragraph(page)
    check(again is not None and again[0] == doc_place[0] and abs(again[1] - doc_place[1]) <= 3,
          f"{label}: the reader returned at {again}, not its paragraph {doc_place}")
    wait_settled(page)
    shoot("reader-after")

    # (C) Back from the document after the toggle: the list's own place.
    page.tap("#doc-back") if has_touch else page.click("#doc-back")
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    wait_top_card(page, list_place, label, "Back after a toggle")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--screenshots", type=Path, default=None)
    parser.add_argument("--browser-executable", default=None)
    args = parser.parse_args()
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="documents-keepplace-check-") as tmp:
        work = Path(tmp)
        for index in range(DOCS):
            (work / f"doc-{index:02d}").write_text(doc_body(index))
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "documents-keepplace-bot-token-never-sent"\n'
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
            links = " ".join(f"https://paste.example/doc-{index:02d}" for index in range(DOCS))
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN, {"text": f"Reports: {links}"})
            deadline = time.time() + 30
            docs: list[dict[str, object]] = []
            while time.time() < deadline:
                call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=10&render=html", WRITE_TOKEN)
                answer = call(base, "GET", "/api/v1/documents?limit=100", READ_TOKEN)
                docs = cast(list[dict[str, object]], answer["documents"])
                if len(docs) == DOCS:
                    break
                time.sleep(0.5)
            check(len(docs) == DOCS, f"the fold registered {len(docs)} documents, not {DOCS}")

            with sync_playwright() as pw:
                browser = (pw.chromium.launch(executable_path=args.browser_executable)
                           if args.browser_executable else pw.chromium.launch())
                for scheme in ("light", "dark"):
                    phone = browser.new_context(
                        viewport={"width": 412, "height": 915}, has_touch=True, is_mobile=True,
                        color_scheme=scheme)
                    phone_page = phone.new_page()
                    phone_page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                    walk(phone_page, base, True, f"phone412-{scheme}", args.screenshots)
                    phone.close()

                    desk = browser.new_context(viewport={"width": 1280, "height": 800}, color_scheme=scheme)
                    desk_page = desk.new_page()
                    desk_page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                    walk(desk_page, base, False, f"desktop1280-{scheme}", args.screenshots)
                    desk.close()
                browser.close()
            print("documents keep-place: toggle restores the open document (top paragraph "
                  "within 3px), the list, and Back after a toggle, at 412x915 and 1280, light and dark")
            return 0
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()


if __name__ == "__main__":
    sys.exit(main())
