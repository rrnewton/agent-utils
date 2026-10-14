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
     chosen from All) or Cmd+2, is at All's foot again with Send on screen; a reader 100, 200 and
     380px up Main, Cmd+3 and then Cmd+1, Esc and, at the desk, Cmd+4, is on the same row within a
     pixel, and from 200px up on it behind the cards too.

Runs: 393x852, 360x780 and 375x667, each with a one-row and a thirty-row transcript, with and
without the first call's connection banner, and with a short and a 1,500-character draft, and points
9 and 10 once; the docked card alone at 667x375 and 375x330; points 11 and 12 at 393x852; and points
8, 11 and 12 at 1280x800 with a mouse.

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
            "elevenlabs_agent_id": None,
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
        if not self.threaded:
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
        rows = (messages + replies if view == "flat"
                else [messages[2], *replies] if view == "thread"
                else [] if view == "threads"
                else messages)
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
                else:
                    message = channel_message(99, "bot", str(proposal["text"]))
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
            relative = "voice.html" if request_path == "/voice" else request_path.lstrip("/")
            target = (web_root / (relative or "index.html")).resolve()
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
    api.propose({"serial": serial, "handle": f"hAnDlE-browser-check-{serial:04d}", "channel_id": CHANNEL["id"],
                 "channel_name": "lead team", "text": text, "reply_to": None, "expires_in_ms": 120_000})
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
    page.locator("#post-confirm").wait_for(state="hidden", timeout=5000)
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
    page.locator("#post-confirm").wait_for(state="hidden", timeout=5000)
    until(page, lambda: any(entry.startswith("claim") for entry in api.log), "the claim of the send")
    page.wait_for_timeout(300)
    frozen = page.evaluate(FROZEN_JS)
    check(isinstance(frozen, dict), f"{where}: nothing measured")
    assert isinstance(frozen, dict)
    check(frozen["count"] == 1 and frozen["last"] is True, f"{where}: the frozen card is not the newest row: {frozen}")
    check(api.log == ["commit", f"claim {api.sends_through}"],
          f"{where}: the server saw {api.log}, not the commit and then the claim of its send")
    text = str(frozen["text"])
    for said in ("Sent to lead team", "approved by tap", SHORT_DRAFT):
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
            page.locator("#post-confirm").wait_for(state="hidden", timeout=5000)
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
            page.locator("#post-confirm").wait_for(state="hidden", timeout=5000)
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
        const b = document.querySelector("#{list_id} > li[data-id='{message_id}'] > .meta > .reply-button");
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
        page.locator("#post-confirm").wait_for(state="hidden", timeout=5000)
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
        page.locator("#post-confirm").wait_for(state="hidden", timeout=5000)
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
        page.locator("#post-confirm").wait_for(state="hidden", timeout=5000)
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
    return (f"{where}: a reader at All's foot at it again by Esc and {mod}+2, Send on screen; one 100 to 380px up"
            f" Main on their row by {', '.join('Esc' if key == 'Escape' else f'{mod}+{key}' for key in keys)}")


def walk_desk(chromium: BrowserType, args: argparse.Namespace, url: str, api: FakeApi) -> str:
    """Points 8 and 11 on a desk with a mouse, where two columns are offered."""
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
    url = f"http://127.0.0.1:{server.server_port}/voice"
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
