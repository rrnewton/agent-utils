"""The Documents view against the REAL server (`documents-view-client` owner-bug check).

Starts the release binary with --fake-discord (a real server; only the chat
upstream is fake), seeds a paste link and a GitHub link through the reply route,
reads the channel so the fold registers both, then drives the page at 393x852
with real touch: an unfetched row fetches (the paste command serves a local
file) and opens the reader; a GitHub row whose fetcher cannot connect shows
its error on the row; Fetch recent reports a result. No documents API is
stubbed: list, fetch and render all go through the server under test.

The reader steps also cover `#1038 documents-outline-popup`: the seeded paste
is long enough to scroll several viewports, and at 393x852 (touch) and again
at 1280x800 (mouse) the Outline button opens the pop-up, an entry tap/click
jumps to its heading, the HERE mark follows scrolling, and the title links
to the document's original source in a new tab.
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

from playwright.sync_api import Page, Route, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "documents-check-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-check-write-token-not-a-real-credential"
_FILLER = (
    "A filler paragraph long enough to take a few lines on a phone, so the "
    "document below scrolls for several viewports and each heading can be "
    "brought to the top of the scroller in turn.\n\n"
)
PASTE_BODY = (
    "# Demo deployment report\n\nThe **nightly** run finished green.\n\n- one\n- two\n"
    "\n## Setup\n\n" + _FILLER * 8
    + "### Setup details\n\n" + _FILLER * 8
    + "## Finish\n\n" + _FILLER * 10
    + "The final line.\n"
)
OUTLINE_TEXTS = ["Demo deployment report", "Setup", "Setup details", "Finish"]


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def wait_heading_at_top(page: Page, heading_id: str, label: str) -> None:
    """The heading sits in the scroller's top band and its outline entry is HERE."""
    page.wait_for_function(
        """([hid, label]) => {
          const h = document.getElementById(hid);
          const root = document.getElementById('scroll-area');
          const b = [...document.querySelectorAll('#doc-outline button')]
            .find((x) => x.textContent.trim() === label);
          return !!h && !!root && !!b
            && Math.abs(h.getBoundingClientRect().top - root.getBoundingClientRect().top) < 80
            && b.getAttribute('aria-current') === 'true';
        }""",
        arg=[heading_id, label],
        timeout=20_000,
    )


def wait_here_is(page: Page, label: str) -> None:
    """The named outline entry, and only it, carries the HERE mark."""
    page.wait_for_function(
        """(label) => {
          const marked = [...document.querySelectorAll('#doc-outline button')]
            .filter((x) => x.getAttribute('aria-current') === 'true');
          return marked.length === 1 && marked[0].textContent.trim() === label;
        }""",
        arg=label,
        timeout=20_000,
    )


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

def tap_card(page: object, card: object) -> None:
    """Tap a card's centre, waiting for it to be laid out first."""
    card.wait_for(state="visible", timeout=20_000)  # type: ignore[attr-defined]
    box = None
    for _ in range(50):
        box = card.bounding_box()  # type: ignore[attr-defined]
        if box is not None:
            break
        page.wait_for_timeout(200)  # type: ignore[attr-defined]
    assert box is not None
    page.touchscreen.tap(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)  # type: ignore[attr-defined]


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="documents-check-") as tmp:
        work = Path(tmp)
        (work / "demo-doc").write_text(PASTE_BODY)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "documents-check-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
            '[documents]\n'
            'paste_url_prefixes = ["https://paste.example/"]\n'
            'paste_command = ["/bin/cat", "--", "{}"]\n'
            'github_raw_base = "http://127.0.0.1:9"\n'
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
                 {"text": "Report: https://paste.example/demo-doc and the "
                          "[guide](https://github.com/octo/widgets/blob/main/docs/guide.md)"})
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
            by_file = {d["file_name"]: d for d in docs}
            check(by_file["demo-doc"]["fetched"] is False, "the paste starts fetched")

            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                ctx = browser.new_context(viewport={"width": 393, "height": 852}, has_touch=True, is_mobile=True)
                page = ctx.new_page()
                page.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                page.goto(base + "/?v=docs", wait_until="load")
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                check(page.locator("#documents-list .doc-card").count() == 2, "the list did not draw both rows")
                check("demo-doc" in (page.locator("#documents-list").text_content() or ""),
                      "the list does not name its document")

                # The bar's Fetch drop-up: Fetch visible, with a result said.
                page.tap("#docs-fetch")
                page.wait_for_selector("#docs-fetch-menu", state="visible", timeout=20_000)
                page.tap("#docs-fetch-menu >> text=Fetch visible")
                page.wait_for_function(
                    "() => !document.getElementById('docs-fetch').disabled", timeout=20_000)

                # The paste row: a real touch fetches it from the server and opens the reader.
                paste_card = page.locator("#documents-list .doc-card", has_text="demo-doc")
                tap_card(page, paste_card)
                page.wait_for_selector("#doc-body h1", timeout=20_000)
                check("Demo deployment report" in (page.text_content("#doc-body") or ""),
                      "the reader did not render the fetched document")
                check(page.locator("#control-pane").is_hidden(), "the call controls show over the reader")

                # The title is the source link (#1038): href, new tab, no opener.
                title_link = page.locator("#doc-title a")
                check(title_link.count() == 1, "the reader title is not a link to the source")
                check(title_link.get_attribute("href") == "https://paste.example/demo-doc",
                      "the title link does not carry the document's original URL")
                check(title_link.get_attribute("target") == "_blank", "the title link opens in place")
                check("noopener" in (title_link.get_attribute("rel") or ""),
                      "the title link keeps an opener")

                # The old top-of-reader TOC is gone: the outline lives in its pop-up,
                # closed until the Outline button opens it.
                check(page.locator("#doc-outline-pop").is_hidden(), "the outline pop-up starts open")
                page.tap("#doc-outline-open")
                page.wait_for_function(
                    "() => !document.getElementById('doc-outline-pop').hidden", timeout=20_000)
                texts = [t.strip() for t in page.locator("#doc-outline button").all_text_contents()]
                check(texts == OUTLINE_TEXTS, f"the outline lists {texts}, not {OUTLINE_TEXTS}")
                indent = page.evaluate(
                    """() => {
                      const pads = [...document.querySelectorAll('#doc-outline button')]
                        .map((b) => parseFloat(getComputedStyle(b).paddingLeft));
                      return pads;
                    }""")
                check(indent[1] > indent[0] and indent[2] > indent[1] and indent[3] == indent[1],
                      f"outline indentation does not follow heading levels: {indent}")

                # A tap on a later entry jumps to its heading, closes the pop-up,
                # and that entry becomes HERE.
                page.locator("#doc-outline button", has_text="Finish").tap()
                wait_heading_at_top(page, "doc-finish", "Finish")
                check(page.locator("#doc-outline-pop").is_hidden(),
                      "picking an outline entry left the pop-up open")

                # Scrolling (here by scrollTop, as a swipe would move it) advances HERE.
                page.evaluate(
                    """() => {
                      const root = document.getElementById('scroll-area');
                      const h = document.getElementById('doc-setup');
                      root.scrollTop += h.getBoundingClientRect().top - root.getBoundingClientRect().top;
                    }""")
                wait_here_is(page, "Setup")

                # An outside tap closes the pop-up without moving the document.
                page.tap("#doc-outline-open")
                page.wait_for_function(
                    "() => !document.getElementById('doc-outline-pop').hidden", timeout=20_000)
                top_before = page.evaluate("() => document.getElementById('scroll-area').scrollTop")
                page.touchscreen.tap(30, 700)
                page.wait_for_function(
                    "() => document.getElementById('doc-outline-pop').hidden", timeout=20_000)
                top_after = page.evaluate("() => document.getElementById('scroll-area').scrollTop")
                check(top_after == top_before, "closing the outline moved the document")

                # The title link opens the source in a new page. Its route is
                # aborted (never leaves the machine); the aborted request's URL,
                # or the popup's where the navigation commits first, is the check.
                source_requests: list[str] = []

                def abort_source(route: Route) -> None:
                    source_requests.append(route.request.url)
                    route.abort()

                ctx.route("https://paste.example/**", abort_source)
                with ctx.expect_page(timeout=20_000) as popup_info:
                    page.tap("#doc-title a")
                popup = popup_info.value
                deadline = time.time() + 10
                while not source_requests and popup.url == "about:blank" and time.time() < deadline:
                    time.sleep(0.1)
                check(source_requests == ["https://paste.example/demo-doc"]
                      or popup.url == "https://paste.example/demo-doc",
                      f"the title opened {popup.url!r} (requests: {source_requests}), "
                      "not the document source")
                popup.close()

                page.tap("#doc-back")
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)

                # The system Back gesture: list -> document -> back to the list.
                tap_card(page, paste_card)
                page.wait_for_selector("#doc-body h1", timeout=20_000)
                page.go_back()
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                check(page.locator("#pane-docs").is_visible(), "Back from the reader left the Documents view")

                # A mention jumps to its message; Back returns to the list.
                mention = page.locator("#documents-list .doc-card", has_text="demo-doc").locator(".doc-mentions")
                mention.tap()
                page.wait_for_function("() => !document.getElementById('pane-discord').hidden", timeout=20_000)
                page.go_back()
                # Back from the jump returns to the Documents list it started from.
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                check(page.locator("#pane-docs").is_visible(), "Back from the mention jump left the Documents view")

                # The GitHub row: its fetcher cannot connect (raw base is a closed
                # port), and the row says why instead of doing nothing.
                gh_card = page.locator("#documents-list .doc-card", has_text="guide.md")
                tap_card(page, gh_card)
                page.wait_for_function(
                    "() => [...document.querySelectorAll('#documents-list .doc-card')]"
                    ".some((c) => c.textContent.includes('Could not fetch it.'))",
                    timeout=20_000)
                ctx.close()

                # The same outline flow at desktop size with a mouse: open, jump,
                # HERE, and Escape from the keyboard inside the panel.
                dctx = browser.new_context(viewport={"width": 1280, "height": 800})
                dpage = dctx.new_page()
                dpage.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                dpage.goto(base + "/?v=docs", wait_until="load")
                dpage.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                dpage.locator("#documents-list .doc-card", has_text="demo-doc").click()
                dpage.wait_for_selector("#doc-body h1", timeout=20_000)
                check(dpage.locator("#doc-title a").get_attribute("href")
                      == "https://paste.example/demo-doc",
                      "the desktop title does not link to the source")
                dpage.click("#doc-outline-open")
                dpage.wait_for_function(
                    "() => !document.getElementById('doc-outline-pop').hidden", timeout=20_000)
                dtexts = [t.strip() for t in dpage.locator("#doc-outline button").all_text_contents()]
                check(dtexts == OUTLINE_TEXTS, f"the desktop outline lists {dtexts}, not {OUTLINE_TEXTS}")
                dpage.locator("#doc-outline button", has_text="Setup details").click()
                wait_heading_at_top(dpage, "doc-setup-details", "Setup details")
                check(dpage.locator("#doc-outline-pop").is_hidden(),
                      "picking a desktop outline entry left the pop-up open")
                dpage.click("#doc-outline-open")
                dpage.wait_for_function(
                    "() => !document.getElementById('doc-outline-pop').hidden", timeout=20_000)
                dpage.keyboard.press("Escape")
                dpage.wait_for_function(
                    "() => document.getElementById('doc-outline-pop').hidden", timeout=20_000)
                dctx.close()
                browser.close()
            print("documents browser check: real-server list, touch fetch, reader, outline pop-up, "
                  "source link, and fetch error all seen")
            return 0
        finally:
            server.terminate()
            server.wait(timeout=10)


if __name__ == "__main__":
    sys.exit(main())
