---
title: 'vibe-talk-theme-picker-structure: fold chevrons left as-is, and whether the accent picker folds into Custom'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-24T09:00:00-05:00
updated_at: 2026-11-24T09:00:00-05:00
---

# Description

Two follow-ups from the #2155 theme settings review: the fold chevrons' rendering, and a structural question about the two theme pickers now on the Appearance screen.

# Design

- (a) The fold-toggle chevrons are a border-drawn square rotated on open, shared by every fold on the page and pre-existing the theme editor; leave them unchanged unless a global fold restyle is chosen.
- (b) With premade themes in the per-scheme Theme picker, ask the owner whether the older Colour theme accent-family picker (Blue, Teal, Plum, Amber) should fold into Custom (as its seed) and leave the screen, or stay as a quick accent choice on the Default.

# Acceptance Criteria

- (a) No change unless a global fold restyle is filed separately.
- (b) The owner's answer is recorded here and, if folding in, implemented as its own change: the Colour picker leaves the Appearance screen and Custom seeds from the chosen preset family.

# Notes

Filed by mu-themes at coordb's direction during #2155 A2 review.

## Resolution

- (a) Left as-is: pre-existing on every fold, not a theme-editor defect.
- (b) Open: awaiting the owner's review of the preset list.
