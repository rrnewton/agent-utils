#!/usr/bin/env python3
"""Photograph the app at / in every state that looks different, so an agent can LOOK at it.

WHY THIS EXISTS
===============

The app has a large behavioural suite (tests/js/voice_page.test.mjs) that drives the real
web/voice.js against a strict fake DOM and asserts that the right properties sit on the right
selectors. Not one of those assertions can tell you the page LOOKS right, and the suite says so
itself: the fixture lays nothing out.

That gap is not theoretical. The owner took ONE photograph of his phone and it immediately showed
three defects the whole suite had passed over -- a clipped paragraph, a control that read as active
on a dead call, and an ambiguous header. Every one of those is a layout fact, and a fixture with no
layout engine cannot have an opinion about it.

So this script closes exactly that gap and nothing else: it drives a REAL browser at a REAL phone
viewport, walks the page into each state that differs visually, and writes a PNG per state. It
asserts nothing about aesthetics -- judging the result is the reader's job, human or agent. What it
DOES assert is that each picture is worth looking at, which is a different thing and is where a
screenshot harness usually fails.

HOW A SCREENSHOT HARNESS FAILS, AND WHAT IS DONE ABOUT IT
=========================================================

A harness that produces images nobody checks is merely useless. A harness that produces the WRONG
images is worse than none, because it looks like evidence:

  * Eight pictures of the same idle screen. A click that silently did not land, a state that was
    never entered, and the run still writes eight files with eight confident names. So every state
    declares what must be TRUE of the live page before the shutter opens -- the sign-in shot must
    have the sign-in control on screen, the post-call shot must have the line that marks the end of
    a conversation -- and a state whose expectations do not hold FAILS BY NAME. Nothing approximate
    is ever captured and labelled as the state; see `Unreachable`.
  * A picture of a page that never rendered. A white rectangle is a valid PNG. Every capture is
    decoded and judged: real dimensions, a real byte count, more than a handful of distinct
    colours, and no single colour covering essentially the whole frame. See `judge_capture`, whose
    thresholds each have a control in --self-test.

The controls are the point. `--self-test` runs them offline, with no browser and no server, and
they are written to go RED when the check they guard is weakened -- that has been verified by
breaking each check in turn.

NO VENDOR CONVERSATION, NO MICROPHONE, NO MONEY
===============================================

The states that need a live call -- talking, muted, the moment after a hang-up -- are reached
without ElevenLabs being involved at all:

  * The conversation WebSocket is replaced, before any page script runs, by a fake that opens,
    accepts frames, and delivers agent and user turns on demand (`STUB_JS`). web/voice.js sees the
    class it always sees; nothing dials out.
  * The mint request to THIS server's /api/v1/signed-url is intercepted and answered with a
    plausible signed URL, so `mintSignedUrl()` runs its real code path and the fake socket is
    handed a URL exactly as the real one would be.
  * The microphone is Chromium's built-in fake capture device, so `getUserMedia`, the AudioContext
    and the real capture graph all run for real, with no hardware and no permission prompt.

Everything else is real: a real vibe-talk server (started by run.sh with --fake-discord), the real
/api/v1/client-config, and the real channel read behind the Discord tab.

THE ONE THING THIS CANNOT SHOW YOU
==================================

Safe-area insets. The page declares `viewport-fit=cover` and pads with `env(safe-area-inset-*)`,
and no browser automation API can make Chromium report a non-zero inset -- it has no such control.
So the notch and home-indicator padding are NOT visually verified here; those `env()` terms resolve
to zero and the captures show the page as it would sit on a phone with no cutouts. What is checked
is that the declaration is still in the served markup (`expect_viewport_meta`). A short phone
profile is captured alongside the tall one because that, not the inset, is where the clipping the
owner photographed actually shows up.

USAGE
=====

    scripts/screenshots.py --url URL --channel SNOWFLAKE [--out DIR] [--only STATE]...
    scripts/screenshots.py --self-test        offline; runs every control, needs no browser.

The write-scope token comes from $VIBE_TALK_WRITE_TOKEN. It is never accepted on the command line,
because a command line is readable by every process on the box, and it is never printed.

Normally you get here through the launcher, which starts a throwaway server for you and stops it
again:

    scripts/run.sh --screenshots

EXIT CODES -- a failure says WHICH failure
==========================================

     0  captured everything
     2  usage or configuration error
    30  playwright_missing   the package or the browser binary is not installed (names the fix)
    31  unreachable          a state could not be reached (names the state and the expectation)
    32  trivial_capture      a screenshot was blank, tiny, or a single flat colour
    33  server_unreachable   the --url given does not answer
    34  control_failed       --self-test: a control failed, so the checks cannot be trusted
"""

from __future__ import annotations

import argparse
import inspect
import json
import multiprocessing
import os
import pathlib
import struct
import sys
import time
import zlib
from collections import Counter
from concurrent.futures import Executor, Future, ProcessPoolExecutor
from itertools import accumulate
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Callable, Iterable, Iterator, Literal, TypedDict, cast
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen

if TYPE_CHECKING:
    from playwright.sync_api import Browser, Page, Playwright, Route

EXIT_OK = 0
EXIT_USAGE = 2
EXIT_PLAYWRIGHT_MISSING = 30
EXIT_UNREACHABLE = 31
EXIT_TRIVIAL_CAPTURE = 32
EXIT_SERVER_UNREACHABLE = 33
EXIT_CONTROL_FAILED = 34

INSTALL_HINT = (
    "python3 -m pip install --user playwright && python3 -m playwright install chromium"
)
BROWSER_INSTALL_HINT = "python3 -m playwright install chromium"


# =================================================================================================
# PNG: read it ourselves.
#
# Judging a capture means looking at its PIXELS, and the saved file is the thing the reader will
# open, so the saved file is the thing that gets judged -- not a smaller copy taken alongside it,
# which is how a harness ends up certifying an image nobody has.
#
# Decoding is done here, in the standard library, rather than through Pillow or numpy. A byte at a
# time in Python that took about 1.8 seconds a phone capture, and the walk judges well over a hundred
# captures one after another, so the common cases are done a whole row at a time instead
# (`test-time-vibe-talk`): `Up`, which is most of the rows a browser writes, as one wide integer
# addition that keeps each byte's carry to itself, `Sub` as a running sum per channel, and the
# pixels cut out of a row by slicing. A 1179x2556 phone capture now decodes in under a second, and
# the walk does not wait for it (`start_capture`). `--self-test` builds its own PNGs with
# `encode_png` below and deliberately varies the row filter, so the decoder's five filter cases are
# each exercised offline.
# =================================================================================================

PNG_MAGIC = b"\x89PNG\r\n\x1a\n"
CHANNELS_FOR_COLOUR_TYPE = {0: 1, 2: 3, 4: 2, 6: 4}


class BadPng(Exception):
    """The bytes are not a PNG this script can read."""


@dataclass(frozen=True)
class Image:
    width: int
    height: int
    pixels: list[tuple[int, ...]]


def _png_chunks(data: bytes) -> tuple[bytes, bytes]:
    """Return (IHDR body, concatenated IDAT body)."""
    if not data.startswith(PNG_MAGIC):
        raise BadPng("not a PNG (the 8-byte signature is wrong or the file is empty)")
    header = b""
    idat = bytearray()
    at = len(PNG_MAGIC)
    while at + 8 <= len(data):
        (length,) = struct.unpack(">I", data[at : at + 4])
        kind = data[at + 4 : at + 8]
        body = data[at + 8 : at + 8 + length]
        if len(body) != length:
            raise BadPng(f"truncated {kind.decode('ascii', 'replace')} chunk")
        if kind == b"IHDR":
            header = body
        elif kind == b"IDAT":
            idat += body
        at += 12 + length
    if not header:
        raise BadPng("no IHDR chunk")
    if not idat:
        raise BadPng("no image data (no IDAT chunk)")
    return header, bytes(idat)


_ROW_MASKS: dict[int, tuple[int, int]] = {}


def _add_rows(line: bytes | bytearray, previous: bytes | bytearray) -> bytearray:
    """Each byte of `line` plus the byte above it, modulo 256: the `Up` filter, a row at a time.

    Both rows as one integer each. The low seven bits of every byte are added together, which can
    carry into that byte's top bit but never out of it, and the top bit is then the exclusive or of
    the two top bits and that carry; a carry out of the top bit, which modulo 256 drops, is never
    made.
    """
    size = len(line)
    masks = _ROW_MASKS.get(size)
    if masks is None:
        masks = _ROW_MASKS.setdefault(
            size, (int.from_bytes(b"\x7f" * size, "little"), int.from_bytes(b"\x80" * size, "little"))
        )
    low, high = masks
    a = int.from_bytes(line, "little")
    b = int.from_bytes(previous, "little")
    return bytearray((((a & low) + (b & low)) ^ ((a ^ b) & high)).to_bytes(size, "little"))


def _png_rows(data: bytes) -> tuple[int, int, int, Iterator[bytearray]]:
    """Width, height, channels, and the rows of an 8-bit non-interlaced PNG, each reconstructed as
    it is asked for. That is what every browser writes."""
    header, idat = _png_chunks(data)
    width, height, depth, colour_type, _compression, _filter, interlace = struct.unpack(
        ">IIBBBBB", header[:13]
    )
    if depth != 8:
        raise BadPng(f"only 8-bit samples are supported, this one is {depth}-bit")
    if interlace != 0:
        raise BadPng("interlaced PNGs are not supported")
    if colour_type not in CHANNELS_FOR_COLOUR_TYPE:
        raise BadPng(f"unknown colour type {colour_type}")
    channels = CHANNELS_FOR_COLOUR_TYPE[colour_type]
    stride = width * channels
    raw = zlib.decompress(idat)
    expected = height * (stride + 1)
    if len(raw) < expected:
        raise BadPng(f"image data is short: {len(raw)} bytes, expected {expected}")
    return width, height, channels, _unfiltered(raw, height, stride, channels)


def _unfiltered(raw: bytes, height: int, stride: int, channels: int) -> Iterator[bytearray]:
    """Each row of `raw` with its row filter undone, in order."""
    previous = bytearray(stride)
    at = 0
    for row in range(height):
        kind = raw[at]
        at += 1
        line = bytearray(raw[at : at + stride])
        at += stride
        if kind == 0:
            pass
        elif kind == 1:
            for channel in range(channels):
                line[channel::channels] = bytes(
                    map((0xFF).__and__, accumulate(line[channel::channels]))
                )
        elif kind == 2:
            line = _add_rows(line, previous)
        elif kind == 3:
            # The first pixel has nothing to its left, which counts as zero.
            for x in range(min(channels, stride)):
                line[x] = (line[x] + (previous[x] >> 1)) & 0xFF
            for x in range(channels, stride):
                line[x] = (line[x] + ((line[x - channels] + previous[x]) >> 1)) & 0xFF
        elif kind == 4:
            # The first pixel has nothing to its left or above-left, so the nearest is the byte above.
            for x in range(min(channels, stride)):
                line[x] = (line[x] + previous[x]) & 0xFF
            for x in range(channels, stride):
                left = line[x - channels]
                up = previous[x]
                up_left = previous[x - channels]
                # The distances of the estimate `left + up - up_left` from each of the three,
                # written out without a call to abs(), which this loop makes millions of times.
                da = up - up_left
                db = left - up_left
                dc = da + db
                if da < 0:
                    da = -da
                if db < 0:
                    db = -db
                if dc < 0:
                    dc = -dc
                if da <= db and da <= dc:
                    nearest = left
                elif db <= dc:
                    nearest = up
                else:
                    nearest = up_left
                line[x] = (line[x] + nearest) & 0xFF
        else:
            raise BadPng(f"unknown row filter {kind} on row {row}")
        yield line
        previous = line


def _row_pixels(line: bytearray, channels: int) -> Iterator[tuple[int, ...]]:
    """The pixels of one reconstructed row, as tuples of their samples."""
    return zip(*(line[channel::channels] for channel in range(channels)))


def decode_png(data: bytes) -> Image:
    """Decode an 8-bit non-interlaced PNG. That is what every browser writes."""
    width, height, channels, rows = _png_rows(data)
    pixels: list[tuple[int, ...]] = []
    for line in rows:
        pixels.extend(_row_pixels(line, channels))
    return Image(width=width, height=height, pixels=pixels)


def encode_png(width: int, height: int, rows: list[list[tuple[int, int, int]]]) -> bytes:
    """Write an 8-bit RGB PNG, varying the row filter so a decoder cannot pass on filter 0 alone.

    Only --self-test uses this. Varying the filter is the point: browser output uses all five, and
    a decoder that handles only the trivial case would otherwise sail through the controls and then
    misjudge every real capture.
    """
    raw = bytearray()
    previous = bytearray(width * 3)
    for y, row in enumerate(rows):
        line = bytearray()
        for pixel in row:
            line += bytes(pixel)
        kind = y % 5
        encoded = bytearray(len(line))
        for x in range(len(line)):
            left = line[x - 3] if x >= 3 else 0
            up = previous[x]
            up_left = previous[x - 3] if x >= 3 else 0
            if kind == 0:
                predictor = 0
            elif kind == 1:
                predictor = left
            elif kind == 2:
                predictor = up
            elif kind == 3:
                predictor = (left + up) >> 1
            else:
                estimate = left + up - up_left
                da, db, dc = (
                    abs(estimate - left),
                    abs(estimate - up),
                    abs(estimate - up_left),
                )
                if da <= db and da <= dc:
                    predictor = left
                elif db <= dc:
                    predictor = up
                else:
                    predictor = up_left
            encoded[x] = (line[x] - predictor) & 0xFF
        raw.append(kind)
        raw += encoded
        previous = line

    def chunk(kind: bytes, body: bytes) -> bytes:
        return (
            struct.pack(">I", len(body))
            + kind
            + body
            + struct.pack(">I", zlib.crc32(kind + body) & 0xFFFFFFFF)
        )

    header = struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0)
    return (
        PNG_MAGIC
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 6))
        + chunk(b"IEND", b"")
    )


# =================================================================================================
# Is this picture worth looking at?
#
# Each threshold below is one way a capture can be technically valid and evidentially worthless. A
# rejection names which one, because "screenshot looked wrong" is not something the next reader can
# act on.
# =================================================================================================

MIN_BYTES = 1_000
MIN_EDGE = 200
MIN_DISTINCT_COLOURS = 16
# A frame where one colour covers essentially everything is a page that did not render, or rendered
# into the wrong place. Real captures of this page sit far below this: the emptiest state measured
# is the sign-in screen at about 0.93.
MAX_DOMINANT_SHARE = 0.995


@dataclass(frozen=True)
class Verdict:
    ok: bool
    reason: str
    width: int = 0
    height: int = 0
    distinct: int = 0
    dominant_share: float = 0.0

    def describe(self) -> str:
        return (
            f"{self.width}x{self.height}, {self.distinct}+ colours, "
            f"largest flat area {self.dominant_share:.1%}"
        )


def judge_capture(data: bytes) -> Verdict:
    """Decide whether a screenshot shows anything. See MIN_* above for what each rejection means."""
    if len(data) < MIN_BYTES:
        return Verdict(
            False,
            f"the file is {len(data)} bytes, under the {MIN_BYTES}-byte floor — nothing rendered",
        )
    # Counted a row at a time rather than through `decode_png`'s list of every pixel, which for a
    # phone capture is millions of tuples held at once in each judging process.
    counts: Counter[tuple[int, ...]] = Counter()
    try:
        width, height, channels, rows = _png_rows(data)
        for line in rows:
            counts.update(_row_pixels(line, channels))
    except BadPng as error:
        return Verdict(False, f"the file is not a readable PNG: {error}")
    if width < MIN_EDGE or height < MIN_EDGE:
        return Verdict(
            False,
            f"the frame is {width}x{height}; both edges must be at least {MIN_EDGE}px",
            width=width,
            height=height,
        )
    distinct = len(counts)
    total = width * height
    dominant = counts.most_common(1)[0][1] / total if total else 1.0
    if distinct < MIN_DISTINCT_COLOURS:
        return Verdict(
            False,
            f"the frame has only {distinct} distinct colour(s) — it is blank or a flat fill, "
            f"not a rendered page",
            width=width,
            height=height,
            distinct=distinct,
            dominant_share=dominant,
        )
    if dominant > MAX_DOMINANT_SHARE:
        return Verdict(
            False,
            f"one colour covers {dominant:.2%} of the frame — the page is effectively blank "
            f"even though {distinct} colours are present",
            width=width,
            height=height,
            distinct=distinct,
            dominant_share=dominant,
        )
    return Verdict(
        True,
        "shows a rendered page",
        width=width,
        height=height,
        distinct=distinct,
        dominant_share=dominant,
    )


class TrivialCapture(Exception):
    """A screenshot was written that shows nothing. Named so the reader knows which one."""

    def __init__(self, label: str, verdict: Verdict) -> None:
        super().__init__(
            f"the capture for '{label}' shows nothing: {verdict.reason}. "
            f"A blank frame is worse than no frame, because it looks like evidence."
        )
        self.label = label
        self.verdict = verdict


def _judge_fields(data: bytes) -> tuple[bool, str, int, int, int, float]:
    """`judge_capture` as plain values, which is what crosses back from a judging process."""
    verdict = judge_capture(data)
    return (
        verdict.ok,
        verdict.reason,
        verdict.width,
        verdict.height,
        verdict.distinct,
        verdict.dominant_share,
    )


class PendingCapture:
    """A capture already on disk, whose judgement may still be running in another process."""

    def __init__(
        self, label: str, judged: Future[tuple[bool, str, int, int, int, float]]
    ) -> None:
        self.label = label
        self._judged = judged

    def done(self) -> bool:
        return self._judged.done()

    def verdict(self) -> Verdict:
        """The judgement, waited for if need be. A frame that shows nothing raises, by name."""
        verdict = Verdict(*self._judged.result())
        if not verdict.ok:
            raise TrivialCapture(self.label, verdict)
        return verdict


def start_capture(
    path: Path, data: bytes, label: str, judges: Executor | None = None
) -> PendingCapture:
    """Write a capture and start judging it. The write comes first, so a rejected frame can be
    looked at.

    This is the only function that puts a screenshot on disk, which is what makes the blank check
    impossible to route around: there is no second path that writes an unjudged file. With
    `judges`, the judgement runs there while the walk goes on to the next state -- decoding a
    capture is about a second of pure Python, and the walk is mostly waiting on the page, so done
    in line it was over a third of the walk (`test-time-vibe-talk`). The walk collects every
    verdict before it calls a profile done, so a blank frame still fails the run by name.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    judged: Future[tuple[bool, str, int, int, int, float]]
    if judges is None:
        judged = Future()
        judged.set_result(_judge_fields(data))
    else:
        judged = judges.submit(_judge_fields, data)
    return PendingCapture(label, judged)


def record_capture(path: Path, data: bytes, label: str) -> Verdict:
    """Write a capture and judge it before returning: `start_capture`, waited for."""
    return start_capture(path, data, label).verdict()


# =================================================================================================
# Reaching a state, and refusing to photograph one we did not reach.
# =================================================================================================


#: `#472 gate-flake-hardening`. Every explicit wait in the walk (`wait_for_function(...,
#: timeout=...)`) is a deadline for a state the page is to reach, never a measure of how fast it gets
#: there, so each is stretched by this much: on a host running several validation gates at once a
#: fresh load took more than the 10 s it was given to show the control pane. A state that is never
#: reached still fails, only later.
WAIT_PATIENCE = 3

#: How long a scene's expectations are looked at again, all together, before it is called unreached.
EXPECT_WAIT_S = 5.0


def _patient_waits(page: Page) -> None:
    """Stretch every explicit `wait_for_function` deadline on `page` by `WAIT_PATIENCE`."""
    wait = page.wait_for_function

    def patient(
        expression: str,
        *,
        arg: object = None,
        timeout: float | None = None,
        polling: float | Literal["raf"] | None = None,
    ) -> object:
        stretched = None if timeout is None else timeout * WAIT_PATIENCE
        return wait(expression, arg=arg, timeout=stretched, polling=polling)

    setattr(page, "wait_for_function", patient)


class Unreachable(Exception):
    """A state's expectations did not hold, so nothing was captured for it."""

    def __init__(self, state: str, expectation: str, detail: str = "") -> None:
        message = (
            f"state '{state}' was NOT reached: {expectation}. "
            f"Nothing was captured for '{state}' — an approximate picture under that name would be "
            f"a false record of what the page does."
        )
        if detail:
            message += f" ({detail})"
        super().__init__(message)
        self.state = state
        self.expectation = expectation


class PlaywrightMissing(Exception):
    """Playwright, or its browser binary, is not installed. Carries the exact install command."""

    def __init__(self, what: str, command: str) -> None:
        super().__init__(f"{what}\n\nInstall it with:\n    {command}")
        self.command = command


class ServerUnreachable(Exception):
    """The --url given does not answer."""


# The stubs, installed before ANY page script runs.
#
# Only two things are replaced, and both are the vendor boundary: the conversation socket and
# nothing else. The microphone is Chromium's own fake capture device (a launch flag, below), so
# getUserMedia, the AudioContext and the whole capture graph run for real.
STUB_JS = r"""
(() => {
  // A stand-in for the ElevenLabs conversation socket. web/voice.js constructs `new WebSocket(url)`
  // and uses onopen/onmessage/onclose/send/close/readyState and the static OPEN constant; all of
  // that is here, and nothing here reaches the network.
  class FakeConversationSocket {
    constructor(url) {
      this.url = url;
      this.readyState = FakeConversationSocket.CONNECTING;
      this.sent = [];
      // `#331 resume-on-websocket`. A URL naming `shot-v1` is a `vibe-talk-v1` bridge that takes a
      // recap: it lists `replay`, and acknowledges the record. It counts what it was sent, and what
      // came first, so a state can hold the page to sending the record once and before anything.
      this.v1 = String(url).includes("shot-v1");
      this.v1Replays = 0;
      this.v1First = null;
      window.__socket = this;
      setTimeout(() => {
        if (this.readyState !== FakeConversationSocket.CONNECTING) return;
        this.readyState = FakeConversationSocket.OPEN;
        if (this.onopen) this.onopen({});
        if (this.v1) {
          this.__deliver({ type: "session_started", greeting: false, capabilities: ["replay"] });
          return;
        }
        this.__deliver({
          type: "conversation_initiation_metadata",
          conversation_initiation_metadata_event: {
            conversation_id: "conv_screenshot_0001",
            agent_output_audio_format: "pcm_16000",
          },
        });
      }, 10);
    }
    send(data) {
      if (this.v1 && typeof data === "string") {
        let frame = null;
        try {
          frame = JSON.parse(data);
        } catch (e) {
          frame = null;
        }
        if (frame && this.v1First === null) this.v1First = frame.type;
        if (frame && frame.type === "replay") {
          this.v1Replays += 1;
          setTimeout(() => this.__deliver({ type: "replay_ack", accepted: true }), 30);
        }
      }
      // Capped: the real capture graph pushes a frame every few milliseconds and this array is
      // only ever read to prove that mute stops it.
      this.sent.push(String(data).slice(0, 40));
      if (this.sent.length > 8) this.sent.splice(0, this.sent.length - 8);
    }
    close(code, reason) {
      if (this.readyState === FakeConversationSocket.CLOSED) return;
      this.readyState = FakeConversationSocket.CLOSED;
      if (this.onclose) this.onclose({ code: code || 1000, reason: reason || "" });
    }
    addEventListener() {}
    removeEventListener() {}
    __deliver(message) {
      if (this.readyState !== FakeConversationSocket.OPEN || !this.onmessage) return;
      this.onmessage({ data: JSON.stringify(message) });
    }
  }
  FakeConversationSocket.CONNECTING = 0;
  FakeConversationSocket.OPEN = 1;
  FakeConversationSocket.CLOSING = 2;
  FakeConversationSocket.CLOSED = 3;
  window.WebSocket = FakeConversationSocket;

  window.__agentSays = (text) =>
    window.__socket &&
    window.__socket.__deliver({ type: "agent_response", agent_response_event: { agent_response: text } });
  window.__userSays = (text) =>
    window.__socket &&
    window.__socket.__deliver({
      type: "user_transcript",
      user_transcription_event: { user_transcript: text },
    });
  // The same on the `vibe-talk-v1` bridge above: an assistant turn, whole.
  window.__v1Says = (text, turn) => {
    if (!window.__socket) return false;
    window.__socket.__deliver({ type: "transcript", role: "assistant", text, turn });
    window.__socket.__deliver({ type: "turn_complete", turn });
    return true;
  };

  // `#54 resume-recovery`. There is no way to really background a Playwright page -- the browser
  // stays visible and `visibilitychange` never fires -- so the two facts the page reads are
  // redefined on the document and the event is dispatched by hand. Everything downstream of that
  // is the page's own code: it classifies the close itself, and nothing here touches the panel or
  // the controls.
  const __setVisibility = (value) => {
    Object.defineProperty(document, "visibilityState", {
      configurable: true,
      get: () => value,
    });
    Object.defineProperty(document, "hidden", {
      configurable: true,
      get: () => value === "hidden",
    });
    document.dispatchEvent(new Event("visibilitychange"));
  };
  window.__suspend = () => {
    __setVisibility("hidden");
    // Exactly what iOS does to a socket belonging to a page it has suspended: an error, then an
    // abnormal close with no reason at all.
    if (window.__socket) {
      if (window.__socket.onerror) window.__socket.onerror({});
      window.__socket.close(1006, "");
    }
  };
  window.__resume = () => __setVisibility("visible");
  // A failure with the page in the FOREGROUND, which is the one case that is still a failure.
  window.__fail = () => {
    if (window.__socket && window.__socket.onerror) window.__socket.onerror({});
  };

  // Visibility as the eye sees it, not as `offsetParent` reports it: `offsetParent` is null for a
  // fixed-position element, which would make a perfectly visible control read as absent.
  window.__visible = (id) => {
    const node = document.getElementById(id);
    if (!node) return false;
    const box = node.getBoundingClientRect();
    if (box.width <= 0 || box.height <= 0) return false;
    const style = getComputedStyle(node);
    return style.visibility !== "hidden" && style.display !== "none" && style.opacity !== "0";
  };
  window.__text = (id) => {
    const node = document.getElementById(id);
    return node ? (node.textContent || "").trim() : null;
  };

  // `#44 live-push`. TWO things are stubbed here and nothing else, and both are about the SERVER
  // rather than about the page:
  //
  //   * The throwaway server this harness starts has live ingestion OFF -- that is the default,
  //     and it is the default for a reason (see src/live.rs). So the client-config answer is
  //     patched to say the server IS watching, which is the only thing that answer is used for.
  //   * The stream route is served from here, as a real ReadableStream this script feeds. The
  //     server runs with --fake-discord, so no real message can ever arrive in a channel and no
  //     poll tick could ever publish one; waiting for a real interval would be waiting for
  //     nothing.
  //
  // Everything downstream is the page's own code: the frame parsing, the de-duplication, the
  // rendering through discordNode, and the relay guards.
  const __realFetch = window.fetch.bind(window);
  let __liveController = null;
  window.__streamOpens = 0;
  window.fetch = async (input, init) => {
    const url = String(typeof input === "string" ? input : (input && input.url) || "");
    if (url.includes("/api/v1/client-config")) {
      const response = await __realFetch(input, init);
      const text = await response.text();
      let body = {};
      try {
        body = JSON.parse(text);
      } catch (e) {
        body = {};
      }
      body.live_poll_seconds = 30;
      // ...and SAY it is pushing. `live_poll_seconds` alone used to imply a stream; the page now
      // reads `live_delivery` and falls back to "poll" -- a server that re-reads on a timer and
      // tells the page nothing -- so a stub that sets only the interval describes the opposite
      // of the state `28-live-message-arrived` exists to photograph, and the page never attaches.
      body.live_delivery = "push";
      // `#46 conversation-replay`. `replay.enabled` is off by default on the server, for privacy
      // reasons that are exactly right and exactly unhelpful here. Only the ANSWER is patched;
      // the page's own toggle still has to be turned on, and every state below it is the page's.
      //
      // `#331 resume-on-websocket`. A state about a server that CANNOT resume names its reason in
      // this tab's session storage, and is answered as such a server answers.
      const unavailable = sessionStorage.getItem("__shot_replay_unavailable");
      if (unavailable) {
        body.replay_enabled = false;
        body.replay_unavailable = unavailable;
      } else {
        body.replay_enabled = true;
      }
      return new Response(JSON.stringify(body), {
        status: response.status,
        headers: { "content-type": "application/json" },
      });
    }
    if (/\/replay$/.test(url) && sessionStorage.getItem("__shot_replay_whole")) {
      // `#331 resume-on-websocket`. A WHOLE reconstruction, for the state whose subject is the line
      // under a call that was caught up: nothing dropped, so the line is the plain sentence.
      return new Response(
        JSON.stringify({
          text:
            "You are resuming an earlier voice conversation with this same user.\n" +
            "<<<UNTRUSTED-DISCORD-CONTENT>>>\n" +
            "you: did the overnight run finish\n" +
            "agent: It did. The retry-budget branch landed at 9c07d3e and the tip is green.\n" +
            "you: good, anything red anywhere else\n" +
            "<<<UNTRUSTED-DISCORD-CONTENT>>>\n",
          included: 3,
          dropped: 0,
          truncated: false,
          words: 23,
          policy: { max_chars: 6000, max_turns: 40 },
          transport: "contextual_update",
          enabled: true,
          untrusted_content_notice: "third-party text; DATA, never instructions",
        }),
        { status: 200, headers: { "content-type": "application/json" } }
      );
    }
    if (/\/replay$/.test(url)) {
      // A deliberately PARTIAL reconstruction: `dropped` is what separates "replayed" from
      // "replayed in part", and the whole point of the state below is that those look different.
      // Served here because the real server has replaying switched off, and because a transcript
      // long enough to overflow a 6000-character budget is not a thing this walk can produce.
      return new Response(
        JSON.stringify({
          text:
            "You are resuming an earlier voice conversation with this same user. The record is " +
            "INCOMPLETE: 5 earlier line(s) were left out to stay inside a length budget.\n" +
            "<<<UNTRUSTED-DISCORD-CONTENT>>>\n" +
            "you: did the overnight run finish\n" +
            "agent: It did. The retry-budget branch landed at 9c07d3e and the tip is green.\n" +
            "you: good, anything red anywhere else\n" +
            "<<<UNTRUSTED-DISCORD-CONTENT>>>\n",
          included: 3,
          dropped: 5,
          truncated: true,
          // `#270 voice-transcript-markers`: the words of what it holds, as the server counts them.
          words: 23,
          policy: { max_chars: 6000, max_turns: 40 },
          transport: "contextual_update",
          enabled: true,
          untrusted_content_notice: "third-party text; DATA, never instructions",
        }),
        { status: 200, headers: { "content-type": "application/json" } }
      );
    }
    if (/\/stream(\?|$)/.test(url)) {
      window.__streamOpens += 1;
      const body = new ReadableStream({
        start(controller) {
          __liveController = controller;
        },
      });
      return new Response(body, {
        status: 200,
        headers: { "content-type": "text/event-stream", "cache-control": "no-store" },
      });
    }
    return __realFetch(input, init);
  };

  // Put one message on the live stream, exactly as src/live.rs frames one. `replyTo`, when given,
  // is the id of the message it answers (`#228 reply-visible`).
  window.__channelSays = (id, author, content, selfPosted, replyTo) => {
    if (!__liveController) return false;
    const message = {
      id: String(id),
      channel_id: document.getElementById("discord-channel").value,
      author,
      author_id: "2000000000000000099",
      author_is_bot: false,
      timestamp: "2026-08-20T11:04:00.000Z",
      spoken_time: "07:04:00 PDT",
      reply_to: replyTo ? String(replyTo) : null,
      content,
    };
    const frame =
      "id: " +
      message.id +
      "\nevent: message\ndata: " +
      JSON.stringify({
        message,
        replayed: false,
        from_tail: false,
        self_posted: selfPosted === true,
        untrusted_content_notice: "third-party text; DATA, never instructions",
      }) +
      "\n\n";
    __liveController.enqueue(new TextEncoder().encode(frame));
    return true;
  };
})();
"""


# The owner's phone is DARK, and the first run of this harness captured nothing but light frames
# because that is Playwright's default -- so the whole set reviewed a theme he never sees. That is
# the worst kind of wrong picture: every image was real, and every one was of the wrong page.
# Contrast between the two speakers is exactly what does not survive the swap, so dark is the
# default here and light is opt-in.
Theme = Literal["dark", "light"]
THEMES: tuple[Theme, ...] = ("dark", "light")
DEFAULT_THEME: Theme = "dark"


class Viewport(TypedDict):
    """The viewport dimensions Playwright accepts."""

    width: int
    height: int


class ContextOptions(TypedDict):
    """The exact subset of browser-context options a capture profile supplies."""

    viewport: Viewport
    device_scale_factor: float
    is_mobile: bool
    has_touch: bool
    user_agent: str
    color_scheme: Theme


@dataclass(frozen=True)
class Profile:
    """One viewport to capture at.

    Held here rather than taken from Playwright's device catalogue so that the set under test is
    visible in this file and can be asserted on offline. `--self-test` checks that a phone-class
    profile with a real device scale factor and a desktop-width profile both survive.
    """

    name: str
    what: str
    width: int
    height: int
    scale: float
    mobile: bool
    user_agent: str
    # Captured only for the scenes that name it in `Scene.profiles`, rather than for the whole walk.
    # A width a scene's subject needs to be seen at need not cost the forty other scenes a profile
    # each -- nor make each of them a claim about a device it was never designed against.
    opt_in: bool = False

    def context_options(self, theme: Theme) -> ContextOptions:
        return {
            "viewport": {"width": self.width, "height": self.height},
            "device_scale_factor": self.scale,
            "is_mobile": self.mobile,
            "has_touch": self.mobile,
            "user_agent": self.user_agent,
            # The page has no theme switch: it follows the OS through
            # `@media (prefers-color-scheme: light)`, so this is the only way to reach either one.
            "color_scheme": theme,
        }


IPHONE_UA = (
    "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 "
    "(KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1"
)
DESKTOP_UA = (
    "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) "
    "Chrome/140.0.0.0 Safari/537.36"
)
ANDROID_UA = (
    "Mozilla/5.0 (Linux; Android 14; Pixel 8) AppleWebKit/537.36 (KHTML, like Gecko) "
    "Chrome/140.0.0.0 Mobile Safari/537.36"
)

PROFILES: tuple[Profile, ...] = (
    Profile(
        name="iphone15",
        what="iPhone 15 — the phone this is actually used on, portrait",
        width=393,
        height=852,
        scale=3.0,
        mobile=True,
        user_agent=IPHONE_UA,
    ),
    Profile(
        # The short phone is where the clipping the owner photographed shows up first: the frame is
        # a three-row grid and the middle row is what gives way when there is less of it.
        name="iphone-se",
        what="iPhone SE — the SHORT phone, where vertical clipping shows first",
        width=375,
        height=667,
        scale=2.0,
        mobile=True,
        user_agent=IPHONE_UA,
    ),
    Profile(
        name="desktop",
        what="desktop width, for the layout at the other end of the range",
        width=1440,
        height=900,
        scale=1.0,
        mobile=False,
        user_agent=DESKTOP_UA,
    ),
    Profile(
        # `#55 voice-desktop-app`. One desktop width proves nothing about a reading column: the
        # column is capped, so at 1440 and at 1280 the transcript is the SAME width and only the
        # margin beside it changes. That is the property, and it takes two widths to see it.
        # 1280x800 is also the smallest window most people ever open, and it is just over the
        # 900px threshold where the desktop regime starts.
        name="laptop-1280",
        what="a small laptop window — the narrow end of the desktop regime",
        width=1280,
        height=800,
        scale=1.0,
        mobile=False,
        user_agent=DESKTOP_UA,
    ),
    Profile(
        # `#217 markdown-blocks`. The Android width the offline-cache suite also uses, for the one
        # state whose subject is how message text lays out at a phone's width: a bullet's hanging
        # indent and the gap at a blank line are exactly what a few pixels of width change.
        name="android-412",
        what="a 412px Android phone, for message text only",
        width=412,
        height=915,
        scale=2.625,
        mobile=True,
        user_agent=ANDROID_UA,
        opt_in=True,
    ),
    Profile(
        # The owner's Settings batch of 2026-10-18 (`#345 settings-multi-column` to `#349
        # channel-settings-all`) asks to be seen at a full-HD desk as well as the small one: that is
        # where Settings has room for more than two columns. Opt-in like the Android width, so only a
        # scene that names it runs here.
        name="desk-1920",
        what="a full-HD desktop window, 1920x1080",
        width=1920,
        height=1080,
        scale=1.0,
        mobile=False,
        user_agent=DESKTOP_UA,
        opt_in=True,
    ),
)

# `#345 settings-multi-column` and the rest of the owner's Settings batch: the phone he uses, the small
# desk and the full-HD one.
SETTINGS_PROFILES = ("iphone15", "laptop-1280", "desk-1920")

# The profiles where `@media (min-width: 900px) and (pointer: fine)` is in force. A scene about the
# desktop composition is meaningless on a phone, and worse than meaningless if it PASSES there.
DESKTOP_PROFILES = ("desktop", "laptop-1280")

# ...and the phones, for the states that only go wrong when the screen is small. Kept beside the
# desktop set so the two are visibly the same mechanism rather than one rule and one exception.
PHONE_PROFILES = ("iphone15", "iphone-se")


@dataclass(frozen=True)
class Scene:
    """One state to photograph.

    `act` walks the page into the state. `expect` is what must be TRUE of the live page before the
    shutter opens: a description a human can read, and a JavaScript expression that must evaluate
    truthy. Every scene must carry at least one, or a navigation failure yields a confidently named
    picture of the previous screen.
    """

    name: str
    what: str
    act: Callable[["Driver"], None]
    expect: tuple[tuple[str, str], ...]
    freeze_animations: bool = True
    animation_offset_ms: int | None = None
    # Which profiles this state means anything on; None is all of them. `#55 voice-desktop-app`
    # added it because a resized reading column does not exist on a phone -- there is no column and
    # no pointer to drag its edge -- so running the scene there would either fail confusingly or,
    # far worse, PASS and be filed as evidence that a phone has a desktop layout.
    profiles: tuple[str, ...] | None = None
    # Run after the capture, to put back what the walk changed ON THE SERVER. The profiles share one
    # server, so a message one profile's walk marked Done would be drawn read in every later
    # profile's pictures, under names that do not say so. `#245 done-feedback` added it for its Done.
    after: Callable[["Driver"], None] | None = None


class Driver:
    """Walks one browser page through the scenes, capturing as it goes."""

    def __init__(
        self,
        page: Page,
        base_url: str,
        token: str,
        out_dir: Path,
        profile: Profile,
        theme: Theme,
        mock_control: str = "",
        judges: Executor | None = None,
    ) -> None:
        self.theme = theme
        self.page = page
        # The offline ElevenLabs mock's `/_mock/` control plane, or "" when the run was not given
        # one. Empty is not the same as "skip": the states that need it say so BY NAME rather than
        # quietly not being captured, because a summary state that silently disappeared from a run
        # is exactly the kind of missing evidence this harness exists to make impossible.
        self.mock_control = mock_control.rstrip("/")
        # A page whose script threw on load still RENDERS: the markup is there, the sign-in screen
        # looks entirely plausible, and every control is inert. That is a photograph of a broken
        # page filed as a photograph of a working one, and it is a live hazard here -- web/voice.js
        # is compiled in beside web/voice.html, so an edit to one and not the other produces
        # exactly this. Collected per page and reported by name at the first capture.
        self.page_errors: list[str] = []
        page.on("pageerror", lambda error: self.page_errors.append(str(error).splitlines()[0]))
        _patient_waits(page)
        self.base_url = base_url.rstrip("/")
        self.token = token
        self.out_dir = out_dir
        self.profile = profile
        # Where captures are judged (`start_capture`); None judges each before the walk goes on.
        self.judges = judges
        self.pending: list[tuple[str, Path, PendingCapture]] = []
        self.results: list[tuple[str, Path, Verdict]] = []

    # -- page-level helpers ----------------------------------------------------------------------

    def load(self, with_token: bool, placement: str | None = None) -> None:
        script = STUB_JS
        if with_token:
            script += (
                "\ntry { localStorage.setItem('vibe-talk.token', "
                + json.dumps(self.token)
                + "); } catch (e) {}\n"
            )
        else:
            script += "\ntry { localStorage.removeItem('vibe-talk.token'); } catch (e) {}\n"
        # Every page-local preference and snapshot is cleared on a load, so a self-contained state
        # really is self-contained. Without this the desktop states inherit the reading column the
        # previous one dragged out, and every later desktop frame is a picture of the wrong width --
        # which is what the first capture of the reply screen actually showed. The message snapshot
        # is state rather than a preference, but it belongs here for the same reason: scene 18 walks
        # the whole channel, while scene 21 must begin with one page so it can photograph the next
        # page arriving. Letting 21 restore 18's complete snapshot leaves no older page to request.
        # The token above is the one thing deliberately carried across, because signing in is not
        # the subject of any state after the first.
        script += (
            "\ntry {\n"
            "  localStorage.removeItem('vibe-talk.voice.width');\n"
            "  localStorage.removeItem('vibe-talk.voice.drafts');\n"
            "  localStorage.removeItem('vibe-talk.voice.bar-placement');\n"
            "  localStorage.removeItem('vibe-talk.voice.message-cache');\n"
            # `#46 conversation-replay`. It is a persisted preference like the others, and a
            # state that inherited it would photograph a resumed call under a name that does not
            # say so -- or, worse, an ordinary post-call frame carrying the resumed wording.
            "  localStorage.removeItem('vibe-talk.voice.resume');\n"
            # `#189 restore-ui-state`. Where the reader left the page is stored, and a reload
            # reopens there; a scene that loads the page must start from the default instead of
            # the view the previous scene left up. `#265 window-channel-memory`: the window keeps
            # its own copy, which a reload reads first, and every scene loads in this one tab.
            "  localStorage.removeItem('vibe-talk.voice.ui-state');\n"
            "  sessionStorage.removeItem('vibe-talk.voice.ui-state');\n"
            # `#212 link-filter`. The kinds of link a reader turned off under Links are kept, and
            # `38-link-kinds-off` turns one off: every later Links frame must start with all on.
            "  localStorage.removeItem('vibe-talk.voice.hidden-link-kinds');\n"
            # `#230 text-size-buttons`. `45-settings-text-size` presses +, and the message text of
            # every later frame must be at its usual size, not a step larger.
            "  localStorage.removeItem('vibe-talk.voice.msg-scale');\n"
            # `#193 compose-draft-loss`. `49-failed-send-and-draft` leaves a refused send in the
            # outbox and `50-send-dismissed` one among the recently dismissed: no later frame may
            # carry either, or what was typed in a channel's box.
            "  localStorage.removeItem('vibe-talk.outgoing-messages');\n"
            "  localStorage.removeItem('vibe-talk.dismissed-sends');\n"
            "  localStorage.removeItem('vibe-talk.channel-drafts');\n"
            # `#298 thread-card-header`. `55-thread-card-header` chooses two columns, and the choice
            # is kept: no later frame may open in a layout it did not choose.
            "  localStorage.removeItem('vibe-talk.voice.columns');\n"
            "  localStorage.removeItem('vibe-talk.voice.columns-before');\n"
            "} catch (e) {}\n"
        )
        # `#58 control-bar`. Where the control bar sits is a stored preference, so it is cleared
        # above with the others and SEEDED here when a state is about to be a picture of it. Seeded
        # rather than clicked, because the alternative -- walking into Settings and back out -- is
        # a different state's subject and would put the previous scene's screen in the way.
        if placement is not None:
            script += (
                "\ntry { localStorage.setItem('vibe-talk.voice.bar-placement', "
                + json.dumps(placement)
                + "); } catch (e) {}\n"
            )
        self.page.add_init_script(script)
        self.page.goto(f"{self.base_url}/", wait_until="load")

    def click(self, element_id: str) -> None:
        self.page.click(f"#{element_id}")

    def settle(self, ms: int = 250) -> None:
        self.page.wait_for_timeout(ms)

    def js(self, expression: str) -> object:
        return cast(object, self.page.evaluate(f"() => ({expression})"))

    # -- capture ---------------------------------------------------------------------------------

    def capture(self, scene: Scene) -> None:
        if self.page_errors:
            raise Unreachable(
                scene.name,
                "the page's own script threw an uncaught error, so the interface is inert no "
                "matter how it looks",
                "; ".join(dict.fromkeys(self.page_errors))[:400],
            )
        # Every expectation at once, looked at again until they all hold or `EXPECT_WAIT_S` has
        # gone by: a state a loaded host brings late is still the state, and is what is captured;
        # one never reached is reported by the first expectation that did not hold.
        deadline = time.monotonic() + EXPECT_WAIT_S
        while True:
            unmet: tuple[str, str] | None = None
            for description, expression in scene.expect:
                try:
                    held = bool(self.js(expression))
                except Exception as error:  # a broken expression is a failed expectation, loudly
                    raise Unreachable(
                        scene.name, description, f"the check itself errored: {error}"
                    ) from error
                if not held:
                    unmet = (description, expression)
                    break
            if unmet is None:
                break
            if time.monotonic() >= deadline:
                raise Unreachable(scene.name, unmet[0], f"check: {unmet[1]}")
            self.page.wait_for_timeout(100)

        animations: Literal["allow", "disabled"]
        if scene.animation_offset_ms is not None:
            # Freeze the pulse at a chosen point of its cycle rather than at whatever instant the
            # shutter happened to fall on. The ring animates from opacity 0.55 to 0, so an
            # unsynchronised capture of a LIVE call can show no ring at all and read as idle.
            self.page.evaluate(
                "(ms) => document.querySelectorAll('.control-ring').forEach((n) => "
                "n.getAnimations().forEach((a) => { a.pause(); a.currentTime = ms; }))",
                scene.animation_offset_ms,
            )
            animations = "allow"
        else:
            animations = "disabled" if scene.freeze_animations else "allow"

        path = self.out_dir / f"{self.theme}--{self.profile.name}--{scene.name}.png"
        data = self.page.screenshot(animations=animations)
        label = f"{self.theme}/{self.profile.name}/{scene.name}"
        self.pending.append((scene.name, path, start_capture(path, data, label, self.judges)))

    def judged(self, wait: bool) -> list[tuple[str, Path, Verdict]]:
        """The captures judged since the last call, in the order they were taken.

        Without `wait`, only those whose judgement has finished, stopping at the first that has
        not, so the order holds; with it, every one. A frame that shows nothing raises
        `TrivialCapture` here, naming it.
        """
        settled: list[tuple[str, Path, Verdict]] = []
        while self.pending and (wait or self.pending[0][2].done()):
            name, path, pending = self.pending.pop(0)
            settled.append((name, path, pending.verdict()))
        self.results.extend(settled)
        return settled


# -- the scenes ------------------------------------------------------------------------------------
#
# Order matters: they are a walk through one session, in the order a person moves through it, so the
# live-call states really are entered from a real idle screen by real clicks.
#
# EVERY selector here was re-derived against web/voice.html after the interface rework (ef24062).
# The old page's tab buttons, its `li.system` end-of-call paragraph and its always-present Hang up
# are all gone, and a scene left pointing at any of them would not have failed -- it would have
# captured whatever happened to be on screen and filed it under the old name. That is the failure
# this table's expectations exist to prevent, so they are the part that had to be rechecked hardest.


def _act_signed_out(driver: Driver) -> None:
    driver.load(with_token=False)
    driver.settle()


def _act_idle(driver: Driver) -> None:
    driver.load(with_token=True)
    # The main screen only appears after /api/v1/client-config answers, which is a real request to a
    # real server. Waiting on the element rather than a timeout keeps a slow box from reading as a
    # broken page.
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    # Settle past the transient message's own life. `#63 status-line-placement` is about a strip
    # that used to hold a row of a phone screen on every frame; THIS is the picture that shows it
    # gone, and a capture taken while "Ready." was still up would show the thing the issue is
    # about. Waiting on the element rather than on a timeout keeps the delay honest if the timer
    # ever changes.
    driver.page.wait_for_function(
        "() => !window.__visible('status-line')", timeout=15_000
    )
    driver.settle()


def _act_live(driver: Driver) -> None:
    driver.click("talk")
    driver.page.wait_for_function(
        "() => window.__text('talk-label') === 'Listening'", timeout=10_000
    )
    driver.js("window.__userSays('can you check whether the retry-budget branch landed')")
    driver.js(
        "window.__agentSays('It landed. integration is at 9c07d3e and the tip is green — "
        "claude-integ posted that eleven minutes ago.')"
    )
    driver.js("window.__userSays('good. anything red anywhere else')")
    driver.settle(400)


def _act_muted(driver: Driver) -> None:
    driver.click("talk")
    driver.page.wait_for_function("() => window.__text('talk-label') === 'Muted'", timeout=5_000)
    driver.settle()


def _act_speaker_silenced(driver: Driver) -> None:
    driver.click("talk")  # unmute, so this shows the speaker control alone, not two states at once
    driver.page.wait_for_function(
        "() => window.__text('talk-label') === 'Listening'", timeout=5_000
    )
    driver.click("speaker")
    driver.page.wait_for_function("() => window.__text('speaker-label') === 'Silent'", timeout=5_000)
    driver.js("window.__agentSays('Nothing else is red. The arm64 runner is still wedged.')")
    driver.settle(300)


def _act_post_call(driver: Driver) -> None:
    driver.click("speaker")  # back to Sound, so the post-call frame is not also a muted-speaker one
    driver.page.wait_for_function("() => window.__text('speaker-label') === 'Sound'", timeout=5_000)
    driver.click("hang-up")
    driver.page.wait_for_function(
        "() => window.__text('talk-label') === 'Start a new call'", timeout=5_000
    )
    driver.settle()


def _act_seam_open(driver: Driver) -> None:
    # The disclosure inside the end-of-call seam. Its first capture caught it unfolding UNDERNEATH
    # the dock, where it is invisible -- the page scrolls it into view on `toggle`, and whether that
    # actually worked is a layout fact no behavioural test can see.
    driver.page.click("#transcript > li.seam summary")
    driver.page.wait_for_function(
        "() => !!document.querySelector('#transcript > li.seam details[open]')", timeout=5_000
    )
    driver.settle(500)  # the scrollIntoView the page runs on toggle has to land before the shutter


def _act_clear_armed(driver: Driver) -> None:
    # One tap ARMS it; a second would clear. It disarms itself after about four seconds, so this
    # capture is taken promptly and deliberately does not settle for long.
    driver.click("clear-view")
    driver.page.wait_for_function(
        "() => window.__text('clear-view-label') === 'Sure?'", timeout=5_000
    )
    driver.settle(120)


# `#346 settings-collapsible`. Every section of Settings and every group in them is folded as the page
# loads, so a state about a setting opens the folds around it first, by their own buttons, outermost
# first, as a reader taps them. Returns how many folds the element is inside.
UNFOLD_JS = (
    "(id) => { const node = document.getElementById(id); const bodies = []; "
    "for (let n = node; n; n = n.parentElement) if (n.classList && n.classList.contains('fold-body')) bodies.unshift(n); "
    "for (const body of bodies) { const toggle = document.querySelector(`[aria-controls=\"${body.id}\"]`); "
    "if (toggle && toggle.getAttribute('aria-expanded') !== 'true') toggle.click(); } return bodies.length; }"
)


def _unfold(driver: Driver, *element_ids: str) -> None:
    """Open the Settings folds around each of `element_ids`, and wait for the last to be drawn."""
    for element_id in element_ids:
        driver.page.evaluate(UNFOLD_JS, element_id)
    # `checkVisibility`, not `__visible`: inside a body hidden until found an element still answers
    # for its geometry (Chromium lays it out on asking) though nothing of it is drawn.
    driver.page.wait_for_function(
        "(id) => document.getElementById(id).checkVisibility()", arg=element_ids[-1], timeout=5_000
    )


def _act_settings(driver: Driver) -> None:
    driver.click("open-settings")
    driver.page.wait_for_function("() => window.__visible('close-settings')", timeout=5_000)
    driver.settle()


def _act_discord(driver: Driver) -> None:
    driver.click("close-settings")
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=5_000)
    driver.click("view-switch")
    # A real read of the real server's channel; --fake-discord seeds it.
    driver.page.wait_for_function(
        "() => document.querySelectorAll('#discord-log > li').length > 0 "
        "&& !window.__visible('channel-loading')",
        timeout=15_000,
    )
    driver.settle(400)


LONG_TRANSCRIPT = [
    ("user", "where did the overnight run get to"),
    (
        "agent",
        "Three things finished and one is still going. The retry-budget branch landed at 9c07d3e, "
        "the cache-key rewrite is open as a draft, and the nightly cache rebuild took twenty-two "
        "minutes which is inside the usual band. Nothing failed outright. The one job still "
        "outstanding is the arm64 build, which has been queued long enough that it is worth "
        "looking at on its own rather than waiting for it.",
    ),
    ("user", "what is still going"),
    (
        "agent",
        "The arm64 job. It has been queued for forty-one minutes with nothing executing, which is "
        "a wedged runner rather than a busy one — the run reads green at the run level but its job "
        "list has a job that never started, so that green is not evidence of anything. The other "
        "three architectures finished inside nine minutes each, which is the usual shape, so this "
        "is the runner and not the change.",
    ),
    ("user", "did anyone re-fire it"),
    (
        "agent",
        "Yes, twice, onto the same runner both times. It was only cleared when the runner was "
        "recycled by hand, and the report after that had a real job list: eleven completed, none "
        "skipped.",
    ),
    ("user", "ok. what about the review comments on the retry branch"),
    (
        "agent",
        "One real finding: the budget was consulted before the jitter was applied, so under "
        "contention every caller woke at the same instant and spent the whole allowance in one "
        "burst. That was fixed and pushed, along with removing an unrelated cache-key rewrite that "
        "had been folded into the same branch.",
    ),
    ("user", "anything I have to do myself"),
    (
        "agent",
        "One thing. The token the release job uses expires in six days and rotating it needs the "
        "maintainer account, so nothing automated can do it for you. Everything else on the list "
        "either landed or has an owner who is awake. If it lapses the release job fails at the "
        "publish step, after the build, which is the most expensive place to find out.",
    ),
    ("user", "and the two issues from the QA sweep"),
    (
        "agent",
        "The first is a genuine flake — it depends on wall-clock ordering between two spawned "
        "processes. The second is not a flake at all; it is a real ordering bug that only appears "
        "on a loaded machine, and it was deliberately not labelled flaky because that is how a "
        "real bug gets ignored.",
    ),
    ("user", "fine. read me the last thing build-bot said"),
    (
        "agent",
        "The nightly cache rebuild finished in twenty-two minutes fourteen seconds, three point "
        "one gigabytes written, no evictions. The night before was twenty-four minutes two, so "
        "that is noise rather than an improvement. It also said the eviction policy has not "
        "triggered once this week, which is the number worth watching if the cache starts missing.",
    ),
    ("user", "how many messages are in the channel now"),
    (
        "agent",
        "Twelve since the run started, from four different authors. The Discord view has all of "
        "them with their ids if you want to check one against what I just told you. Four of them "
        "are build-bot, which is the noisiest one; the rest are split between claude-integ and "
        "the two people who were awake for it. None of them is older than the run itself.",
    ),
    ("user", "give me the whole picture then, one answer"),
    (
        # Deliberately the longest thing in the list, and long enough to run past three lines at
        # DESKTOP width as well as on a phone. `12-collapsed-long-transcript` opens this one, so
        # the picture is of a fold that is really hiding something rather than of a fold that
        # happens to fit. A voice assistant summarising a night's work does answer at this length,
        # which is the whole reason `#47 scrollback-stability` exists.
        "agent",
        "Here is all of it in one go. The retry-budget branch landed at 9c07d3e after the jitter "
        "ordering was fixed, and the unrelated cache-key rewrite was pulled back out of it into a "
        "draft of its own. The nightly cache rebuild finished in twenty-two minutes fourteen with "
        "three point one gigabytes written and no evictions, which is a minute and a half faster "
        "than the night before and inside the ordinary spread either way. The arm64 job is the "
        "only thing still outstanding: it has sat queued for forty-one minutes on a runner that "
        "is wedged rather than busy, it was re-fired twice onto the same runner, and it only "
        "cleared when the runner was recycled by hand. Of the two QA findings the first is a "
        "genuine flake that turns on wall-clock ordering between two spawned processes, and the "
        "second is a real ordering bug that only shows up on a loaded machine. The one thing "
        "waiting on you is the release token, which expires in six days and needs the maintainer "
        "account to rotate.",
    ),
]


def _act_long_scroll(driver: Driver) -> None:
    """A long call, parked mid-scroll — and the state the two below are built on.

    SELF-CONTAINED, by `#74 scroll-test-strength` finding 3. It used to begin by switching back to
    the call, which only worked because `10-discord-view` had left the Discord pane up, and the two
    states after it inherited whatever this one left on the screen. That made the chain invisible:
    `13-jump-to-newest` photographed a "Collapse all" chip that belonged to `12`, and neither could
    be run with `--only` at all — which is exactly when somebody reaches for one scene.

    So it loads the page itself, and `_act_one_expanded` and `_act_jump_to_newest` CALL this rather
    than assuming it ran. The dependency is still real; it is now written down in code that
    executes, which is the only kind that cannot go stale.
    """
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("talk")
    driver.page.wait_for_function(
        "() => window.__text('talk-label') === 'Listening'", timeout=10_000
    )
    # From the newest line. The transcript is kept on the server across a reload and across
    # profiles (`#128 transcript-history`), so from the second scene to build it on there is a
    # history already, and the page reopens it wherever it lands. Turns added from up that history
    # are arrivals in the scrollback, and since `#207 scrollback-jump` the jump counts them: it read
    # "19 new" in a scene promising one arrival, and "18 new" where 11 promises the plain "Newest".
    # The reader of these scenes is following the call, so they start where a follower is.
    driver.js("(() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; })()")
    driver.page.wait_for_function("() => !window.__visible('jump-newest')", timeout=5_000)
    for who, text in LONG_TRANSCRIPT:
        driver.js(
            ("window.__userSays(" if who == "user" else "window.__agentSays(")
            + json.dumps(text)
            + ")"
        )
    driver.settle(400)
    # Park the view in the MIDDLE of the scroll range. Pinned to the bottom is the state every other
    # capture already shows; the middle is what exercises the header and dock edges with content
    # running under them.
    driver.js(
        "(() => { const a = document.getElementById('scroll-area'); "
        "a.scrollTop = Math.round((a.scrollHeight - a.clientHeight) * 0.45); return a.scrollTop; })()"
    )
    driver.settle(300)


def _act_one_expanded(driver: Driver) -> None:
    # `#47 scrollback-stability`. Every long answer ARRIVES showing its first three lines, which is
    # what makes the list above skimmable. The one thing a unit test cannot check is whether three
    # lines is actually three lines — a line clamp is a rendering fact — so this state exists to be
    # LOOKED at, with one message opened among the closed ones for the comparison.
    #
    # It builds its own folded transcript rather than assuming the state before it left one. `#74
    # scroll-test-strength` finding 3: this scene could not be run with `--only` at all, because
    # "there is already a long folded list on the screen" was a dependency nothing declared.
    _act_long_scroll(driver)
    driver.js("(() => { document.getElementById('scroll-area').scrollTop = 0; })()")
    driver.settle(150)
    # Measure ONE message closed, then open THAT ONE. Comparing the first open message against the
    # first closed one compares two lengths, not two states, and passes or fails on which message
    # happened to be longest — at desktop width it passed for the wrong reason.
    driver.js(
        "(() => { const items = "
        "[...document.querySelectorAll(\"#transcript > li[data-collapsed='true']\")]; "
        "const li = items.sort((a, b) => b.textContent.length - a.textContent.length)[0]; "
        "li.id = 'fold-probe'; "
        "window.__foldedHeight = li.querySelector('.body').getBoundingClientRect().height; "
        "return window.__foldedHeight; })()"
    )
    # Opened as a reader opens it, by a tap on its text: the fold button is clipped for the keyboard
    # in the transcript as on a channel row (`#431 voice-transcript-expand`).
    driver.page.click("#fold-probe .body")
    driver.page.wait_for_function(
        "() => !!document.querySelector(\"#transcript > li[data-collapsed='false']\")", timeout=5_000
    )
    # Playwright scrolls an element into view before clicking it, which parks the opened message
    # alone on the screen — a picture of one long message, not of one long message AMONG the
    # three-line openings, which is the whole comparison this state exists to show. Put the
    # boundary back on screen.
    driver.js(
        "(() => { const a = document.getElementById('scroll-area'); "
        "const probe = document.getElementById('fold-probe'); "
        "const offset = probe.getBoundingClientRect().top - a.getBoundingClientRect().top; "
        "a.scrollTop = Math.max(0, a.scrollTop + offset - a.clientHeight * 0.5); "
        "return a.scrollTop; })()"
    )
    driver.settle(300)


# `#207 scrollback-jump`. The jump to the newest message, saying `%s`, at the LEFT end of the chip
# row: within a gutter of the left edge of the column it scrolls — the list on a phone, the reading
# column on a desk — and of the list's foot, and clear of the dock below.
JUMP_AT_LOWER_LEFT = (
    "(() => { const j = document.getElementById('jump-newest').getBoundingClientRect(); "
    "const a = document.getElementById('scroll-area').getBoundingClientRect(); "
    "const pane = document.querySelector('#pane-voice:not([hidden]), #pane-discord:not([hidden])')"
    ".getBoundingClientRect(); "
    "const dock = document.getElementById('dock').getBoundingClientRect(); "
    "return window.__visible('jump-newest') && window.__text('jump-newest-label') === '%s' "
    "&& Math.abs(j.left - pane.left) < 40 && a.bottom - j.bottom < 24 && j.bottom <= dock.top + 1; })()"
)


def _act_jump_to_newest(driver: Driver) -> None:
    """A turn arrives while the reader is up in the history: the view holds, and the jump says so.

    The page must NOT drag them down to it — and must not leave them unaware it happened either,
    which is what the chip is for. Since `#207 scrollback-jump` the jump is already up before the
    turn arrives, because the reader is in the scrollback; the arrival turns its "Newest" into
    "1 new", on the same control.

    ITS OWN TRANSCRIPT, and that is the fix rather than tidying. `#74 scroll-test-strength` finding
    3: this scene used to inherit whatever `12-collapsed-long-transcript` left behind, so the frame
    showed a "Collapse all" chip belonging to a message THAT scene had opened, while this one's own
    description says "the chip appeared", singular. A picture with two chips in it, filed under a
    name that mentions one, is worse than no picture: it is evidence for a state the walk was never
    in. Building the list here makes the frame the one the name promises, and makes `--only` work.
    """
    _act_long_scroll(driver)
    driver.js(
        "(() => { const a = document.getElementById('scroll-area'); "
        "a.scrollTop = Math.round(a.scrollHeight * 0.2); return a.scrollTop; })()"
    )
    driver.settle(200)
    driver.js(
        "window.__agentSays('One more thing while you were reading: the release token expires in "
        "six days and rotating it needs the maintainer account.')"
    )
    # The words, not merely the chip: the jump was up before the turn arrived, so only "1 new" says
    # the arrival has been drawn.
    driver.page.wait_for_function(
        "() => window.__visible('jump-newest') && window.__text('jump-newest-label') === '1 new'", timeout=5_000
    )
    driver.settle(300)


# `#55 voice-desktop-app`. The two states below are the reading column at each end of its range.
#
# SELF-CONTAINED, unlike everything above them: the scenes before this point are one continuous
# walk through a session, and these are appended after it, so each starts by loading the page
# again rather than inheriting whatever state the walk left behind. They also run only on the
# desktop profiles -- see `Scene.profiles`.
#
# The width is set through the SETTINGS SLIDER rather than by dragging the handle, because a
# synthetic drag is three events whose coordinates would have to be right for the picture to mean
# anything, and the slider drives exactly the same code with none of that. What the pictures then
# show, and what nothing in the page suite can show, is the column at a real width in a real
# browser with real text in it.

SET_READING_WIDTH = (
    "(() => { const r = document.getElementById('reading-width'); r.value = '%s'; "
    "r.dispatchEvent(new Event('input', { bubbles: true })); return r.value; })()"
)

MEASURE_COLUMN = (
    "(() => document.getElementById('pane-voice').getBoundingClientRect().width)()"
)


def _act_desktop_column(driver: Driver, ch: str, into: str) -> float:
    """Land on the call screen, set the column to `ch` characters, and record what it measured."""
    driver.click("open-settings")
    driver.page.wait_for_function("() => window.__visible('close-settings')", timeout=5_000)
    _unfold(driver, "reading-width")
    driver.js(SET_READING_WIDTH % ch)
    driver.click("close-settings")
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=5_000)
    driver.settle(200)
    width = float(cast(float, driver.js(MEASURE_COLUMN)))
    driver.js(f"window.{into} = {width}")
    return width


def _act_desktop_seed(driver: Driver) -> None:
    """Load the page fresh and put a real conversation into it.

    A call has to be OPEN for there to be a transcript at all -- the turn injectors talk to the
    fake socket, and that only exists once Talk has been pressed -- and a desktop picture with an
    empty transcript would say nothing at all about line length, which is the whole subject here.
    """
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("talk")
    driver.page.wait_for_function(
        "() => window.__text('talk-label') === 'Listening'", timeout=10_000
    )
    for who, text in LONG_TRANSCRIPT[:8]:
        driver.js(
            ("window.__userSays(" if who == "user" else "window.__agentSays(")
            + json.dumps(text)
            + ")"
        )
    driver.settle(300)


def _act_desktop_narrow(driver: Driver) -> None:
    _act_desktop_seed(driver)
    _act_desktop_column(driver, "48", "__columnWidth")
    driver.settle(200)


def _act_desktop_wide(driver: Driver) -> None:
    _act_desktop_seed(driver)
    # BOTH ends measured inside this one scene. Comparing across scenes is not available -- each
    # of these reloads the page, which wipes anything the previous one left on `window` -- and a
    # single width alone cannot show that the control does anything at all.
    _act_desktop_column(driver, "48", "__narrowColumn")
    _act_desktop_column(driver, "112", "__wideColumn")
    driver.settle(200)


# `#54 resume-recovery`. The two connection outcomes that were previously indistinguishable, and
# which nothing in this table has ever photographed: NONE of the states above shows the error panel
# at all, which is a large part of why the page spent months telling people a suspended call had
# failed. Self-contained, and on every profile -- the phone is the device this actually happens on.


def _act_call_for_drop(driver: Driver) -> None:
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("talk")
    driver.page.wait_for_function(
        "() => window.__text('talk-label') === 'Listening'", timeout=10_000
    )
    driver.js("window.__userSays('did the overnight run finish')")
    driver.js(
        "window.__agentSays('It did. The retry-budget branch landed at 9c07d3e and the tip is "
        "green.')"
    )
    driver.settle(200)


def _act_suspended(driver: Driver) -> None:
    _act_call_for_drop(driver)
    driver.js("window.__suspend()")
    driver.settle(150)
    driver.js("window.__resume()")
    driver.page.wait_for_function("() => window.__text('talk-label') === 'Resume'", timeout=5_000)
    driver.settle(200)


def _act_connection_failed(driver: Driver) -> None:
    _act_call_for_drop(driver)
    # Foreground, so this really is the failure case: an error, and then the close that confirms
    # the page was never hidden.
    driver.js("window.__fail()")
    driver.js("window.__socket.close(1006, '')")
    driver.page.wait_for_function("() => window.__visible('error')", timeout=5_000)
    driver.settle(200)


# `#51 reply-view`, since `#410 inline-reply-editor` a reply open in the list: the reply's box, opened on
# a real seeded channel message, under it in the stream.
#
# Both states are self-contained and run everywhere: the phone is where a reply is actually typed,
# and the thing most likely to be wrong on it -- the message you are answering pushing the text box
# under the keyboard -- is a layout fact nothing else here can see.


# A ROW IS `#discord-log > li`, never `#discord-log li`. `#217 markdown-blocks`: a message body is
# the server's rendered HTML, and a message with a list holds `li` elements of its own, so the
# descendant selector counted a message's bullet points as rows -- and the ordering check of
# `21-channel-older-loaded` read their missing ids as history out of order.
def _act_open_channel(driver: Driver) -> None:
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("view-switch")
    # Cached rows are deliberately painted before the request answers. They are useful to a reader,
    # but they cannot certify a scene that says it read the real throwaway server. Wait for both the
    # rows and the end of the live read, so a saved snapshot alone never satisfies this precondition.
    driver.page.wait_for_function(
        "() => document.querySelectorAll('#discord-log > li').length > 0 "
        "&& !window.__visible('channel-loading')",
        timeout=15_000,
    )
    driver.settle(300)


def _act_whole_channel(driver: Driver) -> None:
    """Open the channel and walk all the way back to its beginning.

    The throwaway server this harness starts is configured with a small fetch ceiling on purpose --
    otherwise `#65 scrollback-paging` is never exercised at all -- so a channel arrives a few
    messages at a time. States that need the WHOLE channel on screen (because they need it to
    overflow, or need its longest message) say so by calling this rather than by assuming the first
    read was everything.
    """
    _act_open_channel(driver)
    # Driven by SCROLLING to the top rather than by clicking the control, and that is not a
    # preference: Playwright scrolls an element into view before clicking it, and #load-older lives
    # at the top of the list, so a click would itself trigger the automatic step it is trying to
    # take and then race the DOM it is standing on.
    for _ in range(20):
        if not bool(driver.js("window.__visible('load-older')")):
            break
        driver.js(
            "(() => { window.__walkFrom = "
            "document.querySelectorAll('#discord-log > li').length; "
            "document.getElementById('scroll-area').scrollTop = 0; return window.__walkFrom; })()"
        )
        driver.page.wait_for_function(
            "() => document.querySelectorAll('#discord-log > li').length > window.__walkFrom",
            timeout=10_000,
        )
        driver.settle(120)
    else:
        raise Unreachable(
            "the channel walk", "the walk back reaches the beginning of the channel", "20 steps"
        )
    driver.js(
        "(() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; })()"
    )
    driver.settle(250)


def _act_channel_older(driver: Driver) -> None:
    _act_open_channel(driver)
    # This is the actual precondition of the scene. Checking it before measuring the anchor turns a
    # stale/complete snapshot into a precise report about a missing cursor, instead of clicking a
    # hidden no-op control and timing out ten seconds later waiting for rows that cannot arrive.
    try:
        driver.page.wait_for_function(
            "() => window.__visible('load-older') "
            "&& !document.getElementById('load-older').disabled",
            timeout=10_000,
        )
    except Exception as error:
        state = driver.js(
            "(() => { const b = document.getElementById('load-older'); "
            "return JSON.stringify({ visible: window.__visible('load-older'), "
            "disabled: b.disabled, rows: document.querySelectorAll('#discord-log > li').length }); })()"
        )
        raise Unreachable(
            "21-channel-older-loaded",
            "an enabled Older messages control proves the server supplied a next-page cursor",
            f"the completed newest-page read left {state}",
        ) from error
    # Park in the MIDDLE. At the bottom of a list the page counts the reader as pinned to the
    # newest line, and following it is then the correct behaviour -- so a scene that measured the
    # anchor from there would be asserting the opposite of what it says.
    driver.js(
        "(() => { const a = document.getElementById('scroll-area'); "
        "a.scrollTop = Math.round((a.scrollHeight - a.clientHeight) * 0.5); "
        "window.__parkedMid = a.scrollTop; return a.scrollTop; })()"
    )
    driver.settle(200)
    # Measured BEFORE the step: how much is loaded, and where on screen the row the reader is
    # looking at sits. Both are compared after, which is what makes this a picture of an anchored
    # prepend rather than of a longer list.
    driver.js(
        "(() => { const rows = [...document.querySelectorAll('#discord-log > li')]; "
        "const a = document.getElementById('scroll-area').getBoundingClientRect(); "
        "const seen = rows.find((n) => n.getBoundingClientRect().bottom > a.top); "
        "seen.id = 'anchor-probe'; "
        "window.__loadedBefore = rows.length; "
        "window.__anchorBefore = seen.getBoundingClientRect().top; "
        "return window.__loadedBefore; })()"
    )
    # Through the DOM, for the same reason the walk above scrolls instead of clicking: a real
    # click would scroll the control into view, and scrolling is what this scene is measuring.
    driver.js("(() => { document.getElementById('load-older').click(); })()")
    driver.page.wait_for_function(
        "() => document.querySelectorAll('#discord-log > li').length > window.__loadedBefore",
        timeout=10_000,
    )
    driver.settle(250)
    driver.js(
        "(() => { window.__anchorAfter = "
        "document.getElementById('anchor-probe').getBoundingClientRect().top; "
        "window.__loadedAfter = document.querySelectorAll('#discord-log > li').length; "
        "return window.__anchorAfter; })()"
    )


def _act_channel_picker_reachable(driver: Driver) -> None:
    """`#83 channel-selector-in-bar`: the picker on the bar, several pages deep in the history.

    The owner's complaint was that the picker was unreachable, and unreachability is a MEASUREMENT
    on a real 375px screen -- is the control inside the viewport, clear of the edges, at a scroll
    position the reader can actually be in. No fixture has an opinion about that, which is why this
    state exists rather than a third unit test.

    The walk is what makes it the owner's case rather than a friendly one. It goes back several
    pages first, so the history above the reader is exactly the thing that used to push the picker
    further away with every scroll toward it.
    """
    _act_open_channel(driver)
    driver.js(
        "(() => { const r = document.getElementById('discord-channel').getBoundingClientRect(); "
        "window.__pickerBefore = { top: r.top, left: r.left }; return window.__pickerBefore; })()"
    )
    # Scrolled, not clicked, for the same reason `_act_whole_channel` scrolls: #load-older lives at
    # the top of the list and Playwright scrolls a control into view before clicking it, which
    # would itself take the step this is trying to take.
    steps = 0
    for _ in range(4):
        if not bool(driver.js("window.__visible('load-older')")):
            break
        driver.js(
            "(() => { window.__walkFrom = "
            "document.querySelectorAll('#discord-log > li').length; "
            "document.getElementById('scroll-area').scrollTop = 0; return window.__walkFrom; })()"
        )
        driver.page.wait_for_function(
            "() => document.querySelectorAll('#discord-log > li').length > window.__walkFrom",
            timeout=10_000,
        )
        driver.settle(120)
        steps += 1
    # AT LEAST ONE, and then however many the channel has -- not a fixed three. The loop above
    # already runs until there is nothing above the reader, so what is left to check here is that
    # the walk really paged rather than finding the whole channel already loaded. How DEEP the
    # history ended up is a separate claim, and it is asserted where it is also photographed:
    # `__loadedAfter >= 12` below. Demanding three steps here pinned the walk to a page size and a
    # seed length instead, and a seeded channel that arrives in two reads is not a broken picture.
    if steps < 1:
        raise Unreachable(
            "30-channel-picker-in-bar",
            "the channel pages at all, so the picker has history above it",
            "the whole channel arrived in one read, so nothing was ever prepended",
        )
    # Parked mid-history: not at the top, where the walk left the reader, and not at the newest
    # line either. This is the position the picker was unreachable FROM.
    driver.js(
        "(() => { const a = document.getElementById('scroll-area'); "
        "a.scrollTop = Math.round((a.scrollHeight - a.clientHeight) * 0.45); return a.scrollTop; })()"
    )
    driver.settle(250)
    driver.js(
        "(() => { const r = document.getElementById('discord-channel').getBoundingClientRect(); "
        "window.__pickerAfter = { top: r.top, left: r.left, right: r.right, bottom: r.bottom, "
        "width: r.width, height: r.height }; "
        "window.__pickerMoved = Math.max(Math.abs(r.top - window.__pickerBefore.top), "
        "Math.abs(r.left - window.__pickerBefore.left)); "
        "window.__pickerInScroll = document.getElementById('scroll-area')"
        ".contains(document.getElementById('discord-channel')); "
        "window.__loadedAfter = document.querySelectorAll('#discord-log > li').length; "
        "return window.__pickerMoved; })()"
    )


# `#68 pull-to-refresh`. A real gesture, built from real `Touch` and `TouchEvent` objects and
# dispatched at the element -- not a synthetic object shaped like one. That is the point of doing it
# here rather than in the page suite: the fixture hands the page whatever a test invents, so only a
# browser can say the handlers read what a browser really sends.
#
# The finger is deliberately NEVER LIFTED. The state worth a picture is the armed one, because it is
# the state that exists so the reader can still change their mind, and it is the only frame in which
# the page is asking for a decision.
#
# IT IS PERFORMED ON A CHANNEL THAT STILL HAS HISTORY, which is the case the first version of this
# feature could not reach at all: the reader drags, the list runs out under the finger, and the page
# hears the browser's own `scroll` while the finger is still down. Synthetic touches move nothing by
# themselves, so the scroll a real drag would produce is supplied here -- but the EVENT is real, it
# is the one `#65 scrollback-paging` listens on, and whether the page takes a step back on hearing it
# is exactly what this scene is here to see.
PULL_GESTURE = """
(() => {
  const area = document.getElementById('scroll-area');
  const box = area.getBoundingClientRect();
  const x = Math.round(box.left + box.width / 2);
  const start = Math.round(box.top + 40);
  const at = (type, y) => {
    const touch = new Touch({
      identifier: 1, target: area,
      clientX: x, clientY: y, pageX: x, pageY: y, screenX: x, screenY: y,
    });
    return new TouchEvent(type, {
      touches: [touch], targetTouches: [touch], changedTouches: [touch],
      bubbles: true, cancelable: true,
    });
  };
  window.__pullRowsBefore = document.querySelectorAll('#discord-log > li').length;
  window.__pullMoreAbove = window.__visible('load-older');
  area.dispatchEvent(at('touchstart', start));
  area.scrollTop = 0;
  area.dispatchEvent(new Event('scroll'));
  area.dispatchEvent(at('touchmove', start + 30));
  area.dispatchEvent(at('touchmove', start + 30 + 140));
  window.__pullRowsAfter = document.querySelectorAll('#discord-log > li').length;
  return document.getElementById('pull-refresh').getAttribute('data-state');
})()
"""


def _act_pull_armed(driver: Driver) -> None:
    """`#68 pull-to-refresh`: the finger is down, past the threshold, and the page says so.

    Deliberately NOT walked to the beginning of the channel. This scene used to do that, and the
    reason it had to was the defect: arriving at the top of a channel with history fired the
    automatic step back, the anchored prepend put the reader at a positive offset again, and so the
    old `scrollTop === 0 at touchstart` precondition could never be met while anything remained
    above. Walking the whole channel first removed the race by removing the case -- and the case is
    every channel this issue is about.

    So it takes ONE step back, leaving history above the reader, and pulls from there. What the
    picture is worth is then a claim about the paged channel and not about the exhausted one.
    """
    _act_open_channel(driver)
    driver.js(
        "(() => { window.__walkFrom = document.querySelectorAll('#discord-log > li').length; "
        "document.getElementById('scroll-area').scrollTop = 0; return window.__walkFrom; })()"
    )
    driver.page.wait_for_function(
        "() => document.querySelectorAll('#discord-log > li').length > window.__walkFrom",
        timeout=10_000,
    )
    driver.settle(250)
    if not bool(driver.js("window.__visible('load-older')")):
        raise Unreachable(
            "31-pull-to-refresh-armed",
            "the channel still has history above the reader, which is the case the pull is for",
            "the walk reached the beginning of the channel in one step",
        )
    state = driver.js(PULL_GESTURE)
    if state != "armed":
        raise Unreachable(
            "31-pull-to-refresh-armed",
            "a pull past the threshold arms, and the page says 'Release to refresh'",
            f"the affordance reported {state!r}",
        )
    driver.settle(250)


def _mock_control(driver: Driver, state: str) -> str:
    """The offline vendor's control plane, or an Unreachable state that says how to get one.

    There is exactly ONE summariser now — a conversation with the configured ElevenLabs agent — so
    the two summary states are the only ones in this walk that need a vendor at all. `scripts/run.sh
    --screenshots` starts `vibe-talk-mock-elevenlabs` and passes `--mock-control`; a run driven by
    hand without it cannot reach these states, and must say so rather than photographing whatever
    was on screen. It is deliberately not "skip": a summary state quietly missing from a run is the
    absent evidence this whole harness exists to prevent.
    """
    if not driver.mock_control:
        raise Unreachable(
            state,
            "the summariser can be driven, which needs the offline ElevenLabs mock's control plane",
            "--mock-control was not given. Use scripts/run.sh --screenshots, which starts "
            "vibe-talk-mock-elevenlabs and passes it. Pointing this state at a real ElevenLabs "
            "account would spend vendor minutes and could not be made to fail on demand.",
        )
    return driver.mock_control


def _tell_the_mock(driver: Driver, state: str, scenario: str) -> None:
    """Put the offline vendor into one of its scenarios, through `POST /_mock/scenario`.

    Set by the state that needs it rather than once at the start of the run, and that is the point:
    the walk visits a working summariser and then a wedged one, in that order, on every profile and
    both themes. A scenario left over from the previous state would make one of the two pictures a
    lie, and which one would depend on `--only`.
    """
    control = _mock_control(driver, state)
    payload = json.dumps({"scenario": scenario}).encode("utf-8")
    request = Request(  # noqa: S310 - a loopback URL this script built
        f"{control}/_mock/scenario",
        data=payload,
        method="POST",
        headers={"Content-Type": "application/json"},
    )
    try:
        with urlopen(request, timeout=10) as response:  # noqa: S310 - loopback
            answered = response.read().decode("utf-8", "replace")
    except HTTPError as error:
        raise Unreachable(
            state,
            f"the offline vendor can be put into its {scenario!r} scenario",
            f"the mock's control plane answered HTTP {error.code}",
        ) from error
    except URLError as error:
        raise Unreachable(
            state,
            f"the offline vendor can be put into its {scenario!r} scenario",
            f"nothing answered at {control}/_mock/scenario ({error})",
        ) from error
    # Read back, not merely "it answered 200": an unknown scenario name is a 400 above, but a
    # RENAMED one that still parses would leave this state photographing the wrong vendor
    # behaviour under a confident name.
    if f'"{scenario}"' not in answered:
        raise Unreachable(
            state,
            f"the offline vendor really is in its {scenario!r} scenario",
            f"the control plane answered {answered.strip()[:200]!r}",
        )


def _keep_summaries_setting(driver: Driver, state: str, on: bool) -> None:
    """Hold "Summarize collapsed messages" on or off ON THE SERVER, as a page's settings write would.

    `#564 message-summary-toggle` made summaries the owner's synced setting, off unless turned on,
    and the server refuses a summary while it holds the setting off. Written here directly rather
    than left to the page's own settings sync, so a scene never races that write: the request a
    scene photographs is answered under the setting the scene chose, and a scene after it starts
    from off however the profile's pages last left it.
    """
    body = json.dumps({"set": [{"key": "vibe-talk.voice.summarize-collapsed",
                                "value": "1" if on else None, "updated_ms": 0}]}).encode()
    request = Request(f"{driver.base_url}/api/v1/client-settings", data=body, method="PUT")
    request.add_header("Authorization", f"Bearer {driver.token}")
    request.add_header("Content-Type", "application/json")
    try:
        with urlopen(request, timeout=10):  # noqa: S310 - a loopback URL this script built
            return
    except HTTPError as error:
        # No store: settings live on the page alone, and the server refuses nothing for them.
        if error.code == 503:
            return
        raise Unreachable(state, "the summaries setting can be kept on the server",
                          f"PUT /api/v1/client-settings answered HTTP {error.code}") from error
    except URLError as error:
        raise Unreachable(state, "the summaries setting can be kept on the server",
                          f"could not reach {driver.base_url} ({error})") from error


def _press_summaries(driver: Driver, on: bool) -> None:
    """The Summaries chip pressed to `on`, only if it is not already there: it is a kept setting now,
    and a blind click would turn it OFF on a page that had already put it back on."""
    want = "true" if on else "false"
    driver.js(f"(() => {{ const chip = document.getElementById('summarise'); "
              f"if (chip.getAttribute('aria-pressed') !== '{want}') chip.click(); }})()")


def _summaries_off_after(driver: Driver) -> None:
    """`after` for a scene that turned summaries on: off on the page and on the server again."""
    _press_summaries(driver, False)
    _keep_summaries_setting(driver, "summaries-off-after", False)


def _forget_cached_summaries(driver: Driver, state: str) -> None:
    """Empty the server's durable store, so the next summary is really generated.

    A summary is CACHED under its policy version, and the profiles share one server: without this,
    the failure state would be handed the answer the success state before it filed — the request
    that is supposed to fail would never leave the server, and the state would be unreachable for a
    reason that reads like a page bug. Purging the whole store rather than one row because there is
    no route that forgets a single summary, and this state is the last of the walk.
    """
    request = Request(f"{driver.base_url}/api/v1/storage", method="DELETE")
    request.add_header("Authorization", f"Bearer {driver.token}")
    try:
        with urlopen(request, timeout=10):  # noqa: S310 - a loopback URL this script built
            return
    except HTTPError as error:
        # No store configured means there is no cache to defeat, which is this function's whole
        # purpose. Anything else is a failure now rather than a confusing one in twenty seconds.
        if error.code == 503:
            return
        raise Unreachable(
            state,
            "the cached summary from the state before this one can be cleared",
            f"DELETE /api/v1/storage answered HTTP {error.code}",
        ) from error
    except URLError as error:
        raise Unreachable(
            state,
            "the cached summary from the state before this one can be cleared",
            f"could not reach {driver.base_url} ({error})",
        ) from error


# A real summary on THE probed row, not the placeholder shown while its request is in flight -- and
# not a summary on any other row. The log keeps other rows' summaries (a cached one from an earlier
# profile, one that simply finished first), and a wait that accepted any of them measured the probe
# while it still read `summarising…`: the same height as the clamp, and a failure blamed on the page.
# Shared by the act's wait and the scene's expectation so the two cannot drift apart again.
SUMMARY_PROBE_SUMMARISED = (
    "(() => { const text = document.querySelector('#summary-probe .summary-text'); "
    "return text !== null && (text.textContent || '').length > 0 "
    "&& !/^summarising/.test(text.textContent); })()"
)


def _act_summary_mode(driver: Driver) -> None:
    """`#49 cached-summaries`: the channel with the collapsed rows summarised instead of clipped."""
    # A WORKING summariser: canned prose, no tool calls, no bridge traffic. Stated here rather than
    # assumed from the mock's start-up flag, because the failure state below wedges the same mock
    # and the walk runs once per profile per theme.
    _tell_the_mock(driver, "32-channel-summarised", "no_tool_call")
    # From OFF, whatever an earlier profile's walk left on the shared server or this page holds:
    # the clamped row below has to be measured clamped.
    _keep_summaries_setting(driver, "32-channel-summarised", False)
    _act_open_channel(driver)
    _press_summaries(driver, False)
    driver.page.wait_for_function(
        "() => !document.querySelector('#discord-log > li[data-summarised=\"true\"]')", timeout=10_000)
    # The row this picture is ABOUT, measured before anything changes. The seeded backlog has
    # exactly one message over the server's summarisation threshold and it is the newest, so it is
    # on screen from the first read; its height here is what the three-line clamp gives it.
    driver.js(
        "(() => { const rows = [...document.querySelectorAll('#discord-log > li')].reverse(); "
        "const row = rows.find((n) => n.getAttribute('data-collapsed') === 'true'); "
        "row.id = 'summary-probe'; "
        "window.__foldedHeight = row.getBoundingClientRect().height; "
        "return window.__foldedHeight; })()"
    )
    # The setting on at the server only now, after the clamped row is measured: held there before
    # the channel opened, the page would have put summaries on while it loaded.
    _keep_summaries_setting(driver, "32-channel-summarised", True)
    # Through the DOM rather than by clicking: the chip floats over the bottom corner of the list
    # and a real click would scroll it into view, which moves the thing being photographed.
    _press_summaries(driver, True)
    # For a REAL summary on the probed row itself; see SUMMARY_PROBE_SUMMARISED.
    driver.page.wait_for_function(f"() => {SUMMARY_PROBE_SUMMARISED}", timeout=20_000)
    driver.settle(300)
    driver.js(
        "(() => { window.__summarisedHeight = "
        "document.getElementById('summary-probe').getBoundingClientRect().height; "
        "return window.__summarisedHeight; })()"
    )
    # The same comparison the expectation makes, made HERE as well, purely so the failure reads as
    # the finding it is. The expectation's message can only quote its own JavaScript, and a reader
    # who sees `__summarisedHeight > 0 && __summarisedHeight < __foldedHeight` in a traceback goes
    # looking for a broken harness. The numbers, and where the extra height comes from, are what
    # turn it into a report about the page. The expectation stays: it is what holds if this check
    # is ever deleted.
    folded = float(cast(float, driver.js("window.__foldedHeight")))
    summarised = float(cast(float, driver.js("window.__summarisedHeight")))
    if not 0 < summarised < folded:
        lines = driver.js(
            "(() => { const row = document.getElementById('summary-probe'); "
            "const note = row.querySelector('.summary'); "
            "const text = row.querySelector('.summary-text'); "
            "return JSON.stringify({note: Math.round(note.getBoundingClientRect().height), "
            "chars: (text.textContent || '').length, "
            "markDisplay: getComputedStyle(row.querySelector('.summary-mark')).display}); })()"
        )
        raise Unreachable(
            "32-channel-summarised",
            "the summarised row really is shorter than the clamped one it replaced",
            f"clamped {folded:.0f}px, summarised {summarised:.0f}px, note {lines}. This is the "
            "claim the whole mode rests on, and it is the page that is failing it, not the walk: "
            "the reader paid a round trip to a vendor for a row no shorter than the one they had. "
            "Check `markDisplay` above first -- the label was on a line of its own once, which "
            "cost a summary that wraps to two lines a third line, exactly what the clamp already "
            "gives the message itself; `inline` is what fixed that and a regression to `block` "
            "would bring it straight back. If the label is already inline, the levers left are "
            "the clamp's line budget and summaries.target_chars. Do not relax this check: it is "
            "the only thing in the repository that can see the difference.",
        )


SUMMARY_FAILED_STATE = "36-channel-summary-failed"


def _act_summary_failed(driver: Driver) -> None:
    """Summary mode when the summariser FAILS: the row says so quietly and keeps the message.

    There is one summariser and it talks to a vendor, so failure is now an ordinary operating state
    rather than an exotic one — a wedged agent, a rejected key, or simply a deployment with no
    ElevenLabs credentials at all. The owner asked for it to be said on the row, and then
    (`#564 message-summary-toggle`) for that to be a subtle label that leaves the row's read state
    and its Done alone; this is the only thing in the repository that can see how it is drawn.

    Driven with a real failure, not a stubbed response. The mock is wedged into `no_reply` — it
    transcribes the question and says nothing, which is what a hung agent looks like from here —
    and the server's own three-second turn deadline turns that into the 503 the page draws.
    """
    # Order matters: purge first, because the summary the state before this one filed would be
    # served straight back out of the cache and nothing would fail.
    _forget_cached_summaries(driver, SUMMARY_FAILED_STATE)
    _tell_the_mock(driver, SUMMARY_FAILED_STATE, "no_reply")
    _act_open_channel(driver)
    driver.js(
        "(() => { const rows = [...document.querySelectorAll('#discord-log > li')].reverse(); "
        "const row = rows.find((n) => n.getAttribute('data-collapsed') === 'true'); "
        "row.id = 'summary-failed-probe'; "
        "window.__failedProbeHeight = row.getBoundingClientRect().height; "
        "return window.__failedProbeHeight; })()"
    )
    # Through the DOM for the same reason as the state above: a real click scrolls the floating
    # chip into view and moves the list being photographed.
    # On at the server after the purge (which forgets the owner's settings with everything else)
    # and after the probe is measured, as in the state above.
    _keep_summaries_setting(driver, SUMMARY_FAILED_STATE, True)
    _press_summaries(driver, True)
    # THE PROBE ROW, not whichever row loses the race.
    #
    # Turning the mode on asks about every long row on screen, and each ask is a separate round
    # trip that the server gives three seconds before it gives up. Waiting for `any row failed`
    # therefore returns as soon as the FIRST of them expires, which is usually not the row this
    # state photographs and asserts on -- so the picture was taken while the probe's own request
    # was still in flight, and the expectation that the probe is marked failed then failed on a
    # page that was about to be right. It went red on whichever profile happened to lose, which is
    # what made it look intermittent rather than wrong.
    driver.page.wait_for_function(
        "() => document.getElementById('summary-failed-probe')"
        ".getAttribute('data-summary-failed') === 'true'",
        timeout=20_000,
    )
    driver.settle(300)


def _choose_hide_read(driver: Driver) -> None:
    """Tap the read-mode button on to Hide read: `#221 read-modes` put Fold read before it."""
    for _tap in range(3):
        if driver.js("document.getElementById('todo-filter').getAttribute('aria-pressed')") == "true":
            return
        driver.click("todo-filter")
        driver.settle(150)


def _act_todo_view(driver: Driver) -> None:
    """`#50 todo-view`: the channel filtered down to what has not been dealt with."""
    _act_open_channel(driver)
    driver.js(
        "(() => { window.__beforeTodo = "
        "document.querySelectorAll('#discord-log > li').length; return window.__beforeTodo; })()"
    )
    _choose_hide_read(driver)
    driver.page.wait_for_function(
        "() => document.getElementById('todo-filter').getAttribute('aria-pressed') === 'true' "
        "&& (window.__text('inbox-note') || '').length > 0",
        timeout=15_000,
    )
    # Deal with the oldest one, so the picture is of a list somebody has WORKED rather than of a
    # filter that has been switched on and has done nothing yet. Through the DOM, because a real
    # click would scroll the row into view and this frame is about the list at rest.
    driver.js(
        "(() => { const row = document.querySelector('#discord-log > li'); "
        "window.__dealtWith = row.getAttribute('data-id'); "
        "row.querySelector('.done-button').click(); return window.__dealtWith; })()"
    )
    driver.page.wait_for_function(
        "() => document.querySelectorAll('#discord-log > li').length < window.__beforeTodo "
        "&& !document.getElementById('undo-dismiss').hidden",
        timeout=15_000,
    )
    driver.settle(300)
    driver.js(
        "(() => { window.__afterTodo = "
        "document.querySelectorAll('#discord-log > li').length; return window.__afterTodo; })()"
    )


def _act_bankruptcy_armed(driver: Driver) -> None:
    """The bulk clear with one tap on it: saying how many, before it has done anything."""
    _act_open_channel(driver)
    _choose_hide_read(driver)
    driver.page.wait_for_function(
        "() => !document.getElementById('clear-backlog').hidden", timeout=15_000
    )
    driver.js(
        "(() => { window.__backlog = "
        "document.querySelectorAll('#discord-log > li').length; return window.__backlog; })()"
    )
    driver.js("(() => { document.getElementById('clear-backlog').click(); })()")
    driver.page.wait_for_function(
        "() => (window.__text('clear-backlog') || '').trim().endsWith('?')", timeout=5_000
    )
    # Promptly: the arming lapses after a few seconds, and a picture taken after that would be of
    # a control at rest filed under a name that says armed.
    driver.settle(150)


def _act_message_search(driver: Driver) -> None:
    """`#129 message-search`: the glass open over the channel, with a quoted phrase in it.

    Driven over the CHANNEL rather than the transcript because that is where the layout risk is:
    the seeded backlog is long-winded on purpose, so the list really has rows to remove, and the
    bar floating over the top of the list (`#197 floating-search`) really has a field, a count, the
    Links and Pinned filters (`#211 link-filter`, `#206 pin-message`) and a glass competing for a
    375px band. Whether those fit — or whether a long query pushes the glass off the edge — is a
    flex question only a layout engine answers, and it is the reason `.search-field` carries
    `min-width: 0` at all.

    A QUOTED phrase, because that is the clause of the request that is invisible in any other
    frame: the seeded messages say "runner" several times over and "mac runner" exactly once, so
    the picture is of quoting having narrowed something rather than of quotes being accepted.
    """
    _act_open_channel(driver)
    driver.js(
        "(() => { window.__beforeSearch = "
        "document.querySelectorAll('#discord-log > li').length; return window.__beforeSearch; })()"
    )
    driver.click("search-toggle")
    driver.page.wait_for_function("() => window.__visible('search-field')", timeout=5_000)
    driver.page.fill("#search-field", '"mac runner"')
    driver.page.wait_for_function(
        "() => (window.__text('search-count') || '').length > 0", timeout=5_000
    )
    # `#262 search-load-older`. The phrase is older than the newest page this run's small ceiling
    # reads, and a search no longer reads older pages by itself -- the emptied list's own scroll
    # position used to chain steps back through the whole channel. Enter in the field walks back a
    # few pages for it, as the reader would.
    driver.page.press("#search-field", "Enter")
    driver.page.wait_for_function(
        "() => document.querySelector('#discord-log > li[data-id]:not(.search-hidden)') !== null "
        "&& document.getElementById('load-older').textContent.indexOf('Searching') !== 0",
        timeout=20_000,
    )
    driver.settle(300)


def _act_links_filter(driver: Driver) -> None:
    """`#211 link-filter`: the Links filter on over the channel, each row's links one per line.

    The seeded backlog holds no links, so one message with two arrives on the stream first, in the
    two forms whose drawing only a browser can judge: a Markdown link, which shows its NAME, and a
    bare address longer than a phone is wide, which is cut short on screen with its href whole.
    Then the bar opens and Links is turned on by a real click, beside Pinned and the glass.

    `#212 link-filter`: the message names a GitHub pull request too, so the kinds of link that come
    up under Links have something to count — and, in `38-link-kinds-off`, something to hide.
    """
    _act_open_channel(driver)
    driver.js(
        "window.__channelSays('9990000000000000211', 'claude-integ', "
        "'the change is [the diff](https://example.com/diff/42), reviewed in "
        "https://github.com/example/project/pull/42, and the full log is at "
        "https://example.com/logs/nightly-integration-shard-nightly-integration-shard-"
        "nightly-integration-shard/summary.html')"
    )
    driver.page.wait_for_function(
        "() => document.querySelector('#discord-log > li[data-id=\"9990000000000000211\"]') !== null",
        timeout=10_000,
    )
    driver.click("search-toggle")
    driver.page.wait_for_function("() => window.__visible('links-filter')", timeout=5_000)
    driver.click("links-filter")
    driver.page.wait_for_function(
        "() => document.querySelector('#discord-log > li[data-links-view=\"true\"]') !== null",
        timeout=5_000,
    )
    driver.settle(300)


def _act_link_kinds_off(driver: Driver) -> None:
    """`#212 link-filter`: the Links view with PRs turned off under the Links button.

    `37-links-filter`'s state, then a real click on PRs: the pull request leaves the row and the
    design's own links stay, and the button reads as off.
    """
    _act_links_filter(driver)
    driver.click("link-kind-pr")
    driver.page.wait_for_function(
        "() => document.getElementById('link-kind-pr').getAttribute('aria-pressed') === 'false'",
        timeout=5_000,
    )
    driver.settle(300)


# `#217 markdown-blocks`. The seeded message in the shape the owner photographed drawn as literal
# dashes: a paragraph, six items two of which run on to an indented line, and two closing paragraphs.
MESSAGE_BLOCKS_PROBE = "where the release stands this morning"


def _act_message_blocks(driver: Driver) -> None:
    """`#217 markdown-blocks`: the server-rendered list message, opened in full and in view.

    The message is over the page's fold, so it arrives folded to its opening lines, as such an
    answer does. The row is opened the way a reader opens it -- the fold control a tap on the row
    toggles -- and brought to the middle of the screen, so the picture is the whole list.
    """
    _act_open_channel(driver)
    driver.js(
        "(() => { const row = [...document.querySelectorAll('#discord-log > li')].find((n) => "
        f"(n.textContent || '').includes({json.dumps(MESSAGE_BLOCKS_PROBE)})); "
        "if (!row) return false; row.id = 'message-blocks-probe'; "
        "if (row.getAttribute('data-collapsed') === 'true') row.querySelector('.fold').click(); "
        "row.scrollIntoView({ block: 'center' }); return true; })()"
    )
    driver.settle(300)


# `#237 nested-lists`. The seeded checklist: numbered steps 9 to 11, their sub-steps indented two
# spaces as chat writes them, one of them three levels deep, and the second step's own steps
# numbered from one. CommonMark drew every level at one margin and numbered those steps 11 and 12.
NESTED_LISTS_PROBE = "steps 9 to 11 of the release checklist"


def _act_nested_lists(driver: Driver) -> None:
    """`#237 nested-lists`: the server-nested checklist, opened in full and in view.

    Opened and centred the way `41-message-blocks` opens its message, so the picture is the whole
    list at once.
    """
    _act_open_channel(driver)
    driver.js(
        "(() => { const row = [...document.querySelectorAll('#discord-log > li')].find((n) => "
        f"(n.textContent || '').includes({json.dumps(NESTED_LISTS_PROBE)})); "
        "if (!row) return false; row.id = 'nested-lists-probe'; "
        "if (row.getAttribute('data-collapsed') === 'true') row.querySelector('.fold').click(); "
        "row.scrollIntoView({ block: 'center' }); return true; })()"
    )
    driver.settle(300)



# `#244 elision-word-count`. The folded messages on screen, each tagged with the count under it, the
# largest and the smallest of them named so the expectations can say which is which.
ELISION_PROBES_JS = (
    # The label the stylesheet shows for the row's width: the line carries one per width class.
    "(() => { window.__elided = (n) => { const line = n && n.querySelector(':scope > .elided'); "
    "const shown = line ? [...line.children].filter((s) => getComputedStyle(s).display !== 'none') : []; "
    "return shown.length === 1 ? shown[0].textContent : ''; }; "
    "const rows = [...document.querySelectorAll('#discord-log > li')].filter((n) => "
    "n.getAttribute('data-collapsed') === 'true' && window.__elided(n)); "
    "const count = (n) => Number((/\\d+/.exec(window.__elided(n)) || ['0'])[0]); "
    "if (rows.length < 2) return false; "
    # The wall, and the labelled message hiding least of those that can share the screen with it.
    "const large = rows.reduce((a, b) => (count(b) > count(a) ? b : a)); "
    "const room = document.getElementById('scroll-area').clientHeight - 48; "
    "const l = large.getBoundingClientRect(); "
    "const near = rows.filter((n) => { const r = n.getBoundingClientRect(); "
    "return n !== large && Math.max(r.bottom, l.bottom) - Math.min(r.top, l.top) <= room; }); "
    "if (near.length === 0) return false; "
    "const small = near.reduce((a, b) => (count(b) < count(a) ? b : a)); "
    "small.id = 'elision-probe-small'; large.id = 'elision-probe-large'; "
    "return true; })()"
)


def _act_elision_word_count(driver: Driver) -> None:
    """`#244 elision-word-count`: the channel as it opens, its long messages folded with a count.

    The seeded backlog's newest page holds a message barely over the fold and the long one this
    project exists for, so the picture is the difference the owner asked to see: a handful of words
    hidden under one, a wall under the other. Both are brought onto the screen together.
    """
    _act_open_channel(driver)
    driver.js(ELISION_PROBES_JS)
    # The upper of the two a little below the top of the list, so both are on the screen together:
    # not flush with it, where a phone's fractional scroll can leave its edge a fraction above.
    driver.js(
        "(() => { const pair = ['elision-probe-small', 'elision-probe-large']"
        ".map((id) => document.getElementById(id)).filter(Boolean); "
        "if (pair.length < 2) return false; pair.sort((a, b) => a.getBoundingClientRect().top - "
        "b.getBoundingClientRect().top); const area = document.getElementById('scroll-area'); "
        "area.scrollTop += pair[0].getBoundingClientRect().top - area.getBoundingClientRect().top - 24; "
        "return true; })()"
    )
    driver.settle(300)



def _act_done_feedback(driver: Driver) -> None:
    """`#245 done-feedback`: a message marked Done from the keyboard, caught while its grey ring holds.

    The list takes the focus, a message is selected and e is pressed: the selection moves on to the
    next unread message, ringed in the accent, and the message just marked keeps a grey ring at the
    read strength for most of a second. The shutter opens inside that second; the animation into it
    is finished by the capture, so the picture is the look the eye is left with. The message is found
    again by its id, since the Done may draw its row afresh.
    """
    _act_open_channel(driver)
    driver.page.mouse.move(1, 1)
    driver.js("(() => { document.getElementById('scroll-area').focus(); return true; })()")
    # n, not an arrow: the first UNREAD message below the top of the screen, so e marks it Done
    # rather than putting back a message something before this state left read.
    driver.page.keyboard.press("n")
    driver.settle(150)
    driver.js(
        "(() => { const row = document.querySelector('#discord-log > li[data-selected=\"true\"]'); "
        "if (!row) return false; window.__doneProbe = row.getAttribute('data-id'); return true; })()"
    )
    driver.page.keyboard.press("e")
    driver.settle(250)


def _undo_done_feedback(driver: Driver) -> None:
    """After `53-done-feedback`'s capture: its Undo, so later pictures do not draw that message read."""
    driver.click("undo-dismiss")
    driver.page.wait_for_function(
        "() => { const r = [...document.querySelectorAll('#discord-log > li')].find((n) => "
        "n.getAttribute('data-id') === window.__doneProbe); return !!r && r.getAttribute('data-archived') "
        "=== 'false'; }",
        timeout=5_000,
    )


# The colour a CSS custom property resolves to, as the browser computes a colour from it.
RESOLVE_COLOUR_JS = (
    "window.__colour = (name) => { const s = document.createElement('span'); "
    "s.style.color = `var(${name})`; document.body.append(s); "
    "const c = getComputedStyle(s).color; s.remove(); return c; }"
)



def _act_help_sections(driver: Driver) -> None:
    """`#246 help-sections`: Help as a ? on Settings opens it, its own entry open and the rest folded.

    The reading width's ?, whose entry is short, so the picture holds the open entry and the folded
    headings after it. A fresh load, Settings, and the ? really clicked.
    """
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("open-settings")
    driver.page.wait_for_function("() => window.__visible('close-settings')", timeout=5_000)
    _unfold(driver, "help-link-reading-width")
    driver.click("help-link-reading-width")
    driver.page.wait_for_function("() => window.__visible('close-help')", timeout=5_000)
    driver.settle(300)


def _act_reply_view(driver: Driver) -> None:
    _act_whole_channel(driver)
    # Park in the middle of the channel, and record it. The round trip below is then a MEASURED
    # claim about the reader's position rather than a hope: putting a reply in the list down moves
    # nothing, which only a browser that lays the box out can say.
    driver.js(
        "(() => { const a = document.getElementById('scroll-area'); "
        "a.scrollTop = Math.round((a.scrollHeight - a.clientHeight) * 0.5); "
        "window.__parkedAt = a.scrollTop; return a.scrollTop; })()"
    )
    driver.settle(200)
    # A row that is ALREADY on screen at the parked position. Playwright scrolls an element into
    # view before clicking it, so reaching for one further up would move the reader itself and this
    # state would then be measuring the harness rather than the page. (It did, the first time.)
    driver.js(
        "(() => { const a = document.getElementById('scroll-area').getBoundingClientRect(); "
        "const li = [...document.querySelectorAll('#discord-log > li')].find((n) => { "
        "const r = n.getBoundingClientRect(); return r.top >= a.top && r.bottom <= a.bottom; }); "
        "if (!li) throw new Error('no channel row is fully on screen to reply to'); "
        "li.id = 'reply-probe'; return li.id; })()"
    )
    driver.page.click("#reply-probe .reply-button")
    driver.page.wait_for_function("() => window.__visible('column-reply')", timeout=5_000)
    driver.settle(200)
    # Where the list stands with the reply open: moved, on a short phone, as far as shows the box.
    driver.js(
        "(() => { window.__openedAt = document.getElementById('scroll-area').scrollTop; "
        "return window.__openedAt; })()"
    )
    driver.click("reply-cancel")
    driver.page.wait_for_function("() => !window.__visible('column-reply')", timeout=5_000)
    driver.settle(200)
    driver.js(
        "(() => { window.__returnedTo = document.getElementById('scroll-area').scrollTop; "
        "return window.__returnedTo; })()"
    )
    # ...and open it again, because THAT is the reply this state is a picture of.
    driver.page.click("#reply-probe .reply-button")
    driver.page.wait_for_function("() => window.__visible('column-reply')", timeout=5_000)
    driver.js(
        "(() => { const t = document.getElementById('reply-text'); t.value = "
        "'looking now — the arm64 runner was wedged, not busy'; "
        "t.dispatchEvent(new Event('input', { bubbles: true })); return t.value; })()"
    )
    driver.settle(250)


# A message longer than any frame this harness captures. Posted into the channel through the REAL
# route, with the page's own token, so the thing being replied to is a message the server actually
# holds -- not one injected into the DOM, which would photograph a render of something that does
# not exist. The seeded messages are all comfortably shorter than a phone screen, so without this
# there is no long target to look at.
LONG_CHANNEL_POST = " ".join(
    [
        "The overnight run, in full, because you asked for all of it in one message.",
        "The retry-budget branch landed at 9c07d3e after the jitter ordering was fixed, and the",
        "unrelated cache-key rewrite was pulled back out of it into a draft of its own.",
        "The nightly cache rebuild finished in twenty-two minutes fourteen with three point one",
        "gigabytes written and no evictions, a minute and a half faster than the night before and",
        "inside the ordinary spread either way. The arm64 job is the only thing still outstanding:",
        "it sat queued for forty-one minutes on a runner that was wedged rather than busy, it was",
        "re-fired twice onto the same runner, and it only cleared when the runner was recycled by",
        "hand. Of the two QA findings the first is a genuine flake that turns on wall-clock",
        "ordering between two spawned processes, and the second is a real ordering bug that only",
        "shows up on a loaded machine and was deliberately not labelled flaky, because that is how",
        "a real bug gets ignored. The one thing waiting on you is the release token, which expires",
        "in six days and needs the maintainer account to rotate; if it lapses the release job fails",
        "at the publish step, after the build, which is the most expensive place to find out.",
        "Nothing else is red. Eleven jobs completed, none skipped, four architectures green.",
    ]
)


def _act_reply_long_target(driver: Driver) -> None:
    # NOT the whole channel: this state needs one very long message, which it posts itself, and
    # walking back first only adds rows for Playwright to scroll past on the way to Refresh.
    _act_open_channel(driver)
    posted = driver.page.evaluate(
        "async (text) => {"
        "  const log = document.getElementById('discord-log');"
        "  if ([...log.children].some((n) => n.textContent.includes('in full, because you asked')))"
        "    return 'already there';"
        "  const channel = document.getElementById('discord-channel').value;"
        "  const response = await fetch('/api/v1/channels/' + encodeURIComponent(channel) +"
        "    '/reply', { method: 'POST', headers: {"
        "      Authorization: 'Bearer ' + localStorage.getItem('vibe-talk.token'),"
        "      'Content-Type': 'application/json' },"
        "    body: JSON.stringify({ text }) });"
        "  return response.status;"
        "}",
        LONG_CHANNEL_POST,
    )
    if posted not in ("already there", 200):
        raise Unreachable(
            "19-reply-view-long-target",
            "a long message could be posted into the channel to reply to",
            f"the server answered {posted!r} to the reply route",
        )
    if posted != "already there":
        # Only when there is something new to see. Asking for a re-read when the message is ALREADY
        # rendered means the marker check below is satisfied by the render that is being replaced,
        # and the element this scene then marks is detached from the document a moment later --
        # which is exactly what happened on the second profile of the first run.
        # Called rather than gestured. There is no Refresh BUTTON any more -- `#68 pull-to-refresh`
        # replaced it with the drag, and a drag to the top of the list is itself the gesture that
        # loads older messages, which is not what this state is about. `loadDiscord` is what the
        # released pull ends up calling, so this takes the same path with none of the scrolling.
        driver.page.evaluate("async () => { await window.loadDiscord(); }")
        driver.page.wait_for_function(
            "() => [...document.querySelectorAll('#discord-log > li')]"
            ".some((n) => n.textContent.includes('in full, because you asked'))",
            timeout=10_000,
        )
        driver.settle(400)
    # The LONGEST message now in the channel. Chosen by measuring rather than by index: the seed is
    # the server's, and an index would silently become a short message the day it changes.
    driver.js(
        "(() => { const items = [...document.querySelectorAll('#discord-log > li')]; "
        "const li = items.sort((a, b) => b.textContent.length - a.textContent.length)[0]; "
        "li.id = 'reply-probe'; return li.textContent.length; })()"
    )
    # And the row really is still in the document with a control on it. Marking a detached element
    # would otherwise fail thirty seconds later as an unexplained click timeout.
    driver.page.wait_for_function(
        "() => !!document.querySelector('#reply-probe .reply-button')", timeout=5_000
    )
    # Bring it to the MIDDLE of the list before clicking. The longest message is usually the newest
    # one, which sits in the bottom corner where the floating chips live and where a short phone
    # leaves it under them -- so a click there can be intercepted by something that is not this
    # control. (It was, on the short phone, the first time.)
    driver.js(
        "(() => { const a = document.getElementById('scroll-area'); "
        "const probe = document.getElementById('reply-probe'); "
        "const offset = probe.getBoundingClientRect().top - a.getBoundingClientRect().top; "
        "a.scrollTop = Math.max(0, a.scrollTop + offset - a.clientHeight * 0.4); "
        "return a.scrollTop; })()"
    )
    driver.settle(150)
    # Through the DOM. The longest message is the NEWEST one, so it is the last row, and the last
    # row cannot be brought up the screen -- there is nothing below it to scroll. Its controls
    # therefore sit on the scroll area's bottom edge, where Playwright's own hit test lands on the
    # list rather than on the button. State 18 is where a real pointer clicks a reply control; this
    # state is about what the reply in the list does with a message longer than the frame.
    # Opened first, as a reader opens a long message to read it before answering: a folded one is a few
    # lines, and the box under it would prove nothing about a long target.
    driver.js(
        "(() => { const row = document.getElementById('reply-probe'); "
        "if (row.getAttribute('data-collapsed') === 'true') row.querySelector('.fold').click(); return true; })()"
    )
    driver.settle(150)
    driver.js("(() => { document.querySelector('#reply-probe .reply-button').click(); })()")
    driver.page.wait_for_function("() => window.__visible('column-reply')", timeout=5_000)
    driver.settle(250)


# `#56 message-hover-highlight`. The pointer half of the capability regime, and the ONLY place the
# treatment can be looked at: the page suite can say which media query the rule sits in and nothing
# more, because a fixture with no pointer cannot hover. `10-discord-view` is the without-hover
# control, and the two frames side by side are the evidence.
#
# Desktop only, and that restriction is load-bearing rather than tidy. Chromium's `page.hover()` in
# a `has_touch=True` context can produce a hover state, so without it this scene would run on both
# iPhone profiles and CERTIFY a sticky hover on the device the rule exists to protect.


def _act_discord_hover(driver: Driver) -> None:
    _act_open_channel(driver)
    driver.js(
        "(() => { const rows = [...document.querySelectorAll('#discord-log > li.discord-message')]; "
        "if (rows.length < 3) throw new Error('not enough channel rows to hover one among them'); "
        "rows[2].id = 'hover-probe'; return rows.length; })()"
    )
    driver.page.hover("#hover-probe")
    driver.page.wait_for_function(
        "() => document.getElementById('hover-probe').matches(':hover')", timeout=5_000
    )
    driver.settle(200)


# Hang up is ABSENT, not disabled, whenever there is no call. Asserted as its own expectation on
# every no-call state, because "dimmed but present" was the defect the rework removed and a
# screenshot is the only thing that can tell the two apart.
def _act_restored_conversation(driver: Driver) -> None:
    """`#48 transcript-storage`: a RELOAD, and the conversation is still there.

    The only scene that reloads the page. That is the whole content of the picture -- everything
    before this point in the walk is one continuous session, and a transcript that survives a
    session is not the claim being made. The turns come from the real calls the earlier scenes
    drove, through the real server, into a real SQLite file in the throwaway directory.
    """
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.page.wait_for_function(
        "() => [...document.querySelectorAll('#transcript .seam-label')]"
        ".some((n) => (n.textContent || '').trim() === 'earlier conversation')",
        timeout=10_000,
    )
    driver.settle(400)


def _act_typed_turn(driver: Driver) -> None:
    """`#43 typed-input`, through `#241 voice-composer`'s box: a turn that was TYPED, not spoken.

    Entered from a real live call, because this scene is about a typed message being a client event
    on the conversation socket that is already open — the box during a call, not the typed
    conversation a first send with nothing open starts.

    This scene is about the TRANSCRIPT — that a typed turn lands in it beside the spoken ones, with
    the box under it and the bar, switch and all, still above the big controls rather than over
    them. What the box looks like at idle, focused, is `26-voice-composer`.
    """
    driver.click("talk")
    driver.page.wait_for_function(
        "() => window.__text('talk-label') === 'Listening'", timeout=10_000
    )
    driver.page.fill("#compose-text", "and 9c07d3e — did that land on integration")
    driver.click("send-text")
    driver.page.wait_for_function(
        "() => document.querySelectorAll('#transcript > li.mine').length > 0", timeout=5_000
    )
    # Typing again, so the field in the photograph is not the empty one a send leaves behind.
    driver.page.fill("#compose-text", "and the nightly")
    driver.settle(300)


def _act_control_bar_top(driver: Driver) -> None:
    """`#58 control-bar`: the bar in its OTHER home, under the title.

    Seeded through storage rather than reached by clicking, because a reader who has chosen top
    gets it from the first frame — which is the thing worth photographing — and because clicking
    it would make this a picture of the settings screen instead.
    """
    driver.load(with_token=True, placement="top")
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.settle(300)


def _act_control_bar_bottom(driver: Driver) -> None:
    """`#58 control-bar`: the default home, reached THROUGH the setting that chooses it.

    Entered from the state above, so this photograph is also the proof that the Settings control
    really moves the bar rather than merely remembering a word — the scene before it left the bar
    in the header.
    """
    driver.click("open-settings")
    driver.page.wait_for_function("() => window.__visible('close-settings')", timeout=5_000)
    _unfold(driver, "bar-placement")
    driver.page.select_option("#bar-placement", "bottom")
    driver.click("close-settings")
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=5_000)
    driver.settle(300)


def _act_voice_composer(driver: Driver) -> None:
    """`#241 voice-composer`: the call's view with its message box focused and a draft in it.

    The owner's report was that typing took the Voice/Channel switch away: `#59 text-entry-button`
    converted the whole bar into a text field. The subject here is the replacement — the box at the
    foot of the transcript, as the channel's is, with the bar left exactly as it rests. Taken at
    IDLE and with the box focused, because clicking into it and writing must open nothing: the
    typed conversation starts on the first send, not on the focus.
    """
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.page.click("#compose-text")
    # `fill`, not typing: the scene before left a draft in the box (`#193 compose-draft-loss` keeps
    # it across the reload), and keys would add to it rather than replace it.
    driver.page.fill("#compose-text", "summarize the last hour")
    driver.settle(300)


def _act_canned_prompts(driver: Driver) -> None:
    """`#60 canned-prompt-buttons`: the bar packed, with the tray of prompts open above it.

    The subject is PACKING — gear, Prompts, Read new and the switch on one strip, and the tray
    hanging above it — and the iphone-se profile is the frame that answers whether that survives a
    short, narrow phone. (Type was on the strip too until `#241 voice-composer` moved writing into
    the transcript.)

    The tray is opened here rather than left shut because it is the half no fake DOM can check: it
    is absolutely positioned, and whether an out-of-flow panel lands over the transcript or off the
    side of a 375px screen is a question only a layout engine has an answer to.
    """
    driver.page.wait_for_function("() => window.__visible('view-switch')", timeout=5_000)
    driver.click("prompts-open")
    driver.page.wait_for_function("() => window.__visible('canned-blockers')", timeout=5_000)
    driver.settle(300)


NO_HANGUP = ("Hang up is absent, not merely dimmed", "!window.__visible('hang-up')")

# `#58 control-bar` put the bar directly ABOVE the big buttons, and three states measure that.
# `#218 desktop-dock` lays the dock on a desk out as one wrapping row, where the bar and the pane's
# buttons share a row when they fit: there the claim is that the bar is BESIDE the pane, to its left.
# Either way the bar is never over the controls, which is the defect those measurements exist for.
# `b` and `p` are the bar's and the pane's boxes; the desktop query is the one web/voice.css uses.
BAR_CLEAR_OF_PANE_JS = (
    "(b.bottom <= p.top + 1 || (matchMedia('(min-width: 900px) and (pointer: fine)').matches "
    "&& b.right <= p.left + 1 && b.bottom > p.top && b.top < p.bottom))"
)

# `#218 desktop-dock`. The controls the dock is showing, without the tray and the pace popover that
# hang out of it, for the desktop dock states.
DOCK_CONTROLS_JS = (
    "[...document.querySelectorAll('#dock button, #dock select')].filter((n) => "
    "n.getClientRects().length > 0 && !n.closest('#prompts-tray') && !n.closest('#speed-popover'))"
)


def dock_one_row_js() -> str:
    """`#406 command-bar-one-row`: every control in the dock on one row, inside the window less the
    dock's padding, and no two overlapping. It was held to the reading column, and wrapped there."""
    return (
        "(() => { const d = document.getElementById('dock'), s = getComputedStyle(d), "
        "dr = d.getBoundingClientRect(); "
        "const col = {left: dr.left + parseFloat(s.paddingLeft), right: dr.right - parseFloat(s.paddingRight)}; "
        f"const boxes = {DOCK_CONTROLS_JS}.map((n) => n.getBoundingClientRect()); "
        "const mids = boxes.map((r) => (r.top + r.bottom) / 2); "
        "return boxes.length > 0 && Math.max(...mids) - Math.min(...mids) <= 4 "
        "&& boxes.every((r) => r.left >= col.left - 1 && r.right <= col.right + 1) "
        "&& boxes.every((a, i) => boxes.every((b, j) => j <= i "
        "|| Math.min(a.right, b.right) - Math.max(a.left, b.left) <= 0.5 "
        "|| Math.min(a.bottom, b.bottom) - Math.max(a.top, b.top) <= 0.5)); })()"
    )


def pane_reads_js(order: str) -> str:
    """The pane's shown buttons, left to right, are `order` (ids joined by commas)."""
    return (
        "[...document.getElementById('control-pane').children].filter((n) => n.matches('button') "
        "&& n.getClientRects().length > 0).sort((a, b) => a.getBoundingClientRect().left - "
        f"b.getBoundingClientRect().left).map((n) => n.id).join(',') === '{order}'"
    )


# Ordinary desktop controls, which is the whole of the owner's complaint about the thumb-sized dock:
# 32px at least, a mouse's target, and no taller than 40px.
DESK_CONTROL_SIZES = (
    "every control in the dock is an ordinary desktop size: 32 to 40px tall, at least 32px wide",
    f"{DOCK_CONTROLS_JS}.every((n) => {{ const r = n.getBoundingClientRect(); "
    "return r.height >= 32 && r.height <= 40.5 && r.width >= 32; })",
)
# Two rows of those at the most, with their gap and padding. The dock stood 149px before.
DESK_DOCK_HEIGHT = (
    "the dock is at most two rows of them, not the 149px of thumb-sized tiles it was",
    "document.getElementById('dock').getBoundingClientRect().height <= 100",
)
# The switch's one place on a desk: the dock's lower right corner — the window's, since `#406
# command-bar-one-row` — at the end of its row,
# whatever the view and whether there is a call. It moved with the row, and the second click of a
# flick between the views met an empty dock.
DESK_SWITCH_CORNER = (
    "the Voice/Channel switch is in the dock's lower right corner, at the end of its row",
    "(() => { const d = document.getElementById('dock'), s = getComputedStyle(d), r = d.getBoundingClientRect(); "
    "const w = document.getElementById('view-switch').getBoundingClientRect(); "
    "return Math.abs(w.right - (r.right - parseFloat(s.paddingRight))) <= 0.5 "
    "&& Math.abs(w.bottom - (r.bottom - parseFloat(s.paddingBottom))) <= 0.5; })()",
)


def _act_desktop_dock_call(driver: Driver) -> None:
    """`#218 desktop-dock`: the dock during a live call on a desk — Prompts and Gist on the bar,
    Sound, Clear, Hang up and Listening beside it."""
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("talk")
    driver.page.wait_for_function("() => window.__text('talk-label') === 'Listening'", timeout=10_000)
    driver.settle(300)

def _act_live_message(driver: Driver) -> None:
    """A message arrives on the stream while the reader is looking at the channel.

    No Refresh click anywhere in here, and that is the whole subject: the list has to grow because
    the server told the page, not because the page asked again. The 45-second background re-read
    is far longer than this scene takes, so a row appearing within it cannot be that.
    """
    _act_open_channel(driver)
    # Counting the RE-READ, not a button. There is no Refresh button left to attach to -- `#68
    # pull-to-refresh` replaced it -- and counting `loadDiscord` is the better instrument anyway:
    # the claim this scene rests on is that nothing went back to the channel and asked, and that
    # is true of the gesture, the background timer and the vanished button alike. A listener on
    # one control could only ever have spoken for that control.
    driver.js(
        "(() => { window.__loadedBefore = "
        "document.querySelectorAll('#discord-log > li').length; "
        "window.__refreshes = 0; "
        "const real = window.loadDiscord; "
        "window.loadDiscord = function (...args) { window.__refreshes += 1; "
        "return real.apply(this, args); }; "
                "return window.__loadedBefore; })()"
    )
    driver.js(
        "window.__channelSays('9990000000000000001', 'claude-integ', "
        "'the arm64 runner came back by itself -- 11 jobs completed, 0 skipped, and the tag is cut')"
    )
    driver.page.wait_for_function(
        "() => document.querySelectorAll('#discord-log > li').length > window.__loadedBefore",
        timeout=10_000,
    )
    driver.settle(250)


def _act_resume_partial(driver: Driver) -> None:
    """`#46 conversation-replay`: the post-call pane when the reconstruction was only PARTIAL.

    The state worth photographing is not "resuming worked"; it is the sentence that stops the
    interface claiming more than it did. So the run below deliberately produces a dropped-turn
    replay and lands on the idle pane, where the clause under the large control has to say "in
    part" rather than "replayed".

    A whole call first, because `resumeConversationId` comes from the store: the page learns which
    conversation a new call would resume from by restoring the most recent one, and a scene that
    assumed an earlier scene had left one behind would fail differently depending on `--only`.
    """
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("talk")
    driver.page.wait_for_function(
        "() => window.__text('talk-label') === 'Listening'", timeout=10_000
    )
    driver.js("window.__userSays('did the overnight run finish')")
    driver.js("window.__agentSays('It did. integration is at 9c07d3e and the tip is green.')")
    driver.settle(200)
    driver.click("hang-up")
    driver.page.wait_for_function(
        "() => window.__text('talk-label') === 'Start a new call'", timeout=5_000
    )

    # Reload, so the page restores that conversation and knows what a new call would resume from.
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.page.wait_for_function(
        "() => (window.__text('resume-state') || '').length > 0", timeout=10_000
    )
    # Through the DOM: the toggle lives on the Settings screen, and Playwright would scroll and
    # click its way there -- which would leave the walk on the wrong screen for the picture.
    driver.js(
        "(() => { const t = document.getElementById('resume-toggle'); t.checked = true; "
        "t.dispatchEvent(new Event('change')); return t.checked; })()"
    )
    driver.page.wait_for_function(
        "() => /next call/.test(window.__text('resume-state') || '')", timeout=5_000
    )

    driver.click("talk")
    driver.page.wait_for_function(
        "() => window.__text('talk-label') === 'Listening'", timeout=10_000
    )
    driver.js("window.__agentSays('Carrying on — nothing else has gone red since.')")
    driver.settle(200)
    driver.click("hang-up")
    driver.page.wait_for_function(
        "() => /in part/.test(window.__text('talk-note') || '')", timeout=5_000
    )
    driver.settle()


# `#228 reply-visible`. --fake-discord makes the tenth seeded message answer the fourth, from far
# below it, and the sixth answer the fifth, from directly under it (src/main.rs). The scene puts a
# second reply to the fourth on the stream.
REPLY_MARKER_PARENT = "seeded: re-fired. Same runner, same stall."
REPLY_MARKER_ADJACENT = "seeded: review of the retry-budget branch."
REPLY_MARKER_FIRST_REPLY = "seeded: landed. integration is at 9c07d3e"
REPLY_MARKER_ARRIVAL = "9990000000000000228"

# A row by the words it says, asked afresh by every check: the list is drawn again as messages
# arrive, and an id the walk gave a row would not survive that.
REPLY_ROW_JS = (
    "window.__replyRow = (text) => [...document.querySelectorAll('#discord-log > li')]"
    ".find((n) => (n.textContent || '').includes(text)) || null"
)

# Where the circle on a row is, and the disc it draws: its centre, and the disc's radius, in pixels.
REPLY_DISC_JS = (
    "window.__replyDisc = (row) => { const m = row && row.querySelector(':scope > .reply-marker'); "
    "if (!m) return null; const b = m.getBoundingClientRect(); "
    "const r = parseFloat(getComputedStyle(m, '::before').width) / 2; "
    "return { marker: m, x: b.left + b.width / 2, y: b.top + b.height / 2, r }; }"
)


def _act_reply_marker(driver: Driver) -> None:
    """`#228 reply-visible`: Reply (N) on a message that has replies, and the circle on its corner.

    The whole channel, then a second reply to the fourth seeded message arriving on the stream, as
    a reply of the owner's would: the fourth's Reply reads Reply (2), and the circle on its
    lower-left corner is the other end of the two disconnected arrows further down. The fifth,
    answered only from directly under it, reads Reply (1), and has no circle: the arrow that rises
    from the sixth already meets its foot. The fourth is brought up under the floating line, so the
    three are on the screen together.
    """
    _act_whole_channel(driver)
    driver.js(REPLY_ROW_JS)
    driver.js(REPLY_DISC_JS)
    driver.js(
        f"window.__channelSays({json.dumps(REPLY_MARKER_ARRIVAL)}, 'claude-integ', "
        "'confirmed: the second mac is back in the pool and the arm64 queue is draining', false, "
        f"window.__replyRow({json.dumps(REPLY_MARKER_PARENT)}).getAttribute('data-id'))"
    )
    driver.page.wait_for_function(
        f"() => {{ const row = window.__replyRow({json.dumps(REPLY_MARKER_PARENT)}); "
        "return row && row.querySelector('.reply-button').textContent === 'Reply (2)'; }",
        timeout=10_000,
    )
    driver.js(
        f"(() => {{ const row = window.__replyRow({json.dumps(REPLY_MARKER_PARENT)}); "
        "const area = document.getElementById('scroll-area'); "
        "area.scrollTop += row.getBoundingClientRect().top - area.getBoundingClientRect().top - 64; "
        "return area.scrollTop; })()"
    )
    # The walk's "fetching the channel…" is still on the status strip, over the rows below; the
    # reader would tap it away, and so does this.
    driver.js(
        "(() => { if (window.__visible('status-line')) "
        "document.getElementById('dismiss-status').click(); return true; })()"
    )
    driver.settle(300)


def _act_reply_marker_jump(driver: Driver) -> None:
    """`#228 reply-visible`: a real click on the circle, and where it lands.

    The first tap goes to the earliest of the two replies, the seeded one, which comes to the head
    of the list lit, as a tap on an arrow brings what it answers; the circle now says the next tap
    goes to the second.
    """
    _act_reply_marker(driver)
    driver.js(
        f"(() => {{ const row = window.__replyRow({json.dumps(REPLY_MARKER_PARENT)}); "
        "row.querySelector(':scope > .reply-marker').id = 'reply-marker-probe'; return true; })()"
    )
    driver.click("reply-marker-probe")
    driver.page.wait_for_function(
        f"() => {{ const row = window.__replyRow({json.dumps(REPLY_MARKER_FIRST_REPLY)}); "
        "return row && row.getAttribute('data-landed') === 'true'; }",
        timeout=5_000,
    )
    # The landing's light fades over two seconds, and a capture with animations disabled would
    # show it already out: held at its start instead, as the reader sees it on arriving.
    driver.js(
        "(() => { const lit = document.getAnimations().filter((a) => a.animationName === 'reply-landed'); "
        "lit.forEach((a) => { a.pause(); a.currentTime = 0; }); return lit.length; })()"
    )
    # The pointer is left where the circle was, over whatever row the jump scrolled there, which a
    # desk would draw as hovered. Moved to the empty margin, as a hand would move on.
    driver.page.mouse.move(2, 200)
    driver.settle(300)


def _act_reply_marker_beside_arrow(driver: Driver) -> None:
    """`#228 reply-visible`: a circle and an arrow at one corner.

    `42-reply-count-and-marker`'s state, then a second reply to the fifth seeded message on the stream:
    answered from directly below and from further away, it has the head of the reply below's arrow and
    the circle at the one corner, each keeping to its own place.
    """
    _act_reply_marker(driver)
    driver.js(
        "window.__channelSays('9990000000000000229', 'codex-review', "
        "'and the same ordering bug is in the cache client', false, "
        f"window.__replyRow({json.dumps(REPLY_MARKER_ADJACENT)}).getAttribute('data-id'))"
    )
    driver.page.wait_for_function(
        f"() => {{ const row = window.__replyRow({json.dumps(REPLY_MARKER_ADJACENT)}); "
        "return row && row.querySelector('.reply-button').textContent === 'Reply (2)' "
        "&& row.querySelector(':scope > .reply-marker') !== null; }",
        timeout=10_000,
    )
    driver.js(
        f"(() => {{ const row = window.__replyRow({json.dumps(REPLY_MARKER_PARENT)}); "
        "const area = document.getElementById('scroll-area'); "
        "area.scrollTop += row.getBoundingClientRect().top - area.getBoundingClientRect().top - 64; "
        "if (window.__visible('status-line')) document.getElementById('dismiss-status').click(); "
        "return area.scrollTop; })()"
    )
    driver.settle(300)


def _set_bookmark_from_end(driver: Driver, back: int) -> None:
    """`#240 reading-bookmark`: the bookmark above the row `back` rows from the newest, set with that
    row's ⋯ item through the DOM (a real click would scroll the row into view), and settled: the
    line drawn and the write answered, so the picture is of a bookmark the server holds. The whole
    channel first: the server pages it six at a time, and the chip's picture needs a list taller than
    two screens."""
    _act_whole_channel(driver)
    driver.js(
        "(() => { const rows = [...document.querySelectorAll('#discord-log > li[data-id]')]; "
        f"const row = rows[Math.max(0, rows.length - 1 - {back})]; row.id = 'bookmark-probe'; "
        "row.querySelector('.row-bookmark-button').click(); return row.getAttribute('data-id'); })()"
    )
    driver.page.wait_for_function(
        "() => { const r = document.getElementById('bookmark-probe'); "
        "return r && r.getAttribute('data-bookmark') === 'above' && r.querySelector(':scope > .bookmark-line') "
        "&& /Bookmarked/.test(window.__text('status') || ''); }",
        timeout=10_000,
    )
    driver.settle(300)


def _act_reading_bookmark(driver: Driver) -> None:
    """`#240 reading-bookmark`: the line above a message near the foot of the channel, a third of the
    way down the list, so the rows on either side of it are in the picture."""
    _set_bookmark_from_end(driver, 3)
    driver.js(
        "(() => { const area = document.getElementById('scroll-area'); "
        "const line = document.querySelector('#bookmark-probe > .bookmark-line'); "
        "area.scrollTop += line.getBoundingClientRect().top - area.getBoundingClientRect().top "
        "- area.clientHeight / 3; return true; })()"
    )
    driver.settle(400)


def _act_bookmark_chip(driver: Driver) -> None:
    """`#240 reading-bookmark`: the line scrolled away above and the newest message below, so the
    jump to each is up at once: Newest at the left of the chips, the Bookmark chip right beside it."""
    _set_bookmark_from_end(driver, 12)
    driver.js(
        "(() => { const area = document.getElementById('scroll-area'); "
        "const line = document.querySelector('#bookmark-probe > .bookmark-line'); "
        "const below = line.getBoundingClientRect().bottom - area.getBoundingClientRect().top; "
        "area.scrollTop += below + 40; "
        "area.dispatchEvent(new Event('scroll')); return true; })()"
    )
    driver.page.wait_for_function(
        "() => window.__visible('jump-bookmark') && window.__visible('jump-newest')", timeout=5_000
    )
    driver.settle(300)


def _act_settings_text_size(driver: Driver) -> None:
    """`#230 text-size-buttons`: the Text size group, a − and a + either side of its slider.

    A fresh load (the size is a stored preference, cleared by `load`), Settings, and the group in the
    middle of the screen. Then + is really clicked once, so the picture shows what a press does (the
    slider a step along, the line under it saying so), and the keyboard backs onto − from the slider
    with Shift+Tab, so the picture shows its focus ring too.
    """
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("open-settings")
    driver.page.wait_for_function("() => window.__visible('close-settings')", timeout=5_000)
    _unfold(driver, "msg-scale")
    driver.js(
        "(() => { document.getElementById('msg-scale').closest('.settings-group')"
        ".scrollIntoView({ block: 'center' }); return true; })()"
    )
    driver.click("msg-scale-larger")
    driver.js("(() => { document.getElementById('msg-scale').focus(); return true; })()")
    driver.page.keyboard.press("Shift+Tab")
    driver.settle(300)


# `#193 compose-draft-loss`. What the refused send and the unsent reply say, written to be read in
# the picture rather than to be pithy.
REFUSED_SEND_TEXT = "the arm64 runner is back; rerunning the release job now"
DRAFT_REPLY_TEXT = "half an answer about the cache rebuild, to finish after lunch"
REFUSED_SEND_ANSWER = json.dumps(
    {"error": "refused", "detail": "This bot is not allowed to post in this channel."}
)


def _refuse_send(route: Route) -> None:
    route.fulfill(status=403, content_type="application/json", body=REFUSED_SEND_ANSWER)


def _act_failed_send_and_draft(driver: Driver) -> None:
    """`#193 compose-draft-loss`: a send the chat service refused, in the outbox with Retry, Edit and
    Dismiss, and the newest message with Draft beside Reply, for a reply left unsent.

    The refusal is this state's one faked answer: the reply route answers 403 while the message is
    sent, so nothing is posted to the throwaway server. The draft is typed into Reply and left there
    by its Back, as a reader leaves one.
    """
    _act_open_channel(driver)
    driver.page.route("**/api/v1/channels/*/reply", _refuse_send)
    try:
        driver.page.fill("#channel-compose-text", REFUSED_SEND_TEXT)
        driver.click("channel-send")
        driver.page.wait_for_function(
            "() => document.querySelector('#outgoing-log > li[data-send-state=\"failed\"]') !== null",
            timeout=10_000,
        )
    finally:
        driver.page.unroute("**/api/v1/channels/*/reply", _refuse_send)
    driver.js(
        "(() => { const rows = document.querySelectorAll('#discord-log > li'); "
        "rows[rows.length - 1].id = 'draft-probe'; return true; })()"
    )
    # `#377 compact-collapsed-rows`. A collapsed row on a phone draws no Reply until it is opened: pressed
    # as r presses it, so the row stays as the reader left it, collapsed, with its Draft on the header.
    driver.js("(() => { document.querySelector('#draft-probe .reply-button').click(); return true; })()")
    driver.page.wait_for_function("() => window.__visible('column-reply')", timeout=5_000)
    driver.page.fill("#reply-text", DRAFT_REPLY_TEXT)
    driver.click("reply-cancel")
    driver.page.wait_for_function("() => !window.__visible('column-reply')", timeout=5_000)
    driver.js(
        "(() => { const a = document.getElementById('scroll-area'); a.scrollTop = a.scrollHeight; "
        "if (window.__visible('status-line')) document.getElementById('dismiss-status').click(); "
        "return true; })()"
    )
    driver.settle(300)


def _act_send_dismissed(driver: Driver) -> None:
    """`#193 compose-draft-loss`: the refused send dismissed, and the Undo chip that names it."""
    _act_failed_send_and_draft(driver)
    driver.page.click("#outgoing-log .outgoing-dismiss")
    driver.page.wait_for_function("() => window.__visible('undo-send-dismiss')", timeout=5_000)
    driver.settle(300)


def _act_recently_dismissed(driver: Driver) -> None:
    """`#193 compose-draft-loss`: Settings' Recently dismissed, holding the send just dismissed."""
    _act_send_dismissed(driver)
    driver.click("open-settings")
    driver.page.wait_for_function("() => window.__visible('close-settings')", timeout=5_000)
    _unfold(driver, "dismissed-sends")
    driver.js(
        "(() => { document.getElementById('dismissed-sends').closest('.settings-group')"
        ".scrollIntoView({ block: 'center' }); return true; })()"
    )
    driver.settle(300)


# `#348 settings-dynamic-lists`: the sends dismissed for the list, oldest first, one of them long enough
# that its row's one line is cut and its pop-out has to scroll on a phone.
DISMISSED_LIST_TEXTS = (
    "rerun the flaky shard before the release cut",
    "the cache rebuild is done; the warm start is back under a minute",
    "ask the runner owner whether arm64 can take the nightly as well",
    "Long note for later, kept in case it is wanted. " + ("The integration shard was retried twice and "
    "passed on the second attempt; the artifact upload waited on the cache for most of an hour, and "
    "the release job should not be cut until the upload settles. ") * 4,
    "ping me when the nightly is green",
    "the arm64 runner is back; rerunning the release job now",
)


def _dismiss_sends(driver: Driver, texts: Iterable[str]) -> None:
    """Send each of `texts` in the channel, have the chat service refuse it, and dismiss it unsent."""
    driver.page.route("**/api/v1/channels/*/reply", _refuse_send)
    try:
        for text in texts:
            driver.page.fill("#channel-compose-text", text)
            driver.click("channel-send")
            driver.page.wait_for_function(
                "() => document.querySelector('#outgoing-log > li[data-send-state=\"failed\"]') !== null",
                timeout=10_000,
            )
            driver.page.click("#outgoing-log > li[data-send-state='failed'] .outgoing-dismiss")
            driver.page.wait_for_function(
                "() => document.querySelector('#outgoing-log > li[data-send-state=\"failed\"]') === null",
                timeout=5_000,
            )
    finally:
        driver.page.unroute("**/api/v1/channels/*/reply", _refuse_send)


def _act_settings_dismissed_list(driver: Driver) -> None:
    """`#348 settings-dynamic-lists`: Settings' Recently dismissed as a data panel of six sends.

    Six sends refused by the chat service and dismissed unsent (the reply route is this state's one
    faked answer), then Settings with the group scrolled to the top: a dark, monospaced panel of a
    fixed height, scrolling inside itself, each row where it was going, how long ago, and one line.
    """
    _act_open_channel(driver)
    _dismiss_sends(driver, DISMISSED_LIST_TEXTS)
    driver.click("open-settings")
    driver.page.wait_for_function("() => window.__visible('close-settings')", timeout=5_000)
    driver.page.wait_for_function("() => document.querySelectorAll('#dismissed-sends > li').length === 6", timeout=5_000)
    _unfold(driver, "dismissed-sends")
    driver.js(
        "(() => { if (window.__visible('status-line')) document.getElementById('dismiss-status').click(); "
        "document.getElementById('dismissed-sends').closest('.settings-group').scrollIntoView({ block: 'start' }); "
        "return true; })()"
    )
    driver.settle(300)


# What a tap or a Tab could land on, for the two checks of the pop-out's dim below.
POPOUT_DIM_CONTROLS = 'button, a, input, select, textarea, label, [tabindex]:not([tabindex="-1"])'

# `#351 settings-polish`. Each control on screen outside the pop-out, with the part of it the window and
# every box around it that clips leave showing, and where the pop-out is: CSS pixels, left, top, right,
# bottom. In document order, so the same page lists the same controls in the same order.
POPOUT_DIM_LAYOUT_JS = (
    "(() => { const pop = document.getElementById('dismissed-send-popout'); "
    "const p = pop.getBoundingClientRect(); const controls = []; "
    f"for (const c of document.querySelectorAll('{POPOUT_DIM_CONTROLS}')) {{ "
    "if (pop.contains(c) || !c.checkVisibility({ visibilityProperty: true })) continue; "
    "const r = c.getBoundingClientRect(); let box = [r.left, r.top, r.right, r.bottom]; "
    "for (let a = c.parentElement; a; a = a.parentElement) { const s = getComputedStyle(a); "
    "if (s.overflowX === 'visible' && s.overflowY === 'visible') continue; "
    "const q = a.getBoundingClientRect(); "
    "box = [Math.max(box[0], q.left), Math.max(box[1], q.top), Math.min(box[2], q.right), Math.min(box[3], q.bottom)]; } "
    "box = [Math.max(box[0], 0), Math.max(box[1], 0), Math.min(box[2], innerWidth), Math.min(box[3], innerHeight)]; "
    "if (box[2] - box[0] < 2 || box[3] - box[1] < 2) continue; "
    "controls.push({ name: c.id || `${c.tagName.toLowerCase()} \"${c.textContent.trim().slice(0, 30)}\"`, box }); } "
    "return { controls, popout: [p.left, p.top, p.right, p.bottom] }; })()"
)

# How far the pop-out's own drop shadow (`0 0.6rem 2rem` in web/voice.css), which is deeper than the dim,
# reaches past its edges at the page's 16px rem: up, to each side and down. A point there is left out.
POPOUT_SHADOW_REACH_PX = (24, 34, 44)

# The share of its brightness a control keeps under the dim: 0.55, the shadow's `rgb(0 0 0 / 0.45)`.
# Under no dim it keeps all of it, and under two about 0.3, so the band takes neither.
POPOUT_DIM_BAND = (0.40, 0.70)


class DimControl(TypedDict):
    name: str
    box: list[float]


class DimLayout(TypedDict):
    controls: list[DimControl]
    popout: list[float]


def popout_dim_misses(lit_layout: DimLayout, lit: Image, dim_layout: DimLayout, dim: Image) -> tuple[list[str], list[str]]:
    """Which controls on screen outside the pop-out are not under its dim, by their pixels.

    `#351 settings-polish`. Each control as drawn with the pop-out up (`dim`) against the same control
    with none up (`lit`), over the part of it not under the pop-out or the pop-out's drop shadow: its
    brightness under the dim must be a share of it within POPOUT_DIM_BAND. A hit test cannot ask this.
    `elementFromPoint` skips an inert element, so over a bright Back nothing can press it says only that
    nothing is pressed. Returns the controls judged and, for each one that is not under the dim, why.
    """
    judged: list[str] = []
    misses: list[str] = []
    if len(lit_layout["controls"]) != len(dim_layout["controls"]):
        return judged, [f"{len(dim_layout['controls'])} controls on screen with the pop-out up and "
                        f"{len(lit_layout['controls'])} before it opened"]
    left, top, right, bottom = dim_layout["popout"]
    up, side, down = POPOUT_SHADOW_REACH_PX
    low, high = POPOUT_DIM_BAND

    def light(image: Image, x: int, y: int) -> float:
        red, green, blue = image.pixels[y * image.width + x][:3]
        return 0.299 * red + 0.587 * green + 0.114 * blue

    for lit_control, dim_control in zip(lit_layout["controls"], dim_layout["controls"]):
        name = dim_control["name"]
        if lit_control["name"] != name or any(abs(a - b) > 1 for a, b in zip(lit_control["box"], dim_control["box"])):
            misses.append(f"{name} moved when the pop-out opened")
            continue
        x0, y0, x1, y1 = (int(round(value)) for value in dim_control["box"])
        lit_sum = dim_sum = 0.0
        for y in range(y0, min(y1, dim.height, lit.height), 2):
            for x in range(x0, min(x1, dim.width, lit.width), 2):
                if left - side <= x <= right + side and top - up <= y <= bottom + down:
                    continue
                lit_sum += light(lit, x, y)
                dim_sum += light(dim, x, y)
        if lit_sum == 0:
            continue  # wholly under the pop-out or its own shadow, or drawn in nothing but black
        judged.append(name)
        share = dim_sum / lit_sum
        if not low <= share <= high:
            misses.append(f"{name} keeps {share:.2f} of its brightness")
    return judged, misses


# `#351 settings-polish`, from the review of its second round. Where the pop-out, Settings' scrolling box
# and the status pill are: CSS pixels, left, top, right, bottom.
POPOUT_FOOT_LAYOUT_JS = (
    "(() => { const at = (id) => { const b = document.getElementById(id).getBoundingClientRect(); "
    "return [b.left, b.top, b.right, b.bottom]; }; "
    "return { popout: at('dismissed-send-popout'), box: at('screen-settings'), pill: at('status-line') }; })()"
)

# How far above the foot of Settings' box Recently dismissed's top edge is put before the long message is
# opened, so that opening it brings the pop-out on screen by its foot (`scrollIntoView`, `block: 'nearest'`).
POPOUT_FOOT_LEAD_PX = 40

# ...and how far above that foot its own foot may then be and still stand there: the box's padding at its
# foot (`1rem`), which the page cannot scroll past when Recently dismissed is near the end of Settings.
POPOUT_FOOT_SLACK_PX = 16

# The folds of the Messages section above Recently dismissed, in their order: opened by the foot scene
# where the folded screen is too short to bring the group low (`#372 settings-header-layout`).
# `#402 who-is-who-channel-box` took Who is who out of Messages, and its fold with it.
DISMISSED_ABOVE_FOLDS = ("mark-own", "auto-read", "empty-messages", "combining", "rich-headers")

# Said by the pill while the pop-out stands at the foot: long enough to draw the pill at its widest, as a
# long channel name or a long error does, so it reaches into the pop-out wherever the column puts it.
POPOUT_FOOT_STATUS = (
    "A status long enough to draw the pill at its widest, as a long channel name or a long error does, "
    "held up while the pop-out stands at the foot of the screen."
)

# How far in from each edge of the pop-out the comparison starts: its rounded corners (`0.8rem` at the
# page's 16px rem), where, outside the curve, the shadow and the pill under it show within its rectangle.
POPOUT_CORNER_PX = 13

# A change in any colour channel larger than this is a pixel of the pop-out drawn differently.
POPOUT_COVER_TOLERANCE = 8


class FootLayout(TypedDict):
    popout: list[float]
    box: list[float]
    pill: list[float]


def popout_covered(bare: Image, pilled: Image, popout: list[float], pill: list[float]) -> list[str]:
    """Whether anything is drawn over the pop-out where the status pill floats under it, by its pixels.

    `#351 settings-polish`, from the review of its second round. The pop-out as drawn with the pill
    hidden (`bare`) and with it up (`pilled`), where it stands in both: inside it, short of its rounded
    corners, no pixel may change, because a modal is drawn over the pill and not the pill over it. The
    pill must reach into that part of the pop-out, or the pictures say nothing. Returns why the pop-out
    is not on top, or nothing.
    """
    x0, y0, x1, y1 = (int(round(value)) for value in popout)
    x0, y0, x1, y1 = x0 + POPOUT_CORNER_PX, y0 + POPOUT_CORNER_PX, x1 - POPOUT_CORNER_PX, y1 - POPOUT_CORNER_PX
    x1, y1 = min(x1, bare.width, pilled.width), min(y1, bare.height, pilled.height)
    reach_x = min(x1, int(round(pill[2]))) - max(x0, int(round(pill[0])))
    reach_y = min(y1, int(round(pill[3]))) - max(y0, int(round(pill[1])))
    if reach_x <= 0 or reach_y <= 0:
        return [f"the pill ({[round(value) for value in pill]}) does not reach into the pop-out "
                f"({[round(value) for value in popout]}), so nothing here says which is drawn on top"]
    changed = 0
    first: tuple[int, int] | None = None
    for y in range(max(y0, 0), y1):
        for x in range(max(x0, 0), x1):
            before = bare.pixels[y * bare.width + x]
            after = pilled.pixels[y * pilled.width + x]
            if any(abs(a - b) > POPOUT_COVER_TOLERANCE for a, b in zip(before[:3], after[:3])):
                changed += 1
                first = first or (x, y)
    if changed:
        return [f"{changed} pixels of the pop-out change when the pill comes up under it, the first at {first}: "
                "the pill is drawn over the pop-out"]
    return []


def _act_settings_dismissed_popout(driver: Driver) -> None:
    """`#348 settings-dynamic-lists`: the long dismissed send opened in its pop-out, whole, with Copy.

    `#351 settings-polish`: the pop-out is a modal, so the real Tab key is walked round it, four Tabs
    and four Shift+Tabs from Copy, recorded for the check (`__popoutTabbed`), which leaves Copy with the
    focus again; and every point of the dim outside it is asked what a tap there would land on
    (`__dimHits`): never a control behind it. Before that, with the status pill put up and held past
    its six seconds, each control on screen is measured in two screenshots, before a pop-out opens and
    with one up: each outside it, the top bar's Back and the pill's x among them, must be drawn under the
    dim, once (`__dimJudged`, `__dimMisses`, `popout_dim_misses`). That pop-out is the top row's, a short
    message: the long one's is brought on screen by scrolling, which would move every control between
    the two pictures, and the comparison is of each control where it stands.

    From the review of the second round: the long one is opened with Recently dismissed low on the
    screen, so opening it brings it on screen by its foot, its buttons where the pill floats, and it is
    photographed so. There it is drawn with the pill hidden and with the pill up at its widest: nothing
    inside it may change (`__footCovered`, `popout_covered`). The pill drawn over Copy, Put back and
    Close at the foot is what keeping the shadow off the pill once did.
    """
    _act_settings_dismissed_list(driver)
    driver.js("(() => { setStatus('Ready.'); clearTimeout(statusTimer); return true; })()")
    driver.page.mouse.move(1, 1)
    driver.settle(200)
    lit_layout = cast(DimLayout, driver.js(POPOUT_DIM_LAYOUT_JS))
    lit = decode_png(driver.page.screenshot(scale="css", animations="disabled"))
    driver.page.click("#dismissed-sends > li:nth-child(1) .dismissed-send-open")
    driver.page.wait_for_function("() => window.__visible('dismissed-send-popout')", timeout=5_000)
    driver.settle(200)
    dim_layout = cast(DimLayout, driver.js(POPOUT_DIM_LAYOUT_JS))
    dim = decode_png(driver.page.screenshot(scale="css", animations="disabled"))
    judged, misses = popout_dim_misses(lit_layout, lit, dim_layout, dim)
    driver.page.evaluate("([judged, misses]) => { window.__dimJudged = judged; window.__dimMisses = misses; }",
                         [judged, misses])
    driver.click("dismissed-send-close")
    driver.page.wait_for_function("() => !window.__visible('dismissed-send-popout')", timeout=5_000)
    # `#372 settings-header-layout` made the head of Settings one short row, and on a 1920px desk the
    # screen in four columns, folded, is then too short to bring the group low: it cannot scroll above its
    # top, and the pop-out opened there fits above the foot. There the Messages groups above Recently
    # dismissed are opened as well, one at a time, until the group can stand low; where it already can,
    # as on a phone and a 1280px desk, nothing more is opened and the picture is as it was.
    for slug in DISMISSED_ABOVE_FOLDS:
        low = driver.js(
            "(() => { const box = document.getElementById('screen-settings'); "
            "const group = document.getElementById('dismissed-group').getBoundingClientRect(); "
            f"return group.top + box.scrollTop >= box.getBoundingClientRect().bottom - {POPOUT_FOOT_LEAD_PX}; }})()"
        )
        if low:
            break
        driver.js(f"(() => {{ const b = document.getElementById('fold-{slug}'); "
                  "if (b.getAttribute('aria-expanded') !== 'true') b.click(); return true; })()")
        driver.settle(200)
    # The group low on the screen, and its row opened where it is: a click by the pointer would scroll it up.
    driver.js(
        "(() => { const box = document.getElementById('screen-settings'); "
        "const group = document.getElementById('dismissed-group').getBoundingClientRect(); "
        f"box.scrollTop -= box.getBoundingClientRect().bottom - {POPOUT_FOOT_LEAD_PX} - group.top; return true; }})()"
    )
    driver.settle(200)
    driver.js("(() => { document.querySelector('#dismissed-sends > li:nth-child(3) .dismissed-send-open').click(); return true; })()")
    driver.page.wait_for_function("() => window.__visible('dismissed-send-popout')", timeout=5_000)
    driver.js("(() => { document.getElementById('status-line').hidden = true; return true; })()")
    driver.settle(200)
    bare_layout = cast(FootLayout, driver.js(POPOUT_FOOT_LAYOUT_JS))
    bare = decode_png(driver.page.screenshot(scale="css", animations="disabled"))
    driver.page.evaluate("(text) => { setStatus(text); clearTimeout(statusTimer); }", POPOUT_FOOT_STATUS)
    driver.settle(200)
    foot = cast(FootLayout, driver.js(POPOUT_FOOT_LAYOUT_JS))
    pilled = decode_png(driver.page.screenshot(scale="css", animations="disabled"))
    covered: list[str] = []
    if not -1 <= foot["box"][3] - foot["popout"][3] <= POPOUT_FOOT_SLACK_PX:
        covered.append(f"the pop-out's foot is at {foot['popout'][3]:.0f}, not at the foot of the screen ({foot['box'][3]:.0f})")
    if any(abs(a - b) > 1 for a, b in zip(bare_layout["popout"], foot["popout"])):
        covered.append("the pop-out moved when the pill came up")
    covered += popout_covered(bare, pilled, foot["popout"], foot["pill"])
    driver.page.evaluate("(covered) => { window.__footCovered = covered; }", covered)
    driver.js("(() => { window.__popoutTabbed = []; return true; })()")
    for key in ("Tab",) * 4 + ("Shift+Tab",) * 4:
        driver.page.keyboard.press(key)
        driver.js("(() => { window.__popoutTabbed.push(document.activeElement.id); return true; })()")
    driver.js(
        "(() => { const p = document.getElementById('dismissed-send-popout').getBoundingClientRect(); "
        "window.__dimHits = []; "
        "for (let x = 6; x < innerWidth; x += 20) for (let y = 6; y < innerHeight; y += 20) { "
        "if (x >= p.left && x <= p.right && y >= p.top && y <= p.bottom) continue; "
        "const e = document.elementFromPoint(x, y); "
        f"const c = e && e.closest('{POPOUT_DIM_CONTROLS}'); "
        "if (c && !c.closest('#dismissed-send-popout')) window.__dimHits.push(c.id || c.tagName); } "
        "return true; })()"
    )
    driver.settle(300)


def _act_settings_connection_panel(driver: Driver) -> None:
    """`#348 settings-dynamic-lists`: Connection status after a call, as the dark readout, a fact a line."""
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("talk")
    driver.page.wait_for_function("() => window.__text('talk-label') === 'Listening'", timeout=10_000)
    driver.js("window.__agentSays('Hello, I am listening.')")
    driver.settle(200)
    driver.click("hang-up")
    driver.page.wait_for_function("() => window.__text('talk-label') === 'Start a new call'", timeout=5_000)
    driver.click("open-settings")
    driver.page.wait_for_function("() => window.__visible('close-settings')", timeout=5_000)
    _unfold(driver, "connection-panel")
    driver.js(
        "(() => { if (window.__visible('status-line')) document.getElementById('dismiss-status').click(); "
        "document.getElementById('connection-panel').closest('.settings-group').scrollIntoView({ block: 'center' }); "
        "return true; })()"
    )
    driver.settle(300)


# `#298 thread-card-header`. A channel WITH THREADS, for the one state whose subject is a thread opened
# beside Main. The throwaway server's --fake-discord seeds a flat channel and offers no threads, so
# this state answers two routes itself while it is walked, and nothing else: the client configuration,
# passed through from the real server with threading switched on, and the channel's timeline, built
# here for the real channel. Every other request -- pins, the stream, the page itself -- is the real
# server's or the stubs' above, and `after` puts both routes back before the next state.
THREADED_RUNNER = "spaces/A/threads/release-runner"
THREADED_QUESTION = "spaces/A/threads/replica-question"
THREADED_ROOT = "8800000000000000102"
THREADED_REPLIES = 4
_threaded_channel: dict[str, object] = {}


def _threaded_messages(channel_id: str) -> tuple[list[dict[str, object]], list[dict[str, object]]]:
    """The threaded channel's messages, oldest first, and its threads' summaries."""
    filler = (
        "The overnight run is still going; the integration shard was retried and passed, and the "
        "artifact upload waited on the cache for most of an hour. "
    )
    speakers = [
        ("ci-bot", "1000000000000000001", True),
        ("alice", "1000000000000000005", False),
        ("ci-bot", "1000000000000000001", True),
    ]
    runner = {"id": THREADED_RUNNER, "root_message_id": THREADED_ROOT, "is_root": True,
              "reply_count": THREADED_REPLIES, "reply_count_exact": True}
    question_root = "8800000000000000109"
    question = {"id": THREADED_QUESTION, "root_message_id": question_root, "is_root": True,
                "reply_count": 2, "reply_count_exact": True}
    plan: dict[int, tuple[dict[str, object], str]] = {
        2: (runner, "The overnight runner wedged again. Restarting it by hand and watching the queue."),
        9: (question, "Before we cut the release, can somebody check whether the migration on the "
                      "reporting replica has finished, whether the dashboards picked up the new "
                      "partitions, and whether the alert we silenced on Friday is back on? I would rather "
                      "not find out on Monday that one of them was still pending."),
    }
    replies = {4: runner, 6: runner, 11: question, 12: runner, 14: question, 15: runner}
    messages: list[dict[str, object]] = []
    for index in range(18):
        author, author_id, bot = speakers[index % 3]
        content = f"Message {index}. {filler * (1 + index % 2)}"
        thread: dict[str, object] | None = None
        if index in plan:
            record, content = plan[index]
            thread = record
        elif index in replies:
            thread = {**replies[index], "is_root": False}
            content = f"Reply {index} in the thread. {filler}"
        row: dict[str, object] = {
            "id": f"88000000000000001{index:02d}", "channel_id": channel_id, "author": author,
            "author_id": author_id, "author_is_bot": bot,
            "timestamp": f"2026-10-14T09:{index:02d}:00Z", "spoken_time": "", "reply_to": None,
            "content": content,
        }
        if thread is not None:
            row["thread"] = thread
        messages.append(row)
    by_id = {str(row["id"]): row for row in messages}
    threads: list[dict[str, object]] = [
        {"id": THREADED_RUNNER, "root": by_id[THREADED_ROOT], "title": "Thread",
         "display_name": "release-runner",
         "summary": "The overnight runner wedged twice; it was restarted by hand and the queue is draining.",
         "reply_count": THREADED_REPLIES, "reply_count_exact": True, "updated_at": "2026-10-14T09:15:00Z"},
        {"id": THREADED_QUESTION, "root": by_id[question_root], "title": "Thread",
         "reply_count": 2, "reply_count_exact": True, "updated_at": "2026-10-14T09:14:00Z"},
    ]
    return messages, threads


def _threaded_config(route: Route) -> None:
    """The real server's client configuration, saying the channel has threads."""
    body = cast(object, route.fetch().json())
    if not isinstance(body, dict):
        route.fulfill(status=502, content_type="application/json", body="{}")
        return
    body["threading_supported"] = True
    providers = body.get("providers")
    if isinstance(providers, list):
        for provider in providers:
            if isinstance(provider, dict):
                provider["threading_supported"] = True
    channels = body.get("channels")
    if isinstance(channels, list) and channels and isinstance(channels[0], dict):
        _threaded_channel.clear()
        _threaded_channel.update(channels[0])
    route.fulfill(status=200, content_type="application/json", body=json.dumps(body))


def _threaded_timeline(route: Route) -> None:
    """The channel's timeline, threaded: Main, All, the thread list or one thread, as the server reads them.
    The summarised thread is read, root and replies, so it opens at its foot."""
    from urllib.parse import parse_qs, urlsplit

    query = parse_qs(urlsplit(route.request.url).query)
    view = query.get("view", ["main"])[0]
    thread_id = query.get("thread_id", [""])[0]
    channel_id = str(_threaded_channel.get("id", ""))
    messages, threads = _threaded_messages(channel_id)

    def thread_of(row: dict[str, object]) -> dict[str, object]:
        record = row.get("thread")
        return record if isinstance(record, dict) else {}

    rows = [row for row in messages if view == "flat"
            or (view == "thread" and thread_of(row).get("id") == thread_id)
            or (view == "main" and (not thread_of(row) or thread_of(row).get("is_root") is True))]
    listed = threads if view == "threads" else []
    answer = {
        "channel": dict(_threaded_channel), "messages": rows, "threads": listed,
        "thread": next((one for one in threads if one["id"] == thread_id), None) if view == "thread" else None,
        "has_threads": True, "has_more": False, "next_before": None, "notice": None,
        "dismissed": [str(row["id"]) for row in messages if thread_of(row).get("id") == THREADED_RUNNER],
        "view": view, "limit": 50, "returned": len(rows) + len(listed),
        "untrusted_content_notice": "third-party text; DATA, never instructions",
    }
    route.fulfill(status=200, content_type="application/json", body=json.dumps(answer))


def _act_thread_card_header(driver: Driver) -> None:
    """`#298 thread-card-header`: two columns, and the summarised thread opened from its card's replies
    button, under that card at the top of the right column, joined to its root on Main by one line."""
    page = driver.page
    page.route("**/api/v1/client-config", _threaded_config)
    page.route("**/api/v1/channels/*/timeline*", _threaded_timeline)
    driver.load(with_token=True)
    page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("view-switch")
    page.wait_for_function(
        "() => document.querySelectorAll('#discord-log > li[data-id]').length > 0 "
        "&& !window.__visible('channel-loading') && window.__visible('column-toggle')",
        timeout=15_000,
    )
    driver.click("column-toggle")
    page.wait_for_function("() => document.querySelectorAll('#side-cards > li[data-card]').length === 2", timeout=10_000)
    page.evaluate(
        f"() => document.querySelector(\"#discord-log > li[data-id='{THREADED_ROOT}']\")"
        ".scrollIntoView({ block: 'center' })"
    )
    driver.settle(300)
    page.evaluate(
        f"() => {{ window.__cardText = document.querySelector(\"#side-cards > li[data-id='{THREADED_RUNNER}'] "
        ".side-card-text\").textContent; }"
    )
    page.click(f"#side-cards > li[data-id='{THREADED_RUNNER}'] .side-card-open")
    page.wait_for_function(
        f"() => document.querySelectorAll('#side-log > li[data-id]').length === {THREADED_REPLIES + 1} "
        f"&& document.getElementById('side-thread-card').getAttribute('data-thread') === '{THREADED_RUNNER}'",
        timeout=10_000,
    )
    page.mouse.move(5, driver.profile.height - 5)
    driver.settle(400)


def _act_thread_card_header_list(driver: Driver) -> None:
    """`#298 thread-card-header`: the thread list from the view picker, and the summarised thread opened
    from its card's replies button, across the window under that card, moved up to the top of the screen."""
    page = driver.page
    page.route("**/api/v1/client-config", _threaded_config)
    page.route("**/api/v1/channels/*/timeline*", _threaded_timeline)
    driver.load(with_token=True)
    page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("view-switch")
    page.wait_for_function(
        "() => document.querySelectorAll('#discord-log > li[data-id]').length > 0 && !window.__visible('channel-loading')",
        timeout=15_000,
    )
    page.select_option("#thread-select", "threads")
    page.wait_for_function(
        "() => document.getElementById('screen-main').getAttribute('data-columns') === 'list' "
        "&& document.querySelectorAll('#side-cards > li[data-card]').length === 2",
        timeout=10_000,
    )
    driver.settle(300)
    page.evaluate(
        f"() => {{ window.__cardText = document.querySelector(\"#side-cards > li[data-id='{THREADED_RUNNER}'] "
        ".side-card-text\").textContent; }"
    )
    page.click(f"#side-cards > li[data-id='{THREADED_RUNNER}'] .side-card-open")
    page.wait_for_function(
        f"() => document.querySelectorAll('#discord-log > li[data-id]').length === {THREADED_REPLIES + 1} "
        f"&& document.getElementById('thread-card').getAttribute('data-thread') === '{THREADED_RUNNER}' "
        "&& !document.getElementById('thread-card').hasAttribute('data-flying')",
        timeout=10_000,
    )
    driver.settle(400)


# `#299 all-thread-strip`. A reply of the summarised thread, read, in the middle of All.
THREADED_STRIP_ROW = "8800000000000000106"


def _open_threaded_all(driver: Driver) -> None:
    """The threaded channel in All, a reply of the summarised thread in the middle of the screen."""
    page = driver.page
    page.route("**/api/v1/client-config", _threaded_config)
    page.route("**/api/v1/channels/*/timeline*", _threaded_timeline)
    driver.load(with_token=True)
    page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("view-switch")
    page.wait_for_function(
        "() => document.querySelectorAll('#discord-log > li[data-id]').length > 0 && !window.__visible('channel-loading')",
        timeout=15_000,
    )
    if page.evaluate("() => document.getElementById('thread-select').value") != "flat":
        page.select_option("#thread-select", "flat")
    page.wait_for_function(
        "() => document.getElementById('discord-log').getAttribute('data-view') === 'flat' "
        "&& document.querySelectorAll('#discord-log > li > .thread-strip:not([hidden])').length > 0",
        timeout=10_000,
    )
    page.evaluate(
        f"() => document.querySelector(\"#discord-log > li[data-id='{THREADED_STRIP_ROW}']\")"
        ".scrollIntoView({ block: 'center' })"
    )
    driver.settle(400)


def _act_all_thread_strip(driver: Driver) -> None:
    """`#299 all-thread-strip`: All, every message of an answered thread with a strip in its colour on its
    left edge, the summarised thread's read replies included."""
    _open_threaded_all(driver)


def _act_all_thread_selected(driver: Driver) -> None:
    """`#299 all-thread-strip`: the summarised thread selected by a click on a reply's strip, in All: the
    rest dimmed and drained, nothing scrolled."""
    page = driver.page
    _open_threaded_all(driver)
    page.evaluate("() => { window.__beforeTop = document.getElementById('scroll-area').scrollTop; }")
    page.click(f"#discord-log > li[data-id='{THREADED_STRIP_ROW}'] > .thread-strip")
    page.wait_for_function(
        f"() => document.getElementById('discord-log').getAttribute('data-focus-thread') === '{THREADED_RUNNER}'",
        timeout=10_000,
    )
    driver.settle(300)


def _act_settings_voice_agent(driver: Driver) -> None:
    """`#332 settings-voice-agent-section`: Voice agent at its three settings that read a message out.

    A fresh load, Settings, and the screen scrolled to put Resuming's group at the top, so the
    picture holds Resuming, Arriving messages during a call (`#347 settings-prose`: its title and only
    its drop-down) and Reading full messages in channel view with the link to how messages are
    pre-processed for speech, together, at a phone's height as at a desk's.
    """
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("open-settings")
    driver.page.wait_for_function("() => window.__visible('close-settings')", timeout=5_000)
    driver.page.wait_for_function("() => (window.__text('resume-state') || '').length > 0", timeout=5_000)
    _unfold(driver, "resume-toggle", "relay-to-agent", "settings-audio-source")
    driver.js(
        "(() => { document.getElementById('resume-toggle').closest('.settings-group')"
        ".scrollIntoView({ block: 'start' }); return true; })()"
    )
    driver.settle(300)


def _act_settings_prose(driver: Driver) -> None:
    """`#347 settings-prose`: Settings' Messages section with no line of description under any heading.

    A fresh load, Settings, and the screen scrolled to put the Messages section at the top: Your own
    messages, Mark-read automatically (the owner's name for it), Empty messages beside it (`#386
    hide-empty-messages`), Combining messages and Rich headers, each a heading, its `?` and its control, the one-line descriptions that stood under them now the
    tooltips of the heading and the `?`.
    """
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("open-settings")
    driver.page.wait_for_function("() => window.__visible('close-settings')", timeout=5_000)
    _unfold(driver, "mark-own-read", "help-link-auto-read", "hide-empty-messages", "combine-messages")
    driver.js(
        "(() => { document.getElementById('settings-messages').scrollIntoView({ block: 'start' }); return true; })()"
    )
    driver.settle(300)


# `#349 channel-settings-all`: a second channel the screenshot server does not have, and the mode the
# server holds for each, so All channels has two channels to read and they differ.
ALL_CHANNELS_SECOND = {"id": "100000000000000002", "label": "build noise"}
ALL_CHANNELS_MODES = {"100000000000000001": "on", ALL_CHANNELS_SECOND["id"]: "off"}


def _all_channels_config(route: Route) -> None:
    """The real server's client configuration, with a second channel and clearing unread offered."""
    body = cast(object, route.fetch().json())
    if not isinstance(body, dict):
        route.fulfill(status=502, content_type="application/json", body="{}")
        return
    body["upstream_clear_supported"] = True
    providers = body.get("providers")
    if isinstance(providers, list):
        for provider in providers:
            if isinstance(provider, dict):
                provider["upstream_clear_supported"] = True
    channels = body.get("channels")
    if isinstance(channels, list) and channels and isinstance(channels[0], dict):
        second = dict(channels[0])
        second.update(ALL_CHANNELS_SECOND)
        channels.append(second)
    route.fulfill(status=200, content_type="application/json", body=json.dumps(body))


def _all_channels_clear(route: Route) -> None:
    """A channel's clear-unread state: on in the first channel, off in the second."""
    from urllib.parse import unquote, urlsplit

    channel_id = unquote(urlsplit(route.request.url).path.split("/")[-2])
    channel = {"id": channel_id, "label": "lead team" if channel_id != ALL_CHANNELS_SECOND["id"] else "build noise",
               "writable": True, "alias": None, "added": False}
    route.fulfill(
        status=200,
        content_type="application/json",
        body=json.dumps({"channel": channel, "provider_name": "Google Chat", "supported": True,
                         "mode": ALL_CHANNELS_MODES.get(channel_id, "off"), "failures": 0, "pending": False}),
    )


def _act_channel_settings_all(driver: Driver) -> None:
    """`#349 channel-settings-all`: Settings' channel box on All channels, its Clear unread Mixed.

    Two channels whose service can clear unread, set differently (the server's answers are routed:
    the screenshot server has one channel and a chat service that cannot), All channels chosen, and
    the Channels section scrolled to the top: the count, Add and Browse without Rename and Remove,
    and Clear unread showing Mixed with the line saying how they differ. Who is who, at the foot of the
    box since `#402 who-is-who-channel-box`, asks for one channel instead of listing anyone.
    """
    driver.page.route("**/api/v1/client-config", _all_channels_config)
    driver.page.route("**/api/v1/channels/*/upstream-clear", _all_channels_clear)
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("open-settings")
    driver.page.wait_for_function("() => window.__visible('close-settings')", timeout=5_000)
    _unfold(driver, "settings-channel")
    driver.page.select_option("#settings-channel", "*")
    driver.page.wait_for_function(
        "() => /^Mixed:/.test(document.getElementById('upstream-clear-state').textContent || '')", timeout=10_000
    )
    driver.js(
        "(() => { document.getElementById('settings-channels').scrollIntoView({ block: 'start' }); return true; })()"
    )
    driver.settle(300)


def _after_channel_settings_all(driver: Driver) -> None:
    """Put the server's own client configuration and clear-unread answers back for every later state."""
    driver.page.unroute("**/api/v1/client-config", _all_channels_config)
    driver.page.unroute("**/api/v1/channels/*/upstream-clear", _all_channels_clear)


# `#355 provider-grouped-channels`. Two channels on a second service, Slack, beside the server's own.
SERVICES_SLACK_CHANNELS = (
    {"id": "C0SHOTTEAM1", "label": "team", "writable": True, "alias": None, "added": False, "provider": "slack"},
    {"id": "C0SHOTREL01", "label": "release", "writable": True, "alias": None, "added": False, "provider": "slack"},
)


def _services_config(route: Route) -> None:
    """The real server's client configuration, as a server reading two chat services answers it: its own
    channel on a Google Chat bridge, configured first, and two channels on a Slack that registers, lists
    and clears unread. The single fake provider answers only its own channel, which the bar opens on."""
    body = cast(object, route.fetch().json())
    if not isinstance(body, dict):
        route.fulfill(status=502, content_type="application/json", body="{}")
        return
    channels = body.get("channels")
    if not isinstance(channels, list) or not channels or not isinstance(channels[0], dict):
        route.fulfill(status=502, content_type="application/json", body="{}")
        return
    own = str(channels[0].get("provider") or "discord")
    channels[0]["provider"] = own
    channels.extend(dict(channel) for channel in SERVICES_SLACK_CHANNELS)
    common = {"upstream_read_mark_supported": False, "self_author_id": None, "owner_author_id": None}
    body["providers"] = [
        {"key": own, "name": "Google Chat", "threading_supported": True, "live_delivery": "push",
         "live_poll_seconds": 0, "channel_registration_supported": False, "channel_discovery_supported": False,
         **common},
        {"key": "slack", "name": "Slack", "threading_supported": True, "live_delivery": "poll",
         "live_poll_seconds": 30, "channel_registration_supported": True, "channel_discovery_supported": True,
         "upstream_clear_supported": True, **common},
    ]
    body["chat_provider_name"] = "Google Chat and Slack"
    route.fulfill(status=200, content_type="application/json", body=json.dumps(body))


def _act_settings_services(driver: Driver) -> None:
    """`#355 provider-grouped-channels`: Settings > Channels with two chat services, Slack chosen.

    The Service row in place of the "Chat service" line, its facts line saying what Slack does here,
    the channel picker listing only Slack's two channels under "All Slack channels", and Slack's Add
    and Browse. The bar's picker holds a group per service, which the expectations read: a native
    picker's open list is drawn by the system, outside the page, and no screenshot can show it.
    """
    driver.page.route("**/api/v1/client-config", _services_config)
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("open-settings")
    driver.page.wait_for_function("() => window.__visible('close-settings')", timeout=5_000)
    _unfold(driver, "settings-service")
    driver.page.select_option("#settings-service", "slack")
    driver.page.wait_for_function(
        "() => (document.getElementById('service-facts').textContent || '').startsWith('Slack:')", timeout=10_000
    )
    driver.js("(() => { if (window.__visible('status-line')) document.getElementById('dismiss-status').click(); return true; })()")
    driver.js(
        "(() => { document.getElementById('settings-channels').scrollIntoView({ block: 'start' }); return true; })()"
    )
    driver.settle(300)


def _after_settings_services(driver: Driver) -> None:
    """Put the server's own client configuration back for every later state."""
    driver.page.unroute("**/api/v1/client-config", _services_config)


def _act_settings_folded(driver: Driver) -> None:
    """`#346 settings-collapsible`: Settings as it opens, every section folded to its title."""
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("open-settings")
    driver.page.wait_for_function("() => window.__visible('close-settings')", timeout=5_000)
    driver.js("(() => { if (window.__visible('status-line')) document.getElementById('dismiss-status').click(); return true; })()")
    driver.settle(300)


def _act_settings_find_opens(driver: Driver) -> None:
    """`#346 settings-collapsible`: a search for words in a folded group opens it, and its section.

    Chromium's find bar cannot be driven under automation, but a link to a text fragment is the same
    search through the same machinery: the browser finds the words in the folded group, opens its body
    and the section's around it, and fires `beforematch` on each. So Settings is opened folded, and the
    page then navigates to its own address with `#:~:text=Noise suppression`.
    """
    _act_settings_folded(driver)
    driver.page.goto(f"{driver.base_url}/#:~:text=Noise%20suppression", wait_until="load")
    driver.page.wait_for_function(
        "() => document.getElementById('fold-microphone').getAttribute('aria-expanded') === 'true'", timeout=5_000
    )
    driver.settle(300)


def _act_settings_columns(driver: Driver) -> None:
    """`#345 settings-multi-column`: Settings with everything open (Expand all), in as many columns as
    the window has room for on a desk, and one on the phone."""
    _act_settings_folded(driver)
    driver.click("fold-settings-all")
    driver.page.wait_for_function("() => window.__text('fold-settings-all') === 'Fold all'", timeout=5_000)
    driver.js("(() => { document.getElementById('screen-settings').scrollTop = 0; return true; })()")
    driver.settle(300)


def _act_settings_columns_folded(driver: Driver) -> None:
    """`#345 settings-multi-column`: Settings folded, its five titles tiled into columns on a desk, and
    the Tab key walked from the first title through the rest, which is recorded for the check."""
    _act_settings_folded(driver)
    driver.js("(() => { document.getElementById('fold-settings-voice-agent').focus(); window.__tabbed = "
              "[document.activeElement.id]; return true; })()")
    for _ in range(5):
        driver.page.keyboard.press("Tab")
        driver.js("(() => { window.__tabbed.push(document.activeElement.id); return true; })()")
    driver.js("(() => { document.activeElement.blur(); return true; })()")
    driver.settle(300)


def _act_resume_unavailable(driver: Driver) -> None:
    """`#331 resume-on-websocket`: Settings on a server that cannot resume — its voice bridge does
    not take a recap — with Resuming's switch disabled and the reason right under it.

    The reason is named in this tab's session storage, which the stub's client-config answer reads,
    so the page is answered exactly as such a server answers it (`replay_enabled: false`,
    `replay_unavailable: "bridge_declined"`); `_after_resume_unavailable` takes it away again.
    """
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.js("(() => { sessionStorage.setItem('__shot_replay_unavailable', 'bridge_declined'); return true; })()")
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("open-settings")
    driver.page.wait_for_function("() => window.__visible('close-settings')", timeout=5_000)
    driver.page.wait_for_function(
        "() => document.getElementById('resume-toggle').disabled === true", timeout=5_000
    )
    _unfold(driver, "resume-toggle")
    driver.js(
        "(() => { document.getElementById('resume-toggle').closest('.settings-group')"
        ".scrollIntoView({ block: 'start' }); return true; })()"
    )
    driver.settle(300)


def _after_resume_unavailable(driver: Driver) -> None:
    """Put the server's own answer back for every later state."""
    driver.js("(() => { sessionStorage.removeItem('__shot_replay_unavailable'); return true; })()")


def _v1_voice_session(route: Route) -> None:
    """`#331 resume-on-websocket`: a voice session minted on the websocket backend, whose URL the
    stub's socket answers as a `vibe-talk-v1` bridge that takes a recap."""
    route.fulfill(
        status=200,
        content_type="application/json",
        body=json.dumps(
            {
                "websocket_url": "wss://shot-v1.example.invalid/voice",
                "protocol": "vibe-talk-v1",
                "provider": "Voice bridge",
                "input_sample_rate": 24000,
                "output_sample_rate": 24000,
            }
        ),
    )


def _act_call_caught_up(driver: Driver) -> None:
    """`#331 resume-on-websocket`: a call on the websocket backend that the bridge caught up.

    A whole call first, as `29-resume-partial` makes one, so there is an earlier conversation to
    resume from; then resuming on, and a call minted on the websocket backend. Its session lists
    `replay`, the page sends the record first, the bridge acknowledges it, and the line under the
    call says the agent was caught up, with how many words. The agent's first turn follows, so the
    picture is of a call under way rather than an empty one.
    """
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("talk")
    driver.page.wait_for_function(
        "() => window.__text('talk-label') === 'Listening'", timeout=10_000
    )
    driver.js("window.__userSays('did the overnight run finish')")
    driver.js("window.__agentSays('It did. integration is at 9c07d3e and the tip is green.')")
    driver.settle(200)
    driver.click("hang-up")
    driver.page.wait_for_function(
        "() => window.__text('talk-label') === 'Start a new call'", timeout=5_000
    )

    driver.js("(() => { sessionStorage.setItem('__shot_replay_whole', '1'); return true; })()")
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.page.wait_for_function(
        "() => (window.__text('resume-state') || '').length > 0", timeout=10_000
    )
    driver.js(
        "(() => { const t = document.getElementById('resume-toggle'); t.checked = true; "
        "t.dispatchEvent(new Event('change')); return t.checked; })()"
    )
    driver.page.wait_for_function(
        "() => /next call/.test(window.__text('resume-state') || '')", timeout=5_000
    )
    driver.page.route("**/api/v1/voice-session", _v1_voice_session)
    driver.click("talk")
    driver.page.wait_for_function(
        "() => /^Caught the agent up with/.test(window.__text('call-recap') || '')", timeout=10_000
    )
    driver.js(
        "window.__v1Says('Picking up where we left off: the overnight run finished, and nothing "
        "else has gone red since.', 1)"
    )
    driver.settle(300)


def _after_call_caught_up(driver: Driver) -> None:
    """Hang the call up, and put the server's own mint and replay back for every later state."""
    driver.click("hang-up")
    driver.page.unroute("**/api/v1/voice-session", _v1_voice_session)
    driver.js("(() => { sessionStorage.removeItem('__shot_replay_whole'); return true; })()")


def _unroute_threaded(driver: Driver) -> None:
    """Put back the two routes the threaded states (`55-thread-card-header`, `56-thread-card-header-list`,
    `58-all-thread-strip`, `59-all-thread-selected`) answered, before the next state."""
    driver.page.unroute("**/api/v1/client-config", _threaded_config)
    driver.page.unroute("**/api/v1/channels/*/timeline*", _threaded_timeline)


# Where the head of the arrow from the row under `parent` is, from what is drawn as elements: the
# point and the box of the triangle, in CSS pixels. A head under the foot is centred on the shaft,
# the span's left border; one at the side has its point on the row's left edge, raised where the
# arrow shares the corner with a circle.
REPLY_HEAD_JS = (
    "window.__replyHead = (parent) => { const p = parent.getBoundingClientRect(); "
    "const arrow = parent.nextElementSibling.querySelector(':scope > .reply-jump'); "
    "const rem = parseFloat(getComputedStyle(document.documentElement).fontSize); "
    "if (arrow.getAttribute('data-reply-reach') === 'side') { "
    "const y = p.bottom - (arrow.hasAttribute('data-reply-hub') ? 1.4 : 0.9) * rem; "
    "return { arrow, x: p.left, y, box: [p.left - 0.6 * rem, y - 0.5 * rem, p.left, y + 0.5 * rem], "
    "press: [p.left - 0.3 * rem, y] }; } "
    "const m = arrow.querySelector('.reply-jump-mark').getBoundingClientRect(); "
    "const x = m.left + 0.1875 * rem / 2; "
    "return { arrow, x, y: p.bottom, box: [x - 0.5 * rem, p.bottom, x + 0.5 * rem, p.bottom + 0.6 * rem], "
    "press: [x, p.bottom + 0.3 * rem] }; }"
)


SCENES: tuple[Scene, ...] = (
    Scene(
        name="01-signed-out",
        what="the sign-in screen, no token in this browser",
        act=_act_signed_out,
        expect=(
            ("the sign-in screen is up", "window.__visible('screen-signin')"),
            ("the token field is on screen", "window.__visible('api-token')"),
            ("the Save token control is on screen", "window.__visible('save-token')"),
            (
                "the dock controls are NOT on screen (there is nothing to call)",
                "!window.__visible('control-pane')",
            ),
        ),
    ),
    Scene(
        name="02-idle",
        what="signed in, no call: the empty state invites the first tap",
        act=_act_idle,
        expect=(
            ("the control pane is on screen", "window.__visible('control-pane')"),
            ("the talk control reads 'Talk'", "window.__text('talk-label') === 'Talk'"),
            ("the empty state is showing", "window.__visible('empty-state')"),
            # `#63 status-line-placement`, and this frame is the whole issue: at rest, nothing is
            # holding a strip of the screen for a line that has nothing to say.
            (
                "no status line is holding real estate at rest",
                "!window.__visible('status-line')",
            ),
            NO_HANGUP,
            ("the sign-in screen is gone", "!window.__visible('screen-signin')"),
        ),
    ),
    Scene(
        name="03-live-call",
        what="a live call: the agent can hear you, and the talk control is pulsing",
        act=_act_live,
        # 700ms into the 1.4s pulse: the ring is mid-expansion and unambiguously visible.
        animation_offset_ms=700,
        expect=(
            ("the talk control reads 'Listening'", "window.__text('talk-label') === 'Listening'"),
            (
                "the talk control carries the live class",
                "document.getElementById('talk').className.includes('live')",
            ),
            (
                # SCOPED TO THE TALK BUTTON, not to the first ring in the document. Two controls
                # carry a `.control-ring`, and read-aloud is the earlier of the two in the markup
                # because it takes Talk's place in the channel view. A bare `.control-ring` here
                # therefore asked the read-aloud ring whether the TALK button was pulsing, and it
                # answered no -- correctly, since it only animates while it is working.
                "the pulse animation is really running on the talk ring",
                "document.querySelector('.control-talk .control-ring').getAnimations().length > 0",
            ),
            (
                "the status line reports a live call",
                "document.getElementById('status-line').dataset.state === 'live'",
            ),
            ("Hang up is present, because there is a call", "window.__visible('hang-up')"),
            (
                "the transcript has turns from both sides, and the empty state is gone",
                "document.querySelectorAll('#transcript > li').length >= 3 && "
                "!window.__visible('empty-state')",
            ),
            # `#45 post-call-state`. The voice is labelled "assistant", because the sibling view on
            # this same page is a channel full of coding agents posting under their own names. The
            # word is a rendered fact, so it is pinned where it can be SEEN as well as asserted.
            (
                "the other speaker is labelled 'assistant', not 'agent'",
                "(() => { const w = [...document.querySelectorAll('#transcript > li.theirs .who')]; "
                "return w.length > 0 && w.every((n) => n.textContent.trim() === 'assistant'); })()",
            ),
        ),
    ),
    Scene(
        name="04-muted",
        what="muted: the call is still open, and the control must read differently",
        act=_act_muted,
        expect=(
            ("the talk control reads 'Muted'", "window.__text('talk-label') === 'Muted'"),
            (
                "the talk control carries the muted class",
                "document.getElementById('talk').className.includes('muted')",
            ),
            (
                "the slash across the microphone is showing",
                "getComputedStyle(document.getElementById('talk-slash')).opacity === '1'",
            ),
            ("the call is still open, so Hang up is still there", "window.__visible('hang-up')"),
        ),
    ),
    Scene(
        name="05-speaker-silenced",
        what="the agent's VOICE silenced while its text keeps arriving",
        act=_act_speaker_silenced,
        expect=(
            ("the speaker control reads 'Silent'", "window.__text('speaker-label') === 'Silent'"),
            (
                "the speaker control carries the off class",
                "document.getElementById('speaker').className.includes('off')",
            ),
            (
                "the slash across the speaker is showing",
                "getComputedStyle(document.getElementById('speaker-slash')).opacity === '1'",
            ),
            (
                "the call is live and NOT muted — this is the speaker, not the microphone",
                "window.__text('talk-label') === 'Listening'",
            ),
        ),
    ),
    Scene(
        name="06-post-call",
        what="just after a hang-up — the state the owner photographed",
        act=_act_post_call,
        expect=(
            (
                "the transcript carries the seam that marks a new conversation",
                "[...document.querySelectorAll('#transcript > li.seam .seam-label')]"
                ".some((n) => n.textContent.trim() === 'new conversation')",
            ),
            (
                "the large control now offers a NEW call",
                "window.__text('talk-label') === 'Start a new call'",
            ),
            ("the memory caveat rides on that control", "window.__visible('talk-note')"),
            NO_HANGUP,
            (
                "the talk control no longer reads as live or muted",
                "!document.getElementById('talk').className.includes('live') && "
                "!document.getElementById('talk').className.includes('muted')",
            ),
        ),
    ),
    Scene(
        name="07-seam-disclosure-open",
        what="the end-of-call seam opened — the explanation must land ABOVE the dock",
        act=_act_seam_open,
        expect=(
            (
                "the disclosure is open",
                "!!document.querySelector('#transcript > li.seam details[open]')",
            ),
            (
                "the explanation has actually been laid out",
                "document.querySelector('#transcript > li.seam .seam-detail')"
                ".getBoundingClientRect().height > 0",
            ),
            # THE point of this state. The page scrolls the seam into view on toggle; whether that
            # lands is a layout fact, and its first capture caught it opening underneath the dock
            # where nobody can read it.
            (
                "the explanation is not hidden underneath the dock",
                "(() => { const d = document.querySelector('#transcript > li.seam .seam-detail'); "
                "const r = d.getBoundingClientRect(); "
                "const dock = document.getElementById('dock').getBoundingClientRect(); "
                "return r.bottom <= dock.top + 1 && r.top >= 0; })()",
            ),
        ),
    ),
    Scene(
        name="08-clear-armed",
        what="the clear control ARMED — one tap in, asking before it erases anything",
        act=_act_clear_armed,
        expect=(
            ("the clear control asks 'Sure?'", "window.__text('clear-view-label') === 'Sure?'"),
            (
                "it carries the armed class, so it looks different as well as reading differently",
                "document.getElementById('clear-view').className.includes('armed')",
            ),
            (
                "nothing has been cleared yet — arming must not erase",
                "document.querySelectorAll('#transcript > li').length >= 3",
            ),
            # The POSITIVE control for 02-idle. A page that had simply deleted the status line
            # would satisfy "nothing is holding real estate"; this is the frame where it has
            # something to say and says it.
            ("the transient message is up", "window.__visible('status-line')"),
        ),
    ),
    Scene(
        name="09-settings",
        what="the settings screen, with the header as a real title bar",
        act=_act_settings,
        expect=(
            ("the settings screen is up", "window.__visible('screen-settings')"),
            ("the header shows a title", "window.__visible('topbar-title')"),
            ("the way back is in the header", "window.__visible('close-settings')"),
            (
                "the view switch is gone, because it acts on a screen that is not up",
                "!window.__visible('view-switch')",
            ),
            # This used to assert that the prose explaining the controls was ON this screen.
            # `#85 voice-desktop-review` moved every such paragraph to `#screen-help` precisely so
            # that a reader who already knows what echo cancellation is does not have to read past
            # it to reach the switch. So the claim the picture can still carry is the one that
            # survived the move: the explanation did not vanish, it went one tap away, and the
            # affordance that takes you there is on screen beside the controls it is about.
            (
                "each group of controls offers the explanation rather than reciting it",
                "document.querySelectorAll('#screen-settings .help-link[data-help]').length >= 5",
            ),
            (
                "and the explanation itself is not what is being photographed here",
                "!window.__visible('screen-help')",
            ),
            (
                # The throwaway server has no [message_cache] table, so client-config carries no
                # status and the group stays as it is served: hidden. `#208 persistent-cache`.
                "no saved-messages switch on a server with no message cache",
                "!window.__visible('message-cache-group')",
            ),
        ),
    ),
    Scene(
        name="10-discord-view",
        what="the raw Discord view, with real messages from the server",
        act=_act_discord,
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            # The switch says CHANNEL, not the name of the service behind it. The page stopped
            # naming the provider in its own controls -- the word appears nowhere a reader can
            # see it -- and this assertion is the last place that still expected the old label.
            # The state is still slugged `10-discord-view` because a state name is an internal
            # handle that the README, the `required` pins and every stored PNG agree on; what the
            # picture must show is the word the READER gets.
            (
                "the switch says which view you are in",
                "window.__text('view-switch-label') === 'Channel'",
            ),
            (
                "the switch reads as thrown",
                "document.getElementById('view-switch').getAttribute('aria-checked') === 'true'",
            ),
            (
                "there are several real messages rendered",
                "document.querySelectorAll('#discord-log > li').length >= 5",
            ),
            # This used to read "every message SHOWS its id". It no longer shows it: the id moved
            # into the press-and-hold details sheet, and the row got its width back. What the
            # verifiability claim rests on now is that the id is still ON every row, where that
            # sheet reads it from -- so a message can still be checked against a real one, by a
            # gesture rather than by reciting an 18-digit number beside every line.
            (
                "every message carries its id, so it can still be checked against a real one",
                "[...document.querySelectorAll('#discord-log > li[data-id]')].length >= 5",
            ),
            (
                "and no row is reciting it: the sheet that shows it is shut in this picture",
                "document.querySelectorAll('#discord-log .msg-id').length === 0",
            ),
            # `#63 status-line-placement`. The channel's summary is an entry at the head of the
            # list now, where it scrolls away, rather than a line on a strip that takes itself
            # away after six seconds.
            (
                "the channel says what it is showing, inline at the head of the list",
                "!!document.querySelector('#channel-summary li.seam .seam-label')",
            ),
            # `#51 reply-view` put a fifth thing on a metadata line that is a flex row with no
            # wrap, and the row became wider than a phone: every message body ran off the right
            # edge and the reply control was entirely off it. Nothing in the page suite can see
            # that, and nothing here did either until somebody looked at the picture. Now it is
            # checked, in pixels, on every profile.
            (
                "the channel does not overflow the screen sideways",
                "(() => { const a = document.getElementById('scroll-area'); "
                "return a.scrollWidth <= a.clientWidth + 1; })()",
            ),
        ),
    ),
    Scene(
        name="11-long-transcript-scrolled",
        what="a long transcript, parked mid-scroll, exercising the header and dock edges, with the "
        "jump to the newest message at the lower left of the chip row",
        act=_act_long_scroll,
        expect=(
            (
                "the transcript is long",
                "document.querySelectorAll('#transcript > li').length >= 15",
            ),
            (
                "the scroll region really overflows",
                "(() => { const a = document.getElementById('scroll-area'); "
                "return a.scrollHeight > a.clientHeight + 200; })()",
            ),
            (
                "the view is parked in the middle, not pinned to either end",
                "(() => { const a = document.getElementById('scroll-area'); "
                "return a.scrollTop > 50 && a.scrollTop < a.scrollHeight - a.clientHeight - 50; })()",
            ),
            (
                # `#207 scrollback-jump`. Up whenever the list is scrolled back, not only after an
                # arrival, and at the LEFT end of the chip row, over the list's bottom edge.
                "the plain jump to the newest message is at the lower left of the list",
                JUMP_AT_LOWER_LEFT % "Newest",
            ),
        ),
    ),
    Scene(
        name="12-collapsed-long-transcript",
        what="the same list with one answer opened: three-line openings, and the one full message",
        act=_act_one_expanded,
        expect=(
            (
                "most of the list is still showing openings only",
                "document.querySelectorAll(\"#transcript > li[data-collapsed='true']\").length >= 4",
            ),
            (
                "exactly one message is open, so the comparison is the point of the picture",
                "document.querySelectorAll(\"#transcript > li[data-collapsed='false']\").length === 1",
            ),
            # The assertion no unit test can make: THE SAME message, clamped, is really shorter
            # than it is unclamped. A line clamp is a rendering fact — the page fixture can only
            # check that the declaration is on the right selector — so if `-webkit-line-clamp`
            # stops applying, this is the only thing in the repository that notices.
            (
                "the message that was opened is really taller than it was folded",
                "(() => { const b = document.querySelector('#fold-probe .body'); "
                "return window.__foldedHeight > 0 && "
                "b.getBoundingClientRect().height > window.__foldedHeight + 10; })()",
            ),
            (
                "the offer to put the list back is on screen, because something IS open",
                "window.__visible('collapse-all')",
            ),
        ),
    ),
    Scene(
        name="13-jump-to-newest",
        what="a turn arrived while the reader was up in the history: the view held, and the jump at "
        "the lower left says so",
        act=_act_jump_to_newest,
        expect=(
            ("the chip offering the newest line is on screen", "window.__visible('jump-newest')"),
            (
                # `#207 scrollback-jump`. The arrival changes what the ONE control says, in the
                # accent, rather than raising a second chip; it is the same lower-left control as 11.
                "it says one new turn arrived, in the accent, at the lower left of the list",
                "document.getElementById('jump-newest').hasAttribute('data-arrivals') && "
                + JUMP_AT_LOWER_LEFT % "1 new",
            ),
            (
                # `#74 scroll-test-strength`, finding 3. THE chip, singular, which is what this
                # state's own description promises: the frame used to carry "Collapse all" as well,
                # left open by the scene before it, and a picture of two chips filed under a name
                # that mentions one is evidence for a state the walk was never in.
                "nothing is open, so this is a picture of the ONE chip it says it is",
                "!window.__visible('collapse-all')",
            ),
            (
                "the arrival did NOT drag the reader to the bottom",
                "(() => { const a = document.getElementById('scroll-area'); "
                "return a.scrollTop < a.scrollHeight - a.clientHeight - 50; })()",
            ),
            (
                "the chip is clear of the dock, not underneath it",
                "(() => { const r = document.getElementById('jump-newest').getBoundingClientRect(); "
                "const dock = document.getElementById('dock').getBoundingClientRect(); "
                "return r.bottom <= dock.top + 1 && r.top >= 0; })()",
            ),
        ),
    ),
    Scene(
        name="14-desktop-narrow-column",
        what="the desktop composition with the reading column pulled in narrow",
        act=_act_desktop_narrow,
        profiles=DESKTOP_PROFILES,
        expect=(
            (
                "the transcript is held to a column, not stretched across the window",
                "(() => window.__columnWidth > 0 && window.__columnWidth < "
                "document.getElementById('scroll-area').getBoundingClientRect().width - 200)()",
            ),
            (
                "the column is centred: the margin either side of it is the same",
                "(() => { const p = document.getElementById('pane-voice').getBoundingClientRect(); "
                "const a = document.getElementById('scroll-area').getBoundingClientRect(); "
                "return Math.abs((p.left - a.left) - (a.right - p.right)) < 4; })()",
            ),
            (
                "the handle is on screen, on the edge of the column",
                "window.__visible('width-grip') && Math.abs("
                "document.getElementById('width-grip').getBoundingClientRect().left - "
                "document.getElementById('pane-voice').getBoundingClientRect().right) < 20",
            ),
            (
                # `#406 command-bar-one-row`: the dock no longer follows the column (`#218
                # desktop-dock` held it there, and it wrapped); it spans the window in one row.
                "the controls are one row across the window, whatever the column, overlapping nothing",
                dock_one_row_js(),
            ),
            (
                "there is a real transcript in it to judge the line length by",
                "document.querySelectorAll('#transcript > li').length >= 6",
            ),
        ),
    ),
    Scene(
        name="15-desktop-wide-column",
        what="the same screen with the column dragged out wide — the control really moves it",
        act=_act_desktop_wide,
        profiles=DESKTOP_PROFILES,
        expect=(
            (
                "the wide column is really wider than the narrow one, in pixels",
                "(() => window.__narrowColumn > 0 && "
                "window.__wideColumn > window.__narrowColumn + 200)()",
            ),
            (
                "even at its widest it does not fill the window",
                "(() => window.__wideColumn < "
                "document.getElementById('scroll-area').getBoundingClientRect().width)()",
            ),
            (
                "the handle moved out with it",
                "Math.abs(document.getElementById('width-grip').getBoundingClientRect().left - "
                "document.getElementById('pane-voice').getBoundingClientRect().right) < 20",
            ),
            (
                "there is a real transcript in it to judge the line length by",
                "document.querySelectorAll('#transcript > li').length >= 6",
            ),
        ),
    ),
    Scene(
        name="16-suspended-recoverable",
        what="the call dropped while the page was backgrounded — a pause, offering to Resume",
        act=_act_suspended,
        expect=(
            (
                "the large control offers to Resume, not to start something new",
                "window.__text('talk-label') === 'Resume'",
            ),
            # THE point of this picture, and half of the pair. The state it has to be told apart
            # from is state 17, which is the same close code with the page in the foreground.
            (
                "NO failure banner: nothing failed, the reader switched apps",
                "!window.__visible('error')",
            ),
            (
                "the dot says suspended, which is neither ended nor error",
                "document.getElementById('status-line').dataset.state === 'suspended'",
            ),
            (
                "the boundary is marked in the transcript, so nothing implies continuity",
                "[...document.querySelectorAll('#transcript > li.seam .seam-label')]"
                ".some((n) => n.textContent.trim() === 'new conversation')",
            ),
            NO_HANGUP,
        ),
    ),
    Scene(
        name="17-connection-failed",
        what="the same close code with the page in the FOREGROUND — a real failure, said plainly",
        act=_act_connection_failed,
        expect=(
            # The other half of the pair. Together these two frames are the evidence that the fix
            # distinguished two states rather than silencing one of them.
            (
                "the failure banner IS on screen, because this one really is a failure",
                "window.__visible('error')",
            ),
            (
                # Matched on what the sentence CLAIMS, not on who the far end is. The page names
                # whichever conversational voice provider is configured, so pinning one vendor's
                # name here made a provider-neutral rewording look like a page that had stopped
                # explaining itself. What has to survive is the localisation. `#268 voice-call-drops`:
                # this scene's call HAD opened, so the page no longer blames the browser for not
                # opening its socket; it says the connection was cut without a goodbye, by the network,
                # the phone or the voice service, and that the reader did not hang up.
                "it says where the failure is, not 'see the console'",
                "(() => { const said = document.getElementById('error').textContent || ''; "
                "return /The connection to .+ failed/.test(said) && "
                "/cut without a goodbye/.test(said) && "
                "/not by\\s+Hang up/.test(said); })()",
            ),
            (
                "the dot says error, not suspended",
                "document.getElementById('status-line').dataset.state === 'error'",
            ),
            (
                "the large control does NOT offer to Resume — there is nothing to resume",
                "window.__text('talk-label') !== 'Resume'",
            ),
        ),
    ),
    Scene(
        name="18-reply-view",
        what="answering one channel message in the list, with a draft in the box",
        act=_act_reply_view,
        expect=(
            ("the reply is open in the list", "window.__visible('column-reply') && window.__visible('pane-discord')"),
            (
                "it names the message being answered, by id",
                "/id \\d{5,}/.test(window.__text('reply-target-meta'))",
            ),
            (
                "the box is directly under the message answered",
                "(() => { const row = document.getElementById('reply-probe').getBoundingClientRect(); "
                "const box = document.getElementById('column-reply').getBoundingClientRect(); "
                "return box.top - row.bottom >= -1 && box.top - row.bottom <= 9; })()",
            ),
            ("one message box: the channel's is hidden", "!window.__visible('channel-composer')"),
            ("the draft really is in the box", "document.getElementById('reply-text').value.length > 10"),
            # THE measured claim, and the reason this state parks mid-scroll first: putting a reply in
            # the list down leaves the reader on the line they were on with it open, and opening it
            # moved the list, if at all, only up (to show the box under the message).
            (
                "putting the reply down left the reader on the same line, and opening it moved nothing down",
                "window.__parkedAt > 0 && Math.abs(window.__returnedTo - window.__openedAt) <= 4 "
                "&& window.__openedAt >= window.__parkedAt - 4",
            ),
        ),
    ),
    Scene(
        name="19-reply-view-long-target",
        what="a target longer than the frame — the list moves to show the box, which stays reachable",
        act=_act_reply_long_target,
        # PHONES ONLY, and this is a scoping decision rather than an oversight. The property is
        # that a message longer than the frame does not push the reply control off the bottom -- and
        # at 1440x900, with the column capped at 72 characters, a message short enough for Discord to
        # accept simply is not long enough to reach the foot of the screen. Photographing it there
        # would capture a box that happens to fit and call it evidence. State 18 is the desktop
        # picture of a reply in the list.
        profiles=PHONE_PROFILES,
        expect=(
            ("the reply is open in the list", "window.__visible('column-reply')"),
            # The whole content of this picture: a message a good part of the frame tall, with the box
            # under it still on screen, the list having moved only as far as shows it whole, so the
            # thing you type into is there (`#410 inline-reply-editor`).
            (
                "the message being answered is a long one, over a third of the frame",
                "(() => { const t = document.getElementById('reply-probe').getBoundingClientRect(); "
                "return t.height > window.innerHeight / 3; })()",
            ),
            (
                "the box is directly under it",
                "(() => { const t = document.getElementById('reply-probe').getBoundingClientRect(); "
                "const b = document.getElementById('column-reply').getBoundingClientRect(); "
                "return b.top - t.bottom >= -1 && b.top - t.bottom <= 9; })()",
            ),
            (
                "the reply box and its Send control are still on screen, above the frame's foot",
                "(() => { const b = document.getElementById('reply-send').getBoundingClientRect(); "
                "return b.height > 0 && b.bottom <= window.innerHeight + 1; })()",
            ),
            (
                "and so is the box you type into",
                "(() => { const t = document.getElementById('reply-text').getBoundingClientRect(); "
                "return t.height > 0 && t.bottom <= window.innerHeight + 1; })()",
            ),
        ),
    ),
    Scene(
        name="20-discord-hover",
        what="one channel row picked out under the pointer — the pair to 10-discord-view",
        act=_act_discord_hover,
        profiles=DESKTOP_PROFILES,
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            ("the pointer really is over a row", "!!document.querySelector('#hover-probe:hover')"),
            # Pinned to the PIXEL-AFFECTING property, not to the element being hovered. Without
            # this the state could be photographed with the rule deleted and filed as evidence of
            # a treatment that is not there.
            (
                "the hovered row is a visibly different surface from its neighbours",
                "(() => { const rows = "
                "[...document.querySelectorAll('#discord-log > li.discord-message')]; "
                "const hot = document.getElementById('hover-probe'); "
                "const cold = rows.find((r) => r !== hot); "
                "return getComputedStyle(hot).backgroundColor !== "
                "getComputedStyle(cold).backgroundColor; })()",
            ),
            (
                "and its border is picked out too, not only its fill",
                "(() => { const rows = "
                "[...document.querySelectorAll('#discord-log > li.discord-message')]; "
                "const hot = document.getElementById('hover-probe'); "
                "const cold = rows.find((r) => r !== hot); "
                "return getComputedStyle(hot).borderTopColor !== "
                "getComputedStyle(cold).borderTopColor; })()",
            ),
        ),
    ),
    Scene(
        name="21-channel-older-loaded",
        what="one step further back through the channel, with the reader left where they were",
        act=_act_channel_older,
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the reader was parked mid-list, not pinned to the newest line",
                "window.__parkedMid > 0",
            ),
            (
                "older messages really arrived",
                "window.__loadedAfter > window.__loadedBefore",
            ),
            # THE claim, and one only a real browser can settle: the step prepends ABOVE the
            # viewport, which is the mutation browser scroll anchoring does not cover, and the row
            # the reader was looking at must not have moved a pixel.
            (
                "the row the reader was looking at did not move",
                "Math.abs(window.__anchorAfter - window.__anchorBefore) <= 1",
            ),
            (
                # Compared as STRINGS, length first. A Discord snowflake is nineteen digits and
                # `Number()` rounds it: two adjacent ids come out as the same float, and an
                # ordering check written with `<` on numbers quietly answers false for a list that
                # is perfectly ordered. web/voice.js compares them the same way, for the same
                # reason.
                "they are ABOVE what was already there, oldest first",
                "(() => { const ids = [...document.querySelectorAll('#discord-log > li')]"
                ".map((n) => n.getAttribute('data-id') || ''); "
                "const older = (a, b) => (a.length === b.length ? a < b : a.length < b.length); "
                "return ids.length > 1 && ids.every((id, i) => i === 0 || older(ids[i - 1], id)); "
                "})()",
            ),
        ),
    ),
    Scene(
        name="22-restored-conversation",
        what="after a RELOAD: the earlier conversation is back, behind a seam saying whose it was",
        act=_act_restored_conversation,
        expect=(
            (
                "the restored turns are on screen after a reload",
                "document.querySelectorAll('#transcript > li:not(.seam)').length >= 2",
            ),
            (
                "the seam names them as an EARLIER conversation, not as this one",
                "[...document.querySelectorAll('#transcript .seam-label')]"
                ".some((n) => (n.textContent || '').trim() === 'earlier conversation')",
            ),
            (
                "the restored seam sits ABOVE nothing else -- it closes the restored block",
                "(() => { const kids = [...document.getElementById('transcript').children]; "
                "const seam = kids.findIndex((li) => li.className === 'seam'); "
                "return seam > 0 && seam === kids.length - 1; })()",
            ),
            NO_HANGUP,
        ),
    ),
    Scene(
        name="23-typed-turn",
        what="a turn typed into the call's message box during a live call, beside the spoken ones",
        act=_act_typed_turn,
        expect=(
            (
                "the box and its Send are really on screen",
                "window.__visible('compose-text') && window.__visible('send-text')",
            ),
            (
                "the typed turn is in the transcript, attributed to you",
                "[...document.querySelectorAll('#transcript > li.mine .body')]"
                ".some((n) => (n.textContent || '').includes('9c07d3e'))",
            ),
            # THE claim no fixture can make. The box is in the transcript now, and the failure mode
            # is not "it looks cramped" -- it is the box under the dock, or the bar overlapping the
            # controls the reader is trying to hit. Measured, not eyeballed.
            (
                "the box sits above the big controls, and the bar, switch and all, above them too "
                "(on a desk, beside them), not over them",
                "(() => { const b = document.getElementById('control-bar').getBoundingClientRect(); "
                "const p = document.getElementById('control-pane').getBoundingClientRect(); "
                "const f = document.getElementById('compose-text').getBoundingClientRect(); "
                "return f.bottom <= Math.min(b.top, p.top) + 1 && window.__visible('view-switch') "
                f"&& b.height > 0 && {BAR_CLEAR_OF_PANE_JS}; }})()",
            ),
            (
                "...and it did not push the transcript off the screen",
                "document.getElementById('scroll-area').getBoundingClientRect().height > 80",
            ),
            (
                "the call is still live — typing did not disturb it",
                "window.__text('talk-label') === 'Listening'",
            ),
        ),
    ),
    Scene(
        name="24-control-bar-top",
        what="the control bar in its other home, under the title at the top of the screen",
        act=_act_control_bar_top,
        expect=(
            (
                "the bar is really up there: above the scrolling body, not in the dock",
                "(() => { const b = document.getElementById('control-bar').getBoundingClientRect(); "
                "const s = document.getElementById('scroll-area').getBoundingClientRect(); "
                "return b.height > 0 && b.bottom <= s.top + 1; })()",
            ),
            (
                "the header is showing, because it has something in it again",
                "window.__visible('topbar')",
            ),
            (
                "the gear and the switch came with it",
                "window.__visible('open-settings') && window.__visible('view-switch')",
            ),
        ),
    ),
    Scene(
        name="25-control-bar-bottom",
        what="the default: the control bar in the dock, directly above the big buttons",
        act=_act_control_bar_bottom,
        expect=(
            # THE claim, and the half of it that is easy to get wrong. "Bottom" is not "below the
            # big buttons" and not "pinned to the viewport floor": it is the band BETWEEN the
            # transcript and the controls. Only a browser can measure that.
            (
                "the bar sits below the transcript and directly above the big buttons (on a desk, "
                "beside them)",
                "(() => { const b = document.getElementById('control-bar').getBoundingClientRect(); "
                "const s = document.getElementById('scroll-area').getBoundingClientRect(); "
                "const p = document.getElementById('control-pane').getBoundingClientRect(); "
                f"return b.height > 0 && b.top >= s.bottom - 1 && {BAR_CLEAR_OF_PANE_JS}; }})()",
            ),
            # ...and the payoff. With nothing left in the header the header is gone, and the body
            # has grown into the row. `#129 message-search` put its glass up there and kept the
            # row on every main screen; `#197 floating-search` floats the glass over the top of
            # the list instead, so the picture carries `#58`'s whole claim again: no strip above
            # the transcript, the bar really out of the header rather than drawn in both places,
            # and the glass still reachable -- over the list, below where the header was.
            (
                "the header is gone, the bar left no trace up there, and the glass floats over "
                "the list",
                "!window.__visible('topbar') && window.__visible('search-toggle') "
                "&& document.getElementById('control-bar-top').children.length === 0 "
                "&& (() => { const g = document.getElementById('search-toggle')"
                ".getBoundingClientRect(); const s = document.getElementById('scroll-area')"
                ".getBoundingClientRect(); return g.top >= s.top && g.right <= s.right; })()",
            ),
            (
                "the switch is still reachable down there",
                "window.__visible('view-switch') && window.__visible('open-settings')",
            ),
        ),
    ),
    Scene(
        name="26-voice-composer",
        what="the call's view with its message box focused and a draft in it, and the bar as it rests",
        act=_act_voice_composer,
        expect=(
            (
                "the box and its Send are on screen, at the foot of the transcript",
                "(() => { const s = document.getElementById('scroll-area').getBoundingClientRect(); "
                "const f = document.getElementById('compose-text').getBoundingClientRect(); "
                "const b = document.getElementById('send-text').getBoundingClientRect(); "
                "return window.__visible('compose-text') && window.__visible('send-text') && "
                "f.top >= s.top - 1 && b.bottom <= s.bottom + 1; })()",
            ),
            (
                "the box has the focus and the draft",
                "document.activeElement === document.getElementById('compose-text') && "
                "document.getElementById('compose-text').value === 'summarize the last hour'",
            ),
            # The owner's report, stated as the thing that must not happen: the switch and the gear
            # going away while you write. They are on the bar, where they rest.
            (
                "the switch and the gear are still on the bar while the box is focused",
                "window.__visible('view-switch') && window.__visible('open-settings')",
            ),
            (
                "clicking into the box and writing opened nothing",
                "!window.__visible('hang-up') && window.__text('talk-label') !== 'Listening'",
            ),
        ),
    ),
    Scene(
        name="27-canned-prompts",
        what=(
            "the control bar packed: gear, Prompts, Read new and the view switch, "
            "tray open above it"
        ),
        act=_act_canned_prompts,
        expect=(
            (
                "every button the bar has is on it at once",
                "['open-settings', 'prompts-open', 'read-new', "
                "'view-switch'].every((id) => window.__visible(id))",
            ),
            (
                "and the tray it opens really holds both prompts",
                "['canned-summary', 'canned-blockers'].every((id) => window.__visible(id))",
            ),
            # `#126 read-new-selector`. What an untouched page is DOING about arriving messages,
            # which is now something rather than nothing: the default is the summarising mode, at
            # the owner's request, and this is the one place the picture and the claim agree.
            (
                "Read new shows the summarising mode it defaults to",
                "document.getElementById('read-new').getAttribute('data-mode') === 'gist' && "
                "window.__text('read-new-label').length > 0",
            ),
            # THE packing claim, and the only place it can be answered: on a 375px phone five
            # controls have to fit a strip that also carries a switch with a 3.6rem word in it.
            # Nothing may hang past the bar's own right edge, because that is where the switch is.
            # (`#218 desktop-dock`: on a desk the switch is not in the bar's box but in the column's
            # lower right corner, so there it is held to the column's edge, and the pack, which
            # scrolls, to hiding nothing past its own.)
            (
                "no member is clipped past the right-hand edge of the bar (on a desk, of the column)",
                "(() => { const b = document.getElementById('control-bar').getBoundingClientRect(); "
                "const s = document.getElementById('view-switch').getBoundingClientRect(); "
                "const g = document.getElementById('open-settings').getBoundingClientRect(); "
                "const d = document.getElementById('dock'), r = d.getBoundingClientRect(); "
                "const pack = document.getElementById('bar-pack'); "
                "const desk = matchMedia('(min-width: 900px) and (pointer: fine)').matches; "
                "const edge = desk ? r.right - parseFloat(getComputedStyle(d).paddingRight) : b.right; "
                "return s.right <= edge + 1 && g.left >= b.left - 1 && s.width > 40 "
                "&& (!desk || pack.scrollWidth <= pack.clientWidth + 1); })()",
            ),
            (
                "...and it is still one strip, not two rows, clear of the big buttons",
                "(() => { const b = document.getElementById('control-bar').getBoundingClientRect(); "
                "const p = document.getElementById('control-pane').getBoundingClientRect(); "
                f"return b.height < 70 && {BAR_CLEAR_OF_PANE_JS}; }})()",
            ),
            # The tray costs the strip NOTHING, which is the entire argument for replacing two
            # members with one. Out of flow, so opening it must not make the bar taller — the
            # assertion above already pins the height, and this one pins that the panel really is
            # elsewhere: above the bar, inside the screen, and over something rather than beside it.
            (
                "the tray hangs ABOVE the bar and stays on the screen",
                "(() => { const b = document.getElementById('control-bar').getBoundingClientRect(); "
                "const t = document.getElementById('prompts-tray').getBoundingClientRect(); "
                "return t.bottom <= b.top + 1 && t.left >= -1 && "
                "t.right <= document.documentElement.clientWidth + 1 && t.top >= -1 && "
                "t.height > 20; })()",
            ),
            # ...and it is legible where it lands. An out-of-flow panel over a transcript with no
            # background of its own is text on text, which is the failure a picture shows and an
            # assertion about `hidden` never would.
            (
                "it is an opaque panel, not text floating over the transcript",
                "(() => { const bg = getComputedStyle("
                "document.getElementById('prompts-tray')).backgroundColor; "
                "return bg !== 'transparent' && !/rgba\\(0, 0, 0, 0\\)/.test(bg); })()",
            ),
        ),
    ),
    Scene(
        name="28-live-message-arrived",
        what="a message pushed by the server appears in the channel with no Refresh click",
        act=_act_live_message,
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the page really attached to the stream rather than falling back to the timer",
                "window.__streamOpens >= 1",
            ),
            (
                "the list grew",
                "document.querySelectorAll('#discord-log > li').length > window.__loadedBefore",
            ),
            (
                # THE claim. A scene that only asserted the list grew would be satisfied by the
                # background re-read, which is the very thing this feature replaces.
                "and it grew without the page going back to ask",
                "window.__refreshes === 0",
            ),
            (
                "the arriving row carries its id, exactly as a fetched one does",
                "!!document.querySelector("
                "'#discord-log > li[data-id=\"9990000000000000001\"]')",
            ),
            (
                "and its text is on screen",
                "(document.getElementById('discord-log').textContent || '')"
                ".includes('the arm64 runner came back by itself')",
            ),
        ),
    ),
    Scene(
        name="29-resume-partial",
        what="after a hang-up, with resuming armed but the record only PARTIAL — and it says so",
        act=_act_resume_partial,
        expect=(
            ("the call is over and the idle pane is back", "!window.__visible('hang-up')"),
            (
                # THE claim. "the earlier conversation is replayed" would be the feature lying
                # about a reconstruction that lost its beginning, and this is the one screen the
                # reader looks at to find out which of the two they got.
                "the clause under the control says the reconstruction was PARTIAL",
                "/replayed in part/.test(window.__text('talk-note') || '')",
            ),
            (
                "and it does not claim a whole one",
                "!/^the earlier conversation is replayed$/.test(window.__text('talk-note') || '')",
            ),
            (
                "Settings says how much was left out, not merely that something was",
                "/IN PART/.test(window.__text('resume-state') || '')",
            ),
            (
                "the transcript seam stops saying the agent remembers nothing above the line",
                "[...document.querySelectorAll('#transcript > li.seam')]"
                ".some((n) => /read the lines above back to it/.test(n.textContent || ''))",
            ),
            (
                # `#270 voice-transcript-markers`. The line above the resumed call says what that
                # call was sent, and, inside it, that the oldest lines did not fit.
                "the line above the resumed call says a recap went, and that older lines did not fit",
                "[...document.querySelectorAll('#transcript .seam-recap')]"
                ".some((n) => /^[\\d,]+-word recap sent$/.test(n.textContent || ''))"
                " && [...document.querySelectorAll('#transcript .seam-recap-detail')]"
                ".some((n) => /older lines did not fit/.test(n.textContent || ''))",
            ),
        ),
    ),
    Scene(
        name="30-channel-picker-in-bar",
        what="the channel picker on the control bar, with the channel walked back several pages",
        act=_act_channel_picker_reachable,
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the channel really is several pages deep, which is where it was unreachable",
                "window.__loadedAfter >= 12",
            ),
            (
                "the reader is parked in the history rather than at either end of it",
                "(() => { const a = document.getElementById('scroll-area'); "
                "return a.scrollTop > 50 && a.scrollTop < a.scrollHeight - a.clientHeight - 50; })()",
            ),
            # THE claim, and the one only a layout engine can settle: the picker is ON the 375px
            # screen, whole, at a scroll position several pages into the history. Clipped off the
            # right-hand edge of the packing bar would satisfy "it is in the markup" and would be
            # the same defect wearing a different coat.
            (
                "the picker is on screen entire, inside the viewport on all four sides",
                "(() => { const r = window.__pickerAfter; return r.width > 20 && r.height > 20 && "
                "r.top >= 0 && r.left >= 0 && r.right <= window.innerWidth && "
                "r.bottom <= window.innerHeight; })()",
            ),
            (
                # The regression half of `#83`: prepending history moved the picker, because the
                # picker was in the thing being prepended to. Measured across the whole walk.
                "walking the channel back did not move the picker at all",
                "window.__pickerMoved <= 1",
            ),
            (
                "and it is not inside the scrolling element, which is WHY it did not move",
                "window.__pickerInScroll === false",
            ),
        ),
    ),
    Scene(
        name="31-pull-to-refresh-armed",
        what="the channel pulled down past the threshold: the finger is still on it, and it says "
        "what letting go will do",
        act=_act_pull_armed,
        # PHONES ONLY, and for the same reason `20-discord-hover` is desktops only: this gesture
        # exists on a touch screen and nowhere else. `has_touch` is set per profile, so a capture
        # on a desktop context would be photographing an interaction that device cannot have.
        profiles=PHONE_PROFILES,
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the list really is at its top, which is the only place the pull begins",
                "document.getElementById('scroll-area').scrollTop === 0",
            ),
            (
                # THE case, and the one the old scene could not photograph: there is still history
                # above the reader. A pull that only works once the channel has been walked to its
                # beginning is a pull that does not exist on the channels this issue is about.
                "the channel still has history above the reader",
                "window.__pullMoreAbove === true && window.__visible('load-older')",
            ),
            (
                # ...and the automatic step back really did stand aside under the finger. Without
                # this the frame above is satisfiable by a page that prepended a page of history
                # mid-gesture and armed anyway.
                "no step back fired while the finger was down",
                "window.__pullRowsAfter === window.__pullRowsBefore",
            ),
            (
                # THE claim. Not "an element is visible": the state is what the reader is being
                # asked to decide, and a page that showed the same strip whatever the finger had
                # done would satisfy any weaker check.
                "the gesture is ARMED, so letting go now would refresh",
                "document.getElementById('pull-refresh').getAttribute('data-state') === 'armed'",
            ),
            (
                "and it says so in words, before the finger lifts",
                "/Release/.test(window.__text('pull-refresh') || '')",
            ),
            (
                # The geometry, which is the half no fixture can see: the affordance is over the
                # list, inside the frame, and clear of the dock it must not hide behind.
                "the affordance is on screen, clear of the dock, and not clipped by the frame",
                "(() => { const r = document.getElementById('pull-refresh')"
                ".getBoundingClientRect(); "
                "const dock = document.getElementById('dock').getBoundingClientRect(); "
                "return r.width > 40 && r.top >= 0 && r.left >= 0 && "
                "r.right <= window.innerWidth && r.bottom <= dock.top; })()",
            ),
        ),
    ),
    Scene(
        name="32-channel-summarised",
        what="the channel in summary mode: a long message shown as one line about it, not its first three",
        act=_act_summary_mode,
        after=_summaries_off_after,
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the control says the mode is on",
                "document.getElementById('summarise').getAttribute('aria-pressed') === 'true'",
            ),
            # THE claim, and the only place in the repository it can be made at all: a summarised
            # row is SHORTER than the same row clamped to three lines. The behavioural suite has no
            # renderer and cannot compare two heights; a scene pinned to "a summary line exists"
            # would be satisfied by one sitting on top of the clamped message, which is the
            # obvious wrong implementation and is not a shorter row.
            (
                "the summarised row really is shorter than the clamped one it replaced",
                "window.__summarisedHeight > 0 && window.__summarisedHeight < window.__foldedHeight",
            ),
            (
                "the probed row shows a real summary, not the placeholder shown while it is fetched",
                SUMMARY_PROBE_SUMMARISED,
            ),
            (
                "the message it summarises is not also on screen underneath it",
                "(() => { const row = document.getElementById('summary-probe'); "
                "return row.querySelector('.body').offsetParent === null; })()",
            ),
            (
                # Quoted from the server's own answer, never from a constant in the page. Every
                # line on this screen was written by a model reading somebody else's message, and
                # a screen that showed those lines without saying where they came from would be
                # offering the reader a reading they cannot check.
                "the view says which summariser produced them",
                "/Summaries by .+/.test(window.__text('summary-note') || '')",
            ),
        ),
    ),
    Scene(
        name="33-todo-view",
        what="the channel filtered to what you have not dealt with, one message already cleared",
        act=_act_todo_view,
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the filter says it is on",
                "document.getElementById('todo-filter').getAttribute('aria-pressed') === 'true'",
            ),
            (
                "a message the reader dealt with really left the list",
                "window.__afterTodo > 0 && window.__afterTodo < window.__beforeTodo && "
                "![...document.querySelectorAll('#discord-log > li')]"
                ".some((n) => n.getAttribute('data-id') === window.__dealtWith)",
            ),
            (
                # THE thing this frame exists to settle, and the reason it is a picture rather
                # than an assertion: `#61 unread-status` says this has to be said in the
                # interface, and whether a sentence is legible on a 375px phone above a list is
                # not a question any fixture can answer.
                # Matched on what the sentence CLAIMS, not on the vendor's name. The server's
                # notice stopped naming the service -- it says "the source chat service" now,
                # because the bridge is no longer written against exactly one of them -- and a
                # regex for the old name failed while the sentence it was checking for was on
                # screen, correct, and legible. The two halves below are the claim itself.
                "the page says on screen that this read state is its own, not the source's",
                "/vibe-talk's own/.test(window.__text('inbox-note') || '') && "
                "/source/.test(window.__text('inbox-note') || '') && "
                "window.__visible('inbox-note')",
            ),
            ("there is a way back from the dismissal", "window.__visible('undo-dismiss')"),
            (
                "every remaining row offers the act, and none of them is clipped past the edge",
                "(() => { const rows = [...document.querySelectorAll('#discord-log > li')]; "
                "const pane = document.getElementById('pane-discord').getBoundingClientRect(); "
                "return rows.length > 0 && rows.every((n) => { "
                "const b = n.querySelector('.done-button'); "
                "return b && b.getBoundingClientRect().right <= pane.right + 1; }); })()",
            ),
        ),
    ),
    Scene(
        name="34-bankruptcy-armed",
        what="the bulk clear with one tap on it: saying how many it is about to take, before it does",
        act=_act_bankruptcy_armed,
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                # The whole content of this frame. A bulk destructive control that did not say
                # how much it was about to take is the defect; one that says it only AFTER acting
                # is the same defect with a receipt.
                "the armed control names the number it is about to clear",
                "(window.__text('clear-backlog') || '').includes(window.__backlog + '?')",
            ),
            (
                "nothing has been cleared yet: the backlog is all still there",
                "document.querySelectorAll('#discord-log > li').length === window.__backlog",
            ),
            (
                "and it is drawn as armed rather than merely relabelled",
                "document.getElementById('clear-backlog').className.includes('armed')",
            ),
        ),
    ),
    Scene(
        name="35-message-search",
        what="the search glass open over the channel, a quoted phrase typed, the list filtered down",
        act=_act_message_search,
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the glass reads as open and the phrase is in the field",
                "document.getElementById('search-toggle').getAttribute('aria-pressed') === 'true' "
                "&& document.getElementById('search-field').value.includes('mac runner')",
            ),
            (
                # THE browser-only half. The page suite can assert that a filtered row carries the
                # class; whether the class reaches the pixels -- past the row's own display mode,
                # past a later declaration, past a stylesheet that simply does not declare it --
                # is a rendering fact, and this is the only thing in the repository that sees it.
                # A state pinned to the class alone would be green with the rule deleted, which is
                # a search that visibly does nothing.
                "a filtered row is really not drawn, not merely marked",
                "(() => { const gone = document.querySelector('#discord-log > li.search-hidden'); "
                "return gone !== null && gone.offsetParent === null; })()",
            ),
            (
                # Quoting is the clause of the request that no other frame shows. The seeded
                # backlog says "runner" several times and "mac runner" once, so a picture where
                # the quotes changed nothing would be visibly a different picture.
                "what is left is fewer than arrived, and every row of it holds the PHRASE",
                "(() => { const left = [...document.querySelectorAll("
                "'#discord-log > li:not(.search-hidden)')]; "
                "return left.length > 0 && left.length < window.__beforeSearch "
                "&& left.every((n) => /mac runner/i.test(n.textContent)); })()",
            ),
            (
                # `#128 transcript-history` is why the denominator is on screen at all: what is
                # loaded is a suffix of a longer record, so "no matches" and "never said" are
                # different answers and a bare number would let a reader conclude the second.
                "the count is on screen and names what it counted out of",
                "window.__visible('search-count') && "
                "/\\bof \\d+ loaded\\b/.test(window.__text('search-count') || '')",
            ),
            (
                # The layout claim, and the reason this scene runs on iphone-se as well. Field,
                # count and glass, left to right, sharing ONE band -- the floating bar over the top
                # of the list since `#197 floating-search`, where the header row used to be asked
                # for -- and nothing pushed past the right edge. `.search-field` carries
                # `min-width: 0` for exactly this, because a flex item's default minimum is its
                # content width and a long query would otherwise shove the glass off a 375px
                # screen. The header is not up at all: opening the search costs no row either.
                # "Over the list" is over the list's place: its box less its top margin, which on a
                # narrow phone is the room under the author filter hanging below the bar, where the
                # list starts (`#251 author-filter`).
                "field, count, Links, Pinned and glass are one row, in that order, on the bar over "
                "the list",
                "(() => { const f = document.getElementById('search-field').getBoundingClientRect(); "
                "const c = document.getElementById('search-count').getBoundingClientRect(); "
                "const l = document.getElementById('links-filter').getBoundingClientRect(); "
                "const p = document.getElementById('pinned-filter').getBoundingClientRect(); "
                "const g = document.getElementById('search-toggle').getBoundingClientRect(); "
                "const bar = document.getElementById('search-float').getBoundingClientRect(); "
                "const area = document.getElementById('scroll-area'); "
                "const s = area.getBoundingClientRect(); "
                "const head = s.top - (parseFloat(getComputedStyle(area).marginTop) || 0); "
                "return f.width > 80 && c.width > 0 && g.width > 0 && l.width >= 44 && p.width >= 44 "
                "&& f.right <= c.left + 1 && c.right <= l.left + 1 && l.right <= p.left + 1 "
                "&& p.right <= g.left + 1 && g.right <= bar.right + 1 "
                "&& bar.top >= head - 1 && bar.right <= s.right + 1 "
                "&& !window.__visible('topbar') "
                "&& f.top < c.bottom && c.top < f.bottom "
                "&& f.top < g.bottom && g.top < f.bottom; })()",
            ),
            (
                # `#212 link-filter`. The kinds of link belong to Links, and Links is off here: a
                # search is a field and its filters, with nothing hung under them.
                "with Links off, no kinds of link hang under the bar",
                "!window.__visible('link-kinds') "
                "&& !document.getElementById('screen-main').hasAttribute('data-link-kinds')",
            ),
        ),
    ),
    Scene(
        name="37-links-filter",
        what="the Links filter on over the channel: only the rows with links, each its links alone",
        act=_act_links_filter,
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                # The toggle the owner asked for, where he asked for it: on, and immediately beside
                # Pinned, which is immediately beside the glass, all on the one bar.
                "Links reads as on, immediately left of Pinned, which is left of the glass",
                "(() => { const l = document.getElementById('links-filter'); "
                "const a = l.getBoundingClientRect(); "
                "const p = document.getElementById('pinned-filter').getBoundingClientRect(); "
                "const g = document.getElementById('search-toggle').getBoundingClientRect(); "
                "return l.getAttribute('aria-pressed') === 'true' && a.width >= 44 "
                "&& a.right <= p.left + 1 && p.left - a.right < 20 && p.right <= g.left + 1 "
                "&& Math.abs((a.top + a.bottom) / 2 - (g.top + g.bottom) / 2) < 2; })()",
            ),
            (
                # THE rendering claim. The page suite asserts the attribute and the rule that hides
                # the text by it; whether the text is really gone from the pixels, past the fold's
                # own display and the summary's, is a layout fact only a browser sees.
                "only rows with links are drawn, and each draws its links instead of its text",
                "(() => { const rows = [...document.querySelectorAll('#discord-log > li[data-id]')]"
                ".filter((r) => r.offsetParent !== null); "
                "return rows.length > 0 && rows.every((r) => r.getAttribute('data-links-view') === "
                "'true' && r.querySelector(':scope > .body').offsetParent === null "
                "&& r.querySelector('.row-links > a') !== null); })()",
            ),
            (
                "the Markdown link shows its name, and opens in a new tab",
                "[...document.querySelectorAll('.row-links > a')].some((a) => "
                "a.textContent === 'the diff' && a.getAttribute('href') === "
                "'https://example.com/diff/42' && a.target === '_blank')",
            ),
            (
                "the long address is cut short on screen, inside the screen, with its href whole",
                "(() => { const a = [...document.querySelectorAll('.row-links > a')]"
                ".find((n) => n.href.endsWith('/summary.html')); "
                "return Boolean(a) && a.scrollWidth > a.clientWidth "
                "&& a.getBoundingClientRect().right <= document.documentElement.clientWidth; })()",
            ),
            (
                # `#212 link-filter`. The owner: "3 buttons to pop up under the link icon I just
                # clicked ... named PRs Commits Actions and be toggled on by default" — and then
                # `#266 link-filter-kinds`, "add Issues and Files and Other". The layout half only a
                # browser has: below the bar, spanning the Links icon's middle, inside the screen and
                # no wider than the bar, each a whole 44px target; the pull request counted, and the
                # two documents beside it counted as Other.
                "PRs, Commits, Actions, Issues, Files and Other hang under Links, all on, inside the screen, 44px each",
                "(() => { const row = document.getElementById('link-kinds'); "
                "const r = row.getBoundingClientRect(); "
                "const l = document.getElementById('links-filter').getBoundingClientRect(); "
                "const bar = document.getElementById('search-float').getBoundingClientRect(); "
                "const mid = (l.left + l.right) / 2; "
                "const chips = [...row.querySelectorAll('button')]; "
                "return window.__visible('link-kinds') "
                "&& chips.map((c) => c.firstChild.textContent).join(' ') === 'PRs Commits Actions Issues Files Other' "
                "&& chips.every((c) => c.getAttribute('aria-pressed') === 'true' "
                "&& c.getBoundingClientRect().width >= 44 && c.getBoundingClientRect().height >= 44) "
                "&& window.__text('link-kind-pr-count') === '1' "
                "&& window.__text('link-kind-other-count') === '2' "
                "&& r.top >= bar.bottom - 1 && r.left <= mid && r.right >= mid "
                "&& r.width <= bar.width + 1 "
                "&& r.left >= 0 && r.right <= document.documentElement.clientWidth; })()",
            ),
        ),
    ),
    Scene(
        name="38-link-kinds-off",
        what="the Links view with PRs turned off under the Links button: the pull request gone from its row",
        act=_act_link_kinds_off,
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                # Off says so by more than colour: struck through, on the panel at rest, beside the
                # five that are still on.
                "PRs reads as off and is struck through; the other five kinds are still on",
                "(() => { const pr = document.getElementById('link-kind-pr'); "
                "return pr.getAttribute('aria-pressed') === 'false' "
                "&& getComputedStyle(pr).textDecorationLine.includes('line-through') "
                "&& ['link-kind-commit', 'link-kind-action', 'link-kind-issue', 'link-kind-file', "
                "'link-kind-other'].every((id) => "
                "document.getElementById(id).getAttribute('aria-pressed') === 'true'); })()",
            ),
            (
                # THE filtering claim: the row keeps its other links, and the pull request is not
                # drawn anywhere on the list.
                "the row keeps its other links and the pull request is drawn nowhere",
                "(() => { const row = document.querySelector('#discord-log > li[data-id=\"9990000000000000211\"]'); "
                "const hrefs = [...row.querySelectorAll('.row-links > a')].map((a) => a.getAttribute('href')); "
                "return row.offsetParent !== null && hrefs.includes('https://example.com/diff/42') "
                "&& hrefs.some((h) => h.endsWith('/summary.html')) "
                "&& ![...document.querySelectorAll('.row-links > a')].some((a) => "
                "a.getAttribute('href').includes('/pull/') && a.offsetParent !== null); })()",
            ),
            (
                "the choice is kept on this device",
                "localStorage.getItem('vibe-talk.voice.hidden-link-kinds') === '[\"pr\"]'",
            ),
        ),
    ),
    Scene(
        name="39-desktop-dock-channel",
        what="the dock on a desk over the channel: one row of ordinary controls across the window, names whole",
        act=_act_open_channel,
        # `#218 desktop-dock`. Desks only: the phone keeps its thumb-sized dock, and these sizes on a
        # phone would be the defect, not the feature.
        profiles=DESKTOP_PROFILES,
        expect=(
            (
                "the channel is up, with its reading buttons in the dock",
                "window.__visible('pane-discord') && window.__visible('read-aloud')",
            ),
            DESK_DOCK_HEIGHT,
            DESK_CONTROL_SIZES,
            DESK_SWITCH_CORNER,
            (
                "Hide read, Pace and Read, left to right as on the phone, and no Sound among them",
                pane_reads_js("todo-filter,read-speed,read-aloud"),
            ),
            (
                # THE other half of the complaint: a picker cut short beside empty space. Measured
                # against the same select sized to the name it shows, with no cap and no shrink.
                "the channel picker is as wide as the name it is showing",
                "(() => { const sel = document.getElementById('discord-channel'); "
                "const probe = document.createElement('select'); probe.className = sel.className; "
                "const o = document.createElement('option'); "
                "o.textContent = sel.selectedOptions[0].textContent; probe.append(o); "
                "probe.style.cssText = 'position:absolute;visibility:hidden;max-width:none;"
                "min-width:0;flex:none;field-sizing:content'; sel.parentNode.append(probe); "
                "const natural = probe.getBoundingClientRect().width; probe.remove(); "
                "return sel.getBoundingClientRect().width >= natural - 1; })()",
            ),
            (
                "the dock's controls are one row across the window and overlap nothing",
                dock_one_row_js(),
            ),
        ),
    ),
    Scene(
        name="40-desktop-dock-call",
        what="the dock on a desk during a live call: Prompts and Gist beside Sound, Clear, Hang up, Listening",
        act=_act_desktop_dock_call,
        profiles=DESKTOP_PROFILES,
        expect=(
            ("the call is live", "window.__text('talk-label') === 'Listening'"),
            DESK_DOCK_HEIGHT,
            DESK_CONTROL_SIZES,
            DESK_SWITCH_CORNER,
            (
                "Sound, Clear, Hang up and Listening, left to right, with Prompts on the bar",
                pane_reads_js("speaker,clear-view,hang-up,talk")
                + " && window.__visible('prompts-open') && window.__visible('read-new')",
            ),
            (
                "the dock's controls are one row across the window and overlap nothing",
                dock_one_row_js(),
            ),
        ),
    ),
    Scene(
        name="41-message-blocks",
        what="an agent's list message as the server rendered it: round bullets, hanging indent, paragraph gaps",
        act=_act_message_blocks,
        profiles=("iphone15", "iphone-se", "desktop", "laptop-1280", "android-412"),
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the probed message is on screen, opened in full",
                "(() => { const r = document.getElementById('message-blocks-probe'); "
                "if (!r) return false; const b = r.getBoundingClientRect(); "
                "return r.getAttribute('data-collapsed') !== 'true' && b.top >= 0 "
                "&& b.bottom <= innerHeight; })()",
            ),
            (
                # The server's blocks, not the page's: one list of six, between paragraphs.
                "its body is a paragraph, ONE list of six items, and two paragraphs",
                "(() => { const md = document.querySelector('#message-blocks-probe .md'); "
                "return [...md.children].map((n) => n.tagName).join(' ') === 'P UL P P' "
                "&& md.querySelectorAll('ul > li').length === 6; })()",
            ),
            (
                "the bullets are round",
                "getComputedStyle(document.querySelector('#message-blocks-probe .md ul'))"
                ".listStyleType === 'disc'",
            ),
            (
                # THE layout claim the page suite cannot make. The item's text is indented clear of
                # the list's edge, and an item's continuation line starts exactly where its first
                # line does -- under the text, not under the bullet.
                "a continuation line hangs under its item's text, clear of the bullet",
                "(() => { const ul = document.querySelector('#message-blocks-probe .md ul'); "
                "const li = ul.children[1]; const range = document.createRange(); "
                "range.selectNodeContents(li); const lines = [...range.getClientRects()]; "
                "const first = lines[0], last = lines[lines.length - 1]; "
                "return li.getBoundingClientRect().left - ul.getBoundingClientRect().left >= 10 "
                "&& last.top > first.top + 4 && Math.abs(last.left - first.left) < 1.5; })()",
            ),
            (
                # A blank line is space: between the introduction and the list, and between the two
                # closing paragraphs that used to run together.
                "a blank line is a gap between blocks, not nothing",
                "(() => { const md = document.querySelector('#message-blocks-probe .md'); "
                "const [intro, list, last1, last2] = md.children; "
                "const gap = (a, b) => b.getBoundingClientRect().top - a.getBoundingClientRect().bottom; "
                "return gap(intro, list) >= 5 && gap(list, last1) >= 5 && gap(last1, last2) >= 5; })()",
            ),
            (
                "no Markdown is left on screen",
                "!/(^|\\n)- /.test(document.querySelector('#message-blocks-probe .body').innerText)",
            ),
        ),
    ),
    Scene(
        name="42-reply-count-and-marker",
        what="Reply (2) on a message two replies answer, and the circle on its corner that leads to them",
        act=_act_reply_marker,
        profiles=("android-412", "laptop-1280"),
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the message with two replies says so on its Reply, and in words to a screen reader",
                f"(() => {{ const b = window.__replyRow({json.dumps(REPLY_MARKER_PARENT)})"
                ".querySelector('.reply-button'); return b.textContent === 'Reply (2)' "
                "&& b.getAttribute('aria-label') === 'Reply, 2 replies already'; })()",
            ),
            (
                # The arrow from the reply directly under it already meets its foot: one end there.
                "the message answered only from directly below reads Reply (1), with no circle",
                f"(() => {{ const row = window.__replyRow({json.dumps(REPLY_MARKER_ADJACENT)}); "
                "return row.querySelector('.reply-button').textContent === 'Reply (1)' "
                "&& !window.__replyDisc(row); })()",
            ),
            (
                "the circle is a button named for where it goes, and the whole of both rows is on screen",
                f"(() => {{ const d = window.__replyDisc(window.__replyRow({json.dumps(REPLY_MARKER_PARENT)})); "
                "const area = document.getElementById('scroll-area').getBoundingClientRect(); "
                f"const rows = [{json.dumps(REPLY_MARKER_PARENT)}, {json.dumps(REPLY_MARKER_ADJACENT)}]"
                ".map((t) => window.__replyRow(t).getBoundingClientRect()); "
                "return d && d.marker.tagName === 'BUTTON' "
                "&& d.marker.getAttribute('aria-label') === 'Jump to reply 1 of 2 to this message' "
                "&& rows.every((r) => r.top >= area.top && r.bottom <= area.bottom); })()",
            ),
            (
                # THE geometry claim, which only a layout engine can make. Centred on the corner of
                # the border box, within a pixel -- or, for the agent's tile on a phone, whose corner
                # is the edge of the screen, just inside it, its left edge on the tile's.
                "the disc is an arrowhead wide and centred on the row's lower-left corner",
                f"(() => {{ const row = window.__replyRow({json.dumps(REPLY_MARKER_PARENT)}); "
                "const d = window.__replyDisc(row), r = row.getBoundingClientRect(); "
                "const rem = parseFloat(getComputedStyle(document.documentElement).fontSize); "
                "const edge = row.getAttribute('data-who') === 'coder' && innerWidth < 900 "
                "&& !row.hasAttribute('data-is-reply'); "
                "const x = edge ? r.left + d.r : r.left; "
                "return Math.abs(d.r * 2 - 0.7 * rem) < 0.5 && Math.abs(d.x - x) <= 1 "
                "&& Math.abs(d.y - r.bottom) <= 1; })()",
            ),
            (
                "the disc is drawn in the arrow's blue",
                f"(() => {{ const d = window.__replyDisc(window.__replyRow({json.dumps(REPLY_MARKER_PARENT)})); "
                "const arrow = document.querySelector('#discord-log > li > .reply-jump'); "
                "return getComputedStyle(d.marker, '::before').backgroundColor === "
                "getComputedStyle(arrow).color; })()",
            ),
            (
                # Over the row's border and its veil: what is under the disc's centre is the circle.
                "the disc is on top of the row's border, where a press finds it",
                f"(() => {{ const d = window.__replyDisc(window.__replyRow({json.dumps(REPLY_MARKER_PARENT)})); "
                "const hit = document.elementFromPoint(d.x, d.y); "
                "return hit !== null && d.marker.contains(hit); })()",
            ),
            (
                "the disc covers none of the row's words, buttons or chips",
                f"(() => {{ const row = window.__replyRow({json.dumps(REPLY_MARKER_PARENT)}); "
                "const d = window.__replyDisc(row); "
                "const parts = [...row.querySelectorAll('.body, .summary, .reactions, button, .chip, a')]"
                ".filter((n) => n !== d.marker && !d.marker.contains(n) && n.getClientRects().length > 0); "
                "return parts.length > 3 && parts.every((n) => { const b = n.getBoundingClientRect(); "
                "if (b.width < 2 || b.height < 2) return true; "
                "return b.right <= d.x - d.r || b.left >= d.x + d.r || "
                "b.bottom <= d.y - d.r || b.top >= d.y + d.r; }); })()",
            ),
            (
                # A label four characters longer must not push the row's controls onto a new line.
                "Reply (2) leaves the row's controls on one line",
                f"(() => {{ const row = window.__replyRow({json.dumps(REPLY_MARKER_PARENT)}); "
                "const tops = [...row.querySelectorAll(':scope > .meta > .row-acts > button, :scope > .meta > .row-acts > .row-more')]"
                ".filter((n) => n.getClientRects().length > 0).map((n) => n.getBoundingClientRect().top); "
                "return tops.length >= 3 && Math.max(...tops) - Math.min(...tops) < 4; })()",
            ),
        ),
    ),
    Scene(
        name="43-reply-marker-jumped",
        what="the circle tapped: the first reply at the head of the list, lit, and the circle on to the next",
        act=_act_reply_marker_jump,
        # The act holds the landing's light where it starts; disabling animations would end it.
        freeze_animations=False,
        profiles=("android-412", "laptop-1280"),
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the earliest reply is lit, at the head of the list",
                f"(() => {{ const row = window.__replyRow({json.dumps(REPLY_MARKER_FIRST_REPLY)}); "
                "const area = document.getElementById('scroll-area').getBoundingClientRect(); "
                "const top = row.getBoundingClientRect().top; "
                "return row.getAttribute('data-landed') === 'true' && top >= area.top "
                "&& top < area.top + area.height / 3; })()",
            ),
            (
                "the status line says which reply it is",
                "(window.__text('status') || '') === 'Here is reply 1 of 2 to it.'",
            ),
            (
                "the circle says its next tap goes to the second",
                f"window.__replyDisc(window.__replyRow({json.dumps(REPLY_MARKER_PARENT)})).marker"
                ".getAttribute('aria-label') === 'Jump to reply 2 of 2 to this message'",
            ),
        ),
    ),
    Scene(
        name="44-reply-marker-beside-arrow",
        what="a message answered from directly below and from further away: the arrow's head and the circle at one corner, apart",
        act=_act_reply_marker_beside_arrow,
        profiles=("android-412", "laptop-1280"),
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the message answered twice reads Reply (2), with a circle, and the arrow below knows it",
                f"(() => {{ {REPLY_HEAD_JS}; const row = window.__replyRow({json.dumps(REPLY_MARKER_ADJACENT)}); "
                "const head = window.__replyHead(row); "
                "return row.querySelector('.reply-button').textContent === 'Reply (2)' "
                "&& window.__replyDisc(row) !== null "
                "&& head.arrow.getAttribute('data-reply-style') === 'adjacent' "
                "&& head.arrow.hasAttribute('data-reply-hub'); })()",
            ),
            (
                # THE claim: two ends of two connections at one corner, neither drawn over the other.
                "the disc keeps at least two pixels clear of the head",
                f"(() => {{ {REPLY_HEAD_JS}; const row = window.__replyRow({json.dumps(REPLY_MARKER_ADJACENT)}); "
                "const d = window.__replyDisc(row), h = window.__replyHead(row).box; "
                "const nx = Math.max(h[0], Math.min(d.x, h[2])), ny = Math.max(h[1], Math.min(d.y, h[3])); "
                "return Math.hypot(d.x - nx, d.y - ny) - d.r >= 2; })()",
            ),
            (
                "a press on the disc is the circle's, and a press on the head is the arrow's",
                f"(() => {{ {REPLY_HEAD_JS}; const row = window.__replyRow({json.dumps(REPLY_MARKER_ADJACENT)}); "
                "const d = window.__replyDisc(row), head = window.__replyHead(row); "
                "const onDisc = document.elementFromPoint(d.x, d.y); "
                "const onHead = document.elementFromPoint(head.press[0], head.press[1]); "
                "return onDisc !== null && d.marker.contains(onDisc) && onHead !== null "
                "&& head.arrow.contains(onHead); })()",
            ),
        ),
    ),
    Scene(
        name="45-settings-text-size",
        what="Settings, Text size: a − and a + either side of the slider, one press of + in, the ring on −",
        act=_act_settings_text_size,
        profiles=("android-412", "laptop-1280"),
        expect=(
            ("the settings screen is up", "window.__visible('screen-settings')"),
            (
                "the slider and both buttons are on screen",
                "['msg-scale-smaller', 'msg-scale', 'msg-scale-larger'].every((id) => { "
                "const b = document.getElementById(id).getBoundingClientRect(); "
                "return window.__visible(id) && b.top >= 0 && b.bottom <= innerHeight; })",
            ),
            (
                # THE layout claim: one row, never the + wrapped under the slider.
                "−, the slider and + are one row, left to right, centred on one line",
                "(() => { const [m, s, p] = ['msg-scale-smaller', 'msg-scale', 'msg-scale-larger']"
                ".map((id) => document.getElementById(id).getBoundingClientRect()); "
                "const mid = (b) => b.top + b.height / 2; "
                "return m.right <= s.left && s.right <= p.left "
                "&& Math.abs(mid(m) - mid(s)) < 3 && Math.abs(mid(p) - mid(s)) < 3; })()",
            ),
            (
                "each button is a target of at least 44 by 44 CSS pixels",
                "['msg-scale-smaller', 'msg-scale-larger'].every((id) => { "
                "const b = document.getElementById(id).getBoundingClientRect(); "
                "return b.width >= 44 && b.height >= 44; })",
            ),
            (
                "the slider keeps a track a thumb can use beside them",
                "document.getElementById('msg-scale').getBoundingClientRect().width >= 150",
            ),
            (
                "one press of + moved the slider a step and the line under it says so",
                "document.getElementById('msg-scale').value === '105' "
                "&& window.__text('msg-scale-state') === 'Saved — message text at 105% of its usual size.' "
                "&& !document.getElementById('msg-scale-smaller').disabled "
                "&& !document.getElementById('msg-scale-larger').disabled",
            ),
            (
                "the keyboard is on −, and the ring is drawn",
                "document.activeElement === document.getElementById('msg-scale-smaller') "
                "&& document.activeElement.matches(':focus-visible') "
                "&& getComputedStyle(document.activeElement).outlineStyle === 'solid'",
            ),
        ),
    ),
    Scene(
        name="46-nested-lists",
        what="numbered steps with sub-steps indented two spaces, as chat writes them: nested, indented, numbered as written",
        act=_act_nested_lists,
        profiles=("android-412", "laptop-1280"),
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the probed message is on screen, opened in full",
                "(() => { const r = document.getElementById('nested-lists-probe'); "
                "if (!r) return false; const b = r.getBoundingClientRect(); "
                "return r.getAttribute('data-collapsed') !== 'true' && b.top >= 0 "
                "&& b.bottom <= innerHeight; })()",
            ),
            (
                # The server's nesting: one numbered list from 9, each step holding its own steps.
                "its body is a paragraph and ONE list from 9 whose steps hold their sub-steps",
                "(() => { const md = document.querySelector('#nested-lists-probe .md'); "
                "if ([...md.children].map((n) => n.tagName).join(' ') !== 'P OL') return false; "
                "const ol = md.children[1], steps = [...ol.children]; "
                "const count = (n, s) => n.querySelectorAll(s).length; "
                "return ol.getAttribute('start') === '9' && steps.length === 3 "
                "&& count(steps[0], ':scope > ul > li') === 2 "
                "&& count(steps[0], ':scope > ul > li > ul > li') === 1 "
                "&& count(steps[1], ':scope > ol > li') === 2 "
                "&& count(steps[2], ':scope > ul > li') === 1; })()",
            ),
            (
                # THE layout claim the unit tests cannot make: each level's text starts clear to the
                # right of the text of the level above it.
                "each level is indented clear of the one above it",
                "(() => { const md = document.querySelector('#nested-lists-probe .md'); "
                "const left = (n) => n.getBoundingClientRect().left; "
                "const step = md.querySelector(':scope > ol > li'); "
                "const sub = step.querySelector(':scope > ul > li'); "
                "const third = sub.parentElement.querySelector(':scope > li > ul > li'); "
                "return left(sub) - left(step) >= 12 && left(third) - left(sub) >= 12; })()",
            ),
            (
                # Depth marked as a browser's own sheet marks it, counting numbered lists too.
                "the markers say the depth: a circle under a number, a square below it",
                "(() => { const md = document.querySelector('#nested-lists-probe .md'); "
                "const type = (s) => getComputedStyle(md.querySelector(s)).listStyleType; "
                "return type(':scope > ol') === 'decimal' "
                "&& type(':scope > ol > li > ul') === 'circle' "
                "&& type(':scope > ol > li > ul > li > ul') === 'square' "
                "&& type(':scope > ol > li > ol') === 'decimal' "
                "&& !md.querySelector(':scope > ol > li > ol').hasAttribute('start'); })()",
            ),
            (
                # A two-digit number is wider than a bullet, and hangs in the same kind of gutter:
                # `10.` has to fit in it rather than stick out past the text above.
                "the widest number fits inside its list's gutter",
                "(() => { const ol = document.querySelector('#nested-lists-probe .md > ol'); "
                "const c = document.createElement('canvas').getContext('2d'); "
                "c.font = getComputedStyle(ol.children[1]).font; "
                "return c.measureText('10. ').width "
                "<= parseFloat(getComputedStyle(ol).paddingLeft) + 0.5; })()",
            ),
            (
                "no Markdown is left on screen",
                "!/(^|\\n)\\s*(- |\\d+\\. )/.test("
                "document.querySelector('#nested-lists-probe .body').innerText)",
            ),
        ),
    ),
    # `#240 reading-bookmark`. Before the last state, which purges the store on the way in and so takes
    # the bookmark these two leave behind with it.
    Scene(
        name="47-reading-bookmark",
        what="the reading bookmark: a line in the accent above one message, between it and the row above",
        act=_act_reading_bookmark,
        profiles=("android-412", "laptop-1280"),
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the line is on screen, a separator that says Bookmark",
                "(() => { const l = document.querySelector('#bookmark-probe > .bookmark-line'); "
                "if (!l) return false; const b = l.getBoundingClientRect(); "
                "const w = l.querySelector(':scope > .bookmark-word'); "
                "return w && w.getAttribute('role') === 'separator' && /bookmark/i.test(l.textContent) "
                "&& b.top >= 0 && b.bottom <= innerHeight && b.height >= 20; })()",
            ),
            (
                # `#420 bookmark-clear`: the X at its end, a touch target, named for what it does.
                "the X at the line's end is a 24px target named Remove bookmark",
                "(() => { const l = document.querySelector('#bookmark-probe > .bookmark-line'); "
                "const x = l && l.querySelector(':scope > .bookmark-clear'); if (!x) return false; "
                "const b = x.getBoundingClientRect(); const r = l.getBoundingClientRect(); "
                "return x.getAttribute('aria-label') === 'Remove bookmark' && b.width >= 24 && b.height >= 24 "
                "&& Math.abs(r.right - b.right) <= 1; })()",
            ),
            (
                # THE layout claim: the line is in the room the row keeps above itself, touching
                # neither the message it is above nor the one before it.
                "the line sits between the bookmarked message and the row above, clear of both",
                "(() => { const row = document.getElementById('bookmark-probe'); "
                "const prev = row.previousElementSibling; "
                "const l = row.querySelector(':scope > .bookmark-line').getBoundingClientRect(); "
                "const r = row.getBoundingClientRect(); "
                "return prev && l.bottom <= r.top + 1 && l.top >= prev.getBoundingClientRect().bottom - 1; })()",
            ),
            (
                "it is drawn in the accent, a solid rule of two pixels either side of the word",
                "(() => { const l = document.querySelector('#bookmark-probe > .bookmark-line'); "
                "const probe = document.createElement('span'); probe.style.color = 'var(--accent)'; "
                "document.body.append(probe); const accent = getComputedStyle(probe).color; probe.remove(); "
                "const rule = getComputedStyle(l, '::before'); "
                "return getComputedStyle(l).color === accent && rule.borderTopStyle === 'solid' "
                "&& rule.borderTopWidth === '2px' && rule.borderTopColor === accent; })()",
            ),
            (
                "the way back to it is not offered while it is on screen",
                "!window.__visible('jump-bookmark')",
            ),
            (
                "nothing runs off the side of the screen",
                "document.scrollingElement.scrollWidth <= innerWidth",
            ),
        ),
    ),
    Scene(
        name="48-bookmark-chip",
        what="the bookmark scrolled away above: the Bookmark chip right beside Newest, pointing up",
        act=_act_bookmark_chip,
        profiles=("android-412", "laptop-1280"),
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the line is above the screen",
                "document.querySelector('#bookmark-probe > .bookmark-line').getBoundingClientRect().bottom "
                "<= document.getElementById('scroll-area').getBoundingClientRect().top + 1",
            ),
            (
                "the chip points up, toward the line",
                "document.getElementById('jump-bookmark').getAttribute('data-direction') === 'up' "
                "&& /above/.test(document.getElementById('jump-bookmark').getAttribute('aria-label'))",
            ),
            (
                # On the bottom line beside Newest at a phone's width too: the two the reader goes
                # between, and the other chips wrap upward rather than between them.
                "the chip stands directly right of Newest, on the same line, overlapping nothing",
                "(() => { const n = document.getElementById('jump-newest').getBoundingClientRect(); "
                "const b = document.getElementById('jump-bookmark').getBoundingClientRect(); "
                "const others = [...document.querySelectorAll('#scroll-tools > .chip')]"
                ".filter((c) => c.id !== 'jump-newest' && c.id !== 'jump-bookmark' && c.getClientRects().length); "
                "const apart = (x, y) => x.right <= y.left || y.right <= x.left || x.bottom <= y.top || y.bottom <= x.top; "
                "return b.left >= n.right - 1 && b.left - n.right < 16 && Math.abs(b.bottom - n.bottom) < 2 "
                "&& others.every((c) => apart(c.getBoundingClientRect(), b) && apart(c.getBoundingClientRect(), n)); })()",
            ),
            (
                "nothing runs off the side of the screen",
                "document.scrollingElement.scrollWidth <= innerWidth",
            ),
        ),
    ),
    Scene(
        name="49-failed-send-and-draft",
        what="a refused send in the outbox with Retry, Edit and Dismiss, and Draft beside Reply on the newest message "
        "(on a phone, where that message is collapsed, Draft on its header line with no Reply drawn)",
        act=_act_failed_send_and_draft,
        profiles=("android-412", "laptop-1280"),
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the refused send is in the outbox, says it was not sent, and offers Retry, Edit and "
                "Dismiss, all of it on screen",
                "(() => { const row = document.querySelector('#outgoing-log > li[data-send-state=\"failed\"]'); "
                "if (!row) return false; const r = row.getBoundingClientRect(); "
                "const area = document.getElementById('scroll-area').getBoundingClientRect(); "
                "const names = [...row.querySelectorAll('.outgoing-actions button')].map((b) => b.textContent); "
                "return names.join(',') === 'Retry,Edit,Dismiss' && /Not sent/.test(row.textContent) "
                "&& r.top >= area.top && r.bottom <= area.bottom; })()",
            ),
            (
                "each of the three is a target a thumb can use",
                "[...document.querySelectorAll('#outgoing-log .outgoing-actions button')]"
                ".every((b) => b.getBoundingClientRect().height >= 40)",
            ),
            (
                # THE claim the page suite cannot make: the mark is drawn, beside Reply, on its line -- or, on
                # a collapsed row whose list is too narrow to draw its Reply (`#377 compact-collapsed-rows`),
                # on the header line beside its time.
                "the newest message says Draft directly left of its Reply, on the same line, on screen "
                "(or on a collapsed row's header line where no Reply is drawn)",
                "(() => { const m = document.querySelector('#draft-probe .draft-marker'); "
                "if (!m || m.hidden || !m.getClientRects().length) return false; "
                "const reply = m.nextElementSibling; if (!reply || !reply.classList.contains('reply-button')) return false; "
                "const r = m.getBoundingClientRect(); "
                "const p = reply.getClientRects().length ? reply.getBoundingClientRect() "
                ": document.querySelector('#draft-probe .msg-time').getBoundingClientRect(); "
                "if (!reply.getClientRects().length && document.getElementById('draft-probe')"
                ".getAttribute('data-collapsed') !== 'true') return false; "
                "const area = document.getElementById('scroll-area').getBoundingClientRect(); "
                "return m.getAttribute('aria-label') === 'Draft reply' "
                "&& (reply.getClientRects().length ? r.right <= p.left : r.left >= p.right) "
                "&& Math.abs((r.top + r.bottom) / 2 - (p.top + p.bottom) / 2) < 3 "
                "&& r.top >= area.top && r.bottom <= area.bottom; })()",
            ),
            (
                "no other message is marked",
                "[...document.querySelectorAll('#discord-log .draft-marker')]"
                ".filter((m) => m.getClientRects().length).length === 1",
            ),
            (
                "nothing runs off the side of the screen",
                "document.scrollingElement.scrollWidth <= innerWidth",
            ),
        ),
    ),
    Scene(
        name="50-send-dismissed",
        what="the refused send dismissed: gone from the outbox, and an Undo chip that names it",
        act=_act_send_dismissed,
        profiles=("android-412", "laptop-1280"),
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            ("the outbox is empty", "!window.__visible('outgoing-log')"),
            (
                "the Undo chip is on screen and names the message by its first words",
                "(() => { const c = document.getElementById('undo-send-dismiss'); "
                "const b = c.getBoundingClientRect(); "
                "return window.__visible('undo-send-dismiss') "
                "&& c.textContent === 'Undo dismiss: “the arm64 runner is back;…”' "
                "&& b.top >= 0 && b.bottom <= innerHeight && b.left >= 0 && b.right <= innerWidth; })()",
            ),
            (
                "the strip says it was dismissed and where to get it back, in words a phone shows whole",
                "window.__text('status') === 'Dismissed. Undo it, or find it in Settings.'",
            ),
            (
                "nothing runs off the side of the screen",
                "document.scrollingElement.scrollWidth <= innerWidth",
            ),
        ),
    ),
    Scene(
        name="51-recently-dismissed",
        what="Settings, Recently dismissed: the send just dismissed, where it was going, as a row that opens it",
        act=_act_recently_dismissed,
        profiles=("android-412", "laptop-1280"),
        expect=(
            ("the settings screen is up", "window.__visible('screen-settings')"),
            (
                # `#348 settings-dynamic-lists`: the row is one button that opens the send's pop-out,
                # where Put back is now; no button of its own in the row.
                "the dismissed send is listed with its text and its age, as one row that opens it, on screen",
                "(() => { const item = document.querySelector('#dismissed-sends > li'); "
                "if (!item) return false; const b = item.getBoundingClientRect(); "
                "const open = item.querySelector('.dismissed-send-open'); "
                "return /the arm64 runner is back/.test(item.textContent) "
                "&& /dismissed just now/.test(item.textContent) && open !== null "
                "&& open.getAttribute('aria-haspopup') === 'dialog' "
                "&& item.querySelector('.dismissed-send-restore') === null "
                "&& b.top >= 0 && b.bottom <= innerHeight; })()",
            ),
            ("Forget these now is offered", "window.__visible('forget-dismissed-sends')"),
            (
                "nothing runs off the side of the screen",
                "document.scrollingElement.scrollWidth <= innerWidth",
            ),
        ),
    ),
    Scene(
        name="52-elision-word-count",
        what="folded messages ending with how many words the fold hides: a few under one, a wall under another",
        act=_act_elision_word_count,
        profiles=("android-412", "laptop-1280"),
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "a message barely over the fold and a long one are both on screen, folded",
                "(() => { const ids = ['elision-probe-small', 'elision-probe-large']; "
                "return ids.every((id) => { const r = document.getElementById(id); "
                "if (!r || r.getAttribute('data-collapsed') !== 'true') return false; "
                "const b = r.getBoundingClientRect(); return b.top >= 0 && b.bottom <= innerHeight; }); })()",
            ),
            (
                "each says how many words it hides, in the form '(~N more words)', one label for its width",
                "['elision-probe-small', 'elision-probe-large'].every((id) => "
                "/^\\(~\\d+ more words\\)$/.test(window.__elided(document.getElementById(id))))",
            ),
            (
                # The difference the owner could not see: tens of words under one, hundreds under the
                # other.
                "the counts tell a few hidden words from a wall of them",
                "(() => { const n = (id) => Number(/\\d+/.exec(window.__elided(document.getElementById(id)))[0]); "
                "return n('elision-probe-small') < 60 && n('elision-probe-large') >= 100; })()",
            ),
            (
                # THE layout claim the page suite cannot make: the count is drawn, on one line, under
                # the clamped text and inside its row.
                "the count is one line under the clamped text, inside its row",
                "['elision-probe-small', 'elision-probe-large'].every((id) => { "
                "const row = document.getElementById(id); const label = row.querySelector(':scope > .elided'); "
                "const body = row.querySelector(':scope > .body').getBoundingClientRect(); "
                "const l = label.getBoundingClientRect(), r = row.getBoundingClientRect(); "
                "const line = parseFloat(getComputedStyle(label).lineHeight) || 1.6 * parseFloat("
                "getComputedStyle(label).fontSize); "
                "return l.height > 0 && l.height <= line + 1 && l.top >= body.bottom - 0.5 "
                "&& l.left >= r.left - 0.5 && l.right <= r.right + 0.5 && l.bottom <= r.bottom + 0.5; })",
            ),
        ),
    ),
    Scene(
        name="53-done-feedback",
        what="a message just marked Done: ringed in dashed grey at the read strength, the next unread selected in the solid accent",
        act=_act_done_feedback,
        after=_undo_done_feedback,
        profiles=("android-412", "laptop-1280"),
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the message e marked is Done, and drawn as just marked",
                "(() => { const r = [...document.querySelectorAll('#discord-log > li')].find((n) => "
                "n.getAttribute('data-id') === window.__doneProbe); "
                "return !!r && r.getAttribute('data-archived') === 'true' "
                "&& r.getAttribute('data-done-flash') === 'marking' "
                "&& r.getAttribute('data-selected') !== 'true'; })()",
            ),
            (
                # THE claim only a browser can make: what the two rings are drawn in.
                "it is ringed in dashed grey; the next unread message is selected and ringed solid in the accent",
                "(() => { " + RESOLVE_COLOUR_JS + "; "
                "const r = [...document.querySelectorAll('#discord-log > li')].find((n) => "
                "n.getAttribute('data-id') === window.__doneProbe); "
                "const next = document.querySelector('#discord-log > li[data-selected=\"true\"]'); "
                "if (!next || next === r || next.getAttribute('data-archived') === 'true') return false; "
                "const a = getComputedStyle(r), b = getComputedStyle(next); "
                "return a.outlineStyle === 'dashed' && a.outlineColor === window.__colour('--read-ring') "
                "&& b.outlineStyle === 'solid' && b.outlineColor === window.__colour('--accent'); })()",
            ),
            (
                # From review, the second round: at 412px the name gave way to one letter. On a phone's
                # narrow row the chip says only "Undo", and the whole name is its tooltip and its
                # accessible name; on a desk it names the message.
                "the Undo chip is up: named on the desk, a plain Undo on the phone, the name whole in its tooltip",
                "(() => { const undo = document.getElementById('undo-dismiss'); "
                "const label = document.getElementById('undo-dismiss-label'); "
                "const drawn = label.innerText.replace(/\\s+/g, ' ').trim(); "
                "const whole = label.textContent; "
                "return window.__visible('undo-dismiss') && /^Undo \u201c.+\u201d$/.test(whole) "
                "&& undo.getAttribute('title') === whole && undo.getAttribute('aria-label') === whole "
                "&& (innerWidth < 480 ? drawn === 'Undo' : drawn.startsWith('Undo \u201c ')); })()",
            ),
            (
                # From review: named, the chip wrapped the row of chips at 412px, and in these captures
                # it stood on Send. The chip's words stay inside it, no chip overlaps another or the
                # screen's edge, and no chip and no status pill is over the message box or its Send
                # where they are on the list's screen. tests/done_feedback_browser.py walks the list up
                # from its end at 360, 412 and 1280px.
                "the chips overlap nothing and stay clear of the message box, and the Undo chip's words stay inside it",
                "(() => { const shown = (e) => !!e && e.getClientRects().length > 0; "
                "const chips = [...document.querySelectorAll('#scroll-tools > .chip')]"
                ".filter(shown).map((c) => c.getBoundingClientRect()); "
                "const apart = (c, d) => c.right <= d.left + 0.5 || d.right <= c.left + 0.5 || c.bottom <= d.top + 0.5 "
                "|| d.bottom <= c.top + 0.5; "
                "const list = document.getElementById('scroll-area').getBoundingClientRect(); "
                "const floating = [...chips, ...[document.getElementById('status-line')].filter(shown)"
                ".map((s) => s.getBoundingClientRect())]; "
                "const box = ['channel-send', 'channel-compose-text'].map((id) => document.getElementById(id))"
                ".filter(shown).map((e) => e.getBoundingClientRect()).filter((b) => b.bottom > list.top && b.top < list.bottom); "
                "const label = document.getElementById('undo-dismiss-label'); "
                "return chips.length > 0 && chips.every((c) => c.left >= 0 && c.right <= innerWidth) "
                "&& chips.every((c, i) => chips.slice(i + 1).every((d) => apart(c, d))) "
                "&& floating.every((f) => box.every((b) => apart(f, b))) "
                "&& label.scrollWidth <= label.clientWidth + 1; })()",
            ),
        ),
    ),
    Scene(
        name="54-help-sections",
        what="Help opened from a ? on Settings: that entry open at the top, every other one folded to its heading",
        act=_act_help_sections,
        profiles=("android-412", "laptop-1280"),
        expect=(
            ("the Help screen is up", "window.__visible('screen-help')"),
            (
                "the entry the ? is about is the one open",
                "(() => { const open = [...document.querySelectorAll('#screen-help details.help-entry')]"
                ".filter((d) => d.open).map((d) => d.id); "
                "return open.length === 1 && open[0] === 'help-reading-width'; })()",
            ),
            (
                # At the top where the screen can scroll that far; on a desk the folded list is too
                # short to, so what is claimed is that it is on the screen with its text.
                "it is on the screen, heading and text",
                "(() => { const d = document.getElementById('help-reading-width'); "
                "const screen = document.getElementById('screen-help').getBoundingClientRect(); "
                "const b = d.getBoundingClientRect(), p = d.querySelector('p').getBoundingClientRect(); "
                "return b.top >= screen.top - 1 && p.height > 0 && p.bottom <= screen.bottom + 1; })()",
            ),
            (
                # THE layout claim: a folded entry is its heading alone, a full-size target, with the
                # browser's triangle in front of it. An entry hidden with the feature it explains is
                # not on the screen at all, so it is no heading to measure: `#208 persistent-cache`
                # hides the one about saved messages on a server without a message cache, as here.
                "every folded entry is one heading at least 44px tall, with its disclosure triangle",
                "(() => { const folded = [...document.querySelectorAll('#screen-help details.help-entry')]"
                ".filter((d) => !d.open && !d.hidden); "
                "return folded.length >= 14 && folded.every((d) => { const s = d.querySelector(':scope > summary'); "
                "const sb = s.getBoundingClientRect(), db = d.getBoundingClientRect(); "
                "return sb.height >= 43.5 && db.height <= sb.height + 2 "
                "&& getComputedStyle(s).display === 'list-item'; }); })()",
            ),
            (
                "the button at the top offers to open them all",
                "document.getElementById('fold-help').textContent.trim() === 'Expand all'",
            ),
            (
                # This throwaway server has no [message_cache] table, so Help has no entry about
                # one: the page describes nothing the server lacks. `#208 persistent-cache`.
                "no entry about saved messages on a server with no message cache",
                "!window.__visible('help-message-cache')",
            ),
        ),
    ),
    Scene(
        name="55-thread-card-header",
        what="two columns, a thread opened from its card: the same card at the top of the right column, joined to its root on Main",
        act=_act_thread_card_header,
        after=_unroute_threaded,
        profiles=DESKTOP_PROFILES,
        expect=(
            ("two columns are up, a thread on the right", "window.__visible('side-pane') && window.__visible('scroll-area')"),
            (
                "the thread is under its card, which says what its card in the list said, its X on it",
                "(() => { const card = document.getElementById('side-thread-card'); "
                "const words = card.querySelector('.side-card-text'); "
                "return window.__visible('side-thread-card') && !!words && words.textContent === window.__cardText "
                "&& card.contains(document.getElementById('side-close')) && window.__visible('side-close') "
                "&& !window.__visible('side-title'); })()",
            ),
            (
                # THE layout claim: the card stands in the heading, above the list, at the list's cards' place.
                "the card is above the list, not in it, its edge in its thread's colour",
                "(() => { const c = document.getElementById('side-thread-card').getBoundingClientRect(); "
                "const l = document.getElementById('side-scroll').getBoundingClientRect(); "
                "const e = (getComputedStyle(document.getElementById('side-thread-card')).borderLeftColor"
                ".match(/[0-9.]+/g) || []).slice(0, 3).map(Number); "
                "return c.height > 60 && c.bottom <= l.top + 0.5 && Math.max(...e) - Math.min(...e) >= 90; })()",
            ),
            (
                "one line joins the root to the card, the selection's, its arrowhead on the card's left edge",
                "(() => { const svg = document.getElementById('column-links'); "
                "const paths = [...svg.querySelectorAll('path.column-link')]; "
                "if (paths.length !== 1 || !paths[0].hasAttribute('data-selected')) return false; "
                "const marks = paths[0].nextElementSibling.getAttribute('d').match(/-?\\d+(?:\\.\\d+)?/g).map(Number); "
                "const tip = marks[marks.length - 6] + svg.getBoundingClientRect().left; "
                "return Math.abs(tip - document.getElementById('side-thread-card').getBoundingClientRect().left) <= 1; })()",
            ),
            (
                "the thread, nothing in it unread, is at its foot",
                "(() => { const a = document.getElementById('side-scroll'); "
                "return a.scrollHeight - a.clientHeight - a.scrollTop <= 2; })()",
            ),
        ),
    ),
    Scene(
        name="56-thread-card-header-list",
        what="the thread list, a thread opened from its card: the same card at the top of the screen, over the thread",
        act=_act_thread_card_header_list,
        after=_unroute_threaded,
        profiles=("android-412", "laptop-1280"),
        expect=(
            (
                "the thread is across the window under its card, which says what its card in the list said, its X on it",
                "(() => { const card = document.getElementById('thread-card'); "
                "const words = card.querySelector('.side-card-text'); "
                "return window.__visible('thread-card') && !!words && words.textContent === window.__cardText "
                "&& card.contains(document.getElementById('thread-back')) && window.__visible('thread-back') "
                "&& !document.getElementById('screen-main').hasAttribute('data-columns'); })()",
            ),
            (
                # THE layout claim: the card heads the screen, outside the list, so it never scrolls away.
                "the card is above the list, not in it, its edge in its thread's colour, and no line is drawn",
                "(() => { const c = document.getElementById('thread-card').getBoundingClientRect(); "
                "const l = document.getElementById('scroll-area').getBoundingClientRect(); "
                "const e = (getComputedStyle(document.getElementById('thread-card')).borderLeftColor"
                ".match(/[0-9.]+/g) || []).slice(0, 3).map(Number); "
                "return c.height > 60 && c.bottom <= l.top + 0.5 && Math.max(...e) - Math.min(...e) >= 90 "
                "&& document.querySelectorAll('#column-links path').length === 0; })()",
            ),
            (
                "the X is a thumb's size under a finger, and nothing scrolls sideways",
                "(() => { const b = document.getElementById('thread-back').getBoundingClientRect(); "
                "const coarse = matchMedia('(pointer: coarse)').matches; "
                "return (!coarse || (b.width >= 44 && b.height >= 44)) "
                "&& document.documentElement.scrollWidth <= window.innerWidth; })()",
            ),
        ),
    ),
    Scene(
        name="58-all-thread-strip",
        what="All: every message of an answered thread has a strip in its thread's colour on its left edge",
        act=_act_all_thread_strip,
        after=_unroute_threaded,
        profiles=("android-412", "laptop-1280"),
        expect=(
            (
                "every row whose tag is a thread's coloured Thread(N) has a strip, and no other row has one",
                "(() => { const rows = [...document.querySelectorAll('#discord-log > li[data-id]')]; "
                "const strip = (li) => [...li.children].find((kid) => kid.classList.contains('thread-strip') && !kid.hidden); "
                "const open = (li) => !!li.querySelector('.thread-badge[data-thread-state=\"open\"]'); "
                "return rows.some(open) && rows.every((li) => Boolean(strip(li)) === open(li)); })()",
            ),
            (
                # THE look: a bar thicker than a border, in the thread's colour, on a read reply too.
                "each strip's bar is at least 5px wide and its thread's colour, a hit area at least 16px wide and the row's height",
                "(() => { const strips = [...document.querySelectorAll('#discord-log > li > .thread-strip:not([hidden])')]; "
                "return strips.length > 0 && strips.every((s) => { const bar = getComputedStyle(s, '::before'); "
                "const c = (bar.backgroundColor.match(/[0-9.]+/g) || []).slice(0, 3).map(Number); "
                "const r = s.getBoundingClientRect(), row = s.parentElement.getBoundingClientRect(); "
                "return parseFloat(bar.width) >= 5 && Math.max(...c) - Math.min(...c) >= 90 "
                "&& r.width >= 16 && r.height >= row.height; }); })()",
            ),
            (
                "nothing scrolls sideways",
                "document.documentElement.scrollWidth <= window.innerWidth",
            ),
        ),
    ),
    Scene(
        name="59-all-thread-selected",
        what="All with the summarised thread selected by its strip: every other message dimmed and drained",
        act=_act_all_thread_selected,
        after=_unroute_threaded,
        profiles=("android-412", "laptop-1280"),
        expect=(
            (
                "still All, the thread selected, and nothing scrolled",
                "document.getElementById('thread-select').value === 'flat' "
                "&& document.getElementById('scroll-area').scrollTop === window.__beforeTop",
            ),
            (
                # THE claim: the thread's own rows at full strength, every other row at about 0.3.
                "the thread's rows are drawn at full strength and every other row at about 0.3",
                "(() => { const drawn = (li) => { const after = getComputedStyle(li, '::after'); "
                "const v = String(after.backgroundColor).match(/[0-9.]+/g) || []; "
                "const alpha = after.content === 'none' ? 0 : v.length >= 4 ? Number(v[3]) : v.length === 3 ? 1 : 0; "
                "return parseFloat(getComputedStyle(li).opacity) * (1 - alpha); }; "
                "const rows = [...document.querySelectorAll('#discord-log > li[data-id]')]"
                ".filter((li) => li.getClientRects().length > 0 && li.getAttribute('data-read-run') !== 'head'); "
                "const mine = rows.filter((li) => li.hasAttribute('data-thread-in')); "
                "return mine.length > 0 && mine.length < rows.length "
                "&& rows.every((li) => Math.abs(drawn(li) - (li.hasAttribute('data-thread-in') ? 1 : 0.3)) < 0.02); })()",
            ),
        ),
    ),
    Scene(
        name="60-settings-voice-agent",
        what="Settings, Voice agent: Resuming, Arriving messages during a call and Reading full messages in channel view together, and the link to how messages are pre-processed for speech",
        act=_act_settings_voice_agent,
        profiles=SETTINGS_PROFILES,
        expect=(
            ("the settings screen is up", "window.__visible('screen-settings')"),
            (
                # THE arrangement: the three, one after another, inside Voice agent.
                "Voice agent holds Resuming, Arriving messages during a call and Reading full messages in channel view, in a row",
                "(() => { const heads = [...document.querySelectorAll('#settings-voice-agent .settings-group "
                ".group-head > h3')].map((h) => h.textContent.trim()); "
                "const at = heads.indexOf('Resuming'); "
                "return at >= 0 && heads[at + 1] === 'Arriving messages during a call' "
                "&& heads[at + 2] === 'Reading full messages in channel view'; })()",
            ),
            (
                "the three headings and their controls are on the screen at once",
                "['resume-toggle', 'relay-to-agent', 'settings-audio-source', 'help-link-resuming', "
                "'help-link-live-messages', 'help-link-speech-prep'].every((id) => { "
                "const b = document.getElementById(id).getBoundingClientRect(); "
                "const s = document.getElementById('screen-settings').getBoundingClientRect(); "
                "return window.__visible(id) && b.top >= s.top - 1 && b.bottom <= s.bottom + 1; })",
            ),
            (
                # Gone from Settings by name, and its line is Help's now.
                "no Message audio, Live messages or How messages are said on Settings; the server's arm is in Help",
                "![...document.querySelectorAll('#screen-settings h3')]"
                ".some((h) => /^(Message audio|Live messages|How messages are said)$/.test(h.textContent.trim())) "
                "&& document.getElementById('speech-prep-state').closest('#help-speech-prep') !== null",
            ),
            (
                # `#347 settings-prose`: the title, and only its drop-down. Nothing else drawn in the
                # group: no label, no state line, no line under the picker.
                "Arriving messages during a call is its title and only its drop-down",
                "(() => { const group = document.getElementById('relay-to-agent').closest('.settings-group'); "
                "const drawn = [...group.querySelectorAll('p, label, select, input')].filter((e) => e.getClientRects().length > 0); "
                "return drawn.length === 1 && drawn[0].id === 'relay-to-agent' "
                "&& !group.contains(document.getElementById('live-state')); })()",
            ),
            (
                # ...and How messages are said is a short link under the reading picker, reading as one.
                "the link under the reading picker says How messages are pre-processed for speech, underlined",
                "(() => { const link = document.getElementById('help-link-speech-prep'); "
                "const pick = document.getElementById('settings-audio-source').getBoundingClientRect(); "
                "const r = link.getBoundingClientRect(); "
                "return link.textContent.trim() === 'How messages are pre-processed for speech' "
                "&& r.top >= pick.bottom - 1 && getComputedStyle(link).textDecorationLine.includes('underline') "
                "&& r.height >= 30; })()",
            ),
            (
                # `#332`'s other half: a state line is a sentence, not a paragraph, on a phone too.
                "the resuming state line is at most two lines tall",
                "['resume-state'].every((id) => { const e = document.getElementById(id); "
                "const line = parseFloat(getComputedStyle(e).lineHeight) || parseFloat(getComputedStyle(e).fontSize) * 1.5; "
                "const h = e.getBoundingClientRect().height; return h > 0 && h <= line * 2 + 1; })",
            ),
            (
                "nothing on the settings screen is wider than it",
                "(() => { const s = document.getElementById('screen-settings'); "
                "return s.scrollWidth <= s.clientWidth + 1; })()",
            ),
        ),
    ),
    Scene(
        name="61-resume-unavailable",
        what="Settings on a server whose voice bridge takes no recap: Resuming's switch disabled, its reason under it",
        act=_act_resume_unavailable,
        after=_after_resume_unavailable,
        profiles=("iphone15", "laptop-1280"),
        expect=(
            ("the settings screen is up", "window.__visible('screen-settings')"),
            (
                # THE claim: not a live control that does nothing.
                "the switch is disabled and unticked",
                "(() => { const t = document.getElementById('resume-toggle'); "
                "return t.disabled === true && t.checked === false; })()",
            ),
            (
                "the reason is on screen and says the voice service takes no recap",
                "window.__visible('resume-unavailable') "
                "&& /does not take a recap/.test(window.__text('resume-unavailable') || '')",
            ),
            (
                # Beside it: directly under the switch's words, indented to them, not a paragraph away.
                "the reason stands right under the switch's words",
                # Its WORDS start past the box, where the switch's own words do: the padding is
                # inside the element's rectangle, so it is added rather than read off the rectangle.
                "(() => { const label = document.getElementById('resume-toggle').closest('label')"
                ".getBoundingClientRect(); const box = document.getElementById('resume-toggle')"
                ".getBoundingClientRect(); const node = document.getElementById('resume-unavailable'); "
                "const why = node.getBoundingClientRect(); const pad = parseFloat(getComputedStyle(node).paddingLeft); "
                "return why.top >= label.bottom - 8 && why.top - label.bottom <= 12 "
                "&& why.left + pad >= box.right - 1 && why.left + pad <= box.right + 16; })()",
            ),
            (
                "the switch and its reason are on the screen at once",
                "['resume-toggle', 'resume-unavailable'].every((id) => { "
                "const b = document.getElementById(id).getBoundingClientRect(); "
                "const s = document.getElementById('screen-settings').getBoundingClientRect(); "
                "return b.top >= s.top - 1 && b.bottom <= s.bottom + 1; })",
            ),
        ),
    ),
    Scene(
        name="62-call-caught-up",
        what="a call on the websocket backend that the bridge caught up: the line under it says so, in words",
        act=_act_call_caught_up,
        after=_after_call_caught_up,
        profiles=("iphone15", "laptop-1280"),
        expect=(
            ("the call is live", "window.__visible('hang-up')"),
            (
                # THE claim, in the server's own count of what the record held.
                "the line under the call says the agent was caught up, with how many words",
                "window.__visible('call-recap') "
                "&& /^Caught the agent up with [\\d,]+ words? from earlier$/.test(window.__text('call-recap') || '') "
                "&& document.getElementById('call-recap').getAttribute('data-recap') === 'accepted'",
            ),
            (
                "the bridge was sent the record once, and before anything else",
                "window.__socket && window.__socket.v1Replays === 1 && window.__socket.v1First === 'replay'",
            ),
            (
                # Under the turns, at the head of the call's box: nothing between them and a card.
                "the line sits under the newest turn of the call, inside the call's box",
                "(() => { const rows = [...document.querySelectorAll('#transcript > li')]; "
                "const last = rows[rows.length - 1]; const line = document.getElementById('call-recap')"
                ".getBoundingClientRect(); return !!last && line.top >= last.getBoundingClientRect().bottom - 1 "
                "&& document.getElementById('call-recap').parentElement.id === 'voice-composer'; })()",
            ),
            (
                "and it is drawn in a colour of its own, not the muted one of a line that started fresh",
                "getComputedStyle(document.getElementById('call-recap')).color "
                "!== getComputedStyle(document.getElementById('resume-state')).color",
            ),
            (
                "the line above the call keeps the count",
                "[...document.querySelectorAll('#transcript .seam-recap')]"
                ".some((n) => /^[\\d,]+-word recap sent$/.test(n.textContent || ''))",
            ),
        ),
    ),
    Scene(
        name="63-settings-prose",
        what="Settings, Messages: Mark-read automatically and its neighbours, each one-line description now its heading's and its ?'s tooltip",
        act=_act_settings_prose,
        profiles=SETTINGS_PROFILES,
        expect=(
            ("the settings screen is up", "window.__visible('screen-settings')"),
            (
                # The owner's name for the list, and the old one gone.
                "Messages holds Your own messages, Mark-read automatically, Empty messages, Combining messages and Rich headers",
                "(() => { const heads = [...document.querySelectorAll('#settings-messages .settings-group .group-head > h3')]"
                ".map((h) => h.textContent.trim()); "
                "return ['Your own messages', 'Mark-read automatically', 'Empty messages', 'Combining messages', 'Rich headers']"
                ".every((h, i) => heads[i] === h) && !heads.includes('Read automatically'); })()",
            ),
            (
                "the Messages title and its first groups are on the screen",
                "['settings-messages-title', 'help-link-mark-own', 'mark-own-read', 'help-link-auto-read', "
                "'help-link-empty-messages', 'hide-empty-messages', 'help-link-combining', 'combine-messages'].every((id) => { "
                "const b = document.getElementById(id).getBoundingClientRect(); "
                "const s = document.getElementById('screen-settings').getBoundingClientRect(); "
                "return window.__visible(id) && b.top >= s.top - 1 && b.bottom <= s.bottom + 1; })",
            ),
            (
                # THE claim: no line of description is drawn. What is drawn in the section is a state
                # line or a quote of the server's, and every one of those has an id.
                "no line of description is drawn under a heading in Messages",
                "[...document.querySelectorAll('#settings-messages p.hint')]"
                ".filter((p) => p.getClientRects().length > 0).every((p) => p.id !== '')",
            ),
            (
                # ...because it is the tooltip of the heading and of its ?, the same words on both.
                "every ? on Settings carries its heading's one-line description as its tooltip",
                "[...document.querySelectorAll('#screen-settings .group-head')].every((head) => { "
                "const h = head.querySelector('h2, h3'); const q = head.querySelector('.help-link'); "
                "return !q || (h.title.length > 10 && h.title === q.title); })",
            ),
            (
                "nothing on the settings screen is wider than it",
                "(() => { const s = document.getElementById('screen-settings'); "
                "return s.scrollWidth <= s.clientWidth + 1; })()",
            ),
        ),
    ),
    Scene(
        name="64-channel-settings-all",
        what="Settings, Channels on All channels: Rename and Remove aside, Clear unread Mixed across two channels, and Who is who asking for one channel",
        act=_act_channel_settings_all,
        after=_after_channel_settings_all,
        profiles=SETTINGS_PROFILES,
        expect=(
            ("the settings screen is up", "window.__visible('screen-settings')"),
            (
                "the picker opens with All channels, and it is chosen",
                "(() => { const s = document.getElementById('settings-channel'); "
                "return s.options[0].value === '*' && s.options[0].textContent === 'All channels' && s.value === '*'; })()",
            ),
            (
                # THE claim: two channels set differently read as Mixed, said in words beside it.
                "Clear unread shows Mixed, and the line says how they differ",
                "(() => { const m = document.getElementById('upstream-clear-mode'); "
                "return m.value === 'mixed' && m.selectedOptions[0].textContent === 'Mixed' "
                "&& document.getElementById('upstream-clear-state').textContent === 'Mixed: On in 1, Off in 1.' "
                "&& document.getElementById('upstream-clear-save').disabled; })()",
            ),
            (
                "Rename and Remove step aside; Add a channel stays; the line says how many",
                "!window.__visible('rename-channel') && !window.__visible('remove-channel') "
                "&& window.__visible('open-add-channel') "
                "&& document.getElementById('channel-facts').textContent === 'All 2 channels on your list.'",
            ),
            (
                "the picker, the count and Clear unread are on the screen at once",
                "['settings-channel', 'channel-facts', 'upstream-clear-mode', 'upstream-clear-state'].every((id) => { "
                "const b = document.getElementById(id).getBoundingClientRect(); "
                "const s = document.getElementById('screen-settings').getBoundingClientRect(); "
                "return window.__visible(id) && b.top >= s.top - 1 && b.bottom <= s.bottom + 1; })",
            ),
            (
                # `#402 who-is-who-channel-box`: who is who is back in the box, and on All channels it asks
                # for one channel rather than listing anyone.
                "Who is who is in the channel box, asking for one channel, and not in Messages",
                "document.getElementById('channel-settings').contains(document.getElementById('identity-list')) "
                "&& !document.getElementById('settings-messages').contains(document.getElementById('identity-list')) "
                "&& document.getElementById('identity-list').textContent === 'Pick one channel to see who is who.'",
            ),
            (
                "nothing on the settings screen is wider than it",
                "(() => { const s = document.getElementById('screen-settings'); "
                "return s.scrollWidth <= s.clientWidth + 1; })()",
            ),
        ),
    ),
    Scene(
        name="65-settings-dismissed-list",
        what="Settings, Recently dismissed as a dark data panel: fixed height, resizable, scrolling, one line a row",
        act=_act_settings_dismissed_list,
        profiles=SETTINGS_PROFILES,
        expect=(
            ("the settings screen is up", "window.__visible('screen-settings')"),
            (
                # THE claim: drawn as data, a terminal's dark and monospaced panel, not a list of cards.
                "the list is a dark, monospaced panel",
                "(() => { const s = getComputedStyle(document.getElementById('dismissed-sends')); "
                "const rgb = (s.backgroundColor.match(/[0-9.]+/g) || []).slice(0, 3).map(Number); "
                "return rgb.length === 3 && Math.max(...rgb) < 48 && /mono/i.test(s.fontFamily); })()",
            ),
            (
                "the list is of a fixed height, can be dragged taller, and scrolls inside itself",
                "(() => { const l = document.getElementById('dismissed-sends'); const s = getComputedStyle(l); "
                "const rem = parseFloat(getComputedStyle(document.documentElement).fontSize); "
                "return Math.abs(l.getBoundingClientRect().height - 11 * rem) <= 2 && s.resize === 'vertical' "
                "&& s.overflowY === 'auto' && l.scrollHeight > l.clientHeight; })()",
            ),
            (
                # ...with shorter previews: each row's text one line, cut with an ellipsis.
                "each row's text is one line",
                "[...document.querySelectorAll('#dismissed-sends .dismissed-send-text')].every((e) => { "
                "const line = parseFloat(getComputedStyle(e).lineHeight) || parseFloat(getComputedStyle(e).fontSize) * 1.5; "
                "return e.getBoundingClientRect().height <= line + 1 && getComputedStyle(e).textOverflow === 'ellipsis'; })",
            ),
            (
                "no row carries a button of its own besides the row",
                "[...document.querySelectorAll('#dismissed-sends > li')].every((li) => li.querySelectorAll('button').length === 1)",
            ),
            (
                "the list is on the screen",
                "(() => { const b = document.getElementById('dismissed-sends').getBoundingClientRect(); "
                "const s = document.getElementById('screen-settings').getBoundingClientRect(); "
                "return b.top >= s.top - 1 && b.bottom <= s.bottom + 1; })()",
            ),
            (
                "nothing on the settings screen is wider than it",
                "(() => { const s = document.getElementById('screen-settings'); "
                "return s.scrollWidth <= s.clientWidth + 1; })()",
            ),
        ),
    ),
    Scene(
        name="66-settings-dismissed-popout",
        what="Settings, a dismissed send opened in its pop-out: the whole text, Copy, Put back and Close",
        act=_act_settings_dismissed_popout,
        profiles=SETTINGS_PROFILES,
        expect=(
            ("the pop-out is up", "window.__visible('dismissed-send-popout')"),
            (
                # THE claim: the whole text, which the row cut to one line.
                "the pop-out holds the whole long message",
                "(() => { const t = document.getElementById('dismissed-send-popout-text').textContent; "
                "return t.startsWith('Long note for later') && t.length > 400; })()",
            ),
            (
                "Copy, Put back and Close are on screen, inside the window",
                "['dismissed-send-copy', 'dismissed-send-restore', 'dismissed-send-close'].every((id) => { "
                "const b = document.getElementById(id).getBoundingClientRect(); "
                "return window.__visible(id) && b.top >= 0 && b.bottom <= innerHeight && b.left >= 0 && b.right <= innerWidth; })",
            ),
            (
                "the pop-out is inside the window, and Copy has the focus",
                "(() => { const b = document.getElementById('dismissed-send-popout').getBoundingClientRect(); "
                "return b.top >= 0 && b.bottom <= innerHeight && b.left >= 0 && b.right <= innerWidth "
                "&& document.activeElement === document.getElementById('dismissed-send-copy'); })()",
            ),
            (
                # `#351 settings-polish`: a modal, said so, with the page behind it inert.
                "the pop-out is a modal, and the page behind it is inert while it is up",
                "(() => { const p = document.getElementById('dismissed-send-popout'); "
                "return p.getAttribute('aria-modal') === 'true' && p.closest('[inert]') === null "
                "&& ['close-settings', 'fold-dismissed', 'dismissed-sends', 'fold-settings-all', 'dock']"
                ".every((id) => document.getElementById(id).closest('[inert]') !== null); })()",
            ),
            (
                "the Tab key goes round the pop-out's own four controls, both ways",
                "JSON.stringify(window.__popoutTabbed) === JSON.stringify(['dismissed-send-restore', "
                "'dismissed-send-close', 'dismissed-send-popout-text', 'dismissed-send-copy', "
                "'dismissed-send-popout-text', 'dismissed-send-close', 'dismissed-send-restore', 'dismissed-send-copy'])",
            ),
            (
                # A hit test, as a tap is: an inert control is skipped, so this says what a tap there
                # lands on, and nothing about how the control looks. The check after it does that.
                "no tap on the dim reaches a control behind it",
                "Array.isArray(window.__dimHits) && window.__dimHits.length === 0",
            ),
            (
                # `#351 settings-polish`, from its review: the top bar's Back was inert and drawn bright,
                # above the shadow, so a tap on it did nothing while it looked live. Measured in pixels.
                "every control on screen outside the pop-out, the top bar's Back and the pill's x among them, is under the dim, once",
                "Array.isArray(window.__dimJudged) && ['close-settings', 'dismiss-status'].every((id) => "
                "window.__dimJudged.includes(id)) && Array.isArray(window.__dimMisses) && window.__dimMisses.length === 0",
            ),
            (
                # `#351 settings-polish`, from the review of its second round: with the shadow kept off the
                # pill, the box and the pop-out in it were drawn under the pill, which covered Copy, Put
                # back and Close wherever the pop-out stood at the foot. Measured in pixels.
                "at the foot of the screen, with the pill up under it, nothing is drawn over the pop-out",
                "Array.isArray(window.__footCovered) && window.__footCovered.length === 0",
            ),
        ),
    ),
    Scene(
        name="67-settings-connection-panel",
        what="Settings, Connection status after a call: the dark readout, one fact to a line, and how live updates arrive",
        act=_act_settings_connection_panel,
        profiles=SETTINGS_PROFILES,
        expect=(
            ("the settings screen is up", "window.__visible('screen-settings')"),
            (
                "the readout is a dark, monospaced panel",
                "(() => { const s = getComputedStyle(document.getElementById('connection-panel')); "
                "const rgb = (s.backgroundColor.match(/[0-9.]+/g) || []).slice(0, 3).map(Number); "
                "return rgb.length === 3 && Math.max(...rgb) < 48 && /mono/i.test(s.fontFamily); })()",
            ),
            (
                # ...one fact to a line: the call's detail drawn on several lines, the live line after it.
                "the call's detail is several lines, and the live-updates line is under it, in the panel",
                "(() => { const d = document.getElementById('settings-detail'); const l = document.getElementById('live-state'); "
                "const line = parseFloat(getComputedStyle(d).lineHeight) || parseFloat(getComputedStyle(d).fontSize) * 1.5; "
                "return d.textContent.split('\\n').length >= 3 && d.getBoundingClientRect().height >= line * 3 - 1 "
                "&& window.__visible('live-state') && l.getBoundingClientRect().top >= d.getBoundingClientRect().bottom - 1 "
                "&& document.getElementById('connection-panel').contains(l); })()",
            ),
            (
                "the panel is on the screen",
                "(() => { const b = document.getElementById('connection-panel').getBoundingClientRect(); "
                "const s = document.getElementById('screen-settings').getBoundingClientRect(); "
                "return b.top >= s.top - 1 && b.bottom <= s.bottom + 1; })()",
            ),
            (
                "nothing on the settings screen is wider than it",
                "(() => { const s = document.getElementById('screen-settings'); "
                "return s.scrollWidth <= s.clientWidth + 1; })()",
            ),
        ),
    ),
    Scene(
        name="68-settings-folded",
        what="Settings as it opens: the server first, then every section folded to its title, and Expand all",
        act=_act_settings_folded,
        profiles=SETTINGS_PROFILES,
        expect=(
            ("the settings screen is up", "window.__visible('screen-settings')"),
            (
                # THE claim: collapsed by default, every section, its title a button that says so.
                "every section but Server is folded to its title, and says so",
                "(() => { const toggles = [...document.querySelectorAll('#screen-settings .settings-section > h2 > .fold-toggle, "
                "#screen-settings .settings-section > .group-head > h2 > .fold-toggle')]; "
                "return toggles.length === 5 && toggles.every((b) => b.getAttribute('aria-expanded') === 'false' "
                "&& document.getElementById(b.getAttribute('aria-controls')).getAttribute('hidden') === 'until-found' "
                "&& window.__visible(b.id)); })()",
            ),
            (
                # `checkVisibility`: a control inside a body hidden until found still answers for its
                # geometry when asked, though nothing of it is drawn; the bodies themselves are flat.
                "no setting is drawn: the folded bodies take no room",
                "['resume-toggle', 'mark-own-read', 'settings-channel', 'msg-scale', 'forget-token']"
                ".every((id) => !document.getElementById(id).checkVisibility()) "
                "&& [...document.querySelectorAll('#screen-settings .fold-body')].every((b) => b.getBoundingClientRect().height === 0)",
            ),
            (
                "the server's address is still the first thing, and Expand all is offered",
                "window.__visible('settings-server-host') && window.__visible('fold-settings-all') "
                "&& document.getElementById('fold-settings-all').textContent.trim() === 'Expand all'",
            ),
            (
                # `#372 settings-header-layout`: quiet, one line, whole in its tooltip.
                "the server's host is one line, no larger than the page's text, in ordinary weight, whole in its tooltip",
                "(() => { const h = document.getElementById('settings-server-host'); const cs = getComputedStyle(h); "
                "return parseFloat(cs.fontSize) <= parseFloat(getComputedStyle(document.body).fontSize) "
                "&& Number(cs.fontWeight) <= 400 && cs.whiteSpace === 'nowrap' "
                "&& getComputedStyle(h.querySelector('.settings-server-name')).textOverflow === 'ellipsis' "
                "&& h.getBoundingClientRect().height < 2 * parseFloat(cs.fontSize) "
                "&& h.title === h.textContent && h.textContent === location.host; })()",
            ),
            (
                # `#372 settings-header-layout`: neither button has a row of its own.
                "Expand all and Open help share a row above the sections: the host's on a desk, the one under it on a phone",
                "(() => { const r = (id) => document.getElementById(id).getBoundingClientRect(); "
                "const mid = (b) => (b.top + b.bottom) / 2; "
                "const host = r('settings-server-host'), all = r('fold-settings-all'), help = r('open-help'), panes = r('settings-panes'); "
                "const desk = matchMedia('(min-width: 900px) and (pointer: fine)').matches; "
                "return Math.abs(mid(all) - mid(help)) < 2 && all.right <= help.left && help.bottom <= panes.top "
                "&& (desk ? Math.abs(mid(host) - mid(all)) < 4 && host.right <= all.left : host.bottom <= all.top + 1); })()",
            ),
            (
                "each folded title is a tap target at least 40px tall, with its chevron",
                "[...document.querySelectorAll('#screen-settings .fold-toggle')].filter((b) => window.__visible(b.id))"
                ".every((b) => b.getBoundingClientRect().height >= 40 && getComputedStyle(b, '::before').content !== 'none')",
            ),
            (
                "nothing on the settings screen is wider than it",
                "(() => { const s = document.getElementById('screen-settings'); "
                "return s.scrollWidth <= s.clientWidth + 1; })()",
            ),
        ),
    ),
    Scene(
        name="69-settings-find-opens",
        what="Settings after a search for 'Noise suppression': the folded Microphone group and its section opened by the browser",
        act=_act_settings_find_opens,
        profiles=SETTINGS_PROFILES,
        expect=(
            ("the settings screen is still up", "window.__visible('screen-settings')"),
            (
                # THE claim: find-in-page still finds hidden text, and opens what it is in.
                "the browser opened the Microphone group and the Voice agent section, and both buttons say so",
                "['fold-microphone', 'fold-settings-voice-agent'].every((id) => { const b = document.getElementById(id); "
                "return b.getAttribute('aria-expanded') === 'true' "
                "&& !document.getElementById(b.getAttribute('aria-controls')).hasAttribute('hidden'); })",
            ),
            (
                "the switch the search was for is drawn, and nothing else was opened",
                "window.__visible('mic-noise-suppression') && document.getElementById('mic-noise-suppression').checkVisibility() "
                "&& document.getElementById('fold-resuming').getAttribute('aria-expanded') === 'false' "
                "&& document.getElementById('fold-settings-messages').getAttribute('aria-expanded') === 'false'",
            ),
        ),
    ),
    Scene(
        name="70-settings-columns",
        what="Settings with every section open: tiled into columns by the window's width on a desk, each whole and packed, in order; one column on the phone",
        act=_act_settings_columns,
        profiles=SETTINGS_PROFILES,
        expect=(
            ("the settings screen is up, everything open", "window.__visible('screen-settings') && window.__text('fold-settings-all') === 'Fold all'"),
            (
                # THE claim: width-dependent columns on a desk, one on a phone.
                "on a desk the sections take two or more columns, no more than the width has room for; on a phone, one",
                "(() => { const rem = parseFloat(getComputedStyle(document.documentElement).fontSize); "
                "const desk = matchMedia('(min-width: 900px) and (pointer: fine)').matches; "
                "const secs = [...document.querySelectorAll('#settings-panes > .settings-section')]; "
                "const lefts = [...new Set(secs.map((s) => Math.round(s.getBoundingClientRect().left)))]; "
                "const panes = document.getElementById('settings-panes'); "
                "const room = Math.max(1, Math.floor((panes.clientWidth + rem) / (24 * rem + rem))); "
                "return desk ? lefts.length >= 2 && lefts.length <= room && room >= (innerWidth >= 1800 ? 4 : innerWidth >= 1200 ? 3 : 2) "
                ": lefts.length === 1; })()",
            ),
            (
                # ...each pane kept whole: a section broken across two columns is two boxes.
                "every section is one box, never broken across two columns",
                "[...document.querySelectorAll('#settings-panes > .settings-section')].every((s) => s.getClientRects().length === 1)",
            ),
            (
                # ...packed: in a column, each section starts where the one above it ends, its margin apart.
                "in each column the sections stack with no gap but their margin",
                "(() => { const rem = parseFloat(getComputedStyle(document.documentElement).fontSize); "
                "const boxes = [...document.querySelectorAll('#settings-panes > .settings-section')].map((s) => s.getBoundingClientRect()); "
                "return boxes.every((b, i) => boxes.slice(i + 1).filter((c) => Math.abs(c.left - b.left) < 2 && c.top > b.top)"
                ".every((c, j, below) => j > 0 || c.top - b.bottom <= rem + 2)); })()",
            ),
            (
                # ...order unchanged: down a column and on into the next is the markup's order.
                "read down each column and on to the next, the sections are in their order",
                "(() => { const secs = [...document.querySelectorAll('#settings-panes > .settings-section')]; "
                "const placed = secs.slice().sort((a, b) => { const p = a.getBoundingClientRect(), q = b.getBoundingClientRect(); "
                "return Math.abs(p.left - q.left) > 2 ? p.left - q.left : p.top - q.top; }); "
                "return placed.every((s, i) => s === secs[i]); })()",
            ),
            (
                "nothing on the settings screen is wider than it",
                "(() => { const s = document.getElementById('screen-settings'); "
                "return s.scrollWidth <= s.clientWidth + 1; })()",
            ),
        ),
    ),
    Scene(
        name="71-settings-columns-folded",
        what="Settings folded: the five titles tiled into columns on a desk, and Tab going through them in their order",
        act=_act_settings_columns_folded,
        profiles=SETTINGS_PROFILES,
        expect=(
            ("the settings screen is up", "window.__visible('screen-settings')"),
            (
                # THE other half: the focus order is the markup's, whatever the columns did with the boxes.
                "Tab went through the five section titles in their order, Channels' ? after its title",
                "JSON.stringify(window.__tabbed) === JSON.stringify(['fold-settings-voice-agent', 'fold-settings-messages', "
                "'fold-settings-channels', 'help-link-channel-alias', 'fold-settings-screen', 'fold-settings-storage'])",
            ),
            (
                "on a desk the titles take two or more columns; on a phone, one",
                "(() => { const rem = parseFloat(getComputedStyle(document.documentElement).fontSize); "
                "const desk = matchMedia('(min-width: 900px) and (pointer: fine)').matches; "
                "const secs = [...document.querySelectorAll('#settings-panes > .settings-section')]; "
                "const lefts = [...new Set(secs.map((s) => Math.round(s.getBoundingClientRect().left)))]; "
                "const panes = document.getElementById('settings-panes'); "
                "const room = Math.max(1, Math.floor((panes.clientWidth + rem) / (24 * rem + rem))); "
                "return desk ? lefts.length >= 2 && lefts.length <= room : lefts.length === 1; })()",
            ),
            (
                "read down each column and on to the next, the titles are in their order",
                "(() => { const secs = [...document.querySelectorAll('#settings-panes > .settings-section')]; "
                "const placed = secs.slice().sort((a, b) => { const p = a.getBoundingClientRect(), q = b.getBoundingClientRect(); "
                "return Math.abs(p.left - q.left) > 2 ? p.left - q.left : p.top - q.top; }); "
                "return placed.every((s, i) => s === secs[i]); })()",
            ),
        ),
    ),
    Scene(
        name="72-settings-services",
        what="Settings, Channels with two chat services: the Service row, Slack's facts, only Slack's channels, Slack's Add and Browse",
        act=_act_settings_services,
        after=_after_settings_services,
        profiles=SETTINGS_PROFILES,
        expect=(
            ("the settings screen is up", "window.__visible('screen-settings')"),
            (
                # THE claim: the service is chosen first, in place of the one-service line.
                "the Service row is shown in configuration order, on Slack, and the Chat service line is not",
                "(() => { const s = document.getElementById('settings-service'); "
                "return window.__visible('settings-service-row') && !window.__visible('chat-provider-line') "
                "&& [...s.options].map((o) => o.textContent).join('|') === 'Google Chat|Slack' && s.value === 'slack'; })()",
            ),
            (
                "the facts line says what Slack does here",
                "document.getElementById('service-facts').textContent === 'Slack: New messages are checked for every "
                "30 seconds. Threads are shown. Add a channel by pasting its link, or browse its channels. Unread can "
                "be cleared in Slack itself.'",
            ),
            (
                "the channel picker lists only Slack's channels, under All Slack channels",
                "(() => { const s = document.getElementById('settings-channel'); "
                "return [...s.options].map((o) => o.textContent).join('|') === 'All Slack channels|team|release'; })()",
            ),
            (
                "Slack's Add a channel and Browse channels are offered",
                "window.__visible('open-add-channel') && window.__visible('open-browse-channels')",
            ),
            (
                "the bar's channel picker holds a group per service, in configuration order",
                "[...document.querySelectorAll('#discord-channel > optgroup')].map((g) => g.label).join('|') "
                "=== 'Google Chat|Slack'",
            ),
            (
                "the Service row, the facts and the channel picker are on the screen at once",
                "['settings-service', 'service-facts', 'settings-channel'].every((id) => { "
                "const b = document.getElementById(id).getBoundingClientRect(); "
                "const s = document.getElementById('screen-settings').getBoundingClientRect(); "
                "return window.__visible(id) && b.top >= s.top - 1 && b.bottom <= s.bottom + 1; })",
            ),
            (
                "nothing on the settings screen is wider than it",
                "(() => { const s = document.getElementById('screen-settings'); "
                "return s.scrollWidth <= s.clientWidth + 1; })()",
            ),
        ),
    ),
    # LAST IN THE WALK, and not by accident: it empties the server's durable store on the way in,
    # so that the summary the state above cached is really regenerated and can really fail. Nothing
    # after it in this profile would miss what it deletes, and the next profile purges anyway.
    Scene(
        name=SUMMARY_FAILED_STATE,
        what="summary mode when the summariser fails: the row says so quietly and keeps the message",
        act=_act_summary_failed,
        after=_summaries_off_after,
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the control says the mode is on",
                "document.getElementById('summarise').getAttribute('aria-pressed') === 'true'",
            ),
            (
                "the row that was asked about is marked as failed",
                "document.getElementById('summary-failed-probe')"
                ".getAttribute('data-summary-failed') === 'true'",
            ),
            (
                # `#564 message-summary-toggle`. The owner, 2026-10-26: "The summary failed is very
                # invasive ... I can no longer see if it's archived or mark it as read." A rendering
                # fact, so checked here: the failed row has no bar down its edge (its left side is
                # its right side's width), and its label is the muted aside, not red capitals.
                "the failed row is drawn as its own row, with only a quiet label",
                "(() => { const f = document.getElementById('summary-failed-probe'); "
                "const a = getComputedStyle(f); "
                "const m = getComputedStyle(f.querySelector('.summary-mark')); "
                "return Math.abs(parseFloat(a.borderLeftWidth) - parseFloat(a.borderRightWidth)) < 0.5 && "
                "m.textTransform === 'none' && parseInt(m.fontWeight, 10) < 600; })()",
            ),
            (
                # A colour is not reachable by a screen reader, is not reachable by a reader who
                # cannot separate this red from the surface behind it, and does not survive being
                # printed. The words are the statement; the tint is only the glance.
                "it says so IN WORDS, not only in colour",
                "/summary failed/i.test(document.getElementById('summary-failed-probe')"
                ".querySelector('.summary-mark').textContent || '')",
            ),
            (
                # The one thing the reader still has. Replacing a message with an apology is
                # strictly worse than the row they would have had with the mode off, and the whole
                # design of the failed row turns on not doing it.
                "the message itself is still on screen, not replaced by the failure",
                "(() => { const row = document.getElementById('summary-failed-probe'); "
                "return row.querySelector('.body').offsetParent !== null; })()",
            ),
            (
                # The owner asked for a pop-up as well as a colour. It counts, and it is transient
                # on purpose — the standing sentence for a permanent fact lives in #summary-note.
                "the strip says it once, in a sentence that counts",
                "/could not be summarised/.test(window.__text('status-line') || '') && "
                "window.__visible('status-line')",
            ),
            (
                # The negative control that stops this state being satisfied by a page that shouts
                # about everything: the sticky full-width panel is for a channel the reader cannot
                # reach, and one unsummarised row out of fifty is not that.
                "the sticky error panel is NOT raised for a summary failure",
                "!window.__visible('error')",
            ),
        ),
    ),
)


def expect_viewport_meta(html: str) -> list[str]:
    """The one safe-area fact this harness CAN check: the declaration is still in the markup.

    Chromium reports zero safe-area insets under automation and offers no control to change that,
    so the notch padding cannot be verified visually here. Losing the declaration silently would
    still be a real regression, and this is what is left that catches it.
    """
    problems = []
    if "viewport-fit=cover" not in html:
        problems.append(
            "the served page's markup no longer declares viewport-fit=cover, so the page will letterbox "
            "inside the safe area on a phone with a cutout"
        )
    if "width=device-width" not in html:
        problems.append("the served page's markup no longer declares width=device-width")
    return problems


# =================================================================================================
# The run.
# =================================================================================================


def check_server(url: str) -> None:
    try:
        with urlopen(f"{url.rstrip('/')}/healthz", timeout=10) as response:
            if response.status != 200:
                raise ServerUnreachable(
                    f"{url}/healthz answered HTTP {response.status}, not 200"
                )
    except URLError as error:
        raise ServerUnreachable(
            f"nothing answered at {url}/healthz ({error}). Start one with: scripts/run.sh --screenshots"
        ) from error


def ensure_playwright() -> None:
    """Check the package AND the browser binary before anything else in the run.

    Checked FIRST, ahead of the server, and that ordering is the fix for a control that was
    vacuous when it was written: a run pointed at a dead server reported the dead server and never
    reached the browser at all, so the missing-browser message was never exercised by the test that
    claimed to exercise it. A missing browser is also the failure a fresh machine hits, and it is
    the one worth reporting before the reader goes looking for a server problem they do not have.
    """
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise PlaywrightMissing(
            "The python3 'playwright' package is not installed.", INSTALL_HINT
        ) from error
    with sync_playwright() as playwright:
        # `executable_path` reports where the browser WOULD be; it does not check that it is there.
        path = playwright.chromium.executable_path
        if not path or not os.path.exists(path):
            raise PlaywrightMissing(
                f"Playwright is installed but its Chromium browser is not at {path}.",
                BROWSER_INSTALL_HINT,
            )


def open_browser(playwright: Playwright) -> Browser:
    try:
        return playwright.chromium.launch(
            args=[
                # A real capture graph with no hardware and no permission prompt. This is what
                # keeps the live-call states honest: getUserMedia, the AudioContext and the
                # ScriptProcessor all run for real.
                "--use-fake-ui-for-media-stream",
                "--use-fake-device-for-media-stream",
                "--autoplay-policy=no-user-gesture-required",
            ]
        )
    except Exception as error:  # playwright raises its own Error type; the message is the signal
        text = str(error)
        if "Executable doesn't exist" in text or "playwright install" in text:
            raise PlaywrightMissing(
                "Playwright is installed but its Chromium browser is not.", BROWSER_INSTALL_HINT
            ) from error
        raise


def run_captures(
    url: str,
    token: str,
    channel: str,
    out_dir: Path,
    only: Iterable[str],
    themes: Iterable[Theme],
    mock_control: str = "",
) -> list[tuple[str, Path, Verdict]]:
    try:
        from playwright.sync_api import sync_playwright
    except ImportError as error:
        raise PlaywrightMissing(
            "The python3 'playwright' package is not installed.", INSTALL_HINT
        ) from error

    wanted = set(only)
    scenes = [scene for scene in SCENES if not wanted or scene.name in wanted]
    if wanted:
        unknown = wanted - {scene.name for scene in SCENES}
        if unknown:
            raise SystemExit(
                f"unknown state name(s): {', '.join(sorted(unknown))}\n"
                f"known states: {', '.join(scene.name for scene in SCENES)}"
            )

    signed = {
        "signed_url": "wss://screenshots.invalid/v1/convai/conversation?token=stub",
        "agent_id": "agent_screenshot_stub",
        "valid_for_seconds": 900,
    }

    def purge_stored_conversations() -> None:
        """Empty the server's transcript store before a profile's walk starts.

        `#48 transcript-storage` made the server keep what these scenes say, and the profiles
        share one server. Without this the SECOND phone opens on a screen restored from the
        FIRST one's invented conversation, and `02-idle` -- whose entire content is the empty
        state -- becomes unreachable for a reason that reads like a page bug.

        A server with no storage configured answers 503, which is not a failure here: there is
        nothing to purge. Anything else is, because the alternative is a confusing failure three
        scenes later.
        """
        request = Request(f"{url.rstrip('/')}/api/v1/conversations", method="DELETE")
        request.add_header("Authorization", f"Bearer {token}")
        try:
            with urlopen(request, timeout=10):  # noqa: S310 - a loopback URL this script built
                return
        except HTTPError as error:
            if error.code == 503:
                return
            raise RuntimeError(
                f"could not empty the transcript store before the walk: HTTP {error.code}. "
                "Every scene after 02-idle would be photographed on top of the previous "
                "profile's conversation."
            ) from error
        except URLError as error:
            raise RuntimeError(
                f"could not reach {url} to empty the transcript store: {error}"
            ) from error

    everything: list[tuple[str, Path, Verdict]] = []
    # Two processes judge the captures while the walk goes on; see `start_capture`. Spawned rather
    # than forked: this process is about to hold a browser driver's threads and pipes.
    judges = ProcessPoolExecutor(max_workers=2, mp_context=multiprocessing.get_context("spawn"))
    try:
        with sync_playwright() as playwright:
            browser = open_browser(playwright)
            try:
                for theme in themes:
                    for profile in PROFILES:
                        context = browser.new_context(**profile.context_options(theme))
                        context.grant_permissions(["microphone"], origin=url)
                        # The vendor boundary, and the ONLY request that is faked. Everything else
                        # -- /api/v1/client-config, the channel read -- goes to the real server.
                        context.route(
                            "**/api/v1/signed-url",
                            lambda route: route.fulfill(
                                status=200,
                                content_type="application/json",
                                body=json.dumps(signed),
                            ),
                        )
                        page = context.new_page()
                        driver = Driver(
                            page, url, token, out_dir, profile, theme, mock_control, judges
                        )
                        print(f"==> {theme} · {profile.name}: {profile.what}")
                        purge_stored_conversations()
                        try:
                            walk_profile(driver, profile, scenes)
                        except Exception:
                            # A frame taken before the walk failed, and judged blank since, is the
                            # walk's first failure and is reported as one; the frames that passed
                            # are reported as they were before.
                            for name, _path, verdict in driver.judged(wait=True):
                                print(f"  ok    {name:<30} {verdict.describe()}")
                            raise
                        for name, _path, verdict in driver.judged(wait=True):
                            print(f"  ok    {name:<30} {verdict.describe()}")
                        everything.extend(driver.results)
                        context.close()
            finally:
                browser.close()
    finally:
        judges.shutdown(wait=True, cancel_futures=True)
    return everything


def walk_profile(driver: Driver, profile: Profile, scenes: list[Scene]) -> None:
    """Walk one profile through every scene it has, capturing each; see `run_captures`."""
    for scene in scenes:
        # A state that does not exist on this device is SKIPPED by name rather than attempted: the
        # desktop reading column has nothing to photograph on a phone, and a scene that quietly
        # succeeded there would be filed as evidence of a layout the phone does not have.
        if (scene.profiles and profile.name not in scene.profiles) or (
            profile.opt_in and not scene.profiles
        ):
            print(f"  skip  {scene.name:<30} not a state this profile has")
            continue
        # A timeout inside the walk is an UNREACHABLE STATE and must read as one. Left to itself
        # Playwright raises a TimeoutError whose traceback names a line number in this file and not
        # the state, which is exactly the report that sends the next reader to debug the harness
        # when the page has moved.
        try:
            scene.act(driver)
        except Exception as error:
            if isinstance(error, (Unreachable, TrivialCapture)):
                raise
            raise Unreachable(
                scene.name,
                f"the walk into it did not complete ({type(error).__name__})",
                str(error).strip().splitlines()[0] if str(error).strip() else "",
            ) from error
        driver.capture(scene)
        if scene.after is not None:
            scene.after(driver)
        for name, _path, verdict in driver.judged(wait=False):
            print(f"  ok    {name:<30} {verdict.describe()}")


# =================================================================================================
# The controls. Offline, no browser, no server, and each one guards a specific check above.
# =================================================================================================


def _flat_png(width: int, height: int, colour: tuple[int, int, int]) -> bytes:
    return encode_png(width, height, [[colour] * width for _ in range(height)])


def _busy_png(width: int, height: int) -> bytes:
    rows = []
    for y in range(height):
        rows.append([((x * 7 + y * 3) % 256, (x * 3) % 256, (y * 5) % 256) for x in range(width)])
    return encode_png(width, height, rows)


def check_image_judgement_controls() -> list[str]:
    problems: list[str] = []

    busy = _busy_png(300, 300)
    verdict = judge_capture(busy)
    if not verdict.ok:
        problems.append(
            "positive control: a varied, plainly-rendered frame was REJECTED "
            f"({verdict.reason}). Every real capture would now fail."
        )
    if verdict.width != 300 or verdict.height != 300:
        problems.append(
            "the decoder read the wrong dimensions from a 300x300 PNG "
            f"({verdict.width}x{verdict.height}) — the row filters are being misapplied."
        )

    for name, colour in (("white", (255, 255, 255)), ("black", (0, 0, 0)), ("grey", (34, 34, 34))):
        verdict = judge_capture(_flat_png(400, 400, colour))
        if verdict.ok:
            problems.append(
                f"negative control: a completely flat {name} frame was ACCEPTED as a rendered "
                "page. A screenshot of a page that never painted would be filed as evidence."
            )

    # A page that painted one thing and nothing else: technically many colours, visually blank.
    rows = [[(17, 17, 17)] * 600 for _ in range(600)]
    for y in range(6):
        for x in range(6):
            rows[y][x] = (x * 40, y * 40, 200)
    nearly_blank = encode_png(600, 600, rows)
    verdict = judge_capture(nearly_blank)
    if verdict.ok:
        problems.append(
            "negative control: a frame that is 99.99% one colour with a speck in the corner was "
            f"ACCEPTED (dominant {verdict.dominant_share:.4%}). MAX_DOMINANT_SHARE is not biting."
        )

    verdict = judge_capture(b"")
    if verdict.ok:
        problems.append("negative control: an EMPTY file was accepted as a screenshot.")
    verdict = judge_capture(b"\x89PNG\r\n\x1a\n" + b"\x00" * 40)
    if verdict.ok:
        problems.append("negative control: a truncated PNG was accepted as a screenshot.")
    verdict = judge_capture(_busy_png(8, 8))
    if verdict.ok:
        problems.append(
            "negative control: an 8x8 frame was accepted. A capture that small is not a page."
        )
    # 60x60 of noise is well over the byte floor, so this control lands on MIN_EDGE and nothing
    # else. Without it MIN_EDGE was dead code that every control passed over: the 8x8 frame above
    # is rejected for being a few hundred bytes long, whatever the dimension rule says. (Found by
    # mutation: disabling the dimension check entirely left every control green.)
    small_but_heavy = _busy_png(60, 60)
    if len(small_but_heavy) <= MIN_BYTES:
        problems.append(
            "the small-frame control is vacuous: its own image is under the byte floor, so it "
            "cannot be testing the dimension rule."
        )
    verdict = judge_capture(small_but_heavy)
    if verdict.ok:
        problems.append(
            "negative control: a 60x60 frame was accepted. It is far too small to be a page, and "
            "it is big enough to clear the byte floor, so MIN_EDGE is what has to reject it."
        )
    verdict = judge_capture(b"this is not a png at all, but it is long enough" * 40)
    if verdict.ok:
        problems.append("negative control: a non-PNG file was accepted as a screenshot.")

    return problems


def check_decoder_controls() -> list[str]:
    """The decoder must handle all five row filters, because browsers emit all five."""
    problems: list[str] = []
    width, height = 40, 25  # 25 rows -> encode_png cycles filters 0..4 five times over
    rows = [
        [((x * 11 + y * 17) % 256, (x * 5) % 256, (y * 9) % 256) for x in range(width)]
        for y in range(height)
    ]
    data = encode_png(width, height, rows)
    try:
        image = decode_png(data)
    except BadPng as error:
        return [f"positive control: a valid PNG using all five row filters failed to decode: {error}"]
    if image.width != width or image.height != height:
        problems.append(
            f"the decoder read {image.width}x{image.height} from a {width}x{height} PNG."
        )
    flat = [pixel for row in rows for pixel in row]
    if image.pixels != flat:
        first = next(
            (i for i, (a, b) in enumerate(zip(image.pixels, flat)) if a != b), len(image.pixels)
        )
        problems.append(
            "positive control: the decoder reconstructed the WRONG pixels from a PNG using all "
            f"five row filters (first mismatch at pixel {first}, row {first // width}, which is "
            f"filter type {(first // width) % 5}). Every judgement below is then about noise."
        )
    return problems


def check_state_controls() -> list[str]:
    """The scene table itself: an unreached state must fail by name, and none may be unguarded."""
    problems: list[str] = []

    if len(SCENES) < 34:
        problems.append(
            f"the scene table has only {len(SCENES)} states. The interface rework added three that "
            "are where the interesting defects now live -- the armed clear control, the seam with "
            "its disclosure open, and settings with its title bar -- `#47 scrollback-stability` "
            "added two more that are pure rendering facts (a folded list with one message opened, "
            "and the chip that appears when a turn arrives off screen), and `#55 "
            "voice-desktop-app` added the reading column at each end of its range, which is the "
            "only place the desktop composition can be judged at all. `#54 resume-recovery` added "
            "the last two: a suspended call and a failed one, which are the same close code and "
            "must not look the same -- and which nothing here photographed before, because no "
            "state above shows the error panel at all. `#51 reply-view` added the reply "
            "screen and the same screen with a target longer than the frame, and `#56 "
            "message-hover-highlight` added the row picked out under a pointer -- the only place "
            "that treatment can be looked at at all -- and `#65 scrollback-paging` added a step "
            "further back through the channel, which is where the anchored prepend is judged. "
            "`#48 transcript-storage` added the one scene that RELOADS the page, and `#43 "
            "typed-input` added the composer open during a live call -- the only place the "
            "question 'does a fourth dock band fit on a 375x667 phone without eating the "
            "transcript or overlapping the controls' can be answered at all. `#58 control-bar` "
            "added the bar in BOTH of its homes, because 'directly above the big buttons' is a "
            "measurement and nothing without a layout engine has an opinion about it, and `#59 "
            "text-entry-button` added the bar converted into a text field (which `#241 "
            "voice-composer` replaced with the call's view and its message box focused, the switch "
            "still on the bar), and `#60 "
            "canned-prompt-buttons` added the bar with every button it has on it at once -- "
            "whether a box and its switch, or five controls, fit on a 375px phone "
            "is the same kind of question, and no fixture can answer it. `#44 live-push` added "
            "the one where a row appears because the SERVER said so rather than because the page "
            "asked, and `#46 conversation-replay` the one where a resumed call admits its "
            "reconstruction was only partial. `#83 channel-selector-in-bar` added the channel "
            "picker on the bar with the history walked back several pages, because 'this control "
            "cannot be reached' is a measurement on a real 375px screen -- whether the picker is "
            "inside the viewport, whole, at a scroll position the reader can be in -- and nothing "
            "without a layout engine can answer it. `#68 pull-to-refresh` added the armed pull, "
            "which is the one state in this whole table built from real Touch and TouchEvent "
            "objects: the page fixture hands the page whatever a test invents, so only a browser "
            "can say the handlers read what a browser really sends. `#49 cached-summaries` added "
            "the channel with its collapsed rows summarised, which is the only place the claim "
            "that a summarised row is SHORTER than a clamped one can be settled -- nothing "
            "without a layout engine can compare two heights. `#50 todo-view` added the last "
            "two: the channel filtered to what has not been dealt with -- where the only "
            "question that matters is whether the statement about whose read state this is fits "
            "on a phone above the list -- and the bulk clear with one tap on it, which is the "
            "frame that shows it saying how many it is about to take BEFORE it takes them."
        )
    names = [scene.name for scene in SCENES]
    if len(set(names)) != len(names):
        problems.append("two scenes share a name, so one would overwrite the other's capture.")
    # States that exist ONLY where `@media (min-width: 900px) and (pointer: fine)` is in force.
    # Named here rather than inferred, because losing the restriction is silent: the scene would
    # then run on both iPhone profiles and photograph the phone composition -- which has no reading
    # column and no handle -- under a name that says desktop. That is worse than a failure.
    restricted = {
        "14-desktop-narrow-column": DESKTOP_PROFILES,
        "15-desktop-wide-column": DESKTOP_PROFILES,
        # `#51 reply-view`. Phones only: at desktop width a message short enough for Discord to
        # accept does not reach the foot of the screen, so the state would photograph a box that
        # happens to fit and file it as evidence that a long one leaves the box reachable.
        "19-reply-view-long-target": PHONE_PROFILES,
        # `#56 message-hover-highlight`. Chromium's page.hover() in a touch context can produce a
        # hover state, so without this the scene would run on both iPhone profiles and CERTIFY a
        # sticky hover on the device the rule exists to protect from one.
        "20-discord-hover": DESKTOP_PROFILES,
        # `#68 pull-to-refresh`. The mirror image: `has_touch` is set per profile, so on a desktop
        # context this scene would photograph an interaction that device cannot have -- and the
        # gesture is the whole content of the picture.
        "31-pull-to-refresh-armed": PHONE_PROFILES,
        # `#218 desktop-dock`. Desks only, for the reason the column scenes are: on a phone these
        # measurements would certify a desktop dock on the device that must keep its thumb's one.
        "39-desktop-dock-channel": DESKTOP_PROFILES,
        "40-desktop-dock-call": DESKTOP_PROFILES,
        # `#298 thread-card-header`. Two columns exist only at a desk: on a phone the scene would find
        # no layout button, or worse, photograph one column under a name that says two.
        "55-thread-card-header": DESKTOP_PROFILES,
        # ...and the thread list's thread across the window, at a desk and on the phone's width, where
        # the X must be a thumb's size; not every phone, whose pictures would show the same column.
        "56-thread-card-header-list": ("android-412", "laptop-1280"),
        # `#299 all-thread-strip`. All at a desk and at a phone's width, where the strip is a finger's.
        "58-all-thread-strip": ("android-412", "laptop-1280"),
        "59-all-thread-selected": ("android-412", "laptop-1280"),
        # `#332 settings-voice-agent-section`. The phone the owner uses and the small desk: the two
        # sizes the issue asks the section to be seen at; `#347 settings-prose` adds the full-HD desk.
        "60-settings-voice-agent": SETTINGS_PROFILES,
        # `#331 resume-on-websocket`. The same two sizes, for the disabled switch and the call's line.
        "61-resume-unavailable": ("iphone15", "laptop-1280"),
        "62-call-caught-up": ("iphone15", "laptop-1280"),
        # `#347 settings-prose`. The owner's Settings batch is seen at the phone and both desks.
        "63-settings-prose": SETTINGS_PROFILES,
        # `#349 channel-settings-all`.
        "64-channel-settings-all": SETTINGS_PROFILES,
        # `#348 settings-dynamic-lists`.
        "65-settings-dismissed-list": SETTINGS_PROFILES,
        "66-settings-dismissed-popout": SETTINGS_PROFILES,
        "67-settings-connection-panel": SETTINGS_PROFILES,
        # `#346 settings-collapsible`.
        "68-settings-folded": SETTINGS_PROFILES,
        "69-settings-find-opens": SETTINGS_PROFILES,
        # `#345 settings-multi-column`.
        "70-settings-columns": SETTINGS_PROFILES,
        "71-settings-columns-folded": SETTINGS_PROFILES,
        # `#355 provider-grouped-channels`.
        "72-settings-services": SETTINGS_PROFILES,
    }
    for name, wanted_profiles in restricted.items():
        scene = next((s for s in SCENES if s.name == name), None)
        if scene is None:
            problems.append(f"the '{name}' state is gone from the scene table.")
        elif scene.profiles != wanted_profiles:
            problems.append(
                f"scene '{name}' is no longer restricted to {wanted_profiles}, so it would be "
                "captured on a device where the state it is named for does not exist."
            )

    known_profiles = {profile.name for profile in PROFILES}
    for scene in SCENES:
        # A restriction naming a profile that does not exist restricts the scene to NOTHING, and
        # a scene that never runs anywhere reports no failure at all -- it just quietly stops
        # being evidence. Renaming a profile is exactly how that happens.
        for wanted in scene.profiles or ():
            if wanted not in known_profiles:
                problems.append(
                    f"scene '{scene.name}' is restricted to profile {wanted!r}, which is not in "
                    "PROFILES, so the state is never captured anywhere."
                )
        if not scene.expect:
            problems.append(
                f"scene '{scene.name}' declares NO expectation, so a navigation failure would "
                "yield a confidently-named picture of whatever was on screen before it."
            )
        for description, expression in scene.expect:
            if not description.strip() or not expression.strip():
                problems.append(f"scene '{scene.name}' has an empty expectation.")

    # Every state, not a sample of them. Pinning four left the other four able to lose the one
    # expectation that distinguishes them and keep only generic ones -- and a scene whose checks are
    # all generic is satisfied by whatever screen happens to be up, which is the exact failure this
    # whole mechanism exists to prevent. (Found by mutation: deleting the settings screen's own
    # check left every control green.)
    required: dict[str, str | tuple[str, ...]] = {
        "01-signed-out": "save-token",
        "02-idle": ("empty-state", "!window.__visible('status-line')"),
        "03-live-call": "Listening",
        "04-muted": "Muted",
        "05-speaker-silenced": "Silent",
        "06-post-call": "new conversation",
        # Pinned to the GEOMETRY, not merely to the element. Pinning it to "seam-detail" was
        # satisfied by the sibling "has it been laid out" check, so the one assertion this state
        # exists for -- that the explanation does not open underneath the dock, which is the bug
        # its first capture caught -- could be deleted with every control still green.
        "07-seam-disclosure-open": "dock.top",
        "08-clear-armed": ("Sure?", "window.__visible('status-line')"),
        "09-settings": "topbar-title",
        "10-discord-view": ("scrollWidth", "channel-summary"),
        "11-long-transcript-scrolled": "scrollTop",
        # Both pinned to the thing the picture is FOR, not to the screen being up. A line clamp is
        # a rendering fact no behavioural test can reach, so "one is open among the closed ones" is
        # the whole content of state 12; "a turn arrived and the view did not move" is the whole
        # content of state 13.
        "12-collapsed-long-transcript": "data-collapsed",
        "13-jump-to-newest": ("jump-newest", "!window.__visible('collapse-all')"),
        # `#55 voice-desktop-app`. Both pinned to a MEASUREMENT rather than to an element being on
        # screen: the whole content of state 14 is that the column is narrower than the window it
        # sits in, and the whole content of state 15 is that the control really moved it. A scene
        # pinned to "#pane-voice exists" would photograph the stretched phone layout this issue
        # was opened about and file it under the desktop name.
        "14-desktop-narrow-column": "__columnWidth",
        "15-desktop-wide-column": "__wideColumn",
        # `#54 resume-recovery`. The pair. 16 is pinned to the ABSENCE of the banner, because a
        # picture of a Resume button beside a red panel would be the defect itself; 17 is pinned to
        # its presence, because a fix that simply stopped reporting failures would satisfy 16.
        "16-suspended-recoverable": "!window.__visible('error')",
        "17-connection-failed": "window.__visible('error')",
        # `#51 reply-view`, `#410 inline-reply-editor`. 18 is pinned to the ROUND TRIP -- opening a
        # reply in the list and putting it down leaves the reader on the same line, a layout fact no
        # unit test can reproduce. 19 is pinned to the target being a long one, the box under it.
        "18-reply-view": "__returnedTo",
        "19-reply-view-long-target": "t.height > window.innerHeight / 3",
        # `#56 message-hover-highlight`. Pinned to the pixel-affecting property. Pinning it to the
        # row being hovered would be satisfied with the rule deleted, which is the whole failure
        # this state exists to catch -- there is nothing else in the repository that can see it.
        "20-discord-hover": "backgroundColor",
        # `#65 scrollback-paging`. Pinned to the anchor, not to the list being longer: a page that
        # loaded older messages and threw the reader to the top would satisfy "more arrived", and
        # that is the defect, not the feature.
        "21-channel-older-loaded": "__anchorAfter",
        # `#48 transcript-storage`. Pinned to the SEAM's own wording, not to "there are lines on
        # screen": every other scene in this walk also has lines on screen, so only the label
        # distinguishes a restored conversation from the live one it was captured after.
        "22-restored-conversation": "earlier conversation",
        # `#43 typed-input`. Two needles, because the two facts are independent and only one of
        # them is reachable from anywhere else. `li.mine` says the typed turn really landed in the
        # transcript; `control-pane` is the GEOMETRY check -- the composer above the big controls
        # rather than over them -- and it is the only thing in the repository that can see it.
        "23-typed-turn": ("li.mine", "control-pane"),
        # `#58 control-bar`. Both placements are pinned to the GEOMETRY rather than to the bar
        # merely existing -- `scroll-area` in each is the edge the bar has to be on the right side
        # of, and `topbar` is the row that is supposed to disappear underneath it.
        "24-control-bar-top": ("scroll-area", "topbar"),
        "25-control-bar-bottom": ("control-pane", "topbar"),
        # `#241 voice-composer`. `view-switch` is the owner's report -- the switch must stay on the
        # bar while the box is focused -- and `send-text` is the geometry: the box at the foot of the
        # transcript, inside the list rather than under the dock.
        "26-voice-composer": ("view-switch", "send-text"),
        # `#60 canned-prompt-buttons`. `prompts-tray` is the tray really being open, `control-bar`
        # is the geometry -- nothing clipped past the edge, still one strip rather than two rows,
        # and the panel above it rather than in it.
        "27-canned-prompts": ("prompts-tray", "control-bar"),
        # `#44 live-push`. Pinned to the ABSENCE of a refresh as well as to the row, because "the
        # list has one more message in it" is satisfied by the 45-second background re-read this
        # feature exists to replace.
        "28-live-message-arrived": ("__refreshes === 0", "9990000000000000001"),
        # `#46 conversation-replay`. Pinned to the PARTIAL wording, not to the pane being idle:
        # every post-call state is idle, and only this sentence distinguishes a reconstruction
        # that lost its beginning from one that did not.
        "29-resume-partial": ("replayed in part", "IN PART"),
        # `#83 channel-selector-in-bar`. Pinned to the GEOMETRY and to the walk, not to the picker
        # existing: it existed before this issue too, on a row at the top of the scrollback, and a
        # scene pinned to `window.__visible('discord-channel')` would have been green throughout
        # the whole time the owner could not reach it.
        "30-channel-picker-in-bar": ("window.innerWidth", "__pickerMoved", "__pickerInScroll"),
        # `#68 pull-to-refresh`. Pinned to the armed STATE and to its wording, not to the strip
        # being on screen: a page that showed the same affordance whatever the finger had done
        # would satisfy "it is visible", and what the reader has to be told is what letting go
        # will do.
        # ...and to the CASE: `__pullRowsAfter` is what says the automatic step back stood aside
        # under the finger rather than the channel having been walked to its beginning first, which
        # is how this scene was reachable at all while the gesture was unreachable in life.
        "31-pull-to-refresh-armed": ("'armed'", "Release", "dock.top", "__pullRowsAfter"),
        # `#49 cached-summaries`. Pinned to the MEASUREMENT and to the attribution, not to a
        # summary line existing: a line stacked above the clamped message satisfies "a summary is
        # on screen" and is the exact wrong implementation, and a page that named a summariser
        # from a constant of its own would satisfy any check that only looked for words.
        "32-channel-summarised": ("__summarisedHeight", "Summaries by"),
        # `#50 todo-view`. 33 is pinned BOTH to a message really leaving the list and to the
        # read-state statement being on screen: the second is the whole of `#61 unread-status`,
        # and a scene pinned only to the filter being on would be satisfied with it deleted. 34 is
        # pinned to the NUMBER, because "the control is armed" is satisfied by a control that
        # relabels itself and still does not say how much it is about to take.
        "33-todo-view": ("__dealtWith", "inbox-note"),
        "34-bankruptcy-armed": ("__backlog", "armed"),
        # `#129 message-search`. Pinned to THREE things, because each is separately deletable and
        # each on its own is satisfied by a search that does not work:
        #   `search-hidden`   — a filtered row is really gone from the layout. A state pinned to
        #     the count alone would be green with the stylesheet rule deleted, which is a page
        #     that reports "1 of 12" above twelve visible rows.
        #   `mac runner`      — the QUOTES did the narrowing. Pinning it to the list being shorter
        #     would be satisfied by reading the phrase as two loose words, which is the exact
        #     defect the double-quote clause of the request exists to rule out.
        #   `getBoundingClientRect` — field, count and glass fit one row on a 375px phone. That is
        #     the only part of this feature no page suite can reach.
        "35-message-search": ("search-hidden", "mac runner", "getBoundingClientRect"),
        # `#211 link-filter`. Pinned to THREE things, each separately deletable and each satisfied
        # on its own by a filter that does not work:
        #   `data-links-view` — every row on screen is showing its links and not its text. Pinned
        #     to the toggle alone it would be green with the stylesheet rule deleted, which is a
        #     list that says "links only" over the messages' whole text.
        #   `the diff`        — a Markdown link shows its NAME, the owner's second request.
        #   `scrollWidth`     — a long address is cut short inside the screen rather than run
        #     off it, which no page suite can measure.
        "37-links-filter": ("data-links-view", "the diff", "scrollWidth", "link-kinds"),
        # `#212 link-filter`. Pinned to the button reading OFF and drawn so (`line-through`), to
        # the pull request being drawn nowhere while the row keeps its other links (`/pull/`) —
        # either alone is satisfied by a button that changes nothing — and to the choice being kept.
        "38-link-kinds-off": ("line-through", "/pull/", "hidden-link-kinds"),
        # `#49 cached-summaries`, the other half. Pinned to THREE independent things, because each
        # is separately deletable and each on its own is satisfiable by a defect:
        #   `textTransform`   — the label is really drawn quietly (`#564 message-summary-toggle`:
        #     the owner found the red row invasive). Pinning this state to `data-summary-failed`
        #     alone would leave it green with the label shouting again.
        #   `summary failed`  — it says so in words. A colour is not readable by a screen reader.
        #   `.body`           — the message is still there. A row that swallowed the message would
        #     be worse than no summary mode at all.
        SUMMARY_FAILED_STATE: ("textTransform", "summary failed", ".body"),
        # `#218 desktop-dock`. Pinned to the two halves of the owner's complaint, each separately
        # deletable: the SIZE of the dock and its controls (`height <= 100`, `r.height <= 40.5`),
        # and the picker as wide as its name (`field-sizing`) — plus, for each view, the pane in
        # the order the phone shows it, which a desk row laid out in markup order could lose, and the
        # switch in its one corner (`paddingBottom`), which a row that carried it along could lose.
        "39-desktop-dock-channel": ("height <= 100", "r.height <= 40.5", "field-sizing",
                                    "todo-filter,read-speed,read-aloud", "paddingBottom"),
        "40-desktop-dock-call": ("height <= 100", "r.height <= 40.5", "speaker,clear-view,hang-up,talk",
                                 "paddingBottom"),
        # `#298 thread-card-header`. Pinned to the card SAYING its card's words (`__cardText`), to its
        # place above the list (`l.top`), and to the one selected line ending on its edge (`tip`): a
        # heading that named the thread in its own words, a card inside the scroller, or no line, each
        # satisfies "a thread is open on the right".
        "55-thread-card-header": ("__cardText", "l.top", "tip"),
        # Pinned to the card saying its list card's words across the window (`__cardText`), above the
        # list (`l.top`), with no line drawn (`column-links`) and a thumb's X (`44`).
        "56-thread-card-header-list": ("__cardText", "l.top", "column-links", "44"),
        # Pinned to a strip on exactly the coloured threads' rows, its bar's width and colour (`::before`),
        # and, selected, the drawn strength of every row (`data-thread-in`, 0.3) with nothing scrolled.
        "58-all-thread-strip": ("thread-strip", "::before", "data-thread-state"),
        "59-all-thread-selected": ("data-thread-in", "0.3", "__beforeTop"),
        # Pinned to the three in a row in Voice agent (`heads[at + 2]`), to the old names being gone
        # with the server's arm in Help (`help-speech-prep`), to Arriving messages during a call
        # drawing only its drop-down (`drawn.length === 1`, `#347 settings-prose`), to the link under
        # the reading picker (`pre-processed for speech`) and to the short lines (`line * 2`).
        "60-settings-voice-agent": ("heads[at + 2]", "help-speech-prep", "drawn.length === 1",
                                    "pre-processed for speech", "line * 2"),
        # `#331 resume-on-websocket`. Pinned to the switch being disabled (`t.disabled === true`) with
        # its reason under it (`why.top`), and to the call's line saying it was caught up
        # (`Caught the agent up`) after the record went first and once (`v1First`).
        "61-resume-unavailable": ("t.disabled === true", "why.top"),
        "62-call-caught-up": ("Caught the agent up", "v1First"),
        # `#347 settings-prose`. Pinned to the owner's name for the list (`Mark-read automatically`),
        # to no line of description drawn (`p.id !== ''`) and to the ? and its heading sharing one
        # tooltip (`h.title === q.title`).
        "63-settings-prose": ("Mark-read automatically", "p.id !== ''", "h.title === q.title"),
        # `#349 channel-settings-all`. Pinned to All channels being chosen (`s.value === '*'`), to Mixed
        # said in words (`Mixed: On in 1`), and, `#402 who-is-who-channel-box`, to Who is who in the box
        # asking for one channel (`Pick one channel`).
        "64-channel-settings-all": ("s.value === '*'", "Mixed: On in 1", "Pick one channel"),
        # `#348 settings-dynamic-lists`. Pinned to the dark monospaced panel (`Math.max(...rgb) < 48`),
        # to the fixed resizable scrolling list (`s.resize === 'vertical'`) and its one-line rows
        # (`textOverflow`); to the pop-out's whole text (`t.length > 400`); and to the readout's
        # fact-a-line detail (`split(`). `#351 settings-polish` pins the pop-out to being a modal
        # (`aria-modal`), Tab kept round it (`__popoutTabbed`), no tap through its dim (`__dimHits`)
        # and every control outside it under the dim, the top bar's Back among them (`__dimMisses`,
        # `'close-settings', 'dismiss-status'`), and to nothing drawn over it at the foot of the screen
        # with the pill up under it (`__footCovered`).
        "65-settings-dismissed-list": ("Math.max(...rgb) < 48", "s.resize === 'vertical'", "textOverflow"),
        "66-settings-dismissed-popout": ("t.length > 400", "dismissed-send-copy", "aria-modal", "__popoutTabbed",
                                         "__dimHits", "__dimMisses", "'close-settings', 'dismiss-status'",
                                         "__footCovered"),
        "67-settings-connection-panel": ("Math.max(...rgb) < 48", "split("),
        # `#346 settings-collapsible`. Pinned to every section folded and hidden until found
        # (`'until-found'`), to no setting drawn (`!window.__visible(id)`), and to the browser's search
        # opening the group and its section (`fold-microphone`, `mic-noise-suppression`).
        "68-settings-folded": ("'until-found'", "checkVisibility()"),
        "69-settings-find-opens": ("fold-microphone", "mic-noise-suppression"),
        # `#345 settings-multi-column`. Pinned to the columns the width has room for (`room`), to each
        # section whole (`getClientRects().length === 1`), packed (`rem + 2`) and in order (`placed`),
        # and to the Tab order through the titles (`__tabbed`).
        "70-settings-columns": ("room", "getClientRects().length === 1", "rem + 2", "placed"),
        "71-settings-columns-folded": ("__tabbed", "placed"),
        # `#355 provider-grouped-channels`. Pinned to the Service row in place of the one-service line
        # (`chat-provider-line`), to Slack's facts (`Slack: New messages`), to the picker listing only
        # Slack's channels (`All Slack channels|team|release`) and to the bar's groups (`optgroup`).
        "72-settings-services": ("chat-provider-line", "Slack: New messages", "All Slack channels|team|release",
                                 "optgroup"),
    }
    for name, needles in required.items():
        required_scene = next((s for s in SCENES if s.name == name), None)
        if required_scene is None:
            problems.append(f"the '{name}' state is gone from the scene table.")
            continue
        # A tuple where one marker is not enough: `02-idle` has to be pinned BOTH to the empty
        # state it is a picture of and to the absence of the status strip, which is the whole
        # content of `#63 status-line-placement`.
        for needle in (needles,) if isinstance(needles, str) else needles:
            if not any(needle in expression for _description, expression in required_scene.expect):
                problems.append(
                    f"scene '{name}' no longer checks for {needle!r} before capturing, so a run "
                    "that never reached that state would still write a picture under its name."
                )

    # The interface rework renamed all of these. A scene still driving one would not fail loudly --
    # `getElementById` returns null, `__visible` answers false, and the run reports an unreachable
    # state for a reason that sounds like a page bug. Catching it here says the harness is stale.
    RETIRED = {
        "tab-voice": "the two tab buttons became the single #view-switch",
        "tab-discord": "the two tab buttons became the single #view-switch",
        "li.system": "the end-of-call marker became li.seam",
        "Call ended": "the seam is labelled 'new conversation'",
        "conversation-state": "the state moved onto #status-line as a data attribute",
        "hangup-label": "Hang up is absent, not relabelled, when there is no call",
        "text-entry": "`#241 voice-composer` replaced the Type toggle with a box in the transcript",
    }
    for scene in SCENES:
        for description, expression in scene.expect:
            for dead, became in RETIRED.items():
                if dead in expression:
                    problems.append(
                        f"scene '{scene.name}' still drives {dead!r}, which the interface rework "
                        f"removed ({became}). The harness is photographing a page that is gone."
                    )

    error = Unreachable("06-post-call", "the transcript carries the seam that marks a new conversation")
    text = str(error)
    if "06-post-call" not in text:
        problems.append(
            "an unreachable state's error does not NAME the state, so a failing run would not say "
            "which picture is missing."
        )
    if "seam that marks" not in text:
        problems.append("an unreachable state's error does not say WHICH expectation failed.")

    return problems


def check_standalone_scene_controls() -> list[str]:
    """`#74 scroll-test-strength`, finding 3: a scene's dependencies are in code, not in the order.

    The walk through a session is deliberately a chain -- `04-muted` is a picture of the call
    `03-live-call` started, and rebuilding it would be a fiction. What is NOT allowed is a scene
    that inherits state it does not photograph: `13-jump-to-newest` used to show a "Collapse all"
    chip opened by the scene before it, and neither that scene nor `12-collapsed-long-transcript`
    could be run with `--only`, because "there is already a folded transcript on screen" was a
    requirement nothing stated.

    The fix was to make the three transcript states build the list they photograph, through one
    shared act. This is what stops that quietly coming undone: the builder has to load the page,
    and the two states after it have to call the builder.
    """
    problems: list[str] = []
    builder = inspect.getsource(_act_long_scroll)
    if "driver.load(" not in builder:
        problems.append(
            "_act_long_scroll no longer loads the page itself, so the three transcript states "
            "depend on whichever scene happened to run before them -- and none of them can be run "
            "with --only."
        )
    for name in ("12-collapsed-long-transcript", "13-jump-to-newest"):
        scene = next((s for s in SCENES if s.name == name), None)
        if scene is None:
            problems.append(f"the '{name}' state is gone from the scene table.")
            continue
        if "_act_long_scroll(" not in inspect.getsource(scene.act):
            problems.append(
                f"scene '{name}' no longer builds the transcript it photographs. It would then "
                "inherit whatever the scene before it left open -- which is the defect `#74 "
                "scroll-test-strength` recorded, and it reads as a page bug rather than as a "
                "harness one."
            )

    for name, act in (
        ("10-discord-view", _act_discord),
        ("self-contained channel scenes", _act_open_channel),
    ):
        if "channel-loading" not in inspect.getsource(act):
            problems.append(
                f"{name} accepts cached channel rows before the live server read finishes. "
                "A saved snapshot can then be photographed as current, or supply a stale paging cursor."
            )

    older = inspect.getsource(_act_channel_older)
    if "window.__visible('load-older')" not in older or ".disabled" not in older:
        problems.append(
            "21-channel-older-loaded no longer requires an enabled Older messages control before "
            "measuring and clicking. A missing cursor would be reported later as a generic row timeout."
        )
    return problems


def check_scene_storage_isolation_controls() -> list[str]:
    """A prior channel walk must not decide how much history the next scene can request."""

    class PageProbe:
        def __init__(self) -> None:
            self.init_scripts: list[str] = []

        def on(self, _event: str, _handler: object) -> None:
            pass

        # `Driver` stretches this one's deadlines (`_patient_waits`); the probe never waits.
        def wait_for_function(self, _expression: str, **_options: object) -> None:
            pass

        def add_init_script(self, script: str) -> None:
            self.init_scripts.append(script)

        def goto(self, _url: str, *, wait_until: str) -> None:
            if wait_until != "load":
                raise AssertionError(f"unexpected wait condition {wait_until!r}")

    page = PageProbe()
    driver = Driver(
        cast("Page", page),
        "http://127.0.0.1:18091",
        "screenshot-probe-token",
        Path("."),
        PROFILES[0],
        DEFAULT_THEME,
    )
    driver.load(with_token=True)
    script = "\n".join(page.init_scripts)
    if "localStorage.removeItem('vibe-talk.voice.message-cache')" not in script:
        return [
            "a screenshot-scene reload keeps the previous scene's saved channel snapshot. "
            "A scene that walks to the beginning then makes the older-page and pull scenes "
            "unreachable, because they restore a channel with no history left above it."
        ]
    # `#193 compose-draft-loss`. A refused send, or one dismissed, outlives the scene that made it
    # unless the reload clears it: every later channel frame would carry it at the foot of the list.
    for key in ("vibe-talk.outgoing-messages", "vibe-talk.dismissed-sends"):
        if f"localStorage.removeItem('{key}')" not in script:
            return [
                f"a screenshot-scene reload keeps {key}, so the refused send one scene leaves "
                "behind is drawn in every later channel frame."
            ]
    # `#265 window-channel-memory`. Every scene loads in one tab, whose own copy of where the reader
    # was a reload reads before the device's: cleared from the device alone, it still reopens there.
    for storage in ("localStorage", "sessionStorage"):
        if f"{storage}.removeItem('vibe-talk.voice.ui-state')" not in script:
            return [
                f"a screenshot-scene reload keeps the UI record in {storage}, so a scene opens on "
                "the view the previous scene left up instead of the default."
            ]
    return []


# The predicate the summarised state used to wait on, kept verbatim so the control below can prove it
# still tells a scoped wait from this one. It accepts ANY row's finished summary, which is how the
# probe got measured while it still read `summarising…`.
ANY_ROW_SUMMARISED = (
    "[...document.querySelectorAll('#discord-log .summary-text')]"
    ".some((n) => (n.textContent || '').length > 0 && !/^summarising/.test(n.textContent))"
)


def summary_wait_scope_problem(predicate: str) -> str | None:
    """Why `predicate` cannot say the PROBED row is summarised, or None if it can."""
    if "querySelector('#summary-probe .summary-text')" not in predicate:
        return "it does not read the summary of #summary-probe, the row whose height is compared"
    if "querySelectorAll" in predicate:
        return "it also looks at other rows, whose summaries say nothing about the probe"
    if "!/^summarising/.test(" not in predicate or ".length > 0" not in predicate:
        return "it does not rule out the empty and `summarising…` states the probe starts in"
    return None


def check_summary_scene_controls() -> list[str]:
    """The two summary states share a vendor and a cache, and both sharings can go quietly wrong.

    Neither failure is loud. A summarised state that inherited the wedged vendor from the state
    before it would be UNREACHABLE, which at least says something — but a failed state that
    inherited the CACHED summary would simply be served the old answer, never fail, and time out
    twenty seconds later reporting the page. And a failed state that ran anywhere but last would
    empty the store underneath whatever came after it.

    So each of the three is asserted here, offline, against the source of the acts themselves.
    """
    problems: list[str] = []

    summarised = inspect.getsource(_act_summary_mode)
    if '_tell_the_mock(' not in summarised or '"no_tool_call"' not in summarised:
        problems.append(
            "32-channel-summarised no longer puts the offline vendor back into a scenario that "
            "ANSWERS. It would then inherit whichever scenario ran last -- and the state after it "
            "deliberately wedges the same mock, so on every profile but the first this state would "
            "photograph a failure under the name of a success."
        )

    scope_problem = summary_wait_scope_problem(SUMMARY_PROBE_SUMMARISED)
    if scope_problem is not None:
        problems.append(
            f"32-channel-summarised cannot tell when the probed row is summarised: {scope_problem}. "
            "The wait can then end while the probe still reads `summarising…` -- another row's "
            "cached summary was enough, once -- and the height comparison fails against the "
            "placeholder, intermittently, blaming the page."
        )
    if summary_wait_scope_problem(ANY_ROW_SUMMARISED) is None:
        problems.append(
            "the summary-wait scope check accepts the any-row predicate that raced, so it cannot "
            "tell a scoped wait from the one that measured the placeholder."
        )
    if "wait_for_function(f\"() => {SUMMARY_PROBE_SUMMARISED}\"" not in summarised:
        problems.append(
            "32-channel-summarised no longer waits on SUMMARY_PROBE_SUMMARISED before measuring "
            "the probe, so its height can be read while the row still shows the placeholder."
        )
    summarised_scene = next(scene for scene in SCENES if scene.name == "32-channel-summarised")
    if SUMMARY_PROBE_SUMMARISED not in (expression for _label, expression in summarised_scene.expect):
        problems.append(
            "32-channel-summarised no longer EXPECTS a real summary on the probed row, so a "
            "photograph of the probe mid-request would be certified as the summarised state."
        )

    failed = inspect.getsource(_act_summary_failed)
    if "_forget_cached_summaries(" not in failed:
        problems.append(
            f"{SUMMARY_FAILED_STATE} no longer clears the summary cache. The state before it files "
            "a summary for the same message under the same policy version, so the request this "
            "state needs to FAIL would be answered out of the store and never reach the "
            "summariser at all."
        )
    if '"no_reply"' not in failed:
        problems.append(
            f"{SUMMARY_FAILED_STATE} no longer wedges the offline vendor, so nothing makes the "
            "summary fail. A state that cannot reach a failure cannot photograph one."
        )

    if SCENES[-1].name != SUMMARY_FAILED_STATE:
        problems.append(
            f"{SUMMARY_FAILED_STATE} is no longer the last state in the walk. It empties the "
            "server's durable store on the way in, so anything after it inherits a purged "
            f"transcript -- and would fail for a reason that names neither state. It is "
            f"'{SCENES[-1].name}' that now runs last."
        )

    return problems


def check_blank_gate_controls(tmp: Path) -> list[str]:
    """record_capture is the only path that writes a screenshot, so the gate cannot be bypassed."""
    problems: list[str] = []
    blank = tmp / "blank.png"
    try:
        record_capture(blank, _flat_png(400, 400, (255, 255, 255)), "control/blank")
    except TrivialCapture as error:
        if "control/blank" not in str(error):
            problems.append("a rejected capture's error does not name WHICH capture was rejected.")
    else:
        problems.append(
            "negative control: record_capture WROTE and accepted a blank frame. The blank check is "
            "not on the path that saves screenshots, which is the only path that matters."
        )
    if not blank.exists():
        problems.append(
            "a rejected capture was not left on disk. It must be, so the reader can look at what "
            "the harness saw rather than taking its word for it."
        )

    good = tmp / "good.png"
    try:
        verdict = record_capture(good, _busy_png(300, 300), "control/good")
    except TrivialCapture as error:
        problems.append(f"positive control: record_capture rejected a rendered frame ({error}).")
    else:
        if not good.exists() or not verdict.ok:
            problems.append("positive control: a good capture was not written.")

    # The walk's own path: judged in another process, as `run_captures` does, the verdict waited for.
    with ProcessPoolExecutor(max_workers=1, mp_context=multiprocessing.get_context("spawn")) as judges:
        later_blank = start_capture(
            tmp / "later-blank.png", _flat_png(400, 400, (0, 0, 0)), "control/later-blank", judges
        )
        later_good = start_capture(
            tmp / "later-good.png", _busy_png(300, 300), "control/later-good", judges
        )
        try:
            later_blank.verdict()
        except TrivialCapture as error:
            if "control/later-blank" not in str(error):
                problems.append("a capture rejected in a judging process is not named.")
        else:
            problems.append(
                "negative control: a blank frame judged in another process was accepted, so the "
                "walk's own captures are not held to the blank check."
            )
        try:
            if not later_good.verdict().ok:
                problems.append("positive control: a good capture judged in another process failed.")
        except TrivialCapture as error:
            problems.append(f"positive control: a judging process rejected a rendered frame ({error}).")
    return problems


def check_profile_controls() -> list[str]:
    problems: list[str] = []
    phones = [p for p in PROFILES if p.mobile and p.scale > 1 and p.width <= 500]
    if not phones:
        problems.append(
            "no phone-class profile survives (mobile, device scale factor above 1, narrow). The "
            "phone is the device this page is used on."
        )
    if len(phones) < 2:
        problems.append(
            "only one phone profile is captured. A tall phone alone hides the vertical clipping a "
            "short one shows."
        )
    desktops = [p for p in PROFILES if p.width >= 900 and not p.mobile]
    if not desktops:
        problems.append("no desktop-width profile survives; the wide layout would go unseen.")
    if len({p.width for p in desktops}) < 2:
        problems.append(
            "only one desktop-class width is captured. `#55 voice-desktop-app` caps the transcript "
            "to a reading column, so at one width there is nothing to compare against: the column "
            "is the same size at 1280 and at 1440 and only the margin beside it changes, and that "
            "is the property. One width cannot show it."
        )
    if not any(p.width >= 1200 for p in desktops):
        problems.append("no wide desktop profile survives; the maximised window would go unseen.")
    # The desktop regime is `(min-width: 900px) and (pointer: fine)`. A profile that claims to be
    # desktop-class but reports a coarse pointer would be photographing the phone composition under
    # a desktop name -- `has_touch` is what Chromium turns into `pointer: coarse`.
    for profile in desktops:
        if profile.context_options("dark")["has_touch"]:
            problems.append(
                f"the {profile.name!r} profile is wide but reports a touch screen, so "
                "`pointer: fine` does not match and it captures the PHONE layout."
            )
    for wanted in DESKTOP_PROFILES:
        if wanted not in {p.name for p in desktops}:
            problems.append(
                f"DESKTOP_PROFILES names {wanted!r}, which is not a desktop-class profile, so "
                "every desktop-only scene is restricted to a device that cannot show it."
            )
    if len({p.name for p in PROFILES}) != len(PROFILES):
        problems.append("two profiles share a name, so their captures would collide.")
    return problems


def check_theme_controls() -> list[str]:
    """Dark must stay the default: the first run of this harness reviewed a theme he never sees."""
    problems: list[str] = []
    if DEFAULT_THEME != "dark":
        problems.append(
            f"the default theme is {DEFAULT_THEME!r}. The owner's device is dark, and a light-only "
            "run reviews a page he never looks at -- which is what happened the first time."
        )
    if set(THEMES) != {"dark", "light"}:
        problems.append(f"the theme list is {THEMES!r}; both schemes must remain capturable.")
    for theme in THEMES:
        options = PROFILES[0].context_options(theme)
        if options.get("color_scheme") != theme:
            problems.append(
                f"a {theme} context does not set color_scheme, so the page would follow "
                "Playwright's default (light) whatever the run was asked for."
            )
    return problems


def check_dependency_message_controls() -> list[str]:
    problems: list[str] = []
    package = PlaywrightMissing("The python3 'playwright' package is not installed.", INSTALL_HINT)
    if "pip install" not in str(package) or "playwright install chromium" not in str(package):
        problems.append(
            "the missing-package error does not carry the install command, so the reader is told "
            "what is wrong but not how to fix it."
        )
    browser = PlaywrightMissing("browser missing", BROWSER_INSTALL_HINT)
    if "playwright install chromium" not in str(browser):
        problems.append("the missing-browser error does not name the install command.")
    return problems


def check_viewport_meta_controls() -> list[str]:
    problems: list[str] = []
    good = '<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover" />'
    if expect_viewport_meta(good):
        problems.append(
            "positive control: the real viewport declaration was reported as a problem."
        )
    stripped = '<meta name="viewport" content="width=device-width, initial-scale=1" />'
    if not expect_viewport_meta(stripped):
        problems.append(
            "negative control: markup with viewport-fit=cover REMOVED was reported as fine. That "
            "removal is a real regression on a phone with a cutout."
        )
    return problems


def strip_js_comments(text: str) -> str:
    """Drop comments before asserting a string is ABSENT from the stub.

    The same trap tests/js/voice_page.test.mjs documents: STUB_JS explains at length WHY it does
    not fake the microphone and WHY it avoids offsetParent, so those words are all present, in
    prose, and an absence assertion fails against a stub that is entirely correct.
    """
    out = []
    at = 0
    while at < len(text):
        if text.startswith("/*", at):
            end = text.find("*/", at + 2)
            at = len(text) if end < 0 else end + 2
        elif text.startswith("//", at):
            end = text.find("\n", at)
            at = len(text) if end < 0 else end
        else:
            out.append(text[at])
            at += 1
    return "".join(out)


def check_popout_dim_controls() -> list[str]:
    """`#351 settings-polish`. The pop-out's dim check, on pictures made here: a control drawn under one
    dim is judged and passes; one drawn bright, one dimmed twice and one that moved each fail by name; and
    one wholly under the pop-out is not judged at all."""
    problems: list[str] = []
    width, height = 120, 60
    grey = (200, 180, 160)

    def picture(shares: dict[tuple[int, int, int, int], float]) -> Image:
        pixels: list[tuple[int, ...]] = []
        for y in range(height):
            for x in range(width):
                share = next((value for (x0, y0, x1, y1), value in shares.items() if x0 <= x < x1 and y0 <= y < y1), 1.0)
                pixels.append(tuple(int(channel * share) for channel in grey))
        return Image(width=width, height=height, pixels=pixels)

    def layout(*controls: tuple[str, list[float]]) -> DimLayout:
        return {"controls": [{"name": name, "box": box} for name, box in controls], "popout": [80.0, 0.0, 120.0, 20.0]}

    once, bright, twice, beside = [0.0, 0.0, 10.0, 10.0], [0.0, 30.0, 10.0, 40.0], [20.0, 30.0, 30.0, 40.0], [90.0, 2.0, 100.0, 12.0]
    lit = picture({})
    dim = picture({(0, 0, 10, 10): 0.55, (20, 30, 30, 40): 0.3, (90, 2, 100, 12): 0.55})
    controls = (("once", once), ("bright", bright), ("twice", twice), ("beside", beside))
    judged, misses = popout_dim_misses(layout(*controls), lit, layout(*controls), dim)
    if judged != ["once", "bright", "twice"]:
        problems.append(f"the pop-out's dim check judged {judged}, not the three controls clear of the pop-out.")
    if [miss.split(" ")[0] for miss in misses] != ["bright", "twice"]:
        problems.append(f"the pop-out's dim check missed {misses}, not the control drawn bright and the one dimmed twice.")
    _judged, moved = popout_dim_misses(layout(("once", once)), lit, layout(("once", [0.0, 5.0, 10.0, 15.0])), dim)
    if moved != ["once moved when the pop-out opened"]:
        problems.append(f"the pop-out's dim check compared a control that moved between its two pictures: {moved}.")
    _judged, counted = popout_dim_misses(layout(("once", once)), lit, layout(), dim)
    if not counted:
        problems.append("the pop-out's dim check passed with a control gone from the screen when the pop-out opened.")
    return problems


def check_popout_covered_controls() -> list[str]:
    """`#351 settings-polish`. The pill-over-the-pop-out check, on pictures made here: the pill drawn
    under the pop-out passes, and so does a change only in a rounded corner; the pill drawn over it, and
    a pill that does not reach into it at all, each fail."""
    problems: list[str] = []
    width, height = 80, 60
    popout, pill = [10.0, 10.0, 70.0, 50.0], [0.0, 35.0, 80.0, 45.0]

    def picture(over: bool, corner: bool = False) -> Image:
        pixels: list[tuple[int, ...]] = []
        for y in range(height):
            for x in range(width):
                inside = 10 <= x < 70 and 10 <= y < 50
                under_pill = 0 <= x < 80 and 35 <= y < 45
                colour = (30, 34, 40)
                if under_pill and (over or not inside):
                    colour = (120, 120, 120)
                elif inside:
                    colour = (60, 66, 76)
                if corner and x == 11 and y == 48:
                    colour = (0, 0, 0)
                pixels.append(colour)
        return Image(width=width, height=height, pixels=pixels)

    bare = Image(width=width, height=height, pixels=[
        (60, 66, 76) if 10 <= x < 70 and 10 <= y < 50 else (30, 34, 40) for y in range(height) for x in range(width)])
    if popout_covered(bare, picture(over=False), popout, pill):
        problems.append("the pill-over-the-pop-out check failed a pill drawn under the pop-out.")
    if popout_covered(bare, picture(over=False, corner=True), popout, pill):
        problems.append("the pill-over-the-pop-out check counted a pixel in the pop-out's rounded corner.")
    if not popout_covered(bare, picture(over=True), popout, pill):
        problems.append("the pill-over-the-pop-out check passed a pill drawn over the pop-out.")
    if not popout_covered(bare, bare, popout, [0.0, 52.0, 80.0, 58.0]):
        problems.append("the pill-over-the-pop-out check passed a pill that does not reach into the pop-out.")
    return problems


def check_page_error_controls() -> list[str]:
    """A page that threw must not be photographed. The check lives on the capture path itself."""
    problems: list[str] = []
    source = pathlib.Path(__file__).read_text() if "__file__" in globals() else ""
    if "pageerror" not in source:
        problems.append(
            "nothing listens for an uncaught page error, so a page whose script died on load "
            "would be photographed looking perfectly plausible and entirely inert."
        )
    if "if self.page_errors:" not in source:
        problems.append(
            "collected page errors are never consulted before a capture, so listening for them "
            "achieves nothing."
        )
    return problems


def check_stub_controls() -> list[str]:
    """The stub must cover exactly what web/voice.js uses of a socket, and no more."""
    problems: list[str] = []
    code = strip_js_comments(STUB_JS)
    for needed in (
        "CONNECTING",
        "OPEN",
        "CLOSING",
        "CLOSED",
        "onopen",
        "onmessage",
        "onclose",
        "readyState",
        "send(",
        "close(",
    ):
        if needed not in code:
            problems.append(
                f"the fake conversation socket no longer provides {needed!r}; web/voice.js uses "
                "it, so the live-call states would not be reachable."
            )
    if "getUserMedia" in code or "AudioContext" in code:
        problems.append(
            "the stub is replacing the microphone or the audio graph. Those run for real against "
            "Chromium's fake capture device; faking them would mean the live-call captures no "
            "longer show the real capture path running."
        )
    if "offsetParent" in code:
        problems.append(
            "the visibility helper is back on offsetParent, which is null for a fixed-position "
            "element and would report a perfectly visible control as absent."
        )
    return problems


SELF_TEST_CHECKS = (
    "the PNG decoder reconstructs all five row filters correctly",
    "the pop-out's dim check fails a control drawn bright, dimmed twice or moved, and passes one dimmed once",
    "the pill-over-the-pop-out check fails a pill drawn over the pop-out or not reaching it, and passes one under",
    "the PNG decoder reads dimensions from a real header",
    "a varied, rendered frame is accepted",
    "a flat WHITE frame is rejected",
    "a flat BLACK frame is rejected",
    "a flat dark-grey frame is rejected",
    "a frame that is 99.99% one colour with a speck is rejected",
    "an empty file is rejected",
    "a truncated PNG is rejected",
    "an 8x8 frame is rejected as too small to be a page",
    "a 60x60 frame is rejected on its DIMENSIONS, not on its byte count",
    "every state is pinned to its own distinguishing marker",
    "the idle frame is pinned to the status strip being GONE, and 08 to it being up",
    "no state still drives a selector the interface rework retired",
    "the seam state is pinned to the dock geometry, not just to the element",
    "the suspended state is pinned to the ABSENCE of the failure banner",
    "the failed state is pinned to its presence, so silencing errors cannot pass",
    "the reply state is pinned to the scroll round trip, not to the form being on screen",
    "the long-target state is pinned to the target really overflowing its own box",
    "the hover state is pinned to the rendered colour, not to the row being hovered",
    "the older-step state is pinned to the anchor holding, not to the list being longer",
    "the typed-turn state is pinned to the dock geometry, not just to the field being up",
    "both control-bar states are pinned to where the bar IS, not to the bar existing",
    "the voice-composer state is pinned to the switch staying on the bar beside the focused box",
    "the packed-bar state is pinned to nothing being clipped, not to the buttons existing",
    "the armed-pull state is pinned to the channel still having history above the reader",
    "dark is the default theme, and both schemes stay capturable",
    "each theme really sets color_scheme on the browser context",
    "a non-PNG file is rejected",
    "record_capture REFUSES to certify a blank frame it wrote",
    "a rejected capture is still left on disk to look at",
    "record_capture accepts and writes a rendered frame",
    "every state declares at least one expectation before its shutter opens",
    "the three transcript states build the list they photograph, so --only reaches any of them",
    "channel scenes wait for the live read, not merely for cached rows",
    "the older-page scene checks its cursor-bearing control before it clicks",
    "each scene reload drops the prior scene's saved channel snapshot",
    "an unreachable state's error names the state AND the expectation",
    "a phone-class profile with a real device scale factor survives",
    "both a tall and a short phone profile survive",
    "a desktop-width profile survives",
    "more than one desktop-class width survives, so a capped column can be compared",
    "every desktop-class profile really reports a fine pointer",
    "a desktop-only scene names profiles that exist",
    "every state that exists on only some devices is still restricted to them",
    "the summarised state resets the offline vendor rather than inheriting a wedged one",
    "the summarised state waits for and expects the PROBED row's summary, not any row's",
    "the failed-summary state clears the cache, so its request really reaches the summariser",
    "the failed-summary state wedges the vendor, so the failure it photographs is real",
    "the failed-summary state runs last, because it empties the store on its way in",
    "the missing-package error carries the pip install command",
    "the missing-browser error carries the browser install command",
    "removing viewport-fit=cover from the markup is reported",
    "the fake socket still provides everything web/voice.js uses",
    "an uncaught page error is reported instead of being photographed",
    "the microphone and audio graph are NOT stubbed",
)


def run_self_test(tmp: Path) -> int:
    print("screenshots self-test — the controls only. No browser, no server, no vendor minutes.")
    print("")
    problems = (
        check_decoder_controls()
        + check_image_judgement_controls()
        + check_blank_gate_controls(tmp)
        + check_state_controls()
        + check_standalone_scene_controls()
        + check_scene_storage_isolation_controls()
        + check_summary_scene_controls()
        + check_profile_controls()
        + check_theme_controls()
        + check_dependency_message_controls()
        + check_viewport_meta_controls()
        + check_stub_controls()
        + check_page_error_controls()
        + check_popout_dim_controls()
        + check_popout_covered_controls()
    )
    if problems:
        for problem in problems:
            print(f"  FAIL  {problem}")
        print("")
        print(f"FAILED — {len(problems)} control(s) failed. The checks cannot be trusted.")
        return EXIT_CONTROL_FAILED
    for check in SELF_TEST_CHECKS:
        print(f"  ok    {check}")
    print("")
    print(f"PASSED — {len(SELF_TEST_CHECKS)} controls")
    return EXIT_OK


# =================================================================================================


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="screenshots.py",
        description="Photograph the app, the page at /, in every state that looks different.",
    )
    parser.add_argument("--url", help="base URL of a running vibe-talk (NOT the live one on 8080)")
    parser.add_argument("--channel", help="channel snowflake the Discord tab should read")
    parser.add_argument("--out", help="directory to write the PNGs into")
    parser.add_argument(
        "--only",
        action="append",
        default=[],
        metavar="STATE",
        help="capture only this state (repeatable); default is all of them",
    )
    parser.add_argument(
        "--theme",
        choices=("dark", "light", "both"),
        default=DEFAULT_THEME,
        help="which colour scheme to capture (default: dark — the owner's phone is dark, and "
        "contrast that reads on white can vanish on black)",
    )
    parser.add_argument(
        "--mock-control",
        default="",
        metavar="URL",
        help="origin of vibe-talk-mock-elevenlabs' /_mock/ control plane, e.g. "
        "http://127.0.0.1:18092. The two summary states need it: there is one summariser and it "
        "holds a conversation with an agent, so a working one and a failing one both have to be "
        "arranged. scripts/run.sh --screenshots starts the mock and passes this",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run the controls offline and exit; needs no browser and no server",
    )
    return parser


def main(argv: list[str]) -> int:
    args = build_parser().parse_args(argv)

    if args.self_test:
        import tempfile

        with tempfile.TemporaryDirectory(prefix="vibe-talk-shots-selftest-") as tmp:
            return run_self_test(Path(tmp))

    if not args.url:
        print("screenshots.py: --url is required (or use --self-test).", file=sys.stderr)
        return EXIT_USAGE
    if not args.out:
        print("screenshots.py: --out is required.", file=sys.stderr)
        return EXIT_USAGE
    token = os.environ.get("VIBE_TALK_WRITE_TOKEN", "")
    if not token:
        print(
            "screenshots.py: VIBE_TALK_WRITE_TOKEN is not set. It comes from the environment and "
            "never from the command line, because a command line is readable by every process on "
            "this box.",
            file=sys.stderr,
        )
        return EXIT_USAGE

    out_dir = Path(args.out).resolve()
    started = time.time()

    try:
        ensure_playwright()
    except PlaywrightMissing as error:
        print(f"screenshots.py: {error}", file=sys.stderr)
        return EXIT_PLAYWRIGHT_MISSING

    try:
        check_server(args.url)
    except ServerUnreachable as error:
        print(f"screenshots.py: {error}", file=sys.stderr)
        return EXIT_SERVER_UNREACHABLE

    try:
        with urlopen(f"{args.url.rstrip('/')}/", timeout=10) as response:
            html = response.read().decode("utf-8", "replace")
    except URLError as error:
        print(f"screenshots.py: could not read {args.url.rstrip('/')}/ ({error})", file=sys.stderr)
        return EXIT_SERVER_UNREACHABLE
    for problem in expect_viewport_meta(html):
        print(f"screenshots.py: {problem}", file=sys.stderr)
        return EXIT_UNREACHABLE

    try:
        themes = THEMES if args.theme == "both" else (cast(Theme, args.theme),)
        results = run_captures(
            args.url,
            token,
            args.channel or "",
            out_dir,
            args.only,
            themes,
            args.mock_control or "",
        )
    except PlaywrightMissing as error:
        print(f"screenshots.py: {error}", file=sys.stderr)
        return EXIT_PLAYWRIGHT_MISSING
    except Unreachable as error:
        print(f"screenshots.py: {error}", file=sys.stderr)
        return EXIT_UNREACHABLE
    except TrivialCapture as error:
        print(f"screenshots.py: {error}", file=sys.stderr)
        return EXIT_TRIVIAL_CAPTURE

    print("")
    print(
        f"{len(results)} capture(s) in {time.time() - started:.0f}s. "
        "Chromium reports no safe-area inset under automation, so the notch and home-indicator "
        "padding are NOT shown; everything else is the page as it lays out."
    )
    print("")
    for _name, path, _verdict in results:
        print(path)
    return EXIT_OK


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
