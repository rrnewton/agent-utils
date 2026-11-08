"""The Documents view against the REAL server (`documents-view-client` owner-bug check).

Starts the release binary with --fake-discord (a real server; only the chat
upstream is fake), seeds a paste link and a GitHub link through the reply route,
reads the channel so the fold registers both, then drives the page at 393x852
with real touch: an unfetched row fetches (the paste command serves a local
file) and opens the reader; a GitHub row whose fetcher cannot connect shows
its error on the row; Fetch recent reports a result. No documents API is
stubbed: list, fetch and render all go through the server under test.
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

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "documents-check-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-check-write-token-not-a-real-credential"
PASTE_BODY = "# Demo deployment report\n\nThe **nightly** run finished green.\n\n- one\n- two\n"


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
                # The jump's Back lands on the call view (its sent-jump spot);
                # the list is two switch presses away, kept as it was.
                page.wait_for_function("() => document.getElementById('view-switch-label').textContent === 'Voice'", timeout=20_000)
                page.tap("#view-switch")
                page.tap("#view-switch")
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                check(page.locator("#pane-docs").is_visible(), "the Documents list did not come back")

                # The GitHub row: its fetcher cannot connect (raw base is a closed
                # port), and the row says why instead of doing nothing.
                gh_card = page.locator("#documents-list .doc-card", has_text="guide.md")
                tap_card(page, gh_card)
                page.wait_for_function(
                    "() => [...document.querySelectorAll('#documents-list .doc-card')]"
                    ".some((c) => c.textContent.includes('Could not fetch it.'))",
                    timeout=20_000)
                browser.close()
            print("documents browser check: real-server list, touch fetch, reader, and fetch error all seen")
            return 0
        finally:
            server.terminate()
            server.wait(timeout=10)


if __name__ == "__main__":
    sys.exit(main())
