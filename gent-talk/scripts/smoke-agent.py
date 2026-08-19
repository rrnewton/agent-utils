#!/usr/bin/env python3
"""Drive the REAL voice agent, headless, and prove it actually called this server.

WHY THIS EXISTS
===============

scripts/verify-deployment.sh checks that this server is reachable and correct. On 2026-08-19 the
owner's session failed while every one of those checks passed. The access log shows the whole
handshake succeeding --

    mcp initialize 200 . notifications/initialized 202 . tools/list 200

-- and then ZERO tool calls. The agent said "the tools are showing as out of date" and never
invoked anything. Nothing we ran asserted that a tool is actually CALLED, so nothing went red.

This script closes exactly that gap. It holds a real conversation with the deployed agent and
fails unless a tool invocation appears in THIS SERVER'S OWN access log during the run.

IT COSTS VENDOR MINUTES. It is opt-in, manual, and is not part of any suite. Run it when you want
to see what the owner sees.

HOW IT DRIVES THE CONVERSATION WITHOUT A MICROPHONE
===================================================

ElevenLabs documents a client-to-server event that makes this fully headless:

    {"type": "user_message", "text": "..."}

"triggers the same response flow as spoken user input"
-- https://elevenlabs.io/docs/eleven-agents/customization/events/client-to-server-events

So the test types instead of speaking. Everything else on the wire is byte-identical to what
web/voice.js does in the owner's browser: mint a signed URL from OUR /api/v1/signed-url, open the
WebSocket, send a bare `conversation_initiation_client_data`, answer `ping` with `pong`. The
initiation payload is deliberately BARE rather than carrying a text-only override, because an
override only takes effect if it has been enabled in the agent's security settings, and a
conversation begun differently from the owner's is not the conversation this is meant to observe.
Audio frames are simply ignored.

THE TWO ASSERTIONS
==================

1. THE MECHANISM (primary). A tool line -- digest_channel, read_message or find_message -- must
   appear in this server's access log during the run. This is the direct check for the failure
   above, and it does not depend on interpreting a word of the agent's prose. It is also
   unforgeable from our side: access::tool_call fires ONLY on the MCP path (src/mcp/protocol.rs),
   so nothing this script does over the REST API can plant one.

2. THE OUTCOME (corroboration). A fluent reply about nothing is the failure mode of a confabulating
   agent -- this one has invented a whole channel digest before. So rather than asking for a
   summary, which can only be judged by eye, this asks the agent to RELAY THE MOST RECENT MESSAGE
   AND ITS TIMESTAMP, which is comparable by string. We fetch that message from our own API first
   and assert the reply carries it back.

   THE CHEAP ROUND IS PROOF, not merely confidence, and the argument is structural rather than
   probabilistic. Nothing lets the agent produce today's latest message without invoking a tool:
   the test opens a FRESH conversation, so there is no prior history to draw on; the channel's
   recent content POSTDATES any training data; and every message in it is from the owner, from an
   agent of his, or from the bot, none of which is in the model's context at turn one. So a green
   run means something on its own -- no second conversation, no writing to the channel.

   THE TOKEN ROUND IS AN AUTOMATIC ESCALATION ON FAILURE, not a mode anyone has to choose. It
   exists to diagnose a false NEGATIVE, which is the error that IS possible here: when the cheap
   round fails, either the agent is not really reading, or it read fine and this script's
   substring matching was too strict. Those have OPPOSITE fixes. So a failing cheap round posts a
   `gtverify-<epoch>-<random>` token through OUR OWN write API -- never through the agent -- and
   asks again, requiring it back verbatim. The two outcomes are reported as different things:

     cheap fails, token PASSES  ->  THE TEST is wrong (exit 18). Fix check_relay(), not the agent.
     cheap fails, token FAILS   ->  THE AGENT is not reading (exit 14), now proven.

   A failing run therefore costs roughly twice the vendor minutes of a passing one. That is the
   price of not sending the next session after the wrong system. --nonce forces the token round
   directly, for when you want that evidence on a run that would otherwise pass cheaply.

   AND SOME RUNS ARE REFUSED RATHER THAN DECIDED. If the channel is empty, or its newest message
   has nothing distinctive to match on, the grounding assertion would pass or fail purely on how
   it happens to be written. That is a green that means nothing, or a red that means nothing. Such
   a run stops BEFORE any conversation is opened, with exit 20 -- neither a pass nor a fail --
   and nothing is billed.

   Each assertion ships with its controls, run every time (--self-test runs them alone, offline):
   the log scanner must find a real captured tool line and must NOT fire on a handshake-only log,
   and the relay check must reject a fluent confabulated reply.

WHAT IT WILL NOT DO
===================

It never asks the agent to post. Summarising and relaying are read-only. If the agent's tool calls
arrive with the WRITE credential -- which the access log records -- post_reply was reachable, and
that is reported as a finding.

USAGE
=====

    scripts/smoke-agent.py --url URL --channel SNOWFLAKE [--nonce] [--container NAME] ...

Tokens come from $GENT_TALK_READ_TOKEN / $GENT_TALK_WRITE_TOKEN. They are never accepted on the
command line, because a command line is visible to every process on the box, and never printed.

    scripts/smoke-agent.py --self-test     offline; runs every control and costs nothing.

Normally you get here through the launcher, which already knows the URL, channel and container:

    scripts/run.sh --smoke-agent

EXIT CODES -- a failure says WHICH failure
==========================================

     0  passed
     2  usage or configuration error
    10  mint_failed          could not mint a signed URL from our own /api/v1/signed-url
    11  socket_refused       the conversation WebSocket would not open
    12  no_reply             connected, but no agent reply arrived within the timeout
    13  no_tool_call         the agent replied and called NO tool  <-- 2026-08-19's failure
    14  ungrounded           a tool was called, but the reply does not carry the message back --
                             and the token round did not either, so this is proven
    15  credential_leak      a credential appeared in this script's own output
    16  baseline_failed      could not read the channel ourselves, so grounding is undecidable
    17  control_failed       an assertion's own control failed: the check cannot be trusted
    18  relay_check_too_strict  the agent IS reading -- the token came back -- but this script's
                             matching rejected the cheap round. The defect is HERE, not there.
    20  channel_unusable     REFUSED, neither pass nor fail: the channel is empty or has nothing
                             distinctive to match on, so no run could have concluded anything.
                             No conversation was opened and nothing was billed.
"""

from __future__ import annotations

import argparse
import asyncio
import json
import os
import random
import re
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from dataclasses import dataclass, field
from typing import Any, Iterable, Sequence

# --- exit codes ---------------------------------------------------------------------------------

EXIT_OK = 0
EXIT_USAGE = 2
EXIT_MINT_FAILED = 10
EXIT_SOCKET_REFUSED = 11
EXIT_NO_REPLY = 12
EXIT_NO_TOOL_CALL = 13
EXIT_UNGROUNDED = 14
EXIT_CREDENTIAL_LEAK = 15
EXIT_BASELINE_FAILED = 16
EXIT_CONTROL_FAILED = 17
EXIT_RELAY_TOO_STRICT = 18
#: Neither a pass nor a fail. The channel cannot decide the question, so the run does not pretend
#: to have answered it.
EXIT_CHANNEL_UNUSABLE = 20

#: The read tools whose appearance in the access log proves the agent really reached this server.
#: post_reply is deliberately NOT here: this test never asks for a post, so a post_reply line would
#: be a finding, not a pass.
READ_TOOLS = ("digest_channel", "read_message", "find_message")


class SmokeFailure(Exception):
    """A failure with a specific taxonomy code, so the report can say WHICH failure."""

    def __init__(self, code: int, label: str, detail: str) -> None:
        super().__init__(detail)
        self.code = code
        self.label = label
        self.detail = detail


# --- output, and the credential scan over it ----------------------------------------------------


class Transcript:
    """Everything this script prints, kept so it can be scanned for credentials at the end.

    The scan is the same one scripts/test-run-sh.sh runs over run.sh's output, and it carries the
    same positive control: a "no credential found" result is worthless unless the scanner has been
    shown it can find one in the very text it is scanning.
    """

    def __init__(self) -> None:
        self.lines: list[str] = []

    def emit(self, text: str = "") -> None:
        self.lines.append(text)
        print(text, flush=True)

    def text(self) -> str:
        return "\n".join(self.lines)


def secrets_found(haystack: str, secrets: Iterable[tuple[str, str]]) -> list[str]:
    """Names of the secrets whose VALUE appears in `haystack`.

    `secrets` is (name, value). A blank or absurdly short value is skipped rather than searched
    for: an empty needle matches everywhere and would make every run look like a leak.
    """
    found = []
    for name, value in secrets:
        if value and len(value) >= 8 and value in haystack:
            found.append(name)
    return found


# --- the access log: did a tool actually get called? --------------------------------------------

ANSI = re.compile(r"\x1b\[[0-9;]*m")

#: One access-log tool line, after ANSI stripping. Real captured example:
#:
#:   2026-08-19T12:56:01.953439Z  INFO gent_talk::access: tool tool="digest_channel"
#:   channel="1538907155300745327" credential="read" outcome="ok" reason="-" text_len=-1
#:
#: Anchored on the target and the "tool" message so a `request` or `mcp` line cannot match. The
#: fields after `tool=` are matched individually rather than positionally, so a re-ordering or an
#: added field degrades to "found the tool, not the channel" rather than to silence.
TOOL_LINE = re.compile(r'gent_talk::access:\s+tool\s+tool="(?P<tool>[^"]*)"(?P<rest>.*)$')
FIELD = re.compile(r'(?P<key>[a-z_]+)="(?P<value>[^"]*)"')


def strip_ansi(text: str) -> str:
    """Remove SGR colour codes.

    Load-bearing, not cosmetic: the container logs in colour, so every field name in a raw line is
    wrapped in escape sequences and an anchored grep over it silently matches nothing. A check that
    can only report "no tool was called" is not a check.
    """
    return ANSI.sub("", text)


@dataclass(frozen=True)
class ToolLine:
    """One observed tool invocation."""

    tool: str
    channel: str
    credential: str
    outcome: str
    reason: str


def tool_lines_in(log_text: str) -> list[ToolLine]:
    """Every tool invocation recorded in a slice of access log."""
    out = []
    for raw in strip_ansi(log_text).splitlines():
        match = TOOL_LINE.search(raw)
        if not match:
            continue
        fields = {m.group("key"): m.group("value") for m in FIELD.finditer(match.group("rest"))}
        out.append(
            ToolLine(
                tool=match.group("tool"),
                channel=fields.get("channel", ""),
                credential=fields.get("credential", ""),
                outcome=fields.get("outcome", ""),
                reason=fields.get("reason", ""),
            )
        )
    return out


#: A REAL line, captured from the live container on 2026-08-19 by calling digest_channel over
#: /mcp with the read token. It is the positive control for `tool_lines_in`: if the log format
#: ever drifts -- a renamed field, a different tracing formatter, colour handling changing -- this
#: fixture stops matching and the run fails loudly, instead of the scanner going quietly vacuous
#: and reporting "no tool was called" forever after.
REAL_TOOL_LINE = (
    '2026-08-19T12:56:01.953439Z  INFO gent_talk::access: tool tool="digest_channel" '
    'channel="1538907155300745327" credential="read" outcome="ok" reason="-" text_len=-1'
)

#: The 2026-08-19 failure itself, as the log recorded it: a complete, successful handshake with no
#: tool call anywhere in it. This is the NEGATIVE control -- the scanner must stay silent here, or
#: it is matching something incidental and would have called that morning a pass.
HANDSHAKE_ONLY_LOG = (
    '2026-08-19T12:46:03.194060Z  INFO gent_talk::access: mcp rpc_method="initialize" '
    'credential="write" is_notification=false\n'
    '2026-08-19T12:46:03.194108Z  INFO gent_talk::access: request method="POST" path="/mcp" '
    'credential="write" status=200 millis=0\n'
    '2026-08-19T12:46:03.284917Z  INFO gent_talk::access: mcp '
    'rpc_method="notifications/initialized" credential="write" is_notification=true\n'
    '2026-08-19T12:46:03.284946Z  INFO gent_talk::access: request method="POST" path="/mcp" '
    'credential="write" status=202 millis=0\n'
    '2026-08-19T12:46:03.432390Z  INFO gent_talk::access: mcp rpc_method="tools/list" '
    'credential="write" is_notification=false\n'
    '2026-08-19T12:46:03.432511Z  INFO gent_talk::access: request method="POST" path="/mcp" '
    'credential="write" status=200 millis=0'
)


def check_log_scanner_controls() -> list[str]:
    """Run the log scanner's own controls. Returns a list of failures, empty when healthy."""
    problems = []

    positive = tool_lines_in(REAL_TOOL_LINE)
    if len(positive) != 1 or positive[0].tool != "digest_channel":
        problems.append(
            "positive control: the scanner did not recognise a REAL captured tool line. The "
            "access-log format has drifted away from this script, so 'no tool was called' would "
            f"now be reported for every run. Line: {REAL_TOOL_LINE}"
        )
    elif positive[0].credential != "read" or positive[0].outcome != "ok":
        problems.append(
            "positive control: the scanner matched the tool but not its fields "
            f"(credential={positive[0].credential!r} outcome={positive[0].outcome!r})."
        )

    # The same scanner, over a colour-coded copy of the same line: the container logs in colour,
    # so a scanner that only works on stripped text works only in tests.
    coloured = (
        "\x1b[2m2026-08-19T12:56:01.953439Z\x1b[0m \x1b[32m INFO\x1b[0m "
        "\x1b[2mgent_talk::access\x1b[0m\x1b[2m:\x1b[0m tool \x1b[3mtool\x1b[0m\x1b[2m=\x1b[0m"
        '"digest_channel" \x1b[3mchannel\x1b[0m\x1b[2m=\x1b[0m"1538907155300745327" '
        '\x1b[3mcredential\x1b[0m\x1b[2m=\x1b[0m"read" \x1b[3moutcome\x1b[0m\x1b[2m=\x1b[0m"ok"'
    )
    if not tool_lines_in(coloured):
        problems.append(
            "positive control: the scanner missed a COLOUR-CODED tool line. The container logs in "
            "colour; this is the form it will actually meet."
        )

    if "post_reply" in READ_TOOLS:
        problems.append(
            "READ_TOOLS contains post_reply. This test never asks the agent to post, so a post "
            "must never be what satisfies the primary assertion: that would make a run pass on the "
            "strength of the one thing it is supposed to stay away from."
        )
    if [t for t in tool_lines_in(REAL_TOOL_LINE) if t.tool in READ_TOOLS] == []:
        problems.append(
            "positive control: a real digest_channel line was parsed but does not count as one of "
            f"the read tools. READ_TOOLS is {READ_TOOLS!r} and would never fire."
        )
    if [t for t in tool_lines_in(HANDSHAKE_ONLY_LOG) if t.tool in READ_TOOLS]:
        problems.append(
            "negative control: a handshake-only log matched one of READ_TOOLS. The primary "
            "assertion would pass on a run where nothing was invoked."
        )

    if tool_lines_in(HANDSHAKE_ONLY_LOG):
        problems.append(
            "negative control: the scanner reported a tool call in a handshake-only log. That log "
            "IS the 2026-08-19 failure, so the check would have passed the very run it exists to "
            "catch."
        )

    return problems


def mcp_connection_summary(diagnostics: Sequence[str]) -> str | None:
    """One line about what the VENDOR thinks of our MCP server, from its own status events.

    This exists because of what the 2026-08-19 failure actually looked like from both sides at
    once. The agent said the tools "appear to be out of date"; ElevenLabs, in the same
    conversation, reported the server connected with all five tools visible. Those two facts
    together move the investigation off the MCP server and onto the agent's own configuration --
    and neither one alone would have.

    The event's field shape is NOT documented by the vendor, so every access is guarded and an
    unrecognised payload yields None rather than a confident wrong summary. The raw payloads are
    printed alongside this regardless, so nothing is lost if the shape drifts.
    """
    for raw in diagnostics:
        try:
            payload = json.loads(raw)
        except json.JSONDecodeError:
            continue
        status = payload.get("mcp_connection_status")
        if not isinstance(status, dict):
            continue
        integrations = status.get("integrations")
        if not isinstance(integrations, list) or not integrations:
            continue
        parts = []
        for entry in integrations:
            if not isinstance(entry, dict):
                continue
            connected = entry.get("is_connected")
            count = entry.get("tool_count")
            if connected is None and count is None:
                continue
            parts.append(
                f"{entry.get('integration_id', 'an MCP server')}: "
                f"{'CONNECTED' if connected else 'NOT connected'}"
                + (f", {count} tool(s) visible" if count is not None else "")
            )
        if parts:
            return "; ".join(parts)
    return None


# --- the relay assertion: is the reply grounded in the channel? ----------------------------------

#: A fluent, confident, entirely invented reply. The NEGATIVE control for the relay check: an agent
#: that read nothing still produces prose like this, and a grep that passes on it is matching
#: something incidental rather than the message.
CONFABULATED_REPLY = (
    "Sure! Looking at the channel now. The most recent message is from earlier today -- someone "
    "was discussing the deployment and mentioned that things are looking good on their end. There "
    "were a few follow-ups about scheduling and one about the weekend. Let me know if you would "
    "like me to go further back or read any of them out in full!"
)

WORD = re.compile(r"[0-9A-Za-z][0-9A-Za-z'_-]*")
WHITESPACE = re.compile(r"\s+")


def normalize(text: str) -> str:
    """Lowercase with runs of whitespace collapsed, for substring comparison."""
    return WHITESPACE.sub(" ", text).strip().lower()


def despace(text: str) -> str:
    """Only the alphanumerics, lowercased.

    A nonce read back by a speech-shaped model can arrive with punctuation or spacing of its own
    ("gtverify - 1755..."). Matching on this is weaker than verbatim, so which one matched is
    reported rather than flattened away.
    """
    return re.sub(r"[^0-9a-z]", "", text.lower())


def timestamp_forms(iso: str) -> list[str]:
    """Renderings of an ISO-8601 timestamp an agent might plausibly speak it back as.

    Matching is to the MINUTE. A minute is already far beyond what a model can invent -- there are
    1440 of them in a day and the reply would have to land on the right one -- while insisting on
    the second would fail honestly-grounded replies that round, which is a false alarm, not rigour.
    """
    match = re.match(r"(\d{4})-(\d{2})-(\d{2})[T ](\d{2}):(\d{2})", iso)
    if not match:
        return []
    year, month, day, hour24, minute = match.groups()
    hour = int(hour24)
    hour12 = hour % 12 or 12
    forms = [
        f"{year}-{month}-{day}t{hour24}:{minute}",
        f"{year}-{month}-{day} {hour24}:{minute}",
        f"{hour24}:{minute}",
        f"{hour12}:{minute}",
    ]
    # Deduplicated, longest first, so the report names the most specific form that matched.
    return sorted(dict.fromkeys(forms), key=len, reverse=True)


def body_evidence(content: str, minimum_length: int = 6) -> list[str]:
    """Substrings of a message body distinctive enough that quoting one implies having read it.

    Longest first: a rare long word is much harder to produce by chance than a common short one.
    A phrase of consecutive words is included as well, because a relayed message is usually quoted
    in runs rather than word by word.
    """
    words = WORD.findall(content)
    candidates: list[str] = []

    phrase_words = [w for w in words if w]
    if len(phrase_words) >= 4:
        candidates.append(" ".join(phrase_words[:6]))

    long_words = sorted({w for w in words if len(w) >= minimum_length}, key=len, reverse=True)
    candidates.extend(long_words[:8])

    seen: set[str] = set()
    out = []
    for candidate in candidates:
        key = normalize(candidate)
        if key and key not in seen:
            seen.add(key)
            out.append(candidate)
    return sorted(out, key=len, reverse=True)


@dataclass
class RelayVerdict:
    """What the reply carried back, and how."""

    timestamp_hit: str | None = None
    body_hit: str | None = None
    nonce_hit: str | None = None
    nonce_hit_kind: str | None = None  # "verbatim" or "despaced"

    @property
    def grounded(self) -> bool:
        """A nonce alone settles it; without one, both halves of the relay are required."""
        if self.nonce_hit:
            return True
        return bool(self.timestamp_hit and self.body_hit)


def check_relay(reply: str, timestamp: str, content: str, nonce: str | None) -> RelayVerdict:
    """Decide whether `reply` really carries the channel message back."""
    haystack = normalize(reply)
    verdict = RelayVerdict()

    for form in timestamp_forms(timestamp):
        if form in haystack:
            verdict.timestamp_hit = form
            break

    for evidence in body_evidence(content):
        if normalize(evidence) in haystack:
            verdict.body_hit = evidence
            break

    if nonce:
        if normalize(nonce) in haystack:
            verdict.nonce_hit, verdict.nonce_hit_kind = nonce, "verbatim"
        elif despace(nonce) in despace(reply):
            verdict.nonce_hit, verdict.nonce_hit_kind = nonce, "despaced"

    return verdict


def check_relay_controls() -> list[str]:
    """Run the relay check's own controls. Returns a list of failures, empty when healthy."""
    problems = []
    timestamp = "2026-08-19T12:45:33.412000+00:00"
    content = "Rebuilt the container after the sparkplug refactor; digest looks right now."
    nonce = "gtverify-1755607000-a3f9c1"

    truthful = (
        "The most recent message, at 12:45 on 2026-08-19, says: Rebuilt the container after the "
        "sparkplug refactor; digest looks right now."
    )
    verdict = check_relay(truthful, timestamp, content, None)
    if not verdict.grounded:
        problems.append(
            "positive control: the relay check rejected a reply that DOES carry the message back "
            f"(timestamp_hit={verdict.timestamp_hit!r} body_hit={verdict.body_hit!r}). It would "
            "now fail every honest run."
        )

    timestamp_only = "The most recent message came in at 12:45; it was about the usual things."
    verdict = check_relay(timestamp_only, timestamp, content, None)
    if verdict.grounded:
        problems.append(
            "negative control: a reply carrying the TIMESTAMP but nothing of the message body was "
            "accepted as a relay. Half the evidence is a description, not the message."
        )
    body_only = "Someone said they rebuilt the container after the sparkplug refactor, some time ago."
    verdict = check_relay(body_only, timestamp, content, None)
    if verdict.grounded:
        problems.append(
            "negative control: a reply carrying part of the BODY but no timestamp was accepted as "
            "a relay."
        )

    # Case and spacing are the model's to choose, not evidence about whether it read anything. A
    # matcher that insists on the channel's exact rendering produces false negatives, which is the
    # error the token round exists to diagnose -- better not to manufacture them here.
    ragged = (
        "THE MOST RECENT MESSAGE  (2026-08-19T12:45)  READS:   REBUILT THE CONTAINER AFTER THE\n"
        "SPARKPLUG REFACTOR; DIGEST LOOKS RIGHT NOW."
    )
    verdict = check_relay(ragged, timestamp, content, None)
    if not verdict.grounded:
        problems.append(
            "positive control: a truthful relay was rejected over CAPITALISATION and line breaks "
            f"(timestamp_hit={verdict.timestamp_hit!r} body_hit={verdict.body_hit!r}). The matcher "
            "is comparing presentation rather than content."
        )

    verdict = check_relay(CONFABULATED_REPLY, timestamp, content, None)
    if verdict.grounded:
        problems.append(
            "negative control: the relay check PASSED a fluent, entirely invented reply "
            f"(timestamp_hit={verdict.timestamp_hit!r} body_hit={verdict.body_hit!r}). It is "
            "matching something incidental, not the message."
        )

    verdict = check_relay(CONFABULATED_REPLY + f" The token was {nonce}.", timestamp, content, nonce)
    if not (verdict.grounded and verdict.nonce_hit_kind == "verbatim"):
        problems.append("positive control: a verbatim nonce in the reply was not recognised.")

    spaced = f"The token reads {nonce[:9]} - {nonce[9:]}, and that is the latest message."
    verdict = check_relay(spaced, timestamp, content, nonce)
    if not (verdict.grounded and verdict.nonce_hit_kind == "despaced"):
        problems.append(
            "positive control: a nonce broken up by punctuation was not recognised by the despaced "
            f"fallback (kind={verdict.nonce_hit_kind!r}). A model that reads a token aloud would be "
            "called ungrounded for punctuating it."
        )

    verdict = check_relay(CONFABULATED_REPLY, timestamp, content, nonce)
    if verdict.grounded:
        problems.append(
            "negative control: a reply with no nonce in it was reported as carrying the nonce."
        )

    # A body with nothing distinctive in it must yield no evidence, so the run says "ask for
    # --nonce" rather than silently asserting nothing and passing.
    if body_evidence("ok"):
        problems.append("negative control: 'ok' was treated as distinctive channel content.")

    return problems


def check_credential_scanner_controls() -> list[str]:
    """Run the credential scanner's own controls.

    These live here, offline, rather than only inside a paid run: a scanner that has quietly
    stopped finding anything reports a clean output forever, and the only signal would have
    been a leak nobody was told about.
    """
    problems = []
    token = "SENTINEL-WRITE-TOKEN-2c98da44"
    if secrets_found(f"prefix {token} suffix", [("write", token)]) != ["write"]:
        problems.append(
            "positive control: the credential scanner did not find a token planted in the "
            "text it was given. Every 'no credential in the output' result it produces is "
            "meaningless."
        )
    if secrets_found("nothing to see here", [("write", token)]):
        problems.append("negative control: the scanner reported a token that is absent.")
    if secrets_found("anything at all", [("blank", "")]):
        problems.append(
            "negative control: a BLANK secret matched. An empty needle is in every haystack, "
            "so an unset variable would make every run look like a leak."
        )
    return problems


#: The real payload ElevenLabs sent during the failing run of 2026-08-19, captured verbatim.
#: It is the positive control for `mcp_connection_summary`: the event's shape is undocumented, so
#: a drift would otherwise turn the summary silently into None and take the sharpest diagnostic
#: this script has with it.
REAL_MCP_STATUS = (
    '{"mcp_connection_status": {"integrations": [{"integration_id": "QGRdu0v2Ww1tPg9H2QUZ", '
    '"integration_type": "mcp_server", "is_connected": true, "tool_count": 5}]}, '
    '"type": "mcp_connection_status"}'
)


def check_diagnostics_controls() -> list[str]:
    """Run the MCP-status reader's own controls."""
    problems = []
    summary = mcp_connection_summary([REAL_MCP_STATUS])
    if not summary or "CONNECTED" not in summary or "5 tool" not in summary:
        problems.append(
            "positive control: the MCP-status reader did not understand a REAL captured "
            f"mcp_connection_status payload (got {summary!r}). The vendor does not document this "
            "event's shape, so a drift here silently removes the diagnostic that says whether the "
            "connection is the problem."
        )
    if mcp_connection_summary(['{"type": "ping"}', 'not json at all']) is not None:
        problems.append(
            "negative control: the MCP-status reader invented a summary from events that carry "
            "no connection status."
        )
    return problems


# --- talking to our own server ------------------------------------------------------------------


def http_json(
    url: str, token: str, method: str = "GET", payload: dict[str, Any] | None = None, timeout: int = 30
) -> tuple[int, Any]:
    """One request to our own API. Returns (status, parsed-body-or-raw-text)."""
    data = json.dumps(payload).encode() if payload is not None else None
    request = urllib.request.Request(url, data=data, method=method)
    request.add_header("authorization", f"Bearer {token}")
    request.add_header("accept", "application/json")
    if data is not None:
        request.add_header("content-type", "application/json")
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            body = response.read().decode("utf-8", "replace")
            status = response.status
    except urllib.error.HTTPError as error:
        body = error.read().decode("utf-8", "replace")
        status = error.code
    except (urllib.error.URLError, OSError) as error:
        raise SmokeFailure(
            EXIT_BASELINE_FAILED,
            "unreachable",
            f"could not reach {url}: {error}. Is the container running, and is --url right?",
        ) from error
    try:
        return status, json.loads(body)
    except json.JSONDecodeError:
        return status, body


# --- reading this server's log --------------------------------------------------------------------


def read_log(command: Sequence[str]) -> str:
    """The server's log so far, as text. stderr is included: tracing writes there."""
    try:
        finished = subprocess.run(command, capture_output=True, timeout=60, check=False)
    except (OSError, subprocess.SubprocessError) as error:
        raise SmokeFailure(
            EXIT_CONTROL_FAILED,
            "log_unreadable",
            f"could not read the server log with {' '.join(command)}: {error}.\n"
            "The tool-call assertion is THE primary check here, so an unreadable log is a failure "
            "rather than something to continue past. Pass --log-command if the server is not the "
            "container this expects, or --no-log-check to run with the primary assertion off.",
        ) from error
    output = finished.stdout.decode("utf-8", "replace") + finished.stderr.decode("utf-8", "replace")
    if finished.returncode != 0 and not output.strip():
        raise SmokeFailure(
            EXIT_CONTROL_FAILED,
            "log_unreadable",
            f"{' '.join(command)} exited {finished.returncode} and printed nothing.",
        )
    return output


# --- the conversation -----------------------------------------------------------------------------


@dataclass
class Conversation:
    """What came back over the WebSocket."""

    conversation_id: str | None = None
    responses: list[str] = field(default_factory=list)
    responses_after_ask: list[str] = field(default_factory=list)
    mcp_tool_calls: list[str] = field(default_factory=list)
    #: Raw payloads of the events that say WHY a tool call did not happen. The vendor documents
    #: `mcp_connection_status` by name but not by shape, so these are kept verbatim rather than
    #: picked apart into fields that may not exist -- an unparsed payload an operator can read
    #: beats a parsed one that silently comes back empty.
    diagnostics: list[str] = field(default_factory=list)
    event_types: dict[str, int] = field(default_factory=dict)
    seconds: float = 0.0
    close_code: int | None = None

    @property
    def reply(self) -> str:
        """Everything the agent said after we asked, as one blob to assert over."""
        return "\n".join(self.responses_after_ask)


async def converse(signed_url: str, ask: str, max_seconds: float, quiet_seconds: float) -> Conversation:
    """Hold one text-driven conversation and come straight back out of it.

    The wire protocol here is deliberately the same one web/voice.js speaks, minus the microphone:
    bare `conversation_initiation_client_data`, `pong` for every `ping`, audio ignored. The one
    addition is `user_message`, which the vendor documents as triggering the same response flow as
    speech.
    """
    try:
        import websockets
    except ImportError as error:
        raise SmokeFailure(
            EXIT_USAGE,
            "missing_dependency",
            "the 'websockets' package is not installed, and this script cannot open a conversation "
            "without it. Install it with:\n"
            "    python3 -m pip install websockets\n"
            "(This is the only dependency beyond the standard library.)",
        ) from error

    conversation = Conversation()
    started = time.monotonic()
    deadline = started + max_seconds

    def note(event_type: str) -> None:
        conversation.event_types[event_type] = conversation.event_types.get(event_type, 0) + 1

    try:
        socket = await websockets.connect(signed_url, max_size=None, open_timeout=25, close_timeout=5)
    except Exception as error:  # noqa: BLE001 - any failure to open is the same finding
        raise SmokeFailure(
            EXIT_SOCKET_REFUSED,
            "socket_refused",
            f"the conversation WebSocket would not open: {type(error).__name__}: {error}\n"
            "The signed URL was minted, so this server and your write token are fine; the failure "
            "is between this host and ElevenLabs. A signed URL is good for 15 minutes and for one "
            "conversation.",
        ) from error

    asked_at: float | None = None
    last_response_at: float | None = None
    try:
        await socket.send(json.dumps({"type": "conversation_initiation_client_data"}))

        while True:
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                break
            try:
                raw = await asyncio.wait_for(socket.recv(), timeout=min(remaining, 2.0))
            except asyncio.TimeoutError:
                raw = None
            except Exception:  # noqa: BLE001 - a closed socket ends the loop, not the run
                break

            if raw is not None:
                try:
                    message = json.loads(raw) if isinstance(raw, str) else {}
                except json.JSONDecodeError:
                    message = {}
                kind = message.get("type", "")
                if kind:
                    note(kind)

                if kind == "conversation_initiation_metadata":
                    meta = message.get("conversation_initiation_metadata_event") or {}
                    conversation.conversation_id = meta.get("conversation_id")
                elif kind == "ping":
                    event_id = (message.get("ping_event") or {}).get("event_id")
                    await socket.send(json.dumps({"type": "pong", "event_id": event_id}))
                elif kind == "agent_response":
                    text = (message.get("agent_response_event") or {}).get("agent_response") or ""
                    if text.strip():
                        conversation.responses.append(text)
                        if asked_at is not None:
                            conversation.responses_after_ask.append(text)
                        last_response_at = time.monotonic()
                elif kind == "agent_response_correction":
                    event = message.get("agent_response_correction_event") or {}
                    text = event.get("corrected_agent_response") or ""
                    if text.strip():
                        conversation.responses.append(text)
                        if asked_at is not None:
                            conversation.responses_after_ask.append(text)
                        last_response_at = time.monotonic()
                elif kind in ("mcp_connection_status", "client_error", "guardrail_triggered"):
                    conversation.diagnostics.append(json.dumps(message)[:1200])
                elif kind == "mcp_tool_call":
                    call = message.get("mcp_tool_call") or {}
                    name = call.get("tool_name")
                    if name:
                        conversation.mcp_tool_calls.append(str(name))

            # Ask once the conversation is actually up. `user_activity` first, which the vendor
            # documents as resetting the turn timeout -- the agent must not start talking over the
            # question while it is being delivered.
            if asked_at is None and conversation.conversation_id is not None:
                await socket.send(json.dumps({"type": "user_activity"}))
                await socket.send(json.dumps({"type": "user_message", "text": ask}))
                asked_at = time.monotonic()
                continue

            # Leave as soon as the answer has settled. Every extra second on this socket is a
            # second of vendor time being spent to learn nothing.
            if (
                asked_at is not None
                and conversation.responses_after_ask
                and last_response_at is not None
                and time.monotonic() - last_response_at >= quiet_seconds
            ):
                break
    finally:
        # Promptly, and on every path out of here including a failure above: an abandoned socket
        # is an open conversation, and an open conversation is being billed.
        try:
            await socket.close()
        except Exception:  # noqa: BLE001
            pass
        conversation.close_code = getattr(socket, "close_code", None)
        conversation.seconds = time.monotonic() - started

    return conversation


def mcp_verdict(conversation: Conversation) -> str:
    """What the vendor's own status events add to a no-tool-call failure."""
    if not conversation.diagnostics:
        return (
            "\n  The vendor sent no mcp_connection_status or client_error event, so it has "
            "nothing to add about why."
        )
    summary = mcp_connection_summary(conversation.diagnostics)
    lines = ["\n  What ELEVENLABS said about our MCP server in this same conversation:"]
    if summary:
        lines.append(f"    {summary}")
        if "CONNECTED" in summary and "NOT connected" not in summary:
            lines.append(
                "    => THE CONNECTION IS FINE AND THE TOOLS ARE VISIBLE TO THE PLATFORM. The "
                "agent's own words about the tools being unavailable are not describing a "
                "connection problem, whatever they sound like. Do not go looking for a fault in "
                "gent-talk or in the MCP endpoint on this evidence: look at the AGENT's "
                "configuration -- its prompt, its tool selection, and its tool-approval mode -- "
                "for why a connected tool was never invoked."
            )
    lines.append("    raw:")
    lines.extend(f"      {payload}" for payload in conversation.diagnostics)
    return "\n".join(lines)


# --- the run --------------------------------------------------------------------------------------

DEFAULT_ASK = (
    "Please read the most recent message in the channel using your tools, and relay it back to me "
    "in full. Quote the message text exactly as it is written, and give me its timestamp. Do not "
    "post anything to the channel, and do not summarise -- I want the message itself."
)


def make_nonce() -> str:
    return f"gtverify-{int(time.time())}-{random.randrange(16**6):06x}"


@dataclass
class Latest:
    """The most recent message in the channel, as WE read it, before the agent says anything."""

    author: str
    timestamp: str
    content: str

    @property
    def assertable(self) -> bool:
        """Whether there is enough here to build a check that could actually fail."""
        return bool(body_evidence(self.content) and timestamp_forms(self.timestamp))


@dataclass
class Round:
    """One conversation, and everything asserted about it."""

    name: str
    nonce: str | None
    latest: Latest
    conversation: Conversation
    tool_lines: list[ToolLine] = field(default_factory=list)
    verdict: RelayVerdict = field(default_factory=RelayVerdict)
    log_checked: bool = True


class Runner:
    """The steps of a run, sharing the connection details so main() reads as the procedure it is."""

    def __init__(self, args: argparse.Namespace, out: Transcript, read_token: str, write_token: str):
        self.args = args
        self.out = out
        self.read_token = read_token
        self.write_token = write_token
        self.base = args.url.rstrip("/")
        self.log_command = (
            args.log_command.split() if args.log_command else [args.engine, "logs", args.container]
        )
        self.signed_urls: list[str] = []
        self.rounds: list[Round] = []

    # -- pieces ----------------------------------------------------------------------------------

    def channel_path(self, suffix: str) -> str:
        return f"{self.base}/api/v1/channels/{urllib.parse.quote(self.args.channel)}/{suffix}"

    def read_latest(self) -> Latest:
        """Read the channel ourselves. This is the ground truth the agent is measured against."""
        status, body = http_json(self.channel_path("messages?limit=5"), self.read_token)
        if status != 200 or not isinstance(body, dict):
            raise SmokeFailure(
                EXIT_BASELINE_FAILED,
                "baseline_failed",
                f"reading the channel ourselves returned {status}: {body}. Without our own copy of "
                "the message there is nothing to compare the agent's reply against.",
            )
        messages = body.get("messages") or []
        if not messages:
            # A REFUSAL, not a verdict. With nothing to relay, the grounding assertion has nothing
            # to match: depending only on how it happens to be written it would trivially pass or
            # trivially fail, and either answer would be meaningless. So this stops before a
            # conversation is opened, with a status that is neither pass nor fail.
            raise SmokeFailure(
                EXIT_CHANNEL_UNUSABLE,
                "channel_empty",
                "the channel came back EMPTY, so there is no most-recent message for the agent to "
                "relay and nothing for the check to match. Refusing to run rather than reporting a "
                "pass or a fail that would mean nothing. No conversation was opened and nothing "
                "was billed.\n"
                "If the channel really does have messages, the usual cause is that Message Content "
                "Intent is not enabled for this bot in the Discord Developer Portal (Bot → "
                "Privileged Gateway Intents) — without it Discord delivers every message with "
                "blank content.",
            )
        latest = messages[-1]
        return Latest(
            author=str(latest.get("author") or "?"),
            timestamp=str(latest.get("timestamp") or ""),
            content=str(latest.get("content") or ""),
        )

    def post_nonce(self) -> tuple[str, Latest]:
        """Put an uninventable token in the channel, through OUR write API and never the agent."""
        nonce = make_nonce()
        status, body = http_json(
            self.channel_path("reply"),
            self.write_token,
            method="POST",
            payload={
                "text": f"gent-talk agent smoke test {nonce} — posted by the smoke test itself, "
                "not by the agent. Safe to delete."
            },
        )
        if status != 200:
            raise SmokeFailure(
                EXIT_BASELINE_FAILED,
                "nonce_post_failed",
                f"posting the token returned {status}: {body}. The channel is probably configured "
                "read-only ('ro'); the nonce round needs 'rw'.",
            )
        time.sleep(2)  # let Discord settle before reading it back
        latest = self.read_latest()
        if nonce not in latest.content:
            raise SmokeFailure(
                EXIT_BASELINE_FAILED,
                "nonce_post_failed",
                "the token we posted is not the most recent message in the channel. Somebody else "
                f"posted in the meantime, or the post did not land. Latest is from "
                f"{latest.author!r} at {latest.timestamp}.",
            )
        self.out.emit(f"  ok    posted a unique token to the channel through OUR write API: {nonce}")
        return nonce, latest

    def tools_offered(self, token: str) -> int | None:
        """How many tools this server offers a given credential. None if it cannot be asked."""
        status, body = http_json(
            f"{self.base}/mcp",
            token,
            method="POST",
            payload={"jsonrpc": "2.0", "id": 1, "method": "tools/list"},
        )
        if status != 200 or not isinstance(body, dict):
            return None
        tools = (body.get("result") or {}).get("tools")
        return len(tools) if isinstance(tools, list) else None

    def report_agent_scope(self, conversation: Conversation) -> None:
        """Say which of our credentials the agent is holding, and so whether it could post.

        This is worth the two extra requests because it is the ONE way to answer the question on
        a run where the agent invokes nothing: with no tool line in the access log there is no
        `credential=` field to read, and the credential itself lives inside the agent's
        configuration at the vendor, where we cannot see it. But ElevenLabs tells us how many
        tools it can see, and this server offers a different NUMBER of tools to each scope --
        post_reply is the difference. So the count identifies the credential.

        The counts are asked of this server rather than written down here, so adding or removing
        a tool cannot silently turn this into a confident wrong answer."""
        counts = {
            name: number
            for name, number in (
                ("read", self.tools_offered(self.read_token)),
                ("write", self.tools_offered(self.write_token)),
            )
            if number is not None
        }
        seen: int | None = None
        for raw in conversation.diagnostics:
            try:
                payload = json.loads(raw)
            except json.JSONDecodeError:
                continue
            for entry in (payload.get("mcp_connection_status") or {}).get("integrations") or []:
                if isinstance(entry, dict) and isinstance(entry.get("tool_count"), int):
                    seen = entry["tool_count"]
        if seen is None or len(counts) != 2 or counts["read"] == counts["write"]:
            return
        matches = [name for name, number in counts.items() if number == seen]
        if matches == ["write"]:
            self.out.emit(
                f"  FINDING  ElevenLabs can see {seen} tools, and this server offers "
                f"{counts['write']} to the WRITE credential and {counts['read']} to the read one. "
                "So the agent is configured with the WRITE token and post_reply IS reachable: it "
                "can post in your name. Nothing here asked it to, and the access log shows it did "
                "not. Give it the read token if you want that to be impossible rather than "
                "merely unasked-for."
            )
        elif matches == ["read"]:
            self.out.emit(
                f"  ok    ElevenLabs can see {seen} tools, which is what this server offers the "
                "READ credential. post_reply is not reachable by the agent at all."
            )
        else:
            self.out.emit(
                f"  NOTE  ElevenLabs can see {seen} tools, which matches neither what this server "
                f"offers the read credential ({counts['read']}) nor the write one "
                f"({counts['write']}). The agent may be pointed at a different server, or at a "
                "stale cached manifest."
            )

    def mint(self) -> str:
        status, body = http_json(f"{self.base}/api/v1/signed-url", self.write_token)
        if status != 200 or not isinstance(body, dict) or not body.get("signed_url"):
            raise SmokeFailure(
                EXIT_MINT_FAILED,
                "mint_failed",
                f"GET /api/v1/signed-url returned {status}: {body}\n"
                "503 elevenlabs_not_configured names the setting that is missing; 502 "
                "elevenlabs_error carries what ElevenLabs said. No conversation was attempted.",
            )
        signed_url = body["signed_url"]
        self.signed_urls.append(signed_url)
        host = urllib.parse.urlsplit(signed_url).netloc
        # The URL itself is a bearer credential for the next fifteen minutes. Its HOST is what an
        # operator needs in order to debug a connection failure; the token in it is not.
        self.out.emit(f"  ok    minted a signed URL for agent {body.get('agent_id')} (host {host})")
        return signed_url

    def hold_round(self, name: str, latest: Latest, nonce: str | None) -> Round:
        """Mark the log, hold one conversation, and assert the mechanism worked."""
        log_before = 0
        if not self.args.no_log_check:
            log_before = len(read_log(self.log_command).splitlines())

        signed_url = self.mint()
        self.out.emit("")
        self.out.emit(f"-- the {name} round: holding a conversation (this is what costs money)")
        conversation = asyncio.run(
            converse(signed_url, self.args.ask or DEFAULT_ASK, self.args.max_seconds, self.args.quiet_seconds)
        )
        round_ = Round(name=name, nonce=nonce, latest=latest, conversation=conversation)
        round_.log_checked = not self.args.no_log_check
        self.rounds.append(round_)

        self.out.emit(
            f"  ok    conversation {conversation.conversation_id or '?'} ran "
            f"{conversation.seconds:.1f}s and is closed (code {conversation.close_code})"
        )
        seen = ", ".join(f"{k}x{v}" for k, v in sorted(conversation.event_types.items())) or "none"
        self.out.emit(f"        events seen: {seen}")
        self.report_agent_scope(conversation)

        if not conversation.responses_after_ask:
            raise SmokeFailure(
                EXIT_NO_REPLY,
                "no_reply",
                f"the socket opened and the question was sent, but the agent said nothing back "
                f"within {self.args.max_seconds:.0f}s. Events seen: {seen}. If the agent greeted us "
                "and then went quiet, look at the agent's own logs in the ElevenLabs dashboard for "
                f"conversation {conversation.conversation_id}.",
            )
        for index, text in enumerate(conversation.responses_after_ask, start=1):
            self.out.emit(f"        agent [{index}]: {text.strip()[:400]}")

        # -- THE PRIMARY ASSERTION -------------------------------------------------------------
        self.out.emit("")
        self.out.emit("-- did the agent actually call a tool? (the primary assertion)")
        if self.args.no_log_check:
            self.out.emit(
                "  SKIP  --no-log-check: the mechanism assertion is OFF. This run CANNOT fail the "
                "way 2026-08-19 failed."
            )
            return round_

        after = read_log(self.log_command).splitlines()[log_before:]
        round_.tool_lines = tool_lines_in("\n".join(after))
        read_calls = [t for t in round_.tool_lines if t.tool in READ_TOOLS]
        if not read_calls:
            raise SmokeFailure(
                EXIT_NO_TOOL_CALL,
                "no_tool_call",
                "the agent REPLIED BUT CALLED NO TOOL. This is exactly the 2026-08-19 failure: "
                "fluent prose, an untouched server.\n"
                f"  what it said: {conversation.reply.strip()[:300]}\n"
                f"  new log lines during the round: {len(after)}, of which tool calls: "
                f"{len(round_.tool_lines)}\n"
                "There is deliberately NO nonce escalation from here. Escalating costs a second "
                "billed conversation, and it could not tell you anything: an agent that invokes no "
                "tool at all cannot read a token either, whoever posted it.\n"
                "Look at the agent's MCP server configuration in the ElevenLabs dashboard: on "
                "2026-08-19 the handshake succeeded and the agent then reported its tools as out "
                "of date."
                + mcp_verdict(conversation),
            )
        for call in read_calls:
            self.out.emit(
                f"  ok    {call.tool} on channel {call.channel} with the {call.credential} "
                f"credential → {call.outcome}"
            )
        return round_

    def check_grounding(self, round_: Round) -> bool:
        """Did the reply carry the message back? Reported, not raised: a failure may escalate."""
        round_.verdict = check_relay(
            round_.conversation.reply, round_.latest.timestamp, round_.latest.content, round_.nonce
        )
        self.out.emit("")
        self.out.emit(f"-- is the {round_.name} round's reply grounded in the channel? (corroboration)")
        verdict = round_.verdict
        if verdict.nonce_hit:
            self.out.emit(
                f"  ok    the reply carries back the token we posted, {verdict.nonce_hit_kind} "
                f"({verdict.nonce_hit}). That string exists nowhere but this channel."
            )
        if verdict.timestamp_hit:
            self.out.emit(f"  ok    the reply carries the message's timestamp ({verdict.timestamp_hit})")
        if verdict.body_hit:
            self.out.emit(f"  ok    the reply quotes the message body ({verdict.body_hit!r})")
        if not verdict.grounded:
            self.out.emit(f"  MISS  timestamp: {verdict.timestamp_hit or 'no'}")
            self.out.emit(f"  MISS  body:      {verdict.body_hit or 'no'}")
            if round_.nonce:
                self.out.emit(f"  MISS  token:     {verdict.nonce_hit or 'no'}")
            self.out.emit(f"        what it said: {round_.conversation.reply.strip()[:400]}")
        return verdict.grounded

    def report_findings(self) -> None:
        """Things worth saying that are not pass/fail."""
        self.out.emit("")
        self.out.emit("-- findings")
        observed = [t for round_ in self.rounds for t in round_.tool_lines]
        if not observed:
            self.out.emit("  (no tool calls were observed, so there is nothing to say about scope)")
            return
        if [t for t in observed if t.credential == "write"]:
            self.out.emit(
                "  FINDING  the agent's tool calls arrived with the WRITE credential, so post_reply "
                "was reachable throughout. It was never asked to post, and the log shows it did "
                "not. If you want a read-only agent, give it the read token."
            )
        else:
            self.out.emit(
                "  ok    the agent's tool calls arrived with the READ credential; post_reply was "
                "not reachable."
            )
        posts = [t for t in observed if t.tool == "post_reply"]
        if posts:
            self.out.emit(
                f"  FINDING  post_reply was CALLED {len(posts)} time(s) during a read-only test. "
                "Nothing here asked for that."
            )
        else:
            self.out.emit("  ok    post_reply was not called")
        vendor_side = sorted({name for r in self.rounds for name in r.conversation.mcp_tool_calls})
        if vendor_side:
            self.out.emit(
                "  ok    the vendor also reported the tool call over the socket: "
                + ", ".join(vendor_side)
            )


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        prog="smoke-agent.py",
        description="Hold a real conversation with the deployed voice agent and prove it called us.",
        epilog="COSTS VENDOR MINUTES. Opt-in and manual; never run this from CI.",
    )
    parser.add_argument("--url", help="this server's base URL, e.g. http://127.0.0.1:8080")
    parser.add_argument("--channel", help="channel snowflake to ask about")
    parser.add_argument(
        "--nonce",
        action="store_true",
        help="go straight to the token round instead of waiting for the cheap check to fail. Use "
        "this when you want proof on a run that would otherwise pass cheaply. It always writes one "
        "line to the channel.",
    )
    parser.add_argument("--ask", help="override the question put to the agent")
    parser.add_argument("--container", default="gent-talk", help="container to read the log from")
    parser.add_argument("--engine", default=os.environ.get("GENT_TALK_ENGINE", "podman"))
    parser.add_argument(
        "--log-command",
        help="shell-free override for reading the server log, e.g. 'journalctl -u gent-talk'. "
        "Defaults to '<engine> logs <container>'.",
    )
    parser.add_argument(
        "--no-log-check",
        action="store_true",
        help="run WITHOUT the primary assertion. The run then cannot fail the way 2026-08-19 "
        "failed; it says so, loudly, in the report.",
    )
    parser.add_argument("--max-seconds", type=float, default=90.0, help="hard cap per conversation")
    parser.add_argument(
        "--quiet-seconds",
        type=float,
        default=8.0,
        help="hang up this long after the agent stops talking",
    )
    parser.add_argument(
        "--self-test",
        action="store_true",
        help="run every control offline. No network, no vendor minutes, no conversation.",
    )
    return parser


def run_self_test(out: Transcript) -> int:
    out.emit("smoke-agent self-test — the controls only. No network, no vendor minutes.")
    out.emit("")
    problems = (
        check_log_scanner_controls()
        + check_relay_controls()
        + check_credential_scanner_controls()
        + check_diagnostics_controls()
    )
    checks = [
        "the log scanner finds a REAL captured tool line",
        "the log scanner finds a COLOUR-CODED tool line",
        "the log scanner stays silent on the 2026-08-19 handshake-only log",
        "post_reply is not something the primary assertion can be satisfied by",
        "a real tool line counts as one of the read tools",
        "a handshake-only log matches none of the read tools",
        "the relay check accepts a reply that carries the message back",
        "the relay check REJECTS a reply carrying only the timestamp",
        "the relay check REJECTS a reply carrying only part of the body",
        "the relay check accepts a truthful relay in a different case and spacing",
        "the relay check REJECTS a fluent confabulated reply",
        "the relay check recognises a verbatim nonce",
        "the relay check recognises a nonce broken up by punctuation",
        "the relay check does not invent a nonce that is not there",
        "an undistinctive body yields no evidence to assert on",
        "the credential scanner finds a planted token",
        "the credential scanner does not report a token that is absent",
        "a blank secret does not match everything",
        "the MCP-status reader understands a real captured payload",
        "the MCP-status reader invents nothing from unrelated events",
    ]
    if problems:
        for problem in problems:
            out.emit(f"  FAIL  {problem}")
        out.emit("")
        out.emit(f"FAILED — {len(problems)} control(s) failed. The checks cannot be trusted.")
        return EXIT_CONTROL_FAILED
    for check in checks:
        out.emit(f"  ok    {check}")
    out.emit("")
    out.emit(f"PASSED — {len(checks)} controls")
    return EXIT_OK


def main(argv: list[str]) -> int:  # noqa: PLR0911, PLR0912, PLR0915 - one linear procedure
    out = Transcript()
    args = build_parser().parse_args(argv)

    if args.self_test:
        return run_self_test(out)

    read_token = os.environ.get("GENT_TALK_READ_TOKEN", "")
    write_token = os.environ.get("GENT_TALK_WRITE_TOKEN", "")
    elevenlabs_key = os.environ.get("GENT_TALK_ELEVENLABS_API_KEY", "")

    if not args.url or not args.channel:
        out.emit("smoke-agent: --url and --channel are both required (or use --self-test).")
        return EXIT_USAGE
    if not write_token:
        out.emit(
            "smoke-agent: GENT_TALK_WRITE_TOKEN is not set. Minting a signed URL requires the "
            "WRITE scope, because the conversation it opens reaches an agent that can post."
        )
        return EXIT_USAGE
    if not read_token:
        out.emit("smoke-agent: GENT_TALK_READ_TOKEN is not set; it is needed to read the channel.")
        return EXIT_USAGE

    runner = Runner(args, out, read_token, write_token)
    wall_started = time.time()
    result, label, summary = EXIT_OK, "PASSED", ""

    out.emit("gent-talk agent smoke test — a REAL conversation, so a REAL cost.")
    out.emit(f"   target:   {runner.base}")
    out.emit(f"   channel:  {args.channel}")
    out.emit(
        "   log:      NOT CHECKED (--no-log-check)"
        if args.no_log_check
        else f"   log:      {' '.join(runner.log_command)}"
    )
    out.emit("")

    try:
        # -- 0. the controls, before anything is spent -------------------------------------------
        # If a check cannot be trusted, that is worth knowing BEFORE a conversation is paid for.
        out.emit("-- the checks' own controls")
        problems = (
            check_log_scanner_controls()
            + check_relay_controls()
            + check_credential_scanner_controls()
            + check_diagnostics_controls()
        )
        if problems:
            for problem in problems:
                out.emit(f"   {problem}")
            raise SmokeFailure(
                EXIT_CONTROL_FAILED,
                "control_failed",
                f"{len(problems)} control(s) failed; nothing below would have meant anything.",
            )
        out.emit("  ok    the log scanner finds a real tool line and ignores a handshake-only log")
        out.emit("  ok    the relay check accepts a grounded reply and rejects a confabulated one")
        out.emit("  ok    the credential scanner finds a planted token and not an absent one")
        out.emit("  ok    the MCP-status reader understands a real payload and invents nothing")

        status, _ = http_json(f"{runner.base}/healthz", "")
        if status != 200:
            raise SmokeFailure(
                EXIT_BASELINE_FAILED, "unreachable", f"GET /healthz returned {status}, expected 200."
            )
        out.emit("  ok    the server is up")

        # -- 1. the cheap round: read the channel ourselves, then ask -----------------------------
        #
        # THE CHEAP ROUND IS PROOF, NOT MERELY CONFIDENCE, and the reason is structural rather
        # than probabilistic. There is no path by which the agent produces today's latest message
        # without invoking a tool:
        #
        #   * this opens a FRESH conversation, so there is no prior history to draw on;
        #   * the channel's recent content POSTDATES any training data;
        #   * every message in it is from the owner, from an agent of his, or from the bot -- none
        #     of which is in the model's context at turn one.
        #
        # Do not re-weaken this into a hedge. It matters because it is what makes a green run mean
        # something on its own, without spending a second conversation to confirm it.
        #
        # The escalation ladder, because getting it wrong costs the owner money:
        #
        #   cheap passes             -> done. One conversation, nothing written to the channel.
        #   cheap fails on GROUNDING -> escalate to a token round. NOT to rule out a false
        #                               POSITIVE -- those are impossible, per the argument above --
        #                               but to diagnose a false NEGATIVE: if the agent really did
        #                               read the channel and our substring matcher was simply too
        #                               strict, the token separates "the agent is not reading" from
        #                               "our check is wrong". That is a question about the quality
        #                               of THIS SCRIPT, not about the agent's honesty, and the two
        #                               have opposite fixes.
        #   cheap fails on the TOOL CALL -> STOP. No escalation (see hold_round): an agent that
        #                               invoked nothing cannot read a token either, so a second
        #                               billed conversation would buy no information.
        #   channel empty or undistinctive -> REFUSE before any conversation (see read_latest and
        #                               just below). A check with nothing to match on is not a
        #                               check, and its answer is not evidence either way.
        latest = runner.read_latest()
        out.emit(
            f"  ok    read the channel ourselves: latest message is from {latest.author!r} "
            f"at {latest.timestamp} ({len(latest.content)} chars)"
        )

        if not args.nonce and not latest.assertable:
            raise SmokeFailure(
                EXIT_CHANNEL_UNUSABLE,
                "channel_unusable",
                "the most recent message has nothing distinctive enough to match on "
                f"(body {len(latest.content)} chars, timestamp {latest.timestamp!r}), so the "
                "grounding check would pass or fail on how it happens to be written rather than on "
                "what the agent did. Refusing to run: that is neither a pass nor a fail. No "
                "conversation was opened and nothing was billed.\n"
                "Re-run with --nonce. That posts a token of our own through this server's write "
                "API first, so the check no longer depends on what happens to be in the channel.",
            )
        if args.nonce:
            out.emit("  NOTE  --nonce: going straight to the token round, as asked.")

        if args.nonce:
            nonce, latest = runner.post_nonce()
            round_ = runner.hold_round("token", latest, nonce)
            if runner.check_grounding(round_):
                summary = (
                    "the agent called a tool AND relayed back a token that exists nowhere but this "
                    "channel. It really read it."
                )
            else:
                raise SmokeFailure(
                    EXIT_UNGROUNDED,
                    "ungrounded",
                    "a tool WAS called, but the reply does not carry back the token we posted "
                    f"({nonce}). That string cannot be guessed, so the agent is invoking a tool "
                    "without using what comes back, or it is answering from something other than "
                    "this channel.",
                )
        else:
            round_ = runner.hold_round("cheap", latest, None)
            if runner.check_grounding(round_):
                summary = "the agent called a tool and relayed the channel's latest message back."
            else:
                # -- 2. ESCALATION ---------------------------------------------------------------
                # WHY THIS EXISTS, precisely, because a guard kept for a rationale that does not
                # apply is one the next reader either deletes or preserves while believing
                # something false about it:
                #
                # It is NOT here to rule out a false positive. A cheap-round PASS cannot be faked
                # (fresh conversation, post-training content, nothing of this channel in the
                # model's context) -- see the ladder comment above.
                #
                # It is here for the opposite error. A cheap-round FAIL has two causes with
                # opposite fixes: the agent is not really reading, or the agent read fine and THIS
                # SCRIPT'S substring matching was too strict. The second is a defect in our test.
                # A token nobody could guess tells the two apart, and that is worth one more
                # conversation because otherwise the next session debugs the wrong system.
                out.emit("")
                out.emit(
                    "-- ESCALATING. The cheap check did not find the message in the reply. That "
                    "has two causes with opposite fixes — the agent is not really reading, or our "
                    "substring matching was too strict — so this posts a token nobody could guess "
                    "and asks again. THIS IS A SECOND BILLED CONVERSATION."
                )
                nonce, nonce_latest = runner.post_nonce()
                nonce_round = runner.hold_round("token", nonce_latest, nonce)
                if runner.check_grounding(nonce_round):
                    raise SmokeFailure(
                        EXIT_RELAY_TOO_STRICT,
                        "relay_check_too_strict",
                        "THE AGENT IS FINE; THIS TEST IS NOT.\n"
                        "  the cheap round FAILED: the reply did not contain the timestamp and a "
                        "distinctive piece of the message body.\n"
                        f"  the token round PASSED: the agent relayed back {nonce}, which exists "
                        "nowhere but this channel, so it genuinely read it.\n"
                        "  => the defect is in this script's matching, not in the deployment. Look "
                        "at check_relay()/timestamp_forms()/body_evidence() against what the agent "
                        "actually said below, and loosen what is too fussy. Do NOT go looking for a "
                        "problem in the agent or the MCP server.\n"
                        f"  cheap round said: {round_.conversation.reply.strip()[:300]}\n"
                        f"  token round said: {nonce_round.conversation.reply.strip()[:300]}",
                    )
                raise SmokeFailure(
                    EXIT_UNGROUNDED,
                    "ungrounded",
                    "THE AGENT IS NOT READING THE CHANNEL, and this is now proven rather than "
                    "inferred.\n"
                    "  the cheap round FAILED: the reply did not carry the latest message back.\n"
                    f"  the token round ALSO FAILED: we posted {nonce} through our own write API "
                    "and the agent did not relay it back, though it did invoke a tool.\n"
                    "  => a tool is being called and its result is not reaching the answer. Check "
                    "what the tool returned in the access log, and what the agent was told to do "
                    "with it in its ElevenLabs prompt.\n"
                    f"  cheap round said: {round_.conversation.reply.strip()[:300]}\n"
                    f"  token round said: {nonce_round.conversation.reply.strip()[:300]}",
                )

        runner.report_findings()
    except SmokeFailure as failure:
        # A refusal is deliberately NOT rendered as a failure. "This run could not conclude
        # anything" and "the deployment is broken" are different facts, and collapsing them is how
        # a meaningless red sends someone hunting a bug that is not there.
        word = "REFUSED" if failure.code == EXIT_CHANNEL_UNUSABLE else "FAILED"
        out.emit("")
        out.emit(f"{word} [{failure.label}]")
        out.emit(failure.detail)
        result, label, summary = failure.code, word, failure.label

    # -- always: cost, and the credential scan over everything printed ------------------------------
    wall = time.time() - wall_started
    out.emit("")
    out.emit(f"wall time: {wall:.1f}s")
    total_seconds = sum(r.conversation.seconds for r in runner.rounds)
    if runner.rounds:
        detail = "; ".join(
            f"{r.name} round {r.conversation.seconds:.1f}s"
            + (f" (id {r.conversation.conversation_id})" if r.conversation.conversation_id else "")
            for r in runner.rounds
        )
        out.emit(
            f"conversations: {len(runner.rounds)} — {detail}. {total_seconds:.1f}s of socket time "
            "in total; that is what was billed."
        )
    else:
        out.emit("conversations: none was opened, so no vendor time was spent.")
    posted = [r.nonce for r in runner.rounds if r.nonce]
    if posted:
        out.emit(
            f"channel: this test posted {len(posted)} message(s) — {', '.join(posted)}. "
            "Delete them if you like."
        )
    else:
        out.emit("channel: nothing was written to it.")

    scanned = out.text()
    secrets = [
        ("GENT_TALK_READ_TOKEN", read_token),
        ("GENT_TALK_WRITE_TOKEN", write_token),
        ("GENT_TALK_ELEVENLABS_API_KEY", elevenlabs_key),
    ]
    # A minted URL is itself a bearer credential for fifteen minutes, so every one of them is
    # scanned for exactly like a token.
    secrets += [("a minted signed URL", url) for url in runner.signed_urls]
    leaks = secrets_found(scanned, secrets)
    # Positive control, through the SAME scanner over the SAME text: "no credential found" means
    # nothing unless the scanner has just been shown finding one planted in it.
    if not secrets_found(scanned + "\n" + write_token, [("planted", write_token)]):
        out.emit("")
        out.emit(
            "FAILED [control_failed]: the credential scan is broken — it cannot find a value "
            "planted in the very text it scans, so its clean result means nothing."
        )
        return EXIT_CONTROL_FAILED
    if leaks:
        out.emit("")
        out.emit(f"FAILED [credential_leak]: {', '.join(leaks)} appeared in this script's output.")
        return EXIT_CREDENTIAL_LEAK
    out.emit("no credential value appears anywhere in this output (scanner positive-controlled).")

    out.emit("")
    out.emit(f"{label} — {summary}")
    return result


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
