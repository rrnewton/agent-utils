#!/usr/bin/env python3
"""Discuss (`catchup-side-agent`, slice 1) in a real Chromium.

INTEGRATOR: run this after the server's catchup route and the page land
together (`make -C vibe-talk discuss-browser` once that target is added, or
`python3 tests/discuss_browser.py` against a built release binary). It is
written to the agreed contract, not run in the panel slice: the catchup
route is answered at the network edge with a contract-shaped body (the model
is nobody's to run in a walk), and dismiss/restore are recorded and answered
there too; everything else — the real server, sign-in, the channel, the bar —
is the shipped page at a 393x852 phone (touch) and a 1280x800 desk. The
walk server has no [summaries.llm], so client-config is answered with
`discuss_available` added — the capability a real Discuss deployment
reports; the disabled-button state without it is covered by unit tests.

The walk: open the channel, tap Discuss (nothing is asked), press Summarize,
see "Reading…" give way to the summary with its covered-messages footer, Mark
read sending exactly the markable ids, a footer link closing the panel, and
the panel overlaying the list without moving #scroll-area. Condition waits
only — no fixed sleeps in any assertion.
"""

import json
import socket
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import Page, Route, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "discuss-check-read-token-not-a-real-credential"
WRITE_TOKEN = "discuss-check-write-token-not-a-real-credential"
MARKABLE = ["100000000000000101", "100000000000000102"]

CATCHUP_ANSWER = {
    "thread_id": None,
    "view": "flat",
    "window": {"count": 12, "unread_count": 2, "read_count": 10,
               "from_id": MARKABLE[0], "through_id": MARKABLE[1],
               "truncated": False, "noise_hidden": 0},
    "unread_ids": MARKABLE,
    "markable_ids": MARKABLE,
    "footer_links": [
        {"id": MARKABLE[0], "label": "09:51 · Ada", "thread_id": None},
        {"id": MARKABLE[1], "label": "09:52 · Ben", "thread_id": None},
    ],
    "text": "## Progress\nAda shipped the fix. Ben is reviewing it.",
    "text_html": "<h2>Progress</h2><p>Ada shipped the fix. "
                 "Ben is reviewing it.</p>",
    "backend": "fake",
    "model": "fake-model",
    "stamp": {"context_ms": 120, "model_ms": 900, "first_token_ms": None,
              "total_ms": 1050, "input_tokens": None, "output_tokens": None,
              "input_chars": 4200, "output_chars": 48},
    "turns": [],
}


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


def walk(page: Page, base: str, width: int, height: int) -> None:
    """One Discuss walk at one size: open, read, mark, jump, close."""
    posts: list[dict[str, object]] = []
    writes: list[tuple[str, dict[str, object]]] = []

    def catchup(route: Route) -> None:
        posts.append(route.request.post_data_json or {})
        route.fulfill(status=200, content_type="application/json",
                      body=json.dumps(CATCHUP_ANSWER))

    def preview(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "channel": {"id": CHANNEL, "label": "lead team"},
            "thread_id": None, "view": "flat",
            "window": {"count": 12, "unread_count": 2, "read_count": 10,
                       "from_id": MARKABLE[0], "through_id": MARKABLE[1],
                       "truncated": False, "noise_hidden": 0},
            "unread": [
                {"id": MARKABLE[0], "at_ms": 1760000000000, "label": "09:51 · Ada"},
                {"id": MARKABLE[1], "at_ms": 1760000060000, "label": "09:52 · Ben"},
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
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup", catchup)
    page.route(f"**/api/v1/channels/{CHANNEL}/dismiss", dismiss)
    page.route(f"**/api/v1/channels/{CHANNEL}/restore", dismiss)
    page.add_init_script(
        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
    page.goto(base + "/", wait_until="load")
    page.wait_for_selector("#view-switch", state="visible", timeout=20_000)
    # To the channel view, which carries Discuss: the switch rotates through
    # the views, so press it until the channel picker (channel view only) is up.
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
    # `#1355 phone-bar-discuss-slot`: a fixed-width bar icon on a phone as on a
    # desk (the menu item stays as the second way in).
    page.wait_for_selector("#discuss-open", state="visible", timeout=20_000)

    scroller_before = page.evaluate("""() => {
      const area = document.getElementById('scroll-area');
      const rect = area.getBoundingClientRect();
      return {top: rect.top, height: rect.height};
    }""")

    page.click("#discuss-open")
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
    # Opening asks nothing: the bound is offered first, and only its press posts.
    page.wait_for_function(
        "() => document.getElementById('discuss-summarize').textContent === 'Summarize'"
        " && document.getElementById('discuss-count').value === '2'"
        " && document.getElementById('discuss-since').value !== ''",
        timeout=20_000)
    check(posts == [], f"opening posted {posts}, not nothing")
    # (the rendered answer is asserted after Summarize, below)
    page.click("#discuss-summarize")
    page.wait_for_function(
        "() => document.getElementById('discuss-answer').textContent.includes('Ada shipped')",
        timeout=20_000)
    check(len(posts) == 1 and posts[0].get("thread_id") is None
          and posts[0].get("view") in ("flat", "main")
          and posts[0].get("since_ms") == 1760000000000,
          f"the panel posted {posts}, not one bounded channel catch-up")
    heading = page.locator("#discuss-answer h2").text_content() or ""
    check(heading == "Progress", f"the answer did not render its heading: {heading!r}")
    raw = page.locator("#discuss-answer").text_content() or ""
    check("##" not in raw, f"raw Markdown showed in the answer: {raw!r}")
    status = page.locator("#discuss-status").text_content() or ""
    check(status == "Read 2 unread of 12", f"the status says {status!r}")
    covers = page.locator("#discuss-covers").text_content() or ""
    check(covers == "Other messages processed:", f"the footer says {covers!r}")
    check(page.locator("#discuss-links .discuss-link").count() == 2, "the footer links are missing")
    check(page.locator("#discuss-links .discuss-link").first.text_content() == "09:51", "the footer links are not dense HH:MM")
    stamp = page.locator("#discuss-stamp").text_content() or ""
    check("in 4200 chars" in stamp and "out 48 chars" in stamp, f"the stamp line says {stamp!r}")

    panel_box = page.locator("#discuss-panel").bounding_box()
    check(panel_box is not None and 0 <= panel_box["x"] and panel_box["x"] + panel_box["width"] <= width,
          f"the panel spills off the {width}x{height} viewport: {panel_box}")
    scroller_after = page.evaluate("""() => {
      const area = document.getElementById('scroll-area');
      const rect = area.getBoundingClientRect();
      return {top: rect.top, height: rect.height};
    }""")
    check(scroller_after == scroller_before,
          f"opening Discuss moved #scroll-area: {scroller_before} -> {scroller_after}")

    # `#1344 discuss-panel-resize`: the kept layout, at this size. A desk gets the
    # wider default, drag-resizable, kept per device; a phone gets a full-width
    # sheet at half height with messages above, Maximize to full, sticking.
    if width >= 900:
        check(panel_box is not None and abs(panel_box["width"] - 560) <= 2,
              f"the desk overlay is not at the wider default: {panel_box}")
        handle = page.locator("#discuss-resize").bounding_box()
        if handle is None:
            raise AssertionError("the desk panel has no resize edge")
        cx = handle["x"] + handle["width"] / 2
        cy = handle["y"] + handle["height"] / 2
        page.mouse.move(cx, cy)
        page.mouse.down()
        page.mouse.move(cx - 120, cy, steps=5)
        page.mouse.up()
        page.wait_for_function(
            "() => document.getElementById('discuss-panel').getBoundingClientRect().width >= 660",
            timeout=20_000)
        kept = page.evaluate("() => localStorage.getItem('vibe-talk.voice.discuss-width')")
        check(kept is not None and int(kept) >= 660, f"the dragged width was not kept: {kept!r}")
        page.click("#discuss-close")
        page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)
        page.click("#discuss-open")
        page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
        page.wait_for_function(
            "() => document.getElementById('discuss-answer').textContent.includes('Ada shipped')",
            timeout=20_000)
        reopened = page.locator("#discuss-panel").bounding_box()
        check(reopened is not None and reopened["width"] >= 660,
              f"the kept width did not survive close/reopen: {reopened}")
        # `#1354 discuss-layout-modes`: docked beside the list, maximized over
        # the main area, the choice kept per device.
        page.click("#discuss-dock-toggle")
        page.wait_for_function(
            "() => document.getElementById('screen-main').dataset.discuss === 'docked'",
            timeout=20_000)
        docked = page.locator("#discuss-panel").bounding_box()
        list_box = page.locator("#scroll-area").bounding_box()
        check(docked is not None and list_box is not None
              and abs(docked["x"] - (list_box["x"] + list_box["width"])) <= 4
              and docked["width"] >= 640,
              f"docked is not a column beside the list: panel={docked} list={list_box}")
        mode_kept = page.evaluate("() => localStorage.getItem('vibe-talk.voice.discuss-mode')")
        check(mode_kept == "docked", f"the docked mode was not kept: {mode_kept!r}")
        page.click("#discuss-dock-toggle")
        page.wait_for_function(
            "() => !document.getElementById('screen-main').hasAttribute('data-discuss')",
            timeout=20_000)
        page.click("#discuss-dock-toggle")
        page.wait_for_function(
            "() => document.getElementById('screen-main').dataset.discuss === 'docked'",
            timeout=20_000)
        # Docked then closed reserves no column (#1397): the screen loses
        # the mark and the list takes the full width again; reopening docks.
        page.click("#discuss-close")
        page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)
        page.wait_for_function(
            "() => !document.getElementById('screen-main').hasAttribute('data-discuss')",
            timeout=20_000)
        freed = page.locator("#scroll-area").bounding_box()
        check(freed is not None and list_box is not None and freed["width"] > list_box["width"],
              f"closing docked did not free the column: list={list_box} after={freed}")
        page.click("#discuss-open")
        page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
        page.wait_for_function(
            "() => document.getElementById('screen-main').dataset.discuss === 'docked'",
            timeout=20_000)
        page.click("#discuss-full-toggle")
        page.wait_for_function(
            "() => document.getElementById('screen-main').dataset.discuss === 'maximized'",
            timeout=20_000)
        full = page.locator("#discuss-panel").bounding_box()
        check(full is not None and full["width"] >= width - 100,
              f"maximized does not fill the main area: {full} in {width}x{height}")
        page.click("#discuss-full-toggle")
        page.wait_for_function(
            "() => document.getElementById('screen-main').dataset.discuss === 'docked'",
            timeout=20_000)
        page.click("#discuss-dock-toggle")
        page.wait_for_function(
            "() => !document.getElementById('screen-main').hasAttribute('data-discuss')",
            timeout=20_000)
    else:
        check(page.locator("#discuss-dock-toggle").is_hidden(),
              "the phone sheet shows the desk dock toggle")
        check(panel_box is not None and panel_box["width"] >= width - 2,
              f"the phone sheet is not full width: {panel_box} in {width}x{height}")
        check(panel_box is not None and height * 0.4 <= panel_box["height"] <= height * 0.6,
              f"the phone sheet is not at half height: {panel_box} in {width}x{height}")
        check(panel_box is not None and panel_box["y"] >= height * 0.2,
              f"no messages show above the half sheet: {panel_box} in {width}x{height}")
        page.click("#discuss-full-toggle")
        page.wait_for_function(
            "() => document.getElementById('discuss-panel').getBoundingClientRect().height >= innerHeight * 0.75",
            timeout=20_000)
        close_box = page.locator("#discuss-close").bounding_box()
        check(close_box is not None and close_box["y"] >= 0,
              f"the full sheet's header is off screen: {close_box}")
        kept = page.evaluate("() => localStorage.getItem('vibe-talk.voice.discuss-sheet-full')")
        check(kept == "1", f"the full height was not kept: {kept!r}")
        page.click("#discuss-close")
        page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)
        page.click("#action-menu-open")
        page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
        page.click("#discuss-menu-open")
        page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
        page.wait_for_function(
            "() => document.getElementById('discuss-panel').getBoundingClientRect().height >= innerHeight * 0.75",
            timeout=20_000)
        page.click("#discuss-full-toggle")
        page.wait_for_function(
            "() => document.getElementById('discuss-panel').getBoundingClientRect().height <= innerHeight * 0.6",
            timeout=20_000)

    page.click("#discuss-mark-read")
    page.wait_for_function(
        "() => !document.getElementById('discuss-mark-read').disabled", timeout=20_000)
    check(bool(writes) and writes[-1][0].endswith("/dismiss") and writes[-1][1] == {"messages": MARKABLE},
          f"Mark read sent {writes}, not exactly the markable ids")

    page.click("#discuss-links .discuss-link >> nth=0")
    # The jump keeps the panel open (it overlays the list it jumped).
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
    page.click("#discuss-close")
    page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="discuss-check-") as tmp:
        work = Path(tmp)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "discuss-check-bot-token-never-sent"\n'
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
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                 {"text": "Ada shipped the fix."})
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                 {"text": "Ben is reviewing it."})

            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                phone = browser.new_context(
                    viewport={"width": 393, "height": 852}, has_touch=True, is_mobile=True)
                walk(phone.new_page(), base, 393, 852)
                phone_small = browser.new_context(
                    viewport={"width": 390, "height": 844}, has_touch=True, is_mobile=True)
                walk(phone_small.new_page(), base, 390, 844)
                desk = browser.new_context(viewport={"width": 1280, "height": 800})
                walk(desk.new_page(), base, 1280, 800)
                browser.close()
            print("discuss browser: phone (393x852, 390x844) and desk walks passed")
            return 0
        finally:
            server.terminate()


if __name__ == "__main__":
    raise SystemExit(main())
