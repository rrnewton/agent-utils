---
title: 'discuss-full-drops-to-half-on-link: full-screen Discuss drops to partial on a message link'
status: open
priority: 1
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-15T15:06:00.000000000+00:00
updated_at: 2026-11-15T15:06:00.000000000+00:00
---

# Description

Owner UI request 095 (fast-track, owner rule 286), part 3 of 3: in
full-screen (maximized) Discuss on mobile, clicking a message link in the
summary should switch Discuss back to the partial (docked half-height)
mode so the linked message shows highlighted in the list above. Pressing
the maximize button puts it back to full screen. This lane is
mu-discussdrop; siblings mu-gripleft (#1584 grip to the left) and
mu-linkexpand (#1585 expand the linked message) land on their own.

# Design

- **Drop on a staying jump only.** A message-link tap inside Discuss (a
  summary citation routed by `view/catchup/index.ts` `routeLink`, or a
  footer time in `panel.ts` `drawFooter`) that leaves the panel open
  (`jump-in-place`, `return-to-bound`, footer jumps) first drops the
  panel out of full screen, synchronously on the tap and before the jump
  is handed to the page, so the #1526 scroll-into-view landing is
  computed against the partial-height scrollport. A `close-and-jump`
  (fallback/out-of-scope) tap does not drop: the panel closes as before
  and no partial choice is kept for it.
- **Phone.** A full sheet (`data-sheet="full"`) drops to half
  (`applySheet(false, kept)`), standing in the #1526 docked grid row at
  the docked height the reader last dragged to (#1552
  `discussDockedHeight`, untouched by the drop), so the target row lands
  fully in the visible list above the pane.
- **Desktop (decided).** A maximized panel (`data-mode="maximized"`,
  which covers the list) drops to the mode it was maximized from — the
  existing Exit-full-screen restore (`beforeFull`: docked if entered
  from docked, overlay otherwise) — kept on the device like the toggle
  keeps it. Side-docked and overlay panels already leave the list
  visible and never drop. The maximize/full-screen button then restores
  full screen exactly as it does after a manual exit.
- **One owner.** The panel gains `dropToPartialForLink()`
  (`web/src/view/catchup/panel.ts`, forwarded by `view/catchup/index.ts`);
  the view calls it on staying routes, the panel calls it for footer
  jumps and its no-binding fallback. No stylesheet change (the docked
  row and mode grids already follow `data-sheet`/`data-mode`), and no
  change to `vibe-talk/web/voice.js` or `web/src/app/main.js` (#624).
  The linked message's uncollapse is #1585, inside the landing, after
  this drop.

# Acceptance Criteria

- Phone (~390px), Discuss full-screen with a dragged docked height kept:
  tapping a summary citation or a footer time drops Discuss to half at
  that kept height, the panel stays open, and the target row lands fully
  inside the visible list area, highlighted; the maximize button
  restores full screen.
- Desktop (1280x800), Discuss maximized: tapping a message link drops to
  the pre-maximize mode with the panel open and the target highlighted;
  the full-screen button maximizes again. Overlay/docked taps never drop.
- A fallback/out-of-scope tap while full still closes Discuss and keeps
  no partial choice; a tap at half/overlay/docked keeps nothing new.
- Unit tests pin the drop, the kept height, the restore, and the
  no-drop routes; the browser walk asserts the visible drop and landing
  at 390px and desktop with condition waits only (owner rule 346).

# Notes

Owner UI request 095; prior work reused: #1344 (half/full sheet, kept
per device), #1354 (overlay/docked/maximized, Exit-full-screen restore),
#1526 (docked pane: the scrollport ends at the pane's top, so a jump
lands visibly at any docked height), #1552 (dragged docked height, kept
per device), #1531 (the slim bar carrying the maximize button).
Coordinated with mu-gripleft (#1584: voice.html grip, `.discuss-grip`,
grip assertions) and mu-linkexpand (#1585: landing uncollapse in
navigation/replyarrows/folding, its own browser file): no shared-file
overlap beyond a rebase-order shout.
