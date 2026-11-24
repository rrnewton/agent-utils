---
title: 'wrkslots-model-lock-liveness: model lock acquisition and check that heartbeats and creates are never starved'
status: closed
priority: 2
issue_type: feature
labels:
- wrkslots
- formal-model
created_at: 2026-11-22T05:50:00-05:00
updated_at: 2026-11-24T04:14:16.000000000-05:00
closed_at: 2026-11-24T04:14:16.000000000-05:00
---

# Description

The owner asked whether the wrkslots formal model could have caught a night-long lock hog: a client repeatedly ran recover-absent-validate-rows, which took every registry lock, loaded the whole event history, and only then refused, blocking heartbeats and validation creates for over an hour (fixed in code by #2088). The transition model has one abstract mutation lock, treats recover edges as atomic with lock-free entry, and has no repeating client, so it cannot express this.

# Design

TLA+ only (owner direction 2026-11-22: the Python explorers are deleted in this change; the gated check becomes a bounded TLC step). In py/wrkslots/spec/WrkslotsLock.tla: model lock acquisition, hold and release as separate steps for recover-absent (acquire, expensive load, refuse-release) and for heartbeat and create; add a client that repeats a refusable call unboundedly; state the liveness property that a heartbeat or create that wants the lock eventually holds it, under weak fairness. Expect a starvation counterexample for the pre-#2088 shape (non-FIFO flock) and a pass for the #2088 shape (refusal checks before the lock); record both runs as committed configurations checked by the gated TLC step (runs when a checksum-pinned jar is provisioned, skips by name otherwise).

# Acceptance Criteria

- The model reproduces the starvation counterexample for the pre-#2088 shape and passes for the #2088 shape; both configurations are committed and documented; the gated TLC step stays fast and skips by name when no jar is provisioned.

# Notes

Filed by the coordinator after the owner's question; the owning lane refines this file as it works.

## Resolution

[watermelon-full-preview] Landed on main as 289639d9 (replay of 1f818c1b under the coordinator waiver for the gate's load-only reds, each 5/5 solo). WrkslotsLock.tla reproduces the pre-#2088 starvation counterexample (48 states) and passes post-#2088 (30 states); test_tlc_gate.py gates the small TLC configs with the pinned jar (5 passed, skip-by-name without one); the Python explorers and their tests are deleted, TRANSITIONMODEL.md is the TLA+ guide.
