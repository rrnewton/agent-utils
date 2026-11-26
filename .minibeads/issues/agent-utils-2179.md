---
title: 'wrkslots-transfer-owner-stale-task-scope: transfer-owner refuses on a dead owner''s task_scope'
status: closed
priority: 2
issue_type: bug
assignee: cl-transfer
labels:
- wrkslots
created_at: 2026-11-24T03:20:00-05:00
updated_at: 2026-11-26T11:15:00-05:00
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

## Resolution

transfer-owner now replaces a recorded task scope together with the owner. The loader binds a scope to its owner generation, so the existing owner proof decides it: a scope whose owner is provably dead (another boot, an absent PID, a reused PID, a zombie) is rebound to the successor's own scope when the successor's cgroup is a transient scope of the user's systemd manager whose invocation link verifies as --task-scope-unit requires, and is dropped otherwise; the record keeps its owner binding either way. A scope still bound to a live or indeterminate owner refuses, with --reopen too (unless the successor is that owner itself). No persisted field or evidence key changed: an absent task_scope already reads as none in every reader that knows the field. Python (_cmd_transfer_owner) and Rust (transfer_write) behave alike; tests: test_owner_transfer.py (previous-boot scope rebound and later release, scope dropped, reused PID, live owner refused, live reopen refused) and the differential in test_transfer_owner_write_rust.py, plus a Rust unit test for the drop branch.

Landed as c98e82e4 (gated tree f0394ff9, replayed onto main from base b3a314d1). Gate evidence: full owner gate 1795706257236807480-7ae4ff24 had 74 of 75 steps green; the red step, rust.wrkslots.test, was a CPU-TIMEOUT with no assertion failure, and the test in flight was one this change does not reach (see #2300). The exclusive rerun of rust.wrkslots.test on the same tree, 1795708440456636001-d02f6f78, passed.
