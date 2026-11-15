---
title: 'wrkslots-census-collapse: collapse torn-down PID-namespace init lines in every census, not only status'
status: open
priority: 1
issue_type: bug
external_ref: gh-302
created_at: 2026-11-15T17:47:04.000000000+00:00
updated_at: 2026-11-15T17:47:04.000000000+00:00
---

# Description

Several wrkslots operations (remove, recover-absent-validate-rows, and validate/sweep cleanup) first take a read-only process census under an operation-wide time bound (120 s) and an output bound. For every PID-namespace init being torn down, the census prints a full line, again on each pass. On a host with several such inits stuck for days, operations then fail on the time/output bounds rather than on a real finding. The classification itself is right (such a process cannot hold a checkout); only the reporting, and possibly repeated probing of the same pids, costs the budget. Status output was already quieted (#908 wrkslots-status-quiet-pidns, closed), but the shared narrator still prints per PID for the other census callers, and bounds were raised instead of the repetition removed.

# Design

Classify each torn-down init once per operation, cache the verdict by (pid, start time), and do not re-probe it on later passes. Print one summary line (count plus one example pid), with the full pid list behind a verbose flag, for every census caller, and do not count these lines toward the output bound. Optionally report how long the oldest has been stuck, so a health check can flag a host that needs attention.

# Acceptance Criteria

- A census over several torn-down inits prints one summary line per operation (full list only when verbose), for remove/recover/validate-cleanup censuses as well as status.
- Repeated passes in one operation do not re-probe a cached (pid, start time) verdict.
- Summary lines do not consume the operation output bound; a busy-host fixture that previously refused on the bound now completes or refuses on a real finding.

# Notes

[watermelon-full-preview] Filed from GitHub #302 wrkslots-census-collapse (https://github.com/rrnewton/agent-utils/issues/302) during Request 098 triage. Related: #908 (status only, closed) and open #792 wrkslots-recover-census-refusals (stderr bound and recovery completion).
