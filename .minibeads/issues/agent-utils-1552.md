---
title: 'discuss-docked-height-drag: drag the "Discuss agent" title bar to resize the docked pane'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-14T19:10:00.000000000+00:00
updated_at: 2026-11-14T19:10:00.000000000+00:00
---

# Description

Owner UI request 073 (fast-track, owner rule 286): when Discuss is at half
height (the docked pane from #1526 discuss-docked-pane-mobile), grab the
"Discuss agent" title bar and drag it up and down to adjust the pane's
height — the split between the message list above and Discuss below. Use
case: jump to a message (citation link), then make room to read it.

# Design

- **Handle.** The `.discuss-head` title bar (`#discuss-head`) is the drag
  handle, only while the pane stands at half height (`data-sheet="half"`).
  Pointer events (`pointerdown`/`pointermove`/`pointerup`/`pointercancel`,
  capture taken only once the drag is real) so mouse, touch and pen share
  one path, following #1186 two-column-splitter and the #1344 width drag.
  On the phone regime the bar gets `cursor: ns-resize` and
  `touch-action: none`, so a touch drag never scrolls the page or the list
  underneath. Mouse drag works wherever the docked pane is a top/bottom
  split (a narrow desktop window in the phone regime included); the desk
  overlay and the side-docked column keep their existing width drag.
- **Buttons are never drags.** A press that starts on a bar button
  (maximize/X/dock) starts no drag, and movement under 6px is a tap: no
  height changes, nothing is kept, and the button's click still lands.
- **Bounds and snaps.** The height is a viewport percentage: default 50,
  clamped 20–85 (`clampDiscussDockedHeight` in `core/settings/bounds.ts`).
  20% still shows the title bar plus a few lines and the bottom Summarize
  bar (#1527); 85% still leaves a few list rows. On release the height
  snaps within 3 points of half to 50 and within 2 points of either end
  to that end. Dragging fully up never enters a new state: it stops at
  85, and the maximize button remains the one way to full height, so the
  half/full states of #1344 stay consistent. Double-click restores 50.
- **Persistence.** Per device as `DEVICE_SETTINGS.discussDockedHeight`
  (`vibe-talk.voice.discuss-docked-height`), kept once at the end of a
  drag, beside the width/sheet/mode choices of #1344/#1354, so it sticks
  across close/reopen and reload. The panel applies it as
  `--discuss-docked-height`, which the #1526 docked grid row, the sheet's
  own height and the floating chips' offset all read; because that row
  still ends `#scroll-area` at the pane's top at any height, the #1526
  citation jump lands in the visible list with no jump-code change, and
  the #1527 bottom bar rides the pane's bottom as before. Where no kept
  choice is handed in, the panel reads/writes the same device key
  directly (the page glue in `voice.js` is untouched, per the lane's
  constraint), so the choice still sticks.
- **Keyboard.** The bar is focusable with `role=separator`,
  `aria-orientation=horizontal`, `aria-valuemin/max/now` (20/85/current):
  ArrowUp/ArrowDown resize by 2 points, PageUp/PageDown by 10, Home/End
  go to the ends, each kept at once.
- **Files.** Bounds and device entry in `web/src/core/settings/`, the
  drag owner in `web/src/view/catchup/panel.ts` forwarded by
  `view/catchup/index.ts`, the bar's id and separator attributes in
  `web/voice.html` and `web/src/view/dom/ids.ts`, the variable height in
  `web/styles/voice/22-discuss.css`. No change to `vibe-talk/web/voice.js`
  or `web/src/app/main.js`, and no collision with #1398's width handle
  (a different control on the message column).

# Acceptance Criteria

- Phone (~390px): dragging the title bar up/down resizes the docked pane
  live, the scroller still ends at the pane's top, and a citation tap
  lands its row fully in the visible list at the dragged height.
- A tap on the bar (under the threshold) and any press on its buttons
  never drags; maximize/X keep working, and dragging the full-height
  sheet's bar changes nothing.
- The height clamps at 20/85, snaps near half and the ends on release,
  double-click restores half, and the choice persists per device across
  close/reopen and reload.
- With the bar focused, ArrowUp/ArrowDown resize and `aria-valuenow`
  reports the height.
- Unit tests pin the default, clamp, snap, drag-keeps-once, threshold,
  keys and persistence; the browser walk drags the bar at 390px and
  asserts the geometry, keeping and citation landing, with condition
  waits only (owner rule 346).

# Notes

Owner UI request 073; prior work reused: #1186 (splitter drag shape),
#1344 (width drag, per-device keep, half/full sheet), #1354 (modes),
#1526 (docked pane — its grid row now reads the variable height),
#1531 (the slim "Discuss agent" bar that becomes the handle),
#1527 (bottom Summarize bar, untouched inside the pane), #1398 (width
handle, untouched).
