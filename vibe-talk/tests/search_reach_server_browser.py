#!/usr/bin/env python3
"""No "does not reach yet" beside the server's own hits, on a REAL server (`#2269`).

`#2269 vibe-talk-search-reach-sentence-beside-server-hits`. On a server that holds the channel's
whole history (`[message_cache] backfill = "full"`, the history complete), a search for a word
only an older message says found it through the server's search -- its chip counted it and its
card stood -- while the None-match line still ended "This server holds the complete history of
this channel, which this search does not reach yet." The sentence is true only of the browser's
own search; once the server's search has answered over the whole history it goes.

This walk runs the shipped server binary (`--fake-discord` is the only fake; every route under
test is real). It posts an old message saying "zygote" and then enough plain ones that the page's
first read never loads it; asks the server to gather the channel's history and waits until the
server says that history is complete; then, in All, at 1280x800 and at 412x915 by touch, light
and dark, searches "zygote" and asserts, with the server's answer settled:

  the "1 in main" chip counts the older hit, its card stands in the list, and the None-match
  line still speaks for the loaded messages ("None match.") but says no reach sentence.

Main before the change is the negative control: run with --binary pointing at its build and the
line still says "does not reach yet" beside the hit. Geometry and text only, no wall-clock budget.
Run against a built release binary (cargo build --release first). Pass --screenshots DIR to keep
a PNG per theme and size.
"""

from __future__ import annotations

import argparse
import itertools
import json
import socket
import subprocess
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
READ = "reachcheck-read-token-not-a-real-credential"
WRITE = "reachcheck-write-token-not-a-real-credential"
WORD = "zygote"
PLAIN = 40  # well past what a first read of the channel loads, so the old message is never loaded
REACH = "does not reach yet"

STATE_JS = """() => {
  const visible = (id) => { const e = document.getElementById(id); return !!e && !e.hidden && e.getClientRects().length > 0; };
  const main = document.getElementById('search-bucket-main');
  const cards = [...document.querySelectorAll('#discord-log > li[data-server-hit]')]
    .filter((li) => li.getClientRects().length && li.textContent.includes('zygote'));
  return {
    line: document.getElementById('server-line').textContent,
    empty: visible('search-empty') ? document.getElementById('search-empty').textContent.trim() : null,
    chip: main && !main.hidden ? main.textContent.trim() : null,
    chipPressed: main ? main.getAttribute('aria-pressed') : null,
    cards: cards.length,
    count: document.getElementById('search-count').textContent,
    error: !document.getElementById('error-wrap').hidden,
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
        loaded = json.loads(raw) if raw else {}
        return loaded if isinstance(loaded, dict) else {}


def check(condition: bool, why: str) -> None:
    if not condition:
        raise AssertionError(why)
    print(f"ok: {why}", flush=True)


def until(page: Page, script: str, why: str, seconds: float = 60) -> None:
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline and not page.evaluate(script):
        page.wait_for_timeout(50)
    check(bool(page.evaluate(script)), why)


def history_complete(base: str) -> bool:
    """The server says it holds the channel's whole history (its history walk reached the start)."""
    history = call(base, "GET", f"/api/v1/channels/{CHANNEL}/history", READ)
    return history.get("complete") is True


def walk(page: Page, label: str, touch: bool, shots: Path | None) -> str:
    until(page, "() => document.querySelectorAll('#discord-log > li[data-id]').length > 0", f"{label}: the channel opened")
    # All: the channel's every message in order. On a channel with no threads (the fake upstream's)
    # that is its one list and the picker offers nothing else.
    offers_all = page.evaluate("() => [...document.getElementById('thread-select').options].some((o) => o.value === 'flat')")
    if offers_all:
        # Chosen as a pick does (the select's value, then its change), as the screenshot walk does.
        page.evaluate("() => { const s = document.getElementById('thread-select'); if (s.value !== 'flat') {"
                      " s.value = 'flat'; s.dispatchEvent(new Event('change', { bubbles: true })); } }")
        until(page, "() => document.getElementById('thread-select').value === 'flat'", f"{label}: All is the view")
    if touch:
        box = page.locator("#search-toggle").bounding_box()
        check(box is not None, f"{label}: the search glass is on the screen")
        if box is not None:
            page.touchscreen.tap(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
    else:
        page.click("#search-toggle")
    until(page, "() => !document.getElementById('search-field').hidden", f"{label}: the search field stood")
    page.fill("#search-field", WORD)
    # The server's answer settles: its line stops saying it is searching.
    until(page, "() => /Server search details|No more hits/.test(document.getElementById('server-line').textContent)",
          f"{label}: the server search settled")
    until(page, "() => { const m = document.getElementById('search-bucket-main');"
                " return !!m && !m.hidden && /^1 in main$/.test(m.textContent.trim()); }",
          f"{label}: the server's older hit is counted on the main chip")
    if shots is not None:
        page.screenshot(path=str(shots / f"{label}-search-reach.png"))
    until(page, "() => !document.getElementById('search-empty').hidden", f"{label}: the None-match line is up")
    state = page.evaluate(STATE_JS)
    check(not state["error"], f"{label}: no error banner ({state})")
    check(state["cards"] == 1, f"{label}: the older hit's card stands once ({state})")
    check(state["empty"] is not None and "None match." in state["empty"],
          f"{label}: the line still speaks for the loaded messages ({state['empty']!r})")
    check(state["empty"] is not None and REACH not in state["empty"],
          f"{label}: no reach sentence beside the server's hit ({state['empty']!r})")
    return f"{label}: chip {state['chip']!r} pressed={state['chipPressed']}, count {state['count']!r}, line {state['empty']!r}"


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/vibe-talk",
                        help="the server binary to run (default: this checkout's release build)")
    parser.add_argument("--screenshots", type=Path)
    args = parser.parse_args()
    binary: Path = args.binary
    if not binary.exists():
        raise SystemExit(f"no server binary at {binary}: cargo build --release first")
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="reachcheck-server-") as name:
        tmp = Path(name)
        tmp.chmod(0o700)
        (tmp / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "reachcheck-never-sent"\ndefault_fetch_limit = 6\nmax_fetch_limit = 6\n'
            f'[auth]\nread_token = "{READ}"\nwrite_token = "{WRITE}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{tmp}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\nserve = true\nbackfill = "full"\npath = "{tmp}/cache.sqlite3"\n')
        server = subprocess.Popen([str(binary), "--config", str(tmp / "vibe-talk.toml"),
                                   "--fake-discord", "--skip-startup-probe"],
                                  cwd=tmp, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        report: list[str] = []
        failed: list[str] = []
        try:
            deadline = time.monotonic() + 60
            while True:
                try:
                    call(base, "GET", "/healthz", READ)
                    break
                except Exception:
                    check(time.monotonic() < deadline, "the server came up")
                    time.sleep(0.25)
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE,
                 {"text": f"an old {WORD} note, long before the rest"})
            for n in range(PLAIN):
                call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE, {"text": f"plain message number {n + 1}"})
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/history/prioritize", WRITE)
            # A condition, not a clock: the server says the channel's whole history is held.
            deadline = time.monotonic() + 120
            whole = history_complete(base)
            while not whole and time.monotonic() < deadline:
                time.sleep(0.5)
                whole = history_complete(base)
            check(whole, "the server holds the channel's whole history")
            with sync_playwright() as playwright:
                browser = playwright.chromium.launch()
                sizes = (("desk", 1280, 800, 1.0, False), ("phone", 412, 915, 2.625, True))
                for dark, (size, width, height, scale, touch) in itertools.product((False, True), sizes):
                    context = browser.new_context(
                        viewport={"width": width, "height": height}, device_scale_factor=scale,
                        color_scheme="dark" if dark else "light", has_touch=touch, is_mobile=touch)
                    label = f"{'dark' if dark else 'light'}-{size}"
                    browser_waits.hot_host_timeouts(context)
                    page = context.new_page()
                    browser_waits.hot_host_timeouts(page)
                    page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE)}); "
                        "localStorage.setItem('vibe-talk.welcome.dismissed','1'); } catch(e) {}")
                    page.goto(base + "/")
                    # Every cell is walked and said, a red one included, so the negative control
                    # (main before the change) shows each cell it fails in, not only the first.
                    try:
                        report.append(walk(page, label, touch, args.screenshots))
                    except AssertionError as error:
                        failed.append(f"{label}: FAIL {error}")
                    context.close()
                browser.close()
        finally:
            server.terminate()
            server.wait(timeout=30)
    for line in report + failed:
        print(line)
    if failed:
        return 1
    print("search reach server browser: all walks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
