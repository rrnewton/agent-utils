---
title: 'live-relay-check: an opt-in live check of relays during a call against any vibe-talk-v1 bridge'
status: closed
priority: 2
issue_type: task
labels:
- vibe-talk
- voice
created_at: 2026-10-19T06:02:48.265004409+00:00
updated_at: 2026-10-19T08:31:02.481491417+00:00
closed_at: 2026-10-19T08:31:02.481491277+00:00
---

# Description

[opus 5.5] An opt-in live check of relays during a call (#337 voice-relay-silent, #330 voice-long-turn-drop) against any vibe-talk-v1 bridge. The page suite and the loopback walks prove the page against fakes; what a real bridge does with a relayed prompt (and with an interrupt followed by a relay, the case where a bridge can lose a turn and ask to reconnect, see #352 voice-reconnect-on-bridge-error) is only seen on a real call.

A script under vibe-talk/tests runs a built vibe-talk binary on loopback with a generated config, a fake chat API (it refuses every post), a loopback front that sends /ws to the bridge, and headless Chromium with a fake microphone. It delivers new messages into watched and unwatched channels during real calls, records every page<->bridge frame, and judges each case: relayed exactly once and read out by the agent, or not relayed and the page says why; never relayed in an unwatched channel; never cut into a long answer. Each verdict names where the evidence points (page, server, bridge, harness).

- The bridge address is a required argument; there is no default, and nothing refers to any particular deployment, host, path or log.
- It refuses a bridge address that asks for agent tools (tools=), so a test call can never act on a real chat.
- One call at a time, a call budget, at most two calls per run, loopback only; it stops only the processes it started.
- A fake bridge mode (no network) and an offline self-test of the judge check the harness itself; the fake bridge can lose a turn after an interrupt and ask to reconnect.
- New case f, interrupt then relay: the owner's spoken turn is cut, then a message arrives a second later. PASS when the relay is read out exactly once, either directly or after the page reconnected.
- Not part of the validate gate, since it needs a live bridge. Documented in the README next to the voice bridge protocol.

# Acceptance Criteria

- The check runs against the fake bridge end to end with no network and passes on the current page; fake faults (a relay ignored, a lost turn after an interrupt) give the expected FAIL or reconnect verdicts.
- The judge self-test covers every verdict, including case f's direct and reconnected PASS and its FAILs (call ended at the error, relay dropped silently, relay read out twice).
- It exits with a usage error without a bridge address, and refuses one containing tools=.
- scripts/check_client_names.py and the repository's checks pass; no private path, host, port or name appears.
- The README documents how to run it beside the voice bridge protocol.

# Notes

[opus 5.5] Delivered on branch vt/voice-reconnect in "vibe-talk: an opt-in live check of relays during a call, against any vibe-talk-v1 bridge": scripts/live-relay-check.py, ported from the private harness behind the 2026-10-18 runs. --bridge is required (ws://, wss:// or fake) with no default; tools= is refused; nothing names a deployment, host, path or log. Cases a-e as before, plus f, interrupt then relay, judged across every socket the call used: read out exactly once, directly or after the page reconnected, or the call ended by a second lost turn saying "not read out"; a lost turn is blamed on the bridge on every verdict. Reading a message out in two turns now fails any case. --replay enables the server's replay and the Resuming switch. The live run is opt-in (make live-relay-check BRIDGE=...) and not in the gate; --self-test (39 checks) is in make validate and the DAG. Against the fake bridge: all six cases PASS; never-start-after-interrupt gives f PASS by reconnect, with and without --replay; never-start-twice gives f PASS by reconnect_limit with "1 new message" not read out; ignore-relay fails a as bridge/agent; a binary built from the page before #352's reconnect fails f. Documented in the README beside the voice bridge protocol.

Not run against a real bridge from this lane. Not ported: the original's bridge-idle check and bridge-log correlation, which read one deployment's logs; run the check only against a bridge nobody else is in a call on. Planned but not added: a typed-interrupt control (f2) and a relay inside the 750 ms hand-off; --interrupt-relay-delay of 2-3 s answers whether a short settle avoids a lost turn.
