#!/usr/bin/env python3
"""Settings' channel section in real Chromium at a desk's size and a phone's: Who is who and the hidden fold.

`#402 who-is-who-channel-box` and `#401 hidden-channels-fold`. The owner, 2026-10-21: Who is who "should
stay under the channel section along with everything that varies depending on selecting the channel in
the drop down", and the hidden channels "should stay at the bottom of that pane and be collapsed by
default. When I'm switching channels and seeing the interface update it seems invasive to have this bit
in the middle". The fake-DOM suite (tests/js/voice_page.test.mjs) pins what the page draws and which
fold is open, but it lays nothing out: nothing there can say that the fold is the last thing drawn in
the section, or that opening it moves nothing above it. This measures both as the browser lays them out:

  At 1280x800 and at 393x852 (a phone, touch and three device pixels to the pixel): signed in, both
  channels read so each has its cast -> Settings, Channels unfolded: Who is who is in the channel box,
  not in Messages, and lists the first channel's cast under a line naming it -> the hidden channels are
  a fold headed "Hidden channels (3)", folded, and the last thing drawn in the section -> opened, its
  three rows show, and nothing above it in the section has moved by a pixel; folded again -> the box's
  picker on the second channel: its cast, the fold still folded and still last, and the picker and the
  line over it where they were -> All channels: one line asking for a channel, the fold still folded ->
  the fold opened and Show again pressed inside it: the channel is back in the box's picker, the heading
  says two, and the fold is still open.

It reuses keyboard_browser's fixture (its fake API and asset server), with two channels, three hidden
ones and a chat service that can clear unread, so All channels is offered. --web-root DIR runs it
against another copy of the page; against one from before the change it fails at the first check, Who is
who outside the box. --screenshots DIR keeps a PNG of each step.
"""

from __future__ import annotations

import argparse
import json
import tempfile
import threading
from dataclasses import dataclass
from http.server import ThreadingHTTPServer
from typing import TYPE_CHECKING
from urllib.parse import unquote, urlsplit

import keyboard_browser as kb
from keyboard_browser import CHANNEL, SECOND, TOKEN, Json, check
import page_sources

if TYPE_CHECKING:
    from pathlib import Path

    from playwright.sync_api import BrowserContext, Page, Playwright

HIDDEN = [
    {"id": "1110000000000000071", "label": "old builds", "writable": True, "alias": None, "added": False},
    {"id": "1110000000000000072", "label": "retired alerts", "writable": True, "alias": None, "added": False},
    {"id": "1110000000000000073", "label": "launch room", "writable": True, "alias": None, "added": False},
]
# Each channel's cast, as (author, author id, bot): who is who lists them, people first.
CASTS = {
    str(CHANNEL["id"]): [("alice", "1000000000000000101", False), ("ci-bot", "1000000000000000001", True)],
    str(SECOND["id"]): [("mallory", "1000000000000000103", False), ("deploy-bot", "1000000000000000104", True)],
}
FIRST_ROW = "#discord-log > li[data-id]"
PICK_ONE = "Pick one channel to see who is who."


@dataclass
class BoxApi(kb.FakeApi):
    """keyboard_browser's two channels, each with its own cast, three hidden ones, and Clear unread."""

    def __post_init__(self) -> None:
        super().__post_init__()
        self.listed: list[Json] = [dict(CHANNEL), dict(SECOND)]
        self.hidden: list[Json] = [dict(one) for one in HIDDEN]

    def client_config(self) -> Json:
        with self.lock:
            return {**super().client_config(), "channels": list(self.listed), "hidden_channels": list(self.hidden),
                    "upstream_clear_supported": True}

    def timeline(self, channel: Json, query: dict[str, list[str]]) -> Json:
        found = super().timeline(channel, query)
        channel_id = str(channel["id"])
        cast = CASTS[channel_id]
        rows = [{**kb.message(n, f"{channel_id[-4:]}0000000000{n:04d}", f"Note {n} in {channel['label']}.",
                              {"author": cast[n % 2][0], "author_id": cast[n % 2][1], "author_is_bot": cast[n % 2][2]},
                              None), "channel_id": channel["id"]} for n in range(12)]
        return {**found, "messages": rows, "threads": [], "thread": None, "dismissed": [], "returned": len(rows)}

    def upstream_clear(self, channel: Json) -> Json:
        return {"channel": channel, "provider_name": "Google Chat", "supported": True, "mode": "off",
                "failures": 0, "pending": False}

    def show_again(self, channel_id: str) -> Json | None:
        with self.lock:
            shown = next((one for one in self.hidden if one["id"] == channel_id), None)
            if shown is None:
                return None
            self.hidden.remove(shown)
            self.listed.append(shown)
            return {"channel": shown, "channels": list(self.listed), "hidden_channels": list(self.hidden)}


def handler_for(api: BoxApi, web_root: Path) -> type:
    base = kb.handler_for(api, web_root)

    class Handler(base):  # type: ignore[valid-type,misc]
        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            path = unquote(urlsplit(self.path).path)
            if path.endswith("/upstream-clear") and self.headers.get("Authorization") == f"Bearer {TOKEN}":
                self.json(200, api.upstream_clear(kb.channel_of(path)))
            else:
                super().do_GET()

        def do_DELETE(self) -> None:  # noqa: N802
            path = unquote(urlsplit(self.path).path)
            parts = path.split("/")
            shown = api.show_again(parts[4]) if len(parts) == 6 and parts[5] == "hidden" else None
            if self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif shown is None:
                self.json(404, {"error": "unknown_channel", "detail": "not hidden here"})
            else:
                self.json(200, shown)

    return Handler


# The channel section as the browser lays it out. Every box is measured from the top of the section,
# so a fold that rebalances Settings' columns (`#345 settings-multi-column`) or scrolls its heading into
# view does not read as a move inside the section.
SECTION_JS = """() => {
  const section = document.getElementById('settings-channels');
  const top = section.getBoundingClientRect().top;
  const fold = document.getElementById('hidden-channels');
  const shown = (node) => node.checkVisibility();
  const above = [...document.querySelectorAll('#fold-settings-channels-body [id]')]
    .filter((node) => !fold.contains(node) && shown(node))
    .map((node) => { const r = node.getBoundingClientRect();
      return {id: node.id, top: Math.round((r.top - top) * 10) / 10, height: Math.round(r.height * 10) / 10,
              bottom: r.bottom - top}; });
  const head = document.getElementById('fold-hidden-channels');
  const list = document.getElementById('identity-list');
  return {
    inBox: document.getElementById('channel-settings').contains(list),
    inMessages: document.getElementById('settings-messages').contains(list),
    names: [...list.querySelectorAll('.identity-name')].map((n) => n.firstChild ? n.firstChild.textContent : ''),
    words: list.textContent,
    picker: document.getElementById('settings-channel').value,
    // The page from before `#401` has no fold to name: null, which the checks refuse by name.
    foldShown: shown(fold), label: head ? head.textContent : null, open: head ? head.getAttribute('aria-expanded') === 'true' : null,
    foldTop: fold.getBoundingClientRect().top - top,
    rows: [...document.querySelectorAll('#hidden-channel-list > li')].filter(shown)
      .map((li) => li.querySelector('.directory-name').textContent),
    above, width: window.innerWidth,
    sideways: document.getElementById('screen-settings').scrollWidth - document.getElementById('screen-settings').clientWidth,
  };
}"""


def section(page: Page) -> Json:
    found = page.evaluate(SECTION_JS)
    assert isinstance(found, dict)
    return found


def boxes(found: Json) -> list[Json]:
    listed = found["above"]
    assert isinstance(listed, list)
    return [one for one in listed if isinstance(one, dict)]


def check_last(found: Json, where: str) -> None:
    """The fold shown, and nothing in the section drawn below its top."""
    check(found["foldShown"] is True, f"{where}: the hidden channels are not shown: {found}")
    top = float(str(found["foldTop"]))
    below = [one["id"] for one in boxes(found) if float(str(one["bottom"])) > top + 1]
    check(not below, f"{where}: drawn below the hidden channels, which should be last: {below}")


def shot(page: Page, shots: Path | None, name: str) -> None:
    if shots is not None:
        page.screenshot(path=str(shots / f"channel-box-{name}.png"))


def sign_in(page: Page, url: str) -> None:
    page.goto(url, wait_until="load")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    if page.evaluate("() => document.getElementById('view-switch').getAttribute('aria-checked')") != "true":
        page.click("#view-switch")
    page.wait_for_selector(FIRST_ROW, state="visible", timeout=10_000)


def read(page: Page, channel: Json) -> None:
    """Read `channel` on the bar, so its cast is learned."""
    page.select_option("#discord-channel", str(channel["id"]))
    page.wait_for_function("(name) => [...document.querySelectorAll('#discord-log .discord-text, #discord-log li')]"
                           ".some((li) => li.textContent.includes(name))",
                           arg=f"in {channel['label']}.", timeout=10_000)


def open_channels(page: Page) -> None:
    page.click("#open-settings")
    page.wait_for_selector("#close-settings", state="visible", timeout=10_000)
    if page.get_attribute("#fold-settings-channels", "aria-expanded") != "true":
        page.click("#fold-settings-channels")
    page.wait_for_selector("#settings-channel", state="visible", timeout=10_000)


def show_section(page: Page, element: str = "settings-channels") -> None:
    page.evaluate("(id) => document.getElementById(id).scrollIntoView({block: 'start'})", element)
    page.wait_for_timeout(150)


def walk(context: BrowserContext, url: str, shots: Path | None, where: str) -> str:
    page = context.pages[0]
    tag = "desk" if where.startswith("1280") else "phone"
    sign_in(page, url)
    read(page, SECOND)
    read(page, CHANNEL)
    open_channels(page)
    show_section(page)
    # Waited for as both the page before `#402` and after it word it, so that page fails by name below.
    page.wait_for_function("() => document.getElementById('identity-list').textContent.startsWith('In lead team')",
                           timeout=10_000)
    found = section(page)
    check(found["inBox"] is True and found["inMessages"] is False,
          f"{where}: Who is who is not in the channel box (in the box {found['inBox']}, in Messages {found['inMessages']})")
    check(found["names"] == ["alice", "ci-bot"], f"{where}: Who is who lists {found['names']}, not the first channel's cast")
    check(found["label"] == "Hidden channels (3)", f"{where}: the fold is headed {found['label']!r}")
    check(found["open"] is False and found["rows"] == [], f"{where}: the hidden channels are open as Settings opens")
    check_last(found, f"{where}, on the first channel")
    check(float(str(found["sideways"])) <= 0, f"{where}: Settings scrolls sideways by {found['sideways']}px")
    shot(page, shots, f"{tag}-first-channel")
    before = boxes(found)

    # Opened: its rows, and nothing above it moved.
    page.click("#fold-hidden-channels")
    page.wait_for_function("() => document.getElementById('fold-hidden-channels').getAttribute('aria-expanded') === 'true'",
                           timeout=5_000)
    found = section(page)
    check(found["rows"] == [one["label"] for one in HIDDEN], f"{where}: the open fold shows {found['rows']}")
    moved = [(was["id"], was["top"], now["top"]) for was, now in zip(before, boxes(found))
             if was["id"] != now["id"] or was["top"] != now["top"] or was["height"] != now["height"]]
    check(len(before) == len(boxes(found)) and not moved, f"{where}: opening the fold moved what is above it: {moved}")
    check_last(found, f"{where}, the fold open")
    show_section(page, "hidden-channels")
    shot(page, shots, f"{tag}-fold-open")
    page.click("#fold-hidden-channels")
    page.wait_for_function("() => document.getElementById('fold-hidden-channels').getAttribute('aria-expanded') === 'false'",
                           timeout=5_000)

    # Another channel in the box: its cast; the fold folded and last; the picker where it was.
    show_section(page)
    head = {one["id"]: one["top"] for one in boxes(section(page)) if one["id"] in ("chat-provider-line", "settings-channel")}
    page.select_option("#settings-channel", str(SECOND["id"]))
    page.wait_for_function("() => document.getElementById('identity-list').textContent.startsWith('In second team:')",
                           timeout=10_000)
    found = section(page)
    check(found["names"] == ["mallory", "deploy-bot"], f"{where}: on the second channel Who is who lists {found['names']}")
    check(found["open"] is False, f"{where}: a picker change opened the hidden channels")
    check_last(found, f"{where}, on the second channel")
    now = {one["id"]: one["top"] for one in boxes(found) if one["id"] in head}
    check(now == head, f"{where}: a picker change moved the picker or the line over it: {head} -> {now}")
    shot(page, shots, f"{tag}-second-channel")

    # All channels: one line in place of the list, and the fold untouched.
    page.select_option("#settings-channel", "*")
    page.wait_for_function("(words) => document.getElementById('identity-list').textContent === words",
                           arg=PICK_ONE, timeout=10_000)
    found = section(page)
    check(found["open"] is False, f"{where}: All channels opened the hidden channels")
    check_last(found, f"{where}, on All channels")
    show_section(page, "identity-group")
    shot(page, shots, f"{tag}-all-channels")

    # Show again from inside the open fold.
    page.click("#fold-hidden-channels")
    page.click(f"#hidden-channel-list button[aria-label='Show {HIDDEN[0]['label']} again']")
    page.wait_for_function("() => document.getElementById('fold-hidden-channels').textContent === 'Hidden channels (2)'",
                           timeout=10_000)
    found = section(page)
    check(found["picker"] == HIDDEN[0]["id"], f"{where}: after Show again the box is on {found['picker']!r}")
    check(found["open"] is True and found["rows"] == [one["label"] for one in HIDDEN[1:]],
          f"{where}: after Show again the fold is open {found['open']} with {found['rows']}")
    check_last(found, f"{where}, after Show again")
    return (f"at {where} Who is who in the box following its picker, the fold headed 'Hidden channels (3)', folded "
            f"and last, opening it moved none of the {len(before)} boxes above it, and Show again worked inside it")


def browse(playwright: Playwright, args: argparse.Namespace, url: str, width: int, height: int,
           mobile: bool) -> tuple[str, str]:
    with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-box-") as profile:
        context = playwright.chromium.launch_persistent_context(
            profile, headless=True, executable_path=args.browser_executable,
            viewport={"width": width, "height": height}, is_mobile=mobile, has_touch=mobile,
            device_scale_factor=3 if mobile else 1,
        )
        errors: list[str] = []
        context.pages[0].on("pageerror", lambda error: errors.append(str(error)))
        note = walk(context, url, args.screenshots, f"{width}x{height}")
        check(not errors, f"the page threw: {errors}")
        version = context.browser.version if context.browser else "Chromium"
        context.close()
    return version, note


def serve(web_root: Path) -> tuple[BoxApi, ThreadingHTTPServer, threading.Thread, str]:
    """The fixture's server on a free loopback port: the API, the server, its thread and its URL."""
    api = BoxApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api, web_root.resolve()))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return api, server, thread, f"http://127.0.0.1:{server.server_port}/"


def main() -> int:
    args = kb.arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    # The fixture answers a channel's reads by its id; the hidden ones come back by Show again.
    kb.CHANNELS.update({str(one["id"]): one for one in HIDDEN})
    notes: dict[str, str] = {}
    with sync_playwright() as playwright:
        # A server each, so the phone starts with the three hidden channels the desk's Show again took one of.
        for name, width, height, mobile in (("desk", 1280, 800, False), ("phone", 393, 852, True)):
            api, server, thread, url = serve(page_sources.served(args.web_root))
            try:
                notes["browser"], notes[name] = browse(playwright, args, url, width, height, mobile)
            finally:
                api.stopping.set()
                server.shutdown()
                server.server_close()
                thread.join()
    print(json.dumps(notes, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
