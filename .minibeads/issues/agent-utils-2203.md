---
title: 'vibe-talk-folded-card-preview-clamp-overlap: folded bullet preview draws lines on top of each other when selected'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-24T06:54:00-05:00
updated_at: 2026-11-24T06:54:00-05:00
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
