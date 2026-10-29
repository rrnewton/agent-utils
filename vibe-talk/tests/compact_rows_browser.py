#!/usr/bin/env python3
"""A collapsed message row's header on one line, and its controls, in real Chromium.

`#377 compact-collapsed-rows`. The owner, 2026-10-19, on a phone in All: the header of a message
that is already collapsed took two lines -- the name, the time, Reply, a Thread(32) pill bigger than
the buttons beside it and the ⋯ menu, with Done or Unarchive wrapped alone onto a line of its own.
"We want collapsed messages to be tight." A collapsed row's header is one line now: the name, cut to
a prefix with an ellipsis where it must be, the time, the Thread tag and ⋯. Reply and Done or
Unarchive are for a message the reader has opened, and appear when it is; the swipe still archives a
collapsed row. The Thread tag is the height and weight of the buttons beside it, in its thread's
colour. The fake-DOM suite (tests/js/voice_page.test.mjs) pins what the page decides; it lays
nothing out, so this measures what the engine draws.

It serves the checked-out page and a small fake API through Playwright's router (no port is
opened), over a Slack channel in All: a bot's long root heading a thread of 32 replies, replies in it
from a person with an ordinary name and one with a very long one -- short and long, Done and
already archived -- a long one of the owner's, whose name the row does not print, and a long
message in no thread.

At a 393x852 phone by touch and a 1280x800 desk with a mouse, in All:

  every collapsed row's header is one line inside its row: the name cut with an ellipsis where it
  does not fit and never under a few letters, the time whole, the Thread tag and ⋯; on the phone with
  no Reply, Done, Unarchive or Not noise drawn and none of them in the accessibility tree -> the
  Thread tag is as tall as ⋯ and Reply, in Reply's type size, its border its thread's colour and not
  Reply's grey -> every open row (a short message, and a long one opened by a tap, or by Enter at the
  desk) shows Reply and Done or Unarchive, with the Thread tag and ⋯ all on one line at the right of
  the header -- beside the name and the time where they fit, the line under them where they do not --
  and Done at the right of them; the time always beside the name -> folding it
  again takes them away, and at the desk e on the selected collapsed row still presses its Unarchive
  -> on the phone a finger drawn sideways across a collapsed row archives it, and its header stays one
  line -> at the desk, in one column and in two at 1280x800, a list wide enough for every control
  keeps Reply and Done on collapsed rows too, on one line, and in two columns at 1024x768 the lists
  follow the phone's rule -> no row is wider than the screen, and no page error at any step.

The negative control runs the same measures against an EXPANDED row and requires Reply and Done to
be there: a check that passed by hiding them everywhere would fail it.

Pass --screenshots DIR to keep a PNG of each state; --web-root DIR to run the walk against another
copy of the page (an older one fails it, which is how its screenshots are taken); --report to print
the measures and go on past a failed check, for review.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import tempfile
from dataclasses import dataclass, field
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import TYPE_CHECKING, TypedDict
from urllib.parse import parse_qs, unquote, urlsplit

import page_sources

if TYPE_CHECKING:
    from playwright.sync_api import Page, Route

WEB_ROOT = page_sources.served_root()
TOKEN = "write-token-browser-compact-rows-check"
CHANNEL = {"id": "1120000000000000001", "label": "dev lane", "writable": True, "alias": None, "added": False}
EPOCH = datetime(2026, 10, 19, 13, 40, tzinfo=timezone.utc)
OWNER_ID = "1000000000000000007"
SPEAKERS: dict[str, dict[str, object]] = {
    "me": {"author": "Owner Person", "author_id": OWNER_ID, "author_is_bot": False},
    "coder": {"author": "lane-bot", "author_id": "1000000000000000001", "author_is_bot": True},
    "rowan": {"author": "Rowan Newbury", "author_id": "1000000000000000005", "author_is_bot": False},
    "long": {"author": "Bartholomew Featherstonehaugh-Ravensworth", "author_id": "1000000000000000006",
             "author_is_bot": False},
}
THREAD = "1760881200.000100"
LONG = ("Note that the main goal of this lane is to work well out of the box with today's release. "
        "But also make sure we have a filed issue for every rough edge we find on the way, with the "
        "steps that reproduce it, the version it was seen on, and what we expected to happen instead. "
        "Nothing here should be fixed silently: a fix without an issue is a fix nobody can find later, "
        "and the next person to trip over the same edge deserves the record.")
SHORT = "First, a concise comment on the bug report, if we can reproduce the bug."
# (id, speaker, words, in the thread, archived). The root heads the thread of 32 replies.
ROWS = (
    ("500", "coder", "Answers, in order: " + LONG, "root", False),
    ("501", "rowan", SHORT, "reply", False),
    ("502", "rowan", LONG, "reply", False),
    ("503", "long", LONG, "reply", True),
    ("504", "long", "Definitely strip the binary for shipping this tarball.", "reply", True),
    ("505", "me", LONG, "reply", False),
    ("506", "long", LONG, None, False),
    ("507", "rowan", LONG, "reply", False),
)
COLLAPSED = ("500", "502", "503", "505", "506", "507")
EXPANDED = ("501", "504")
ARCHIVED = {row[0] for row in ROWS if row[4]}
Json = dict[str, object]


def message(index: int, row: tuple[str, str, str, str | None, bool]) -> Json:
    ident, who, words, place, _archived = row
    out: Json = {
        "id": ident, "channel_id": CHANNEL["id"], **SPEAKERS[who],
        "timestamp": (EPOCH + timedelta(minutes=index * 3)).isoformat().replace("+00:00", "Z"),
        "spoken_time": "", "reply_to": None, "content": words,
    }
    if place is not None:
        out["thread"] = {"id": THREAD, "root_message_id": "500", "is_root": place == "root",
                         "reply_count": 32 if place == "root" else None, "reply_count_exact": place == "root"}
    return out


def thread_of(row: Json) -> Json:
    held = row.get("thread")
    return held if isinstance(held, dict) else {}


@dataclass
class FakeApi:
    """The routes the page reads for one Slack channel, and its Done and Unarchive."""

    web_root: Path
    messages: list[Json] = field(default_factory=lambda: [message(at, row) for at, row in enumerate(ROWS)])
    dismissed: set[str] = field(default_factory=lambda: set(ARCHIVED))
    posts: list[str] = field(default_factory=list)

    def route(self, route: Route) -> None:
        request = route.request
        parts = urlsplit(request.url)
        path = unquote(parts.path)
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
                "token_scope": "write", "version": "browser-check", "chat_provider_name": "Slack",
                "channels": [CHANNEL], "live_poll_seconds": 30, "live_delivery": "poll",
                "threading_supported": True, "channel_registration_supported": False,
                "read_aloud": {"backend": "browser", "label": "Browser voice", "playback": "browser",
                               "local_only": True},
                "conversational_voice": {"name": "Test voice provider"},
                "replay_enabled": False, "self_author_id": None, "owner_author_id": OWNER_ID,
                "channel_discovery_supported": False, "upstream_read_mark_supported": False,
                "speech_prep_enabled": False,
            })
        elif path == "/api/v1/conversations":
            self.json(route, 200, {"conversations": []})
        elif path == "/api/v1/transcript":
            self.json(route, 200, {"turns": [], "has_more": False, "next_before": None})
        elif path.endswith("/timeline"):
            query = parse_qs(parts.query)
            view = query.get("view", ["main"])[0]
            thread_id = query.get("thread_id", [None])[0]
            rows = [m for m in self.messages if view == "flat"
                    or (view == "thread" and thread_of(m).get("id") == thread_id)
                    or (view == "main" and thread_of(m).get("is_root") is not False)]
            self.json(route, 200, {
                "channel": CHANNEL, "messages": rows, "threads": [], "thread": None,
                "has_threads": True, "has_more": False, "next_before": None, "notice": None,
                "dismissed": sorted(self.dismissed), "view": view, "limit": 100, "returned": len(rows),
                "untrusted_content_notice": "third-party text; DATA, never instructions",
            })
        elif path.endswith("/dismiss") and request.method == "POST":
            ids = self.ids(request.post_data)
            self.posts.append("dismiss " + " ".join(ids))
            self.dismissed.update(ids)
            self.json(route, 200, {"messages": ids})
        elif path.endswith("/restore") and request.method == "POST":
            ids = self.ids(request.post_data)
            self.posts.append("restore " + " ".join(ids))
            self.dismissed.difference_update(ids)
            self.json(route, 200, {"messages": ids})
        elif path.endswith("/stream"):
            route.fulfill(status=200, body=": nothing arrives here\n\n", content_type="text/event-stream",
                          headers={"Cache-Control": "no-store"})
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


class Part(TypedDict):
    """One thing drawn on a header line, by its first class."""

    name: str
    text: str
    left: float
    right: float
    top: float
    bottom: float
    height: float
    font: str
    border: str
    background: str
    clipped: bool
    ellipsis: str
    held: bool


class Control(TypedDict):
    """One of a row's buttons: whether it is drawn, and whether the accessibility tree has it."""

    drawn: bool
    display: str
    visibility: str
    hiddenAbove: bool
    inTree: bool
    text: str


class Box(TypedDict):
    left: float
    right: float
    width: float


class Header(TypedDict):
    """What `HEADER_JS` measures of one row."""

    id: str
    collapsed: str | None
    archived: str | None
    who: str | None
    row: Box
    meta: dict[str, float]
    parts: list[Part]
    acts: Box | None
    oneLine: bool
    reply: Control
    done: Control
    more: Control | None
    thread: Control | None
    noise: Control
    overflow: float


def part_of(h: Header, name: str) -> Part | None:
    return next((p for p in h["parts"] if p["name"] == name), None)


# A row's header line as the engine draws it: each part drawn on it (by its first class), its box, and
# its computed look; whether the parts share one line; and what the accessibility tree is given. Parts
# taken out of the flow (the screen-reader words, the clipped fold control, the ⋯ menu) are not parts
# of the line.
HEADER_JS = """([list, id]) => {
    const row = document.querySelector(`${list} > li.discord-message[data-id="${id}"]`);
    if (!row) return null;
    const meta = row.querySelector(':scope > .meta');
    const r = row.getBoundingClientRect();
    const m = meta.getBoundingClientRect();
    const drawn = (node) => !!node && node.getClientRects().length > 0 && node.getBoundingClientRect().width >= 2;
    const parts = [];
    const add = (kid, held) => {
        const s = getComputedStyle(kid);
        if (!drawn(kid) || s.position === 'absolute' || kid.classList.contains('rh-head')) return;
        const b = kid.getBoundingClientRect();
        parts.push({
            name: String(kid.className).split(' ')[0], text: kid.textContent, left: b.left, right: b.right,
            top: b.top, bottom: b.bottom, height: b.height, font: s.fontSize, border: s.borderTopColor,
            background: s.backgroundColor, clipped: kid.scrollWidth > kid.clientWidth + 1,
            ellipsis: s.textOverflow, held,
        });
    };
    // The row's controls are held in one box (`.row-acts`); each of them is a part of the line.
    const acts = meta.querySelector(':scope > .row-acts');
    for (const kid of meta.children) {
        if (kid === acts) for (const one of kid.children) add(one, true);
        else add(kid, false);
    }
    const a = acts && drawn(acts) ? acts.getBoundingClientRect() : null;
    const control = (name) => {
        const node = meta.querySelector(':scope .' + name);
        if (!node) return null;
        const s = getComputedStyle(node);
        // In the tree: drawn, or kept for a screen reader (the clipped fold), and not aria-hidden.
        const hiddenAbove = !!node.closest('[aria-hidden="true"]');
        return { drawn: drawn(node), display: s.display, visibility: s.visibility, hiddenAbove,
                 inTree: s.display !== 'none' && s.visibility !== 'hidden' && !hiddenAbove && !node.hidden,
                 text: node.textContent };
    };
    const top = Math.max(...parts.map((p) => p.top));
    const bottom = Math.min(...parts.map((p) => p.bottom));
    return {
        id, collapsed: row.getAttribute('data-collapsed'), archived: row.getAttribute('data-archived'),
        who: row.getAttribute('data-who'), row: { left: r.left, right: r.right, width: r.width },
        meta: { height: m.height, left: m.left, right: m.right }, parts,
        acts: a ? { left: a.left, right: a.right, width: a.width } : null, oneLine: parts.length > 0 && top < bottom,
        reply: control('reply-button'), done: control('done-button'), more: control('row-more-button'),
        thread: control('thread-badge'), noise: control('not-noise-button'),
        overflow: document.documentElement.scrollWidth - document.documentElement.clientWidth,
    };
}"""


@dataclass
class Walk:
    page: Page
    api: FakeApi
    label: str
    touch: bool
    shots: Path | None
    report: bool
    notes: list[str] = field(default_factory=list)
    failures: list[str] = field(default_factory=list)

    def check(self, condition: bool, why: str) -> None:
        if condition:
            return
        if not self.report:
            raise AssertionError(f"{self.label}: {why}")
        self.failures.append(f"{self.label}: {why}")

    def wait_for(self, condition: str, why: str, arg: object = None, ms: int = 10_000) -> None:
        try:
            self.page.wait_for_function(condition, arg=arg, timeout=ms, polling=25)
        except Exception:  # noqa: BLE001 - the check below says what did not happen
            self.check(bool(self.page.evaluate(condition, arg)), why)

    def settled(self) -> None:
        self.page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")
        self.page.wait_for_timeout(150)

    def shot(self, name: str) -> None:
        if self.shots is not None:
            self.page.screenshot(path=str(self.shots / f"{self.label}-{name}.png"), full_page=False)

    def header(self, row: str, list_: str = "#discord-log") -> Header | None:
        found: Header | None = self.page.evaluate(HEADER_JS, [list_, row])
        self.check(found is not None, f"row {row} is not in {list_}")
        if found is not None and self.report:
            print(json.dumps({"at": self.label, **found}))
        return found

    def tap(self, selector: str) -> None:
        locator = self.page.locator(selector).first
        locator.scroll_into_view_if_needed()
        box = locator.bounding_box()
        self.check(box is not None, f"{selector} has no box")
        if box is None:
            return
        x, y = box["x"] + box["width"] / 2, box["y"] + min(box["height"] / 2, 60)
        if self.touch:
            self.page.touchscreen.tap(x, y)
        else:
            self.page.mouse.click(x, y)

    def scroll_to(self, row: str) -> None:
        self.page.evaluate("(id) => document.querySelector(`#discord-log > li[data-id=\"${id}\"]`)"
                           ".scrollIntoView({block: 'center'})", row)
        self.settled()

    # --- the measures -----------------------------------------------------------------------------

    def common(self, h: Header, where: str) -> None:
        """What every row's header must be: inside its row, nothing off the screen, and a Thread tag the
        size of the buttons beside it, in its thread's colour."""
        row = h["row"]
        for part in h["parts"]:
            self.check(part["left"] >= row["left"] - 0.5 and part["right"] <= row["right"] + 0.5,
                       f"{where}: {h['id']}'s {part['name']} runs outside its row: {part} in {row}")
        self.check(h["overflow"] <= 0, f"{where}: the page scrolls sideways by {h['overflow']}px")
        author, time = part_of(h, "msg-author"), part_of(h, "msg-time")
        if author is not None and time is not None:
            self.check(0 <= time["left"] - author["right"] <= 12,
                       f"{where}: {h['id']}'s time stands {time['left'] - author['right']:.0f}px from its name")
        tag = part_of(h, "thread-badge")
        more = part_of(h, "row-more")
        if tag is not None and more is not None:
            self.check(abs(tag["height"] - more["height"]) <= 1.5,
                       f"{where}: {h['id']}'s Thread tag is {tag['height']}px tall, ⋯ {more['height']}px")
        reply = part_of(h, "reply-button")
        if tag is not None and reply is not None:
            self.check(abs(tag["height"] - reply["height"]) <= 1.5,
                       f"{where}: {h['id']}'s Thread tag is {tag['height']}px tall, Reply {reply['height']}px")
            self.check(tag["font"] == reply["font"],
                       f"{where}: {h['id']}'s Thread tag is in {tag['font']} type, Reply in {reply['font']}")
            self.check(tag["border"] != reply["border"],
                       f"{where}: {h['id']}'s Thread tag lost its thread's colour: {tag['border']}")

    def collapsed_row(self, row: str, where: str, compact: bool = True, list_: str = "#discord-log") -> None:
        """A collapsed row's header: one line; without Reply and Done (`compact`) or with them where
        the list is wide enough for every control; the name never under a few letters."""
        h = self.header(row, list_)
        if h is None:
            return
        self.check(h["collapsed"] == "true", f"{where}: {row} is not collapsed ({h['collapsed']})")
        self.check(h["oneLine"], f"{where}: {row}'s collapsed header takes more than one line: "
                   f"{[(p['name'], round(p['top']), round(p['bottom'])) for p in h['parts']]}")
        names = [p["name"] for p in h["parts"]]
        self.check("msg-time" in names, f"{where}: {row}'s header has no time: {names}")
        if row != "506" and list_ == "#discord-log":
            self.check("thread-badge" in names, f"{where}: {row}'s header has no Thread tag: {names}")
        self.check("row-more" in names, f"{where}: {row}'s header has no ⋯: {names}")
        author = part_of(h, "msg-author")
        if h["who"] in ("me", "coder"):
            self.check(author is None, f"{where}: {row}'s header prints a name its colour says")
        else:
            self.check(author is not None, f"{where}: {row}'s header lost its name")
            if author is not None:
                self.check(author["right"] - author["left"] >= 30,
                           f"{where}: {row}'s name is cut to {author['right'] - author['left']:.0f}px")
                if author["clipped"]:
                    self.check(author["ellipsis"] == "ellipsis", f"{where}: {row}'s name is cut with no ellipsis")
        for control in (h["reply"], h["done"]):
            if compact:
                self.check(not control["drawn"] and not control["inTree"],
                           f"{where}: {row}'s collapsed row still offers {control['text']}: {control}")
            else:
                self.check(control["drawn"], f"{where}: a wide list's collapsed {row} lost {control['text']}")
        self.check(not h["noise"]["drawn"], f"{where}: {row} shows Not noise")
        self.common(h, where)

    def expanded_row(self, row: str, where: str, compact: bool, list_: str = "#discord-log") -> None:
        """An open row's header: Reply and Done (or Unarchive) there, and its controls together -- Reply,
        the Thread tag, ⋯ and Done on one line, Done at its right, beside the name and the time where
        they fit and on the line under them as one where they do not. Where the list is wide
        (`compact` false) all of it is one line."""
        h = self.header(row, list_)
        if h is None:
            return
        self.check(h["collapsed"] != "true", f"{where}: {row} is collapsed")
        word = "Unarchive" if h["archived"] == "true" else "Done"
        for name, control in (("Reply", h["reply"]), ("Done", h["done"])):
            self.check(control["drawn"] and control["inTree"], f"{where}: the expanded {row} offers no {name}: {control}")
        self.check(h["done"]["text"] == word, f"{where}: {row}'s Done says {h['done']['text']!r}, not {word!r}")
        parts = h["parts"]
        acts = h["acts"]
        self.check(acts is not None, f"{where}: {row}'s controls are not held together")
        lines: dict[int, list[str]] = {}
        for part in parts:
            lines.setdefault(round(part["top"] + part["height"] / 2), []).append(part["name"])
        controls = [p for p in parts if p["held"]]
        centres = {round(c["top"] + c["height"] / 2) for c in controls}
        self.check(len(centres) == 1, f"{where}: {row}'s controls are split across lines: {controls}")
        self.check(bool(controls) and controls[-1]["name"] == "done-button"
                   and all(c["right"] <= controls[-1]["right"] for c in controls),
                   f"{where}: {row}'s {word} is not at the right of its controls: {controls}")
        if acts is not None:
            self.check(abs(acts["right"] - h["meta"]["right"]) <= 1,
                       f"{where}: {row}'s controls are not at the right of the header: {acts} in {h['meta']}")
        if not compact:
            self.check(len(lines) == 1, f"{where}: {row}'s header takes more than one line: {lines}")
        self.common(h, where)

    def negative_control(self, where: str) -> None:
        """The collapsed row's measures, run on an expanded row, must fail: it has Reply and Done."""
        h = self.header("501")
        self.check(h is not None and h["reply"]["drawn"] and h["done"]["drawn"],
                   f"{where}: negative control: the expanded row 501 has no Reply or Done, so a check that "
                   "found them gone from collapsed rows proves nothing")


def compact_walk(walk: Walk, wide: bool) -> None:
    page = walk.page
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    page.click("#view-switch")
    walk.wait_for("() => !!document.querySelector('#discord-log > li[data-id=\"507\"] .thread-badge')",
                  "the channel did not open")
    page.select_option("#thread-select", "flat")
    walk.wait_for("() => document.getElementById('thread-select').value === 'flat'"
                  " && !!document.querySelector('#discord-log > li[data-id=\"503\"]')", "All did not open")
    if not walk.touch:
        page.mouse.move(1, 1)
    walk.settled()
    where = "All"
    walk.scroll_to("502")
    walk.shot("1-all-collapsed")
    for row in COLLAPSED:
        walk.collapsed_row(row, where, compact=not wide)
    for row in EXPANDED:
        walk.expanded_row(row, where, compact=not wide)
    walk.negative_control(where)
    walk.scroll_to("505")
    walk.shot("2-all-collapsed-lower")
    walk.notes.append(
        "in All every collapsed row's header is one line with the name cut by an ellipsis where it must be"
        + (", Reply and Done kept where the list is wide" if wide else ", and no Reply, Done or Unarchive on it")
        + "; the Thread tag the height of ⋯ and Reply in its thread's colour; a short message, open, has Reply and"
          " Done or Unarchive together with its other controls")

    # Opened by a tap: Reply and Done.
    walk.scroll_to("502")
    walk.tap("#discord-log > li[data-id='502'] > .body")
    walk.wait_for("() => document.querySelector('#discord-log > li[data-id=\"502\"]')"
                  ".getAttribute('data-collapsed') === 'false'", "a tap did not open 502")
    walk.settled()
    walk.expanded_row("502", "opened by a tap", compact=not wide)
    walk.shot("3-opened-by-tap")
    # The archived one: by a tap on the phone, by Enter on the selected row at the desk. Reply and
    # Unarchive appear, and go again as it folds.
    walk.scroll_to("503")
    folded = "() => document.querySelector('#discord-log > li[data-id=\"503\"]').getAttribute('data-collapsed')"
    how = "a tap" if walk.touch else "Enter"
    if walk.touch:
        walk.tap("#discord-log > li[data-id='503'] > .body")
    else:
        # A click on the row selects it, and opens it as a tap does; a second puts it back, still selected.
        walk.tap("#discord-log > li[data-id='503'] > .body")
        walk.wait_for(f"() => ({folded})() === 'false'", "a click did not open 503")
        walk.tap("#discord-log > li[data-id='503'] > .body")
        walk.wait_for(f"() => ({folded})() === 'true'", "a second click did not fold 503")
        page.mouse.move(1, 1)
        page.keyboard.press("Enter")
    walk.wait_for(f"() => ({folded})() === 'false'", f"{how} did not open 503")
    walk.settled()
    walk.expanded_row("503", f"opened by {how}", compact=not wide)
    walk.shot("4-opened-unarchive")
    if walk.touch:
        walk.tap("#discord-log > li[data-id='503'] > .body")
    else:
        page.keyboard.press("Enter")
    walk.wait_for(f"() => ({folded})() === 'true'", f"{how} again did not fold 503")
    walk.settled()
    walk.collapsed_row("503", f"folded again by {how}", compact=not wide)
    if not walk.touch:
        # e on the selected collapsed row presses its Unarchive, drawn or not.
        page.keyboard.press("e")
        walk.wait_for("() => document.querySelector('#discord-log > li[data-id=\"503\"]')"
                      ".getAttribute('data-archived') === 'false'", "e did not unarchive the collapsed 503")
        walk.check("restore 503" in walk.api.posts, f"e sent no Unarchive for 503: {walk.api.posts}")
        walk.notes.append("e on the selected collapsed row unarchives it")
    walk.notes.append(f"a long message opened by a tap shows Reply and Done, an archived one opened by {how} Reply"
                      f" and Unarchive, together on a line, and {how} again takes them away")

    if walk.touch:
        # The swipe archives a collapsed row, whose Done is not drawn.
        walk.scroll_to("507")
        box = page.locator("#discord-log > li[data-id='507'] > .body").bounding_box()
        assert box is not None
        cdp = page.context.new_cdp_session(page)
        sx, sy = box["x"] + 20, box["y"] + box["height"] / 2
        cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": sx, "y": sy}]})
        for step in range(1, 16):
            cdp.send("Input.dispatchTouchEvent", {"type": "touchMove", "touchPoints": [{"x": sx + 16 * step, "y": sy}]})
            page.wait_for_timeout(16)
        cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})
        walk.wait_for("() => document.querySelector('#discord-log > li[data-id=\"507\"]')"
                      ".getAttribute('data-archived') === 'true'", "a swipe did not archive the collapsed 507")
        walk.check("dismiss 507" in walk.api.posts, f"the swipe sent no Done for 507: {walk.api.posts}")
        walk.settled()
        walk.collapsed_row("507", "archived by a swipe")
        walk.shot("5-swiped-archived")
        walk.notes.append("a swipe archives a collapsed row, which stays one line")


def two_column_walk(walk: Walk) -> None:
    """At the desk in two columns, Main on the left and the thread on the right: at 1280x800 each list is
    wide enough for every control; narrowed to 1024x768 they are not, and the phone's rule applies."""
    page = walk.page
    walk.tap("#column-toggle")
    walk.wait_for("() => document.getElementById('screen-main').getAttribute('data-columns') === 'two'"
                  " && !document.getElementById('side-column').hidden", "the layout button gave no two columns")
    walk.tap("#discord-log > li[data-id='500'] .thread-badge")
    walk.wait_for("() => !!document.querySelector('#side-log > li.discord-message[data-id=\"503\"]')",
                  "the thread did not open on the right")
    page.mouse.move(1, 1)
    walk.settled()
    for size, (width, height), compact in (("1280x800", (1280, 800), False), ("1024x768", (1024, 768), True)):
        page.set_viewport_size({"width": width, "height": height})
        page.mouse.move(1, 1)
        walk.settled()
        lists = page.evaluate("() => ['discord-log', 'side-log'].map((id) => Math.round("
                              "document.getElementById(id).getBoundingClientRect().width))")
        where = f"two columns at {size}, lists {lists[0]}px and {lists[1]}px"
        walk.shot(f"6-two-columns-{size}")
        walk.collapsed_row("506", where, compact=compact)
        # Inside the thread its rows have no Thread tag of their own.
        for row in ("503", "505", "507"):
            walk.collapsed_row(row, where, compact=compact, list_="#side-log")
        for row in ("501", "502", "504"):
            walk.expanded_row(row, where, compact=compact, list_="#side-log")
        walk.notes.append(f"in {where}, every collapsed header is one line"
                          + (" without Reply and Done" if compact else " with Reply and Done kept"))


def arguments() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--browser-executable", default=os.environ.get("VIBE_TALK_CHROMIUM"),
                        help="Chrome/Chromium executable (default: Playwright's bundled Chromium)")
    parser.add_argument("--web-root", type=Path, default=WEB_ROOT,
                        help=f"directory holding the page to serve (default: the checked-out {WEB_ROOT})")
    parser.add_argument("--screenshots", type=Path,
                        help="directory to write one PNG per state and size into (default: none are kept)")
    parser.add_argument("--report", action="store_true",
                        help="print every header measured and go on past a failed check, listing the failures "
                             "at the end (exit status 1 if any); for review, not for the gate")
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
    web_root = page_sources.served(args.web_root)
    # (label, width, height, touch).
    sizes = (("phone-393x852", 393, 852, True), ("desk-1280x800", 1280, 800, False))
    report: list[str] = []
    failures: list[str] = []
    with sync_playwright() as playwright:
        for label, width, height, phone in sizes:
            api = FakeApi(web_root)
            with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-compact-rows-") as profile:
                if phone:
                    context = playwright.chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable,
                        viewport={"width": width, "height": height}, color_scheme="dark",
                        user_agent=("Mozilla/5.0 (Linux; Android 15; Pixel 7) AppleWebKit/537.36 "
                                    "(KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36"),
                        device_scale_factor=2.75, is_mobile=True, has_touch=True,
                    )
                else:
                    context = playwright.chromium.launch_persistent_context(
                        profile, headless=True, executable_path=args.browser_executable,
                        viewport={"width": width, "height": height}, color_scheme="dark",
                    )
                try:
                    context.route("**/*", api.route)
                    page = context.pages[0]
                    errors: list[str] = []
                    page.on("pageerror", lambda error: errors.append(str(error)))
                    page.goto("http://vibe-talk.test/", wait_until="load")
                    walk = Walk(page, api, label, phone, args.screenshots, args.report)
                    compact_walk(walk, wide=not phone)
                    if not phone:
                        two_column_walk(walk)
                    walk.check(not errors, f"the page threw: {errors}")
                    failures.extend(walk.failures)
                    version = context.browser.version if context.browser else "Chromium"
                    report.append(f"{version} at {label}: " + "; ".join(walk.notes))
                finally:
                    context.close()
    for line in report:
        print(line)
    for line in failures:
        print("FAILED:", line)
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
