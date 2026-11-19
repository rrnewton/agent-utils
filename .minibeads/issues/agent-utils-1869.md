---
title: 'wrkslots-adopt-reports-success-binds-nothing: adopt exits 0 but binds no owner'
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

[watermelon-full-preview] Reported 2026-11-18 on build f1697dc3: `adopt SLOT --agent A --owner-pid P --expected-generation 1` — bare, and with `--task-scope-unit run-pNNN-iNNN.scope --task-scope-invocation-id ID` (a sandbox-scoped harness) — exits 0 with only RETAINED warnings, but the ACTIVE row keeps `owner: null` and an old `heartbeat_at`, and `finish` then refuses `no-bound-owner`. An adopt that binds nothing must never report success.

# Design

`adopt` either binds the named owner (the ACTIVE row gains that owner and a fresh heartbeat) or refuses with a reason; a silent exit 0 is the bug. The sandbox-scoped owner case (cgroup/scope) is reproduced in a test, because that is the shape that silently bound nothing. No new persisted field is introduced; any new persisted field would be reported to the coordinator before gating and gated by `writer_ceiling >= 3` (the #1788 mechanism) or not persisted.

# Acceptance Criteria

- `adopt` with a sandbox-scoped owner either binds (ACTIVE row shows the owner, `finish` no longer refuses `no-bound-owner`) or exits non-zero with a stated reason, in a deterministic end-to-end test through the real CLI (both editions where the surface exists).
- The bare adopt form behaves the same way: bind or refuse, never a silent 0.
- Existing adopt/create/heartbeat suites pass.

# Notes

One of three related lifecycle dead-ends reported together; see #1868 wrkslots-finish-without-handoff-dead-end and #1870 wrkslots-transfer-owner-held-dead-owner. Whether current main already changes this behaviour is checked before the fix and recorded in the resolution.
