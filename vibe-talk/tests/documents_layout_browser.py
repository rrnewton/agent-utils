"""The Documents reader's layout against the REAL server (`#1236 documents-reader-layout`).

Starts the release binary with --fake-discord (a real server; only the chat
upstream is fake), seeds one paste document — headings, a bullet list, and a
four-row table — plus one long channel message past the fold, then opens the
reader at 393x852 (touch) and 1280x800 (mouse). At both sizes: the column is
bounded and centred on the desk and full-width on the phone, the
reading-width grip has no box over the text and no point over the column
hits it, table body rows alternate grounds, `h1`/`h2` carry a bottom rule,
the list keeps its air around it with dense items, and the hamburger menu
offers no "Expand all" in this view. Back in the channel, where the seeded
long message folds, the grip and the item are offered again. No documents
API is stubbed: list, fetch and render all go through the server under test.

Split from `documents_browser.py` so `#1235 documents-list-continuation` and
this check gate in parallel without sharing a file.
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
READ_TOKEN = "documents-layout-check-read-token-not-a-real-credential"
WRITE_TOKEN = "documents-layout-check-write-token-not-a-real-credential"
# The reader-layout matter: bullets long enough to wrap onto a second line
# of the reader, and a table with enough body rows to alternate twice over.
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
DOC_BODY = (
    "# Layout report\n\nThe **nightly** run finished green.\n\n"
    + _LAYOUT_LIST + "\n" + _LAYOUT_TABLE
    + "\n## Details\n\nThe detail under the second heading.\n"
)
# A channel message well past the fold: in the channel it gives "Expand all"
# work to do, which is what makes its absence from the menu in the Documents
# view a check rather than the menu's resting state.
_LONG_PARAGRAPH = (
    "A long status note for the fold: the nightly run rebuilt every target "
    "from a clean tree, ran each suite in turn, compared the results with "
    "the run before it, and wrote the whole account down where the next "
    "reader can find it without opening a single log by hand. Nothing in "
    "it is a link, so it registers no document."
)
LONG_MESSAGE = "\n\n".join([_LONG_PARAGRAPH] * 4)


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


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
    with tempfile.TemporaryDirectory(prefix="documents-layout-check-") as tmp:
        work = Path(tmp)
        (work / "layout-doc").write_text(DOC_BODY)
        (work / "vibe-talk.toml").write_text(
            f'[server]\nbind = "127.0.0.1:{port}"\n'
            '[discord]\nbot_token = "documents-layout-check-bot-token-never-sent"\n'
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
                 {"text": "Report: https://paste.example/layout-doc"})
            # The fold "Expand all" exists for: a long message with no
            # document in it, so the document count below is untouched.
            call(base, "POST", f"/api/v1/channels/{CHANNEL}/reply", WRITE_TOKEN,
                 {"text": LONG_MESSAGE})
            docs = []
            deadline = time.time() + 30
            while time.time() < deadline:
                call(base, "GET", f"/api/v1/channels/{CHANNEL}/page?limit=10&render=html", WRITE_TOKEN)
                answer = call(base, "GET", "/api/v1/documents?limit=10", READ_TOKEN)
                docs = cast(list[dict[str, object]], answer["documents"])
                if len(docs) == 1:
                    break
                time.sleep(0.5)
            check(len(docs) == 1, f"the fold registered {len(docs)} documents, not 1: {docs}")

            with sync_playwright() as pw:
                browser = pw.chromium.launch()
                ctx = browser.new_context(viewport={"width": 393, "height": 852}, has_touch=True, is_mobile=True)
                page = ctx.new_page()
                page.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                page.goto(base + "/?v=docs", wait_until="load")
                page.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                paste_card = page.locator("#documents-list .doc-card", has_text="layout-doc")
                tap_card(page, paste_card)
                page.wait_for_selector("#doc-body h1", timeout=20_000)
                check("Layout report" in (page.text_content("#doc-body") or ""),
                      "the reader did not render the fetched document")
                # The reader's layout on the phone, and its menu: full
                # width, no grip over the text, zebra rows, heading rules,
                # and no "Expand all".
                check_reader_layout(page, desktop=False)
                check_action_menu(page, desktop=False, expand_visible=False)
                ctx.close()

                dctx = browser.new_context(viewport={"width": 1280, "height": 800})
                dpage = dctx.new_page()
                dpage.add_init_script(
                    f"try {{ localStorage.setItem('vibe-talk.token', {json.dumps(WRITE_TOKEN)}); }} catch (e) {{}}")
                dpage.goto(base + "/?v=docs", wait_until="load")
                dpage.wait_for_selector("#documents-list .doc-card", timeout=20_000)
                dpage.locator("#documents-list .doc-card", has_text="layout-doc").click()
                dpage.wait_for_selector("#doc-body h1", timeout=20_000)
                # The reader's layout on the desk, and its menu: a bounded,
                # centred column, no grip over the text, zebra rows,
                # heading rules, and no "Expand all".
                check_reader_layout(dpage, desktop=True)
                check_action_menu(dpage, desktop=True, expand_visible=False)

                # The message view is unchanged: on to the channel
                # (Documents -> Voice -> Channel), where the seeded long
                # message folds — the grip stands on the column again and
                # the menu offers "Expand all" again.
                dpage.click("#view-switch")
                dpage.click("#view-switch")
                dpage.wait_for_function(
                    "() => !document.getElementById('pane-discord').hidden", timeout=20_000)
                dpage.wait_for_function(
                    "() => document.querySelectorAll('#discord-log > li[data-collapsed=\"true\"]').length > 0",
                    timeout=20_000)
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
            print("documents layout browser check: reader measure, grip gate, zebra rows, "
                  "heading rules, and menu gating all seen at phone and desktop sizes")
            return 0
        finally:
            server.terminate()
            server.wait(timeout=10)


if __name__ == "__main__":
    sys.exit(main())
