---
title: 'audit-format-label-mismatch: the human audit labels BLOCKED what the JSON format calls UNKNOWN'
status: closed
priority: 3
issue_type: bug
created_at: 2026-11-09T02:10:00.000000000+00:00
updated_at: 2026-11-09T05:30:00.000000000+00:00
closed_at: 2026-11-09T05:30:00.000000000+00:00
---

# Description

The same `wrkslots audit` evidence gets different verdict labels depending on the output format. When only cache-census reasons remain on a row, `--format json` reports the slot UNKNOWN, while the human format prints, for example:

> BLOCKED: <slot>: cache inspection incomplete: bounded census is partial

(Seen on a consumer host, Python edition at agent-utils a0eeb67d, 2026-11-08.) An operator reading the human format sees a definite block where the machine format says "we don't know".

# Design

One verdict per row, computed once and rendered by both formats. The human output prints the same label as the JSON `verdict` (or status) field. Check both editions.

# Acceptance Criteria

- A test renders the same audit evidence in both formats and asserts identical per-row labels, including the cache-only-reasons case.
- Both editions agree, or the parity gap is recorded.

# Notes

Reported by the lead (devbig014) on 2026-11-08. Related: #1106 audit-cache-allowance-load.

## Resolution

Delivered by cl-wrkid, 2026-11-08 ET, in both editions, stacked on #1106 audit-cache-allowance-load.

- Cause: both formats printed the row's own `verdict`, which was BLOCKED. The UNKNOWN the reporter saw in JSON was the gate-level `unknown_slots` / `state: unknown` classification, which also counts BLOCKED rows whose reclaim state cannot be determined.
- Fix: one verdict per row. After the gate classification, a row whose slot is in `unknown_slots` gets `verdict: "UNKNOWN"`, so JSON and text print the same label (Python `_cmd_audit`; Rust `parse_audit`, through one shared `indeterminate` predicate). A row whose slot name is also an attention slot keeps its verdict, as the gate lists already do. `unknown_slots`, `state` and the exit codes are unchanged.
- The case the report showed, cache-only reasons, no longer arises for an allowance shortfall after #1106. A cache census that FOUND something on an expired row with no live lane is the remaining cache case, and it now reads UNKNOWN in both formats.
- Tests (py/wrkslots/tests/test_audit_state_rust.py):
  - `test_audit_text_labels_match_the_json_verdicts` renders one fixture in JSON and text, in both editions, and asserts identical per-row labels (UNKNOWN);
  - `test_audit_census_nested_git_matches_python` now expects UNKNOWN, with the slot in `unknown_slots`.
- User guide: "one verdict" paragraph beside `audit --gate`.
