---
title: 'wrkslots-bounded-evidence-fast-fixture: bring the 117-second bounded-evidence enforcement test under about 20 seconds'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
- test-robustness
created_at: 2026-11-18T10:45:00-05:00
updated_at: 2026-11-18T10:45:00-05:00
---

# Description

[watermelon-full-preview] Filed at the #1772 landing (coordinator condition): `py/wrkslots/tests/test_finish_provisioned.py::test_many_ignored_files_finish_bounded_and_remove_enforces` takes about 117 seconds solo on this host, breaking the fast-tests rule. Most of the time is fixture scale (70 ignored files through create, finish and two removes with full censuses), not the bound it proves.

# Design

Shrink the fixture while still proving the bound: fewer, smaller files that still exceed the 64-entry inline cap (for example 65 tiny files), the smallest project shape the lifecycle helpers allow, and no wall-clock budgets. The test must keep covering the summary shape, the manifest verification, and the changed-file refusal through the manifest.

# Acceptance Criteria

- The test passes in under about 20 seconds solo on this host and still asserts the summary, the manifest, and the enforcement refusal.
- No gating verdict depends on a wall-clock budget.

# Notes

Related: #1772 wrkslots-ignored-evidence-unbounded-events (the feature), #1765 wrkslots-load-sensitive-gate-tests (load-robustness tracking).
