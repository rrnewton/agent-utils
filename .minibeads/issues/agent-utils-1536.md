---
title: 'discuss-sparkle-thread-spool-icons: sparkle for Discuss and a spool for the thread picker'
status: closed
priority: 0
issue_type: feature
labels:
- vibe-talk
created_at: 2026-11-14T16:14:54.000000000+00:00
updated_at: 2026-11-14T16:14:54.000000000+00:00
closed_at: 2026-11-14T16:14:54.000000000+00:00
---

# Description

[watermelon-full-preview] Owner asks on 2026-11-13: the Discuss button's icon read too much like the thread list icon, so the two were hard to tell apart at a glance. The owner picked a sparkle for Discuss and a spool of thread for the thread picker.

# Design

- Discuss (both the bar button and the menu entry) draws a four-point sparkle with no bubbles, set-matched in stroke and size and following `currentColor` in light and dark.
- The thread-list picker draws an own-drawn spool of thread with a loose end in the bar's line style, with dark and light variants.
- The author-filter bubbles are untouched; only the Discuss and thread-picker glyphs change.

# Acceptance Criteria

- Discuss and the thread picker show distinct glyphs (sparkle and spool) on the bar and in the menu at phone and desk widths.
- Both glyphs remain legible in light and dark.

# Notes

Owner asks of 2026-11-13 (audit rows 51-52). Number assigned for this reconcile; no tracker issue existed before. Lands as two small commits completing one pair.

## Resolution

[watermelon-full-preview] Landed in 344d064b92fe2af857e55fb5cdacc12d77a61f70 (sparkle for Discuss) and 8920426b66239aa0496373a2188c61e194c4f787 (spool for the thread picker) on main and live in release ba329b5a. Verified as ancestors of the live release.
