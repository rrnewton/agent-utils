---
title: 'vibe-talk-folded-card-preview-clamp-overlap: folded bullet preview draws lines on top of each other when selected'
status: closed
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-24T06:54:00-05:00
updated_at: 2026-11-24T12:11:24-05:00
closed_at: 2026-11-24T12:11:24-05:00
---

# Description

Owner-visible (fast-track, rule 286): in a selected folded
(clamped-preview) message card, two lines of text are drawn on top
of each other inside the clamped preview. Visible on the owner's
phone in any selected bullet card: in screenshot scene 92
(android-412, dark) the third bullet's lines ("the loaded-host repro
is understood and has ..." over "... has an issue filed against it,
with both logs / ... you say so.") paint at the same vertical
position. Pre-existing: identical on c4174fb3 and e0edfbb3, so it is
not a regression from the chip-row work over the same cards.

Suspected area: the folded-card preview clamp CSS or layout
(`-webkit-line-clamp` / `line-clamp`, `overflow`, an absolutely
positioned fade or ellipsis, a selected-state style, or bullet-list
margins interacting with the clamp). Touch only the preview clamp,
not the chips.

# Acceptance Criteria

- A clamped preview never overlaps lines in any state: selected or
  not, bullets or plain text, light or dark, phone or desktop,
  keyboard open or closed.
- A page test detects overlapping line boxes inside a clamped
  preview (compare the client rects of text lines) and fails on the
  old CSS, passes on the fix.
- Dynamic testing on a local test instance through the real server,
  with the screenshot matrix (before and after, the selected bullet
  card in focus) to the liaison before landing.

# Notes

Pre-assigned #2203 vibe-talk-folded-card-preview-clamp-overlap.
Coordinate with the lane whose chip row sits over the same cards;
touch only the preview clamp, not the chips.

## Resolution

[watermelon-full-preview] Root cause: `-webkit-line-clamp` lays the
lines past the clamp collapsed at the clamp line's position, and
`overflow: hidden` made the clamped preview a scroll box, so a
selection reveal (scroll-into-view or a drag selection) scrolled
those collapsed lines into the visible box, where they painted on
top of each other. Fixed in `34ac4ffc` on main by changing only
`overflow: hidden` to `overflow: clip` on `.body.clamped` in
`vibe-talk/web/styles/voice/07-long-messages.css`: the preview clips
identically but can no longer be scrolled by any means, so the
collapsed lines can never enter the box. Clamp geometry, heights,
and the trailing ellipsis are unchanged.

A new browser check, `vibe-talk/tests/clamp_overlap_browser.py`
(registered in `vibe-talk/tests/dag.d/180-clamp-overlap-browser.yaml`),
compares the client rects of the text lines standing inside a folded
preview, folded and after a scene-92-style selection; it fails on the
old CSS in the selected state and passes on the fix at phone and
desktop sizes. The full validation gate passed on the candidate, the
liaison approved the before/after screenshot matrix (selected bullet
card in focus, light and dark, phone and desktop, keyboard open and
closed), and the coordinator's post-deploy watch was clean.
