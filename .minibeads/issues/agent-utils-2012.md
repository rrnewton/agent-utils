---
title: 'vibe-talk-offline-cache-phone-360-arrow-head: coalesce walk hit-tests a head settled below the scrollport'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
- ui
- browser
created_at: 2026-11-20T18:00:00-05:00
updated_at: 2026-11-20T18:00:00-05:00
---

# Description

[watermelon-full-preview] `vibe-talk.check.offline-cache-phone-360-browser` is intermittently red on main: "phone-360, before a tap on its head, the agent's reply under a short reply to a message elsewhere: a tap on 216's head at (55.1, 652.2) lands on div.solo". Local reproduction at 360x800 (CoalesceApi fixture) failed 3 of 12 isolated coalesce runs, always identically. Verdict reported before fixing: a test flake, not a page bug — the bottom dock is a static layout sibling below `#scroll-area` (scroll-area bottom 651.06 equals dock top 651.06) and never covers a visible arrow; on failing runs the walk centres row 216, waits a fixed 120ms, and hit-tests a head that has settled 24px below the scrollport (arrow at y 675.2, `scrollTop` 1269 versus 1651 when centred), where `elementFromPoint` correctly returns the dock painted at that viewport line.

# Design

- In `vibe-talk/tests/offline_cache_browser.py`'s coalesce walk, replace the fixed centre-and-wait with a condition-based wait: centre the row, wait until its arrow (or, for arrowless bridge rows, its top) stands inside `#scroll-area`'s visible band, re-centring while it settles outside (bounded retries).
- Keep the hit-test strict once the row is in view: `head()` re-checks the band before `HEAD_HIT_JS` and only retries when the row has drifted out of the band, never when an in-view head is genuinely covered.
- No page change, no fixed sleeps added.

# Acceptance Criteria

- The coalesce walk at phone-360 (360x800) passes 12 of 12 consecutive local runs.
- The full `offline-cache-phone-360-browser` check passes, and the gate on the fix commit is green (or any unrelated red is named with its cause).
- No weakening of the head hit-test: an in-view covered head still fails.

# Notes

[watermelon-full-preview] mu-discussfix, after #2010 vibe-talk-discuss-browser-red-on-main (b908f455). Number 2012 pre-assigned by coordb. Evidence: failing run arrow (13.0, 675.2) 44x44, row 216 top 674.2, `#scroll-area` 360x651.1 `scrollTop` 1269, `#dock` top 651.1, `elementFromPoint` at (55.1, 652.2) and (51.8, 652.2) returns `div.solo` (`#control-pane.solo`); passing run `scrollTop` 1651, row at y 292.2, hit is the arrow itself.
