---
title: 'agentctl-turnover-pin-recheck: re-read turnover pins right before touching each unit'
status: open
priority: 1
issue_type: bug
labels:
- agentctl
created_at: 2026-11-22T11:25:00-05:00
updated_at: 2026-11-22T11:25:00-05:00
---

# Description

A services turnover planned its units, then a consumer wrote a pin for one of them about half an hour later, while the turnover was still waiting on another unit. When the turnover reached the pinned unit it started work on it anyway and rewrote the unit's start command to the new snapshot before an operator stopped the turnover by hand. For a boot-restore unit, a start could archive a live chat-bridge binding.

# Design

Re-read the pin files for a unit immediately before any action on it (drop-in rewrite, start, restart); a pin found at that point skips the unit and reports it, as at planning. A turnover that waits a long time between units re-checks every pending unit's pin before each one. Report the skip in the run's notices.

# Acceptance Criteria

- A test writes a pin after planning and before the unit's turn; the turnover leaves that unit's drop-ins and state untouched and reports the skip.

# Notes

Found on one host on 2026-11-22; filed by the coordinator.
