#!/usr/bin/env python3
"""GitHub helper failures in real Chromium (`#1997` / `#1998`).

Starts the real server with `--fake-discord` and a `[documents]
github_token_command` naming a helper that does not exist, with fallback
helpers turned off. A private GitHub document can then never load, and this
walk proves what the owner sees instead of the old cut-off line:

  * `/healthz` reports the helper as `missing` with the layer-named cause;
  * the document row shows "vibe-talk could not get your GitHub login:
    the helper ... is missing" plus its Open on GitHub way out;
  * the reader shows the same sentence, never a bare could-not-load.

At a phone (412x915, touch) and a desk (1280x800, mouse), in light and
dark. Screenshots of the row and reader states go to --screenshots DIR
when given. Fake helpers only: the configured helper is a path in a
temporary directory that is never created, and no real token exists here.
"""

from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path
from typing import TYPE_CHECKING, Literal

from playwright.sync_api import sync_playwright
import browser_waits

if TYPE_CHECKING:
    from playwright.sync_api import Browser, Page

ROOT = Path(__file__).resolve().parent.parent
BIN = Path(os.environ.get("VIBE_TALK_BIN", ROOT / "target/release/vibe-talk"))
CHANNEL = "100000000000000001"
READ_TOKEN = "ghhelper-check-read-token-not-a-real-credential"
WRITE_TOKEN = "ghhelper-check-write-token-not-a-real-credential"
HANG_BACKSTOP_MS = 120_000
CAUSE = "vibe-talk could not get your GitHub login: the helper"
DOC_URL = "https://github.com/octo/widgets/blob/main/docs/private.md"
DOC_KEY = "github:octo/widgets@main:docs/private.md"


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)
    print(f"ok: {message}")


def free_port() -> int:
    sock = socket.socket()
    sock.bind(("127.0.0.1", 0))
    port = int(sock.getsockname()[1])
    sock.close()
    return port


def call(
    base: str,
    method: str,
    path: str,
    token: str | None = None,
    body: dict[str, object] | None = None,
) -> tuple[int, dict[str, object]]:
    req = urllib.request.Request(base + path, method=method)
    if token:
        req.add_header("Authorization", f"Bearer {token}")
    data = None
    if body is not None:
        data = json.dumps(body).encode()
        req.add_header("Content-Type", "application/json")
    try:
        with urllib.request.urlopen(req, data=data, timeout=10) as resp:
            parsed: object = json.loads(resp.read() or b"null")
            return resp.status, parsed if isinstance(parsed, dict) else {"value": parsed}
    except urllib.error.HTTPError as error:
        raw = error.read()
        try:
            parsed = json.loads(raw or b"null")
        except Exception:
            return error.code, {"raw": raw.decode(errors="replace")}
        return error.code, parsed if isinstance(parsed, dict) else {"value": parsed}


def wait_helper(base: str) -> dict[str, object]:
    deadline = time.time() + 30
    helper: dict[str, object] = {}
    while time.time() < deadline:
        try:
            _, body = call(base, "GET", "/healthz")
        except Exception:
            time.sleep(0.3)
            continue
        found = body.get("github_helper")
        helper = found if isinstance(found, dict) else {}
        if helper.get("status") == "missing":
            return helper
        time.sleep(0.3)
    raise AssertionError(f"/healthz never reported the helper missing: {helper}")


def exercise(page: Page, base: str, shots: Path | None, label: str, scheme: str) -> None:
    page.add_init_script(
        "try { localStorage.setItem('vibe-talk.token', "
        + json.dumps(WRITE_TOKEN)
        + "); } catch (e) {}"
    )
    page.goto(base + "/?v=docs", wait_until="load")
    card = page.locator("#documents-list .doc-card", has_text="private.md")
    card.wait_for(timeout=HANG_BACKSTOP_MS)
    # The row: fetching it fails at the helper, and the row says why.
    card.locator(".doc-download").click()
    page.wait_for_function(
        "() => [...document.querySelectorAll('#documents-list .doc-card')]"
        ".some((c) => c.textContent.includes(" + json.dumps(CAUSE) + "))",
        timeout=HANG_BACKSTOP_MS,
    )
    row_text = card.text_content() or ""
    check(CAUSE in row_text and "is missing" in row_text,
          f"{label}/{scheme}: the row names vibe-talk and the missing helper: {row_text!r}")
    check("Could not fetch it." in row_text,
          f"{label}/{scheme}: the row keeps its fetch wording: {row_text!r}")
    link = card.locator("a.doc-open-source")
    check(link.count() == 1 and link.get_attribute("href") == DOC_URL,
          f"{label}/{scheme}: the failed row keeps its Open on GitHub way out")
    if shots is not None:
        page.screenshot(path=str(shots / f"{label}-{scheme}-row.png"))
    # The reader: a card tap on an unfetched row only refetches it (and
    # stays on the list when that fails), so the reader is opened through
    # the `?v=docs&d=KEY` deep link a document link itself uses.
    page.goto(base + "/?v=docs&d=" + urllib.parse.quote(DOC_KEY, safe=""),
              wait_until="load")
    page.wait_for_function(
        "() => (document.getElementById('doc-body')?.textContent || '')"
        ".includes(" + json.dumps(CAUSE) + ")",
        timeout=HANG_BACKSTOP_MS,
    )
    body_text = page.locator("#doc-body").text_content() or ""
    check("is missing" in body_text,
          f"{label}/{scheme}: the reader names the missing helper: {body_text!r}")
    check("Could not load the document." not in body_text,
          f"{label}/{scheme}: the reader shows no bare could-not-load")
    if shots is not None:
        page.screenshot(path=str(shots / f"{label}-{scheme}-reader.png"))


def run_context(
    browser: Browser,
    base: str,
    shots: Path | None,
    label: str,
    width: int,
    height: int,
    touch: bool,
    scheme: Literal["light", "dark"],
) -> None:
    ctx = browser.new_context(
        viewport={"width": width, "height": height}, has_touch=touch,
        is_mobile=touch, color_scheme=scheme)
    browser_waits.hot_host_timeouts(ctx)
    ctx.set_default_timeout(HANG_BACKSTOP_MS)
    page = ctx.new_page()
    browser_waits.hot_host_timeouts(page)
    exercise(page, base, shots, label, scheme)
    ctx.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--screenshots", type=Path, default=None,
                        help="directory for the row/reader screenshots")
    args = parser.parse_args()
    shots = args.screenshots
    if shots is not None:
        shots.mkdir(parents=True, exist_ok=True)
    if not BIN.exists():
        raise AssertionError(f"server binary missing: {BIN} (build release first)")
    print(f"ok: server binary is ready: {BIN}")
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    with tempfile.TemporaryDirectory(prefix="ghhelper-check-") as tmp:
        work = Path(tmp)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "ghhelper-check-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
            "[documents]\n"
            f'github_token_command = ["{work}/gh-gone", "auth", "token"]\n'
            "github_token_helper_fallbacks = []\n"
        )
        server = subprocess.Popen(
            [str(BIN), "--config", str(work / "vibe-talk.toml"),
             "--fake-discord", "--skip-startup-probe"],
            cwd=work, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        try:
            helper = wait_helper(base)
            print("healthz github_helper:", json.dumps(helper))
            check(helper.get("status") == "missing" and CAUSE in str(helper.get("detail")),
                  "healthz reports the helper missing with the layer-named cause")
            check(helper.get("fallback") is None, "no fallback is claimed while off")
            reply_status, reply_body = call(
                base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                {"text": f"Private: {DOC_URL}"})
            print("seed reply:", reply_status, json.dumps(reply_body)[:200])
            if reply_status != 200:
                raise AssertionError(f"the seed reply was refused: {reply_status}")
            print("ok: the seed reply posted")
            docs: list[object] = []
            deadline = time.time() + 30
            while time.time() < deadline:
                call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=10&render=html",
                     WRITE_TOKEN)
                _, answer = call(base, "GET", "/api/v1/documents?limit=10", READ_TOKEN)
                found = answer.get("documents")
                docs = found if isinstance(found, list) else []
                if len(docs) == 1:
                    break
                time.sleep(0.5)
            check(len(docs) == 1, f"the fold registered {len(docs)} documents, not 1: {docs}")
            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                for label, width, height, touch in (
                    ("phone", 412, 915, True),
                    ("desktop", 1280, 800, False),
                ):
                    for scheme in ("light", "dark"):
                        run_context(browser, base, shots, label, width, height, touch, scheme)
                browser.close()
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()
    print("GHHELPER BROWSER OK")
    return 0


if __name__ == "__main__":
    sys.exit(main())
