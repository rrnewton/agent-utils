---
title: 'vibe-talk-readerbar-sound-overlap: Sound toggle covers the cloud/device voice selector in the phone reader bar'
status: closed
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-16T12:36:00+00:00
updated_at: 2026-11-16T12:55:00+00:00
closed_at: 2026-11-16T12:55:00+00:00
---

# Description

[watermelon-full-preview] Owner report 2026-11-16 7:36 AM ET (Android phone, mobile page): in the bottom reader bar's left column, the SOUND on/off toggle box sits on top of the cloud/device voice selector — only the top of the cloud icon shows beneath SOUND. Both controls must be fully visible and tappable at phone widths (~400 CSS px), with no overlap at any font scale up to 130%.

Root cause: `#1355 phone-bar-discuss-slot` (01aa6c67) placed `#audio-source` at `grid-column: 1; grid-row: 1 / span 2` in `#control-pane`, the cell `#speaker` (row 1) occupies while reading; `.control` is positioned, so Sound paints over the selector. Desktop hides Sound in this state; the phone tile does not.

# Design

Stack the two in the left column with real spacing: Sound keeps row 1, the voice selector takes row 2 — the row Clear vacates while reading — so both keep the full 3.5rem column width and the grid gap separates them. Chosen over merging into one segmented control: the two are independent binary choices (mute vs voice engine) with their own pressed/switch semantics, and a merge would be a new control, not a layout fix. CSS only, no new features.

# Acceptance Criteria

- At ~412x915 CSS px (device scale 2.6) the bounding boxes of `#speaker` and `#audio-source` do not intersect and each centre is hittable (`elementFromPoint` returns the control).
- The same holds at 130% root font size.
- Desktop layout is unchanged.
- A browser test covering the above is kept with the page tests.

## Resolution

[watermelon-full-preview] Delivered with this closing commit. `#audio-source` now takes row 2 of the reader bar's left column, under Sound in row 1, instead of spanning both rows into Sound's cell; the grid gap separates them and each keeps the column's full width. Stacked rather than merged into one segmented control: Sound (a mute) and the selector (a voice-engine switch) are independent binary choices with their own pressed/switch semantics, and a merge would be a new control, not a layout fix. Clear holds row 2 only in a call, when the selector is hidden, so no state contends for the row. Desktop is unchanged: its regime already hides Sound while reading and lays the pane out as a row, where the grid row does not apply.

Evidence: `vibe-talk/tests/readerbar_overlap_browser.py` (new, wired as `check.readerbar-overlap-browser`) fails on the old sheet with a 56x38px intersection at 412x915 and passes with the fix at 412x915 (device scale 2.6, touch), again at a 130% root font, and at 1280x800 for the desktop pane; each control's centre hit-tests to itself and a real tap flips its own state. Phone screenshots before and after were read directly: before, SOUND covers the selector with only the cloud's top showing; after, both stand whole with spacing between them.
