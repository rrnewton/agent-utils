---
title: 'vibe-talk-reading-row-stays-put: the message being read keeps its screen position through refreshes and inserts below it'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- scroll
- owner-ui
created_at: 2026-11-25T06:28:00-05:00
updated_at: 2026-11-25T06:28:00-05:00
---

# Description

Owner (request 370, 2026-11-25 6:14 AM ET): "I am reading at message x. Below it a refresh occurs. It jumps down. I don't want that. I want my message to stay on the screen and stable. Stuff appears under it if I can see past its bottom edge. Maybe the newest chip appears if it wasn't already there. That's it. Stability."

Invariant: the message being read keeps its exact screen position (rect.top, dy 0) through any refresh, cache-refresh publish, live insert, settle pass or restore that changes content below it. New content may appear below it, visible only past its bottom edge, and the Newest chip may appear. Nothing else moves. Live-log timings at the report point to replacement reads (visible, replay and mutation cursor=newest reads) and a hidden-to-visible return, after which the reading row moved. (No content or ids from the log are recorded here.)

# Design

- No automatic scroll on any arrival: mutation re-read, visible catch-up after a hidden period, replay, live insert or refresh, at the foot or mid-list. Content is applied once at the place content below the reader is applied: measure the reading anchor before, restore after, in the same frame, with no visible frame of drift. The Newest chip appears or updates.
- Still follows: the reader's own send, an explicit Newest tap, End or Ctrl+End. Selection survival and holding the row when not at the foot are unchanged.
- Applies the top-anchor principle of the open top-anchor policy issue to refreshes and inserts, superseding follow-at-rest for arrivals.

# Acceptance Criteria

- A page test asserts the reading row's top stays fixed (dy exactly 0, or under 1 px subpixel) across each event kind landing below it (plain insert below, mutation re-read, visible catch-up after a hidden period, replay), at the foot and mid-list, at phone 412x915 and desktop 1280x800, keyboard closed, condition-based waits only; the reader's own send still follows and the Newest chip appears.
- The test fails on the live build 7352d93f's code as a negative control.
- Dynamic exercise through the real server router (page's exact requests, only external upstreams stubbed) and before/after frames of the same moment around an insert below (phone and desktop, light and dark), with the anchor's text and rect.top in a sidecar; the usual matrix scenes 73 to 76; BUILD.txt in each folder.

# Notes

Diagnose first: prior issues for the top-anchor policy, shrink-anchor policy and scroll stability, and the settle/anchor code paths; whether branches for in-flight anchor work touch the same code. Fix on a branch from github/main; one owner gate; land only on the coordinator's LAND NOW.
