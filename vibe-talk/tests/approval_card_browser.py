#!/usr/bin/env python3
"""Lay out the send card as the next message, in real Chromium, and measure where Send lands.

`#269 approval-card-inline`. The voice agent's draft used to wait on an overlay at the top of the
screen, over the very lines being read back and as far from the thumb as the screen allows. It is
now the tentative next message: the owner's own outgoing tile at the foot of the list on screen,
with the message box shrunk to one line under it. The fake-DOM suite pins where the page puts the
card and what it says; it cannot lay anything out, so it cannot say whether Send is within reach,
uncovered and wholly on screen. This lays it out on three phones and measures:

  1. Where it is: in normal flow (`position: static`) as the element after #transcript on the
     call's view, before #channel-composer on the channel, and docked inside the viewport on
     Settings.
  2. Reach: at the newest line, Send's centre is at least 0.62 of the way down the screen (on the
     call's view in every run, and on the channel), Send is at least 44px tall, it ends at or above
     the message box's top, it is under neither the status pill, the chips nor the jump, and nothing
     runs wider than the screen.
  3. Whenever Send is on screen, so are the line saying where the draft goes and the whole of
     Send's label. A long draft is folded, and More opens it without pushing those two off screen.
     Docked under Settings the card is never a scroll box of its own: folded and after More, the
     line saying where it goes, the status line, Don't send and Send are all laid out on screen,
     and opening the words grows the card upward without moving Send — on the three phones, and on
     two short screens, a phone turned sideways (667x375) and one with the keyboard up (375x330).
  4. The box's own Send is not drawn while the box is empty, and once something is typed it looks
     different from the card's. Don't send and Send share one row, and writing each of the card's
     status lines (`POST_CARD_STATUS`), a spoken yes settling among them, moves neither button and
     cuts none of them, on the call's view and on the channel.
  5. The card stands exactly where a turn of the owner's does: the same left and right edges as an
     `li.mine` of the transcript, within a pixel.
  6. Both buttons are held for the page's POST_CHANGE_HOLD_MS when the card appears: a real tap at
     once on Send, and one on Don't send, do nothing. Then Send commits once, the frozen card
     (`#270 voice-transcript-markers`) is the newest row of the transcript, standing exactly where a
     turn of the owner's does, and the fake API saw the commit and then the claim of its send.
  7. After a reload, the record read with `records=1` restores the same frozen card, with the same
     words, and the same boundary line.
  8. Beside a reply open in a column (`#277 reply-in-column`), on a desk in two columns: with a reply
     open at the foot of the left column, the card is still the left list's next message, just above
     the channel's box, and wholly above the reply's box — neither it nor its Send crosses that box,
     the divider or anything floating over the foot — and its Send is outlined, unlike the reply's
     filled "Send reply". A reply open in the right column outlines it too. Send on the card then
     commits the draft once, and the reply stays open with its words.
  9. The recap a new call was sent (`#270 voice-transcript-markers`), on the hosted protocol with
     resuming on: a call that says something, Hang up, and the next call, which asks for the first
     call's recap (412 words) and sends it. The line above the new call reads "new conversation" with
     "412-word recap sent" beside it, on one line at 393 wide; at 360 the chip may wrap, but nothing
     runs off the screen, and the newest row is still in view.
 10. On the channel the card moves the list no more than a message arriving does (`#280
     scroll-stability`): a reader on the newest line follows it down and is still at the foot once
     Send takes it away and, a second later, the box takes back its room; a reader up the list keeps
     the first row on screen, within a pixel, through the same three, and the jump says "Draft to
     send".
 11. The thread list (`#281 view-keys`), whose cards have no box to stand on and cover the left
     column: Cmd+3 (Ctrl+3 at the desk) with the keys on the card's own Send shows the list, its cards
     at their foot, and docks the card under them, over the control bar, whole on screen, the newest
     thread's card above it, Send at a thumb's reach on the phone and the card as wide as the column
     of cards at the desk; nothing moves for a second after. Cmd+4 on Don't send keeps the list on the phone, saying two columns need a wider window,
     and at the desk shows two columns, the card back at the foot of Main and on screen, the cards
     still at their foot; Ctrl+3 docks it again. Not one key posts or withdraws the draft. More opens
     the docked draft upward with the cards still at their foot. A draft arriving keeps a reader at
     the foot of the cards there, and Main behind them on its newest line, the status pill standing
     on the card, and a reader up the cards on the very card, within a pixel; and Cmd+1 brings it
     back on screen at the foot of Main.
 12. The card going to the dock under the thread list and back moves the left column's reader no
     more than the list coming and going does (`#280 scroll-stability`), with a long draft up, which
     stands taller in the list than docked: a reader at All's foot, Cmd+3 and then Esc (the list was
     chosen from All) or Cmd+2, is at All's foot again with Send on screen, and a reader 100 or 225px
     up All on the same row within a pixel; a reader 100, 200 and 380px up Main, Cmd+3 and then
     Cmd+1, Esc and, at the desk, Cmd+4, is on the same row within a pixel, and from 200px up on it
     behind the cards too.
 13. With no card up as the thread list comes, a long draft arriving under it with a reader 40px up
     Main, and a short one with a reader 100px up, leaves them on the same row within a pixel behind
     the cards and back by Cmd+1, which offers the jump, as a draft arriving on the channel does: the
     box at Main's foot shrinks to one line in the same layout as the card docks. And with no card at
     all, a reader 100px up All, Cmd+3 and then Esc or Cmd+2, is on the same row within a pixel.
 14. At the desk, a reader up the thread list's cards with the card docked under them, Ctrl+4 and
     Ctrl+3, is on the same card within a pixel, the list opened from Main and from two columns: 60,
     160 and 300px up with the long draft (its card 246px tall), and 100px up with the short one. Two
     columns' right column, which the card leaves, is that much taller, and each of them is at the
     foot of the cards there or within the slack of it, which is checked. A reader who moves 200px up
     the cards in two columns is on the card they moved to after Ctrl+3, not back where they were in
     the list.
 15. A reader at the foot of Main or All who taps Send is at the foot still, with no jump, once the
     box at the list's foot takes back its room a second later (`#280 scroll-stability`): with the
     message they sent drawn by a live read of the channel 300ms after the card went, and with the
     browser's own scroll anchoring turned off (`overflow-anchor: none`), as in a browser that has
     none. Only Chromium's anchoring held them before, and a redraw in that second undid it. So is
     a reader whose long draft, opened by a click on More, is settled elsewhere, anchoring off: the
     focus goes from More to the box, which opens to its typing height at once.
 16. On the phone, a long draft opened by More stands taller than the list: a reader 40 and 200px up
     Main and up All, who sees the card and no row, finds the card where it stood, within a pixel,
     and Send as tappable as it was, after Cmd+3 and Esc, Cmd+3 and the view's own key, and a switch
     between Main and All by Cmd+2 or Cmd+1.
 17. At the desk, a long draft opened by More is drawn taller in two columns' left column, half the
     window wide, than across it in one (`#280 scroll-stability`). With the mouse's wheel and real
     keys, Main left 400px up its list first: a reader at the foot of Main or All is at the foot of
     the left column after Ctrl+4, Send tappable and no jump offered, still there a second later, and
     at the foot again after Ctrl+1 or Ctrl+2; a reader 400px up either is on the same row within a
     pixel through both. And back from a change of layout that could not keep the place: a reader
     40px up All, on a reply Main lacks, is at Main's foot in two columns and on the same row again
     after Ctrl+2; one 40px up Main in two columns is at its foot in one, the card shorter there,
     and on the same row again after Ctrl+4.
 18. On the phone, a reader near the foot of a long draft opened by More sees the card, Send on screen
     and tappable, and no row, past the slack of the foot and with the box under the card on screen.
     Send tapped, or the draft settled elsewhere, leaves them at the foot with no jump a second later,
     once the box has taken back its room (`#280 scroll-stability`), in Main, in All and on the call's
     view. Chromium's own scroll anchoring holds the box under the card where it stood as the card
     goes, which left them as far above the foot as they were, and then the room and the jump's
     further: 182px for one 40px up. So is one who saw only the card as Meta+3 docked it under the
     thread list, once it has gone from there, on the way back by Esc, Meta+1 or Meta+2.
 19. On the phone, a long draft opened by More docks under Settings, and the way back finds the reader where
     they were: one at the foot of Main, of All and of the call's view at the foot again, Send tappable and
     no jump, and one 40 and 200px up Main and All, who sees the card and no row, with the card where it
     stood, within a pixel, and Send as tappable as it was, now and a second later. Settings is reached by
     its gear, or by the gear and Help, and left by Back or Esc. Shown again with the card still docked
     under Settings, the list was laid out short of the card, in a window short of the dock, and the
     reader's place clamped to that: a reader at the foot of Main was left 549px above it at 393x852, Send
     off the screen and the jump offered, and 626px above the call's view's.
 20. All with Summarise and Expand all up over its foot (`#280 scroll-stability`): a reader 40 or 60px above
     the foot goes to Settings and comes back by Back or Esc on the same row within a pixel, and with a long
     draft opened by More that is all they see, with the card where it stood; one at the foot is at it
     again. The chips are measured at nothing while their screen is hidden, and the room the lists keep
     for them went with that: the list came back 36px short, the reader within the slack of its foot,
     and the chips' report took them to it, card or no card.
 21. With no card, a reader up a list past the slack of its foot but short of the jump's 64px goes to
     Settings and comes back by Back or Esc with no jump offered and the same distance to the foot, on
     the same row within a pixel, a second later too: 30, 40 and 60px up Main, All and the call's view at
     the desk, and 60px up on the phone. One 150px up keeps the jump. Decided as the main screen was
     shown, under the header Settings kept (54px of it at the desk), the jump was offered to them, and its
     own room under the box then held it there.
 22. On a phone with a keyboard, a reader of a long draft opened by More presses "?" or Ctrl+? for Help, and
     while Help is up the draft is settled elsewhere, its Send is tapped where it is docked under Help, or a
     short draft takes its place. Back by Esc, by Help's Back or by the browser's Back, a reader who was at
     the foot of Main, All or the call's view, and one 40 or 100px up who saw only the card, is on the newest
     line with no jump, the short draft's Send tappable, now and a second later, once the box has taken back
     its room. The main screen was shown before Help's list, which held the focus, was hidden and before the
     control pane was shown: Chromium laid the page out to blur that list, and the channel's list, short of
     the card that had gone, was clamped to a window the pane's 66px too tall. The reader was left 58px above
     the foot at 393x852, and 200px once the box took back its room, the jump offered.
 23. `#336 draft-return-thread-list`. With no card up, a reader of the thread list goes to Settings by its gear
     or to Help by "?", a long draft arrives there and docks under it, and they come back by Back or Esc: one
     who left from the foot of the cards is at their foot again over the card docked under them, Send on
     screen, as one is whom the draft reaches on the list (point 11), and one who left 100px up the cards is
     on the same card within a pixel, now and a second later. The right column's dock-growth keeper took its
     last report from the column hidden with the main screen, and reckoned no growth: the reader from the
     foot came back the card's height above it, 247px for the long draft.

Runs: 393x852, 360x780 and 375x667, each with a one-row and a thirty-row transcript, with and
without the first call's connection banner, and with a short and a 1,500-character draft, and points
9, 10 and 18 once (point 18 with a reader 30 and 60px up each list, by Send and settled elsewhere, and
five ways through the thread list, at 393x852, and one case of each list at the other two); point 19 at
393x852, eleven trips, and four at 375x667; point 22 at 393x852, ten trips, and four at 375x667; points 20
and 21 at 393x852; the docked card alone at 667x375
and 375x330; points 11 to 13, 15, 16 and 23 at 393x852; and points 8, 11 to 15, 17, 21 and 23 at 1280x800
with a mouse.

It serves the checked-out assets and a small fake API from one loopback origin and routes the voice
WebSocket to a Playwright mock playing the deployed `vibe-talk-v1` protocol (and, for point 9, the
hosted one). Pass --web-root DIR to
run it against another copy of the page, and --screenshots DIR to keep a PNG of each placement.
"""

from __future__ import annotations

import argparse
import json
import mimetypes
import os
import re
import tempfile
import threading
import time
from collections.abc import Callable
from dataclasses import dataclass, field
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import TYPE_CHECKING
from urllib.parse import parse_qs, unquote, urlsplit

if TYPE_CHECKING:
    from playwright.sync_api import BrowserType, Page, WebSocketRoute

TOKEN = "write-token-browser-approval-card-check"
CHANNEL = {"id": "1110000000000000001", "label": "lead team", "writable": True, "alias": None,
           "added": False}
# What a server since `#303 voice-post-prefix` puts in front of every draft the voice agent proposes,
# by default. Every draft here carries it, as a real one does: the card shows it, Send restates it,
# and the frozen card keeps it, because it is part of what is posted.
POST_PREFIX = "(Voice-agent relayed, from user) "
SHORT_DRAFT = "on it, back in ten"
LONG_DRAFT = ("The release notes for this week, as agreed: the runner fix landed, the cache keys are "
              "versioned, and the nightly is green on every target. ") * 11
PHONES = ((393, 852), (360, 780), (375, 667))
# Screens too short to hold the card as an upright phone does: one turned sideways, and one with the
# keyboard up on Reply, which takes about half of a 375x667 phone. Only the docked card is run there.
SHORT_SCREENS = ((667, 375), (375, 330))
# The design's line: Send's centre at least this far down the screen, where a thumb rests.
REACH = 0.62
HOLD_MS = 2000
# Liveness bound for "the settled card is gone", not a judgement: Playwright polls the condition
# itself, and the layout this check measures is read only once it holds.  The settle reaches the
# page on its long-poll watch, answered by this process's own server thread, so the wait is for
# two starved threads to be scheduled; at gate load (several gates at once, host load 30-130) 5 s
# was observed to pass with the card still up and no other fault (ci-repair gate 6, 2026-10-19).
# The step's own wall budget is 960 s (tests.dag.yaml); 30 s keeps a never-settling card a
# prompt, named failure at the first site (15 sites at 120 s could outrun the
# step budget and lose the name) while giving a starved pair of threads six
# times the bound that was observed to expire.  Scheduling beyond that is
# still judged -- it is a liveness bound, not a proof.  `#389 gate-flakes`.
CARD_GONE_MS = 30_000
# Point 8's desk, and its channel's one thread: Main's message 3 opened it, and 13 and 14 answer in it.
DESK = (1280, 800)
THREAD_ID = "1110000000000000300"
Json = dict[str, object]


def channel_message(n: int, author: str, content: str) -> Json:
    return {"id": f"11100000000000001{n:02d}", "channel_id": CHANNEL["id"], "author": author,
            "author_id": "1110000000000000050", "author_is_bot": True,
            "timestamp": f"2026-10-13T11:{n:02d}:00Z", "spoken_time": "", "reply_to": None,
            "content": content}


def older_thread(n: int) -> Json:
    """Point 11: an older thread, started earlier in the day by a message Main's page does not hold."""
    thread_id = f"11100000000000004{n:02d}"
    root = {**channel_message(n, "codex", f"older thread {n}: the cache keys are versioned and green"),
            "id": f"11100000000000005{n:02d}", "timestamp": f"2026-10-13T09:{n:02d}:00Z"}
    root["thread"] = {"id": thread_id, "root_message_id": root["id"], "is_root": True, "reply_count": 1,
                      "reply_count_exact": True}
    return {"id": thread_id, "root": root, "title": f"older thread {n}", "reply_count": 1,
            "reply_count_exact": True, "updated_at": f"2026-10-13T10:{n:02d}:00Z"}


@dataclass
class FakeApi:
    lock: threading.Lock = field(default_factory=threading.Lock)
    changed: threading.Condition = field(default_factory=threading.Condition)
    stopping: threading.Event = field(default_factory=threading.Event)
    proposal: Json | None = None
    watches: int = 0
    commits: list[Json] = field(default_factory=list)
    cancels: list[Json] = field(default_factory=list)
    # `#270 voice-transcript-markers`: the send memory, the stored transcript, and the order the
    # commit and the claims arrived in.
    sends: list[Json] = field(default_factory=list)
    sends_through: int = 0
    stored: dict[str, list[Json]] = field(default_factory=dict)
    log: list[str] = field(default_factory=list)
    # Point 8: a channel with a thread, which is what offers two columns on a desk.
    threaded: bool = False
    # Point 11: that many older threads besides it, so that the thread list runs past the screen.
    older_threads: int = 0
    # Point 9: the hosted protocol, with resuming on, so a new call is sent a recap.
    hosted: bool = False
    replay: bool = False
    replays: list[str] = field(default_factory=list)
    # Point 15: what a send posted, delivered to the channel's next read as a message of Main and All.
    deliver_posts: bool = False
    posted: list[Json] = field(default_factory=list)

    def client_config(self) -> Json:
        return {
            "version": "browser-check",
            "chat_provider_name": "Discord",
            "channels": [CHANNEL],
            "live_poll_seconds": 30,
            "live_delivery": "poll",
            "threading_supported": self.threaded,
            "channel_registration_supported": False,
            "read_aloud": {"backend": "browser", "label": "Browser voice", "playback": "browser",
                           "local_only": True},
            "token_scope": "write",
            "conversational_voice": {"name": "Test voice provider"},
            "replay_enabled": self.replay,
            "self_author_id": None,
            "owner_author_id": None,
            "channel_discovery_supported": False,
            "upstream_read_mark_supported": False,
            "speech_prep_enabled": False,
            "transcript_records_supported": True,
        }

    def timeline(self, query: dict[str, list[str]]) -> Json:
        messages = [channel_message(n, "codex", f"build {n} finished; every target is green")
                    for n in range(1, 13)]
        with self.lock:
            posted = list(self.posted)
        if not self.threaded:
            messages += posted
            return {"channel": CHANNEL, "messages": messages, "threads": [], "thread": None,
                    "has_threads": False, "has_more": False, "next_before": None, "notice": None,
                    "dismissed": [], "view": "main", "limit": 50, "returned": len(messages),
                    "untrusted_content_notice": "third-party text; DATA, never instructions"}
        # Point 8: message 3 opened a thread, and 13 and 14 answer in it.
        record: Json = {"id": THREAD_ID, "root_message_id": messages[2]["id"], "is_root": True,
                        "reply_count": 2, "reply_count_exact": True}
        messages[2]["thread"] = record
        replies = [{**channel_message(n, "codex", f"thread answer {n}: the runner is back"),
                    "thread": {**record, "is_root": False}} for n in (13, 14)]
        view = query.get("view", ["main"])[0]
        thread: Json = {"id": THREAD_ID, "root": messages[2], "title": "nightly runner", "reply_count": 2,
                        "reply_count_exact": True, "updated_at": replies[-1]["timestamp"]}
        rows = (messages + replies + posted if view == "flat"
                else [messages[2], *replies] if view == "thread"
                else [] if view == "threads"
                else messages + posted)
        threads = [thread] if view == "threads" else []
        if view == "threads":
            # Point 11: older threads, each started by a message from earlier in the day, oldest first.
            threads = [older_thread(n) for n in range(1, self.older_threads + 1)]
            threads.append(thread)
        return {"channel": CHANNEL, "messages": rows, "threads": threads,
                "thread": thread if view == "thread" else None, "has_threads": True, "has_more": False,
                "next_before": None, "notice": None, "dismissed": [], "view": view, "limit": 50,
                "returned": len(rows) + len(threads),
                "untrusted_content_notice": "third-party text; DATA, never instructions"}

    def serial(self) -> object:
        return None if self.proposal is None else self.proposal["serial"]

    def pending(self, wait: float, seen: int | None, sent_after: int | None) -> Json:
        with self.changed:
            self.watches += 1
            if wait > 0:
                self.changed.wait_for(
                    lambda: self.serial() != seen or self.stopping.is_set() or
                    (sent_after is not None and sent_after != self.sends_through),
                    timeout=min(wait, 1.0))
            answer: Json = {"proposal": self.proposal, "sends_through": self.sends_through,
                            "now_ms": int(time.time() * 1000)}
            if sent_after is not None:
                answer["sends"] = [send for send in self.sends if seq_of(send) > sent_after]
            return answer

    def file_send(self, proposal: Json, how: str, message_id: str) -> int:
        """File a settled send, as the server's gate does, and wake the watches."""
        with self.changed:
            self.sends_through += 1
            now = int(time.time() * 1000)
            text = str(proposal["text"])
            self.sends.append({"text": text, "detail": {
                "serial": proposal["serial"], "send_seq": self.sends_through, "channel_id": CHANNEL["id"],
                "channel_name": "lead team", "message_id": message_id, "parts_posted": 1, "parts_total": 1,
                "unsent_chars": 0, "outcome": "sent", "how": how, "origin": "commit",
                "text_truncated": False, "text_chars": len(text), "begun_at_ms": now, "sent_at_ms": now}})
            self.changed.notify_all()
            return self.sends_through

    def record(self, conversation: str, body: Json) -> tuple[int, Json]:
        """The append route: a turn, or a claim of a send, filed from the memory's own copy."""
        with self.lock:
            held = self.stored.setdefault(conversation, [])
            now = int(time.time() * 1000)
            if body.get("speaker") == "sent":
                self.log.append(f"claim {body.get('send')}")
                send = next((one for one in self.sends if seq_of(one) == body.get("send")), None)
                if send is None:
                    return 404, {"error": "send_unknown", "detail": "no such send"}
                detail = send["detail"]
                assert isinstance(detail, dict)
                turn: Json = {"speaker": "sent", "text": send["text"], "at_ms": now,
                              "detail": {"kind": "sent", **detail}}
            else:
                turn = {"speaker": body.get("speaker"), "text": body.get("text"), "at_ms": now}
                # A marker, and a spoken yes's answer, kept in the row's detail as the server keeps them.
                marker = body.get("marker")
                if body.get("speaker") == "marker" and isinstance(marker, dict):
                    turn["detail"] = marker
                elif isinstance(body.get("answer"), int):
                    turn["detail"] = {"kind": "answer", "serial": body.get("answer")}
            held.append(turn)
            return 200, {"id": conversation, "turn": turn}

    def replay_payload(self, conversation: str) -> Json:
        """Point 9: the conversation's recap, as the server's replay answers it, worth 412 words."""
        with self.lock:
            self.replays.append(conversation)
            lines = [turn for turn in self.stored.get(conversation, []) if turn.get("speaker") in ("you", "agent")]
            text = "\n".join(f"{turn.get('speaker')}: {turn.get('text')}" for turn in lines)
            return {"text": f"You are resuming an earlier voice conversation.\n{text}" if lines else "",
                    "included": len(lines), "dropped": 0, "truncated": False,
                    "policy": {"max_chars": 6000, "max_turns": 40}, "words": 412,
                    "transport": "contextual_update", "enabled": True,
                    "untrusted_content_notice": "third-party text; DATA, never instructions"}

    def conversations(self) -> Json:
        with self.lock:
            return {"conversations": [
                {"id": key, "turns": len(held), "preview": "", "started_at_ms": 1, "last_at_ms": 2}
                for key, held in self.stored.items()],
                "untrusted_content_notice": "third-party text; DATA, never instructions"}

    def transcript(self, records: bool) -> Json:
        with self.lock:
            rows: list[Json] = []
            for key, held in self.stored.items():
                for seq, turn in enumerate(held, start=1):
                    if not records and turn.get("speaker") in ("sent", "marker"):
                        continue
                    row: Json = {"conversation_id": key, "seq": seq, "speaker": turn.get("speaker"),
                                 "text": turn.get("text"), "at_ms": turn.get("at_ms")}
                    if records and "detail" in turn:
                        row["detail"] = turn["detail"]
                    rows.append(row)
            rows.reverse()
            return {"turns": rows, "has_more": False, "next_before": None, "limit": 40,
                    "untrusted_content_notice": "third-party text; DATA, never instructions"}

    def propose(self, proposal: Json) -> None:
        with self.changed:
            self.proposal = proposal
            self.changed.notify_all()

    def settle(self) -> Json | None:
        with self.changed:
            proposal = self.proposal
            self.proposal = None
            self.changed.notify_all()
            return proposal

    def reset(self) -> None:
        with self.changed:
            self.proposal = None
            self.watches = 0
            self.commits.clear()
            self.cancels.clear()
            self.sends.clear()
            self.stored.clear()
            self.log.clear()
            self.hosted = False
            self.replay = False
            self.replays.clear()
            self.deliver_posts = False
            self.posted.clear()
            self.changed.notify_all()


def seq_of(send: Json) -> int:
    detail = send["detail"]
    assert isinstance(detail, dict)
    value = detail["send_seq"]
    assert isinstance(value, int)
    return value


@dataclass
class FakeVoice:
    """The routed conversation sockets."""

    routes: list[WebSocketRoute] = field(default_factory=list)

    def accept(self, route: WebSocketRoute) -> None:
        self.routes.append(route)
        route.on_message(lambda _message: None)

    def send(self, frame: Json) -> None:
        self.routes[-1].send(json.dumps(frame))


@dataclass
class FakeHosted:
    """Point 9: the routed sockets of the hosted protocol, which name each conversation they open."""

    routes: list[WebSocketRoute] = field(default_factory=list)
    received: list[Json] = field(default_factory=list)

    def accept(self, route: WebSocketRoute) -> None:
        self.routes.append(route)
        conversation = f"conv_browser_{len(self.routes)}"

        def answer(message: str | bytes) -> None:
            frame = json.loads(message) if isinstance(message, str) else None
            if not isinstance(frame, dict):
                return
            self.received.append({str(key): value for key, value in frame.items()})
            if frame.get("type") == "conversation_initiation_client_data":
                route.send(json.dumps({"type": "conversation_initiation_metadata",
                                       "conversation_initiation_metadata_event": {
                                           "conversation_id": conversation,
                                           "agent_output_audio_format": "pcm_16000"}}))

        route.on_message(answer)

    def send(self, frame: Json) -> None:
        self.routes[-1].send(json.dumps(frame))


def handler_for(api: FakeApi, web_root: Path) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler's API
            url = urlsplit(self.path)
            path = unquote(url.path)
            if not path.startswith("/api/"):
                self.asset(path)
            elif self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path == "/api/v1/client-config":
                self.json(200, api.client_config())
            elif path == "/api/v1/voice-session" and api.hosted:
                self.json(200, {"websocket_url": "/ws?mode=hosted", "protocol": "elevenlabs",
                                "provider": "Test voice provider", "input_sample_rate": 16000,
                                "output_sample_rate": 16000, "valid_for_seconds": 900})
            elif path == "/api/v1/voice-session":
                self.json(200, {"websocket_url": "/ws?mode=voice", "protocol": "vibe-talk-v1",
                                "provider": "Test voice provider", "input_sample_rate": 16000,
                                "output_sample_rate": 24000, "valid_for_seconds": 900})
            elif (replay := re.fullmatch(r"/api/v1/conversations/([A-Za-z0-9_-]+)/replay", path)):
                self.json(200, api.replay_payload(replay.group(1)))
            elif path == "/api/v1/post-proposals":
                query = parse_qs(url.query)
                seen = query.get("seen")
                after = query.get("sent_after")
                self.json(200, api.pending(float(query.get("wait", ["0"])[0]), int(seen[0]) if seen else None,
                                           int(after[0]) if after else None))
            elif path == "/api/v1/conversations":
                self.json(200, api.conversations())
            elif path == "/api/v1/transcript":
                self.json(200, api.transcript(parse_qs(url.query).get("records") == ["1"]))
            elif path.endswith("/timeline") or path.endswith("/page"):
                self.json(200, api.timeline(parse_qs(url.query)))
            else:
                self.json(404, {"error": "not_found", "detail": "not served by this check"})

        def do_POST(self) -> None:  # noqa: N802
            path = unquote(urlsplit(self.path).path)
            raw = self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"null"
            body: object = json.loads(raw)
            if self.headers.get("Authorization") != f"Bearer {TOKEN}":
                self.json(401, {"error": "unauthorized", "detail": "unknown token"})
            elif path == "/api/v1/post-proposals/commit" and isinstance(body, dict):
                api.commits.append({str(key): value for key, value in body.items()})
                proposal = api.settle()
                if proposal is None or body.get("handle") != proposal["handle"]:
                    self.json(409, {"error": "proposal_used", "detail": "already settled"})
                elif body.get("text") != proposal["text"]:
                    # As the server's gate refuses it: the words confirmed are not the words proposed.
                    self.json(409, {"error": "proposal_mismatch", "detail": "the text differs"})
                else:
                    message = channel_message(99, "bot", str(proposal["text"]))
                    if api.deliver_posts:
                        with api.lock:
                            message = channel_message(40 + len(api.posted), "bot", str(proposal["text"]))
                            api.posted.append(message)
                    how = "tap" if body.get("confirmed_by") == "ui" else "voice"
                    api.log.append("commit")
                    seq = api.file_send(proposal, how, str(message["id"]))
                    self.json(200, {"parts": [message], "posted": message, "serial": proposal["serial"],
                                    "send_seq": seq})
            elif path == "/api/v1/post-proposals/cancel" and isinstance(body, dict):
                api.cancels.append({str(key): value for key, value in body.items()})
                api.settle()
                self.json(204, {})
            elif (turns := re.fullmatch(r"/api/v1/conversations/([A-Za-z0-9_-]+)/turns", path)) and \
                    isinstance(body, dict):
                status, answer = api.record(turns.group(1), {str(key): value for key, value in body.items()})
                self.json(status, answer)
            else:
                # Timing and health: accepted and dropped.
                self.json(204, {})

        def asset(self, request_path: str) -> None:
            relative = "voice.html" if request_path == "/" else request_path.lstrip("/")
            target = (web_root / relative).resolve()
            if web_root not in target.parents or not target.is_file():
                self.send_error(404)
                return
            content = target.read_bytes()
            self.send_response(200)
            self.send_header("Content-Type", mimetypes.guess_type(target.name)[0] or "application/octet-stream")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(content)))
            self.end_headers()
            self.wfile.write(content)

        def json(self, status: int, payload: Json) -> None:
            content = b"" if status == 204 else json.dumps(payload).encode()
            self.send_response(status)
            if status != 204:
                self.send_header("Content-Type", "application/json")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(content)))
            self.end_headers()
            try:
                self.wfile.write(content)
            except OSError:
                pass

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
        "--web-root",
        type=Path,
        default=Path(__file__).resolve().parents[1] / "web",
        help="the page assets to serve (default: this checkout's vibe-talk/web)",
    )
    parser.add_argument(
        "--screenshots",
        type=Path,
        help="keep a PNG of the card on the call's view, the channel and Settings at each size here "
             "(default: none kept)",
    )
    return parser.parse_args()


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


def card_gone(page: Page) -> None:
    """Wait until the settled send card is no longer drawn (see CARD_GONE_MS)."""
    page.locator("#post-confirm").wait_for(state="hidden", timeout=CARD_GONE_MS)


# Every number this check judges, read in one go from the laid-out page.
MEASURE_JS = """() => {
  const box = (id) => { const n = document.getElementById(id); return n ? n.getBoundingClientRect() : null; };
  const shown = (id) => { const n = document.getElementById(id); if (!n || n.hidden) return false;
    const s = getComputedStyle(n); const r = n.getBoundingClientRect();
    return s.display !== 'none' && s.visibility !== 'hidden' && r.width > 0 && r.height > 0; };
  const card = document.getElementById('post-confirm');
  const send = box('post-confirm-send'), label = document.getElementById('post-confirm-send-label');
  const mine = [...document.querySelectorAll('#transcript > li.mine')].pop();
  return {
    innerHeight: window.innerHeight, innerWidth: window.innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
    position: getComputedStyle(card).position,
    parent: card.parentElement ? card.parentElement.id : '',
    previous: card.previousElementSibling ? card.previousElementSibling.id : '',
    next: card.nextElementSibling ? card.nextElementSibling.id : '',
    card: box('post-confirm'), send, where: box('post-confirm-where'),
    label: label.getBoundingClientRect(),
    labelCut: label.scrollWidth > label.clientWidth + 1,
    composer: box(card.parentElement && card.parentElement.id === 'pane-discord' ? 'channel-composer' : 'voice-composer'),
    pill: shown('status-line') ? box('status-line') : null,
    tools: shown('scroll-tools') ? box('scroll-tools') : null,
    jump: shown('jump-newest') ? box('jump-newest') : null,
    mine: mine ? mine.getBoundingClientRect() : null,
    boxSendShown: shown('send-text'),
    sendBackground: getComputedStyle(document.getElementById('post-confirm-send')).backgroundColor,
    boxSendBackground: getComputedStyle(document.getElementById('send-text')).backgroundColor,
    sendDisabled: document.getElementById('post-confirm-send').disabled,
    cancelDisabled: document.getElementById('post-confirm-cancel').disabled,
    folded: card.hasAttribute('data-folded'),
  };
}"""

TO_NEWEST_JS = "() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }"


def rect(value: object, key: str) -> float:
    check(isinstance(value, dict), f"no box measured for {key}")
    assert isinstance(value, dict)
    number = value[key]
    check(isinstance(number, (int, float)), f"{key} is not a number: {number!r}")
    assert isinstance(number, (int, float))
    return float(number)


def overlaps(a: object, b: object) -> bool:
    if not isinstance(a, dict) or not isinstance(b, dict):
        return False
    return (min(rect(a, "right"), rect(b, "right")) - max(rect(a, "left"), rect(b, "left")) > 0.5 and
            min(rect(a, "bottom"), rect(b, "bottom")) - max(rect(a, "top"), rect(b, "top")) > 0.5)


def inside(value: object, height: float, width: float) -> bool:
    return (rect(value, "top") >= -0.5 and rect(value, "bottom") <= height + 0.5 and
            rect(value, "left") >= -0.5 and rect(value, "right") <= width + 0.5)


def measure(page: Page) -> dict[str, object]:
    measured = page.evaluate(MEASURE_JS)
    check(isinstance(measured, dict), "the page measured nothing")
    assert isinstance(measured, dict)
    return {str(key): value for key, value in measured.items()}


def open_call(page: Page, url: str, voice: FakeVoice, api: FakeApi, rows: int, banner: bool) -> None:
    """Sign in, open a typed call with `rows` turns in it, and leave the banner up or dismissed."""
    api.reset()
    voice.routes.clear()
    # A fresh page each run: nothing kept on the device from the run before.
    page.goto(url, wait_until="load")
    page.evaluate("() => { localStorage.clear(); sessionStorage.clear(); }")
    page.goto(url, wait_until="load")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    page.fill("#compose-text", "question 0")
    page.click("#send-text")
    until(page, lambda: len(voice.routes) == 1, "the first send to open a conversation")
    voice.send({"type": "session_started", "greeting": False})
    until(page, lambda: api.watches >= 1, "the call to start watching for proposals")
    for turn in range(1, rows):
        role = "assistant" if turn % 2 else "user"
        text = (f"Answer {turn}: the nightly ran green on every target." if role == "assistant"
                else f"question {turn}")
        voice.send({"type": "transcript", "role": role, "turn": turn, "text": text})
        voice.send({"type": "turn_complete", "turn": turn})
    if rows > 1:
        page.wait_for_function(f"() => document.querySelectorAll('#transcript > li').length >= {rows}",
                               timeout=10_000)
    if banner:
        page.evaluate("() => { const b = document.getElementById('connection-banner'); b.hidden = false; }")
    else:
        page.evaluate("() => { document.getElementById('connection-banner').hidden = true; }")
    page.wait_for_timeout(100)


def propose(page: Page, api: FakeApi, serial: int, text: str) -> None:
    """The voice agent proposes `text`, which the server holds tagged (`#303 voice-post-prefix`)."""
    api.propose({"serial": serial, "handle": f"hAnDlE-browser-check-{serial:04d}", "channel_id": CHANNEL["id"],
                 "channel_name": "lead team", "text": POST_PREFIX + text, "prefix": POST_PREFIX,
                 "reply_to": None, "expires_in_ms": 120_000})
    page.locator("#post-confirm").wait_for(state="visible", timeout=10_000)


# Every status line the card can show, written in turn, and where the buttons are after each. A
# spoken yes settling is also driven through the page's own `renderPostExpiry`, so the line it
# writes is the one measured.
STATUS_JS = """() => {
  const line = document.getElementById('post-confirm-expiry');
  const cancel = document.getElementById('post-confirm-cancel');
  const send = document.getElementById('post-confirm-send');
  const read = (said) => ({ said, line: line.getBoundingClientRect().height,
    cut: line.scrollWidth > line.clientWidth + 1,
    cancel: cancel.getBoundingClientRect().top, send: send.getBoundingClientRect().top });
  const out = [];
  for (const value of Object.values(POST_CARD_STATUS)) {
    line.textContent = typeof value === 'function' ? value(118) : value;
    out.push(read(line.textContent));
  }
  spokenYes.settling = { handle: postWatch.shown.handle, proposal: postWatch.shown, timer: null };
  renderPostExpiry();
  out.push(read(line.textContent));
  spokenYes.settling = null;
  renderPostExpiry();
  out.push(read(line.textContent));
  return out;
}"""


def status_steady(page: Page, where: str) -> None:
    """Point 4's second half: the buttons on one row, unmoved by any status line, none of them cut."""
    readings = page.evaluate(STATUS_JS)
    check(isinstance(readings, list) and len(readings) > 2, f"{where}: no status lines measured")
    assert isinstance(readings, list)
    first = readings[0]
    assert isinstance(first, dict)
    for reading in readings:
        assert isinstance(reading, dict)
        said = reading["said"]
        check(abs(rect(reading, "cancel") - rect(first, "cancel")) <= 0.5,
              f"{where}: writing {said!r} moved Don't send from {rect(first, 'cancel'):.1f} to {rect(reading, 'cancel'):.1f}")
        check(abs(rect(reading, "send") - rect(reading, "cancel")) <= 0.5,
              f"{where}: Send is not on Don't send's row with {said!r} up")
        # Its own height too: scroll anchoring can hide a line that wrapped (the list moves, not
        # the buttons), and a wrapped line is still one the next write can unwrap under a thumb.
        check(abs(rect(reading, "line") - rect(first, "line")) <= 0.5,
              f"{where}: the status line is {rect(reading, 'line'):.1f}px tall with {said!r}, "
              f"{rect(first, 'line'):.1f}px with {first['said']!r}")
        check(reading["cut"] is False, f"{where}: the status line {said!r} is cut on screen")


def reach_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int],
               rows: int, banner: bool, draft: str) -> float:
    """One run of points 1 to 5 on the call's view. Answers Send's centre as a share of the screen."""
    width, height = size
    where = f"{width}x{height}, {rows} row(s), banner {'up' if banner else 'gone'}, " \
            f"{'long' if draft is LONG_DRAFT else 'short'} draft"
    open_call(page, url, voice, api, rows, banner)
    propose(page, api, 7, draft)
    page.wait_for_timeout(HOLD_MS + 200)
    page.evaluate(TO_NEWEST_JS)
    page.wait_for_timeout(150)
    m = measure(page)

    # 1. In flow, the next thing after the transcript, before the box.
    check(m["position"] == "static", f"{where}: the card is positioned ({m['position']}), not in the list")
    check(m["parent"] == "pane-voice" and m["previous"] == "transcript" and m["next"] == "voice-composer",
          f"{where}: the card is not between the transcript and the box: {m['previous']} < card < {m['next']}")

    # 2. Reach, size, and clear of everything floating over the foot of the list.
    inner_height = float(str(m["innerHeight"]))
    inner_width = float(str(m["innerWidth"]))
    centre = (rect(m["send"], "top") + rect(m["send"], "bottom")) / 2 / inner_height
    check(centre >= REACH, f"{where}: Send's centre is at {centre:.2f} of the screen, above {REACH}")
    check(rect(m["send"], "height") >= 43.5, f"{where}: Send is {rect(m['send'], 'height'):.0f}px tall")
    check(rect(m["send"], "bottom") <= rect(m["composer"], "top") + 0.5,
          f"{where}: Send ends below the message box's top")
    for floating in ("pill", "tools", "jump"):
        check(not overlaps(m["send"], m[floating]), f"{where}: the {floating} is drawn over Send")
    check(float(str(m["scrollWidth"])) <= inner_width + 0.5, f"{where}: the page is wider than the screen")

    # 3. Send on screen means where it goes, and all of Send's label, on screen too.
    if inside(m["send"], inner_height, inner_width):
        check(inside(m["where"], inner_height, inner_width), f"{where}: Send is on screen and where it goes is not")
        check(inside(m["label"], inner_height, inner_width) and m["labelCut"] is False,
              f"{where}: Send's label is cut or off screen")
    if draft is LONG_DRAFT:
        check(m["folded"] is True, f"{where}: a long draft is not folded")
        page.click("#post-confirm-more")
        page.wait_for_timeout(150)
        opened = measure(page)
        check(opened["folded"] is False, f"{where}: More did not open the draft")
        if inside(opened["send"], inner_height, inner_width):
            check(inside(opened["where"], inner_height, inner_width),
                  f"{where}: opening the draft pushed where it goes off screen")
        page.click("#post-confirm-more")
        page.wait_for_timeout(100)

    # 4. The box's own Send: not drawn while the box is empty, and unlike the card's once typed.
    check(m["boxSendShown"] is False, f"{where}: the box's Send is drawn beside the card's")
    page.fill("#compose-text", "and one more thing")
    page.wait_for_timeout(100)
    typed = measure(page)
    check(typed["boxSendShown"] is True, f"{where}: typing in the box did not bring its Send back")
    check(typed["sendBackground"] != typed["boxSendBackground"],
          f"{where}: the box's Send looks like the card's ({typed['sendBackground']})")
    page.fill("#compose-text", "")

    # 5. Where a turn of the owner's stands.
    if m["mine"] is not None:
        for side in ("left", "right"):
            check(abs(rect(m["card"], side) - rect(m["mine"], side)) <= 1,
                  f"{where}: the card's {side} edge is {rect(m['card'], side):.1f}, a turn of yours "
                  f"{rect(m['mine'], side):.1f}")
    return centre


def placement_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int],
                   screenshots: Path | None) -> None:
    """Point 1 on the channel and Settings, and point 6, the hold, with real taps."""
    width, height = size
    where = f"{width}x{height}"
    open_call(page, url, voice, api, 30, False)
    propose(page, api, 9, SHORT_DRAFT)
    # 6. Both buttons held: real taps at once land on nothing.
    held = measure(page)
    check(held["sendDisabled"] is True and held["cancelDisabled"] is True,
          f"{where}: a button was live on the card's first frame")
    for button in ("post-confirm-cancel", "post-confirm-send"):
        b = page.locator(f"#{button}").bounding_box()
        check(b is not None, f"{where}: #{button} has no box")
        assert b is not None
        page.touchscreen.tap(b["x"] + b["width"] / 2, b["y"] + b["height"] / 2)
    page.wait_for_timeout(HOLD_MS // 2)
    check(not api.commits and not api.cancels,
          f"{where}: a tap during the hold reached the server: {api.commits or api.cancels}")
    page.wait_for_timeout(HOLD_MS)
    page.evaluate(TO_NEWEST_JS)
    status_steady(page, f"{where}, call's view")
    if screenshots:
        page.screenshot(path=str(screenshots / f"card-voice-{width}x{height}.png"), animations="disabled")

    page.click("#view-switch")
    page.wait_for_function("() => !document.getElementById('pane-discord').hidden", timeout=10_000)
    page.wait_for_function("() => document.querySelectorAll('#discord-log > li').length > 0", timeout=10_000)
    page.evaluate(TO_NEWEST_JS)
    page.wait_for_timeout(200)
    # The status pill up, as it is for six seconds after almost anything: it stands on the card,
    # never on its buttons.
    page.evaluate("() => setStatus('fetching the channel…')")
    page.wait_for_timeout(150)
    m = measure(page)
    check(m["parent"] == "pane-discord" and m["next"] == "channel-composer",
          f"{where}: on the channel the card is not just above the box: {m['parent']}, before {m['next']}")
    check(m["position"] == "static", f"{where}: on the channel the card is positioned")
    # Point 2's reach on the channel too: the card is the channel's next message there.
    channel_centre = (rect(m["send"], "top") + rect(m["send"], "bottom")) / 2 / float(str(m["innerHeight"]))
    check(channel_centre >= REACH,
          f"{where}: on the channel Send's centre is at {channel_centre:.2f} of the screen, above {REACH}")
    for floating in ("pill", "tools", "jump"):
        check(not overlaps(m["send"], m[floating]), f"{where}: on the channel the {floating} is drawn over Send")
    status_steady(page, f"{where}, channel")
    if screenshots:
        page.screenshot(path=str(screenshots / f"card-channel-{width}x{height}.png"), animations="disabled")

    page.click("#open-settings")
    page.wait_for_selector("#screen-settings", state="visible", timeout=10_000)
    page.wait_for_timeout(200)
    page.evaluate("() => setStatus('fetching the channel…')")
    page.wait_for_timeout(150)
    m = measure(page)
    check(not overlaps(m["send"], m["pill"]), f"{where}: on Settings the status pill is drawn over Send")
    inner_height = float(str(m["innerHeight"]))
    inner_width = float(str(m["innerWidth"]))
    check(m["parent"] == "frame-body", f"{where}: on Settings the card is in #{m['parent']}, not docked")
    check(inside(m["card"], inner_height, inner_width), f"{where}: the docked card runs off the screen")
    check(inside(m["send"], inner_height, inner_width), f"{where}: the docked card's Send is off the screen")
    if screenshots:
        page.screenshot(path=str(screenshots / f"card-settings-{width}x{height}.png"), animations="disabled")
    page.click("#post-confirm-send")
    card_gone(page)
    check([commit.get("confirmed_by") for commit in api.commits] == ["ui"],
          f"{where}: Send on the docked card did not commit once, as the UI: {api.commits}")
    page.click("#close-settings")


# The docked card, read in one go: its box, whether it scrolls as a whole, and every line under its
# words. `null` for a part not drawn.
DOCK_JS = """() => {
  const shown = (id) => { const n = document.getElementById(id); if (!n || n.hidden) return null;
    const s = getComputedStyle(n); const r = n.getBoundingClientRect();
    return s.display !== 'none' && r.width > 0 && r.height > 0 ? r : null; };
  const card = document.getElementById('post-confirm');
  return {
    innerHeight: window.innerHeight, innerWidth: window.innerWidth,
    parent: card.parentElement ? card.parentElement.id : '',
    card: card.getBoundingClientRect(), scrollHeight: card.scrollHeight, clientHeight: card.clientHeight,
    text: shown('post-confirm-text'), more: shown('post-confirm-more'), where: shown('post-confirm-where'),
    expiry: shown('post-confirm-expiry'), cancel: shown('post-confirm-cancel'), send: shown('post-confirm-send'),
    folded: card.hasAttribute('data-folded'),
  };
}"""


def dock_lines_on_screen(m: dict[str, object], where: str) -> None:
    """Every line under the docked card's words is drawn, inside the card and inside the screen."""
    inner_height = float(str(m["innerHeight"]))
    inner_width = float(str(m["innerWidth"]))
    check(m["parent"] == "frame-body", f"{where}: the card is in #{m['parent']}, not docked")
    check(float(str(m["scrollHeight"])) <= float(str(m["clientHeight"])) + 1,
          f"{where}: the docked card scrolls as a whole ({m['scrollHeight']} in {m['clientHeight']}), "
          "so what is under its words can be below its own fold")
    check(inside(m["card"], inner_height, inner_width), f"{where}: the docked card runs off the screen")
    card = m["card"]
    for part in ("where", "expiry", "cancel", "send"):
        check(m[part] is not None, f"{where}: the docked card does not draw its {part}")
        check(inside(m[part], inner_height, inner_width), f"{where}: the docked card's {part} is off the screen")
        check(rect(m[part], "top") >= rect(card, "top") - 0.5 and rect(m[part], "bottom") <= rect(card, "bottom") + 0.5,
              f"{where}: the docked card's {part} is outside the card's own box")
    check(rect(m["send"], "height") >= 43.5, f"{where}: the docked Send is {rect(m['send'], 'height'):.0f}px tall")


def dock_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int],
              screenshots: Path | None) -> None:
    """Point 3 for the docked card: never a scroll box, every line under its words on screen."""
    width, height = size
    where = f"{width}x{height}, docked on Settings"
    open_call(page, url, voice, api, 30, False)
    propose(page, api, 13, LONG_DRAFT)
    page.wait_for_timeout(HOLD_MS + 200)
    page.click("#open-settings")
    page.wait_for_selector("#screen-settings", state="visible", timeout=10_000)
    page.wait_for_timeout(200)
    folded = page.evaluate(DOCK_JS)
    check(isinstance(folded, dict), f"{where}: nothing measured")
    assert isinstance(folded, dict)
    check(folded["folded"] is True, f"{where}: a long draft is not folded")
    dock_lines_on_screen(folded, f"{where}, folded")
    if screenshots:
        page.screenshot(path=str(screenshots / f"card-dock-{width}x{height}.png"), animations="disabled")
    if folded["more"] is None:
        # A short screen: the words are two lines that scroll in place, and nothing opens them.
        check(height <= 500, f"{where}: a long draft docked on an upright phone has no More")
        return
    page.click("#post-confirm-more")
    page.wait_for_timeout(200)
    opened = page.evaluate(DOCK_JS)
    assert isinstance(opened, dict)
    check(opened["folded"] is False, f"{where}: More did not open the draft")
    dock_lines_on_screen(opened, f"{where}, after More")
    check(abs(rect(opened["send"], "top") - rect(folded["send"], "top")) <= 1,
          f"{where}: opening the words moved Send from {rect(folded['send'], 'top'):.0f} to "
          f"{rect(opened['send'], 'top'):.0f}; the card should grow upward")
    if screenshots:
        page.screenshot(path=str(screenshots / f"card-dock-more-{width}x{height}.png"), animations="disabled")
    page.click("#post-confirm-more")
    page.click("#close-settings")


FROZEN_JS = """() => {
  const rows = [...document.querySelectorAll('#transcript > li')];
  const cards = rows.filter((li) => li.classList.contains('sent-card'));
  const card = cards[cards.length - 1];
  const mine = rows.filter((li) => li.classList.contains('mine') && !li.classList.contains('sent-card')).pop();
  const box = (n) => n ? n.getBoundingClientRect() : null;
  return {
    count: cards.length,
    last: rows.length > 0 && rows[rows.length - 1] === card,
    text: card ? card.innerText.replace(/\\s+/g, ' ').trim() : '',
    card: box(card), mine: box(mine),
    seamAfterCard: card ? rows.slice(rows.indexOf(card) + 1).some((li) => li.classList.contains('seam')) : false,
    seams: rows.filter((li) => li.classList.contains('seam')).map((li) => li.innerText.replace(/\\s+/g, ' ').trim()),
  };
}"""


def frozen_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int],
                screenshots: Path | None) -> None:
    """Points 5 to 7 for the frozen card: where it stands, what reached the server, and a reload."""
    width, height = size
    where = f"{width}x{height}, frozen card"
    open_call(page, url, voice, api, 30, False)
    propose(page, api, 11, SHORT_DRAFT)
    page.wait_for_timeout(HOLD_MS + 200)
    page.evaluate(TO_NEWEST_JS)
    page.click("#post-confirm-send")
    card_gone(page)
    until(page, lambda: any(entry.startswith("claim") for entry in api.log), "the claim of the send")
    page.wait_for_timeout(300)
    frozen = page.evaluate(FROZEN_JS)
    check(isinstance(frozen, dict), f"{where}: nothing measured")
    assert isinstance(frozen, dict)
    check(frozen["count"] == 1 and frozen["last"] is True, f"{where}: the frozen card is not the newest row: {frozen}")
    check(api.log == ["commit", f"claim {api.sends_through}"],
          f"{where}: the server saw {api.log}, not the commit and then the claim of its send")
    text = str(frozen["text"])
    for said in ("Sent to lead team", "approved by tap", POST_PREFIX + SHORT_DRAFT):
        check(said in text, f"{where}: the frozen card does not say {said!r}: {text!r}")
    for side in ("left", "right"):
        check(abs(rect(frozen["card"], side) - rect(frozen["mine"], side)) <= 1,
              f"{where}: the frozen card's {side} edge is not a turn of yours'")
    if screenshots:
        page.screenshot(path=str(screenshots / f"card-frozen-{width}x{height}.png"), animations="disabled")

    # 7. A reload restores the same card from the record, read with records=1.
    page.goto(url, wait_until="load")
    page.wait_for_selector("#transcript > li.sent-card", timeout=10_000)
    page.evaluate(TO_NEWEST_JS)
    page.wait_for_timeout(300)
    restored = page.evaluate(FROZEN_JS)
    assert isinstance(restored, dict)
    check(restored["count"] == 1, f"{where}: the reload drew {restored['count']} frozen cards")
    check(restored["text"] == text, f"{where}: the restored card says {restored['text']!r}, the live one said {text!r}")
    check(restored["seamAfterCard"] is True, f"{where}: the restored card is not inside its conversation")
    seams = restored["seams"]
    check(isinstance(seams, list) and any("earlier conversation" in str(seam).lower() for seam in seams),
          f"{where}: the restored boundary does not say so: {seams}")
    for side in ("left", "right"):
        check(abs(rect(restored["card"], side) - rect(restored["mine"], side)) <= 1,
              f"{where}: the restored card's {side} edge is not a turn of yours'")


# Point 9, read in one go: the "new conversation" line above the newest call, its chip, and the newest
# row, with the scroll area they are in.
RECAP_JS = """() => {
  const box = (n) => { if (!n) return null; const r = n.getBoundingClientRect();
    return { top: r.top, bottom: r.bottom, left: r.left, right: r.right, width: r.width, height: r.height }; };
  const lines = [...document.querySelectorAll('#transcript > li.seam')].filter((li) => li.hasAttribute('data-ends'));
  const seam = lines[lines.length - 1] || null;
  const chip = seam ? seam.querySelector('.seam-recap') : null;
  const summary = seam ? seam.querySelector('.seam-summary') : null;
  const label = seam ? seam.querySelector('.seam-label') : null;
  const rows = [...document.querySelectorAll('#transcript > li')];
  let chipLines = 0;
  if (chip) {
    const style = getComputedStyle(chip);
    const inner = chip.getBoundingClientRect().height - parseFloat(style.paddingTop) - parseFloat(style.paddingBottom) -
      parseFloat(style.borderTopWidth) - parseFloat(style.borderBottomWidth);
    const line = parseFloat(style.lineHeight) || parseFloat(style.fontSize) * 1.2;
    chipLines = Math.round(inner / line);
  }
  return {
    label: label ? label.textContent : null, chip: chip ? chip.textContent : null, chipLines,
    chipBox: box(chip), summaryBox: box(summary),
    summaryOverflow: summary ? summary.scrollWidth - summary.clientWidth : 0,
    pageOverflow: document.documentElement.scrollWidth - innerWidth,
    newest: box(rows[rows.length - 1]), newestText: rows.length ? rows[rows.length - 1].innerText : '',
    area: box(document.getElementById('scroll-area')),
    width: innerWidth,
  };
}"""


def recap_case(page: Page, url: str, hosted: FakeHosted, api: FakeApi, size: tuple[int, int],
               screenshots: Path | None) -> None:
    """Point 9: a call, Hang up, and a call that is sent a 412-word recap, which its line says."""
    width, height = size
    where = f"{width}x{height}, recap line"
    api.reset()
    api.hosted = True
    api.replay = True
    hosted.routes.clear()
    hosted.received.clear()
    page.goto(url, wait_until="load")
    page.evaluate("() => { localStorage.clear(); sessionStorage.clear(); localStorage.setItem('vibe-talk.voice.resume', 'on'); }")
    page.goto(url, wait_until="load")
    page.fill("#api-token", TOKEN)
    page.click("#save-token")
    page.wait_for_selector("#view-switch", state="visible", timeout=10_000)
    page.fill("#compose-text", "question 0")
    page.click("#send-text")
    until(page, lambda: len(hosted.routes) == 1, "the first send to open a conversation")
    until(page, lambda: len(api.stored.get("conv_browser_1", [])) >= 2, "the first call's start and turn to be filed")
    for turn in range(1, 12):
        hosted.send({"type": "agent_response",
                     "agent_response_event": {"agent_response": f"Answer {turn}: the nightly ran green on every target."}})
    page.wait_for_function("() => document.querySelectorAll('#transcript > li').length >= 12", timeout=10_000)
    page.click("#hang-up")
    page.wait_for_selector("#transcript > li.seam[data-ends='conv_browser_1']", timeout=10_000)
    page.fill("#compose-text", "and the arm64 job")
    page.click("#send-text")
    until(page, lambda: len(hosted.routes) == 2, "the second send to open the next conversation")
    page.wait_for_selector("#transcript .seam-recap", timeout=10_000)
    page.wait_for_function("() => document.querySelector('#transcript > li:last-child').innerText.includes('arm64')",
                           timeout=10_000)
    page.wait_for_timeout(300)
    check(api.replays == ["conv_browser_1"], f"{where}: the next call asked for the recap of {api.replays}")
    check(any(frame.get("type") == "contextual_update" for frame in hosted.received),
          f"{where}: no recap went to the next call")
    measured = page.evaluate(RECAP_JS)
    check(isinstance(measured, dict), f"{where}: nothing measured")
    assert isinstance(measured, dict)
    check(measured["label"] == "new conversation", f"{where}: the line says {measured['label']!r}")
    check(measured["chip"] == "412-word recap sent", f"{where}: the chip says {measured['chip']!r}")
    if width >= 393:
        check(measured["chipLines"] == 1, f"{where}: the chip took {measured['chipLines']} lines")
    check(int(str(measured["summaryOverflow"])) <= 1 and int(str(measured["pageOverflow"])) <= 0,
          f"{where}: the line runs wider than the screen: {measured}")
    check(rect(measured["chipBox"], "right") <= float(str(measured["width"])) and rect(measured["chipBox"], "left") >= 0,
          f"{where}: the chip is cut off: {measured['chipBox']}")
    # The newest row, the message that opened the call, still in view under the line.
    check(rect(measured["newest"], "bottom") <= rect(measured["area"], "bottom") + 1 and
          rect(measured["newest"], "top") >= rect(measured["area"], "top") - 1,
          f"{where}: the newest row left the view: {measured['newest']} in {measured['area']}")
    if screenshots:
        page.screenshot(path=str(screenshots / f"recap-line-{width}x{height}.png"), animations="disabled")


# Point 10, read in one go: how far the channel's list runs below what it shows, the first row on
# screen and its offset, and what the jump to the newest says.
STILL_JS = """() => {
  const area = document.getElementById('scroll-area');
  const top = area.getBoundingClientRect().top;
  let first = null;
  for (const row of document.getElementById('discord-log').children) {
    if (!row.hasAttribute('data-id') || !row.getClientRects().length) continue;
    const box = row.getBoundingClientRect();
    if (box.bottom > top) { first = { id: row.getAttribute('data-id'), offset: box.top - top }; break; }
  }
  const jump = document.getElementById('jump-newest');
  return { gap: area.scrollHeight - area.scrollTop - area.clientHeight, first,
           jump: jump.hidden ? null : document.getElementById('jump-newest-label').textContent };
}"""


def still(page: Page) -> dict[str, object]:
    measured = page.evaluate(STILL_JS)
    check(isinstance(measured, dict), "the page measured nothing")
    assert isinstance(measured, dict)
    return {str(key): value for key, value in measured.items()}


def channel_still_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int]) -> None:
    """Point 10: on the channel the card comes and goes moving the list no more than a message does."""
    width, height = size
    where = f"{width}x{height}, channel"
    open_call(page, url, voice, api, 30, False)
    page.click("#view-switch")
    page.wait_for_function("() => document.querySelectorAll('#discord-log > li').length > 0", timeout=10_000)
    page.wait_for_timeout(300)

    # A reader on the newest line follows the card down, and is still on it once Send has taken the card
    # away and, a second later, the box its room.
    page.evaluate(TO_NEWEST_JS)
    page.wait_for_timeout(200)
    propose(page, api, 21, SHORT_DRAFT)
    page.wait_for_timeout(300)
    for stage in ("the card arriving", "Send", "the box taking back its room"):
        if stage == "Send":
            page.wait_for_timeout(HOLD_MS)
            page.click("#post-confirm-send")
            card_gone(page)
            page.wait_for_timeout(150)
        elif stage == "the box taking back its room":
            page.wait_for_timeout(1300)
        gap = rect(still(page), "gap")
        check(gap <= 1, f"{where}: after {stage} a reader on the newest line is {gap:.0f}px above the foot")

    # A reader up the list keeps the row they are on, within a pixel, through the same three, and is told
    # a draft waits below.
    page.evaluate("() => { document.getElementById('scroll-area').scrollTop = 150; }")
    page.wait_for_timeout(300)
    before = still(page)
    first = before["first"]
    check(isinstance(first, dict) and rect(before, "gap") > 100, f"{where}: the premise, a reader up the list: {before}")
    assert isinstance(first, dict)
    propose(page, api, 22, SHORT_DRAFT)
    page.wait_for_timeout(300)
    arrived = still(page)
    check(arrived["jump"] == "Draft to send", f"{where}: the jump says {arrived['jump']!r}, not that a draft waits")
    for stage in ("the card arriving", "Send", "the box taking back its room"):
        if stage == "Send":
            page.wait_for_timeout(HOLD_MS)
            # Not a click: Playwright would scroll the card into view first, which is a move the reader made.
            page.evaluate("() => document.getElementById('post-confirm-send').click()")
            card_gone(page)
            page.wait_for_timeout(150)
        elif stage == "the box taking back its room":
            page.wait_for_timeout(1300)
        now = still(page)["first"]
        check(isinstance(now, dict) and now["id"] == first["id"] and
              abs(rect(now, "offset") - rect(first, "offset")) <= 1,
              f"{where}: {stage} moved the row a reader up the list was on from {first} to {now}")


def walk(chromium: BrowserType, args: argparse.Namespace, url: str, api: FakeApi,
         size: tuple[int, int]) -> str:
    width, height = size
    voice = FakeVoice()
    hosted = FakeHosted()
    with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-approval-card-") as directory:
        context = chromium.launch_persistent_context(
            directory,
            headless=True,
            executable_path=args.browser_executable,
            viewport={"width": width, "height": height},
            is_mobile=True,
            has_touch=True,
            device_scale_factor=2.75,
            user_agent=(
                "Mozilla/5.0 (Linux; Android 15; Pixel 7) AppleWebKit/537.36 "
                "(KHTML, like Gecko) Chrome/151.0.0.0 Mobile Safari/537.36"
            ),
        )
        try:
            context.route_web_socket(re.compile(r"/ws\?mode=voice$"), voice.accept)
            context.route_web_socket(re.compile(r"/ws\?mode=hosted$"), hosted.accept)
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            lowest = 1.0
            for rows in (1, 30):
                for banner in (False, True):
                    for draft in (SHORT_DRAFT, LONG_DRAFT):
                        lowest = min(lowest, reach_case(page, url, voice, api, size, rows, banner, draft))
            placement_case(page, url, voice, api, size, args.screenshots)
            channel_still_case(page, url, voice, api, size)
            dock_case(page, url, voice, api, size, args.screenshots)
            frozen_case(page, url, voice, api, size, args.screenshots)
            recap_case(page, url, hosted, api, size, args.screenshots)
            listed = ""
            if size == PHONES[0]:
                listed = "; " + thread_list_case(page, url, voice, api, size, True, args.screenshots)
                listed += "; " + thread_list_place_case(page, url, voice, api, size, True)
                listed += "; " + thread_list_arrival_case(page, url, voice, api, size, True)
                listed += "; " + thread_list_away_case(page, url, voice, api, size, True)
                listed += "; " + room_back_case(page, url, voice, api, size, True)
                listed += "; " + opened_card_case(page, url, voice, api, size)
            listed += "; " + card_alone_case(page, url, voice, api, size,
                                             CARD_ALONE_CASES if size == PHONES[0] else CARD_ALONE_FEW)
            if size in (PHONES[0], PHONES[2]):
                listed += "; " + settings_trip_case(page, url, voice, api, size,
                                                    SETTINGS_TRIPS if size == PHONES[0] else SETTINGS_TRIPS_FEW)
                listed += "; " + help_key_trip_case(page, url, voice, api, size,
                                                    HELP_KEY_TRIPS if size == PHONES[0] else HELP_KEY_TRIPS_FEW)
            if size == PHONES[0]:
                listed += "; " + chips_room_case(page, url, voice, api, size)
                listed += "; " + jump_after_settings_case(page, url, voice, api, size, True, JUMP_TRIPS_PHONE)
            check(not errors, f"{width}x{height}: the page threw: {errors}")
        finally:
            context.close()
    return (f"{width}x{height}: Send's centre at >= {lowest:.3f} of the screen in every run; on the channel"
            " the card coming and going left a reader up the list on their row and one at the foot there;"
            f" the next call's line says '412-word recap sent'{listed}")


def walk_short(chromium: BrowserType, args: argparse.Namespace, url: str, api: FakeApi,
               size: tuple[int, int]) -> str:
    """The docked card alone, on a screen too short to hold it as an upright phone does."""
    width, height = size
    voice = FakeVoice()
    with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-approval-card-") as directory:
        context = chromium.launch_persistent_context(
            directory, headless=True, executable_path=args.browser_executable,
            viewport={"width": width, "height": height}, is_mobile=True, has_touch=True,
            device_scale_factor=2.75,
        )
        try:
            context.route_web_socket(re.compile(r"/ws\?mode=voice$"), voice.accept)
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            dock_case(page, url, voice, api, size, args.screenshots)
            check(not errors, f"{width}x{height}: the page threw: {errors}")
        finally:
            context.close()
    return f"{width}x{height}: the docked card's lines and buttons all on screen"


# Point 8, read in one go: the card, its Send and the reply's box and Send, in two columns.
COLUMNS_JS = """() => {
  const box = (id) => { const n = document.getElementById(id); if (!n || n.hidden) return null;
    const s = getComputedStyle(n); const r = n.getBoundingClientRect();
    return s.display !== 'none' && s.visibility !== 'hidden' && r.width > 0 && r.height > 0 ? r : null; };
  const card = document.getElementById('post-confirm');
  const compose = document.getElementById('reply-compose');
  return {
    innerHeight: window.innerHeight, innerWidth: window.innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
    twoColumns: document.getElementById('screen-main').getAttribute('data-columns') === 'two',
    parent: card.parentElement ? card.parentElement.id : '',
    next: card.nextElementSibling ? card.nextElementSibling.id : '',
    beside: card.hasAttribute('data-beside-reply'),
    replyIn: compose && compose.parentElement ? compose.parentElement.id : '',
    card: box('post-confirm'), send: box('post-confirm-send'), list: box('scroll-area'),
    side: box('side-column'), leftReply: box('column-reply'), rightReply: box('side-reply'),
    replySend: box('reply-send'), composer: box('channel-composer'),
    pill: box('status-line'), tools: box('scroll-tools'), jump: box('jump-newest'),
    sendBackground: getComputedStyle(document.getElementById('post-confirm-send')).backgroundColor,
    replySendBackground: getComputedStyle(document.getElementById('reply-send')).backgroundColor,
    replyText: document.getElementById('reply-text').value,
  };
}"""

TRANSPARENT = ("rgba(0, 0, 0, 0)", "transparent")


def columns_measure(page: Page) -> dict[str, object]:
    measured = page.evaluate(COLUMNS_JS)
    check(isinstance(measured, dict), "the page measured nothing in two columns")
    assert isinstance(measured, dict)
    return {str(key): value for key, value in measured.items()}


def press_reply(page: Page, list_id: str, message_id: str, mount: str, where: str) -> None:
    """A real click on the row's Reply, and the reply's box open at the foot of `mount`."""
    found = page.evaluate(f"""() => {{
        const b = document.querySelector("#{list_id} > li[data-id='{message_id}'] > .meta .reply-button");
        if (!b) return null;
        b.scrollIntoView({{block: 'center'}});
        const r = b.getBoundingClientRect();
        return {{x: r.left + r.width / 2, y: r.top + r.height / 2}};
    }}""")
    check(isinstance(found, dict), f"{where}: message {message_id} has no Reply")
    assert isinstance(found, dict)
    page.mouse.click(float(found["x"]), float(found["y"]))
    page.wait_for_function(f"() => document.getElementById('reply-compose').parentElement.id === '{mount}'",
                           timeout=10_000)
    page.wait_for_timeout(200)


def columns_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, screenshots: Path | None) -> str:
    """Point 8: the card and a reply open in a column, on a desk in two columns."""
    width, height = DESK
    where = f"{width}x{height}, two columns"
    api.threaded = True
    try:
        open_call(page, url, voice, api, 30, False)
        propose(page, api, 15, SHORT_DRAFT)
        page.wait_for_timeout(HOLD_MS + 200)
        page.click("#view-switch")
        # The channel opens in All, the thread's answers among Main's messages.
        page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length === 14",
                               timeout=10_000)
        page.click("#column-toggle")
        # Two columns: Main on the left, the thread's card on the right.
        page.wait_for_function("() => document.getElementById('side-column').getClientRects().length > 0"
                               " && document.querySelectorAll('#discord-log > li[data-id]').length === 12"
                               " && document.querySelectorAll('#side-cards > li').length === 1",
                               timeout=10_000)
        page.mouse.move(width / 2, 5)
        page.evaluate(TO_NEWEST_JS)
        page.wait_for_timeout(200)
        alone = columns_measure(page)
        check(alone["twoColumns"] is True, f"{where}: the layout button did not give two columns")
        check(alone["parent"] == "pane-discord" and alone["next"] == "channel-composer",
              f"{where}: the card is not the left list's next message: {alone['parent']}, before {alone['next']}")
        check(alone["beside"] is False and alone["sendBackground"] not in TRANSPARENT,
              f"{where}: with no reply open the card's Send is not the filled one ({alone['sendBackground']})")

        # The left column: the reply's box at its foot, under the very list the card ends.
        press_reply(page, "discord-log", str(channel_message(12, "codex", "")["id"]), "column-reply", where)
        page.fill("#reply-text", "still writing")
        page.evaluate(TO_NEWEST_JS)
        page.wait_for_timeout(200)
        m = columns_measure(page)
        inner_height = float(str(m["innerHeight"]))
        inner_width = float(str(m["innerWidth"]))
        check(m["replyIn"] == "column-reply" and m["leftReply"] is not None,
              f"{where}: the reply did not open at the foot of the left column ({m['replyIn']})")
        check(m["parent"] == "pane-discord" and m["next"] == "channel-composer",
              f"{where}: the reply moved the card: {m['parent']}, before {m['next']}")
        check(m["card"] is not None and m["send"] is not None, f"{where}: the reply hid the draft")
        for part in ("card", "send"):
            check(not overlaps(m[part], m["leftReply"]), f"{where}: the card's {part} crosses the reply's box")
            check(rect(m[part], "bottom") <= rect(m["leftReply"], "top") + 0.5,
                  f"{where}: the card's {part} ends at {rect(m[part], 'bottom'):.1f}, below the reply box's "
                  f"top at {rect(m['leftReply'], 'top'):.1f}")
            check(rect(m[part], "right") <= rect(m["side"], "left") + 0.5,
                  f"{where}: the card's {part} crosses the divider into the right column")
        check(rect(m["send"], "bottom") <= rect(m["composer"], "top") + 0.5,
              f"{where}: the card's Send ends below the channel box's top")
        check(inside(m["send"], inner_height, inner_width) and inside(m["replySend"], inner_height, inner_width),
              f"{where}: a Send is off the screen")
        check(rect(m["replySend"], "top") >= rect(m["leftReply"], "top") - 0.5,
              f"{where}: the reply's Send is not inside its box")
        for floating in ("pill", "tools", "jump"):
            check(not overlaps(m["send"], m[floating]), f"{where}: the {floating} is drawn over the card's Send")
            check(not overlaps(m["replySend"], m[floating]), f"{where}: the {floating} is drawn over the reply's Send")
        check(m["beside"] is True and m["sendBackground"] in TRANSPARENT,
              f"{where}: beside the reply's box the card's Send is filled ({m['sendBackground']})")
        check(m["replySendBackground"] not in TRANSPARENT and m["replySendBackground"] != m["sendBackground"],
              f"{where}: the card's Send looks like the reply's ({m['replySendBackground']})")
        check(float(str(m["scrollWidth"])) <= inner_width + 0.5, f"{where}: the page is wider than the window")
        if screenshots:
            page.screenshot(path=str(screenshots / f"card-beside-reply-{width}x{height}.png"), animations="disabled")

        # Send on the card commits the draft once, and the reply stays as it was.
        page.click("#post-confirm-send")
        card_gone(page)
        check([commit.get("confirmed_by") for commit in api.commits] == ["ui"],
              f"{where}: Send on the card beside a reply did not commit the draft once: {api.commits}")
        after = columns_measure(page)
        check(after["replyIn"] == "column-reply" and after["replyText"] == "still writing",
              f"{where}: sending the draft took the reply away ({after['replyIn']}, {after['replyText']!r})")
        page.click("#reply-cancel")
        page.wait_for_function("() => document.getElementById('column-reply').hidden", timeout=5000)

        # The right column: a reply open in the thread there outlines the card's Send as well.
        propose(page, api, 16, SHORT_DRAFT)
        page.wait_for_timeout(HOLD_MS + 200)
        page.click(f"#side-cards > li[data-id='{THREAD_ID}'] .side-card-open")
        page.wait_for_function("() => document.querySelectorAll('#side-log > li[data-id]').length === 3",
                               timeout=10_000)
        press_reply(page, "side-log", str(channel_message(14, "codex", "")["id"]), "side-reply", where)
        page.evaluate(TO_NEWEST_JS)
        page.wait_for_timeout(200)
        right = columns_measure(page)
        check(right["beside"] is True and right["sendBackground"] in TRANSPARENT,
              f"{where}: beside a reply open in the right column the card's Send is filled")
        check(right["card"] is not None and not overlaps(right["card"], right["rightReply"]),
              f"{where}: the card crosses the right column's reply box")
        if screenshots:
            page.screenshot(path=str(screenshots / f"card-beside-side-reply-{width}x{height}.png"),
                            animations="disabled")
        page.click("#reply-cancel")
        page.wait_for_function("() => document.getElementById('side-reply').hidden", timeout=5000)
        cleared = columns_measure(page)
        check(cleared["beside"] is False and cleared["sendBackground"] not in TRANSPARENT,
              f"{where}: the card's Send stayed outlined after the reply went")
    finally:
        api.threaded = False
    return f"{where}: the card above a reply open in the left column, its Send outlined beside either"


# Point 11, read in one go: the card, the thread list's column and cards, the dock and what floats.
THREAD_LIST_JS = """() => {
  const box = (id) => { const n = document.getElementById(id); if (!n || n.hidden) return null;
    const s = getComputedStyle(n); const r = n.getBoundingClientRect();
    return s.display !== 'none' && s.visibility !== 'hidden' && r.width > 0 && r.height > 0 ? r : null; };
  const card = document.getElementById('post-confirm');
  const side = document.getElementById('side-scroll');
  const main = document.getElementById('scroll-area');
  const top = side.getBoundingClientRect().top;
  const rows = [...document.querySelectorAll('#side-cards > li[data-id]')].filter((row) => row.getClientRects().length);
  const firstRow = rows.find((row) => row.getBoundingClientRect().bottom > top);
  const last = rows.length ? rows[rows.length - 1] : null;
  return {
    innerHeight: window.innerHeight, innerWidth: window.innerWidth,
    scrollWidth: document.documentElement.scrollWidth,
    layout: document.getElementById('screen-main').getAttribute('data-columns'),
    parent: card.parentElement ? card.parentElement.id : '',
    next: card.nextElementSibling ? card.nextElementSibling.id : '',
    place: card.getAttribute('data-place'), screen: card.getAttribute('data-screen'),
    card: box('post-confirm'), send: box('post-confirm-send'), cancel: box('post-confirm-cancel'),
    where: box('post-confirm-where'), side: box('side-scroll'), dock: box('dock'),
    pill: box('status-line'), jump: box('jump-newest'),
    sideGap: side.scrollHeight - side.scrollTop - side.clientHeight,
    sideRuns: side.scrollHeight - side.clientHeight,
    mainGap: main.scrollHeight - main.scrollTop - main.clientHeight,
    lastCard: last ? last.getBoundingClientRect() : null,
    first: firstRow ? { id: firstRow.getAttribute('data-id'), offset: firstRow.getBoundingClientRect().top - top } : null,
    status: document.getElementById('status').textContent,
  };
}"""


def thread_list_measure(page: Page) -> dict[str, object]:
    measured = page.evaluate(THREAD_LIST_JS)
    check(isinstance(measured, dict), "the page measured nothing over the thread list")
    assert isinstance(measured, dict)
    return {str(key): value for key, value in measured.items()}


def view_key(page: Page, mod: str, digit: int, focus: str | None, layout: str | None) -> None:
    """A view key (`#281 view-keys`), pressed with the focus on `focus`, then the layout `layout` (none
    for one column) waited for, and a moment for anything that would move after it."""
    if focus:
        page.focus(focus)
    page.keyboard.press(f"{mod}+{digit}")
    want = "null" if layout is None else f"'{layout}'"
    page.wait_for_function(f"() => document.getElementById('screen-main').getAttribute('data-columns') === {want}",
                           timeout=10_000)
    page.wait_for_timeout(400)


def docked_under_list(m: dict[str, object], where: str, phone: bool) -> None:
    """The card docked under the thread list: on screen whole, under the cards and over the control bar,
    the newest thread's card above it with the list at its foot, and Send within a thumb's reach."""
    inner_height = float(str(m["innerHeight"]))
    inner_width = float(str(m["innerWidth"]))
    check(m["layout"] == "list", f"{where}: the thread list is not up ({m['layout']})")
    check(m["parent"] == "frame-body" and m["place"] == "dock" and m["screen"] == "main",
          f"{where}: the card is not docked under the thread list: #{m['parent']}, {m['place']}, {m['screen']}")
    for part in ("card", "where", "cancel", "send"):
        check(m[part] is not None and inside(m[part], inner_height, inner_width),
              f"{where}: the card's {part} is not on screen")
    check(rect(m["card"], "top") >= rect(m["side"], "bottom") - 0.5,
          f"{where}: the card at {rect(m['card'], 'top'):.1f} covers the cards' column, which ends at "
          f"{rect(m['side'], 'bottom'):.1f}")
    check(rect(m["card"], "bottom") <= rect(m["dock"], "top") + 0.5,
          f"{where}: the card runs under the control bar ({rect(m['card'], 'bottom'):.1f} > {rect(m['dock'], 'top'):.1f})")
    check(float(str(m["sideRuns"])) > 100, f"{where}: the cards do not run past the column, so this proves nothing")
    check(float(str(m["sideGap"])) <= 1, f"{where}: the thread list is {float(str(m['sideGap'])):.0f}px above its foot")
    check(rect(m["lastCard"], "bottom") <= rect(m["card"], "top") + 0.5,
          f"{where}: the newest thread's card is under the send card")
    check(rect(m["send"], "height") >= 43.5, f"{where}: Send is {rect(m['send'], 'height'):.0f}px tall")
    if phone:
        centre = (rect(m["send"], "top") + rect(m["send"], "bottom")) / 2 / inner_height
        check(centre >= REACH, f"{where}: Send's centre is at {centre:.3f} of the screen, above the thumb's {REACH}")
    else:
        # As wide as the column of cards it stands under, not the window.
        mid = (rect(m["card"], "left") + rect(m["card"], "right")) / 2
        cards_mid = (rect(m["lastCard"], "left") + rect(m["lastCard"], "right")) / 2
        check(rect(m["card"], "width") <= rect(m["lastCard"], "width") + 1 and abs(mid - cards_mid) <= 2,
              f"{where}: the card ({rect(m['card'], 'left'):.0f} to {rect(m['card'], 'right'):.0f}) is not the "
              f"column of cards ({rect(m['lastCard'], 'left'):.0f} to {rect(m['lastCard'], 'right'):.0f})")
    for floating in ("pill", "jump"):
        check(not overlaps(m["send"], m[floating]), f"{where}: the {floating} is drawn over the card's Send")
    check(float(str(m["scrollWidth"])) <= inner_width + 0.5, f"{where}: the page is wider than the window")


def thread_list_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int], phone: bool,
                     screenshots: Path | None) -> str:
    """Point 11: the card under the thread list, and the view keys pressed while it is up."""
    width, height = size
    where = f"{width}x{height}, thread list"
    mod = "Meta" if phone else "Control"
    api.threaded = True
    api.older_threads = 14
    try:
        open_call(page, url, voice, api, 30, False)
        propose(page, api, 31, LONG_DRAFT)
        page.wait_for_timeout(HOLD_MS + 200)
        page.click("#view-switch")
        page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                               timeout=10_000)
        page.wait_for_timeout(300)

        # Cmd-3 (Ctrl+3 at the desk) with the keys on the card's own Send: the thread list, its cards at
        # their foot, the card docked under them; nothing posted, nothing withdrawn.
        view_key(page, mod, 3, "#post-confirm-send", "list")
        page.wait_for_function("() => document.querySelectorAll('#side-cards > li[data-id]').length > 10",
                               timeout=10_000)
        page.wait_for_timeout(400)
        docked = thread_list_measure(page)
        docked_under_list(docked, f"{where}, {mod}+3 on the card's Send", phone)
        check(not api.commits and not api.cancels and api.proposal is not None,
              f"{where}: {mod}+3 on the card posted or withdrew it ({api.commits}, {api.cancels})")
        page.wait_for_timeout(1000)
        later = thread_list_measure(page)
        check(float(str(later["sideGap"])) <= 1 and later["first"] == docked["first"],
              f"{where}: the thread list moved after {mod}+3: {docked['first']} to {later['first']}")
        if screenshots:
            page.screenshot(path=str(screenshots / f"card-thread-list-{width}x{height}.png"), animations="disabled")
        # More opens the long draft upward, and the cards stay at their foot above it; Less folds it again.
        page.click("#post-confirm-more")
        page.wait_for_timeout(400)
        opened = thread_list_measure(page)
        check(rect(opened["card"], "height") > rect(docked["card"], "height") + 20,
              f"{where}: More did not open the docked draft")
        docked_under_list(opened, f"{where}, after More", phone)
        page.click("#post-confirm-more")
        page.wait_for_timeout(400)
        docked_under_list(thread_list_measure(page), f"{where}, after Less", phone)

        # Cmd-4 with the keys on Don't send.
        if phone:
            view_key(page, mod, 4, "#post-confirm-cancel", "list")
            narrow = thread_list_measure(page)
            check("wider window" in str(narrow["status"]), f"{where}: {mod}+4 on a phone said {narrow['status']!r}")
            docked_under_list(narrow, f"{where}, {mod}+4 on a phone", phone)
        else:
            view_key(page, mod, 4, "#post-confirm-cancel", "two")
            two = thread_list_measure(page)
            inner_height = float(str(two["innerHeight"]))
            inner_width = float(str(two["innerWidth"]))
            check(two["parent"] == "pane-discord" and two["next"] == "channel-composer",
                  f"{where}: {mod}+4 did not put the card back at the foot of Main: #{two['parent']}")
            check(float(str(two["mainGap"])) <= 1, f"{where}: Main is {float(str(two['mainGap'])):.0f}px above the card")
            for part in ("card", "send"):
                check(two[part] is not None and inside(two[part], inner_height, inner_width),
                      f"{where}: after {mod}+4 the card's {part} is not on screen")
            check(rect(two["send"], "right") <= rect(two["side"], "left") + 0.5,
                  f"{where}: after {mod}+4 the card crosses into the right column")
            check(float(str(two["sideGap"])) <= 1, f"{where}: {mod}+4 took the cards off their foot")
            view_key(page, mod, 3, "#post-confirm-send", "list")
            docked_under_list(thread_list_measure(page), f"{where}, {mod}+3 from two columns", phone)
        check(not api.commits and not api.cancels and api.proposal is not None,
              f"{where}: {mod}+4 posted or withdrew the draft ({api.commits}, {api.cancels})")

        # A draft arriving under the list, the reader at the foot of the cards and Main behind them on its
        # newest line: both stay at their foot, and the status pill stands on the card. Then a reader up
        # the cards keeps the very card through another.
        api.settle()
        card_gone(page)
        page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
        page.wait_for_timeout(300)
        propose(page, api, 32, LONG_DRAFT)
        page.wait_for_timeout(400)
        arrived = thread_list_measure(page)
        docked_under_list(arrived, f"{where}, a draft arriving at the foot of the cards", phone)
        check(float(str(arrived["mainGap"])) <= 1,
              f"{where}: a draft arriving left Main behind the list {float(str(arrived['mainGap'])):.0f}px above its foot")
        check(arrived["pill"] is not None and rect(arrived["pill"], "bottom") <= rect(arrived["card"], "top") + 0.5,
              f"{where}: the status pill does not stand on the docked card")
        api.settle()
        card_gone(page)
        page.evaluate("() => { const s = document.getElementById('side-scroll'); s.scrollTop = s.scrollHeight - s.clientHeight - 160; }")
        page.wait_for_timeout(300)
        before = thread_list_measure(page)["first"]
        check(isinstance(before, dict), f"{where}: no card on screen up the list")
        propose(page, api, 33, SHORT_DRAFT)
        page.wait_for_timeout(400)
        up = thread_list_measure(page)
        now = up["first"]
        check(isinstance(now, dict) and isinstance(before, dict) and now["id"] == before["id"] and
              abs(rect(now, "offset") - rect(before, "offset")) <= 1,
              f"{where}: a draft arriving moved the card a reader up the list was on from {before} to {now}")

        # Cmd-1 with Main at its newest behind the list: the card at its foot, on screen.
        page.evaluate("() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; }")
        page.wait_for_timeout(200)
        view_key(page, mod, 1, "#post-confirm-send", None)
        main = thread_list_measure(page)
        check(main["parent"] == "pane-discord" and float(str(main["mainGap"])) <= 1 and main["send"] is not None and
              inside(main["send"], float(str(main["innerHeight"])), float(str(main["innerWidth"]))),
              f"{where}: {mod}+1 did not bring the card back on screen at the foot of Main: {main['parent']}, "
              f"{main['mainGap']}")
        check(not api.commits and not api.cancels, f"{where}: a view key posted or withdrew the draft")
    finally:
        api.threaded = False
        api.older_threads = 0
    return (f"{where}: {mod}+3 and {mod}+4 on the card's buttons posted nothing; docked under the cards, at their"
            " foot, Send on screen; a reader at the foot or up the cards kept as a draft arrived")


PICKED_VIEW_JS = "() => document.getElementById('thread-select').value"
COLUMNS_ATTR_JS = "() => document.getElementById('screen-main').getAttribute('data-columns')"
UP_THE_LIST_JS = ("(up) => { const a = document.getElementById('scroll-area');"
                  " a.scrollTop = a.scrollHeight - a.clientHeight - up; }")


def same_row(was: object, now: object) -> bool:
    """The same first row on screen, within a pixel of where it stood."""
    return (isinstance(was, dict) and isinstance(now, dict) and now["id"] == was["id"] and
            abs(rect(now, "offset") - rect(was, "offset")) <= 1)


def all_on_row(page: Page, where: str, mod: str, ups: tuple[int, ...]) -> None:
    """A reader `up` px up All, Cmd+3 and then Esc (the list chosen from All) or Cmd+2: on the same row."""
    for up in ups:
        for key in ("Escape", "2"):
            how = "Esc" if key == "Escape" else f"{mod}+2"
            if page.evaluate(COLUMNS_ATTR_JS) or page.evaluate(PICKED_VIEW_JS) != "flat":
                view_key(page, mod, 2, None, None)
            page.evaluate(UP_THE_LIST_JS, up)
            page.wait_for_timeout(300)
            before = still(page)
            first = before["first"]
            check(isinstance(first, dict) and rect(before, "gap") > 30,
                  f"{where}: the premise, a reader {up}px up All: {before}")
            view_key(page, mod, 3, None, "list")
            leave_thread_list(page, mod, key)
            now = still(page)["first"]
            check(page.evaluate(PICKED_VIEW_JS) == "flat" and same_row(first, now),
                  f"{where}: {mod}+3 and {how} moved the row a reader {up}px up All was on from {first} to {now}")


def leave_thread_list(page: Page, mod: str, key: str) -> None:
    """Out of the thread list by Esc, or by a view key, and a moment for anything that would move after it."""
    if key == "Escape":
        page.keyboard.press("Escape")
        page.wait_for_function("() => document.getElementById('screen-main').getAttribute('data-columns') === null",
                               timeout=10_000)
        page.wait_for_timeout(400)
    else:
        view_key(page, mod, int(key), None, "two" if key == "4" else None)


def thread_list_place_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int],
                           phone: bool) -> str:
    """Point 12: the card going to the dock under the thread list and back moves the left column's reader
    no more than the list coming and going does."""
    width, height = size
    where = f"{width}x{height}, thread list and back"
    mod = "Meta" if phone else "Control"
    keys = ("Escape", "1") if phone else ("Escape", "1", "4")
    api.threaded = True
    api.older_threads = 14
    try:
        open_call(page, url, voice, api, 30, False)
        propose(page, api, 41, LONG_DRAFT)
        page.wait_for_timeout(HOLD_MS + 200)
        page.click("#view-switch")
        page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                               timeout=10_000)
        page.wait_for_timeout(300)

        # A reader at All's foot: Cmd+3, then Esc, the list having been chosen from All, or Cmd+2, finds All
        # at its foot again with the card's Send on screen.
        view_key(page, mod, 2, None, None)
        page.evaluate(TO_NEWEST_JS)
        page.wait_for_timeout(300)
        for key in ("Escape", "2"):
            how = "Esc" if key == "Escape" else f"{mod}+2"
            view_key(page, mod, 3, None, "list")
            leave_thread_list(page, mod, key)
            back = thread_list_measure(page)
            check(page.evaluate(PICKED_VIEW_JS) == "flat" and back["parent"] == "pane-discord",
                  f"{where}: {how} from the thread list did not show All with the card: "
                  f"{page.evaluate(PICKED_VIEW_JS)}, #{back['parent']}")
            check(float(str(back["mainGap"])) <= 1,
                  f"{where}: {how} left a reader who was at All's foot {float(str(back['mainGap'])):.0f}px above it")
            check(back["send"] is not None and
                  inside(back["send"], float(str(back["innerHeight"])), float(str(back["innerWidth"]))),
                  f"{where}: after {how} the card's Send is off screen")

        # A reader up All: Cmd+3, then Esc or Cmd+2, finds them on the same row. All is Main behind the
        # list, and Main lacks All's replies, so their place was clamped at Main's foot, and the way back
        # took Main's reader for one on its newest line and All to its foot.
        all_on_row(page, where, mod, (100, 225))

        # A reader up Main, the card in view or just below it: on their row behind the cards and on it again
        # by Cmd+1, Esc and, at the desk, Cmd+4. Nearer the foot than the card is taller in the list than
        # docked (150px on the phone, 118px at the desk), Main behind the cards is too short to keep them
        # where they were, and only the way back is judged.
        view_key(page, mod, 1, None, None)
        for up in (100, 200, 380):
            for key in keys:
                how = "Esc" if key == "Escape" else f"{mod}+{key}"
                if page.evaluate("() => document.getElementById('screen-main').getAttribute('data-columns')"):
                    view_key(page, mod, 1, None, None)
                page.evaluate("(up) => { const a = document.getElementById('scroll-area');"
                              " a.scrollTop = a.scrollHeight - a.clientHeight - up; }", up)
                page.wait_for_timeout(300)
                before = still(page)
                first = before["first"]
                check(isinstance(first, dict) and rect(before, "gap") > 30,
                      f"{where}: the premise, a reader {up}px up Main: {before}")
                assert isinstance(first, dict)
                view_key(page, mod, 3, None, "list")
                if up >= 200:
                    behind = still(page)["first"]
                    check(isinstance(behind, dict) and behind["id"] == first["id"] and
                          abs(rect(behind, "offset") - rect(first, "offset")) <= 1,
                          f"{where}: {mod}+3 moved the row a reader {up}px up Main was on from {first} to {behind},"
                          " behind the cards")
                leave_thread_list(page, mod, key)
                now = still(page)["first"]
                check(isinstance(now, dict) and now["id"] == first["id"] and
                      abs(rect(now, "offset") - rect(first, "offset")) <= 1,
                      f"{where}: {mod}+3 and {how} moved the row a reader {up}px up Main was on from {first} to {now}")
        check(not api.commits and not api.cancels and api.proposal is not None,
              f"{where}: a view key posted or withdrew the draft ({api.commits}, {api.cancels})")
    finally:
        api.threaded = False
        api.older_threads = 0
    return (f"{where}: a reader at All's foot at it again by Esc and {mod}+2, Send on screen; one 100 or 225px up"
            f" All on their row by both; one 100 to 380px up Main on their row by"
            f" {', '.join('Esc' if key == 'Escape' else f'{mod}+{key}' for key in keys)}")


def thread_list_arrival_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int],
                             phone: bool) -> str:
    """Point 13: with no card up when the thread list comes, a draft arriving under it moves Main's reader no
    more than one arriving on the channel does, and the way back to All finds a reader up All on their row."""
    width, height = size
    where = f"{width}x{height}, a draft arriving under the thread list"
    mod = "Meta" if phone else "Control"
    api.threaded = True
    api.older_threads = 14
    try:
        open_call(page, url, voice, api, 30, False)
        page.click("#view-switch")
        page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                               timeout=10_000)
        page.wait_for_timeout(300)
        view_key(page, mod, 1, None, None)

        # A reader 40 or 100px up Main, Cmd+3, and a draft arrives: on the same row behind the cards, and by
        # Cmd+1 on it again with the jump offered. The card docks, and the box at Main's foot gives up its
        # room in the same layout: Main's dock-growth keeper took them for readers at the foot.
        for serial, up, draft in ((51, 40, LONG_DRAFT), (52, 100, SHORT_DRAFT)):
            words = "long" if draft is LONG_DRAFT else "short"
            if page.evaluate(COLUMNS_ATTR_JS):
                view_key(page, mod, 1, None, None)
            page.evaluate(UP_THE_LIST_JS, up)
            page.wait_for_timeout(300)
            before = still(page)
            first = before["first"]
            check(isinstance(first, dict) and rect(before, "gap") > 30,
                  f"{where}: the premise, a reader {up}px up Main: {before}")
            view_key(page, mod, 3, None, "list")
            propose(page, api, serial, draft)
            page.wait_for_timeout(500)
            check(thread_list_measure(page)["place"] == "dock", f"{where}: the {words} draft did not dock under the cards")
            behind = still(page)["first"]
            check(same_row(first, behind),
                  f"{where}: a {words} draft arriving moved the row a reader {up}px up Main was on from {first} to"
                  f" {behind}, behind the cards")
            view_key(page, mod, 1, None, None)
            back = still(page)
            check(same_row(first, back["first"]) and back["jump"] is not None,
                  f"{where}: a {words} draft arriving and {mod}+1 moved the row a reader {up}px up Main was on from"
                  f" {first} to {back['first']}, or offered no jump ({back['jump']!r})")
            api.settle()
            card_gone(page)
            page.wait_for_timeout(1300)

        # No card at all: a reader up All, Cmd+3, then Esc or Cmd+2, on the same row.
        all_on_row(page, where, mod, (100,))
        check(not api.commits and not api.cancels, f"{where}: a view key posted or withdrew a draft")
    finally:
        api.threaded = False
        api.older_threads = 0
    return (f"{where}: a reader 40 or 100px up Main on their row behind the cards and back by {mod}+1, offered the"
            f" jump; with no card, one 100px up All on their row by Esc and {mod}+2")


SIDE_UP_JS = ("(up) => { const a = document.getElementById('side-scroll');"
              " a.scrollTop = a.scrollHeight - a.clientHeight - up; }")


# Point 23's trips, (the way there, the way back, px up the cards): the gear and Help's "?", Back and Esc, from the
# foot of the cards and from 100px up them.
AWAY_TRIPS: tuple[tuple[str, str, int], ...] = (
    ("gear", "back", 0), ("help", "esc", 0), ("gear", "esc", 100), ("help", "back", 100),
)


def thread_list_away_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int],
                          phone: bool) -> str:
    """Point 23: a draft arriving while Settings or Help is up, back to the thread list, finds a reader who left
    from the foot of its cards at their foot over the card docked under them, and one who left up them on their
    card (`#336 draft-return-thread-list`)."""
    width, height = size
    where = f"{width}x{height}, a draft arriving while Settings or Help is up"
    mod = "Meta" if phone else "Control"
    api.threaded = True
    api.older_threads = 14
    try:
        open_call(page, url, voice, api, 30, False)
        page.click("#view-switch")
        page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                               timeout=10_000)
        page.wait_for_timeout(300)
        view_key(page, mod, 3, None, "list")
        for serial, (there, back, up) in enumerate(AWAY_TRIPS, start=231):
            how = ("the gear" if there == "gear" else "Help's \"?\"") + (" and Back" if back == "back" else " and Esc")
            page.evaluate(SIDE_UP_JS, up)
            page.wait_for_timeout(400)
            before = thread_list_measure(page)
            check(before["layout"] == "list" and before["card"] is None and
                  abs(float(str(before["sideGap"])) - up) <= 1 and float(str(before["sideRuns"])) > 300,
                  f"{where}: the premise, the thread list's cards with no card under them and a reader {up}px up"
                  f" them: {before}")
            if there == "gear":
                page.click("#open-settings")
                page.wait_for_function("() => currentScreen === 'settings'", timeout=5000)
            else:
                page.keyboard.press("?")
                page.wait_for_function("() => currentScreen === 'help'", timeout=5000)
            page.wait_for_timeout(300)
            propose(page, api, serial, LONG_DRAFT)
            page.wait_for_timeout(400)
            if back == "back":
                page.click("#close-settings" if there == "gear" else "#close-help")
            else:
                page.keyboard.press("Escape")
            page.wait_for_function("() => currentScreen === 'main'", timeout=5000)
            for when in ("", " a second later"):
                page.wait_for_timeout(1000 if when else 400)
                after = thread_list_measure(page)
                check(after["layout"] == "list" and after["place"] == "dock" and after["screen"] == "main",
                      f"{where}: {how} did not come back to the thread list with the card docked under it: {after}")
                if up == 0:
                    check(float(str(after["sideGap"])) <= 1 and after["send"] is not None and
                          inside(after["send"], float(str(after["innerHeight"])), float(str(after["innerWidth"]))),
                          f"{where}: {how} left a reader who was at the foot of the cards"
                          f" {float(str(after['sideGap'])):.0f}px above it{when}, Send on screen: {after['send']}")
                else:
                    first, now = before["first"], after["first"]
                    check(isinstance(first, dict) and isinstance(now, dict) and first["id"] == now["id"] and
                          abs(float(first["offset"]) - float(now["offset"])) <= 1,
                          f"{where}: {how} moved a reader {up}px up the cards from {first} to {now}{when}")
            api.settle()
            card_gone(page)
            page.wait_for_timeout(1300)
        check(not api.commits and not api.cancels, f"{where}: a trip posted or withdrew a draft")
    finally:
        api.threaded = False
        api.older_threads = 0
    return (f"{where}: a reader who left from the foot of the thread list's cards at it again over the card, Send on"
            " screen, and one 100px up on their card, by the gear or Help and Back or Esc")


def thread_list_cards_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int]) -> str:
    """Point 14, at the desk: a reader up the thread list's cards with a send card docked under them is on
    their card again by Ctrl+4 and Ctrl+3, though two columns' right column, the card gone from under it, is
    taller than the list's column of cards and put them at its foot or within the slack of it."""
    width, height = size
    where = f"{width}x{height}, up the thread list's cards and back from two columns"
    mod = "Control"
    api.threaded = True
    api.older_threads = 14
    lines: list[str] = []
    try:
        open_call(page, url, voice, api, 30, False)
        page.click("#view-switch")
        page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                               timeout=10_000)
        page.wait_for_timeout(300)
        view_key(page, mod, 1, None, None)

        def to_list(start: int) -> None:
            """Into the thread list from Main (`start` 1) or two columns (4), the card docked under it."""
            layout = page.evaluate(COLUMNS_ATTR_JS)
            if start == 1 and layout is not None:
                view_key(page, mod, 1, None, None)
            if start == 4 and layout != "two":
                view_key(page, mod, 4, None, "two")
            view_key(page, mod, 3, None, "list")
            page.wait_for_function("() => document.querySelectorAll('#side-cards > li[data-id]').length > 10",
                                   timeout=10_000)

        def up_the_cards(up: int, label: str) -> dict[str, object]:
            page.evaluate(SIDE_UP_JS, up)
            page.wait_for_timeout(300)
            m = thread_list_measure(page)
            check(isinstance(m["first"], dict) and abs(float(str(m["sideGap"])) - up) <= 1,
                  f"{where}: the premise, a reader {up}px up the cards {label}: {m['first']}, {m['sideGap']}px")
            return m

        for serial, draft, ups in ((61, LONG_DRAFT, (60, 160, 300)), (62, SHORT_DRAFT, (100,))):
            words = "long" if draft is LONG_DRAFT else "short"
            propose(page, api, serial, draft)
            page.wait_for_timeout(HOLD_MS + 200)
            for up in ups:
                for start in (1, 4):
                    whence = "Main" if start == 1 else "two columns"
                    to_list(start)
                    before = up_the_cards(up, "of the list")
                    check(before["place"] == "dock",
                          f"{where}: the {words} draft is not docked under the cards ({before['place']})")
                    card = rect(before["card"], "height")
                    view_key(page, mod, 4, None, "two")
                    two = thread_list_measure(page)
                    # The premise: two columns read this reader as at the foot of their cards.
                    check(float(str(two["sideGap"])) <= 24,
                          f"{where}: the premise, a reader {up}px up the cards ({words} draft, card {card:.0f}px) is"
                          f" {float(str(two['sideGap'])):.0f}px from the foot in two columns, outside the slack")
                    view_key(page, mod, 3, None, "list")
                    now = thread_list_measure(page)
                    check(same_row(before["first"], now["first"]),
                          f"{where}: {mod}+4 and {mod}+3 moved the card a reader {up}px up the list's cards was on, the"
                          f" list opened from {whence} with the {words} draft's {card:.0f}px card docked under it, from"
                          f" {before['first']} to {now['first']} ({float(str(now['sideGap'])):.0f}px from the foot)")
            lines.append(f"{', '.join(str(up) for up in ups)}px up with the {words} draft")
            if draft is LONG_DRAFT:
                # A reader who moves the cards in two columns is placed where they moved to, not where they were.
                to_list(1)
                up_the_cards(60, "of the list")
                view_key(page, mod, 4, None, "two")
                moved = up_the_cards(200, "in two columns")
                view_key(page, mod, 3, None, "list")
                now = thread_list_measure(page)
                check(same_row(moved["first"], now["first"]),
                      f"{where}: a reader who moved 200px up the cards in two columns was not kept there by {mod}+3:"
                      f" from {moved['first']} to {now['first']}")
            api.settle()
            card_gone(page)
            page.wait_for_timeout(1300)
        check(not api.commits and not api.cancels, f"{where}: a view key posted or withdrew a draft")
    finally:
        api.threaded = False
        api.older_threads = 0
    return (f"{where}: a reader {' and '.join(lines)} on their card again by {mod}+4 and {mod}+3, from Main and"
            " from two columns; one who moved in two columns kept where they moved to")


# Point 15: whether the main screen still keeps the room it made for the card, and Main's or All's last row.
ROOM_JS = """() => {
  const rows = [...document.querySelectorAll('#discord-log > li[data-id]')];
  return { room: document.getElementById('screen-main').hasAttribute('data-draft'),
           newest: rows.length ? rows[rows.length - 1].getAttribute('data-id') : null };
}"""

# Point 15: the live feed's read of the channel, as the message a send posted brings it, and whether the room
# made for the card was still kept when the read had drawn it.
LIVE_READ_JS = """async () => {
  await loadDiscord({ keepPosition: true, reason: 'live' });
  return document.getElementById('screen-main').hasAttribute('data-draft');
}"""


def room_back_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int], phone: bool) -> str:
    """Point 15: a reader at the foot of Main or All taps Send, and is still at the foot once the box at the
    list's foot takes back its room a second later, though the message they sent is drawn by a live read in
    that second, and with the browser's own scroll anchoring off; and so is one whose draft, opened by More
    with a click, is settled elsewhere, which hands the focus from More to the box."""
    width, height = size
    where = f"{width}x{height}, the box taking back its room"
    mod = "Meta" if phone else "Control"
    api.threaded = True
    api.older_threads = 14
    try:
        open_call(page, url, voice, api, 30, False)
        api.deliver_posts = True
        page.click("#view-switch")
        page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                               timeout=10_000)
        page.wait_for_timeout(300)
        serial = 70
        hows = ("the message it sent drawn by a live read", "the browser's scroll anchoring off",
                "the draft settled elsewhere with the focus on More, scroll anchoring off")
        for view, digit in (("Main", 1), ("All", 2)):
            for how in hows:
                view_key(page, mod, digit, None, None)
                anchoring = "" if how.startswith("the message") else "none"
                page.evaluate("(value) => { document.getElementById('scroll-area').style.overflowAnchor = value; }",
                              anchoring)
                serial += 1
                more = how.startswith("the draft")
                posted_before = len(api.posted)
                propose(page, api, serial, LONG_DRAFT if more else SHORT_DRAFT)
                page.wait_for_timeout(HOLD_MS + 200)
                if more:
                    # A click, as a mouse makes one: the focus stays on More, inside the card.
                    page.click("#post-confirm-more")
                    page.wait_for_function("() => !document.getElementById('post-confirm').hasAttribute('data-folded')",
                                           timeout=5000)
                page.evaluate(TO_NEWEST_JS)
                page.wait_for_timeout(300)
                check(rect(still(page), "gap") <= 1, f"{where}: the premise, a reader at {view}'s foot")
                if more:
                    api.settle()
                elif phone:
                    page.tap("#post-confirm-send")
                else:
                    page.click("#post-confirm-send")
                card_gone(page)
                gone = time.monotonic()
                if more:
                    focus = page.evaluate("() => document.activeElement ? document.activeElement.id : ''")
                    check(focus == "channel-compose-text",
                          f"{where}: the premise, the focus handed from More to the box, not to #{focus}")
                if anchoring == "":
                    # The live read has to land while the card's room is kept (the page
                    # returns it a second after Send).  The card is already observed
                    # gone above; spend none of what is left of that second on a
                    # fixed 300 ms before starting the read -- on a loaded machine
                    # the wait and the read together outlasted it and the premise
                    # read a room already handed back (done-races validate-land3,
                    # 2026-10-19).  The read itself is still served by a starved
                    # server thread, so this narrows the window rather than
                    # closing it; the premise below fails by name if it misses.
                    # `#389 gate-flakes`.
                    if not more:
                        until(page, lambda: len(api.posted) > posted_before,
                              "the sent message to be posted")
                    kept = page.evaluate(LIVE_READ_JS)
                    drawn = page.evaluate(ROOM_JS)
                    check(kept is True and drawn["newest"] == api.posted[-1]["id"],
                          f"{where}: the premise, the sent message drawn by a read while the card's room is kept"
                          f" ({kept}, newest {drawn['newest']})")
                page.wait_for_timeout(max(0, int(1800 - (time.monotonic() - gone) * 1000)))
                check(page.evaluate(ROOM_JS)["room"] is False, f"{where}: the premise, the box has its room back")
                now = still(page)
                check(rect(now, "gap") <= 1 and now["jump"] is None,
                      f"{where}: {'the draft going' if more else 'Send'} at {view}'s foot, {how}, left the reader"
                      f" {rect(now, 'gap'):.0f}px above the foot once the box took back its room (the jump says"
                      f" {now['jump']!r})")
                page.evaluate("() => { document.getElementById('scroll-area').style.overflowAnchor = ''; }")
                if more:
                    # The focus back on the list, so that the next card's Send is clicked as the others were.
                    page.evaluate("() => document.getElementById('scroll-area').focus()")
        check(len(api.commits) == 4, f"{where}: {len(api.commits)} commits, not one per tap")
        check(not api.cancels, f"{where}: a draft was withdrawn ({api.cancels})")
    finally:
        api.threaded = False
        api.older_threads = 0
        api.deliver_posts = False
    return (f"{where}: a reader at the foot of Main and of All at it a second after Send, the sent message drawn"
            " by a live read in that second, and with scroll anchoring off; and after the draft opened by More was"
            " settled elsewhere, the focus handed to the box")


# Point 16: the card's top and height against the channel's list, the list's height, and whether a tap at
# Send's centre lands on Send.
OPENED_CARD_JS = """() => {
  const area = document.getElementById('scroll-area');
  const card = document.getElementById('post-confirm');
  const send = document.getElementById('post-confirm-send');
  const box = card.getBoundingClientRect();
  const r = send.getBoundingClientRect();
  let hit = false;
  if (r.height > 0 && r.top >= -0.5 && r.bottom <= innerHeight + 0.5) {
    const n = document.elementFromPoint((r.left + r.right) / 2, (r.top + r.bottom) / 2);
    hit = Boolean(n && (n === send || send.contains(n)));
  }
  return { parent: card.parentElement ? card.parentElement.id : '', folded: card.hasAttribute('data-folded'),
           top: box.top - area.getBoundingClientRect().top, height: box.height, list: area.clientHeight, sendHit: hit };
}"""


def opened_card_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int]) -> str:
    """Point 16, on the phone: a long draft opened by More is taller than the list, and a reader whose screen
    shows only the card finds it where it stood through the thread list and back, and across Main and All."""
    width, height = size
    where = f"{width}x{height}, the draft opened by More"
    mod = "Meta"
    api.threaded = True
    api.older_threads = 14
    try:
        open_call(page, url, voice, api, 30, False)
        propose(page, api, 81, LONG_DRAFT)
        page.wait_for_timeout(HOLD_MS + 200)
        page.click("#view-switch")
        page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                               timeout=10_000)
        page.wait_for_timeout(300)
        page.click("#post-confirm-more")
        page.wait_for_function("() => !document.getElementById('post-confirm').hasAttribute('data-folded')",
                               timeout=5000)
        page.wait_for_timeout(300)
        routes = {1: (("Escape",), (1,), (2,)), 2: (("Escape",), (2,), (1,))}
        for view, digit in (("Main", 1), ("All", 2)):
            for up in (40, 200):
                for keys in routes[digit]:
                    through = keys[0] != (2 if digit == 1 else 1)
                    how = (f"{mod}+3 and " if through else "") + " and ".join(
                        "Esc" if key == "Escape" else f"{mod}+{key}" for key in keys)
                    view_key(page, mod, digit, None, None)
                    page.evaluate(UP_THE_LIST_JS, up)
                    page.wait_for_timeout(300)
                    before = page.evaluate(OPENED_CARD_JS)
                    first = still(page)["first"]
                    check(before["parent"] == "pane-discord" and not before["folded"] and
                          before["height"] > before["list"] and first is None and before["top"] < 0,
                          f"{where}: the premise, a reader {up}px up {view} who sees the opened card and no row:"
                          f" {before}, first row {first}")
                    if through:
                        view_key(page, mod, 3, None, "list")
                    for key in keys:
                        if through:
                            leave_thread_list(page, mod, str(key))
                        else:
                            view_key(page, mod, int(key), None, None)
                    after = page.evaluate(OPENED_CARD_JS)
                    check(after["parent"] == "pane-discord" and abs(after["top"] - before["top"]) <= 1 and
                          after["sendHit"] == before["sendHit"],
                          f"{where}: {how} moved the card a reader {up}px up {view} saw alone from {before['top']:.0f}"
                          f" to {after['top']:.0f}px against the list (Send tappable {before['sendHit']} then"
                          f" {after['sendHit']})")
        check(not api.commits and not api.cancels and api.proposal is not None,
              f"{where}: a view key posted or withdrew the draft ({api.commits}, {api.cancels})")
    finally:
        api.threaded = False
        api.older_threads = 0
    return (f"{where}: a reader 40 and 200px up Main and All who saw only the card found it where it stood by"
            f" {mod}+3 and Esc, {mod}+3 and the view's key, and {mod}+2 and {mod}+1")


def wheel_to_foot(page: Page) -> None:
    """The mouse's wheel turned down over the channel's list until it can go no further."""
    page.mouse.move(300, 400)
    for _ in range(12):
        page.mouse.wheel(0, 600)
        page.wait_for_timeout(60)
    page.wait_for_timeout(500)


def wheel_up(page: Page, px: int) -> None:
    """The mouse's wheel turned up `px` over the channel's list."""
    page.mouse.move(300, 400)
    page.mouse.wheel(0, -px)
    page.wait_for_timeout(600)


def two_columns_opened_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int]) -> str:
    """Point 17, at the desk: a long draft opened by More is drawn taller in two columns' left column than
    across the window, and a reader at the foot of Main or All is at the left column's foot through Ctrl+4
    and back, Send tappable; one up either is on their row."""
    width, height = size
    where = f"{width}x{height}, two columns and the draft opened by More"
    mod = "Control"
    api.threaded = True
    api.older_threads = 14
    try:
        open_call(page, url, voice, api, 30, False)
        propose(page, api, 91, LONG_DRAFT)
        page.wait_for_timeout(HOLD_MS + 200)
        page.click("#view-switch")
        page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                               timeout=10_000)
        page.wait_for_timeout(300)
        page.click("#post-confirm-more")
        page.wait_for_function("() => !document.getElementById('post-confirm').hasAttribute('data-folded')",
                               timeout=5000)
        page.wait_for_timeout(300)
        for view, digit in (("Main", 1), ("All", 2)):
            # At the foot. Main is left up its list first, so that where it was left is no help: Ctrl+4 from
            # All's foot read the reader as up All, on replies Main lacks, and put Main back there.
            view_key(page, mod, 1, None, None)
            wheel_up(page, 400)
            if digit == 2:
                view_key(page, mod, 2, None, None)
            page.evaluate("() => document.getElementById('scroll-area').focus({ preventScroll: true })")
            wheel_to_foot(page)
            before = page.evaluate(OPENED_CARD_JS)
            check(rect(still(page), "gap") <= 1 and before["parent"] == "pane-discord" and before["sendHit"],
                  f"{where}: the premise, a reader at {view}'s foot, Send tappable: {before}, {still(page)}")
            view_key(page, mod, 4, None, "two")
            for when in ("", " a second later"):
                if when:
                    page.wait_for_timeout(1000)
                two = page.evaluate(OPENED_CARD_JS)
                now = still(page)
                check(rect(two, "height") > rect(before, "height") + 24,
                      f"{where}: the premise, the card taller in two columns than in one ({rect(two, 'height'):.0f}px,"
                      f" {rect(before, 'height'):.0f}px across the window)")
                check(rect(now, "gap") <= 1 and two["sendHit"] and now["jump"] is None,
                      f"{where}: {mod}+4 left a reader who was at {view}'s foot {rect(now, 'gap'):.0f}px above the left"
                      f" column's foot{when}, Send tappable {two['sendHit']}, the jump saying {now['jump']!r}")
            view_key(page, mod, digit, None, None)
            back = page.evaluate(OPENED_CARD_JS)
            now = still(page)
            check(page.evaluate(PICKED_VIEW_JS) == ("main" if digit == 1 else "flat") and rect(now, "gap") <= 1 and
                  back["sendHit"],
                  f"{where}: {mod}+4 and {mod}+{digit} left a reader who was at {view}'s foot {rect(now, 'gap'):.0f}px"
                  f" above it, Send tappable {back['sendHit']}")

            # 400px up: on the same row through two columns and back.
            wheel_up(page, 400)
            up = still(page)
            first = up["first"]
            check(isinstance(first, dict) and rect(up, "gap") > 30, f"{where}: the premise, a reader up {view}: {up}")
            view_key(page, mod, 4, None, "two")
            two_first = still(page)["first"]
            view_key(page, mod, digit, None, None)
            back_first = still(page)["first"]
            check(same_row(first, two_first) and same_row(first, back_first),
                  f"{where}: {mod}+4 and {mod}+{digit} moved the row a reader 400px up {view} was on from {first} to"
                  f" {two_first}, then {back_first}")

        # Back from a change of layout that could not keep the place: a reader 40px up All, on a reply Main
        # lacks, is at Main's foot in two columns, and on their row again by Ctrl+2; one 40px up Main in two
        # columns, nearer its foot than the card grows there, is at Main's foot in one column, and on their
        # row again by Ctrl+4.
        for start, there, back_digit, label in ((2, None, 2, "All"), (4, "two", 1, "Main in two columns")):
            if page.evaluate(COLUMNS_ATTR_JS) != there or (start == 2 and page.evaluate(PICKED_VIEW_JS) != "flat"):
                view_key(page, mod, start, None, there)
            wheel_to_foot(page)
            wheel_up(page, 40)
            before_trip = still(page)
            check(isinstance(before_trip["first"], dict) and 24 < rect(before_trip, "gap") < 100,
                  f"{where}: the premise, a reader 40px up {label}: {before_trip}")
            view_key(page, mod, 4 if start == 2 else 1, None, "two" if start == 2 else None)
            away = still(page)
            check(rect(away, "gap") <= 1,
                  f"{where}: the premise, the reader 40px up {label} at the foot after the change ({away})")
            view_key(page, mod, back_digit if start == 2 else 4, None, None if start == 2 else "two")
            returned = still(page)["first"]
            check(same_row(before_trip["first"], returned),
                  f"{where}: the way back moved the row a reader 40px up {label} was on from {before_trip['first']}"
                  f" to {returned}")
        check(not api.commits and not api.cancels and api.proposal is not None,
              f"{where}: a view key posted or withdrew the draft ({api.commits}, {api.cancels})")
    finally:
        api.threaded = False
        api.older_threads = 0
    return (f"{where}: a reader at the foot of Main and of All at the left column's foot by {mod}+4, Send tappable,"
            f" and at it again by {mod}+1 and {mod}+2; one 400px up either on their row; one 40px up All on their"
            f" row again by {mod}+4 and {mod}+2, and one 40px up Main in two columns by {mod}+1 and {mod}+4")


# Point 18: the list on screen, the channel's or the call's, and the reader against it: how far above its foot
# they are, whether any of its rows is on screen, the card's top and height against the list, whether a tap at
# Send's centre lands on Send, and what the jump says.
CARD_ALONE_JS = """() => {
  const area = document.getElementById('scroll-area');
  const top = area.getBoundingClientRect().top;
  const bottom = top + area.clientHeight;
  const call = document.getElementById('pane-discord').hidden;
  const rows = call ? [...document.getElementById('transcript').children]
    : [...document.querySelectorAll('#discord-log > li[data-id]')];
  const row = rows.some((li) => {
    if (!li.getClientRects().length) return false;
    const b = li.getBoundingClientRect();
    return b.bottom > top && b.top < bottom;
  });
  const card = document.getElementById('post-confirm');
  const send = document.getElementById('post-confirm-send');
  const r = send.getBoundingClientRect();
  let hit = false;
  if (!card.hidden && r.height > 0 && r.top >= -0.5 && r.bottom <= innerHeight + 0.5) {
    const n = document.elementFromPoint((r.left + r.right) / 2, (r.top + r.bottom) / 2);
    hit = Boolean(n && (n === send || send.contains(n)));
  }
  const box = card.getBoundingClientRect();
  const jump = document.getElementById('jump-newest');
  return { call, gap: area.scrollHeight - area.scrollTop - area.clientHeight, row, list: area.clientHeight,
           card: card.hidden ? null : box.top - top, height: card.hidden ? null : box.height,
           folded: card.hasAttribute('data-folded'), sendHit: hit,
           jump: jump.hidden ? null : document.getElementById('jump-newest-label').textContent };
}"""

# Point 18's cases, (list, how the card goes, px up, the way back from the thread list or None): every list, both
# ways and two heights, at 393x852, with the card going from the list, and through the thread list the card going
# from under its cards and each way back; and one of each list at the two other phones. Each reader is past the
# slack of the foot and has the box under the card on screen, where Chromium's anchoring holds it, in the list
# and as the card goes to the dock; further up than SCROLLBACK_PX, 64px, the jump comes with its room and takes
# the box off screen. Through the thread list from Main, Main is left behind the cards where that anchoring left
# it, and the place the way back holds is the card. From All, Main stood at its foot behind the cards in these
# runs, and the way back reached the foot before the place was put back as well: those two are kept as cases the
# change must not break.
CARD_ALONE_CASES: tuple[tuple[str, str, int, str | None], ...] = (
    *((view, how, up, None) for view in ("Main", "All", "the call's view") for how in ("tap", "settle")
      for up in (30, 60)),
    ("Main", "settle", 40, "Escape"), ("Main", "tap", 60, "1"), ("Main", "settle", 40, "2"),
    ("All", "tap", 40, "1"), ("All", "settle", 60, "Escape"),
)
CARD_ALONE_FEW: tuple[tuple[str, str, int, str | None], ...] = (
    ("Main", "tap", 40, None), ("All", "settle", 60, None), ("the call's view", "tap", 50, None),
)
# web/voice.js's BOTTOM_SLACK_PX: a reader this near the foot is at it, and followed.
FOOT_SLACK_PX = 24


def card_alone_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int],
                    cases: tuple[tuple[str, str, int, str | None], ...]) -> str:
    """Point 18, on the phone: a reader who sees only a long draft opened by More, Send on screen, past the slack
    of the foot, is at the foot with no jump a second after Send is tapped or the draft is settled elsewhere — in
    Main, in All and on the call's view — though Chromium's own scroll anchoring holds the box under the card
    where it stood as the card goes; and so is one who saw only the card as Cmd+3 docked it under the thread
    list, on the way back by Esc, Cmd+1 or Cmd+2 once it has gone from there."""
    width, height = size
    where = f"{width}x{height}, the card going from a reader who saw only it"
    api.threaded = True
    api.older_threads = 14
    try:
        open_call(page, url, voice, api, 30, False)
        api.deliver_posts = True
        page.click("#view-switch")
        page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                               timeout=10_000)
        page.wait_for_timeout(300)
        serial = 180
        for view, how, up, back in cases:
            on_call = view == "the call's view"
            if page.evaluate("() => document.getElementById('pane-discord').hidden") != on_call:
                page.click("#view-switch")
                page.wait_for_timeout(400)
            if not on_call:
                view_key(page, "Meta", 1 if view == "Main" else 2, None, None)
            serial += 1
            propose(page, api, serial, LONG_DRAFT)
            # Send is held a moment after a card appears; a draft settled elsewhere need not wait for it.
            page.wait_for_timeout(HOLD_MS + 200 if how == "tap" else 300)
            # Opened by More without taking the focus onto the card, as a finger leaves it.
            page.evaluate("() => document.getElementById('post-confirm-more').click()")
            page.wait_for_function("() => !document.getElementById('post-confirm').hasAttribute('data-folded')",
                                   timeout=5000)
            page.wait_for_timeout(300)
            page.evaluate(UP_THE_LIST_JS, up)
            page.wait_for_timeout(400)
            before = page.evaluate(CARD_ALONE_JS)
            check(before["call"] is on_call and not before["folded"] and rect(before, "height") > rect(before, "list")
                  and before["row"] is False and rect(before, "card") < 0 and before["sendHit"] is True and
                  rect(before, "gap") > FOOT_SLACK_PX,
                  f"{where}: the premise, a reader {up}px up {view} who sees the opened card, Send tappable, and no"
                  f" row, past the slack of the foot: {before}")
            route = ""
            if back is not None:
                view_key(page, "Meta", 3, None, "list")
                route = f" after Meta+3, and {'Esc' if back == 'Escape' else f'Meta+{back}'} once it had gone,"
            if how == "tap":
                page.tap("#post-confirm-send")
            else:
                api.settle()
            card_gone(page)
            gone = time.monotonic()
            if back is not None:
                page.wait_for_timeout(200)
                leave_thread_list(page, "Meta", back)
                picked = page.evaluate(PICKED_VIEW_JS)
                to = view if back == "Escape" else ("Main" if back == "1" else "All")
                check(picked == ("main" if to == "Main" else "flat"),
                      f"{where}: the premise, {back} left the thread list for {to}, not {picked}")
                # The way back puts the card's place back on the main screen, and the room a second after that.
                gone = time.monotonic()
            page.wait_for_timeout(max(0, int(1800 - (time.monotonic() - gone) * 1000)))
            check(page.evaluate(ROOM_JS)["room"] is False, f"{where}: the premise, the box has its room back")
            after = page.evaluate(CARD_ALONE_JS)
            check(rect(after, "gap") <= 1 and after["jump"] is None,
                  f"{where}: {'Send tapped' if how == 'tap' else 'the draft settled elsewhere'}{route} left a reader"
                  f" who saw only the card {rect(before, 'gap'):.0f}px up {view} {rect(after, 'gap'):.0f}px above the"
                  f" foot a second after the card went (the jump says {after['jump']!r})")
            # The focus back on the list, so that the next card's Send is tapped as the others were.
            page.evaluate("() => document.getElementById('scroll-area').focus()")
        taps = sum(1 for _, how, _, _ in cases if how == "tap")
        check(len(api.commits) == taps, f"{where}: {len(api.commits)} commits, not one per tap ({taps})")
        check(not api.cancels, f"{where}: a draft was withdrawn ({api.cancels})")
    finally:
        api.threaded = False
        api.older_threads = 0
        api.deliver_posts = False
    ups = sorted({up for _, _, up, _ in cases})
    listed = any(back is not None for _, _, _, back in cases)
    return (f"{where}: readers {', '.join(str(up) for up in ups)}px up Main, All and the call's view, who saw only"
            " the card, at the foot with no jump a second after Send and after the draft was settled elsewhere"
            + ("; and those who saw only it as Meta+3 docked it, on the way back by Esc, Meta+1 and Meta+2 once it"
               " had gone from under the cards" if listed else ""))


# Point 19's trips, (list, px up, how Settings is left for, how the reader comes back): at the foot of each list,
# and 40 and 200px up Main and All, where a reader sees the opened card and no row; every way there and back
# from each list. Settings is reached by its gear, or by the gear and then Help's button there; Back is the
# screen's own button (Help's and then Settings'), Esc the key (from Help to Settings and from Settings back).
SETTINGS_TRIPS: tuple[tuple[str, int, str, str], ...] = (
    ("Main", 0, "gear", "back"), ("Main", 0, "help", "esc"),
    ("Main", 40, "gear", "esc"), ("Main", 200, "help", "back"),
    ("All", 0, "gear", "esc"), ("All", 0, "help", "back"),
    ("All", 40, "gear", "back"), ("All", 200, "help", "esc"),
    ("the call's view", 0, "gear", "back"), ("the call's view", 0, "help", "esc"),
    ("the call's view", 40, "gear", "esc"),
)
SETTINGS_TRIPS_FEW: tuple[tuple[str, int, str, str], ...] = (
    ("Main", 0, "gear", "back"), ("All", 0, "help", "esc"),
    ("Main", 40, "gear", "esc"), ("the call's view", 0, "gear", "back"),
)


def settings_trip_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int],
                       trips: tuple[tuple[str, int, str, str], ...]) -> str:
    """Point 19, on the phone: a reader of a long draft opened by More goes to Settings, docking the card there,
    and comes back where they were: at the foot of Main, All or the call's view with Send tappable and no jump,
    and one who saw only the card with it where it stood, within a pixel. Shown again with the card still
    docked under Settings, the list was laid out short of the card and the reader's place clamped to it."""
    width, height = size
    where = f"{width}x{height}, Settings and back with the draft opened by More"
    api.threaded = True
    api.older_threads = 14
    try:
        open_call(page, url, voice, api, 30, False)
        propose(page, api, 191, LONG_DRAFT)
        page.wait_for_timeout(HOLD_MS + 200)
        page.click("#view-switch")
        page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                               timeout=10_000)
        page.wait_for_timeout(300)
        # Opened by More without taking the focus onto the card, as a finger leaves it.
        page.evaluate("() => document.getElementById('post-confirm-more').click()")
        page.wait_for_function("() => !document.getElementById('post-confirm').hasAttribute('data-folded')",
                               timeout=5000)
        page.wait_for_timeout(300)
        for view, up, there, back in trips:
            on_call = view == "the call's view"
            if page.evaluate("() => document.getElementById('pane-discord').hidden") != on_call:
                page.click("#view-switch")
                page.wait_for_timeout(400)
            if not on_call:
                view_key(page, "Meta", 1 if view == "Main" else 2, None, None)
            page.evaluate(UP_THE_LIST_JS, up)
            page.wait_for_timeout(400)
            before = page.evaluate(CARD_ALONE_JS)
            if up == 0:
                check(before["call"] is on_call and rect(before, "gap") <= 1 and before["sendHit"] is True and
                      not before["folded"],
                      f"{where}: the premise, a reader at the foot of {view}, Send tappable: {before}")
            else:
                check(before["call"] is on_call and not before["folded"] and before["row"] is False and
                      rect(before, "height") > rect(before, "list") and rect(before, "card") < 0,
                      f"{where}: the premise, a reader {up}px up {view} who sees the opened card and no row: {before}")
            how = (("the gear" if there == "gear" else "the gear and Help") +
                   (" and Back" if back == "back" else " and Esc"))
            page.click("#open-settings")
            page.wait_for_function("() => currentScreen === 'settings'", timeout=5000)
            page.wait_for_timeout(300)
            docked = page.evaluate("() => { const c = document.getElementById('post-confirm');"
                                   " return [c.parentElement.id, c.getAttribute('data-place'), c.hidden]; }")
            check(docked == ["frame-body", "dock", False],
                  f"{where}: the premise, the card docked under Settings: {docked}")
            if there == "help":
                page.click("#open-help")
                page.wait_for_function("() => currentScreen === 'help'", timeout=5000)
                page.wait_for_timeout(300)
                if back == "back":
                    page.click("#close-help")
                else:
                    page.keyboard.press("Escape")
                page.wait_for_function("() => currentScreen === 'settings'", timeout=5000)
                page.wait_for_timeout(200)
            if back == "back":
                page.click("#close-settings")
            else:
                page.keyboard.press("Escape")
            page.wait_for_function("() => currentScreen === 'main'", timeout=5000)
            for when in ("", " a second later"):
                page.wait_for_timeout(1000 if when else 400)
                after = page.evaluate(CARD_ALONE_JS)
                if up == 0:
                    check(rect(after, "gap") <= 1 and after["sendHit"] is True and after["jump"] is None,
                          f"{where}: {how} left a reader who was at the foot of {view} {rect(after, 'gap'):.0f}px above"
                          f" it{when}, Send tappable {after['sendHit']}, the card's top at {rect(after, 'card'):.0f}px"
                          f" against the list, the jump saying {after['jump']!r}")
                else:
                    check(after["card"] is not None and abs(rect(after, "card") - rect(before, "card")) <= 1 and
                          after["sendHit"] == before["sendHit"],
                          f"{where}: {how} moved the card a reader {up}px up {view} saw alone from"
                          f" {rect(before, 'card'):.0f} to {rect(after, 'card'):.0f}px against the list{when}"
                          f" (Send tappable {before['sendHit']} then {after['sendHit']})")
        check(not api.commits and not api.cancels and api.proposal is not None,
              f"{where}: a trip to Settings posted or withdrew the draft ({api.commits}, {api.cancels})")
    finally:
        api.threaded = False
        api.older_threads = 0
    lists = sorted({view for view, _, _, _ in trips}, key=["Main", "All", "the call's view"].index)
    ups = sorted({up for _, up, _, _ in trips if up})
    return (f"{where}: a reader at the foot of {', '.join(lists)} at it again, Send tappable, and one"
            f" {' and '.join(str(up) for up in ups)}px up who saw only the card with it where it stood, by the gear"
            " or Settings and Help, and Back or Esc")


# Point 22's trips, (list, px up, the key for Help, what becomes of the draft while Help is up, the way back): at
# the foot of each list, and 40 and 100px up Main and All, where a reader sees the opened card and no row. The
# draft is settled elsewhere (the server's proposal goes to none, as after a spoken yes, another device or its
# lapse), its Send is tapped where it is docked under Help, or a short draft takes its place. Help is left by
# Esc, by its own Back tapped, or by the browser's Back.
HELP_KEY_TRIPS: tuple[tuple[str, int, str, str, str], ...] = (
    ("Main", 0, "?", "settle", "esc"), ("Main", 0, "?", "settle", "back"), ("Main", 0, "?", "settle", "browser"),
    ("Main", 0, "Control+Shift+?", "settle", "esc"), ("Main", 40, "?", "settle", "esc"),
    ("All", 0, "?", "tap", "back"), ("All", 100, "?", "settle", "browser"), ("All", 0, "?", "shorter", "esc"),
    ("the call's view", 0, "?", "settle", "esc"), ("the call's view", 0, "?", "tap", "back"),
)
HELP_KEY_TRIPS_FEW: tuple[tuple[str, int, str, str, str], ...] = (
    ("Main", 0, "?", "settle", "esc"), ("All", 0, "?", "settle", "browser"),
    ("the call's view", 0, "?", "settle", "back"), ("Main", 0, "?", "shorter", "back"),
)
HELP_KEY_CHANGES = {"settle": "the draft settled elsewhere", "tap": "Send tapped under Help",
                    "shorter": "a short draft in its place"}
HELP_KEY_BACKS = {"esc": "Esc", "back": "Help's Back", "browser": "the browser's Back"}


def help_key_trip_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int],
                       trips: tuple[tuple[str, int, str, str, str], ...]) -> str:
    """Point 22, on a phone with a keyboard: a reader of a long draft opened by More presses "?" or Ctrl+? for
    Help, the draft goes from under Help or a short one takes its place, and the way back by Esc, Help's Back or
    the browser's Back finds them on the newest line, with no jump, now and once the box has taken back its
    room. The main screen was shown before Help's list, which held the focus, was hidden, and before the control
    pane was shown: Chromium laid the page out to blur that list, and the channel's list, short of the card that
    had gone, was clamped to a window the pane's height too tall."""
    width, height = size
    where = f"{width}x{height}, Help by its key and back"
    api.threaded = True
    api.older_threads = 14
    try:
        open_call(page, url, voice, api, 30, False)
        api.deliver_posts = True
        page.click("#view-switch")
        page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                               timeout=10_000)
        page.wait_for_timeout(300)
        serial = 220
        for view, up, key, change, back in trips:
            on_call = view == "the call's view"
            if page.evaluate("() => document.getElementById('pane-discord').hidden") != on_call:
                page.click("#view-switch")
                page.wait_for_timeout(400)
            if not on_call:
                view_key(page, "Meta", 1 if view == "Main" else 2, None, None)
            serial += 1
            propose(page, api, serial, LONG_DRAFT)
            page.wait_for_timeout(HOLD_MS + 200)
            # Opened by More without taking the focus onto the card, as a finger leaves it.
            page.evaluate("() => document.getElementById('post-confirm-more').click()")
            page.wait_for_function("() => !document.getElementById('post-confirm').hasAttribute('data-folded')",
                                   timeout=5000)
            page.wait_for_timeout(300)
            page.evaluate(UP_THE_LIST_JS, up)
            # The keys on the list, as after the reader's own scroll.
            page.evaluate("() => document.getElementById('scroll-area').focus({ preventScroll: true })")
            page.wait_for_timeout(400)
            before = page.evaluate(CARD_ALONE_JS)
            if up == 0:
                check(before["call"] is on_call and rect(before, "gap") <= 1 and before["sendHit"] is True and
                      not before["folded"],
                      f"{where}: the premise, a reader at the foot of {view}, Send tappable: {before}")
            else:
                check(before["call"] is on_call and not before["folded"] and before["row"] is False and
                      rect(before, "height") > rect(before, "list") and rect(before, "card") < 0,
                      f"{where}: the premise, a reader {up}px up {view} who sees the opened card and no row: {before}")
            how = f"{key} for Help, {HELP_KEY_CHANGES[change]} and {HELP_KEY_BACKS[back]}"
            page.keyboard.press(key)
            page.wait_for_function("() => currentScreen === 'help'", timeout=5000)
            page.wait_for_timeout(300)
            docked = page.evaluate("() => { const c = document.getElementById('post-confirm');"
                                   " return [c.parentElement.id, c.getAttribute('data-place'), c.hidden]; }")
            check(docked == ["frame-body", "dock", False], f"{where}: the premise, the card docked under Help: {docked}")
            if change == "settle":
                api.settle()
                card_gone(page)
            elif change == "tap":
                page.tap("#post-confirm-send")
                card_gone(page)
            else:
                serial += 1
                api.propose({"serial": serial, "handle": f"hAnDlE-browser-check-{serial:04d}",
                             "channel_id": CHANNEL["id"], "channel_name": "lead team",
                             "text": POST_PREFIX + SHORT_DRAFT, "prefix": POST_PREFIX, "reply_to": None,
                             "expires_in_ms": 120_000})
                page.wait_for_function(f"() => postWatch.shown !== null && postWatch.shown.serial === {serial}",
                                       timeout=10_000)
            page.wait_for_timeout(300)
            if back == "esc":
                page.keyboard.press("Escape")
            elif back == "back":
                page.tap("#close-help")
            else:
                page.go_back()
            page.wait_for_function("() => currentScreen === 'main'", timeout=5000)
            for when in ("", " a second later"):
                page.wait_for_timeout(1400 if when else 400)
                after = page.evaluate(CARD_ALONE_JS)
                kept = rect(after, "gap") <= 1 and after["jump"] is None and (
                    after["card"] is not None and after["sendHit"] is True if change == "shorter" else after["card"] is None)
                check(kept, f"{where}: {how} left a reader {up}px up {view} {rect(after, 'gap'):.0f}px above the foot"
                            f"{when}, the jump saying {after['jump']!r}, the card {'up' if after['card'] is not None else 'gone'}"
                            f" and Send tappable {after['sendHit']}")
            if change == "shorter":
                api.settle()
                card_gone(page)
                page.wait_for_timeout(1300)
            page.evaluate("() => document.getElementById('scroll-area').focus()")
        taps = sum(1 for *_, change, _ in trips if change == "tap")
        check(len(api.commits) == taps, f"{where}: {len(api.commits)} commits, not one per tap ({taps})")
        check(not api.cancels, f"{where}: a draft was withdrawn ({api.cancels})")
    finally:
        api.threaded = False
        api.older_threads = 0
        api.deliver_posts = False
    lists = sorted({view for view, *_ in trips}, key=["Main", "All", "the call's view"].index)
    ups = sorted({up for _, up, *_ in trips if up})
    keys = sorted({key for _, _, key, _, _ in trips})
    return (f"{where}: a reader at the foot of {', '.join(lists)}"
            + (f", and one {' and '.join(str(up) for up in ups)}px up who saw only the card," if ups else "")
            + f" on the newest line with no jump by {' and '.join(keys)},"
            f" {', '.join(sorted({HELP_KEY_CHANGES[change] for *_, change, _ in trips}))},"
            f" and {', '.join(sorted({HELP_KEY_BACKS[back] for *_, back in trips}))}")


# Point 20: the chips over the foot of the list on screen, by id, and the room the lists keep for them.
CHIPS_JS = """() => {
  const shown = (id) => {
    const n = document.getElementById(id);
    return Boolean(n && !n.hidden && n.getClientRects().length);
  };
  return { summarise: shown('summarise'), expand: shown('expand-all'),
           room: getComputedStyle(document.getElementById('channel-composer')).marginTop };
}"""


def chips_room_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int]) -> str:
    """Point 20, on the phone: All with Summarise and Expand all up, a reader 40 or 60px above its foot goes to
    Settings and comes back where they were, with no card and with a long draft opened by More that is all
    they see; one at the foot is at it again. The chips are measured at nothing while their screen is hidden,
    and the room the lists keep for them used to go with that, so the list came back short of it and the
    reader within the slack of its foot."""
    width, height = size
    where = f"{width}x{height}, Settings and back with the chips up"
    api.threaded = True
    api.older_threads = 14
    try:
        open_call(page, url, voice, api, 30, False)
        # Three long messages in the channel, which bring up Summarise and Expand all.
        api.posted.extend(channel_message(n, "codex", LONG_DRAFT) for n in (30, 31, 32))
        page.click("#view-switch")
        page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                               timeout=10_000)
        page.wait_for_timeout(300)
        view_key(page, "Meta", 2, None, None)
        chips = page.evaluate(CHIPS_JS)
        check(chips["summarise"] is True and chips["expand"] is True and chips["room"] != "0px",
              f"{where}: the premise, Summarise and Expand all up over All, the channel's box keeping room: {chips}")
        for card in (False, True):
            if card:
                propose(page, api, 201, LONG_DRAFT)
                page.wait_for_timeout(HOLD_MS + 200)
                page.evaluate("() => document.getElementById('post-confirm-more').click()")
                page.wait_for_function("() => !document.getElementById('post-confirm').hasAttribute('data-folded')",
                                       timeout=5000)
                page.wait_for_timeout(300)
            for up in (0, 40, 60):
                for back in ("back", "esc"):
                    page.evaluate(UP_THE_LIST_JS, up)
                    page.wait_for_timeout(400)
                    before = page.evaluate(CARD_ALONE_JS)
                    first = still(page)["first"]
                    alone = " who sees only the card" if card and up else ""
                    check(rect(before, "gap") == up and (not card or up == 0 or before["row"] is False),
                          f"{where}: the premise, a reader {up}px up All{alone}: {before}")
                    page.click("#open-settings")
                    page.wait_for_function("() => currentScreen === 'settings'", timeout=5000)
                    page.wait_for_timeout(400)
                    if back == "back":
                        page.click("#close-settings")
                    else:
                        page.keyboard.press("Escape")
                    page.wait_for_function("() => currentScreen === 'main'", timeout=5000)
                    page.wait_for_timeout(500)
                    after = page.evaluate(CARD_ALONE_JS)
                    now = still(page)["first"]
                    label = (("with the opened card" if card else "with no card") +
                             (", by Back" if back == "back" else ", by Esc"))
                    if up == 0:
                        check(rect(after, "gap") <= 1 and after["jump"] is None and (not card or after["sendHit"]),
                              f"{where}: {label}, a reader at All's foot came back {rect(after, 'gap'):.0f}px above it")
                    elif card:
                        check(after["card"] is not None and abs(rect(after, "card") - rect(before, "card")) <= 1,
                              f"{where}: {label}, the card a reader {up}px up All saw alone moved from"
                              f" {rect(before, 'card'):.0f} to {rect(after, 'card'):.0f}px against the list (now"
                              f" {rect(after, 'gap'):.0f}px above the foot)")
                    else:
                        check(same_row(first, now) and abs(rect(after, "gap") - up) <= 1,
                              f"{where}: {label}, a reader {up}px up All on {first} came back on {now},"
                              f" {rect(after, 'gap'):.0f}px above the foot")
        check(not api.commits and not api.cancels and api.proposal is not None,
              f"{where}: a trip to Settings posted or withdrew the draft ({api.commits}, {api.cancels})")
    finally:
        api.threaded = False
        api.older_threads = 0
    return (f"{where}: a reader 40 and 60px up All with Summarise and Expand all up where they were by Back and Esc,"
            " card or no card, and one at its foot at it again")


# Point 21's trips, (list, px up, how the reader comes back): readers up past the slack of the foot but short of
# the jump's SCROLLBACK_PX, 64px, who the header Settings kept would put past it (54px of it at the desk, 8px on
# a phone), and one far up who has the jump and keeps it.
JUMP_TRIPS_DESK: tuple[tuple[str, int, str], ...] = (
    ("Main", 30, "back"), ("Main", 40, "esc"), ("All", 60, "back"), ("the call's view", 40, "esc"),
    ("Main", 150, "back"),
)
JUMP_TRIPS_PHONE: tuple[tuple[str, int, str], ...] = (
    ("Main", 60, "back"), ("All", 60, "esc"), ("the call's view", 60, "back"), ("All", 150, "esc"),
)


def jump_after_settings_case(page: Page, url: str, voice: FakeVoice, api: FakeApi, size: tuple[int, int],
                             phone: bool, trips: tuple[tuple[str, int, str], ...]) -> str:
    """Point 21: with no card, a reader up a list short of the jump's distance goes to Settings and comes back
    with no jump offered and nothing grown under them, and one far up keeps the jump. Decided as the main screen
    was shown, under the header Settings kept, the jump was offered to the first and its room then held it."""
    width, height = size
    where = f"{width}x{height}, the jump back from Settings"
    mod = "Meta" if phone else "Control"
    api.threaded = True
    api.older_threads = 14
    try:
        open_call(page, url, voice, api, 30, False)
        page.click("#view-switch")
        page.wait_for_function("() => document.querySelectorAll('#discord-log > li[data-id]').length > 0",
                               timeout=10_000)
        page.wait_for_timeout(300)
        for view, up, back in trips:
            on_call = view == "the call's view"
            if page.evaluate("() => document.getElementById('pane-discord').hidden") != on_call:
                page.click("#view-switch")
                page.wait_for_timeout(400)
            if not on_call:
                view_key(page, mod, 1 if view == "Main" else 2, None, None)
            # From the foot, as a thumb comes up the list: below SCROLLBACK_PX the jump is not up.
            page.evaluate(TO_NEWEST_JS)
            page.wait_for_timeout(300)
            page.evaluate(UP_THE_LIST_JS, up)
            page.wait_for_timeout(400)
            before = page.evaluate(CARD_ALONE_JS)
            first = None if on_call else still(page)["first"]
            # Far up, the jump comes, and the room the box keeps for it grows the list under the reader.
            check((rect(before, "gap") == up if up < 64 else rect(before, "gap") >= up) and before["card"] is None and
                  (before["jump"] is None) == (up < 64) and (on_call or isinstance(first, dict)),
                  f"{where}: the premise, a reader {up}px up {view} with no card: {before}, first row {first}")
            page.click("#open-settings")
            page.wait_for_function("() => currentScreen === 'settings'", timeout=5000)
            page.wait_for_timeout(400)
            if back == "back":
                page.click("#close-settings")
            else:
                page.keyboard.press("Escape")
            page.wait_for_function("() => currentScreen === 'main'", timeout=5000)
            for when in ("", " a second later"):
                page.wait_for_timeout(1000 if when else 400)
                after = page.evaluate(CARD_ALONE_JS)
                now = None if on_call else still(page)["first"]
                check(abs(rect(after, "gap") - rect(before, "gap")) <= 1 and after["jump"] == before["jump"] and
                      (on_call or same_row(first, now)),
                      f"{where}: {'Back' if back == 'back' else 'Esc'} left a reader {up}px up {view}"
                      f" {rect(after, 'gap'):.0f}px above the foot (was {rect(before, 'gap'):.0f}){when}, the jump"
                      f" saying {after['jump']!r} (it said {before['jump']!r}), on {now} (was {first})")
    finally:
        api.threaded = False
        api.older_threads = 0
    near = sorted({up for _, up, _ in trips if up < 64})
    far = sorted({up for _, up, _ in trips if up >= 64})
    return (f"{where}: a reader {' and '.join(str(up) for up in near)}px up offered no jump and on their row by Back"
            f" and Esc, one {' and '.join(str(up) for up in far)}px up offered it still")


def walk_desk(chromium: BrowserType, args: argparse.Namespace, url: str, api: FakeApi) -> str:
    """Points 8, 11 to 15, 17 and 21 on a desk with a mouse, where two columns are offered."""
    width, height = DESK
    voice = FakeVoice()
    with tempfile.TemporaryDirectory(prefix="vibe-talk-chrome-approval-card-") as directory:
        context = chromium.launch_persistent_context(
            directory, headless=True, executable_path=args.browser_executable,
            viewport={"width": width, "height": height},
        )
        try:
            context.route_web_socket(re.compile(r"/ws\?mode=voice$"), voice.accept)
            page = context.pages[0]
            errors: list[str] = []
            page.on("pageerror", lambda error: errors.append(str(error)))
            line = columns_case(page, url, voice, api, args.screenshots)
            line += "; " + thread_list_case(page, url, voice, api, DESK, False, args.screenshots)
            line += "; " + thread_list_place_case(page, url, voice, api, DESK, False)
            line += "; " + thread_list_arrival_case(page, url, voice, api, DESK, False)
            line += "; " + thread_list_away_case(page, url, voice, api, DESK, False)
            line += "; " + thread_list_cards_case(page, url, voice, api, DESK)
            line += "; " + room_back_case(page, url, voice, api, DESK, False)
            line += "; " + two_columns_opened_case(page, url, voice, api, DESK)
            line += "; " + jump_after_settings_case(page, url, voice, api, DESK, False, JUMP_TRIPS_DESK)
            check(not errors, f"{width}x{height}: the page threw: {errors}")
        finally:
            context.close()
    return line


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

    api = FakeApi()
    server = ThreadingHTTPServer(("127.0.0.1", 0), handler_for(api, args.web_root.resolve()))
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_port}/"
    try:
        with sync_playwright() as playwright:
            lines = [walk(playwright.chromium, args, url, api, size) for size in PHONES]
            lines += [walk_short(playwright.chromium, args, url, api, size) for size in SHORT_SCREENS]
            lines.append(walk_desk(playwright.chromium, args, url, api))
        for line in lines:
            print(line)
        return 0
    finally:
        api.stopping.set()
        with api.changed:
            api.changed.notify_all()
        server.shutdown()
        server.server_close()
        thread.join()


if __name__ == "__main__":
    raise SystemExit(main())
