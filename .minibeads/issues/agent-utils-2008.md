---
title: 'vibe-talk-windowed-send-frame-drop: windowed send on the phone drops on-screen rows for one frame and moves the anchor'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- ui
- scroll
created_at: 2026-11-20T17:40:00-05:00
updated_at: 2026-11-20T17:40:00-05:00
---

# Description

[watermelon-full-preview] Pre-assigned follow-up from #1985 vibe-talk-windowed-all-view-fixes, on the combined landed tree measured by the coordinator on 2026-11-20: with windowed lists ON at 412x915, sending in All drops on-screen rows 286-289 for one frame and moves the anchor 11.9px during the send. This is the owner's #1983 All-view send complaint manifesting in windowed mode: a sent row must appear without the surrounding rows disappearing, even briefly, and the reader's anchor must stay put.

# Design

- Reproduce on a local fixture through the real page (the send-flash browser walk's windowed plain phone case is the existing guard: flash frames with rows missing for a frame, plus the anchor move), never the live server.
- Coordinate with the #1983 lane on who owns which code: the frame drop and the anchor move sit in the windowed render path and its stand-down for directly appended rows, or in the send/dispatch follow path, wherever the trace puts them.
- Invariants: zero frames with on-screen rows missing during a windowed send at the foot, and the anchor row within a few px, with windowed on and off.

# Acceptance Criteria

- The windowed phone send case in the send-flash walk shows zero missing-row frames and an anchor move within the invariant, on the landed combined tree.
- Un-mark the two send-flash expected-fails for this issue (remove `expected_fail_2008` in tests/send_flash_browser.py once phone-412-dark-plainwindowed and phone-412-dark-late-readwindowed pass on the combined tree; they were marked at 565e459f by the #1983 lane with coordinator approval, still run and record, and an XPASS under the mark means un-mark).
- Dynamic evidence from a local test instance (walk output and screenshots), focused suites and an owner-priority gate before landing.

# Notes

[watermelon-full-preview] Number pre-assigned by the lead coordinator (2026-11-20). Related: #1985 vibe-talk-windowed-all-view-fixes (landed as b1c721cc; its appended-row stand-down is in this area), #1983 All-view send flash (default path, its lane still held for its own landing).

## Resolution

[watermelon-full-preview] Landed on main as 5633ce89 (2026-11-21): sliceFor now returns the last cap of groups for a reader priced past the end of the offsets instead of an empty range, so a send-landing render can no longer unmount the rows being read for that frame; the two phone windowed send walks lost their expected-fail marks. Send-flash rc=0 on the coordinator's devbig030 run with all 12 walks at 0 flash frames (windowed anchor moves 0.2-0.7px), and windowed_all rc=0.
