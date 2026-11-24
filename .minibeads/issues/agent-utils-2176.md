---
title: 'vibe-talk-discuss-browser-family-root-cause'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- flaky-test
created_at: 2026-11-24T06:00:00-05:00
updated_at: 2026-11-24T06:00:00-05:00
---

# Description

The Discuss browser family (discuss-browser, discuss-dock-browser, discuss-strike-live-browser, discuss-readjump-browser) is red on 80+ distinct gate heads in 7 days, with wait_for_function timeouts as the sampled signature and clean passes in recent kept step logs. One shared fixture/panel path serves the family, so one investigation should find whether this is fixture timing, a panel behaviour under load, or a real product defect.

Sampled kept logs show timeouts or passes, never a failing product assertion — so today there is no proven product bug here; this investigation is how one would be found if it exists.

# Acceptance Criteria

- A named root cause per family member (fixture, harness, or product), with either a fix or a recommendation to keep or drop its #2175 known-red entry.

# Notes

Filed by the coordinator from the browser-red plan (mu-clientperf, 2026-11-24); the owning lane refines this file as it works.
