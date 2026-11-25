---
title: 'vibe-talk-reading-row-stays-put: the message being read keeps its screen position through refreshes and inserts below it'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
- scroll
- owner-ui
created_at: 2026-11-25T06:28:00-05:00
updated_at: 2026-11-25T12:48:40-05:00
closed_at: 2026-11-25T17:48:40.000000000+00:00
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

## Resolution

Landed on main as c8896041 in the stack range ending e637f97c (with #2233 and #2242, one owner gate on the stack top), deployed 12:33 PM ET 11-25, and the deploy watch came back clean (12:33 to 12:48 PM ET: no errors, no restarts). The same patch passed its own owner gate as 319969b4 (11:34 AM ET, no failed steps) before it was rebased unchanged onto the stack. Nothing that arrives moves the row being read, at the foot or mid-list: a mutation re-read, a visible catch-up, a replay, a live insert (threaded or not) and a refresh, pull-up refresh included, all keep the reading row's top where it was (dy 0) and show or update the Newest chip. The three follow paths were the post-read settle, the live-insert path and the right column's foot keeper. Still follows: the reader's own composer send exactly as #1983 specifies (no move outside the fitted keyboard), a confirmed approval-card send at the foot, a Newest tap, and End or Ctrl+End. The reader's own messages, composer or card, never count as new. The voice-call transcript is unchanged. Page tests cover each arrival kind at the foot and mid-list, with threads on and off, at phone and desktop sizes, and the own-send paths (optimistic draw, live read, mutation re-read, catch-up). They fail on the code before the fix, and the column-overscroll negative controls still fail for the new policy's reason. Evidence: real-browser top-anchor checks measured 94 px of movement before the fix and 0.0 px after. Screenshot scenes 104 to 106 (before and after the same arrival, with the reading row's text and top in a sidecar) were taken in light and dark at phone 412 and laptop 1280 from the gated tree, the stack top and landed main, each folder with BUILD.txt, and sent to the coordinator; the matrix scenes 73 to 76 were taken on the branch before its gate. Scene 106 shows dy 0.0 with the "1 new" chip in every combination. The stack-top shots are byte-identical to the gated tree's. Landed main differs from the stack top only by one colour level on rounded corners outside the list. The windowed renderer's coverage shift, which can still lose the reading row in shrink-anchor fuzz runs (opt-in, off by default), is #2239.
