---
title: 'reading-width-handle-gate: the reading-width handle stands on the message column and only past its widest width'
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-12T20:10:00+00:00
updated_at: 2026-11-12T20:10:00+00:00
---

# Description

Owner (Slack 020, 3:10 PM ET): "the vertical bar that is supposed to control the WIDTH of the main message view... that is totally messed up. When we go into docked Discuss view it stays hovering OVER the discuss view uselessly. When we have a sufficiently squeezed main column we should NOT show the width adjuster. It should just be fixed at 'full width'. It is only over a certain size that we should show the option of artificially narrowing the text width. If we have a GIANT super wide monitor MAYBE we would still exceed that width threshold in two column view or even two columns PLUS docked discussion side bar (effectively 3 columns)."

The handle (`#width-grip`, `mountReadingWidth`) was positioned against the screen (`left: 50%` of `#screen-main` plus half the reading width), so docking Discuss — which gives the panel its own grid column inside `#screen-main` — left it standing over the panel. It was also hidden by a blanket rule in two columns, whatever the column's actual width.

# Acceptance Criteria

- The handle stands on the message column's own right edge in every layout; it never overlaps a docked Discuss panel or the second column.
- It shows only when the message column's actual rendered width exceeds the threshold (see Design), re-evaluated on resize, on docking/undocking/maximizing Discuss, and on switching between one and two columns. Below the threshold there is no handle and the text is full width; a remembered narrower width is kept on the device but not applied, and returns when the column widens again.
- On a very wide monitor, two columns, or two columns plus a docked panel, may still qualify; then the main column is capped at the remembered width again with its handle on its edge. Only the main column gets a handle; the side column keeps full width.
- Unit tests pin the threshold decision (fed measured widths) and the positioning; browser walks measure one and two columns with Discuss closed, overlaying and docked at 1280x800, 1920x1080 and 3440x1440.

# Design

- Threshold: `MAX_READING_CH` (120ch, the widest width the setting itself allows in `core/settings/bounds.ts`), rendered in the column's own font. A column no wider than the widest allowed reading column gains nothing from artificial narrowing, so below it the column is applied at full width (the widest allowed value, which cannot cap a column that size) and the remembered width is untouched in storage, the slider and the handle's value.
- The column is measured, never the viewport: a `ResizeObserver` on the message column (and its panes) plus a `MutationObserver` on the screen's `data-discuss`/`data-columns` re-place the handle, whose left edge the script sets from the shown pane's measured right edge. Pixels-per-character come from the font itself (ten zeros in the pane's font), so the decision stays true while a pane spans its column uncapped in two columns (`#373`) or stands at full width under the gate.
- While the gate says "shown" in two columns, the stylesheet re-caps the main pane at the reading width (`data-reading-grip="shown"` on the screen); below the gate the `#373` span stands. A maximized Discuss panel hides the handle. Only the main (left) column qualifies for a handle.
- The fix lives in the reading-width owner (`view/settings/screen.ts`) and its stylesheet (`20-desktop-regime.css`). Note the liaison's pointer to `view/channel/splitter.ts` names the two-column separator, a different control; it is untouched, as are the Discuss panel and its grid.

# Notes

Reserved as #1398 for the evening Discuss batch. Neighbours: #1397 mu-dockclose (docked-close grid), #1395 mu-discussbind, #1386 mu-discussui — no shared files with this change. Screenshots from the browser walks are linked in the landing report to the coordinator.
