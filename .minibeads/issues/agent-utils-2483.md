---
title: 'labels-redraw-drops-focus: the Labels drop-down''s refresh redraw removes the focused option, so a fast keyboard pick lands nowhere'
status: open
priority: 3
issue_type: bug
assignee: cl-filternote
labels:
- vibe-talk
- tests
created_at: 2026-11-28T12:35:00-05:00
updated_at: 2026-11-28T12:35:00-05:00
---

# Description

`vibe-talk.check.labels-browser` (`vibe-talk/tests/labels_browser.py`) failed in the full gate of `#2453` with "desk-light: Enter on the first option did not pick Pinned". Its keyboard step presses the Labels button, then ArrowDown, ArrowDown and Enter with no wait between them. It failed the same way without `#2453` (the walk passed 16 of 17 runs on the `#2453` commit and 16 of 17 on the tree without it, both failures "phone-dark: Enter on the first option did not pick Pinned"; load1 197 to 261), and `#2453`'s change cannot run in that walk (its channel has eight messages, all loaded).

From the code (not yet proven by a trace): opening the drop-down (`labelsfilter.ts`, `setOpen`) draws the options, focuses the filter box, and starts `refreshLabels()`, whose answer draws the options again. `draw()` rebuilds every option with `list.replaceChildren(...)`. If that second draw lands after ArrowDown has moved the focus to an option, the focused element is removed, the focus falls to the page body, and the next ArrowDown or Enter goes nowhere. So it is a page bug as well as a walk race: a keyboard user who opens the drop-down and presses Down and Enter quickly can find nothing picked, and has to start again. The walk's timing only decides whether the refresh lands between its keys.

# Design

- Page: keep the reader's place across the refresh redraw. Either redraw only when the refreshed labels differ from the ones drawn, or rebuild and then put the focus back on the option for `activeValue` (or the filter box, if that held it) when the focus was inside the listbox before the redraw.
- Walk: the keyboard step waits for the panel's refreshed draw (a marker the panel sets when the refresh's draw is done, or the listbox settled over two frames) before its keys, and checks where the focus is after each key.

# Acceptance Criteria

- A page test (tests/js) that opens the drop-down, moves the focus to an option, lets the refresh answer arrive, and finds the focus still on that option (fails before the page fix).
- `labels_browser.py` passes 20 of 20 at once (8 or more copies concurrently) on a host at load1 >= 237.
- Owner-UI evidence is not needed for the page part (no visible change), but a desk and phone keyboard walk of the drop-down shows the same picks.

# Notes

- Filed at the coordinator's request after the `#2453` gate (number pre-assigned); after `#2453`, with `#2473`. Evidence: the `#2453` gate log of `vibe-talk.check.labels-browser` and the A/B runs in the `#2453` lane's report.
