---
title: 'vibe-talk-read-aloud-chunk-start-clipping: first samples of a chunk dropped on Bluetooth sinks'
status: closed
priority: 0
issue_type: bug
assignee: mu-audiopreroll
labels:
- vibe-talk
created_at: 2026-11-17T19:09:00.000000000+00:00
updated_at: 2026-11-17T22:35:00.000000000+00:00
closed_at: 2026-11-17T22:35:00.000000000+00:00
---

# Description

[watermelon-full-preview] Owner request, fast-tracked (owner thread 147, 2026-11-17), verbatim: "when I use this in the car, and the audio output is going over the car speakers, I think I notice more of the audio skips at the start of a new Chunk. Like it drops the first bit of the audio. I'm trying to remember if I had this with airpods output or with phone speaker. I think I did but I'll have to watch out for it. There are also various pronunciation glitches which sound weird."

Likely cause (unverified): Bluetooth sinks (car, AirPods) idle or re-negotiate between chunks and clip the first few hundred ms of each new chunk.

# Design

Confirmed mechanism (see the lane's report to coordb): the server TTS path already uses one persistent `AudioContext` with gapless scheduling across blocks, but suspends it at every read end and pauses the silent keeper, so the next read resumes into a sleeping sink; the browser `speechSynthesis` path speaks one fresh utterance per chunk with no keep-alive between chunks for message reads. Fix: pre-roll silence (default 300 ms, a clamped per-device setting, 0 disables) before a chunk that starts cold (context was suspended / no contiguous predecessor), keep gapless starts untouched, and keep the silent keeper playing across the chunks of a multi-chunk device-voice read.

# Acceptance Criteria

- A chunk starting cold is preceded by the configured silence pre-roll on the shared context; a chunk continuing gaplessly from its predecessor gets none.
- The pre-roll length is a per-device setting with a 300 ms default; 0 turns it off.
- Deterministic page tests with a fake audio sink record start offsets proving the first samples of each chunk are scheduled, not dropped (owner rule 346: no wall-clock assertions).
- Pronunciation glitches from `src/speakable.rs` are collected as examples with proposed fixes, kept out of this change unless trivial.

# Notes

Owner fast-track (owner thread 147). No UI change in this lane, so no screenshot matrix; test evidence goes to coordb and the liaison reviews before anything is announced.

## Resolution

[watermelon-full-preview] Landed on github/main as `f7e53e15033bd1b188eb3e2d35ded25825472fbd` (gated head `892aa537`; local validate OK; gate sole failure the known floating-chip-overlap load flake, passed 3/3 solo and on the exclusive gate rerun). A chunk starting cold on the shared audio clock is now preceded by `readPrerollMs` of scheduled silence (300 ms default, clamped 0-2000, 0 off), scheduled only after the context resumes and reports running; gapless chunks get none. The context stays alive `readKeepAliveMs` (25 s default) after a read ends before suspending — a pause still suspends at once — and device-voice message reads of several chunks hold the silent keeper between utterances. Deterministic fake-sink page tests record start offsets proving the first samples are delivered (0.3 s silence, speech whole after it, gapless starts exact), with no wall-clock assertions. No Bluetooth sink is exercisable on this host; the owner's listening check is in the lane handoff. Pronunciation proposals (abbreviations, camelCase, raw emoji) are recorded in the handoff and deliberately out of this change.
