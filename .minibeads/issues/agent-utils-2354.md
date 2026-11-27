---
title: 'two-column-search-replies-place-left-scroll: the two-column check sees the left column scroll by 184px during the cards walk on unrelated heads at normal load'
status: open
priority: 1
issue_type: bug
assignee: cl-flaketriage
labels:
- vibe-talk
- tests
- flaky-test
created_at: 2026-11-27T00:23:00-05:00
updated_at: 2026-11-27T00:23:00-05:00
---

# Description

`vibe-talk.check.two-column-browser` (`vibe-talk/tests/two_column_browser.py`, `search_replies` in place mode, `replies_place`) failed twice in an hour on two unrelated heads with the same assertion and the same numbers:

- gate `3b18855a`, head `8cfd09e0`, finished 2026-11-26 11:28 PM ET (with `collapse-anchor-browser` and `page-3`); the same head's next gate was green.
- gate `0733039c`, head `dd2ef05e` (`#2324`, which changes only `vibe-talk/scripts/screenshots.py`, which this check does not run), finished 2026-11-27 12:20 AM ET, load1 107 to 151 for the whole gate (under the hot bar of 237).

The assertion: `search-replies-place-1280: the cards' walk scrolled the left column: {'top': 243.90625, 'right': 338, 'left': 379, ...} {'top': 243.703125, 'right': 384, 'left': 563, ...}`. The reader scrolls the right column's cards, presses "Search replies before", and the walk adds three threads; the check then requires the left column's `scrollTop` (`#scroll-area`) to be unchanged within 1px, and saw it grow by 184px. The same numbers on two unrelated heads at normal load point to a real race in the check or the page, not to overload.

# Design

First decide which it is. `scrollTop` alone cannot say whether the reader saw the left column move: if the walk's threads enter the left list above the rows on screen, the page (or Chromium's own scroll anchoring, which the check leaves on for the left column) raises `scrollTop` by their height while the reader's rows stay put. Repeat the step on its own and on a main control, and record, before and after the walk, the first left-column row on screen and its offset from the column's head.

- Rows on screen unchanged (only `scrollTop` moved, by the height of rows inserted above): the check measures the wrong thing. Make it compare the reader's place in the left column (the first row on screen and its offset), as it already does for the right column's card, and wait for the walk's rows to settle on a condition rather than a fixed pause.
- Rows on screen changed: the page moves the reader's left column during a walk in the right column. That is a reader-visible scroll (owner-UI class, rule 370 scroll stability) and goes to the coordinator as a product bug; the check stays as it is.

# Acceptance Criteria

- The cause is named with evidence: the step on its own and a main control, with load1 recorded per run, and the left column's first row on screen before and after the walk for at least one failing run.
- If the check is at fault: it compares the reader's place in the left column, passes 10 consecutive runs on its own, and still fails when the page moves the rows on screen (a negative control).
- If the page is at fault: a product issue is filed through the coordinator and this check is not weakened.
- No wall-clock budget added (owner rule 346).

# Notes

Assigned to `cl-flaketriage` by the coordinator (2026-11-27). The left column's place is guarded by owner rule 370 (scroll stability), so this check is not disabled without liaison clearance.
