---
title: "vibe-talk-save-draft-chip-covers-text: the Save draft chip must never cover typed words"
status: open
priority: 0
issue_type: bug
labels:
- vibe-talk
created_at: 2026-11-18T16:32:01.000000000-05:00
updated_at: 2026-11-18T16:32:01.000000000-05:00
---

# Description

In the #1840 screenshots (phone-light-a-draft-saved.png, dark-phone-kbopen.png) the "Save draft" chip stands over the channel box's top corner and hides the end of the first typed line ("Continuing the release notes fro…" under the chip). A control that covers the owner's own words while they write them is the defect: the chip must never intersect the box's text area.

# Design

- Reserve the chip's room in the box itself: `padding-inline-end` on the channel box wide enough for the chip and its inset, so typed text wraps before it reaches the chip at any width or type size. The actions row and the composer height do not change (the row attempt at 5620edf1 grew the composer and raised the floating pills).
- A browser assertion: the chip's rect never intersects the textarea's text area (its content box minus padding) at 412x915 and 1280x800, with a long first line typed.

# Acceptance Criteria

- The assertion passes at both sizes with a long first line; the typed line is fully readable beside the chip.
- The existing composer, keyboard and offline browser walks still pass; the composer keeps its height.

# Notes

Fast follow to #1840 vibe-talk-draft-sync-across-devices (landed e166ec38, deployed), from the coordinator's review of the lane's own screenshots. Owner-priority, path-selected gate.
