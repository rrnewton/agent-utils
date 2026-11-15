#!/usr/bin/env python3
"""Discuss docked on a phone (`#1526 discuss-docked-pane-mobile`).

The half-height Discuss sheet used to overlay the message list: the list
kept its full height underneath, so a citation jump centred its row in a
scrollport whose lower half lay behind the sheet and the row landed hidden
at the sheet's top edge (owner UI request 055). Docked, the sheet takes a
grid row under the list and #scroll-area ends at its top edge.

The walk, at a 390x844 phone and a 1280x800 desk against the real server
(fake chat upstream, contract-shaped catchup answer at the network edge):
seed a long channel, open Discuss at half height, and on the phone assert
the scroller's bottom is the panel's top, the list scrolls to its last row
inside the visible upper area, a citation tap on the oldest message lands
that row fully inside the visible list area with the panel still open, the
full-height sheet and the closed state still work, and the scroller takes
its full height back on close. On the desk the overlay is unchanged: the
scroller does not move. Condition waits only — no fixed sleeps.
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
READ_TOKEN = "discuss-dock-check-read-token-not-a-real-credential"
WRITE_TOKEN = "discuss-dock-check-write-token-not-a-real-credential"
SEEDED = 40


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


def install_routes(page: Page, base: str, target: dict[str, str]) -> None:
    """The catchup contract at the network edge; `target` names the oldest
    seeded message once the list has drawn it."""

    def answer() -> dict[str, object]:
        cited = target.get("id", "")
        return {
            "thread_id": None,
            "view": "flat",
            "window": {"count": SEEDED, "unread_count": 2, "read_count": SEEDED - 2,
                       "from_id": cited, "through_id": cited,
                       "truncated": False, "noise_hidden": 0},
            "unread_ids": [cited],
            "markable_ids": [cited],
            "cited_ids": [cited],
            "footer_links": [{"id": cited, "label": "07:10 · First", "thread_id": None}],
            "text": "## Dock check\nThe first message is cited.",
            "text_html": "<h2>Dock check</h2><ul><li>The first message is cited "
                         f"<a href=\"?c={CHANNEL}&v=all&m=discord:{CHANNEL}:{cited}\">"
                         "(07:10 ↩)</a></li></ul>",
            "fresh_ids": [],
            "backend": "fake",
            "model": "fake-model",
            "stamp": {"context_ms": 10, "model_ms": 10, "first_token_ms": None,
                      "total_ms": 20, "input_tokens": None, "output_tokens": None,
                      "input_chars": 100, "output_chars": 10},
            "turns": [],
        }

    def catchup(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json",
                      body=json.dumps(answer()))

    def preview(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "channel": {"id": CHANNEL, "label": "lead team"},
            "thread_id": None, "view": "flat",
            "window": {"count": SEEDED, "unread_count": 1, "read_count": SEEDED - 1,
                       "from_id": target.get("id", ""), "through_id": target.get("id", ""),
                       "truncated": False, "noise_hidden": 0},
            "unread": [{"id": target.get("id", ""), "at_ms": 1760000000000,
                        "label": "07:10 · First"}],
        }))

    def saved_record(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "exists": False, "thread_id": None, "current_summary": None,
            "timeline": [], "staleness": None,
        }))

    def client_config(route: Route) -> None:
        body = call(base, "GET", "/api/v1/client-config", WRITE_TOKEN)
        body["discuss_available"] = True
        route.fulfill(status=200, content_type="application/json",
                      body=json.dumps(body))

    page.route("**/api/v1/client-config", client_config)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/preview*", preview)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/record*", saved_record)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup", catchup)


def to_channel(page: Page) -> None:
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


def open_discuss(page: Page) -> None:
    if page.locator("#discuss-open").is_visible():
        page.click("#discuss-open")
    else:
        page.click("#action-menu-open")
        page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
        page.click("#discuss-menu-open")
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)


def seeded_id(base: str, label: str) -> str:
    """The id of the seeded message whose text is `label`, from the API."""
    body = call(base, "GET", f"/api/v1/channels/{CHANNEL}/messages?limit=100",
                READ_TOKEN)
    found: list[str] = []

    def visit(node: object) -> None:
        if isinstance(node, dict):
            texts = [v for v in node.values() if isinstance(v, str)]
            if any(label in t for t in texts) and isinstance(node.get("id"), str):
                found.append(str(node["id"]))
            for v in node.values():
                visit(v)
        elif isinstance(node, list):
            for v in node:
                visit(v)

    visit(body)
    check(bool(found), f"the seeded message {label!r} is not in the channel")
    return found[0]


def start(page: Page, base: str, target: dict[str, str]) -> None:
    install_routes(page, base, target)
    page.add_init_script(
        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
    page.goto(base + "/", wait_until="load")
    to_channel(page)
    page.wait_for_function(
        "() => document.querySelectorAll('#scroll-area [data-ids]').length >= 20",
        timeout=20_000)


def walk_phone(page: Page, base: str, width: int, height: int) -> None:
    target: dict[str, str] = {"id": seeded_id(base, "Dock check message 05.")}
    start(page, base, target)
    before = page.evaluate(
        "() => document.getElementById('scroll-area').getBoundingClientRect().height")
    open_discuss(page)
    page.wait_for_function(
        "() => document.getElementById('discuss-summarize').textContent === 'Summarize'",
        timeout=20_000)

    # Docked: the panel is in the layout, and the scroller ends at its top.
    geo = page.evaluate("""() => {
      const area = document.getElementById('scroll-area').getBoundingClientRect();
      const panel = document.getElementById('discuss-panel');
      const box = panel.getBoundingClientRect();
      return {areaBottom: area.bottom, areaHeight: area.height, panelTop: box.top,
              panelHeight: box.height, position: getComputedStyle(panel).position};
    }""")
    check(geo["position"] == "relative",
          f"the half-height phone panel is not docked in the layout: {geo}")
    check(abs(geo["areaBottom"] - geo["panelTop"]) <= 2,
          f"the scroller does not end at the panel's top edge: {geo}")
    check(height * 0.4 <= geo["panelHeight"] <= height * 0.6,
          f"the docked panel is not at half height: {geo} in {width}x{height}")
    check(geo["areaHeight"] < before,
          f"opening Discuss did not give the list its own upper area: {before} -> {geo}")

    # `#1552`: drag the "Discuss agent" title bar up: the pane grows, the
    # list shrinks, the choice is kept on the device, and a tap on a bar
    # button is not a drag (the full toggle below still works).
    title = page.locator(".discuss-title").bounding_box()
    check(title is not None, "the Discuss title bar has no box to drag")
    cx, cy = title["x"] + title["width"] / 2, title["y"] + title["height"] / 2
    page.mouse.move(cx, cy)
    page.mouse.down()
    page.mouse.move(cx, cy - 120, steps=12)
    page.mouse.up()
    page.wait_for_function(
        "() => document.getElementById('discuss-panel')"
        ".getBoundingClientRect().height >= innerHeight * 0.6",
        timeout=20_000)
    dragged = page.evaluate("""() => ({
      now: document.getElementById('discuss-head').getAttribute('aria-valuenow'),
      kept: localStorage.getItem('vibe-talk.voice.discuss-docked-height'),
      areaBottom: document.getElementById('scroll-area').getBoundingClientRect().bottom,
      panelTop: document.getElementById('discuss-panel').getBoundingClientRect().top,
    })""")
    check(dragged["now"] is not None and int(dragged["now"]) >= 60,
          f"the title-bar drag did not resize the docked pane: {dragged}")
    check(dragged["kept"] == dragged["now"],
          f"the dragged height was not kept per device: {dragged}")
    check(abs(dragged["areaBottom"] - dragged["panelTop"]) <= 2,
          f"the scroller no longer ends at the dragged pane's top: {dragged}")
    # Keyboard: the focused bar resizes with the arrows.
    page.locator("#discuss-head").focus()
    page.keyboard.press("ArrowDown")
    page.wait_for_function(
        "() => Number(document.getElementById('discuss-head')"
        ".getAttribute('aria-valuenow')) < 60 + 5",
        timeout=20_000)
    # Close/reopen keeps the dragged height.
    kept_now = page.evaluate(
        "() => document.getElementById('discuss-head').getAttribute('aria-valuenow')")
    page.click("#discuss-close")
    page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)
    open_discuss(page)
    page.wait_for_function(
        f"() => document.getElementById('discuss-head')"
        f".getAttribute('aria-valuenow') === '{kept_now}'",
        timeout=20_000)
    import os
    if os.environ.get("DISCUSS_SHOT_DIR"):
        page.screenshot(path=f"{os.environ['DISCUSS_SHOT_DIR']}/discuss-drag-phone.png")

    page.click("#discuss-summarize")
    page.wait_for_selector("#discuss-answer a", state="attached", timeout=20_000)
    page.locator("#discuss-answer .discuss-collapse-bar button").first.click()
    page.wait_for_function(
        "() => [...document.querySelectorAll('#discuss-answer details')]"
        ".every((d) => d.open)",
        timeout=20_000)
    page.wait_for_selector("#discuss-answer a", state="visible", timeout=20_000)

    # (a) The list scrolls to its true bottom inside the visible upper area:
    # at the end the list's foot (the channel composer) stands fully in the
    # upper area, and the last message row can be stood fully there too —
    # neither is under the panel.
    page.evaluate("() => { const a = document.getElementById('scroll-area');"
                  " a.scrollTop = a.scrollHeight; }")
    page.wait_for_function("""() => {
      const area = document.getElementById('scroll-area');
      const box = area.getBoundingClientRect();
      const foot = document.getElementById('channel-composer')
        .getBoundingClientRect();
      return area.scrollTop + area.clientHeight >= area.scrollHeight - 2
        && foot.bottom <= box.bottom + 1.5 && foot.top >= box.top - 1.5;
    }""", timeout=20_000)
    page.wait_for_function("""() => {
      const area = document.getElementById('scroll-area');
      const rows = [...area.querySelectorAll('[data-ids]')]
        .filter((r) => !r.hidden && r.offsetParent !== null);
      const last = rows.reduce((a, b) => (a.offsetTop > b.offsetTop ? a : b));
      last.scrollIntoView({block: 'end'});
      const r = last.getBoundingClientRect();
      const box = area.getBoundingClientRect();
      return r.top >= box.top - 1.5 && r.bottom <= box.bottom + 1.5 && r.height > 0;
    }""", timeout=20_000)

    # (b) A citation tap far up lands its row fully in the visible list area.
    page.click("#discuss-answer a")
    page.wait_for_function("""() => {
      const area = document.getElementById('scroll-area');
      const row = area.querySelector('[data-linked]');
      if (!row) return false;
      const r = row.getBoundingClientRect();
      const box = area.getBoundingClientRect();
      return r.top >= box.top - 1.5 && r.bottom <= box.bottom + 1.5
        && r.height > 0;
    }""", timeout=20_000)
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)

    # A double-click on the bar restores half height.
    page.dblclick(".discuss-title")
    page.wait_for_function(
        "() => document.getElementById('discuss-head')"
        ".getAttribute('aria-valuenow') === '50'",
        timeout=20_000)

    # Full height still covers; back to half docks again; close frees the list.
    page.click("#discuss-full-toggle")
    page.wait_for_function(
        "() => document.getElementById('discuss-panel')"
        ".getBoundingClientRect().height >= innerHeight * 0.75",
        timeout=20_000)
    page.click("#discuss-full-toggle")
    page.wait_for_function(
        "() => document.getElementById('discuss-panel')"
        ".getBoundingClientRect().height <= innerHeight * 0.6",
        timeout=20_000)
    page.click("#discuss-close")
    page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)
    page.wait_for_function(
        f"() => Math.abs(document.getElementById('scroll-area')"
        f".getBoundingClientRect().height - {before}) <= 2",
        timeout=20_000)


def walk_desk(page: Page, base: str, width: int, height: int) -> None:
    target: dict[str, str] = {"id": seeded_id(base, "Dock check message 05.")}
    start(page, base, target)
    before = page.evaluate("""() => {
      const r = document.getElementById('scroll-area').getBoundingClientRect();
      return {top: r.top, height: r.height};
    }""")
    open_discuss(page)
    page.wait_for_function(
        "() => document.getElementById('discuss-summarize').textContent === 'Summarize'",
        timeout=20_000)
    after = page.evaluate("""() => {
      const r = document.getElementById('scroll-area').getBoundingClientRect();
      const panel = document.getElementById('discuss-panel');
      return {top: r.top, height: r.height, position: getComputedStyle(panel).position};
    }""")
    check(after == {**before, "position": "absolute"},
          f"the desk overlay changed: {before} -> {after}")
    page.click("#discuss-summarize")
    page.wait_for_selector("#discuss-answer a", state="attached", timeout=20_000)
    page.locator("#discuss-answer .discuss-collapse-bar button").first.click()
    page.wait_for_function(
        "() => [...document.querySelectorAll('#discuss-answer details')]"
        ".every((d) => d.open)",
        timeout=20_000)
    page.click("#discuss-answer a")
    page.wait_for_selector("#scroll-area [data-linked]", state="attached", timeout=20_000)
    overlap = page.evaluate("""() => {
      const row = document.querySelector('#scroll-area [data-linked]')
        .getBoundingClientRect();
      const panel = document.getElementById('discuss-panel').getBoundingClientRect();
      const x = Math.max(0, Math.min(row.right, panel.right) - Math.max(row.left, panel.left));
      const y = Math.max(0, Math.min(row.bottom, panel.bottom) - Math.max(row.top, panel.top));
      return {row: {top: row.top, bottom: row.bottom},
              panel: {top: panel.top, bottom: panel.bottom}, overlapPx: x * y};
    }""")
    print(f"discuss dock desk note: jumped row vs overlay panel: {overlap}")
    page.click("#discuss-close")
    page.wait_for_selector("#discuss-panel", state="hidden", timeout=20_000)


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="discuss-dock-check-") as tmp:
        work = Path(tmp)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "discuss-dock-check-bot-token-never-sent"\n'
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
            for n in range(1, SEEDED + 1):
                call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                     {"text": f"Dock check message {n:02d}."})

            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                phone = browser.new_context(
                    viewport={"width": 390, "height": 844}, has_touch=True, is_mobile=True)
                walk_phone(phone.new_page(), base, 390, 844)
                desk = browser.new_context(viewport={"width": 1280, "height": 800})
                walk_desk(desk.new_page(), base, 1280, 800)
                browser.close()
            print("discuss dock browser: phone docked and desk overlay walks passed")
            return 0
        finally:
            server.terminate()


if __name__ == "__main__":
    raise SystemExit(main())
