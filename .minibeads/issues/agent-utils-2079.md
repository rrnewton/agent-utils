---
title: 'agent-log-archive-incremental-capacity: the fetcher refuses an incremental pull that would fit'
status: open
priority: 2
issue_type: bug
labels:
- agent-log-archive
created_at: 2026-11-21T18:00:00-05:00
updated_at: 2026-11-21T18:00:00-05:00
---

# Description

With --min-free-bytes N, scripts/agent-log-archive/fetch_agent_logs.py refuses a root when the probe's estimate of the WHOLE source tree exceeds (free bytes - N), in both the initial and the refreshed capacity check of execute_transfer_operation. The bytes the archive already holds for that root are not subtracted (the destination is scanned only after the check), so a mostly archived root is judged as if copied from scratch. A consumer saw a root refused for an 88 GB source estimate when its measured net growth was about 7.3 GB, well inside the headroom.

# Design

Compare the headroom with the expected growth: the source estimate minus the destination's regular bytes already present for that root (floored at 0), or the rsync dry-run delta the tool can already compute. Keep the refreshed second check. Report both numbers in the refusal.

# Acceptance Criteria

- A mostly archived root whose growth fits the headroom is pulled; a root whose growth does not fit is still refused.
- Deterministic tests with a fake probe and destination.

# Notes

Filed by the coordinator from a consumer report; the owning lane refines this file as it works.
