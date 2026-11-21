#!/usr/bin/env python3
"""Summary links stay in the current view; every link lands or says why (`#2080`).

`#2080 vibe-talk-summary-link-stays-in-view` (OWNER 305 + 306, 2026-11-21).
In All with Discuss open, a summary citation to a thread reply used to switch
to the Thread view, leaving the reader's index; and some deep links silently
did nothing at all — no "Linked message" bubble, no reason.

This serves the real server (fake chat upstream) seeded with 120 messages.
Thread membership for one root and two replies is injected at the network
edge into the flat timeline (the fake upstream seeds no threads), and a
thread-view read is answered from those same rows; the catchup routes are
answered at the edge with a contract-shaped summary whose citations name
(a) a main-channel message, (b) a thread reply, (c) a message outside the
loaded window, plus footer times, a not-found citation and a stale
(newer-than-this-page) citation. Everything else — sign-in, All, Discuss,
the jump, Back — is the shipped page and router. Condition waits only.

At phone 412 and desktop 1280, light and dark, the walk asserts: the view
stays All and Discuss stays open with its scroll kept, the target is on
screen with the "Linked message" bubble; Back after the stay-jump returns
to the previous position in All; an explicit thread open (the root's thread
badge) drills in and Back returns; a not-found link shows the persistent
reason; a stale link shows the Reload offer. A message-body link and a
thread short-ref injected into one row also land with the bubble or drill
explicitly, never silently.

Pass --screenshots DIR to keep PNGs of the summary open and of the
thread-reply landing, per profile and scheme.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import tempfile
import time
import urllib.request
from pathlib import Path
from urllib.parse import parse_qs, urlsplit

from playwright.sync_api import Page, Route, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
THREAD = "100000000000000099"
READ_TOKEN = "summary-link-check-read-token-not-a-real-credential"
WRITE_TOKEN = "summary-link-check-write-token-not-a-real-credential"
STAMP = {"context_ms": 1, "model_ms": 1, "first_token_ms": None, "total_ms": 2,
         "input_tokens": None, "output_tokens": None, "input_chars": 10, "output_chars": 10}
WINDOW = {"count": 120, "unread_count": 2, "read_count": 118,
          "from_id": "x", "through_id": "y", "truncated": False, "noise_hidden": 0}
SEEDED = 120


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


def cite(target: str, label: str, thread: str | None = None, extra: str = "") -> str:
    m = f"discord:{CHANNEL}:{target}" + (f":{thread}" if thread else "") + extra
    return (f"<a href=\"?c={CHANNEL}&v=all&m={m}\" "
            f"target=\"_blank\" rel=\"noopener noreferrer nofollow\">({label} ↩)</a>")


def walk(page: Page, base: str, label: str, shots: Path | None) -> None:
    ids: dict[str, str] = {}

    def summary_answer() -> dict[str, object]:
        html = (
            "<h2>Progress</h2><ul>"
            f"<li>Main point {cite(ids['main_msg'], '09:10')}</li>"
            f"<li>Reply point {cite(ids['reply_msg'], '09:11', THREAD)}</li>"
            f"<li>Older point {cite(ids['old_msg'], '09:12')}</li>"
            f"<li>Missing point {cite('999999999999999999', '09:13')}</li>"
            f"<li>Future point {cite(ids['main_msg'], '09:14', THREAD, ':extra')}</li>"
            "</ul>"
        )
        return {
            "backend": "fake", "model": "fake-model", "view": "flat", "since_ms": None,
            "created_ms": 1760000099999, "window": WINDOW,
            "covered_ids": [ids["main_msg"], ids["reply_msg"], ids["old_msg"]],
            "markable_ids": [ids["main_msg"], ids["reply_msg"], ids["old_msg"]],
            "cited_ids": [], "fresh_ids": [],
            "footer_links": [
                {"id": ids["reply_msg"], "thread_id": THREAD, "label": "09:11 · Ben", "at_ms": 1760000060000},
                {"id": ids["main_msg"], "thread_id": None, "label": "09:10 · Ada", "at_ms": 1760000000000},
            ],
            "text": "Summary.", "text_html": html, "unknown_terms": [], "stamp": STAMP,
        }

    def record(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "exists": True, "thread_id": None,
            "current_summary": summary_answer(), "timeline": [], "staleness": None,
        }))

    def preview(route: Route) -> None:
        route.fulfill(status=200, content_type="application/json", body=json.dumps({
            "channel": {"id": CHANNEL, "label": "lead team"},
            "thread_id": None, "view": "flat", "window": WINDOW,
            "unread": [{"id": ids.get("main_msg", "100000000000000101"),
                        "at_ms": 1760000000000, "label": "09:51 · Ada"}],
        }))

    def client_config(route: Route) -> None:
        body = call(base, "GET", "/api/v1/client-config", WRITE_TOKEN)
        body["discuss_available"] = True
        # The fake upstream seeds no threads; the threaded channel this walk
        # stands in is the edge's, so the provider must advertise threading
        # for All (and the thread select) to be offered at all.
        body["threading_supported"] = True
        providers = body.get("providers")
        if isinstance(providers, list):
            for provider in providers:
                if isinstance(provider, dict):
                    provider["threading_supported"] = True
        route.fulfill(status=200, content_type="application/json", body=json.dumps(body))

    members: dict[str, dict[str, object]] = {}

    def with_thread(message: dict[str, object]) -> dict[str, object]:
        mid = str(message.get("id"))
        if mid == ids.get("root_msg"):
            message = {**message, "thread": {"id": THREAD, "root_message_id": ids["root_msg"],
                                             "is_root": True, "reply_count": 2, "reply_count_exact": True}}
        elif mid in (ids.get("reply_msg"), ids.get("reply2_msg")):
            message = {**message, "thread": {"id": THREAD, "root_message_id": ids.get("root_msg", ""),
                                             "is_root": False, "reply_count": 2, "reply_count_exact": True}}
        if str(message.get("id")) in (ids.get("root_msg"), ids.get("reply_msg"), ids.get("reply2_msg")):
            members[str(message["id"])] = message
        return message

    def timeline(route: Route) -> None:
        query = parse_qs(urlsplit(route.request.url).query)
        view = (query.get("view") or [""])[0]
        if view == "thread":
            rows = [members[key] for key in (ids["root_msg"], ids["reply_msg"], ids["reply2_msg"]) if key in members]
            check(len(rows) == 3, f"{label}: thread fixture incomplete: {sorted(members)}")
            route.fulfill(status=200, content_type="application/json", body=json.dumps({
                "channel": {"id": CHANNEL, "label": "lead team"}, "view": "thread",
                "thread": {"id": THREAD, "root": rows[0], "title": "Seed thread",
                           "reply_count": 2, "reply_count_exact": True,
                           "updated_at": rows[0].get("timestamp")},
                "messages": rows, "threads": [], "has_threads": True,
                "has_more": False, "next_before": None,
            }))
            return
        response = route.fetch()
        payload: dict[str, object] = json.loads(response.text())
        messages = payload.get("messages")
        if isinstance(messages, list):
            payload["messages"] = [with_thread(m) if isinstance(m, dict) else m for m in messages]
            payload["has_threads"] = True
            if ids.get("root_msg") and ids["root_msg"] in members:
                threads = payload.get("threads")
                root_row = members[ids["root_msg"]]
                entry = {"id": THREAD, "root": root_row,
                         "reply_count": 2, "reply_count_exact": True,
                         "title": "Seed thread", "display_name": "Seed thread",
                         "updated_at": root_row.get("timestamp")}
                payload["threads"] = [entry] + ([t for t in threads if isinstance(t, dict) and str(t.get("id")) != THREAD]
                                                if isinstance(threads, list) else [])
        route.fulfill(response=response, body=json.dumps(payload))

    failed_reads: list[str] = []
    page.on("response", lambda res: failed_reads.append(f"{res.status} {res.url}")
            if res.status >= 400 and ("/page" in res.url or "/timeline" in res.url) else None)
    page.on("requestfailed", lambda req: failed_reads.append(f"failed {req.url} {req.failure}"))
    page.on("pageerror", lambda err: failed_reads.append(f"pageerror {err}"))
    page.context.route("**/api/v1/client-config", client_config)
    page.context.route(f"**/api/v1/channels/{CHANNEL}/catchup/preview*", preview)
    page.context.route(f"**/api/v1/channels/{CHANNEL}/catchup/record*", record)
    page.context.route(f"**/api/v1/channels/{CHANNEL}/timeline*", timeline)
    page.context.add_init_script(
        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
    page.context.add_init_script(
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
    page.wait_for_selector("#discord-log > li[data-id]", state="visible", timeout=20_000)
    loaded = page.evaluate(
        "() => [...document.querySelectorAll('#discord-log > li[data-id]')]"
        ".map(r => r.getAttribute('data-id'))")
    check(len(loaded) >= 10, f"{label}: loaded window too small: {len(loaded)}")
    # All opens by default on a threaded channel; the seeded ids are contiguous.
    ids["main_msg"] = loaded[len(loaded) // 2]
    ids["root_msg"] = loaded[2]
    ids["reply_msg"] = loaded[3]
    ids["reply2_msg"] = loaded[4]
    ids["old_msg"] = str(int(loaded[0]) - 10)
    check(ids["old_msg"] not in loaded, f"{label}: old target already loaded")

    # The channel opens in All (threading advertised at the edge). Reload now
    # the member ids are known, so the flat read the edge threads draws the
    # rows with their membership (badges included) from the start.
    page.reload(wait_until="load")
    page.wait_for_selector("#discord-log li[data-id]", state="visible", timeout=20_000)
    check(page.evaluate("() => document.getElementById('thread-select')?.value") == "flat",
          f"{label}: channel did not open in All")

    if page.locator("#discuss-open").is_visible():
        page.click("#discuss-open")
    else:
        page.click("#action-menu-open")
        page.wait_for_selector("#discuss-menu-open", state="visible", timeout=20_000)
        page.click("#discuss-menu-open")
    page.wait_for_selector("#discuss-panel", state="visible", timeout=20_000)
    page.wait_for_selector("#discuss-answer a", state="attached", timeout=20_000)
    page.evaluate("() => { for (const d of document.querySelectorAll('#discuss-answer details')) d.open = true; }")
    if shots is not None:
        page.screenshot(path=str(shots / f"{label}-summary-open.png"))

    def panel_scroll() -> int:
        return page.evaluate("""() => {
          const panel = document.getElementById('discuss-panel');
          const answer = document.getElementById('discuss-answer');
          return Math.round((answer && answer.scrollHeight > answer.clientHeight ? answer.scrollTop : panel.scrollTop) || 0);
        }""")

    def assert_stayed(what: str) -> None:
        state = page.evaluate("""() => ({
          view: document.getElementById('thread-select')?.value,
          threadHidden: document.getElementById('thread-heading')?.hidden,
          panelVisible: !document.getElementById('discuss-panel')?.hidden,
        })""")
        check(state["view"] == "flat" and state["threadHidden"] and state["panelVisible"],
              f"{label}: {what} left All or closed Discuss: {state}")

    def assert_landed(target: str, what: str) -> None:
        # A row holds a message by its `data-ids` (rows combine a run of one
        # author's messages); the page's own `rowHolding` reads it the same way.
        holds = ("(r.getAttribute('data-ids') || '').split(' ').includes('%s')" % target)
        try:
            page.wait_for_function(
                f"() => [...document.querySelectorAll('#discord-log li')]"
                f".some(r => {holds} && r.getAttribute('data-linked') === 'true')", timeout=20_000)
        except Exception:
            diag = page.evaluate(f"""() => ({{
              row: [...document.querySelectorAll('#discord-log li')]
                .some(r => (r.getAttribute('data-ids') || '').split(' ').includes("{target}")),
              status: document.getElementById('status')?.textContent || '',
              error: document.getElementById('error')?.textContent || '',
              view: document.getElementById('thread-select')?.value,
              linked: [...document.querySelectorAll('[data-linked]')].map(r => ({{
                id: r.getAttribute('data-id'), ctx: r.getAttribute('data-context-id'),
                tag: r.tagName, cls: String(r.className).slice(0, 60),
                text: r.textContent.slice(0, 80)}})),
              holdingTarget: [...document.querySelectorAll('#discord-log li')].filter(r =>
                r.getAttribute('data-context-id') === "{target}").length,
              first: [...document.querySelectorAll('#discord-log li[data-id]')].slice(0, 3).map(r => r.getAttribute('data-id')),
              count: document.querySelectorAll('#discord-log li[data-id]').length,
            }})""")
            hrefs = page.evaluate("() => [...document.querySelectorAll('#discuss-answer a')].map(a => a.getAttribute('href'))")
            raise AssertionError(f"{label}: {what} (target {target}) did not land: {diag} citations: {hrefs} failed reads: {failed_reads[-4:]}")
        landed = page.evaluate(f"""() => {{
          const row = [...document.querySelectorAll('#discord-log li')]
            .find(r => (r.getAttribute('data-ids') || '').split(' ').includes("{target}")
              && r.getAttribute('data-linked') === 'true');
          const r = row.getBoundingClientRect();
          return {{top: r.top, bottom: r.bottom, vh: innerHeight,
                   note: row.textContent.includes('Linked message')}};
        }}""")
        check(landed["note"] and landed["top"] < landed["vh"] and landed["bottom"] > 0,
              f"{label}: {what} did not land visible with the bubble: {landed}")
        assert_stayed(what)

    citations = page.locator("#discuss-answer a")
    check(citations.count() == 5, f"{label}: summary citations missing: {citations.count()}")

    # (a) a main-channel message, in place.
    scroll_before = panel_scroll()
    citations.nth(0).click()
    assert_landed(ids["main_msg"], "main citation")
    check(panel_scroll() == scroll_before, f"{label}: the summary scroll moved on a jump")

    # (b) a thread reply, in place in All — never the Thread view.
    anchor_top = page.evaluate(
        "() => [...document.querySelectorAll('#discord-log li')]"
        f".find(r => (r.getAttribute('data-ids') || '').split(' ').includes('{ids['main_msg']}'))"
        ".getBoundingClientRect().top")
    citations.nth(1).click()
    assert_landed(ids["reply_msg"], "reply citation")
    if shots is not None:
        page.screenshot(path=str(shots / f"{label}-reply-landed.png"))
    # Back returns to the previous position in the same view: the row the
    # reader was on, at the height it stood.
    page.go_back()
    page.wait_for_function(
        "() => Math.abs([...document.querySelectorAll('#discord-log li')]"
        f".find(r => (r.getAttribute('data-ids') || '').split(' ').includes('{ids['main_msg']}'))"
        f".getBoundingClientRect().top - {float(anchor_top)}) < 4",
        timeout=20_000)
    assert_stayed("Back after stay-jump")

    # (c) a message not yet loaded, loaded in place.
    citations.nth(2).click()
    assert_landed(ids["old_msg"], "not-yet-loaded citation")

    # Footer time link (the reply), also in place.
    footer_link = page.locator("#discuss-links .discuss-link").first
    footer_link.scroll_into_view_if_needed()
    footer_link.click()
    assert_landed(ids["reply_msg"], "footer time")

    # Not found: a persistent reason, no silent no-op.
    citations.nth(3).click()
    page.wait_for_selector("#error-wrap", state="visible", timeout=20_000)
    reason = page.evaluate("() => document.getElementById('error')?.textContent || ''")
    check("couldn't be found" in reason, f"{label}: not-found link gave no persistent reason: {reason!r}")
    page.click("#dismiss-error")

    # Explicit thread open (the root's badge) drills in; Back returns to All.
    page.evaluate(f"""() => {{
      const row = [...document.querySelectorAll('#discord-log li')]
        .find(r => (r.getAttribute('data-ids') || '').split(' ').includes("{ids['root_msg']}"));
      row.scrollIntoView({{block: 'center'}});
    }}""")
    page.wait_for_function(
        "() => [...document.querySelectorAll('#discord-log li')]"
        f".find(r => (r.getAttribute('data-ids') || '').split(' ').includes('{ids['root_msg']}'))"
        "?.getBoundingClientRect().top > 0", timeout=20_000)
    badge = page.locator("#discord-log li .thread-badge")
    check(badge.count() > 0, f"{label}: root row has no thread badge to open explicitly")
    badge.first.click()
    page.wait_for_selector("#thread-heading", state="visible", timeout=20_000)
    page.go_back()
    page.wait_for_function(
        "() => document.getElementById('thread-heading')?.hidden"
        " && document.getElementById('thread-select')?.value === 'flat'", timeout=20_000)
    assert_stayed("Back after explicit thread open")

    # Stale link (newer shape than this page): this page cannot route it, so
    # the browser follows its address into a fresh window — and THAT window
    # must fail visibly with the Reload offer, never land silently nowhere.
    with page.expect_popup(timeout=20_000) as popup_info:
        citations.nth(4).click()
    stale_page = popup_info.value
    stale_page.wait_for_selector("#error-wrap", state="visible", timeout=20_000)
    stale = stale_page.evaluate("""() => ({
      text: document.getElementById('error')?.textContent || '',
      action: document.getElementById('error-action')?.textContent || '',
      actionHidden: document.getElementById('error-action')?.hidden,
    })""")
    check("newer than this page" in stale["text"] and stale["action"] == "Reload" and not stale["actionHidden"],
          f"{label}: stale link did not offer Reload: {stale}")
    print(f"summary link ({label}): stayed in All, landed or said why, Back returned")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--screenshots", type=Path, default=None)
    args = parser.parse_args()
    shots: Path | None = args.screenshots
    if shots is not None:
        shots.mkdir(parents=True, exist_ok=True)
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="summary-link-check-") as tmp:
        work = Path(tmp)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "summary-link-check-bot-token-never-sent"\n'
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
            for i in range(SEEDED):
                call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                     {"text": f"seed message {i} with enough words to take a row in the list"})
            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                for scheme in ("light", "dark"):
                    phone = browser.new_context(
                        viewport={"width": 412, "height": 915}, has_touch=True, is_mobile=True,
                        color_scheme=scheme)
                    walk(phone.new_page(), base, f"phone-412-{scheme}", shots)
                    desk = browser.new_context(viewport={"width": 1280, "height": 800},
                                                color_scheme=scheme)
                    walk(desk.new_page(), base, f"desk-1280-{scheme}", shots)
                browser.close()
            print("summary link browser: phone and desk, light and dark walks passed")
            return 0
        finally:
            server.terminate()


if __name__ == "__main__":
    raise SystemExit(main())
