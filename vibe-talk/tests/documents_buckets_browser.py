"""The Documents kind buckets against the REAL server (`#1176 doc-kind-buckets`).

Seeds one document of each kind (GitHub markdown, a minibeads issue, a gist,
a paste) through the reply route, then walks the Documents view at 393x852:
every row lists, the bucket filter shows one type at a time (the gist sits
with the paste), the issue card names its number and (once fetched from the
local raw stub) its status, and opening the issue renders its front matter
as a header — `#<n>`, title, status — not raw YAML. The reader's header bar
names each opened document's kind (`#1327 doc-reader-kind-header`): the
minibeads issue with number, status and repository, the guide as GitHub
markdown under its repository, and the paste as a paste. The gist's API
fetch is not stubbed (its host is fixed), so only its listing and bucket
are walked.
"""

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

from playwright.sync_api import sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "buckets-check-read-token-not-a-real-credential"
WRITE_TOKEN = "buckets-check-write-token-not-a-real-credential"

GUIDE_BODY = "# Guide\n\nA plain markdown document.\n"
ISSUE_BODY = (
    "---\n"
    'title: "Buckets walk issue"\n'
    "status: in_progress\n"
    "---\n\n"
    "# Buckets walk issue\n\nThe issue body.\n"
)
RAW_FILES: dict[str, str] = {
    "/octo/widgets/main/docs/guide.md": GUIDE_BODY,
    "/octo/widgets/main/.minibeads/issues/agent-utils-1176.md": ISSUE_BODY,
}


class RawHandler(BaseHTTPRequestHandler):
    """A raw-content host: the seeded files, else 404."""

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
        body = RAW_FILES.get(self.path.split("?", 1)[0])
        if body is None:
            self.reply(404, b"not found")
        else:
            self.reply(200, body.encode())

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


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    raw = ThreadingHTTPServer(("127.0.0.1", 0), RawHandler)
    threading.Thread(target=raw.serve_forever, daemon=True).start()
    raw_base = f"http://127.0.0.1:{raw.server_address[1]}"
    with tempfile.TemporaryDirectory(prefix="buckets-check-") as tmp:
        work = Path(tmp)
        (work / "demo-doc").write_text("# Paste note\n\nA paste body.\n")
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "buckets-check-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
            "[documents]\n"
            'paste_url_prefixes = ["https://paste.example/"]\n'
            'paste_command = ["/bin/cat", "--", "{}"]\n'
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
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                 {"text": "Docs: https://paste.example/demo-doc and "
                          "[guide](https://github.com/octo/widgets/blob/main/docs/guide.md) and "
                          "https://github.com/octo/widgets/blob/main/.minibeads/issues/agent-utils-1176.md and "
                          "https://gist.github.com/octo/0123456789abcdef0123456789abcdef"})
            docs: list[dict[str, object]] = []
            deadline = time.time() + 30
            while time.time() < deadline:
                call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=10&render=html", WRITE_TOKEN)
                answer = call(base, "GET", "/api/v1/documents?limit=10", READ_TOKEN)
                docs = cast(list[dict[str, object]], answer["documents"])
                if len(docs) == 4:
                    break
                time.sleep(0.5)
            check(len(docs) == 4, f"the fold registered {len(docs)} documents, not 4: {docs}")
            kinds = {d["kind"] for d in docs}
            check(kinds == {"paste", "github_markdown", "github_issue", "github_gist"},
                  f"unexpected kinds: {kinds}")
            issue = next(d for d in docs if d["kind"] == "github_issue")
            check(issue["issue_number"] == 1176, f"the issue row names no number: {issue}")

            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                ctx = browser.new_context(viewport={"width": 393, "height": 852}, has_touch=True, is_mobile=True)
                page = ctx.new_page()
                page.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                page.goto(base + "/?v=docs", wait_until="load")
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                check(page.locator("#documents-list .doc-card").count() == 4, "the list did not draw every row")
                listing = page.locator("#documents-list").text_content() or ""
                check("#1176" in listing, "the issue card names no number")

                def bucket_keys(bucket: str) -> list[str]:
                    page.locator(f".doc-bucket[data-bucket='{bucket}']").click()
                    page.wait_for_timeout(300)
                    return cast(list[str], page.locator("#documents-list .doc-card").evaluate_all(
                        "(els) => els.map((e) => e.getAttribute('data-key'))"))

                check(bucket_keys("issues") == ["github:octo/widgets@main:.minibeads/issues/agent-utils-1176.md"],
                      "the Issues bucket does not hold only the issue")
                pastes = bucket_keys("pastes")
                check(sorted(pastes) == ["gist:0123456789abcdef0123456789abcdef", "paste:demo-doc"],
                      f"the Pastes bucket holds {pastes}, not the gist and the paste")
                check(len(bucket_keys("markdown")) == 1, "the Markdown bucket does not hold only the guide")
                check(len(bucket_keys("all")) == 4, "All does not restore every row")

                # Open the issue: it fetches from the raw stub, and its front
                # matter renders as a header, not raw YAML.
                page.locator(".doc-bucket[data-bucket='issues']").click()
                page.locator("#documents-list .doc-card").first.click()
                page.wait_for_selector("#doc-body .doc-issue-header", timeout=20_000)
                body_text = page.locator("#doc-body").text_content() or ""
                check("in_progress" in body_text, "the issue header names no status")
                check("status:" not in body_text, "the reader shows raw front matter")
                check("The issue body." in body_text, "the reader lost the issue body")

                # The header bar names the kind (`#1327 doc-reader-kind-header`):
                # a minibeads issue with its number, status and repository —
                # never "GitHub issue" — and the guide and the paste theirs.
                def kind_is(text: str) -> None:
                    page.wait_for_function(
                        "(wanted) => { const el = document.getElementById('doc-kind');"
                        " return el !== null && !el.hidden && el.textContent === wanted; }",
                        arg=text, timeout=20_000)

                kind_is("Minibeads issue #1176 · in_progress · octo/widgets")
                page.locator("#doc-back").click()
                page.wait_for_function(
                    "() => !document.getElementById('docs-list-view').hidden",
                    timeout=20_000)
                page.locator(".doc-bucket[data-bucket='all']").click()
                page.locator(
                    "#documents-list .doc-card"
                    "[data-key='github:octo/widgets@main:docs/guide.md']").click()
                kind_is("GitHub markdown · octo/widgets")
                page.locator("#doc-back").click()
                page.wait_for_function(
                    "() => !document.getElementById('docs-list-view').hidden",
                    timeout=20_000)
                page.locator(
                    "#documents-list .doc-card[data-key='paste:demo-doc']").click()
                kind_is("Paste")
                browser.close()
            print("documents buckets browser walk: ok")
            return 0
        finally:
            server.terminate()
            try:
                server.wait(timeout=10)
            except subprocess.TimeoutExpired:
                server.kill()


if __name__ == "__main__":
    sys.exit(main())
