#!/usr/bin/env python3
"""Photograph the /voice page in every state that looks different, so an agent can LOOK at it.

WHY THIS EXISTS
===============

The /voice page has a large behavioural suite (tests/js/voice_page.test.mjs) that drives the real
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

Everything else is real: a real gent-talk server (started by run.sh with --fake-discord), the real
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

The write-scope token comes from $GENT_TALK_WRITE_TOKEN. It is never accepted on the command line,
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
import json
import os
import pathlib
import struct
import sys
import time
import zlib
from collections import Counter
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Callable, Iterable, Literal, TypedDict, cast
from urllib.error import URLError
from urllib.request import urlopen

if TYPE_CHECKING:
    from playwright.sync_api import Browser, Page, Playwright

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
# Decoding is done here, in the standard library, rather than through Pillow or numpy. Measured on
# a 1179x1977 phone capture the pure-Python path takes about 1.8 seconds, which is a fine price for
# not making this script's one hard dependency into two. `--self-test` builds its own PNGs with
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


def decode_png(data: bytes) -> Image:
    """Decode an 8-bit non-interlaced PNG. That is what every browser writes."""
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

    pixels: list[tuple[int, ...]] = []
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
            for x in range(channels, stride):
                line[x] = (line[x] + line[x - channels]) & 0xFF
        elif kind == 2:
            for x in range(stride):
                line[x] = (line[x] + previous[x]) & 0xFF
        elif kind == 3:
            for x in range(stride):
                left = line[x - channels] if x >= channels else 0
                line[x] = (line[x] + ((left + previous[x]) >> 1)) & 0xFF
        elif kind == 4:
            for x in range(stride):
                left = line[x - channels] if x >= channels else 0
                up = previous[x]
                up_left = previous[x - channels] if x >= channels else 0
                estimate = left + up - up_left
                da, db, dc = (
                    abs(estimate - left),
                    abs(estimate - up),
                    abs(estimate - up_left),
                )
                if da <= db and da <= dc:
                    nearest = left
                elif db <= dc:
                    nearest = up
                else:
                    nearest = up_left
                line[x] = (line[x] + nearest) & 0xFF
        else:
            raise BadPng(f"unknown row filter {kind} on row {row}")
        for x in range(0, stride, channels):
            pixels.append(tuple(line[x : x + channels]))
        previous = line
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
    try:
        image = decode_png(data)
    except BadPng as error:
        return Verdict(False, f"the file is not a readable PNG: {error}")
    if image.width < MIN_EDGE or image.height < MIN_EDGE:
        return Verdict(
            False,
            f"the frame is {image.width}x{image.height}; both edges must be at least {MIN_EDGE}px",
            width=image.width,
            height=image.height,
        )
    counts = Counter(image.pixels)
    distinct = len(counts)
    total = len(image.pixels)
    dominant = counts.most_common(1)[0][1] / total if total else 1.0
    if distinct < MIN_DISTINCT_COLOURS:
        return Verdict(
            False,
            f"the frame has only {distinct} distinct colour(s) — it is blank or a flat fill, "
            f"not a rendered page",
            width=image.width,
            height=image.height,
            distinct=distinct,
            dominant_share=dominant,
        )
    if dominant > MAX_DOMINANT_SHARE:
        return Verdict(
            False,
            f"one colour covers {dominant:.2%} of the frame — the page is effectively blank "
            f"even though {distinct} colours are present",
            width=image.width,
            height=image.height,
            distinct=distinct,
            dominant_share=dominant,
        )
    return Verdict(
        True,
        "shows a rendered page",
        width=image.width,
        height=image.height,
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


def record_capture(path: Path, data: bytes, label: str) -> Verdict:
    """Write a capture and judge it. The write happens first, so a rejected frame can be looked at.

    This is the only function that puts a screenshot on disk, which is what makes the blank check
    impossible to route around: there is no second path that writes an unjudged file.
    """
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(data)
    verdict = judge_capture(data)
    if not verdict.ok:
        raise TrivialCapture(label, verdict)
    return verdict


# =================================================================================================
# Reaching a state, and refusing to photograph one we did not reach.
# =================================================================================================


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
      window.__socket = this;
      setTimeout(() => {
        if (this.readyState !== FakeConversationSocket.CONNECTING) return;
        this.readyState = FakeConversationSocket.OPEN;
        if (this.onopen) this.onopen({});
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
)

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


class Driver:
    """Walks one browser page through the scenes, capturing as it goes."""

    def __init__(
        self, page: Page, base_url: str, token: str, out_dir: Path, profile: Profile, theme: Theme
    ) -> None:
        self.theme = theme
        self.page = page
        # A page whose script threw on load still RENDERS: the markup is there, the sign-in screen
        # looks entirely plausible, and every control is inert. That is a photograph of a broken
        # page filed as a photograph of a working one, and it is a live hazard here -- web/voice.js
        # is compiled in beside web/voice.html, so an edit to one and not the other produces
        # exactly this. Collected per page and reported by name at the first capture.
        self.page_errors: list[str] = []
        page.on("pageerror", lambda error: self.page_errors.append(str(error).splitlines()[0]))
        self.base_url = base_url.rstrip("/")
        self.token = token
        self.out_dir = out_dir
        self.profile = profile
        self.results: list[tuple[str, Path, Verdict]] = []

    # -- page-level helpers ----------------------------------------------------------------------

    def load(self, with_token: bool) -> None:
        script = STUB_JS
        if with_token:
            script += (
                "\ntry { localStorage.setItem('gent-talk.token', "
                + json.dumps(self.token)
                + "); } catch (e) {}\n"
            )
        else:
            script += "\ntry { localStorage.removeItem('gent-talk.token'); } catch (e) {}\n"
        # Every PREFERENCE the page keeps is cleared on a load, so a self-contained state really is
        # self-contained. Without this the desktop states inherit the reading column the previous
        # one dragged out, and every later desktop frame is a picture of the wrong width -- which
        # is what the first capture of the reply screen actually showed. The token above is the one
        # thing deliberately carried across, because signing in is not the subject of any state
        # after the first.
        script += (
            "\ntry {\n"
            "  localStorage.removeItem('gent-talk.voice.width');\n"
            "  localStorage.removeItem('gent-talk.voice.drafts');\n"
            "} catch (e) {}\n"
        )
        self.page.add_init_script(script)
        self.page.goto(f"{self.base_url}/voice", wait_until="load")

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
        for description, expression in scene.expect:
            try:
                held = bool(self.js(expression))
            except Exception as error:  # a broken expression is a failed expectation, loudly
                raise Unreachable(
                    scene.name, description, f"the check itself errored: {error}"
                ) from error
            if not held:
                raise Unreachable(scene.name, description, f"check: {expression}")

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
        verdict = record_capture(path, data, f"{self.theme}/{self.profile.name}/{scene.name}")
        self.results.append((scene.name, path, verdict))


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
    driver.page.click("#transcript li.seam summary")
    driver.page.wait_for_function(
        "() => !!document.querySelector('#transcript li.seam details[open]')", timeout=5_000
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
        "() => document.querySelectorAll('#discord-log li').length > 0", timeout=15_000
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
    driver.click("view-switch")  # back to the call
    driver.page.wait_for_function(
        "() => window.__text('view-switch-label') === 'Voice'", timeout=5_000
    )
    driver.click("talk")
    driver.page.wait_for_function(
        "() => window.__text('talk-label') === 'Listening'", timeout=10_000
    )
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
    driver.js("(() => { document.getElementById('scroll-area').scrollTop = 0; })()")
    driver.settle(150)
    # Measure ONE message closed, then open THAT ONE. Comparing the first open message against the
    # first closed one compares two lengths, not two states, and passes or fails on which message
    # happened to be longest — at desktop width it passed for the wrong reason.
    driver.js(
        "(() => { const items = "
        "[...document.querySelectorAll(\"#transcript li[data-collapsed='true']\")]; "
        "const li = items.sort((a, b) => b.textContent.length - a.textContent.length)[0]; "
        "li.id = 'fold-probe'; "
        "window.__foldedHeight = li.querySelector('.body').getBoundingClientRect().height; "
        "return window.__foldedHeight; })()"
    )
    driver.page.click("#fold-probe .fold")
    driver.page.wait_for_function(
        "() => !!document.querySelector(\"#transcript li[data-collapsed='false']\")", timeout=5_000
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


def _act_jump_to_newest(driver: Driver) -> None:
    # A turn arrives while the reader is up in the history. The page must NOT drag them down to it
    # — and must not leave them unaware it happened either, which is what the chip is for.
    driver.js(
        "(() => { const a = document.getElementById('scroll-area'); "
        "a.scrollTop = Math.round(a.scrollHeight * 0.2); return a.scrollTop; })()"
    )
    driver.settle(200)
    driver.js(
        "window.__agentSays('One more thing while you were reading: the release token expires in "
        "six days and rotating it needs the maintainer account.')"
    )
    driver.page.wait_for_function("() => window.__visible('jump-newest')", timeout=5_000)
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
    driver.page.wait_for_function("() => window.__visible('reading-width')", timeout=5_000)
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


# `#51 reply-view`. The reply screen, opened on a real seeded channel message.
#
# Both states are self-contained and run everywhere: the phone is where a reply is actually typed,
# and the thing most likely to be wrong on it -- the message you are answering pushing the text box
# under the keyboard -- is a layout fact nothing else here can see.


def _act_open_channel(driver: Driver) -> None:
    driver.load(with_token=True)
    driver.page.wait_for_function("() => window.__visible('control-pane')", timeout=10_000)
    driver.click("view-switch")
    driver.page.wait_for_function(
        "() => document.querySelectorAll('#discord-log li').length > 0", timeout=15_000
    )
    driver.settle(300)


def _act_reply_view(driver: Driver) -> None:
    _act_open_channel(driver)
    # Park in the middle of the channel, and record it. The round trip below is then a MEASURED
    # claim about the reader's position rather than a hope -- and it is one no unit test can make,
    # because the reset a browser performs on a hidden element's scrollTop is a browser fact.
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
        "const li = [...document.querySelectorAll('#discord-log li')].find((n) => { "
        "const r = n.getBoundingClientRect(); return r.top >= a.top && r.bottom <= a.bottom; }); "
        "if (!li) throw new Error('no channel row is fully on screen to reply to'); "
        "li.id = 'reply-probe'; return li.id; })()"
    )
    driver.page.click("#reply-probe .reply-button")
    driver.page.wait_for_function("() => window.__visible('screen-reply')", timeout=5_000)
    driver.click("reply-cancel")
    driver.page.wait_for_function("() => window.__visible('pane-discord')", timeout=5_000)
    driver.settle(200)
    driver.js(
        "(() => { window.__returnedTo = document.getElementById('scroll-area').scrollTop; "
        "return window.__returnedTo; })()"
    )
    # ...and open it again, because THAT is the screen this state is a picture of.
    driver.page.click("#reply-probe .reply-button")
    driver.page.wait_for_function("() => window.__visible('screen-reply')", timeout=5_000)
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
    _act_open_channel(driver)
    posted = driver.page.evaluate(
        "async (text) => {"
        "  const log = document.getElementById('discord-log');"
        "  if ([...log.children].some((n) => n.textContent.includes('in full, because you asked')))"
        "    return 'already there';"
        "  const channel = document.getElementById('discord-channel').value;"
        "  const response = await fetch('/api/v1/channels/' + encodeURIComponent(channel) +"
        "    '/reply', { method: 'POST', headers: {"
        "      Authorization: 'Bearer ' + localStorage.getItem('gent-talk.token'),"
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
        driver.click("refresh-discord")
        driver.page.wait_for_function(
            "() => [...document.querySelectorAll('#discord-log li')]"
            ".some((n) => n.textContent.includes('in full, because you asked'))",
            timeout=10_000,
        )
        driver.settle(400)
    # The LONGEST message now in the channel. Chosen by measuring rather than by index: the seed is
    # the server's, and an index would silently become a short message the day it changes.
    driver.js(
        "(() => { const items = [...document.querySelectorAll('#discord-log li')]; "
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
    driver.page.click("#reply-probe .reply-button")
    driver.page.wait_for_function("() => window.__visible('screen-reply')", timeout=5_000)
    driver.settle(250)


# Hang up is ABSENT, not disabled, whenever there is no call. Asserted as its own expectation on
# every no-call state, because "dimmed but present" was the defect the rework removed and a
# screenshot is the only thing that can tell the two apart.
NO_HANGUP = ("Hang up is absent, not merely dimmed", "!window.__visible('hang-up')")

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
                "the pulse animation is really running on the ring",
                "document.querySelector('.control-ring').getAnimations().length > 0",
            ),
            (
                "the status line reports a live call",
                "document.getElementById('status-line').dataset.state === 'live'",
            ),
            ("Hang up is present, because there is a call", "window.__visible('hang-up')"),
            (
                "the transcript has turns from both sides, and the empty state is gone",
                "document.querySelectorAll('#transcript li').length >= 3 && "
                "!window.__visible('empty-state')",
            ),
            # `#45 post-call-state`. The voice is labelled "assistant", because the sibling view on
            # this same page is a channel full of coding agents posting under their own names. The
            # word is a rendered fact, so it is pinned where it can be SEEN as well as asserted.
            (
                "the other speaker is labelled 'assistant', not 'agent'",
                "(() => { const w = [...document.querySelectorAll('#transcript li.theirs .who')]; "
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
                "[...document.querySelectorAll('#transcript li.seam .seam-label')]"
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
                "!!document.querySelector('#transcript li.seam details[open]')",
            ),
            (
                "the explanation has actually been laid out",
                "document.querySelector('#transcript li.seam .seam-detail')"
                ".getBoundingClientRect().height > 0",
            ),
            # THE point of this state. The page scrolls the seam into view on toggle; whether that
            # lands is a layout fact, and its first capture caught it opening underneath the dock
            # where nobody can read it.
            (
                "the explanation is not hidden underneath the dock",
                "(() => { const d = document.querySelector('#transcript li.seam .seam-detail'); "
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
                "document.querySelectorAll('#transcript li').length >= 3",
            ),
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
            (
                "the explanation of what the controls do is on screen",
                "window.__visible('continuity-note')",
            ),
        ),
    ),
    Scene(
        name="10-discord-view",
        what="the raw Discord view, with real messages from the server",
        act=_act_discord,
        expect=(
            ("the Discord pane is up", "window.__visible('pane-discord')"),
            (
                "the switch says which view you are in",
                "window.__text('view-switch-label') === 'Discord'",
            ),
            (
                "the switch reads as thrown",
                "document.getElementById('view-switch').getAttribute('aria-checked') === 'true'",
            ),
            (
                "there are several real messages rendered",
                "document.querySelectorAll('#discord-log li').length >= 5",
            ),
            (
                "every message shows its id, so it can be checked against a real one",
                "[...document.querySelectorAll('#discord-log li .msg-id')].length >= 5",
            ),
        ),
    ),
    Scene(
        name="11-long-transcript-scrolled",
        what="a long transcript, parked mid-scroll, exercising the header and dock edges",
        act=_act_long_scroll,
        expect=(
            (
                "the transcript is long",
                "document.querySelectorAll('#transcript li').length >= 15",
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
        ),
    ),
    Scene(
        name="12-collapsed-long-transcript",
        what="the same list with one answer opened: three-line openings, and the one full message",
        act=_act_one_expanded,
        expect=(
            (
                "most of the list is still showing openings only",
                "document.querySelectorAll(\"#transcript li[data-collapsed='true']\").length >= 4",
            ),
            (
                "exactly one message is open, so the comparison is the point of the picture",
                "document.querySelectorAll(\"#transcript li[data-collapsed='false']\").length === 1",
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
        what="a turn arrived while the reader was up in the history: the view held, the chip appeared",
        act=_act_jump_to_newest,
        expect=(
            ("the chip offering the newest line is on screen", "window.__visible('jump-newest')"),
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
                "the controls follow the column rather than spanning the desk",
                "document.getElementById('control-pane').getBoundingClientRect().width <= "
                "document.getElementById('dock').getBoundingClientRect().width - 100",
            ),
            (
                "there is a real transcript in it to judge the line length by",
                "document.querySelectorAll('#transcript li').length >= 6",
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
                "document.querySelectorAll('#transcript li').length >= 6",
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
                "[...document.querySelectorAll('#transcript li.seam .seam-label')]"
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
                "it says where the failure is, not 'see the console'",
                "/between this browser and ElevenLabs/.test("
                "document.getElementById('error').textContent)",
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
        what="answering one channel message, with a draft in the box",
        act=_act_reply_view,
        expect=(
            ("the reply screen is up", "window.__visible('screen-reply')"),
            (
                "it names the message being answered, by id",
                "/id \\d{5,}/.test(window.__text('reply-target-meta'))",
            ),
            (
                "the dock is gone: those controls act on the call, not on this",
                "!window.__visible('control-pane')",
            ),
            ("there is a way back in the header", "window.__visible('close-reply')"),
            ("the draft really is in the box", "document.getElementById('reply-text').value.length > 10"),
            # THE measured claim, and the reason this state parks mid-scroll first: leaving to
            # reply and coming back lands on the same line. A browser is allowed to reset a hidden
            # element's scrollTop to zero, which is exactly what no unit test can reproduce.
            (
                "leaving to reply and coming back landed on the same line",
                "window.__parkedAt > 0 && Math.abs(window.__returnedTo - window.__parkedAt) <= 4",
            ),
        ),
    ),
    Scene(
        name="19-reply-view-long-target",
        what="a target longer than the frame — it scrolls itself, and the box stays reachable",
        act=_act_reply_long_target,
        # PHONES ONLY, and this is a scoping decision rather than an oversight. The property is
        # that a message longer than the frame scrolls inside its own box instead of pushing the
        # reply control off the bottom -- and at 1440x900, with the column capped at 72 characters,
        # a message short enough for Discord to accept simply is not long enough to reach the foot
        # of the screen. Photographing it there would capture a box that happens to fit and call it
        # evidence. State 18 is the desktop picture of this screen.
        profiles=PHONE_PROFILES,
        expect=(
            ("the reply screen is up", "window.__visible('screen-reply')"),
            # The whole content of this picture: the message being answered overflows its own box
            # rather than the screen, so the thing you type into is still there.
            (
                "the message being answered overflows its OWN box",
                "(() => { const t = document.getElementById('reply-target'); "
                "return t.scrollHeight > t.clientHeight + 10; })()",
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
            "the served /voice markup no longer declares viewport-fit=cover, so the page will letterbox "
            "inside the safe area on a phone with a cutout"
        )
    if "width=device-width" not in html:
        problems.append("the served /voice markup no longer declares width=device-width")
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
    url: str, token: str, channel: str, out_dir: Path, only: Iterable[str], themes: Iterable[Theme]
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

    everything: list[tuple[str, Path, Verdict]] = []
    with sync_playwright() as playwright:
        browser = open_browser(playwright)
        try:
            for theme in themes:
                for profile in PROFILES:
                    context = browser.new_context(**profile.context_options(theme))
                    context.grant_permissions(["microphone"], origin=url)
                    # The vendor boundary, and the ONLY request that is faked. Everything else --
                    # /api/v1/client-config, the channel read -- goes to the real server.
                    context.route(
                        "**/api/v1/signed-url",
                        lambda route: route.fulfill(
                            status=200,
                            content_type="application/json",
                            body=json.dumps(signed),
                        ),
                    )
                    page = context.new_page()
                    driver = Driver(page, url, token, out_dir, profile, theme)
                    print(f"==> {theme} · {profile.name}: {profile.what}")
                    for scene in scenes:
                        # A state that does not exist on this device is SKIPPED by name rather
                        # than attempted: the desktop reading column has nothing to photograph on
                        # a phone, and a scene that quietly succeeded there would be filed as
                        # evidence of a layout the phone does not have.
                        if scene.profiles and profile.name not in scene.profiles:
                            print(f"  skip  {scene.name:<30} not a state this profile has")
                            continue
                        # A timeout inside the walk is an UNREACHABLE STATE and must read as one.
                        # Left to itself Playwright raises a TimeoutError whose traceback names a
                        # line number in this file and not the state, which is exactly the report
                        # that sends the next reader to debug the harness when the page has moved.
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
                        name, path, verdict = driver.results[-1]
                        print(f"  ok    {name:<30} {verdict.describe()}")
                    everything.extend(driver.results)
                    context.close()
        finally:
            browser.close()
    return everything


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

    if len(SCENES) < 19:
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
            "screen and the same screen with a target longer than the frame."
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
        # happens to fit and file it as evidence that a long one scrolls.
        "19-reply-view-long-target": PHONE_PROFILES,
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
    required = {
        "01-signed-out": "save-token",
        "02-idle": "empty-state",
        "03-live-call": "Listening",
        "04-muted": "Muted",
        "05-speaker-silenced": "Silent",
        "06-post-call": "new conversation",
        # Pinned to the GEOMETRY, not merely to the element. Pinning it to "seam-detail" was
        # satisfied by the sibling "has it been laid out" check, so the one assertion this state
        # exists for -- that the explanation does not open underneath the dock, which is the bug
        # its first capture caught -- could be deleted with every control still green.
        "07-seam-disclosure-open": "dock.top",
        "08-clear-armed": "Sure?",
        "09-settings": "topbar-title",
        "10-discord-view": "Discord",
        "11-long-transcript-scrolled": "scrollTop",
        # Both pinned to the thing the picture is FOR, not to the screen being up. A line clamp is
        # a rendering fact no behavioural test can reach, so "one is open among the closed ones" is
        # the whole content of state 12; "a turn arrived and the view did not move" is the whole
        # content of state 13.
        "12-collapsed-long-transcript": "data-collapsed",
        "13-jump-to-newest": "jump-newest",
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
        # `#51 reply-view`. 18 is pinned to the ROUND TRIP -- a browser is allowed to reset a
        # hidden element's scrollTop to zero, so "coming back lands on the same line" is a browser
        # fact no unit test can reproduce, and it is the only interesting thing in that frame that
        # is not simply "a form is on screen". 19 is pinned to the target overflowing its own box.
        "18-reply-view": "__returnedTo",
        "19-reply-view-long-target": "scrollHeight > t.clientHeight",
    }
    for name, needle in required.items():
        required_scene = next((s for s in SCENES if s.name == name), None)
        if required_scene is None:
            problems.append(f"the '{name}' state is gone from the scene table.")
            continue
        if not any(needle in expression for _description, expression in required_scene.expect):
            problems.append(
                f"scene '{name}' no longer checks for {needle!r} before capturing, so a run that "
                "never reached that state would still write a picture under its name."
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
    "no state still drives a selector the interface rework retired",
    "the seam state is pinned to the dock geometry, not just to the element",
    "the suspended state is pinned to the ABSENCE of the failure banner",
    "the failed state is pinned to its presence, so silencing errors cannot pass",
    "the reply state is pinned to the scroll round trip, not to the form being on screen",
    "the long-target state is pinned to the target really overflowing its own box",
    "dark is the default theme, and both schemes stay capturable",
    "each theme really sets color_scheme on the browser context",
    "a non-PNG file is rejected",
    "record_capture REFUSES to certify a blank frame it wrote",
    "a rejected capture is still left on disk to look at",
    "record_capture accepts and writes a rendered frame",
    "every state declares at least one expectation before its shutter opens",
    "an unreachable state's error names the state AND the expectation",
    "a phone-class profile with a real device scale factor survives",
    "both a tall and a short phone profile survive",
    "a desktop-width profile survives",
    "more than one desktop-class width survives, so a capped column can be compared",
    "every desktop-class profile really reports a fine pointer",
    "a desktop-only scene names profiles that exist",
    "every state that exists on only some devices is still restricted to them",
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
        + check_profile_controls()
        + check_theme_controls()
        + check_dependency_message_controls()
        + check_viewport_meta_controls()
        + check_stub_controls()
        + check_page_error_controls()
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
        description="Photograph the /voice page in every state that looks different.",
    )
    parser.add_argument("--url", help="base URL of a running gent-talk (NOT the live one on 8080)")
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
        "--self-test",
        action="store_true",
        help="run the controls offline and exit; needs no browser and no server",
    )
    return parser


def main(argv: list[str]) -> int:
    args = build_parser().parse_args(argv)

    if args.self_test:
        import tempfile

        with tempfile.TemporaryDirectory(prefix="gent-talk-shots-selftest-") as tmp:
            return run_self_test(Path(tmp))

    if not args.url:
        print("screenshots.py: --url is required (or use --self-test).", file=sys.stderr)
        return EXIT_USAGE
    if not args.out:
        print("screenshots.py: --out is required.", file=sys.stderr)
        return EXIT_USAGE
    token = os.environ.get("GENT_TALK_WRITE_TOKEN", "")
    if not token:
        print(
            "screenshots.py: GENT_TALK_WRITE_TOKEN is not set. It comes from the environment and "
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
        with urlopen(f"{args.url.rstrip('/')}/voice", timeout=10) as response:
            html = response.read().decode("utf-8", "replace")
    except URLError as error:
        print(f"screenshots.py: could not read {args.url}/voice ({error})", file=sys.stderr)
        return EXIT_SERVER_UNREACHABLE
    for problem in expect_viewport_meta(html):
        print(f"screenshots.py: {problem}", file=sys.stderr)
        return EXIT_UNREACHABLE

    try:
        themes = THEMES if args.theme == "both" else (cast(Theme, args.theme),)
        results = run_captures(args.url, token, args.channel or "", out_dir, args.only, themes)
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
