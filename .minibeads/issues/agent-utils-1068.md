---
title: 'message-cache-deletion-flake: the down-server deletion test fails under gate load'
status: closed
priority: 2
issue_type: bug
created_at: 2026-11-08T15:30:00.000000000+00:00
updated_at: 2026-11-08T18:02:31.000000000+00:00
closed_at: 2026-11-08T18:02:31.000000000+00:00
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

## Resolution

[muse watermelon-full-preview] Fixed in the delivering commit on `mu-cacheflake/1068-message-cache-deletion-flake` (test only; no product change). The race: the shadow evaluation beside a by-id ask waits at most `shadow::BARRIER` (100 ms of real time) for the cache writer to apply everything queued before the ask, and the test's second server never waited for the writer - the whole-channel read's revalidation and the adapter push could still be queued when the asks judging them were made. Under gate load the writer missed the wait and the final, serve-expecting ask was counted `barrier` instead of `would_serve`: the captured rerun counts are `asked: 3, would_serve: 0, misses { stale_head: 2, barrier: 1 }, late: 0`, left `(0, 0)` against right `(1, 1)`. Only that ask can flake so, since a barrier timeout downgrades `Serve` alone. The test now binds the cache and `quiesce_blocking(WAIT)` after install, after the whole-channel read, and after the push - the wait sibling serve-expecting lookups in the same file already make - with every assertion unchanged.

Evidence: with vouching deliberately broken (`shadow::message`'s feed check forced true, then reverted) the test still fails at the first ask with `would_serve: 1, extra_rows: 1`, left `(1, 1)` against `(0, 0)`. Load proof through a gate ticket on devbig030 (admitted 2026-11-08 12:53 ET): 4 concurrent streams of 50 runs each, test processes and CPU burners pinned to the same two CPUs, 200 consecutive passes. Pinned local runs before the fix passed (60 single; 12 + 6 whole-module), as solo runs do; the boxed, saturated gate host is what starves the writer past 100 ms. Full-gate record is in the delivering commit.
