---
title: 'python-lifecycle host and namespace shards fail on live-host scans under load and match no known-red entry: quarantine the live-host cases under the #2065 hermetic programme'
status: open
priority: 2
issue_type: task
labels:
- wrkslots
- validation
- flake-triage
created_at: 2026-11-27T17:46:00-05:00
updated_at: 2026-11-27T17:46:00-05:00
---

# Description

The 2026-11-27 frequent-red audit (P6) found lifecycle shard reds on untouched heads:

- `python-lifecycle.host.shard-2`: 16 heads (FLAKE 3, UNKNOWN 13).
- `python-lifecycle.namespace.shard-1`: 12 heads (FLAKE 1, UNKNOWN 11).

The family shape is live-host process and mount scanning under load (#2065 census, #2372 mount-holder scan). Unlike `python.wrkslots.shard-*`, no known-red entry names these steps, so each red stops a gate.

# Design

- With kept logs (#2409), name the failing cases.
- Move the cases that scan the live host under #2065's hermetic programme: fake process and mount sources, or wait on conditions instead of a scan budget.
- Meanwhile, an interim known-red entry that names the exact cases and expires, only with that evidence.

# Acceptance Criteria

- The lifecycle shards pass at load1 >= 237 for a whole run, or the live-host cases are quarantined with this issue as the tracker.

# Notes

Filed 2026-11-27 at the lead coordinator's request.
Source: the harness repository's `ai_docs/reports/measurements/2026-11-27-frequent-red-steps-audit.md`, item P6, step 13 and annex A1.
Related: #2065, #2372, #2415.
