"""The Documents view against the REAL server (`documents-view-client` owner-bug check).

Starts the release binary with --fake-discord (a real server; only the chat
upstream is fake), seeds a paste link and GitHub links through the reply
route, reads the channel so the fold registers them, then drives the page at
393x852 with real touch: an unfetched row fetches (the paste command serves
a local file) and opens the reader; Fetch recent reports a result. The
GitHub raw base points at a local stub HTTP server the test starts, serving
an index document whose links are relative (`#1081 documents-relative-links`):
a tap on one opens the linked document in the reader though no message ever
mentioned it, a ctrl+click leaves for GitHub, a non-document link still
leaves the app, and a GitHub row the stub answers with an error shows that
error on the row. No documents API is stubbed: list, fetch and render all go
through the server under test.

The reader steps also cover `#1038 documents-outline-popup`: the seeded paste
is long enough to scroll several viewports, and at 393x852 (touch) and again
at 1280x800 (mouse) the Outline button opens the pop-up, an entry tap/click
jumps to its heading, the HERE mark follows scrolling, and the title links
to the document's original source in a new tab.

The Fetch since… steps cover `#1308 doc-fetch-since-prompt`: scrolled to
the bottom at both sizes, the prompt card is fixed inside the viewport and
focused, Cancel fetches nothing, a 0d/0h confirmation matches nothing, and
a confirmation spanning the seeded mentions fetches them.

The last steps cover `#1265 documents-star-hide`: starring from a card and
the reader header, persistence across reload and through the server's
synced settings to a fresh device, the Starred chip, a real touch swipe to
hide with its Undo chip, the hamburger's Show hidden with dimmed cards and
Unhide, and the desktop Hide button. At phone and desktop sizes they also
check `#1306 doc-star-gold`: the starred star is visibly different — its
glyph fills and its computed colour is neither its own background (the old
accent-blue-on-accent-blue blank button) nor the unstarred colour.
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

from playwright.sync_api import Locator, Page, Route, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "documents-check-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-check-write-token-not-a-real-credential"
_FILLER = (
    "A filler paragraph long enough to take a few lines on a phone, so the "
    "document below scrolls for several viewports and each heading can be "
    "brought to the top of the scroller in turn.\n\n"
)
# The wrapped items are `#1235 documents-list-continuation`'s shape: an item over
# source lines, its tail 2-space indented, must draw as one item.
PASTE_BODY = (
    "# Demo deployment report\n\nThe **nightly** run finished green.\n\n"
    "- Live frames arrive whenever the network\n"
    "  says so and redraw at once.\n"
    "- Timeline reads land in batches and\n"
    "  settle the list afterwards.\n"
    "- Viewport events ride along.\n"
    "\n## Setup\n\n" + _FILLER * 8
    + "### Setup details\n\n" + _FILLER * 8
    + "## Finish\n\n" + _FILLER * 10
    + "The final line.\n"
)
OUTLINE_TEXTS = ["Demo deployment report", "1 Setup", "1.1 Setup details", "2 Finish"]

# The GitHub-shaped documents the local raw stub serves (`#1081`): an index
# whose links are relative, and the document they name. The linked document
# is never mentioned in chat, so only the reader's fetch-by-key admission can
# open it.
GUIDE_BODY = (
    "# Guide index\n\nStart with [the next document](next.md), or jump to "
    "[its second part](next.md#part-two).\n\nElsewhere: "
    "[an example](https://example.com/elsewhere).\n"
)
NEXT_BODY = (
    "# Next document\n\nOpened from the guide.\n\n" + _FILLER * 6
    + "## Part two\n\nThe anchored part.\n"
)
RAW_FILES: dict[str, str] = {
    "/octo/widgets/main/docs/guide.md": GUIDE_BODY,
    "/octo/widgets/main/docs/next.md": NEXT_BODY,
}
NEXT_KEY = "github:octo/widgets@main:docs/next.md"
NEXT_URL = "https://github.com/octo/widgets/blob/main/docs/next.md"


class RawHandler(BaseHTTPRequestHandler):
    """A raw-content host: the seeded files, an error for broken.md, else 404."""

    def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
        path = self.path.split("?", 1)[0]
        body = RAW_FILES.get(path)
        if body is not None:
            self.reply(200, body.encode())
        elif path.endswith("/docs/broken.md"):
            self.reply(500, b"broken upstream")
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


def wait_heading_at_top(page: Page, heading_id: str, label: str) -> None:
    """The heading sits in the scroller's top band and its outline entry is HERE."""
    page.wait_for_function(
        """([hid, label]) => {
          const h = document.getElementById(hid);
          const root = document.getElementById('scroll-area');
          const b = [...document.querySelectorAll('#doc-outline button')]
            .find((x) => x.textContent.trim() === label);
          return !!h && !!root && !!b
            && Math.abs(h.getBoundingClientRect().top - root.getBoundingClientRect().top) < 80
            && b.getAttribute('aria-current') === 'true';
        }""",
        arg=[heading_id, label],
        timeout=20_000,
    )


def wait_here_is(page: Page, label: str) -> None:
    """The named outline entry, and only it, carries the HERE mark."""
    page.wait_for_function(
        """(label) => {
          const marked = [...document.querySelectorAll('#doc-outline button')]
            .filter((x) => x.getAttribute('aria-current') === 'true');
          return marked.length === 1 && marked[0].textContent.trim() === label;
        }""",
        arg=label,
        timeout=20_000,
    )


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

def tap_card(page: object, card: object) -> None:
    """Tap a card's centre, waiting for it to be laid out first."""
    card.wait_for(state="visible", timeout=20_000)  # type: ignore[attr-defined]
    box = None
    for _ in range(50):
        box = card.bounding_box()  # type: ignore[attr-defined]
        if box is not None:
            break
        page.wait_for_timeout(200)  # type: ignore[attr-defined]
    assert box is not None
    page.touchscreen.tap(box["x"] + box["width"] / 2, box["y"] + box["height"] / 2)  # type: ignore[attr-defined]


def star_appearance(star: Locator) -> dict[str, str]:
    """One star button's glyph, pressed state, and computed ink and ground."""
    found = star.evaluate(
        """(el) => {
          const s = getComputedStyle(el);
          return {text: el.textContent, pressed: el.getAttribute('aria-pressed'),
                  color: s.color, background: s.backgroundColor};
        }""")
    return cast(dict[str, str], found)


def check_star_gold(label: str, off: dict[str, str], on: dict[str, str]) -> None:
    """A starred star reads as one (`#1306 doc-star-gold`): the glyph fills,
    and each state's ink differs from its own ground — on the old CSS the
    pressed colour was the generic button's accent ground itself, so the
    starred check here failed with the two equal."""
    check(off["text"] == "☆" and off["pressed"] == "false",
          f"the {label} is not an unpressed outline star: {off}")
    check(on["text"] == "★" and on["pressed"] == "true",
          f"the {label} is not a pressed filled star: {on}")
    check(off["color"] != off["background"],
          f"the unstarred {label} is its own background colour: {off}")
    check(on["color"] != on["background"],
          f"the starred {label} is its own background colour (a blank button): {on}")
    check(on["color"] != off["color"],
          f"the {label}'s starred and unstarred colours are the same: off {off}, on {on}")


def check_fetch_since_prompt(page: Page, width: int, height: int, use_touch: bool) -> None:
    """Fetch since… presents its since-when prompt in the viewport
    (`#1308 doc-fetch-since-prompt`): with the list scrolled to its newest
    (bottom) end the card hangs on screen above the bar, asks since when,
    holds the focus, and Cancel puts it away."""
    press = page.tap if use_touch else page.click
    page.evaluate(
        "() => { const root = document.getElementById('scroll-area');"
        " root.scrollTop = root.scrollHeight; }")
    press("#docs-fetch")
    page.wait_for_selector("#docs-fetch-menu", state="visible", timeout=20_000)
    press("#docs-fetch-since")
    page.wait_for_selector("#docs-since", state="visible", timeout=20_000)
    check("Fetch documents mentioned since" in (page.locator("#docs-since-title").text_content() or ""),
          "the prompt does not ask since when")
    position = page.evaluate(
        "() => getComputedStyle(document.getElementById('docs-since')).position")
    check(position == "absolute", f"the prompt is not anchored above the bar (position: {position})")
    box = page.locator("#docs-since").bounding_box()
    check(box is not None and box["x"] >= 0 and box["y"] >= 0
          and box["x"] + box["width"] <= width and box["y"] + box["height"] <= height,
          f"the prompt is not inside the {width}x{height} viewport: {box}")
    page.wait_for_function(
        "() => document.activeElement && document.activeElement.id === 'docs-since-days'",
        timeout=20_000)
    press("#docs-since-cancel")
    page.wait_for_selector("#docs-since", state="hidden", timeout=20_000)


def check_doc_search(page: Page, width: int, height: int) -> None:
    """Fetched-documents search (`#1266 documents-search`): a result, the
    stated reach, and the reader opened at the highlighted match."""
    page.click("#docs-search-toggle")
    page.wait_for_selector("#docs-search-field", state="visible", timeout=20_000)
    page.fill("#docs-search-field", "final")
    page.click("#docs-search-go")
    page.wait_for_selector(".doc-search-hit", timeout=20_000)
    note = page.locator("#docs-search-note").text_content() or ""
    check("match" in note and "fetched document" in note and "searched" in note,
          f"the search note does not state its reach: {note!r}")
    page.click(".doc-search-hit")
    page.wait_for_selector("#doc-body mark.search-hit", timeout=20_000)
    marks = page.locator("#doc-body mark.search-hit")
    check("final" in (marks.first.text_content() or "").lower(),
          "the reader's first mark is not the searched word")
    box = marks.first.bounding_box()
    check(box is not None and 0 <= box["y"] and box["y"] + box["height"] <= height,
          f"the highlighted match is not in the {width}x{height} viewport: {box}")
    check("Demo deployment report" in (page.locator("#doc-body").text_content() or ""),
          "the search result opened the wrong document")


def main() -> int:
    port = free_port()
    base = f"http://127.0.0.1:{port}"
    raw = ThreadingHTTPServer(("127.0.0.1", 0), RawHandler)
    threading.Thread(target=raw.serve_forever, daemon=True).start()
    raw_base = f"http://127.0.0.1:{raw.server_address[1]}"
    with tempfile.TemporaryDirectory(prefix="documents-check-") as tmp:
        work = Path(tmp)
        (work / "demo-doc").write_text(PASTE_BODY)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "documents-check-bot-token-never-sent"\n'
            f'[auth]\nread_token = "{READ_TOKEN}"\nwrite_token = "{WRITE_TOKEN}"\n'
            f'[[channels]]\nid = "{CHANNEL}"\nlabel = "lead team"\nwritable = true\n'
            f'[storage]\npath = "{work}/store.sqlite3"\n'
            f'[message_cache]\nenabled = true\npath = "{work}/cache.sqlite3"\n'
            '[documents]\n'
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
                 {"text": "Report: https://paste.example/demo-doc and the "
                          "[guide](https://github.com/octo/widgets/blob/main/docs/guide.md) "
                          "plus https://github.com/octo/widgets/blob/main/docs/broken.md"})
            docs = []
            deadline = time.time() + 30
            while time.time() < deadline:
                call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=10&render=html", WRITE_TOKEN)
                answer = call(base, "GET", "/api/v1/documents?limit=10", READ_TOKEN)
                docs = cast(list[dict[str, object]], answer["documents"])
                if len(docs) == 3:
                    break
                time.sleep(0.5)
            check(len(docs) == 3, f"the fold registered {len(docs)} documents, not 3: {docs}")
            by_file = {d["file_name"]: d for d in docs}
            check(by_file["demo-doc"]["fetched"] is False, "the paste starts fetched")

            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                ctx = browser.new_context(viewport={"width": 393, "height": 852}, has_touch=True, is_mobile=True)
                page = ctx.new_page()
                page.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                page.goto(base + "/?v=docs", wait_until="load")
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                check(page.locator("#documents-list .doc-card").count() == 3, "the list did not draw every row")
                check("demo-doc" in (page.locator("#documents-list").text_content() or ""),
                      "the list does not name its document")

                # Fetch since… (`#1308`): the prompt is in view at the
                # bottom of the list; Cancel fetched nothing (every row
                # still unfetched below). Confirming 0d/0h matches nothing;
                # confirming a span covering the seeded mentions (the fake
                # message's snowflake dates them years back, so the span is
                # measured from their own mention time) fetches the paste
                # and the guide, while the broken row's fetch fails alone.
                check_fetch_since_prompt(page, 393, 852, use_touch=True)
                still = {d["file_name"]: d for d in cast(
                    list[dict[str, object]],
                    call(base, "GET", "/api/v1/documents?limit=10", READ_TOKEN)["documents"])}
                check(all(d["fetched"] is False for d in still.values()),
                      f"cancelling the since prompt fetched a document: {still}")
                mentioned_ms = by_file["demo-doc"]["last_mentioned_ms"]
                assert isinstance(mentioned_ms, (int, float))
                cover_days = str(int((time.time() * 1000 - mentioned_ms) // 86_400_000) + 1)
                for days, hours in (("0", "0"), (cover_days, "0")):
                    page.tap("#docs-fetch")
                    page.wait_for_selector("#docs-fetch-menu", state="visible", timeout=20_000)
                    page.tap("#docs-fetch-since")
                    page.wait_for_selector("#docs-since", state="visible", timeout=20_000)
                    page.fill("#docs-since-days", days)
                    page.fill("#docs-since-hours", hours)
                    page.tap("#docs-since-go")
                    page.wait_for_selector("#docs-since", state="hidden", timeout=20_000)
                    page.wait_for_function(
                        "() => !document.getElementById('docs-fetch').disabled", timeout=20_000)
                    landed = {d["file_name"]: d for d in cast(
                        list[dict[str, object]],
                        call(base, "GET", "/api/v1/documents?limit=10", READ_TOKEN)["documents"])}
                    if days == "0":
                        check(all(d["fetched"] is False for d in landed.values()),
                              f"a 0d/0h since fetch fetched a document: {landed}")
                    else:
                        check(landed["demo-doc"]["fetched"] is True
                              and landed["guide.md"]["fetched"] is True
                              and landed["broken.md"]["fetched"] is False,
                              f"the covering since fetch landed the wrong documents: {landed}")

                # The bar's Fetch drop-up: Fetch visible, with a result said.
                page.tap("#docs-fetch")
                page.wait_for_selector("#docs-fetch-menu", state="visible", timeout=20_000)
                page.tap("#docs-fetch-menu >> text=Fetch visible")
                page.wait_for_function(
                    "() => !document.getElementById('docs-fetch').disabled", timeout=20_000)

                # The paste row: a real touch fetches it from the server and opens the reader.
                paste_card = page.locator("#documents-list .doc-card", has_text="demo-doc")
                tap_card(page, paste_card)
                page.wait_for_selector("#doc-body h1", timeout=20_000)
                check("Demo deployment report" in (page.text_content("#doc-body") or ""),
                      "the reader did not render the fetched document")
                # Wrapped items stay whole (`#1235 documents-list-continuation`): one
                # list, one item per source item, each tail inside its item, and no
                # spilled paragraph carrying a tail at the left margin.
                wrapped = page.evaluate(
                    """() => {
                      const body = document.getElementById('doc-body');
                      return {
                        lists: body.querySelectorAll('ul').length,
                        items: [...body.querySelectorAll('li')].map((li) => li.textContent),
                        spills: [...body.querySelectorAll('p')]
                          .filter((p) => /^(says so|settle the list)/.test(p.textContent || ''))
                          .length,
                      };
                    }""")
                check(wrapped["lists"] == 1, f"the wrapped list drew as {wrapped['lists']} lists")
                check(wrapped["items"] == [
                    "Live frames arrive whenever the network says so and redraw at once.",
                    "Timeline reads land in batches and settle the list afterwards.",
                    "Viewport events ride along.",
                ], f"the wrapped items drew as {wrapped['items']}")
                check(wrapped["spills"] == 0, "a wrapped tail spilled as its own paragraph")
                # `#1515 page-update-reload-loses-controls`: the reader's
                # transport pane stands over the reader in every state.
                check(page.locator("#control-pane").is_visible(), "the reader's controls do not stand over the reader")

                # The title is the source link (#1038): href, new tab, no opener.
                title_link = page.locator("#doc-title a")
                check(title_link.count() == 1, "the reader title is not a link to the source")
                check(title_link.get_attribute("href") == "https://paste.example/demo-doc",
                      "the title link does not carry the document's original URL")
                check(title_link.get_attribute("target") == "_blank", "the title link opens in place")
                check("noopener" in (title_link.get_attribute("rel") or ""),
                      "the title link keeps an opener")

                # The old top-of-reader TOC is gone: the outline lives in its pop-up,
                # closed until the Outline button opens it.
                check(page.locator("#doc-outline-pop").is_hidden(), "the outline pop-up starts open")
                page.tap("#doc-outline-open")
                page.wait_for_function(
                    "() => !document.getElementById('doc-outline-pop').hidden", timeout=20_000)
                texts = [t.strip() for t in page.locator("#doc-outline button").all_text_contents()]
                check(texts == OUTLINE_TEXTS, f"the outline lists {texts}, not {OUTLINE_TEXTS}")
                indent = page.evaluate(
                    """() => {
                      const pads = [...document.querySelectorAll('#doc-outline button')]
                        .map((b) => parseFloat(getComputedStyle(b).paddingLeft));
                      return pads;
                    }""")
                check(indent[1] > indent[0] and indent[2] > indent[1] and indent[3] == indent[1],
                      f"outline indentation does not follow heading levels: {indent}")

                # A tap on a later entry jumps to its heading, closes the pop-up,
                # and that entry becomes HERE.
                page.locator("#doc-outline button", has_text="Finish").tap()
                wait_heading_at_top(page, "doc-finish", "2 Finish")
                check(page.locator("#doc-outline-pop").is_hidden(),
                      "picking an outline entry left the pop-up open")

                # Scrolling (here by scrollTop, as a swipe would move it) advances HERE.
                page.evaluate(
                    """() => {
                      const root = document.getElementById('scroll-area');
                      const h = document.getElementById('doc-setup');
                      root.scrollTop += h.getBoundingClientRect().top - root.getBoundingClientRect().top;
                    }""")
                wait_here_is(page, "1 Setup")

                # An outside tap closes the pop-up without moving the document.
                page.tap("#doc-outline-open")
                page.wait_for_function(
                    "() => !document.getElementById('doc-outline-pop').hidden", timeout=20_000)
                top_before = page.evaluate("() => document.getElementById('scroll-area').scrollTop")
                # Inside the scroller, clear of the dock: the reader's pane
                # stands there now (`#1515`), so a fixed phone coordinate
                # would land on its controls instead of the document.
                area_box = page.locator("#scroll-area").bounding_box()
                check(area_box is not None, "the scroller has no box to tap in")
                page.touchscreen.tap(area_box["x"] + 30, area_box["y"] + 40)
                page.wait_for_function(
                    "() => document.getElementById('doc-outline-pop').hidden", timeout=20_000)
                top_after = page.evaluate("() => document.getElementById('scroll-area').scrollTop")
                check(top_after == top_before, "closing the outline moved the document")

                # The title link opens the source in a new page. Its route is
                # aborted (never leaves the machine); the aborted request's URL,
                # or the popup's where the navigation commits first, is the check.
                source_requests: list[str] = []

                def abort_source(route: Route) -> None:
                    source_requests.append(route.request.url)
                    route.abort()

                ctx.route("https://paste.example/**", abort_source)
                with ctx.expect_page(timeout=20_000) as popup_info:
                    page.tap("#doc-title a")
                popup = popup_info.value
                deadline = time.time() + 10
                while not source_requests and popup.url == "about:blank" and time.time() < deadline:
                    time.sleep(0.1)
                check(source_requests == ["https://paste.example/demo-doc"]
                      or popup.url == "https://paste.example/demo-doc",
                      f"the title opened {popup.url!r} (requests: {source_requests}), "
                      "not the document source")
                popup.close()

                page.tap("#doc-back")
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)

                # Fetched-documents search at phone size (`#1266`).
                check_doc_search(page, 393, 852)
                page.tap("#doc-back")
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)

                # The system Back gesture: list -> document -> back to the list.
                tap_card(page, paste_card)
                page.wait_for_selector("#doc-body h1", timeout=20_000)
                page.go_back()
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                check(page.locator("#pane-docs").is_visible(), "Back from the reader left the Documents view")

                # A mention jumps to its message; Back returns to the list.
                mention = page.locator("#documents-list .doc-card", has_text="demo-doc").locator(".doc-mentions-link")
                mention.tap()
                page.wait_for_function("() => !document.getElementById('pane-discord').hidden", timeout=20_000)
                page.go_back()
                # Back from the jump returns to the Documents list it started from.
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                check(page.locator("#pane-docs").is_visible(), "Back from the mention jump left the Documents view")

                # A document link in a message opens the reader in-app (reply
                # 220), and Back returns to that channel and message, not to
                # the Documents list (#1082 documents-back-chain). A doc-to-doc
                # step joins this walk with #1081's in-reader links.
                page.tap("#view-switch")
                page.tap("#view-switch")
                page.wait_for_function("() => !document.getElementById('pane-discord').hidden", timeout=20_000)
                page.wait_for_selector("#discord-log a[data-doc]", timeout=20_000)
                page.tap("#discord-log a[data-doc]")
                page.wait_for_function("() => !document.getElementById('docs-reader').hidden", timeout=20_000)
                check(page.locator("#pane-docs").is_visible(), "the document link did not open the reader in-app")
                page.go_back()
                page.wait_for_function("() => !document.getElementById('pane-discord').hidden", timeout=20_000)
                check(page.locator("#pane-docs").is_hidden(),
                      "Back from a channel-opened document landed on the documents list")
                page.wait_for_function(
                    """() => {
                      const link = document.querySelector('#discord-log a[data-doc]');
                      const root = document.getElementById('scroll-area');
                      if (!link || !root) return false;
                      const at = link.getBoundingClientRect();
                      const within = root.getBoundingClientRect();
                      return at.bottom > within.top && at.top < within.bottom;
                    }""",
                    timeout=20_000)
                # On to the list again for the row checks below.
                page.tap("#view-switch")
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)

                # The index document (`#1081`): its relative links render as
                # in-app document links, and a tap opens the linked document
                # in the reader though no message ever mentioned it (the
                # reader's fetch admits it by key).
                guide_card = page.locator("#documents-list .doc-card", has_text="guide.md")
                tap_card(page, guide_card)
                page.wait_for_selector("#doc-body h1", timeout=20_000)
                check("Guide index" in (page.text_content("#doc-body") or ""),
                      "the reader did not render the guide")
                next_link = page.locator("#doc-body a", has_text="the next document")
                check(next_link.get_attribute("data-doc") == NEXT_KEY,
                      "the relative link carries no document key")
                check(next_link.get_attribute("href") == NEXT_URL,
                      "the relative link does not carry its GitHub address")
                anchored = page.locator("#doc-body a", has_text="its second part")
                check(anchored.get_attribute("data-doc-anchor") == "part-two",
                      "the anchored link carries no scroll fragment")
                external = page.locator("#doc-body a", has_text="an example")
                check(external.get_attribute("data-doc") is None,
                      "a non-document link was marked as a document")
                next_link.tap()
                page.wait_for_function(
                    "() => (document.getElementById('doc-body')?.textContent || '')"
                    ".includes('Next document')",
                    timeout=20_000)
                listed = cast(list[dict[str, object]],
                              call(base, "GET", "/api/v1/documents?limit=10", READ_TOKEN)["documents"])
                admitted = [d for d in listed if d["key"] == NEXT_KEY]
                check(len(admitted) == 1 and admitted[0]["mention_count"] == 0
                      and admitted[0]["fetched"] is True,
                      f"the linked document was not admitted mention-less and fetched: {listed}")
                # The back chain (#1082): one Back returns to the guide the
                # link was followed from; the next reaches the list.
                page.tap("#doc-back")
                page.wait_for_function(
                    "() => (document.getElementById('doc-body')?.textContent || '')"
                    ".includes('Guide index')",
                    timeout=20_000)
                page.tap("#doc-back")
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)

                # The broken GitHub row: the raw stub answers an error, and
                # the row says why instead of doing nothing.
                gh_card = page.locator("#documents-list .doc-card", has_text="broken.md")
                tap_card(page, gh_card)
                page.wait_for_function(
                    "() => [...document.querySelectorAll('#documents-list .doc-card')]"
                    ".some((c) => c.textContent.includes('Could not fetch it.'))",
                    timeout=20_000)
                ctx.close()

                # The same outline flow at desktop size with a mouse: open, jump,
                # HERE, and Escape from the keyboard inside the panel.
                dctx = browser.new_context(viewport={"width": 1280, "height": 800})
                dpage = dctx.new_page()
                dpage.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                dpage.goto(base + "/?v=docs", wait_until="load")
                dpage.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                # The since prompt at desktop size too (`#1308`).
                check_fetch_since_prompt(dpage, 1280, 800, use_touch=False)
                dpage.locator("#documents-list .doc-card", has_text="demo-doc").click()
                dpage.wait_for_selector("#doc-body h1", timeout=20_000)
                check(dpage.locator("#doc-title a").get_attribute("href")
                      == "https://paste.example/demo-doc",
                      "the desktop title does not link to the source")
                dpage.click("#doc-outline-open")
                dpage.wait_for_function(
                    "() => !document.getElementById('doc-outline-pop').hidden", timeout=20_000)
                dtexts = [t.strip() for t in dpage.locator("#doc-outline button").all_text_contents()]
                check(dtexts == OUTLINE_TEXTS, f"the desktop outline lists {dtexts}, not {OUTLINE_TEXTS}")
                dpage.locator("#doc-outline button", has_text="Setup details").click()
                wait_heading_at_top(dpage, "doc-setup-details", "1.1 Setup details")
                check(dpage.locator("#doc-outline-pop").is_hidden(),
                      "picking a desktop outline entry left the pop-up open")
                dpage.click("#doc-outline-open")
                dpage.wait_for_function(
                    "() => !document.getElementById('doc-outline-pop').hidden", timeout=20_000)
                dpage.keyboard.press("Escape")
                dpage.wait_for_function(
                    "() => document.getElementById('doc-outline-pop').hidden", timeout=20_000)

                # At desktop size (`#1081`): a ctrl+click on an in-document
                # link follows its GitHub address in a new page, and a plain
                # click on a non-document link still leaves the app. Both
                # destinations are fulfilled locally, never really fetched.
                dpage.click("#doc-back")
                dpage.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                github_requests: list[str] = []

                def abort_github(route: Route) -> None:
                    github_requests.append(route.request.url)
                    route.abort()

                dctx.route("https://github.com/**", abort_github)
                dctx.route("https://example.com/**",
                           lambda route: route.fulfill(status=200, body="example stub"))
                dpage.locator("#documents-list .doc-card", has_text="guide.md").click()
                dpage.wait_for_selector("#doc-body h1", timeout=20_000)
                with dctx.expect_page(timeout=20_000) as popup_info:
                    dpage.locator("#doc-body a", has_text="the next document") \
                        .click(modifiers=["Control"])
                popup = popup_info.value
                deadline = time.time() + 10
                while not github_requests and popup.url == "about:blank" and time.time() < deadline:
                    time.sleep(0.1)
                check(github_requests == [NEXT_URL] or popup.url == NEXT_URL,
                      f"the ctrl+click opened {popup.url!r} (requests: {github_requests}), "
                      "not the GitHub page")
                popup.close()
                check("Guide index" in (dpage.text_content("#doc-body") or ""),
                      "the ctrl+click moved the reader")
                dpage.locator("#doc-body a", has_text="an example").click()
                dpage.wait_for_url("https://example.com/elsewhere", timeout=20_000)
                dpage.go_back()
                # The app's own Back handling takes the return from there;
                # what matters is being back in the app, in Documents.
                dpage.wait_for_function(
                    "() => !document.getElementById('pane-docs').hidden", timeout=20_000)
                # Fetched-documents search at desktop size (`#1266`).
                dpage.goto(base + "/?v=docs", wait_until="load")
                dpage.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                check_doc_search(dpage, 1280, 800)
                dctx.close()

                # Star and hide (`#1265 documents-star-hide`) at phone size,
                # with a real touch swipe, and again on the desktop.
                sctx = browser.new_context(
                    viewport={"width": 393, "height": 852}, has_touch=True, is_mobile=True)
                spage = sctx.new_page()
                spage.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                spage.goto(base + "/?v=docs", wait_until="load")
                spage.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                paste_key = str(by_file["demo-doc"]["key"])

                # Star the paste document from its card; the star fills and
                # the state reaches the server's synced settings. Filled has
                # to be visible (`#1306 doc-star-gold`): the gold check reads
                # the card star's appearance on both sides of the tap.
                paste_card = spage.locator("#documents-list .doc-card", has_text="demo-doc")
                phone_card_off = star_appearance(paste_card.locator(".doc-star"))
                paste_card.locator(".doc-star").tap()
                spage.wait_for_function(
                    "() => [...document.querySelectorAll('#documents-list .doc-card')]"
                    ".some((c) => c.textContent.includes('demo-doc')"
                    " && c.querySelector('.doc-star')?.getAttribute('aria-pressed') === 'true')",
                    timeout=20_000)
                check_star_gold("phone card star", phone_card_off,
                                star_appearance(paste_card.locator(".doc-star")))
                deadline = time.time() + 30
                held_states: dict[str, object] = {}
                while time.time() < deadline:
                    settings = call(base, "GET", "/api/v1/client-settings", READ_TOKEN)
                    for entry in cast(list[dict[str, object]], settings["settings"]):
                        if entry["key"] == "vibe-talk.voice.doc-states" and entry["value"]:
                            held_states = json.loads(str(entry["value"]))
                    if held_states.get(paste_key) == {"s": 1}:
                        break
                    time.sleep(0.5)
                check(held_states.get(paste_key) == {"s": 1},
                      f"the star did not reach the server state: {held_states}")

                # The Starred chip shows only it; a reload keeps the star.
                spage.locator(".doc-bucket[data-bucket='starred']").tap()
                spage.wait_for_function(
                    "() => document.querySelectorAll('#documents-list .doc-card').length === 1",
                    timeout=20_000)
                spage.reload(wait_until="load")
                # Four by now: the linked next.md was admitted in the #1081 walk.
                spage.wait_for_function(
                    "() => document.querySelectorAll('#documents-list .doc-card').length >= 3",
                    timeout=20_000)
                starred = spage.locator(
                    "#documents-list .doc-card", has_text="demo-doc").locator(".doc-star")
                check(starred.get_attribute("aria-pressed") == "true",
                      "the star did not persist across reload")

                # The reader header carries the same star, visibly: read it
                # starred, tap it off and read the outline, then tap the
                # star back on for the steps below (`#1306`).
                tap_card(spage, spage.locator("#documents-list .doc-card", has_text="demo-doc"))
                spage.wait_for_selector("#doc-body h1", timeout=20_000)
                check(spage.locator("#doc-star").get_attribute("aria-pressed") == "true",
                      "the reader header does not show the card's star")
                phone_header_on = star_appearance(spage.locator("#doc-star"))
                spage.tap("#doc-star")
                spage.wait_for_function(
                    "() => document.getElementById('doc-star')"
                    "?.getAttribute('aria-pressed') === 'false'",
                    timeout=20_000)
                check_star_gold("phone reader star",
                                star_appearance(spage.locator("#doc-star")), phone_header_on)
                spage.tap("#doc-star")
                spage.wait_for_function(
                    "() => document.getElementById('doc-star')"
                    "?.getAttribute('aria-pressed') === 'true'",
                    timeout=20_000)
                spage.tap("#doc-back")
                spage.wait_for_selector("#documents-list .doc-card", timeout=20_000)

                # A real swipe hides the guide card; Undo brings it back.
                guide_card = spage.locator("#documents-list .doc-card", has_text="guide.md")
                guide_card.scroll_into_view_if_needed()
                box = guide_card.bounding_box()
                check(box is not None, "the guide card has no box to swipe")
                assert box is not None
                cdp = sctx.new_cdp_session(spage)
                sx, sy = box["x"] + 60, box["y"] + box["height"] / 2
                cdp.send("Input.dispatchTouchEvent",
                         {"type": "touchStart", "touchPoints": [{"x": sx, "y": sy}]})
                for step in range(1, 7):
                    cdp.send("Input.dispatchTouchEvent",
                             {"type": "touchMove",
                              "touchPoints": [{"x": sx + 24 * step, "y": sy}]})
                cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
                spage.wait_for_function(
                    "() => ![...document.querySelectorAll('#documents-list .doc-card')]"
                    ".some((c) => c.textContent.includes('guide.md'))",
                    timeout=20_000)
                spage.locator("#docs-undo").click()
                spage.wait_for_function(
                    "() => [...document.querySelectorAll('#documents-list .doc-card')]"
                    ".some((c) => c.textContent.includes('guide.md'))",
                    timeout=20_000)

                # Swipe again, then the hamburger's Show hidden shows it
                # dimmed, and Unhide restores it for good.
                guide_card = spage.locator("#documents-list .doc-card", has_text="guide.md")
                guide_card.scroll_into_view_if_needed()
                box = guide_card.bounding_box()
                check(box is not None, "the guide card has no box to swipe again")
                assert box is not None
                sx, sy = box["x"] + 60, box["y"] + box["height"] / 2
                cdp.send("Input.dispatchTouchEvent",
                         {"type": "touchStart", "touchPoints": [{"x": sx, "y": sy}]})
                for step in range(1, 7):
                    cdp.send("Input.dispatchTouchEvent",
                             {"type": "touchMove",
                              "touchPoints": [{"x": sx + 24 * step, "y": sy}]})
                cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
                spage.wait_for_function(
                    "() => ![...document.querySelectorAll('#documents-list .doc-card')]"
                    ".some((c) => c.textContent.includes('guide.md'))",
                    timeout=20_000)
                spage.locator("#action-menu-open").click()
                spage.wait_for_selector("#docs-show-hidden", state="visible", timeout=20_000)
                spage.locator("#docs-show-hidden").click()
                dimmed = spage.locator("#documents-list .doc-card.doc-hidden", has_text="guide.md")
                dimmed.wait_for(state="visible", timeout=20_000)
                opacity = dimmed.evaluate("(el) => getComputedStyle(el).opacity")
                check(float(opacity) < 0.9, f"the shown hidden card is not dimmed (opacity {opacity})")
                dimmed.locator(".doc-unhide").click()
                spage.wait_for_function(
                    "() => ![...document.querySelectorAll('#documents-list .doc-card')]"
                    ".some((c) => c.classList.contains('doc-hidden'))",
                    timeout=20_000)
                sctx.close()

                # A fresh device (empty storage) converges from the server:
                # the paste is starred, and nothing is hidden any more.
                fctx = browser.new_context(viewport={"width": 393, "height": 852})
                fpage = fctx.new_page()
                fpage.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                fpage.goto(base + "/?v=docs", wait_until="load")
                fpage.wait_for_function(
                    "() => [...document.querySelectorAll('#documents-list .doc-card')]"
                    ".some((c) => c.textContent.includes('demo-doc')"
                    " && c.querySelector('.doc-star')?.getAttribute('aria-pressed') === 'true')",
                    timeout=30_000)
                fctx.close()

                # Desktop: the Hide button hides, Show hidden and Unhide undo it.
                hctx = browser.new_context(viewport={"width": 1280, "height": 800})
                hpage = hctx.new_page()
                hpage.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                hpage.goto(base + "/?v=docs", wait_until="load")
                hpage.wait_for_selector("#documents-list .doc-card", timeout=20_000)

                # The gold star at desktop size (`#1306 doc-star-gold`), on
                # the guide's card and then in its reader header, restored
                # to unstarred for the Hide walk below. Convergence first —
                # the paste's star arriving from the server — so the guide's
                # toggle is not racing this fresh device's first sync.
                hpage.wait_for_function(
                    "() => [...document.querySelectorAll('#documents-list .doc-card')]"
                    ".some((c) => c.textContent.includes('demo-doc')"
                    " && c.querySelector('.doc-star')?.getAttribute('aria-pressed') === 'true')",
                    timeout=30_000)
                guide_star = hpage.locator(
                    "#documents-list .doc-card", has_text="guide.md").locator(".doc-star")
                desk_card_off = star_appearance(guide_star)
                guide_star.click()
                hpage.wait_for_function(
                    "() => [...document.querySelectorAll('#documents-list .doc-card')]"
                    ".some((c) => c.textContent.includes('guide.md')"
                    " && c.querySelector('.doc-star')?.getAttribute('aria-pressed') === 'true')",
                    timeout=20_000)
                check_star_gold("desktop card star", desk_card_off, star_appearance(guide_star))
                hpage.locator("#documents-list .doc-card", has_text="guide.md").click()
                hpage.wait_for_selector("#doc-body h1", timeout=20_000)
                hpage.wait_for_function(
                    "() => document.getElementById('doc-star')"
                    "?.getAttribute('aria-pressed') === 'true'",
                    timeout=20_000)
                desk_header_on = star_appearance(hpage.locator("#doc-star"))
                hpage.click("#doc-star")
                hpage.wait_for_function(
                    "() => document.getElementById('doc-star')"
                    "?.getAttribute('aria-pressed') === 'false'",
                    timeout=20_000)
                check_star_gold("desktop reader star",
                                star_appearance(hpage.locator("#doc-star")), desk_header_on)
                hpage.click("#doc-back")
                hpage.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                hpage.locator("#documents-list .doc-card", has_text="guide.md") \
                    .locator(".doc-hide").click()
                hpage.wait_for_function(
                    "() => ![...document.querySelectorAll('#documents-list .doc-card')]"
                    ".some((c) => c.textContent.includes('guide.md'))",
                    timeout=20_000)
                hpage.click("#action-menu-open")
                hpage.wait_for_selector("#docs-show-hidden", state="visible", timeout=20_000)
                hpage.click("#docs-show-hidden")
                hdim = hpage.locator("#documents-list .doc-card.doc-hidden", has_text="guide.md")
                hdim.wait_for(state="visible", timeout=20_000)
                hdim.locator(".doc-unhide").click()
                hpage.wait_for_function(
                    "() => [...document.querySelectorAll('#documents-list .doc-card')]"
                    ".some((c) => c.textContent.includes('guide.md')"
                    " && !c.classList.contains('doc-hidden'))",
                    timeout=20_000)
                hctx.close()
                browser.close()
            print("documents browser check: real-server list, touch fetch, reader, outline pop-up, "
                  "source link, fetch error, and in-document links all seen")
            return 0
        finally:
            server.terminate()
            server.wait(timeout=10)
            raw.shutdown()


if __name__ == "__main__":
    sys.exit(main())
