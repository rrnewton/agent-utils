#!/usr/bin/env python3
"""`#1820 vibe-talk-summary-strikethrough-live` in a real Chromium.

The summary bullet strike must be event-driven: archiving, marking Done or
putting back a message from its own row updates the citing bullet's
strikethrough at once, with no refresh, and following a bullet's link must
always bring the cited message on screen. Against the real server with
`--fake-discord`; the catchup route is answered at the network edge with a
contract-shaped body citing two real seeded rows, and dismiss/restore are
recorded and answered there too (as in discuss_browser.py). Condition
waits only — no fixed sleeps in any assertion.
"""

import json
import socket
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path

from typing import Literal

from playwright.sync_api import Page, Route, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "strike-live-check-read-token-not-a-real-credential"
WRITE_TOKEN = "strike-live-check-write-token-not-a-real-credential"


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port: int = sock.getsockname()[1]
        return port


def call(base: str, method: str, path: str, token: str,
         body: dict[str, object] | None = None) -> dict[str, object]:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method)
    req.add_header("Authorization", f"Bearer {token}")
    if data:
        req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, timeout=15) as res:
        parsed: object = json.loads(res.read() or b"null")
    assert isinstance(parsed, dict)
    return parsed


def walk(page: Page, base: str, width: int, height: int, scheme: Literal["dark", "light"]) -> None:
    cited: dict[str, str] = {}
    writes: list[tuple[str, dict[str, object]]] = []

    def answer() -> dict[str, object]:
        a, b = cited["a"], cited["b"]
        link = ("?c=" + CHANNEL + "&v=all&m=discord:" + CHANNEL + ":")
        return {
            "thread_id": None, "view": "flat",
            "window": {"count": 12, "unread_count": 2, "read_count": 10,
                       "from_id": a, "through_id": b,
                       "truncated": False, "noise_hidden": 0},
            "unread_ids": [a, b], "markable_ids": [a, b],
            "footer_links": [
                {"id": a, "label": "09:51 · Ada", "thread_id": None},
                {"id": b, "label": "09:52 · Ben", "thread_id": None},
            ],
            "text": "## Progress\nFirst point.\n\n## Review\nSecond point.",
            "text_html": (
                "<h2>Progress</h2><ul><li>First point "
                f"<a href=\"{link}{a}\">(09:51 ↩)</a></li></ul>"
                "<h2>Review</h2><ul><li>Second point "
                f"<a href=\"{link}{b}\">(09:52 ↩)</a></li></ul>"),
            "fresh_ids": [], "backend": "fake", "model": "fake-model",
            "stamp": {"context_ms": 120, "model_ms": 900, "first_token_ms": None,
                      "total_ms": 1050, "input_tokens": None, "output_tokens": None,
                      "input_chars": 4200, "output_chars": 48},
            "turns": [],
        }

    def catchup(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json",
                      body=json.dumps(answer()))

    def saved_record(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "exists": False, "thread_id": None, "current_summary": None,
            "timeline": [], "staleness": None,
        }))

    def preview(route: Route) -> None:
        a, b = cited["a"], cited["b"]
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "channel": {"id": CHANNEL, "label": "lead team"},
            "thread_id": None, "view": "flat",
            "window": {"count": 12, "unread_count": 2, "read_count": 10,
                       "from_id": a, "through_id": b,
                       "truncated": False, "noise_hidden": 0},
            "unread": [
                {"id": a, "at_ms": 1760000000000, "label": "09:51 · Ada"},
                {"id": b, "at_ms": 1760000060000, "label": "09:52 · Ben"},
            ],
        }))

    def dismiss(route: Route) -> None:
        writes.append((route.request.url, route.request.post_data_json or {}))
        route.fulfill(status=200, content_type="application/json", body="{}")

    def client_config(route: Route) -> None:
        body = call(base, "GET", "/api/v1/client-config", WRITE_TOKEN)
        body["discuss_available"] = True
        route.fulfill(status=200, content_type="application/json",
                      body=json.dumps(body))

    page.route("**/api/v1/client-config", client_config)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/preview*", preview)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/record*", saved_record)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup", catchup)
    page.route(f"**/api/v1/channels/{CHANNEL}/dismiss", dismiss)
    page.route(f"**/api/v1/channels/{CHANNEL}/restore", dismiss)
    page.add_init_script(
        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
    page.goto(base + "/", wait_until="load")
    page.wait_for_selector("#view-switch", state="visible", timeout=20_000)
    for _ in range(3):
        if not page.locator("#discord-channel").is_hidden():
            break
        page.click("#view-switch")
        try:
            page.wait_for_selector("#discord-channel", state="visible", timeout=5_000)
            break
        except Exception:
            continue
    page.wait_for_selector("#discord-channel", state="visible", timeout=20_000)
    page.wait_for_selector("#discord-log > *", timeout=20_000)
    # Two real seeded rows the summary will cite: one near the head, one
    # further down, so following their links has somewhere to travel.
    ids = page.evaluate(
        "() => [...document.querySelectorAll('#discord-log > *')]"
        ".map((r) => r.getAttribute('data-id')).filter(Boolean)")
    check(len(ids) >= 6, f"the seeded channel drew {len(ids)} rows")
    cited["a"], cited["b"] = ids[1], ids[5]

    if page.locator("#discuss-open").is_visible():
        page.click("#discuss-open")
    else:
        page.click("#action-menu-open")
        page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
        page.click("#discuss-menu-open")
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
    page.click("#discuss-summarize")
    page.wait_for_function(
        "() => document.getElementById('discuss-answer').textContent.includes('First point')",
        timeout=20_000)
    check(page.locator("#discuss-answer .discuss-struck").count() == 0,
          "a bullet stood struck before anything was read")
    if page.locator("#discuss-answer .discuss-collapse-bar button").count() > 0:
        page.locator("#discuss-answer .discuss-collapse-bar button").first.click()
        page.wait_for_function(
            "() => [...document.querySelectorAll('#discuss-answer details')].every((d) => d.open)",
            timeout=20_000)
    page.evaluate("() => { window.__strikeLiveMark = 'no reload'; }")

    def on_screen(mid: str) -> str:
        return f"""() => {{
          const row = document.querySelector('#discord-log > [data-id="{mid}"]');
          const area = document.getElementById('scroll-area');
          if (!row || !area) return false;
          const r = row.getBoundingClientRect();
          const a = area.getBoundingClientRect();
          return r.bottom > a.top && r.top < a.bottom;
        }}"""

    def follow(bullet: int, mid: str) -> None:
        page.locator("#discuss-answer li a").nth(bullet).click()
        page.wait_for_function(on_screen(mid), timeout=20_000)

    # Repeated link following always lands on screen: A, B, back to A,
    # B again, A again — the owner's 1st/3rd/back-to-1st walk.
    follow(0, cited["a"])
    follow(1, cited["b"])
    follow(0, cited["a"])
    follow(1, cited["b"])
    follow(0, cited["a"])
    check(page.evaluate("() => window.__strikeLiveMark") == "no reload",
          "the page reloaded while following links")

    # Archiving from the message's own row strikes its bullet at once,
    # with no refresh — the same turn the row itself greys.
    row_a = page.locator(f'#discord-log > [data-id="{cited["a"]}"]')
    row_a.locator(".done-button").click()
    page.wait_for_function(
        "() => { const li = document.querySelectorAll('#discuss-answer li')[0];"
        " return li && li.classList.contains('discuss-struck')"
        " && getComputedStyle(li).textDecorationLine.includes('line-through'); }",
        timeout=20_000)
    check(page.evaluate("() => window.__strikeLiveMark") == "no reload",
          "the page reloaded when the row was archived")
    check(page.locator("#discuss-answer li.discuss-struck").count() == 1,
          "the other bullet was struck by the first row's archive")

    # Putting it back from its row un-strikes the bullet live too.
    row_a.locator(".done-button").click()
    page.wait_for_function(
        "() => { const li = document.querySelectorAll('#discuss-answer li')[0];"
        " return li && !li.classList.contains('discuss-struck'); }",
        timeout=20_000)

    # And archiving both cited rows strikes both bullets, still no reload.
    row_a.locator(".done-button").click()
    page.wait_for_function(
        "() => document.querySelectorAll('#discuss-answer li')[0]"
        ".classList.contains('discuss-struck')", timeout=20_000)
    follow(1, cited["b"])
    row_b = page.locator(f'#discord-log > [data-id="{cited["b"]}"]')
    row_b.locator(".done-button").click()
    page.wait_for_function(
        "() => document.querySelectorAll('#discuss-answer li.discuss-struck').length === 2",
        timeout=20_000)
    check(page.evaluate("() => window.__strikeLiveMark") == "no reload",
          "the page reloaded while archiving from the rows")
    check(any(url.endswith("/dismiss") for url, _ in writes),
          f"the row archive never reached the dismiss route: {writes}")


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="strike-live-check-") as tmp:
        work = Path(tmp)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "strike-live-check-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
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
            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                combos: list[tuple[int, int, Literal["dark", "light"]]] = [(412, 915, "dark"), (1280, 800, "light")]
                for width, height, scheme in combos:
                    context = browser.new_context(
                        viewport={"width": width, "height": height},
                        color_scheme=scheme)
                    walk(context.new_page(), base, width, height, scheme)
                    context.close()
                browser.close()
        finally:
            server.terminate()
    print("discuss strike-live browser: phone (412x915 dark) and desk (1280x800 light) walks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
