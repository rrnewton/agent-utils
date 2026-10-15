---
title: 'done-stale-across-views: a Done made in one view shows as not done in another view loaded earlier'
status: closed
priority: 2
issue_type: bug
labels:
- vibe-talk
created_at: 2026-10-14T19:25:04.974901824+00:00
updated_at: 2026-10-15T08:33:57.463478432+00:00
closed_at: 2026-10-15T08:33:57.463478262+00:00
---

# Description

Found in review of #295 shift-e-read-collapse (2026-10-14), reproducible with e on main's page: mark a message Done in Main; if All was loaded earlier in the session, the same message still shows as not Done in All for 3 s or more, until some later redraw. A Done (and Undo) should update every held view's copy of that message at once (Main, All, threads, the right column, cards' unread counts), without a read.
