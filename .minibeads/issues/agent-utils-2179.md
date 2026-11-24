---
title: 'wrkslots-transfer-owner-stale-task-scope: transfer-owner refuses on a dead owner''s task_scope'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-24T03:20:00-05:00
updated_at: 2026-11-24T03:20:00-05:00
---

# Description

transfer-owner refused for a slot with: "refusing to append an event the registry loader would reject: append-only active-record event.record.task_scope does not match its bound owner process". The slot's task_scope belongs to its previous owner, dead for about 61 hours, so no transfer can ever carry that scope proof for the new owner.

# Design

Transfer must carry or reset task_scope for the new owner so that the loader accepts the appended event. A scope bound to a proven-dead owner must not block the transfer; a scope that still binds a live process must keep refusing, exactly as today.

# Acceptance Criteria

- transfer-owner succeeds for a slot whose recorded task_scope owner is proven dead, and the resulting record passes the registry loader.
- A task_scope still bound to a live process still refuses.
- Python and Rust parity, covered by deterministic regression tests.

# Notes

Field report from a consumer's post-reboot re-bind; the dead owner's scope had been stale for about 61 hours at refusal time.
