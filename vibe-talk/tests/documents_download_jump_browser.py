"""The tapped Documents card holds its place across a download tap (`#1999 vibe-talk-documents-download-jump`).

Seeds thirty GitHub markdown documents through the reply route (three sets
of ten, one per walked size), each with a short file name and a fetched
title long enough to wrap onto extra lines on a phone, then at phone
412x915, phone 393x852 and desktop 1280x800 (touch on the phones): a
download tap that errors, the retry of that same card from error to
success, a first-try success, a download that stays broken, and (at the
opening size and on desktop) a success tapped while parked at the list's
bottom. Every tap is traced per animation frame for about 1.5 s — each
card's rect.top, the scroller's scrollTop/scrollHeight, the tapped card's
fetch dress and the focused element, printed as TRACE lines, in
send_flash_browser.py's shape — and the tapped card must move at most
3px from where the tap found it. With --screenshots DIR, before/after
shots of the retry tap (light and dark, phone 412 and desktop 1280) land
there with BUILD.txt.

Run: python3 tests/documents_download_jump_browser.py [--screenshots DIR]
"""

import argparse
import json
import socket
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import cast

from playwright.sync_api import Page, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "documents-jump-check-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-jump-check-write-token-not-a-real-credential"
SETS = ("p", "n", "d")
LONG_TITLE = "Quarterly migration review with appendices, rollout notes and sign-off"
HITS: dict[str, int] = {}
HITS_LOCK = threading.Lock()


def body_for(name: str) -> str:
    return f"# {LONG_TITLE} {name}\n\nThe body of {name}.\n"


def doc_name(prefix: str, index: int) -> str:
    return f"{prefix}{index:02d}"


def doc_path(name: str) -> str:
    return f"/octo/widgets/main/docs/{name}.md"


def doc_key(name: str) -> str:
    return f"github:octo/widgets@main:docs/{name}.md"


def special(prefix: str, kind: str) -> str:
    index = {"broken": 2, "flaky": 4, "good": 7}[kind]
    return doc_name(prefix, index)


class RawHandler(BaseHTTPRequestHandler):
    """A raw-content host: flaky paths fail once with 500 then serve, broken paths always 500."""

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
        path = self.path.split("?", 1)[0]
        with HITS_LOCK:
            HITS[path] = HITS.get(path, 0) + 1
            hits = HITS[path]
        name = path.rsplit("/", 1)[-1].removesuffix(".md")
        if name.endswith("02"):
            self.reply(500, b"broken upstream")
        elif name.endswith("04") and hits == 1:
            self.reply(500, b"flaky upstream")
        elif path.startswith("/octo/widgets/main/docs/"):
            self.reply(200, body_for(name).encode())
        else:
            self.reply(404, b"not found")

    def reply(self, status: int, data: bytes) -> None:
        self.send_response(status)
        self.send_header("Content-Type", "text/markdown")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, format: str, *args: object) -> None:  # noqa: A002 - the base name
        pass


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


def write_build_stamp(folder: Path, page_build: str) -> None:
    def git(*argv: str) -> str:
        return subprocess.run(["git", "-C", str(ROOT.parent), *argv],
                              capture_output=True, text=True).stdout.strip() or "unknown"

    (folder / "BUILD.txt").write_text(
        f"commit {git('rev-parse', 'HEAD')}\n"
        f"commit-date {git('log', '-1', '--format=%cI', 'HEAD')}\n"
        f"page-build {page_build}\n",
        encoding="utf-8")


TRACE_START = """(key) => {
  window.__jump = { samples: [], stop: false };
  const area = document.getElementById('scroll-area');
  const sample = () => {
    const j = window.__jump;
    if (!j || j.stop) return;
    const card = document.querySelector('#documents-list .doc-card[data-key="' + key + '"]');
    const rect = card ? card.getBoundingClientRect() : null;
    const active = document.activeElement;
    j.samples.push({
      t: Math.round(performance.now()),
      st: Math.round(area.scrollTop * 10) / 10,
      sh: area.scrollHeight,
      top: rect ? Math.round(rect.top * 10) / 10 : null,
      cls: card ? card.className : null,
      dl: card ? (card.querySelector('.doc-download')?.textContent ?? null) : null,
      active: active ? (active.className || active.tagName) : null,
    });
    requestAnimationFrame(sample);
  };
  requestAnimationFrame(sample);
}"""


def start_trace(page: Page, key: str) -> None:
    page.evaluate(TRACE_START, arg=key)


def stop_trace(page: Page, label: str) -> list[dict[str, object]]:
    samples = cast(list[dict[str, object]], page.evaluate(
        "() => { if (window.__jump) window.__jump.stop = true; return window.__jump ? window.__jump.samples : []; }"))
    check(len(samples) >= 10, f"{label}: the trace holds {len(samples)} frames, not a 1.5 s walk")
    last_top: float | None = None
    for i, sample in enumerate(samples):
        top = sample["top"]
        if i == 0 or i == len(samples) - 1 or (top is not None and last_top is not None and abs(cast(float, top) - last_top) >= 0.5):
            print(f"TRACE {label} t={sample['t']} scrollTop={sample['st']} scrollHeight={sample['sh']} "
                  f"tappedTop={top} download={sample['dl']!r} active={sample['active']!r}")
            if top is not None:
                last_top = cast(float, top)
    return samples


def assert_held(samples: list[dict[str, object]], label: str) -> None:
    tops = [cast(float, s["top"]) for s in samples if s["top"] is not None]
    check(len(tops) == len(samples), f"{label}: the tapped card left the list mid-trace")
    check(len(tops) > 0, f"{label}: the trace never saw the tapped card")
    base = tops[0]
    worst = max(abs(top - base) for top in tops)
    print(f"TRACE {label} frames={len(samples)} tappedTop first={base} worst-move={worst:.1f}px")
    check(worst <= 3, f"{label}: the tapped card moved {worst:.1f}px from {base} (tops: {tops[:40]})")


def place_mid(page: Page, key: str) -> None:
    """Scroll so the card named `key` sits at 40% of the scroller's height; no tap."""
    page.evaluate(
        """(key) => {
          const area = document.getElementById('scroll-area');
          const card = document.querySelector('#documents-list .doc-card[data-key="' + key + '"]');
          const band = area.getBoundingClientRect();
          area.scrollTop += card.getBoundingClientRect().top - band.top - area.clientHeight * 0.4;
        }""", arg=key)
    page.wait_for_timeout(150)


def tap_download(page: Page, key: str, *, touch: bool) -> None:
    button = page.locator(f'#documents-list .doc-card[data-key="{key}"] .doc-download')
    box = button.bounding_box()
    check(box is not None, f"the download button for {key} has no box to tap")
    assert box is not None
    if touch:
        page.touchscreen.tap(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
    else:
        page.mouse.click(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)


def wait_card(page: Page, key: str, needle: str, *, label: str) -> None:
    page.wait_for_function(
        """([key, needle]) => {
          const card = document.querySelector('#documents-list .doc-card[data-key="' + key + '"]');
          return card !== null && card.textContent.includes(needle);
        }""", arg=[key, needle], timeout=30_000)


def traced_tap(page: Page, key: str, *, touch: bool, label: str, needle: str) -> list[dict[str, object]]:
    start_trace(page, key)
    tap_download(page, key, touch=touch)
    wait_card(page, key, needle, label=label)
    page.wait_for_timeout(1500)
    return stop_trace(page, label)


def walk(page: Page, prefix: str, *, label: str, touch: bool, shots: Path | None, scheme: str) -> None:
    page.wait_for_selector("#documents-list .doc-card", timeout=30_000)
    page.wait_for_function(
        "() => document.querySelectorAll('#documents-list .doc-card').length >= 30",
        timeout=30_000)
    flaky = doc_key(special(prefix, "flaky"))
    good = doc_key(special(prefix, "good"))
    broken = doc_key(special(prefix, "broken"))

    # The owner's starred markdown card: star it, then tap its download.
    page.locator(f'#documents-list .doc-card[data-key="{flaky}"] .doc-star').click()
    page.wait_for_function(
        """(key) => document.querySelector('#documents-list .doc-card[data-key="' + key + '"]')
          ?.querySelector('.doc-star')?.getAttribute('aria-pressed') === 'true'""",
        arg=flaky, timeout=10_000)

    place_mid(page, flaky)
    samples = traced_tap(page, flaky, touch=touch, label=f"{label} error", needle="Could not fetch")
    assert_held(samples, f"{label} error")

    place_mid(page, flaky)
    if shots is not None:
        page.screenshot(path=str(shots / f"download-jump-{label}-{scheme}-before.png"))
    samples = traced_tap(page, flaky, touch=touch, label=f"{label} retry", needle="✓")
    assert_held(samples, f"{label} retry error-to-success")
    status = cast(str, page.evaluate("() => document.getElementById('status')?.textContent ?? ''"))
    check("Could not fetch" not in status,
          f"{label} retry: the failure toast survived the successful retry: {status!r}")
    check(status == f"Fetched {special(prefix, 'flaky')}.md.",
          f"{label} retry: the status does not say the retry fetched it: {status!r}")
    if shots is not None:
        page.screenshot(path=str(shots / f"download-jump-{label}-{scheme}-after.png"))

    place_mid(page, good)
    samples = traced_tap(page, good, touch=touch, label=f"{label} success", needle="✓")
    assert_held(samples, f"{label} first-try success")

    place_mid(page, broken)
    samples = traced_tap(page, broken, touch=touch, label=f"{label} broken", needle="Could not fetch")
    assert_held(samples, f"{label} stays broken")


def walk_bottom(page: Page, *, label: str, touch: bool) -> None:
    """Parked at the bottom (where the list opens), tap the bottom-most unfetched card."""
    page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
    page.wait_for_timeout(200)
    key = cast(str, page.evaluate(
        """() => {
          const area = document.getElementById('scroll-area');
          const band = area.getBoundingClientRect();
          const cards = [...document.querySelectorAll('#documents-list .doc-card.doc-unfetched')];
          const visible = cards.filter((c) => {
            const r = c.getBoundingClientRect();
            return r.bottom > band.top && r.top < band.bottom;
          });
          const pick = visible[visible.length - 1] || cards[cards.length - 1];
          return pick ? pick.getAttribute('data-key') : null;
        }"""))
    check(key is not None, f"{label}: no unfetched card stands at the bottom to tap")
    samples = traced_tap(page, key, touch=touch, label=f"{label} bottom", needle="✓")
    assert_held(samples, f"{label} bottom success")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--screenshots", type=Path, default=None)
    args = parser.parse_args()
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    raw = ThreadingHTTPServer(("127.0.0.1", 0), RawHandler)
    threading.Thread(target=raw.serve_forever, daemon=True).start()
    raw_base = f"http://127.0.0.1:{raw.server_address[1]}"
    with tempfile.TemporaryDirectory(prefix="documents-jump-check-") as tmp:
        work = Path(tmp)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "documents-jump-check-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
            "[documents]\n"
            f'github_raw_base = "{raw_base}"\n'
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
            for prefix in SETS:
                for index in range(10):
                    name = doc_name(prefix, index)
                    call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                         {"text": f"Doc {name}: https://github.com/octo/widgets/blob/main/docs/{name}.md"})
            docs: list[dict[str, object]] = []
            deadline = time.time() + 30
            while time.time() < deadline:
                call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=40&render=html", WRITE_TOKEN)
                answer = call(base, "GET", "/api/v1/documents?limit=100", READ_TOKEN)
                docs = cast(list[dict[str, object]], answer["documents"])
                if len(docs) == 30:
                    break
                time.sleep(0.5)
            check(len(docs) == 30, f"the fold registered {len(docs)} documents, not 30")

            if args.screenshots is not None:
                args.screenshots.mkdir(parents=True, exist_ok=True)

            with sync_playwright() as pw:
                browser = pw.chromium.launch()

                def open_page(width: int, height: int, *, touch: bool, scheme: str = "light") -> Page:
                    ctx = browser.new_context(viewport={"width": width, "height": height},
                                              has_touch=touch, is_mobile=touch, color_scheme=scheme)  # type: ignore[arg-type]
                    page = ctx.new_page()
                    page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                    page.goto(base + "/?v=docs", wait_until="load")
                    return page

                page = open_page(412, 915, touch=True)
                walk(page, "p", label="phone412", touch=True, shots=args.screenshots, scheme="light")
                walk_bottom(page, label="phone412", touch=True)
                page.context.close()

                page = open_page(393, 852, touch=True)
                walk(page, "n", label="phone393", touch=True, shots=None, scheme="light")
                page.context.close()

                page = open_page(1280, 800, touch=False)
                walk(page, "d", label="desktop", touch=False, shots=args.screenshots, scheme="light")
                walk_bottom(page, label="desktop", touch=False)
                page.context.close()

                if args.screenshots is not None:
                    # Dark pair: the contexts share one server, so the dark pages
                    # photograph a success tap on any still-unfetched card they find.
                    for label, width, height, touch in (("phone412", 412, 915, True), ("desktop", 1280, 800, False)):
                        page = open_page(width, height, touch=touch, scheme="dark")
                        page.wait_for_selector("#documents-list .doc-card", timeout=30_000)
                        key = cast(str, page.evaluate(
                            """() => {
                              const card = document.querySelector('#documents-list .doc-card.doc-unfetched');
                              return card ? card.getAttribute('data-key') : null;
                            }"""))
                        check(key is not None, f"dark {label}: no unfetched card to photograph")
                        place_mid(page, key)
                        page.screenshot(path=str(args.screenshots / f"download-jump-{label}-dark-before.png"))
                        samples = traced_tap(page, key, touch=touch, label=f"dark {label}", needle="✓")
                        assert_held(samples, f"dark {label} success")
                        page.screenshot(path=str(args.screenshots / f"download-jump-{label}-dark-after.png"))
                        build = cast(str, page.evaluate(
                            "() => { const m = document.body.innerText.match(/build\\s+(\\d+)/i); return m ? m[1] : 'unknown'; }"))
                        page.context.close()
                    write_build_stamp(args.screenshots, build)
                browser.close()
            print("documents download jump: the tapped card held within 3px across error, "
                  "retry-to-success, first-try success and bottom taps at 412x915, 393x852 and 1280x800")
            return 0
        finally:
            server.terminate()
            server.wait(timeout=10)


if __name__ == "__main__":
    sys.exit(main())
