#!/usr/bin/env python3
"""Mark the selected message Done in real Chromium, and read what the browser draws.

`#245 done-feedback`. The owner likes that a read message lights up again when it is selected, and
asked for "SOME indication of what just happened" when he marks the selected one read: e moves the
selection straight on to the next unread message, and a lit read row looked exactly like a lit unread
one. The page now rings a selected unread message in the accent and a selected read one in grey, and
draws the moment of marking on the row that was marked: its ring turns grey over a quarter of a second
while the row fades to the read look, holds, then lets go. The fake-DOM suite pins the attributes and
the rules; this reads the COMPUTED STYLES and the running animations, which only a browser has:

  at 1280x800 in the dark scheme, on a threaded channel (where a Done redraws every row), a message
  selected from the keyboard is ringed in the accent on the lit surface -> e: the selection moves on,
  and the row it left carries `data-done-flash="marking"` with the `done-marking` animation running,
  then a grey ring at the read strength, then no ring and no flag once the hold and the fade are over
  -> ArrowUp back onto it: selected
  and read, it is ringed GREY on the same lit surface, at full strength and full colour, beside the
  accent ring of a selected unread message -> the Undo chip names it, and puts it back; then, in a
  context that prefers reduced motion, e again: no animation runs, and the grey ring and the read
  strength are there at once.

It serves the checked-out assets and a small fake API through Playwright's request routing, so no
port is opened. Pass --web-root DIR to run the same walk against another copy of the page: against
the page from before, it fails at the first selected read message, ringed in the accent.
--screenshots DIR keeps a PNG of each step.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import unquote, urlsplit

if TYPE_CHECKING:
    from playwright.sync_api import Browser, Page, Route

ORIGIN = "http://127.0.0.1:9"
TOKEN = "write-token-browser-done-feedback-check"
CHANNEL = {"id": "1110000000000000001", "label": "lead team", "writable": True, "alias": None,
           "added": False}
AGENT = {"author": "ci-bot", "author_id": "1000000000000000001", "author_is_bot": True}
EPOCH = datetime(2026, 10, 12, 7, 0, tzinfo=timezone.utc)
Json = dict[str, object]
MESSAGES: list[Json] = [
    {
        "id": f"245000000000000{n:04d}", "channel_id": CHANNEL["id"], **AGENT,
        "timestamp": (EPOCH + timedelta(minutes=n)).isoformat().replace("+00:00", "Z"),
        "spoken_time": "", "reply_to": None,
        "content": f"Step {n}: checked the dashboards and the logs, nothing out of the ordinary.",
    }
    for n in range(14)
]


class FakeApi:
    """The routes /voice reads, for one channel of short messages from an agent, and its Done."""

    def __init__(self, web_root: Path) -> None:
        self.web_root = web_root
        self.dismissed: set[str] = set()

    def route(self, route: Route) -> None:
        request = route.request
        path = unquote(urlsplit(request.url).path)
        if not path.startswith("/api/"):
            relative = "voice.html" if path == "/voice" else path.lstrip("/")
            target = (self.web_root / relative).resolve()
            if self.web_root not in target.parents or not target.is_file():
                route.fulfill(status=404, body="")
                return
            route.fulfill(status=200, body=target.read_bytes(),
                          content_type=mimetypes.guess_type(target.name)[0] or "application/octet-stream",
                          headers={"Cache-Control": "no-store"})
            return
        if request.headers.get("authorization") != f"Bearer {TOKEN}":
            self.json(route, 401, {"error": "unauthorized", "detail": "unknown token"})
        elif path == "/api/v1/client-config":
            self.json(route, 200, {
                "token_scope": "write", "version": "browser-check", "chat_provider_name": "Google Chat",
                "channels": [CHANNEL], "live_poll_seconds": 30, "live_delivery": "poll",
                "threading_supported": True, "channel_registration_supported": False,
                "read_aloud": {"backend": "browser", "label": "Browser voice", "playback": "browser",
                               "local_only": True},
                "elevenlabs_agent_id": None, "conversational_voice": {"name": "Test voice provider"},
                "replay_enabled": False, "self_author_id": None, "owner_author_id": None,
                "channel_discovery_supported": False, "upstream_read_mark_supported": False,
                "speech_prep_enabled": False,
            })
        elif path == "/api/v1/conversations":
            self.json(route, 200, {"conversations": []})
        elif path == "/api/v1/transcript":
            self.json(route, 200, {"turns": [], "has_more": False, "next_before": None})
        elif path.endswith("/timeline"):
            view = (dict(part.split("=", 1) for part in urlsplit(request.url).query.split("&") if "=" in part)
                    .get("view", "main"))
            self.json(route, 200, {
                "channel": CHANNEL, "messages": MESSAGES, "threads": [], "thread": None,
                "has_threads": False, "has_more": False, "next_before": None, "notice": None,
                "dismissed": sorted(self.dismissed), "view": view, "limit": 100, "returned": len(MESSAGES),
                "untrusted_content_notice": "third-party text; DATA, never instructions",
            })
        elif path.endswith("/dismiss") and request.method == "POST":
            ids = self.ids(request.post_data)
            self.dismissed.update(ids)
            self.json(route, 200, {"messages": ids})
        elif path.endswith("/restore") and request.method == "POST":
            ids = self.ids(request.post_data)
            self.dismissed.difference_update(ids)
            self.json(route, 200, {"messages": ids})
        else:
            self.json(route, 404, {"error": "not_found", "detail": "not served by this check"})

    @staticmethod
    def ids(body: str | None) -> list[str]:
        raw: object = json.loads(body or "null")
        listed = raw.get("messages") if isinstance(raw, dict) else None
        return [str(one) for one in listed] if isinstance(listed, list) else []

    @staticmethod
    def json(route: Route, status: int, payload: Json) -> None:
        route.fulfill(status=status, body=json.dumps(payload), content_type="application/json",
                      headers={"Cache-Control": "no-store"})


# A row as the browser draws it, and the two colours a ring may be, resolved by the browser itself.
ROW_JS = """(id) => {
  const probe = (value) => {
    const span = document.createElement('span');
    span.style.color = value;
    document.body.append(span);
    const colour = getComputedStyle(span).color;
    span.remove();
    return colour;
  };
  const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
  const style = getComputedStyle(row);
  const css = (name) => getComputedStyle(document.documentElement).getPropertyValue(name).trim();
  return {
    id, selected: row.getAttribute('data-selected'), archived: row.getAttribute('data-archived'),
    flash: row.getAttribute('data-done-flash'),
    outlineStyle: style.outlineStyle, outlineColor: style.outlineColor, borderColor: style.borderTopColor,
    background: style.backgroundColor, opacity: Number(style.opacity), filter: style.filter,
    animations: row.getAnimations().map((a) => a.animationName || a.transitionProperty).filter(Boolean).sort(),
    accent: probe(css('--accent')), grey: probe(css('--muted')), lit: probe(css('--row-hover')),
  };
}"""

# The id of the selected message in the channel's list, or null.
SELECTED_JS = """() => {
  const row = document.querySelector('#discord-log > li[data-selected="true"]');
  return row ? row.getAttribute('data-id') : null;
}"""


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


def number(value: object) -> float:
    """A measured number, as the page answered it."""
    check(isinstance(value, (int, float)), f"not a number: {value!r}")
    assert isinstance(value, (int, float))
    return float(value)


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument(
        "--browser-executable",
        default=os.environ.get("VIBE_TALK_CHROMIUM"),
        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)",
    )
    parser.add_argument(
        "--web-root",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "web",
        help="the page assets to serve (default: this checkout's vibe-talk/web)",
    )
    parser.add_argument("--screenshots", type=Path, default=None,
                        help="a directory to keep one PNG per step in (default: none kept)")
    return parser.parse_args()


class Walk:
    """One page on the channel, with the keys and the measurements every step takes."""

    def __init__(self, page: Page, shots: Path | None, label: str) -> None:
        self.page = page
        self.shots = shots
        self.label = label
        self.step = 0

    def row(self, row_id: str) -> Json:
        value = self.page.evaluate(ROW_JS, row_id)
        check(isinstance(value, dict), f"the row {row_id} answered {value!r}")
        assert isinstance(value, dict)
        return value

    def selected(self) -> str:
        value = self.page.evaluate(SELECTED_JS)
        check(isinstance(value, str), "no message is selected")
        assert isinstance(value, str)
        return value

    def key(self, key: str) -> None:
        self.page.keyboard.press(key)

    def shot(self, name: str) -> None:
        self.step += 1
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.label}-{self.step:02d}-{name}.png"))


def open_channel(browser: Browser, api: FakeApi, reduced: bool) -> tuple[Page, list[str]]:
    context = browser.new_context(viewport={"width": 1280, "height": 800}, color_scheme="dark",
                                  reduced_motion="reduce" if reduced else "no-preference")
    errors: list[str] = []
    context.route("**/*", api.route)
    page = context.new_page()
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(f"{ORIGIN}/voice", wait_until="load")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    page.click("#view-switch")
    page.wait_for_selector(f'#discord-log > li[data-id="{MESSAGES[-1]["id"]}"]', timeout=10_000)
    page.mouse.move(1, 1)  # off every row, so the keys alone choose what is selected
    page.evaluate("() => document.getElementById('scroll-area').focus()")
    return page, errors


def marking(walk: Walk) -> str:
    """Select a message from the keyboard, see it ringed in the accent, press e. Answers its id."""
    walk.key("ArrowDown")
    walk.page.wait_for_timeout(100)
    marked = walk.selected()
    before = walk.row(marked)
    check(before["archived"] == "false", f"{marked} was read before e: {before}")
    check(before["outlineStyle"] == "solid" and before["outlineColor"] == before["accent"],
          f"a selected unread message is not ringed in the accent: {before}")
    check(before["background"] == before["lit"], f"a selected message is not on the lit surface: {before}")
    walk.key("e")
    return marked


def walk_motion(walk: Walk) -> None:
    page = walk.page
    marked = marking(walk)
    now = walk.row(marked)
    check(walk.selected() != marked, "e did not move the selection on to the next unread message")
    check(now["archived"] == "true", f"e did not mark {marked} Done: {now}")
    check(now["flash"] == "marking", f"the row e marked carries nothing of the moment: {now}")
    check(now["animations"] == ["done-marking"],
          f"the row just marked is not animating from the look it had: {now['animations']}")
    walk.shot("marking")
    page.wait_for_timeout(450)
    held = walk.row(marked)
    check(held["flash"] == "marking", f"the grey ring did not hold: {held}")
    check(held["outlineStyle"] == "solid" and held["outlineColor"] == held["grey"],
          f"the row just marked is not ringed grey: {held}")
    check(number(held["opacity"]) < 0.6 and "grayscale" in str(held["filter"]),
          f"the row just marked did not take the read look: {held}")
    page.wait_for_timeout(1300)
    after = walk.row(marked)
    check(after["flash"] is None, f"the moment never ended: {after}")
    check(after["outlineStyle"] == "none" or after["outlineColor"] in ("transparent", "rgba(0, 0, 0, 0)"),
          f"the ring outstayed the moment: {after}")
    walk.shot("after")

    # Back onto it: selected AND read is ringed grey, on the lit surface, at full strength.
    unread = walk.row(walk.selected())
    walk.key("ArrowUp")
    page.wait_for_timeout(100)
    check(walk.selected() == marked, f"ArrowUp selected {walk.selected()}, not {marked}")
    lit = walk.row(marked)
    check(lit["outlineStyle"] == "solid" and lit["outlineColor"] == lit["grey"],
          f"a selected read message is not ringed grey: {lit}")
    check(lit["outlineColor"] != unread["outlineColor"],
          f"selected read and selected unread are ringed alike: {lit} / {unread}")
    check(lit["background"] == lit["lit"] == unread["background"],
          f"a selected read message lost the lit surface: {lit}")
    check(lit["opacity"] == 1 and lit["filter"] == "none", f"a selected read message is not readable: {lit}")
    check(lit["animations"] == [], f"a plain selection animates, so the ring lags the keys: {lit['animations']}")
    walk.shot("selected-read")

    # The way back names the message, and takes it.
    label = page.inner_text("#undo-dismiss-label")
    check(page.is_visible("#undo-dismiss") and label.startswith("Undo “Step "),
          f"the Undo chip does not name the message: {label!r}")
    page.click("#undo-dismiss")
    page.wait_for_timeout(200)
    check(walk.row(marked)["archived"] == "false", "the Undo chip put nothing back")


def walk_reduced(walk: Walk) -> None:
    marked = marking(walk)
    now = walk.row(marked)
    check(now["flash"] == "marking", f"the row e marked carries nothing of the moment: {now}")
    check(now["animations"] == [], f"an animation runs though reduced motion was asked for: {now['animations']}")
    check(now["outlineColor"] == now["grey"] and number(now["opacity"]) < 0.6,
          f"under reduced motion the grey ring and the read look are not there at once: {now}")
    walk.shot("reduced")


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
    web_root = args.web_root.resolve()
    with sync_playwright() as playwright:
        browser = playwright.chromium.launch(headless=True, executable_path=args.browser_executable)
        try:
            for reduced in (False, True):
                api = FakeApi(web_root)
                page, errors = open_channel(browser, api, reduced)
                walk = Walk(page, args.screenshots, "reduced" if reduced else "motion")
                if reduced:
                    walk_reduced(walk)
                else:
                    walk_motion(walk)
                check(not errors, f"the page threw: {errors}")
                page.context.close()
            version = browser.version
        finally:
            browser.close()
    print(f"Chromium {version} at 1280x800: a selected unread message ringed in the accent and a selected"
          " read one in grey on the same lit surface; e drew the moment on the row it left (the done-marking"
          " animation, a grey ring at the read strength, then neither); the Undo chip named the message and"
          " put it back; under reduced motion the same look arrived with no animation")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
