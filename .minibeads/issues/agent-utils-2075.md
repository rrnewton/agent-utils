---
title: 'read-aloud-interrupt-ack-window: a 3 s interrupt acknowledgement kills a live but slow provider session'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- read-aloud
created_at: 2026-11-21T14:16:00-05:00
updated_at: 2026-11-21T14:16:00-05:00
---

# Description

When a new read-aloud selection supersedes the current turn, the session sends an interrupt and waits at most 3 s (INTERRUPT_ACK_TIMEOUT) for the superseded turn to complete. No inbound frame extends that deadline while yielding, so a provider that is alive but slow to cancel mid-generation gets its session killed; the reopen then costs the next block another wait. It happened twice on 2026-11-21. Queued blocks are retried once on the fresh session (no silent loss), but only once and from the block's start.

# Design

While yielding, extend the deadline on any inbound frame, with a hard cap of about 15 s (or a fixed 8-10 s window); keep 3 s only as the bound for complete silence.

Refined by mu-s2s (2026-11-21): `SessionTask` records when the yield began. `interrupt()` sets the deadline to now + 3 s as today; every inbound frame while yielding moves it to min(now + 3 s, yield-start + 15 s). Only the superseded turn's `turn_complete` clears the yield, as today. `expire()` keeps the session-death path and its `Failed{retry:true}` fan-out unchanged; only its reason text distinguishes the silence bound from the cap. Overlap with #2056 (mu-readretry): that branch renames `interrupt(cause)` and adds yield counters; this change keeps the current signature, so a rebase over it is confined to the `interrupt()`/`complete()` bodies. The ack-timeout counter gap (timeouts never reach `complete()`) stays with #2056's follow-up, not this issue.

# Acceptance Criteria

- A slow but live provider is not killed during an interrupt; a silent one still is.
- The retry-once behaviour for queued/unheard blocks is unchanged.
- Deterministic tests with a fake provider (condition-based, no wall-clock budgets).

# Notes

Filed by the coordinator from the investigating lane's evidence; the owning lane refines this file as it works.
