---
title: 'wrkslots-adopt-reports-success-binds-nothing: adopt exits 0 but binds no owner'
status: closed
priority: 2
issue_type: bug
labels:
- wrkslots
- lifecycle
created_at: 2026-11-18T12:30:00+00:00
updated_at: 2026-11-20T14:00:00+00:00
---

# Description

[watermelon-full-preview] Reported 2026-11-18 on build f1697dc3: `adopt SLOT --agent A --owner-pid P --expected-generation 1` — bare, and with `--task-scope-unit run-pNNN-iNNN.scope --task-scope-invocation-id ID` (a sandbox-scoped harness) — exits 0 with only RETAINED warnings, but the ACTIVE row keeps `owner: null` and an old `heartbeat_at`, and `finish` then refuses `no-bound-owner`. An adopt that binds nothing must never report success.

# Design

`adopt` either binds the named owner (the ACTIVE row gains that owner and a fresh heartbeat) or refuses with a reason; a silent exit 0 is the bug. The sandbox-scoped owner case (cgroup/scope) is reproduced in a test, because that is the shape that silently bound nothing. No new persisted field is introduced; any new persisted field would be reported to the coordinator before gating and gated by `writer_ceiling >= 3` (the #1788 mechanism) or not persisted.

# Acceptance Criteria

- `adopt` with a sandbox-scoped owner either binds (ACTIVE row shows the owner, `finish` no longer refuses `no-bound-owner`) or exits non-zero with a stated reason, in a deterministic end-to-end test through the real CLI (Python edition; the Rust adopt has no seal surface yet, see Notes).
- The bare adopt form behaves the same way: bind or refuse, never a silent 0.
- An `adopt` facing a validation-batch seal recovers it when the operator is proven dead with no paired finish, proceeds beside it when the adopted slot is disjoint from every sealed target, and refuses with the canonical seal text when the slot overlaps; a seal whose operator is live, moved cgroup, recorded without an operator, or judged from inside a PID namespace is never retired by adopt. Tests cover these with real processes, including a namespace-run adopt.
- Existing adopt/create/heartbeat suites pass.

# Notes

One of three related lifecycle dead-ends reported together; see #1868 wrkslots-finish-without-handoff-dead-end and #1870 wrkslots-transfer-owner-held-dead-owner. Whether current main already changes this behaviour is checked before the fix and recorded in the resolution.

Second case (2026-11-19, from the same reporter): `finish` refuses `no bound owner process` and `adopt` is blocked by a dead VALIDATE-BATCH-SEAL journal (`interrupted validation-batch seal recorded in ...; run wrkslots recover first`). Scope therefore includes seals: `adopt` recovers an orphaned (operator proven dead, no paired finish) seal under its lock, and proceeds beside a remaining seal when the adopted slot is disjoint from every sealed target (different slot, no directory containment, different (device, inode)); an overlapping adopt keeps the canonical seal refusal. The orphan proof was tightened after a cold review: only a recorded boot that ended, a PID absent from a verified whole-host process view, or the exact recorded generation as an unreaped zombie counts as dead; a cgroup-only move, a reused PID, a namespace-local recording, a missing operator, and an unverifiable host view all keep the seal binding. The pre-existing create-side forms of the cgroup and no-operator misjudgments are tracked as #1922 wrkslots-seal-orphan-misjudges-live-operator. The stuck paired-finish census behind the reporter's seal is #1883. This change is Python-only: the Rust edition has no seal-recovery writer yet (its adopt is fixture-only before cutover), so the Rust port is a follow-up the cutover must include. Case 1's silent exit 0 was traced to the reporter's `timeout 60 ... | head -20` pipeline (head's exit code, with adopt starved by full-registry validations under churn); every adopt path in the code refuses non-zero, so that half closes as not-a-bug in the resolution.

## Resolution

[watermelon-full-preview] Landed as c98f8428. Adopt now judges a validation-batch seal against its own slot under the mutation lock (overlap by slot, path containment, or recorded device+inode keeps the canonical refusal; recovery only after every precondition passes), and adopt, create and recover share a judge-namespace-relative orphan proof in both editions: only a recorded boot that ended, a PID absent or reused under the seal's own namespace stamp (an unstamped legacy seal under a verified whole-host view), or the exact recorded generation as an unreaped zombie proves a batch dead. The writer stamps its own pidns (never refusing for its namespace, but refusing when /proc is not its own), the override is gated to its own namespace or a verified host view, scans for live batches including inside the recorded namespace, and records each retirement after it succeeds in the existing recovery event. Case 1's silent exit 0 was the reporter's `timeout 60 ... | head -20` pipeline (head's exit code), not adopt: every adopt path refuses non-zero, so that half closes as not-a-bug. The Rust recovery writer remains a cutover follow-up, as noted; seventh-review residuals are #1958 wrkslots-seal-override-scan-residuals.
