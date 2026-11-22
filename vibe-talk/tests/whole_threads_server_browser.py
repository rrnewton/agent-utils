#!/usr/bin/env python3
"""Whole threads, no duplicate hits, on a REAL server (`#1864`).

The #1852 retake defect: with Whole threads on, each matching message stood
twice — once greyed in its thread-context position (its real row, archived
dress, reply arrow) and once bright and highlighted as a second row with
the same message id. The second row was a server-search card: the page's
first paint is server-rendered HTML, so the store had not folded those
rows when the server's answer landed, the canon-based freshness check in
`serverFreshHits` called every hit fresh, and `placeServerHits` drew each
on-screen hit again beside its own row.

This walk runs the shipped server binary (`--fake-discord` upstream and a
loopback model stub are the only fakes; every route under test is real),
seeds a thread holding a "quixotic" hit through the real reply route, and
at a 412x915 phone and a 1280x800 desk, in light and dark, asserts with
the server search settled and the Whole-threads toggle off and then on:

  each message id appears exactly once in the visible list, the hit's
  text appears once, no `data-server-hit` row repeats an id a real row
  already stands for, and (toggle on) the hit is marked in place.

Run against a built release binary: `make -C vibe-talk whole-threads-server-browser`.
Pass --screenshots DIR to keep a PNG per stage.
"""

from __future__ import annotations

import argparse
import json
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from typing import TYPE_CHECKING

from playwright.sync_api import sync_playwright
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import Page

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ = "wholedup-check-read-token-not-a-real-credential"
WRITE = "wholedup-check-write-token-not-a-real-credential"

ONCE_JS = """() => {
  const rows = [...document.querySelectorAll('#discord-log > li[data-id]')]
    .filter((li) => li.getClientRects().length);
  const counts = {};
  for (const li of rows) counts[li.dataset.id] = (counts[li.dataset.id] || 0) + 1;
  return {
    dupes: Object.entries(counts).filter(([, n]) => n > 1).map(([id]) => id),
    serverDupes: rows.filter((li) => li.hasAttribute('data-server-hit')
      && rows.some((other) => other !== li && other.dataset.id === li.dataset.id
        && !other.hasAttribute('data-server-hit'))).map((li) => li.dataset.id),
    textOnce: rows.filter((li) => li.textContent.includes('the quixotic plan is ready')).length,
    hitMarked: rows.some((li) => [...li.querySelectorAll('mark.search-hit')]
      .some((m) => m.textContent === 'quixotic')),
    errorHidden: document.getElementById('error-wrap').hidden,
    bodyHas404: document.body.textContent.includes('not served by this check'),
  }; }"""


def free_port() -> int:
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        return int(sock.getsockname()[1])


def call(base: str, method: str, path: str, token: str, body: object = None) -> dict[str, object]:
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(base + path, data=data, method=method)
    req.add_header("Authorization", f"Bearer {token}")
    if data:
        req.add_header("Content-Type", "application/json")
    with urllib.request.urlopen(req, timeout=20) as resp:
        raw = resp.read()
        return json.loads(raw) if raw else {}


def check(condition: bool, why: str) -> None:
    if not condition:
        raise AssertionError(why)
    print(f"ok: {why}", flush=True)


def until(page: Page, script: str, why: str, seconds: float = 30) -> None:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline and not page.evaluate(script):
        page.wait_for_timeout(25)
    check(bool(page.evaluate(script)), why)


def assert_once(page: Page, label: str) -> None:
    state = page.evaluate(ONCE_JS)
    check(state["dupes"] == [], f"{label}: ids shown twice: {state['dupes']}")
    check(state["serverDupes"] == [], f"{label}: server rows repeat on-screen ids: {state['serverDupes']}")
    check(state["textOnce"] == 1, f"{label}: the hit's text stands {state['textOnce']} times")
    check(state["errorHidden"] and not state["bodyHas404"], f"{label}: dirty page {state}")


def walk(page: Page, label: str, shots: Path | None) -> None:
    until(page, "() => !!document.querySelector('#discord-log li')", f"{label}: the channel opened")
    page.click("#search-toggle")
    until(page, "() => !document.getElementById('search-field').hidden", f"{label}: the search field stood")
    page.fill("#search-field", "quixotic")
    until(page, "() => document.getElementById('search-count').textContent.includes('of')",
          f"{label}: the word search was counted")
    # The server's answer settles: its line stops saying it is searching.
    until(page, "() => /Server search details|No more hits/.test(document.getElementById('server-line').textContent)",
          f"{label}: the server search settled")
    assert_once(page, f"{label} toggle off")
    if shots is not None:
        page.screenshot(path=str(shots / f"{label}-1-toggle-off.png"))
    page.click("#search-entire-threads")
    until(page, "() => document.getElementById('search-entire-threads').getAttribute('aria-pressed') === 'true'",
          f"{label}: the Whole-threads toggle pressed on")
    assert_once(page, f"{label} toggle on")
    state = page.evaluate(ONCE_JS)
    check(state["hitMarked"], f"{label} toggle on: the hit is not marked in place")
    if shots is not None:
        page.screenshot(path=str(shots / f"{label}-2-toggle-on.png"))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    binary = ROOT / "target/release/vibe-talk"
    if not binary.exists():
        raise SystemExit(f"no server binary at {binary}: cargo build --release first")
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    port, stub_port = free_port(), free_port()
    base = f"http://127.0.0.1:{port}"
    tmp = Path(tempfile.mkdtemp(prefix="wholedup-server-"))
    tmp.chmod(0o700)
    (tmp / "model-key").write_text("wholedup-check-model-key-not-a-real-credential")
    (tmp / "vibe-talk.toml").write_text(
        f'[server]\nbind = "127.0.0.1:{port}"\n'
        '[discord]\nbot_token = "wholedup-check-never-sent"\ndefault_fetch_limit = 6\nmax_fetch_limit = 6\n'
        f'[auth]\nread_token = "{READ}"\nwrite_token = "{WRITE}"\n'
        f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
        f'[storage]\npath = "{tmp}/state/store.sqlite3"\n'
        f'[message_cache]\nenabled = true\nserve = true\nbackfill = "full"\npath = "{tmp}/state/cache.sqlite3"\n'
        f'[summaries.llm]\nbase_url = "http://127.0.0.1:{stub_port}"\nmodel = "model-stub"\napi_key_file = "{tmp}/model-key"\n')
    stub = subprocess.Popen([sys.executable, str(ROOT / "scripts/model_stub.py"), "--port", str(stub_port)],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    server = subprocess.Popen([str(binary), "--config", str(tmp / "vibe-talk.toml"),
                               "--fake-discord", "--skip-startup-probe"],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    try:
        deadline = time.time() + 40
        while True:
            try:
                call(base, "GET", "/healthz", READ)
                break
            except Exception:
                check(time.time() < deadline, "the server did not come up")
                time.sleep(0.25)
        call(base, "GET", f"/api/v1/channels/{CHANNEL}/messages?limit=6", WRITE)
        posted = call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE,
                      {"text": "Alpha thread root about deployment."})["posted"]
        root = posted["id"] if isinstance(posted, dict) else ""
        call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE,
             {"text": "the quixotic plan is ready", "reply_to": root})
        call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE,
             {"text": "ordinary follow up in alpha.", "reply_to": root})
        call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE,
             {"text": "Beta note: quixotic checklist for the release."})
        call(base, "POST", f"/api/v1/channels/{CHANNEL}/history/prioritize", WRITE)
        with sync_playwright() as playwright:
            browser = playwright.chromium.launch()
            for theme in ("light", "dark"):
                for label, width, height, scale in (("phone", 412, 915, 2.625), ("desktop", 1280, 800, 1.0)):
                    context = browser.new_context(
                        viewport={"width": width, "height": height}, device_scale_factor=scale,
                        color_scheme=theme, has_touch=width < 600, is_mobile=width < 600)
                    browser_waits.hot_host_timeouts(context)
                    page = context.new_page()
                    browser_waits.hot_host_timeouts(page)
                    page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE)}); "
                        "localStorage.setItem('vibe-talk.welcome.dismissed','1'); } catch(e) {}")
                    page.goto(base + "/")
                    walk(page, f"{theme}-{label}", args.screenshots)
                    context.close()
            browser.close()
        print("whole threads server browser: all walks passed")
    finally:
        server.kill()
        stub.kill()


if __name__ == "__main__":
    main()
