---
title: 'wrkslots-transfer-owner-held-dead-owner: a held slot with a dead owner cannot change hands'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
- lifecycle
created_at: 2026-11-18T12:30:00+00:00
updated_at: 2026-11-18T12:30:00+00:00
---

# Description

[watermelon-full-preview] Reported 2026-11-18: `transfer-owner` refuses a slot whose owner is DEAD while the slot holds protected ignored evidence (a hold), so a held slot cannot be handed to a living owner. The evidence and the hold are exactly what must survive the transfer, yet they are the reason it is refused.

# Design

Wanted: a `transfer-owner` that preserves the evidence and the hold (or an explicit adopt-held path), still refusing when the current owner is alive — a living owner's slot is never taken from it. No new persisted field is introduced; any new persisted field would be reported to the coordinator before gating and gated by `writer_ceiling >= 3` (the #1788 mechanism) or not persisted.

# Acceptance Criteria

- A held slot whose owner is dead transfers to a living owner with its evidence and hold intact, in a deterministic end-to-end test through the real CLI (both editions where the surface exists).
- `transfer-owner` still refuses while the current owner is alive.
- Existing transfer/hold suites pass.

# Notes

One of three related lifecycle dead-ends reported together; see #1868 wrkslots-finish-without-handoff-dead-end and #1869 wrkslots-adopt-reports-success-binds-nothing. Whether current main already changes this behaviour is checked before the fix and recorded in the resolution.
