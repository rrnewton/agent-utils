---
title: 'compact-rows-swipe-post-race: the compact rows check asserts the swipe''s Done reached the server the instant the row shows archived'
status: closed
priority: 2
issue_type: bug
assignee: cl-flaketriage
labels:
- vibe-talk
- tests
- hot-host
created_at: 2026-11-28T00:40:00-05:00
updated_at: 2026-11-28T01:31:00-05:00
closed_at: 2026-11-28T01:31:00-05:00
---

# Description

`vibe-talk/tests/compact_rows_browser.py` swipes the collapsed row 507 on a phone and then:

- waits, as a condition, for the row's `data-archived` to turn `true`;
- at once asserts that the fake server has logged the Done request (`"dismiss 507" in walk.api.posts`), with no wait.

The page draws the archive before its request reaches the server. On a loaded host the request can still be in flight when the assertion runs. Then the check fails with "the swipe sent no Done for 507: []" although the row shows archived and the request is on its way.

Evidence:

- Gate `1795841057868790835-086e5e2e` (#2427, dbecb0bb, 2026-11-28) failed on exactly this, on one step only.
- The same check ran alone 3 times on main 3700adec and 3 times on dbecb0bb, alternating, at load1 176-197: 6 of 6 passed.
- The step has 61 earlier failures in the gate ledger.

This breaks owner rule 346: a gating check must be robust on a hot host and wait on conditions, not on timing.

# Design

Wait for the condition before asserting it: poll `walk.api.posts` until it holds "dismiss 507", under the walk's existing hang backstop, and fail by name only past it. Every other assertion that reads the fake server's request log right after a DOM change gets the same treatment, and the check is scanned for them.

# Acceptance Criteria

- The check waits for "dismiss 507" to be logged before asserting it. No fixed sleep and no new wall-clock budget.
- The check passes 5 times alone at load1 of 150 or more.
- A negative control: with the page's Done request blocked, the check still fails by name after the backstop.

# Notes

Filed by `cl-flaketriage` at the coordinator's request (2026-11-28), from the #2427 owner gate. Not part of #2427: its residency code cannot reach this walk (8 rows, no page boundary).

The fix (2026-11-28) is in the same commit as this file. The walk gains `posted(entry, why)`, which waits for the fake server to log a request, under the hang backstop. It is used for both such assertions: the swipe's Done for 507 and the Unarchive for 503.
- Alone 5 of 5 at load1 226-241.
- Negative control with the Done never logged: fails by name after the backstop (126 s), "the swipe sent no Done for 507: []".

## Resolution

Fixed by e28099e1 (vibe-talk: the compact rows check waits for the swipe's Done and the Unarchive to reach the server before asserting them). The walk's `posted(entry, why)` waits for the fake server's request log under the hang backstop, for both the swipe's Done for 507 and the Unarchive for 503.

- Gate `1795844620776974561-9132a702` on 46458d22: the changed check passed. The only red was the known-red `vibe-talk.check.discuss-browser` (#2175), under the liaison's STEP-WAIVER.
- Alone 5 of 5 at load1 226-241. The negative control (Done never logged) fails by name after the backstop.
