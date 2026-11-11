---
title: 'wrkslots-ttl-rust-parity: Rust parity for the TTL ladder, report, rescue and teardown'
status: open
priority: 1
issue_type: task
labels:
- wrkslots
created_at: 2026-11-11T13:45:00+00:00
updated_at: 2026-11-11T13:45:00+00:00
---

# Description

[watermelon-full-preview] Follow-up to #1293 wrkslots-ttl-escalation-teardown. The Python edition gained the TTL escalation ladder, the read-only ttl-report (human, json and tick-hub HEALTH formats), the deterministic rescue executor (dated rescue refs, verified bundle fallback, HOW-only hook) and the teardown core. The Rust edition (rs/wrkslotsd, including remove_write.rs) has none of it, so the two editions disagree on what an overdue slot gets.

# Design

- Port the ladder thresholds and stage computation exactly (warn at expiry, alarm after 1 day, stop after 2 days, teardown after 3 days by default), with the same fixed-clock behaviour: callers pass the judging timestamp, tests never read the wall clock.
- Port ttl-report with byte-equal human and json rows and the same HEALTH line grammar, and the rescue/teardown receipts, so the Python/Rust differential stays exact.
- Rescue and teardown follow the lock pattern the retire-pending narrowing establishes: slow Git and bundle work outside the registry locks, state transitions inside them.
- No behaviour change in the Python edition; any divergence found while porting is filed against #1293, not fixed silently here.

# Acceptance Criteria

- Rust ttl-report output matches Python on shared fixtures (stages, rescue refs, bundle paths, teardown times), proven by a differential test.
- Rust rescue pushes the same dated rescue refs and writes an equivalent verified bundle on push failure; a failing or absent hook falls back to the deterministic names exactly as Python does.
- Rust teardown at the teardown rung removes after rescue and never refuses on a failed rescue that its bundle holds.
- Focused tests count operations and assert refs or bundles, never wall-clock budgets.

# Notes

Filed by mu-ttlteardown; implementation waits until the Python slices land and the lock pattern it must follow is on main.
