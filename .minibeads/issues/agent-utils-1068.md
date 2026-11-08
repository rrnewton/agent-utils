---
title: 'message-cache-deletion-flake: the down-server deletion test fails under gate load'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-08T15:30:00.000000000+00:00
updated_at: 2026-11-08T15:30:00.000000000+00:00
---

# Description

`vibe-talk.test.unit` test `message_cache_fill::a_deletion_made_while_the_server_was_down_is_never_vouched_for` (vibe-talk/tests/message_cache_fill.rs:3102) failed in two full gates on 2026-11-08 morning, both under gate load, once with the assertion comparing `(0, 0)` against `(1, 1)`; both passed when rerun solo. Every such failure costs a rerun for whichever lane hits it.

# Design

- Find the race: most likely timing between the fill pass and the deletion check (the test observes before the fill or the vouch decision has settled, or a timer or interval in the fill races the check).
- Make the test deterministic without weakening what it proves: wait on the event or state the assertion depends on (a fill-complete signal, a deterministic clock, or an explicit step), never a longer sleep or a retry that would hide a real vouching bug.

# Acceptance Criteria

- The race is named in the commit body with the evidence for it.
- The test passes 50 consecutive runs under load (CPU burners or a concurrent gate), run through gate.py tickets.
- A deliberately broken vouching path (the deletion vouched for) still fails the test.

# Notes

Reported by the lead coordinator from two gates on its host (mu-argmax's and mu-predict's). Low urgency; for a vibe-talk lane.
