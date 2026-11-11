"""The Documents list's cards and chips against the REAL server (`#1263 documents-list-cards`).

Seeds pastes and GitHub markdown documents through the reply route — one
paste mentioned three times, one GitHub file linked at two different refs
(the duplicate from the owner's screenshot: two addresses, one document),
and enough fillers that the list scrolls — then opens the Documents view
at 393x852 (touch) and 1280x800 (mouse). At both sizes: every card holds
exactly one meta line that does not wrap (the filename ellipsises first),
the mentions entry is plain linked text that opens the mentioning-messages
pop-up, the duplicated file renders ONE card, and the filter chips are the
list's last row, visible at the bottom on open without scrolling.
"""

import json
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import cast

from playwright.sync_api import Page, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "documents-list-check-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-list-check-write-token-not-a-real-credential"

PROPOSAL = "2026-11-10-android-quest-research-proposal-very-long-name.md"
RAW_FILES: dict[str, str] = {
    f"/octo/widgets/main/docs/{PROPOSAL}": "# Proposal\n\nThe proposal body.\n",
}


class RawHandler(BaseHTTPRequestHandler):
    """A raw-content host: the seeded files, else 404."""

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
        body = RAW_FILES.get(self.path.split("?", 1)[0])
        if body is None:
            self.reply(404, b"not found")
        else:
            self.reply(200, body.encode())

    def reply(self, status: int, data: bytes) -> None:
        self.send_response(status)
        self.send_header("Content-Type", "text/markdown")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, format: str, *args: object) -> None:  # noqa: A002 - the base name
        pass


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


def check_list(page: Page, *, label: str, narrow: bool) -> None:
    """One meta line per card, mentions pop-up, one duplicate, chips docked."""
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    page.wait_for_function(
        "() => document.querySelectorAll('#documents-list .doc-card').length >= 12",
        timeout=20_000)
    facts = page.evaluate(
        """() => {
          const list = document.getElementById('documents-list');
          const area = document.getElementById('scroll-area');
          const bar = document.getElementById('control-bar');
          const cards = [...list.querySelectorAll('.doc-card')];
          const chips = list.querySelector('.doc-buckets');
          const chipsRect = chips.getBoundingClientRect();
          const barRect = bar.getBoundingClientRect();
          const cardFacts = cards.map((card) => {
            const metas = [...card.querySelectorAll('.doc-meta')];
            const meta = metas[0];
            const file = meta ? meta.querySelector('.doc-meta-file') : null;
            const tops = meta
              ? [...meta.children].map((kid) => Math.round(kid.getBoundingClientRect().top))
              : [];
            return {
              metas: metas.length,
              text: meta ? meta.textContent : '',
              oneLine: tops.length > 0 && Math.max(...tops) - Math.min(...tops) <= 2,
              fileEllipsised: file ? file.scrollWidth > file.clientWidth : false,
              mentionsTag: meta && meta.querySelector('.doc-mentions-link')
                ? getComputedStyle(meta.querySelector('.doc-mentions-link')).textDecorationLine
                : '',
            };
          });
          return {
            cards: cards.length,
            proposalCards: cards.filter((c) => c.textContent.includes('research-proposal')).length,
            cardFacts,
            chipsIsLast: list.lastElementChild === chips,
            chipsTop: chipsRect.top, chipsBottom: chipsRect.bottom,
            barTop: barRect.top, innerHeight,
            scrollTop: area.scrollTop, scrollHeight: area.scrollHeight,
            clientHeight: area.clientHeight,
          };
        }""")
    check(facts["cards"] == 12,
          f"{label}: expected 12 cards (13 server documents, one duplicated), got {facts['cards']}")
    check(facts["proposalCards"] == 1,
          f"{label}: the duplicated file renders {facts['proposalCards']} cards, not 1")
    for i, card in enumerate(facts["cardFacts"]):
        check(card["metas"] == 1, f"{label}: card {i} has {card['metas']} meta lines")
        check(card["oneLine"], f"{label}: card {i} meta wraps onto more than one line: {card['text']}")
        check("mentioned" in card["text"], f"{label}: card {i} meta misses the mentioned time: {card['text']}")
        check("mention" in card["text"], f"{label}: card {i} meta misses the mention count: {card['text']}")
        check("underline" in card["mentionsTag"],
              f"{label}: card {i} mentions is not underlined linked text: {card['mentionsTag']}")
    if narrow:
        check(any(card["fileEllipsised"] for card in facts["cardFacts"]),
              f"{label}: no long filename ellipsised on its meta line")
    check(facts["chipsIsLast"], f"{label}: the filter chips are not the list's last row")
    check(facts["scrollTop"] + facts["clientHeight"] >= facts["scrollHeight"] - 24,
          f"{label}: the list did not open at the bottom: {facts}")
    check(facts["chipsBottom"] <= facts["innerHeight"] and facts["chipsBottom"] <= facts["barTop"] + 2,
          f"{label}: the chips are not docked visible above the bottom bar on open: {facts}")

    # The mentions text link opens the mentioning-messages pop-up (3 items).
    demo = page.locator("#documents-list .doc-card", has_text="demo-doc")
    demo.locator(".doc-mentions-link").click()
    page.wait_for_selector("#docs-mentions .doc-mention-item", timeout=20_000)
    items = page.locator("#docs-mentions .doc-mention-item").count()
    check(items == 3, f"{label}: the mentions pop-up holds {items} messages, not 3")


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    raw = ThreadingHTTPServer(("127.0.0.1", 0), RawHandler)
    threading.Thread(target=raw.serve_forever, daemon=True).start()
    raw_base = f"http://127.0.0.1:{raw.server_address[1]}"
    with tempfile.TemporaryDirectory(prefix="documents-list-check-") as tmp:
        work = Path(tmp)
        (work / "demo-doc").write_text("# Paste note\n\nA paste body.\n")
        for n in range(10):
            (work / f"filler-{n}").write_text(f"# Filler {n}\n")
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "documents-list-check-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
            "[documents]\n"
            'paste_url_prefixes = ["https://paste.example/"]\n'
            'paste_command = ["/bin/cat", "--", "{}"]\n'
            f'github_raw_base = "{raw_base}"\n'
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
            for n in range(10):
                call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                     {"text": f"Filler {n}: https://paste.example/filler-{n}"})
            for n in range(3):
                call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                     {"text": f"Note {n}: https://paste.example/demo-doc"})
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                 {"text": f"Proposal: https://github.com/octo/widgets/blob/main/docs/{PROPOSAL}"})
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                 {"text": f"Proposal branch: https://github.com/octo/widgets/blob/feature/docs/{PROPOSAL}"})
            docs = []
            deadline = time.time() + 30
            while time.time() < deadline:
                call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=30&render=html", WRITE_TOKEN)
                answer = call(base, "GET", "/api/v1/documents?limit=30", READ_TOKEN)
                docs = cast(list[dict[str, object]], answer["documents"])
                if len(docs) == 13:
                    break
                time.sleep(0.5)
            check(len(docs) == 13,
                  f"the fold registered {len(docs)} documents, not 13 (the duplicate is two of them): {docs}")

            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                ctx = browser.new_context(viewport={"width": 393, "height": 852}, has_touch=True, is_mobile=True)
                page = ctx.new_page()
                page.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                page.goto(base + "/?v=docs", wait_until="load")
                check_list(page, label="phone", narrow=True)
                ctx.close()

                dctx = browser.new_context(viewport={"width": 1280, "height": 800})
                dpage = dctx.new_page()
                dpage.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                dpage.goto(base + "/?v=docs", wait_until="load")
                check_list(dpage, label="desktop", narrow=False)
                dctx.close()
                browser.close()
            print("documents list browser check: one meta line per card, mentions pop-up, "
                  "one card for the duplicated file, and bottom-docked chips, at phone and desktop sizes")
            return 0
        finally:
            server.terminate()
            server.wait(timeout=10)


if __name__ == "__main__":
    sys.exit(main())
