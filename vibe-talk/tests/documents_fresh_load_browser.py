"""A fresh Documents load lands at the bottom and stays there (`#2002 vibe-talk-documents-fresh-load-bottom`).

Seeds 120 paste documents through the reply route (more than the first
100-row page, so the older-page path arms), then at phone 412x915 and
desktop 1280x800 opens the Documents view fresh (a navigation, then a
reload) with a per-frame trace installed before the page's own scripts:
scrollTop against scrollHeight-clientHeight (the gap from the bottom),
the drawn card count and the pane's hidden flag, printed as TRACE lines
for about 2 s. Once the list first reaches the bottom it must never
visibly leave it, and once settled it must sit within 1px of it.

Run: python3 tests/documents_fresh_load_browser.py
"""

import json
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path
from typing import cast

from playwright.sync_api import Page, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "documents-fresh-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-fresh-write-token-not-a-real-credential"
DOCS = 120

RECORDER = """
window.__load = { samples: [], stop: false };
const bootLoadTrace = () => {
  const sample = () => {
    const j = window.__load;
    if (!j || j.stop) return;
    const area = document.getElementById('scroll-area');
    if (area) {
      const pane = document.getElementById('pane-docs');
      j.samples.push({
        t: Math.round(performance.now()),
        st: Math.round(area.scrollTop * 10) / 10,
        sh: area.scrollHeight,
        ch: area.clientHeight,
        gap: Math.round((area.scrollHeight - area.clientHeight - area.scrollTop) * 10) / 10,
        cards: document.querySelectorAll('#documents-list .doc-card').length,
        paneHidden: pane ? pane.hidden : null,
      });
    }
    requestAnimationFrame(sample);
  };
  requestAnimationFrame(sample);
};
if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', bootLoadTrace);
else bootLoadTrace();
"""


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


def assert_lands_and_stays(page: Page, label: str) -> None:
    page.wait_for_function(
        "() => document.querySelectorAll('#documents-list .doc-card').length >= 100",
        timeout=30_000)
    page.wait_for_timeout(2000)
    samples = cast(list[dict[str, object]], page.evaluate(
        "() => { if (window.__load) window.__load.stop = true; return window.__load ? window.__load.samples : []; }"))
    drawn = [s for s in samples if cast(int, s["cards"]) >= 100]
    check(len(drawn) > 0, f"{label}: the trace never saw the drawn list ({len(samples)} frames)")
    arrived = next((i for i, s in enumerate(drawn) if cast(float, s["gap"]) <= 1), None)
    last_gap: float | None = None
    for s in drawn:
        gap = cast(float, s["gap"])
        if last_gap is None or abs(gap - last_gap) >= 0.5:
            print(f"TRACE {label} t={s['t']} scrollTop={s['st']} scrollHeight={s['sh']} "
                  f"clientHeight={s['ch']} gap={gap} cards={s['cards']} paneHidden={s['paneHidden']}")
            last_gap = gap
    check(arrived is not None, f"{label}: a fresh load never reached the bottom (last gap {drawn[-1]['gap']})")
    assert arrived is not None
    after = drawn[arrived:]
    worst = max(cast(float, s["gap"]) for s in after)
    settled = cast(float, drawn[-1]["gap"])
    print(f"TRACE {label} frames={len(samples)} drawn={len(drawn)} worst-gap-after-arrival={worst:.1f}px settled-gap={settled:.1f}px")
    check(worst <= 3, f"{label}: after landing at the bottom the list left it by {worst:.1f}px")
    check(settled <= 1, f"{label}: once settled the list sits {settled:.1f}px from the bottom, not within 1px")


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="documents-fresh-check-") as tmp:
        work = Path(tmp)
        for index in range(DOCS):
            (work / f"doc-{index:03d}").write_text(f"# Fresh document {index:03d}\n\nA body.\n")
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "documents-fresh-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
            "[documents]\n"
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
            # Twenty replies of six links: mention times stay ordered oldest to newest.
            names = [f"doc-{index:03d}" for index in range(DOCS)]
            for start in range(0, DOCS, 6):
                batch = " ".join(f"https://paste.example/{name}" for name in names[start:start + 6])
                call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                     {"text": f"Batch: {batch}"})
            docs: list[dict[str, object]] = []
            deadline = time.time() + 30
            while time.time() < deadline:
                call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=30&render=html", WRITE_TOKEN)
                answer = call(base, "GET", "/api/v1/documents?limit=200", READ_TOKEN)
                docs = cast(list[dict[str, object]], answer["documents"])
                if len(docs) == DOCS:
                    break
                time.sleep(0.5)
            check(len(docs) == DOCS, f"the fold registered {len(docs)} documents, not {DOCS}")

            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                for label, width, height, touch in (
                    ("phone412", 412, 915, True),
                    ("desktop", 1280, 800, False),
                ):
                    ctx = browser.new_context(viewport={"width": width, "height": height},
                                              has_touch=touch, is_mobile=touch)
                    page = ctx.new_page()
                    page.add_init_script(RECORDER)
                    page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                    page.goto(base + "/?v=docs", wait_until="load")
                    assert_lands_and_stays(page, f"{label} fresh")
                    page.reload(wait_until="load")
                    # The recorder was an init script: it re-arms on reload.
                    assert_lands_and_stays(page, f"{label} reload")
                    ctx.close()
                browser.close()
            print("documents fresh load: lands at the bottom within 1px and never visibly "
                  "leaves it, on fresh navigation and reload, at 412x915 and 1280x800")
            return 0
        finally:
            server.terminate()
            server.wait(timeout=10)


if __name__ == "__main__":
    sys.exit(main())
