#!/usr/bin/env python3
"""A Discuss citation jumps in the same window, in real Chromium (`#1911`).

`#1911 vibe-talk-discuss-message-link-opens-new-window`. The owner, 2026-11-19:
message links inside the Discuss pane "used to jump to the message INSIDE THE
SAME window... now when I click the ↩ link it consistently opens a NEW vibe
talk window". Every server-rendered anchor carries `target="_blank"` (the
renderer's stamp on all links), so any Discuss citation tap the panel does not
route in-app — the timeline's earlier summaries and follow-up turns had no
routing at all — opens a second window, and a far target's seek gave up
without ever showing the linked message.

This serves the real server (fake chat upstream) with 120 seeded messages, so
the loaded window holds only the newest page and both citations below name
messages OUTSIDE it. The catchup and record routes are answered at the network
edge with contract-shaped bodies (the model is nobody's to run in a walk), as
in tests/discuss_browser.py; everything else — sign-in, the channel, the
panel, the jump — is the shipped page. At 412x915 and 1280x800:

  the saved summary draws with its (09:12 ↩) citation carrying no
  `target=_blank` -> tapped: no new page opens, the reader lands in the same
  window with the cited message on screen and marked "Linked message" -> the
  timeline's earlier-summary citation, also without the stamp, tapped from
  inside its collapsed summary: the same same-window landing on its message.

The timeline target stands two pages back, and its walk is first answered as
the server's copy answers a slow step back since `#1886`: six empty pages at
the same cursor while the background fill runs. A walk that counted those as
pages would give up before the real page arrives (`#1911`); this one must
wait them out and still land, fast, visible and marked.

Condition waits only (owner rule 346). Pass --screenshots DIR to keep PNGs.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import Page, Route, sync_playwright
import browser_waits

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "discuss-link-check-read-token-not-a-real-credential"
WRITE_TOKEN = "discuss-link-check-write-token-not-a-real-credential"
STAMP = {"context_ms": 1, "model_ms": 1, "first_token_ms": None, "total_ms": 2,
         "input_tokens": None, "output_tokens": None, "input_chars": 10, "output_chars": 10}
WINDOW = {"count": 120, "unread_count": 2, "read_count": 118,
          "from_id": "x", "through_id": "y", "truncated": False, "noise_hidden": 0}


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def free_port() -> int:
    import socket
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


def citation(target: str, label: str) -> str:
    """One citation anchor as the real renderer stamps it (`target=_blank` included)."""
    return (f"<a href=\"?c={CHANNEL}&v=all&m=discord:{CHANNEL}:{target}\" "
            f"target=\"_blank\" rel=\"noopener noreferrer nofollow\">({label} ↩)</a>")


def saved_answer(text_html: str) -> dict[str, object]:
    return {
        "backend": "fake", "model": "fake-model", "view": "flat", "since_ms": None,
        "created_ms": 1760000099999, "window": WINDOW,
        "covered_ids": [], "markable_ids": [], "cited_ids": [], "fresh_ids": [],
        "footer_links": [], "text": "Old news landed.", "text_html": text_html,
        "unknown_terms": [], "stamp": STAMP,
    }


def walk(page: Page, base: str, label: str, shots: Path | None) -> None:
    targets: dict[str, str] = {}

    def record(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "exists": True, "thread_id": None,
            "current_summary": saved_answer(
                "<h2>Progress</h2><ul><li>Old news landed "
                + citation(targets["answer"], "09:12") + "</li></ul>"),
            "timeline": [{
                "kind": "summary",
                "summary": saved_answer(
                    "<p>Earlier point " + citation(targets["timeline"], "09:11") + "</p>"),
            }],
            "staleness": None,
        }))

    def preview(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "channel": {"id": CHANNEL, "label": "lead team"},
            "thread_id": None, "view": "flat", "window": WINDOW,
            "unread": [
                {"id": "100000000000000101", "at_ms": 1760000000000, "label": "09:51 · Ada"},
                {"id": "100000000000000102", "at_ms": 1760000060000, "label": "09:52 · Ben"},
            ],
        }))

    def client_config(route: Route) -> None:
        body = call(base, "GET", "/api/v1/client-config", WRITE_TOKEN)
        body["discuss_available"] = True
        route.fulfill(status=200, content_type="application/json", body=json.dumps(body))

    # Older reads the walk makes, answered at first as the server's copy
    # answers a slow step back (`#1886`): empty, more above, same cursor.
    stalls = {"left": 0}

    def older_page(route: Route) -> None:
        from urllib.parse import parse_qs, urlsplit
        before = (parse_qs(urlsplit(route.request.url).query).get("before") or [""])[0]
        if before and stalls["left"] > 0:
            stalls["left"] -= 1
            route.fulfill(status=200, content_type="application/json", body=json.dumps({
                "channel": {"id": CHANNEL, "label": "lead team"},
                "messages": [], "has_more": True, "next_before": before,
            }))
            return
        route.continue_()

    popups: list[str] = []
    page.context.on("page", lambda new_page: popups.append(new_page.url))
    page.route(f"**/api/v1/channels/{CHANNEL}/page*", older_page)
    page.route("**/api/v1/client-config", client_config)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/preview*", preview)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/record*", record)
    page.add_init_script(
        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
    page.add_init_script(
        "try { localStorage.setItem('vibe-talk.welcome.dismissed', '1'); } catch (e) {}")
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
    page.wait_for_selector("#discord-log > li[data-id]", state="visible", timeout=20_000)
    loaded = page.evaluate(
        "() => [...document.querySelectorAll('#discord-log > li[data-id]')]"
        ".map(r => r.getAttribute('data-id'))")
    check(len(loaded) >= 10, f"{label}: the loaded window is too small to walk from: {len(loaded)}")
    # Both targets stand outside the loaded window (one and two pages back);
    # the fake upstream's ids are contiguous, as its seeded rows show.
    targets["answer"] = str(int(loaded[0]) - 10)
    targets["timeline"] = str(int(loaded[0]) - 70)
    check(targets["answer"] not in loaded and targets["timeline"] not in loaded,
          f"{label}: fixture targets are already loaded: {targets} in {loaded[:3]}…")

    if page.locator("#discuss-open").is_visible():
        page.click("#discuss-open")
    else:
        page.click("#action-menu-open")
        page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
        page.click("#discuss-menu-open")
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
    page.wait_for_selector("#discuss-answer a", state="attached", timeout=20_000)
    page.wait_for_selector("#discuss-timeline a", state="attached", timeout=20_000)

    stamps = page.evaluate("""() => [...document.querySelectorAll('#discuss-answer a, #discuss-timeline a')]
      .map(a => a.getAttribute('target'))""")
    check(all(stamp is None for stamp in stamps),
          f"{label}: a drawn Discuss citation still opens a new window: {stamps}")
    page.evaluate("() => { for (const d of document.querySelectorAll('#discuss-answer details, #discuss-timeline details')) d.open = true; }")
    if shots is not None:
        page.screenshot(path=str(shots / f"{label}-discuss.png"))

    def click_and_land(selector: str, target: str, what: str) -> None:
        url_before = page.url
        page.click(selector)
        page.wait_for_function(
            f"() => document.querySelector('#discord-log > li[data-id=\"{target}\"]') !== null"
            f" && document.querySelector('#discord-log > li[data-id=\"{target}\"]')"
            ".getAttribute('data-linked') === 'true'",
            timeout=20_000)
        landed = page.evaluate(f"""() => {{
          const row = document.querySelector('#discord-log > li[data-id="{target}"]');
          const r = row.getBoundingClientRect();
          return {{top: r.top, bottom: r.bottom, vh: innerHeight,
                   note: row.textContent.includes('Linked message'),
                   status: document.getElementById('status')?.textContent || ''}};
        }}""")
        check(not popups, f"{label}: {what} opened a new window: {popups}")
        check(landed["note"] and landed["top"] < landed["vh"] and landed["bottom"] > 0,
              f"{label}: {what} did not land visible and marked: {landed}")
        check("linked message" in landed["status"].lower(),
              f"{label}: {what} landing was not said: {landed['status']!r} (from {url_before})")
        if shots is not None:
            page.screenshot(path=str(shots / f"{label}-{what}.png"))

    click_and_land("#discuss-answer a", targets["answer"], "answer-citation")
    # The timeline walk first meets six server-copy stalls (see the head of
    # this file): it must wait them out, not spend its pages on them.
    stalls["left"] = 6
    click_and_land("#discuss-timeline a", targets["timeline"], "timeline-citation")
    check(stalls["left"] == 0, f"{label}: the stall fixture was never exercised: {stalls}")
    print(f"discuss link ({label}): same-window landings passed, no new window")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--screenshots", type=Path, default=None)
    args = parser.parse_args()
    shots: Path | None = args.screenshots
    if shots is not None:
        shots.mkdir(parents=True, exist_ok=True)
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="discuss-link-check-") as tmp:
        work = Path(tmp)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "discuss-link-check-bot-token-never-sent"\n'
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
            for i in range(120):
                call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                     {"text": f"seed message {i} with enough words to take a row in the list"})
            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                phone = browser.new_context(
                    viewport={"width": 412, "height": 915}, has_touch=True, is_mobile=True,
                    color_scheme="dark")
                browser_waits.hot_host_timeouts(phone)
                walk(phone.new_page(), base, "phone-412", shots)
                desk = browser.new_context(viewport={"width": 1280, "height": 800},
                                            color_scheme="dark")
                browser_waits.hot_host_timeouts(desk)
                walk(desk.new_page(), base, "desk-1280", shots)
                browser.close()
            print("discuss link browser: phone (412x915) and desk (1280x800) walks passed")
            return 0
        finally:
            server.terminate()


if __name__ == "__main__":
    raise SystemExit(main())
