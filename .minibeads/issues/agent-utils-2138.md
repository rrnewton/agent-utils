---
title: 'wrkslots-v2-review-and-design: independent review of wrkslots feeding a verification-friendly Rust V2'
status: open
priority: 1
issue_type: task
labels:
- wrkslots
- rust
created_at: 2026-11-22T18:05:00-05:00
updated_at: 2026-11-22T18:05:00-05:00
---

# Description

Owner direction: the Python editions are secondary and deprecated (no parity work), and wrkslots moves to Rust as a V2 that aims to be substantially better, not a port. wrkslots has had many bugs and stuck states, so the first step is an independent review of the current code and its history, feeding the V2 design.

# Design

Review deliverable: a bug taxonomy by root cause with counts and live classes; the state machine as implemented and every stuck or work-losing state; what the formal model and tests really cover; V2 options for this code (typed state machine in safe Rust, totality proofs with Verus or Kani, model checking of the real code with Stateright, loom or shuttle for concurrency, property tests and fuzzing for no-stuck-state, keeping TLA+ for the protocol); asymptotic and memory costs per operation; only constant-time work under locks; and a migration plan that keeps existing registries and pinned readers working.

# Acceptance Criteria

- The review report is published and linked here.
- A V2 design note follows, citing the review findings it answers.

# Notes

Owner request; filed by the coordinator. The design work waits for the review.
