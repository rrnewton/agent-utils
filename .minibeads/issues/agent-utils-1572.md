---
title: 'audit-sees-private-history: audit and unpushed must find submodule and reflog-only history'
status: open
priority: 1
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-15T14:36:00+00:00
updated_at: 2026-11-15T14:36:00+00:00
---

# Description

[watermelon-full-preview] Audit reported slots DELETABLE that
`remove` then refused rc 3 for unpushed private history in a
submodule. `_audit_record` establishes submodule remote authority
but never runs the private-history scan; `unpushed` judges only HEAD
containment. Removal scans reflog endpoints, pseudorefs, and every
commit object (`_submodule_private_history`). The lifecycle model
(#1568) reproduces this as the audit-safety counterexample.

# Design

Give audit (and `unpushed` where it claims containment) a bounded
tips-presence probe reusing the declared-submodule private-history
machinery, so DELETABLE never names a slot remove would refuse for
history the probe can see. Keep audit fetch-free (stored refs) and
report any probe budget exhaustion as BLOCKED, never DELETABLE.

# Acceptance Criteria

- Fixture: submodule with reflog-only unpushed commits audits
  BLOCKED with the private-history reason, not DELETABLE.
- Clean submodule fixtures still audit DELETABLE.
- Probe cost is bounded and reported; exhaustion blocks.
