---
title: 'vibe-talk-search-landed-reader-follows-foot: a reader the search entry took to the newest hits follows the foot as the search line and chips change height'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-26T22:20:00-05:00
updated_at: 2026-11-26T22:20:00-05:00
---

# Description

Found while tracing #2260 vibe-talk-search-barely-overflowing-list-clamp on the real throwaway server (the screenshot harness channel, phone 412x915). A reader who is NOT at the head (scrollTop 428 in a list overflowing by about 1860 px) types the first key of a search whose results leave the list a few pixels taller than the screen. With no hit on the screen they typed at, the entry landing takes them to the newest hits (#1734), as intended. After that the reader counts as following the foot: as the search's own line and the chips change height in the next frames, the list follows the foot and the browser clamps it, so the row being read moves 51 -> 20 -> 5 px over about ten frames (reading row worst 46 px). Owner request 370: nothing may move the row being read. #2260 fixed the same shape for a reader at the head; this is the landed reader, which #2260 leaves to #1734 on purpose.

# Design

- The entry landing places the reader once, on the list as it will stand after the search's own height changes (its line at the foot, the chips' room), or holds the landed row by its top instead of following the foot afterwards, so no later keeper or restore moves it.
- #1734's rule stays: a reader with no hit on the screen they typed at still lands at the newest hits.

# Acceptance Criteria

- A per-frame trace on the real throwaway server (phone 412x915 and desktop 1280x800, light and dark) of a search's first key from a reader mid-list with no hit on screen, results a few pixels taller than the screen: after the landing frame, the landed row keeps its top (<= 0.5 px) through the settled answer; main's 51 -> 20 -> 5 px as the negative control, with a JSON sidecar.
- A page test for the landing on a list a few pixels taller than the screen.
- #1734's page test and search_scroll_browser.py unchanged.

# Notes

Owner-UI (owner 370). Filed at the coordinator's request from the #2260 traces (issue file only). Related: #2260, #1734 vibe-talk-search-view-scroll-default, #2262 vibe-talk-search-head-room-two-columns, #2230 vibe-talk-reading-row-stays-put.
