"""The Documents desktop control bar (`#1544 documents-desktop-control-bar`).

Against the REAL server (release binary, --fake-discord, one seeded paste
document with headings): at 1280x800 (mouse) the reader controls — the
device/cloud audio toggle, Pace, and Read — stand in one centred box named
Reader, Fetch stands at the bar's right in the list view and gives way to
Outline in the reader (never both), and the outline and pace popups centre
over their buttons inside the viewport. At 393x852 (touch) the bar keeps
its phone layout and the outline popup still stays on screen.
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
READ_TOKEN = "documents-control-bar-check-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-control-bar-check-write-token-not-a-real-credential"
DOC_BODY = (
    "# Control bar report\n\nThe nightly run finished green.\n\n"
    "## Details\n\nThe detail under the second heading.\n\n"
    "## Later\n\nThe words under the third heading.\n"
)


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


def bar_state(page: Page) -> dict[str, object]:
    result = cast(dict[str, object], page.evaluate(
        """() => {
          const box = (id) => {
            const el = document.getElementById(id);
            if (el === null) return null;
            const r = el.getBoundingClientRect();
            const s = getComputedStyle(el);
            return {
              hidden: el.hidden, display: s.display,
              left: r.left, right: r.right, top: r.top, bottom: r.bottom,
              width: r.width, height: r.height,
              centre: r.left + r.width / 2,
              parent: el.parentElement === null ? null : el.parentElement.id,
              border: parseFloat(s.borderTopWidth),
            };
          };
          const pane = document.getElementById('control-pane');
          return {
            innerWidth,
            fetch: box('docs-fetch'), outline: box('doc-outline-open'),
            pane: box('control-pane'), audio: box('audio-source'),
            speed: box('read-speed'), read: box('read-aloud'),
            menu: box('action-menu-open'), outlinePop: box('doc-outline-pop'),
            speedPop: box('speed-popover'),
            paneLabel: pane === null ? null
              : getComputedStyle(pane, '::before').content,
          };
        }"""))
    return result


def num(box: dict[str, object], key: str) -> float:
    return float(cast(float, box[key]))


def flag(box: dict[str, object], key: str) -> bool:
    return cast(bool, box[key])


def check_desktop(page: Page) -> None:
    # List view: Fetch at the right, no Outline, no Reader box yet.
    state = bar_state(page)
    fetch = cast(dict[str, object], state["fetch"])
    outline = cast(dict[str, object], state["outline"])
    menu = cast(dict[str, object], state["menu"])
    check(fetch["display"] != "none" and not flag(fetch, "hidden"),
          f"Fetch is not standing in the list view: {state}")
    check(outline["display"] == "none" or flag(outline, "hidden"),
          f"Outline shows in the list view beside Fetch: {state}")
    check(abs(num(fetch, "right") - num(menu, "left")) < 80,
          f"Fetch is not at the bar's right beside the hamburger: {state}")

    page.locator("#documents-list .doc-card", has_text="control-doc").click()
    page.wait_for_selector("#doc-body h1", timeout=20_000)
    page.wait_for_function(
        "() => !document.getElementById('read-aloud').hidden", timeout=20_000)

    # Reader view: Outline replaces Fetch, and the reader controls stand in
    # one centred box named Reader.
    state = bar_state(page)
    fetch = cast(dict[str, object], state["fetch"])
    outline = cast(dict[str, object], state["outline"])
    pane = cast(dict[str, object], state["pane"])
    check(flag(fetch, "hidden") or fetch["display"] == "none",
          f"Fetch still shows in the reader view: {state}")
    check(outline["display"] != "none" and not flag(outline, "hidden"),
          f"Outline is not standing in the reader view: {state}")
    for part in ["audio", "speed", "read"]:
        part_box = cast(dict[str, object], state[part])
        check(part_box["parent"] == "control-pane",
              f"#{part} is not grouped in the Reader box: {state}")
    check(num(pane, "border") > 0, f"the Reader box has no border: {state}")
    check(state["paneLabel"] is not None and "Reader" in str(state["paneLabel"]),
          f"the Reader box is not named Reader: {state}")
    check(abs(num(pane, "centre") - num(state, "innerWidth") / 2) < 60,
          f"the Reader box is not centred: {state}")

    # The outline popup centres over its button, on screen.
    page.click("#doc-outline-open")
    page.wait_for_selector("#doc-outline-pop", state="visible", timeout=20_000)
    state = bar_state(page)
    pop = cast(dict[str, object], state["outlinePop"])
    outline = cast(dict[str, object], state["outline"])
    check(abs(num(pop, "centre") - num(outline, "centre")) < 40,
          f"the outline popup is not centred over Outline: {state}")
    check(num(pop, "left") >= 0 and num(pop, "right") <= num(state, "innerWidth"),
          f"the outline popup spills off screen: {state}")
    page.click("#doc-outline-open")

    # The pace popup centres over Pace, on screen.
    page.click("#read-speed")
    page.wait_for_selector("#speed-popover", state="visible", timeout=20_000)
    state = bar_state(page)
    pop = cast(dict[str, object], state["speedPop"])
    speed = cast(dict[str, object], state["speed"])
    check(abs(num(pop, "centre") - num(speed, "centre")) < 60,
          f"the pace popup is not centred over Pace: {state}")
    check(num(pop, "left") >= 0 and num(pop, "right") <= num(state, "innerWidth"),
          f"the pace popup spills off screen: {state}")


def check_phone(page: Page) -> None:
    page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
    state = bar_state(page)
    fetch = cast(dict[str, object], state["fetch"])
    check(fetch["display"] != "none" and not flag(fetch, "hidden"),
          f"Fetch is not standing in the phone list view: {state}")
    card = page.locator("#documents-list .doc-card", has_text="control-doc")
    card.wait_for(state="visible", timeout=20_000)
    box = card.bounding_box()
    assert box is not None
    page.touchscreen.tap(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)
    page.wait_for_selector("#doc-body h1", timeout=20_000)
    state = bar_state(page)
    fetch = cast(dict[str, object], state["fetch"])
    pane = cast(dict[str, object], state["pane"])
    check(flag(fetch, "hidden") or fetch["display"] == "none",
          f"Fetch still shows in the phone reader view: {state}")
    check(num(pane, "border") == 0,
          f"the phone pane took the desktop Reader box: {state}")
    page.tap("#doc-outline-open")
    page.wait_for_selector("#doc-outline-pop", state="visible", timeout=20_000)
    state = bar_state(page)
    pop = cast(dict[str, object], state["outlinePop"])
    check(num(pop, "left") >= 0 and num(pop, "right") <= num(state, "innerWidth") + 1,
          f"the phone outline popup spills off screen: {state}")


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="documents-control-bar-check-") as tmp:
        work = Path(tmp)
        (work / "control-doc").write_text(DOC_BODY)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "documents-control-bar-check-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
            '[documents]\n'
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
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                 {"text": "Report: https://paste.example/control-doc"})
            docs: list[dict[str, object]] = []
            deadline = time.time() + 30
            while time.time() < deadline:
                call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=10&render=html", WRITE_TOKEN)
                answer = call(base, "GET", "/api/v1/documents?limit=10", READ_TOKEN)
                docs = cast(list[dict[str, object]], answer["documents"])
                if len(docs) == 1:
                    break
                time.sleep(0.5)
            check(len(docs) == 1, f"registered {len(docs)} documents, not 1: {docs}")

            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                dctx = browser.new_context(viewport={"width": 1280, "height": 800})
                dpage = dctx.new_page()
                dpage.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                dpage.goto(base + "/?v=docs", wait_until="load")
                dpage.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                check_desktop(dpage)
                dctx.close()

                ctx = browser.new_context(viewport={"width": 393, "height": 852}, has_touch=True, is_mobile=True)
                page = ctx.new_page()
                page.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                page.goto(base + "/?v=docs", wait_until="load")
                check_phone(page)
                ctx.close()
                browser.close()
            print("documents control bar browser check: centred Reader box, "
                  "Fetch/Outline exclusivity, and centred clamped popups at desk and phone")
            return 0
        finally:
            server.terminate()
            server.wait(timeout=10)


if __name__ == "__main__":
    sys.exit(main())
