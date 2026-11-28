---
title: 'search-panel-design-followups: the stale "Filtered out" chip after a clear, two X glyphs in the search panel, the phone magnifier over a row''s Done, and the Discuss sheet jumping as a search clears'
status: open
priority: 1
issue_type: bug
assignee: cl-searchux
labels:
- vibe-talk
- search
- owner-ui
created_at: 2026-11-27T23:40:00-05:00
updated_at: 2026-11-27T23:40:00-05:00
---

# Description

Four defects already on main, found by the advisory design review of `#2423` search-escape-clear-moves-found-message (ui-rules R1-R17) in its screenshot matrix (fixture server, desk 1280x800 and phone 412x915, light and dark, keyboard open and closed, Discuss open). None was introduced by `#2423`; each shows in main's stills of the same walks.

- R5 (status as a chip, never stale): after a search is cleared, a floating "Filtered out — the nearest is here." chip still stands at the foot of the list. It was written when the search hid the selected message (the place-keeper's substitute sentence); once the clear brings every row back, the sentence is no longer true. Seen on desk after-clear stills in both themes.
- R8 (one glyph per meaning): the open search panel shows two X glyphs for two jobs, a blue × inside the field (clear the query) and a black ✕ at the end of the tool row (close the search). Every search-open still, phone and desk.
- R9 (8 px between targets): on a phone, the floating search button (about 35 CSS px across) overlaps the top row's "Done" chip after a clear; the gap between the two targets is negative.
- R12 (the reader's place): on a phone with the keyboard open and Discuss open, clearing the search brings the footer back and the Discuss sheet's top edge jumps (CSS y 459 to about 575), over the list the reader was reading. The found message itself holds (`#2423`); the sheet moves over it.

# Design

- R5: the clear (or any change that shows the selected message again) dismisses a substitute sentence it no longer makes true; the place-keeper writes it only while the selection is hidden.
- R8: one dismiss glyph across the panel; the field's clear and the panel's close are told apart by place and label (accessible names "Clear search" and "Close search"), per R8's single-glyph rule.
- R9: the floating search button keeps 8 px clear of the first row's controls on a phone (the list's head room, or the button's own place), checked at 412x915.
- R12: the Discuss sheet keeps its top edge through the footer coming back as the search clears with the keyboard open, or the footer's return is held until the keyboard closes; measure on a real visualViewport trace before choosing.

# Acceptance Criteria

- A browser walk per item (fixture server, never the live server) that fails on main and passes with the fix: no "Filtered out" chip after a clear that shows the selected message; one X glyph in the open panel; >= 8 px between the phone's floating search button and a row's Done; the Discuss sheet's top edge within 0.5 px through a keyboard-open clear.
- Owner-UI matrix: desk and phone, light and dark, keyboard open and closed, Discuss open, with before and after screenshots and BUILD.txt; the advisory design review.

# Notes

- Filed at the coordinator's request after the liaison accepted `#2423` (follow-up of its design review; issue file only). To be fixed after `#2424`.
- The review and its crops: the `#2423` evidence (design-review.md and crops/ of that lane's evidence).
