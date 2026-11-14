"""The bottom controls bar after pressing `New version — reload` (`#1515
page-update-reload-loses-controls`), against the REAL server.

The owner, 2026-11-14: "I pressed the chip that said new version reload
and it reloaded but with the controls bar at the bottom missing!" The
chip (`#page-update`, `#529`) reloads with `location.reload()`. A
document read cannot survive that reload, and since `#1505` the control
pane in Documents stood only while such a read stood — the reader
reopened from its deep link with no read standing and no bar, and
nothing re-showed it. The pane is the reader's transport in every
reader state now.

At 393x852 (touch) and 1280x800 (mouse): land on the call view, switch
to the channel, press the chip, and assert the dock and the bar inside
it are on screen; then open a document, reload at its deep link, and
assert the reopened reader's control pane stands with no read playing;
the Documents list keeps no pane. The fake chat backend's `/page` reads
carry no `page_build`, so the chip is shown as a timeline read would
show it and then really pressed — build detection itself is the page
suite's (`#529` in tests/js/page/shell.mjs). No wall-clock waits: every
step waits on a selector or a page condition.
"""

import json
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path

from playwright.sync_api import Page, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "page-update-reload-read-token-not-a-real-credential"
WRITE_TOKEN = "page-update-reload-write-token-not-a-real-credential"
PASTE_BODY = (
    "# Demo report\n\nThe nightly run finished green.\n\n"
    + "A filler paragraph long enough to take a few lines on a phone.\n\n" * 10
)

MEASURE = """() => {
  const out = {};
  for (const id of ["dock", "control-pane", "control-bar"]) {
    const el = document.getElementById(id);
    if (!el) { out[id] = null; continue; }
    const r = el.getBoundingClientRect();
    out[id] = {
      hidden: el.hidden, display: getComputedStyle(el).display,
      inView: r.bottom > 0 && r.top < innerHeight && r.height > 0,
    };
  }
  return out;
}"""


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def free_port() -> int:
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = sock.getsockname()[1]
    sock.close()
    return port


def call(base: str, method: str, path: str, token: str, body: dict | None = None) -> dict:
    req = urllib.request.Request(
        base + path, method=method,
        data=json.dumps(body).encode() if body is not None else None,
        headers={"Authorization": f"Bearer {token}", "Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=10) as resp:
        return json.loads(resp.read().decode() or "{}")


def assert_bar(page: Page, label: str, pane: bool) -> None:
    bars = page.evaluate(MEASURE)
    check(bars["dock"]["inView"] and bars["dock"]["display"] != "none",
          f"{label}: the dock is not on screen: {bars}")
    check(bars["control-bar"]["inView"] and not bars["control-bar"]["hidden"],
          f"{label}: the bottom controls bar is not on screen: {bars}")
    check(bars["control-pane"]["inView"] != (not pane) and bars["control-pane"]["hidden"] != pane,
          f"{label}: the control pane should be {'standing' if pane else 'away'}: {bars}")


def walk(page: Page, base: str, label: str, doc_key: str) -> None:
    page.goto(base + "/", wait_until="load")
    page.wait_for_selector("#screen-main:not([hidden])", timeout=20_000)
    assert_bar(page, f"{label} call view", pane=True)
    page.click("#view-switch")
    page.wait_for_selector("#pane-discord:not([hidden])", timeout=20_000)
    assert_bar(page, f"{label} channel before the chip", pane=True)
    # Shown as a timeline read would show it (see the docstring), then
    # really pressed: this is the owner's reload, `location.reload()`.
    page.evaluate("() => { document.getElementById('page-update').hidden = false; }")
    page.wait_for_selector("#page-update:not([hidden])", timeout=20_000)
    page.click("#page-update")
    page.wait_for_selector("#pane-discord:not([hidden])", timeout=20_000)
    page.wait_for_function(
        "() => document.getElementById('page-update').hidden", timeout=20_000)
    assert_bar(page, f"{label} channel after the chip reload", pane=True)
    # The Documents list: a bar, but no control pane.
    page.goto(base + "/?v=docs", wait_until="load")
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    assert_bar(page, f"{label} documents list", pane=False)
    # Open the reader and reload at its deep link: the reader reopens
    # with no read standing, and its transport stands with it (`#1515`).
    page.click("#documents-list .doc-card")
    page.wait_for_selector("#doc-body h1", timeout=20_000)
    assert_bar(page, f"{label} reader before reload", pane=True)
    page.goto(base + "/?v=docs&d=" + doc_key, wait_until="load")
    page.wait_for_function(
        "() => !document.getElementById('pane-docs').hidden && "
        "!document.getElementById('docs-reader').hidden", timeout=20_000)
    assert_bar(page, f"{label} reader after reload", pane=True)


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="page-update-reload-") as tmp:
        work = Path(tmp)
        (work / "demo-doc").write_text(PASTE_BODY)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "page-update-reload-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
            '[documents]\n'
            'paste_url_prefixes = ["https://paste.example/"]\n'
            'paste_command = ["/bin/cat", "--", "{}"]\n')
        server = subprocess.Popen(
            [str(ROOT / "target/release/vibe-talk"), "--config", str(work / "vibe-talk.toml"),
             "--fake-discord", "--skip-startup-probe"],
            cwd=work, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
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
                 {"text": "Report: https://paste.example/demo-doc"})
            deadline = time.time() + 30
            doc_key = ""
            while time.time() < deadline:
                answer = call(base, "GET", "/api/v1/documents?limit=10", READ_TOKEN)
                docs = answer.get("documents", [])
                if len(docs) >= 1:
                    doc_key = str(docs[0].get("key") or docs[0].get("file_name") or "")
                    break
                time.sleep(0.5)
            check(doc_key != "", "the seeded document never registered")
            with sync_playwright() as pw:
                browser = pw.chromium.launch(
                    args=["--autoplay-policy=no-user-gesture-required"])
                for (width, height, touch, label) in [
                        (393, 852, True, "phone"), (1280, 800, False, "desktop")]:
                    ctx = browser.new_context(
                        viewport={"width": width, "height": height},
                        has_touch=touch, is_mobile=touch)
                    page = ctx.new_page()
                    page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }}"
                        " catch (e) {}")
                    walk(page, base, label, doc_key)
                    ctx.close()
                browser.close()
        finally:
            server.terminate()
    print("page-update reload check: the bottom bar stands after every reload")
    return 0


if __name__ == "__main__":
    sys.exit(main())
