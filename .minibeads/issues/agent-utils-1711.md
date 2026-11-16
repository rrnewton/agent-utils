---
title: 'vibe-talk-floating-chip-overlap: floating chips and toasts must not cover card controls on the mobile page'
status: open
priority: 1
issue_type: bug
labels:
- vibe-talk
- ui
created_at: 2026-11-16T12:39:59.452163968+00:00
updated_at: 2026-11-16T12:39:59.452163968+00:00
---

# Description

Owner report 2026-11-16 7:36 AM ET (Android phone, mobile vibe-talk page): At the top, the 'New version - reload' chip covers the 'Live - updated 07:33' chip and the first card's Thread(6) chip. At the bottom, the 'Agent voice on.' toast covers the last card's reaction chips. Floating chips and toasts must never cover each other or a card's controls: lay the top status chips out in one row that wraps or stacks without overlap, and keep toasts clear of interactive content (reserve space or offset them). Sibling lane fixes the reader-bar sound/voice-selector overlap in the same screenshot; do not touch that area. No new features.

# Acceptance Criteria

Phone viewport about 412x915 CSS px (device scale 2.6): bounding boxes of top status chips and first card Thread chip do not intersect and each is hittable via elementFromPoint at its center; toast does not intersect last card reaction chips and they remain hittable; desktop layout unchanged; browser test uses condition-based waits only.
