---
title: 'scrubber-thumb-arrows: section and chunk arrows on the right of the read-aloud strip'
status: closed
priority: 2
issue_type: feature
labels:
- vibe-talk
created_at: 2026-10-28T13:17:04.273297697+00:00
updated_at: 2026-10-28T15:52:43.928299202+00:00
closed_at: 2026-10-28T15:52:43.928299031+00:00
---

# Description

Owner, 2026-10-28 09:12 ET: move the read-aloud strip's Previous/Next to the right side (thumb reach), and add section skips. Both pairs are arrows: fine thin arrows for the small jumps among chunks (read-aloud blocks/pieces), big fat arrows for the jumps between chapters/sections. Correction 09:16 ET: all four arrows point left/right, matching the progress bar's left-to-right idiom. Layout, right side, left to right: [fat prev][fat next]  k of n  [thin prev][thin next]. The Media Session previous/next keep stepping chunks. Related: #376 read-aloud-blocks, #573 scrubber-full-width, #577 read-chunk-balance.

# Acceptance Criteria

Four arrow buttons, 44px targets, accessible labels; section jumps defined and tested at both boundaries; phone layout puts the group at the right edge; page tests plus Chromium at 393x852 and a desk.
