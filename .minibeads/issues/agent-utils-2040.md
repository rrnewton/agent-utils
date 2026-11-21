---
title: 'wrkslots-partial-status-claims-consistent: a partial status must not claim the registry is consistent'
status: open
priority: 2
issue_type: bug
labels:
- wrkslots
created_at: 2026-11-21T11:45:00-05:00
updated_at: 2026-11-21T11:45:00-05:00
---

# Description

When the scan bound is exceeded, a partial `wrkslots status` still prints registry_storage=CONSISTENT inconsistencies=0, while a full scan of the same tree finds 48 inconsistencies. That is an absence claim from an incomplete scan.

# Design

A partial run prints UNKNOWN for registry storage and inconsistencies and says it is partial; it never asserts absence. Both editions.

# Acceptance Criteria

- The behaviour described under Design, covered by a deterministic regression test (condition-based, no wall-clock budgets).

# Notes

Filed by the coordinator so the issue link resolves on main; the owning lane refines this file as it works.

## Implementation notes

The absolute claim lives only in `_cmd_status`
(`py/wrkslots/cli.py`): the text header and the JSON
`registry_storage_state`. Both now say `UNKNOWN` / `"unknown"`
whenever `partial_detail` is set (scan bound or memory bound exceeded).
Findings actually measured still print; only the summary verdict stops
asserting absence. The Rust mirror (`status_state.rs`) never produces
partial output — its census refuses on error — so it never asserts
absence from an incomplete scan; the envelope differential test keeps
the two editions' full-scan claims identical.
