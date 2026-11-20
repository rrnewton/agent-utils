"""Documents toolbar pair against the REAL server (`#2001 vibe-talk-documents-toolbar-star-downloaded`).

Seeds six paste documents, fetches three and stars two (one fetched, one
not), then at phone 412 and desktop 1280, in dark and light schemes:

1. The toolbar reads [downloaded][star][hamburger][Fetch][Documents], the
   star immediately left of the hamburger, with no member outside the
   viewport and no horizontal overflow.
2. Downloaded shows only the fetched documents, Starred only the starred
   ones, both together only the starred fetched one, and each composes
   with the bucket chips.
3. Both toggles work from the keyboard; a reload resets them to the full list, as the star filter does today, while each document's own star and fetched state survive.

Run where a browser works (headless shell is enough):
`python3 tests/documents_toolbar_browser.py [--screenshots DIR]`
Shots land in DIR/<scheme>-<phone|desktop>/ with a BUILD.txt in every
folder; without DIR they land in a temporary folder whose path is printed.
"""

import json
import socket
import subprocess
import sys
import tempfile
import time
import urllib.request
from pathlib import Path
from typing import Literal, cast

from playwright.sync_api import Page, ViewportSize, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "documents-toolbar-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-toolbar-write-token-not-a-real-credential"
DOCS = 6
FETCHED = ["doc-000", "doc-001", "doc-002"]
STARRED = ["doc-001", "doc-004"]


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


def write_build_stamp(folder: Path) -> None:
    def git(*argv: str) -> str:
        return subprocess.run(["git", "-C", str(ROOT.parent), *argv],
                              capture_output=True, text=True).stdout.strip() or "unknown"

    folder.mkdir(parents=True, exist_ok=True)
    (folder / "BUILD.txt").write_text(
        f"commit {git('rev-parse', 'HEAD')}\n"
        f"commit-date {git('log', '-1', '--format=%cI', 'HEAD')}\n"
        "page-build real-server target/release/vibe-talk\n",
        encoding="utf-8",
    )


def card_names(page: Page) -> list[str]:
    names = page.evaluate(
        """() => [...document.querySelectorAll('#documents-list .doc-card')].map((card) => {
          const file = card.querySelector('.doc-meta-file');
          return file ? file.textContent : '';
        })""")
    return cast(list[str], names)


def wait_names(page: Page, want: list[str], label: str) -> None:
    page.wait_for_function(
        """(want) => {
          const names = [...document.querySelectorAll('#documents-list .doc-card')].map((card) => {
            const file = card.querySelector('.doc-meta-file');
            return file ? file.textContent : '';
          });
          return JSON.stringify(names) === JSON.stringify(want);
        }""", arg=want, timeout=20_000)
    check(card_names(page) == want, f"{label}: cards are {card_names(page)}, not {want}")


def pressed(page: Page, element_id: str) -> bool:
    return page.get_attribute(f"#{element_id}", "aria-pressed") == "true"


def ensure_star(page: Page, name: str) -> None:
    is_starred = page.evaluate(
        """(name) => {
          const cards = [...document.querySelectorAll('#documents-list .doc-card')];
          const card = cards.find((el) => {
            const file = el.querySelector('.doc-meta-file');
            return file && file.textContent === name;
          });
          const star = card ? card.querySelector('.doc-star') : null;
          return !!star && star.getAttribute('aria-pressed') === 'true';
        }""", name)
    if is_starred:
        return
    card = page.locator("#documents-list .doc-card", has_text=name).first
    card.locator(".doc-star").click()
    page.wait_for_function(
        """(name) => {
          const cards = [...document.querySelectorAll('#documents-list .doc-card')];
          const card = cards.find((el) => {
            const file = el.querySelector('.doc-meta-file');
            return file && file.textContent === name;
          });
          const star = card ? card.querySelector('.doc-star') : null;
          return !!star && star.getAttribute('aria-pressed') === 'true';
        }""", arg=name, timeout=20_000)


def fetch_doc(page: Page, name: str) -> None:
    is_fetched = page.evaluate(
        """(name) => {
          const cards = [...document.querySelectorAll('#documents-list .doc-card')];
          const card = cards.find((el) => {
            const file = el.querySelector('.doc-meta-file');
            return file && file.textContent === name;
          });
          return !!card && card.classList.contains('doc-card') && !card.classList.contains('doc-unfetched');
        }""", name)
    if is_fetched:
        return
    card = page.locator("#documents-list .doc-card", has_text=name).first
    card.locator(".doc-download").click()
    page.wait_for_function(
        """(name) => {
          const cards = [...document.querySelectorAll('#documents-list .doc-card')];
          const card = cards.find((el) => {
            const file = el.querySelector('.doc-meta-file');
            return file && file.textContent === name;
          });
          const download = card ? card.querySelector('.doc-download') : null;
          return !!download && (download.getAttribute('aria-label') || '').endsWith('is fetched');
        }""", arg=name, timeout=20_000)


def check_toolbar(page: Page, label: str) -> None:
    facts = cast(dict[str, object], page.evaluate(
        """() => {
          const box = (id) => {
            const el = document.getElementById(id);
            if (!el) return null;
            const r = el.getBoundingClientRect();
            return { left: r.left, right: r.right, width: r.width, hidden: el.hidden,
                     display: getComputedStyle(el).display };
          };
          return {
            innerWidth,
            scrollWidth: document.documentElement.scrollWidth,
            downloaded: box('docs-downloaded-toggle'),
            starred: box('docs-starred-toggle'),
            menu: box('action-menu-open'),
            fetch: box('docs-fetch'),
            viewSwitch: box('view-switch'),
          };
        }"""))
    order = ["downloaded", "starred", "menu", "fetch", "viewSwitch"]
    boxes = [cast(dict[str, object], facts[name]) for name in order]
    for name, box in zip(order, boxes):
        check(box is not None and not box["hidden"] and box["display"] != "none",
              f"{label}: #{name} is not standing on the toolbar: {facts}")
        check(float(cast(float, box["left"])) >= 0 and float(cast(float, box["right"])) <= float(cast(int, facts["innerWidth"])) + 1,
              f"{label}: #{name} is outside the viewport: {facts}")
    lefts = [float(cast(float, box["left"])) for box in boxes]
    check(lefts == sorted(lefts), f"{label}: the toolbar does not read downloaded, star, hamburger, Fetch, Documents: {facts}")
    gap = float(cast(float, boxes[2]["left"])) - float(cast(float, boxes[1]["right"]))
    check(gap < 24, f"{label}: the star is not immediately left of the hamburger (gap {gap}px): {facts}")
    check(int(cast(int, facts["scrollWidth"])) <= int(cast(int, facts["innerWidth"])) + 1,
          f"{label}: the page overflows horizontally: {facts}")


def walk(page: Page, label: str, shots: Path | None) -> None:
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    page.wait_for_function(
        f"() => document.querySelectorAll('#documents-list .doc-card').length === {DOCS}",
        timeout=20_000)
    for name in FETCHED:
        fetch_doc(page, name)
    for name in STARRED:
        ensure_star(page, name)
    check_toolbar(page, label)
    if shots is not None:
        page.screenshot(path=str(shots / "documents-toolbar.png"))

    page.locator("#docs-downloaded-toggle").click()
    check(pressed(page, "docs-downloaded-toggle"), f"{label}: Downloaded did not press")
    wait_names(page, FETCHED, f"{label} downloaded")
    if shots is not None:
        page.screenshot(path=str(shots / "documents-toolbar-downloaded.png"))
    page.locator("#docs-downloaded-toggle").click()
    wait_names(page, [f"doc-00{i}" for i in range(DOCS)], f"{label} downloaded off")

    page.locator("#docs-starred-toggle").click()
    check(pressed(page, "docs-starred-toggle"), f"{label}: Starred did not press")
    wait_names(page, STARRED, f"{label} starred")
    if shots is not None:
        page.screenshot(path=str(shots / "documents-toolbar-starred.png"))
    # Both together, then with a bucket chip.
    page.locator("#docs-downloaded-toggle").click()
    wait_names(page, ["doc-001"], f"{label} starred+downloaded")
    page.locator(".doc-bucket[data-bucket='markdown']").click()
    page.wait_for_function(
        "() => document.querySelectorAll('#documents-list .doc-card').length === 0",
        timeout=20_000)
    note = page.text_content("#documents-list .doc-empty")
    check(note == "No starred downloaded documents.",
          f"{label}: the combined empty note reads {note!r}")
    page.locator(".doc-bucket[data-bucket='all']").click()
    wait_names(page, ["doc-001"], f"{label} back to All")
    page.locator("#docs-downloaded-toggle").click()
    page.locator("#docs-starred-toggle").click()
    wait_names(page, [f"doc-00{i}" for i in range(DOCS)], f"{label} filters off")


def functional_walk(page: Page, base: str, label: str) -> None:
    # Keyboard: focus the Downloaded toggle and press Space.
    page.locator("#docs-downloaded-toggle").focus()
    page.keyboard.press("Space")
    check(pressed(page, "docs-downloaded-toggle"), f"{label}: Space did not press Downloaded")
    wait_names(page, FETCHED, f"{label} keyboard downloaded")
    # Reload, as the star filter behaves today: the toggle is session state
    # and resets to the full list, while each document's own star and
    # fetched state — the state that has always persisted — remain.
    page.reload(wait_until="load")
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    check(not pressed(page, "docs-downloaded-toggle"), f"{label}: Downloaded stayed on across a reload")
    wait_names(page, [f"doc-00{i}" for i in range(DOCS)], f"{label} reloaded full list")
    page.locator("#docs-downloaded-toggle").click()
    wait_names(page, FETCHED, f"{label} reloaded downloaded")


def main() -> int:
    shots_root: Path | None = None
    if "--screenshots" in sys.argv:
        shots_root = Path(sys.argv[sys.argv.index("--screenshots") + 1])
    else:
        shots_root = Path(tempfile.mkdtemp(prefix="documents-toolbar-shots-"))
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="documents-toolbar-check-") as tmp:
        work = Path(tmp)
        for index in range(DOCS):
            (work / f"doc-{index:03d}").write_text(f"# Toolbar document {index:03d}\n\nIts body.\n")
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "documents-toolbar-bot-token-never-sent"\n'
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
            for index in range(DOCS):
                call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                     {"text": f"Document {index:03d}: https://paste.example/doc-{index:03d}"})
            limit = time.time() + 60
            while time.time() < limit:
                call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=10&render=html", WRITE_TOKEN)
                answer = call(base, "GET", "/api/v1/documents?limit=500", READ_TOKEN)
                if len(cast(list[object], answer["documents"])) == DOCS:
                    break
                time.sleep(0.5)
            else:
                check(False, "the fold did not register the documents")

            combos: list[tuple[Literal["dark", "light"], ViewportSize, bool, str]] = [
                ("dark", {"width": 412, "height": 915}, True, "dark-phone-412"),
                ("dark", {"width": 1280, "height": 800}, False, "dark-desktop-1280"),
                ("light", {"width": 412, "height": 915}, True, "light-phone-412"),
                ("light", {"width": 1280, "height": 800}, False, "light-desktop-1280"),
            ]
            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                for scheme, viewport, touch, folder_name in combos:
                    shots = shots_root / folder_name
                    shots.mkdir(parents=True, exist_ok=True)
                    write_build_stamp(shots)
                    ctx = browser.new_context(
                        viewport=viewport, has_touch=touch,
                        is_mobile=touch, color_scheme=scheme)
                    page = ctx.new_page()
                    page.add_init_script(
                        f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                    label = f"{scheme} {folder_name}"
                    page.goto(base + "/?v=docs", wait_until="load")
                    walk(page, label, shots)
                    if touch:
                        # The audio toggle the phone list bar gave up (`#2001`):
                        # one tap on a document, and it stands in the reader's
                        # controls; Settings keeps the same choice too.
                        page.locator("#documents-list .doc-card", has_text="doc-000").first.click()
                        page.wait_for_selector("#doc-body h1", timeout=20_000)
                        page.wait_for_function(
                            "() => { const a = document.getElementById('audio-source');"
                            " return a && !a.hidden && a.parentElement && a.parentElement.id === 'control-pane'; }",
                            timeout=20_000)
                        page.screenshot(path=str(shots / "documents-toolbar-reader-audio.png"))
                        page.locator("#doc-back").click()
                        page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                    if folder_name == "dark-phone-412":
                        functional_walk(page, base, label)
                    ctx.close()
                browser.close()
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
    print(f"documents toolbar: pair beside the hamburger, both filters, keyboard and reload, at phone and desktop in dark and light; shots in {shots_root}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
