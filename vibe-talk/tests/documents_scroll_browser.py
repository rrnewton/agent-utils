"""Documents scroll landings against the REAL server (`#1264 documents-scroll-landings`).

Seeds 24 paste documents through the reply route, then at 393x852 (touch)
and 1280x800 (mouse): a SLOW-FETCH first open (the single-document GET
delayed at the route) lands at the reader's top; Back with the list rows
delayed lands at the list's bottom; and Back after deliberately scrolling
the list up returns exactly there. No documents API is stubbed: the route
handlers only delay the real server's answers.
"""

import json
import re
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
READ_TOKEN = "documents-scroll-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-scroll-write-token-not-a-real-credential"
DOCS = 24
_FILLER = (
    "A filler paragraph long enough to take a few lines on a phone, so the "
    "document below scrolls for several viewports and the list of documents "
    "is far taller than the screen that shows it.\n\n"
)


def doc_body(index: int) -> str:
    return f"# Scroll document {index:02d}\n\n" + _FILLER * 18 + f"The final line of document {index:02d}.\n"


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


def at_bottom(page: Page) -> bool:
    state = scroll_state(page)
    return state["top"] + state["view"] >= state["height"] - 2


def delay_routes(page: Page, pattern: str) -> re.Pattern[str]:
    regex = re.compile(pattern)

    def handler(route: Route) -> None:
        time.sleep(1.2)
        route.continue_()

    page.route(regex, handler)
    return regex


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


def walk(page: Page, base: str, has_touch: bool, label: str) -> None:
    page.goto(base + "/?v=docs", wait_until="load")
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    check(page.locator("#documents-list .doc-card").count() == DOCS, f"{label}: the list did not draw every row")
    page.wait_for_function(
        """() => {
          const root = document.getElementById('scroll-area');
          return root.scrollTop + root.clientHeight >= root.scrollHeight - 2;
        }""", timeout=20_000)
    check(at_bottom(page), f"{label}: entering the list did not land at its bottom")

    # (A) A first open with a slow document body lands at the top.
    single = delay_routes(page, r"/api/v1/documents/[^/?]+$")
    tap_visible_card(page, has_touch)
    page.wait_for_selector("#doc-body h1", timeout=20_000)
    page.unroute(single)
    page.wait_for_function(
        "() => document.getElementById('scroll-area').scrollTop === 0", timeout=20_000)
    state = scroll_state(page)
    check(state["top"] == 0, f"{label}: a first open landed at scrollTop {state['top']}, not the top")

    # (B) Back with slow list rows lands at the bottom.
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 400; }")
    listing = delay_routes(page, r"/api/v1/documents\?")
    page.tap("#doc-back") if has_touch else page.click("#doc-back")
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    page.wait_for_function(
        """() => {
          const root = document.getElementById('scroll-area');
          return root.scrollTop + root.clientHeight >= root.scrollHeight - 2;
        }""", timeout=20_000)
    page.unroute(listing)
    check(at_bottom(page), f"{label}: Back did not land at the list's bottom: {scroll_state(page)}")

    # (B') Back after deliberately scrolling up returns exactly there.
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 120; }")
    left_at = scroll_state(page)["top"]
    check(left_at == 120, f"{label}: the list would not park at 120 (at {left_at})")
    tap_visible_card(page, has_touch)
    page.wait_for_selector("#doc-body h1", timeout=20_000)
    page.wait_for_function(
        "() => document.getElementById('scroll-area').scrollTop === 0", timeout=20_000)
    state = scroll_state(page)
    check(state["top"] == 0, f"{label}: the second first-open landed at {state['top']}, not the top")
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 500; }")
    listing = delay_routes(page, r"/api/v1/documents\?")
    page.tap("#doc-back") if has_touch else page.click("#doc-back")
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    page.unroute(listing)
    page.wait_for_function(
        "(want) => document.getElementById('scroll-area').scrollTop === want",
        arg=120, timeout=20_000)
    state = scroll_state(page)
    check(state["top"] == 120, f"{label}: Back returned to {state['top']}, not the scrolled-up 120")


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="documents-scroll-check-") as tmp:
        work = Path(tmp)
        for index in range(DOCS):
            (work / f"doc-{index:02d}").write_text(doc_body(index))
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "documents-scroll-bot-token-never-sent"\n'
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
                browser = pw.chromium.launch()
                phone = browser.new_context(viewport={"width": 393, "height": 852}, has_touch=True, is_mobile=True)
                phone_page = phone.new_page()
                phone_page.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                phone_page.goto(base + "/?v=docs", wait_until="load")
                walk(phone_page, base, True, "phone 393x852")
                phone.close()

                desk = browser.new_context(viewport={"width": 1280, "height": 800})
                desk_page = desk.new_page()
                desk_page.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                desk_page.goto(base + "/?v=docs", wait_until="load")
                walk(desk_page, base, False, "desktop 1280x800")
                desk.close()
                browser.close()
            print("documents scroll landings: first open at the top, Back at the bottom, "
                  "Back after scrolling up exact, slow-fetch, at 393x852 and 1280x800")
            return 0
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()


if __name__ == "__main__":
    sys.exit(main())
