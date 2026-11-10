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

They also cover `#1236 documents-reader-layout`: the seeded paste carries
headings, a wrapped bullet list and a table, and with the reader open at
both sizes the column is bounded and centred on the desk and full-width on
the phone, the reading-width grip has no box over the text, table body
rows alternate grounds, `h1`/`h2` carry a bottom rule, and the hamburger
menu offers no "Expand all" in this view — while back in the channel,
where a seeded long message folds, the grip and the item are offered again.
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

from playwright.sync_api import Page, Route, sync_playwright

ROOT = Path(__file__).resolve().parent.parent
CHANNEL = "100000000000000001"
READ_TOKEN = "documents-check-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-check-write-token-not-a-real-credential"
_FILLER = (
    "A filler paragraph long enough to take a few lines on a phone, so the "
    "document below scrolls for several viewports and each heading can be "
    "brought to the top of the scroller in turn.\n\n"
)
# The reader-layout matter (`#1236 documents-reader-layout`): bullets long
# enough to wrap onto a second line of the reader, and a table with enough
# body rows to alternate grounds twice over.
_LAYOUT_LIST = (
    "- the first check ran the whole suite from a clean tree, and this line "
    "is long enough to wrap onto the next line of the reader at either size\n"
    "- the second check kept its items close together, so the list reads as "
    "one block with the air around it rather than inside it\n"
    "- the third check signed the run off\n"
)
_LAYOUT_TABLE = (
    "| Step | Result | Note |\n"
    "| --- | --- | --- |\n"
    "| Build | green | first body row |\n"
    "| Test | green | second body row |\n"
    "| Ship | green | third body row |\n"
    "| Watch | green | fourth body row |\n"
)
PASTE_BODY = (
    "# Demo deployment report\n\nThe **nightly** run finished green.\n\n"
    + _LAYOUT_LIST + "\n" + _LAYOUT_TABLE
    + "\n## Setup\n\n" + _FILLER * 8
    + "### Setup details\n\n" + _FILLER * 8
    + "## Finish\n\n" + _FILLER * 10
    + "The final line.\n"
)
# A channel message past the fold (`#1236`): in the channel it gives
# "Expand all" work to do, which is what makes its absence from the menu
# in the Documents view a check rather than the menu's resting state.
LONG_MESSAGE = (
    "A long status note for the fold: the nightly run rebuilt every target "
    "from a clean tree, ran each suite in turn, compared the results with "
    "the run before it, and wrote the whole account down where the next "
    "reader can find it without opening a single log by hand. Nothing in "
    "it is a link, so it registers no document."
)
OUTLINE_TEXTS = ["Demo deployment report", "Setup", "Setup details", "Finish"]

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


def check_reader_layout(page: Page, *, desktop: bool) -> None:
    """The open reader's layout at this size (`#1236 documents-reader-layout`).

    On the desk the column is bounded and centred in the scroller; on the
    phone it takes the screen. At both sizes the grip has no box here and
    no point over the text column hits it, table body rows alternate
    grounds, `h1`/`h2` carry their rule, and the list keeps its air around
    it rather than between its items.
    """
    layout = page.evaluate(
        """() => {
          const body = document.getElementById('doc-body');
          const area = document.getElementById('scroll-area');
          const grip = document.getElementById('width-grip');
          const r = body.getBoundingClientRect();
          const a = area.getBoundingClientRect();
          const rows = [...body.querySelectorAll('tbody tr')];
          let gripHit = false;
          for (const fx of [0.1, 0.3, 0.5, 0.7, 0.9]) {
            for (const fy of [0.3, 0.5, 0.7]) {
              const hit = document.elementFromPoint(
                r.left + r.width * fx, innerHeight * fy);
              if (hit === grip || (hit !== null && grip.contains(hit))) gripHit = true;
            }
          }
          const rule = (el) => {
            const s = getComputedStyle(el);
            return s.borderBottomStyle !== 'none'
              && parseFloat(s.borderBottomWidth) > 0;
          };
          return {
            bodyLeft: r.left, bodyRight: r.right, bodyWidth: r.width,
            areaLeft: a.left, areaClientWidth: area.clientWidth,
            innerWidth,
            gripRects: grip.getClientRects().length,
            gripDisplay: getComputedStyle(grip).display,
            gripHit,
            rowBgs: rows.map((row) => getComputedStyle(row).backgroundColor),
            h1Rule: rule(body.querySelector('h1')),
            h2Rule: rule(body.querySelector('h2')),
            listMarginTop: parseFloat(
              getComputedStyle(body.querySelector('ul')).marginTop),
            itemMarginTop: parseFloat(
              getComputedStyle(body.querySelector('li')).marginTop),
          };
        }""")
    if desktop:
        check(layout["bodyWidth"] < layout["innerWidth"] - 200
              and 480 < layout["bodyWidth"] < 1000,
              f"the desktop reader column is not bounded: {layout}")
        left_gap = layout["bodyLeft"] - layout["areaLeft"]
        right_gap = (layout["areaLeft"] + layout["areaClientWidth"]
                     - layout["bodyRight"])
        check(abs(left_gap - right_gap) <= 24,
              f"the desktop reader column is not centred: "
              f"left {left_gap}, right {right_gap}")
    else:
        check(layout["bodyWidth"] > layout["innerWidth"] - 64,
              f"the phone reader does not take the screen: {layout}")
    check(layout["gripRects"] == 0 and layout["gripDisplay"] == "none",
          f"the width grip has a box over the reader: {layout}")
    check(layout["gripHit"] is False,
          "a point over the text column hits the width grip")
    check(len(layout["rowBgs"]) >= 3
          and layout["rowBgs"][0] != layout["rowBgs"][1]
          and layout["rowBgs"][1] != layout["rowBgs"][2],
          f"table body rows do not alternate grounds: {layout['rowBgs']}")
    check(layout["h1Rule"] is True and layout["h2Rule"] is True,
          "h1/h2 carry no bottom rule in the reader")
    check(layout["listMarginTop"] >= 8 and layout["itemMarginTop"] <= 6,
          f"the list's air is not around it with dense items: {layout}")


def check_action_menu(page: Page, *, desktop: bool, expand_visible: bool) -> None:
    """Open the hamburger menu by real input and hold "Expand all" to account.

    Both the `hidden` state the menu counts and what is actually on screen
    must agree (`#1236 documents-reader-layout`); the menu is closed again
    by the same input that opened it.
    """
    opener = page.locator("#action-menu-open")
    opener.wait_for(state="visible", timeout=20_000)
    if desktop:
        opener.click()
    else:
        opener.tap()
    page.wait_for_selector("#action-menu", state="visible", timeout=20_000)
    hidden = page.evaluate(
        "() => document.getElementById('expand-all').hidden")
    check(hidden is (not expand_visible),
          f"Expand all's hidden is {hidden}, "
          f"not {not expand_visible}, with the menu open here")
    check(page.locator("#expand-all").is_visible() is expand_visible,
          f"the open menu's Expand all is not "
          f"{'shown' if expand_visible else 'away'} as it should be here")
    if desktop:
        opener.click()
    else:
        opener.tap()
    page.wait_for_function(
        "() => document.getElementById('action-menu').hidden", timeout=20_000)


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
            # The fold "Expand all" exists for (`#1236`): a long message with
            # no document in it, so the document count below is untouched.
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                 {"text": LONG_MESSAGE})
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
                check(page.locator("#control-pane").is_hidden(), "the call controls show over the reader")

                # The title is the source link (#1038): href, new tab, no opener.
                title_link = page.locator("#doc-title a")
                check(title_link.count() == 1, "the reader title is not a link to the source")
                check(title_link.get_attribute("href") == "https://paste.example/demo-doc",
                      "the title link does not carry the document's original URL")
                check(title_link.get_attribute("target") == "_blank", "the title link opens in place")
                check("noopener" in (title_link.get_attribute("rel") or ""),
                      "the title link keeps an opener")

                # The reader's layout on the phone, and its menu (`#1236
                # documents-reader-layout`): full width, no grip over the
                # text, zebra rows, heading rules, and no "Expand all".
                check_reader_layout(page, desktop=False)
                check_action_menu(page, desktop=False, expand_visible=False)

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
                wait_heading_at_top(page, "doc-finish", "Finish")
                check(page.locator("#doc-outline-pop").is_hidden(),
                      "picking an outline entry left the pop-up open")

                # Scrolling (here by scrollTop, as a swipe would move it) advances HERE.
                page.evaluate(
                    """() => {
                      const root = document.getElementById('scroll-area');
                      const h = document.getElementById('doc-setup');
                      root.scrollTop += h.getBoundingClientRect().top - root.getBoundingClientRect().top;
                    }""")
                wait_here_is(page, "Setup")

                # An outside tap closes the pop-up without moving the document.
                page.tap("#doc-outline-open")
                page.wait_for_function(
                    "() => !document.getElementById('doc-outline-pop').hidden", timeout=20_000)
                top_before = page.evaluate("() => document.getElementById('scroll-area').scrollTop")
                page.touchscreen.tap(30, 700)
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

                # The system Back gesture: list -> document -> back to the list.
                tap_card(page, paste_card)
                page.wait_for_selector("#doc-body h1", timeout=20_000)
                page.go_back()
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                check(page.locator("#pane-docs").is_visible(), "Back from the reader left the Documents view")

                # A mention jumps to its message; Back returns to the list.
                mention = page.locator("#documents-list .doc-card", has_text="demo-doc").locator(".doc-mentions")
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
                dpage.locator("#documents-list .doc-card", has_text="demo-doc").click()
                dpage.wait_for_selector("#doc-body h1", timeout=20_000)
                check(dpage.locator("#doc-title a").get_attribute("href")
                      == "https://paste.example/demo-doc",
                      "the desktop title does not link to the source")
                # The reader's layout on the desk, and its menu (`#1236
                # documents-reader-layout`): a bounded, centred column, no
                # grip over the text, zebra rows, heading rules, and no
                # "Expand all".
                check_reader_layout(dpage, desktop=True)
                check_action_menu(dpage, desktop=True, expand_visible=False)
                dpage.click("#doc-outline-open")
                dpage.wait_for_function(
                    "() => !document.getElementById('doc-outline-pop').hidden", timeout=20_000)
                dtexts = [t.strip() for t in dpage.locator("#doc-outline button").all_text_contents()]
                check(dtexts == OUTLINE_TEXTS, f"the desktop outline lists {dtexts}, not {OUTLINE_TEXTS}")
                dpage.locator("#doc-outline button", has_text="Setup details").click()
                wait_heading_at_top(dpage, "doc-setup-details", "Setup details")
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

                # The message view is unchanged (`#1236`): on to the channel
                # (Documents -> Voice -> Channel), where the seeded long
                # message folds — the grip stands on the column again and
                # the menu offers "Expand all" again.
                dpage.click("#view-switch")
                dpage.click("#view-switch")
                dpage.wait_for_function(
                    "() => !document.getElementById('pane-discord').hidden", timeout=20_000)
                dpage.wait_for_function(
                    "() => !document.getElementById('expand-all').hidden", timeout=20_000)
                grip_back = dpage.evaluate(
                    """() => {
                      const grip = document.getElementById('width-grip');
                      return {
                        rects: grip.getClientRects().length,
                        display: getComputedStyle(grip).display,
                      };
                    }""")
                check(grip_back["rects"] > 0 and grip_back["display"] != "none",
                      f"the width grip did not return with the channel: {grip_back}")
                check_action_menu(dpage, desktop=True, expand_visible=True)
                dctx.close()
                browser.close()
            print("documents browser check: real-server list, touch fetch, reader, outline pop-up, "
                  "source link, fetch error, in-document links, and reader layout all seen")
            return 0
        finally:
            server.terminate()
            server.wait(timeout=10)
            raw.shutdown()


if __name__ == "__main__":
    sys.exit(main())
