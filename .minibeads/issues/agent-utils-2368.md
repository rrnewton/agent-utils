---
title: 'vibe-talk-newest-chip-phone-inset: the Newest jump chip stands 12 px from the left edge at 375x812, inside the 16 px phone margin'
status: open
priority: 2
issue_type: bug
labels:
- vibe-talk
- owner-ui
created_at: 2026-11-27T05:57:00-05:00
updated_at: 2026-11-27T05:57:00-05:00
---

# Description

Found by the advisory design review of #2351 vibe-talk-read-strip-foot-reader-moves (R10, phone side margin). At 375x812 (DPR 1), the "Newest" jump chip stands at x=12-99, y=427-459 in the read-aloud walk's paused and stopped captures, dark and light: a 12 px left inset, where R10 asks for at least 16 px on a phone. The cards in the same frames stand at 24 px. The chip's placement is not new; #2351 is only the first change whose captures show it, because the reader now stays above the list's end while a read plays.

# Design

- Inset the jump chip to the phone side margin (16 px), or align it to the cards' left edge (24 px), at every phone width. A layout change only; the chip's behaviour stays as it is.

# Acceptance Criteria

- Shots at 375x812 and 393x852, light and dark, with the chip up (a reader above the list's end): its left edge at the chosen inset, measured, with BUILD.txt.
- The advisory design review finds no R10 violation for the chip.

# Notes

Filed at the coordinator's request from #2351's design review (issue file only); the crop is in that review. Related: #2351, #1057 newest-count-chip, R10.
