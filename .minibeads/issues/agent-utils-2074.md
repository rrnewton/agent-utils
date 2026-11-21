---
title: 'read-aloud-serial-yield: one shared conversation session serialises blocks, so tap-to-audible grows per block'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- read-aloud
created_at: 2026-11-21T14:16:00-05:00
updated_at: 2026-11-21T14:16:00-05:00
---

# Description

Measured from 70 timed read-aloud requests (server speech timing reports): the provider's first audio after a prompt has a median of 41 ms (max 275 ms), but the time a block waits for the previous block's turn to complete on the single shared conversation session (speech.rs SessionTask::advance) has a median of 6.6 s (max 24.5 s). That wait is 99% of request-to-first-audio, and 54 of 70 waits exceed 1 s. One tap waited 14.8 s of its 19.7 s tap-to-audible this way. Blocks synthesise strictly one after another, so tap-to-play grows linearly with the block's position.

# Design

Options (owner choice pending): (1) a two-session prefetch pipeline that prompts block N+1 while block N plays, played in order (recommended; needs two concurrent provider sessions); (2) parallel synthesis with in-order assembly; (3) smaller blocks as a stopgap (not recommended alone).

# Acceptance Criteria

- The per-block wait drops to near zero while first audio stays around 40 ms, measured from the speech timing reports.
- Ordered playback, interrupts and supersession covered by deterministic tests.

# Notes

Filed by the coordinator from the investigating lane's evidence; the owning lane refines this file as it works.
