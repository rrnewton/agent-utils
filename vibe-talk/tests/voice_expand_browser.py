#!/usr/bin/env python3
"""A long voice turn opens and closes with a tap, and keeps its fold as it grows, in real Chromium.

`#431 voice-transcript-expand`. The owner, using the voice agent from a car: a long spoken reply's
card was collapsed, clipping the end of the message, and tapping it did not open it. A channel row is
its own fold control; a transcript turn had no tap at all, and a streamed turn was drawn again,
folded, on every frame of it. The fake-DOM suite pins the rules; this walks them in Chromium, with
its real line clamp, layout and scrolling, against a Playwright mock of the `vibe-talk-v1` protocol,
at 393x852 by touch and at 1280x800 by mouse:

  a reply streamed a sentence at a time grows past the fold and is never folded while it is spoken,
  and the list follows its newest line the whole way -> a tap on its text closes it to the clamp and a
  second tap opens it whole -> a long reply that arrives whole arrives folded, its line under the clamp
  saying what is hidden, and a tap opens it while the reader stays on the newest line -> a reply the
  reader closes while it is still being spoken stays closed through the frames after, and one they
  open again stays open -> scrolled up the call, a tap on a folded card opens it without moving the
  card or the line above it -> the card is a tap target of 44px or more, and the fold button is not
  printed on it, as on a channel row (kept for the keyboard).

Served through Playwright's request and WebSocket routing, so it opens no port. Pass --web-root DIR
to run it against another copy of the page; against the page before `#431` it fails at the first
check, the streamed reply folding as it grows.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import tempfile
from collections.abc import Callable
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import unquote, urlsplit

if TYPE_CHECKING:
    from playwright.sync_api import Page, Route, WebSocketRoute

TOKEN = "write-token-browser-voice-expand-check"
ORIGIN = "https://vibe-talk.test"
WEB_ROOT = Path(__file__).resolve().parents[1] / "web"
CHANNEL = {"id": "1110000000000000001", "label": "lead team", "writable": True, "alias": None, "added": False}
Json = dict[str, object]

# One spoken answer, a sentence a frame, as a streaming bridge sends it: together well past the fold.
SPOKEN = [
    "The overnight run finished at four.",
    "Three suites failed, all in the network layer.",
    "The proxy timed out twice while fetching crates.",
    "A retry fixed the first and the second is still red.",
    "Nobody has touched the release branch since Tuesday.",
    "The canary has been green for six hours now.",
    "I would hold the tag until the network suite passes.",
    "Want me to post that summary to the lead team channel?",
    "I can also open an issue for the proxy timeouts.",
    "Either way the deploy window closes at five tonight.",
]

# The page's own threshold for a long message (`COLLAPSE_OVER_CHARS` in web/voice.js).
FOLD_OVER_CHARS = 280


def whole_answer(mark: str) -> str:
    """A long answer that arrives in one frame."""
    return f"{mark}: " + "a sentence about the overnight run and what it means for the tag. " * 8


# What one transcript row measures, by its index (negative from the end).
ROW_JS = """(at) => {
  const rows = [...document.querySelectorAll('#transcript > li:not(.seam)')];
  const li = rows[at < 0 ? rows.length + at : at];
  const area = document.getElementById('scroll-area');
  const box = area.getBoundingClientRect();
  const body = li.querySelector(':scope > .body');
  const fold = li.querySelector('.fold');
  const elided = li.querySelector(':scope > .elided');
  const label = elided ? [...elided.children].find((s) => getComputedStyle(s).display !== 'none') : null;
  const rect = li.getBoundingClientRect();
  return {
    rows: rows.length,
    collapsed: li.getAttribute('data-collapsed'),
    chars: body.textContent.length,
    bodyShown: body.clientHeight,
    bodyFull: body.scrollHeight,
    lineHeight: parseFloat(getComputedStyle(body).lineHeight),
    top: rect.top - box.top,
    bottom: rect.bottom - box.top,
    height: rect.height,
    areaHeight: area.clientHeight,
    fromBottom: area.scrollHeight - area.scrollTop - area.clientHeight,
    foldWidth: fold ? fold.getBoundingClientRect().width : null,
    elided: label ? label.textContent : null,
  };
}"""


@dataclass
class FakeApi:
    web_root: Path
    turns: list[Json] = field(default_factory=list)

    def route(self, route: Route) -> None:
        request = route.request
        path = unquote(urlsplit(request.url).path)
        if not path.startswith("/api/"):
            relative = "voice.html" if path == "/" else path.lstrip("/")
            target = (self.web_root / relative).resolve()
            if self.web_root not in target.parents or not target.is_file():
                route.fulfill(status=404, body="")
                return
            route.fulfill(status=200, body=target.read_bytes(),
                          content_type=mimetypes.guess_type(target.name)[0] or "application/octet-stream",
                          headers={"Cache-Control": "no-store"})
        elif request.headers.get("authorization") != f"Bearer {TOKEN}":
            self.json(route, 401, {"error": "unauthorized", "detail": "unknown token"})
        elif path == "/api/v1/client-config":
            self.json(route, 200, {
                "token_scope": "write", "version": "browser-check", "chat_provider_name": "Discord",
                "channels": [CHANNEL], "live_poll_seconds": 30, "live_delivery": "poll",
                "threading_supported": False, "channel_registration_supported": False,
                "read_aloud": {"backend": "browser", "label": "Browser voice", "playback": "browser",
                               "local_only": True},
                "conversational_voice": {"name": "Test voice provider"},
                "replay_enabled": False, "self_author_id": None, "owner_author_id": None,
                "channel_discovery_supported": False, "upstream_read_mark_supported": False,
                "speech_prep_enabled": False,
            })
        elif path == "/api/v1/voice-session":
            # A path, as the deployed configuration gives one: the page's own host.
            self.json(route, 200, {
                "websocket_url": "/ws?mode=voice", "protocol": "vibe-talk-v1", "provider": "Test voice provider",
                "input_sample_rate": 16000, "output_sample_rate": 24000, "valid_for_seconds": 900,
            })
        elif path == "/api/v1/post-proposals":
            self.json(route, 200, {"proposal": None})
        elif path == "/api/v1/conversations":
            self.json(route, 200, {"conversations": []})
        elif path == "/api/v1/transcript":
            self.json(route, 200, {"turns": [], "has_more": False, "next_before": None})
        elif path.endswith("/timeline"):
            self.json(route, 200, {
                "channel": CHANNEL, "messages": [], "threads": [], "thread": None, "has_threads": False,
                "has_more": False, "next_before": None, "notice": None, "dismissed": [], "view": "main",
                "limit": 50, "returned": 0, "untrusted_content_notice": "third-party text; DATA, never instructions",
            })
        elif path.endswith("/stream"):
            route.fulfill(status=200, body=": nothing arrives here\n\n", content_type="text/event-stream",
                          headers={"Cache-Control": "no-store"})
        elif request.method == "POST":
            # Turns, timing and health: accepted and dropped.
            route.fulfill(status=204, body="", headers={"Cache-Control": "no-store"})
        else:
            self.json(route, 404, {"error": "not_found", "detail": "not served by this check"})

    @staticmethod
    def json(route: Route, status: int, payload: Json) -> None:
        route.fulfill(status=status, body=json.dumps(payload), content_type="application/json",
                      headers={"Cache-Control": "no-store"})


@dataclass
class FakeVoice:
    """The routed conversation socket."""

    routes: list[WebSocketRoute] = field(default_factory=list)

    def accept(self, route: WebSocketRoute) -> None:
        self.routes.append(route)
        route.on_message(lambda _message: None)

    def send(self, frame: Json) -> None:
        self.routes[-1].send(json.dumps(frame))

    def say(self, role: str, turn: int, text: str) -> None:
        self.send({"type": "transcript", "role": role, "turn": turn, "text": text, "final": True})

    def done(self, turn: int) -> None:
        self.send({"type": "turn_complete", "turn": turn})


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def until(page: Page, condition: Callable[[], bool], what: str, timeout_ms: int = 10_000) -> None:
    """Wait, while the browser runs (so routed sockets are served), until `condition()` holds."""
    for _ in range(timeout_ms // 20):
        if condition():
            return
        page.wait_for_timeout(20)
    raise AssertionError(f"timed out waiting for {what}")


@dataclass
class Walk:
    page: Page
    voice: FakeVoice
    label: str
    phone: bool
    screenshots: Path | None
    notes: list[str] = field(default_factory=list)

    def row(self, at: int = -1) -> Json:
        measured: object = self.page.evaluate(ROW_JS, at)
        check(isinstance(measured, dict), f"{self.label}: no row {at}")
        return {str(key): value for key, value in measured.items()}  # type: ignore[union-attr]

    def rows(self) -> int:
        return int(self.page.evaluate("() => document.querySelectorAll('#transcript > li:not(.seam)').length"))

    def frame(self) -> None:
        """Let the page draw what a frame brought."""
        self.page.wait_for_timeout(120)

    def tap(self, at: int = -1) -> None:
        """A finger, or the mouse, on the row's text: its first line, clear of everything else on it."""
        body = self.page.locator("#transcript > li:not(.seam)").nth(at if at >= 0 else self.rows() + at).locator(":scope > .body")
        box, area = body.bounding_box(), self.page.locator("#scroll-area").bounding_box()
        check(box is not None and area is not None, f"{self.label}: the row's text is not on screen to tap")
        assert box is not None and area is not None
        # The middle of the part of the text the list shows: a long card can run past either edge.
        top, foot = max(box["y"], area["y"]), min(box["y"] + box["height"], area["y"] + area["height"])
        check(foot - top >= 16, f"{self.label}: too little of the row's text is on screen to tap ({top}..{foot})")
        x, y = box["x"] + min(40.0, box["width"] / 2), (top + foot) / 2
        if self.phone:
            self.page.touchscreen.tap(x, y)
        else:
            self.page.mouse.click(x, y)
        self.page.wait_for_timeout(250)

    def shot(self, name: str) -> None:
        if self.screenshots is not None:
            self.page.screenshot(path=str(self.screenshots / f"{self.label}-{name}.png"))

    def clamped(self, row: Json, why: str) -> None:
        check(row["collapsed"] == "true", f"{self.label}: {why}: the row is not folded ({row})")
        check(float(row["bodyFull"]) > float(row["bodyShown"]) + 4, f"{self.label}: {why}: nothing is clamped ({row})")
        shown_lines = float(row["bodyShown"]) / float(row["lineHeight"])
        check(shown_lines <= 8.5, f"{self.label}: {why}: {shown_lines:.1f} lines show folded ({row})")

    def whole(self, row: Json, why: str) -> None:
        check(row["collapsed"] != "true", f"{self.label}: {why}: the row is folded ({row})")
        check(float(row["bodyFull"]) <= float(row["bodyShown"]) + 1, f"{self.label}: {why}: its end is clamped away ({row})")

    def at_newest(self, why: str) -> None:
        row = self.row()
        check(float(row["fromBottom"]) <= 24, f"{self.label}: {why}: the list is {row['fromBottom']}px above its newest line")
        check(float(row["bottom"]) <= float(row["areaHeight"]) + 1,
              f"{self.label}: {why}: the newest row's foot is below the list ({row})")


def start_call(walk: Walk) -> None:
    page = walk.page
    page.goto(f"{ORIGIN}/", wait_until="load")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#talk", state="visible", timeout=10_000)
    page.click("#talk")
    until(page, lambda: len(walk.voice.routes) == 1, "Talk to open the call's socket")
    page.wait_for_function("() => document.getElementById('status-line').dataset.state === 'live'", timeout=10_000)
    walk.voice.send({"type": "session_started", "greeting": False})


def expand_walk(walk: Walk) -> None:
    voice = walk.voice
    # Enough of a call above that the list scrolls at either size, each long answer arriving folded.
    for turn in range(1, 6):
        voice.say("user", turn, f"question {turn} about the overnight run")
        voice.say("assistant", turn, whole_answer(f"answer {turn}"))
        voice.done(turn)
    until(walk.page, lambda: walk.rows() == 10, "the call's first ten rows")
    walk.page.wait_for_selector("#transcript", state="visible", timeout=10_000)
    walk.frame()

    # 1. A reply streamed a sentence a frame: whole as it grows past the fold, the list following it.
    voice.say("user", 6, "so what happened overnight")
    grew_from = None
    for fragment in SPOKEN:
        voice.say("assistant", 6, fragment)
        walk.frame()
        row = walk.row()
        if grew_from is None and int(row["chars"]) <= FOLD_OVER_CHARS:
            grew_from = int(row["chars"])
        walk.whole(row, f"the streamed reply at {row['chars']} characters")
        walk.at_newest(f"the streamed reply at {row['chars']} characters")
    voice.done(6)
    walk.frame()
    row = walk.row()
    check(grew_from is not None and int(row["chars"]) > FOLD_OVER_CHARS + 60,
          f"{walk.label}: the streamed reply did not grow from short past the fold ({grew_from} -> {row['chars']})")
    check(float(row["height"]) >= 44, f"{walk.label}: the card is {row['height']}px tall, under a 44px tap target")
    check(row["foldWidth"] is not None and float(row["foldWidth"]) <= 2,
          f"{walk.label}: the fold button is printed on the card ({row['foldWidth']}px wide), unlike a channel row")
    walk.shot("1-streamed-whole")
    walk.notes.append(f"a reply streamed from {grew_from} to {row['chars']} characters stayed whole and followed")

    # 2. A tap on its text closes it to the clamp, and a second opens it whole.
    walk.tap()
    walk.clamped(walk.row(), "the streamed reply after a tap")
    walk.shot("2-tapped-closed")
    walk.tap()
    walk.whole(walk.row(), "the streamed reply after a second tap")
    walk.at_newest("the streamed reply opened again")
    walk.notes.append("a tap closed it and a second opened it")

    # 3. A long reply that arrives whole arrives folded, saying what is hidden; a tap opens it, on the newest line.
    voice.say("user", 7, "and the canary")
    voice.say("assistant", 7, whole_answer("arrived whole"))
    voice.done(7)
    walk.frame()
    row = walk.row()
    walk.clamped(row, "a long reply that arrived whole")
    check(bool(row["elided"]), f"{walk.label}: the folded reply says nothing of what is hidden ({row})")
    walk.at_newest("a long reply that arrived whole")
    walk.shot("3-arrived-folded")
    walk.tap()
    walk.whole(walk.row(), "the reply that arrived whole, tapped")
    walk.at_newest("the reply that arrived whole, opened")
    walk.shot("4-tapped-open")
    walk.notes.append(f"a reply that arrived whole arrived folded, {row['elided']!s}, and a tap opened it on the newest line")

    # 4. A fold the reader sets while the reply is spoken holds through the frames after it.
    voice.say("user", 8, "anything else")
    for fragment in SPOKEN[:7]:
        voice.say("assistant", 8, fragment)
    walk.frame()
    walk.whole(walk.row(), "the second streamed reply")
    walk.tap()
    for fragment in SPOKEN[7:9]:
        voice.say("assistant", 8, fragment)
        walk.frame()
        walk.clamped(walk.row(), "a reply closed by the reader, as it goes on")
    walk.shot("5-closed-while-spoken")
    walk.tap()
    voice.say("assistant", 8, SPOKEN[9])
    walk.frame()
    walk.whole(walk.row(), "a reply opened again by the reader, as it goes on")
    walk.at_newest("a reply opened again by the reader, as it goes on")
    voice.done(8)
    walk.notes.append("a reply closed while spoken stayed closed through two frames, and opened stayed open")

    # 5. Scrolled up the call, a tap on a folded card opens it, and neither it nor the line above moves.
    page = walk.page
    target = 3  # "answer 2", folded since it arrived
    page.evaluate("""(at) => {
      const area = document.getElementById('scroll-area');
      const li = document.querySelectorAll('#transcript > li:not(.seam)')[at];
      area.scrollTop += li.getBoundingClientRect().top - area.getBoundingClientRect().top - area.clientHeight / 3;
    }""", target)
    walk.frame()
    before, above = walk.row(target), walk.row(target - 1)
    walk.clamped(before, "the earlier answer before the tap")
    walk.tap(target)
    after, above_after = walk.row(target), walk.row(target - 1)
    walk.whole(after, "the earlier answer, tapped")
    check(abs(float(after["top"]) - float(before["top"])) <= 1,
          f"{walk.label}: opening the card moved it from {before['top']} to {after['top']}")
    check(abs(float(above_after["top"]) - float(above["top"])) <= 1,
          f"{walk.label}: opening the card moved the row above it from {above['top']} to {above_after['top']}")
    walk.shot("6-opened-scrolled-up")
    walk.notes.append(f"scrolled up, a card {before['height']:.0f}px folded opened to {after['height']:.0f}px where it stood")


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path,
                        help="directory to write a PNG of each state at each size into (default: none are kept)")
    return parser.parse_args()


def main() -> int:
    args = arguments()
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise SystemExit(
            "Python Playwright is required: python3 -m pip install --user playwright && "
            "python3 -m playwright install chromium"
        ) from error
    if args.screenshots is not None:
        args.screenshots.mkdir(parents=True, exist_ok=True)
    web_root = args.web_root.resolve()
    sizes = (("phone-393x852", 393, 852, True), ("desk-1280x800", 1280, 800, False))
    report: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, phone in sizes:
            api = FakeApi(web_root)
            voice = FakeVoice()
            with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-voice-expand-") as profile:
                media = ["--use-fake-ui-for-media-stream", "--use-fake-device-for-media-stream",
                         "--autoplay-policy=no-user-gesture-required"]
                if phone:
                    context = playwright.chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable, args=media,
                        viewport={"width": width, "height": height},
                        user_agent=("Mozilla/5.0 (Linux; Android 15; Pixel 7) AppleWebKit/537.36 "
                                    "(KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36"),
                        device_scale_factor=2.75, is_mobile=True, has_touch=True,
                    )
                else:
                    context = playwright.chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable, args=media,
                        viewport={"width": width, "height": height},
                    )
                try:
                    context.grant_permissions(["microphone"], origin=ORIGIN)
                    context.route("**/*", api.route)
                    context.route_web_socket("**/ws?mode=voice", voice.accept)
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    walk = Walk(page, voice, label, phone, args.screenshots)
                    start_call(walk)
                    expand_walk(walk)
                    check(not errors, f"{label}: the page threw: {errors}")
                    version = context.browser.version if context.browser else "Chromium"
                    report.append(f"{version} at {label}: " + "; ".join(walk.notes))
                finally:
                    context.close()
    for line in report:
        print(line)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
