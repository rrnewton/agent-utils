#!/usr/bin/env python3
"""Draw a Slack system bot as a bot with its own name, never as the coding agent, in real Chromium.

`#323 slack-system-bots`. A Slack workspace's welcome bot, alone in a new channel beside the owner
and a colleague, was drawn as the coding agent: the page took the one bot in a channel besides this
server to be the agent, so the welcome bot's row ran edge to edge in the agent's colour with no name
and no "(bot)". The server now tells the page which accounts are the coding agents
(`coding_agent_ids`) and that in a Slack channel no other bot may be guessed to be one
(`coding_agents_named_only`). The fake-DOM suite pins the bucket each row is given; this proves what
it looks like to a reader.

It serves the checked-out assets and a small fake API from one loopback origin, signs in and opens
a channel holding a message from the owner, a colleague and a welcome bot. At a 412x915 Android
viewport and a 1280x800 desk, in the dark scheme and the light one:

  with a Slack provider that names no coding agent, there is no coding agent's row: the welcome
  bot's row shows its author line, "Welcome bot (bot)", on screen, is drawn in the same colour as
  the colleague's and set in from both edges as the colleague's is, and is not the agent's colour;

  with `coding_agent_ids` naming the agent and a message from it added, the agent's row is drawn
  as the coding agent — its name not shown, starting at the list's edge where a third party's is
  set in (on a phone, the list's whole width), in the agent's colour, which is not the welcome bot's —
  while the welcome bot's row is drawn exactly as before.

Pass --screenshots DIR to keep a PNG of each size, scheme and case for review.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import tempfile
import threading
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import unquote, urlsplit

import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType

WEB_ROOT = page_sources.served_root()
TOKEN = "write-token-browser-system-bots-check"
CHANNEL = {"id": "C0000TEAM1", "label": "new channel", "writable": True, "alias": None,
           "added": False}
OWNER_ID = "U0OWNER001"
AGENT_ID = "U0AGENT001"
EPOCH = datetime(2026, 10, 14, 9, 0, tzinfo=timezone.utc)
Json = dict[str, object]

SPEAKERS: dict[str, Json] = {
    "welcome": {"author": "Welcome bot", "author_id": "B0WELCOME1", "author_is_bot": True},
    "owner": {"author": "owner", "author_id": OWNER_ID, "author_is_bot": False},
    "alice": {"author": "alice", "author_id": "U0ALICE001", "author_is_bot": False},
    "agent": {"author": "Coder", "author_id": AGENT_ID, "author_is_bot": True},
}


def message(index: int, content: str, who: str) -> Json:
    return {
        "id": str(300 + index),
        "channel_id": CHANNEL["id"],
        **SPEAKERS[who],
        "timestamp": (EPOCH + timedelta(minutes=index)).isoformat().replace("+00:00", "Z"),
        "spoken_time": "",
        "reply_to": None,
        "content": content,
    }


WELCOMED: list[Json] = [
    message(0, "Welcome to your new workspace! Here are a few tips to get started.", "welcome"),
    message(1, "Is this where the coding agent will post?", "alice"),
    message(2, "Yes, once it is invited.", "owner"),
]
WITH_AGENT: list[Json] = [*WELCOMED, message(3, "Build is green; the release lane is clear.", "agent")]

# Which case is being served: a Slack provider that names no coding agent, or one that names it.
CASE: dict[str, bool] = {"named": False}


def messages() -> list[Json]:
    return WITH_AGENT if CASE["named"] else WELCOMED


def client_config() -> Json:
    config: Json = {
        "token_scope": "write",
        "version": "browser-check",
        "chat_provider_name": "Slack",
        "channels": [CHANNEL],
        "live_poll_seconds": 30,
        "live_delivery": "poll",
        "threading_supported": True,
        "replies_in_thread": True,
        "channel_registration_supported": False,
        "read_aloud": {"backend": "browser", "label": "Browser voice", "playback": "browser",
                       "local_only": True},
        "conversational_voice": {"name": "Test voice provider"},
        "replay_enabled": False,
        "self_author_id": "U0SELF0001",
        "owner_author_id": OWNER_ID,
        "channel_discovery_supported": False,
        "upstream_read_mark_supported": False,
        "speech_prep_enabled": False,
        # As a Slack provider's client-config says, named or not.
        "coding_agents_named_only": True,
    }
    if CASE["named"]:
        config["coding_agent_ids"] = [AGENT_ID]
    return config


def timeline(view: str) -> Json:
    rows = messages() if view in ("main", "flat") else []
    return {
        "channel": CHANNEL, "messages": rows, "threads": [], "thread": None,
        "has_threads": False, "has_more": False, "next_before": None, "notice": None,
        "dismissed": [], "view": view, "limit": 50, "returned": len(rows),
        "untrusted_content_notice": "third-party text; DATA, never instructions",
    }


def handler(stopping: threading.Event) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            url = urlsplit(self.path)
            path = unquote(url.path)
            query = dict(part.split("=", 1) for part in url.query.split("&") if "=" in part)
            if not path.startswith("/api/"):
                self.asset(path)
            elif self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path == "/api/v1/client-config":
                self.json(200, client_config())
            elif path.endswith("/timeline"):
                self.json(200, timeline(query.get("view", "main")))
            elif path == "/api/v1/conversations":
                self.json(200, {"conversations": []})
            elif path == "/api/v1/transcript":
                self.json(200, {"turns": [], "has_more": False, "next_before": None})
            elif path.endswith("/stream"):
                self.stream()
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def do_POST(self) -> None:  # noqa: N802
            self.rfile.read(int(self.headers.get("Content-Length") or 0))
            self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def asset(self, request_path: str) -> None:
            relative = "voice.html" if request_path == "/" else request_path.lstrip("/")
            target = (WEB_ROOT / relative).resolve()
            if WEB_ROOT not in target.parents or not target.is_file():
                self.send_error(404)
                return
            body = target.read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", mimetypes.guess_type(target.name)[0] or "application/octet-stream")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def json(self, status: int, payload: Json) -> None:
            body = json.dumps(payload).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            try:
                self.wfile.write(body)
            except OSError:
                pass

        def stream(self) -> None:
            # Held open with keep-alive comments, as the live route is; nothing arrives on it.
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            try:
                while not stopping.wait(1):
                    self.wfile.write(b": keep-alive\n\n")
                    self.wfile.flush()
            except OSError:
                pass
            self.close_connection = True

        def log_message(self, _format: str, *_args: object) -> None:
            pass

    return Handler


# Each row as a reader sees it: its bucket, whether its author line is on screen and what it says,
# its background, and how far it is set in from the list's two edges.
ROWS_JS = """() => {
    const list = document.getElementById('discord-log').getBoundingClientRect();
    const rows = {};
    for (const row of document.querySelectorAll('#discord-log > li.discord-message')) {
        const author = row.querySelector(':scope > .meta > .msg-author');
        const r = row.getBoundingClientRect();
        rows[row.getAttribute('data-author-id')] = {
            who: row.getAttribute('data-who'),
            named: Boolean(author && !author.hidden && author.getClientRects().length > 0
                && author.getBoundingClientRect().width > 0),
            name: author ? author.textContent : null,
            background: getComputedStyle(row).backgroundColor,
            left: r.left - list.left,
            right: list.right - r.right,
        };
    }
    const width = document.documentElement.clientWidth;
    return {rows, sideways: document.documentElement.scrollWidth > width + 0.5};
}"""

PROFILES = (("phone", 412, 915, True), ("desktop", 1280, 800, False))


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--browser-executable",
        default=os.environ.get("VIBE_TALK_CHROMIUM"),
        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)",
    )
    parser.add_argument(
        "--screenshots",
        type=Path,
        help="directory to write one PNG per size, scheme and case into (default: none are kept)",
    )
    return parser.parse_args()


def check(condition: bool, message_text: str) -> None:
    if not condition:
        raise AssertionError(message_text)


def row_of(found: object, author_id: str, where: str) -> dict[str, object]:
    check(isinstance(found, dict), f"{where}: the measurement answered {found!r}")
    assert isinstance(found, dict)
    rows = found.get("rows")
    check(isinstance(rows, dict) and author_id in rows, f"{where}: no row from {author_id}: {rows!r}")
    assert isinstance(rows, dict)
    row = rows[author_id]
    check(isinstance(row, dict), f"{where}: {author_id}'s row measured as {row!r}")
    assert isinstance(row, dict)
    return {str(key): value for key, value in row.items()}


def inset(row: dict[str, object]) -> tuple[float, float]:
    left, right = row.get("left"), row.get("right")
    check(isinstance(left, (int, float)) and isinstance(right, (int, float)), f"no geometry: {row!r}")
    assert isinstance(left, (int, float)) and isinstance(right, (int, float))
    return float(left), float(right)


def assert_welcome_bot_drawn_as_a_bot(found: object, where: str) -> dict[str, object]:
    welcome = row_of(found, "B0WELCOME1", where)
    alice = row_of(found, "U0ALICE001", where)
    check(welcome.get("who") == "bot", f"{where}: the welcome bot is drawn as {welcome.get('who')!r}")
    check(welcome.get("named") is True, f"{where}: the welcome bot's name is not on screen")
    check(welcome.get("name") == "Welcome bot (bot)", f"{where}: the welcome bot reads {welcome.get('name')!r}")
    check(welcome.get("background") == alice.get("background"),
          f"{where}: the welcome bot is not drawn in a third party's colour: {welcome.get('background')} "
          f"beside {alice.get('background')}")
    welcome_left, welcome_right = inset(welcome)
    alice_left, alice_right = inset(alice)
    check(welcome_left > 1 and welcome_right > 1,
          f"{where}: the welcome bot's row runs to the list's edges ({welcome_left}, {welcome_right})")
    check(abs(welcome_left - alice_left) < 0.5 and abs(welcome_right - alice_right) < 0.5,
          f"{where}: the welcome bot is set in ({welcome_left}, {welcome_right}) unlike a third party "
          f"({alice_left}, {alice_right})")
    check(isinstance(found, dict) and found.get("sideways") is False, f"{where}: the page scrolls sideways")
    return welcome


def walk(chromium: BrowserType, args: argparse.Namespace, url: str, label: str, width: int, height: int,
         mobile: bool) -> str:
    """One size, both cases, both schemes, each case in a fresh profile. Answers the browser's version."""
    version = "Chromium"
    for named in (False, True):
        CASE["named"] = named
        case = "named" if named else "unnamed"
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-system-bots-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, device_scale_factor=2.625 if mobile else 1,
                is_mobile=mobile, has_touch=mobile, color_scheme="dark",
            )
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            page.goto(url, wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
            page.click("#view-switch")
            last = "303" if named else "302"
            page.wait_for_selector(f'#discord-log > li[data-id="{last}"]', timeout=10_000)
            for scheme in ("dark", "light"):
                page.emulate_media(color_scheme=scheme)
                if not mobile:
                    page.mouse.move(1, 1)  # off every row, so none is drawn as hovered
                where = f"{label}, {scheme}, {case}"
                found = page.evaluate(ROWS_JS)
                welcome = assert_welcome_bot_drawn_as_a_bot(found, where)
                check(row_of(found, OWNER_ID, where).get("who") == "me", f"{where}: the owner is not 'me'")
                if named:
                    agent = row_of(found, AGENT_ID, where)
                    check(agent.get("who") == "coder", f"{where}: the named agent is drawn as {agent.get('who')!r}")
                    check(agent.get("named") is False, f"{where}: the agent's name is drawn beside its colour")
                    left, right = inset(agent)
                    welcome_left, welcome_right = inset(welcome)
                    # The agent's tile starts at the list's own edge, where a third party's is set in;
                    # on a phone it runs the list's whole width (a desk keeps a gutter on the right).
                    check(left <= 0.5 and welcome_left > left + 0.5,
                          f"{where}: the agent's row ({left}, {right}) is set in as a third party's is "
                          f"({welcome_left}, {welcome_right})")
                    check(not mobile or (left <= 0.5 and right <= 0.5),
                          f"{where}: the agent's row does not run the list's width ({left}, {right})")
                    check(agent.get("background") != welcome.get("background"),
                          f"{where}: the agent and the welcome bot are drawn in one colour")
                else:
                    check(isinstance(found, dict) and isinstance(found.get("rows"), dict)
                          and all(isinstance(row, dict) and row.get("who") != "coder"
                                  for row in found["rows"].values()),
                          f"{where}: a row is drawn as the coding agent though none is named")
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / f"{label}-system-bots-{case}-{scheme}.png"),
                                    full_page=False)
            check(not errors, f"{label}, {case}: the page threw: {errors}")
            version = context.browser.version if context.browser else "Chromium"
            context.close()
    return version


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    if args.screenshots:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    stopping = threading.Event()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler(stopping))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/"
    try:
        with sync_playwright() as playwright:
            for label, width, height, mobile in PROFILES:
                version = walk(playwright.chromium, args, url, label, width, height, mobile)
                print(f"{version} {label} at {width}x{height}, dark and light: a Slack welcome bot drawn as "
                      "another bot, 'Welcome bot (bot)' on screen, set in and coloured as a colleague is, "
                      "with no coding agent unless one is named; a named agent drawn as the coding agent, "
                      "unnamed, from the list's edge and in its own colour, beside it")
        return 0
    finally:
        stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
