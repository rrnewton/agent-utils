---
title: 'vibe-talk-whole-threads-no-duplicate-hits: Whole threads shows each hit twice — once greyed as thread context, once bright as the hit'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- search
- ui
created_at: 2026-11-18T15:31:14-05:00
updated_at: 2026-11-18T15:31:14-05:00
---

# Description

[watermelon-full-preview] Follow-up to #1852 vibe-talk-search-whole-threads-toggle (live since 55dac4d7). Owner request (thread 190): "search is a filter but threads are all-in or all-out ... I want to view those threads in the entirety and interact with them while leaving the filter on."

DEFECT found in review: with Whole threads ON, each matching message is shown TWICE: once greyed in its thread-context position (with the reply arrow) and once again bright and highlighted as the hit, the same message id two times in a row. Evidence: /tmp/coordb-1852-retake2/light-phone-2-toggle-on.png and dark-desktop-4-discuss.png (real server from 862e06d2; driver /tmp/coordb-1852-driver2.py, log /tmp/coordb-1852-retake2.log: the DOM ids are unique, so it is two presentations of one message, not two elements with one id). Toggle OFF shows each hit once.

Owner-priority, deployed on landing.

# Design

With Whole threads on, a matching thread renders ONCE in its entirety, in order. The matching messages are highlighted in place (match marks, bright), non-matching messages in the thread are shown normally (not greyed as "unmatched"), and no second copy of a hit appears anywhere in the list. Keep the off behaviour unchanged. Check the server-hit merge ("Server search details") too: a server hit that is already on screen must not be drawn again.

# Acceptance Criteria

- With Whole threads on, each message id appears exactly once in the visible list and its text appears once; matching messages carry the match highlight in place; non-matching thread messages show normally, not greyed.
- Toggle OFF behaviour unchanged.
- A server hit already on screen is not drawn again by the server-hit merge.
- Browser test on a real server (the #1855 harness or the retake driver as a starting point) asserting the exactly-once property; before/after shots on phone and desktop, light and dark.

# Notes

Prior work: #1852 vibe-talk-search-whole-threads-toggle (landed 55dac4d7) — the search-bar Whole-threads toggle, `setEntireThreads`/`entireThreadHits` in `web/src/view/channel/searchrows.ts`.
