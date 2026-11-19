---
title: 'wrkslots-seal-orphan-misjudges-live-operator: cgroup moves and operator-less seals read as dead in create recovery'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
- lifecycle
created_at: 2026-11-19T12:00:00+00:00
updated_at: 2026-11-19T12:00:00+00:00
---

# Description

[watermelon-full-preview] Found by the #1869 cold review (2026-11-19) in code that already exists on main: `create`'s orphaned-seal recovery (`_recover_orphaned_validate_batch_seal`, judged by `_validate_batch_seal_orphaned`) can retire a validation-batch seal whose batch is still running. Two shapes: (D2) the operator moved to another cgroup -- `ProcessIdentity` equality includes `cgroup_path`, so the live generation reads as "reused" and therefore dead; (D3) a seal recording no operator is judged by its authorizing actor alone, so a dead coordinator beside a live `remove-validate-batch` process still counts as orphaned. #1869 only made `adopt` a new trigger for the same judge; this issue tracks the pre-existing `create` path.

# Design

Tighten the one orphan judge both commands share: only a proven-dead operator (recorded boot ended, PID absent from a verified whole-host process view, or the exact recorded generation an unreaped zombie) makes a seal orphaned. A cgroup-only mismatch is the same live generation; a reused PID or a namespace-local recording proves nothing; a seal with no recorded operator is never auto-recovered (only `recover` retires it). The #1869 change applies this judge to adopt; if it lands first, this issue remains for verifying the create-side tests and any create-only callers of the same predicate.

# Acceptance Criteria

- A seal whose operator moved cgroup (same PID, start ticks, boot and host) is not retired by create, in a deterministic test with a real or faithfully simulated process.
- A seal recording no operator is not auto-retired by create even when its actor is dead and a batch-shaped process lives.
- Seals whose operators genuinely exited or ran in an earlier boot still retire as before.

# Notes

Split out of the #1869 cold review so the pre-existing create-path defect is tracked even though #1869's own fix is scoped to adopt. No new persisted field is expected.
