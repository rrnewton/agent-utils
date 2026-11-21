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

# Acceptance Criteria

- A slow but live provider is not killed during an interrupt; a silent one still is.
- Deterministic tests with a fake provider (condition-based, no wall-clock budgets).

# Notes

Filed by the coordinator from the investigating lane's evidence; the owning lane refines this file as it works.
