"""Documents Back and filter place against the REAL server (`#1908 vibe-talk-documents-back-and-filter-place`).

Seeds 100 paste documents (one full first page) through the reply route,
stars three of them, then — while the reader is in the newest starred
document — posts 20 more, so the first page no longer reaches the older
starred ones. At phone and desktop sizes it walks the owner's sequences:

1. Starred (a short list), open a document, press Back: the SAME starred
   list comes back — same cards, same place, its count line reading the
   restored window. Before the fix the Back refresh re-read only the
   newest page, so the older starred documents silently left the list
   (the owner's "1 of 89" in place of "6 of 276"). (The fake chat upstream
   stamps every message alike, so the window is shifted by new arrivals
   rather than by time-paging, which needs real mention times.)
2. Near the bottom of the list, clear the Starred filter: the reader's
   place is kept (the anchor card's rect.top within 2 px), never a jump
   to the top.
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

from playwright.sync_api import Page, ViewportSize, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "documents-backfilter-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-backfilter-write-token-not-a-real-credential"
DOCS = 100
EXTRA = 20
STARRED = ["doc-002", "doc-005", "doc-099"]


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


def list_facts(page: Page) -> dict[str, object]:
    facts = page.evaluate(
        """() => {
          const list = document.getElementById('documents-list');
          const area = document.getElementById('scroll-area');
          const head = list.querySelector('.doc-list-head');
          const cards = [...list.querySelectorAll('.doc-card')];
          return {
            head: head ? head.textContent : '',
            keys: cards.map((card) => card.getAttribute('data-key')),
            names: cards.map((card) => {
              const file = card.querySelector('.doc-meta-file');
              return file ? file.textContent : '';
            }),
            scrollTop: area.scrollTop,
            scrollHeight: area.scrollHeight,
            clientHeight: area.clientHeight,
          };
        }""")
    return cast(dict[str, object], facts)


def card_top(page: Page, name: str) -> float:
    top = page.evaluate(
        """(name) => {
          const cards = [...document.querySelectorAll('#documents-list .doc-card')];
          const card = cards.find((el) => {
            const file = el.querySelector('.doc-meta-file');
            return file && file.textContent === name;
          });
          return card ? card.getBoundingClientRect().top : null;
        }""", name)
    check(top is not None, f"no card named {name} is drawn")
    return float(top)


def anchor_now(page: Page) -> tuple[str, float]:
    """The first card at or below the scroller's top, and its height."""
    anchor = page.evaluate(
        """() => {
          const area = document.getElementById('scroll-area');
          const band = area.getBoundingClientRect();
          const cards = [...document.querySelectorAll('#documents-list .doc-card')];
          const card = cards.find((el) => el.getBoundingClientRect().bottom > band.top);
          if (!card) return null;
          const file = card.querySelector('.doc-meta-file');
          return { name: file ? file.textContent : '', top: card.getBoundingClientRect().top };
        }""")
    check(anchor is not None, "no card is at the scroller's top to anchor on")
    held = cast(dict[str, object], anchor)
    return str(held["name"]), float(cast(float, held["top"]))


def star(page: Page, name: str) -> None:
    card = page.locator("#documents-list .doc-card", has_text=name).first
    card.locator(".doc-star").click()
    page.wait_for_function(
        """(name) => {
          const cards = [...document.querySelectorAll('#documents-list .doc-card')];
          const card = cards.find((el) => {
            const file = el.querySelector('.doc-meta-file');
            return file && file.textContent === name;
          });
          const star = card ? card.querySelector('.doc-star') : null;
          return !!star && star.getAttribute('aria-pressed') === 'true';
        }""", arg=name, timeout=20_000)


def walk(page: Page, base: str, grow: object, label: str) -> None:
    page.goto(base + "/?v=docs", wait_until="load")
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    page.wait_for_function(
        f"() => document.querySelectorAll('#documents-list .doc-card').length === {DOCS}",
        timeout=20_000)

    for name in STARRED:
        star(page, name)

    # The owner's filtered list, parked at its bottom.
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    page.locator('.doc-buckets [data-bucket="starred"]').click()
    page.wait_for_function(
        f"() => document.querySelectorAll('#documents-list .doc-card').length === {len(STARRED)}",
        timeout=20_000)
    before = list_facts(page)
    check(before["head"] == f"Oldest first · {len(STARRED)} of {DOCS} documents · newest at the bottom",
          f"{label}: the starred list reads {before['head']!r}")
    check(before["names"] == STARRED, f"{label}: the starred cards are {before['names']}, not {STARRED}")
    top_before = card_top(page, "doc-099")

    # Into the newest starred document; while it is open, newer documents
    # arrive and the first page stops reaching the older starred ones.
    page.locator("#documents-list .doc-card", has_text="doc-099").first.click()
    page.wait_for_selector("#doc-body h1", timeout=20_000)
    grow()  # type: ignore[operator]
    page.click("#doc-back")
    page.wait_for_function(
        """() => {
          const view = document.getElementById('docs-list-view');
          return view && !view.hidden && !!document.querySelector('#documents-list .doc-card');
        }""", timeout=20_000)
    page.wait_for_function(
        f"() => document.querySelectorAll('#documents-list .doc-card').length === {len(STARRED)}",
        timeout=20_000)
    total = DOCS + EXTRA
    after = list_facts(page)
    check(after["head"] == f"Oldest first · {len(STARRED)} of {total} documents · newest at the bottom",
          f"{label}: Back read {after['head']!r}, not the restored starred list over {total}")
    check(after["names"] == before["names"],
          f"{label}: Back changed the starred cards from {before['names']} to {after['names']}")
    top_after = card_top(page, "doc-099")
    check(abs(top_after - top_before) <= 2,
          f"{label}: Back moved the opened card from rect.top {top_before} to {top_after}")

    # Near the bottom, clear Starred: the place is kept, never the top.
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    anchor_name, anchor_top = anchor_now(page)
    page.locator('.doc-buckets [data-bucket="starred"]').click()
    page.wait_for_function(
        f"() => document.querySelectorAll('#documents-list .doc-card').length === {total}",
        timeout=20_000)
    kept = card_top(page, anchor_name)
    check(abs(kept - anchor_top) <= 2,
          f"{label}: clearing Starred moved the anchor card {anchor_name} "
          f"from rect.top {anchor_top} to {kept} (a jump to the top reads as a large move)")
    facts = list_facts(page)
    check(float(cast(float, facts["scrollTop"])) > 100,
          f"{label}: clearing Starred left the reader at scrollTop {facts['scrollTop']}, the top of the list")


def run_size(viewport: ViewportSize, touch: bool, label: str) -> None:
    """One full walk on a fresh server: starred state is synced server-side
    (`#1265`), so each size's starring starts from an empty store."""
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="documents-backfilter-check-") as tmp:
        work = Path(tmp)
        for index in range(DOCS + EXTRA):
            (work / f"doc-{index:03d}").write_text(f"# Backfilter document {index:03d}\n\nIts body.\n")
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "documents-backfilter-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
            "[documents]\n"
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

            def post(index: int) -> None:
                call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                     {"text": f"Document {index:03d}: https://paste.example/doc-{index:03d}"})

            def folded(count: int) -> None:
                limit = time.time() + 60
                docs: list[dict[str, object]] = []
                while time.time() < limit:
                    call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=10&render=html", WRITE_TOKEN)
                    answer = call(base, "GET", "/api/v1/documents?limit=500", READ_TOKEN)
                    docs = cast(list[dict[str, object]], answer["documents"])
                    if len(docs) == count:
                        return
                    time.sleep(0.5)
                check(False, f"the fold registered {len(docs)} documents, not {count}")

            for index in range(DOCS):
                post(index)
            folded(DOCS)

            def grow() -> None:
                for index in range(DOCS, DOCS + EXTRA):
                    post(index)
                folded(DOCS + EXTRA)

            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                ctx = browser.new_context(viewport=viewport, has_touch=touch, is_mobile=touch)
                page = ctx.new_page()
                page.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                walk(page, base, grow, label)
                ctx.close()
                browser.close()
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()


def main() -> int:
    run_size({"width": 412, "height": 915}, True, "phone 412x915")
    run_size({"width": 1280, "height": 800}, False, "desktop 1280x800")
    print("documents back/filter place: Back restores the exact starred list and "
          "clearing the filter keeps the reader's place, at phone and desktop sizes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
