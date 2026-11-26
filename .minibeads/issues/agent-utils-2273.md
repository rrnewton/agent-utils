---
title: 'vibe-talk-cross-view-back-lands-wrong: Back after a citation jump from a thread into All returns to the message and view it came from'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
- ui
created_at: 2026-11-25T20:30:00-05:00
updated_at: 2026-11-25T20:30:00-05:00
---

# Description

A reader in a thread view reads a reply and taps a citation link in it to a main-channel message about a day older, which opens in All. They then press Back. They expect to land back in the thread view, on the reply they came from, at the height it stood. Instead the page shows an unrelated message more than a day older, which sits in a different thread.

The jump crosses views: from the thread view (`v=thread`, the thread's ts and the reply as the focused message) into All (`v=all`, the cited message). The live build already carries the same-view Back fix (#2261 vibe-talk-summary-link-back-race: Back ends the landing it undoes) and the search-query fix (#2251 Back keeps the search query), so this is a different path. The older-page load fix (#2264) is not live yet; the cited message may lie beyond the cached window, so an older-page load can be part of the path.

# Design

Step 1, reproduce on a local test instance (never the live one): thread view, then a citation into All to an older message (also one beyond the cached window), then Back by each means the reader has: browser Back, the phone's system Back, and the in-app Back. Use the Back dump and per-frame Back trace from #2261.

Step 2, root cause before any fix: name what Back restores (the URL's view, thread and message; `history.state`; the saved scroll anchor per view; the thread view's own state) and why it picks a message from another thread (a stale anchor saved for another thread, the All view's anchor applied to the thread view, or an older-page load holding the wrong spot).

Step 3, fix: Back from a cross-view jump returns to the view it left, on the row it left, at that row's height, through the scroll owner.

# Acceptance Criteria

- A browser test through the real server runs thread to All, All to thread, thread to thread and All to All citation jumps, each followed by Back by all three means (browser Back, system Back, in-app Back).
- In every case Back lands in the view the jump left, on the row it left, with a per-frame trace within 1 px where placement is on whole pixels (else 0.5 px), at 412x915 and 1280x800, light and dark.
- The test fails on main where the bug shows, and passes with the fix.
- The test waits on conditions, not wall-clock budgets.
- Before and after evidence on a local test instance, each folder with its BUILD.txt.

# Notes

Owner request 377 (2026-11-25 8:21 PM ET): Back after a citation jump from a thread reply into All landed on an unrelated older message in another thread instead of the reply the reader came from. Owner-UI fast-track, owner tier.
