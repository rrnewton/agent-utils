---
title: 'audit-cache-allowance-load: cache-census allowances leave slots UNKNOWN or BLOCKED on a loaded host'
status: open
priority: 2
issue_type: bug
created_at: 2026-11-09T02:10:00.000000000+00:00
updated_at: 2026-11-09T02:10:00.000000000+00:00
---

# Description

On a consumer host with 78 slots at load 100-200, `wrkslots audit` (Python edition, agent-utils a0eeb67d) reports many slots UNKNOWN or BLOCKED only because cache inspection ran out of its allowance:

- `cache inspection failed: cache planning exhausted this subject's share of 3297 directory listings; no cache total can be published within this allowance` (the number varies per subject: 1373, 1636, ...)
- `cache inspection: cache census traversal exhausted the fixed wall allowance of 5 seconds; no cache total can be published within this allowance`
- `cache inspection incomplete: bounded census is partial`

Two runs minutes apart gave different verdicts (DELETABLE 0, then 7), so these allowances make audit's answer depend on host load. The process/path census part of the same evidence is #1102 wrkslots-census-churn-tolerant; this issue covers only the cache-census allowances.

# Design

- Decide what a cache-census shortfall should mean for the row. A missing cache TOTAL is a size-reporting gap, not evidence that the slot is in use. Should it ever block DELETABLE, or only leave the size unknown?
- If the allowances stay, make them deterministic under load (a listing budget rather than a wall clock), or let the operator raise them for one run, and say in the reason how to.
- Python and Rust editions alike, or record the parity gap.

# Acceptance Criteria

- A test with a large cache tree under a forced-small allowance shows the chosen row verdict, the same in both editions.
- The 5-second wall allowance no longer decides a verdict under load (test with an injected slow listing).
- The reason text tells the operator how to get a complete answer.

# Notes

Reported by the lead (devbig014) on 2026-11-08 from a consumer's audit output. Related: #1102 wrkslots-census-churn-tolerant, #1107 audit-format-label-mismatch.
