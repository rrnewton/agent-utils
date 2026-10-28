---
title: 'top-level-views-equal: Main, All, Threads and Two-column are equal top-level views; Esc never switches views'
status: open
priority: 2
issue_type: task
labels:
- vibe-talk
depends_on:
  agent-utils-624: blocks
created_at: 2026-10-28T15:13:15.881850231+00:00
updated_at: 2026-10-28T17:38:43.639036754+00:00
---

# Description

[opus 5.5] Owner 2026-10-28 11:03 ET. Esc keeps exiting search and thread drill-in (back to the parent view) but must NOT switch from the Threads list to All. Remove the Two-column toggle that returns to the view you came from; Two-column becomes its own top-level view with its own shortcut (e.g. Ctrl/Cmd+4; check the #281 key map); leaving it is picking another view. All stays the default when arriving from elsewhere. Page change: lands before #624 voice-page-modularity starts (it blocks #624).
