---
title: 'mark-read-on-reply-settings: Mark read on reply with quoted-reply guards'
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

[watermelon-full-preview] Owner asks on 2026-11-13: replying to a message should be able to mark what it answers as read, under the owner's control. A Settings subsection "Mark read on reply" with a No/Yes switch and sub-options, a quoted-reply group, and adjustable numbers; "Not in Fold read" and the non-quote choice default on, the under-5% size guard defaults off, "Before final paragraph" defaults on, and the first-50% / over-0-words numbers are adjustable.

# Design

- A No/Yes switch (default Yes) reveals: "Not in Fold read", "Mark non-quote replies as read", and a labelled Quoted replies group evaluated by block position (size under 5% default off, before the final block on, first 50% of over-0-words on; every number adjustable and synced as a setting).
- Quote position counts rendered blocks, so the guards judge the message as drawn.
- The choices are device settings with `onUnlessOff` defaults where the owner asked for default-on.

# Acceptance Criteria

- With the switch on, replying marks the answered message read exactly when the guards allow it (fold state, quote vs non-quote, size, final-block and first-half rules); with it off, replying marks nothing.
- The defaults match the owner's: Not in Fold read on, non-quote on, under-5% off, before-final on, 50% / 0 words.
- The numbers are adjustable and persist.

# Notes

Owner asks of 2026-11-13 (audit rows 53-58). Number assigned for this reconcile; no tracker issue existed before.

## Resolution

[watermelon-full-preview] Landed in fb293e5872e69b0d0a6026c07b2f0f368f6d8e14 on main and live in release ba329b5a. The Mark read on reply switch, its sub-options and the quoted-reply position guards are in the shipped settings with the owner's defaults. Verified as an ancestor of the live release.
