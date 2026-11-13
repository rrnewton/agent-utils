---
title: 'agentctl-drain-holds-queue-lock-across-delivery: claim under the queue lock, deliver without it'
status: open
priority: 1
issue_type: bug
created_at: 2026-11-13T11:24:51.869593512+00:00
updated_at: 2026-11-13T11:24:51.869593512+00:00
---

# Description

A drain holds the queue delivery lock for its whole delivery attempt (up to the 600s bound) while waiting on the host-wide account target lock, so a concurrent send waits out its full deadline (Rust rc 75 pending vs Python rc 76 possibly_submitted seen in differential under saturation). Second site: the drain waits on the account target lock, which lives in the real account target-locks directory even in tests, shared across cases and with real agents.

# Design

Rust agent.rs and Python agent.py parity: the queue lock is held only to claim an item (move inbox to inflight with the at-most-once barrier) and to commit its outcome (processed, back to pending, or failed); it is released across the target-lock wait, readiness wait and typing. Crash semantics unchanged: a claimed item whose delivery never completed is recovered exactly as today (pending when provably untyped, possibly_submitted otherwise), never lost and never typed twice. Add an injectable account/state root for target locks (AGENTCTL_TARGET_LOCK_ROOT, both editions) so tests never touch the real directory; production default unchanged.

# Acceptance Criteria

A test that kills the drain between claim and type recovers the item as pending or possibly_submitted, never lost, never typed twice (both editions where the path exists). A concurrent send during a held target lock does not wait out its deadline. Target-lock tests run under an injected root. Focused Rust and Python tests pass; differential cases pass.

# Notes

Evidence: saturation-scope differential (Rust rc 75 pending vs Python rc 76 possibly_submitted); second site in adopt/muse native-session yolo-delivery with the same signature. Coordinate the injectable-root need with the cross-robustness lane.
