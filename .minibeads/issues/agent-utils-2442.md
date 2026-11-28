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
updated_at: 2026-11-28T12:05:00-05:00
---

# Description

Four defects already on main, found by the advisory design review of `#2423` search-escape-clear-moves-found-message (ui-rules R1-R17) in its screenshot matrix (fixture server, desk 1280x800 and phone 412x915, light and dark, keyboard open and closed, Discuss open). None was introduced by `#2423`; each shows in main's stills of the same walks.

- R5 (status as a chip, never stale): after a search is cleared, a floating "Filtered out — the nearest is here." chip still stands at the foot of the list. It was written when the search hid the selected message (the place-keeper's substitute sentence); once the clear brings every row back, the sentence is no longer true. Seen on desk after-clear stills in both themes.
- R8 (one glyph per meaning): the open search panel shows two X glyphs for two jobs, a blue × inside the field (clear the query) and a black ✕ at the end of the tool row (close the search). Every search-open still, phone and desk.
- R9 (8 px between targets): on a phone, the floating search button (about 35 CSS px across) overlaps the top row's "Done" chip after a clear; the gap between the two targets is negative. Closed, not to be fixed: see Notes (floating controls may cover text).
- R12 (the reader's place): on a phone with the keyboard open and Discuss open, clearing the search brings the footer back and the Discuss sheet's top edge jumps (CSS y 459 to about 575), over the list the reader was reading. The found message itself holds (`#2423`); the sheet moves over it.

# Design

- R5: the clear (or any change that shows the selected message again) dismisses a substitute sentence it no longer makes true; the place-keeper writes it only while the selection is hidden.
- R8: one dismiss glyph across the panel; the field's clear and the panel's close are told apart by place and label (accessible names "Clear search" and "Close search"), per R8's single-glyph rule.
- R9: the floating search button keeps 8 px clear of the first row's controls on a phone (the list's head room, or the button's own place), checked at 412x915.
- R12: the Discuss sheet keeps its top edge through the footer coming back as the search clears with the keyboard open, or the footer's return is held until the keyboard closes; measure on a real visualViewport trace before choosing.

# Acceptance Criteria

- A browser walk per item (fixture server, never the live server) that fails on main and passes with the fix: no "Filtered out" chip after a clear that shows the selected message; one X glyph in the open panel; >= 8 px between the phone's floating search button and a row's Done (closed: see Notes); the Discuss sheet's top edge within 0.5 px through a keyboard-open clear.
- Owner-UI matrix: desk and phone, light and dark, keyboard open and closed, Discuss open, with before and after screenshots and BUILD.txt; the advisory design review.

# Notes

- Filed at the coordinator's request after the liaison accepted `#2423` (follow-up of its design review; issue file only). To be fixed after `#2424`.
- R5 resolved (2026-11-28, cl-filternote, owner-UI fast track at liaison 1156: "announce, don't float; and it shouldn't survive the clear at all"): the filter place's substitute sentences are said in the polite live region and floated over no row, only while the selection is hidden, and the change that shows the selected message again takes them away (commit "vibe-talk: the filter place's "Filtered out" sentence is announced, never floated, and goes with the clear that shows the selection (#2442 R5)"). Walk: `tests/search_clear_note_browser.py` (desk, desk with two columns, phone with the keyboard closed and open, Discuss open; every clear path; fails on main before it). R8 and R12 stay open.
- R5 live in 458854c6 (2026-11-28, cl-filternote): landed 11:33 AM ET (gate on the pre-replay commit: one unrelated step failed, `vibe-talk.check.bridge-even-browser`, a hover read under load, unreproduced in 26 local runs, passed on its rerun; walk fix `#2473`) and deployed about 11:39 AM ET with a clean watch. On the landed page afterwards: `tests/search_clear_note_browser.py` 16 of 16 walks pass, `tests/search_clear_stays_browser.py` 51 of 51 plus the two-column walk (worst drift 0.48 px, limit 0.5).
- Floating controls may cover text; floating notes may not. Owner decision 2026-11-27 on `#2314` (floating controls covering message text: the freshness pill, the phone's corner Send): "these kind of floating controls necessarily do that". The coordinator applies it to every floating control (the floating search button, the "Newest (N)" jump pill), over a row's text or its controls, so design reviews stop re-flagging them; a floating note (a status sentence, a toast) still never stands over a row (R5 above, `#2350`, `#2365`, `#2375`). R9 is closed on that basis (2026-11-28, the coordinator's routing of the R5 design review, which also found the button over the top row's Done at a desk with two columns and over a row's body text on a phone with Discuss open), and so is the folded `#2424` R9 below (the "Newest (N)" pill over the "N in main" bucket chip: a floating control over a chip, not a note over row text).
- The review and its crops: the `#2423` evidence (design-review.md and crops/ of that lane's evidence).
- Three more, from the advisory design review of `#2424` search-view-unstable-while-searching (also in the stills of the page before that change; folded in at the coordinator's request):
  - R12: the row the place-keepers treat as the one being read (the first row below the floating panel, `floatingClearance`: the freshness pill and the search bar) can stand under the search's bucket-chips sheet, which that clearance does not count: wholly on a desk, about 94 of its 146 CSS px on a phone, after typing near the foot.
  - R9: on a phone with Discuss open, the "Newest (N)" jump pill covers the "N in main" bucket chip. Closed on the floating-controls rule above.
  - R5 and R7: on a desk the count line ("29 found (23 loaded, 6 older)") wraps to four lines beside the search bar's tools.
  Each joins the acceptance criteria as a walk that fails on main: the row being read stands clear of the chips sheet, >= 8 px between the pill and the chips, the count line on one line (or a chip) at 1280x800.
