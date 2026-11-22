#!/usr/bin/env python3
"""New summary (`#2073 vibe-talk-new-summary-button`) in a real Chromium.

The catch-up routes are answered at the network edge with contract-shaped
bodies (a model stub — never a real LLM), over the real server and the
shipped page. One walk per size (phone 412, desk 1280): the saved first
summary stands with its staleness banner carrying Update and New summary
side by side; New summary stands a fresh card over only the newer range
on top and keeps the first in the timeline as an earlier summary naming
the range it covers; a reload shows the same two; then Update still
merges in place and adds no timeline entry. Condition waits only — no
fixed sleeps in any assertion.
"""

import json
import socket
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import Page, Route, sync_playwright
import browser_waits

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "newsummary-check-read-token-not-a-real-credential"
WRITE_TOKEN = "newsummary-check-write-token-not-a-real-credential"
OLD_IDS = ["100000000000000101", "100000000000000102"]
NEW_ID = "100000000000000103"
STAMP = {"context_ms": 10, "model_ms": 20, "first_token_ms": None,
         "total_ms": 30, "input_tokens": None, "output_tokens": None,
         "input_chars": 100, "output_chars": 20, "unknown_term_count": 0}

OLD_SUMMARY: dict[str, object] = {
    "backend": "fake", "model": "fake-model", "view": "flat",
    "since_ms": None, "created_ms": 1760000000000,
    "window": {"count": 2, "unread_count": 2, "read_count": 0,
               "from_id": OLD_IDS[0], "through_id": OLD_IDS[1],
               "truncated": False, "noise_hidden": 0},
    "covered_ids": OLD_IDS, "markable_ids": OLD_IDS, "cited_ids": [],
    "fresh_ids": [],
    "footer_links": [
        {"id": OLD_IDS[0], "label": "09:51 · Ada", "thread_id": None},
        {"id": OLD_IDS[1], "label": "09:52 · Ben", "thread_id": None},
    ],
    "text": "## Progress\nAda shipped the fix. Ben is reviewing it.",
    "text_html": "<h2>Progress</h2><p>Ada shipped the fix. Ben is reviewing it.</p>",
    "unknown_terms": [], "stamp": STAMP,
}
FRESH_SUMMARY: dict[str, object] = {
    **OLD_SUMMARY,
    "created_ms": 1760000060000,
    "window": {"count": 1, "unread_count": 1, "read_count": 0,
               "from_id": NEW_ID, "through_id": NEW_ID,
               "truncated": False, "noise_hidden": 0},
    "covered_ids": [NEW_ID], "markable_ids": [NEW_ID],
    "footer_links": [{"id": NEW_ID, "label": "09:53 · Cy", "thread_id": None}],
    "text": "## Deploy\nCy deployed the fix.",
    "text_html": "<h2>Deploy</h2><p>Cy deployed the fix.</p>",
}
ANSWER_BASE = {
    "channel": {"id": CHANNEL, "label": "lead team"},
    "thread_id": None, "view": "flat", "backend": "fake",
    "model": "fake-model", "stamp": STAMP,
}


def as_answer(summary: dict[str, object]) -> dict[str, object]:
    return {
        **ANSWER_BASE,
        "window": summary["window"],
        "unread_ids": summary["covered_ids"],
        "markable_ids": summary["markable_ids"],
        "footer_links": summary["footer_links"],
        "cited_ids": [], "fresh_ids": [],
        "text": summary["text"], "text_html": summary["text_html"],
        "unknown_terms": [],
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
        parsed = json.loads(res.read() or b"null")
    assert isinstance(parsed, dict)
    return parsed


def walk(page: Page, base: str, width: int) -> None:
    state = {"fresh": False, "stale_again": False, "updated": False,
             "new_calls": 0, "update_calls": 0}
    merged_summary: dict[str, object] = {
        **FRESH_SUMMARY,
        "text": str(FRESH_SUMMARY["text"]) + "\n\n## Later\nDi confirmed it.",
        "text_html": str(FRESH_SUMMARY["text_html"]) + "<h2>Later</h2><p>Di confirmed it.</p>",
    }

    def record(route: Route) -> None:
        body: dict[str, object]
        if not state["fresh"]:
            body = {"exists": True, "thread_id": None,
                    "current_summary": OLD_SUMMARY, "timeline": [],
                    "staleness": {"new_unread": 1, "marked_read": 0}}
        else:
            body = {"exists": True, "thread_id": None,
                    "current_summary": merged_summary if state["updated"] else FRESH_SUMMARY,
                    "timeline": [{"kind": "summary", "summary": OLD_SUMMARY}],
                    "staleness": ({"new_unread": 1, "marked_read": 0}
                                  if state["stale_again"] and not state["updated"] else None)}
        route.fulfill(status=200, content_type="application/json",
                      body=json.dumps(body))

    def preview(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "channel": {"id": CHANNEL, "label": "lead team"},
            "thread_id": None, "view": "flat",
            "window": {"count": 3, "unread_count": 1, "read_count": 2,
                       "from_id": OLD_IDS[0], "through_id": NEW_ID,
                       "truncated": False, "noise_hidden": 0},
            "unread": [{"id": NEW_ID, "at_ms": 1760000060000,
                        "label": "09:53 · Cy"}],
        }))

    def new_summary(route: Route) -> None:
        state["new_calls"] += 1
        state["fresh"] = True
        route.fulfill(status=200, content_type="application/json",
                      body=json.dumps(as_answer(FRESH_SUMMARY)))

    def update(route: Route) -> None:
        state["update_calls"] += 1
        state["updated"] = True
        route.fulfill(status=200, content_type="application/json",
                      body=json.dumps(as_answer(merged_summary)))

    def client_config(route: Route) -> None:
        body = call(base, "GET", "/api/v1/client-config", WRITE_TOKEN)
        body["discuss_available"] = True
        route.fulfill(status=200, content_type="application/json",
                      body=json.dumps(body))

    page.route("**/api/v1/client-config", client_config)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/preview*", preview)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/record*", record)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/new-summary", new_summary)
    page.route(f"**/api/v1/channels/{CHANNEL}/catchup/update", update)
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
    if page.locator("#discuss-open").is_visible():
        page.click("#discuss-open")
    else:
        page.click("#action-menu-open")
        page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
        page.click("#discuss-menu-open")
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
    # Before: the first summary stands; both banner buttons stand together.
    page.wait_for_function(
        "() => document.getElementById('discuss-answer')?.textContent.includes('Ada shipped')",
        timeout=20_000)
    page.wait_for_selector("#discuss-stale", state="visible", timeout=20_000)
    check(page.locator("#discuss-new-summary").is_visible(),
          "New summary does not stand next to Update")
    check(page.locator("#discuss-new-summary").text_content() == "New summary",
          "the second banner button is not New summary")
    check(page.locator("#discuss-update").is_visible(), "Update left the banner")
    # New summary: a fresh card over the newer range; the first is kept.
    page.click("#discuss-new-summary")
    page.wait_for_function(
        "() => document.getElementById('discuss-answer')?.textContent.includes('Cy deployed')",
        timeout=20_000)
    check(state["new_calls"] == 1, "New summary did not make exactly one call")
    page.wait_for_function(
        "() => document.getElementById('discuss-timeline')?.textContent.includes('Earlier summary')",
        timeout=20_000)
    timeline_text = page.locator("#discuss-timeline").text_content() or ""
    check("covers 09:51–09:52" in timeline_text,
          f"the earlier summary does not name its range: {timeline_text!r}")
    check("Ada shipped" in timeline_text, "the first summary left the timeline")
    check(page.locator("#discuss-answer").text_content() is not None
          and "Ada shipped" not in (page.locator("#discuss-answer").text_content() or ""),
          "the fresh card still carries the first summary")
    # Reload persistence: the same two summaries stand.
    page.reload(wait_until="load")
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
    page.wait_for_function(
        "() => document.getElementById('discuss-answer')?.textContent.includes('Cy deployed')"
        " && document.getElementById('discuss-timeline')?.textContent.includes('Earlier summary')",
        timeout=20_000)
    # Update still updates in place: merged text, no new timeline entry.
    state["stale_again"] = True
    page.reload(wait_until="load")
    page.wait_for_selector("#discuss-stale", state="visible", timeout=20_000)
    entries_before = page.locator("#discuss-timeline .discuss-old").count()
    page.click("#discuss-update")
    page.wait_for_function(
        "() => document.getElementById('discuss-answer')?.textContent.includes('Di confirmed')",
        timeout=20_000)
    check(state["update_calls"] == 1, "Update did not make exactly one call")
    check(page.locator("#discuss-timeline .discuss-old").count() == entries_before,
          "Update added a timeline entry instead of merging in place")
    print(f"new-summary browser: width {width} walk passed")


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="newsummary-check-") as tmp:
        work = Path(tmp)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "newsummary-check-bot-token-never-sent"\n'
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
                    viewport={"width": 412, "height": 915}, has_touch=True,
                    is_mobile=True, color_scheme="dark")
                browser_waits.hot_host_timeouts(phone)
                walk(phone.new_page(), base, 412)
                desk = browser.new_context(
                    viewport={"width": 1280, "height": 800}, color_scheme="light")
                browser_waits.hot_host_timeouts(desk)
                walk(desk.new_page(), base, 1280)
                browser.close()
            print("new-summary browser: phone 412 and desk 1280 walks passed")
            return 0
        finally:
            server.terminate()


if __name__ == "__main__":
    raise SystemExit(main())
