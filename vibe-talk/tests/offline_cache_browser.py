#!/usr/bin/env python3
"""Exercise /voice's saved-message snapshot in real Chromium at a phone's size.

The fake-DOM suite (tests/js/voice_page.test.mjs) covers every branch of the snapshot. This covers
what only a browser engine can: real localStorage, a real reload, real fetch failures, the Cache
Storage and service-worker registries, and layout at 412x915 and at a 1280x800 desk. For each of
those it serves the checked-out assets and a small fake API from one loopback origin, then walks
one reader through:

  sign in, open the channel — in All, the default where the provider has threads — and read Threads
  and Main too -> reload with the network held: the page reopens on the channel in All by itself,
  its saved rows drawn before any answer (`#189 restore-ui-state`) -> one bounded newest-page read
  of All, sent once the token is proved and the live stream has attached, merges a new row without
  duplicating -> switching views makes no request -> a reload with the API unreachable reopens on
  Main, the view chosen in that channel and kept as its own (`#205 channel-view-memory`), at the
  message a reader parked mid-list was on, keeps the rows and says so -> a reload whose refresh
  fails keeps them and says that -> signing out removes them from the screen and the device. Before
  the reloads, a refresh also fails and recovers in place, on the channel's own poll, under a reader
  at the top, in the middle and at the newest line.

While the freshness pill is up — refreshing, offline, failed — it must cover neither the view tabs
nor the list's header seam or first row, and #scroll-area must be the same box with it as without
it (`#32 freshness-pill-overlap`). As the pill's room and the error panel above the list come and
go, a scrolled reader's line stays where it is on the screen, and a reader at the newest line stays
there (`#33 error-banner-scroll-shift`).

On a touchscreen, a real finger parked at the newest line and drawn up past the end of the list
reads the channel once, says "Updated" at the foot of the list and does not reload the page; a drag
short of the threshold reads nothing (`#188 pull-refresh-bottom`).

At the newest line there is no jump to the newest message; halfway up the channel it is at the lower
left of the chip row in the dark theme — legible, a 44px target, inside the list's column and above
the dock, clear of the floating pills and of every other chip forced up beside it, at the ordinary
type size and at 150%, where on a phone the row wraps and the others go up a line. A real tap a few
pixels above the drawn chip lands on the newest message, reads nothing, starts no pull, and the jump
is gone. It is decided again when the list changes size under a reader who has not scrolled, with the
list's scroll events held back from the page: it comes when the newest message grows below a reader on
the newest line, and goes when it shrinks under one just above it (`#207 scrollback-jump`).

Opening a thread whose title is longer than the screen, with a long unbroken token and inline code
in its reply, leaves the page no wider than the viewport and no shown part of the main screen past
its right edge; on a phone the title is ellipsised instead (`#36 thread-view-phone-overflow`). With
every floating chip showing at once and that thread scrolled to its end, no chip covers the reply
composer's text box or its Send button (`#194 thread-picker-polish`).

The search glass floats on the pill's line rather than costing a header row (`#197
floating-search`): on the main screen no header strip stands above the list, on the call view and
the channel alike; wherever the pill is measured the glass is centred on its line, over the list and
clear of it and of the head of the list, drawn larger than the pill and at least 36px across since
the owner called the pill-sized glass very small (`#211 link-filter`), and a square around the disc
presses it. A real tap below the
disc opens the search as a bar across the top of the list — no wider than the list, not hanging
over what is above it, the pill given way and the head of the list clear of it — the bar filters,
and the glass folds it back with #scroll-area unmoved. Settings still has its title bar and a way
back, and in a thread neither floating thing covers the heading's Back button or title.

Last, a fresh page signed in with a read-scope token reads the channel without a single 4xx answer
or console error: the stored-conversation routes are write-scope, answered 403 here as the server
answers them, and the page must not ask (`#38 read-token-conversation-probe`).

Then, in the dark theme, a second reader pins (`#206 pin-message`): a pin made elsewhere shows on its
row as a "Pinned" chip whose words read against it, and a gold edge; the ⋯ menus of that row and of
an unpinned one open as Copy text, Pin, and the two that mark read under a hairline and a caption —
for a provider that can move its own read marker, whose menu is the widest — every item at least
44px tall, on the screen, and answering a tap at its centre; Pin shows at once and reaches the
server; and the open search bar carries the Pinned filter immediately left of the glass, with Links
immediately left of it — each a 44px target every point of which presses it, clear of the other, of
the glass's own square and of a field still 140px wide beside the count — whose tap shows the pins,
clear of the bar, and which the glass takes away with the bar.

Then, in the dark theme, a reader looks for links (`#211 link-filter`): with the bar open, Links and
Pinned measure as above, and a real tap on Links leaves only the rows that hold a link, each showing
its links where its text was, under its author and time — one per line, each a real link opening a
new tab, at least 44px tall, a Markdown or chat-service link by its name, and an address longer than
the row cut short on screen with its href whole. A real tap on a link opens a new tab at that address
without navigating the app, folding the row or opening its details; the glass takes Links away with
the bar; and over the call view, which has no pins, Links stands beside the glass on its own.

And where a phone reader's own settings wrap a row's meta line — the page zoomed to a 313px
viewport, or the root font at 115%, 130% and 150% — every row's ⋯ menu, and the pinned row's in the
Pinned filter, opens wholly on the screen with every item a thumb can reach, in both provider shapes,
and that row's Unpin is tapped for real. A menu hung from the "⋯" opened off the left of the screen
wherever the "⋯" wrapped to the start of a line. Under the same settings, at 360px and 412px, the open
search bar with Links on and its count up keeps the glass full size and each filter a 44px target, and
its field still leaves room for a query's text, which buttons that grew with the font took away; the
Links view under it shows each link on its own 44px line, clear of the bar; and the kinds of link
under Links measure as below.

Then the kinds of link under Links (`#212 link-filter`), in the dark theme at 412px, 360px, 360px at
150% type and a 1280px desk, over a channel holding GitHub pull requests, commits and runs of Actions
beside documents: a real tap on Links brings up PRs, Commits and Actions, all on, with how many links
of each are in view, in a row below the bar and under the Links icon — centred on it, or right-aligned
under the bar's end where centred would run past it — wholly inside the viewport, each a target at
least 44px each way every point of which presses it, covering neither the field, the glass, the
filters, the pill nor the first row, its words legible on and off and the two states drawn apart. A
real tap on PRs takes the pull requests out of the rows that hold them, and a row left with nothing
goes; Commits and Actions off as well leave only the documents and the GitHub issue, which is no kind.
After a reload the three are still off; turned on again every link is back; the glass takes the row
away; and over the call view, where Links stands beside the glass, the row is right-aligned under the
bar's end and still inside the screen. With over a hundred links of each kind loaded, every button
reading "99+", the row measures the same at 313px, and at 360px under 130% and 150% type.

And where the filters empty the list (`EMPTY_SIZES`): a phone reader at the channel's newest line
turns all three kinds off, types text that only a row without a link matches, types text nothing
matches, or turns Links on over a channel with no link — at 412px under 150% type and on the two
phones with the keyboard up, where the composer below the emptied list still overflows it. The
sentence saying why is then wholly on the screen, below the bar and the kinds of link, with nothing
covering a word of it and no jump to the newest message offered; undone, the rows come back with the
reader at the newest line.

Then, in a second fresh profile at each size, a channel whose foot holds five replies to messages
above them (`#204 reply-arrow`): from the owner, the agent and a third party, and between them in
every state a row recedes in — the owner's own, read by default; one somebody answered; one swiped
Done; an automatic placeholder; and one in none of them. Every reply's arrow is a 44px square in
the gutter to the left of its own box: touching the box, inside the list, within the row's height,
covering none of the row's text or buttons, and the thing a tap on it lands on. On a screenshot of
that square, the mark is drawn in the accent at 3:1 or better against the page beside it, however
far its row recedes — measured in pixels, because a computed colour knows nothing of the opacity and
filter of the row around it. All of that at the default type size, at 150% type and in the dark
scheme, with the page no wider than the viewport. A real tap on the newest reply's arrow, from the
newest line, brings the message it answers under the floating pill and lights it without folding
the reply; "Back to reply", tapped, brings the reply back and goes; and in reading mode the arrow
still only jumps, with no read of the reply asked for.

Then, in a third fresh profile at each size and in the dark theme, the coding agent's tiles (`#213
row-side-borders`), beside the owner's and a third party's rows and in every state that draws a
row's border differently — summarised, its summary failed, being read, asked to be read, the kept
place, pinned, swiped Done, an automatic placeholder, a reply, the owner's own read by default, one
somebody answered, and on a desk under the pointer. Every row has four drawn sides, and the agent's
tiles are square, with a left and a right side the width, style and colour of their top, but for a
summary's bar, which is the left side and wider than the right. On a phone a tile reaches from one
edge of the screen to the other, a reply's from its gutter to the right edge; on a desk it is inside
the reading column. Nothing on the page scrolls sideways, at 360px as at the other sizes.

Pass --screenshots DIR to keep a PNG of each step for review.
"""

from __future__ import annotations

import argparse
import base64
import json
import mimetypes
import os
import re
import tempfile
import threading
import time
from datetime import datetime, timedelta, timezone
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, unquote, urlsplit

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, FloatRect, Page, Route


WEB_ROOT = Path(__file__).resolve().parents[1] / "web"
TOKEN = "write-token-browser-cache-check"
READ_TOKEN = "read-token-browser-cache-check"
SCOPES = {f"Bearer {TOKEN}": "write", f"Bearer {READ_TOKEN}": "read"}
CHANNEL = {"id": "1110000000000000001", "label": "lead team", "writable": True, "alias": None,
           "added": False}
CACHE_KEY = "vibe-talk.voice.message-cache"
UI_STATE_KEY = "vibe-talk.voice.ui-state"
EPOCH = datetime(2026, 1, 5, 9, 0, tzinfo=timezone.utc)
# `#36 thread-view-phone-overflow`: wider than a phone as words, and unbreakable at its tail.
LONG_TOKEN = "0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0-0f1e2d3c4b5a69788796a5b4c3d2e1f0"
LONG_TITLE = f"Overnight coordinator for the release lane #link {LONG_TOKEN}"
# `#211 link-filter`. The smallest the search glass may be drawn. It was the freshness pill's height,
# 26px, until the owner called it "very small" on his phone.
GLASS_MIN_PX = 36
# `#211 link-filter`, from review. The least room the search field may leave for its TEXT — inside its
# own padding — on the open bar with both filters and a count beside it: seven characters of its 17px
# type, about 8.5px each, a word or two of a query. Measured where a reader's own settings narrow the
# line (`BAR_SCALES`), where buttons sized in rem once left it none at all.
FIELD_TEXT_MIN_PX = 60
Json = dict[str, object]


def message(index: int, content: str, thread: Json | None = None) -> Json:
    row: Json = {
        "id": str(200 + index),
        "channel_id": CHANNEL["id"],
        "author": "ci-bot",
        "author_id": "1000000000000000001",
        "author_is_bot": True,
        "timestamp": (EPOCH + timedelta(minutes=index)).isoformat().replace("+00:00", "Z"),
        "spoken_time": "",
        "reply_to": None,
        "content": content,
    }
    if thread is not None:
        row["thread"] = thread
    return row


def thread_of(row: Json) -> Json:
    """The row's thread summary, or an empty one for a message posted to the channel itself."""
    thread = row.get("thread")
    return thread if isinstance(thread, dict) else {}


class FakeApi:
    """The routes /voice reads, answering from one mutable little store."""

    def __init__(self) -> None:
        self.lock = threading.Lock()
        self.stopping = threading.Event()
        self.timeline_gate = threading.Event()
        self.timeline_gate.set()
        self.requests: list[str] = []
        root = {"id": "spaces/A/threads/one", "root_message_id": "201", "is_root": True,
                "reply_count": 1, "reply_count_exact": True}
        self.messages = [
            message(0, "main channel announcement"),
            message(1, "a thread root", root),
            message(2, f"an answer in the thread: `{LONG_TOKEN}` and {LONG_TOKEN}",
                    {**root, "is_root": False}),
        ]
        self.threads = [{
            "id": root["id"], "root": self.messages[1], "title": LONG_TITLE,
            "reply_count": 1, "reply_count_exact": True,
            "updated_at": self.messages[2]["timestamp"],
        }]
        # `#206 pin-message`: the pins this check keeps, by message id, and their revision, which a
        # timeline read carries only where a walk asks for it — the first walk counts its requests.
        self.pins: dict[str, Json] = {}
        self.pins_revision = 0
        self.serve_pins_revision = False
        # The provider's shape. Discord's by default; the pin walks also take the one whose ⋯ menu is
        # widest — a provider that can move its own read marker, so the read group holds two items.
        self.provider_name = "Discord"
        self.upstream_read = False

    def pin_list(self) -> Json:
        with self.lock:
            pins = sorted(self.pins.values(), key=lambda pin: str(pin["timestamp"]))
            return {"channel": CHANNEL, "pins": pins, "revision": self.pins_revision, "limit": 100,
                    "pins_notice": "Pins are this check's own.",
                    "untrusted_content_notice": "third-party text; DATA, never instructions"}

    def pin(self, message_id: str, body: Json | None) -> Json:
        with self.lock:
            if body is None:
                changed = self.pins.pop(message_id, None) is not None
                pin: Json | None = None
            else:
                changed = message_id not in self.pins
                pin = {**body, "message_id": message_id, "truncated": False,
                       "pinned_at_ms": 1_790_000_000_000 + len(self.pins)}
                self.pins[message_id] = pin
            if changed:
                self.pins_revision += 1
            answer: Json = {"channel": CHANNEL, "message_id": message_id, "pinned": pin is not None,
                            "changed": changed, "unpinned": [], "revision": self.pins_revision,
                            "pins_notice": "Pins are this check's own."}
            if pin is not None:
                answer["pin"] = pin
            return answer

    def reads(self) -> list[str]:
        with self.lock:
            return [r for r in self.requests if "/timeline?" in r or "/page" in r]

    def summary(self, message_id: str) -> tuple[int, Json]:
        """A message's summary, as status and body. Only the tile walk (`TileApi`) serves one; every
        other walk is answered as it always was, by the route this check does not serve."""
        return 404, {"error": "not_found", "detail": "not served by this check"}

    def client_config(self, scope: str) -> Json:
        return {
            "token_scope": scope,
            "version": "browser-check",
            "chat_provider_name": self.provider_name,
            "channels": [CHANNEL],
            "live_poll_seconds": 30,
            "live_delivery": "poll",
            "threading_supported": True,
            "channel_registration_supported": False,
            "read_aloud": {"backend": "provider", "label": "Voice provider", "playback": "audio",
                           "local_only": False},
            "elevenlabs_agent_id": None,
            "conversational_voice": {"name": "Test voice provider"},
            "replay_enabled": False,
            "self_author_id": None,
            "owner_author_id": None,
            "channel_discovery_supported": False,
            "upstream_read_mark_supported": self.upstream_read,
            "speech_prep_enabled": True,
        }

    def timeline(self, query: dict[str, list[str]]) -> Json:
        view = query.get("view", ["main"])[0]
        thread_id = query.get("thread_id", [None])[0]
        with self.lock:
            rows = [m for m in self.messages if view == "flat"
                    or (view == "thread" and thread_of(m).get("id") == thread_id)
                    or (view == "main" and (not thread_of(m) or thread_of(m)["is_root"]))]
            threads = list(self.threads) if view == "threads" else []
            thread = next((t for t in self.threads if t["id"] == thread_id), None)
        answer: Json = {
            "channel": CHANNEL, "messages": rows, "threads": threads,
            "thread": thread if view == "thread" else None,
            "has_threads": True, "has_more": False, "next_before": None, "notice": None,
            "dismissed": [], "view": view, "limit": 50, "returned": len(rows) + len(threads),
            "untrusted_content_notice": "third-party text; DATA, never instructions",
        }
        if self.serve_pins_revision:
            answer["pins_revision"] = self.pins_revision
        return answer


# `#204 reply-arrow`. The owner, a third party and the agent, as the page tells them apart.
OWNER_ID = "1000000000000000007"
SPEAKERS: dict[str, Json] = {
    "me": {"author": "owner", "author_id": OWNER_ID, "author_is_bot": False},
    "coder": {"author": "ci-bot", "author_id": "1000000000000000001", "author_is_bot": True},
    "human": {"author": "alice", "author_id": "1000000000000000005", "author_is_bot": False},
}


def said(index: int, content: str, who: str = "coder", reply_to: str | None = None) -> Json:
    """`message`, from one of the three speakers and answering `reply_to` when it is given."""
    return {**message(index, content), **SPEAKERS[who], "reply_to": reply_to}


# The reply the reader has swiped Done, as the server reports it.
ARCHIVED_REPLY = "213"


class ReplyApi(FakeApi):
    """`#204 reply-arrow`: a question, more than a screen of the agent's long answers under it, and
    five replies at the foot: from each kind of speaker, so every gutter is measured, and in every
    state a row recedes in, so every arrow is seen as it is drawn on a row that has faded.

    211, the owner's, is already read as his own and answered by 212; 212, the agent's, is answered
    by 213; 213 is swiped Done; 214 is an automatic placeholder; 215 is in none of those, and is the
    one tapped."""

    def __init__(self) -> None:
        super().__init__()
        filler = ("The overnight run is still going: the integration shard has been retried twice "
                  "and the artifact upload is waiting on the cache to warm. ") * 3
        self.messages = [
            said(0, "Is the overnight runner wedged? It has not reported since 03:10."),
            *(said(i, f"Step {i}. {filler}") for i in range(1, 11)),
            said(11, "Restarted it from the console; it should report within the minute.", "me", "200"),
            said(12, "It reported at 03:42 and the queue is draining.", "coder", "211"),
            said(13, "Good, that matches what the dashboard shows here.", "human", "212"),
            {**said(14, "_Working…_", "coder", "213"), "noise": True},
            said(15, "The same thing happened on the other runner last week: it was the disk.", "human", "200"),
        ]
        self.threads = []

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope), "owner_author_id": OWNER_ID}

    def timeline(self, query: dict[str, list[str]]) -> Json:
        return {**super().timeline(query), "dismissed": [ARCHIVED_REPLY]}


# `#211 link-filter`. Where the Links walk's links go. Answered by the browser context itself, so a
# tapped link opens a real tab without anything leaving this machine.
LINK_HOSTS = ("https://example.com/**", "https://example.org/**")
LONG_ADDRESS = "https://example.com/reports/" + "nightly-integration-shard-" * 6 + "summary.html"


class LinkApi(FakeApi):
    """`#211 link-filter`: messages with links in every form the Links view reads, and some without.

    200 is long enough to fold and holds a bare address and a Markdown link; 201 holds none; 202 is a
    chat service's `<address|name>`; 203 is an address longer than any phone is wide; 204 holds none.
    """

    def __init__(self) -> None:
        super().__init__()
        filler = ("The integration shard was retried twice overnight and the artifact upload waited on "
                  "the cache to warm before it went through. ") * 2
        self.messages = [
            said(0, f"{filler}The build log is at https://example.com/build/7, and the change itself is "
                    "[the diff](https://example.org/diff/7)."),
            said(1, "Nothing to open in this one."),
            said(2, "Dashboard: <https://example.com/dash|the dashboard>", "human"),
            said(3, f"Full report: {LONG_ADDRESS}"),
            said(4, "Also nothing to open here.", "me"),
        ]
        self.threads = []


# `#212 link-filter`. GitHub links of every kind the buttons under Links hide, and documents beside them,
# in one repository of a neutral example project.
REPO = "https://github.com/example/project"


class KindsApi(FakeApi):
    """`#212 link-filter`: GitHub pull requests, commits and runs of Actions beside documents.

    200 holds a pull request and a document; 201 a run of Actions and a commit named in Markdown; 202 a
    commit by its full id; 203 a document and a GitHub issue, which is no kind; 204 holds no link; 205
    is a dozen pull requests' files and checks, so the PRs button carries a two-digit number.
    """

    def __init__(self) -> None:
        super().__init__()
        prs = " ".join(f"{REPO}/pull/{50 + n}/{'files' if n % 2 else 'checks'}" for n in range(12))
        self.messages = [
            said(0, f"Review please: {REPO}/pull/41 and the design note https://example.com/docs/design.html"),
            said(1, f"CI failed at {REPO}/actions/runs/123456/job/789 on [abc1234]({REPO}/commit/abc1234def)"),
            said(2, f"Landed in {REPO}/commit/0123456789abcdef0123456789abcdef01234567", "human"),
            said(3, f"Spec: [the spec](https://example.org/spec) and the issue {REPO}/issues/7"),
            said(4, "Nothing to open in this one.", "me"),
            said(5, f"The rest of the queue: {prs}"),
        ]
        self.threads = []


class CrowdedKindsApi(FakeApi):
    """`#212 link-filter`: over a hundred links of each kind, so every button under Links reads "99+".

    The widest the row of kinds ever gets, at the narrowest sizes a reader's own settings make: 200 a
    hundred and twenty pull requests, 201 as many commits, 202 as many runs of Actions, 203 a document.
    """

    def __init__(self) -> None:
        super().__init__()
        self.messages = [
            said(0, " ".join(f"{REPO}/pull/{n}" for n in range(120))),
            said(1, " ".join(f"{REPO}/commit/{n:07x}abc" for n in range(120))),
            said(2, " ".join(f"{REPO}/actions/runs/{n}" for n in range(120))),
            said(3, "The one document: https://example.com/docs/guide.html"),
        ]
        self.threads = []


class EmptyKindsApi(FakeApi):
    """`#212 link-filter`, from review: a channel whose only links are of the three kinds — 200 a pull
    request, 201 a commit, 202 a run of Actions — and 203 none, so turning all three off empties the
    Links view, and text that matches only 203 empties it with every kind on."""

    def __init__(self) -> None:
        super().__init__()
        self.messages = [
            said(0, f"Review please: {REPO}/pull/41"),
            said(1, f"Landed in {REPO}/commit/abc1234def", "human"),
            said(2, f"CI is green: {REPO}/actions/runs/123456"),
            said(3, "Nothing to open in this one.", "me"),
        ]
        self.threads = []


class LinklessApi(FakeApi):
    """`#212 link-filter`, from review: a channel with no link in it at all, so Links on finds none."""

    def __init__(self) -> None:
        super().__init__()
        self.messages = [
            said(0, "The overnight run is still going."),
            said(1, "It reported at 03:42 and the queue is draining.", "human"),
            said(2, "Good, that matches the dashboard here."),
            said(3, "Nothing to open in this one either.", "me"),
        ]
        self.threads = []


# `#213 row-side-borders`. The rows of the tile walk that are put in a state by name: the one whose
# summary arrives, the one whose summary fails, and the three this check marks itself (see
# `tile_walk`).
SUMMARISED_ROW, FAILED_SUMMARY_ROW = "207", "200"
READING_ROW, MARKED_ROW, PENDING_ROW = "208", "204", "205"


class TileApi(FakeApi):
    """`#213 row-side-borders`: the coding agent's tiles beside the owner's and a third party's rows,
    and a row in every state that draws a row's border differently.

    At the newest line: 206, the owner's, already read as his own; 207, the agent's, long enough to
    fold, and summarised; 208, the agent's, the row being read; 209, a third party's, answered by 210,
    the agent's reply to it; and 211, the agent's, in no state at all. At the top: 200, the agent's,
    whose summary fails; 201, pinned elsewhere; 202, swiped Done; 203, an automatic placeholder; 204,
    the kept place; and 205, asked to be read and not yet speaking."""

    def __init__(self) -> None:
        super().__init__()
        filler = ("The overnight run is still going: the integration shard has been retried twice "
                  "and the artifact upload is waiting on the cache to warm. ") * 3
        self.messages = [
            said(0, f"Shard report. {filler}"),
            said(1, "The cache warmed at 03:20 and the upload is retrying."),
            said(2, "Shard 4 passed on its second attempt."),
            {**said(3, "_Working…_"), "noise": True},
            said(4, "The artifact upload finished at 03:31."),
            said(5, "Restarting the integration shard now."),
            said(6, "Is the overnight runner wedged? It has not reported since 03:10.", "me"),
            said(7, f"Runner status. {filler}"),
            said(8, "It reported at 03:42 and the queue is draining."),
            said(9, "Good, that matches what the dashboard shows here.", "human"),
            said(10, "Thanks. I will keep watching the queue until it is empty.", "coder", "209"),
            said(11, "The next run starts at 04:00, and I will report when it does."),
        ]
        self.threads = []
        self.serve_pins_revision = True
        self.pins["201"] = {
            "message_id": "201", "author": "ci-bot", "author_id": "1000000000000000001", "author_is_bot": True,
            "content": str(self.messages[1]["content"]), "truncated": False,
            "timestamp": str(self.messages[1]["timestamp"]), "thread_id": None, "thread_root": False,
            "pinned_at_ms": 1_790_000_000_000,
        }
        self.pins_revision = 1

    def client_config(self, scope: str) -> Json:
        return {**super().client_config(scope), "owner_author_id": OWNER_ID}

    def timeline(self, query: dict[str, list[str]]) -> Json:
        return {**super().timeline(query), "dismissed": ["202"]}

    def summary(self, message_id: str) -> tuple[int, Json]:
        if message_id == FAILED_SUMMARY_ROW:
            return 503, {"error": "summarizer_error", "detail": "the summariser did not answer (browser check)"}
        return 200, {
            "channel": CHANNEL, "message_id": message_id, "state": "generated",
            "summary": "The runner is fine: a shard was retried and the upload waited on the cache.",
            "backend": "browser check", "generated_in_ms": 900, "version": "v1-browser-check",
            "threshold_chars": 280, "untrusted_content_notice": "third-party text; DATA, never instructions",
        }


def handler_for(api: FakeApi) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            parts = urlsplit(self.path)
            path = unquote(parts.path)
            if not path.startswith("/api/"):
                self.asset(path)
                return
            with api.lock:
                api.requests.append(f"GET {self.path}")
            scope = SCOPES.get(self.headers.get("Authorization") or "")
            if scope is None:
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path == "/api/v1/client-config":
                self.json(200, api.client_config(scope))
            elif path in ("/api/v1/conversations", "/api/v1/transcript"):
                # Write-scope routes, refused to a read token as the server refuses them.
                if scope != "write":
                    self.json(403, {"error": "forbidden", "detail": "this token may read but not post"})
                elif path == "/api/v1/conversations":
                    self.json(200, {"conversations": []})
                else:
                    self.json(200, {"turns": [], "has_more": False, "next_before": None})
            elif path.endswith("/timeline"):
                api.timeline_gate.wait(20)
                self.json(200, api.timeline(parse_qs(parts.query)))
            elif path.endswith("/pins"):
                self.json(200, api.pin_list())
            elif summary := re.fullmatch(r"/api/v1/channels/[^/]+/messages/([^/]+)/summary", path):
                self.json(*api.summary(summary.group(1)))
            elif path.endswith("/stream"):
                self.stream()
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def do_POST(self) -> None:  # noqa: N802
            with api.lock:
                api.requests.append(f"POST {self.path}")
            self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def do_PUT(self) -> None:  # noqa: N802
            self.pin_write(json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"{}"))

        def do_DELETE(self) -> None:  # noqa: N802
            self.pin_write(None)

        def pin_write(self, body: Json | None) -> None:
            """`#206 pin-message`: a PUT or DELETE of one pin, the only writes this check answers."""
            path = unquote(urlsplit(self.path).path)
            with api.lock:
                api.requests.append(f"{'PUT' if body is not None else 'DELETE'} {self.path}")
            found = re.fullmatch(r"/api/v1/channels/[^/]+/pins/(.+)", path)
            if not found:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})
            elif SCOPES.get(self.headers.get("Authorization") or "") != "write":
                self.json(403, {"error": "forbidden", "detail": "this token may read but not write"})
            else:
                self.json(200, api.pin(found.group(1), body))

        def asset(self, request_path: str) -> None:
            relative = "voice.html" if request_path == "/voice" else request_path.lstrip("/")
            target = (WEB_ROOT / (relative or "index.html")).resolve()
            if WEB_ROOT not in target.parents or not target.is_file():
                self.send_error(404)
                return
            body = target.read_bytes()
            content_type = mimetypes.guess_type(target.name)[0] or "application/octet-stream"
            self.send_response(200)
            self.send_header("Content-Type", content_type)
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
                pass  # a held read the browser gave up on when the page reloaded

        def stream(self) -> None:
            # Held open with keep-alive comments, as the live route is; nothing arrives on it.
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            try:
                while not api.stopping.wait(1):
                    self.wfile.write(b": keep-alive\n\n")
                    self.wfile.flush()
            except OSError:
                pass
            self.close_connection = True

        def log_message(self, _format: str, *_args: object) -> None:
            pass

    return Handler


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
        help="directory to write one PNG per step into (default: none are kept)",
    )
    return parser.parse_args()


def check(condition: bool, message: str) -> None:
    if not condition:
        raise AssertionError(message)


# Park the reader (`where`: "top", "middle", "newest", or "" to stay put), then say where they are:
# the top of the thread root's row on the screen, how far the newest line is below the fold, the scroll
# offset, the list's bottom edge, and whether the error panel is up with something in it.
PLACE_JS = """async (where) => {
    const area = document.getElementById('scroll-area');
    const range = area.scrollHeight - area.clientHeight;
    if (where) area.scrollTop = {top: 0, middle: Math.round(range / 2), newest: area.scrollHeight}[where];
    await new Promise((settled) => requestAnimationFrame(() => requestAnimationFrame(settled)));
    const row = document.querySelector('#discord-log > li[data-id="201"]');
    const error = document.getElementById('error-wrap');
    return {row: row.getBoundingClientRect().top, gap: area.scrollHeight - area.clientHeight - area.scrollTop,
            top: area.scrollTop, bottom: area.getBoundingClientRect().bottom,
            banner: !error.hidden && error.getClientRects().length > 0
                && document.getElementById('error').textContent.trim() !== ''};
}"""

# Everything that can be covered, measured at the head of the list, with the view tabs and without
# them. `problems` is empty when the pill covers none of the tabs, the header seams or the first row,
# fits on its one line, and sits over the list; `area` is #scroll-area's box and `bare` the same box without the pill.
# Scrolling to the top is free only because FakeApi answers has_more: False; with more history it
# would page older rows in and spend a read the "one newest-page read" check counts.
GEOMETRY_JS = """() => {""" + f"""
    const GLASS_MIN_PX = {GLASS_MIN_PX};""" + """
    const area = document.getElementById('scroll-area');
    area.scrollTop = 0;
    const shown = (e) => Boolean(e) && !e.hidden && e.getClientRects().length > 0;
    const box = (e) => e.getBoundingClientRect();
    const meets = (a, b) => a.top < b.bottom && b.top < a.bottom && a.left < b.right && b.left < a.right;
    const pill = document.getElementById('channel-freshness');
    const tabs = document.getElementById('channel-view-tabs');
    const problems = [];
    const glassProblems = (layout, p, list) => {
        const glass = document.getElementById('search-toggle');
        if (!shown(glass)) return [`${layout}: the search glass is not on screen`];
        const g = box(glass), found = [], mid = (r) => (r.top + r.bottom) / 2;
        const off = mid(g) - mid(p);
        if (Math.abs(off) > 1.5) found.push(`${layout}: the glass is centred ${off.toFixed(1)}px off the pill's line`);
        // `#211 link-filter`: drawn larger than the pill since the owner called the pill-sized one
        // "very small", and still a disc.
        if (g.height < GLASS_MIN_PX || g.height < p.height + 8 || Math.abs(g.width - g.height) > 0.5) {
            found.push(`${layout}: the glass is ${g.width}x${g.height}px beside a ${p.height}px pill`);
        }
        if (meets(p, g)) found.push(`${layout}: the glass overlaps the pill "${pill.textContent}"`);
        if (g.top < list.top || g.left < list.left || g.right > list.right) found.push(`${layout}: the glass is not over the list`);
        // ...and clear of the head of the list, which makes room for it as it does for the pill.
        const head = [...document.querySelectorAll('#pane-discord .seam, #discord-log > li')].find(shown);
        if (head && box(head).top < g.bottom - 0.5) found.push(`${layout}: the glass reaches ${(g.bottom - box(head).top).toFixed(1)}px over the head of the list`);
        // The 48px target: beside and below the disc, outside it but inside the square, is the glass.
        const cx = (g.left + g.right) / 2, cy = mid(g);
        for (const [dx, dy] of [[-22, 0], [20, 0], [0, 22]]) {
            const hit = document.elementFromPoint(cx + dx, cy + dy);
            if (!hit || !glass.contains(hit)) {
                const what = hit ? (hit.id ? `#${hit.id}` : hit.tagName.toLowerCase()) : 'nothing';
                found.push(`${layout}: a tap at (${dx}, ${dy})px from the glass's centre lands on ${what}`);
            }
        }
        return found;
    };
    const covering = (layout) => {
        const p = box(pill), list = box(area);
        if (shown(tabs) && meets(p, box(tabs))) problems.push(`${layout}: it covers the Main/Threads/All tabs`);
        const seams = [...document.querySelectorAll('#pane-discord .seam')].filter(shown);
        if (seams.length === 0) problems.push(`${layout}: there is no header seam to measure against`);
        seams.forEach((seam) => {
            if (meets(p, box(seam))) problems.push(`${layout}: it covers the seam "${seam.textContent.trim()}"`);
        });
        const first = document.querySelector('#discord-log > li');
        if (!shown(first)) problems.push(`${layout}: there is no first row to measure against`);
        else if (meets(p, box(first))) problems.push(`${layout}: it covers the first row`);
        if (pill.scrollWidth > pill.clientWidth + 1) problems.push(`${layout}: it is cut short: ${pill.textContent}`);
        // ...and to the fraction of a pixel, which the check above rounds away: a flex line that
        // took a quarter of a pixel off the pill ellipsised the whole of its time (`#197
        // floating-search`) — an ellipsis removes whole glyphs, however small the shortfall. Its
        // width with nothing constraining it is the width it needs; 1/64px is layout's own unit.
        const saved = pill.getAttribute('style');
        pill.style.cssText = 'flex: none; max-width: none; width: max-content';
        const natural = box(pill).width;
        if (saved === null) pill.removeAttribute('style'); else pill.setAttribute('style', saved);
        if (natural > p.width + 1 / 64) {
            problems.push(`${layout}: it needs ${natural.toFixed(2)}px and has ${p.width.toFixed(2)}: ${pill.textContent}`);
        }
        if (p.top < list.top || p.left < list.left || p.right > list.right) {
            problems.push(`${layout}: it is not over the list`);
        }
        // `#197 floating-search`: the glass on the pill's line, the pill's height, clear of it.
        // Measured in every pill state, because the longest sentences are the ones that reach it.
        glassProblems(layout, p, list).forEach((problem) => problems.push(problem));
    };
    // An empty header strip over the list is the defect `#197 floating-search` removed.
    const header = document.getElementById('topbar');
    if (shown(header)) {
        problems.push(`a ${Math.round(box(header).height)}px header strip stands over the main screen`);
    }
    if (!shown(pill)) {
        problems.push('the freshness pill is not shown');
    } else {
        covering('with tabs');
        // A source without threads hides the tabs by this same attribute, and the grid row they
        // held collapses: the layout the overlap was first seen in.
        const tabsHidden = tabs.hidden;
        tabs.hidden = true;
        covering('without tabs');
        tabs.hidden = tabsHidden;
    }
    const size = () => {
        const a = box(area);
        return [a.top, a.left, a.width, a.height, area.clientWidth, area.clientHeight]
            .map((n) => n.toFixed(2)).join(',');
    };
    // The same box with the pill and its room taken away, in the same state, so that a banner
    // appearing for its own reasons is not mistaken for the pill resizing the list.
    const measured = size(), pane = document.getElementById('pane-discord');
    const wasHidden = pill.hidden, reserved = pane.hasAttribute('data-freshness');
    pill.hidden = true;
    pane.removeAttribute('data-freshness');
    const bare = size();
    pill.hidden = wasHidden;
    if (reserved) pane.setAttribute('data-freshness', '');
    // `gchat-thread-selector`: the tabs left the list for the bar's thread picker.
    const picker = document.getElementById('thread-select');
    if (shown(tabs)) problems.push('the Main/Threads/All tabs are back over the list');
    return {problems, area: measured, bare, picker: shown(picker)};
}"""

# `#36 thread-view-phone-overflow`. `problems` is empty when the page is no wider than the viewport
# and no shown element of the main screen reaches past its right edge; `clipped` says the thread
# title is ellipsised, which is what a title longer than the screen should do instead.
WIDTH_JS = """() => {
    const viewport = document.documentElement.clientWidth;
    const problems = [];
    const wide = document.documentElement.scrollWidth;
    if (wide > viewport) problems.push(`the page is ${wide}px wide in a ${viewport}px viewport`);
    for (const element of document.querySelectorAll('#screen-main, #screen-main *')) {
        const box = element.getBoundingClientRect();
        if (box.width === 0 || element.closest('[hidden]')) continue;
        if (box.right > viewport + 1) {
            const name = element.id ? `#${element.id}` : `${element.tagName.toLowerCase()}.${element.className}`;
            problems.push(`${name} reaches ${Math.round(box.right)}px`);
        }
    }
    const title = document.getElementById('thread-title');
    return {problems: problems.slice(0, 8), clipped: title.scrollWidth > title.clientWidth};
}"""

# `#194 thread-picker-polish`. The owner photographed Summaries, Undo, Expand all and Collapse all
# sitting on a thread's "Reply in this thread" box. Run with every chip forced up — the worst case —
# this scrolls the thread to its end and returns what covers the composer: empty when no chip meets
# the text box or Send, and both sit inside the list rather than below its foot.
COMPOSER_JS = """async () => {
    const area = document.getElementById('scroll-area');
    const frames = () => new Promise((settled) => requestAnimationFrame(() => requestAnimationFrame(settled)));
    await frames();
    area.scrollTop = area.scrollHeight;
    await frames();
    const box = (e) => e.getBoundingClientRect();
    const meets = (a, b) => a.top < b.bottom && b.top < a.bottom && a.left < b.right && b.left < a.right;
    const chips = [...document.querySelectorAll('#scroll-tools .chip')].filter((c) => c.getClientRects().length > 0);
    const problems = [];
    if (chips.length < 4) problems.push(`only ${chips.length} chips are showing`);
    if (area.scrollHeight - area.clientHeight - area.scrollTop > 2) problems.push('the thread is not at its end');
    for (const id of ['channel-compose-text', 'channel-send']) {
        const target = box(document.getElementById(id));
        chips.filter((chip) => meets(box(chip), target))
            .forEach((chip) => problems.push(`"${chip.textContent.trim()}" covers #${id}`));
        if (target.bottom > box(area).bottom + 1) problems.push(`#${id} ends below the list`);
    }
    return problems;
}"""

# `#197 floating-search`. Shared by the checks below: is an element really laid out, its box, and
# whether two boxes meet.
FLOAT_HELPERS_JS = f"""
    const GLASS_MIN_PX = {GLASS_MIN_PX};""" + """
    const shown = (e) => Boolean(e) && !e.hidden && e.getClientRects().length > 0;
    const box = (e) => e.getBoundingClientRect();
    const meets = (a, b) => a.top < b.bottom && b.top < a.bottom && a.left < b.right && b.left < a.right;
    const name = (e) => e ? (e.id ? `#${e.id}` : e.tagName.toLowerCase()) : 'nothing';
"""

# The call view has no pill, so the glass is measured against the line it shares with one: centred
# on the zero-height anchor (whose top IS the line), over the list's top-right corner, and no header
# strip standing above it. `problems` is empty when all of that holds.
GLASS_ALONE_JS = """() => {""" + FLOAT_HELPERS_JS + """
    const problems = [];
    const header = document.getElementById('topbar');
    if (shown(header)) problems.push(`a ${Math.round(box(header).height)}px header strip stands over the call view`);
    const glass = document.getElementById('search-toggle');
    if (!shown(glass)) return {problems: [...problems, 'the search glass is not on the call view']};
    const g = box(glass), list = box(document.getElementById('scroll-area'));
    const line = box(document.getElementById('channel-freshness-anchor'));
    const off = (g.top + g.bottom) / 2 - line.top;
    if (Math.abs(off) > 1) problems.push(`the glass is centred ${off.toFixed(1)}px off the pill's line`);
    if (g.height < GLASS_MIN_PX) problems.push(`the glass is ${g.height}px across, the size the owner called very small`);
    if (g.top < list.top || g.right > list.right) problems.push('the glass is not over the list');
    // The line is the list's width on a phone and the reading column's on a desk.
    if (line.right - g.right > 24) problems.push(`the glass is ${Math.round(line.right - g.right)}px from the corner`);
    return {problems};
}"""

# The search open over the channel: a bar across the top of the list, no wider than it and not
# hanging over whatever is above it, with the field focused and 140px wide or more, the pill given
# way, and the first thing the list shows clear of the bar. `query`, when given, has just been typed;
# `rows` is what is left on screen and `count` what the bar says about it.
SEARCH_OPEN_JS = """(query) => {""" + FLOAT_HELPERS_JS + """
    const area = document.getElementById('scroll-area');
    area.scrollTop = 0;
    const problems = [];
    const bar = document.getElementById('search-float'), field = document.getElementById('search-field');
    const count = document.getElementById('search-count'), glass = document.getElementById('search-toggle');
    if (glass.getAttribute('aria-pressed') !== 'true') problems.push('the glass does not read as open');
    if (!shown(field)) return {problems: [...problems, 'the field is not open'], rows: [], count: ''};
    const list = box(area), b = box(bar), f = box(field), g = box(glass);
    if (b.top < list.top - 0.5) problems.push(`the bar hangs ${(list.top - b.top).toFixed(1)}px over what is above the list`);
    if (b.left < list.left - 0.5 || b.right > list.right + 0.5) problems.push('the bar is wider than the list');
    // 140 since `#211 link-filter` put Links beside Pinned on the same line.
    if (f.width < 140) problems.push(`the field is ${Math.round(f.width)}px wide`);
    if (f.left < b.left || g.right > b.right + 0.5 || g.top < b.top - 0.5 || g.bottom > b.bottom + 0.5) {
        problems.push('the glass or the field is outside the bar');
    }
    if (query && (!shown(count) || meets(box(count), f) || meets(box(count), g))) problems.push('the count is not on the bar beside the field');
    if (shown(document.getElementById('channel-freshness'))) problems.push('the pill is drawn under the open bar');
    if (document.activeElement !== field) problems.push(`the field did not take focus: ${name(document.activeElement)}`);
    const first = [...document.querySelectorAll('#pane-discord .seam, #discord-log > li')].find(shown);
    if (!first) problems.push('there is nothing at the head of the list to measure against');
    else if (box(first).top < b.bottom) problems.push(`the open bar covers ${(b.bottom - box(first).top).toFixed(1)}px of the head of the list`);
    const rows = [...document.querySelectorAll('#discord-log > li[data-id]')].filter(shown).map((li) => li.getAttribute('data-id'));
    return {problems, rows, count: count.textContent};
}"""

# In a thread the heading row is above the list; neither floating thing may cover its Back button
# or title, nor take the Back button's tap.
HEADING_JS = """() => {""" + FLOAT_HELPERS_JS + """
    const problems = [];
    const glass = document.getElementById('search-toggle'), pill = document.getElementById('channel-freshness');
    const back = document.getElementById('thread-back');
    for (const id of ['thread-back', 'thread-title']) {
        const target = box(document.getElementById(id));
        if (meets(box(glass), target)) problems.push(`the glass covers #${id}`);
        if (shown(pill) && meets(box(pill), target)) problems.push(`the pill covers #${id}`);
    }
    if (box(glass).top < box(document.getElementById('channel-navigation')).bottom) {
        problems.push('the glass is drawn over the thread heading row');
    }
    const b = box(back), hit = document.elementFromPoint((b.left + b.right) / 2, (b.top + b.bottom) / 2);
    if (!back.contains(hit)) problems.push(`a tap on Back lands on ${name(hit)}`);
    return problems;
}"""

# Settings is a destination: the header comes back as a title bar with a way back.
TITLE_BAR_JS = """() => {""" + FLOAT_HELPERS_JS + """
    const header = document.getElementById('topbar');
    return {shown: shown(header), height: box(header).height,
            title: shown(document.getElementById('topbar-title')) ? document.getElementById('topbar-title').textContent : '',
            back: shown(document.getElementById('close-settings')),
            glass: shown(document.getElementById('search-toggle'))};
}"""

# `#204 reply-arrow`. The replies in the list, and what holds across the whole of it: no arrow on a row
# that answers nothing, and the list and the page no wider than themselves.
REPLY_LIST_JS = """() => {
    const area = document.getElementById('scroll-area');
    const problems = [];
    const replies = [...document.querySelectorAll('#discord-log > li[data-is-reply="true"]')]
        .map((row) => row.getAttribute('data-id'));
    if (document.querySelector('#discord-log > li:not([data-is-reply="true"]) > .reply-jump')) {
        problems.push('a row that answers nothing has an arrow');
    }
    if (area.scrollWidth > area.clientWidth + 1) problems.push(`the list is ${area.scrollWidth}px wide in ${area.clientWidth}px`);
    const viewport = document.documentElement.clientWidth;
    if (document.documentElement.scrollWidth > viewport) problems.push(`the page is wider than its ${viewport}px viewport`);
    return {problems, replies};
}"""

# One reply's arrow, measured where it is drawn, its row brought to the middle of the list first.
# `problems` is empty when the arrow is a square at least 44px across in the gutter left of its own
# row's box — touching that box, inside the list and the viewport, within the row's own height so it
# cannot reach a neighbouring row — meets none of the row's text or controls, is what a tap at its
# centre and near its corners lands on, and draws a mark at least 20px each way. `square` and `mark`
# say where to look to see what it looks like (`ARROW_INK_JS`), `accent` is the colour it is drawn
# in, and `state` is how far its row recedes.
REPLY_ARROW_JS = """async (id) => {""" + FLOAT_HELPERS_JS + """
    const area = document.getElementById('scroll-area');
    const frames = () => new Promise((settled) => requestAnimationFrame(() => requestAnimationFrame(settled)));
    const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
    if (!row) return {problems: [`no row ${id}`]};
    const label = `${id} (${row.getAttribute('data-who')})`;
    const problems = [];
    row.scrollIntoView({block: 'center'});
    await frames();
    const arrow = row.querySelector(':scope > .reply-jump');
    if (!shown(arrow)) return {problems: [`${label}: no arrow`]};
    const a = box(arrow), r = box(row), list = box(area);
    const mark = box(arrow.querySelector('.reply-jump-mark'));
    if (a.width < 43.5 || a.height < 43.5) problems.push(`${label}: the target is ${a.width}x${a.height}px`);
    if (mark.width < 20 || mark.height < 20) problems.push(`${label}: the mark is ${mark.width}x${mark.height}px`);
    if (Math.abs(a.right - r.left) > 1.5) problems.push(`${label}: the arrow ends at ${a.right}px and its box starts at ${r.left}px`);
    if (a.left < list.left - 0.5 || a.left < -0.5) problems.push(`${label}: the arrow starts at ${a.left}px, off the list`);
    if (a.top < r.top - 0.5 || a.bottom > r.bottom + 0.5) {
        problems.push(`${label}: the arrow spans ${a.top}..${a.bottom}px of a row at ${r.top}..${r.bottom}px`);
    }
    for (const part of row.querySelectorAll('*')) {
        if (arrow.contains(part) || !shown(part)) continue;
        const p = box(part);
        const across = Math.min(a.right, p.right) - Math.max(a.left, p.left);
        const down = Math.min(a.bottom, p.bottom) - Math.max(a.top, p.top);
        if (across > 0.5 && down > 0.5) problems.push(`${label}: the arrow covers ${name(part)}.${part.className}`);
    }
    for (const [x, y] of [[0.5, 0.5], [0.1, 0.1], [0.9, 0.1], [0.1, 0.9], [0.9, 0.9]]) {
        const hit = document.elementFromPoint(a.left + a.width * x, a.top + a.height * y);
        if (!hit || !arrow.contains(hit)) problems.push(`${label}: a tap at (${x}, ${y}) of the arrow lands on ${name(hit)}`);
    }
    const flag = (attribute) => row.getAttribute(attribute) === 'true';
    const state = flag('data-noise') ? 'noise' : flag('data-archived') ? 'archived'
        : flag('data-own-read') ? 'own-read' : flag('data-replied') ? 'replied' : 'plain';
    return {problems, label, who: row.getAttribute('data-who'), state, accent: getComputedStyle(arrow).color,
            square: {x: a.left, y: a.top, width: a.width, height: a.height},
            mark: {x: mark.left - a.left, y: mark.top - a.top, width: mark.width, height: mark.height}};
}"""

# What an arrow looks like on the screen, from a screenshot of its square (`png`, base64): the page
# behind it, from the square's empty lower-left corner, and the mark, as the pixel inside the mark's
# box furthest in contrast from that, with the contrast between the two. From PIXELS, because a
# computed colour says what colour the arrow is and nothing about the opacity and filter of the row
# it is inside: that is how a mark drawn at 2:1, grey, on every row that had faded passed as the
# accent at 3:1.
ARROW_INK_JS = """async ({png, square, mark}) => {
    const image = new Image();
    image.src = `data:image/png;base64,${png}`;
    await image.decode();
    const canvas = document.createElement('canvas');
    canvas.width = image.naturalWidth;
    canvas.height = image.naturalHeight;
    const context = canvas.getContext('2d');
    context.drawImage(image, 0, 0);
    const {data, width, height} = context.getImageData(0, 0, canvas.width, canvas.height);
    const pixel = (x, y) => [0, 1, 2].map((channel) => data[(y * width + x) * 4 + channel]);
    const luminance = (rgb) => {
        const [r, g, b] = rgb.map((v) => {
            const c = v / 255;
            return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
        });
        return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };
    const contrast = (one, other) => {
        const [x, y] = [luminance(one), luminance(other)];
        return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
    };
    const scale = width / square.width;
    const inset = Math.round(2 * scale);
    const ground = pixel(inset, height - 1 - inset);
    const [left, right] = [Math.floor(mark.x * scale), Math.ceil((mark.x + mark.width) * scale)];
    const [top, bottom] = [Math.floor(mark.y * scale), Math.ceil((mark.y + mark.height) * scale)];
    let ink = ground;
    let best = 1;
    for (let y = Math.max(0, top); y < Math.min(height, bottom); y += 1) {
        for (let x = Math.max(0, left); x < Math.min(width, right); x += 1) {
            const seen = contrast(pixel(x, y), ground);
            if (seen > best) [best, ink] = [seen, pixel(x, y)];
        }
    }
    return {ground, ink, contrast: best};
}"""

# Every state a row recedes in, and none: a ReplyApi channel has a reply in each.
REPLY_STATES = {"own-read", "replied", "archived", "noise", "plain"}

# Where a row is on the screen relative to the floating line over the head of the list and the
# list's foot, and its state: is it lit, folded, being read or waiting to be. The floating line is
# the lower of the freshness pill and the search glass, as the page measures it.
REPLY_ROW_JS = """(id) => {""" + FLOAT_HELPERS_JS + """
    const area = document.getElementById('scroll-area');
    const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
    let line = box(area).top;
    for (const floating of ['channel-freshness', 'search-float']) {
        const element = document.getElementById(floating);
        if (shown(element)) line = Math.max(line, box(element).bottom);
    }
    const r = box(row), arrow = row.querySelector(':scope > .reply-jump');
    const a = arrow ? box(arrow) : null;
    const chip = document.getElementById('back-to-reply');
    return {top: r.top, bottom: r.bottom, line, foot: box(area).bottom,
            landed: row.getAttribute('data-landed'), collapsed: row.getAttribute('data-collapsed'),
            reading: row.getAttribute('data-reading'), pending: row.getAttribute('data-pending'),
            arrow: a ? [a.left + a.width / 2, a.top + a.height / 2] : null,
            chip: shown(chip) ? [box(chip).left + box(chip).width / 2, box(chip).top + box(chip).height / 2] : null};
}"""

# `#213 row-side-borders`. Every channel row on the screen, as drawn. `problems` is empty when every row
# has four sides, each of some width, style and colour; the coding agent's tiles have square corners,
# and their left and right sides the width, style and colour of their top — but where a summary's bar
# is the left side, which is wider than the right; on a phone (`edge`) a tile reaches both edges of the
# screen, or only the right one where it is a reply, and on a desk it is inside the reading column and
# set in from the list's right; the row being read has its bar; and nothing on the page has a
# horizontal scrollbar: no element that draws one has content wider than itself, and the page is no
# wider than the viewport. `count` is how many rows were measured.
TILE_JS = """({edge}) => {""" + FLOAT_HELPERS_JS + """
    const problems = [];
    const area = document.getElementById('scroll-area'), column = box(document.getElementById('pane-discord'));
    const viewport = document.documentElement.clientWidth;
    const rows = [...document.querySelectorAll('#discord-log > li[data-id]')].filter(shown);
    for (const row of rows) {
        const who = row.getAttribute('data-who'), s = getComputedStyle(row);
        const label = `${row.getAttribute('data-id')} (${who})`, flag = (attribute) => row.getAttribute(attribute) === 'true';
        const side = (name) => ({width: parseFloat(s[`border${name}Width`]), style: s[`border${name}Style`],
                                 color: s[`border${name}Color`]});
        const [top, right, bottom, left] = ['Top', 'Right', 'Bottom', 'Left'].map(side);
        for (const [name, one] of [['top', top], ['right', right], ['bottom', bottom], ['left', left]]) {
            if (!(one.width > 0) || one.style === 'none' || one.style === 'hidden' || /^rgba\\([^)]*,\\s*0\\)$/.test(one.color)) {
                problems.push(`${label}: no ${name} side, ${one.width}px ${one.style} ${one.color}`);
            }
        }
        if (who !== 'coder') continue;
        const radii = ['TopLeft', 'TopRight', 'BottomRight', 'BottomLeft'].map((corner) => parseFloat(s[`border${corner}Radius`]));
        if (radii.some((radius) => radius !== 0)) problems.push(`${label}: its corners are rounded, ${radii}`);
        const like = (one) => one.width === top.width && one.style === top.style && one.color === top.color;
        const said = (one) => `${one.width}px ${one.style} ${one.color}`;
        if (!like(right)) problems.push(`${label}: its right side is ${said(right)} and its top ${said(top)}`);
        if (flag('data-summarised') || flag('data-summary-failed') ? !(left.width > right.width + 1) : !like(left)) {
            problems.push(`${label}: its left side is ${said(left)} and its top ${said(top)}`);
        }
        const r = box(row), list = box(area);
        if (edge && (r.right < viewport - 0.5 || (!flag('data-is-reply') && r.left > 0.5))) {
            problems.push(`${label}: on a phone the tile spans ${r.left}..${r.right}px of a ${viewport}px screen`);
        }
        if (!edge && (r.left < column.left - 0.5 || r.right > column.right + 0.5 || r.right > list.right - 8)) {
            problems.push(`${label}: on a desk the tile spans ${r.left}..${r.right}px, in a column at ${column.left}..${column.right}px`);
        }
        if (flag('data-reading') && !/inset/.test(s.boxShadow)) problems.push(`${label}: the row being read has no bar`);
    }
    if (document.documentElement.scrollWidth > viewport) {
        problems.push(`the page is ${document.documentElement.scrollWidth}px wide in a ${viewport}px viewport`);
    }
    for (const element of document.querySelectorAll('body, body *')) {
        const style = getComputedStyle(element);
        // A strip built to scroll sideways with no bar, as the control bar's pack is, draws none.
        if (style.scrollbarWidth === 'none') continue;
        if (['auto', 'scroll'].includes(style.overflowX) && element.scrollWidth > element.clientWidth + 1) {
            problems.push(`${name(element)} has a horizontal scrollbar: ${element.scrollWidth}px in ${element.clientWidth}px`);
        }
    }
    return {problems, count: rows.length};
}"""

# `#206 pin-message` and `#211 link-filter`. The open search bar with its filters in it: Links, then
# Pinned where the view has pins, then the glass, each immediately beside the next and on the glass's
# line, each a whole 44px box every point of which presses it, meeting neither its neighbours, the
# glass nor the field. The glass's own centre is still the glass, and so is the strip of its 48px
# target just left of its disc, which no filter's box may cover. The field is still `fieldMin` px wide
# with both filters and the count beside it — 140 at the ordinary type size — and whatever the size,
# leaves FIELD_TEXT_MIN_PX inside its padding for what is typed. `pinned` says whether Pinned should be
# on the bar.
FILTER_BAR_JS = """({pinned, fieldMin}) => {""" + FLOAT_HELPERS_JS + f"""
    const FIELD_TEXT_MIN_PX = {FIELD_TEXT_MIN_PX};""" + """
    const problems = [];
    const mid = (r) => (r.top + r.bottom) / 2;
    const bar = document.getElementById('search-float'), field = document.getElementById('search-field');
    const glass = document.getElementById('search-toggle');
    const filters = ['links-filter', 'pinned-filter'].map((id) => document.getElementById(id)).filter(shown);
    const wanted = pinned ? 'links-filter pinned-filter' : 'links-filter';
    if (filters.map((f) => f.id).join(' ') !== wanted) {
        return {problems: [`the open bar holds ${filters.map(name).join(', ') || 'no filter'}, not ${wanted}`], field: 0, text: 0};
    }
    const g = box(glass), b = box(bar), fl = box(field);
    filters.forEach((filter, i) => {
        const f = box(filter), label = name(filter);
        if (f.width < 43.5 || f.height < 43.5) problems.push(`${label}'s target is ${f.width.toFixed(1)}x${f.height.toFixed(1)}px`);
        if (meets(f, g)) problems.push(`${label} overlaps the glass`);
        if (meets(f, fl)) problems.push(`${label} overlaps the field`);
        const next = i + 1 < filters.length ? box(filters[i + 1]) : g;
        const gap = next.left - f.right;
        if (gap < -0.5 || gap > 20) problems.push(`${label} is ${gap.toFixed(1)}px from what follows it, not beside it`);
        if (Math.abs(mid(f) - mid(g)) > 1.5) problems.push(`${label} is off the glass's line`);
        if (f.left < b.left - 0.5 || f.right > b.right + 0.5) problems.push(`${label} is outside the bar`);
        // Every point of the square that is ON the screen.
        for (const fx of [0.04, 0.5, 0.96]) {
            for (const fy of [0.04, 0.5, 0.96]) {
                const y = Math.max(f.top + f.height * fy, 1);
                const hit = document.elementFromPoint(f.left + f.width * fx, y);
                if (!hit || !filter.contains(hit)) problems.push(`a tap at (${fx}, ${fy}) of ${label}'s box lands on ${name(hit)}`);
            }
        }
    });
    if (fl.width < fieldMin) problems.push(`the field is ${Math.round(fl.width)}px wide beside the filters`);
    const inside = getComputedStyle(field);
    const text = field.clientWidth - parseFloat(inside.paddingLeft) - parseFloat(inside.paddingRight);
    if (text < FIELD_TEXT_MIN_PX) problems.push(`the field leaves ${text.toFixed(1)}px for its text beside the filters`);
    for (const [x, why] of [[(g.left + g.right) / 2, 'centre'], [g.left - 2, 'target just left of its disc']]) {
        const hit = document.elementFromPoint(x, mid(g));
        if (!hit || !glass.contains(hit)) problems.push(`the glass's ${why} lands on ${name(hit)}`);
    }
    return {problems, field: fl.width, text};
}"""

# `#211 link-filter`. The Links view as it is drawn, the list scrolled to its top: which rows are on
# screen, and for each the links it shows as [text, href, target, rel]. `problems` is empty when every
# shown row has a list of links standing where its text was, the text itself not drawn, each link
# on a line of its own inside the row and at least 44px tall, a link too long for the row cut short
# with an ellipsis rather than wrapped or run off it, and the open bar covering none of the first row.
LINK_VIEW_JS = """() => {""" + FLOAT_HELPERS_JS + """
    const area = document.getElementById('scroll-area');
    area.scrollTop = 0;
    const problems = [];
    const rows = [...document.querySelectorAll('#discord-log > li[data-id]')].filter(shown);
    const bar = box(document.getElementById('search-float'));
    if (rows.length && box(rows[0]).top < bar.bottom - 0.5) problems.push(`the open bar covers ${(bar.bottom - box(rows[0]).top).toFixed(1)}px of the first row`);
    const shownRows = rows.map((row) => {
        const id = row.getAttribute('data-id'), r = box(row);
        const list = row.querySelector(':scope > .row-links');
        if (!shown(list)) problems.push(`row ${id} shows no links`);
        const text = row.querySelector(':scope > .body');
        if (shown(text)) problems.push(`row ${id} still draws its text beside its links`);
        if (!shown(row.querySelector(':scope > .meta'))) problems.push(`row ${id} lost its author and time`);
        let above = list ? box(list).top - 0.5 : 0;
        const links = [...row.querySelectorAll('.row-links > a')].map((a) => {
            const l = box(a);
            if (l.top < above) problems.push(`row ${id}: "${a.textContent}" is not on a line of its own`);
            above = l.bottom - 0.5;
            if (l.height < 43.5) problems.push(`row ${id}: "${a.textContent}" is ${l.height.toFixed(1)}px tall`);
            if (l.left < r.left || l.right > r.right + 0.5) problems.push(`row ${id}: "${a.textContent}" runs out of its row`);
            const style = getComputedStyle(a);
            if (a.scrollWidth > a.clientWidth + 1 && style.textOverflow !== 'ellipsis') problems.push(`row ${id}: a long link is not ellipsised`);
            return [a.textContent, a.getAttribute('href'), a.getAttribute('target'), a.getAttribute('rel'),
                    a.scrollWidth > a.clientWidth + 1];
        });
        return [id, links];
    });
    return {problems, rows: shownRows};
}"""

# `#212 link-filter`. The kinds of link under Links, as drawn: shown, PRs then Commits then Actions, in a
# row below the bar and under the Links icon — centred on it, or right-aligned under the bar's end where
# centred would run past that end, and only then — wholly inside the viewport; each a target at least 44px
# each way every point of which presses it, meeting neither the field, the glass, the filters nor the bar;
# the pill not drawn, and the first row of the list, scrolled to its top, clear below them. `pinned`
# says whether Pinned is on the bar. Answers how the row was placed and, for each kind, [its name, its
# aria-pressed, the number on it].
KINDS_JS = """({pinned}) => {""" + FLOAT_HELPERS_JS + """
    const problems = [];
    const row = document.getElementById('link-kinds');
    if (!shown(row)) return {problems: ['no kinds of link are shown under Links'], placement: '', chips: []};
    if (shown(document.getElementById('pinned-filter')) !== pinned) problems.push(`Pinned is ${pinned ? 'not ' : ''}on the bar`);
    const b = box(document.getElementById('search-float')), l = box(document.getElementById('links-filter'));
    const r = box(row), width = document.documentElement.clientWidth;
    const mid = (l.left + l.right) / 2, rowMid = (r.left + r.right) / 2;
    if (r.top < b.bottom - 0.5) problems.push(`the kinds reach ${(b.bottom - r.top).toFixed(1)}px up into the bar`);
    if (r.top - b.bottom > 8) problems.push(`the kinds float ${(r.top - b.bottom).toFixed(1)}px below the bar, not under it`);
    if (r.left > mid || r.right < mid) problems.push('the kinds are not under the Links icon');
    const centred = Math.abs(rowMid - mid) <= 1;
    if (!centred && Math.abs(r.right - b.right) > 2) {
        problems.push(`the kinds are ${(rowMid - mid).toFixed(1)}px off Links' middle and not right-aligned under the bar's end`);
    }
    if (!centred && mid + r.width / 2 < b.right - 2) problems.push('the kinds are right-aligned where centred under Links would fit');
    if (r.left < -0.5 || r.right > width + 0.5) problems.push(`the kinds run off the screen: ${r.left.toFixed(1)}..${r.right.toFixed(1)} of ${width}`);
    const others = ['search-field', 'search-toggle', 'links-filter', 'pinned-filter'].map((id) => document.getElementById(id)).filter(shown);
    const chips = [...row.querySelectorAll('button')];
    const said = chips.map((chip) => (chip.firstChild ? chip.firstChild.textContent.trim() : ''));
    if (said.join(' ') !== 'PRs Commits Actions') problems.push(`the kinds read ${said.join(', ')}`);
    chips.forEach((chip, i) => {
        const c = box(chip), label = said[i];
        if (c.width < 43.5 || c.height < 43.5) problems.push(`${label}'s target is ${c.width.toFixed(1)}x${c.height.toFixed(1)}px`);
        if (meets(c, b)) problems.push(`${label} reaches into the bar`);
        for (const other of others) if (meets(c, box(other))) problems.push(`${label} covers ${name(other)}`);
        for (const fx of [0.04, 0.5, 0.96]) {
            for (const fy of [0.04, 0.5, 0.96]) {
                const hit = document.elementFromPoint(c.left + c.width * fx, c.top + c.height * fy);
                if (!hit || !chip.contains(hit)) problems.push(`a tap at (${fx}, ${fy}) of ${label} lands on ${name(hit)}`);
            }
        }
    });
    if (shown(document.getElementById('channel-freshness'))) problems.push('the pill is drawn beside the kinds');
    document.getElementById('scroll-area').scrollTop = 0;
    const first = [...document.querySelectorAll('#discord-log > li[data-id], #transcript > li')].find(shown);
    if (first && box(first).top < r.bottom - 0.5) problems.push(`the kinds cover ${(r.bottom - box(first).top).toFixed(1)}px of the first row`);
    return {problems, placement: centred ? 'centred' : 'right-aligned',
            chips: chips.map((chip, i) => [said[i], chip.getAttribute('aria-pressed'), chip.querySelector('.link-kind-count').textContent])};
}"""

# `#212 link-filter`. How one kind's button is drawn: [its words' colour, their decoration, its pill's
# fill, its pill's edge], and the contrast of the words against the pill — composited over the bar's
# panel, since the row floats over the list. Colours come from the computed style, which writes
# `color-mix` out as `color(srgb …)`.
KIND_LOOK_JS = """(id) => {
    const chip = document.getElementById(id), own = getComputedStyle(chip), pill = getComputedStyle(chip, '::before');
    const parse = (value) => {
        const srgb = /color\\(srgb ([\\d.]+) ([\\d.]+) ([\\d.]+)/.exec(value);
        if (srgb) return [1, 2, 3].map((i) => Number(srgb[i]) * 255);
        return (value.match(/[\\d.]+/g) || []).slice(0, 3).map(Number);
    };
    const luminance = (rgb) => {
        const [r, g, b] = rgb.map((v) => { const c = v / 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; });
        return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };
    const ink = luminance(parse(own.color)), ground = luminance(parse(pill.backgroundColor));
    const contrast = (Math.max(ink, ground) + 0.05) / (Math.min(ink, ground) + 0.05);
    return {look: [own.color, own.textDecorationLine, pill.backgroundColor, pill.borderTopColor], contrast};
}"""

# `#212 link-filter`, from review. The sentence an emptied list says instead of its rows, where the reader
# was left — the list NOT scrolled first: shown, wholly below the bar and the kinds of link under it,
# above the foot of the list and the screen, and every point of it the sentence's own, so nothing
# floating covers a word. The kinds' room at the head of the list holds only at its top, and a reader
# put at the newest line of a list whose composer still overflows it had the sentence under the three
# buttons it points to, or above the screen altogether. Answers the problems, the words, and how far
# the list is scrolled.
EMPTY_SENTENCE_JS = """() => {""" + FLOAT_HELPERS_JS + """
    const problems = [];
    const sentence = document.getElementById('search-empty');
    const area = document.getElementById('scroll-area');
    if (!shown(sentence)) return {problems: ['no sentence says why the list is empty'], text: '', top: area.scrollTop};
    const s = box(sentence), list = box(area), kinds = document.getElementById('link-kinds');
    const over = shown(kinds) ? ['the kinds of link', box(kinds)] : ['the bar', box(document.getElementById('search-float'))];
    if (s.top < over[1].bottom - 0.5) problems.push(`the sentence starts ${(over[1].bottom - s.top).toFixed(1)}px up under ${over[0]}`);
    const foot = Math.min(list.bottom, window.innerHeight);
    if (s.bottom > foot + 0.5) problems.push(`the sentence runs ${(s.bottom - foot).toFixed(1)}px past the foot of the list`);
    for (const fx of [0.1, 0.5, 0.9]) {
        for (const fy of [0.1, 0.5, 0.9]) {
            const x = s.left + s.width * fx, y = s.top + s.height * fy;
            if (y < 0 || y > window.innerHeight) continue;
            const hit = document.elementFromPoint(x, y);
            if (!hit || !sentence.contains(hit)) problems.push(`(${fx}, ${fy}) of the sentence is under ${name(hit)}`);
        }
    }
    return {problems, text: sentence.textContent.trim(), top: area.scrollTop};
}"""

# A row's open ⋯ menu: Copy text, Pin, then the read group under a hairline and a caption — two
# items in it where the provider can move its own read marker, one where it cannot — every item at
# least 44px tall and as wide as the others, and the whole menu ACROSS the screen wherever the "⋯"
# that opened it sits: at a wrapping width the "⋯" lands at the start of a line, and a menu hung
# from its right edge opened off the left of the screen. Every item then answers a real point at its
# centre once the list is scrolled to it, which is what a thumb needs of it; the list is scrolled
# vertically only, after the menu's own box is measured, so nothing slides it back on screen first.
MENU_JS = """({list, id, upstream}) => {""" + FLOAT_HELPERS_JS + """
    const problems = [];
    const row = document.querySelector(`#${list} > li[data-id="${id}"]`);
    const menu = row && row.querySelector('.row-more-menu');
    if (!menu || !shown(menu)) return {problems: [`row ${id}'s ⋯ menu is not open`], items: []};
    const names = [...menu.children].filter(shown).map((e) => e.className);
    if (names.join(' ') !== 'row-copy-button row-pin-button row-more-group') problems.push(`the menu holds ${names.join(', ')}`);
    const group = menu.querySelector('.row-more-group');
    const caption = group && group.querySelector('.row-more-caption');
    if (!caption || !shown(caption) || caption.textContent !== 'Mark read') problems.push('the read group has no caption');
    if (group && parseFloat(getComputedStyle(group).borderTopWidth) < 1) problems.push('the read group has no rule above it');
    const grouped = group ? [...group.querySelectorAll('button')].filter(shown).length : 0;
    if (grouped !== (upstream ? 2 : 1)) problems.push(`the read group offers ${grouped} items`);
    const buttons = [...menu.querySelectorAll('button')].filter(shown);
    for (const button of buttons) {
        const r = box(button);
        if (r.height < 43.5) problems.push(`"${button.textContent}" is ${r.height.toFixed(1)}px tall`);
    }
    if (new Set(buttons.map((button) => Math.round(box(button).width))).size > 1) problems.push('the items are not one width');
    const pin = menu.querySelector('.row-pin-button');
    if (group && pin && box(group).top < box(pin).bottom) problems.push('the read group is not below Pin');
    const m = box(menu), width = document.documentElement.clientWidth;
    if (m.left < -0.5 || m.right > width + 0.5) {
        problems.push(`the menu runs off the screen: ${Math.round(m.left)}..${Math.round(m.right)} of ${width}px`);
    }
    for (const button of buttons) {
        const r = box(button);
        if (r.left < -0.5 || r.right > width + 0.5) problems.push(`"${button.textContent}" runs off the screen`);
    }
    const area = document.getElementById('scroll-area'), parked = area.scrollTop;
    for (const button of buttons) {
        const a = box(area);
        area.scrollTop += (box(button).top + box(button).bottom) / 2 - (a.top + a.bottom) / 2;
        const r = box(button);
        const hit = document.elementFromPoint((r.left + r.right) / 2, (r.top + r.bottom) / 2);
        if (!hit || !button.contains(hit)) problems.push(`a tap on "${button.textContent}" lands on ${name(hit)}`);
    }
    area.scrollTop = parked;
    return {problems, items: buttons.map((button) => button.textContent)};
}"""

# The pinned chip on a row: shown, in words, its ink readable against its own surface in the theme in
# force (WCAG's 4.5:1 for small text), and the row's gold edge drawn.
CHIP_JS = """(id) => {""" + FLOAT_HELPERS_JS + """
    const row = document.querySelector(`#discord-log > li[data-id="${id}"]`);
    const chip = row && row.querySelector('.pin-chip');
    if (!chip || !shown(chip)) return {problems: [`row ${id} has no Pinned chip`], contrast: 0};
    const problems = [];
    if (chip.textContent.trim() !== 'Pinned') problems.push(`the chip says ${chip.textContent}`);
    const channels = (text) => (text.match(/[0-9.]+/g) || []).map(Number);
    const luminance = ([r, g, b]) => {
        const linear = (c) => { c /= 255; return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4; };
        return 0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b);
    };
    const style = getComputedStyle(chip);
    const ink = luminance(channels(style.color)), ground = luminance(channels(style.backgroundColor));
    const contrast = (Math.max(ink, ground) + 0.05) / (Math.min(ink, ground) + 0.05);
    if (contrast < 4.5) problems.push(`the chip's words are ${contrast.toFixed(2)}:1 against it`);
    const edge = getComputedStyle(row, '::after');
    if (edge.content === 'none' || parseFloat(edge.width) < 3) problems.push('the pinned row has no edge');
    return {problems, contrast};
}"""

# `#207 scrollback-jump`. Put the reader `gap` px short of the end of the list, or halfway up it for
# "middle". From the first call on, while `__holdScrolls` is set the list's scroll events are kept from
# the page: the listener is the document's, in the capture phase, so it runs before the page's own on
# the list and can stop the event reaching it.
PARK_JS = """(gap) => {
    const area = document.getElementById('scroll-area');
    if (window.__holdScrolls === undefined) {
        window.__holdScrolls = false;
        document.addEventListener('scroll', (event) => {
            if (event.target === area && window.__holdScrolls) event.stopImmediatePropagation();
        }, true);
    }
    const range = area.scrollHeight - area.clientHeight;
    area.scrollTop = range - (gap === 'middle' ? Math.round(range / 2) : gap);
}"""

# Whether the jump to the newest message is up, for waiting on.
JUMP_UP_JS = "() => !document.getElementById('jump-newest').hidden"

# Whether the room the channel composer keeps under itself has caught up with the chip row: the page
# sets it from a ResizeObserver on the row, so it follows the row's height a rendering frame late, and
# the end of the list moves when it does.
ROOM_SETTLED_JS = """() => {
    const row = document.getElementById('scroll-tools').getBoundingClientRect().height;
    const room = document.getElementById('frame-body').style.getPropertyValue('--scroll-tools-clearance');
    const px = /^calc\\(([\\d.]+)px \\+ 0\\.5rem\\)$/.exec(room);
    return row > 0 ? Boolean(px) && Math.abs(Number(px[1]) - row) < 0.5 : room === '0px' || room === '';
}"""

# Measure the jump to the newest message where the reader is. `problems` is empty when it is at the
# lower left of the chip row — the left end of the list's column, on the row's bottom line — inside the
# list and above the dock, clear of every other chip shown and of the floating pills, legible against
# its own chip in the colours in force, and pressed by a tap anywhere in a 44px square around its
# centre. `gap` is how far the list runs below the reader.
JUMP_JS = """() => {""" + FLOAT_HELPERS_JS + """
    const area = document.getElementById('scroll-area');
    const gap = area.scrollHeight - area.clientHeight - area.scrollTop;
    const jump = document.getElementById('jump-newest');
    if (!shown(jump)) return {shown: false, gap, label: '', problems: [], centre: [0, 0], top: 0, lines: 0};
    const problems = [];
    const j = box(jump), list = box(area), column = box(document.getElementById('pane-discord'));
    if (j.left < list.left || j.right > list.right || j.top < list.top || j.bottom > list.bottom) {
        problems.push('it is not inside the list');
    }
    if (j.left - column.left < 0 || j.left - column.left > 24) {
        problems.push(`it is ${(j.left - column.left).toFixed(1)}px from the column's left edge`);
    }
    if (list.bottom - j.bottom > 16) problems.push(`it floats ${(list.bottom - j.bottom).toFixed(1)}px above the list's foot`);
    if (j.bottom > box(document.getElementById('dock')).top + 0.5) problems.push('it is under the dock');
    // Laid out, not merely un-hidden: the checks below force hidden chips up with a style, and those
    // still carry the attribute.
    const chips = [...document.querySelectorAll('#scroll-tools .chip')].filter((c) => c.getClientRects().length > 0);
    const lines = new Set(chips.map((c) => Math.round(box(c).bottom))).size;
    for (const chip of chips.filter((c) => c !== jump)) {
        const c = box(chip), what = `"${chip.textContent.trim()}"`;
        if (meets(j, c)) problems.push(`it overlaps ${what}`);
        if (c.bottom > j.bottom + 0.5) problems.push(`${what} is on a line below it`);
        if (c.left < j.right - 0.5 && c.bottom > j.top + 0.5) problems.push(`${what} is beside it on the left`);
        if (c.right > list.right + 0.5) problems.push(`${what} runs off the list`);
    }
    for (const id of ['channel-freshness', 'status-line', 'pull-refresh']) {
        const pill = document.getElementById(id);
        if (shown(pill) && meets(j, box(pill))) problems.push(`it is under #${id}`);
    }
    // Legible: its words against its own chip, by the WCAG contrast ratio, normal text.
    const rgb = (value) => (value.match(/[\\d.]+/g) || []).slice(0, 3).map(Number);
    const lum = (value) => {
        const [r, g, b] = rgb(value).map((v) => v / 255).map((v) => v <= 0.03928 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
        return 0.2126 * r + 0.7152 * g + 0.0722 * b;
    };
    const contrast = (style) => {
        const [a, b] = [lum(style.color), lum(style.backgroundColor)].sort((x, y) => y - x);
        return (a + 0.05) / (b + 0.05);
    };
    const plain = contrast(getComputedStyle(jump));
    if (plain < 4.5) problems.push(`its words are ${plain.toFixed(2)}:1 against the chip`);
    const flagged = jump.hasAttribute('data-arrivals');
    jump.setAttribute('data-arrivals', '');
    const arrival = contrast(getComputedStyle(jump));
    if (!flagged) jump.removeAttribute('data-arrivals');
    if (arrival < 4.5) problems.push(`its arrival accent is ${arrival.toFixed(2)}:1 against the chip`);
    // The 44px target: 21px above, below and to either side of its centre is still the jump.
    const cx = (j.left + j.right) / 2, cy = (j.top + j.bottom) / 2;
    for (const [dx, dy] of [[0, 0], [0, -21], [0, 21], [-21, 0], [21, 0]]) {
        const hit = document.elementFromPoint(cx + dx, cy + dy);
        if (!hit || !jump.contains(hit)) problems.push(`a tap at (${dx}, ${dy})px from its centre lands on ${name(hit)}`);
    }
    return {shown: true, gap, label: jump.textContent.trim(), problems, centre: [cx, cy], top: j.top, lines};
}"""

# Where the newest channel row sits after a tap on the jump: `visible` when its end is inside the list.
# Its end, not all of it: the rows are 70vh tall here and the composer follows them, so the whole of
# the newest one does not fit above the composer on a phone.
NEWEST_ROW_JS = """() => {
    const rows = [...document.querySelectorAll('#discord-log > li[data-id]')];
    const r = rows[rows.length - 1].getBoundingClientRect(), list = document.getElementById('scroll-area').getBoundingClientRect();
    return {id: rows[rows.length - 1].getAttribute('data-id'), visible: r.bottom > list.top && r.bottom <= list.bottom + 0.5};
}"""

# A phone, the common narrow Android width, and a desk: the desktop regime is `(min-width: 900px) and
# (pointer: fine)`, so the last is a fine pointer without touch, not a wide phone. Below 360 the
# pill's longest line is ellipsised, which the "cut short" check would report.
PROFILES = (("phone", 412, 915, True), ("phone-360", 360, 800, True), ("desktop", 1280, 800, False))

# `#206 pin-message`. What a phone reader's own settings do to a row's meta line: the page zoomed,
# which narrows the CSS viewport (115% on a 360px phone leaves 313px), or a larger system font,
# stood in for by scaling the root font that every rem on the page follows. Either wraps the line
# sooner and puts some row's "⋯" at the START of a line — the pinned row's first, its chip taking
# room — and every menu must still open wholly on the screen. As (label, width, height, root %).
MENU_SCALES = (("zoom-313", 313, 680, 100), ("font-115", 360, 800, 115), ("font-130", 412, 915, 130),
               ("font-150", 360, 800, 150))

# `#211 link-filter`, from review. The same settings on the open search bar, with Links on and so its
# count up: at 360px, where the field is narrowest, at every type size, and at 130% on 412px too. The
# glass and the filters were sized in rem, so the larger font grew them and the field gave up the width
# — none left for its text at 150%. They are px now, and the field keeps the rest.
BAR_SCALES = (("zoom-313", 313, 680, 100), ("font-115", 360, 800, 115), ("font-130", 360, 800, 130),
              ("font-130-412", 412, 915, 130), ("font-150", 360, 800, 150))

# `#212 link-filter`. Where the kinds of link under Links are walked: the two phones, 360px at 150% type
# — where the row, numbers and all, is widest against the narrowest line — and a desk. Label, width,
# height, touch, root font percent.
KINDS_SIZES = (("phone", 412, 915, True, 100), ("phone-360", 360, 800, True, 100),
               ("font-150", 360, 800, True, 150), ("desktop", 1280, 800, False, 100))
# ...and where the row is at its widest, every button reading "99+", against the narrowest lines: the
# page zoomed to 313px, and 360px at 130% and 150% type, where it once ran off the left of the screen.
CROWDED_KINDS_SIZES = (("zoom-313", 313, 680, 100), ("font-130", 360, 800, 130), ("font-150", 360, 800, 150))
# `#212 link-filter`, from review. Where a list the filters emptied is read: 412px at 150% type, 800px
# tall as a phone's browser leaves it, where the composer still overflows the list by 102px with every
# row gone; the two phones with the keyboard up, which leaves a list about 420px tall; and a phone at
# the ordinary size, where it does not overflow at all. Label, width, height, root font percent, and
# how it is emptied: "kinds" turns all three kinds off, "typed" types text matching only the row without
# a link with every kind on, "search" types text matching nothing with Links off, and "linkless" turns
# Links on over a channel with no link in it.
EMPTY_SIZES = (("font-150", 412, 800, 150, "kinds"), ("font-150", 412, 800, 150, "linkless"),
               ("keyboard-360", 360, 420, 100, "typed"), ("keyboard-412", 412, 450, 100, "kinds"),
               ("keyboard-360", 360, 420, 100, "search"), ("phone", 412, 915, 100, "kinds"))
# What each of those says.
EMPTY_SENTENCES = {
    "kinds": "No links left once PRs, Commits and Actions are hidden — turn them back on under the Links button.",
    "typed": "None of the loaded messages with a link match that search.",
    "search": "Nothing in the messages loaded so far matches that search.",
    "linkless": "No links in the messages loaded so far.",
}


def larger_root_font(page: Page, root_percent: int) -> None:
    """Stand in for a phone reader's larger system font: every rem on the page follows the root's."""
    if root_percent != 100:
        page.add_init_script(
            "document.addEventListener('DOMContentLoaded', () => {"
            " const larger = document.createElement('style');"
            f" larger.textContent = 'html {{ font-size: {root_percent}% !important; }}';"
            " document.head.append(larger); });")


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

    with sync_playwright() as playwright:
        for label, width, height, mobile in PROFILES:
            pin_walk(playwright.chromium, args, label, width, height, mobile)
            print(f"{label} at {width}x{height}: a pin made elsewhere shown as a legible Pinned chip and an"
                  " edge, the ⋯ menus of a pinned and an unpinned row as Copy text, Pin and the captioned"
                  " two-item read group at full size and on the screen, Pin shown at once and stored, and"
                  " the Pinned filter beside the glass with a 44px target clear of the glass and the field,"
                  " showing the pins and folded away with the bar")
            links_walk(playwright.chromium, args, label, width, height, mobile)
            print(f"{label} at {width}x{height}: the search glass at least {GLASS_MIN_PX}px across; Links and"
                  " Pinned beside it on its line, each a 44px target clear of the field and the glass, the"
                  " field at least 140px wide beside a count; Links showing only the rows with links, each"
                  " link real, on its own line and a 44px target, a long one ellipsised; a real"
                  f" {'tap' if mobile else 'click'} on a link opening a new tab without folding its row; Links"
                  " folded away with the bar, and alone beside the glass over the call view")
            browser_version = walk(playwright.chromium, args, label, width, height, mobile)
            print(f"{browser_version} {label} at {width}x{height}: All by default, a reload reopened on the"
                  " channel, its view and the reader's message, snapshot drawn before the network,"
                  " one newest-page read of the"
                  " view on screen, local view switches, offline and failed states, the pill"
                  " clear of the tabs and the header with #scroll-area unmoved, a scrolled reader"
                  " held in place as it and the error panel come and go, the jump to the newest message"
                  " at the lower left of the chip row whenever the list is scrolled back and gone at the"
                  " newest line, a real tap on it landing there, the same when the list resizes under a"
                  " still reader, the search glass on the pill's"
                  " line with no header strip and a 44px target, the search bar opened by a real tap and"
                  " folded with #scroll-area unmoved, Settings' title bar,"
                  f"{' a pull up past the newest line refreshes in place,' if mobile else ''} no Cache Storage or"
                  " service worker, a long thread title ellipsised within the viewport, the thread"
                  " composer clear of every floating chip, sign-out clears,"
                  " a read-scope token reads with no refused request or console error")
            print(f"{reply_walk(playwright.chromium, args, label, width, height, mobile)} {label} at"
                  f" {width}x{height}: every reply's arrow a 44px target in the gutter left of its box, clear"
                  " of the row's text and buttons, drawn in the accent at 3:1 or better on every row"
                  " that recedes, at the default type size, at 150% and in the dark scheme; a real"
                  f" {'tap' if mobile else 'click'} on it brought the message it answers under the floating"
                  " line, lit, without folding the reply or, in reading mode, reading it aloud; Back to reply"
                  " brought the reply back")
            rows = tile_walk(playwright.chromium, args, label, width, height, mobile)
            print(f"{label} at {width}x{height}, dark: {rows} channel rows each drawn with four sides; the"
                  " coding agent's tiles square, their left and right sides the width, style and colour of"
                  " their top or a summary's wider bar on the left, "
                  f"{'from edge to edge of the screen' if mobile else 'inside the reading column'}, beside the"
                  " owner's and a third party's rows and in every state that redraws a border; nothing on"
                  " the page scrolling sideways")
        for label, width, height, root_percent in MENU_SCALES:
            menu_walk(playwright.chromium, args, label, width, height, root_percent)
            print(f"{label} at {width}x{height}, root font {root_percent}%: every row's ⋯ menu, the pinned"
                  " row's in the Pinned filter too, on the screen with full-size items a tap reaches, in"
                  " both provider shapes, and Unpin tapped for real")
        for label, width, height, root_percent in BAR_SCALES:
            text = links_bar_walk(playwright.chromium, args, label, width, height, root_percent)
            print(f"{label} at {width}x{height}, root font {root_percent}%: the open bar with Links on and its"
                  f" count up — the glass at least {GLASS_MIN_PX}px, Links and Pinned 44px targets beside it,"
                  f" the field leaving {text:.0f}px for its text (at least {FIELD_TEXT_MIN_PX}) — every"
                  " link on its own 44px line, clear of the bar, and PRs, Commits and Actions 44px targets"
                  " under Links, inside the screen")
        for label, width, height, mobile, root_percent in KINDS_SIZES:
            placed = kinds_walk(playwright.chromium, args, label, width, height, mobile, root_percent)
            print(f"{label} at {width}x{height}, root font {root_percent}%: PRs, Commits and Actions under Links,"
                  f" {placed} under it over the channel and right-aligned under the bar's end over the call,"
                  " inside the screen, each a 44px target clear of the bar, the pill and the first row,"
                  " legible on and off; a real tap on each took its links out of their rows, and a row with"
                  " nothing left went; the choice outlived a reload, and the glass took the row away")
        for label, width, height, root_percent in CROWDED_KINDS_SIZES:
            span = crowded_kinds_walk(playwright.chromium, args, label, width, height, root_percent)
            print(f"{label} at {width}x{height}, root font {root_percent}%: PRs, Commits and Actions each"
                  f" reading 99+, {span:.0f}px wide, under Links and inside the screen, each a 44px target")
        for label, width, height, root_percent, mode in EMPTY_SIZES:
            top = empty_walk(playwright.chromium, args, label, width, height, root_percent, mode)
            print(f"{label} at {width}x{height}, root font {root_percent}%: a list emptied ({mode}) from its"
                  " newest line says why wholly on the screen, below the bar and any kinds of link under it,"
                  f" covered by nothing and with no jump offered past it, the list at {top:.0f}px; undone, the"
                  " rows came back at the newest line")
    return 0


def walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
         mobile: bool) -> str:
    """One reader, start to finish, against a fresh fake API and a fresh browser profile."""
    api = FakeApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/voice"
    platform = "Linux; Android 15; Pixel 7" if mobile else "X11; Linux x86_64"
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-") as profile:
            context = chromium.launch_persistent_context(
                profile,
                headless=True,
                executable_path=args.browser_executable,
                user_agent=(
                    f"Mozilla/5.0 ({platform}) AppleWebKit/537.36 (KHTML, like Gecko)"
                    f" Chrome/151.0.0.0 {'Mobile ' if mobile else ''}Safari/537.36"
                ),
                viewport={"width": width, "height": height},
                device_scale_factor=2.625 if mobile else 1,
                is_mobile=mobile,
                has_touch=mobile,
            )
            page = context.pages[0]
            # Time flows as normal; `#32`'s in-place step jumps it past the channel's poll.
            page.clock.install()
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            # Everything the browser itself calls an error, and every refused answer: a 4xx logs a
            # console error however the page handles it, so the only fix is not to ask.
            console_errors: list[str] = []
            page.on("console", lambda entry: console_errors.append(entry.text) if entry.type == "error" else None)
            refused: list[str] = []
            page.on("response", lambda answer: refused.append(
                f"{answer.status} {answer.request.method} {urlsplit(answer.url).path}")
                if answer.status >= 400 else None)

            def shot(name: str) -> None:
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / f"{label}-{name}.png"))

            def rows() -> list[str]:
                return [str(row) for row in page.eval_on_selector_all(
                    "#discord-log > li[data-id]", "items => items.map(i => i.getAttribute('data-id'))")]

            def wait_rows(expected: list[str], why: str) -> None:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and rows() != expected:
                    page.wait_for_timeout(50)
                check(rows() == expected, f"{why}: showing {rows()}, expected {expected}")

            def pill() -> str:
                return str(page.evaluate(
                    "() => { const p = document.getElementById('channel-freshness');"
                    " return p.hidden ? '' : p.textContent; }"))

            def current(text: str) -> bool:
                # A current list says so with the time of its last read; blank means unknown.
                return re.fullmatch(r"(Live · u|U)pdated \d{2}:\d{2}", text) is not None

            def view(name: str) -> None:
                page.evaluate(f"() => document.getElementById('channel-view-{name}').click()")

            def reopened(why: str) -> None:
                """`#189 restore-ui-state`: a reload of a page left on the channel comes back on it."""
                shown = page.evaluate("() => !document.getElementById('pane-discord').hidden")
                check(bool(shown), f"{label}: {why}: the reload did not reopen on the channel")

            def failing(route: Route) -> None:
                route.fulfill(status=502, content_type="application/json",
                              body=json.dumps({"error": "discord_error", "detail": "the provider is down"}))

            def timeline_read(address: str) -> bool:
                return "/timeline" in address

            def wait_pill(prefix: str, why: str) -> None:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not (pill().startswith(prefix) if prefix else current(pill())):
                    page.wait_for_timeout(50)
                check(pill().startswith(prefix) if prefix else current(pill()), f"{label}, {why}: pill {pill()!r}")

            def poll_channel(fail: bool) -> None:
                """The channel's own re-read, brought forward on the page clock rather than awaited."""
                if fail:
                    page.route(timeline_read, failing)
                page.clock.fast_forward(60_000)
                if fail:
                    wait_pill("Refresh failed", "the in-place refresh did not fail")
                    page.unroute(timeline_read, failing)
                else:
                    wait_pill("", "the in-place refresh did not recover")
                page.wait_for_timeout(100)

            def place(where: str) -> dict[str, float]:
                found = page.evaluate(PLACE_JS, where)
                return {key: float(found[key]) for key in ("row", "gap", "top", "bottom", "banner")}

            def geometry(state: str, name: str) -> str:
                # At the head of the list, where the header is: a pill over row 40 of 90 is the
                # accepted cost of an overlay, a pill over the header is `#32 freshness-pill-overlap`.
                found = page.evaluate(GEOMETRY_JS)
                shot(name)
                problems = [str(problem) for problem in found["problems"]]
                check(not problems, f"{label}, {state}: {problems}")
                check(str(found["area"]) == str(found["bare"]),
                      f"{label}, {state}: #scroll-area is {found['area']}, {found['bare']} without the pill")
                check(bool(found["picker"]), f"{label}, {state}: the bar's thread picker was not on screen")
                return str(found["area"])

            def wait_jump(up: bool) -> None:
                """`#207 scrollback-jump`: until the jump is up (or gone), for ten seconds at most.

                Waited for rather than given two frames. The page decides the jump on the list's scroll
                event, which the browser dispatches in its next RENDERING frame, and a loaded host can
                hold that frame back for half a second; `page.clock` has made requestAnimationFrame a
                16ms timer, so two frames counted in the page pass whether one was drawn or not. A jump
                that is stuck, rather than late, still fails here: ten seconds later.
                """
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and bool(page.evaluate(JUMP_UP_JS)) != up:
                    page.wait_for_timeout(50)

            def wait_room() -> None:
                """Until the room under the composer has caught up with the chip row, ten seconds at most."""
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not page.evaluate(ROOM_SETTLED_JS):
                    page.wait_for_timeout(50)

            def park_still(gap: int) -> None:
                """Park the reader `gap` px short of the end, and again until the end stays where it is.

                The room under the composer follows the chip row a frame late (ROOM_SETTLED_JS), and a jump
                that has just come or gone has just changed the row, so the end of the list can move after
                the reader was put somewhere relative to it. Ten seconds at most.
                """
                deadline = time.monotonic() + 10
                while True:
                    page.evaluate(PARK_JS, gap)
                    wait_room()
                    if abs(float(page.evaluate(JUMP_JS)["gap"]) - gap) <= 1 or time.monotonic() > deadline:
                        return

            # 1. Sign in and open the channel: in All, the default where the provider has threads
            # (`#189 restore-ui-state`); then Threads and Main. The call view comes up first: its
            # glass floats over the transcript with no header strip above it (`#197`).
            page.goto(url, wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            alone = page.evaluate(GLASS_ALONE_JS)
            shot("1-call-view-glass")
            check(not alone["problems"], f"{label}, the call view: {alone['problems']}")
            page.click("#view-switch")
            wait_rows(["200", "201", "202"], "All after sign-in")
            check(page.evaluate("() => document.getElementById('thread-select').value") == "flat",
                  f"{label}: a channel with threads did not open in All")
            view("threads")
            page.wait_for_selector("#thread-list > li", timeout=10_000)
            view("main")
            wait_rows(["200", "201"], "Main after sign-in")
            view("flat")
            wait_rows(["200", "201", "202"], "All again")
            saved = json.loads(page.evaluate(f"() => localStorage.getItem({json.dumps(CACHE_KEY)})") or "{}")
            scope = saved.get("scopes", {}).get(CHANNEL["id"], {})
            check(sorted(scope.get("views", {})) == ["flat", "main", "threads"],
                  f"the snapshot did not cover each view read: {sorted(scope.get('views', {}))}")
            shot("1-signed-in")
            baseline = str(page.evaluate(GEOMETRY_JS)["area"])

            # 2. Reload with the history read held: the page reopens on the channel, in All, and the
            # saved rows draw before any answer. The read goes once /client-config has answered and
            # the live stream has attached, and the pill says it is refreshing from then.
            api.timeline_gate.clear()
            with api.lock:
                api.requests.clear()
                api.messages.append(message(3, "posted while the page was closed"))
            page.reload(wait_until="load")
            reopened("the reload with the read held")
            check(page.evaluate("() => document.getElementById('thread-select').value") == "flat",
                  f"{label}: the reload did not reopen in All")
            wait_rows(["200", "201", "202"], "the snapshot was not drawn before the network answered")
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and "refreshing" not in pill():
                page.wait_for_timeout(50)
            text = pill()
            check(text.startswith("Saved ") and "refreshing" in text, f"pill while refreshing: {text!r}")
            check(geometry("refreshing", "2-saved-before-network") == baseline,
                  f"{label}: #scroll-area is not the size it was before the reload")

            # 3. One bounded newest page for the view on screen — All — merges without duplicating.
            api.timeline_gate.set()
            wait_rows(["200", "201", "202", "203"], "the refresh did not merge")
            reads = api.reads()
            check(len(reads) == 1, f"a cold start made {len(reads)} history reads: {reads}")
            check("view=flat" in reads[0] and "before=" not in reads[0],
                  f"the cold-start read was not the newest page of All: {reads[0]}")
            # After the token was proved and the live stream answered: a read begun before the
            # attach cannot answer for the stream's replay tail (`#43 replay-burst-double-read`).
            with api.lock:
                order = [r for r in api.requests if "/client-config" in r or "/stream" in r or r in reads]
            kinds = ["config" if "/client-config" in r else "stream" if "/stream" in r else "read" for r in order]
            check(kinds[:3] == ["config", "stream", "read"],
                  f"the cold-start read did not wait for sign-in and the stream: {order}")
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline and not current(pill()):
                page.wait_for_timeout(50)
            check(current(pill()), f"a refreshed view still says {pill()!r}")
            check(str(page.evaluate(GEOMETRY_JS)["area"]) == baseline,
                  f"{label}: #scroll-area changed size when the pill turned current")
            shot("3-merged")

            # 4. Switching among views already covered is local. It ends on Main, the reader's
            # choice, which the reloads below reopen on.
            view("threads")
            page.wait_for_selector("#thread-list > li", timeout=5_000)
            view("main")
            wait_rows(["200", "201", "203"], "Main after the refresh")
            view("flat")
            wait_rows(["200", "201", "202", "203"], "All after switching back")
            view("main")
            wait_rows(["200", "201", "203"], "Main again")
            check(len(api.reads()) == 1, f"switching views read history again: {api.reads()}")
            shot("4-switched-locally")

            # 4a. `#197 floating-search`. Settings is still a title bar with a way back, and leaving
            # it leaves no strip behind. A real tap BELOW the disc — outside it, inside its 44px
            # square — opens the search as a bar over the top of the list; the bar filters; the
            # glass folds it again and the list is the box it was.
            page.click("#open-settings")
            title_bar = page.evaluate(TITLE_BAR_JS)
            shot("4a-settings-title-bar")
            check(bool(title_bar["shown"]) and float(title_bar["height"]) > 30
                  and title_bar["title"] == "Settings" and bool(title_bar["back"])
                  and not title_bar["glass"], f"{label}, Settings lost its title bar: {title_bar}")
            page.click("#close-settings")
            check(not page.evaluate(TITLE_BAR_JS)["shown"],
                  f"{label}: coming back from Settings left a header strip over the channel")
            centre = page.evaluate("() => { const b = document.getElementById('search-toggle')"
                                   ".getBoundingClientRect(); return [b.left + b.width / 2, b.bottom + 5]; }")
            if mobile:
                page.touchscreen.tap(float(centre[0]), float(centre[1]))
            else:
                page.mouse.click(float(centre[0]), float(centre[1]))
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline and page.evaluate("() => document.getElementById('search-field').hidden"):
                page.wait_for_timeout(50)
            opened = page.evaluate(SEARCH_OPEN_JS, "")
            shot("4a-search-open")
            check(not opened["problems"], f"{label}, a tap 5px below the glass's disc: {opened['problems']}")
            page.fill("#search-field", "thread root")
            found = page.evaluate(SEARCH_OPEN_JS, "thread root")
            shot("4a-search-filtered")
            check(not found["problems"], f"{label}, the search filtered: {found['problems']}")
            check(found["rows"] == ["201"] and found["count"] == "1 of 3 loaded",
                  f"{label}: the floating search left {found['rows']} saying {found['count']!r}")
            page.click("#search-toggle")
            check(bool(page.evaluate("() => document.getElementById('search-field').hidden")),
                  f"{label}: the glass did not fold the search bar")
            wait_rows(["200", "201", "203"], "folding the search did not put every row back")
            check(geometry("search folded", "4a-search-folded") == baseline,
                  f"{label}: #scroll-area changed size across opening and folding the search")

            # 4b. A failed refresh IN PLACE, over a list long enough to scroll: the pill's room is made
            # at the head of the list and the error panel takes a row above it, and both go again
            # when the next read succeeds. Browser scroll anchoring holds for neither, so the page
            # must: the row being read stays where it is on the screen, and a reader at the newest
            # line stays there (`#32 freshness-pill-overlap`, `#33 error-banner-scroll-shift`). At the
            # very top the header moves down out from under them instead. Tall rows, so that three
            # of them overflow.
            page.add_style_tag(content="#discord-log > li[data-id] { min-height: 70vh; }")
            for where in ("middle", "newest", "top"):
                before = place(where)
                parked = {"middle": 0 < before["top"] and before["gap"] > 100,
                          "newest": before["gap"] <= 2, "top": before["top"] == 0}[where]
                check(parked, f"{label}: could not park the reader at the {where}: {before}")
                # A failed POLL is carried by the pill alone since `#195 send-resilience`: nobody asked
                # for that read, so the error panel stays down and only the pill's room is made at the
                # head of the list. Twice, so a second failure after a recovery is held the same way.
                for step in ("fail", "recover", "fail", "recover"):
                    poll_channel(step == "fail")
                    after = place("")
                    state = f"{label}, reader at the {where}: {step}"
                    check(after["banner"] == 0.0, f"{state} but a background refresh raised the error panel: {after}")
                    check(abs(after["bottom"] - before["bottom"]) < 1, f"{state} and moved the list's foot: {after}")
                    if where == "middle":
                        moved = after["row"] - before["row"]
                        # Under 2px a step: a refresh makes several position fixes (its loading line
                        # arriving, the pill's room, the panel, the loading line going), each landing
                        # on a whole pixel. The defects were the pill's height, ~32px, and the
                        # panel's, 74-89px.
                        check(abs(moved) < 2, f"{state} and moved the row being read by {moved:.1f}px: {after}")
                    elif where == "newest":
                        check(after["gap"] <= 2, f"{state} and left the newest line {after['gap']:.1f}px below")
                    else:
                        check(after["top"] == 0, f"{state} and scrolled the header away: {after}")
                        if step == "fail":
                            geometry("in place at the top", "4b-top")
                    before = after
            shot("4b-in-place")

            # 4b2. `#207 scrollback-jump`, in the dark theme the owner reads in. At the newest line there
            # is no jump; halfway up the channel it is at the lower left of the chip row, legible, hit at
            # 44px and clear of everything around it — alone, beside every other chip forced up, and
            # beside them at a large system font, where the row has to wrap and the others go up a line.
            # Then a real tap a few pixels ABOVE the drawn chip lands on the newest message, reads
            # nothing, starts no pull, and the jump is gone. Each state is waited for (`wait_jump`).
            page.emulate_media(color_scheme="dark")
            page.evaluate(PARK_JS, 0)
            wait_jump(False)
            resting = page.evaluate(JUMP_JS)
            check(float(resting["gap"]) <= 2, f"{label}: could not park the reader at the newest line: {resting}")
            check(not resting["shown"], f"{label}: the jump to the newest message is up at the newest line: {resting}")
            page.evaluate(PARK_JS, "middle")
            wait_jump(True)
            back = page.evaluate(JUMP_JS)
            shot("4b2-jump-scrolled-back")
            check(bool(back["shown"]) and float(back["gap"]) > 100,
                  f"{label}: halfway up the channel there is no jump to the newest message: {back}")
            check(back["label"] == "Newest", f"{label}: the jump says {back['label']!r} with nothing new below")
            check(not back["problems"], f"{label}, the jump halfway up the channel: {back['problems']}")
            crowd = page.add_style_tag(content="#scroll-tools .chip[hidden] { display: inline-flex !important; }")
            for size in ("100%", "150%"):
                font = page.add_style_tag(content=f"html {{ font-size: {size} !important; }}")
                crowded = page.evaluate(JUMP_JS)
                shot(f"4b2-jump-beside-every-chip-{size.rstrip('%')}")
                lines = int(crowded["lines"])
                check(not crowded["problems"],
                      f"{label}, the jump beside every chip at {size} type, {lines} lines: {crowded['problems']}")
                # On a phone the full row does not fit one line, so this IS the overflow case.
                check(lines >= 2 or not mobile, f"{label}: every chip at {size} type fit one line, so nothing wrapped")
                font.evaluate("element => element.remove()")
            crowd.evaluate("element => element.remove()")
            ready = page.evaluate(JUMP_JS)
            check(bool(ready["shown"]) and not ready["problems"], f"{label}: the jump before the tap: {ready}")
            reads_before = len(api.reads())
            tap_x, tap_y = float(ready["centre"][0]), float(ready["top"]) - 4
            if mobile:
                page.touchscreen.tap(tap_x, tap_y)
            else:
                page.mouse.click(tap_x, tap_y)
            wait_jump(False)
            landed = page.evaluate(JUMP_JS)
            newest = page.evaluate(NEWEST_ROW_JS)
            shot("4b2-jump-tapped")
            check(float(landed["gap"]) <= 2,
                  f"{label}: a tap 4px above the jump left the newest line {float(landed['gap']):.1f}px below")
            check(bool(newest["visible"]), f"{label}: after the tap the end of the newest message is not on screen: {newest}")
            check(not landed["shown"], f"{label}: the jump is still up at the newest line: {landed}")
            check(len(api.reads()) == reads_before, f"{label}: the tap read the channel: {api.reads()[reads_before:]}")
            check(bool(page.evaluate("() => document.getElementById('pull-refresh').hidden")),
                  f"{label}: the tap started the pull's affordance")

            # ...and decided again when the list changes size under a reader who has NOT scrolled, which
            # no scroll event reports — so each change is made with the list's scroll events held back
            # from the page, and the scroll listener cannot be what answers it. The newest message growing
            # 200px below a reader on the newest line puts them up the history: the jump comes. Parked
            # 40px up with it showing, the newest message then shrinking by 30px puts them on the newest
            # line: it goes.
            # Taller than the row is drawn now, by exactly `px`: a min-height stated in vh would grow it
            # by less whenever its words already make it taller than that.
            drawn = float(page.evaluate(
                "() => [...document.querySelectorAll('#discord-log > li[data-id]')].pop().getBoundingClientRect().height"))

            def newest_row_taller(px: int) -> str:
                return (f"#discord-log > li[data-id]:last-child {{ box-sizing: border-box !important;"
                        f" min-height: {drawn + px}px !important; }}")
            # On the newest line, with the room under the composer settled: it can still be giving back the
            # height of the crowded chip rows above, and that moves the end the reader is measured against.
            park_still(0)
            page.evaluate("() => { window.__holdScrolls = true; }")
            grown = page.add_style_tag(content=newest_row_taller(200))
            wait_jump(True)
            below = page.evaluate(JUMP_JS)
            page.evaluate("() => { window.__holdScrolls = false; }")
            check(bool(below["shown"]) and float(below["gap"]) > 100,
                  f"{label}: the newest message grew 200px below a reader on the newest line, and no jump came: {below}")
            # Down from halfway up, so the jump is up by the scroll listener's own decision.
            page.evaluate(PARK_JS, "middle")
            wait_jump(True)
            park_still(40)
            parked = page.evaluate(JUMP_JS)
            check(bool(parked["shown"]) and abs(float(parked["gap"]) - 40) <= 1,
                  f"{label}: come down to 40px short of the end from halfway up, the jump is not still up: {parked}")
            page.evaluate("() => { window.__holdScrolls = true; }")
            grown.evaluate(f"(element) => {{ element.textContent = {json.dumps(newest_row_taller(170))}; }}")
            wait_jump(False)
            met = page.evaluate(JUMP_JS)
            page.evaluate("() => { window.__holdScrolls = false; }")
            check(not met["shown"] and float(met["gap"]) <= 24,
                  f"{label}: the newest message shrank 30px under a reader 40px up, onto the newest line, and the"
                  f" jump stayed: {met}")
            grown.evaluate("element => element.remove()")
            page.evaluate(PARK_JS, 0)
            wait_jump(False)
            page.emulate_media(color_scheme="light")

            # 4c. `#188 pull-refresh-bottom`, with a real finger: parked at the newest line, a drag UP
            # past the end of the list reads the channel once and says so at the foot of the list,
            # in this same page — no reload — and a drag short of the threshold reads nothing. The
            # touches go through Chromium's own input pipeline, so this is also the check that the
            # passive listeners still hear the overscroll at the end of a list that cannot scroll.
            if mobile:
                cdp = context.new_cdp_session(page)

                def drag_up(travel: float) -> None:
                    """One finger, landed on a message low in the list and drawn up, then lifted."""
                    box = page.evaluate("() => { const r = document.getElementById('scroll-area')"
                                        ".getBoundingClientRect(); return [r.left + r.width / 2,"
                                        " r.top + r.height * 0.6]; }")
                    x, y = float(box[0]), float(box[1])
                    steps = 12
                    cdp.send("Input.dispatchTouchEvent", {"type": "touchStart", "touchPoints": [{"x": x, "y": y}]})
                    for step in range(1, steps + 1):
                        page.wait_for_timeout(16)
                        cdp.send("Input.dispatchTouchEvent", {
                            "type": "touchMove", "touchPoints": [{"x": x, "y": y - travel * step / steps}]})
                    page.wait_for_timeout(16)
                    cdp.send("Input.dispatchTouchEvent", {"type": "touchEnd", "touchPoints": []})

                def affordance() -> tuple[str, str, str]:
                    found = page.evaluate("() => { const p = document.getElementById('pull-refresh');"
                                          " return [p.hidden ? '' : p.textContent,"
                                          " p.getAttribute('data-state'), p.getAttribute('data-edge')]; }")
                    return str(found[0]), str(found[1]), str(found[2])

                resting = place("newest")
                check(resting["gap"] <= 2, f"{label}: could not park the reader at the newest line: {resting}")
                page.evaluate("() => { window.pullRefreshPageMark = true; }")
                # The channel's own poll — or any other timer that re-reads it, such as the live
                # stream re-attaching — would be indistinguishable from the pull's read, so the page
                # clock stands still while the finger is down; the fetch itself needs no timer. Then
                # a moment for anything those timers already put on the wire to land before counting.
                page.clock.pause_at(int(page.evaluate("() => Date.now()")) + 1000)
                page.wait_for_timeout(500)
                with api.lock:
                    seen = len(api.requests)
                reads_before = len(api.reads())
                drag_up(30)
                page.wait_for_timeout(400)
                with api.lock:
                    during = api.requests[seen:]
                check(len(api.reads()) == reads_before,
                      f"{label}: a 30px drag at the newest line read the channel; requests since: {during}")
                check(affordance()[0] == "", f"{label}: a short drag left the affordance up: {affordance()}")
                drag_up(160)
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not affordance()[0].startswith("Updated"):
                    page.wait_for_timeout(50)
                shown = affordance()
                shot("4c-pulled-up")
                fresh = api.reads()[reads_before:]
                check(len(fresh) == 1 and "before=" not in fresh[0],
                      f"{label}: a pull up past the newest line made {len(fresh)} reads: {fresh}")
                check(re.fullmatch(r"Updated \d{2}:\d{2} · nothing new", shown[0]) is not None
                      and shown[2] == "end", f"{label}: the foot of the list said {shown}")
                check(bool(page.evaluate("() => window.pullRefreshPageMark === true")),
                      f"{label}: the pull up reloaded the page instead of re-reading the channel")
                page.clock.resume()
                # After the resume: `place` waits on animation frames, which the paused clock holds.
                check(place("")["gap"] <= 2, f"{label}: the refresh moved the reader off the newest line")
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and affordance()[0]:
                    page.wait_for_timeout(50)
                check(affordance()[0] == "", f"{label}: the pull's result never went: {affordance()}")
                cdp.detach()

            # 4d. `#189 restore-ui-state`. A reader parked mid-list, whose app goes away, comes back
            # on the same message. The tall rows are served in the stylesheet from here on, so every
            # later load lays them out before the page's script runs, as a phone does; the reload
            # is step 5's, with the API unreachable, so only the device can put the reader back.
            def tall_rows(route: Route) -> None:
                served = route.fetch()
                route.fulfill(response=served,
                              body=served.text() + "\n#discord-log > li[data-id] { min-height: 70vh; }\n")

            page.route("**/voice.css", tall_rows)
            reading = place("middle")
            check(reading["top"] > 0 and reading["gap"] > 100, f"{label}: could not park the reader mid-list: {reading}")

            # 5. The API unreachable: the rows stay, and the pill says they are old.
            page.route("**/api/**", lambda route: route.abort("internetdisconnected"))
            page.reload(wait_until="load")
            reopened("offline")
            wait_rows(["200", "201", "203"], "the snapshot was not kept while offline")
            # `#205 channel-view-memory`: Main came back as this channel's own choice, kept in the
            # browser's storage with the channel it was chosen in.
            record = json.loads(page.evaluate(f"() => localStorage.getItem({json.dumps(UI_STATE_KEY)})") or "{}")
            chosen = [(entry.get("channel"), entry.get("channelView")) for entry in record.get("channels", [])]
            check(record.get("v") == 2 and chosen == [(CHANNEL["id"], "main")],
                  f"{label}: Main was not kept as the channel's choice: v{record.get('v')} {chosen}")
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and not pill().startswith("Offline"):
                page.wait_for_timeout(50)
            check(pill().startswith("Offline · showing messages saved "), f"offline pill: {pill()!r}")
            back = place("")
            check(abs(back["row"] - reading["row"]) < 2,
                  f"{label}: the reopen moved the reader's message by {back['row'] - reading['row']:.1f}px: {back}")
            geometry("offline", "5-offline")
            page.unroute("**/api/**")

            # 5b. The API reachable but failing: the rows stay, and the pill says the refresh failed.
            page.route(timeline_read, failing)
            page.reload(wait_until="load")
            reopened("a failing refresh")
            wait_rows(["200", "201", "203"], "the snapshot was not kept after a failed refresh")
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and not pill().startswith("Refresh failed"):
                page.wait_for_timeout(50)
            check(pill().startswith("Refresh failed · showing messages from "), f"failed pill: {pill()!r}")
            geometry("failed", "5b-failed")
            page.unroute(timeline_read, failing)

            # 6. Nothing private is in Cache Storage, and no service worker holds one.
            cache_names = page.evaluate("() => caches.keys()")
            check(cache_names == [], f"Cache Storage holds {cache_names}")
            workers = page.evaluate("() => navigator.serviceWorker.getRegistrations().then(r => r.length)")
            check(workers == 0, f"{workers} service workers are registered")

            # 6b. A thread named longer than the screen: its title is ellipsised, not the page widened.
            view("threads")
            page.click("#thread-list > li button.thread-open")
            page.wait_for_selector("#thread-heading:not([hidden])", timeout=5_000)
            wait_rows(["201", "202"], "the long-titled thread did not open")
            found = page.evaluate(WIDTH_JS)
            shot("6b-long-thread-title")
            check(not found["problems"], f"{label}, a long-titled thread: {found['problems']}")
            check(bool(found["clipped"]) or not mobile, f"{label}: the long title was not ellipsised: {found}")
            heading = page.evaluate(HEADING_JS)
            check(not heading, f"{label}, a thread's heading and the floating line: {heading}")

            # 6c. Every floating chip up at once, over that thread scrolled to its end: none of them
            # covers the reply composer, which keeps their measured height as room under itself
            # (`#194 thread-picker-polish`). The rows are still 70vh tall from 4b, so the composer
            # really is at the foot of the list, where the chips float.
            forced = page.add_style_tag(content="#scroll-tools .chip[hidden] { display: inline-flex !important; }")
            covered = [str(problem) for problem in page.evaluate(COMPOSER_JS)]
            shot("6c-composer-clear-of-chips")
            check(not covered, f"{label}, the thread composer under the floating chips: {covered}")
            forced.evaluate("element => element.remove()")
            page.click("#thread-back")

            # 7. Signing out takes the rows off the screen and off the device.
            page.evaluate("() => document.getElementById('forget-token').click()")
            check(page.evaluate(f"() => localStorage.getItem({json.dumps(CACHE_KEY)})") is None,
                  "signing out left the snapshot on the device")
            check(rows() == [], f"signing out left rows on the screen: {rows()}")
            shot("7-signed-out")

            # 8. A fresh page and a READ-scope token: it reads the channel and asks for nothing it
            # cannot have. Reloaded first, so the stored record has not been looked for yet. Signing
            # out forgot where the reader was, so this opens on the call view, and the channel in All.
            page.reload(wait_until="load")
            console_errors.clear()
            refused.clear()
            page.fill("#api-token", READ_TOKEN)
            page.click("#save-token")
            page.click("#view-switch")
            wait_rows(["200", "201", "202", "203"], "a read-scope token did not read the channel")
            def storage_state() -> str:
                return str(page.evaluate("() => document.getElementById('storage-state').textContent"))

            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and not refused and "write-scope token" not in storage_state():
                page.wait_for_timeout(50)
            page.wait_for_timeout(300)
            shot("8-read-scope")
            check(not refused, f"{label}, read-scope token: the page was refused {refused}")
            check(not console_errors, f"{label}, read-scope token: console errors {console_errors}")
            check("write-scope token" in storage_state(),
                  f"{label}, read-scope token: Settings says {storage_state()!r} about the stored record")
            check(bool(page.evaluate("() => document.getElementById('error-wrap').hidden")),
                  f"{label}, read-scope token: the error panel is up")

            desktop = bool(page.evaluate("() => matchMedia('(min-width: 900px) and (pointer: fine)').matches"))
            check(desktop != mobile, f"{label}: the page chose the wrong layout regime")
            check(not errors, f"the page threw: {errors}")
            browser_version = context.browser.version if context.browser else "Chromium"
            context.close()
        return browser_version
    finally:
        api.stopping.set()
        api.timeline_gate.set()
        server.shutdown()
        server.server_close()
        thread.join()


def reply_walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
               mobile: bool) -> str:
    """`#204 reply-arrow`: the arrows' geometry, and a real tap on one, against a fresh profile."""
    api = ReplyApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/voice"
    platform = "Linux; Android 15; Pixel 7" if mobile else "X11; Linux x86_64"
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-") as profile:
            context = chromium.launch_persistent_context(
                profile,
                headless=True,
                executable_path=args.browser_executable,
                user_agent=(
                    f"Mozilla/5.0 ({platform}) AppleWebKit/537.36 (KHTML, like Gecko)"
                    f" Chrome/151.0.0.0 {'Mobile ' if mobile else ''}Safari/537.36"
                ),
                viewport={"width": width, "height": height},
                device_scale_factor=2.625 if mobile else 1,
                is_mobile=mobile,
                has_touch=mobile,
            )
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))

            def shot(name: str) -> None:
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / f"{label}-reply-{name}.png"))

            def where(row_id: str) -> dict[str, object]:
                found = page.evaluate(REPLY_ROW_JS, row_id)
                if not isinstance(found, dict):
                    raise AssertionError(f"{label}: row {row_id} could not be measured: {found!r}")
                return {str(key): value for key, value in found.items()}

            def number(found: dict[str, object], key: str) -> float:
                return float(str(found[key]))

            def tap(found: dict[str, object], key: str, why: str) -> None:
                """A real finger on a phone, a real mouse on a desk, at the centre `key` names."""
                at = found[key]
                if not isinstance(at, list) or len(at) != 2:
                    raise AssertionError(f"{label}: {why}: nothing to tap: {found}")
                x, y = float(at[0]), float(at[1])
                if mobile:
                    page.touchscreen.tap(x, y)
                else:
                    page.mouse.click(x, y)

            def wait_for(row_id: str, ready: str, why: str) -> dict[str, object]:
                deadline = time.monotonic() + 5
                found = where(row_id)
                while time.monotonic() < deadline and not page.evaluate(ready, found):
                    page.wait_for_timeout(50)
                    found = where(row_id)
                check(bool(page.evaluate(ready, found)), f"{label}: {why}: {found}")
                return found

            def arrows(state: str) -> None:
                if not mobile:
                    # Off every arrow, so no square being photographed has its hover disc behind it.
                    page.mouse.move(1, 1)
                listed = page.evaluate(REPLY_LIST_JS)
                problems = [str(problem) for problem in listed["problems"]]
                speakers: set[str] = set()
                states: set[str] = set()
                for row_id in listed["replies"]:
                    found = page.evaluate(REPLY_ARROW_JS, row_id)
                    problems += [str(problem) for problem in found["problems"]]
                    if "square" not in found:
                        continue
                    speakers.add(str(found["who"]))
                    states.add(str(found["state"]))
                    square = found["square"]
                    clip: FloatRect = {"x": float(square["x"]), "y": float(square["y"]),
                                       "width": float(square["width"]), "height": float(square["height"])}
                    png = page.screenshot(clip=clip)
                    drawn = page.evaluate(ARROW_INK_JS, {"png": base64.b64encode(png).decode("ascii"),
                                                         "square": square, "mark": found["mark"]})
                    which = f"{found['label']}, {found['state']}"
                    if float(drawn["contrast"]) < 3:
                        problems.append(f"{which}: the arrow is drawn {float(drawn['contrast']):.2f}:1 against"
                                        f" the page, as {drawn['ink']} on {drawn['ground']}")
                    accent = [round(float(value)) for value in re.findall(r"[0-9.]+", str(found["accent"]))[:3]]
                    if max(abs(int(seen) - want) for seen, want in zip(drawn["ink"], accent)) > 24:
                        problems.append(f"{which}: the arrow is drawn {drawn['ink']}, not the accent {accent}")
                check(not problems, f"{label}, {state}: {problems}")
                check(speakers == {"coder", "human", "me"},
                      f"{label}, {state}: the replies are from {sorted(speakers)}, not every kind of speaker")
                check(states == REPLY_STATES,
                      f"{label}, {state}: the replies are {sorted(states)}, not one in every state a row recedes in")

            page.goto(url, wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
            page.click("#view-switch")
            page.wait_for_selector('#discord-log > li[data-id="215"]', timeout=10_000)
            arrows("at the default type size")

            # The reader at the newest line, the third party's answer on screen and the question
            # it answers more than a screen above it. A real tap on the arrow.
            page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
            page.wait_for_timeout(200)
            reply = where("215")
            question = where("200")
            check(number(question, "bottom") < number(question, "line"),
                  f"{label}: the question was on screen before the tap: {question}")
            shot("1-before-tap")
            tap(reply, "arrow", "the reply's arrow")
            landed = wait_for("200", "(r) => r.landed === 'true'", "the tap did not land on the question")
            shot("2-landed")
            check(number(landed, "line") - 0.5 <= number(landed, "top") < number(landed, "foot") - 40,
                  f"{label}: the question landed at {landed['top']}px, not under the floating line at"
                  f" {landed['line']}px: {landed}")
            after = where("215")
            check(after["collapsed"] == reply["collapsed"],
                  f"{label}: the tap on the arrow also folded or opened the reply: {reply} -> {after}")
            check(after["chip"] is not None, f"{label}: the reply is off screen and Back to reply is not offered")

            # The way back, by a real tap on the chip.
            tap(after, "chip", "Back to reply")
            back = wait_for("215", "(r) => r.top < r.foot && r.bottom > r.line && r.chip === null",
                            "Back to reply did not bring the reply back and go")
            shot("3-back-to-reply")
            check(back["landed"] == "true", f"{label}: Back to reply did not say where it landed: {back}")

            # In reading mode a tap on a row reads it aloud. A tap on its arrow still only jumps.
            page.evaluate("() => document.getElementById('read-aloud').click()")
            page.evaluate("() => document.querySelector('#discord-log > li[data-id=\"215\"]')"
                          ".scrollIntoView({block: 'center'})")
            page.wait_for_timeout(300)
            with api.lock:
                seen = len(api.requests)
            tap(where("215"), "arrow", "the reply's arrow in reading mode")
            wait_for("200", "(r) => r.landed === 'true' && r.top < r.foot", "in reading mode the arrow did not jump")
            page.wait_for_timeout(300)
            with api.lock:
                spoken = [r for r in api.requests[seen:] if "/speak" in r or "/speech" in r]
            reading = where("215")
            check(not spoken and reading["reading"] != "true" and reading["pending"] != "true",
                  f"{label}: in reading mode the arrow's tap started reading the reply: {spoken} {reading}")
            page.evaluate("() => document.getElementById('read-aloud').click()")

            # A reader's large type: everything in the gutter is in rem, so it grows, and must still fit.
            large = page.add_style_tag(content="html { font-size: 150% !important; }")
            page.wait_for_timeout(200)
            arrows("at 150% type")
            shot("4-large-type")
            large.evaluate("element => element.remove()")

            # The owner's devices are in the dark scheme, where an accent close in value to the
            # surface is the defect this page has had before (see `data-reading` in web/voice.css).
            page.emulate_media(color_scheme="dark")
            page.wait_for_timeout(200)
            arrows("in the dark scheme")
            shot("5-dark")

            check(not errors, f"{label}: the page threw: {errors}")
            browser_version = context.browser.version if context.browser else "Chromium"
            context.close()
        return browser_version
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def tile_walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
              mobile: bool) -> int:
    """`#213 row-side-borders`: the coding agent's tiles, with four sides, beside the other speakers and
    in every state that draws a border differently, in the dark theme the owner reads in. Answers how
    many rows were measured with every speaker on the screen."""
    api = TileApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/voice"
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-tiles-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, device_scale_factor=2.625 if mobile else 1,
                is_mobile=mobile, has_touch=mobile, color_scheme="dark",
            )
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))

            def shot(name: str) -> None:
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / f"{label}-tiles-{name}.png"))

            def row(row_id: str) -> str:
                return f"document.querySelector('#discord-log > li[data-id=\"{row_id}\"]')"

            def wait_for(ready: str, why: str) -> None:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not page.evaluate(ready):
                    page.wait_for_timeout(50)
                check(bool(page.evaluate(ready)), f"{label}: {why}")

            def measured(where: str) -> int:
                if not mobile:
                    page.mouse.move(1, 1)  # off every row, so none is drawn as hovered
                found = page.evaluate(TILE_JS, {"edge": mobile})
                check(not found["problems"], f"{label}, {where}: {found['problems']}")
                return int(found["count"])

            # Being read and asked to be read need audio, and the kept place a gesture; the page suite
            # and tests/read_aloud_browser.py drive those. What is measured here is how the stylesheet
            # draws each state on a tile, so the attribute the page would set is set here.
            def as_if(row_id: str, attribute: str) -> None:
                page.evaluate(f"() => {row(row_id)}.setAttribute('{attribute}', 'true')")

            page.goto(url, wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
            page.click("#view-switch")
            page.wait_for_selector('#discord-log > li[data-id="211"]', timeout=10_000)
            page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
            page.wait_for_selector("#summarise", state="visible", timeout=10_000)
            page.click("#summarise")
            wait_for(f"() => {row(SUMMARISED_ROW)}.getAttribute('data-summarised') === 'true'",
                     "the long row at the newest line was not summarised")
            wait_for(f"() => {row('209')}.getAttribute('data-replied') === 'true'"
                     f" && {row('206')}.getAttribute('data-own-read') === 'true'",
                     "the third party's row is not answered, or the owner's not read as his own")
            as_if(READING_ROW, "data-reading")
            # The owner's question just under the floating line, so every speaker, the reply, the
            # summary and the row being read are on the screen together.
            page.evaluate(f"""() => {{
                const area = document.getElementById('scroll-area');
                let line = area.getBoundingClientRect().top;
                for (const id of ['channel-freshness', 'search-toggle']) {{
                    const floating = document.getElementById(id);
                    if (floating && floating.getClientRects().length) line = Math.max(line, floating.getBoundingClientRect().bottom);
                }}
                area.scrollTop += {row('206')}.getBoundingClientRect().top - line - 8;
            }}""")
            page.wait_for_timeout(200)
            count = measured("with the owner's question at the head of the list")
            shot("1-speakers")

            # The top of the channel, where the rest of the states are; the failed summary is asked
            # for as it comes into view.
            page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
            wait_for(f"() => {row(FAILED_SUMMARY_ROW)}.getAttribute('data-summary-failed') === 'true'",
                     "the row whose summary fails was not drawn as failed")
            wait_for(f"() => {row('201')}.getAttribute('data-pinned') === 'true'"
                     f" && {row('202')}.getAttribute('data-archived') === 'true'"
                     f" && {row('203')}.getAttribute('data-noise') === 'true'",
                     "the pinned, archived and placeholder rows are not in their states")
            as_if(MARKED_ROW, "data-marked")
            as_if(PENDING_ROW, "data-pending")
            page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 0; }")
            page.wait_for_timeout(200)
            measured("at the top")
            shot("2-states")

            if not mobile:
                # Under the pointer the whole border takes the accent, so all four sides do.
                page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
                page.wait_for_timeout(200)
                page.hover('#discord-log > li[data-id="211"] .body')
                found = page.evaluate(TILE_JS, {"edge": mobile})
                check(not found["problems"], f"{label}, under the pointer: {found['problems']}")
                shot("3-hover")

            check(not errors, f"{label}: the page threw: {errors}")
            context.close()
        return count
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def pinned_elsewhere(api: FakeApi) -> FakeApi:
    """`api` with the thread root, row 201, already pinned — by another device, before this page."""
    api.serve_pins_revision = True
    api.pins["201"] = {
        "message_id": "201", "author": "ci-bot", "author_id": "1000000000000000001", "author_is_bot": True,
        "content": "a thread root", "truncated": False, "timestamp": str(api.messages[1]["timestamp"]),
        "thread_id": "spaces/A/threads/one", "thread_root": True, "pinned_at_ms": 1_790_000_000_000,
    }
    api.pins_revision = 1
    return api


def menu_walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
              root_percent: int) -> None:
    """`#206 pin-message`: every row's ⋯ menu on the screen where the meta line wraps, in both shapes.

    A phone in the dark theme, at one of `MENU_SCALES`. Each row's menu in the channel is opened by a
    real tap and measured, then the pinned row's in the Pinned filter, whose Unpin is then tapped for
    real — the item a menu hung off the left of the screen put out of reach.
    """
    for provider, upstream in (("Discord", False), ("Google Chat", True)):
        api = pinned_elsewhere(FakeApi())
        api.provider_name, api.upstream_read = provider, upstream
        server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
        server.daemon_threads = True
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        shape = f"{label}, {provider}"
        try:
            with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-menus-") as profile:
                context = chromium.launch_persistent_context(
                    profile, headless=True, executable_path=args.browser_executable,
                    viewport={"width": width, "height": height}, device_scale_factor=2.625,
                    is_mobile=True, has_touch=True, color_scheme="dark",
                )
                page = context.pages[0]
                larger_root_font(page, root_percent)
                errors: list[str] = []
                page.on("pageerror", lambda error: errors.append(str(error)))

                def until(script: str, why: str) -> None:
                    deadline = time.monotonic() + 10
                    while time.monotonic() < deadline and not page.evaluate(script):
                        page.wait_for_timeout(50)
                    check(bool(page.evaluate(script)), f"{shape}: {why}")

                def tap(selector: str) -> None:
                    # Scrolled to the middle of the list first: at a large font a row's controls can
                    # be past either end of it, and the floating bar sits over its top.
                    found = page.evaluate(
                        "(selector) => { const e = document.querySelector(selector);"
                        " const area = document.getElementById('scroll-area');"
                        " if (area.contains(e)) { const a = area.getBoundingClientRect(), b = e.getBoundingClientRect();"
                        "   area.scrollTop += (b.top + b.bottom) / 2 - (a.top + a.bottom) / 2; }"
                        " const b = e.getBoundingClientRect(); return [b.left + b.width / 2, b.top + b.height / 2]; }",
                        selector)
                    page.touchscreen.tap(float(found[0]), float(found[1]))

                def menu_of(list_id: str, row_id: str) -> str:
                    row = f'#{list_id} > li[data-id="{row_id}"]'
                    hidden = f"document.querySelector('{row} .row-more-menu').hidden"
                    tap(f"{row} .row-more-button")
                    until(f"() => !{hidden}", f"row {row_id}'s ⋯ menu in #{list_id} did not open")
                    menu = page.evaluate(MENU_JS, {"list": list_id, "id": row_id, "upstream": upstream})
                    if args.screenshots:
                        page.screenshot(path=str(args.screenshots / f"{label}-{provider.split()[0].lower()}-"
                                                                     f"menu-{list_id}-{row_id}.png"))
                    check(not menu["problems"], f"{shape}, row {row_id}'s ⋯ menu in #{list_id}: {menu['problems']}"
                                                f" ({menu['items']})")
                    return hidden

                page.goto(f"http://127.0.0.1:{server.server_port}/voice", wait_until="load")
                page.fill("#api-token", TOKEN)
                page.click("#save-token")
                page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
                page.click("#view-switch")
                until("() => document.querySelectorAll('#discord-log > li[data-id]').length === 3",
                      "the channel did not open")
                until("() => { const c = document.querySelector('#discord-log > li[data-id=\"201\"] .pin-chip');"
                      " return Boolean(c) && !c.hidden; }", "the pin made elsewhere never showed on its row")
                for row_id in ("200", "201", "202"):
                    hidden = menu_of("discord-log", row_id)
                    page.keyboard.press("Escape")
                    until(f"() => {hidden}", f"Escape left row {row_id}'s ⋯ menu open")

                # The pinned row again, in the Pinned filter, and its Unpin reached by a thumb.
                tap("#search-toggle")
                until("() => !document.getElementById('pinned-filter').hidden", "the open bar has no Pinned filter")
                tap("#pinned-filter")
                until("() => document.querySelectorAll('#pinned-log > li[data-id]').length === 1",
                      "the pinned row did not show in the filter")
                menu_of("pinned-log", "201")
                tap('#pinned-log > li[data-id="201"] .row-pin-button')
                until("() => !document.querySelector('#pinned-log > li[data-id]')", "Unpin left the row in the filter")
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and "201" in api.pins:
                    page.wait_for_timeout(50)
                check("201" not in api.pins, f"{shape}: a tap on Unpin never reached the server")
                check(not errors, f"{shape}: the page threw: {errors}")
                context.close()
        finally:
            api.stopping.set()
            server.shutdown()
            server.server_close()
            thread.join()


def pin_walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
             mobile: bool) -> None:
    """`#206 pin-message`, in the dark theme the owner reads in, against a fresh fake API."""
    api = pinned_elsewhere(FakeApi())
    # The shape whose ⋯ menu is widest: the read group holds both items, the longer label naming the
    # service. `menu_walk` takes the one-item shape too.
    api.provider_name, api.upstream_read = "Google Chat", True
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-pins-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, device_scale_factor=2.625 if mobile else 1,
                is_mobile=mobile, has_touch=mobile, color_scheme="dark",
            )
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))

            def shot(name: str) -> None:
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / f"{label}-pins-{name}.png"))

            def tap(selector: str) -> None:
                found = page.evaluate(f"() => {{ const b = document.querySelector({json.dumps(selector)})"
                                      ".getBoundingClientRect(); return [b.left + b.width / 2, b.top + b.height / 2]; }")
                if mobile:
                    page.touchscreen.tap(float(found[0]), float(found[1]))
                else:
                    page.mouse.click(float(found[0]), float(found[1]))

            def until(script: str, why: str) -> None:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not page.evaluate(script):
                    page.wait_for_timeout(50)
                check(bool(page.evaluate(script)), f"{label}: {why}")

            def ids(list_id: str) -> list[str]:
                return [str(row) for row in page.eval_on_selector_all(
                    f"#{list_id} > li[data-id]", "items => items.map(i => i.getAttribute('data-id'))")]

            page.goto(f"http://127.0.0.1:{server.server_port}/voice", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            page.click("#view-switch")
            until("() => document.querySelectorAll('#discord-log > li[data-id]').length === 3", "the channel did not open")

            # A pin made elsewhere: read because the channel's read carried its revision.
            until("() => { const c = document.querySelector('#discord-log > li[data-id=\"201\"] .pin-chip');"
                  " return Boolean(c) && !c.hidden; }", "the pin made elsewhere never showed on its row")
            chip = page.evaluate(CHIP_JS, "201")
            shot("1-pinned-chip")
            check(not chip["problems"], f"{label}, the pinned chip in the dark theme: {chip['problems']}")

            # The ⋯ menu, opened by a real tap: on the pinned row, whose chip takes room on its meta
            # line, and closed by Escape; then on an unpinned one, left open for its Pin.
            for row_id in ("201", "200"):
                hidden = f"document.querySelector('#discord-log > li[data-id=\"{row_id}\"] .row-more-menu').hidden"
                tap(f'#discord-log > li[data-id="{row_id}"] .row-more-button')
                until(f"() => !{hidden}", f"row {row_id}'s ⋯ menu did not open")
                menu = page.evaluate(MENU_JS, {"list": "discord-log", "id": row_id, "upstream": True})
                shot(f"2-menu-{row_id}")
                check(not menu["problems"], f"{label}, row {row_id}'s ⋯ menu: {menu['problems']} ({menu['items']})")
                if row_id == "201":
                    page.keyboard.press("Escape")
                    until(f"() => {hidden}", "Escape left the ⋯ menu open")

            # Pin, shown at once and stored.
            tap('#discord-log > li[data-id="200"] .row-pin-button')
            until("() => { const c = document.querySelector('#discord-log > li[data-id=\"200\"] .pin-chip');"
                  " return Boolean(c) && !c.hidden; }", "Pin did not show on the row")
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline and "200" not in api.pins:
                page.wait_for_timeout(50)
            check("200" in api.pins, f"{label}: the pin never reached the server")

            # The Pinned filter on the open bar, before and after the count takes its room.
            tap("#search-toggle")
            until("() => !document.getElementById('pinned-filter').hidden", "the open bar has no Pinned filter")
            bare = page.evaluate(FILTER_BAR_JS, {"pinned": True, "fieldMin": 140})
            check(not bare["problems"], f"{label}, the filters on the open bar: {bare['problems']}")
            page.fill("#search-field", "a")
            counted = page.evaluate(FILTER_BAR_JS, {"pinned": True, "fieldMin": 140})
            shot("3-filter-beside-glass")
            check(not counted["problems"], f"{label}, the filters beside a count: {counted['problems']}")
            page.fill("#search-field", "")

            # Its tap shows the pins, and none of them is under the bar.
            tap("#pinned-filter")
            until("() => document.getElementById('pinned-filter').getAttribute('aria-pressed') === 'true'",
                  "a tap on the filter did not turn it on")
            until("() => document.querySelectorAll('#pinned-log > li[data-id]').length === 2", "the pins did not show")
            check(ids("pinned-log") == ["200", "201"], f"{label}: the filter showed {ids('pinned-log')}")
            covered = page.evaluate("() => { const bar = document.getElementById('search-float').getBoundingClientRect();"
                                    " document.getElementById('scroll-area').scrollTop = 0;"
                                    " const first = document.querySelector('#pinned-log > li');"
                                    " return first.getBoundingClientRect().top - bar.bottom; }")
            shot("4-pinned-shown")
            check(float(covered) >= -0.5, f"{label}: the open bar covers {-float(covered):.1f}px of the first pin")
            check(page.evaluate("() => document.getElementById('discord-log').hidden"),
                  f"{label}: the channel's list stayed up under the filter")

            # The glass folds the bar, and the filter with it.
            tap("#search-toggle")
            until("() => document.getElementById('pinned-log').hidden", "folding the bar left the filter on")
            check(page.evaluate("() => document.getElementById('pinned-filter').getAttribute('aria-pressed')") == "false",
                  f"{label}: the filter still says it is on")
            check(ids("discord-log") == ["200", "201", "202"], f"{label}: the channel came back as {ids('discord-log')}")
            check(not errors, f"{label}, pins: the page threw: {errors}")
            context.close()
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def links_walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
               mobile: bool) -> None:
    """`#211 link-filter`, in the dark theme the owner reads in, against a fresh fake API.

    The glass larger than it was; the open bar holding Links and Pinned beside the glass, each a 44px
    target clear of the field and the glass, with the field still 140px wide; Links showing only the
    rows with links, each link real and on its own line; a real tap on a link opening a new tab
    without folding its row; and, over the call view, Links beside the glass on its own.
    """
    api = LinkApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-links-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, device_scale_factor=2.625 if mobile else 1,
                is_mobile=mobile, has_touch=mobile, color_scheme="dark",
            )
            opened: list[str] = []

            def answer(route: Route) -> None:
                opened.append(route.request.url)
                route.fulfill(status=200, content_type="text/html", body="<title>opened</title>opened")

            for pattern in LINK_HOSTS:
                context.route(pattern, answer)
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))

            def shot(name: str) -> None:
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / f"{label}-links-{name}.png"))

            def point(selector: str) -> tuple[float, float]:
                found = page.evaluate(f"() => {{ const b = document.querySelector({json.dumps(selector)})"
                                      ".getBoundingClientRect(); return [b.left + b.width / 2, b.top + b.height / 2]; }")
                return float(found[0]), float(found[1])

            def tap(selector: str) -> None:
                x, y = point(selector)
                if mobile:
                    page.touchscreen.tap(x, y)
                else:
                    page.mouse.click(x, y)

            def until(script: str, why: str) -> None:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not page.evaluate(script):
                    page.wait_for_timeout(50)
                check(bool(page.evaluate(script)), f"{label}: {why}")

            page.goto(f"http://127.0.0.1:{server.server_port}/voice", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            page.click("#view-switch")
            until("() => document.querySelectorAll('#discord-log > li[data-id]').length === 5", "the channel did not open")
            glass = page.evaluate("() => { const b = document.getElementById('search-toggle').getBoundingClientRect();"
                                  " return [b.width, b.height]; }")
            check(float(glass[0]) >= GLASS_MIN_PX and float(glass[1]) >= GLASS_MIN_PX,
                  f"{label}: the glass is drawn {glass[0]}x{glass[1]}px, under {GLASS_MIN_PX}px")
            shot("1-glass")

            # The open bar: Links, then Pinned, then the glass; then the same with a count beside them.
            tap("#search-toggle")
            until("() => !document.getElementById('links-filter').hidden", "the open bar has no Links filter")
            bare = page.evaluate(FILTER_BAR_JS, {"pinned": True, "fieldMin": 140})
            check(not bare["problems"], f"{label}, the filters on the open bar: {bare['problems']}")
            page.fill("#search-field", "a")
            counted = page.evaluate(FILTER_BAR_JS, {"pinned": True, "fieldMin": 140})
            shot("2-bar-with-count")
            check(not counted["problems"], f"{label}, the filters beside a count: {counted['problems']}")
            page.fill("#search-field", "")

            # A real tap on Links: only the rows with links, each showing only its links.
            folded = page.evaluate("() => document.querySelector('#discord-log > li[data-id=\"200\"]')"
                                   ".getAttribute('data-collapsed')")
            check(folded == "true", f"{label}: the long message did not arrive folded ({folded!r})")
            tap("#links-filter")
            until("() => document.getElementById('links-filter').getAttribute('aria-pressed') === 'true'",
                  "a tap on Links did not turn it on")
            until("() => document.querySelectorAll('#discord-log > li[data-links-view=\"true\"]').length === 3",
                  "the rows with links did not show their links")
            view = page.evaluate(LINK_VIEW_JS)
            shot("3-links-shown")
            check(not view["problems"], f"{label}, the Links view: {view['problems']}")
            rows = {row_id: links for row_id, links in view["rows"]}
            check(sorted(rows) == ["200", "202", "203"], f"{label}: the Links view shows rows {sorted(rows)}")
            check([link[:2] for link in rows["200"]] == [["https://example.com/build/7", "https://example.com/build/7"],
                                                         ["the diff", "https://example.org/diff/7"]],
                  f"{label}: row 200 shows {rows['200']}")
            check([link[:2] for link in rows["202"]] == [["the dashboard", "https://example.com/dash"]],
                  f"{label}: row 202 shows {rows['202']}")
            check(rows["203"][0][1] == LONG_ADDRESS and bool(rows["203"][0][4]),
                  f"{label}: the long address is not whole in its href and cut short on screen: {rows['203']}")
            for links in rows.values():
                for link in links:
                    check(link[2] == "_blank" and link[3] == "noopener noreferrer",
                          f"{label}: {link[0]} does not open in a new tab on its own: {link}")

            # A real tap on a link opens it in a new tab, and the row it is in does not fold.
            anchor = '#discord-log > li[data-id="200"] .row-link[href="https://example.org/diff/7"]'
            x, y = point(anchor)
            hit = page.evaluate(f"() => document.elementFromPoint({x}, {y}) === document.querySelector({json.dumps(anchor)})")
            check(bool(hit), f"{label}: something covers the link to be tapped")
            with context.expect_page(timeout=10_000) as tab:
                tap(anchor)
            opened_page = tab.value
            opened_page.wait_for_load_state()
            check(opened_page.url == "https://example.org/diff/7", f"{label}: the tap opened {opened_page.url}")
            check(page.url.endswith("/voice"), f"{label}: the tap navigated the app itself to {page.url}")
            opened_page.close()
            after = page.evaluate("() => { const row = document.querySelector('#discord-log > li[data-id=\"200\"]');"
                                  " return [row.getAttribute('data-collapsed'), row.getAttribute('data-links-view'),"
                                  " Boolean(row.querySelector('.msg-details'))]; }")
            shot("4-link-tapped")
            check(after[0] == folded and after[1] == "true" and not after[2],
                  f"{label}: the tap on a link also did something to its row: {after}")

            # The glass folds the bar, and Links with it.
            tap("#search-toggle")
            until("() => document.getElementById('links-filter').hidden", "folding the bar left Links on screen")
            check(page.evaluate("() => document.getElementById('links-filter').getAttribute('aria-pressed')") == "false",
                  f"{label}: Links still says it is on")
            until("() => !document.querySelector('#discord-log li.search-hidden, #discord-log .row-links')",
                  "folding the bar left the Links view up")

            # Over the call view there are no pins: Links stands beside the glass on its own.
            page.click("#view-switch")
            page.wait_for_selector("#pane-voice", state="visible", timeout=5_000)
            tap("#search-toggle")
            until("() => !document.getElementById('links-filter').hidden", "the call view's bar has no Links filter")
            alone = page.evaluate(FILTER_BAR_JS, {"pinned": False, "fieldMin": 140})
            shot("5-call-view-bar")
            check(not alone["problems"], f"{label}, the call view's bar: {alone['problems']}")
            check(not errors, f"{label}, links: the page threw: {errors}")
            check(opened == ["https://example.org/diff/7"], f"{label}: the walk opened {opened}")
            context.close()
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def links_bar_walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
                   root_percent: int) -> float:
    """`#211 link-filter`: the open bar with Links on, where a reader's own settings narrow the line.

    A phone in the dark theme at one of `BAR_SCALES`. A real tap opens the bar and another turns Links
    on, which puts the count up with nothing typed, so the bar holds everything it ever holds: the
    field, the count, Links, Pinned and the glass. The glass is still its full size, each filter a 44px
    target clear of its neighbours, and the field still leaves room for a query's text; the Links view
    under the bar shows each link on its own 44px line, none of it under the bar; and the kinds of link
    under Links (`#212 link-filter`) are 44px targets under its icon and inside the screen, clear of the
    bar and the first row. Answers the room the field leaves for its text.
    """
    api = LinkApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    shape = f"{label}, the open bar"
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-bar-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, device_scale_factor=2.625,
                is_mobile=True, has_touch=True, color_scheme="dark",
            )
            page = context.pages[0]
            larger_root_font(page, root_percent)
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))

            def until(script: str, why: str) -> None:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not page.evaluate(script):
                    page.wait_for_timeout(50)
                check(bool(page.evaluate(script)), f"{shape}: {why}")

            def tap(selector: str) -> None:
                found = page.evaluate(f"() => {{ const b = document.querySelector({json.dumps(selector)})"
                                      ".getBoundingClientRect(); return [b.left + b.width / 2, b.top + b.height / 2]; }")
                page.touchscreen.tap(float(found[0]), float(found[1]))

            page.goto(f"http://127.0.0.1:{server.server_port}/voice", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            page.click("#view-switch")
            until("() => document.querySelectorAll('#discord-log > li[data-id]').length === 5", "the channel did not open")
            tap("#search-toggle")
            until("() => !document.getElementById('links-filter').hidden", "the open bar has no Links filter")
            tap("#links-filter")
            until("() => document.getElementById('links-filter').getAttribute('aria-pressed') === 'true'",
                  "a tap on Links did not turn it on")
            until("() => document.querySelectorAll('#discord-log > li[data-links-view=\"true\"]').length === 3",
                  "the rows with links did not show their links")
            until("() => document.getElementById('search-count').textContent === '3 of 5 loaded'",
                  "the count is not up beside the field")
            glass = page.evaluate("() => { const b = document.getElementById('search-toggle').getBoundingClientRect();"
                                  " return [b.width, b.height]; }")
            check(float(glass[0]) >= GLASS_MIN_PX and float(glass[1]) >= GLASS_MIN_PX,
                  f"{shape}: the glass is {glass[0]}x{glass[1]}px, under {GLASS_MIN_PX}px")
            bar = page.evaluate(FILTER_BAR_JS, {"pinned": True, "fieldMin": 0})
            view = page.evaluate(LINK_VIEW_JS)
            # `#212 link-filter`. The kinds of link under Links, with no GitHub link loaded and so no
            # number on any of them: centred under Links where they fit, and inside the screen at
            # every one of these sizes.
            kinds = page.evaluate(KINDS_JS, {"pinned": True})
            if args.screenshots:
                page.screenshot(path=str(args.screenshots / f"{label}-bar-links-on.png"))
            check(not bar["problems"], f"{shape}, with Links on: {bar['problems']}")
            check(not view["problems"], f"{shape}, the Links view under it: {view['problems']}")
            check(not kinds["problems"], f"{shape}, the kinds of link under Links: {kinds['problems']}")
            check([chip[2] for chip in kinds["chips"]] == ["", "", ""], f"{shape}: the kinds read {kinds['chips']}")
            shown = sorted(str(row[0]) for row in view["rows"])
            check(shown == ["200", "202", "203"], f"{shape}: the Links view shows rows {shown}")
            check(not errors, f"{shape}: the page threw: {errors}")
            context.close()
            return float(bar["text"])
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def kinds_walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
               mobile: bool, root_percent: int) -> str:
    """`#212 link-filter`: the kinds of link under Links, in the dark theme, against a fresh `KindsApi`.

    A real tap on Links brings up PRs, Commits and Actions, all on and numbered, measured by
    `KINDS_JS`; their words are legible on and off and the two states look different. Real taps then
    turn the kinds off one by one and the Links view loses those links, row by row, a row with none
    left going; a reload keeps the choice; turned back on, every link returns. The glass takes the row
    away, and over the call view it is right-aligned under the bar's end. Answers how the row was
    placed over the channel.
    """
    api = KindsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    shape = f"{label}, the kinds of link"
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-kinds-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, device_scale_factor=2.625 if mobile else 1,
                is_mobile=mobile, has_touch=mobile, color_scheme="dark",
            )
            page = context.pages[0]
            larger_root_font(page, root_percent)
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))

            def shot(name: str) -> None:
                if args.screenshots:
                    page.screenshot(path=str(args.screenshots / f"{label}-kinds-{name}.png"))

            def until(script: str, why: str) -> None:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not page.evaluate(script):
                    page.wait_for_timeout(50)
                check(bool(page.evaluate(script)), f"{shape}: {why}")

            def tap(selector: str) -> None:
                found = page.evaluate(f"() => {{ const b = document.querySelector({json.dumps(selector)})"
                                      ".getBoundingClientRect(); return [b.left + b.width / 2, b.top + b.height / 2]; }")
                if mobile:
                    page.touchscreen.tap(float(found[0]), float(found[1]))
                else:
                    page.mouse.click(float(found[0]), float(found[1]))

            def links_on() -> None:
                tap("#search-toggle")
                until("() => !document.getElementById('links-filter').hidden", "the open bar has no Links filter")
                check(page.evaluate("() => document.getElementById('link-kinds').hidden"),
                      f"{shape}: the kinds are up before Links is on")
                tap("#links-filter")
                until("() => document.getElementById('links-filter').getAttribute('aria-pressed') === 'true'",
                      "a tap on Links did not turn it on")
                until("() => !document.getElementById('link-kinds').hidden", "Links came on without its kinds")

            def links_shown() -> dict[str, list[str]]:
                view = page.evaluate(LINK_VIEW_JS)
                check(not view["problems"], f"{shape}, the Links view: {view['problems']}")
                return {str(row_id): [str(link[1]) for link in links] for row_id, links in view["rows"]}

            def toggle(kind: str, pressed: str) -> None:
                tap(f"#link-kind-{kind}")
                until(f"() => document.getElementById('link-kind-{kind}').getAttribute('aria-pressed') === '{pressed}'",
                      f"a tap on {kind} did not turn it {'on' if pressed == 'true' else 'off'}")

            page.goto(f"http://127.0.0.1:{server.server_port}/voice", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            page.click("#view-switch")
            until("() => document.querySelectorAll('#discord-log > li[data-id]').length === 6", "the channel did not open")

            # Links on: the three kinds under it, all on, numbered, every link in view.
            links_on()
            kinds = page.evaluate(KINDS_JS, {"pinned": True})
            shot("1-kinds-on")
            check(not kinds["problems"], f"{shape}, over the channel: {kinds['problems']}")
            check(kinds["chips"] == [["PRs", "true", "13"], ["Commits", "true", "2"], ["Actions", "true", "1"]],
                  f"{shape}: the kinds read {kinds['chips']}")
            everything = {
                "200": [f"{REPO}/pull/41", "https://example.com/docs/design.html"],
                "201": [f"{REPO}/actions/runs/123456/job/789", f"{REPO}/commit/abc1234def"],
                "202": [f"{REPO}/commit/0123456789abcdef0123456789abcdef01234567"],
                "203": ["https://example.org/spec", f"{REPO}/issues/7"],
            }
            shown = links_shown()
            check({k: v for k, v in shown.items() if k != "205"} == everything and len(shown.get("205", [])) == 12,
                  f"{shape}: with every kind on, the Links view shows {shown}")

            # PRs off: the pull requests leave their rows, and the row of nothing else goes.
            toggle("pr", "false")
            until("() => !document.querySelector('#discord-log > li[data-id=\"205\"]').offsetParent",
                  "the row holding only pull requests stayed")
            shown = links_shown()
            shot("2-prs-off")
            check(shown == {**everything, "200": ["https://example.com/docs/design.html"]},
                  f"{shape}: with PRs off the Links view shows {shown}")
            on, off = (page.evaluate(KIND_LOOK_JS, f"link-kind-{kind}") for kind in ("commit", "pr"))
            check(on["look"][0] != off["look"][0] and on["look"][2] != off["look"][2] and on["look"][3] != off["look"][3],
                  f"{shape}: on and off are drawn alike: {on['look']} and {off['look']}")
            check("line-through" in off["look"][1] and "line-through" not in on["look"][1],
                  f"{shape}: off is not struck through, or on is: {off['look']}, {on['look']}")
            check(on["contrast"] >= 4.5 and off["contrast"] >= 4.5,
                  f"{shape}: the words on a kind read at {on['contrast']:.2f}:1 on and {off['contrast']:.2f}:1 off")

            # Commits and Actions off too: the documents, and the issue, which is no kind.
            toggle("commit", "false")
            toggle("action", "false")
            until("() => document.getElementById('search-count').textContent === '2 of 6 loaded'",
                  "the count does not say what the kinds left")
            shown = links_shown()
            check(shown == {"200": ["https://example.com/docs/design.html"], "203": everything["203"]},
                  f"{shape}: with every kind off the Links view shows {shown}")
            kinds = page.evaluate(KINDS_JS, {"pinned": True})
            shot("3-every-kind-off")
            check(not kinds["problems"], f"{shape}, every kind off: {kinds['problems']}")
            check([chip[:2] for chip in kinds["chips"]] == [["PRs", "false"], ["Commits", "false"], ["Actions", "false"]],
                  f"{shape}: the kinds read {kinds['chips']}")
            stored = page.evaluate("() => localStorage.getItem('vibe-talk.voice.hidden-link-kinds')")
            check(stored == '["pr","commit","action"]', f"{shape}: the device kept {stored!r}")

            # A reload keeps the choice; turned back on, every link is back.
            page.reload(wait_until="load")
            until("() => document.querySelectorAll('#discord-log > li[data-id]').length === 6"
                  " && !document.getElementById('pane-discord').hidden", "the reload did not reopen the channel")
            links_on()
            kinds = page.evaluate(KINDS_JS, {"pinned": True})
            check([chip[1] for chip in kinds["chips"]] == ["false", "false", "false"],
                  f"{shape}: after a reload the kinds read {kinds['chips']}")
            check(set(links_shown()) == {"200", "203"}, f"{shape}: after a reload the hidden kinds came back")
            for kind in ("pr", "commit", "action"):
                toggle(kind, "true")
            until("() => document.getElementById('search-count').textContent === '5 of 6 loaded'",
                  "turning every kind back on did not bring every row back")
            shown = links_shown()
            check({k: v for k, v in shown.items() if k != "205"} == everything and len(shown.get("205", [])) == 12,
                  f"{shape}: turned back on, the Links view shows {shown}")

            # The glass takes the row away with the bar.
            tap("#search-toggle")
            until("() => document.getElementById('link-kinds').hidden"
                  " && !document.getElementById('screen-main').hasAttribute('data-link-kinds')",
                  "folding the bar left the kinds up")

            # Over the call view Links stands beside the glass: right-aligned under the bar's end.
            page.click("#view-switch")
            page.wait_for_selector("#pane-voice", state="visible", timeout=5_000)
            links_on()
            call = page.evaluate(KINDS_JS, {"pinned": False})
            shot("4-call-view")
            check(not call["problems"], f"{shape}, over the call view: {call['problems']}")
            check(call["placement"] == "right-aligned", f"{shape}: over the call view the kinds are {call['placement']}")
            tap("#search-toggle")
            until("() => document.getElementById('link-kinds').hidden", "folding the bar over the call left the kinds up")
            check(not errors, f"{shape}: the page threw: {errors}")
            context.close()
            return str(kinds["placement"])
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def crowded_kinds_walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
                       root_percent: int) -> float:
    """`#212 link-filter`: the row of kinds at its widest — "99+" on every button — on a narrow line.

    A phone in the dark theme, against `CrowdedKindsApi`, at one of `CROWDED_KINDS_SIZES`: Links on by
    real taps, then `KINDS_JS`. With the words free to grow with the type, this row was 355px wide at
    150% on a 360px phone and ran off the left of the screen. Answers how wide it is.
    """
    api = CrowdedKindsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    shape = f"{label}, the kinds of link at their widest"
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-crowded-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, device_scale_factor=2.625,
                is_mobile=True, has_touch=True, color_scheme="dark",
            )
            page = context.pages[0]
            larger_root_font(page, root_percent)
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))

            def until(script: str, why: str) -> None:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not page.evaluate(script):
                    page.wait_for_timeout(50)
                check(bool(page.evaluate(script)), f"{shape}: {why}")

            def tap(selector: str) -> None:
                found = page.evaluate(f"() => {{ const b = document.querySelector({json.dumps(selector)})"
                                      ".getBoundingClientRect(); return [b.left + b.width / 2, b.top + b.height / 2]; }")
                page.touchscreen.tap(float(found[0]), float(found[1]))

            page.goto(f"http://127.0.0.1:{server.server_port}/voice", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            page.click("#view-switch")
            until("() => document.querySelectorAll('#discord-log > li[data-id]').length === 4", "the channel did not open")
            tap("#search-toggle")
            until("() => !document.getElementById('links-filter').hidden", "the open bar has no Links filter")
            tap("#links-filter")
            until("() => !document.getElementById('link-kinds').hidden", "Links came on without its kinds")
            kinds = page.evaluate(KINDS_JS, {"pinned": True})
            if args.screenshots:
                page.screenshot(path=str(args.screenshots / f"{label}-kinds-crowded.png"))
            check(not kinds["problems"], f"{shape}: {kinds['problems']}")
            check([chip[2] for chip in kinds["chips"]] == ["99+", "99+", "99+"], f"{shape}: the kinds read {kinds['chips']}")
            span = page.evaluate("() => document.getElementById('link-kinds').getBoundingClientRect().width")
            check(not errors, f"{shape}: the page threw: {errors}")
            context.close()
            return float(span)
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


def empty_walk(chromium: BrowserType, args: argparse.Namespace, label: str, width: int, height: int,
               root_percent: int, mode: str) -> float:
    """`#212 link-filter`, from review: the sentence a list the filters emptied says, where it is read.

    A phone in the dark theme at one of `EMPTY_SIZES`, the channel opened at its newest line, and the
    list emptied by real taps and typing as `mode` says. The sentence must then be wholly on the screen
    below the bar and the kinds of link (`EMPTY_SENTENCE_JS`): the composer below every list kept the
    list overflowing with all its rows gone, so a reader left at the newest line had the sentence under
    the kinds' three buttons — at 412px and 150% type — or, with the keyboard up, above the screen.
    No jump to the newest message is offered past it. Undone the same way, the rows come back with the reader at the newest line again. Answers how far
    the list was scrolled while the sentence was up.
    """
    api: FakeApi = LinklessApi() if mode == "linkless" else EmptyKindsApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    shape = f"{label}, an emptied list ({mode})"
    try:
        with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-empty-") as profile:
            context = chromium.launch_persistent_context(
                profile, headless=True, executable_path=args.browser_executable,
                viewport={"width": width, "height": height}, device_scale_factor=2.625,
                is_mobile=True, has_touch=True, color_scheme="dark",
            )
            page = context.pages[0]
            larger_root_font(page, root_percent)
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))

            def until(script: str, why: str) -> None:
                deadline = time.monotonic() + 10
                while time.monotonic() < deadline and not page.evaluate(script):
                    page.wait_for_timeout(50)
                check(bool(page.evaluate(script)), f"{shape}: {why}")

            def tap(selector: str) -> None:
                found = page.evaluate(f"() => {{ const b = document.querySelector({json.dumps(selector)})"
                                      ".getBoundingClientRect(); return [b.left + b.width / 2, b.top + b.height / 2]; }")
                page.touchscreen.tap(float(found[0]), float(found[1]))

            def pressed(selector: str, value: str, why: str) -> None:
                tap(selector)
                until(f"() => document.querySelector({json.dumps(selector)}).getAttribute('aria-pressed') === '{value}'", why)

            def settled() -> None:
                # Two frames, so a scroll the page made has been laid out and its events delivered.
                page.evaluate("() => new Promise((done) => requestAnimationFrame(() => requestAnimationFrame(done)))")

            def at_newest() -> bool:
                return bool(page.evaluate("() => { const a = document.getElementById('scroll-area');"
                                          " return a.scrollHeight - a.scrollTop - a.clientHeight <= 24; }"))

            page.goto(f"http://127.0.0.1:{server.server_port}/voice", wait_until="load")
            page.fill("#api-token", TOKEN)
            page.click("#save-token")
            page.wait_for_selector("#search-toggle", state="visible", timeout=10_000)
            page.click("#view-switch")
            until("() => document.querySelectorAll('#discord-log > li[data-id]').length === 4", "the channel did not open")
            settled()
            check(at_newest(), f"{shape}: the channel did not open at its newest line")
            tap("#search-toggle")
            until("() => !document.getElementById('links-filter').hidden", "the open bar has no Links filter")
            if mode != "search":
                pressed("#links-filter", "true", "a tap on Links did not turn it on")
            if mode == "kinds":
                for kind in ("pr", "commit", "action"):
                    pressed(f"#link-kind-{kind}", "false", f"a tap on {kind} did not turn it off")
            elif mode in ("typed", "search"):
                page.fill("#search-field", "nothing to open" if mode == "typed" else "no such words anywhere")
            sentence = EMPTY_SENTENCES[mode]
            until(f"() => document.getElementById('search-empty').textContent.trim() === {json.dumps(sentence)}"
                  " && !document.getElementById('search-empty').hidden", f"the emptied list does not say {sentence!r}")
            settled()
            empty = page.evaluate(EMPTY_SENTENCE_JS)
            if args.screenshots:
                page.screenshot(path=str(args.screenshots / f"{label}-empty-{mode}.png"))
            check(not empty["problems"], f"{shape}, scrolled to {empty['top']}: {empty['problems']}")
            check(bool(page.evaluate("() => document.getElementById('jump-newest').hidden")),
                  f"{shape}: the jump to the newest message is offered over a list with none shown")

            # Undone the way it was done: the rows are back, at the newest line, and the sentence gone.
            if mode == "kinds":
                pressed("#link-kind-pr", "true", "a tap on PRs did not turn it back on")
            elif mode in ("typed", "search"):
                page.fill("#search-field", "")
            else:
                pressed("#links-filter", "false", "a tap on Links did not turn it off")
            until("() => document.getElementById('search-empty').hidden"
                  " && Boolean(document.querySelector('#discord-log > li[data-id=\"200\"]').offsetParent)",
                  "undoing the filter did not bring the first row back")
            settled()
            check(at_newest(), f"{shape}: the rows came back with the reader short of the newest line")
            check(not errors, f"{shape}: the page threw: {errors}")
            context.close()
            return float(empty["top"])
    finally:
        api.stopping.set()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
